//! Streaming full-scan DataSource for JSONL files.
//!
//! Reads the file as a byte stream, splits it into lines incrementally, and
//! emits bounded [`RecordBatch`]es — the file is never fully materialized in
//! memory (ADR-003). Values are stringified to all-Utf8 columns, matching the
//! schema produced by the JSONL reader's schema inference.

use arrow::array::StringBuilder;
use arrow::datatypes::SchemaRef;
use arrow::record_batch::{RecordBatch, RecordBatchOptions};
use bundlebase_io::plugin::object_store::ObjectStoreFile;
use bundlebase_io::IOReadFile;
use datafusion::common::{project_schema, DataFusionError, Statistics};
use datafusion::datasource::source::DataSource;
use datafusion::execution::TaskContext;
use datafusion::physical_expr::expressions::Column;
use datafusion::physical_expr::projection::ProjectionExprs;
use datafusion::physical_expr::EquivalenceProperties;
use datafusion::physical_plan::stream::RecordBatchStreamAdapter;
use datafusion::physical_plan::{DisplayFormatType, Partitioning, SendableRecordBatchStream};
use futures::stream::BoxStream;
use futures::StreamExt;
use object_store::{GetOptions, ObjectStore};
use std::any::Any;
use std::collections::HashMap;
use std::fmt::{Debug, Display, Formatter};
use std::sync::Arc;

/// Rows per emitted RecordBatch.
const BATCH_ROWS: usize = 8192;

/// Streaming full-scan source for JSON Lines files (bb-xn0.2).
pub struct StreamingJsonlDataSource {
    /// The source file
    file: ObjectStoreFile,
    /// Schema of the data (original full schema)
    schema: SchemaRef,
    /// Schema after projection is applied (computed at construction time)
    projected_schema: SchemaRef,
    /// Optional column projection (indices of columns to read)
    projection: Option<Vec<usize>>,
    /// Object store for reading file data
    object_store: Arc<dyn ObjectStore>,
    /// Optional row fetch limit pushed down by the planner.
    fetch: Option<usize>,
}

impl StreamingJsonlDataSource {
    pub fn new(
        file: &ObjectStoreFile,
        schema: SchemaRef,
        projection: Option<Vec<usize>>,
    ) -> datafusion::common::Result<Self> {
        let projected_schema = project_schema(&schema, projection.as_ref())?;
        let object_store = file.store();
        Ok(Self {
            file: file.clone(),
            schema,
            projected_schema,
            projection,
            object_store,
            fetch: None,
        })
    }

    fn with_projection_and_fetch(
        &self,
        projection: Option<Vec<usize>>,
        fetch: Option<usize>,
    ) -> datafusion::common::Result<Self> {
        let mut source = Self::new(&self.file, self.schema.clone(), projection)?;
        source.fetch = fetch;
        Ok(source)
    }

    fn current_projection_indices(&self) -> Vec<usize> {
        match &self.projection {
            Some(p) => p.clone(),
            None => (0..self.schema.fields().len()).collect(),
        }
    }

    /// Map a planner projection (over our current output columns) back to
    /// indices into the original schema. Returns `None` if any expression is
    /// not an alias-free column reference — an alias (e.g. `SELECT a AS x`)
    /// must keep its Projection node, otherwise the output loses the alias
    /// and duplicate source columns collapse in the by-name builders
    /// (matching DataFusion's `all_alias_free_columns` rule).
    fn remap_projection(&self, projection: &ProjectionExprs) -> Option<Vec<usize>> {
        let current_projection = self.current_projection_indices();
        projection
            .iter()
            .map(|expr| {
                let column = expr.expr.as_any().downcast_ref::<Column>()?;
                if column.name() != expr.alias {
                    return None;
                }
                current_projection.get(column.index()).copied()
            })
            .collect()
    }
}

/// Incremental line-to-batch converter: accumulates parsed rows and produces
/// a batch every [`BATCH_ROWS`] rows.
struct ScanState {
    object_store: Arc<dyn ObjectStore>,
    path: object_store::path::Path,
    /// Byte stream; opened lazily on first poll (DataSource::open is sync).
    byte_stream: Option<BoxStream<'static, object_store::Result<bytes::Bytes>>>,
    /// Unterminated tail bytes carried across chunks.
    pending: Vec<u8>,
    /// Projected column names (builder order).
    col_names: Vec<String>,
    builders: Vec<StringBuilder>,
    rows: usize,
    remaining_fetch: Option<usize>,
    output_schema: SchemaRef,
    done: bool,
}

