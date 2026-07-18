//! Dependency management for external binaries (currently sra-tools).
//!
//! Provides automatic download, verification, extraction, and configuration
//! of NCBI sra-tools (`prefetch` + `fasterq-dump`) from official pre-built
//! releases. This eliminates the need for users to manually install
//! sra-tools or edit YAML configuration files.

use crate::{Config, SoftwarePaths};
use anyhow::{anyhow, Context, Result};
use flate2::read::GzDecoder;
use futures::StreamExt;
use serde::Serialize;
use std::fmt;
use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tar::Archive;
use tracing::{info, warn};

/// Default sra-tools version to install.
pub const DEFAULT_SRA_TOOLS_VERSION: &str = "3.1.1";

/// Base URL for NCBI sra-tools pre-built releases.
const SRA_TOOLS_BASE_URL: &str = "https://ftp-trace.ncbi.nlm.nih.gov/sra/sdk";

/// Progress event emitted during dependency installation.
#[derive(Debug, Clone)]
pub enum DepProgressEvent {
    DownloadStarted { url: String, size: Option<u64> },
    DownloadProgress { downloaded: u64, total: Option<u64> },
    DownloadCompleted,
    Verifying,
    Extracting,
    Completed,
    Error { message: String },
}

pub type DepProgressCallback = Arc<dyn Fn(DepProgressEvent) + Send + Sync>;

/// Status of the sra-tools dependency.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "status")]
pub enum DepStatus {
    Ready {
        prefetch: PathBuf,
        fasterq_dump: PathBuf,
        source: DepSource,
    },
    Missing {
        reason: String,
    },
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DepSource {
    Config,
    Managed,
    Path,
}

impl fmt::Display for DepSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DepSource::Config => write!(f, "polariseq.yaml"),
            DepSource::Managed => write!(f, "managed dependency directory"),
            DepSource::Path => write!(f, "system PATH"),
        }
    }
}

/// Describes an sra-tools release.
#[derive(Debug, Clone)]
pub struct SraToolsRelease {
    pub version: String,
    pub platform: String,
    pub url: String,
    pub checksum_url: String,
}

impl SraToolsRelease {
    /// Build a release descriptor for the current platform.
    pub fn for_current_platform(version: &str) -> Result<Self> {
        let platform = detect_platform()?;
        let file_name = format!("sratoolkit.{}-{}.tar.gz", version, platform);
        let url = format!("{}/{}/{}", SRA_TOOLS_BASE_URL, version, file_name);
        let checksum_url = format!("{}/{}/md5sum.txt", SRA_TOOLS_BASE_URL, version);
        Ok(Self {
            version: version.to_string(),
            platform,
            url,
            checksum_url,
        })
    }

    /// Build a release descriptor from a custom URL.
    pub fn from_url(version: &str, url: &str) -> Result<Self> {
        let platform = detect_platform()?;
        let checksum_url = format!("{}/{}/md5sum.txt", SRA_TOOLS_BASE_URL, version);
        Ok(Self {
            version: version.to_string(),
            platform,
            url: url.to_string(),
            checksum_url,
        })
    }
}

/// Detect the NCBI platform identifier for the current OS + architecture.
pub fn detect_platform() -> Result<String> {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;

    match (os, arch) {
        ("linux", "x86_64") => Ok("ubuntu64".to_string()),
        ("macos", "x86_64") => Ok("mac64".to_string()),
        ("macos", "aarch64") => Ok("mac-arm64".to_string()),
        ("windows", "x86_64") => Ok("win64".to_string()),
        _ => Err(anyhow!("Unsupported platform: {} {}", os, arch)),
    }
}

/// Returns the root directory for managed dependencies.
pub fn deps_root() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
        .join("polariseq")
        .join("deps")
}

/// Returns the installation directory for a specific sra-tools version.
pub fn sra_tools_install_dir(version: &str) -> PathBuf {
    deps_root().join("sra-tools").join(version)
}

/// Check whether sra-tools are available.
///
/// Lookup order:
/// 1. `polariseq.yaml` explicit configuration
/// 2. Managed dependency directory
/// 3. System PATH
pub fn check_sra_tools(config: Option<&Config>) -> DepStatus {
    // 1. Config
    if let Some(config) = config {
        let prefetch = &config.software.prefetch;
        let fasterq = &config.software.fasterq_dump;
        if prefetch.exists() && fasterq.exists() {
            return DepStatus::Ready {
                prefetch: prefetch.clone(),
                fasterq_dump: fasterq.clone(),
                source: DepSource::Config,
            };
        }
    }

    // 2. Managed
    if let Some(paths) = find_managed_sra_tools() {
        return DepStatus::Ready {
            prefetch: paths.prefetch.clone(),
            fasterq_dump: paths.fasterq_dump.clone(),
            source: DepSource::Managed,
        };
    }

    // 3. PATH
    if let Some(paths) = find_sra_tools_in_path() {
        return DepStatus::Ready {
            prefetch: paths.prefetch.clone(),
            fasterq_dump: paths.fasterq_dump.clone(),
            source: DepSource::Path,
        };
    }

    DepStatus::Missing {
        reason:
            "sra-tools (prefetch / fasterq-dump) not found in config, managed dependencies, or PATH"
                .to_string(),
    }
}

