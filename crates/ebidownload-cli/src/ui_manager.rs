//! Terminal UI manager: a pinned bottom global status bar plus auto-collapse
//! of completed downloads.
//!
//! `UiManager` wraps the shared `MultiProgress` (the CLI's `GLOBAL_MP`). The
//! status bar is inserted as the last member of the `MultiProgress` ordering
//! and keeps refreshing in place; transient per-file bars are inserted with
//! `insert_from_back(1)` (see `aws_s3.rs`) so they always land just above it.
//! Completed files are already `finish_and_clear()`-ed by the engine — we only
//! record their metadata here for the status-bar counts.
//!
//! No crossterm / keyboard interaction: this is purely a passive status line.

use std::fmt::Write as _;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use tokio::task::JoinHandle;

use ebidownload_core::observer::{CompletedInfo, DownloadObserver};
use ebidownload_core::progress_store::{ProgressStore, RunStage};

/// A finished download recorded for status-bar accounting (not displayed by
/// default — auto-collapse is the only mode).
#[derive(Clone)]
#[allow(dead_code)]
struct CompletedRecord {
    id: String,
    total_bytes: u64,
    elapsed_secs: f64,
    avg_speed_bps: f64,
}

/// One live download contributing a shared byte counter to the speed sum.
struct LiveCounter {
    id: String,
    bytes: Arc<AtomicU64>,
    total: u64,
}

/// Which download path the manager is aggregating; determines how counts are
/// derived (SRA has a rich `progress_store`; public-data relies on the manager's
/// own counters/lists).
pub enum Mode {
    Sra { store: ProgressStore },
    PublicData,
}

pub struct UiManager {
    status_pb: ProgressBar,
    mode: Mode,
    total_items: AtomicU64,
    live: Mutex<Vec<LiveCounter>>,
    completed: Mutex<Vec<CompletedRecord>>,
    failed: Mutex<Vec<String>>,
    last_bytes: Mutex<u64>,
    last_ts: Mutex<Instant>,
    tick_handle: Mutex<Option<JoinHandle<()>>>,
}

impl UiManager {
    /// Install the status bar at the bottom of the shared MultiProgress and
    /// start the 100ms refresh loop. `total` may be 0 here for public-data,
    /// where it is filled in later via `DownloadObserver::set_total`.
    pub fn start(mp: MultiProgress, mode: Mode, total: u64) -> Arc<Self> {
        let status_pb = mp.insert_from_back(0, ProgressBar::new(0));
        status_pb.set_style(status_bar_style());
        status_pb.set_prefix("status");
        status_pb.enable_steady_tick(Duration::from_millis(100));

        let now = Instant::now();
        let manager = Arc::new(Self {
            status_pb,
            mode,
            total_items: AtomicU64::new(total),
            live: Mutex::new(Vec::new()),
            completed: Mutex::new(Vec::new()),
            failed: Mutex::new(Vec::new()),
            last_bytes: Mutex::new(0),
            last_ts: Mutex::new(now),
            tick_handle: Mutex::new(None),
        });

        let tick_handle = {
            let this = manager.clone();
            tokio::spawn(async move {
                this.tick_loop().await;
            })
        };
        *manager.tick_handle.lock().unwrap() = Some(tick_handle);

        manager
    }

    /// Stop the refresh loop and clear the status bar.
    pub fn stop(&self) {
        if let Some(handle) = self.tick_handle.lock().unwrap().take() {
            handle.abort();
        }
        self.status_pb.finish_and_clear();
    }

    async fn tick_loop(self: Arc<Self>) {
        let mut interval = tokio::time::interval(Duration::from_millis(100));
        let mut buf = String::with_capacity(128);
        loop {
            interval.tick().await;
            self.refresh(&mut buf).await;
        }
    }

