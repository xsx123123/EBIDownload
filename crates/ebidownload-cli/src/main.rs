use anyhow::{anyhow, Context, Result};
use chrono::Local;
use clap::Parser;
use clap::Subcommand;
use indicatif::{MultiProgress, HumanBytes};
use csv::{ReaderBuilder, WriterBuilder};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{ExitCode, Stdio};
use std::sync::Arc;
use tokio::process::Command;
use tokio::sync::Semaphore;
use tracing::{info, warn, error};
use tracing_subscriber::{fmt, EnvFilter};
use std::time::Duration;

use ebidownload_core::*;

const VERSION: &str = "1.3.7";
const SCRIPT_NAME: &str = "EBIDownload";

use clap::builder::styling::{AnsiColor, Effects, Styles};

const HELP_LOGO: &str = "\n\n\x1b[1;37m    ███████╗██████╗ ██╗██████╗  ██████╗ ██╗      ██████╗  █████╗ ██████╗ \x1b[0m\n\
\x1b[1;37m    ██╔════╝██╔══██╗██║██╔══██╗██╔═══██╗██║     ██╔═══██╗██╔══██╗██╔══██╗\x1b[0m\n\
\x1b[1;37m    █████╗  ██████╔╝██║██║  ██║██║   ██║██║     ██║   ██║███████║██║  ██║\x1b[0m\n\
\x1b[1;37m    ██╔══╝  ██╔══██╗██║██║  ██║██║   ██║██║     ██║   ██║██╔══██║██║  ██║\x1b[0m\n\
\x1b[1;37m    ███████╗██████╔╝██║██████╔╝╚██████╔╝███████╗╚██████╔╝██║  ██║██████╔╝\x1b[0m\n\
\x1b[1;37m    ╚══════╝╚═════╝ ╚═╝╚═════╝  ╚═════╝ ╚══════╝ ╚═════╝ ╚═╝  ╚═╝╚═════╝ \x1b[0m\n\
                                                                          \n\
\x1b[1;37m              🧬  EMBL-ENA Data Toolkit   |  v1.3.7\x1b[0m";

const HELP_STYLES: Styles = Styles::styled()
    .header(AnsiColor::Green.on_default().effects(Effects::BOLD))
    .usage(AnsiColor::Cyan.on_default().effects(Effects::BOLD))
    .literal(AnsiColor::Blue.on_default().effects(Effects::BOLD))
    .placeholder(AnsiColor::Cyan.on_default())
    .error(AnsiColor::Red.on_default().effects(Effects::BOLD))
    .valid(AnsiColor::Green.on_default())
    .invalid(AnsiColor::Yellow.on_default());

// ============================================================
// CLI Structure: Subcommands (download / upload)
// ============================================================

#[derive(Parser, Debug)]
#[command(
    author,
    version = VERSION,
    about = "Download and upload sequencing data (EBI ENA / NCBI SRA)",
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
struct Cli {
    #[command(subcommand)]
    command: Commands,

    #[arg(short, long, global = true, default_value = "EBIDownload.yaml", value_name = "FILE", help_heading = "Global Options")]
    yaml: PathBuf,
    #[arg(long, global = true, default_value = "info", help = "Log level: trace/debug/info/warn/error", help_heading = "Global Options")]
    log_level: String,
    #[arg(long, global = true, default_value = "text", help = "Log format: text or json", help_heading = "Global Options")]
    log_format: LogFormat,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Download sequencing data from EBI ENA / NCBI SRA
    Download(DownloadArgs),
    /// Upload sequencing data to AWS S3 for NCBI SRA submission
    Upload(UploadArgs),
}

// ============================================================
// Download Subcommand Arguments (unchanged from original Args)
// ============================================================

#[derive(Parser, Debug)]
struct DownloadArgs {
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

    #[arg(long, default_value = "false", help = "Remove intermediate .sra files after conversion", help_heading = "Advanced Options")]
    cleanup_sra: bool,
    #[arg(long, default_value = "false", help = "Show what would be downloaded without actually downloading", help_heading = "Advanced Options")]
    dry_run: bool,
}

// ============================================================
// Upload Subcommand Arguments (NEW)
// ============================================================

