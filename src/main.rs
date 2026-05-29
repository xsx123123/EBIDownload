use anyhow::{anyhow, Context, Result};
use chrono::Local;
use clap::Parser;
use indicatif::{MultiProgress, HumanBytes};
use csv::{ReaderBuilder, WriterBuilder};
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

const VERSION: &str = "1.3.6";
const SCRIPT_NAME: &str = "EBIDownload";

use clap::builder::styling::{AnsiColor, Effects, Styles};

const HELP_LOGO: &str = "\n\n\x1b[1;37m    ███████╗██████╗ ██╗██████╗  ██████╗ ██╗      ██████╗  █████╗ ██████╗ \x1b[0m\n\
\x1b[1;37m    ██╔════╝██╔══██╗██║██╔══██╗██╔═══██╗██║     ██╔═══██╗██╔══██╗██╔══██╗\x1b[0m\n\
\x1b[1;37m    █████╗  ██████╔╝██║██║  ██║██║   ██║██║     ██║   ██║███████║██║  ██║\x1b[0m\n\
\x1b[1;37m    ██╔══╝  ██╔══██╗██║██║  ██║██║   ██║██║     ██║   ██║██╔══██║██║  ██║\x1b[0m\n\
\x1b[1;37m    ███████╗██████╔╝██║██████╔╝╚██████╔╝███████╗╚██████╔╝██║  ██║██████╔╝\x1b[0m\n\
\x1b[1;37m    ╚══════╝╚═════╝ ╚═╝╚═════╝  ╚═════╝ ╚══════╝ ╚═════╝ ╚═╝  ╚═╝╚═════╝ \x1b[0m\n\
                                                                          \n\
\x1b[1;37m              🧬  EMBL-ENA Data Downloader  |  v1.3.6\x1b[0m";

const HELP_STYLES: Styles = Styles::styled()
    .header(AnsiColor::Green.on_default().effects(Effects::BOLD))
    .usage(AnsiColor::Cyan.on_default().effects(Effects::BOLD))
    .literal(AnsiColor::Blue.on_default().effects(Effects::BOLD))
    .placeholder(AnsiColor::Cyan.on_default())
    .error(AnsiColor::Red.on_default().effects(Effects::BOLD))
    .valid(AnsiColor::Green.on_default())
    .invalid(AnsiColor::Yellow.on_default());

#[derive(Parser, Debug)]
#[command(
    author,
    version = VERSION,
    about = "Download EMBL-ENA sequencing data",
    long_about = None,
    color = clap::ColorChoice::Always,
    styles = HELP_STYLES,
    before_help = HELP_LOGO,
    help_template = r#"{before-help}
{about-with-newline}
{usage-heading} {usage}

{all-args}
"#
)]
struct Args {
    #[arg(short = 'A', long, value_name = "ID", help = "ENA project accession, e.g. PRJNA1251654", help_heading = "Input Options")]
    accession: Option<String>,
    #[arg(short = 'T', long, value_name = "FILE", help = "Path to a TSV file with run list", help_heading = "Input Options")]
    tsv: Option<PathBuf>,

    #[arg(short, long, value_name = "DIR", help = "Output directory for downloaded data", help_heading = "Input Options")]
    output: PathBuf,

    #[arg(short, long, default_value = "aws", help_heading = "Download Options")]
    download: DownloadMethod,

    #[arg(short = 'p', long, default_value = "4", help = "File-level concurrency", help_heading = "Download Options")]
    multithreads: usize,
    #[arg(short = 't', long = "aws-threads", default_value = "8", help = "Threads per file (AWS/Prefetch)", help_heading = "Download Options")]
    aws_threads: usize,
    #[arg(long = "chunk-size", default_value = "20", help = "Chunk size in MB (AWS only)", help_heading = "Download Options")]
    chunk_size: u64,
    #[arg(long = "max-size", default_value = "100G", help = "Max size limit (Prefetch only)", help_heading = "Download Options")]
    prefetch_max_size: String,
    #[arg(long = "pe-only", default_value = "false", help = "Only download Paired-End data", help_heading = "Download Options")]
    pe_only: bool,

