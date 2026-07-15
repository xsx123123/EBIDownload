use crate::progress::{transfer_bar_style, verify_bar_style};
use crate::progress_store::ProgressStore;
use anyhow::{anyhow, Result};
use futures::StreamExt;
use indicatif::{MultiProgress, ProgressBar};
use md5;
use quick_xml::events::Event;
use quick_xml::Reader;
use reqwest::{header, Client, StatusCode};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs::File;
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::str;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::sync::{mpsc, Mutex};
use tracing::{info, warn};

// ============================
// 1. Data Structures
// ============================

#[derive(Debug, Clone)]
pub struct SraMetadata {
    pub s3_uri: String,
    pub http_url: String,
    pub md5: Option<String>,
    pub size: u64,
}

/// Simple pause/resume token that can be shared between the GUI and the
/// AWS download workers.
#[derive(Clone, Default)]
pub struct PauseToken {
    paused: Arc<AtomicBool>,
}

impl PauseToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn pause(&self) {
        self.paused.store(true, Ordering::Relaxed);
    }

    pub fn resume(&self) {
        self.paused.store(false, Ordering::Relaxed);
    }

    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Relaxed)
    }

    /// Async-friendly wait: yields back to the Tokio runtime while paused.
    pub async fn wait_while_paused(&self) {
        while self.is_paused() {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}

#[derive(Debug, Clone)]
struct ChunkInfo {
    id: usize,
    start: u64,
    end: u64,
}

#[derive(Debug, Deserialize, Serialize)]
struct ProgressData {
    downloaded_chunks: Vec<usize>,
}

// ============================
// 2. Metadata Parsing and Conversion
// ============================

pub struct SraUtils;

impl SraUtils {
    pub async fn get_metadata(run_id: &str, _api_key: Option<&str>) -> Result<Option<SraMetadata>> {
        let url = format!(
            "https://eutils.ncbi.nlm.nih.gov/entrez/eutils/efetch.fcgi?db=sra&id={}&rettype=full&retmode=xml",
            run_id
        );

        // 🟢 Modification 1: Timeout increased to 60 seconds
        let client = Client::builder().timeout(Duration::from_secs(60)).build()?;

        let mut attempt = 0;
        let max_retries = 10; // 🟢 Modification 2: Max retries increased to 10

        loop {
            attempt += 1;
            let result = client.get(&url).send().await;

            match result {
                Ok(resp) => {
                    if resp.status().is_success() {
                        let text = resp.text().await?;
                        return parse_sra_xml(&text);
                    } else {
                        if attempt >= max_retries {
                            return Err(anyhow!("NCBI API Error: Status {}", resp.status()));
                        }
                        warn!(
                            "⚠️  [Network] NCBI Server Error ({}), retrying ({}/{})...",
                            resp.status(),
                            attempt,
                            max_retries
                        );
                    }
                }
                Err(e) => {
                    if attempt >= max_retries {
                        return Err(anyhow!(
                            "Failed to connect to NCBI after {} attempts: {}",
                            max_retries,
                            e
                        ));
                    }
                    // 🟢 Modification 3: Retry wait time increased to 10 seconds (more stable)
                    warn!(
                        "⚠️  [Network] Connection failed: {}. Retrying in 10s ({}/{})...",
                        e, attempt, max_retries
                    );
                }
            }

            // 🟢 Wait 10 seconds
            tokio::time::sleep(Duration::from_secs(10)).await;
        }
    }
}

// ... (resolve_urls, parse_sra_xml and other functions remain unchanged, please copy the previous code or keep it as is)
// To save space, only the SraUtils modification part is listed here. If the ResumableDownloader part has not changed, it does not need to be moved.
// But for completeness, here is the rest:

fn resolve_urls(raw_url: &str) -> Option<(String, String)> {
    if let Some(rest) = raw_url.strip_prefix("https://") {
        if let Some((bucket, key)) = rest.split_once(".s3.amazonaws.com/") {
            let s3 = format!("s3://{}/{}", bucket, key);
            return Some((s3, raw_url.to_string()));
        }
    }
    if let Some(rest) = raw_url.strip_prefix("s3://") {
        if let Some((bucket, key)) = rest.split_once('/') {
            let https = format!("https://{}.s3.amazonaws.com/{}", bucket, key);
            return Some((raw_url.to_string(), https));
        }
    }
    None
}

fn parse_sra_xml(xml_text: &str) -> Result<Option<SraMetadata>> {
    let mut reader = Reader::from_str(xml_text);
    let mut buf = Vec::new();
    let mut current_file_md5: Option<String> = None;
    let mut current_file_size: u64 = 0;
    let mut found_metadata: Option<SraMetadata> = None;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) | Ok(Event::Empty(ref e)) => {
                let name = e.local_name();
                let name_str = str::from_utf8(name.as_ref()).unwrap_or("");
                if name_str.eq_ignore_ascii_case("SRAFile") || name_str.eq_ignore_ascii_case("Run")
                {
                    current_file_md5 = None;
                    current_file_size = 0;
                    for attr in e.attributes().flatten() {
                        let k = str::from_utf8(attr.key.as_ref()).unwrap_or("");
                        let v = str::from_utf8(attr.value.as_ref()).unwrap_or("");
                        if k.eq_ignore_ascii_case("md5") {
                            current_file_md5 = Some(v.to_string());
                        } else if k.eq_ignore_ascii_case("size") {
                            current_file_size = v.parse().unwrap_or(0);
                        }
                    }
                } else if name_str.eq_ignore_ascii_case("Alternatives") {
                    let mut is_aws = false;
                    let mut is_worldwide = false;
                    let mut curr_url = String::new();
                    for attr in e.attributes().flatten() {
                        let k = str::from_utf8(attr.key.as_ref()).unwrap_or("");
                        let v = str::from_utf8(attr.value.as_ref()).unwrap_or("");
                        if k.eq_ignore_ascii_case("org") && v.eq_ignore_ascii_case("AWS") {
                            is_aws = true;
                        } else if k.eq_ignore_ascii_case("free_egress")
                            && v.eq_ignore_ascii_case("worldwide")
                        {
                            is_worldwide = true;
                        } else if k.eq_ignore_ascii_case("url") {
                            curr_url = v.to_string();
                        }
                    }
                    if is_aws && is_worldwide && !curr_url.is_empty() {
                        if let Some((s3_uri, http_url)) = resolve_urls(&curr_url) {
                            found_metadata = Some(SraMetadata {
                                s3_uri,
                                http_url,
                                md5: current_file_md5.clone(),
                                size: current_file_size,
                            });
                            break;
                        }
                    }
                }
            }
            Ok(Event::Eof) => break,
            _ => {}
        }
        buf.clear();
    }
    Ok(found_metadata)
}