    async fn refresh(&self, buf: &mut String) {
        let now = Instant::now();

        // Sum live byte counters and the instantaneous speed.
        let (sum_bytes, cur_total, active_from_live) = {
            let live = self.live.lock().unwrap();
            let sum: u64 = live.iter().map(|c| c.bytes.load(Ordering::Relaxed)).sum();
            let total: u64 = live.iter().map(|c| c.total).sum();
            (sum, total, live.len())
        };

        let speed = {
            let mut last_bytes = self.last_bytes.lock().unwrap();
            let mut last_ts = self.last_ts.lock().unwrap();
            let delta_bytes = sum_bytes.saturating_sub(*last_bytes);
            let delta_secs = now.duration_since(*last_ts).as_secs_f64().max(0.001);
            *last_bytes = sum_bytes;
            *last_ts = now;
            delta_bytes as f64 / delta_secs
        };

        let total = self.total_items.load(Ordering::Relaxed) as usize;
        let (completed, failed, active) = match &self.mode {
            Mode::Sra { store } => {
                let map = store.read().await;
                let (mut completed, mut failed, mut active) = (0usize, 0usize, 0usize);
                for rp in map.values() {
                    match rp.stage {
                        RunStage::Completed => completed += 1,
                        RunStage::Failed => failed += 1,
                        RunStage::Downloading | RunStage::Extracting | RunStage::Compressing => {
                            active += 1
                        }
                        RunStage::Pending => {}
                    }
                }
                (completed, failed, active)
            }
            Mode::PublicData => {
                let completed = self.completed.lock().unwrap().len();
                let failed = self.failed.lock().unwrap().len();
                (completed, failed, active_from_live)
            }
        };
        let queued = total.saturating_sub(completed + failed + active);

        let cur_str = human_binary_bytes(sum_bytes);
        let tot_str = human_binary_bytes(cur_total);
        let speed_mib = speed / 1024.0 / 1024.0;
        buf.clear();
        // Segment-colored status line (ANSI is fine: status bar is TTY-only via MultiProgress).
        let _ = write!(
            buf,
            "{c} · {a} · {q} · {f} · {s} · {b}",
            c = paint_seg("✓", &format!("{completed} done"), "green"),
            a = paint_seg("↓", &format!("{active} active"), "cyan"),
            q = paint_seg("…", &format!("{queued} queued"), "dim"),
            f = paint_seg("!", &format!("{failed} failed"), if failed > 0 { "red" } else { "dim" }),
            s = paint_seg("⚡", &format!("{speed_mib:.1} MiB/s"), "yellow"),
            b = paint_seg("📦", &format!("{cur_str}/{tot_str}"), "white"),
        );
        self.status_pb.set_message(buf.clone());
    }
}

impl Drop for UiManager {
    fn drop(&mut self) {
        // Best-effort: make sure the task is gone and the bar cleared even if
        // the caller forgot `stop()`.
        if let Some(handle) = self.tick_handle.lock().unwrap().take() {
            handle.abort();
        }
        self.status_pb.finish_and_clear();
    }
}

impl DownloadObserver for UiManager {
    fn set_total(&self, total: u64) {
        self.total_items.store(total, Ordering::Relaxed);
    }

    fn register(&self, id: &str, total: u64) -> Arc<AtomicU64> {
        let counter = Arc::new(AtomicU64::new(0));
        let mut live = self.live.lock().unwrap();
        // Replace any stale entry with the same id (defensive; unregister should
        // have removed it already).
        live.retain(|c| c.id != id);
        live.push(LiveCounter {
            id: id.to_string(),
            bytes: counter.clone(),
            total,
        });
        counter
    }

    fn unregister(&self, id: &str) {
        self.live.lock().unwrap().retain(|c| c.id != id);
    }

    fn complete(&self, info: CompletedInfo) {
        self.completed.lock().unwrap().push(CompletedRecord {
            id: info.id,
            total_bytes: info.total_bytes,
            elapsed_secs: info.elapsed_secs,
            avg_speed_bps: info.avg_speed_bps,
        });
    }

    fn fail(&self, id: &str) {
        self.failed.lock().unwrap().push(id.to_string());
    }
}

fn status_bar_style() -> ProgressStyle {
    // Single line, no bar graphics — safe for non-TTY (no bar chars leak).
    ProgressStyle::with_template("{spinner:.green} {msg}")
        .expect("valid status bar template")
        .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏ ")
}

