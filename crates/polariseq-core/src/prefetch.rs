use crate::{Config, ProcessedRecord};
use anyhow::{Context, Result};
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use tokio::process::Command;
use tokio::sync::Semaphore;
use tracing::{error, info, warn};

pub async fn download_all(
    records: &[ProcessedRecord],
    config: &Config,
    output_dir: &Path,
    file_threads: usize,
    process_threads: usize,
    max_size: &str, // New param: Receive max-size string
    cleanup_sra: bool,
) -> Result<()> {
    info!("Starting Prefetch pipeline...");
    info!(
        "Config: Parallel Files = {}, Threads/Process = {}, Max Size = {}",
        file_threads, process_threads, max_size
    );

    let semaphore = Arc::new(Semaphore::new(file_threads));
    let mut handles = Vec::new();

    let prefetch_bin = config.software.prefetch.display().to_string();
    let fasterq_dump_bin = config.software.fasterq_dump.display().to_string();

    for record in records {
        let run_id = record.run_accession.clone();
        let output_dir = output_dir.to_path_buf();
        let sem = semaphore.clone();
        let prefetch = prefetch_bin.clone();
        let fasterq_dump = fasterq_dump_bin.clone();
        let threads = process_threads;
        let max_size_arg = max_size.to_string(); // Clone for thread

        let handle = tokio::spawn(async move {
            let _permit = sem.acquire().await.expect("semaphore closed");

            // --- Path Calculation ---
            // Full path is: ./aws_data/SRRxxx/SRRxxx.sra
            let sra_dir = output_dir.join(&run_id);
            let sra_file = sra_dir.join(format!("{}.sra", run_id));

            let relative_sra_path = format!("{}/{}.sra", run_id, run_id);

            // --- Execution Flow ---

            // 1. Prefetch (Direct Command)
            if sra_file.exists() && sra_file.metadata()?.len() > 0 {
                info!("[{}] SRA file exists, skipping download.", run_id);
            } else {
                info!("[{}] Step 1: Prefetching...", run_id);
                // Direct execution
                let output = Command::new(&prefetch)
                    .arg(&run_id)
                    .arg("-O")
                    .arg(".")
                    .arg("--max-size")
                    .arg(&max_size_arg)
                    .arg("--verify")
                    .arg("yes")
                    .arg("--force")
                    .arg("no")
                    .current_dir(&output_dir)
                    .stdout(Stdio::null())
                    .stderr(Stdio::piped())
                    .output()
                    .await?;

                if !output.status.success() {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    error!("Prefetch failed for {}\nError: {}", run_id, stderr);
                    return Err(anyhow::anyhow!("Prefetch failed"));
                }
            }

            // 2. Convert (Direct Command)
            let fq_1 = output_dir.join(format!("{}_1.fastq", run_id));
            let fq_single = output_dir.join(format!("{}.fastq", run_id));

            if (fq_1.exists() && fq_1.metadata()?.len() > 0)
                || (fq_single.exists() && fq_single.metadata()?.len() > 0)
            {
                info!("[{}] FASTQ files exist, skipping conversion.", run_id);
            } else {
                info!("[{}] Step 2: Converting (fasterq-dump)...", run_id);
                let fasterq_tmp_dir = output_dir.join(".fasterq_tmp").join(&run_id);
                tokio::fs::create_dir_all(&fasterq_tmp_dir)
                    .await
                    .with_context(|| {
                        format!(
                            "Failed to create fasterq-dump temporary directory: {}",
                            fasterq_tmp_dir.display()
                        )
                    })?;
                let fasterq_tmp_dir = tokio::fs::canonicalize(&fasterq_tmp_dir)
                    .await
                    .with_context(|| {
                        format!(
                            "Failed to resolve fasterq-dump temporary directory: {}",
                            fasterq_tmp_dir.display()
                        )
                    })?;
                let fasterq_output_dir = tokio::fs::canonicalize(&output_dir)
                    .await
                    .with_context(|| {
                        format!(
                            "Failed to resolve fasterq-dump output directory: {}",
                            output_dir.display()
                        )
                    })?;

                // Direct execution
                let output = Command::new(&fasterq_dump)
                    .arg("--split-3")
                    .arg("-e")
                    .arg(threads.to_string())
                    .arg("-O")
                    .arg(&fasterq_output_dir)
                    .arg("-t")
                    .arg(&fasterq_tmp_dir)
                    .arg("-f")
                    .arg(&relative_sra_path)
                    .current_dir(&output_dir)
                    .stdout(Stdio::null())
                    .stderr(Stdio::piped())
                    .output()
                    .await;

                match output {
                    Ok(out) if !out.status.success() => {
                        warn!(
                            "[{}] fasterq-dump error: {}. Checking output...",
                            run_id,
                            String::from_utf8_lossy(&out.stderr)
                        );
                    }
                    Ok(_) => {}
                    Err(e) => warn!("[{}] fasterq-dump exec error: {}", run_id, e),
                }
            }

            // 3. Compress
            if (fq_1.exists() && fq_1.metadata()?.len() > 0)
                || (fq_single.exists() && fq_single.metadata()?.len() > 0)
            {
                info!("[{}] Step 3: Compressing...", run_id);
                let output_dir_compress = output_dir.clone();
                let run_id_compress = run_id.clone();
                let threads_compress = threads;
                tokio::task::spawn_blocking(move || {
                    crate::compress_fastq_files(
                        &output_dir_compress,
                        &run_id_compress,
                        threads_compress,
                        None,
                    )
                })
                .await
                .context("Compression task panicked")?
                .context("Compression failed")?;

                if cleanup_sra && sra_file.exists() {
                    info!(
                        "[{}] Cleaning up SRA file: {}",
                        run_id,
                        sra_file.display()
                    );
                    if let Err(e) = tokio::fs::remove_file(&sra_file).await {
                        warn!("[{}] Failed to remove SRA file: {}", run_id, e);
                    }
                }

                info!("[{}] All steps completed!", run_id);
                Ok(())
            } else {
                error!("[{}] Conversion failed, no output found.", run_id);
                Err(anyhow::anyhow!("Process failed for {}", run_id))
            }
        });
        handles.push(handle);
    }

    for handle in handles {
        if let Err(e) = handle.await {
            warn!("Task error: {}", e);
        }
    }
    info!("All Prefetch tasks completed");
    Ok(())
}
