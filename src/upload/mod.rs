//! S3 Upload module for SRA/GEO data submission.
//!
//! Supports uploading sequencing data to AWS S3 for NCBI SRA submission,
//! including automatic Bucket Policy configuration for NCBI IAM user access.

use crate::progress::transfer_bar_style;
use anyhow::{anyhow, Context, Result};
use aws_sdk_s3::primitives::ByteStream;
use indicatif::{MultiProgress, ProgressBar};
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::Semaphore;
use tracing::{info, warn, error};

// NCBI SRA Submission Portal IAM User ARN
const NCBI_SRA_IAM_ARN: &str = "arn:aws:iam::228184908524:user/SA-SubmissionPortal-S3";

/// Upload command arguments (defined in main.rs, used here via reference)

/// Execute the upload command
pub async fn run_upload(
    bucket: &str,
    prefix: &Option<String>,
    files: &[PathBuf],
    region: &str,
    concurrent: usize,
    apply_policy: bool,
    metadata_template: &Option<PathBuf>,
    dry_run: bool,
) -> Result<()> {
    info!("📤 Starting S3 upload workflow...");
    info!("   Bucket: {}", bucket);
    if let Some(p) = prefix {
        info!("   Prefix: {}", p);
    }
    info!("   Region: {}", region);
    info!("   Concurrency: {} files", concurrent);
    if dry_run {
        info!("   Mode: Dry Run (no actual uploads)");
    }

    // Warn if region is not us-east-1 (NCBI requirement)
    if region != "us-east-1" {
        warn!(
            "⚠️  S3 bucket region is '{}', but NCBI SRA requires 'us-east-1' (US East - N. Virginia).",
            region
        );
        warn!(
            "   Buckets in other regions will NOT be accepted by the SRA Submission Portal."
        );
        warn!(
            "   See: https://www.ncbi.nlm.nih.gov/sra/docs/data-delivery"
        );
    }

    // 1. Initialize AWS S3 client
    let config = aws_config::defaults(aws_config::BehaviorVersion::latest())
        .region(aws_config::Region::new(region.to_string()))
        .load()
        .await;

    let client = aws_sdk_s3::Client::new(&config);

    // 2. Verify bucket accessibility
    if !dry_run {
        check_bucket(&client, bucket).await?;
    }

    // 3. Collect and validate files
    let file_list = collect_files(files)?;
    if file_list.is_empty() {
        return Err(anyhow!("No valid files to upload"));
    }

    info!("📦 Files to upload: {}", file_list.len());
    for (path, size) in &file_list {
        info!(
            "   - {} ({})",
            path.file_name().unwrap_or_default().to_string_lossy(),
            indicatif::HumanBytes(*size)
        );
    }

    // Generate metadata template (works in both dry-run and real mode)
    if let Some(template_path) = metadata_template {
        generate_metadata_template(template_path, &file_list, bucket, prefix)?;
    }

    if dry_run {
        info!("🏜️  Dry Run completed. No files were uploaded.");
        return Ok(());
    }

    // 4. Upload files
    upload_files(&client, bucket, prefix, &file_list, concurrent).await?;

    // 5. Apply bucket policy if requested
    if apply_policy {
        apply_ncbi_bucket_policy(&client, bucket).await?;
    }

    info!("🎉 Upload workflow completed successfully!");
    Ok(())
}

/// Check if S3 bucket exists and is accessible
async fn check_bucket(client: &aws_sdk_s3::Client, bucket: &str) -> Result<()> {
    info!("🔍 Checking bucket accessibility: {}", bucket);
    client
        .head_bucket()
        .bucket(bucket)
        .send()
        .await
        .map_err(|e| {
            anyhow!(
                "Cannot access S3 bucket '{}': {}. \
                Please ensure the bucket exists and you have the necessary permissions.",
                bucket,
                e
            )
        })?;
    info!("✅ Bucket is accessible");
    Ok(())
}

/// Collect files to upload, validating that they exist and are non-empty
fn collect_files(files: &[PathBuf]) -> Result<Vec<(PathBuf, u64)>> {
    let mut file_list = Vec::new();
    for path in files {
        if !path.exists() {
            warn!("⚠️  File not found, skipping: {}", path.display());
            continue;
        }
        let metadata = path.metadata().context(format!(
            "Failed to read file metadata: {}",
            path.display()
        ))?;
        if metadata.is_dir() {
            warn!(
                "⚠️  Skipping directory (not supported yet): {}",
                path.display()
            );
            continue;
        }
        if metadata.len() == 0 {
            warn!("⚠️  Skipping empty file: {}", path.display());
            continue;
        }
        file_list.push((path.clone(), metadata.len()));
    }
    Ok(file_list)
}

