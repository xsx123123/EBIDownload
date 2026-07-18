//! Polariseq library

pub mod aws_s3;
pub mod deps;
pub mod ftp;
pub mod md5;
pub mod observer;
pub mod progress;
pub mod progress_store;
pub mod public_data;
pub mod upload;

use anyhow::{anyhow, Context, Result};
use gzp::{deflate::Gzip, ZBuilder};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};
use tracing::info;

// Configuration
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Config {
    pub software: SoftwarePaths,
    #[serde(default)]
    pub public_data: HashMap<String, public_data::PublicDatabase>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct SoftwarePaths {
    pub prefetch: PathBuf,
    pub fasterq_dump: PathBuf,
    pub blastdbcmd: Option<PathBuf>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct EnaRecord {
    pub run_accession: String,
    pub study_accession: Option<String>,
    pub secondary_study_accession: Option<String>,
    pub sample_accession: Option<String>,
    pub secondary_sample_accession: Option<String>,
    pub experiment_accession: Option<String>,
    pub submission_accession: Option<String>,
    pub tax_id: Option<String>,
    pub scientific_name: Option<String>,
    pub instrument_platform: Option<String>,
    pub instrument_model: Option<String>,
    pub library_name: Option<String>,
    pub nominal_length: Option<String>,
    pub library_layout: Option<String>,
    pub library_strategy: Option<String>,
    pub library_source: Option<String>,
    pub library_selection: Option<String>,
    pub read_count: Option<String>,
    pub center_name: Option<String>,
    pub first_public: Option<String>,
    pub last_updated: Option<String>,
    pub experiment_title: Option<String>,
    pub study_title: Option<String>,
    pub study_alias: Option<String>,
    pub run_alias: Option<String>,
    #[serde(default)]
    pub fastq_bytes: String,
    #[serde(default)]
    pub fastq_md5: String,
    #[serde(default)]
    pub fastq_ftp: String,
    pub fastq_aspera: Option<String>,
    pub fastq_galaxy: Option<String>,
    pub submitted_bytes: Option<String>,
    pub submitted_md5: Option<String>,
    pub submitted_ftp: Option<String>,
    pub submitted_aspera: Option<String>,
    pub submitted_galaxy: Option<String>,
    pub submitted_format: Option<String>,
    pub sra_bytes: Option<String>,
    pub sra_md5: Option<String>,
    pub sra_ftp: Option<String>,
    pub sra_aspera: Option<String>,
    pub sra_galaxy: Option<String>,
    pub sample_alias: Option<String>,
    #[serde(default)]
    pub sample_title: String,
    pub nominal_sdev: Option<String>,
    pub first_created: Option<String>,
    pub bam_ftp: Option<String>,
    pub fastq_file_role: Option<String>,
    pub submitted_file_role: Option<String>,
    pub sra_file_role: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessedRecord {
    pub run_accession: String,
    pub fastq_ftp_1_url: String,
    pub fastq_ftp_2_url: Option<String>,
    pub fastq_ftp_1_name: String,
    pub fastq_ftp_2_name: Option<String>,
    pub fastq_md5_1: String,
    pub fastq_md5_2: Option<String>,
    pub fastq_bytes_1: u64,
    pub fastq_bytes_2: Option<u64>,
    pub sample_title: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "cli", derive(clap::ValueEnum))]
pub enum DownloadMethod {
    Ftp,
    Aws,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadOptions {
    pub accession: Option<String>,
    pub tsv: Option<PathBuf>,
    pub output: PathBuf,
    pub download_method: DownloadMethod,
    pub multithreads: usize,
    pub aws_threads: usize,
    pub chunk_size: u64,
    pub prefetch_max_size: String,
    pub pe_only: bool,
    pub filter_sample: Vec<String>,
    pub filter_run: Vec<String>,
    pub exclude_sample: Vec<String>,
    pub exclude_run: Vec<String>,
    pub cleanup_sra: bool,
    pub dry_run: bool,
}

impl Default for DownloadOptions {
    fn default() -> Self {
        Self {
            accession: None,
            tsv: None,
            output: PathBuf::from("."),
            download_method: DownloadMethod::Aws,
            multithreads: 4,
            aws_threads: 8,
            chunk_size: 200,
            prefetch_max_size: "100G".to_string(),
            pe_only: false,
            filter_sample: Vec::new(),
            filter_run: Vec::new(),
            exclude_sample: Vec::new(),
            exclude_run: Vec::new(),
            cleanup_sra: false,
            dry_run: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadOptions {
    pub bucket: String,
    pub prefix: Option<String>,
    pub files: Vec<PathBuf>,
    pub region: String,
    pub concurrent: usize,
    pub apply_policy: bool,
    pub metadata_template: Option<PathBuf>,
    pub dry_run: bool,
}

impl Default for UploadOptions {
    fn default() -> Self {
        Self {
            bucket: String::new(),
            prefix: None,
            files: Vec::new(),
            region: "us-east-1".to_string(),
            concurrent: 4,
            apply_policy: false,
            metadata_template: None,
            dry_run: false,
        }
    }
}

pub fn load_config(yaml_path: &Path) -> Result<Config> {
    if !yaml_path.exists() {
        return Err(anyhow!(
            "YAML configuration file not found: {}",
            yaml_path.display()
        ));
    }
    let content = std::fs::read_to_string(yaml_path)?;
    let config: Config = serde_yaml::from_str(&content)?;
    Ok(config)
}

pub async fn fetch_ena_data(accession: &str) -> Result<Vec<EnaRecord>> {
    use csv::ReaderBuilder;

    let fields = "run_accession,study_accession,secondary_study_accession,sample_accession,secondary_sample_accession,experiment_accession,submission_accession,tax_id,scientific_name,instrument_platform,instrument_model,library_name,nominal_length,library_layout,library_strategy,library_source,library_selection,read_count,center_name,first_public,last_updated,experiment_title,study_title,study_alias,run_alias,fastq_bytes,fastq_md5,fastq_ftp,fastq_aspera,fastq_galaxy,submitted_bytes,submitted_md5,submitted_ftp,submitted_aspera,submitted_galaxy,submitted_format,sra_bytes,sra_md5,sra_ftp,sra_aspera,sra_galaxy,sample_alias,sample_title,nominal_sdev,first_created,bam_ftp,fastq_file_role,submitted_file_role,sra_file_role";
    let url = format!("https://www.ebi.ac.uk/ena/portal/api/filereport?accession={}&result=read_run&fields={}&format=tsv", accession, fields);

    let client = reqwest::Client::builder().build()?;
    let response = client.get(&url).send().await?;
    if !response.status().is_success() {
        return Err(anyhow!(
            "Failed to get response. Status code: {}",
            response.status()
        ));
    }
    let text = response.text().await?;
    let mut reader = ReaderBuilder::new()
        .has_headers(true)
        .delimiter(b'\t')
        .from_reader(text.as_bytes());
    let mut records = Vec::new();
    for result in reader.deserialize() {
        let record: EnaRecord = result?;
        records.push(record);
    }
    Ok(records)
}

pub fn read_tsv_data(tsv_path: &Path) -> Result<Vec<EnaRecord>> {
    use csv::ReaderBuilder;

    let mut reader = ReaderBuilder::new()
        .has_headers(true)
        .delimiter(b'\t')
        .from_path(tsv_path)?;
    let mut records = Vec::new();
    for result in reader.deserialize() {
        let record: EnaRecord = result?;
        records.push(record);
    }
    Ok(records)
}

pub struct RegexFilters {
    pub include_sample: Vec<Regex>,
    pub include_run: Vec<Regex>,
    pub exclude_sample: Vec<Regex>,
    pub exclude_run: Vec<Regex>,
}

impl RegexFilters {
    pub fn new(options: &DownloadOptions) -> Result<Self> {
        let include_sample = options
            .filter_sample
            .iter()
            .map(|s| Regex::new(s))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| anyhow!("Invalid regex pattern for filter_sample: {}", e))?;

        let include_run = options
            .filter_run
            .iter()
            .map(|s| Regex::new(s))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| anyhow!("Invalid regex pattern for filter_run: {}", e))?;

        let exclude_sample = options
            .exclude_sample
            .iter()
            .map(|s| Regex::new(s))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| anyhow!("Invalid regex pattern for exclude_sample: {}", e))?;

        let exclude_run = options
            .exclude_run
            .iter()
            .map(|s| Regex::new(s))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| anyhow!("Invalid regex pattern for exclude_run: {}", e))?;

        Ok(Self {
            include_sample,
            include_run,
            exclude_sample,
            exclude_run,
        })
    }

    pub fn should_include(&self, record: &EnaRecord) -> bool {
        if !self.include_sample.is_empty()
            && !self
                .include_sample
                .iter()
                .any(|r| r.is_match(&record.sample_title))
        {
            return false;
        }
        if !self.include_run.is_empty()
            && !self
                .include_run
                .iter()
                .any(|r| r.is_match(&record.run_accession))
        {
            return false;
        }
        if !self.exclude_sample.is_empty()
            && self
                .exclude_sample
                .iter()
                .any(|r| r.is_match(&record.sample_title))
        {
            return false;
        }
        if !self.exclude_run.is_empty()
            && self
                .exclude_run
                .iter()
                .any(|r| r.is_match(&record.run_accession))
        {
            return false;
        }
        true
    }
}

pub fn process_records(
    records: Vec<EnaRecord>,
    pe_only: bool,
    filters: Option<&RegexFilters>,
) -> Result<Vec<ProcessedRecord>> {
    let mut processed = Vec::new();
    for record in records {
        if let Some(f) = filters {
            if !f.should_include(&record) {
                continue;
            }
        }

        let ftp_urls: Vec<&str> = record
            .fastq_ftp
            .split(';')
            .filter(|s| !s.is_empty())
            .collect();
        let md5s: Vec<&str> = record
            .fastq_md5
            .split(';')
            .filter(|s| !s.is_empty())
            .collect();
        let sizes: Vec<u64> = record
            .fastq_bytes
            .split(';')
            .filter_map(|s| s.parse::<u64>().ok())
            .collect();

        if ftp_urls.is_empty() || md5s.is_empty() {
            continue;
        }
        if pe_only && ftp_urls.len() < 2 {
            continue;
        }

        let fastq_ftp_1_url = ftp_urls[0].to_string();
        let fastq_ftp_1_name = fastq_ftp_1_url.rsplit('/').next().unwrap_or("").to_string();
        let fastq_md5_1 = md5s[0].to_string();
        let fastq_bytes_1 = *sizes.first().unwrap_or(&0);

        let (fastq_ftp_2_url, fastq_ftp_2_name, fastq_md5_2, fastq_bytes_2) =
            if ftp_urls.len() >= 2 && md5s.len() >= 2 {
                (
                    Some(ftp_urls[1].to_string()),
                    Some(ftp_urls[1].rsplit('/').next().unwrap_or("").to_string()),
                    Some(md5s[1].to_string()),
                    sizes.get(1).copied(),
                )
            } else {
                (None, None, None, None)
            };

        processed.push(ProcessedRecord {
            run_accession: record.run_accession,
            fastq_ftp_1_url,
            fastq_ftp_2_url,
            fastq_ftp_1_name,
            fastq_ftp_2_name,
            fastq_md5_1,
            fastq_md5_2,
            fastq_bytes_1,
            fastq_bytes_2,
            sample_title: record.sample_title,
        });
    }
    Ok(processed)
}

/// Compress all FASTQ files for a given run_id in output_dir using native parallel gzip.
/// Returns the list of created .fastq.gz files. Deletes original .fastq files on success.
pub fn compress_fastq_files(
    output_dir: &Path,
    run_id: &str,
    threads: usize,
    progress_cb: Option<progress_store::CompressionProgressCallback>,
) -> Result<Vec<PathBuf>> {
    let mut compressed = Vec::new();
    let candidates = [
        format!("{}.fastq", run_id),
        format!("{}_1.fastq", run_id),
        format!("{}_2.fastq", run_id),
    ];

    for name in &candidates {
        let input_path = output_dir.join(name);
        if !input_path.exists() || input_path.metadata()?.len() == 0 {
            continue;
        }

        let output_path = output_dir.join(format!("{}.gz", name));
        info!(
            "Compressing {} -> {}",
            input_path.display(),
            output_path.display()
        );

        let input_size = input_path.metadata()?.len();
        let input = File::open(&input_path)
            .with_context(|| format!("Failed to open {}", input_path.display()))?;
        let input = BufReader::new(input);
        let output = File::create(&output_path)
            .with_context(|| format!("Failed to create {}", output_path.display()))?;

        let mut writer = ZBuilder::<Gzip, _>::new()
            .num_threads(threads)
            .from_writer(output);

        if let Some(cb) = &progress_cb {
            let mut counting = CountingReader::new(input, input_size, cb.clone());
            std::io::copy(&mut counting, &mut writer)
                .with_context(|| format!("Failed to compress {}", input_path.display()))?;
        } else {
            let mut input = input;
            std::io::copy(&mut input, &mut writer)
                .with_context(|| format!("Failed to compress {}", input_path.display()))?;
        }
        writer
            .finish()
            .with_context(|| format!("Failed to finalize {}", output_path.display()))?;

        std::fs::remove_file(&input_path)
            .with_context(|| format!("Failed to remove original {}", input_path.display()))?;

        compressed.push(output_path);
    }

    Ok(compressed)
}

struct CountingReader<R: std::io::Read> {
    inner: R,
    bytes_read: u64,
    total: u64,
    callback: progress_store::CompressionProgressCallback,
}

impl<R: std::io::Read> CountingReader<R> {
    fn new(inner: R, total: u64, callback: progress_store::CompressionProgressCallback) -> Self {
        Self {
            inner,
            bytes_read: 0,
            total,
            callback,
        }
    }
}

impl<R: std::io::Read> std::io::Read for CountingReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.bytes_read += n as u64;
        (self.callback)(self.bytes_read, self.total);
        Ok(n)
    }
}

