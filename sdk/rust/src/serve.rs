use std::io::{BufRead, BufReader, Write};

use crate::protocol::{
    parse_location, parse_string_map, parse_string_slice, write_arrow_ipc, write_error,
    write_response, JsonRpcRequest,
};
use crate::source::Connector;

/// Run the connector as a JSON-RPC subprocess on stdin/stdout.
pub fn serve(source: &dyn Connector) {
    let stdin = std::io::stdin().lock();
    let stdout = std::io::stdout().lock();
    serve_io(source, &mut BufReader::new(stdin), &mut std::io::BufWriter::new(stdout));
}

/// Run the connector on the given reader/writer (for testing).
pub fn serve_io(source: &dyn Connector, r: &mut dyn BufRead, w: &mut dyn Write) {
    let mut line = String::new();
    loop {
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

        let should_stop = handle_request(source, &req, w);
        let _ = w.flush();
        if should_stop {
            return;
        }
    }
}

fn handle_request(source: &dyn Connector, req: &JsonRpcRequest, w: &mut dyn Write) -> bool {
    match req.method.as_str() {
        "handshake" => {
            let _ = write_response(w, &req.id, serde_json::json!({"protocol_version": "1"}));
        }
        "ping" => {
            let _ = write_response(w, &req.id, serde_json::json!("pong"));
        }
        "discover" => handle_discover(source, req, w),
        "data" => handle_data(source, req, w),
        "stable_url" => handle_stable_url(source, req, w),
        "shutdown" => {
            let _ = write_response(
                w,
                &req.id,
                serde_json::json!({"ok": true}),
            );
            return true;
        }
        _ => {
            let msg = format!("Method not found: {}", req.method);
            let _ = write_error(w, &req.id, -32601, &msg);
        }
    }
    false
}

fn handle_discover(source: &dyn Connector, req: &JsonRpcRequest, w: &mut dyn Write) {
    let attached = parse_string_slice(req.params.get("attached_locations"));
    let args = parse_string_map(&req.params, &["attached_locations"]);

    match source.discover(&attached, &args) {
        Ok(locations) => {
            let _ = write_response(
                w,
                &req.id,
                serde_json::json!({"locations": locations}),
            );
        }
        Err(e) => {
            let _ = write_error(w, &req.id, -32000, &e.to_string());
        }
    }
}

fn handle_data(source: &dyn Connector, req: &JsonRpcRequest, w: &mut dyn Write) {
    let location = parse_location(req.params.get("location"));
    let args = parse_string_map(&req.params, &["location"]);

    match source.data(&location, &args) {
        Ok(batches) => {
            // Buffer Arrow IPC first so we can send an error if serialization fails
            let mut buf = Vec::new();
            if let Err(e) = write_arrow_ipc(&mut buf, batches.as_deref()) {
                let msg = format!("failed to serialize Arrow IPC data: {e}");
                let _ = write_error(w, &req.id, -32000, &msg);
                return;
            }
            let _ = write_response(w, &req.id, serde_json::json!({"ok": true}));
            let _ = w.write_all(&buf);
        }
        Err(e) => {
            let _ = write_error(w, &req.id, -32000, &e.to_string());
        }
    }
}

