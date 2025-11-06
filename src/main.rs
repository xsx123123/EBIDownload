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
use which::which; // --- MODIFICATION: Added for pigz check ---

const VERSION: &str = "1.2.4v";
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

    #[arg(short = 'p', long, default_value = "4")]
    multithreads: usize,

    #[arg(short, long, default_value = "prefetch")]
    download: DownloadMethod,

    #[arg(short = 'O', long, default_value = "false")]
    only_scripts: bool,

    #[arg(short, long, default_value = "EBIDownload.yaml")]
    yaml: PathBuf,

    #[arg(long = "filter-sample")]
    filter_sample: Option<String>,

    #[arg(long = "filter-run")]
    filter_run: Option<String>,

    #[arg(long = "exclude-sample")]
    exclude_sample: Option<String>,

    #[arg(long = "exclude-run")]
    exclude_run: Option<String>,
}

#[derive(Debug, Clone, clap::ValueEnum)]
enum DownloadMethod {
    Ascp,
    Ftp,
    Prefetch,
}

// --- START MODIFICATION: Updated structs for new YAML format ---
#[derive(Debug, Deserialize)]
struct Config {
    software: SoftwarePaths,
    setting: SettingPaths,
}

#[derive(Debug, Deserialize)]
struct SoftwarePaths {
    ascp: PathBuf,
    prefetch: PathBuf,
    fasterq_dump: PathBuf,
}

#[derive(Debug, Deserialize)]
struct SettingPaths {
    openssh: PathBuf,
}
// --- END MODIFICATION ---

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
    fastq_ftp_2_url: String,
    fastq_ftp_1_name: String,
    fastq_ftp_2_name: String,
    fastq_md5_1: String,
    fastq_md5_2: String,
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
            include_sample: args
                .filter_sample
                .as_deref()
                .map(Regex::new)
                .transpose()
                .context("Invalid regex pattern for --filter-sample")?,
            include_run: args
                .filter_run
                .as_deref()
                .map(Regex::new)
                .transpose()
                .context("Invalid regex pattern for --filter-run")?,
            exclude_sample: args
                .exclude_sample
                .as_deref()
                .map(Regex::new)
                .transpose()
                .context("Invalid regex pattern for --exclude-sample")?,
            exclude_run: args
                .exclude_run
                .as_deref()
                .map(Regex::new)
                .transpose()
                .context("Invalid regex pattern for --exclude-run")?,
        })
    }

    fn should_include(&self, record: &EnaRecord) -> bool {
        if let Some(ref regex) = self.include_sample {
            if !regex.is_match(&record.sample_title) {
                return false;
            }
        }
        if let Some(ref regex) = self.include_run {
            if !regex.is_match(&record.run_accession) {
                return false;
            }
        }
        if let Some(ref regex) = self.exclude_sample {
            if regex.is_match(&record.sample_title) {
                return false;
            }
        }
        if let Some(ref regex) = self.exclude_run {
            if regex.is_match(&record.run_accession) {
                return false;
            }
        }
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
async fn main() -> Result<()> {
    let args = Args::parse();
    setup_logging()?;
    print_banner();

    // --- START MODIFICATION: Added pigz check early ---
    check_pigz_dependency().context("pigz dependency check failed")?;
    // --- END MODIFICATION ---

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

    let processed = process_records(filtered_records)?;
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
            download_with_prefetch(&processed, &config, &args).await?;
        }
    }

    info!("🎉 {} download completed successfully!", SCRIPT_NAME);
    Ok(())
}

fn print_banner() {
    println!("\n{}", "=".repeat(60));
    println!("  🧬 {} - EMBL-ENA Data Downloader v{}", SCRIPT_NAME, VERSION);
    println!("{}\n", "=".repeat(60));
}

