//! Tauri commands and event system for the GUI application

use crate::*;
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use serde_json;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use ::tauri::{State, Emitter};

// ============================================================
// App State
// ============================================================

pub struct AppState {
    config: Mutex<Option<Config>>,
    config_path: Mutex<PathBuf>,
    is_downloading: Arc<Mutex<bool>>,
    is_uploading: Arc<Mutex<bool>>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            config: Mutex::new(None),
            config_path: Mutex::new(PathBuf::from("EBIDownload.yaml")),
            is_downloading: Arc::new(Mutex::new(false)),
            is_uploading: Arc::new(Mutex::new(false)),
        }
    }
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
    Log { level: String, message: String },
    Progress { run_id: String, percent: f64, status: String },
    Started { total: usize },
    Completed,
    Error { message: String },
    DryRun { files: Vec<DryRunFile> },
    Metadata { records: Vec<EnaRecord> },
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
    Log { level: String, message: String },
    Progress { filename: String, percent: f64, status: String },
    Started { total: usize },
    Completed,
    Error { message: String },
    DryRun { files: Vec<String> },
}

// ============================================================
// Tauri Commands
// ============================================================

#[::tauri::command]
pub async fn load_config_command(
    state: State<'_, AppState>,
    path: Option<String>,
) -> Result<(), String> {
    let config_path = path.map(PathBuf::from).unwrap_or_else(|| PathBuf::from("EBIDownload.yaml"));
    *state.config_path.lock().unwrap() = config_path.clone();

    match load_config(&config_path) {
        Ok(config) => {
            *state.config.lock().unwrap() = Some(config);
            Ok(())
        }
        Err(e) => Err(format!("Failed to load config: {}", e)),
    }
}

#[::tauri::command]
pub async fn save_config_command(
    state: State<'_, AppState>,
    config: ConfigInput,
) -> Result<(), String> {
    let config_path = state.config_path.lock().unwrap().clone();

    let yaml_config = serde_yaml::to_string(&serde_json::json!({
        "software": {
            "ascp": "",
            "prefetch": config.prefetch_path,
            "fasterq_dump": config.fasterq_dump_path,
        },
        "setting": {
            "openssh": "",
        }
    }))
    .map_err(|e| format!("Failed to serialize config: {}", e))?;

    std::fs::write(&config_path, yaml_config)
        .map_err(|e| format!("Failed to write config: {}", e))?;

    // Reload into state
    let new_config = load_config(&config_path)
        .map_err(|e| format!("Failed to reload config: {}", e))?;
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
    Ok(state.config.lock().unwrap().clone())
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
        read_tsv_data(&PathBuf::from(tsv_path))
            .map_err(|e| format!("Failed to read TSV: {}", e))
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

    // Spawn download task
    tokio::spawn(async move {
        let result = run_download_async(config, options, app_handle.clone()).await;

        let mut is_downloading = is_downloading_flag.lock().unwrap();
        *is_downloading = false;

        if let Err(e) = result {
            eprintln!("Download failed: {}", e);
            let _ = app_handle.emit("download-event", DownloadEvent::Error { message: e.to_string() });
        }
    });

    Ok(())
}

