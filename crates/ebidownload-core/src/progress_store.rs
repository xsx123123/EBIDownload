use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

pub type ProgressStore = Arc<RwLock<HashMap<String, RunProgress>>>;

pub fn new_progress_store() -> ProgressStore {
    Arc::new(RwLock::new(HashMap::new()))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunProgress {
    pub run_id: String,
    pub stage: RunStage,
    pub overall_percent: f64,
    pub download: StageProgress,
    pub extraction: StageProgress,
    pub compression: StageProgress,
}

impl RunProgress {
    pub fn recalculate_overall(&mut self) {
        let total_weight = self.download.weight + self.extraction.weight + self.compression.weight;
        if total_weight == 0.0 {
            self.overall_percent = 0.0;
            return;
        }
        self.overall_percent = (
            self.download.percent * self.download.weight
                + self.extraction.percent * self.extraction.weight
                + self.compression.percent * self.compression.weight
        ) / total_weight;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStage {
    Pending,
    Downloading,
    Extracting,
    Compressing,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StageProgress {
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub weight: f64,
    pub percent: f64,
}

impl StageProgress {
    pub fn new(weight: f64) -> Self {
        Self {
            bytes_done: 0,
            bytes_total: 0,
            weight,
            percent: 0.0,
        }
    }

    pub fn update(&mut self, done: u64, total: u64) {
        self.bytes_done = done;
        self.bytes_total = total;
        self.percent = if total > 0 {
            (done as f64 / total as f64 * 100.0).min(100.0)
        } else {
            0.0
        };
    }
}

pub type CompressionProgressCallback = Arc<dyn Fn(u64, u64) + Send + Sync>;
