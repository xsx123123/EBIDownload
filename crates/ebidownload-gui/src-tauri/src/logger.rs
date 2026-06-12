//! Tracing subscriber layer that forwards log messages to the Tauri frontend.

use std::sync::OnceLock;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::{Context, Layer};
use tracing_subscriber::prelude::*;
use tracing_subscriber::registry::LookupSpan;

/// A single log entry forwarded to the frontend.
#[derive(Debug, Clone, serde::Serialize)]
pub struct LogEntry {
    pub level: String,
    pub message: String,
}

static LOG_SENDER: OnceLock<UnboundedSender<LogEntry>> = OnceLock::new();

/// Tracing layer that captures log events and sends them to the frontend.
pub struct TauriLogLayer;

impl<S> Layer<S> for TauriLogLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let level = event.metadata().level().to_string().to_lowercase();

        let mut visitor = MessageVisitor::default();
        event.record(&mut visitor);

        if visitor.message.is_empty() {
            return;
        }

        if let Some(tx) = LOG_SENDER.get() {
            let _ = tx.send(LogEntry {
                level,
                message: visitor.message,
            });
        }
    }
}

#[derive(Default)]
struct MessageVisitor {
    message: String,
}

impl tracing::field::Visit for MessageVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.message = format!("{:?}", value).trim_matches('"').to_string();
        } else if self.message.is_empty() {
            self.message = format!("{} = {:?}", field.name(), value);
        } else {
            self.message.push_str(&format!(", {} = {:?}", field.name(), value));
        }
    }
}

/// Initialize the tracing subscriber that forwards logs to the frontend.
/// Returns a receiver that must be drained by the Tauri application.
pub fn init_logging() -> anyhow::Result<UnboundedReceiver<LogEntry>> {
    let (tx, rx) = unbounded_channel();
    LOG_SENDER
        .set(tx)
        .map_err(|_| anyhow::anyhow!("Logging already initialized"))?;

    let subscriber = tracing_subscriber::registry().with(TauriLogLayer);

    tracing::subscriber::set_global_default(subscriber)
        .map_err(|e| anyhow::anyhow!("Failed to set tracing subscriber: {}", e))?;

    Ok(rx)
}
