use crate::{Config, ProcessedRecord};
use crate::progress::{spinner_style, transfer_bar_style};
use anyhow::{anyhow, Result};
use indicatif::{MultiProgress, ProgressBar};
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use tokio::fs::{self, File}; // 🟢 Import fs for checking file size
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::sync::Semaphore;
use tokio::time::{sleep, Duration}; // 🟢 Import time
use tracing::{info, warn, error};

pub enum Protocol {
    Ftp,
    Ascp,
}

pub async fn process_downloads(
    records: &[ProcessedRecord],
    config: &Config,
    output_dir: &Path,
    protocol: Protocol,
    threads: usize,
) -> Result<()> {
    info!("🚀 Starting {:?} download pipeline with {} threads...", 
        match protocol { Protocol::Ftp => "FTP", Protocol::Ascp => "Aspera" }, 
        threads
    );

    let semaphore = Arc::new(Semaphore::new(threads));
    let mp = Arc::new(MultiProgress::new());
    let mut handles = Vec::new();

    let ascp_bin = config.software.ascp.display().to_string();
    let ssh_key = config.setting.openssh.display().to_string();

    struct Task {
        url: String,
        md5: String,
        filename: String,
        total_size: u64, // 🟢 Added: Total size
    }
    
    let mut tasks = Vec::new();
    for record in records {
        tasks.push(Task {
            url: record.fastq_ftp_1_url.clone(),
                            md5: record.fastq_md5_1.clone(),
                        filename: record.fastq_ftp_1_name.clone(),
                        total_size: record.fastq_bytes_1, // 🟢 Pass size
                    });
                    if let (Some(url), Some(md5), Some(name), Some(size)) = (&record.fastq_ftp_2_url, &record.fastq_md5_2, &record.fastq_ftp_2_name, record.fastq_bytes_2) {
                        tasks.push(Task {
                            url: url.clone(),
                            md5: md5.clone(),
                            filename: name.clone(),
                            total_size: size, // 🟢 Pass size
                        });
                    }
                }
    for task in tasks {
        let sem = semaphore.clone();
        let mp = mp.clone();
        let output_dir = output_dir.to_path_buf();
        
        let t_url = task.url.clone();
        let t_md5 = task.md5.clone();
        let t_file = task.filename.clone();
        let t_size = task.total_size; // 🟢
        
        let (cmd_bin, cmd_args, cmd_string_for_script) = match protocol {
            Protocol::Ftp => {
                ("wget".to_string(), vec!["-c".to_string(), t_url.clone()], format!("wget -c {}", t_url))
            },
            Protocol::Ascp => {
                let ascp_url = t_url.replace("ftp.sra.ebi.ac.uk", "era-fasp@fasp.sra.ebi.ac.uk:");
                (
                    ascp_bin.clone(), 
                    vec![
                        "-QT".to_string(), "-k2".to_string(), 
                        "-l".to_string(), "800m".to_string(), 
                        "-P33001".to_string(), 
                        "-i".to_string(), ssh_key.clone(), 
                        ascp_url.clone(), 
                        ".".to_string()
                    ],
                    format!("{} -QT -k2 -l 800m -P33001 -i {} {} .", ascp_bin, ssh_key, ascp_url)
                )
            }
        };

        let handle = tokio::spawn(async move {
            let _permit = sem.acquire().await.expect("semaphore closed");

            // 🟢 ProgressBar init: Show bar if size available, else show Spinner
            let pb = if t_size > 0 {
                let p = mp.add(ProgressBar::new(t_size));
                p.set_style(transfer_bar_style());
                p
            } else {
                let p = mp.add(ProgressBar::new_spinner());
                p.set_style(spinner_style());
                p
            };
            
            pb.set_prefix(t_file.clone());
            pb.enable_steady_tick(Duration::from_millis(120));

            let output_file_path = output_dir.join(&t_file);

            // Check existing file
            if output_file_path.exists() {
                // If file exists and size matches (simple check), or MD5 matches
                if let Ok(meta) = fs::metadata(&output_file_path).await {
                     if meta.len() == t_size && t_size > 0 {
                         // Size matches, verify MD5 first
                         pb.set_message("🔍 Checking existing file...");
                         if let Ok(true) = verify_md5(&output_file_path, &t_md5).await {
                             pb.finish_and_clear();
                             return Ok(());
                         }
                     } else if meta.len() > 0 {
                         // Set current progress before resuming
                         pb.set_position(meta.len());
                     }
                }
            }

            pb.set_message("Downloading");

            // 🟢 Start background monitor: Check file size every 500ms and update progress
            let monitor_path = output_file_path.clone();
            let monitor_pb = pb.clone();
            let monitor_handle = tokio::spawn(async move {
                loop {
                    sleep(Duration::from_millis(500)).await;
                    if let Ok(meta) = fs::metadata(&monitor_path).await {
                        monitor_pb.set_position(meta.len());
                    }
                }
            });

            // Execute download command
            let output = Command::new(&cmd_bin)
                .args(&cmd_args)
                .current_dir(&output_dir)
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .output()
                .await;

            // 🛑 Download finished, stop monitor
            monitor_handle.abort();

            match output {
                Ok(out) => {
                    if !out.status.success() {
                        let stderr = String::from_utf8_lossy(&out.stderr);
                        pb.finish_with_message(format!("❌ Failed (Exit {})", out.status));
                        error!("Command failed: {}\nError: {}", cmd_string_for_script, stderr);
                        return Err(anyhow!("Download failed"));
                    }
                }
                Err(e) => {
                    pb.finish_with_message(format!("❌ Exec Error: {}", e));
                    return Err(anyhow::anyhow!(e));
                }
            }

            // Complete progress bar (in case monitor missed the last update)
            if t_size > 0 {
                pb.set_position(t_size);
            }

            pb.set_message("Verifying MD5");
            match verify_md5(&output_file_path, &t_md5).await {
                Ok(true) => {
                    pb.finish_and_clear();
                    Ok(())
                }
                Ok(false) => {
                    pb.finish_with_message("❌ MD5 Mismatch");
                    warn!("MD5 Mismatch for {}: expected {}, but check failed.", t_file, t_md5);
                    Err(anyhow!("MD5 mismatch"))
                }
                Err(e) => {
                    pb.finish_with_message(format!("❌ Check Error: {}", e));
                    Err(e)
                }
            }
        });
        handles.push(handle);
    }

    for handle in handles {
        if let Err(_e) = handle.await { }
    }
    
    mp.clear().ok();
    Ok(())
}

async fn verify_md5(path: &Path, expected: &str) -> Result<bool> {
    if !path.exists() { return Ok(false); }
    let mut file = File::open(path).await?;
    let mut context = md5::Context::new();
    let mut buffer = vec![0; 1024 * 1024 * 4];
    loop {
        let n = file.read(&mut buffer).await?;
        if n == 0 { break; }
        context.consume(&buffer[..n]);
    }
    let digest = context.compute();
    Ok(format!("{:x}", digest) == expected)
}