/// Find sra-tools in the managed dependency directory.
pub fn find_managed_sra_tools() -> Option<SoftwarePaths> {
    let install_dir = sra_tools_install_dir(DEFAULT_SRA_TOOLS_VERSION);
    if !install_dir.exists() {
        return None;
    }

    find_sra_tools_in_dir(&install_dir)
}

/// Find sra-tools binaries inside a directory tree.
fn find_sra_tools_in_dir(dir: &Path) -> Option<SoftwarePaths> {
    let prefetch = find_executable(dir, "prefetch")?;
    let fasterq_dump = find_executable(dir, "fasterq-dump")?;
    Some(SoftwarePaths {
        prefetch,
        fasterq_dump,
        blastdbcmd: None,
    })
}

/// Find a binary inside a directory tree, looking in `bin/` subdirectories.
fn find_executable(dir: &Path, name: &str) -> Option<PathBuf> {
    let exe_name = if std::env::consts::OS == "windows" {
        format!("{}.exe", name)
    } else {
        name.to_string()
    };

    // Direct check
    let direct = dir.join(&exe_name);
    if direct.exists() {
        return Some(direct);
    }

    // Search one level of bin/ subdirectories
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let candidate = path.join("bin").join(&exe_name);
                if candidate.exists() {
                    return Some(candidate);
                }
            }
        }
    }

    None
}

/// Find sra-tools in system PATH.
pub fn find_sra_tools_in_path() -> Option<SoftwarePaths> {
    let prefetch = which::which("prefetch").ok()?;
    let fasterq_dump = which::which("fasterq-dump").ok()?;
    Some(SoftwarePaths {
        prefetch,
        fasterq_dump,
        blastdbcmd: None,
    })
}

/// Install sra-tools into the managed dependency directory.
pub async fn install_sra_tools(
    version: Option<&str>,
    url: Option<&str>,
    progress_cb: Option<DepProgressCallback>,
) -> Result<SoftwarePaths> {
    let version = version.unwrap_or(DEFAULT_SRA_TOOLS_VERSION);
    let release = match url {
        Some(u) => SraToolsRelease::from_url(version, u)?,
        None => SraToolsRelease::for_current_platform(version)?,
    };

    info!(
        "Installing sra-tools {} for platform {} from {}",
        release.version, release.platform, release.url
    );

    if let Some(cb) = &progress_cb {
        cb(DepProgressEvent::DownloadStarted {
            url: release.url.clone(),
            size: None,
        });
    }

    let install_dir = sra_tools_install_dir(version);
    std::fs::create_dir_all(&install_dir).with_context(|| {
        format!(
            "Failed to create install directory {}",
            install_dir.display()
        )
    })?;

    // Download to a temporary file
    let temp_dir = tempfile::tempdir()?;
    let archive_path = temp_dir.path().join(format!(
        "sratoolkit.{}-{}.tar.gz",
        release.version, release.platform
    ));

    download_file_with_progress(&release.url, &archive_path, progress_cb.clone()).await?;

    if let Some(cb) = &progress_cb {
        cb(DepProgressEvent::DownloadCompleted);
        cb(DepProgressEvent::Verifying);
    }

    // Verify checksum
    verify_download_checksum(&archive_path, &release).await?;

    if let Some(cb) = &progress_cb {
        cb(DepProgressEvent::Extracting);
    }

    // Extract
    extract_tar_gz(&archive_path, &install_dir)?;

    // Locate binaries
    let paths = find_sra_tools_in_dir(&install_dir)
        .ok_or_else(|| anyhow!("Could not find prefetch / fasterq-dump after extraction"))?;

    if !paths.prefetch.exists() || !paths.fasterq_dump.exists() {
        return Err(anyhow!(
            "Extracted binaries are missing: prefetch={}, fasterq-dump={}",
            paths.prefetch.display(),
            paths.fasterq_dump.display()
        ));
    }

    info!(
        "sra-tools installed:\n   prefetch: {}\n   fasterq-dump: {}",
        paths.prefetch.display(),
        paths.fasterq_dump.display()
    );

    if let Some(cb) = &progress_cb {
        cb(DepProgressEvent::Completed);
    }

    Ok(paths)
}

/// Download a file with optional progress callbacks.
async fn download_file_with_progress(
    url: &str,
    dest: &Path,
    progress_cb: Option<DepProgressCallback>,
) -> Result<()> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(600))
        .build()?;

    let response = client.get(url).send().await?;
    let total = response.content_length();

    if let Some(cb) = &progress_cb {
        cb(DepProgressEvent::DownloadStarted {
            url: url.to_string(),
            size: total,
        });
    }

    let file =
        File::create(dest).with_context(|| format!("Failed to create file {}", dest.display()))?;
    let mut writer = BufWriter::new(file);
    let mut stream = response.bytes_stream();
    let mut downloaded: u64 = 0;
    let mut last_reported: u64 = 0;
    const REPORT_INTERVAL: u64 = 512 * 1024; // report progress every 512 KiB

    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        writer.write_all(&chunk)?;
        downloaded += chunk.len() as u64;

        if let Some(cb) = &progress_cb {
            if downloaded.saturating_sub(last_reported) >= REPORT_INTERVAL {
                cb(DepProgressEvent::DownloadProgress { downloaded, total });
                last_reported = downloaded;
            }
        }
    }

    // Final progress report
    if let Some(cb) = &progress_cb {
        cb(DepProgressEvent::DownloadProgress { downloaded, total });
    }

    writer.flush()?;
    Ok(())
}