/// Generate md5.txt in md5sum-compatible format: "<md5>  <filename>\n"
pub fn generate_md5sum_file(output_dir: &Path, files: &[PathBuf]) -> Result<PathBuf> {
    generate_md5sum_file_at(&output_dir.join("md5.txt"), files)
}

/// Generate an md5sum-compatible manifest at the requested path.
pub fn generate_md5sum_file_at(md5_path: &Path, files: &[PathBuf]) -> Result<PathBuf> {
    let mut file = File::create(md5_path)?;

    for path in files {
        let mut f = File::open(path)?;
        let mut ctx = ::md5::Context::new();
        let mut buf = vec![0u8; 1024 * 1024];
        loop {
            let n = f.read(&mut buf)?;
            if n == 0 {
                break;
            }
            ctx.consume(&buf[..n]);
        }
        let hash = format!("{:x}", ctx.compute());
        let filename = path.file_name().unwrap().to_string_lossy();
        writeln!(file, "{}  {}", hash, filename)?;
    }

    info!("MD5 manifest generated: {}", md5_path.display());
    Ok(md5_path.to_path_buf())
}

pub fn validate_config(config: &Config, method: DownloadMethod) -> Result<()> {
    match method {
        DownloadMethod::Aws => {
            check_executable(&config.software.fasterq_dump, "fasterq-dump")?;
        }
        DownloadMethod::Ftp => {}
    }
    Ok(())
}