    #[arg(long = "filter-sample", num_args = 1.., help = "Include samples matching regex", help_heading = "Filters")]
    filter_sample: Vec<String>,
    #[arg(long = "filter-run", num_args = 1.., help = "Include runs matching regex", help_heading = "Filters")]
    filter_run: Vec<String>,
    #[arg(long = "exclude-sample", num_args = 1.., help = "Exclude samples matching regex", help_heading = "Filters")]
    exclude_sample: Vec<String>,
    #[arg(long = "exclude-run", num_args = 1.., help = "Exclude runs matching regex", help_heading = "Filters")]
    exclude_run: Vec<String>,

    #[arg(short, long, default_value = "EBIDownload.yaml", value_name = "FILE", help_heading = "Advanced Options")]
    yaml: PathBuf,
    #[arg(long, default_value = "info", help = "Log level: trace/debug/info/warn/error", help_heading = "Advanced Options")]
    log_level: String,
    #[arg(long, default_value = "text", help = "Log format: text or json", help_heading = "Advanced Options")]
    log_format: LogFormat,
    #[arg(long, default_value = "false", help = "Remove intermediate .sra files after conversion", help_heading = "Advanced Options")]
    cleanup_sra: bool,

    #[arg(long, default_value = "false", help = "Show what would be downloaded without actually downloading", help_heading = "Advanced Options")]
    dry_run: bool,
}

#[derive(Debug, Clone, clap::ValueEnum)]
enum DownloadMethod {
    Ascp,
    Ftp,
    Prefetch,
    Aws,
    Auto,
}

#[derive(Debug, Clone, clap::ValueEnum)]
enum LogFormat {
    Text,
    Json,
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
    study_accession: Option<String>,
    secondary_study_accession: Option<String>,
    sample_accession: Option<String>,
    secondary_sample_accession: Option<String>,
    experiment_accession: Option<String>,
    submission_accession: Option<String>,
    tax_id: Option<String>,
    scientific_name: Option<String>,
    instrument_platform: Option<String>,
    instrument_model: Option<String>,
    library_name: Option<String>,
    nominal_length: Option<String>,
    library_layout: Option<String>,
    library_strategy: Option<String>,
    library_source: Option<String>,
    library_selection: Option<String>,
    read_count: Option<String>,
    center_name: Option<String>,
    first_public: Option<String>,
    last_updated: Option<String>,
    experiment_title: Option<String>,
    study_title: Option<String>,
    study_alias: Option<String>,
    run_alias: Option<String>,
    #[serde(default)]
    fastq_bytes: String,
    #[serde(default)]
    fastq_md5: String,
    #[serde(default)]
    fastq_ftp: String,
    fastq_aspera: Option<String>,
    fastq_galaxy: Option<String>,
    submitted_bytes: Option<String>,
    submitted_md5: Option<String>,
    submitted_ftp: Option<String>,
    submitted_aspera: Option<String>,
    submitted_galaxy: Option<String>,
    submitted_format: Option<String>,
    sra_bytes: Option<String>,
    sra_md5: Option<String>,
    sra_ftp: Option<String>,
    sra_aspera: Option<String>,
    sra_galaxy: Option<String>,
    sample_alias: Option<String>,
    #[serde(default)]
    sample_title: String,
    nominal_sdev: Option<String>,
    first_created: Option<String>,
    bam_ftp: Option<String>,
    fastq_file_role: Option<String>,
    submitted_file_role: Option<String>,
    sra_file_role: Option<String>,
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
    include_sample: Vec<Regex>,
    include_run: Vec<Regex>,
    exclude_sample: Vec<Regex>,
    exclude_run: Vec<Regex>,
}

impl RegexFilters {
    fn new(args: &Args) -> Result<Self> {
        Ok(Self {
            include_sample: args.filter_sample.iter().map(|s| Regex::new(s)).collect::<Result<Vec<_>, _>>().context("Invalid regex pattern for --filter-sample")?,
            include_run: args.filter_run.iter().map(|s| Regex::new(s)).collect::<Result<Vec<_>, _>>().context("Invalid regex pattern for --filter-run")?,
            exclude_sample: args.exclude_sample.iter().map(|s| Regex::new(s)).collect::<Result<Vec<_>, _>>().context("Invalid regex pattern for --exclude-sample")?,
            exclude_run: args.exclude_run.iter().map(|s| Regex::new(s)).collect::<Result<Vec<_>, _>>().context("Invalid regex pattern for --exclude-run")?,
        })
    }