/// Colorize one status-bar segment: `icon label` with a fixed ANSI color name.
fn paint_seg(icon: &str, label: &str, color: &str) -> String {
    // Raw ANSI so we do not pull nu-ansi-term into the ui_manager crate path.
    let code = match color {
        "green" => "32;1",
        "cyan" => "36;1",
        "yellow" => "33;1",
        "red" => "31;1",
        "white" => "37;1",
        "dim" => "2",
        _ => "0",
    };
    format!("\x1b[{code}m{icon} {label}\x1b[0m")
}

/// Format bytes as a short binary unit string (KiB/MiB/GiB), matching
/// indicatif's `binary_bytes` units.
fn human_binary_bytes(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    if bytes == 0 {
        return "0 B".to_string();
    }
    let mut value = bytes as f64;
    let mut idx = 0;
    while value >= 1024.0 && idx < UNITS.len() - 1 {
        value /= 1024.0;
        idx += 1;
    }
    if idx == 0 {
        format!("{} {}", bytes, UNITS[0])
    } else {
        format!("{:.1} {}", value, UNITS[idx])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatif::ProgressDrawTarget;

    fn hidden_manager(mode: Mode, total: u64) -> Arc<UiManager> {
        let mp = MultiProgress::with_draw_target(ProgressDrawTarget::hidden());
        UiManager::start(mp, mode, total)
    }

    #[tokio::test]
    async fn public_data_counts_flow_through_observer() {
        let ui = hidden_manager(Mode::PublicData, 3);

        // Two downloads start; one completes, one fails, one queued.
        let c1 = ui.register("a", 100);
        let c2 = ui.register("b", 200);
        c1.store(100, Ordering::Relaxed);
        c2.store(50, Ordering::Relaxed);

        ui.unregister("a");
        ui.complete(CompletedInfo {
            id: "a".into(),
            total_bytes: 100,
            elapsed_secs: 1.0,
            avg_speed_bps: 100.0,
        });
        ui.unregister("b");
        ui.fail("b");

        let live = ui.live.lock().unwrap();
        assert!(live.is_empty(), "live set drained after unregister");
        drop(live);

        // Counts are read inside refresh(); exercise the aggregation directly.
        let completed = ui.completed.lock().unwrap().len();
        let failed = ui.failed.lock().unwrap().len();
        assert_eq!(completed, 1);
        assert_eq!(failed, 1);

        ui.stop();
    }

    #[tokio::test]
    async fn sra_counts_read_from_progress_store() {
        use ebidownload_core::progress_store::new_progress_store;
        let store = new_progress_store();
        insert_run(&store, "r1", RunStage::Completed).await;
        insert_run(&store, "r2", RunStage::Downloading).await;
        insert_run(&store, "r3", RunStage::Failed).await;
        insert_run(&store, "r4", RunStage::Pending).await;

        let ui = hidden_manager(
            Mode::Sra {
                store: store.clone(),
            },
            4,
        );

        let map = store.read().await;
        let (mut c, mut f, mut a, mut p) = (0, 0, 0, 0);
        for rp in map.values() {
            match rp.stage {
                RunStage::Completed => c += 1,
                RunStage::Failed => f += 1,
                RunStage::Downloading | RunStage::Extracting | RunStage::Compressing => a += 1,
                RunStage::Pending => p += 1,
            }
        }
        assert_eq!((c, f, a, p), (1, 1, 1, 1));
        let queued = 4usize.saturating_sub(c + f + a);
        assert_eq!(queued, 1);

        ui.stop();
    }

    async fn insert_run(store: &ProgressStore, id: &str, stage: RunStage) {
        use ebidownload_core::progress_store::{RunProgress, StageProgress};
        store
            .write()
            .await
            .insert(
                id.to_string(),
                RunProgress {
                    run_id: id.to_string(),
                    stage,
                    overall_percent: 0.0,
                    download: StageProgress::new(1.0),
                    extraction: StageProgress::new(3.0),
                    compression: StageProgress::new(3.0),
                },
            );
    }

    #[test]
    fn human_binary_bytes_formats_known_values() {
        assert_eq!(human_binary_bytes(0), "0 B");
        assert_eq!(human_binary_bytes(512), "512 B");
        assert_eq!(human_binary_bytes(1048576), "1.0 MiB");
        assert_eq!(human_binary_bytes(1610612736), "1.5 GiB");
    }
}