/// Append one line's values to the builders. Returns `Ok(false)` for
/// blank/whitespace-only lines (not logical rows).
fn process_line(
    line: &[u8],
    name_to_idx: &HashMap<&str, usize>,
    builders: &mut [StringBuilder],
) -> datafusion::common::Result<bool> {
    let line = if line.last() == Some(&b'\r') {
        &line[..line.len() - 1]
    } else {
        line
    };
    if line.is_empty() || line.iter().all(|b| b.is_ascii_whitespace()) {
        // Whitespace-only lines are not logical rows (the layout's row
        // numbering skips them too).
        return Ok(false);
    }
    // A malformed line must be an error, not a silent skip: dropped lines
    // desync logical row numbers from the byte-offset layout, and every
    // downstream RowId (tombstones, overlays, indexes) would point at the
    // wrong row (bb-dvo.1).
    if !crate::jsonl_row::append_jsonl_row_to_builders(line, name_to_idx, builders) {
        return Err(DataFusionError::External(
            format!(
                "Malformed JSONL line (not a JSON object): {}",
                String::from_utf8_lossy(&line[..line.len().min(120)])
            )
            .into(),
        ));
    }
    Ok(true)
}

impl ScanState {
    /// Drain buffered complete lines, stopping at a full batch or the fetch
    /// limit — a single large chunk (e.g. the memory store returns the whole
    /// object at once) must still come out as bounded batches. Lines are
    /// scanned in place with a cursor and the buffer compacted ONCE at the
    /// end: per-line `Vec::drain` memmoves made this quadratic in chunk size.
    fn drain_complete_lines(&mut self) -> datafusion::common::Result<()> {
        let name_to_idx: HashMap<&str, usize> = self
            .col_names
            .iter()
            .enumerate()
            .map(|(i, n)| (n.as_str(), i))
            .collect();

        let mut consumed = 0usize;
        let result = loop {
            if self.rows >= BATCH_ROWS {
                break Ok(());
            }
            if matches!(self.remaining_fetch, Some(0)) {
                self.done = true;
                break Ok(());
            }
            let Some(nl_rel) = memchr::memchr(b'\n', &self.pending[consumed..]) else {
                break Ok(());
            };
            let line_end = consumed + nl_rel;
            match process_line(
                &self.pending[consumed..line_end],
                &name_to_idx,
                &mut self.builders,
            ) {
                Ok(true) => {
                    self.rows += 1;
                    if let Some(remaining) = &mut self.remaining_fetch {
                        *remaining -= 1;
                    }
                }
                Ok(false) => {}
                Err(e) => {
                    consumed = line_end + 1;
                    break Err(e);
                }
            }
            consumed = line_end + 1;
        };
        if consumed > 0 {
            self.pending.drain(..consumed);
        }
        result
    }

    /// Build a batch from the accumulated rows and reset the accumulators.
    fn take_batch(&mut self) -> datafusion::common::Result<RecordBatch> {
        let arrays: Vec<Arc<dyn arrow::array::Array>> = self
            .builders
            .iter_mut()
            .map(|b| Arc::new(b.finish()) as Arc<dyn arrow::array::Array>)
            .collect();
        let options = RecordBatchOptions::new().with_row_count(Some(self.rows));
        let batch = RecordBatch::try_new_with_options(self.output_schema.clone(), arrays, &options)
            .map_err(|e| DataFusionError::ArrowError(Box::new(e), None))?;
        self.rows = 0;
        Ok(batch)
    }
}

