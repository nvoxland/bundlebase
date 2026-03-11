use std::collections::HashMap;
use std::io::{BufRead, BufReader, Cursor, Write};
use std::time::Instant;

use arrow::datatypes::{Field, Schema};
use arrow::ipc::reader::StreamReader;
use arrow::record_batch::RecordBatch;

use crate::function::{FunctionProvider, FunctionRef};
use crate::protocol::{write_arrow_ipc, write_error, write_response, JsonRpcRequest};

/// Run the function provider as a JSON-RPC subprocess on stdin/stdout.
///
/// If the first command-line argument is `--bundlebase-functions`,
/// prints the function manifest as JSON and exits.
pub fn serve_functions(provider: &dyn FunctionProvider) {
    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 && args[1] == "--bundlebase-functions" {
        let manifest = provider.metadata();
        match serde_json::to_string(&manifest) {
            Ok(json) => {
                println!("{}", json);
            }
            Err(e) => {
                eprintln!("Failed to serialize manifest: {}", e);
                std::process::exit(1);
            }
        }
        return;
    }

    let stdin = std::io::stdin().lock();
    let stdout = std::io::stdout().lock();
    serve_functions_io(
        provider,
        &mut BufReader::new(stdin),
        &mut std::io::BufWriter::new(stdout),
    );
}

/// Run the function provider on the given reader/writer (for testing).
pub fn serve_functions_io(provider: &dyn FunctionProvider, r: &mut dyn BufRead, w: &mut dyn Write) {
    let mut states: HashMap<String, Box<dyn std::any::Any + Send>> = HashMap::new();
    let mut state_last_access: HashMap<String, Instant> = HashMap::new();
    let mut next_state_id: u64 = 1;
    let mut line = String::new();
    let mut last_cleanup = Instant::now();
    let ttl = std::time::Duration::from_secs(300);
    let cleanup_interval = std::time::Duration::from_secs(60);

    loop {
        // Periodically clean up expired aggregate state
        let now = Instant::now();
        if now.duration_since(last_cleanup) >= cleanup_interval {
            state_last_access.retain(|id, created| {
                if now.duration_since(*created) > ttl {
                    states.remove(id);
                    false
                } else {
                    true
                }
            });
            last_cleanup = now;
        }

        line.clear();
        match r.read_line(&mut line) {
            Ok(0) => return, // EOF
            Ok(_) => {}
            Err(_) => return,
        }

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let req: JsonRpcRequest = match serde_json::from_str(trimmed) {
            Ok(r) => r,
            Err(e) => {
                let _ = write_error(w, &serde_json::Value::Null, -32700, &format!("Parse error: {}", e));
                let _ = w.flush();
                continue;
            }
        };

        let should_stop =
            handle_request(provider, &req, r, w, &mut states, &mut state_last_access, &mut next_state_id);
        let _ = w.flush();
        if should_stop {
            return;
        }
    }
}

