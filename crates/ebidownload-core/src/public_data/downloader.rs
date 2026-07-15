use super::{parse_s3_url, s3_url_to_https, should_download_key, DatabaseType, PublicDatabase};
use crate::aws_s3::{ResumableDownloader, SraMetadata};
use crate::generate_md5sum_file_at;
use anyhow::{anyhow, Context, Result};
use aws_sdk_s3::Client;
use indicatif::{HumanBytes, MultiProgress};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Semaphore;
use tracing::{info, warn};

const DEFAULT_FILE_WORKERS: usize = 8;
const DEFAULT_INNER_WORKERS: usize = 4;
const DEFAULT_CHUNK_SIZE_MB: u64 = 64;

#[derive(Debug, Clone)]
struct PublicObject {
    key: String,
    size: u64,
    md5: Option<String>,
}

/// Coordinates anonymous S3 listing and resumable downloads for public data.
#[derive(Clone)]
pub struct PublicDataDownloader {
    client: Client,
    file_workers: usize,
    inner_workers: usize,
    chunk_size_mb: u64,
    progress: Arc<MultiProgress>,
}

impl PublicDataDownloader {
    /// Create an unsigned S3 client for public buckets in `us-east-1`.
    pub async fn new() -> Result<Self> {
        let config = aws_config::defaults(aws_config::BehaviorVersion::latest())
            .no_credentials()
            .region(aws_config::Region::new("us-east-1"))
            .load()
            .await;

        Ok(Self {
            client: Client::new(&config),
            file_workers: DEFAULT_FILE_WORKERS,
            inner_workers: DEFAULT_INNER_WORKERS,
            chunk_size_mb: DEFAULT_CHUNK_SIZE_MB,
            progress: Arc::new(MultiProgress::new()),
        })
    }

    /// Override concurrency for callers that need to reduce request pressure.
    pub fn with_workers(mut self, file_workers: usize, inner_workers: usize) -> Self {
        self.file_workers = file_workers.max(1);
        self.inner_workers = inner_workers.max(1);
        self
    }

    /// Override the HTTP range chunk size in MiB.
    pub fn with_chunk_size_mb(mut self, chunk_size_mb: u64) -> Self {
        self.chunk_size_mb = chunk_size_mb.max(1);
        self
    }

    /// Use a caller-owned progress renderer for all concurrent file downloads.
    pub fn with_progress(mut self, progress: Arc<MultiProgress>) -> Self {
        self.progress = progress;
        self
    }

    /// Download the public database selected by its YAML map key.
    pub async fn download_named(
        &self,
        databases: &HashMap<String, PublicDatabase>,
        name: &str,
        output_dir: &Path,
        dry_run: bool,
    ) -> Result<()> {
        if databases.is_empty() {
            return Err(anyhow!(
                "No public_data entries found in the YAML configuration"
            ));
        }

        std::fs::create_dir_all(output_dir).with_context(|| {
            format!(
                "Failed to create public data output directory {}",
                output_dir.display()
            )
        })?;

        let database = databases.get(name).ok_or_else(|| {
            let mut available = databases.keys().cloned().collect::<Vec<_>>();
            available.sort();
            anyhow!(
                "Public database '{name}' is not configured. Available entries: {}",
                available.join(", ")
            )
        })?;
        self.download_database(name, database, output_dir, dry_run)
            .await
    }

    /// Download a configured public database into `output_dir`.
    pub async fn download_database(
        &self,
        name: &str,
        database: &PublicDatabase,
        output_dir: &Path,
        dry_run: bool,
    ) -> Result<()> {
        let source = parse_s3_url(&database.s3_url)
            .with_context(|| format!("Invalid S3 URL for public database '{name}'"))?;

        info!(
            "📚 Downloading public database '{}' ({}) from {}",
            name, database.description, database.s3_url
        );

        match database.database_type {
            DatabaseType::File => {
                if source.key.is_empty() || source.key.ends_with('/') {
                    return Err(anyhow!(
                        "Public database '{name}' is type file but does not identify an S3 object"
                    ));
                }
                let object = self.head_object(&source.bucket, &source.key).await?;
                if dry_run {
                    info!(
                        "🏜️ Would download s3://{}/{} ({})",
                        source.bucket,
                        source.key,
                        HumanBytes(object.size)
                    );
                    return Ok(());
                }
                self.download_object(&source.bucket, &object, output_dir)
                    .await?;
                self.generate_md5_manifest(output_dir, name, &[object])
            }
            DatabaseType::Folder => {
                let objects = self
                    .list_objects(&source.bucket, &source.key, database)
                    .await?;
                if objects.is_empty() {
                    warn!("No objects matched public database '{}'", name);
                    return Ok(());
                }
                info!("📦 '{}' contains {} matching objects", name, objects.len());
                if dry_run {
                    info!("🏜️ Dry-run mode: no public data will be downloaded");
                    for object in &objects {
                        info!("   - {} ({})", object.key, HumanBytes(object.size));
                    }
                    return Ok(());
                }
                self.download_objects(&source.bucket, &objects, output_dir)
                    .await?;
                self.generate_md5_manifest(output_dir, name, &objects)
            }
        }
    }

