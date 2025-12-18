use aws_sdk_s3::{Client as S3Client, config::Region};
use aws_config;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::fs::File;
use std::io::{Seek, SeekFrom, Write};
use serde::{Deserialize, Serialize};
use indicatif::{ProgressBar, ProgressStyle};
use anyhow::{Result, Context};
use reqwest;
use regex::Regex;
use std::time::Duration;
use quick_xml::Reader;
use quick_xml::events::Event;
use tokio::io::AsyncReadExt;
use md5;

/// SRA metadata containing the S3 URI, MD5, and size
#[derive(Debug, Clone)]
pub struct SraMetadata {
    pub s3_uri: String,
    pub md5: Option<String>,
    pub size: u64,
}

/// Utility functions for SRA handling
pub struct SraUtils;

impl SraUtils {
    /// Get SRA metadata from NCBI API
    pub async fn get_metadata(run_id: &str, api_key: Option<&str>) -> Result<Option<SraMetadata>> {
        let mut params = vec![
            ("db", "sra"),
            ("id", run_id),
            ("rettype", "full"),
            ("retmode", "xml"),
        ];

        if let Some(key) = api_key {
            params.push(("api_key", key));
        }

        let url = "https://eutils.ncbi.nlm.nih.gov/entrez/eutils/efetch.fcgi";
        println!("[{}] 正在获取元数据...", run_id);

        let client = reqwest::Client::new();
        let response = client
            .get(url)
            .query(&params)
            .timeout(Duration::from_secs(30))
            .send()
            .await
            .context("Failed to connect to NCBI API")?;

        if !response.status().is_success() {
            return Err(anyhow::anyhow!(
                "NCBI API request failed with status: {}",
                response.status()
            ));
        }

        let xml_text = response
            .text()
            .await
            .context("Failed to read response body")?;

        parse_sra_xml(&xml_text, run_id)
    }
}