fn setup_logging() -> Result<()> {
    let timestamp = Local::now().format("%Y-%m-%d_%H-%M-%S");
    let log_file = format!("{}_EMBI-ENA_Download_{}.log", SCRIPT_NAME, timestamp);

    let file = File::create(&log_file)?;

    let file_layer = fmt::layer()
        .with_writer(file)
        .with_ansi(false)
        .with_target(false)
        .with_thread_ids(false)
        .with_timer(fmt::time::LocalTime::rfc_3339())
        .compact();

    let stdout_layer = fmt::layer()
        .with_writer(std::io::stdout)
        .with_ansi(true)
        .with_target(false)
        .with_thread_ids(false)
        .with_timer(fmt::time::LocalTime::rfc_3339())
        .compact();

    use tracing_subscriber::layer::SubscriberExt;
    let subscriber = tracing_subscriber::registry()
        .with(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with(file_layer)
        .with(stdout_layer);

    tracing::subscriber::set_global_default(subscriber).context("Failed to set subscriber")?;
    info!("📝 Log file created: {}", log_file);
    Ok(())
}

fn load_config(yaml_path: &Path) -> Result<Config> {
    if !yaml_path.exists() {
        return Err(anyhow!(
            "YAML configuration file not found: {}",
            yaml_path.display()
        ));
    }
    info!("⚙️  Loading configuration from: {}", yaml_path.display());
    let content = fs::read_to_string(yaml_path)?;
    let config: Config = serde_yaml::from_str(&content)?;
    info!("✅ Configuration loaded successfully");
    Ok(config)
}

async fn fetch_ena_data(accession: &str) -> Result<Vec<EnaRecord>> {
    let url = format!(
        "https://www.ebi.ac.uk/ena/portal/api/filereport?accession={}&result=read_run&fields=run_accession,fastq_ftp,fastq_md5,fastq_bytes,sample_title&format=tsv",
        accession
    );

    info!("🌐 Fetching data from ENA API for: {}", accession);

    let response = reqwest::get(&url)
        .await
        .context("Failed to fetch data from ENA API")?;

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

    info!("✅ Fetched {} records from ENA", records.len());
    Ok(records)
}

fn read_tsv_data(tsv_path: &Path) -> Result<Vec<EnaRecord>> {
    info!("📄 Reading TSV file: {}", tsv_path.display());

    let mut reader = ReaderBuilder::new()
        .has_headers(true)
        .delimiter(b'\t')
        .from_path(tsv_path)?;

    let mut records = Vec::new();
    for result in reader.deserialize() {
        let record: EnaRecord = result?;
        records.push(record);
    }

    info!("✅ Read {} records from TSV", records.len());
    Ok(records)
}

fn apply_filters(records: Vec<EnaRecord>, filters: &RegexFilters) -> Result<Vec<EnaRecord>> {
    let mut filtered = Vec::new();
    let mut filtered_count = 0;

    for record in records {
        if filters.should_include(&record) {
            filtered.push(record);
        } else {
            filtered_count += 1;
        }
    }

    if filtered_count > 0 {
        info!("🔍 Filtered out {} records based on regex patterns", filtered_count);
    }

    Ok(filtered)
}

fn process_records(records: Vec<EnaRecord>) -> Result<Vec<ProcessedRecord>> {
    info!("⚙️  Processing records...");
    let mut processed = Vec::new();

    for record in records {
        let ftp_urls: Vec<&str> = record.fastq_ftp.split(';').filter(|s| !s.is_empty()).collect();
        let md5s: Vec<&str> = record.fastq_md5.split(';').filter(|s| !s.is_empty()).collect();

        if ftp_urls.len() < 2 || md5s.len() < 2 {
            warn!("⚠️  Skipping incomplete record: {}", record.run_accession);
            continue;
        }

        let ftp_1_url = ftp_urls[0].to_string();
        let ftp_2_url = ftp_urls[1].to_string();

        let ftp_1_name = ftp_1_url.rsplit('/').next().unwrap_or("").to_string();
        let ftp_2_name = ftp_2_url.rsplit('/').next().unwrap_or("").to_string();

        processed.push(ProcessedRecord {
            run_accession: record.run_accession,
            fastq_ftp_1_url: ftp_1_url,
            fastq_ftp_2_url: ftp_2_url,
            fastq_ftp_1_name: ftp_1_name,
            fastq_ftp_2_name: ftp_2_name,
            fastq_md5_1: md5s[0].to_string(),
            fastq_md5_2: md5s[1].to_string(),
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
        writeln!(
            r1_file,
            "{}\t{}\t{}",
            record.fastq_md5_1, record.fastq_ftp_1_name, record.sample_title
        )?;
        writeln!(
            r2_file,
            "{}\t{}\t{}",
            record.fastq_md5_2, record.fastq_ftp_2_name, record.sample_title
        )?;
    }

    info!("✅ MD5 files saved: R1_fastq_md5.tsv, R2_fastq_md5.tsv");
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

    #[cfg(unix)]
    {
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
        urls.push(record.fastq_ftp_2_url.clone());
    }

    // progress bars
    let semaphore = Arc::new(Semaphore::new(args.multithreads)); // <-- Corrected this line
    let mp = Arc::new(MultiProgress::new());
    let mut bars: HashMap<String, ProgressBar> = HashMap::new();
    for url in &urls {
        let run = url.rsplit('/').next().unwrap_or("unknown").to_string();
        let pb = mp.add(ProgressBar::new(100));
        pb.set_style(ProgressStyle::with_template("[{prefix}] {bar:40.cyan/blue} {percent:>3}% {msg}")?.progress_chars("##-"));
        pb.set_prefix(run.clone());
        bars.insert(run, pb);
    }

    let (tx, mut rx) = mpsc::channel::<DlEvent>(1024);

    // render task
    let mp_render = mp.clone();
    let mut bars_render = bars.clone();
    let render = tokio::spawn(async move {
        while let Some(ev) = rx.recv().await {
            match ev {
                DlEvent::Stage { run, msg, pct } => {
                    if let Some(pb) = bars_render.get(&run) {
                        pb.set_position(pct.min(100));
                        pb.set_message(msg);
                    }
                }
                DlEvent::Done { run } => {
                    if let Some(pb) = bars_render.remove(&run) {
                        pb.finish_with_message("✅ Done");
                    }
                }
                DlEvent::Fail { run, err } => {
                    if let Some(pb) = bars_render.remove(&run) {
                        pb.finish_with_message(format!("❌ Failed: {}", err));
                    }
                }
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

        let task = tokio::spawn(async move {
            let _permit = sem.acquire().await.expect("semaphore closed");
            let _ = tx.send(DlEvent::Stage { run: run_id.clone(), msg: "Queued".into(), pct: 0 }).await;

            let cmd = format!("wget -c {}", url);
            if only_scripts {
                let _ = create_script(&output_path, &run_id, &cmd);
                let _ = tx.send(DlEvent::Done { run: run_id.clone() }).await;
                return;
            }

            let _ = tx.send(DlEvent::Stage { run: run_id.clone(), msg: "Downloading".into(), pct: 30 }).await;
            
            let spawn_result = Command::new("bash")
                .arg("-lc")
                .arg(&cmd)
                .current_dir(&output_path)
                .stdout(Stdio::null()) 
                .stderr(Stdio::null()) 
                .spawn();

            let status;
            match spawn_result {
                Ok(mut child) => {
                    status = child.wait().await;
                }
                Err(e) => {
                    status = Err(e);
                }
            }

            match status {
                Ok(s) if s.success() => {
                    let _ = tx.send(DlEvent::Stage { run: run_id.clone(), msg: "Verifying".into(), pct: 90 }).await;
                    let _ = tx.send(DlEvent::Done { run: run_id.clone() }).await;
                }
                Ok(s) => {
                    let _ = tx.send(DlEvent::Fail { run: run_id.clone(), err: format!("exit {}", s) }).await;
                }
                Err(e) => {
                    let _ = tx.send(DlEvent::Fail { run: run_id.clone(), err: e.to_string() }).await;
                }
            }
        });

        tasks.push(task);
    }
    drop(tx);

    for t in tasks {
        t.await.map_err(|e| anyhow!(e))?;
    }
    render.await.map_err(|e| anyhow!(e))?;
    info!("🎉 All FTP downloads completed");
    Ok(())
}

async fn download_with_ascp(records: &[ProcessedRecord], config: &Config, args: &Args) -> Result<()> {
    info!("🚀 Starting Aspera downloads with {} threads...", args.multithreads);

    let mut urls = Vec::new();
    for record in records {
        let url1 = record.fastq_ftp_1_url.replace("ftp.sra.ebi.ac.uk", "era-fasp@fasp.sra.ebi.ac.uk:");
        let url2 = record.fastq_ftp_2_url.replace("ftp.sra.ebi.ac.uk", "era-fasp@fasp.sra.ebi.ac.uk:");
        urls.push(url1);
        urls.push(url2);
    }

    let semaphore = Arc::new(Semaphore::new(args.multithreads));
    let mp = Arc::new(MultiProgress::new());
    let mut bars: HashMap<String, ProgressBar> = HashMap::new();
    for url in &urls {
        let run = url.rsplit('/').next().unwrap_or("unknown").to_string();
        let pb = mp.add(ProgressBar::new(100));
        pb.set_style(ProgressStyle::with_template("[{prefix}] {bar:40.green/black} {percent:>3}% {msg}")?.progress_chars("##-"));
        pb.set_prefix(run.clone());
        bars.insert(run, pb);
    }

    let (tx, mut rx) = mpsc::channel::<DlEvent>(1024);
    let mp_render = mp.clone();
    let mut bars_render = bars.clone();
    let render = tokio::spawn(async move {
        while let Some(ev) = rx.recv().await {
            match ev {
                DlEvent::Stage { run, msg, pct } => {
                    if let Some(pb) = bars_render.get(&run) {
                        pb.set_position(pct.min(100));
                        pb.set_message(msg);
                    }
                }
                DlEvent::Done { run } => {
                    if let Some(pb) = bars_render.remove(&run) {
                        pb.finish_with_message("✅ Done");
                    }
                }
                DlEvent::Fail { run, err } => {
                    if let Some(pb) = bars_render.remove(&run) {
                        pb.finish_with_message(format!("❌ Failed: {}", err));
                    }
                }
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
        let ascp = config.software.ascp.clone();
        // --- START MODIFICATION: Use new config structure ---
        let openssh = config.setting.openssh.clone();
        // --- END MODIFICATION ---
        let run_id = url.rsplit('/').next().unwrap_or("unknown").to_string();

        let task = tokio::spawn(async move {
            let _permit = sem.acquire().await.expect("semaphore closed");
            let _ = tx.send(DlEvent::Stage { run: run_id.clone(), msg: "Queued".into(), pct: 0 }).await;

            let cmd = format!(
                "{} -QT -k2 -l 800m -P33001 -i {} {} {}",
                ascp.display(),
                openssh.display(),
                url,
                output_path.display()
            );

            if only_scripts {
                let _ = create_script(&output_path, &run_id, &cmd);
                let _ = tx.send(DlEvent::Done { run: run_id.clone() }).await;
                return;
            }

            let _ = tx.send(DlEvent::Stage { run: run_id.clone(), msg: "Transferring".into(), pct: 50 }).await;

            let spawn_result = Command::new("bash")
                .arg("-lc")
                .arg(&cmd)
                .current_dir(&output_path)
                .stdout(Stdio::null()) 
                .stderr(Stdio::null()) 
                .spawn();

            let status;
            match spawn_result {
                Ok(mut child) => {
                    status = child.wait().await;
                }
                Err(e) => {
                    status = Err(e);
                }
            }

            match status {
                Ok(s) if s.success() => {
                    let _ = tx.send(DlEvent::Done { run: run_id.clone() }).await;
                }
                Ok(s) => {
                    let _ = tx.send(DlEvent::Fail { run: run_id.clone(), err: format!("exit {}", s) }).await;
                }
                Err(e) => {
                    let _ = tx.send(DlEvent::Fail { run: run_id.clone(), err: e.to_string() }).await;
                }
            }
        });
        tasks.push(task);
    }
    drop(tx);

    for t in tasks {
        t.await.map_err(|e| anyhow!(e))?;
    }
    render.await.map_err(|e| anyhow!(e))?;
    info!("🎉 All Aspera downloads completed");
    Ok(())
}

async fn download_with_prefetch(records: &[ProcessedRecord], config: &Config, args: &Args) -> Result<()> {
    info!("📦 Starting prefetch downloads with {} threads...", args.multithreads);

    let semaphore = Arc::new(Semaphore::new(args.multithreads));

    // progress bars
    let mp = Arc::new(MultiProgress::new());
    let mut bars: HashMap<String, ProgressBar> = HashMap::new();
    for rec in records {
        let run = rec.run_accession.clone();
        let pb = mp.add(ProgressBar::new(100));
        pb.set_style(ProgressStyle::with_template("[{prefix}] {bar:40.magenta/black} {percent:>3}% {msg}")?.progress_chars("##-"));
        pb.set_prefix(run.clone());
        bars.insert(run, pb);
    }

    let (tx, mut rx) = mpsc::channel::<DlEvent>(1024);

    // render task
    let mp_render = mp.clone();
    let mut bars_render = bars.clone();
    let render = tokio::spawn(async move {
        while let Some(ev) = rx.recv().await {
            match ev {
                DlEvent::Stage { run, msg, pct } => {
                    if let Some(pb) = bars_render.get(&run) {
                        pb.set_position(pct.min(100));
                        pb.set_message(msg);
                    }
                }
                DlEvent::Done { run } => {
                    if let Some(pb) = bars_render.remove(&run) {
                        pb.finish_with_message("✅ Done");
                    }
                }
                DlEvent::Fail { run, err } => {
                    if let Some(pb) = bars_render.remove(&run) {
                        pb.finish_with_message(format!("❌ Failed: {}", err));
                    }
                }
            }
        }
        let _ = mp_render.clear();
    });

    // spawn per record
    let mut tasks = Vec::new();
    for record in records {
        let sem = semaphore.clone();
        let tx = tx.clone();
        let output_path = args.output.clone();
        let only_scripts = args.only_scripts;
        let run_acc = record.run_accession.clone();
        let prefetch = config.software.prefetch.clone();
        let fasterq_dump = config.software.fasterq_dump.clone();
        
        // --- START MODIFICATION: Assume pigz is in PATH ---
        // No longer read from config
        let pigz = PathBuf::from("pigz");
        // --- END MODIFICATION ---


        let task = tokio::spawn(async move {
            let _permit = sem.acquire().await.expect("semaphore closed");

            let _ = tx.send(DlEvent::Stage { run: run_acc.clone(), msg: "Queued".into(), pct: 0 }).await;

            // --- START MODIFICATION: Removed conda activate path ---
            let prefetch_command = format!(
                "mkdir -p ./{}\n{} {} --max-size 100G --output-directory ./{}\n{} -p 20 -O ./{} ./{}\n{} -p 20 ./{}/*.fastq",
                // Removed conda/qc args
                run_acc,
                prefetch.display(), run_acc, run_acc,
                fasterq_dump.display(), run_acc, run_acc,
                pigz.display(), run_acc
            );
            // --- END MODIFICATION ---

            if only_scripts {
                let _ = create_script(&output_path, &run_acc, &prefetch_command);
                let _ = tx.send(DlEvent::Done { run: run_acc.clone() }).await;
                return;
            }

            let _ = tx.send(DlEvent::Stage { run: run_acc.clone(), msg: "Resolving".into(), pct: 10 }).await;
            let _ = tx.send(DlEvent::Stage { run: run_acc.clone(), msg: "Downloading SRA".into(), pct: 40 }).await;

            let spawn_result = Command::new("bash")
                .arg("-lc")
                .arg(&prefetch_command)
                .current_dir(&output_path)
                .stdout(Stdio::null()) 
                .stderr(Stdio::null()) 
                .spawn();

            let status;
            match spawn_result {
                Ok(mut child) => {
                    status = child.wait().await;
                }
                Err(e) => {
                    status = Err(e);
                }
            }

            match status {
                Ok(s) if s.success() => {
                    let _ = tx.send(DlEvent::Stage { run: run_acc.clone(), msg: "Converting (fasterq-dump)".into(), pct: 85 }).await;
                    let _ = tx.send(DlEvent::Stage { run: run_acc.clone(), msg: "Compressing (pigz)".into(), pct: 95 }).await;
                    let _ = tx.send(DlEvent::Done { run: run_acc.clone() }).await;
                }
                Ok(s) => {
                    let _ = tx.send(DlEvent::Fail { run: run_acc.clone(), err: format!("exit {}", s) }).await;
                }
                Err(e) => {
                    let _ = tx.send(DlEvent::Fail { run: run_acc.clone(), err: e.to_string() }).await;
                }
            }
        });

        tasks.push(task);
    }
    drop(tx);

    for t in tasks {
        t.await.map_err(|e| anyhow!(e))?;
    }
    render.await.map_err(|e| anyhow!(e))?;
    info!("🎉 All prefetch downloads completed");
    Ok(())
}

fn check_ascp_config(config: &Config) -> Result<()> {
    info!("🔍 Checking Aspera configuration...");

    if !config.software.ascp.exists() {
        return Err(anyhow!(
            "❌ ASCP command not found: {}",
            config.software.ascp.display()
        ));
    }
    info!("  ✓ ASCP: {}", config.software.ascp.display());

    // --- START MODIFICATION: Use new config structure ---
    if !config.setting.openssh.exists() {
        return Err(anyhow!(
            "❌ ASCP openssh not found: {}",
            config.setting.openssh.display()
        ));
    }
    info!("  ✓ OpenSSH key: {}", config.setting.openssh.display());
    // --- END MODIFICATION ---

    Ok(())
}

fn check_prefetch_config(config: &Config) -> Result<()> {
    info!("🔍 Checking prefetch configuration...");

    if !config.software.prefetch.exists() {
        return Err(anyhow!(
            "prefetch command not found: {}",
            config.software.prefetch.display()
        ));
    }
    info!("  ✓ prefetch: {}", config.software.prefetch.display());

    if !config.software.fasterq_dump.exists() {
        return Err(anyhow!(
            "fasterq_dump command not found: {}",
            config.software.fasterq_dump.display()
        ));
    }
    info!("  ✓ fasterq-dump: {}", config.software.fasterq_dump.display());

    // --- START MODIFICATION: Removed pigz check from here ---
    // The check is now done in main() at startup.
    // --- END MODIFICATION ---

    Ok(())
}

// --- START MODIFICATION: Added new function to check for pigz ---
/// Checks if 'pigz' is available in the system PATH.
/// If not, it returns an error with installation instructions.
fn check_pigz_dependency() -> Result<()> {
    info!("🔍 Checking for 'pigz' dependency...");
    if which("pigz").is_err() {
        // Return a hard error that will stop the program
        return Err(anyhow!(
            "❌ 'pigz' is not found in your system PATH.\n  \
             'pigz' is required for the prefetch download method to compress fastq files.\n  \
             请访问 https://github.com/madler/pigz 安装 pigz"
        ));
    }
    info!("  ✓ 'pigz' dependency found in PATH");
    Ok(())
}
// --- END MODIFICATION ---