/// Upload multiple files to S3 with concurrency control
async fn upload_files(
    client: &aws_sdk_s3::Client,
    bucket: &str,
    prefix: &Option<String>,
    files: &[(PathBuf, u64)],
    concurrent: usize,
) -> Result<()> {
    info!("📤 Uploading {} files to S3...", files.len());

    let semaphore = Arc::new(Semaphore::new(concurrent));
    let mp = Arc::new(MultiProgress::new());
    let success_count = Arc::new(AtomicUsize::new(0));
    let fail_count = Arc::new(AtomicUsize::new(0));
    let client = Arc::new(client.clone());

    let mut handles = Vec::new();

    for (path, size) in files {
        let sem = semaphore.clone();
        let mp = mp.clone();
        let success_count = success_count.clone();
        let fail_count = fail_count.clone();
        let client = client.clone();
        let bucket = bucket.to_string();
        let prefix = prefix.clone();
        let path = path.clone();
        let size = *size;

        let handle = tokio::spawn(async move {
            let _permit = sem.acquire().await.expect("semaphore closed");

            let filename = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();

            let key = match &prefix {
                Some(p) => format!("{}/{}", p.trim_end_matches('/'), filename),
                None => filename.clone(),
            };

            match upload_single_file(&client, &bucket, &key, &path, size, &mp).await {
                Ok(_) => {
                    success_count.fetch_add(1, Ordering::Relaxed);
                }
                Err(e) => {
                    error!("❌ Failed to upload {}: {}", filename, e);
                    fail_count.fetch_add(1, Ordering::Relaxed);
                }
            }
        });

        handles.push(handle);
    }

    for handle in handles {
        if let Err(e) = handle.await {
            warn!("Task join error: {}", e);
        }
    }

    let success = success_count.load(Ordering::Relaxed);
    let failed = fail_count.load(Ordering::Relaxed);

    info!(
        "📊 Upload results: {} succeeded, {} failed (total: {})",
        success,
        failed,
        files.len()
    );

    if failed > 0 {
        return Err(anyhow!("{} file(s) failed to upload", failed));
    }

    Ok(())
}

/// Upload a single file to S3 with progress tracking
async fn upload_single_file(
    client: &aws_sdk_s3::Client,
    bucket: &str,
    key: &str,
    path: &Path,
    size: u64,
    mp: &MultiProgress,
) -> Result<()> {
    let filename = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    let pb = mp.add(ProgressBar::new(size));
    pb.set_style(transfer_bar_style());
    pb.set_message(filename.clone());

    let body = ByteStream::from_path(path).await
        .context(format!("Failed to open file for upload: {}", path.display()))?;

    client
        .put_object()
        .bucket(bucket)
        .key(key)
        .body(body)
        .send()
        .await
        .context(format!("S3 PutObject failed for: {}", filename))?;

    pb.set_position(size);
    pb.finish_with_message(format!("✅ {}", filename));
    info!(
        "   ✅ Uploaded: {} → s3://{}/{} ({})",
        filename,
        bucket,
        key,
        indicatif::HumanBytes(size)
    );
    Ok(())
}

