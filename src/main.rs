use anyhow::{anyhow, Context, Result};
use chrono::Local;
use clap::Parser;
use csv::ReaderBuilder;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use tokio::process::Command;
use tokio::sync::Semaphore;
use tracing::{info, warn, error};
use tracing_subscriber::{fmt, EnvFilter};
use std::time::Duration;

mod aws_s3;
mod ftp;
mod prefetch;

const VERSION: &str = "1.3.2";
const SCRIPT_NAME: &str = "EBIDownload";

#[derive(Parser, Debug)]
#[command(author, version = VERSION, about = "Download EMBL-ENA sequencing data", long_about = None)]
struct Args {
    #[arg(short = 'A', long)]
    accession: Option<String>,
    #[arg(short = 'T', long)]
    tsv: Option<PathBuf>,
    #[arg(short, long)]
    output: PathBuf,
    #[arg(short = 'p', long, default_value = "4", help = "File-level concurrency: Number of files downloaded simultaneously")]
    multithreads: usize,
    #[arg(short, long, default_value = "aws")]
    download: DownloadMethod,
    #[arg(short = 'O', long, default_value = "false")]
    only_scripts: bool,
    #[arg(short, long, default_value = "EBIDownload.yaml")]
    yaml: PathBuf,
    #[arg(long, default_value = "info")]
    log_level: String,
    #[arg(long = "filter-sample")]
    filter_sample: Option<String>,
    #[arg(long = "filter-run")]
    filter_run: Option<String>,
    #[arg(long = "exclude-sample")]
    exclude_sample: Option<String>,
    #[arg(long = "exclude-run")]
    exclude_run: Option<String>,
    #[arg(short = 't', long = "aws-threads", default_value = "8", help = "AWS/Prefetch only: Threads for internal chunk download or conversion per file")]
    aws_threads: usize,
    #[arg(long = "chunk-size", default_value = "20", help = "AWS only: Chunk size (MB)")]
    chunk_size: u64,
    #[arg(long = "max-size", default_value = "100G", help = "Prefetch only: Max download size limit (e.g., 100G, 50G)")]
    prefetch_max_size: String,
    #[arg(long = "pe-only", default_value = "false", help = "Only download Paired-End data, ignore Single-End data")]
    pe_only: bool,
}

#[derive(Debug, Clone, clap::ValueEnum)]
enum DownloadMethod {
    Ascp,
    Ftp,
    Prefetch,
    Aws,
}

// Must be pub for submodules
#[derive(Debug, Deserialize)]
pub struct Config {
    #[allow(dead_code)]
    pub software: SoftwarePaths,
    pub setting: SettingPaths,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct SoftwarePaths {
    pub ascp: PathBuf,
    pub prefetch: PathBuf,
    pub fasterq_dump: PathBuf,
}

#[derive(Debug, Deserialize)]
pub struct SettingPaths {
    pub openssh: PathBuf,
}

#[derive(Debug, Deserialize, Serialize)]
struct EnaRecord {
    run_accession: String,
    fastq_ftp: String,
    fastq_md5: String,
    #[serde(default)]
    fastq_bytes: String, // Byte size string returned by ENA API (semicolon separated)
    sample_title: String,
}

// Must be pub
#[derive(Debug)]
pub struct ProcessedRecord {
    pub run_accession: String,
    pub fastq_ftp_1_url: String,
    pub fastq_ftp_2_url: Option<String>,
    pub fastq_ftp_1_name: String,
    pub fastq_ftp_2_name: Option<String>,
    pub fastq_md5_1: String,
    pub fastq_md5_2: Option<String>,
    // 🟢 Added: Store parsed file size
    pub fastq_bytes_1: u64,
    pub fastq_bytes_2: Option<u64>,
    pub sample_title: String,
}

struct RegexFilters {
    include_sample: Option<Regex>,
    include_run: Option<Regex>,
    exclude_sample: Option<Regex>,
    exclude_run: Option<Regex>,
}

impl RegexFilters {
    fn new(args: &Args) -> Result<Self> {
        Ok(Self {
            include_sample: args.filter_sample.as_deref().map(Regex::new).transpose().context("Invalid regex pattern for --filter-sample")?,
            include_run: args.filter_run.as_deref().map(Regex::new).transpose().context("Invalid regex pattern for --filter-run")?,
            exclude_sample: args.exclude_sample.as_deref().map(Regex::new).transpose().context("Invalid regex pattern for --exclude-sample")?,
            exclude_run: args.exclude_run.as_deref().map(Regex::new).transpose().context("Invalid regex pattern for --exclude-run")?,
        })
    }