/// Verify the downloaded archive against the official MD5 checksum file.
async fn verify_download_checksum(archive_path: &Path, release: &SraToolsRelease) -> Result<()> {
    let file_name = archive_path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| anyhow!("Invalid archive path"))?;

    let expected_md5 = fetch_expected_md5(&release.checksum_url, file_name).await?;

    if expected_md5.is_empty() {
        warn!("Skipping checksum verification (no checksum found)");
        return Ok(());
    }

    let actual_md5 = compute_md5(archive_path)?;

    if expected_md5 != actual_md5 {
        return Err(anyhow!(
            "Checksum mismatch for {}: expected {}, got {}",
            file_name,
            expected_md5,
            actual_md5
        ));
    }

    info!("Checksum verified: {}", actual_md5);
    Ok(())
}

/// Fetch the expected MD5 for a given file from NCBI's md5sum.txt.
async fn fetch_expected_md5(checksum_url: &str, file_name: &str) -> Result<String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()?;

    let text = client.get(checksum_url).send().await?.text().await?;

    for line in text.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 2 {
            let md5 = parts[0];
            let name = parts[1].trim_start_matches("./");
            if name == file_name || name.ends_with(&format!("/{}", file_name)) {
                return Ok(md5.to_string());
            }
        }
    }

    warn!(
        "Could not find checksum for {} in {}, skipping verification",
        file_name, checksum_url
    );
    Ok(String::new())
}

/// Compute MD5 hex digest of a file.
fn compute_md5(path: &Path) -> Result<String> {
    let file = File::open(path)
        .with_context(|| format!("Failed to open {} for checksum", path.display()))?;
    let mut reader = BufReader::new(file);
    let mut context = md5::Context::new();
    let mut buffer = [0u8; 8192];

    loop {
        let n = reader.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        context.consume(&buffer[..n]);
    }

    Ok(format!("{:x}", context.compute()))
}

/// Extract a `.tar.gz` archive into the destination directory.
fn extract_tar_gz(archive_path: &Path, dest_dir: &Path) -> Result<()> {
    let file = File::open(archive_path)
        .with_context(|| format!("Failed to open archive {}", archive_path.display()))?;
    let gz = GzDecoder::new(file);
    let mut archive = Archive::new(gz);

    archive
        .unpack(dest_dir)
        .with_context(|| format!("Failed to extract archive to {}", dest_dir.display()))?;

    Ok(())
}

/// List installed sra-tools versions.
pub fn list_installed() -> Vec<String> {
    let base = deps_root().join("sra-tools");
    let mut versions = Vec::new();

    if let Ok(entries) = std::fs::read_dir(&base) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    versions.push(name.to_string());
                }
            }
        }
    }

    versions.sort();
    versions
}

/// Remove a managed sra-tools installation.
pub fn remove_sra_tools(version: &str) -> Result<()> {
    let dir = sra_tools_install_dir(version);
    if dir.exists() {
        std::fs::remove_dir_all(&dir)
            .with_context(|| format!("Failed to remove {}", dir.display()))?;
        info!("Removed sra-tools {}", version);
    } else {
        warn!("sra-tools {} is not installed", version);
    }
    Ok(())
}

/// Write or update `polariseq.yaml` with the given sra-tools paths.
pub fn write_software_paths_to_yaml(yaml_path: &Path, paths: &SoftwarePaths) -> Result<()> {
    let mut config: Config = if yaml_path.exists() {
        let content = std::fs::read_to_string(yaml_path)?;
        serde_yaml::from_str(&content).unwrap_or_else(|_| Config {
            software: SoftwarePaths {
                prefetch: paths.prefetch.clone(),
                fasterq_dump: paths.fasterq_dump.clone(),
                blastdbcmd: paths.blastdbcmd.clone(),
            },
            public_data: Default::default(),
        })
    } else {
        Config {
            software: SoftwarePaths {
                prefetch: paths.prefetch.clone(),
                fasterq_dump: paths.fasterq_dump.clone(),
                blastdbcmd: paths.blastdbcmd.clone(),
            },
            public_data: Default::default(),
        }
    };

    config.software.prefetch = paths.prefetch.clone();
    config.software.fasterq_dump = paths.fasterq_dump.clone();

    let yaml = serde_yaml::to_string(&config)?;
    std::fs::write(yaml_path, yaml)
        .with_context(|| format!("Failed to write {}", yaml_path.display()))?;

    info!("Updated configuration: {}", yaml_path.display());
    Ok(())
}