#[derive(Parser, Debug)]
struct UploadArgs {
    #[arg(short, long, value_name = "NAME", help = "AWS S3 bucket name", help_heading = "S3 Options")]
    bucket: String,
    #[arg(long, value_name = "PREFIX", help = "S3 key prefix (subdirectory)", help_heading = "S3 Options")]
    prefix: Option<String>,
    #[arg(short = 'f', long, num_args = 1.., value_name = "FILE", help = "Files to upload", help_heading = "S3 Options")]
    files: Vec<PathBuf>,

    #[arg(long, default_value = "us-east-1", help = "AWS region for the S3 bucket", help_heading = "AWS Options")]
    region: String,
    #[arg(short = 'c', long, default_value = "4", help = "Concurrent file uploads", help_heading = "AWS Options")]
    concurrent: usize,

    #[arg(long, default_value = "false", help = "Apply NCBI SRA submission bucket policy", help_heading = "NCBI SRA")]
    apply_policy: bool,
    #[arg(long, value_name = "FILE", help = "Generate SRA metadata template TSV", help_heading = "NCBI SRA")]
    metadata_template: Option<PathBuf>,

    #[arg(long, default_value = "false", help = "Show what would be uploaded without actually uploading", help_heading = "Advanced Options")]
    dry_run: bool,
}

// ============================================================
// Shared Types
// ============================================================

#[derive(Debug, Clone, clap::ValueEnum)]
enum LogFormat {
    Text,
    Json,
}

// ============================================================
// Progress-aware logging infrastructure
// ============================================================

/// Global MultiProgress instance shared between logging and progress bars.
/// When progress bars are active, log messages are rendered above them via
/// MultiProgress::println(), preventing display corruption.
static GLOBAL_MP: std::sync::LazyLock<MultiProgress> =
    std::sync::LazyLock::new(MultiProgress::new);

/// Tracks whether any progress bars are currently active on GLOBAL_MP.
/// When true, MpWriter routes through MultiProgress::println() (which draws
/// above active bars). When false, MpWriter writes directly to stderr
/// (because MultiProgress::println() is a no-op without active bars).
static BARS_ACTIVE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Custom writer that routes tracing output intelligently:
/// - Progress bars active → MultiProgress::println() (renders above bars)
/// - No progress bars → direct stderr (MultiProgress::println is a no-op)
struct MpWriter {
    buf: Vec<u8>,
}

impl std::io::Write for MpWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.buf.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if !self.buf.is_empty() {
            let s = String::from_utf8_lossy(&self.buf);
            let s = s.trim_end_matches('\n');
            if !s.is_empty() {
                if BARS_ACTIVE.load(std::sync::atomic::Ordering::Relaxed) {
                    let _ = GLOBAL_MP.println(s);
                } else {
                    eprintln!("{}", s);
                }
            }
            self.buf.clear();
        }
        Ok(())
    }
}