    fn should_include(&self, record: &EnaRecord) -> bool {
        if let Some(ref regex) = self.include_sample { if !regex.is_match(&record.sample_title) { return false; } }
        if let Some(ref regex) = self.include_run { if !regex.is_match(&record.run_accession) { return false; } }
        if let Some(ref regex) = self.exclude_sample { if regex.is_match(&record.sample_title) { return false; } }
        if let Some(ref regex) = self.exclude_run { if regex.is_match(&record.run_accession) { return false; } }
        true
    }
}

// Network health check
async fn check_network_health() {
    info!("🏥 Performing network connectivity check...");
    let targets = vec![
        ("https://www.ebi.ac.uk", "EBI API"),
        ("https://eutils.ncbi.nlm.nih.gov", "NCBI API"),
        ("https://s3.amazonaws.com", "AWS S3 Endpoint"),
    ];
    let client = match reqwest::Client::builder().timeout(Duration::from_secs(3)).build() {
        Ok(c) => c,
        Err(e) => { warn!("⚠️  Failed to initialize network checker: {}", e); return; }
    };
    for (url, name) in targets {
        match client.head(url).send().await {
            Ok(_) => { info!("   ✅ {} is reachable.", name); }
            Err(e) => {
                warn!("   ⚠️  {} is NOT reachable! ({})", name, e);
                if e.is_connect() || e.is_timeout() {
                    warn!("      👉 Hint: Check DNS (/etc/resolv.conf) or Proxy (export https_proxy=...).");
                }
            }
        }
    }
    info!("🏥 Network check finished. Proceeding...");
}

#[tokio::main]
async fn main() {
    let result: Result<()> = async {
        let args = Args::parse();
        setup_logging(&args.log_level)?;
        print_banner();
        check_network_health().await;
        check_pigz_dependency().context("pigz dependency check failed")?;

        let filters = RegexFilters::new(&args)?;
        let config = load_config(&args.yaml).context("Failed to load YAML configuration")?;

        fs::create_dir_all(&args.output).context("Failed to create output directory")?;
        info!("📁 Output directory: {}", args.output.display());

        let records = if let Some(accession) = &args.accession {
            fetch_ena_data(accession).await?
        } else if let Some(tsv_path) = &args.tsv {
            read_tsv_data(tsv_path)?
        } else {
            return Err(anyhow!("Either --accession or --tsv must be provided"));
        };

        info!("📊 Total records fetched: {}", records.len());
        let filtered_records = apply_filters(records, &filters)?;
        info!("✅ Records after filtering: {}", filtered_records.len());
        
        if filtered_records.is_empty() {
            warn!("⚠️  No records match the filter criteria. Exiting.");
            return Ok(());
        }

        let processed = process_records(filtered_records, &args)?;
        save_md5_files(&processed)?;

        match args.download {
            DownloadMethod::Ascp => {
                check_ascp_config(&config)?;
                download_with_ascp(&processed, &config, &args).await?;
            }
            DownloadMethod::Ftp => {
                // 🟢 Fix: &config parameter added here
                download_with_ftp(&processed, &config, &args).await?;
            }
            DownloadMethod::Prefetch => {
                check_prefetch_config(&config)?;
                warn!("================================[ Prefetch Mode ]================================");
                warn!("Using SRA Toolkit 'prefetch' -> 'fasterq-dump' -> 'pigz'");
                warn!("Ensure 'prefetch' and 'fasterq-dump' paths in YAML are correct.");
                warn!("=================================================================================");
                download_with_prefetch(&processed, &config, &args).await?;
            }
            DownloadMethod::Aws => {
                download_with_aws(&processed, &config, &args).await?;
            }
        }

        info!("🎉 {} download completed successfully!", SCRIPT_NAME);
        Ok(())
    }
    .await;

    if let Err(e) = result {
        tracing::error!("Application failed: {:?}", e);
        eprintln!("\n❌ An error occurred. Please check the log file for detailed error information.");
        std::process::exit(1);
    }
}

fn print_banner() {
    println!("\n{}", "=".repeat(60));
    println!("  🧬 {} - EMBL-ENA Data Downloader v{}", SCRIPT_NAME, VERSION);
    println!("{}\n", "=".repeat(60));
}

fn setup_logging(log_level: &str) -> Result<()> {
    use tracing_subscriber::{layer::SubscriberExt, Layer};
    struct LocalTimer;
    impl fmt::time::FormatTime for LocalTimer {
        fn format_time(&self, w: &mut fmt::format::Writer<'_>) -> std::fmt::Result {
            write!(w, "{}", Local::now().format("%Y-%m-%d %H:%M:%S"))
        }
    }
    let timestamp = Local::now().format("%Y-%m-%d_%H-%M-%S");
    let log_file = format!("{}_EMBI-ENA_Download_{}.log", SCRIPT_NAME, timestamp);
    let file = File::create(&log_file)?;
    let file_layer = fmt::layer().with_writer(file).with_ansi(false).with_target(true).with_thread_ids(true).with_timer(fmt::time::LocalTime::rfc_3339()).with_filter(EnvFilter::new("debug"));
    let stdout_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(log_level));
    let stdout_layer = fmt::layer().with_writer(std::io::stdout).with_ansi(true).with_target(false).with_thread_ids(false).with_timer(LocalTimer).compact().with_filter(stdout_filter);
    let subscriber = tracing_subscriber::registry().with(file_layer).with(stdout_layer);
    tracing::subscriber::set_global_default(subscriber).context("Failed to set subscriber")?;
    info!("📝 Log file created: {}", log_file);
    Ok(())
}

