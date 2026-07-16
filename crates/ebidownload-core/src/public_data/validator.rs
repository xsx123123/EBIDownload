//! 使用 `blastdbcmd` 校验 BLAST 数据库分卷。
//!
//! BLAST 数据库以多分卷形式分发，每个分卷共享同一个文件名前缀，并
//! 由多个文件组成（`.phr`、`.psq`、`.pin` ...）。当某个分卷的所有文件
//! 都下载到本地后，对该前缀执行 `blastdbcmd -info` 来验证该分卷是否
//! 可以正常打开。

use std::path::Path;

use anyhow::{anyhow, Context, Result};
use indicatif::{MultiProgress, ProgressBar};
use tokio::process::Command;
use tokio::time::{sleep, Duration};

/// 单个 BLAST 分卷可能包含的文件后缀。
const BLAST_VOLUME_EXTENSIONS: &[&str] = &[
    "phr", "psq", "pin", "pog", "pni", "pnd", "psi", "psd", "aln", "freq",
];

const GREEN: &str = "\x1b[32m";
const RED_BOLD: &str = "\x1b[1;31m";
const RESET: &str = "\x1b[0m";

/// 执行 `blastdbcmd -db <volume_prefix> -dbtype <dbtype> -info`。
///
/// - 命令成功退出 → `Ok(true)`
/// - 命令报告分卷损坏/无效 → `Ok(false)`
/// - 工具或 I/O 出错 → `Err(...)`
pub async fn validate_blast_volume(
    volume_prefix: &Path,
    dbtype: &str,
    tool_path: &Path,
) -> Result<bool> {
    let prefix_str = volume_prefix
        .to_str()
        .ok_or_else(|| anyhow!("无效的分卷路径前缀: {}", volume_prefix.display()))?;

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
                "运行 blastdbcmd 校验 {} 失败",
                volume_prefix.display()
            )
        })?;

    Ok(output.status.success())
}

/// 删除指定 BLAST 分卷前缀对应的所有已知文件。
pub async fn delete_volume_files(volume_prefix: &Path) -> Result<()> {
    for ext in BLAST_VOLUME_EXTENSIONS {
        let path = volume_prefix.with_extension(ext);
        if path.exists() {
            tokio::fs::remove_file(&path)
                .await
                .with_context(|| format!("删除 {} 失败", path.display()))?;
        }
    }
    Ok(())
}

/// 遍历 `db_dir` 下所有 `.phr` 文件，作为 BLAST 分卷前缀批量校验。
///
/// 返回 `(通过数, 损坏数)`。进度和结果通过 `progress` 渲染，避免破坏
/// 已有的 indicatif 进度条布局。
pub async fn validate_all_volumes(
    db_dir: &Path,
    dbtype: &str,
    tool_path: &Path,
    progress: &MultiProgress,
) -> Result<(usize, usize)> {
    let mut entries = tokio::fs::read_dir(db_dir)
        .await
        .with_context(|| format!("读取目录 {} 失败", db_dir.display()))?;

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
        spinner.set_message(format!("⏳ 校验 {}...", name));
        spinner.enable_steady_tick(Duration::from_millis(100));

        match validate_blast_volume(&prefix, dbtype, tool_path).await {
            Ok(true) => {
                spinner.finish_with_message(format!(
                    "{GREEN}  ✅  {:<8} 校验通过{RESET}",
                    name
                ));
                passed += 1;
            }
            Ok(false) => {
                spinner.abandon_with_message(format!(
                    "{RED_BOLD}  ❌  {:<8} 分卷已损坏  ❌{RESET}",
                    name
                ));
                failed += 1;
            }
            Err(e) => {
                spinner.abandon_with_message(format!(
                    "{RED_BOLD}  ❌  {:<8} 校验出错: {} ❌{RESET}",
                    name, e
                ));
                failed += 1;
            }
        }
    }

    Ok((passed, failed))
}

/// 对单个分卷执行校验，失败时自动重试并重新下载。
///
/// `download_fn` 在每次重试前被调用，用于重新获取该分卷文件。
/// `progress` 用于向终端输出状态，不破坏 indicatif 布局。
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
        spinner.set_message(format!("⏳ 校验 {}...", volume_name));
        spinner.enable_steady_tick(Duration::from_millis(100));

        match validate_blast_volume(volume_prefix, dbtype, tool_path).await? {
            true => {
                spinner.finish_with_message(format!(
                    "{GREEN}  ✅  {:<8} 校验通过{RESET}",
                    volume_name
                ));
                return Ok(());
            }
            false => {
                spinner.abandon_with_message(format!(
                    "{RED_BOLD}  ❌  {:<8} 分卷已损坏  ❌{RESET}",
                    volume_name
                ));
                attempts += 1;
                if attempts > max_retries {
                    return Err(anyhow!(
                        "分卷 {} 经过 {} 次重试后仍校验失败",
                        volume_name, max_retries
                    ));
                }
                progress.println(format!(
                    "🔄 {} 校验失败 ({}/{}), {} 秒后重新下载",
                    volume_name, attempts, max_retries, retry_delay_seconds
                ))?;
                sleep(Duration::from_secs(retry_delay_seconds)).await;
                delete_volume_files(volume_prefix).await?;
            }
        }
    }
}
