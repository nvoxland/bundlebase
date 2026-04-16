//! MCP progress notification tracker.
//!
//! Bridges bundlebase's `ProgressTracker` interface to the MCP
//! `notifications/progress` protocol. When a client sends a `progressToken`
//! in a tool request's `_meta`, this tracker captures progress events from
//! operations (fetch, attach, etc.) and forwards them as MCP notifications.
//!
//! # Architecture
//!
//! The tracker holds an unbounded channel sender. Events are serializable and
//! sent synchronously (non-blocking) from the tracker's sync methods. A
//! background tokio task holds the receiver and the `Peer`, and sends MCP
//! notifications for each event. The tracker is installed as a task-local
//! (via `run_with_tracker`) so concurrent tool calls don't interfere.

use bundlebase_common::progress::{ProgressId, ProgressTracker};
use rmcp::model::{Notification, ProgressNotificationParam, ProgressToken, ServerNotification};
use rmcp::service::Peer;
use rmcp::RoleServer;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::mpsc::UnboundedSender;

/// Events emitted by the tracker and forwarded to the MCP client.
enum ProgressEvent {
    Start {
        id: ProgressId,
        operation: String,
        total: Option<u64>,
    },
    Update {
        id: ProgressId,
        current: u64,
        message: Option<String>,
    },
    Finish {
        id: ProgressId,
    },
}

/// `ProgressTracker` implementation that forwards events as MCP progress notifications.
pub struct McpProgressTracker {
    tx: UnboundedSender<ProgressEvent>,
}

impl ProgressTracker for McpProgressTracker {
    fn start(&self, operation: &str, total: Option<u64>) -> ProgressId {
        let id = ProgressId::new();
        let _ = self.tx.send(ProgressEvent::Start {
            id,
            operation: operation.to_string(),
            total,
        });
        id
    }

    fn update(&self, id: ProgressId, current: u64, message: Option<&str>) {
        let _ = self.tx.send(ProgressEvent::Update {
            id,
            current,
            message: message.map(String::from),
        });
    }

    fn finish(&self, id: ProgressId) {
        let _ = self.tx.send(ProgressEvent::Finish { id });
    }
}

/// Create an `McpProgressTracker` and spawn a background task that forwards
/// progress events to the MCP client as `notifications/progress` messages.
///
/// Returns an `Arc<McpProgressTracker>` ready to be installed via
/// `run_with_tracker`. The background task stops automatically when the
/// tracker is dropped (channel closes).
pub fn create_mcp_tracker(peer: Peer<RoleServer>, token: ProgressToken) -> Arc<McpProgressTracker> {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<ProgressEvent>();

    tokio::spawn(async move {
        // Track (operation_name, total) for each active operation.
        // operation_name is included in the Finish notification so clients know what completed.
        let mut ops: HashMap<u64, (String, Option<u64>)> = HashMap::new();

        while let Some(event) = rx.recv().await {
            let notification = match event {
                ProgressEvent::Start {
                    id,
                    operation,
                    total,
                } => {
                    ops.insert(id.0, (operation.clone(), total));
                    let mut param =
                        ProgressNotificationParam::new(token.clone(), 0.0).with_message(&operation);
                    if let Some(t) = total {
                        param = param.with_total(t as f64);
                    }
                    Notification::new(param)
                }
                ProgressEvent::Update {
                    id,
                    current,
                    message,
                } => {
                    let total = ops.get(&id.0).and_then(|(_, t)| *t);
                    let mut param = ProgressNotificationParam::new(token.clone(), current as f64);
                    if let Some(t) = total {
                        param = param.with_total(t as f64);
                    }
                    if let Some(msg) = message {
                        param = param.with_message(msg);
                    }
                    Notification::new(param)
                }
                ProgressEvent::Finish { id } => {
                    let (op, total) = ops.remove(&id.0).unwrap_or_default();
                    let progress = total.unwrap_or(1) as f64;
                    let mut param = ProgressNotificationParam::new(token.clone(), progress);
                    param = param
                        .with_total(progress)
                        .with_message(format!("Done: {}", op));
                    Notification::new(param)
                }
            };

            let server_notif = ServerNotification::from(notification);
            // Ignore send errors — client may have disconnected.
            let _ = peer.send_notification(server_notif).await;
        }
    });

    Arc::new(McpProgressTracker { tx })
}