/// Parse the XML response from NCBI API to extract SRA metadata
fn parse_sra_xml(xml_text: &str, run_id: &str) -> Result<Option<SraMetadata>> {
    let mut reader = Reader::from_str(xml_text);
    let mut buf = Vec::new();

    let mut target_url: Option<String> = None;
    let mut expected_md5: Option<String> = None;
    let mut file_size: u64 = 0;
    let mut current_element = String::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                current_element = std::str::from_utf8(e.local_name().as_ref()).unwrap_or("").to_ascii_lowercase();

                if current_element == "alternatives" {
                    let mut org_value = String::new();
                    let mut egress_value = String::new();
                    let mut url_value = String::new();

                    for attr in e.attributes() {
                        if let Ok(attr) = attr {
                            let attr_name = std::str::from_utf8(attr.key.as_ref())?;
                            match attr_name {
                                "org" => {
                                    org_value = std::str::from_utf8(&attr.value)?.to_ascii_lowercase();
                                }
                                "free_egress" => {
                                    egress_value = std::str::from_utf8(&attr.value)?.to_ascii_lowercase();
                                }
                                "url" => {
                                    url_value = std::str::from_utf8(&attr.value)?.to_string();
                                }
                                _ => {}
                            }
                        }
                    }

                    if org_value == "aws" && egress_value == "worldwide" && !url_value.is_empty() {
                        target_url = Some(url_value);
                    }
                } else if current_element == "srafile" {
                    let mut current_filename = String::new();
                    let mut current_md5 = String::new();
                    let mut current_size = String::new();

                    for attr in e.attributes() {
                        if let Ok(attr) = attr {
                            let attr_name = std::str::from_utf8(attr.key.as_ref())?;
                            match attr_name {
                                "filename" => {
                                    current_filename = std::str::from_utf8(&attr.value)?.to_string();
                                }
                                "md5" => {
                                    current_md5 = std::str::from_utf8(&attr.value)?.to_string();
                                }
                                "size" => {
                                    current_size = std::str::from_utf8(&attr.value)?.to_string();
                                }
                                _ => {}
                            }
                        }
                    }

                    // If this matches our target URL's filename, use its MD5 and size
                    if let Some(uri) = &target_url {
                        let s3_filename = uri.split('/').last().unwrap_or("");
                        if current_filename == s3_filename {
                            if !current_md5.is_empty() {
                                expected_md5 = Some(current_md5.clone()); // Clone to avoid move
                            }
                            if !current_size.is_empty() {
                                if let Ok(size_val) = current_size.parse::<u64>() {
                                    file_size = size_val;
                                }
                            }
                        }
                    }

                    // Capture other MD5s if we don't have one yet
                    if expected_md5.is_none() && !current_md5.is_empty() {
                        expected_md5 = Some(current_md5.clone()); // Clone to avoid move
                    }
                    if file_size == 0 && !current_size.is_empty() {
                        if let Ok(size_val) = current_size.parse::<u64>() {
                            file_size = size_val;
                        }
                    }
                } else if current_element == "run" {
                    for attr in e.attributes() {
                        if let Ok(attr) = attr {
                            let attr_name = std::str::from_utf8(attr.key.as_ref())?;
                            if attr_name == "size" {
                                let attr_value = std::str::from_utf8(&attr.value)?;
                                if let Ok(size_val) = attr_value.parse::<u64>() {
                                    file_size = size_val;
                                    break;
                                }
                            }
                        }
                    }
                }
            }
            Ok(Event::Empty(ref e)) => {
                // Handle empty elements
                current_element = std::str::from_utf8(e.local_name().as_ref()).unwrap_or("").to_ascii_lowercase();

                if current_element == "alternatives" {
                    let mut org_value = String::new();
                    let mut egress_value = String::new();
                    let mut url_value = String::new();

                    for attr in e.attributes() {
                        if let Ok(attr) = attr {
                            let attr_name = std::str::from_utf8(attr.key.as_ref())?;
                            match attr_name {
                                "org" => {
                                    org_value = std::str::from_utf8(&attr.value)?.to_ascii_lowercase();
                                }
                                "free_egress" => {
                                    egress_value = std::str::from_utf8(&attr.value)?.to_ascii_lowercase();
                                }
                                "url" => {
                                    url_value = std::str::from_utf8(&attr.value)?.to_string();
                                }
                                _ => {}
                            }
                        }
                    }

                    if org_value == "aws" && egress_value == "worldwide" && !url_value.is_empty() {
                        target_url = Some(url_value);
                    }
                }
            }
            Ok(Event::End(ref _e)) => {
                current_element.clear();
            }
            Ok(Event::Eof) => break,
            Err(_) => continue, // Ignore malformed XML
            _ => {}
        }
        buf.clear();
    }

    if let Some(url) = target_url {
        // Convert S3 URL to S3 URI format
        let s3_uri = if url.starts_with("https://") {
            // Extract bucket name and key from URL like https://bucket-name.s3.amazonaws.com/key
            let re = Regex::new(r"https://([^.]+)\.s3\.amazonaws\.com/(.+)")?;
            if let Some(caps) = re.captures(&url) {
                format!("s3://{}/{}", &caps[1], &caps[2])
            } else {
                url.replace("https://", "s3://").replace(".s3.amazonaws.com", "")
            }
        } else {
            url
        };

        println!("[{}] 元数据解析成功: Size={}, MD5={:?}", run_id, file_size, expected_md5);

        Ok(Some(SraMetadata {
            s3_uri,
            md5: expected_md5,
            size: file_size,
        }))
    } else {
        println!("[{}] 未找到有效的 AWS S3 链接", run_id);
        Ok(None)
    }
}

/// Chunk information for concurrent download
#[derive(Debug, Clone)]
struct ChunkInfo {
    id: usize,
    start: u64,
    end: u64,
}

/// A resumable downloader that uses AWS S3 client to download files in chunks
pub struct ResumableDownloader {
    run_id: String,
    metadata: SraMetadata,
    bucket: String,
    key: String,
    filename: String,
    filepath: PathBuf,
    meta_file: PathBuf,
    chunk_size: u64,
    max_workers: usize,
    s3_client: S3Client,
}

impl ResumableDownloader {
    pub async fn new(
        run_id: String,
        metadata: SraMetadata,
        save_dir: PathBuf,
        chunk_size_mb: u64,
        max_workers: usize,
    ) -> Result<Self> {
        let (bucket, key) = Self::parse_s3_uri(&metadata.s3_uri)?;

        let filename = Path::new(&key)
            .file_name()
            .ok_or_else(|| anyhow::anyhow!("Could not extract filename from key"))?
            .to_string_lossy()
            .to_string();

        let filepath = save_dir.join(&filename);
        let meta_file = filepath.with_extension(format!("{}.meta.json", filename));

        // Configure AWS SDK with anonymous requests for public S3 buckets
        let config = aws_config::from_env()
            .region(Region::new("us-east-1")) // Default region, can be overridden
            .load()
            .await;
        let s3_client = S3Client::new(&config);

        Ok(Self {
            run_id,
            metadata,
            bucket,
            key,
            filename,
            filepath,
            meta_file,
            chunk_size: chunk_size_mb * 1024 * 1024,
            max_workers,
            s3_client,
        })
    }

