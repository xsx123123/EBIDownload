//! Multi-threaded MD5 generation and verification for local files.
//!
//! Used by the `md5` CLI subcommand to produce `md5sum`-compatible manifests
//! and to verify files against an existing manifest.

use crate::progress::verify_bar_style;
use anyhow::{anyhow, Context, Result};
use indicatif::{MultiProgress, ProgressBar};
use std::fs::File;
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Semaphore;
use tracing::{info, warn};

/// Compute the MD5 hex digest of a single file.
pub fn compute_md5(path: &Path) -> Result<String> {
    compute_md5_with_progress(path, None)
}

/// Compute the MD5 hex digest of a single file, reporting bytes read to an
/// optional progress bar.
pub fn compute_md5_with_progress(path: &Path, progress: Option<&ProgressBar>) -> Result<String> {
    let file = File::open(path)
        .with_context(|| format!("Failed to open {}", path.display()))?;
    let mut reader = BufReader::new(file);
    let mut ctx = md5::Context::new();
    let mut buf = vec![0u8; 1024 * 1024];
    loop {
        let n = reader
            .read(&mut buf)
            .with_context(|| format!("Failed to read {}", path.display()))?;
        if n == 0 {
            break;
        }
        ctx.consume(&buf[..n]);
        if let Some(pb) = progress {
            pb.inc(n as u64);
        }
    }
    Ok(format!("{:x}", ctx.compute()))
}

/// A per-file hashing bar on the shared MultiProgress; matches the style used
/// for post-download integrity checks in `aws_s3.rs`.
fn new_hash_bar(mp: &MultiProgress, file: &Path, verb: &str) -> ProgressBar {
    let size = std::fs::metadata(file).map(|m| m.len()).unwrap_or(0);
    let pb = mp.add(ProgressBar::new(size));
    pb.set_style(verify_bar_style());
    let name = file
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| file.display().to_string());
    pb.set_prefix(name);
    pb.set_message(verb.to_string());
    pb.enable_steady_tick(std::time::Duration::from_millis(100));
    pb
}

/// Parse an md5sum-compatible manifest.
///
/// Each line is expected to be `"<md5>  <filename>"`. Lines that are empty or
/// start with `#` are ignored.
pub fn parse_md5_manifest(path: &Path) -> Result<Vec<(String, String)>> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read MD5 manifest {}", path.display()))?;
    let mut entries = Vec::new();
    for (line_no, line) in content.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = line.splitn(2, "  ").collect();
        if parts.len() != 2 {
            return Err(anyhow!(
                "Invalid line {} in {}: expected '<md5>  <filename>'",
                line_no + 1,
                path.display()
            ));
        }
        let md5 = parts[0].to_lowercase();
        if md5.len() != 32 || !md5.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(anyhow!(
                "Invalid MD5 on line {} in {}: {}",
                line_no + 1,
                path.display(),
                md5
            ));
        }
        entries.push((md5, parts[1].to_string()));
    }
    Ok(entries)
}

/// Name prefix of the log files written by the `md5` CLI subcommand itself
/// (the CLI names them `EBIDownload_md5_<timestamp>.log`). These logs live
/// next to the hashed data and change on every run, so they are never hashed
/// or verified.
pub const MD5_LOG_PREFIX: &str = "EBIDownload_md5";

/// True when `name` (a file name, not a path) is an md5-subcommand log file.
fn is_md5_log(name: &str) -> bool {
    name.starts_with(MD5_LOG_PREFIX) && name.ends_with(".log")
}

/// Recursively collect regular files under `dir`, skipping hidden entries.
///
/// Hidden entries are those whose file name starts with `.`. Log files
/// written by the `md5` subcommand itself (`EBIDownload_md5_*.log`) are
/// skipped as well.
pub fn collect_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    collect_files_recursive(dir, &mut files)?;
    files.sort();
    Ok(files)
}

fn collect_files_recursive(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(dir)
        .with_context(|| format!("Failed to read directory {}", dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy())
            .unwrap_or_default();
        if name.starts_with('.') {
            continue;
        }
        if path.is_dir() {
            collect_files_recursive(&path, out)?;
        } else if path.is_file() {
            if is_md5_log(&name) {
                continue;
            }
            out.push(path);
        }
    }
    Ok(())
}