/// Advance the scan: pull chunks until a full batch is ready, the stream
/// ends, or an error surfaces.
async fn next_batch(
    mut state: ScanState,
) -> Option<(datafusion::common::Result<RecordBatch>, ScanState)> {
    loop {
        // Drain buffered complete lines first — a full batch may already be
        // sitting in `pending` without any further IO.
        if !state.done {
            if let Err(e) = state.drain_complete_lines() {
                state.done = true;
                return Some((Err(e), state));
            }
        }
        if state.rows >= BATCH_ROWS {
            let batch = state.take_batch();
            return Some((batch, state));
        }
        if state.done {
            if state.rows > 0 {
                let batch = state.take_batch();
                return Some((batch, state));
            }
            return None;
        }

        if state.byte_stream.is_none() {
            let opened = state
                .object_store
                .get_opts(&state.path, GetOptions::default())
                .await;
            match opened {
                Ok(result) => state.byte_stream = Some(result.into_stream()),
                Err(e) => {
                    state.done = true;
                    return Some((Err(DataFusionError::External(Box::new(e))), state));
                }
            }
        }

        let Some(byte_stream) = state.byte_stream.as_mut() else {
            // Unreachable: initialized above; treat defensively as EOF.
            state.done = true;
            continue;
        };
        let next_chunk = byte_stream.next().await;
        match next_chunk {
            Some(Ok(chunk)) => {
                // Draining and batch emission happen at the top of the loop.
                state.pending.extend_from_slice(&chunk);
            }
            Some(Err(e)) => {
                state.done = true;
                return Some((Err(DataFusionError::External(Box::new(e))), state));
            }
            None => {
                state.done = true;
                // EOF terminates any final unterminated line.
                if !state.pending.is_empty() && !matches!(state.remaining_fetch, Some(0)) {
                    let line: Vec<u8> = std::mem::take(&mut state.pending);
                    let name_to_idx: HashMap<&str, usize> = state
                        .col_names
                        .iter()
                        .enumerate()
                        .map(|(i, n)| (n.as_str(), i))
                        .collect();
                    match process_line(&line, &name_to_idx, &mut state.builders) {
                        Ok(true) => state.rows += 1,
                        Ok(false) => {}
                        Err(e) => return Some((Err(e), state)),
                    }
                }
            }
        }
    }
}

impl DataSource for StreamingJsonlDataSource {
    fn open(
        &self,
        _partition: usize,
        _context: Arc<TaskContext>,
    ) -> datafusion::common::Result<SendableRecordBatchStream> {
        let output_schema = self.projected_schema.clone();
        let col_names: Vec<String> = output_schema
            .fields()
            .iter()
            .map(|f| f.name().clone())
            .collect();
        let builders = (0..col_names.len()).map(|_| StringBuilder::new()).collect();

        log::debug!(
            "StreamingJsonlDataSource: full scan of {} ({} projected columns)",
            self.file.url(),
            col_names.len()
        );

        let state = ScanState {
            object_store: self.object_store.clone(),
            path: self.file.store_path().clone(),
            byte_stream: None,
            pending: Vec::new(),
            col_names,
            builders,
            rows: 0,
            remaining_fetch: self.fetch,
            output_schema: output_schema.clone(),
            done: false,
        };

        let stream = futures::stream::unfold(state, next_batch);
        Ok(Box::pin(RecordBatchStreamAdapter::new(
            output_schema,
            stream,
        )))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn fmt_as(&self, _t: DisplayFormatType, f: &mut Formatter) -> std::fmt::Result {
        write!(f, "{}", self)
    }

    fn output_partitioning(&self) -> Partitioning {
        Partitioning::UnknownPartitioning(1)
    }

    fn eq_properties(&self) -> EquivalenceProperties {
        EquivalenceProperties::new(self.projected_schema.clone())
    }

    fn partition_statistics(
        &self,
        _partition: Option<usize>,
    ) -> datafusion::common::Result<Statistics> {
        Ok(Statistics::new_unknown(&self.projected_schema))
    }

    fn with_fetch(&self, limit: Option<usize>) -> Option<Arc<dyn DataSource>> {
        self.with_projection_and_fetch(self.projection.clone(), limit)
            .ok()
            .map(|s| Arc::new(s) as Arc<dyn DataSource>)
    }

    fn fetch(&self) -> Option<usize> {
        self.fetch
    }

    fn try_swapping_with_projection(
        &self,
        projection: &ProjectionExprs,
    ) -> datafusion::common::Result<Option<Arc<dyn DataSource>>> {
        let Some(remapped_projection) = self.remap_projection(projection) else {
            return Ok(None);
        };
        Ok(Some(Arc::new(self.with_projection_and_fetch(
            Some(remapped_projection),
            self.fetch,
        )?)))
    }
}

impl Debug for StreamingJsonlDataSource {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StreamingJsonlDataSource")
            .field("file", &self.file)
            .field("projection", &self.projection)
            .field("fetch", &self.fetch)
            .finish()
    }
}

impl Display for StreamingJsonlDataSource {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "StreamingJsonlDataSource({}, projection={:?})",
            self.file.url(),
            self.projection
        )
    }
}