    fn parse_s3_uri(uri: &str) -> Result<(String, String)> {
        let re = Regex::new(r"s3://([^/]+)/(.+)")?;
        if let Some(caps) = re.captures(uri) {
            Ok((caps[1].to_string(), caps[2].to_string()))
        } else {
            Err(anyhow::anyhow!("Invalid S3 URI: {}", uri))
        }
    }

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

    pub async fn start(&self) -> Result<bool> {
        let start_time = std::time::Instant::now();
        println!(
            "[{}] 准备下载: {} ({:.2} GB)",
            self.run_id,
            self.filename,
            self.metadata.size as f64 / (1024.0_f64.powi(3))
        );

        // Pre-allocate file
        if !self.filepath.exists() {
            let file = File::create(&self.filepath)?;
            file.set_len(self.metadata.size)?;
        }

        // Calculate chunk tasks
        let total_size = self.metadata.size;
        let num_chunks = (total_size + self.chunk_size - 1) / self.chunk_size;
        let mut downloaded_chunks = self.load_progress();

        let mut tasks = Vec::new();
        for i in 0..num_chunks {
            if !downloaded_chunks.contains(&(i as usize)) {
                let start = i * self.chunk_size;
                let end = std::cmp::min(start + self.chunk_size - 1, total_size - 1);
                tasks.push(ChunkInfo {
                    id: i as usize,
                    start,
                    end,
                });
            }
        }

        let initial_downloaded = downloaded_chunks.len() as u64 * self.chunk_size;
        let initial_downloaded = std::cmp::min(initial_downloaded, total_size);

        if tasks.is_empty() {
            println!("[{}] 文件已存在，跳过下载，直接校验...", self.run_id);
            return self.verify_integrity(start_time.elapsed().as_secs_f64(), true).await;
        }

        println!(
            "[{}] 剩余分片: {}/{} | 线程: {}",
            self.run_id,
            tasks.len(),
            num_chunks,
            self.max_workers
        );

        // Progress bar
        let progress_bar = ProgressBar::new(total_size);
        progress_bar.set_position(initial_downloaded);
        progress_bar.set_style(
            ProgressStyle::default_bar()
                .template("{prefix:.cyan} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta}) {msg}")?
                .progress_chars("#>-"),
        );
        progress_bar.set_prefix(self.run_id.clone());

        // Download chunks in parallel
        let mut handles = Vec::new();
        for task in tasks.clone() {
            let s3_client = self.s3_client.clone();
            let bucket = self.bucket.clone();
            let key = self.key.clone();
            let filepath = self.filepath.clone();
            let pb = progress_bar.clone();

            // Create a copy of the task id to avoid the borrow issue
            let task_id = task.id;
            let handle = tokio::spawn(async move {
                match download_chunk_with_progress(s3_client, &bucket, &key, task, &filepath, pb).await {
                    Ok(chunk_id) => Ok(chunk_id),
                    Err(e) => {
                        eprintln!("❌ [{}] 分片下载失败: Chunk {}: {}",
                                 std::thread::current().name().unwrap_or("unknown"), task_id, e);
                        Err(e)
                    }
                }
            });
            handles.push(handle);
        }

        // Wait for all tasks to complete
        let mut success_count = 0;
        let mut failed_chunks = Vec::new();

        for (i, handle) in handles.into_iter().enumerate() {
            match handle.await {
                Ok(Ok(chunk_id)) => {
                    success_count += 1;
                    downloaded_chunks.insert(chunk_id);

                    // Save progress every 10 completed chunks to reduce I/O
                    if success_count % 10 == 0 {
                        let _ = self.save_progress(&downloaded_chunks);
                    }
                }
                Ok(Err(_)) => {
                    failed_chunks.push(tasks[i].id);
                }
                Err(e) => {
                    eprintln!("Download task {} panicked: {}", tasks[i].id, e);
                    failed_chunks.push(tasks[i].id);
                }
            }
        }

        // Save final progress
        self.save_progress(&downloaded_chunks)?;

        progress_bar.finish_with_message(format!("下载完成: {}/{}", success_count, tasks.len()));

        if downloaded_chunks.len() as u64 == num_chunks {
            self.verify_integrity(start_time.elapsed().as_secs_f64(), false).await
        } else {
            eprintln!("[{}] 下载未完成，部分分片失败。", self.run_id);
            Ok(false)
        }
    }