async fn run_download_async(
    config: Config,
    options: DownloadOptions,
    app_handle: ::tauri::AppHandle,
) -> Result<()> {
    let records = if let Some(accession) = &options.accession {
        fetch_ena_data(accession).await?
    } else if let Some(tsv_path) = &options.tsv {
        read_tsv_data(tsv_path)?
    } else {
        return Err(anyhow!("Either --accession or --tsv must be provided"));
    };

    app_handle.emit("download-event", DownloadEvent::Metadata { records: records.clone() })?;
    let processed = process_records(records, options.pe_only)?;
    app_handle.emit("download-event", DownloadEvent::Started { total: processed.len() })?;

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
        app_handle.emit("download-event", DownloadEvent::DryRun { files: dry_run_files })?;
        app_handle.emit("download-event", DownloadEvent::Completed)?;
        return Ok(());
    }

    match options.download_method {
        DownloadMethod::Aws => download_aws(processed, config, options, app_handle).await,
        DownloadMethod::Prefetch => download_prefetch(processed, config, options, app_handle).await,
        DownloadMethod::Ftp => download_ftp(processed, config, options, app_handle, crate::ftp::Protocol::Ftp).await,
        DownloadMethod::Ascp => download_ftp(processed, config, options, app_handle, crate::ftp::Protocol::Ascp).await,
        DownloadMethod::Auto => {
            app_handle.emit("download-event", DownloadEvent::Log {
                level: "info".to_string(),
                message: "Auto mode: trying AWS S3 first...".to_string(),
            })?;
            if let Err(e) = download_aws(processed.clone(), config.clone(), options.clone(), app_handle.clone()).await {
                app_handle.emit("download-event", DownloadEvent::Log {
                    level: "warn".to_string(),
                    message: format!("AWS failed ({}), falling back to Prefetch", e),
                })?;
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
) -> Result<()> {
    let file_concurrency = options.multithreads;
    let chunk_concurrency = options.aws_threads;
    let process_threads = if options.aws_threads > 4 { options.aws_threads } else { 4 };
    let chunk_size_mb = options.chunk_size;
    let output_dir = options.output;
    let fasterq_dump = config.software.fasterq_dump.display().to_string();
    let pigz = "pigz";
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
        let pigz = pigz.to_string();
        let cleanup_sra = cleanup_sra;

        let handle = tokio::spawn(async move {
            let _permit = sem.acquire().await.expect("semaphore closed");

            app_handle.emit("download-event", DownloadEvent::Progress {
                run_id: run_id.clone(),
                percent: 0.0,
                status: "Downloading".to_string(),
            })?;

            let metadata = crate::aws_s3::SraUtils::get_metadata(&run_id, None).await?;
            if let Some(sra_metadata) = metadata {
                let downloader = crate::aws_s3::ResumableDownloader::new(
                    run_id.clone(),
                    sra_metadata,
                    output_dir.clone(),
                    chunk_size,
                    max_workers,
                    None,
                ).await?;
                let success = downloader.start().await?;
                if !success {
                    return Err(anyhow::anyhow!("Download failed for {}", run_id));
                }
            } else {
                return Err(anyhow::anyhow!("No S3 URI for {}", run_id));
            }

            app_handle.emit("download-event", DownloadEvent::Progress {
                run_id: run_id.clone(),
                percent: 50.0,
                status: "Converting".to_string(),
            })?;

            // fasterq-dump
            let sra_filename = format!("{}.sra", run_id);
            let fq_1 = output_dir.join(format!("{}_1.fastq", run_id));
            let fq_single = output_dir.join(format!("{}.fastq", run_id));

            let fq_exists = (fq_1.exists() && fq_1.metadata()?.len() > 0)
                || (fq_single.exists() && fq_single.metadata()?.len() > 0);

            if !fq_exists {
                let output = tokio::process::Command::new(&fasterq_dump)
                    .arg("--split-3")
                    .arg("-e").arg(process_threads.to_string())
                    .arg("-O").arg(".")
                    .arg("-f")
                    .arg(&sra_filename)
                    .current_dir(&output_dir)
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::piped())
                    .output()
                    .await;

                match output {
                    Ok(out) if !out.status.success() => {
                        let _ = app_handle.emit("download-event", DownloadEvent::Log {
                            level: "warn".to_string(),
                            message: format!("fasterq-dump warning for {}: {}", run_id, String::from_utf8_lossy(&out.stderr)),
                        });
                    }
                    Ok(_) => {}
                    Err(e) => {
                        let _ = app_handle.emit("download-event", DownloadEvent::Log {
                            level: "warn".to_string(),
                            message: format!("fasterq-dump exec error for {}: {}", run_id, e),
                        });
                    }
                }
            }

            // pigz
            let cmd_compress = format!("{} -p {} {}*.fastq", pigz, process_threads, run_id);
            let fq_exists_after = (fq_1.exists() && fq_1.metadata()?.len() > 0)
                || (fq_single.exists() && fq_single.metadata()?.len() > 0);

            if fq_exists_after {
                let output = tokio::process::Command::new("bash")
                    .arg("-c")
                    .arg(&cmd_compress)
                    .current_dir(&output_dir)
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::piped())
                    .output()
                    .await?;

                if !output.status.success() {
                    return Err(anyhow::anyhow!("pigz failed for {}", run_id));
                }

                if cleanup_sra {
                    let sra_path = output_dir.join(&sra_filename);
                    if sra_path.exists() {
                        let _ = tokio::fs::remove_file(&sra_path).await;
                    }
                }

                app_handle.emit("download-event", DownloadEvent::Progress {
                    run_id: run_id.clone(),
                    percent: 100.0,
                    status: "Completed".to_string(),
                })?;
            } else {
                return Err(anyhow::anyhow!("Conversion failed for {}", run_id));
            }

            Ok::<_, anyhow::Error>(())
        });
        handles.push(handle);
    }

    for handle in handles {
        if let Err(e) = handle.await {
            let _ = app_handle.emit("download-event", DownloadEvent::Log {
                level: "warn".to_string(),
                message: format!("Download task error: {}", e),
            });
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
    app_handle.emit("download-event", DownloadEvent::Log {
        level: "info".to_string(),
        message: "Starting Prefetch download...".to_string(),
    })?;

    crate::prefetch::download_all(
        &processed,
        &config,
        &options.output,
        options.multithreads,
        options.aws_threads,
        &options.prefetch_max_size,
        options.cleanup_sra,
    ).await?;

    app_handle.emit("download-event", DownloadEvent::Completed)?;
    Ok(())
}

async fn download_ftp(
    processed: Vec<ProcessedRecord>,
    config: Config,
    options: DownloadOptions,
    app_handle: ::tauri::AppHandle,
    protocol: crate::ftp::Protocol,
) -> Result<()> {
    let protocol_name = match protocol {
        crate::ftp::Protocol::Ftp => "FTP",
        crate::ftp::Protocol::Ascp => "Aspera",
    };
    app_handle.emit("download-event", DownloadEvent::Log {
        level: "info".to_string(),
        message: format!("Starting {} download...", protocol_name),
    })?;

    crate::ftp::process_downloads(
        &processed,
        &config,
        &options.output,
        protocol,
        options.multithreads,
    ).await?;

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

    // Spawn upload task
    tokio::spawn(async move {
        let result = run_upload_async(options, app_handle.clone()).await;

        let mut is_uploading = is_uploading_flag.lock().unwrap();
        *is_uploading = false;

        if let Err(e) = result {
            eprintln!("Upload failed: {}", e);
            let _ = app_handle.emit("upload-event", UploadEvent::Error { message: e.to_string() });
        }
    });

    Ok(())
}

async fn run_upload_async(
    options: UploadOptions,
    app_handle: ::tauri::AppHandle,
) -> Result<()> {
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

    // For now, simulate progress
    for (i, file) in options.files.iter().enumerate() {
        let filename = file
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| format!("file_{}", i));

        app_handle.emit(
            "upload-event",
            UploadEvent::Progress {
                filename: filename.clone(),
                percent: 0.0,
                status: "Uploading".to_string(),
            },
        )?;

        tokio::time::sleep(std::time::Duration::from_millis(500)).await;

        app_handle.emit(
            "upload-event",
            UploadEvent::Progress {
                filename,
                percent: 100.0,
                status: "Completed".to_string(),
            },
        )?;
    }

    app_handle.emit("upload-event", UploadEvent::Completed)?;
    Ok(())
}

#[::tauri::command]
pub async fn cancel_download_command(state: State<'_, AppState>) -> Result<(), String> {
    let mut is_downloading = state.is_downloading.lock().unwrap();
    *is_downloading = false;
    Ok(())
}

#[::tauri::command]
pub async fn cancel_upload_command(state: State<'_, AppState>) -> Result<(), String> {
    let mut is_uploading = state.is_uploading.lock().unwrap();
    *is_uploading = false;
    Ok(())
}
