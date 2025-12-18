use anyhow::{anyhow, Context, Result};
use chrono::Local;
use clap::Parser;
use csv::ReaderBuilder;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use tokio::process::Command;
use tokio::sync::{mpsc, Semaphore};
use tracing::{info, warn};
use tracing_subscriber::{fmt, EnvFilter};

mod aws_s3;

const VERSION: &str = "1.2.6"; 
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
    #[arg(short = 'p', long, default_value = "4", help = "文件级并发数：同时下载的文件数量")]
    multithreads: usize,
    #[arg(short, long, default_value = "prefetch")]
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
    #[arg(short = 't', long = "aws-threads", default_value = "8", help = "AWS专用：单文件内部分片下载线程数")]
    aws_threads: usize,
    #[arg(long = "chunk-size", default_value = "20", help = "AWS专用：分片大小 (MB)")]
    chunk_size: u64,
    #[arg(long = "pe-only", default_value = "false", help = "仅下载双端测序(Paired-End)数据，忽略单端数据")]
    pe_only: bool,
}

#[derive(Debug, Clone, clap::ValueEnum)]
enum DownloadMethod {
    Ascp,
    Ftp,
    Prefetch,
    Aws,
}

#[derive(Debug, Deserialize)]
struct Config {
    #[allow(dead_code)] 
    software: SoftwarePaths,
    setting: SettingPaths,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)] 
struct SoftwarePaths {
    ascp: PathBuf,
    prefetch: PathBuf,
    fasterq_dump: PathBuf,
}

#[derive(Debug, Deserialize)]
struct SettingPaths {
    openssh: PathBuf,
}

#[derive(Debug, Deserialize, Serialize)]
struct EnaRecord {
    run_accession: String,
    fastq_ftp: String,
    fastq_md5: String,
    #[serde(default)]
    fastq_bytes: String,
    sample_title: String,
}

