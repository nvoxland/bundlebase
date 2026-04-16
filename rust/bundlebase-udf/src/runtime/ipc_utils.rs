//! Shared IPC helper functions used by multiple runtime implementations.

use crate::bridge::ipc_bridge::{self, SubprocessCache};
use arrow::array::ArrayRef;
use arrow::datatypes::DataType;
use bundlebase_common::BundlebaseError;
use datafusion::common::Result as DFResult;
use datafusion::logical_expr::{Accumulator, ColumnarValue};
use std::sync::Arc;

/// Shared IPC scalar invocation for Ipc, Java, and Docker runtimes.
pub(crate) fn invoke_ipc_scalar_impl(
    name: &str,
    entrypoint: &str,
    args: &datafusion::logical_expr::ScalarFunctionArgs,
    subprocess_cache: &SubprocessCache,
) -> DFResult<ColumnarValue> {
    let arrays: Vec<ArrayRef> = args
        .args
        .iter()
        .map(|cv| match cv {
            ColumnarValue::Array(arr) => Ok(Arc::clone(arr)),
            ColumnarValue::Scalar(scalar) => scalar
                .to_array_of_size(args.number_rows)
                .map_err(|e| datafusion::common::DataFusionError::Execution(e.to_string())),
        })
        .collect::<DFResult<Vec<_>>>()?;

    // Extract function name from the call - use the name parameter which is the display name
    // For IPC, we need to extract the actual function name from the display name (namespace.name -> name)
    let func_name = name.rsplit('.').next().unwrap_or(name);

    let result = ipc_bridge::invoke_ipc_scalar(subprocess_cache, entrypoint, func_name, &arrays)
        .map_err(|e| {
            datafusion::common::DataFusionError::Execution(format!(
                "IPC function '{}' ({}) failed: {}",
                name, entrypoint, e
            ))
        })?;

    Ok(ColumnarValue::Array(result))
}

/// Shared IPC accumulator creation for Ipc, Java, and Docker runtimes.
pub(crate) fn create_ipc_accumulator(
    name: &str,
    entrypoint: &str,
    function_name: &str,
    return_type: &DataType,
    subprocess_cache: &SubprocessCache,
) -> DFResult<Box<dyn Accumulator>> {
    let state_id =
        ipc_bridge::ipc_aggregate_create_state(subprocess_cache, entrypoint, function_name)
            .map_err(|e| {
                datafusion::common::DataFusionError::Execution(format!(
                    "Failed to create IPC aggregate state for '{}': {}",
                    name, e
                ))
            })?;

    Ok(Box::new(crate::bridge::aggregate::IpcAccumulator {
        entrypoint: entrypoint.to_string(),
        function_name: function_name.to_string(),
        display_name: name.to_string(),
        state_id,
        return_type: return_type.clone(),
        subprocess_cache: Arc::clone(subprocess_cache),
    }))
}

/// Spawn an IPC subprocess, perform a handshake, then shut it down.
///
/// Used as a smoke test to verify a bundled connector binary is functional.
/// Accepts both success and `method_not_found` responses as valid — the point
/// is just to confirm the binary can be spawned and responds to JSON-RPC.
pub(crate) async fn verify_ipc_handshake(call_string: &str) -> Result<(), BundlebaseError> {
    let command = crate::bridge::ipc_bridge::parse_call(call_string)?;

    if command.is_empty() {
        return Err("Cannot verify connector with empty command".into());
    }

    let mut child = tokio::process::Command::new(&command[0])
        .args(&command[1..])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::inherit())
        .spawn()
        .map_err(|e| {
            BundlebaseError::from(format!(
                "Bundled connector verification failed: could not spawn '{}': {}",
                command[0], e
            ))
        })?;

    let stdin = child
        .stdin
        .take()
        .ok_or("Failed to capture stdin for connector verification")?;
    let stdout = child
        .stdout
        .take()
        .ok_or("Failed to capture stdout for connector verification")?;

    let mut writer = tokio::io::BufWriter::new(stdin);
    let mut reader = tokio::io::BufReader::new(stdout);

    // Send handshake request
    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "handshake",
        "params": {"protocol_version": "1"}
    });
    let mut line = serde_json::to_string(&request)
        .map_err(|e| format!("Failed to serialize handshake request: {}", e))?;
    line.push('\n');

    use tokio::io::AsyncBufReadExt;
    use tokio::io::AsyncWriteExt;

    writer
        .write_all(line.as_bytes())
        .await
        .map_err(|e| format!("Connector verification: failed to write handshake: {}", e))?;
    writer
        .flush()
        .await
        .map_err(|e| format!("Connector verification: failed to flush handshake: {}", e))?;

    // Read response (with timeout)
    let mut response_line = String::new();
    let read_result = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        reader.read_line(&mut response_line),
    )
    .await;

    // Send shutdown and kill regardless of response
    let shutdown = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "shutdown",
        "params": {}
    });
    let mut shutdown_line = serde_json::to_string(&shutdown).unwrap_or_default();
    shutdown_line.push('\n');
    let _ = writer.write_all(shutdown_line.as_bytes()).await;
    let _ = writer.flush().await;
    let _ = child.kill().await;

    // Now check the read result
    match read_result {
        Ok(Ok(0)) => {
            Err("Connector verification failed: subprocess closed stdout without responding".into())
        }
        Ok(Ok(_)) => {
            // Got a response — parse it to check for errors (but method_not_found is OK)
            if let Ok(resp) = serde_json::from_str::<serde_json::Value>(response_line.trim()) {
                if let Some(err) = resp.get("error") {
                    let code = err.get("code").and_then(|c| c.as_i64()).unwrap_or(0);
                    if code != -32601 {
                        // Not method_not_found — report the error
                        let message = err
                            .get("message")
                            .and_then(|m| m.as_str())
                            .unwrap_or("unknown error");
                        return Err(format!(
                            "Connector verification failed: handshake error (code {}): {}",
                            code, message
                        )
                        .into());
                    }
                }
            }
            Ok(())
        }
        Ok(Err(e)) => Err(format!(
            "Connector verification failed: error reading response: {}",
            e
        )
        .into()),
        Err(_) => Err(
            "Connector verification failed: subprocess did not respond within 10 seconds".into(),
        ),
    }
}