impl Drop for MpWriter {
    fn drop(&mut self) {
        let _ = self.flush();
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
async fn main() -> ExitCode {
    let cli = Cli::parse();

    let output_dir = match &cli.command {
        Commands::Download(args) => args.output.clone(),
        Commands::Upload(_) => PathBuf::from("."),
    };

    if let Commands::Download(args) = &cli.command {
        if let Err(e) = fs::create_dir_all(&args.output) {
            eprintln!("❌ Failed to create output directory: {}", e);
            return ExitCode::FAILURE;
        }
    }

    print_banner();

    if let Err(e) = setup_logging(&output_dir, &cli.log_level, &cli.log_format, match &cli.command {
        Commands::Download(args) => args.accession.as_deref(),
        Commands::Upload(_) => None,
    }) {
        eprintln!("❌ Failed to setup logging: {}", e);
        return ExitCode::FAILURE;
    }

    check_network_health().await;

    let result: Result<()> = async {
        match &cli.command {
            Commands::Download(args) => {
                ebidownload_core::check_pigz_dependency().context("pigz dependency check failed")?;
                run_download(args, &cli).await
            }
            Commands::Upload(args) => {
                run_upload(args).await
            }
        }
    }
    .await;

    if let Err(e) = result {
        tracing::error!("Application failed: {:?}", e);
        eprintln!("\n❌ An error occurred. Please check the log file for detailed error information.");
        return ExitCode::FAILURE;
    }

    ExitCode::SUCCESS
}

// ============================================================
// Download Command Entry Point (original main logic, unchanged)
// ============================================================

async fn run_download(args: &DownloadArgs, cli: &Cli) -> Result<()> {
    let filters = RegexFilters {
        include_sample: args.filter_sample.iter().map(|s| Regex::new(s)).collect::<Result<Vec<_>, _>>().context("Invalid regex pattern for --filter-sample")?,
        include_run: args.filter_run.iter().map(|s| Regex::new(s)).collect::<Result<Vec<_>, _>>().context("Invalid regex pattern for --filter-run")?,
        exclude_sample: args.exclude_sample.iter().map(|s| Regex::new(s)).collect::<Result<Vec<_>, _>>().context("Invalid regex pattern for --exclude-sample")?,
        exclude_run: args.exclude_run.iter().map(|s| Regex::new(s)).collect::<Result<Vec<_>, _>>().context("Invalid regex pattern for --exclude-run")?,
    };
    let config = load_config(&cli.yaml).context("Failed to load YAML configuration")?;

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

    let processed = process_records(filtered_records, args.pe_only, None)?;
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
            validate_config(&config, DownloadMethod::Ascp)?;
            download_with_ascp(&processed, &config, args).await?;
        }
        DownloadMethod::Ftp => {
            download_with_ftp(&processed, &config, args).await?;
        }
        DownloadMethod::Prefetch => {
            validate_config(&config, DownloadMethod::Prefetch)?;
            validate_config(&config, DownloadMethod::Aws)?;
            download_with_prefetch(&processed, &config, args).await?;
        }
        DownloadMethod::Aws => {
            validate_config(&config, DownloadMethod::Aws)?;
            download_with_aws(&processed, &config, args).await?;
        }
        DownloadMethod::Auto => {
            info!("🤖 Auto Mode: Attempting AWS S3 first...");
            validate_config(&config, DownloadMethod::Aws)?;
            validate_config(&config, DownloadMethod::Prefetch)?;
            // Note: In a full production system, we would track individual file failures.
            // Here we attempt AWS. If it completes, great.
            // If the entire batch fails (e.g. API error), we catch it and try Prefetch.
            if let Err(e) = download_with_aws(&processed, &config, args).await {
                warn!("⚠️  AWS S3 download encountered issues: {}. Switching to Prefetch...", e);
                download_with_prefetch(&processed, &config, args).await?;
            }
        }
    }

    info!("🎉 {} download completed successfully!", SCRIPT_NAME);
    Ok(())
}

// ============================================================
// Upload Command Entry Point (NEW)
// ============================================================

