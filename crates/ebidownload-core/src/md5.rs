//! Multi-threaded MD5 generation and verification for local files.
//!
//! Used by the `md5` CLI subcommand to produce `md5sum`-compatible manifests
//! and to verify files against an existing manifest.

use anyhow::{anyhow, Context, Result};
use std::fs::File;
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Semaphore;
use tracing::{info, warn};

/// Compute the MD5 hex digest of a single file.
pub fn compute_md5(path: &Path) -> Result<String> {
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
    }
    Ok(format!("{:x}", ctx.compute()))
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

/// Recursively collect regular files under `dir`, skipping hidden entries.
///
/// Hidden entries are those whose file name starts with `.`.
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
pub async fn generate_md5_manifest(
    target: &Path,
    output: &Path,
    threads: usize,
) -> Result<()> {
    let files = if target.is_file() {
        vec![target.to_path_buf()]
    } else if target.is_dir() {
        collect_files(target)?
    } else {
        return Err(anyhow!("Target {} is neither a file nor a directory", target.display()));
    };

    if files.is_empty() {
        warn!("No files found to hash under {}", target.display());
        return Ok(());
    }

    info!("🔐 Computing MD5 for {} file(s) using {} thread(s)", files.len(), threads.max(1));

    let semaphore = Arc::new(Semaphore::new(threads.max(1)));
    let mut handles = Vec::with_capacity(files.len());

    for file in files {
        let semaphore = semaphore.clone();
        handles.push(tokio::spawn(async move {
            let _permit = semaphore
                .acquire()
                .await
                .expect("md5 semaphore closed");
            let path = file.clone();
            let md5 = tokio::task::spawn_blocking(move || compute_md5(&path))
                .await
                .context("MD5 compute task panicked")?
                .with_context(|| format!("Failed to compute MD5 for {}", file.display()))?;
            Ok::<_, anyhow::Error>((file, md5))
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

    info!("📝 MD5 manifest written: {}", output.display());
    Ok(())
}

/// Verify files in `root_dir` against an md5sum-compatible manifest.
///
/// Returns `(passed, failed)` counts. Files missing from `root_dir` are counted
/// as failed and logged.
pub async fn verify_md5_manifest(
    md5_path: &Path,
    root_dir: &Path,
    threads: usize,
) -> Result<(usize, usize)> {
    let entries = parse_md5_manifest(md5_path)?;
    if entries.is_empty() {
        warn!("MD5 manifest {} is empty", md5_path.display());
        return Ok((0, 0));
    }

    info!(
        "🔐 Verifying {} file(s) from {} using {} thread(s)",
        entries.len(),
        md5_path.display(),
        threads.max(1)
    );

    let semaphore = Arc::new(Semaphore::new(threads.max(1)));
    let mut handles = Vec::with_capacity(entries.len());

    for (expected_md5, filename) in entries {
        let file_path = root_dir.join(&filename);
        let semaphore = semaphore.clone();
        handles.push(tokio::spawn(async move {
            let _permit = semaphore
                .acquire()
                .await
                .expect("md5 semaphore closed");

            if !file_path.exists() {
                return Ok::<_, anyhow::Error>((filename, expected_md5, None));
            }

            let path = file_path.clone();
            let actual_md5 = tokio::task::spawn_blocking(move || compute_md5(&path))
                .await
                .context("MD5 verify task panicked")?
                .with_context(|| format!("Failed to compute MD5 for {}", file_path.display()))?;
            Ok::<_, anyhow::Error>((filename, expected_md5, Some(actual_md5)))
        }));
    }

    let mut passed = 0usize;
    let mut failed = 0usize;

    for handle in handles {
        let (filename, expected_md5, actual_md5) =
            handle.await.context("MD5 verification task panicked")??;

        match actual_md5 {
            None => {
                warn!("❌ {} missing", filename);
                failed += 1;
            }
            Some(actual) if actual == expected_md5 => {
                info!("✅ {} OK", filename);
                passed += 1;
            }
            Some(actual) => {
                warn!(
                    "❌ {} MD5 mismatch: expected {} got {}",
                    filename, expected_md5, actual
                );
                failed += 1;
            }
        }
    }

    Ok((passed, failed))
}