fn load_config(yaml_path: &Path) -> Result<Config> {
    if !yaml_path.exists() { return Err(anyhow!("YAML configuration file not found: {}", yaml_path.display())); }
    info!("⚙️  Loading configuration from: {}", yaml_path.display());
    let content = fs::read_to_string(yaml_path)?;
    let config: Config = serde_yaml::from_str(&content)?;
    info!("✅ Configuration loaded successfully");
    Ok(config)
}

async fn fetch_ena_data(accession: &str) -> Result<Vec<EnaRecord>> {
    let url = format!("https://www.ebi.ac.uk/ena/portal/api/filereport?accession={}&result=read_run&fields=run_accession,fastq_ftp,fastq_md5,fastq_bytes,sample_title&format=tsv", accession);
    info!("🌐 Fetching data from ENA API for: {}", accession);
    
    // Fix for DNS issues without Root: Hardcode EBI IP
    let client = reqwest::Client::builder()
        .resolve("www.ebi.ac.uk", "193.62.193.80:0".parse().unwrap()) 
        .build()?;
        
    let response = client.get(&url).send().await.context("Failed to fetch data from ENA API")?;
    if !response.status().is_success() { return Err(anyhow!("Failed to get response. Status code: {}", response.status())); }
    let text = response.text().await?;
    let mut reader = ReaderBuilder::new().has_headers(true).delimiter(b'\t').from_reader(text.as_bytes());
    let mut records = Vec::new();
    for result in reader.deserialize() { let record: EnaRecord = result?; records.push(record); }
    info!("✅ Fetched {} records from ENA", records.len());
    Ok(records)
}

fn read_tsv_data(tsv_path: &Path) -> Result<Vec<EnaRecord>> {
    info!("📄 Reading TSV file: {}", tsv_path.display());
    let mut reader = ReaderBuilder::new().has_headers(true).delimiter(b'\t').from_path(tsv_path)?;
    let mut records = Vec::new();
    for result in reader.deserialize() { let record: EnaRecord = result?; records.push(record); }
    info!("✅ Read {} records from TSV", records.len());
    Ok(records)
}

fn apply_filters(records: Vec<EnaRecord>, filters: &RegexFilters) -> Result<Vec<EnaRecord>> {
    let mut filtered = Vec::new();
    let mut filtered_count = 0;
    for record in records {
        if filters.should_include(&record) { filtered.push(record); } else { filtered_count += 1; }
    }
    if filtered_count > 0 { info!("🔍 Filtered out {} records based on regex patterns", filtered_count); }
    Ok(filtered)
}