    fn should_include(&self, record: &EnaRecord) -> bool {
        if !self.include_sample.is_empty() && !self.include_sample.iter().any(|r| r.is_match(&record.sample_title)) { return false; }
        if !self.include_run.is_empty() && !self.include_run.iter().any(|r| r.is_match(&record.run_accession)) { return false; }
        if !self.exclude_sample.is_empty() && self.exclude_sample.iter().any(|r| r.is_match(&record.sample_title)) { return false; }
        if !self.exclude_run.is_empty() && self.exclude_run.iter().any(|r| r.is_match(&record.run_accession)) { return false; }
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
        fs::create_dir_all(&args.output).context("Failed to create output directory")?;
        print_banner();
        
        setup_logging(&args.output, &args.log_level, &args.log_format, args.accession.as_deref())?;
        check_network_health().await;
        check_pigz_dependency().context("pigz dependency check failed")?;

        let filters = RegexFilters::new(&args)?;
        let config = load_config(&args.yaml).context("Failed to load YAML configuration")?;

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

        save_metadata_tsv(&filtered_records, &args.output, args.accession.as_deref())?;

        let processed = process_records(filtered_records, &args)?;
        save_md5_files(&processed, &args.output, args.accession.as_deref())?;

        if args.dry_run {
            info!("🏜️  Dry Run Mode: Listing files that would be downloaded:");
            for record in &processed {
                info!("   📦 [{}]", record.run_accession);
                info!("      - File 1: {} ({})", record.fastq_ftp_1_name, HumanBytes(record.fastq_bytes_1));
                
                if let (Some(name), Some(size)) = (&record.fastq_ftp_2_name, record.fastq_bytes_2) {
                    info!("      - File 2: {} ({})", name, HumanBytes(size));
                }
            }
            info!("🏜️  Dry Run completed. No files were downloaded.");
            return Ok(());
        }

        match args.download {
            DownloadMethod::Ascp => {
                check_ascp_config(&config)?;
                download_with_ascp(&processed, &config, &args).await?;
            }
            DownloadMethod::Ftp => {
                download_with_ftp(&processed, &config, &args).await?;
            }
            DownloadMethod::Prefetch => {
                check_prefetch_config(&config)?;
                check_fasterq_dump_config(&config)?;
                download_with_prefetch(&processed, &config, &args).await?;
            }
            DownloadMethod::Aws => {
                check_fasterq_dump_config(&config)?;
                download_with_aws(&processed, &config, &args).await?;
            }
            DownloadMethod::Auto => {
                info!("🤖 Auto Mode: Attempting AWS S3 first...");
                check_fasterq_dump_config(&config)?;
                check_prefetch_config(&config)?;
                // Note: In a full production system, we would track individual file failures.
                // Here we attempt AWS. If it completes, great.
                // If the entire batch fails (e.g. API error), we catch it and try Prefetch.
                if let Err(e) = download_with_aws(&processed, &config, &args).await {
                    warn!("⚠️  AWS S3 download encountered issues: {}. Switching to Prefetch...", e);
                    download_with_prefetch(&processed, &config, &args).await?;
                }
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
    let logo = format!(
        r#"
    ███████╗██████╗ ██╗██████╗  ██████╗ ██╗      ██████╗  █████╗ ██████╗ 
    ██╔════╝██╔══██╗██║██╔══██╗██╔═══██╗██║     ██╔═══██╗██╔══██╗██╔══██╗
    █████╗  ██████╔╝██║██║  ██║██║   ██║██║     ██║   ██║███████║██║  ██║
    ██╔══╝  ██╔══██╗██║██║  ██║██║   ██║██║     ██║   ██║██╔══██║██║  ██║
    ███████╗██████╔╝██║██████╔╝╚██████╔╝███████╗╚██████╔╝██║  ██║██████╔╝
    ╚══════╝╚═════╝ ╚═╝╚═════╝  ╚═════╝ ╚══════╝ ╚═════╝ ╚═╝  ╚═╝╚═════╝ 
                                                                          
              🧬  EMBL-ENA Data Downloader  |  v{}"#,
        VERSION
    );
    println!("{}\n", logo);
}

fn setup_logging(output_dir: &Path, log_level: &str, format: &LogFormat, accession: Option<&str>) -> Result<()> {
    use tracing_subscriber::{layer::SubscriberExt, Layer};
    struct LocalTimer;
    impl fmt::time::FormatTime for LocalTimer {
        fn format_time(&self, w: &mut fmt::format::Writer<'_>) -> std::fmt::Result {
            write!(w, "{}", Local::now().format("%Y-%m-%d %H:%M:%S"))
        }
    }
    let timestamp = Local::now().format("%Y-%m-%d_%H-%M-%S");
    let log_name = if let Some(acc) = accession {
        format!("{}_{}_{}.log", SCRIPT_NAME, acc, timestamp)
    } else {
        format!("{}_{}.log", SCRIPT_NAME, timestamp)
    };
    let log_path = output_dir.join(&log_name);
    let file = File::create(&log_path)?;
    
    // File layer always uses simple text for readability
    let file_layer = fmt::layer()
        .with_writer(file)
        .with_ansi(false)
        .with_target(true)
        .with_thread_ids(true)
        .with_timer(fmt::time::LocalTime::rfc_3339())
        .with_filter(EnvFilter::new("debug"));

    let mut stdout_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(log_level));
    if let Ok(directive) = "download_detail=off".parse() {
        stdout_filter = stdout_filter.add_directive(directive);
    }
    
    match format {
        LogFormat::Json => {
            let json_layer = fmt::layer()
                .json()
                .with_writer(std::io::stdout)
                .with_timer(fmt::time::LocalTime::rfc_3339())
                .flatten_event(true)
                .with_target(false)
                .with_filter(stdout_filter);
                
            let subscriber = tracing_subscriber::registry().with(file_layer).with(json_layer);
            tracing::subscriber::set_global_default(subscriber).context("Failed to set subscriber")?;
        }
        LogFormat::Text => {
            let stdout_layer = fmt::layer()
                .with_writer(std::io::stdout)
                .with_ansi(true)
                .with_target(false)
                .with_thread_ids(false)
                .with_timer(LocalTimer)
                .compact()
                .with_filter(stdout_filter);
                
            let subscriber = tracing_subscriber::registry().with(file_layer).with(stdout_layer);
            tracing::subscriber::set_global_default(subscriber).context("Failed to set subscriber")?;
        }
    }

    info!("📝 Log file created: {}", log_path.display());
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
    let fields = "run_accession,study_accession,secondary_study_accession,sample_accession,secondary_sample_accession,experiment_accession,submission_accession,tax_id,scientific_name,instrument_platform,instrument_model,library_name,nominal_length,library_layout,library_strategy,library_source,library_selection,read_count,center_name,first_public,last_updated,experiment_title,study_title,study_alias,run_alias,fastq_bytes,fastq_md5,fastq_ftp,fastq_aspera,fastq_galaxy,submitted_bytes,submitted_md5,submitted_ftp,submitted_aspera,submitted_galaxy,submitted_format,sra_bytes,sra_md5,sra_ftp,sra_aspera,sra_galaxy,sample_alias,sample_title,nominal_sdev,first_created,bam_ftp,fastq_file_role,submitted_file_role,sra_file_role";
    let url = format!("https://www.ebi.ac.uk/ena/portal/api/filereport?accession={}&result=read_run&fields={}&format=tsv", accession, fields);
    info!("🌐 Fetching data from ENA API for: {}", accession);
    
    let client = reqwest::Client::builder()
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

fn save_md5_files(records: &[ProcessedRecord], output_dir: &Path, accession: Option<&str>) -> Result<()> {
    let save_dir = if let Some(acc) = accession {
        let meta_dir = output_dir.join(format!("{}_metadata", acc));
        fs::create_dir_all(&meta_dir)?;
        meta_dir
    } else {
        output_dir.to_path_buf()
    };
    info!("💾 Saving MD5 files to {}...", save_dir.display());
    let (r1_path, r2_path) = if let Some(acc) = accession {
        (
            save_dir.join(format!("R1_fastq_md5_{}.tsv", acc)),
            save_dir.join(format!("R2_fastq_md5_{}.tsv", acc)),
        )
    } else {
        (
            save_dir.join("R1_fastq_md5.tsv"),
            save_dir.join("R2_fastq_md5.tsv"),
        )
    };
    
    let mut r1_file = File::create(&r1_path)?;
    let mut r2_file = File::create(&r2_path)?;
    
    for record in records {
        writeln!(r1_file, "{}\t{}\t{}", record.fastq_md5_1, record.fastq_ftp_1_name, record.sample_title)?;
        if let (Some(md5), Some(name)) = (&record.fastq_md5_2, &record.fastq_ftp_2_name) {
             writeln!(r2_file, "{}\t{}\t{}", md5, name, record.sample_title)?;
        }
    }
    info!("✅ MD5 files saved");
    Ok(())
}

fn save_metadata_tsv(records: &[EnaRecord], output_dir: &Path, accession: Option<&str>) -> Result<()> {
    let save_dir = if let Some(acc) = accession {
        let meta_dir = output_dir.join(format!("{}_metadata", acc));
        fs::create_dir_all(&meta_dir)?;
        meta_dir
    } else {
        output_dir.to_path_buf()
    };
    let path = if let Some(acc) = accession {
        save_dir.join(format!("ena_metadata_{}.tsv", acc))
    } else {
        save_dir.join("ena_metadata.tsv")
    };
    info!("💾 Saving ENA metadata to {}...", path.display());
    
    let mut file = File::create(&path)?;
    if let Some(acc) = accession {
        writeln!(file, "# Project Accession: {}", acc)?;
    }
    
    let mut wtr = WriterBuilder::new()
        .delimiter(b'\t')
        .from_writer(file);
        
    for record in records {
        wtr.serialize(record)?;
    }
    wtr.flush()?;
    info!("✅ Metadata saved");
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
    prefetch::download_all(records, config, &args.output, args.multithreads, args.aws_threads, &args.prefetch_max_size, args.cleanup_sra).await
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
    let mp = Arc::new(MultiProgress::new());
    let mut handles = Vec::new();

    let fasterq_dump_path = config.software.fasterq_dump.display().to_string();
    let pigz_path = "pigz"; 

    for record in records {
        let run_id = record.run_accession.clone();
        let output_dir = args.output.clone();
        let sem = semaphore.clone();
        let mp = mp.clone();
        let max_workers = chunk_concurrency;
        let chunk_size = chunk_size_mb;
        let fasterq_dump = fasterq_dump_path.clone();
        let pigz = pigz_path.to_string();
        let cleanup_sra = args.cleanup_sra;

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
                    Some(mp),
                ).await?;

                let success = downloader.start().await?;
                if !success {
                    return Err(anyhow::anyhow!("Download failed for {}", run_id));
                }
            } else {
                warn!("❌ [{}] No AWS S3 URI found", run_id);
                return Err(anyhow::anyhow!("No S3 URI for {}", run_id));
            }

            let cmd_compress = format!("{} -p {} {}*.fastq", pigz, process_threads, run_id);

            // Smart check: If FASTQ file exists and is not empty, skip conversion
            let fq_1 = output_dir.join(format!("{}_1.fastq", run_id));
            let fq_single = output_dir.join(format!("{}.fastq", run_id));
            let fq_exists = (fq_1.exists() && fq_1.metadata().map(|m| m.len() > 0).unwrap_or(false)) || 
                            (fq_single.exists() && fq_single.metadata().map(|m| m.len() > 0).unwrap_or(false));

            if fq_exists {
                info!("⏩ [{}] FASTQ files already exist, skipping conversion.", run_id);
            } else {
                info!("🔄 [{}] Step 2: Converting (fasterq-dump)...", run_id);
                // Safe command execution
                let output = Command::new(&fasterq_dump)
                    .arg("--split-3")
                    .arg("-e").arg(process_threads.to_string())
                    .arg("-O").arg(".")
                    .arg("-f")
                    .arg(&sra_filename)
                    .current_dir(&output_dir)
                    .stdout(Stdio::null())
                    .stderr(Stdio::piped())
                    .output()
                    .await;

                match output {
                     Ok(out) if out.status.success() => {},
                     Ok(out) => warn!("⚠️ [{}] fasterq-dump error: {}", run_id, String::from_utf8_lossy(&out.stderr)),
                     Err(e) => warn!("⚠️ [{}] fasterq-dump execution failed: {}", run_id, e),
                }
            }

            // Fault-tolerant compression
            if (fq_1.exists() && fq_1.metadata().map(|m| m.len() > 0).unwrap_or(false)) || 
               (fq_single.exists() && fq_single.metadata().map(|m| m.len() > 0).unwrap_or(false)) {
                
                info!("📦 [{}] Step 3: Compressing (pigz)...", run_id);
                // Pigz with wildcard still needs shell or glob expansion. 
                // Using bash -c here is acceptable for wildcard, but we can make it slightly safer by avoiding string formatting if possible.
                // However, pigz *.fastq is inherently shell-dependent unless we expand in Rust.
                // For simplicity/robustness, we keep the run_command (shell) for pigz as it is complex to reimplement globbing.
                run_command(&cmd_compress, &output_dir).await.context("pigz failed")?;
                
                if cleanup_sra {
                    let sra_path = output_dir.join(&sra_filename);
                    if sra_path.exists() {
                        info!("🧹 [{}] Cleaning up SRA file: {}", run_id, sra_path.display());
                        if let Err(e) = tokio::fs::remove_file(&sra_path).await {
                            warn!("⚠️ [{}] Failed to remove SRA file: {}", run_id, e);
                        }
                    }
                }
                
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
        args.multithreads
    ).await
}

// Aspera Entry
async fn download_with_ascp(records: &[ProcessedRecord], config: &Config, args: &Args) -> Result<()> {
    ftp::process_downloads(
        records, 
        config, 
        &args.output, 
        ftp::Protocol::Ascp, 
        args.multithreads
    ).await
}
fn check_executable(path: &std::path::Path, name: &str) -> Result<()> {
    if !path.exists() {
        return Err(anyhow::anyhow!(
            "{} not found at configured path: {}. Please check your EBIDownload.yaml configuration.",
            name,
            path.display()
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let metadata = std::fs::metadata(path)?;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err(anyhow::anyhow!(
                "{} at {} exists but is not executable. Please check permissions.",
                name,
                path.display()
            ));
        }
    }
    info!("✅ {} check passed: {}", name, path.display());
    Ok(())
}

fn check_file_exists(path: &std::path::Path, name: &str) -> Result<()> {
    if !path.exists() {
        return Err(anyhow::anyhow!(
            "{} not found at configured path: {}. Please check your EBIDownload.yaml configuration.",
            name,
            path.display()
        ));
    }
    info!("✅ {} check passed: {}", name, path.display());
    Ok(())
}

fn check_prefetch_config(config: &Config) -> Result<()> {
    check_executable(&config.software.prefetch, "prefetch")
}

fn check_ascp_config(config: &Config) -> Result<()> {
    check_executable(&config.software.ascp, "ascp")?;
    check_file_exists(&config.setting.openssh, "Aspera openssh key")
}

fn check_fasterq_dump_config(config: &Config) -> Result<()> {
    check_executable(&config.software.fasterq_dump, "fasterq-dump")
}

fn check_pigz_dependency() -> Result<()> {
    if which::which("pigz").is_err() {
        return Err(anyhow::anyhow!(
            "pigz not found in system PATH. Please install pigz first:\n  Ubuntu/Debian: sudo apt-get install pigz\n  macOS: brew install pigz\n  Or ensure pigz is in your PATH."
        ));
    }
    info!("✅ pigz check passed");
    Ok(())
}