/// Generate an md5sum-compatible manifest for `target`.
///
/// - If `target` is a file, only that file is hashed.
/// - If `target` is a directory, all non-hidden regular files under it are
///   hashed recursively.
///
/// The manifest uses base names (`path.file_name()`) so that it can later be
/// verified from any directory containing those files.
///
/// When `progress` is given, each file gets its own hashing bar on the shared
/// `MultiProgress`.
pub async fn generate_md5_manifest(
    target: &Path,
    output: &Path,
    threads: usize,
    progress: Option<Arc<MultiProgress>>,
) -> Result<()> {
    let mut files = if target.is_file() {
        vec![target.to_path_buf()]
    } else if target.is_dir() {
        collect_files(target)?
    } else {
        return Err(anyhow!("Target {} is neither a file nor a directory", target.display()));
    };

    // Never hash the manifest we are about to (over)write: an existing output
    // file would otherwise be hashed first and then replaced, guaranteeing a
    // mismatch on the next verification.
    if let Ok(output_canon) = output.canonicalize() {
        files.retain(|f| f.canonicalize().ok().as_ref() != Some(&output_canon));
    }

    if files.is_empty() {
        warn!("No files found to hash under {}", target.display());
        return Ok(());
    }

    info!("Computing MD5 for {} file(s) using {} thread(s)", files.len(), threads.max(1));

    let semaphore = Arc::new(Semaphore::new(threads.max(1)));
    let mut handles = Vec::with_capacity(files.len());

    for file in files {
        let semaphore = semaphore.clone();
        let progress = progress.clone();
        handles.push(tokio::spawn(async move {
            let _permit = semaphore
                .acquire()
                .await
                .expect("md5 semaphore closed");
            let pb = progress
                .as_ref()
                .map(|mp| new_hash_bar(mp, &file, "Hashing"));
            let path = file.clone();
            let pb_ref = pb.clone();
            let result = tokio::task::spawn_blocking(move || {
                compute_md5_with_progress(&path, pb_ref.as_ref())
            })
            .await
            .context("MD5 compute task panicked")?
            .with_context(|| format!("Failed to compute MD5 for {}", file.display()));
            if let Some(pb) = &pb {
                pb.finish_and_clear();
            }
            Ok::<_, anyhow::Error>((file, result?))
        }));
    }

    let mut results = Vec::with_capacity(handles.len());
    for handle in handles {
        let (file, md5) = handle.await.context("MD5 generation task panicked")??;
        results.push((file, md5));
    }

    results.sort_by(|a, b| a.0.cmp(&b.0));

    let mut manifest = File::create(output)
        .with_context(|| format!("Failed to create {}", output.display()))?;
    for (file, md5) in &results {
        let filename = file
            .file_name()
            .ok_or_else(|| anyhow!("Path has no file name: {}", file.display()))?
            .to_string_lossy();
        writeln!(manifest, "{}  {}", md5, filename)
            .with_context(|| format!("Failed to write to {}", output.display()))?;
    }

    info!("MD5 manifest written: {}", output.display());
    Ok(())
}