pub struct ResumableDownloader {
    run_id: String,
    metadata: SraMetadata,
    filepath: PathBuf,
    meta_file: PathBuf,
    chunk_size: u64,
    max_workers: usize,
    client: Client,
    mp: Option<Arc<MultiProgress>>,
    progress_bytes: Option<Arc<AtomicU64>>,
    pause_token: Option<PauseToken>,
    progress_store: Option<ProgressStore>,
}

impl ResumableDownloader {
    pub async fn new(
        run_id: String,
        metadata: SraMetadata,
        save_dir: PathBuf,
        chunk_size_mb: u64,
        max_workers: usize,
        mp: Option<Arc<MultiProgress>>,
        progress_store: Option<ProgressStore>,
    ) -> Result<Self> {
        let filename = metadata
            .s3_uri
            .split('/')
            .next_back()
            .filter(|name| !name.is_empty())
            .unwrap_or(&run_id)
            .to_string();
        let filepath = save_dir.join(&filename);
        let meta_file = filepath.with_extension("meta.json");

        let client = Client::builder()
            .http1_only()
            .timeout(Duration::from_secs(60))
            .connect_timeout(Duration::from_secs(10))
            .pool_max_idle_per_host(max_workers)
            .build()?;

        Ok(Self {
            run_id,
            metadata,
            filepath,
            meta_file,
            chunk_size: chunk_size_mb * 1024 * 1024,
            max_workers,
            client,
            mp,
            progress_bytes: None,
            pause_token: None,
            progress_store,
        })
    }

    pub fn with_progress_bytes(mut self, progress: Arc<AtomicU64>) -> Self {
        self.progress_bytes = Some(progress);
        self
    }

    pub fn with_pause_token(mut self, token: PauseToken) -> Self {
        self.pause_token = Some(token);
        self
    }

    // ... (load_progress, save_progress, start, verify_integrity methods remain unchanged)
    fn load_progress(&self) -> HashSet<usize> {
        if self.meta_file.exists() {
            if let Ok(content) = std::fs::read_to_string(&self.meta_file) {
                if let Ok(progress) = serde_json::from_str::<ProgressData>(&content) {
                    return progress.downloaded_chunks.into_iter().collect();
                }
            }
        }
        HashSet::new()
    }
    fn save_progress(&self, downloaded_chunks: &HashSet<usize>) -> Result<()> {
        let progress_data = ProgressData {
            downloaded_chunks: downloaded_chunks.iter().cloned().collect(),
        };
        let content = serde_json::to_string(&progress_data)?;
        std::fs::write(&self.meta_file, content)?;
        Ok(())
    }

