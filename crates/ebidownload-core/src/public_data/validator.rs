//! BLAST database volume validation using `blastdbcmd`.
//!
//! A BLAST database is shipped as a set of volumes. Each volume shares a
//! common prefix and is made of several files (`.phr`, `.psq`, `.pin`, ...).
//! After all files of a volume are present locally, `blastdbcmd -info` is run
//! on the prefix to verify the volume can be opened.

use std::path::Path;

use anyhow::{anyhow, Context, Result};
use indicatif::{MultiProgress, ProgressBar};
use tokio::process::Command;
use tokio::time::{sleep, Duration};

/// File extensions that may belong to a single BLAST database volume.
const BLAST_VOLUME_EXTENSIONS: &[&str] = &[
    "phr", "psq", "pin", "pog", "pni", "pnd", "psi", "psd", "aln", "freq",
];

const GREEN: &str = "\x1b[32m";
const RED_BOLD: &str = "\x1b[1;31m";
const RESET: &str = "\x1b[0m";

/// Run `blastdbcmd -db <volume_prefix> -dbtype <dbtype> -info`.
///
/// Returns `Ok(true)` when the command exits successfully, `Ok(false)` when it
/// reports a corrupted/invalid volume, and `Err(...)` for I/O or tool errors.
pub async fn validate_blast_volume(
    volume_prefix: &Path,
    dbtype: &str,
    tool_path: &Path,
) -> Result<bool> {
    let prefix_str = volume_prefix
        .to_str()
        .ok_or_else(|| anyhow!("Invalid volume prefix path: {}", volume_prefix.display()))?;

    let output = Command::new(tool_path)
        .arg("-db")
        .arg(prefix_str)
        .arg("-dbtype")
        .arg(dbtype)
        .arg("-info")
        .output()
        .await
        .with_context(|| {
            format!(
                "Failed to run blastdbcmd for volume {}",
                volume_prefix.display()
            )
        })?;

    Ok(output.status.success())
}

/// Delete all known files for a BLAST volume identified by its prefix.
pub async fn delete_volume_files(volume_prefix: &Path) -> Result<()> {
    for ext in BLAST_VOLUME_EXTENSIONS {
        let path = volume_prefix.with_extension(ext);
        if path.exists() {
            tokio::fs::remove_file(&path)
                .await
                .with_context(|| format!("Failed to remove {}", path.display()))?;
        }
    }
    Ok(())
}

/// Validate every `.phr` file in `db_dir` as a BLAST volume prefix.
///
/// Returns `(passed, failed)` counts. Progress and results are rendered via
/// `progress` so they do not corrupt active indicatif bars.
pub async fn validate_all_volumes(
    db_dir: &Path,
    dbtype: &str,
    tool_path: &Path,
    progress: &MultiProgress,
) -> Result<(usize, usize)> {
    let mut entries = tokio::fs::read_dir(db_dir)
        .await
        .with_context(|| format!("Failed to read directory {}", db_dir.display()))?;

    let mut phr_files = Vec::new();
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "phr") {
            phr_files.push(path);
        }
    }
    phr_files.sort();

    let mut passed = 0usize;
    let mut failed = 0usize;

    for phr_path in phr_files {
        let prefix = phr_path.with_extension("");
        let name = prefix
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();

        let spinner = progress.add(ProgressBar::new_spinner());
        spinner.set_message(format!("validating {}...", name));
        spinner.enable_steady_tick(Duration::from_millis(100));

        match validate_blast_volume(&prefix, dbtype, tool_path).await {
            Ok(true) => {
                spinner.finish_with_message(format!(
                    "{GREEN}  ✅  {:<8} validated{RESET}",
                    name
                ));
                passed += 1;
            }
            Ok(false) => {
                spinner.abandon_with_message(format!(
                    "{RED_BOLD}  ❌  {:<8} corrupted ❌{RESET}",
                    name
                ));
                failed += 1;
            }
            Err(e) => {
                spinner.abandon_with_message(format!(
                    "{RED_BOLD}  ❌  {:<8} validation error: {} ❌{RESET}",
                    name, e
                ));
                failed += 1;
            }
        }
    }

    Ok((passed, failed))
}

/// Validate a single volume with retries and automatic re-download on failure.
///
/// `download_fn` is called for each retry attempt after corrupted files have
/// been removed. `progress` is used for user-facing status messages.
#[allow(clippy::too_many_arguments)]
pub async fn validate_volume_with_retry<F, Fut>(
    volume_prefix: &Path,
    volume_name: &str,
    dbtype: &str,
    tool_path: &Path,
    max_retries: u32,
    retry_delay_seconds: u64,
    progress: &MultiProgress,
    mut download_fn: F,
) -> Result<()>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<()>>,
{
    let mut attempts: u32 = 0;

    loop {
        download_fn().await?;

        let spinner = progress.add(ProgressBar::new_spinner());
        spinner.set_message(format!("validating {}...", volume_name));
        spinner.enable_steady_tick(Duration::from_millis(100));

        match validate_blast_volume(volume_prefix, dbtype, tool_path).await? {
            true => {
                spinner.finish_with_message(format!(
                    "{GREEN}  ✅  {:<8} validated{RESET}",
                    volume_name
                ));
                return Ok(());
            }
            false => {
                spinner.abandon_with_message(format!(
                    "{RED_BOLD}  ❌  {:<8} corrupted ❌{RESET}",
                    volume_name
                ));
                attempts += 1;
                if attempts > max_retries {
                    return Err(anyhow!(
                        "Volume {} failed validation after {} retries",
                        volume_name, max_retries
                    ));
                }
                progress.println(format!(
                    "🔄 {} validation failed ({}/{}), re-downloading in {}s",
                    volume_name, attempts, max_retries, retry_delay_seconds
                ))?;
                sleep(Duration::from_secs(retry_delay_seconds)).await;
                delete_volume_files(volume_prefix).await?;
            }
        }
    }
}
