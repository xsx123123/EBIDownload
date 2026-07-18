//! Tauri commands and event system for the GUI application

use crate::*;
use ::tauri::{Emitter, State};
use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

// ============================================================
// App State
// ============================================================

pub struct AppState {
    config: Mutex<Option<Config>>,
    config_path: Mutex<PathBuf>,
    is_downloading: Arc<Mutex<bool>>,
    is_uploading: Arc<Mutex<bool>>,
    download_handle: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
    upload_handle: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
    download_pause_token: Arc<Mutex<Option<crate::aws_s3::PauseToken>>>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            config: Mutex::new(None),
            config_path: Mutex::new(default_config_path()),
            is_downloading: Arc::new(Mutex::new(false)),
            is_uploading: Arc::new(Mutex::new(false)),
            download_handle: Arc::new(Mutex::new(None)),
            upload_handle: Arc::new(Mutex::new(None)),
            download_pause_token: Arc::new(Mutex::new(None)),
        }
    }
}

/// Returns the default user-specific config path.
/// All platforms: ~/.polariseq/polariseq.yaml
fn default_config_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".polariseq")
        .join("polariseq.yaml")
}

/// Ensure the parent directory of the given path exists.
fn ensure_parent_dir(path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        if !parent.exists() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create config directory: {}", e))?;
        }
    }
    Ok(())
}