    fn invalidate_download(&self) {
        for path in [&self.filepath, &self.meta_file] {
            match std::fs::remove_file(path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => warn!(
                    "Failed to remove invalid download {}: {error}",
                    path.display()
                ),
            }
        }
    }

    pub async fn start(&self) -> Result<bool> {
        let start_time = std::time::Instant::now();

        // 🟢 Check if file exists and size matches, then verify MD5 first
        if self.filepath.exists() {
            if let Ok(meta) = tokio::fs::metadata(&self.filepath).await {
                if meta.len() == self.metadata.size {
                    let msg = format!(
                        "   📂 Found existing file with correct size: {}",
                        self.run_id
                    );
                    if let Some(mp) = &self.mp {
                        let _ = mp.println(&msg);
                    } else {
                        println!("{}", msg);
                    }

                    // Verify integrity
                    if self.verify_integrity(0.0, true).await? {
                        return Ok(true);
                    } else {
                        let msg = format!(
                            "   ❌ Integrity check failed for existing file. Redownloading: {}",
                            self.run_id
                        );
                        if let Some(mp) = &self.mp {
                            let _ = mp.println(&msg);
                        } else {
                            println!("{}", msg);
                        }
                    }
                }
            }
        }

        if !self.filepath.exists() {
            if let Some(parent) = self.filepath.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let file = File::create(&self.filepath)?;
            file.set_len(self.metadata.size)?;
        }

        let mut downloaded_chunks = self.load_progress();
        let num_chunks = self.metadata.size.div_ceil(self.chunk_size);
        let mut tasks = Vec::new();
        for i in 0..num_chunks {
            if !downloaded_chunks.contains(&(i as usize)) {
                tasks.push(ChunkInfo {
                    id: i as usize,
                    start: i * self.chunk_size,
                    end: std::cmp::min((i + 1) * self.chunk_size - 1, self.metadata.size - 1),
                });
            }
        }

        // 🟢 Setup Progress Bar
        let pb = if let Some(mp) = &self.mp {
            mp.add(ProgressBar::new(self.metadata.size))
        } else {
            ProgressBar::new(self.metadata.size)
        };
        pb.set_style(transfer_bar_style());
        pb.set_prefix(self.run_id.clone());
        pb.set_message("Downloading");
        pb.enable_steady_tick(std::time::Duration::from_millis(100));

        // Keep per-file details in the log file without cluttering active progress bars.
        let size_gb = self.metadata.size as f64 / 1024.0 / 1024.0 / 1024.0;
        let details = format!(
            "📌 {} │ 📦 {:.2} GB │ 🔑 {} │ 💾 {}",
            self.run_id,
            size_gb,
            self.metadata.md5.as_deref().unwrap_or("N/A"),
            self.filepath.display()
        );
        info!(target: "download_detail", "{}", details);

        if tasks.is_empty() {
            let msg = format!(
                "✅ {} │ File exists, starting integrity check...",
                self.run_id
            );
            pb.println(&msg);
            info!(target: "download_detail", "{}", msg);
            pb.finish_and_clear();
            return self
                .verify_integrity(start_time.elapsed().as_secs_f64(), true)
                .await;
        }

        let initial_bytes = downloaded_chunks.len() as u64 * self.chunk_size;
        // 🟢 Fix: Use AtomicU64 to track global progress safely (handles retries)
        // If the caller supplied a shared counter (e.g. the GUI), use it so the
        // progress can be observed externally.
        let global_bytes = self
            .progress_bytes
            .clone()
            .unwrap_or_else(|| Arc::new(AtomicU64::new(0)));
        global_bytes.store(
            std::cmp::min(initial_bytes, self.metadata.size),
            Ordering::Relaxed,
        );

        pb.set_position(global_bytes.load(Ordering::Relaxed));

        // 🟢 Spawn progress monitor
        let pb_monitor = pb.clone();
        let gb_monitor = global_bytes.clone();
        let store_monitor = self.progress_store.clone();
        let run_id_monitor = self.run_id.clone();
        let sra_size_monitor = self.metadata.size;
        let monitor_handle = tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(100));
            loop {
                interval.tick().await;
                let bytes = gb_monitor.load(Ordering::Relaxed);
                pb_monitor.set_position(bytes);
                if let Some(store) = &store_monitor {
                    let mut map = store.write().await;
                    if let Some(rp) = map.get_mut(&run_id_monitor) {
                        rp.download.update(bytes, sra_size_monitor);
                        rp.recalculate_overall();
                    }
                }
            }
        });

        let (tx, mut rx) = mpsc::channel(100);
        let shared_tasks = Arc::new(Mutex::new(tasks));
        let pause_token = self.pause_token.clone();
        for _ in 0..self.max_workers {
            let client = self.client.clone();
            let url = self.metadata.http_url.clone();
            let filepath = self.filepath.clone();
            let queue = shared_tasks.clone();
            let tx = tx.clone();
            let gb_clone = global_bytes.clone();
            let pause_token_worker = pause_token.clone();
            tokio::spawn(async move {
                loop {
                    if let Some(token) = &pause_token_worker {
                        token.wait_while_paused().await;
                    }

                    let task = {
                        let mut q = queue.lock().await;
                        q.pop()
                    };
                    match task {
                        Some(t) => {
                            match download_chunk_http(
                                client.clone(),
                                &url,
                                &t,
                                &filepath,
                                gb_clone.clone(),
                                pause_token_worker.clone(),
                            )
                            .await
                            {
                                Ok(_) => {
                                    if tx.send(Ok(t.id)).await.is_err() {
                                        break;
                                    }
                                }
                                Err(e) => {
                                    let _ = tx.send(Err(e)).await;
                                }
                            }
                        }
                        None => break,
                    }
                }
            });
        }
        drop(tx);
        while let Some(msg) = rx.recv().await {
            match msg {
                Ok(chunk_id) => {
                    downloaded_chunks.insert(chunk_id);
                    if let Err(e) = self.save_progress(&downloaded_chunks) {
                        eprintln!("Warning: Failed to save progress: {}", e);
                    }
                }
                Err(_e) => {}
            }
        }

        monitor_handle.abort();
        pb.finish_and_clear();
        if downloaded_chunks.len() as u64 == num_chunks {
            self.verify_integrity(start_time.elapsed().as_secs_f64(), false)
                .await
        } else {
            let msg = format!(
                "❌ {} │ Download incomplete. Progress saved, please retry.",
                self.run_id
            );
            pb.println(&msg);
            warn!("{}", msg);
            Ok(false)
        }
    }
    async fn verify_integrity(
        &self,
        download_duration: f64,
        skipped_download: bool,
    ) -> Result<bool> {
        let start_time = std::time::Instant::now();
        if self.metadata.md5.is_none() {
            let local_size = tokio::fs::metadata(&self.filepath).await?.len();
            if local_size != self.metadata.size {
                warn!(
                    "❌ {} │ Size mismatch: local={} remote={}",
                    self.run_id, local_size, self.metadata.size
                );
                self.invalidate_download();
                return Ok(false);
            }
            let _ = std::fs::remove_file(&self.meta_file);
            return Ok(true);
        }

        let pb = if let Some(mp) = &self.mp {
            mp.add(ProgressBar::new(self.metadata.size))
        } else {
            ProgressBar::new(self.metadata.size)
        };

        pb.set_style(verify_bar_style());
        pb.set_prefix(self.run_id.clone());
        pb.set_message("Verifying");
        pb.enable_steady_tick(std::time::Duration::from_millis(100));

        let mut file = tokio::fs::File::open(&self.filepath).await?;
        let mut ctx = md5::Context::new();
        let mut buf = vec![0u8; 1024 * 1024];
        loop {
            let n = file.read(&mut buf).await?;
            if n == 0 {
                break;
            }
            ctx.consume(&buf[..n]);
            pb.inc(n as u64);
        }
        pb.finish_and_clear();

        let local_md5 = format!("{:x}", ctx.compute());
        let expected_md5 = self.metadata.md5.as_ref().unwrap();
        if &local_md5 == expected_md5 {
            if !skipped_download {
                let speed = (self.metadata.size as f64 / 1024.0 / 1024.0) / download_duration;
                let msg = format!("✅ {} │ 🚀 {:.2} MB/s", self.run_id, speed);
                info!(target: "download_detail", "{}", msg);
            }
            let msg = format!(
                "✅ {} │ 🔍 MD5 OK ({:.2}s)",
                self.run_id,
                start_time.elapsed().as_secs_f64()
            );
            info!(target: "download_detail", "{}", msg);

            let _ = std::fs::remove_file(&self.meta_file);
            Ok(true)
        } else {
            let msg = format!(
                "❌ {} │ MD5 mismatch! Local: {}  Remote: {}",
                self.run_id, local_md5, expected_md5
            );
            warn!("{}", msg);
            self.invalidate_download();
            Ok(false)
        }
    }
}