fn handle_stable_url(source: &dyn Connector, req: &JsonRpcRequest, w: &mut dyn Write) {
    let location = parse_location(req.params.get("location"));
    let args = parse_string_map(&req.params, &["location"]);

    match source.stable_url(&location, &args) {
        Ok(Some(stable_url)) => {
            let _ = write_response(
                w,
                &req.id,
                serde_json::json!({"url": stable_url.url}),
            );
        }
        Ok(None) => {
            let _ = write_response(w, &req.id, serde_json::Value::Null);
        }
        Err(e) => {
            let _ = write_error(w, &req.id, -32000, &e.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::array::{Int64Array, StringArray};
    use arrow::datatypes::{DataType, Field, Schema};
    use arrow::record_batch::RecordBatch;
    use crate::types::Location;
    use std::collections::HashMap;
    use std::io::Cursor;
    use std::sync::Arc;

    // -- test sources --

    struct TestSource;

    impl Connector for TestSource {
        fn discover(
            &self,
            _attached: &[String],
            _args: &HashMap<String, String>,
        ) -> Result<Vec<Location>, Box<dyn std::error::Error>> {
            Ok(vec![
                Location {
                    location: "file1.parquet".into(),
                    must_copy: true,
                    format: "parquet".into(),
                    version: "v1".into(),
                },
                Location {
                    location: "file2.parquet".into(),
                    must_copy: true,
                    format: "parquet".into(),
                    version: "v1".into(),
                },
            ])
        }

        fn data(
            &self,
            location: &Location,
            _args: &HashMap<String, String>,
        ) -> Result<Option<Vec<RecordBatch>>, Box<dyn std::error::Error>> {
            let schema = Arc::new(Schema::new(vec![
                Field::new("id", DataType::Int64, false),
                Field::new("name", DataType::Utf8, false),
            ]));

            match location.location.as_str() {
                "file1.parquet" => {
                    let batch1 = RecordBatch::try_new(
                        schema.clone(),
                        vec![
                            Arc::new(Int64Array::from(vec![1, 2])),
                            Arc::new(StringArray::from(vec!["alice", "bob"])),
                        ],
                    )?;
                    let batch2 = RecordBatch::try_new(
                        schema,
                        vec![
                            Arc::new(Int64Array::from(vec![3])),
                            Arc::new(StringArray::from(vec!["charlie"])),
                        ],
                    )?;
                    Ok(Some(vec![batch1, batch2]))
                }
                "file2.parquet" => {
                    let batch = RecordBatch::try_new(
                        schema,
                        vec![
                            Arc::new(Int64Array::from(vec![4, 5])),
                            Arc::new(StringArray::from(vec!["dave", "eve"])),
                        ],
                    )?;
                    Ok(Some(vec![batch]))
                }
                _ => Ok(None),
            }
        }
    }

    struct ErrorSource;

    impl Connector for ErrorSource {
        fn discover(
            &self,
            _attached: &[String],
            _args: &HashMap<String, String>,
        ) -> Result<Vec<Location>, Box<dyn std::error::Error>> {
            Err("discover exploded".into())
        }

        fn data(
            &self,
            _location: &Location,
            _args: &HashMap<String, String>,
        ) -> Result<Option<Vec<RecordBatch>>, Box<dyn std::error::Error>> {
            Ok(None)
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
    fn test_discover() {
        let input = make_request(
            "discover",
            serde_json::json!({"attached_locations": []}),
            1,
        ) + &make_request("shutdown", serde_json::json!({}), 2);

        let mut output = Vec::new();
        serve_io(
            &TestSource,
            &mut Cursor::new(input.as_bytes()),
            &mut output,
        );

        let (resp, _) = read_response(&output, 0);
        let locations = resp["result"]["locations"]
            .as_array()
            .expect("locations array");
        assert_eq!(locations.len(), 2);
        assert_eq!(locations[0]["location"], "file1.parquet");
        assert_eq!(locations[0]["version"], "v1");
    }

    #[test]
    fn test_data_returns_arrow() {
        let input = make_request(
            "data",
            serde_json::json!({
                "location": {
                    "location": "file1.parquet",
                    "must_copy": true,
                    "format": "parquet",
                    "version": "v1",
                }
            }),
            1,
        ) + &make_request("shutdown", serde_json::json!({}), 2);

        let mut output = Vec::new();
        serve_io(
            &TestSource,
            &mut Cursor::new(input.as_bytes()),
            &mut output,
        );

        let (resp, offset) = read_response(&output, 0);
        assert_eq!(resp["result"]["ok"], true);

        let (total_rows, _) = read_arrow_frame(&output, offset);
        assert_eq!(total_rows, 3);
    }

    #[test]
    fn test_data_none() {
        let input = make_request(
            "data",
            serde_json::json!({"location": {"location": "nonexistent"}}),
            1,
        ) + &make_request("shutdown", serde_json::json!({}), 2);

        let mut output = Vec::new();
        serve_io(
            &TestSource,
            &mut Cursor::new(input.as_bytes()),
            &mut output,
        );

        let (_, offset) = read_response(&output, 0);

        // Should be zero-length frame
        let length =
            u32::from_be_bytes(output[offset..offset + 4].try_into().expect("4 bytes"));
        assert_eq!(length, 0);
    }

    #[test]
    fn test_unknown_method() {
        let input = make_request("bogus", serde_json::json!({}), 1)
            + &make_request("shutdown", serde_json::json!({}), 2);

        let mut output = Vec::new();
        serve_io(
            &TestSource,
            &mut Cursor::new(input.as_bytes()),
            &mut output,
        );

        let (resp, _) = read_response(&output, 0);
        assert_eq!(resp["error"]["code"], -32601);
    }

    #[test]
    fn test_user_error_wrapped() {
        let input = make_request(
            "discover",
            serde_json::json!({"attached_locations": []}),
            1,
        ) + &make_request("shutdown", serde_json::json!({}), 2);

        let mut output = Vec::new();
        serve_io(
            &ErrorSource,
            &mut Cursor::new(input.as_bytes()),
            &mut output,
        );

        let (resp, _) = read_response(&output, 0);
        assert_eq!(resp["error"]["code"], -32000);
        assert!(resp["error"]["message"]
            .as_str()
            .expect("message string")
            .contains("discover exploded"));
    }
}