    async fn verify_integrity(&self, download_duration: f64, skipped_download: bool) -> Result<bool> {
        let start_time = std::time::Instant::now();

        if self.metadata.md5.is_none() {
            println!("[{}] XML未提供MD5，跳过校验。", self.run_id);
            println!(
                "[{}] 任务完成 (无校验). 耗时: {:.2} s",
                self.run_id,
                download_duration
            );
            if self.meta_file.exists() {
                let _ = std::fs::remove_file(&self.meta_file);
            }
            return Ok(true);
        }

        println!("[{}] 正在校验 MD5 (本地计算中)...", self.run_id);
        let local_md5 = calculate_md5(&self.filepath).await?;
        let verify_duration = start_time.elapsed().as_secs_f64();
        let expected_md5 = self.metadata.md5.as_ref().unwrap();

        if local_md5 == *expected_md5 {
            let speed_info = if !skipped_download {
                let speed = (self.metadata.size as f64 / 1024.0 / 1024.0) / download_duration;
                format!(" | 速度: {:.2} MB/s", speed)
            } else {
                "".to_string()
            };

            println!(
                "[{}] ✅ 校验通过! 总耗时: {:.2}s (下载: {:.2}s{}, 校验: {:.2}s)",
                self.run_id,
                download_duration + verify_duration,
                download_duration,
                speed_info,
                verify_duration
            );

            if self.meta_file.exists() {
                let _ = std::fs::remove_file(&self.meta_file);
            }
            Ok(true)
        } else {
            eprintln!(
                "[{}] ❌ 校验失败! 本地:{} != 远程:{}",
                self.run_id, local_md5, expected_md5
            );
            Ok(false)
        }
    }
}

/// Represents the format of the progress metadata file
#[derive(Debug, Deserialize, Serialize)]
struct ProgressData {
    downloaded_chunks: Vec<usize>,
}

/// Download a single chunk of the file from S3 with progress tracking
async fn download_chunk_with_progress(
    s3_client: S3Client,
    bucket: &str,
    key: &str,
    chunk: ChunkInfo,
    filepath: &Path,
    progress_bar: ProgressBar,
) -> Result<usize> {
    let range_header = format!("bytes={}-{}", chunk.start, chunk.end);

    // Retry download up to 5 times with exponential backoff
    let mut attempt = 0;
    let max_attempts = 5;

    loop {
        attempt += 1;
        match s3_client
            .get_object()
            .bucket(bucket)
            .key(key)
            .range(range_header.clone())
            .send()
            .await
        {
            Ok(response) => {
                // Read the response body into bytes
                let body_bytes = response.body.collect().await?.into_bytes();

                // Write the chunk to the appropriate position in the file
                let file = std::fs::OpenOptions::new()
                    .write(true)
                    .create(true)  // Create if doesn't exist
                    .open(filepath)?;
                let mut file = file; // Rebind as mutable for seek/write
                file.seek(SeekFrom::Start(chunk.start))?;
                file.write_all(&body_bytes)?;

                // Update progress bar
                progress_bar.inc(body_bytes.len() as u64);

                return Ok(chunk.id);
            }
            Err(e) => {
                if attempt >= max_attempts {
                    return Err(anyhow::anyhow!("Failed to download chunk {} after {} attempts: {}", chunk.id, max_attempts, e));
                }
                eprintln!("Chunk {} download attempt {} failed: {}, retrying in 2 seconds...", chunk.id, attempt, e);
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }
    }
}

/// Calculate MD5 hash of a file
async fn calculate_md5(filepath: &Path) -> Result<String> {
    let mut file = tokio::fs::File::open(filepath).await?;
    let mut context = md5::Context::new();
    let mut buffer = [0; 8192]; // 8KB buffer

    loop {
        let bytes_read = file.read(&mut buffer).await?;
        if bytes_read == 0 {
            break; // End of file
        }
        context.write_all(&buffer[..bytes_read])?;
    }

    let result = context.compute();
    Ok(format!("{:x}", result))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_s3_uri() {
        let (bucket, key) = ResumableDownloader::parse_s3_uri("s3://my-bucket/my-key/file.txt").unwrap();
        assert_eq!(bucket, "my-bucket");
        assert_eq!(key, "my-key/file.txt");
    }
}