// 🟢 Modified ProcessedRecord generation logic, parse file size
fn process_records(records: Vec<EnaRecord>, args: &Args) -> Result<Vec<ProcessedRecord>> {
    info!("⚙️  Processing records...");
    let mut processed = Vec::new();
    for record in records {
        let ftp_urls: Vec<&str> = record.fastq_ftp.split(';').filter(|s| !s.is_empty()).collect();
        let md5s: Vec<&str> = record.fastq_md5.split(';').filter(|s| !s.is_empty()).collect();
        // 🟢 Parse file size
        let sizes: Vec<u64> = record.fastq_bytes.split(';')
            .filter_map(|s| s.parse::<u64>().ok())
            .collect();

        if ftp_urls.is_empty() || md5s.is_empty() {
            warn!("⚠️  Skipping invalid record (no files): {}", record.run_accession);
            continue;
        }
        if args.pe_only && ftp_urls.len() < 2 {
            warn!("⚠️  Skipping Single-End record (--pe-only active): {}", record.run_accession);
            continue;
        }

        let ftp_1_url = ftp_urls[0].to_string();
        let ftp_1_name = ftp_1_url.rsplit('/').next().unwrap_or("").to_string();
        let md5_1 = md5s[0].to_string();
        let size_1 = *sizes.get(0).unwrap_or(&0);

        let (ftp_2_url, ftp_2_name, md5_2, size_2) = if ftp_urls.len() >= 2 && md5s.len() >= 2 {
            (
                Some(ftp_urls[1].to_string()), 
                Some(ftp_urls[1].rsplit('/').next().unwrap_or("").to_string()), 
                Some(md5s[1].to_string()),
                sizes.get(1).copied()
            )
        } else {
            (None, None, None, None)
        };

        processed.push(ProcessedRecord {
            run_accession: record.run_accession,
            fastq_ftp_1_url: ftp_1_url,
            fastq_ftp_2_url: ftp_2_url,
            fastq_ftp_1_name: ftp_1_name,
            fastq_ftp_2_name: ftp_2_name,
            fastq_md5_1: md5_1,
            fastq_md5_2: md5_2,
            // 🟢 Assign size
            fastq_bytes_1: size_1,
            fastq_bytes_2: size_2,
            sample_title: record.sample_title,
        });
    }
    info!("✅ Processed {} records", processed.len());
    Ok(processed)
}

fn save_md5_files(records: &[ProcessedRecord]) -> Result<()> {
    info!("💾 Saving MD5 files...");
    let mut r1_file = File::create("R1_fastq_md5.tsv")?;
    let mut r2_file = File::create("R2_fastq_md5.tsv")?;
    for record in records {
        writeln!(r1_file, "{}\t{}\t{}", record.fastq_md5_1, record.fastq_ftp_1_name, record.sample_title)?;
        if let (Some(md5), Some(name)) = (&record.fastq_md5_2, &record.fastq_ftp_2_name) {
             writeln!(r2_file, "{}\t{}\t{}", md5, name, record.sample_title)?;
        }
    }
    info!("✅ MD5 files saved");
    Ok(())
}

// Must be pub for submodules
pub fn create_script(output_path: &Path, fastq_id: &str, command: &str) -> Result<PathBuf> {
    let scripts_dir = output_path.join("scripts");
    fs::create_dir_all(&scripts_dir)?;
    let script_path = scripts_dir.join(format!("{}.sh", fastq_id));
    let mut file = File::create(&script_path)?;
    writeln!(file, "#!/usr/bin/env bash")?;
    writeln!(file, "set -euo pipefail")?;
    writeln!(file, "mkdir -p {}", output_path.display())?;
    writeln!(file, "cd {}", output_path.display())?;
    writeln!(file, "{}", command)?;
    #[cfg(unix)] {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&script_path)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script_path, perms)?;
    }
    Ok(script_path)
}

// Helper: Execute Shell command with error echo
async fn run_command(cmd: &str, dir: &Path) -> Result<()> {
    info!("   Step: {}", cmd);
    let output = Command::new("bash").arg("-c").arg(cmd).current_dir(dir).stdout(Stdio::null()).stderr(Stdio::piped()).output().await?;
    if output.status.success() { Ok(()) } else { let stderr = String::from_utf8_lossy(&output.stderr); error!("❌ Command failed: {}\nError Output:\n{}", cmd, stderr); Err(anyhow::anyhow!("Command failed")) }
}

// Prefetch Entry
async fn download_with_prefetch(records: &[ProcessedRecord], config: &Config, args: &Args) -> Result<()> {
    prefetch::download_all(records, config, &args.output, args.multithreads, args.aws_threads,&args.prefetch_max_size,args.only_scripts).await
}