/// Verify files in `root_dir` against an md5sum-compatible manifest.
///
/// Returns `(passed, failed)` counts. Files missing from `root_dir` are counted
/// as failed and logged.
///
/// When `progress` is given, each existing file gets its own verifying bar on
/// the shared `MultiProgress`.
pub async fn verify_md5_manifest(
    md5_path: &Path,
    root_dir: &Path,
    threads: usize,
    progress: Option<Arc<MultiProgress>>,
) -> Result<(usize, usize)> {
    let entries = parse_md5_manifest(md5_path)?;
    if entries.is_empty() {
        warn!("MD5 manifest {} is empty", md5_path.display());
        return Ok((0, 0));
    }

    // Skip tool artifacts: the subcommand's own logs change on every run, and
    // a manifest can never match itself while it is being rewritten.
    let md5_canon = md5_path.canonicalize().ok();
    let (skipped, entries): (Vec<_>, Vec<_>) = entries.into_iter().partition(|(_, filename)| {
        let entry_path = root_dir.join(filename);
        let is_self = md5_canon
            .as_ref()
            .is_some_and(|c| entry_path.canonicalize().ok().as_ref() == Some(c));
        let is_log = Path::new(filename)
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(is_md5_log);
        is_self || is_log
    });
    if !skipped.is_empty() {
        info!(
            "Skipping {} tool artifact entr{} ({} logs / manifest itself)",
            skipped.len(),
            if skipped.len() == 1 { "y" } else { "ies" },
            MD5_LOG_PREFIX
        );
    }
    if entries.is_empty() {
        warn!(
            "Nothing to verify: {} contains only tool artifacts",
            md5_path.display()
        );
        return Ok((0, 0));
    }

    info!(
        "Verifying {} file(s) from {} using {} thread(s)",
        entries.len(),
        md5_path.display(),
        threads.max(1)
    );

    let semaphore = Arc::new(Semaphore::new(threads.max(1)));
    let mut handles = Vec::with_capacity(entries.len());

    for (expected_md5, filename) in entries {
        let file_path = root_dir.join(&filename);
        let semaphore = semaphore.clone();
        let progress = progress.clone();
        handles.push(tokio::spawn(async move {
            let _permit = semaphore
                .acquire()
                .await
                .expect("md5 semaphore closed");

            if !file_path.exists() {
                return Ok::<_, anyhow::Error>((filename, expected_md5, None));
            }

            let pb = progress
                .as_ref()
                .map(|mp| new_hash_bar(mp, &file_path, "Verifying"));
            let path = file_path.clone();
            let pb_ref = pb.clone();
            let result = tokio::task::spawn_blocking(move || {
                compute_md5_with_progress(&path, pb_ref.as_ref())
            })
            .await
            .context("MD5 verify task panicked")?
            .with_context(|| format!("Failed to compute MD5 for {}", file_path.display()));
            if let Some(pb) = &pb {
                pb.finish_and_clear();
            }
            Ok::<_, anyhow::Error>((filename, expected_md5, Some(result?)))
        }));
    }

    let mut passed = 0usize;
    let mut failed = 0usize;

    for handle in handles {
        let (filename, expected_md5, actual_md5) =
            handle.await.context("MD5 verification task panicked")??;

        match actual_md5 {
            None => {
                warn!("{} missing", filename);
                failed += 1;
            }
            Some(actual) if actual == expected_md5 => {
                info!("{} OK", filename);
                passed += 1;
            }
            Some(actual) => {
                warn!(
                    "{} MD5 mismatch: expected {} got {}",
                    filename, expected_md5, actual
                );
                failed += 1;
            }
        }
    }

    Ok((passed, failed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn md5_log_names_are_detected() {
        assert!(is_md5_log("EBIDownload_md5_2026-07-17_13-32-27.log"));
        assert!(is_md5_log("EBIDownload_md5_x.log"));
        assert!(!is_md5_log("EBIDownload_2026-07-17_13-32-27.log"));
        assert!(!is_md5_log("EBIDownload_PRJNA123_2026-07-17_13-32-27.log"));
        assert!(!is_md5_log("md5.txt"));
        assert!(!is_md5_log("EBIDownload_md5_notes.txt"));
    }

    #[test]
    fn collect_files_skips_md5_logs_and_hidden() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("data.fa"), b"acgt").unwrap();
        std::fs::write(
            dir.path().join("EBIDownload_md5_2026-07-17_13-32-27.log"),
            b"log",
        )
        .unwrap();
        std::fs::write(dir.path().join(".hidden"), b"h").unwrap();

        let files = collect_files(dir.path()).unwrap();
        let names: Vec<_> = files
            .iter()
            .map(|f| f.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["data.fa".to_string()]);
    }

    #[tokio::test]
    async fn generate_excludes_output_manifest_and_logs() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("data.fa"), b"acgt").unwrap();
        std::fs::write(dir.path().join("EBIDownload_md5_run.log"), b"log").unwrap();
        // A stale manifest from a previous run must not hash itself.
        let output = dir.path().join("md5.txt");
        std::fs::write(&output, b"stale").unwrap();

        generate_md5_manifest(dir.path(), &output, 2, None)
            .await
            .unwrap();

        let manifest = std::fs::read_to_string(&output).unwrap();
        assert!(manifest.contains("data.fa"), "manifest: {manifest}");
        assert!(!manifest.contains("md5.txt"), "manifest: {manifest}");
        assert!(!manifest.contains("EBIDownload_md5"), "manifest: {manifest}");
    }

    #[tokio::test]
    async fn verify_skips_logs_and_manifest_itself() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("data.fa"), b"acgt").unwrap();

        let md5 = compute_md5(&dir.path().join("data.fa")).unwrap();
        let manifest = dir.path().join("md5.txt");
        std::fs::write(
            &manifest,
            format!(
                "{md5}  data.fa\n\
                 00000000000000000000000000000000  EBIDownload_md5_run.log\n\
                 00000000000000000000000000000000  md5.txt\n"
            ),
        )
        .unwrap();

        let (passed, failed) = verify_md5_manifest(&manifest, dir.path(), 2, None)
            .await
            .unwrap();
        assert_eq!((passed, failed), (1, 0));
    }
}