    async fn head_object(&self, bucket: &str, key: &str) -> Result<PublicObject> {
        let response = self
            .client
            .head_object()
            .bucket(bucket)
            .key(key)
            .send()
            .await
            .with_context(|| format!("Failed to read metadata for s3://{bucket}/{key}"))?;
        let size = response
            .content_length()
            .ok_or_else(|| anyhow!("S3 object did not report a size: s3://{bucket}/{key}"))?;
        Ok(PublicObject {
            key: key.to_string(),
            size: u64::try_from(size)
                .map_err(|_| anyhow!("S3 object has an invalid size: s3://{bucket}/{key}"))?,
            md5: md5_from_etag(response.e_tag()),
        })
    }

    async fn list_objects(
        &self,
        bucket: &str,
        prefix: &str,
        database: &PublicDatabase,
    ) -> Result<Vec<PublicObject>> {
        let mut objects = Vec::new();
        let mut continuation_token = None;

        loop {
            let mut request = self.client.list_objects_v2().bucket(bucket).prefix(prefix);
            if let Some(token) = continuation_token.as_deref() {
                request = request.continuation_token(token);
            }

            let response = request
                .send()
                .await
                .with_context(|| format!("Failed to list s3://{bucket}/{prefix}"))?;

            for object in response.contents() {
                let Some(key) = object.key() else {
                    continue;
                };
                let Some(size) = object.size() else {
                    warn!("Skipping S3 object without a size: s3://{bucket}/{key}");
                    continue;
                };
                if key.ends_with('/') && size == 0 {
                    continue;
                }
                let relative_key = key.strip_prefix(prefix).unwrap_or(key);
                if !should_download_key(
                    relative_key,
                    database.exclude.as_deref(),
                    database.include.as_deref(),
                ) {
                    continue;
                }
                let size = u64::try_from(size)
                    .map_err(|_| anyhow!("S3 object has an invalid size: s3://{bucket}/{key}"))?;
                objects.push(PublicObject {
                    key: key.to_string(),
                    size,
                    md5: md5_from_etag(object.e_tag()),
                });
            }

            if !response.is_truncated().unwrap_or(false) {
                break;
            }
            continuation_token = response.next_continuation_token().map(ToOwned::to_owned);
            if continuation_token.is_none() {
                return Err(anyhow!(
                    "S3 list response for s3://{bucket}/{prefix} is truncated without a continuation token"
                ));
            }
        }

        Ok(objects)
    }

    async fn download_objects(
        &self,
        bucket: &str,
        objects: &[PublicObject],
        output_dir: &Path,
    ) -> Result<()> {
        let semaphore = Arc::new(Semaphore::new(self.file_workers));
        let mut handles = Vec::with_capacity(objects.len());

        for object in objects {
            let downloader = self.clone();
            let bucket = bucket.to_string();
            let output_dir = output_dir.to_path_buf();
            let semaphore = semaphore.clone();
            let object = object.clone();
            handles.push(tokio::spawn(async move {
                let _permit = semaphore.acquire_owned().await.expect("semaphore closed");
                downloader
                    .download_object(&bucket, &object, &output_dir)
                    .await
            }));
        }

        let mut first_error = None;
        for handle in handles {
            match handle.await.context("Public data download task panicked")? {
                Ok(()) => {}
                Err(error) if first_error.is_none() => first_error = Some(error),
                Err(_) => {}
            }
        }
        if let Some(error) = first_error {
            return Err(error);
        }
        Ok(())
    }

    async fn download_object(
        &self,
        bucket: &str,
        object: &PublicObject,
        output_dir: &Path,
    ) -> Result<()> {
        let http_url = s3_url_to_https(bucket, &object.key)?;
        let downloader = ResumableDownloader::new(
            object.key.to_string(),
            SraMetadata {
                s3_uri: format!("s3://{bucket}/{}", object.key),
                http_url,
                md5: object.md5.clone(),
                size: object.size,
            },
            PathBuf::from(output_dir),
            self.chunk_size_mb,
            self.inner_workers,
            Some(self.progress.clone()),
            None,
        )
        .await?;

        if downloader.start().await? {
            Ok(())
        } else {
            Err(anyhow!(
                "Download did not complete for s3://{bucket}/{}",
                object.key
            ))
        }
    }

    fn generate_md5_manifest(
        &self,
        output_dir: &Path,
        database_name: &str,
        objects: &[PublicObject],
    ) -> Result<()> {
        let files = objects
            .iter()
            .map(|object| {
                let filename = object
                    .key
                    .rsplit('/')
                    .next()
                    .filter(|name| !name.is_empty())
                    .ok_or_else(|| anyhow!("S3 object key has no filename: {}", object.key))?;
                Ok(output_dir.join(filename))
            })
            .collect::<Result<Vec<_>>>()?;
        let manifest_path = output_dir.join(format!("{database_name}.md5"));
        generate_md5sum_file_at(&manifest_path, &files)?;
        Ok(())
    }
}

fn md5_from_etag(etag: Option<&str>) -> Option<String> {
    let value = etag?.trim_matches('"');
    (value.len() == 32 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| value.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::md5_from_etag;

    #[test]
    fn accepts_only_plain_md5_etags() {
        assert_eq!(
            md5_from_etag(Some("\"d41d8cd98f00b204e9800998ecf8427e\"")),
            Some("d41d8cd98f00b204e9800998ecf8427e".to_string())
        );
        assert_eq!(
            md5_from_etag(Some("d41d8cd98f00b204e9800998ecf8427e-2")),
            None
        );
        assert_eq!(md5_from_etag(None), None);
    }
}