async fn download_chunk_http(
    client: Client,
    url: &str,
    chunk: &ChunkInfo,
    filepath: &Path,
    global_bytes: Arc<AtomicU64>,
    pause_token: Option<PauseToken>,
) -> Result<()> {
    let mut retry = 0;
    let mut current_offset = chunk.start;

    loop {
        // Yield while paused so the user can pause/resume the download.
        if let Some(token) = &pause_token {
            token.wait_while_paused().await;
        }

        if current_offset > chunk.end {
            return Ok(());
        }

        let range_header = format!("bytes={}-{}", current_offset, chunk.end);
        let resp = client
            .get(url)
            .header(header::RANGE, range_header)
            .send()
            .await;

        if let Ok(response) = resp {
            let expected_content_range = format!("bytes {}-{}/", current_offset, chunk.end);
            let has_expected_range = response
                .headers()
                .get(header::CONTENT_RANGE)
                .and_then(|value| value.to_str().ok())
                .is_some_and(|value| value.starts_with(&expected_content_range));
            if response.status() != StatusCode::PARTIAL_CONTENT || !has_expected_range {
                retry += 1;
                if retry > 10 {
                    return Err(anyhow!(
                        "Unexpected HTTP Range response: status={}, content-range={:?}",
                        response.status(),
                        response.headers().get(header::CONTENT_RANGE)
                    ));
                }
                tokio::time::sleep(Duration::from_secs(retry)).await;
                continue;
            }
            let mut stream = response.bytes_stream();
            let mut file = std::fs::OpenOptions::new().write(true).open(filepath)?;
            file.seek(SeekFrom::Start(current_offset))?;

            let mut stream_error = false;
            let offset_start = current_offset;

            while let Some(item) = stream.next().await {
                // Check pause inside the byte stream loop so an active
                // HTTP connection also stops downloading immediately.
                if let Some(token) = &pause_token {
                    token.wait_while_paused().await;
                }

                match item {
                    Ok(bytes) => {
                        if file.write_all(&bytes).is_err() {
                            stream_error = true;
                            break;
                        }
                        let len = bytes.len() as u64;
                        global_bytes.fetch_add(len, Ordering::Relaxed);
                        current_offset += len;
                    }
                    Err(_) => {
                        stream_error = true;
                        break;
                    }
                }
            }

            if !stream_error && current_offset > chunk.end {
                return Ok(());
            }

            // If we made progress, reset retry counter
            if current_offset > offset_start {
                retry = 0;
            }
        }

        retry += 1;
        if retry > 20 {
            return Err(anyhow!("Chunk failed after multiple retries"));
        }
        let sleep_sec = std::cmp::min(30, 1_u64 << std::cmp::min(retry, 5));
        tokio::time::sleep(Duration::from_secs(sleep_sec)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn keeps_the_source_object_filename_for_generic_downloads() {
        let temp_dir = tempfile::tempdir().unwrap();
        let downloader = ResumableDownloader::new(
            "k2_viral".to_string(),
            SraMetadata {
                s3_uri: "s3://genome-idx/kraken/k2_viral_20240112.tar.gz".to_string(),
                http_url: "https://genome-idx.s3.amazonaws.com/kraken/k2_viral_20240112.tar.gz"
                    .to_string(),
                md5: None,
                size: 1,
            },
            temp_dir.path().to_path_buf(),
            64,
            1,
            None,
            None,
        )
        .await
        .unwrap();

        assert_eq!(
            downloader.filepath.file_name().unwrap(),
            "k2_viral_20240112.tar.gz"
        );
    }

    #[tokio::test]
    async fn removes_corrupt_file_and_progress_metadata_after_md5_failure() {
        let temp_dir = tempfile::tempdir().unwrap();
        let downloader = ResumableDownloader::new(
            "example".to_string(),
            SraMetadata {
                s3_uri: "s3://example-bucket/example.dat".to_string(),
                http_url: "https://example-bucket.s3.amazonaws.com/example.dat".to_string(),
                md5: Some("d41d8cd98f00b204e9800998ecf8427e".to_string()),
                size: 3,
            },
            temp_dir.path().to_path_buf(),
            64,
            1,
            None,
            None,
        )
        .await
        .unwrap();

        std::fs::write(&downloader.filepath, b"bad").unwrap();
        std::fs::write(&downloader.meta_file, r#"{"downloaded_chunks":[0]}"#).unwrap();

        assert!(!downloader.verify_integrity(0.0, false).await.unwrap());
        assert!(!downloader.filepath.exists());
        assert!(!downloader.meta_file.exists());
    }
}
