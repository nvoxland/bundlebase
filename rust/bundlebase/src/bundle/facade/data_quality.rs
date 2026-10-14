//! Single-scan data-quality profiling behind [`BundleFacade::data_quality`].

use super::BundleFacade;
use crate::bundle::bundle_schema::BundleSchema;
use crate::BundlebaseError;
use arrow::datatypes::DataType;
use arrow_schema::Schema;
use futures::StreamExt;
use serde::Serialize;

/// Per-column profile produced by [`BundleFacade::data_quality`].
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ColumnQuality {
    /// User-visible column name.
    pub name: String,
    pub non_null: i64,
    pub null_count: i64,
    /// `null_count / total_rows`, or `None` for an empty bundle.
    pub null_rate: Option<f64>,
    pub distinct: i64,
    /// Every row is null (and there is at least one row).
    pub all_null: bool,
    /// Exactly one distinct non-null value.
    pub constant: bool,
    /// For text columns: the SQL type every non-null value parses as
    /// (`BIGINT`, `DOUBLE` or `DATE`), suggesting a cast before numeric
    /// comparison or aggregation. Never suggested for zero-padded values
    /// ("007"), whose padding a numeric cast would destroy.
    pub suggested_cast: Option<String>,
}

/// Result of [`BundleFacade::data_quality`].
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DataQualityReport {
    pub total_rows: i64,
    pub columns: Vec<ColumnQuality>,
}

/// Where each column's aggregates land in the single-scan result row.
struct ColSlots {
    name: String,
    is_text: bool,
    non_null: usize,
    distinct: usize,
    as_int: Option<usize>,
    as_float: Option<usize>,
    as_date: Option<usize>,
    zero_padded: Option<usize>,
}

/// Every CSV/TSV/JSONL column arrives as Utf8, and a numeric comparison on
/// text compares lexicographically ("9" > "50") — silently wrong. Each text
/// column is probed with TRY_CAST over the same scan (so it costs nothing
/// extra); when *every* non-null value parses as one type a cast is suggested.
pub(super) async fn compute<F: BundleFacade + ?Sized>(
    facade: &F,
    schema: &Schema,
    bundle_schema: &BundleSchema,
) -> Result<DataQualityReport, BundlebaseError> {
    // Real columns are the ones the bundle schema knows; this skips the
    // empty-bundle sentinel field without excluding a real column that
    // happens to share its name.
    let cols: Vec<(String, bool)> = schema
        .fields()
        .iter()
        .filter(|f| bundle_schema.column_id(f.name()).is_some())
        .map(|f| {
            let is_text = matches!(
                f.data_type(),
                DataType::Utf8 | DataType::LargeUtf8 | DataType::Utf8View
            );
            (f.name().to_string(), is_text)
        })
        .collect();

    if cols.is_empty() {
        return Ok(DataQualityReport {
            total_rows: 0,
            columns: vec![],
        });
    }

    let q = |name: &str| format!("\"{}\"", name.replace('"', "\"\""));

    // DataFusion's expression naming is transparent to casts: COUNT(TRY_CAST(x AS t))
    // and COUNT(x) both auto-name to `count(x)`, so several probes over one column
    // collide as "duplicate unqualified field name". Project the casts into a
    // subquery as distinctly *aliased* columns and aggregate those instead.
    let mut inner: Vec<String> = Vec::new();
    let mut selects = vec!["COUNT(*) AS c_total".to_string()];
    let next = |selects: &mut Vec<String>, expr: String| -> usize {
        selects.push(expr);
        selects.len() - 1
    };

    let mut layout: Vec<ColSlots> = Vec::with_capacity(cols.len());
    for (i, (name, is_text)) in cols.iter().enumerate() {
        inner.push(format!("{} AS c{}", q(name), i));
        let non_null = next(&mut selects, format!("COUNT(c{}) AS c{}_nn", i, i));
        let distinct = next(&mut selects, format!("COUNT(DISTINCT c{}) AS c{}_dc", i, i));
        let (mut as_int, mut as_float, mut as_date, mut zero_padded) = (None, None, None, None);
        if *is_text {
            inner.push(format!("TRY_CAST({} AS BIGINT) AS c{}_i", q(name), i));
            inner.push(format!("TRY_CAST({} AS DOUBLE) AS c{}_f", q(name), i));
            inner.push(format!("TRY_CAST({} AS DATE) AS c{}_d", q(name), i));
            // '0' followed by a digit — "007", "01234". Not "0.5", a real number.
            inner.push(format!(
                "CASE WHEN regexp_like({}, '^0[0-9]') THEN 1 END AS c{}_z",
                q(name),
                i
            ));
            as_int = Some(next(&mut selects, format!("COUNT(c{}_i) AS c{}_ci", i, i)));
            as_float = Some(next(&mut selects, format!("COUNT(c{}_f) AS c{}_cf", i, i)));
            as_date = Some(next(&mut selects, format!("COUNT(c{}_d) AS c{}_cd", i, i)));
            zero_padded = Some(next(&mut selects, format!("COUNT(c{}_z) AS c{}_cz", i, i)));
        }
        layout.push(ColSlots {
            name: name.clone(),
            is_text: *is_text,
            non_null,
            distinct,
            as_int,
            as_float,
            as_date,
            zero_padded,
        });
    }
    let sql = format!(
        "SELECT {} FROM (SELECT {} FROM bundle)",
        selects.join(", "),
        inner.join(", ")
    );

    // A global aggregate yields exactly one row; stream until we see it.
    let mut stream = facade.query(&sql, vec![], None).await?;
    let mut batch = None;
    while let Some(b) = stream.next().await {
        let b = b?;
        if b.num_rows() > 0 {
            batch = Some(b);
            break;
        }
    }
    let batch = batch.ok_or("data-quality query returned no rows")?;

    let read_i64 = |col: usize| -> i64 {
        batch
            .column(col)
            .as_any()
            .downcast_ref::<arrow::array::Int64Array>()
            .map(|a| a.value(0))
            .unwrap_or(0)
    };
    let total = read_i64(0);

    let columns = layout
        .iter()
        .map(|c| {
            let non_null = read_i64(c.non_null);
            let distinct = read_i64(c.distinct);
            let null_count = total - non_null;
            let null_rate = if total > 0 {
                Some((null_count as f64) / (total as f64))
            } else {
                None
            };
            let suggested_cast = if c.is_text && non_null > 0 {
                let all = |slot: Option<usize>| slot.map(read_i64).unwrap_or(0) == non_null;
                let padded = c.zero_padded.map(read_i64).unwrap_or(0) > 0;
                if !padded && all(c.as_int) {
                    Some("BIGINT")
                } else if !padded && all(c.as_float) {
                    Some("DOUBLE")
                } else if all(c.as_date) {
                    Some("DATE")
                } else {
                    None
                }
            } else {
                None
            };
            ColumnQuality {
                name: c.name.clone(),
                non_null,
                null_count,
                null_rate,
                distinct,
                all_null: non_null == 0 && total > 0,
                constant: non_null > 0 && distinct == 1,
                suggested_cast: suggested_cast.map(str::to_string),
            }
        })
        .collect();

    Ok(DataQualityReport {
        total_rows: total,
        columns,
    })
}