fn handle_request(
    provider: &dyn FunctionProvider,
    req: &JsonRpcRequest,
    r: &mut dyn BufRead,
    w: &mut dyn Write,
    states: &mut HashMap<String, Box<dyn std::any::Any + Send>>,
    state_last_access: &mut HashMap<String, Instant>,
    next_state_id: &mut u64,
) -> bool {
    match req.method.as_str() {
        "handshake" => {
            let _ = write_response(w, &req.id, serde_json::json!({"protocol_version": "1"}));
        }
        "ping" => {
            let _ = write_response(w, &req.id, serde_json::json!("pong"));
        }
        "manifest" => handle_manifest(provider, req, w),
        "invoke" => handle_invoke(provider, req, r, w),
        "create_state" => handle_create_state(provider, req, w, states, state_last_access, next_state_id),
        "accumulate" => handle_accumulate(provider, req, r, w, states, state_last_access),
        "merge" => handle_merge(provider, req, w, states, state_last_access),
        "evaluate" => handle_evaluate(provider, req, w, states, state_last_access),
        "shutdown" => {
            let _ = write_response(w, &req.id, serde_json::json!({"ok": true}));
            return true;
        }
        _ => {
            let msg = format!("Method not found: {}", req.method);
            let _ = write_error(w, &req.id, -32601, &msg);
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Arrow IPC reading helpers
// ---------------------------------------------------------------------------

/// Read a length-prefixed Arrow IPC frame from the reader.
/// Returns the column arrays from the first batch.
fn read_arrow_ipc_columns(
    r: &mut dyn BufRead,
) -> Result<Vec<arrow::array::ArrayRef>, String> {
    let mut len_buf = [0u8; 4];
    r.read_exact(&mut len_buf)
        .map_err(|e| format!("Failed to read Arrow IPC length prefix: {}", e))?;
    let data_len = u32::from_be_bytes(len_buf) as usize;

    if data_len == 0 {
        return Ok(Vec::new());
    }

    let mut data = vec![0u8; data_len];
    r.read_exact(&mut data)
        .map_err(|e| format!("Failed to read Arrow IPC data ({} bytes): {}", data_len, e))?;

    let reader = StreamReader::try_new(Cursor::new(data), None)
        .map_err(|e| format!("Failed to create Arrow IPC reader: {}", e))?;

    let batches: Vec<RecordBatch> = reader
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("Failed to read Arrow IPC batches: {}", e))?;

    if batches.is_empty() {
        return Ok(Vec::new());
    }

    Ok(batches[0]
        .columns()
        .iter()
        .map(std::sync::Arc::clone)
        .collect())
}

// ---------------------------------------------------------------------------
// Method handlers
// ---------------------------------------------------------------------------

fn handle_manifest(provider: &dyn FunctionProvider, req: &JsonRpcRequest, w: &mut dyn Write) {
    let manifest = provider.metadata();
    match serde_json::to_value(&manifest) {
        Ok(val) => {
            let _ = write_response(w, &req.id, val);
        }
        Err(e) => {
            let _ = write_error(w, &req.id, -32000, &format!("Failed to serialize manifest: {}", e));
        }
    }
}

fn handle_invoke(
    provider: &dyn FunctionProvider,
    req: &JsonRpcRequest,
    r: &mut dyn BufRead,
    w: &mut dyn Write,
) {
    let function_name = req
        .params
        .get("function")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let func_ref = match provider.get_function(function_name) {
        Some(f) => f,
        None => {
            let _ = write_error(
                w,
                &req.id,
                -32000,
                &format!("Function not found: {}", function_name),
            );
            return;
        }
    };

    let scalar = match func_ref {
        FunctionRef::Scalar(s) => s,
        FunctionRef::Aggregate(_) => {
            let _ = write_error(
                w,
                &req.id,
                -32000,
                &format!("Function '{}' is aggregate, not scalar", function_name),
            );
            return;
        }
    };

    // Response is sent before reading/writing Arrow IPC (matching the core protocol)
    let _ = write_response(w, &req.id, serde_json::json!({"ok": true}));
    let _ = w.flush();

    // Read input Arrow IPC
    let args = match read_arrow_ipc_columns(r) {
        Ok(cols) => cols,
        Err(e) => {
            eprintln!("failed to read Arrow IPC input for invoke: {}", e);
            let _ = write_arrow_ipc(w, None);
            return;
        }
    };

    // Invoke the scalar function
    match scalar.invoke(&args) {
        Ok(result) => {
            let schema = std::sync::Arc::new(Schema::new(vec![Field::new(
                "result",
                result.data_type().clone(),
                true,
            )]));
            match RecordBatch::try_new(schema, vec![result]) {
                Ok(batch) => {
                    if let Err(e) = write_arrow_ipc(w, Some(&[batch])) {
                        eprintln!("failed to write Arrow IPC output for invoke: {}", e);
                    }
                }
                Err(e) => {
                    eprintln!("failed to create output RecordBatch for invoke: {}", e);
                    let _ = write_arrow_ipc(w, None);
                }
            }
        }
        Err(e) => {
            eprintln!("function '{}' invoke error: {}", function_name, e);
            let _ = write_arrow_ipc(w, None);
        }
    }
}

fn handle_create_state(
    provider: &dyn FunctionProvider,
    req: &JsonRpcRequest,
    w: &mut dyn Write,
    states: &mut HashMap<String, Box<dyn std::any::Any + Send>>,
    state_last_access: &mut HashMap<String, Instant>,
    next_state_id: &mut u64,
) {
    let function_name = req
        .params
        .get("function")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let func_ref = match provider.get_function(function_name) {
        Some(f) => f,
        None => {
            let _ = write_error(
                w,
                &req.id,
                -32000,
                &format!("Function not found: {}", function_name),
            );
            return;
        }
    };

    let agg = match func_ref {
        FunctionRef::Aggregate(a) => a,
        FunctionRef::Scalar(_) => {
            let _ = write_error(
                w,
                &req.id,
                -32000,
                &format!("Function '{}' is scalar, not aggregate", function_name),
            );
            return;
        }
    };

    match agg.create_state_dyn() {
        Ok(state) => {
            let state_id = format!("state_{}", *next_state_id);
            *next_state_id += 1;
            states.insert(state_id.clone(), state);
            state_last_access.insert(state_id.clone(), Instant::now());
            let _ = write_response(w, &req.id, serde_json::json!({"state_id": state_id}));
        }
        Err(e) => {
            let _ = write_error(
                w,
                &req.id,
                -32000,
                &format!("Failed to create state for '{}': {}", function_name, e),
            );
        }
    }
}

fn handle_accumulate(
    provider: &dyn FunctionProvider,
    req: &JsonRpcRequest,
    r: &mut dyn BufRead,
    w: &mut dyn Write,
    states: &mut HashMap<String, Box<dyn std::any::Any + Send>>,
    state_last_access: &mut HashMap<String, Instant>,
) {
    let function_name = req
        .params
        .get("function")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let state_id = req
        .params
        .get("state_id")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let func_ref = match provider.get_function(function_name) {
        Some(f) => f,
        None => {
            let _ = write_error(
                w,
                &req.id,
                -32000,
                &format!("Function not found: {}", function_name),
            );
            return;
        }
    };

    let agg = match func_ref {
        FunctionRef::Aggregate(a) => a,
        FunctionRef::Scalar(_) => {
            let _ = write_error(
                w,
                &req.id,
                -32000,
                &format!("Function '{}' is scalar, not aggregate", function_name),
            );
            return;
        }
    };

    // Send response before reading Arrow IPC (matching protocol)
    let _ = write_response(w, &req.id, serde_json::json!({"ok": true}));
    let _ = w.flush();

    // Read input Arrow IPC
    let args = match read_arrow_ipc_columns(r) {
        Ok(cols) => cols,
        Err(e) => {
            eprintln!("failed to read Arrow IPC input for accumulate: {}", e);
            return;
        }
    };

    // Update state
    let state = match states.get_mut(state_id) {
        Some(s) => s,
        None => {
            eprintln!("state '{}' not found for accumulate", state_id);
            return;
        }
    };

    // Update last-access time for TTL
    state_last_access.insert(state_id.to_string(), Instant::now());

    if let Err(e) = agg.accumulate_dyn(state, &args) {
        eprintln!(
            "function '{}' accumulate error for state '{}': {}",
            function_name, state_id, e
        );
    }
}

fn handle_merge(
    provider: &dyn FunctionProvider,
    req: &JsonRpcRequest,
    w: &mut dyn Write,
    states: &mut HashMap<String, Box<dyn std::any::Any + Send>>,
    state_last_access: &mut HashMap<String, Instant>,
) {
    let function_name = req
        .params
        .get("function")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let state_id1 = req
        .params
        .get("state_id1")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let state_id2 = req
        .params
        .get("state_id2")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let func_ref = match provider.get_function(function_name) {
        Some(f) => f,
        None => {
            let _ = write_error(
                w,
                &req.id,
                -32000,
                &format!("Function not found: {}", function_name),
            );
            return;
        }
    };

    let agg = match func_ref {
        FunctionRef::Aggregate(a) => a,
        FunctionRef::Scalar(_) => {
            let _ = write_error(
                w,
                &req.id,
                -32000,
                &format!("Function '{}' is scalar, not aggregate", function_name),
            );
            return;
        }
    };

    // Remove state_b first (needs owned value)
    let state_b = match states.remove(state_id2) {
        Some(s) => s,
        None => {
            let _ = write_error(
                w,
                &req.id,
                -32000,
                &format!("State '{}' not found", state_id2),
            );
            return;
        }
    };

    let state_a = match states.get_mut(state_id1) {
        Some(s) => s,
        None => {
            // Put state_b back since merge failed
            states.insert(state_id2.to_string(), state_b);
            let _ = write_error(
                w,
                &req.id,
                -32000,
                &format!("State '{}' not found", state_id1),
            );
            return;
        }
    };

    match agg.merge_dyn(state_a, state_b) {
        Ok(()) => {
            state_last_access.remove(state_id2);
            let _ = write_response(
                w,
                &req.id,
                serde_json::json!({"state_id": state_id1}),
            );
        }
        Err(e) => {
            let _ = write_error(
                w,
                &req.id,
                -32000,
                &format!("Merge failed for '{}': {}", function_name, e),
            );
        }
    }
}

fn handle_evaluate(
    provider: &dyn FunctionProvider,
    req: &JsonRpcRequest,
    w: &mut dyn Write,
    states: &mut HashMap<String, Box<dyn std::any::Any + Send>>,
    state_last_access: &mut HashMap<String, Instant>,
) {
    let function_name = req
        .params
        .get("function")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let state_id = req
        .params
        .get("state_id")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let func_ref = match provider.get_function(function_name) {
        Some(f) => f,
        None => {
            let _ = write_error(
                w,
                &req.id,
                -32000,
                &format!("Function not found: {}", function_name),
            );
            return;
        }
    };

    let agg = match func_ref {
        FunctionRef::Aggregate(a) => a,
        FunctionRef::Scalar(_) => {
            let _ = write_error(
                w,
                &req.id,
                -32000,
                &format!("Function '{}' is scalar, not aggregate", function_name),
            );
            return;
        }
    };

    let state = match states.get(state_id) {
        Some(s) => s,
        None => {
            let _ = write_error(
                w,
                &req.id,
                -32000,
                &format!("State '{}' not found", state_id),
            );
            return;
        }
    };

    // Update last-access time for TTL
    state_last_access.insert(state_id.to_string(), Instant::now());

    match agg.evaluate_dyn(state) {
        Ok(result) => {
            let _ = write_response(w, &req.id, serde_json::json!({"ok": true}));
            let _ = w.flush();

            let schema = std::sync::Arc::new(Schema::new(vec![Field::new(
                "result",
                result.data_type().clone(),
                true,
            )]));
            match RecordBatch::try_new(schema, vec![result]) {
                Ok(batch) => {
                    if let Err(e) = write_arrow_ipc(w, Some(&[batch])) {
                        eprintln!("failed to write Arrow IPC output for evaluate: {}", e);
                    }
                }
                Err(e) => {
                    eprintln!("failed to create output RecordBatch for evaluate: {}", e);
                    let _ = write_arrow_ipc(w, None);
                }
            }
        }
        Err(e) => {
            let _ = write_error(
                w,
                &req.id,
                -32000,
                &format!("Evaluate failed for '{}': {}", function_name, e),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::function::{
        AggregateFunction, FunctionManifest, FunctionMeta, FunctionProvider, FunctionRef,
        ScalarFunction,
    };
    use arrow::array::{ArrayRef, Float64Array, Int64Array};
    use arrow::datatypes::{DataType, Field, Schema};
    use arrow::error::ArrowError;
    use arrow::ipc::writer::StreamWriter;
    use arrow::record_batch::RecordBatch;
    use std::io::Cursor;
    use std::sync::Arc;

    // -- test scalar function --

    struct DoubleScalar;

    impl ScalarFunction for DoubleScalar {
        fn invoke(&self, args: &[ArrayRef]) -> Result<ArrayRef, ArrowError> {
            let input = args[0]
                .as_any()
                .downcast_ref::<Int64Array>()
                .ok_or_else(|| {
                    ArrowError::InvalidArgumentError("Expected Int64 input".to_string())
                })?;
            let result: Int64Array = input.iter().map(|v| v.map(|x| x * 2)).collect();
            Ok(Arc::new(result))
        }
    }

    // -- test aggregate function --

    struct SumAggregate;

    impl AggregateFunction for SumAggregate {
        type State = f64;

        fn create_state(&self) -> Result<f64, ArrowError> {
            Ok(0.0)
        }

        fn accumulate(&self, state: &mut f64, args: &[ArrayRef]) -> Result<(), ArrowError> {
            let input = args[0]
                .as_any()
                .downcast_ref::<Float64Array>()
                .ok_or_else(|| {
                    ArrowError::InvalidArgumentError("Expected Float64 input".to_string())
                })?;
            for v in input.iter().flatten() {
                *state += v;
            }
            Ok(())
        }

        fn merge(&self, state_a: &mut f64, state_b: f64) -> Result<(), ArrowError> {
            *state_a += state_b;
            Ok(())
        }

        fn evaluate(&self, state: &f64) -> Result<ArrayRef, ArrowError> {
            Ok(Arc::new(Float64Array::from(vec![*state])))
        }
    }

    // -- test provider --

    struct TestProvider;

    impl FunctionProvider for TestProvider {
        fn get_function(&self, name: &str) -> Option<FunctionRef<'_>> {
            match name {
                "double" => Some(FunctionRef::Scalar(&DoubleScalar)),
                "my_sum" => Some(FunctionRef::Aggregate(&SumAggregate)),
                _ => None,
            }
        }

        fn metadata(&self) -> FunctionManifest {
            FunctionManifest {
                functions: vec![
                    FunctionMeta {
                        name: "double".to_string(),
                        input_types: vec!["Int64".to_string()],
                        return_type: "Int64".to_string(),
                        kind: "scalar".to_string(),
                        symbol: None,
                    },
                    FunctionMeta {
                        name: "my_sum".to_string(),
                        input_types: vec!["Float64".to_string()],
                        return_type: "Float64".to_string(),
                        kind: "aggregate".to_string(),
                        symbol: None,
                    },
                ],
            }
        }
    }

    // -- helpers --

    fn make_request(method: &str, params: serde_json::Value, id: u64) -> String {
        let req = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        format!("{}\n", serde_json::to_string(&req).expect("serialize request"))
    }

    fn read_response(data: &[u8], offset: usize) -> (serde_json::Value, usize) {
        let remaining = &data[offset..];
        let end = remaining
            .iter()
            .position(|&b| b == b'\n')
            .expect("no newline found in response");
        let line = &remaining[..end];
        let resp: serde_json::Value =
            serde_json::from_slice(line).expect("failed to parse response");
        (resp, offset + end + 1)
    }

    fn make_arrow_ipc_frame(batch: &RecordBatch) -> Vec<u8> {
        let mut buf = Vec::new();
        {
            let mut writer =
                StreamWriter::try_new(&mut buf, &batch.schema()).expect("create writer");
            writer.write(batch).expect("write batch");
            writer.finish().expect("finish writer");
        }
        let len = buf.len() as u32;
        let mut frame = Vec::new();
        frame.extend_from_slice(&len.to_be_bytes());
        frame.extend_from_slice(&buf);
        frame
    }

    fn read_arrow_frame(data: &[u8], offset: usize) -> (i64, usize) {
        assert!(
            offset + 4 <= data.len(),
            "not enough data for length prefix"
        );
        let length =
            u32::from_be_bytes(data[offset..offset + 4].try_into().expect("4 bytes")) as usize;
        let offset = offset + 4;
        if length == 0 {
            return (0, offset);
        }

        let ipc_data = &data[offset..offset + length];
        let reader = arrow::ipc::reader::StreamReader::try_new(Cursor::new(ipc_data), None)
            .expect("failed to create Arrow reader");

        let mut total_rows: i64 = 0;
        for batch_result in reader {
            let batch = batch_result.expect("batch should be Ok");
            total_rows += batch.num_rows() as i64;
        }
        (total_rows, offset + length)
    }

    // -- tests --

    #[test]
    fn test_manifest() {
        let input = make_request("manifest", serde_json::json!({}), 1)
            + &make_request("shutdown", serde_json::json!({}), 2);

        let mut output = Vec::new();
        serve_functions_io(
            &TestProvider,
            &mut Cursor::new(input.as_bytes()),
            &mut output,
        );

        let (resp, _) = read_response(&output, 0);
        let functions = resp["result"]["functions"]
            .as_array()
            .expect("functions array");
        assert_eq!(functions.len(), 2);
        assert_eq!(functions[0]["name"], "double");
        assert_eq!(functions[0]["kind"], "scalar");
        assert_eq!(functions[1]["name"], "my_sum");
        assert_eq!(functions[1]["kind"], "aggregate");
    }

    #[test]
    fn test_invoke_scalar() {
        // Build Arrow IPC frame for input
        let schema = Arc::new(Schema::new(vec![Field::new("arg_0", DataType::Int64, true)]));
        let batch = RecordBatch::try_new(
            schema,
            vec![Arc::new(Int64Array::from(vec![1, 2, 3]))],
        )
        .expect("create batch");
        let ipc_frame = make_arrow_ipc_frame(&batch);

        let json_part = make_request(
            "invoke",
            serde_json::json!({"function": "double"}),
            1,
        );

        let mut input_bytes = json_part.into_bytes();
        // The IPC frame comes after the JSON-RPC response is sent back.
        // We append it to the input stream so the server can read it.
        input_bytes.extend_from_slice(&ipc_frame);
        input_bytes.extend_from_slice(
            make_request("shutdown", serde_json::json!({}), 2).as_bytes(),
        );

        let mut output = Vec::new();
        serve_functions_io(
            &TestProvider,
            &mut Cursor::new(&input_bytes),
            &mut output,
        );

        let (resp, offset) = read_response(&output, 0);
        assert_eq!(resp["result"]["ok"], true);

        // Read Arrow IPC output
        let (total_rows, _) = read_arrow_frame(&output, offset);
        assert_eq!(total_rows, 3);
    }

    #[test]
    fn test_aggregate_lifecycle() {
        // 1. Create state
        // 2. Accumulate a batch
        // 3. Evaluate
        let create_req = make_request(
            "create_state",
            serde_json::json!({"function": "my_sum"}),
            1,
        );

        let accumulate_req = make_request(
            "accumulate",
            serde_json::json!({"function": "my_sum", "state_id": "state_1"}),
            2,
        );

        // Build IPC frame for accumulate
        let schema = Arc::new(Schema::new(vec![Field::new(
            "val_0",
            DataType::Float64,
            true,
        )]));
        let batch = RecordBatch::try_new(
            schema,
            vec![Arc::new(Float64Array::from(vec![1.0, 2.0, 3.0]))],
        )
        .expect("create batch");
        let ipc_frame = make_arrow_ipc_frame(&batch);

        let evaluate_req = make_request(
            "evaluate",
            serde_json::json!({"function": "my_sum", "state_id": "state_1"}),
            3,
        );

        let shutdown_req = make_request("shutdown", serde_json::json!({}), 4);

        let mut input_bytes = Vec::new();
        input_bytes.extend_from_slice(create_req.as_bytes());
        input_bytes.extend_from_slice(accumulate_req.as_bytes());
        input_bytes.extend_from_slice(&ipc_frame);
        input_bytes.extend_from_slice(evaluate_req.as_bytes());
        input_bytes.extend_from_slice(shutdown_req.as_bytes());

        let mut output = Vec::new();
        serve_functions_io(
            &TestProvider,
            &mut Cursor::new(&input_bytes),
            &mut output,
        );

        // Response 1: create_state
        let (resp, offset) = read_response(&output, 0);
        assert_eq!(resp["result"]["state_id"], "state_1");

        // Response 2: accumulate
        let (resp, offset) = read_response(&output, offset);
        assert_eq!(resp["result"]["ok"], true);

        // Response 3: evaluate
        let (resp, offset) = read_response(&output, offset);
        assert_eq!(resp["result"]["ok"], true);

        // Read Arrow IPC output from evaluate
        assert!(
            offset + 4 <= output.len(),
            "not enough data for Arrow IPC frame"
        );
        let length =
            u32::from_be_bytes(output[offset..offset + 4].try_into().expect("4 bytes")) as usize;
        assert!(length > 0, "evaluate should return data");

        let ipc_data = &output[offset + 4..offset + 4 + length];
        let reader =
            arrow::ipc::reader::StreamReader::try_new(Cursor::new(ipc_data), None)
                .expect("create reader");

        let batches: Vec<RecordBatch> = reader
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .expect("read batches");

        assert_eq!(batches.len(), 1);
        let result = batches[0]
            .column(0)
            .as_any()
            .downcast_ref::<Float64Array>()
            .expect("Float64 result");
        assert_eq!(result.value(0), 6.0);
    }

    #[test]
    fn test_function_not_found() {
        let input = make_request(
            "invoke",
            serde_json::json!({"function": "nonexistent"}),
            1,
        ) + &make_request("shutdown", serde_json::json!({}), 2);

        let mut output = Vec::new();
        serve_functions_io(
            &TestProvider,
            &mut Cursor::new(input.as_bytes()),
            &mut output,
        );

        let (resp, _) = read_response(&output, 0);
        assert_eq!(resp["error"]["code"], -32000);
        assert!(resp["error"]["message"]
            .as_str()
            .expect("message")
            .contains("not found"));
    }

    #[test]
    fn test_unknown_method() {
        let input = make_request("bogus", serde_json::json!({}), 1)
            + &make_request("shutdown", serde_json::json!({}), 2);

        let mut output = Vec::new();
        serve_functions_io(
            &TestProvider,
            &mut Cursor::new(input.as_bytes()),
            &mut output,
        );

        let (resp, _) = read_response(&output, 0);
        assert_eq!(resp["error"]["code"], -32601);
    }
}