async fn run_upload(args: &UploadArgs) -> Result<()> {
    ebidownload_core::upload::run_upload(
        &args.bucket,
        &args.prefix,
        &args.files,
        &args.region,
        args.concurrent,
        args.apply_policy,
        &args.metadata_template,
        args.dry_run,
    )
    .await
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

              🧬  EMBL-ENA Data Toolkit    |  v{}"#,
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

    // stdout layer writes through MpWriter so that log messages are rendered
    // above active progress bars via MultiProgress::println(), preventing
    // display corruption when progress bars and logs share the terminal.
    match format {
        LogFormat::Json => {
            let json_layer = fmt::layer()
                .json()
                .with_writer(|| MpWriter { buf: Vec::new() })
                .with_timer(fmt::time::LocalTime::rfc_3339())
                .flatten_event(true)
                .with_target(false)
                .with_filter(stdout_filter);

            let subscriber = tracing_subscriber::registry().with(file_layer).with(json_layer);
            tracing::subscriber::set_global_default(subscriber).context("Failed to set subscriber")?;
        }
        LogFormat::Text => {
            let stdout_layer = fmt::layer()
                .with_writer(|| MpWriter { buf: Vec::new() })
                .with_ansi(false)
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

fn apply_filters(records: Vec<EnaRecord>, filters: &RegexFilters) -> Result<Vec<EnaRecord>> {
    let mut filtered = Vec::new();
    let mut filtered_count = 0;
    for record in records {
        if filters.should_include(&record) { filtered.push(record); } else { filtered_count += 1; }
    }
    if filtered_count > 0 { info!("🔍 Filtered out {} records based on regex patterns", filtered_count); }
    Ok(filtered)
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
async fn download_with_prefetch(records: &[ProcessedRecord], config: &Config, args: &DownloadArgs) -> Result<()> {
    ebidownload_core::prefetch::download_all(records, config, &args.output, args.multithreads, args.aws_threads, &args.prefetch_max_size, args.cleanup_sra).await
}

// AWS Entry (Keep original logic)
async fn download_with_aws(records: &[ProcessedRecord], config: &Config, args: &DownloadArgs) -> Result<()> {
    info!("☁️  Starting AWS S3 downloads...");

    let file_concurrency = args.multithreads;
    let chunk_concurrency = args.aws_threads;
    let process_threads = if args.aws_threads > 4 { args.aws_threads } else { 4 };
    let chunk_size_mb = args.chunk_size;

    info!("⚙️  Config: Parallel Files = {}, Threads/File = {}, Chunk Size = {}MB", file_concurrency, chunk_concurrency, chunk_size_mb);

    let semaphore = Arc::new(Semaphore::new(file_concurrency));
    let mp = Arc::new(GLOBAL_MP.clone());
    BARS_ACTIVE.store(true, std::sync::atomic::Ordering::Relaxed);
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

            let metadata = ebidownload_core::aws_s3::SraUtils::get_metadata(&run_id, None).await?;
            let sra_filename = format!("{}.sra", run_id);
            info!(target: "download_detail", "📥 [{}] Step 1: Downloading via AWS S3...", run_id);

            if let Some(sra_metadata) = metadata {
                let downloader = ebidownload_core::aws_s3::ResumableDownloader::new(
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
                info!(target: "download_detail", "⏩ [{}] FASTQ files already exist, skipping conversion.", run_id);
            } else {
                info!(target: "download_detail", "🔄 [{}] Step 2: Converting (fasterq-dump)...", run_id);
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

                info!(target: "download_detail", "📦 [{}] Step 3: Compressing (pigz)...", run_id);
                // Pigz with wildcard still needs shell or glob expansion.
                // Using bash -c here is acceptable for wildcard, but we can make it slightly safer by avoiding string formatting if possible.
                // However, pigz *.fastq is inherently shell-dependent unless we expand in Rust.
                // For simplicity/robustness, we keep the run_command (shell) for pigz as it is complex to reimplement globbing.
                run_command(&cmd_compress, &output_dir).await.context("pigz failed")?;

                if cleanup_sra {
                    let sra_path = output_dir.join(&sra_filename);
                    if sra_path.exists() {
                        info!(target: "download_detail", "🧹 [{}] Cleaning up SRA file: {}", run_id, sra_path.display());
                        if let Err(e) = tokio::fs::remove_file(&sra_path).await {
                            warn!("⚠️ [{}] Failed to remove SRA file: {}", run_id, e);
                        }
                    }
                }

                info!("✅ [{}] Done", run_id);
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
    BARS_ACTIVE.store(false, std::sync::atomic::Ordering::Relaxed);
    info!("🎉 All AWS S3 tasks completed");
    Ok(())
}

// FTP Entry
async fn download_with_ftp(records: &[ProcessedRecord], config: &Config, args: &DownloadArgs) -> Result<()> {
    // 🟢 Call ftp.rs, pass file size to enable percentage progress bar
    ebidownload_core::ftp::process_downloads(
        records,
        config,
        &args.output,
        ebidownload_core::ftp::Protocol::Ftp,
        args.multithreads
    ).await
}

// Aspera Entry
async fn download_with_ascp(records: &[ProcessedRecord], config: &Config, args: &DownloadArgs) -> Result<()> {
    ebidownload_core::ftp::process_downloads(
        records,
        config,
        &args.output,
        ebidownload_core::ftp::Protocol::Ascp,
        args.multithreads
    ).await
}