fn check_executable(path: &Path, name: &str) -> Result<()> {
    if !path.exists() {
        return Err(anyhow::anyhow!(
            "{} not found at configured path: {}",
            name,
            path.display()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_compress_fastq_files() {
        let tmp = tempfile::tempdir().unwrap();
        let run_id = "SRR000001";

        let fq1 = tmp.path().join(format!("{}_1.fastq", run_id));
        let fq2 = tmp.path().join(format!("{}_2.fastq", run_id));
        let mut f1 = File::create(&fq1).unwrap();
        writeln!(f1, "@read1/1").unwrap();
        writeln!(f1, "ACGTACGT").unwrap();
        writeln!(f1, "+").unwrap();
        writeln!(f1, "!!!!!!!!").unwrap();
        let mut f2 = File::create(&fq2).unwrap();
        writeln!(f2, "@read1/2").unwrap();
        writeln!(f2, "TGCATGCA").unwrap();
        writeln!(f2, "+").unwrap();
        writeln!(f2, "!!!!!!!!").unwrap();

        let compressed = compress_fastq_files(tmp.path(), run_id, 2, None).unwrap();
        assert_eq!(compressed.len(), 2);

        assert!(tmp.path().join(format!("{}_1.fastq.gz", run_id)).exists());
        assert!(tmp.path().join(format!("{}_2.fastq.gz", run_id)).exists());
        assert!(!fq1.exists());
        assert!(!fq2.exists());

        // Verify gzip validity by decompressing the first file
        let gz1 = File::open(&compressed[0]).unwrap();
        let mut decoder = flate2::read::GzDecoder::new(gz1);
        let mut contents = String::new();
        std::io::Read::read_to_string(&mut decoder, &mut contents).unwrap();
        assert!(contents.contains("@read1/1"));
        assert!(contents.contains("ACGTACGT"));
    }
}