/// Apply NCBI SRA Submission Bucket Policy
///
/// Adds s3:ListBucket and s3:GetObject permissions for the NCBI SRA IAM user,
/// allowing NCBI to read files directly from your bucket for SRA submission.
async fn apply_ncbi_bucket_policy(
    client: &aws_sdk_s3::Client,
    bucket: &str,
) -> Result<()> {
    info!("🔐 Configuring NCBI SRA Submission Bucket Policy...");
    info!("   Principal: {}", NCBI_SRA_IAM_ARN);
    info!("   Actions: s3:ListBucket, s3:GetObject");

    // Get existing policy (if any)
    let existing_policy = match client
        .get_bucket_policy()
        .bucket(bucket)
        .send()
        .await
    {
        Ok(output) => {
            let policy_bytes = output.policy().ok_or_else(|| anyhow!("Empty policy response"))?;
            let policy_str = std::str::from_utf8(policy_bytes.as_ref())
                .context("Policy is not valid UTF-8")?;
            let policy: serde_json::Value =
                serde_json::from_str(policy_str).context("Failed to parse existing bucket policy")?;
            Some(policy)
        }
        Err(_) => {
            info!("   No existing bucket policy found, creating new one");
            None
        }
    };

    // Build NCBI SRA submission policy statements
    let ncbi_statements = serde_json::json!([
        {
            "Sid": "NCBISRASubmissionListBucket",
            "Effect": "Allow",
            "Principal": {
                "AWS": NCBI_SRA_IAM_ARN
            },
            "Action": "s3:ListBucket",
            "Resource": format!("arn:aws:s3:::{}", bucket)
        },
        {
            "Sid": "NCBISRASubmissionGetObject",
            "Effect": "Allow",
            "Principal": {
                "AWS": NCBI_SRA_IAM_ARN
            },
            "Action": "s3:GetObject",
            "Resource": format!("arn:aws:s3:::{}/*", bucket)
        }
    ]);

    // Merge with existing policy or create new one
    let policy_doc = if let Some(mut existing) = existing_policy {
        // Check if NCBI policy already exists
        if has_ncbi_sra_statement(&existing) {
            info!("   ✅ NCBI SRA submission policy already exists, skipping");
            return Ok(());
        }

        // Append new statements to existing policy
        if let Some(stmts) = existing.get_mut("Statement").and_then(|s| s.as_array_mut()) {
            if let Some(new_stmts) = ncbi_statements.as_array() {
                stmts.extend(new_stmts.clone());
            }
        }
        existing
    } else {
        serde_json::json!({
            "Version": "2012-10-17",
            "Statement": ncbi_statements
        })
    };

    let policy_json = serde_json::to_string_pretty(&policy_doc)?;

    client
        .put_bucket_policy()
        .bucket(bucket)
        .policy(policy_json)
        .send()
        .await
        .context("Failed to apply bucket policy")?;

    info!("   ✅ NCBI SRA submission bucket policy applied successfully");
    info!("   📋 Next steps:");
    info!("      1. Go to https://submit.ncbi.nlm.nih.gov/subs/sra/");
    info!("      2. In the file upload step, select 'Upload from Amazon S3 storage'");
    info!("      3. Provide your S3 paths in the format: s3://{}/<filename>", bucket);
    Ok(())
}

/// Check if NCBI SRA submission statement already exists in policy
fn has_ncbi_sra_statement(policy: &serde_json::Value) -> bool {
    if let Some(statements) = policy.get("Statement").and_then(|s| s.as_array()) {
        for stmt in statements {
            if let Some(principal) = stmt.get("Principal") {
                if let Some(aws) = principal.get("AWS").and_then(|a| a.as_str()) {
                    if aws == NCBI_SRA_IAM_ARN {
                        return true;
                    }
                }
                if let Some(aws_arr) = principal.get("AWS").and_then(|a| a.as_array()) {
                    if aws_arr.iter().any(|a| {
                        a.as_str()
                            .map(|s| s == NCBI_SRA_IAM_ARN)
                            .unwrap_or(false)
                    }) {
                        return true;
                    }
                }
            }
        }
    }
    false
}

/// Generate a TSV metadata template for SRA submission
///
/// Creates a tab-separated file with required SRA metadata fields that users
/// can fill in before submitting to the NCBI SRA Submission Portal.
fn generate_metadata_template(
    path: &Path,
    files: &[(PathBuf, u64)],
    bucket: &str,
    prefix: &Option<String>,
) -> Result<()> {
    info!("📝 Generating SRA metadata template: {}", path.display());

    let mut file =
        File::create(path).context(format!("Failed to create metadata template: {:?}", path))?;

    // Write TSV header with SRA submission fields
    writeln!(
        file,
        "filename\tfiletype\tassembly\ttitle\tlibrary_strategy\tlibrary_source\t\
         library_selection\tlibrary_layout\tplatform\tinstrument_model\t\
         design_description\ts3_path"
    )?;

    // Write rows for each file
    for (file_path, _size) in files {
        let filename = file_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy();

        let s3_key = match prefix {
            Some(p) => format!("{}/{}", p.trim_end_matches('/'), filename),
            None => filename.to_string(),
        };
        let s3_path = format!("s3://{}/{}", bucket, s3_key);

        // Determine filetype based on extension
        let filetype = if filename.ends_with(".fastq.gz") || filename.ends_with(".fq.gz") {
            "fastq"
        } else if filename.ends_with(".bam") {
            "bam"
        } else if filename.ends_with(".cram") {
            "cram"
        } else if filename.ends_with(".vcf") || filename.ends_with(".vcf.gz") {
            "vcf"
        } else {
            "fastq"
        };

        writeln!(
            file,
            "{}\t{}\t\t\t\t\t\t\t\t\t\t{}",
            filename, filetype, s3_path
        )?;
    }

    info!(
        "   ✅ Metadata template saved: {} ({} file entries)",
        path.display(),
        files.len()
    );
    info!("   📋 Fill in the empty columns before submitting to NCBI SRA");
    Ok(())
}