/// Create a minimal default config file if it does not already exist.
fn ensure_default_config(path: &Path) -> Result<(), String> {
    ensure_parent_dir(path)?;
    if !path.exists() {
        let default = serde_yaml::to_string(&serde_json::json!({
            "software": {
                "prefetch": "",
                "fasterq_dump": "",
            }
        }))
        .map_err(|e| format!("Failed to serialize default config: {}", e))?;
        fs::write(path, default).map_err(|e| format!("Failed to write default config: {}", e))?;
    }
    Ok(())
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================
// Progress Events
// ============================================================

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", content = "data")]
pub enum DownloadEvent {
    Log {
        level: String,
        message: String,
    },
    Progress {
        run_id: String,
        percent: f64,
        status: String,
        speed_mbps: f64,
    },
    Started {
        total: usize,
    },
    Completed,
    Error {
        message: String,
    },
    DryRun {
        files: Vec<DryRunFile>,
    },
    Metadata {
        records: Vec<EnaRecord>,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct DryRunFile {
    pub run_id: String,
    pub file1: String,
    pub size1: u64,
    pub file2: Option<String>,
    pub size2: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", content = "data")]
pub enum UploadEvent {
    Log {
        level: String,
        message: String,
    },
    Progress {
        filename: String,
        percent: f64,
        status: String,
    },
    Started {
        total: usize,
    },
    Completed,
    Error {
        message: String,
    },
    DryRun {
        files: Vec<String>,
    },
}

// ============================================================
// Tauri Commands
// ============================================================

#[::tauri::command]
pub async fn load_config_command(
    state: State<'_, AppState>,
    path: Option<String>,
) -> Result<(), String> {
    let config_path = path
        .map(PathBuf::from)
        .unwrap_or_else(|| state.config_path.lock().unwrap().clone());
    *state.config_path.lock().unwrap() = config_path.clone();

    // If the requested config does not exist, create a default one.
    ensure_default_config(&config_path)?;

    match load_config(&config_path) {
        Ok(config) => {
            *state.config.lock().unwrap() = Some(config);
            Ok(())
        }
        Err(e) => Err(format!("Failed to load config: {}", e)),
    }
}

#[::tauri::command]
pub async fn get_config_path_command(state: State<'_, AppState>) -> Result<String, String> {
    Ok(state
        .config_path
        .lock()
        .unwrap()
        .to_string_lossy()
        .to_string())
}

#[::tauri::command]
pub async fn set_config_path_command(
    state: State<'_, AppState>,
    path: String,
) -> Result<(), String> {
    let new_path = PathBuf::from(path);
    ensure_default_config(&new_path)?;
    *state.config_path.lock().unwrap() = new_path;
    Ok(())
}

#[::tauri::command]
pub async fn save_config_command(
    state: State<'_, AppState>,
    config: ConfigInput,
) -> Result<(), String> {
    let config_path = state.config_path.lock().unwrap().clone();

    // Make sure the target directory exists before writing.
    ensure_parent_dir(&config_path)?;

    let yaml_config = serde_yaml::to_string(&serde_json::json!({
        "software": {
            "prefetch": config.prefetch_path,
            "fasterq_dump": config.fasterq_dump_path,
        }
    }))
    .map_err(|e| format!("Failed to serialize config: {}", e))?;

    fs::write(&config_path, yaml_config).map_err(|e| format!("Failed to write config: {}", e))?;

    // Reload into state
    let new_config =
        load_config(&config_path).map_err(|e| format!("Failed to reload config: {}", e))?;
    *state.config.lock().unwrap() = Some(new_config);

    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigInput {
    pub prefetch_path: String,
    pub fasterq_dump_path: String,
}

#[::tauri::command]
pub async fn get_config_command(state: State<'_, AppState>) -> Result<Option<Config>, String> {
    let mut config_guard = state.config.lock().unwrap();
    if config_guard.is_none() {
        let config_path = state.config_path.lock().unwrap().clone();
        if let Ok(config) = load_config(&config_path) {
            *config_guard = Some(config);
        }
    }
    Ok(config_guard.clone())
}

#[::tauri::command]
pub async fn fetch_metadata_command(
    accession: Option<String>,
    tsv: Option<String>,
) -> Result<Vec<EnaRecord>, String> {
    if let Some(acc) = accession {
        fetch_ena_data(&acc)
            .await
            .map_err(|e| format!("Failed to fetch metadata: {}", e))
    } else if let Some(tsv_path) = tsv {
        read_tsv_data(&PathBuf::from(tsv_path)).map_err(|e| format!("Failed to read TSV: {}", e))
    } else {
        Err("Either accession or TSV file must be provided".to_string())
    }
}

#[::tauri::command]
pub async fn start_download_command(
    state: State<'_, AppState>,
    options: DownloadOptions,
    app_handle: ::tauri::AppHandle,
) -> Result<(), String> {
    let mut is_downloading = state.is_downloading.lock().unwrap();
    if *is_downloading {
        return Err("Download already in progress".to_string());
    }
    *is_downloading = true;
    drop(is_downloading);

    let config = state.config.lock().unwrap().clone();
    let config = config.ok_or_else(|| "Config not loaded".to_string())?;
    let is_downloading_flag = state.is_downloading.clone();
    let download_handle_slot = state.download_handle.clone();
    let pause_token = crate::aws_s3::PauseToken::new();
    *state.download_pause_token.lock().unwrap() = Some(pause_token.clone());

    // Validate config first
    if let Err(e) = validate_config(&config, options.download_method) {
        let mut is_downloading = state.is_downloading.lock().unwrap();
        *is_downloading = false;
        return Err(format!("Config validation failed: {}", e));
    }

    // Create output directory
    if let Err(e) = std::fs::create_dir_all(&options.output) {
        let mut is_downloading = state.is_downloading.lock().unwrap();
        *is_downloading = false;
        return Err(format!("Failed to create output directory: {}", e));
    }

    // Mirror runtime logs to a file inside the output directory.
    let log_file_path = PathBuf::from(&options.output).join("polariseq.log");
    if let Err(e) = crate::logger::set_log_file(&log_file_path) {
        eprintln!("Failed to set log file at {:?}: {}", log_file_path, e);
    }

    // Spawn download task
    let handle = tokio::spawn(async move {
        let result =
            run_download_async(config, options, app_handle.clone(), Some(pause_token)).await;

        let mut is_downloading = is_downloading_flag.lock().unwrap();
        *is_downloading = false;

        if let Err(e) = result {
            eprintln!("Download failed: {}", e);
            let _ = app_handle.emit(
                "download-event",
                DownloadEvent::Error {
                    message: e.to_string(),
                },
            );
        }

        crate::logger::clear_log_file();
    });
    *download_handle_slot.lock().unwrap() = Some(handle);

    Ok(())
}

async fn run_download_async(
    config: Config,
    options: DownloadOptions,
    app_handle: ::tauri::AppHandle,
    pause_token: Option<crate::aws_s3::PauseToken>,
) -> Result<()> {
    let records = if let Some(accession) = &options.accession {
        fetch_ena_data(accession).await?
    } else if let Some(tsv_path) = &options.tsv {
        read_tsv_data(tsv_path)?
    } else {
        return Err(anyhow!("Either --accession or --tsv must be provided"));
    };

    app_handle.emit(
        "download-event",
        DownloadEvent::Metadata {
            records: records.clone(),
        },
    )?;

    let filters = RegexFilters::new(&options)?;
    let processed = process_records(records, options.pe_only, Some(&filters))?;

    if processed.is_empty() {
        app_handle.emit("download-event", DownloadEvent::Log {
            level: "warn".to_string(),
            message: "Records were found, but none have downloadable FASTQ/SRA files. The data may not have been synced to SRA/ENA yet. Please try again later.".to_string(),
        })?;
        return Err(anyhow!(
            "No downloadable records found; data may not be synced to SRA/ENA yet"
        ));
    }

    app_handle.emit(
        "download-event",
        DownloadEvent::Started {
            total: processed.len(),
        },
    )?;

    if options.dry_run {
        let mut dry_run_files = Vec::new();
        for record in &processed {
            dry_run_files.push(DryRunFile {
                run_id: record.run_accession.clone(),
                file1: record.fastq_ftp_1_name.clone(),
                size1: record.fastq_bytes_1,
                file2: record.fastq_ftp_2_name.clone(),
                size2: record.fastq_bytes_2,
            });
        }
        app_handle.emit(
            "download-event",
            DownloadEvent::DryRun {
                files: dry_run_files,
            },
        )?;
        app_handle.emit("download-event", DownloadEvent::Completed)?;
        return Ok(());
    }

    match options.download_method {
        DownloadMethod::Aws => {
            download_aws(processed, config, options, app_handle, pause_token).await
        }
        DownloadMethod::Prefetch => download_prefetch(processed, config, options, app_handle).await,
        DownloadMethod::Ftp => download_ftp(processed, config, options, app_handle).await,
        DownloadMethod::Auto => {
            app_handle.emit(
                "download-event",
                DownloadEvent::Log {
                    level: "info".to_string(),
                    message: "Auto mode: trying AWS S3 first...".to_string(),
                },
            )?;
            if let Err(e) = download_aws(
                processed.clone(),
                config.clone(),
                options.clone(),
                app_handle.clone(),
                pause_token.clone(),
            )
            .await
            {
                app_handle.emit(
                    "download-event",
                    DownloadEvent::Log {
                        level: "warn".to_string(),
                        message: format!("AWS failed ({}), falling back to Prefetch", e),
                    },
                )?;
                download_prefetch(processed, config, options, app_handle).await
            } else {
                Ok(())
            }
        }
    }
}

// ============================================================
// Download Engine Helpers
// ============================================================

async fn download_aws(
    processed: Vec<ProcessedRecord>,
    config: Config,
    options: DownloadOptions,
    app_handle: ::tauri::AppHandle,
    pause_token: Option<crate::aws_s3::PauseToken>,
) -> Result<()> {
    let file_concurrency = options.multithreads;
    let chunk_concurrency = options.aws_threads;
    let process_threads = if options.aws_threads > 4 {
        options.aws_threads
    } else {
        4
    };
    let chunk_size_mb = options.chunk_size;
    let output_dir = options.output;
    let fasterq_dump = config.software.fasterq_dump.display().to_string();
    let cleanup_sra = options.cleanup_sra;

    let semaphore = Arc::new(tokio::sync::Semaphore::new(file_concurrency));
    let mut handles = Vec::new();

    for record in processed {
        let run_id = record.run_accession.clone();
        let output_dir = output_dir.clone();
        let sem = semaphore.clone();
        let app_handle = app_handle.clone();
        let max_workers = chunk_concurrency;
        let chunk_size = chunk_size_mb;
        let fasterq_dump = fasterq_dump.clone();
        let pause_token = pause_token.clone();

        let handle = tokio::spawn(async move {
            let _permit = sem.acquire().await.expect("semaphore closed");

            app_handle.emit(
                "download-event",
                DownloadEvent::Progress {
                    run_id: run_id.clone(),
                    percent: 0.0,
                    status: "Downloading".to_string(),
                    speed_mbps: 0.0,
                },
            )?;

            let metadata = crate::aws_s3::SraUtils::get_metadata(&run_id, None).await?;
            if let Some(sra_metadata) = metadata {
                let total_size = sra_metadata.size;
                let progress_bytes = Arc::new(AtomicU64::new(0));
                let progress_bytes_monitor = progress_bytes.clone();
                let app_handle_monitor = app_handle.clone();
                let run_id_monitor = run_id.clone();

                // Periodically report real-time download progress and speed to the frontend.
                let monitor_handle = tokio::spawn(async move {
                    let mut interval = tokio::time::interval(Duration::from_millis(500));
                    let mut last_bytes = 0u64;
                    let mut last_instant = Instant::now();
                    loop {
                        interval.tick().await;
                        let bytes = progress_bytes_monitor.load(Ordering::Relaxed);
                        let percent = if total_size > 0 {
                            (bytes as f64 / total_size as f64) * 50.0
                        } else {
                            0.0
                        };

                        let elapsed = last_instant.elapsed().as_secs_f64().max(1e-6);
                        let delta_bytes = bytes.saturating_sub(last_bytes);
                        let speed_mbps = (delta_bytes as f64 / 1024.0 / 1024.0) / elapsed;

                        last_bytes = bytes;
                        last_instant = Instant::now();

                        let _ = app_handle_monitor.emit(
                            "download-event",
                            DownloadEvent::Progress {
                                run_id: run_id_monitor.clone(),
                                percent,
                                status: "Downloading".to_string(),
                                speed_mbps,
                            },
                        );
                    }
                });

                let downloader = crate::aws_s3::ResumableDownloader::new(
                    run_id.clone(),
                    sra_metadata,
                    output_dir.clone(),
                    chunk_size,
                    max_workers,
                    None,
                    None,
                )
                .await?
                .with_progress_bytes(progress_bytes)
                .with_pause_token(pause_token.clone().unwrap_or_default());

                let result = downloader.start().await;
                monitor_handle.abort();
                let success = result?;
                if !success {
                    return Err(anyhow::anyhow!("Download failed for {}", run_id));
                }
            } else {
                return Err(anyhow::anyhow!("No S3 URI for {}", run_id));
            }

            app_handle.emit(
                "download-event",
                DownloadEvent::Progress {
                    run_id: run_id.clone(),
                    percent: 50.0,
                    status: "Converting".to_string(),
                    speed_mbps: 0.0,
                },
            )?;

            // fasterq-dump
            let sra_filename = format!("{}.sra", run_id);
            let fq_1 = output_dir.join(format!("{}_1.fastq", run_id));
            let fq_single = output_dir.join(format!("{}.fastq", run_id));

            let fq_exists = (fq_1.exists() && fq_1.metadata()?.len() > 0)
                || (fq_single.exists() && fq_single.metadata()?.len() > 0);

            if !fq_exists {
                let output = tokio::process::Command::new(&fasterq_dump)
                    .arg("--split-3")
                    .arg("-e")
                    .arg(process_threads.to_string())
                    .arg("-O")
                    .arg(".")
                    .arg("-f")
                    .arg(&sra_filename)
                    .current_dir(&output_dir)
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::piped())
                    .output()
                    .await;

                match output {
                    Ok(out) if !out.status.success() => {
                        let _ = app_handle.emit(
                            "download-event",
                            DownloadEvent::Log {
                                level: "warn".to_string(),
                                message: format!(
                                    "fasterq-dump warning for {}: {}",
                                    run_id,
                                    String::from_utf8_lossy(&out.stderr)
                                ),
                            },
                        );
                    }
                    Ok(_) => {}
                    Err(e) => {
                        let _ = app_handle.emit(
                            "download-event",
                            DownloadEvent::Log {
                                level: "warn".to_string(),
                                message: format!("fasterq-dump exec error for {}: {}", run_id, e),
                            },
                        );
                    }
                }
            }

            // Compress
            let fq_exists_after = (fq_1.exists() && fq_1.metadata()?.len() > 0)
                || (fq_single.exists() && fq_single.metadata()?.len() > 0);

            if fq_exists_after {
                app_handle.emit(
                    "download-event",
                    DownloadEvent::Progress {
                        run_id: run_id.clone(),
                        percent: 75.0,
                        status: "Compressing".to_string(),
                        speed_mbps: 0.0,
                    },
                )?;

                let output_dir_compress = output_dir.clone();
                let run_id_compress = run_id.clone();
                tokio::task::spawn_blocking(move || {
                    crate::compress_fastq_files(
                        &output_dir_compress,
                        &run_id_compress,
                        process_threads,
                        None,
                    )
                })
                .await
                .context("Compression task panicked")?
                .context("Compression failed")?;

                if cleanup_sra {
                    let sra_path = output_dir.join(&sra_filename);
                    if sra_path.exists() {
                        let _ = tokio::fs::remove_file(&sra_path).await;
                    }
                }

                app_handle.emit(
                    "download-event",
                    DownloadEvent::Progress {
                        run_id: run_id.clone(),
                        percent: 100.0,
                        status: "Completed".to_string(),
                        speed_mbps: 0.0,
                    },
                )?;
            } else {
                return Err(anyhow::anyhow!("Conversion failed for {}", run_id));
            }

            Ok::<_, anyhow::Error>(())
        });
        handles.push(handle);
    }

    for handle in handles {
        if let Err(e) = handle.await {
            let _ = app_handle.emit(
                "download-event",
                DownloadEvent::Log {
                    level: "warn".to_string(),
                    message: format!("Download task error: {}", e),
                },
            );
        }
    }

    app_handle.emit("download-event", DownloadEvent::Completed)?;
    Ok(())
}

async fn download_prefetch(
    processed: Vec<ProcessedRecord>,
    config: Config,
    options: DownloadOptions,
    app_handle: ::tauri::AppHandle,
) -> Result<()> {
    app_handle.emit(
        "download-event",
        DownloadEvent::Log {
            level: "info".to_string(),
            message: "Starting Prefetch download...".to_string(),
        },
    )?;

    // Show indeterminate progress for each run while Prefetch works in batch mode.
    for record in &processed {
        app_handle.emit(
            "download-event",
            DownloadEvent::Progress {
                run_id: record.run_accession.clone(),
                percent: 0.0,
                status: "Downloading".to_string(),
                speed_mbps: 0.0,
            },
        )?;
    }

    crate::prefetch::download_all(
        &processed,
        &config,
        &options.output,
        options.multithreads,
        options.aws_threads,
        &options.prefetch_max_size,
        options.cleanup_sra,
    )
    .await?;

    for record in &processed {
        app_handle.emit(
            "download-event",
            DownloadEvent::Progress {
                run_id: record.run_accession.clone(),
                percent: 100.0,
                status: "Completed".to_string(),
                speed_mbps: 0.0,
            },
        )?;
    }

    app_handle.emit("download-event", DownloadEvent::Completed)?;
    Ok(())
}

async fn download_ftp(
    processed: Vec<ProcessedRecord>,
    config: Config,
    options: DownloadOptions,
    app_handle: ::tauri::AppHandle,
) -> Result<()> {
    app_handle.emit(
        "download-event",
        DownloadEvent::Log {
            level: "info".to_string(),
            message: "Starting FTP download...".to_string(),
        },
    )?;

    // Show indeterminate progress for each run while FTP works in batch mode.
    for record in &processed {
        app_handle.emit(
            "download-event",
            DownloadEvent::Progress {
                run_id: record.run_accession.clone(),
                percent: 0.0,
                status: "Downloading".to_string(),
                speed_mbps: 0.0,
            },
        )?;
    }

    crate::ftp::process_downloads(
        &processed,
        &config,
        &options.output,
        crate::ftp::Protocol::Ftp,
        options.multithreads,
    )
    .await?;

    for record in &processed {
        app_handle.emit(
            "download-event",
            DownloadEvent::Progress {
                run_id: record.run_accession.clone(),
                percent: 100.0,
                status: "Completed".to_string(),
                speed_mbps: 0.0,
            },
        )?;
    }

    app_handle.emit("download-event", DownloadEvent::Completed)?;
    Ok(())
}

#[::tauri::command]
pub async fn start_upload_command(
    state: State<'_, AppState>,
    options: UploadOptions,
    app_handle: ::tauri::AppHandle,
) -> Result<(), String> {
    let mut is_uploading = state.is_uploading.lock().unwrap();
    if *is_uploading {
        return Err("Upload already in progress".to_string());
    }
    *is_uploading = true;
    drop(is_uploading);

    let is_uploading_flag = state.is_uploading.clone();
    let upload_handle_slot = state.upload_handle.clone();

    // Spawn upload task
    let handle = tokio::spawn(async move {
        let result = run_upload_async(options, app_handle.clone()).await;

        let mut is_uploading = is_uploading_flag.lock().unwrap();
        *is_uploading = false;

        if let Err(e) = result {
            eprintln!("Upload failed: {}", e);
            let _ = app_handle.emit(
                "upload-event",
                UploadEvent::Error {
                    message: e.to_string(),
                },
            );
        }
    });
    *upload_handle_slot.lock().unwrap() = Some(handle);

    Ok(())
}

async fn run_upload_async(options: UploadOptions, app_handle: ::tauri::AppHandle) -> Result<()> {
    // Send started event
    let file_count = options.files.len();
    app_handle.emit("upload-event", UploadEvent::Started { total: file_count })?;

    if options.dry_run {
        let filenames: Vec<String> = options
            .files
            .iter()
            .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
            .collect();
        app_handle.emit("upload-event", UploadEvent::DryRun { files: filenames })?;
        app_handle.emit("upload-event", UploadEvent::Completed)?;
        return Ok(());
    }

    let app_handle_cb = app_handle.clone();
    let progress_cb: crate::upload::UploadProgressCallback = Arc::new(move |event| {
        let status = match event.status {
            crate::upload::UploadProgressStatus::Started => "Uploading",
            crate::upload::UploadProgressStatus::Completed => "Completed",
            crate::upload::UploadProgressStatus::Failed => "Failed",
        };
        let _ = app_handle_cb.emit(
            "upload-event",
            UploadEvent::Progress {
                filename: event.filename,
                percent: event.percent,
                status: status.to_string(),
            },
        );
    });

    crate::upload::run_upload(
        &options.bucket,
        &options.prefix,
        &options.files,
        &options.region,
        options.concurrent,
        options.apply_policy,
        &options.metadata_template,
        options.dry_run,
        Some(progress_cb),
    )
    .await?;

    app_handle.emit("upload-event", UploadEvent::Completed)?;
    Ok(())
}

#[::tauri::command]
pub async fn pause_download_command(
    state: State<'_, AppState>,
    paused: bool,
) -> Result<(), String> {
    let token = state.download_pause_token.lock().unwrap().clone();
    if let Some(token) = token {
        if paused {
            token.pause();
        } else {
            token.resume();
        }
    }
    Ok(())
}

#[::tauri::command]
pub async fn cancel_download_command(state: State<'_, AppState>) -> Result<(), String> {
    let mut is_downloading = state.is_downloading.lock().unwrap();
    *is_downloading = false;
    drop(is_downloading);

    if let Some(handle) = state.download_handle.lock().unwrap().take() {
        handle.abort();
    }
    Ok(())
}

#[::tauri::command]
pub async fn cancel_upload_command(state: State<'_, AppState>) -> Result<(), String> {
    let mut is_uploading = state.is_uploading.lock().unwrap();
    *is_uploading = false;
    drop(is_uploading);

    if let Some(handle) = state.upload_handle.lock().unwrap().take() {
        handle.abort();
    }
    Ok(())
}

// ============================================================
// Dependency Management Commands
// ============================================================

#[::tauri::command]
pub async fn check_deps_command(
    state: State<'_, AppState>,
) -> Result<crate::deps::DepStatus, String> {
    let mut config_guard = state.config.lock().unwrap();
    if config_guard.is_none() {
        let config_path = state.config_path.lock().unwrap().clone();
        if let Ok(config) = load_config(&config_path) {
            *config_guard = Some(config);
        }
    }
    Ok(crate::deps::check_sra_tools(config_guard.as_ref()))
}

/// Structured progress payload forwarded to the frontend during sra-tools installation.
#[derive(Debug, Clone, serde::Serialize)]
struct DepInstallProgress {
    step: String,
    percent: f64,
    message: String,
}

#[::tauri::command]
pub async fn install_deps_command(
    state: State<'_, AppState>,
    app_handle: ::tauri::AppHandle,
) -> Result<(), String> {
    let config_path = state.config_path.lock().unwrap().clone();
    let app_handle_cb = app_handle.clone();

    let progress_cb: crate::deps::DepProgressCallback = Arc::new(move |event| {
        let (level, message, progress) = match event {
            crate::deps::DepProgressEvent::DownloadStarted { url, size } => {
                let size_str = size
                    .map(|s| format!("{} bytes", s))
                    .unwrap_or_else(|| "unknown".to_string());
                let message = format!("Downloading sra-tools from {} ({})", url, size_str);
                (
                    "info".to_string(),
                    message.clone(),
                    DepInstallProgress {
                        step: "download".to_string(),
                        percent: 0.0,
                        message,
                    },
                )
            }
            crate::deps::DepProgressEvent::DownloadProgress { downloaded, total } => {
                let percent = total
                    .map(|t| (downloaded as f64 / t as f64) * 100.0)
                    .unwrap_or(0.0);
                // Map download progress to 0-70% of the overall installation.
                let overall = percent * 0.7;
                let message = format!("Downloading sra-tools: {:.1}%", percent);
                (
                    "info".to_string(),
                    message.clone(),
                    DepInstallProgress {
                        step: "download".to_string(),
                        percent: overall,
                        message,
                    },
                )
            }
            crate::deps::DepProgressEvent::DownloadCompleted => {
                let message = "Download completed".to_string();
                (
                    "info".to_string(),
                    message.clone(),
                    DepInstallProgress {
                        step: "download".to_string(),
                        percent: 70.0,
                        message,
                    },
                )
            }
            crate::deps::DepProgressEvent::Verifying => {
                let message = "Verifying checksum...".to_string();
                (
                    "info".to_string(),
                    message.clone(),
                    DepInstallProgress {
                        step: "verify".to_string(),
                        percent: 75.0,
                        message,
                    },
                )
            }
            crate::deps::DepProgressEvent::Extracting => {
                let message = "Extracting sra-tools...".to_string();
                (
                    "info".to_string(),
                    message.clone(),
                    DepInstallProgress {
                        step: "extract".to_string(),
                        percent: 85.0,
                        message,
                    },
                )
            }
            crate::deps::DepProgressEvent::Completed => {
                let message = "sra-tools installation completed".to_string();
                (
                    "info".to_string(),
                    message.clone(),
                    DepInstallProgress {
                        step: "complete".to_string(),
                        percent: 100.0,
                        message,
                    },
                )
            }
            crate::deps::DepProgressEvent::Error { message: err } => {
                let message = format!("Installation error: {}", err);
                (
                    "error".to_string(),
                    message.clone(),
                    DepInstallProgress {
                        step: "error".to_string(),
                        percent: 0.0,
                        message,
                    },
                )
            }
        };
        let _ = app_handle_cb.emit("app-log", crate::logger::LogEntry { level, message });
        let _ = app_handle_cb.emit("dep-progress", progress);
    });

    let paths = crate::deps::install_sra_tools(None, None, Some(progress_cb))
        .await
        .map_err(|e| e.to_string())?;

    crate::deps::write_software_paths_to_yaml(&config_path, &paths).map_err(|e| e.to_string())?;

    // Reload config into state
    let new_config = load_config(&config_path).map_err(|e| e.to_string())?;
    *state.config.lock().unwrap() = Some(new_config);

    app_handle
        .emit("deps-installed", ())
        .map_err(|e| e.to_string())?;
    Ok(())
}
