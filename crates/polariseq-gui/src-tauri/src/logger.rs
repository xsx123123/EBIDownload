//! Tracing subscriber layer that forwards log messages to the Tauri frontend
//! and optionally writes them to a file in the current output directory.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tracing::{Event, Subscriber};
use tracing_subscriber::filter::LevelFilter;
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
static LOG_FILE: OnceLock<Arc<Mutex<Option<File>>>> = OnceLock::new();

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

        // Forward to frontend.
        if let Some(tx) = LOG_SENDER.get() {
            let _ = tx.send(LogEntry {
                level: level.clone(),
                message: visitor.message.clone(),
            });
        }

        // Also append to the configured log file, if any.
        if let Some(log_file_holder) = LOG_FILE.get() {
            if let Ok(mut guard) = log_file_holder.lock() {
                if let Some(file) = guard.as_mut() {
                    let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
                    let line = format!(
                        "[{}] [{}] {}\n",
                        timestamp,
                        level.to_uppercase(),
                        visitor.message
                    );
                    let _ = file.write_all(line.as_bytes());
                    let _ = file.flush();
                }
            }
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

    // Initialize the file holder so it can be configured later.
    let _ = LOG_FILE.set(Arc::new(Mutex::new(None)));

    let subscriber = tracing_subscriber::registry()
        .with(TauriLogLayer)
        .with(LevelFilter::INFO);

    tracing::subscriber::set_global_default(subscriber)
        .map_err(|e| anyhow::anyhow!("Failed to set tracing subscriber: {}", e))?;

    Ok(rx)
}

/// Configure the subscriber to also append log entries to `path`.
/// Creates the parent directory and the file if necessary.
pub fn set_log_file<P: AsRef<Path>>(path: P) -> anyhow::Result<()> {
    let path = path.as_ref();
    if let Some(parent) = path.parent() {
        if !parent.exists() {
            std::fs::create_dir_all(parent)
                .map_err(|e| anyhow::anyhow!("Failed to create log directory: {}", e))?;
        }
    }

    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| anyhow::anyhow!("Failed to open log file: {}", e))?;

    let holder = LOG_FILE.get_or_init(|| Arc::new(Mutex::new(None)));
    *holder.lock().unwrap() = Some(file);
    Ok(())
}

/// Stop writing logs to the currently configured file.
pub fn clear_log_file() {
    if let Some(holder) = LOG_FILE.get() {
        *holder.lock().unwrap() = None;
    }
}
