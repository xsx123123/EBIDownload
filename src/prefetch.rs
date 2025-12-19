use crate::{Config, ProcessedRecord, create_script};
use anyhow::{Context, Result};
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use tokio::process::Command;
use tokio::sync::Semaphore;
use tracing::{info, warn, error};

// Helper: Execute Shell command (with error echo)
async fn run_command(cmd: &str, dir: &Path) -> Result<()> {
    info!("   Step: {}", cmd);
    // Note: This switches current directory to dir (i.e., output_dir)
    let output = Command::new("bash")
        .arg("-c")
        .arg(cmd)
        .current_dir(dir) 
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .await?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        error!("❌ Command failed: {}\nError Output:\n{}", cmd, stderr);
        Err(anyhow::anyhow!("Command failed"))
    }
}

pub async fn download_all(
    records: &[ProcessedRecord],
    config: &Config,
    output_dir: &Path,
    file_threads: usize,    
    process_threads: usize, 
    max_size: &str, // 🟢 New param: Receive max-size string
    only_scripts: bool,
) -> Result<()> {
    info!("📦 Starting Prefetch pipeline...");
    info!("⚙️  Config: Parallel Files = {}, Threads/Process = {}, Max Size = {}", file_threads, process_threads, max_size);

    let semaphore = Arc::new(Semaphore::new(file_threads));
    let mut handles = Vec::new();

    let prefetch_bin = config.software.prefetch.display().to_string();
    let fasterq_dump_bin = config.software.fasterq_dump.display().to_string();
    let pigz_bin = "pigz"; 

    for record in records {
        let run_id = record.run_accession.clone();
        let output_dir = output_dir.to_path_buf();
        let sem = semaphore.clone();
        let prefetch = prefetch_bin.clone();
        let fasterq_dump = fasterq_dump_bin.clone();
        let pigz = pigz_bin.to_string();
        let threads = process_threads;
        let max_size_arg = max_size.to_string(); // Clone for thread

        let handle = tokio::spawn(async move {
            let _permit = sem.acquire().await.expect("semaphore closed");

            // --- Path Calculation ---
            // Full path is: ./aws_data/SRRxxx/SRRxxx.sra
            let sra_dir = output_dir.join(&run_id);
            let sra_file = sra_dir.join(format!("{}.sra", run_id));
            
            // --- Command Construction ---
            
            // 1. Prefetch: 
            // 🟢 Replace hardcoded "100G" with max_size_arg
            let cmd_prefetch = format!(
                "{} {} -O . --max-size {} --verify yes --force no",
                prefetch, run_id, max_size_arg
            );

            // 2. Convert: 
            let relative_sra_path = format!("{}/{}.sra", run_id, run_id);
            let cmd_convert = format!(
                "{} --split-3 -e {} -O . {} -f",
                fasterq_dump, threads, relative_sra_path
            );

            // 3. Compress: 
            let cmd_compress = format!(
                "{} -p {} {}*.fastq",
                pigz, threads, run_id
            );

            // --- Script Generation Mode ---
            if only_scripts {
                let full_script = format!(
                    "cd {}\n{}\n{}\n{}", 
                    output_dir.display(),
                    cmd_prefetch, 
                    cmd_convert, 
                    cmd_compress
                );
                create_script(&output_dir, &run_id, &full_script)?;
                info!("📝 [{}] Script generated", run_id);
                return Ok(());
            }

            // --- Execution Flow ---
            
            // 1. Prefetch
            if sra_file.exists() && sra_file.metadata()?.len() > 0 {
                info!("⏩ [{}] SRA file exists, skipping download.", run_id);
            } else {
                info!("📥 [{}] Step 1: Prefetching...", run_id);
                run_command(&cmd_prefetch, &output_dir).await.context("prefetch failed")?;
            }

            // 2. Convert
            let fq_1 = output_dir.join(format!("{}_1.fastq", run_id));
            let fq_single = output_dir.join(format!("{}.fastq", run_id));
            
            if (fq_1.exists() && fq_1.metadata()?.len() > 0) || (fq_single.exists() && fq_single.metadata()?.len() > 0) {
                 info!("⏩ [{}] FASTQ files exist, skipping conversion.", run_id);
            } else {
                info!("🔄 [{}] Step 2: Converting (fasterq-dump)...", run_id);
                let res = run_command(&cmd_convert, &output_dir).await;
                if let Err(e) = res {
                    warn!("⚠️ [{}] fasterq-dump error: {}. Checking output...", run_id, e);
                }
            }

            // 3. Compress
            if (fq_1.exists() && fq_1.metadata()?.len() > 0) || (fq_single.exists() && fq_single.metadata()?.len() > 0) {
                info!("📦 [{}] Step 3: Compressing (pigz)...", run_id);
                run_command(&cmd_compress, &output_dir).await.context("pigz failed")?;
                
                info!("✅ [{}] All steps completed!", run_id);
                Ok(())
            } else {
                error!("❌ [{}] Conversion failed, no output found.", run_id);
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
    info!("🎉 All Prefetch tasks completed");
    Ok(())
}