// AWS Entry (Keep original logic)
async fn download_with_aws(records: &[ProcessedRecord], config: &Config, args: &Args) -> Result<()> {
    info!("☁️  Starting AWS S3 downloads...");

    let file_concurrency = args.multithreads;
    let chunk_concurrency = args.aws_threads;
    let process_threads = if args.aws_threads > 4 { args.aws_threads } else { 4 }; 
    let chunk_size_mb = args.chunk_size;

    info!("⚙️  Config: Parallel Files = {}, Threads/File = {}, Chunk Size = {}MB", file_concurrency, chunk_concurrency, chunk_size_mb);

    let semaphore = Arc::new(Semaphore::new(file_concurrency));
    let mut handles = Vec::new();

    let fasterq_dump_path = config.software.fasterq_dump.display().to_string();
    let pigz_path = "pigz"; 

    for record in records {
        let run_id = record.run_accession.clone();
        let output_dir = args.output.clone();
        let sem = semaphore.clone();
        let max_workers = chunk_concurrency;
        let chunk_size = chunk_size_mb;
        let fasterq_dump = fasterq_dump_path.clone();
        let pigz = pigz_path.to_string();
        let only_scripts = args.only_scripts;

        let handle = tokio::spawn(async move {
            let _permit = sem.acquire().await.expect("semaphore closed");
            
            info!("📥 [{}] Step 1: Downloading via AWS S3...", run_id);
            let metadata = aws_s3::SraUtils::get_metadata(&run_id, None).await?;
            let sra_filename = format!("{}.sra", run_id);
            
            if let Some(sra_metadata) = metadata {
                let downloader = aws_s3::ResumableDownloader::new(
                    run_id.clone(),
                    sra_metadata,
                    output_dir.clone(),
                    chunk_size, 
                    max_workers,
                ).await?;

                if !only_scripts {
                    let success = downloader.start().await?;
                    if !success {
                        return Err(anyhow::anyhow!("Download failed for {}", run_id));
                    }
                }
            } else {
                warn!("❌ [{}] No AWS S3 URI found", run_id);
                return Err(anyhow::anyhow!("No S3 URI for {}", run_id));
            }

            let cmd_convert = format!("{} --split-3 -e {} -O . {} -f", fasterq_dump, process_threads, sra_filename);
            let cmd_compress = format!("{} -p {} {}*.fastq", pigz, process_threads, run_id);

            if only_scripts {
                let full_script = format!("{}\n{}", cmd_convert, cmd_compress);
                create_script(&output_dir, &run_id, &full_script)?;
                info!("📝 [{}] Script generated", run_id);
                return Ok(());
            }

            // Smart check: If FASTQ file exists and is not empty, skip conversion
            let fq_1 = output_dir.join(format!("{}_1.fastq", run_id));
            let fq_single = output_dir.join(format!("{}.fastq", run_id));
            let fq_exists = (fq_1.exists() && fq_1.metadata().map(|m| m.len() > 0).unwrap_or(false)) || 
                            (fq_single.exists() && fq_single.metadata().map(|m| m.len() > 0).unwrap_or(false));

            if fq_exists {
                info!("⏩ [{}] FASTQ files already exist, skipping conversion.", run_id);
            } else {
                info!("🔄 [{}] Step 2: Converting (fasterq-dump)...", run_id);
                let result = run_command(&cmd_convert, &output_dir).await;
                if let Err(e) = result {
                    warn!("⚠️ [{}] fasterq-dump reported error: {}. Checking if output exists...", run_id, e);
                }
            }

            // Fault-tolerant compression
            if (fq_1.exists() && fq_1.metadata().map(|m| m.len() > 0).unwrap_or(false)) || 
               (fq_single.exists() && fq_single.metadata().map(|m| m.len() > 0).unwrap_or(false)) {
                
                info!("📦 [{}] Step 3: Compressing (pigz)...", run_id);
                run_command(&cmd_compress, &output_dir).await.context("pigz failed")?;
                info!("✅ [{}] All steps completed successfully!", run_id);
                Ok(())
            } else {
                error!("❌ [{}] Conversion failed and no FASTQ output found.", run_id);
                Err(anyhow::anyhow!("Conversion failed for {}", run_id))
            }
        });

        handles.push(handle);
    }

    for handle in handles {
        if let Err(e) = handle.await { warn!("Task error: {}", e); }
    }
    info!("🎉 All AWS S3 tasks completed");
    Ok(())
}

// FTP Entry
async fn download_with_ftp(records: &[ProcessedRecord], config: &Config, args: &Args) -> Result<()> {
    // 🟢 Call ftp.rs, pass file size to enable percentage progress bar
    ftp::process_downloads(
        records, 
        config, 
        &args.output, 
        ftp::Protocol::Ftp, 
        args.multithreads, 
        args.only_scripts
    ).await
}

// Aspera Entry
async fn download_with_ascp(records: &[ProcessedRecord], config: &Config, args: &Args) -> Result<()> {
    ftp::process_downloads(
        records, 
        config, 
        &args.output, 
        ftp::Protocol::Ascp, 
        args.multithreads, 
        args.only_scripts
    ).await
}
fn check_prefetch_config(_config: &Config) -> Result<()> { Ok(()) }
fn check_ascp_config(_config: &Config) -> Result<()> { Ok(()) }
fn check_pigz_dependency() -> Result<()> { Ok(()) }