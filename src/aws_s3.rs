use anyhow::{anyhow, Result};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use md5;
use quick_xml::events::Event;
use quick_xml::Reader;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs::File;
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, mpsc}; 
use tokio::io::AsyncReadExt; 
use std::str;
use reqwest::{Client, header};
use futures::StreamExt;

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
        let client = Client::builder()
            .timeout(Duration::from_secs(60)) 
            .build()?;

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
                        eprintln!("⚠️  [Network] NCBI Server Error ({}), retrying ({}/{})...", resp.status(), attempt, max_retries);
                    }
                },
                Err(e) => {
                    if attempt >= max_retries {
                        return Err(anyhow!("Failed to connect to NCBI after {} attempts: {}", max_retries, e));
                    }
                    // 🟢 Modification 3: Retry wait time increased to 10 seconds (more stable)
                    eprintln!("⚠️  [Network] Connection failed: {}. Retrying in 10s ({}/{})...", e, attempt, max_retries);
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
                if name_str.eq_ignore_ascii_case("SRAFile") || name_str.eq_ignore_ascii_case("Run") {
                    current_file_md5 = None;
                    current_file_size = 0;
                    for attr in e.attributes().flatten() {
                        let k = str::from_utf8(attr.key.as_ref()).unwrap_or("");
                        let v = str::from_utf8(attr.value.as_ref()).unwrap_or("");
                        if k.eq_ignore_ascii_case("md5") { current_file_md5 = Some(v.to_string()); }
                        else if k.eq_ignore_ascii_case("size") { current_file_size = v.parse().unwrap_or(0); }
                    }
                } else if name_str.eq_ignore_ascii_case("Alternatives") {
                    let mut is_aws = false;
                    let mut is_worldwide = false;
                    let mut curr_url = String::new();
                    for attr in e.attributes().flatten() {
                        let k = str::from_utf8(attr.key.as_ref()).unwrap_or("");
                        let v = str::from_utf8(attr.value.as_ref()).unwrap_or("");
                        if k.eq_ignore_ascii_case("org") && v.eq_ignore_ascii_case("AWS") { is_aws = true; }
                        else if k.eq_ignore_ascii_case("free_egress") && v.eq_ignore_ascii_case("worldwide") { is_worldwide = true; }
                        else if k.eq_ignore_ascii_case("url") { curr_url = v.to_string(); }
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
}

impl ResumableDownloader {
    pub async fn new(
        run_id: String,
        metadata: SraMetadata,
        save_dir: PathBuf,
        chunk_size_mb: u64,
        max_workers: usize,
        mp: Option<Arc<MultiProgress>>,
    ) -> Result<Self> {
        let raw_name = metadata.s3_uri.split('/').last().unwrap_or(&run_id).to_string();
        let filename = if raw_name.ends_with(".sra") { raw_name } else { format!("{}.sra", raw_name) };
        let filepath = save_dir.join(&filename);
        let meta_file = filepath.with_extension("meta.json");

        // 🟢 Config: Download client also adds 60s timeout
        let client = Client::builder()
            .http1_only()
            .timeout(Duration::from_secs(60)) // Increase timeout
            .connect_timeout(Duration::from_secs(10))
            .pool_max_idle_per_host(max_workers)
            .build()?;

        Ok(Self { run_id, metadata, filepath, meta_file, chunk_size: chunk_size_mb * 1024 * 1024, max_workers, client, mp })
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
        let progress_data = ProgressData { downloaded_chunks: downloaded_chunks.iter().cloned().collect() };
        let content = serde_json::to_string(&progress_data)?;
        std::fs::write(&self.meta_file, content)?;
        Ok(())
    }
    pub async fn start(&self) -> Result<bool> {
        let start_time = std::time::Instant::now();
        if !self.filepath.exists() {
            if let Some(parent) = self.filepath.parent() { std::fs::create_dir_all(parent)?; }
            let file = File::create(&self.filepath)?;
            file.set_len(self.metadata.size)?;
        }
        
        let mut downloaded_chunks = self.load_progress();
        let num_chunks = (self.metadata.size + self.chunk_size - 1) / self.chunk_size;
        let mut tasks = Vec::new();
        for i in 0..num_chunks {
            if !downloaded_chunks.contains(&(i as usize)) {
                tasks.push(ChunkInfo { id: i as usize, start: i * self.chunk_size, end: std::cmp::min((i + 1) * self.chunk_size - 1, self.metadata.size - 1) });
            }
        }

        // 🟢 Setup Progress Bar
        let pb = if let Some(mp) = &self.mp {
             mp.add(ProgressBar::new(self.metadata.size))
        } else {
             ProgressBar::new(self.metadata.size)
        };
        pb.set_style(ProgressStyle::default_bar().template("{prefix:.cyan} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({binary_bytes_per_sec}, {eta}) {msg}")?.progress_chars("#>-"));
        pb.set_prefix(self.run_id.clone());
        
        // 🟢 Print details using pb.println to avoid interfering with bars
        let details = format!(
            "\n📌 [Details] {}\n   ├─ 📦 Size: {:.2} GB\n   ├─ 🔑 MD5 : {}\n   └─ 💾 Save: {}\n", 
            self.run_id,
            self.metadata.size as f64 / 1024.0 / 1024.0 / 1024.0,
            self.metadata.md5.as_deref().unwrap_or("Unknown"),
            self.filepath.display()
        );
        pb.println(details);

        if tasks.is_empty() {
            pb.println(format!("   ✅ File exists, starting integrity check: {}", self.run_id));
            pb.finish_and_clear();
            return self.verify_integrity(start_time.elapsed().as_secs_f64(), true).await;
        }

        let initial_bytes = downloaded_chunks.len() as u64 * self.chunk_size;
        pb.set_position(std::cmp::min(initial_bytes, self.metadata.size));
        let (tx, mut rx) = mpsc::channel(100); 
        let shared_tasks = Arc::new(Mutex::new(tasks));
        for _ in 0..self.max_workers {
            let client = self.client.clone();
            let url = self.metadata.http_url.clone();
            let filepath = self.filepath.clone();
            let queue = shared_tasks.clone();
            let tx = tx.clone();
            let pb_clone = pb.clone();
            tokio::spawn(async move {
                loop {
                    let task = { let mut q = queue.lock().await; q.pop() };
                    match task {
                        Some(t) => {
                            match download_chunk_http(client.clone(), &url, &t, &filepath, pb_clone.clone()).await {
                                Ok(_) => { if let Err(_) = tx.send(Ok(t.id)).await { break; } },
                                Err(e) => { let _ = tx.send(Err(e)).await; }
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
                    if let Err(e) = self.save_progress(&downloaded_chunks) { eprintln!("Warning: Failed to save progress: {}", e); }
                },
                Err(_e) => {}
            }
        }
        pb.finish_and_clear();
        if downloaded_chunks.len() as u64 == num_chunks {
            self.verify_integrity(start_time.elapsed().as_secs_f64(), false).await
        } else {
            pb.println("❌ Download incomplete. Progress saved, please retry.");
            Ok(false)
        }
    }
    async fn verify_integrity(&self, download_duration: f64, skipped_download: bool) -> Result<bool> {
        let start_time = std::time::Instant::now();
        if self.metadata.md5.is_none() { 
            let msg = "   ⚠️ No MD5 info, skipping verification";
            if let Some(mp) = &self.mp { let _ = mp.println(msg); } else { println!("{}", msg); }
            return Ok(true); 
        }
        
        let pb = if let Some(mp) = &self.mp {
             mp.add(ProgressBar::new(self.metadata.size))
        } else {
             ProgressBar::new(self.metadata.size)
        };
        
        pb.set_style(ProgressStyle::default_bar().template("🔍 Verifying [{bar:40.green/white}] {bytes}/{total_bytes} ({binary_bytes_per_sec})")?.progress_chars("##-"));
        
        let mut file = tokio::fs::File::open(&self.filepath).await?;
        let mut ctx = md5::Context::new();
        let mut buf = vec![0u8; 1024 * 1024]; 
        loop {
            let n = file.read(&mut buf).await?;
            if n == 0 { break; }
            ctx.consume(&buf[..n]);
            pb.inc(n as u64);
        }
        pb.finish_and_clear();
        
        let local_md5 = format!("{:x}", ctx.compute());
        let expected_md5 = self.metadata.md5.as_ref().unwrap();
        if &local_md5 == expected_md5 {
            if !skipped_download {
               let speed = (self.metadata.size as f64 / 1024.0 / 1024.0) / download_duration;
               let msg = format!("   └─ 🚀 Download speed: {:.2} MB/s", speed);
               if let Some(mp) = &self.mp { let _ = mp.println(msg); } else { println!("{}", msg); }
            }
            let msg = format!("   └─ ✅ MD5 verified (Time: {:.2}s)", start_time.elapsed().as_secs_f64());
            if let Some(mp) = &self.mp { let _ = mp.println(msg); } else { println!("{}", msg); }
            
            let _ = std::fs::remove_file(&self.meta_file);
            Ok(true)
        } else {
            let msg = format!("   └─ ❌ MD5 verification failed!\n      Local: {}\n      Remote: {}", local_md5, expected_md5);
            if let Some(mp) = &self.mp { let _ = mp.println(msg); } else { println!("{}", msg); }
            Ok(false)
        }
    }
}

async fn download_chunk_http(client: Client, url: &str, chunk: &ChunkInfo, filepath: &Path, pb: ProgressBar) -> Result<()> {
    let mut retry = 0;
    loop {
        let range_header = format!("bytes={}-{}", chunk.start, chunk.end);
        let resp = client.get(url).header(header::RANGE, range_header).send().await;
        match resp {
            Ok(response) => {
                if !response.status().is_success() {
                    retry += 1;
                    if retry > 5 { return Err(anyhow!("HTTP Status {}", response.status())); }
                    tokio::time::sleep(Duration::from_secs(retry)).await;
                    continue;
                }
                let mut stream = response.bytes_stream();
                let mut file = std::fs::OpenOptions::new().write(true).open(filepath)?;
                file.seek(SeekFrom::Start(chunk.start))?;
                let mut stream_error = false;
                while let Some(item) = stream.next().await {
                    match item {
                        Ok(bytes) => {
                            if let Err(_) = file.write_all(&bytes) { stream_error = true; break; }
                            pb.inc(bytes.len() as u64);
                        }
                        Err(_) => { stream_error = true; break; }
                    }
                }
                if !stream_error { return Ok(()); }
            }
            Err(_) => {}
        }
        retry += 1;
        if retry > 15 { return Err(anyhow!("Chunk failed")); }
        tokio::time::sleep(Duration::from_millis(500 * 2_u64.pow(retry as u32))).await;
    }
}