#[derive(Debug)]
struct ProcessedRecord {
    run_accession: String,
    fastq_ftp_1_url: String,
    fastq_ftp_2_url: Option<String>, 
    fastq_ftp_1_name: String,
    fastq_ftp_2_name: Option<String>, 
    fastq_md5_1: String,
    fastq_md5_2: Option<String>,      
    sample_title: String,
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

#[derive(Debug, Clone)]
enum DlEvent {
    Stage { run: String, msg: String, pct: u64 },
    Done { run: String },
    Fail { run: String, err: String },
}

#[tokio::main]
async fn main() {
    let result: Result<()> = async {
        let args = Args::parse();
        setup_logging(&args.log_level)?;
        print_banner();

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
                download_with_ftp(&processed, &args).await?;
            }
            DownloadMethod::Prefetch => {
                check_prefetch_config(&config)?;
                warn!("================================[ MD5 Mismatch Warning ]================================");
                warn!("You have selected the 'prefetch' download method.");
                warn!("Please note: FASTQ files from the SRA database may have different headers/MD5s");
                warn!("than EBI files. This is expected.");
                warn!("========================================================================================");
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
    let timestamp = Local::now().format("%Y-%m-%d_%H-%M-%S");
    let log_file = format!("{}_EMBI-ENA_Download_{}.log", SCRIPT_NAME, timestamp);
    let file = File::create(&log_file)?;
    let file_layer = fmt::layer().with_writer(file).with_ansi(false).with_target(true).with_thread_ids(true).with_timer(fmt::time::LocalTime::rfc_3339()).with_filter(EnvFilter::new("debug"));
    let stdout_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(log_level));
    let stdout_layer = fmt::layer().with_writer(std::io::stdout).with_ansi(true).with_target(false).with_thread_ids(false).with_timer(fmt::time::LocalTime::rfc_3339()).compact().with_filter(stdout_filter);
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
    let response = reqwest::get(&url).await.context("Failed to fetch data from ENA API")?;
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

fn process_records(records: Vec<EnaRecord>, args: &Args) -> Result<Vec<ProcessedRecord>> {
    info!("⚙️  Processing records...");
    let mut processed = Vec::new();
    for record in records {
        let ftp_urls: Vec<&str> = record.fastq_ftp.split(';').filter(|s| !s.is_empty()).collect();
        let md5s: Vec<&str> = record.fastq_md5.split(';').filter(|s| !s.is_empty()).collect();
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
        let (ftp_2_url, ftp_2_name, md5_2) = if ftp_urls.len() >= 2 && md5s.len() >= 2 {
            (Some(ftp_urls[1].to_string()), Some(ftp_urls[1].rsplit('/').next().unwrap_or("").to_string()), Some(md5s[1].to_string()))
        } else {
            (None, None, None)
        };
        processed.push(ProcessedRecord {
            run_accession: record.run_accession,
            fastq_ftp_1_url: ftp_1_url,
            fastq_ftp_2_url: ftp_2_url,
            fastq_ftp_1_name: ftp_1_name,
            fastq_ftp_2_name: ftp_2_name,
            fastq_md5_1: md5_1,
            fastq_md5_2: md5_2,
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

fn create_script(output_path: &Path, fastq_id: &str, command: &str) -> Result<PathBuf> {
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

async fn download_with_ftp(records: &[ProcessedRecord], args: &Args) -> Result<()> {
    info!("🌐 Starting FTP downloads with {} threads...", args.multithreads);
    let mut urls = Vec::new();
    for record in records {
        urls.push(record.fastq_ftp_1_url.clone());
        if let Some(u2) = &record.fastq_ftp_2_url { urls.push(u2.clone()); }
    }
    run_generic_download(urls, args, |url, _output_path| format!("wget -c {}", url)).await
}

async fn download_with_ascp(records: &[ProcessedRecord], config: &Config, args: &Args) -> Result<()> {
    info!("🚀 Starting Aspera downloads with {} threads...", args.multithreads);
    let mut urls = Vec::new();
    for record in records {
        let u1 = record.fastq_ftp_1_url.replace("ftp.sra.ebi.ac.uk", "era-fasp@fasp.sra.ebi.ac.uk:");
        urls.push(u1);
        if let Some(ftp2) = &record.fastq_ftp_2_url {
             let u2 = ftp2.replace("ftp.sra.ebi.ac.uk", "era-fasp@fasp.sra.ebi.ac.uk:");
             urls.push(u2);
        }
    }
    let ascp_bin = config.software.ascp.display().to_string();
    let ssh_key = config.setting.openssh.display().to_string();
    run_generic_download(urls, args, move |url, output_path| {
         format!("{} -QT -k2 -l 800m -P33001 -i {} {} {}", ascp_bin, ssh_key, url, output_path.display())
    }).await
}

async fn run_generic_download<F>(urls: Vec<String>, args: &Args, cmd_builder: F) -> Result<()> 
where F: Fn(&str, &Path) -> String + Send + Sync + 'static + Clone
{
    let semaphore = Arc::new(Semaphore::new(args.multithreads));
    let mp = Arc::new(MultiProgress::new());
    let mut bars: HashMap<String, ProgressBar> = HashMap::new();
    for url in &urls {
        let run = url.rsplit('/').next().unwrap_or("unknown").to_string();
        let pb = mp.add(ProgressBar::new(100));
        pb.set_style(ProgressStyle::with_template("[{prefix}] {spinner} {bar:40.cyan/blue} {percent:>3}% {msg}")?.progress_chars("##-"));
        pb.set_prefix(run.clone());
        bars.insert(run, pb);
    }
    let (tx, mut rx) = mpsc::channel::<DlEvent>(1024);
    let mp_render = mp.clone();
    let mut bars_render = bars.clone();
    let render = tokio::spawn(async move {
        while let Some(ev) = rx.recv().await {
            match ev {
                DlEvent::Stage { run, msg, pct } => { if let Some(pb) = bars_render.get(&run) { pb.set_position(pct.min(100)); pb.set_message(msg); } }
                DlEvent::Done { run } => { if let Some(pb) = bars_render.remove(&run) { pb.finish_with_message("✅ Done"); } }
                DlEvent::Fail { run, err } => { if let Some(pb) = bars_render.remove(&run) { pb.finish_with_message(format!("❌ Failed: {}", err)); } }
            }
        }
        let _ = mp_render.clear();
    });
    
    let mut tasks = Vec::new();
    for url in urls {
        let sem = semaphore.clone();
        let tx = tx.clone();
        let output_path = args.output.clone();
        let only_scripts = args.only_scripts;
        let run_id = url.rsplit('/').next().unwrap_or("unknown").to_string();
        let cmd = cmd_builder(&url, &output_path);

        tasks.push(tokio::spawn(async move {
            let _permit = sem.acquire().await.expect("semaphore closed");
            let _ = tx.send(DlEvent::Stage { run: run_id.clone(), msg: "Queued".into(), pct: 0 }).await;
            if only_scripts {
                let _ = create_script(&output_path, &run_id, &cmd);
                let _ = tx.send(DlEvent::Done { run: run_id.clone() }).await;
                return;
            }
            let _ = tx.send(DlEvent::Stage { run: run_id.clone(), msg: "Running".into(), pct: 50 }).await;
            let spawn = Command::new("bash").arg("-lc").arg(&cmd).current_dir(&output_path).stdout(Stdio::null()).stderr(Stdio::piped()).spawn();
            match spawn {
                Ok(child) => {
                    let out = child.wait_with_output().await;
                    match out {
                        Ok(o) if o.status.success() => { let _ = tx.send(DlEvent::Done { run: run_id.clone() }).await; }
                        Ok(o) => { let _ = tx.send(DlEvent::Fail { run: run_id.clone(), err: format!("Exit {}", o.status) }).await; }
                        Err(e) => { let _ = tx.send(DlEvent::Fail { run: run_id.clone(), err: e.to_string() }).await; }
                    }
                }
                Err(e) => { let _ = tx.send(DlEvent::Fail { run: run_id.clone(), err: e.to_string() }).await; }
            }
        }));
    }
    drop(tx);
    for t in tasks { t.await?; }
    render.await?;
    Ok(())
}

async fn download_with_prefetch(_records: &[ProcessedRecord], _config: &Config, args: &Args) -> Result<()> {
    info!("📦 Starting prefetch downloads with {} threads...", args.multithreads);
    Ok(())
}
fn check_prefetch_config(_config: &Config) -> Result<()> { Ok(()) }
fn check_ascp_config(_config: &Config) -> Result<()> { Ok(()) }
fn check_pigz_dependency() -> Result<()> { Ok(()) }

// 辅助函数：执行 Shell 命令
async fn run_command(cmd: &str, dir: &Path) -> Result<()> {
    info!("   Step: {}", cmd);
    let status = Command::new("bash")
        .arg("-c")
        .arg(cmd)
        .current_dir(dir)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .status()
        .await?;

    if status.success() { Ok(()) } else { Err(anyhow::anyhow!("Command failed: {}", cmd)) }
}

async fn download_with_aws(records: &[ProcessedRecord], config: &Config, args: &Args) -> Result<()> {
    info!("☁️  Starting AWS S3 downloads...");

    let file_concurrency = args.multithreads;
    let chunk_concurrency = args.aws_threads;
    // 转换压缩线程数：建议稍微低一点，避免 CPU 爆炸，这里简单复用 aws_threads 或设为 4
    let process_threads = if args.aws_threads > 4 { args.aws_threads } else { 4 }; 
    let chunk_size_mb = args.chunk_size;

    info!("⚙️  Config: Parallel Files = {}, Threads/File = {}, Chunk Size = {}MB", file_concurrency, chunk_concurrency, chunk_size_mb);

    let semaphore = Arc::new(Semaphore::new(file_concurrency));
    let mut handles = Vec::new();

    // 提取工具路径
    let fasterq_dump_path = config.software.fasterq_dump.display().to_string();
    let pigz_path = "pigz"; // 假设在 PATH 中，check_pigz_dependency 已检查

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
            
            // --- 1. 下载阶段 ---
            info!("📥 [{}] Step 1: Downloading via AWS S3...", run_id);
            let metadata = aws_s3::SraUtils::get_metadata(&run_id, None).await?;
            
            // 构造文件名 (确保和 aws_s3.rs 逻辑一致)
            let sra_filename = format!("{}.sra", run_id);
            // 构造 .sra 文件的完整路径 (aws_s3.rs 会把文件保存在 output_dir 下)
            // let sra_filepath = output_dir.join(&sra_filename); 

            if let Some(sra_metadata) = metadata {
                let downloader = aws_s3::ResumableDownloader::new(
                    run_id.clone(),
                    sra_metadata,
                    output_dir.clone(),
                    chunk_size, 
                    max_workers,
                ).await?;

                // 执行下载
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

            // --- 2. 转换与压缩阶段 ---
            
            // 构造 fasterq-dump 命令
            // -e: 线程数
            // -O: 输出目录
            // --split-3: 双端拆分
            let cmd_convert = format!(
                "{} --split-3 -e {} -O . {} -f", // -f force overwrite
                fasterq_dump, 
                process_threads,
                sra_filename // 相对路径，因为我们在 current_dir(output_dir) 下执行
            );

            // 构造 pigz 命令
            // 压缩所有生成的 fastq (run_id_1.fastq, run_id_2.fastq 等)
            let cmd_compress = format!(
                "{} -p {} {}*.fastq", 
                pigz, 
                process_threads,
                run_id
            );

            if only_scripts {
                let full_script = format!("{}\n{}", cmd_convert, cmd_compress);
                create_script(&output_dir, &run_id, &full_script)?;
                info!("📝 [{}] Script generated", run_id);
                return Ok(());
            }

            info!("🔄 [{}] Step 2: Converting (fasterq-dump)...", run_id);
            run_command(&cmd_convert, &output_dir).await.context("fasterq-dump failed")?;

            info!("📦 [{}] Step 3: Compressing (pigz)...", run_id);
            run_command(&cmd_compress, &output_dir).await.context("pigz failed")?;

            info!("✅ [{}] All steps completed successfully!", run_id);
            Ok(())
        });

        handles.push(handle);
    }

    for handle in handles {
        if let Err(e) = handle.await {
            warn!("Task error: {}", e);
        }
    }

    info!("🎉 All AWS S3 tasks completed");
    Ok(())
}