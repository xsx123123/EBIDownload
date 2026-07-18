use anyhow::{anyhow, Result};
use reqwest::Url;
use wildmatch::WildMatch;

/// Parsed S3 location with a bucket and the path after the bucket name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct S3Location {
    pub bucket: String,
    pub key: String,
}

/// Parse an `s3://bucket/key` URL.
pub fn parse_s3_url(value: &str) -> Result<S3Location> {
    let value = value.trim();
    let remainder = value
        .strip_prefix("s3://")
        .ok_or_else(|| anyhow!("S3 URL must start with s3://: {value}"))?;

    let (bucket, key) = remainder
        .split_once('/')
        .map(|(bucket, key)| (bucket, key.to_string()))
        .unwrap_or((remainder, String::new()));

    if bucket.is_empty() {
        return Err(anyhow!("S3 URL is missing a bucket name: {value}"));
    }
    if bucket.contains('/') || bucket.contains('?') || bucket.contains('#') {
        return Err(anyhow!("Invalid S3 bucket name in URL: {value}"));
    }

    Ok(S3Location {
        bucket: bucket.to_string(),
        key,
    })
}

/// Convert an S3 bucket/key pair to its anonymous virtual-hosted HTTPS URL.
pub fn s3_url_to_https(bucket: &str, key: &str) -> Result<String> {
    let mut url = Url::parse(&format!("https://{bucket}.s3.amazonaws.com/"))?;
    url.set_path(key);
    Ok(url.into())
}

/// Apply AWS CLI-like filtering: exclude first, then include as an override.
///
/// `key` must be relative to the configured S3 prefix. This makes a pattern
/// such as `nt.*` match `prefix/nt.000.nsq` after the prefix is removed.
pub fn should_download_key(key: &str, exclude: Option<&str>, include: Option<&str>) -> bool {
    let excluded = exclude.is_some_and(|pattern| WildMatch::new(pattern).matches(key));
    if include.is_some_and(|pattern| WildMatch::new(pattern).matches(key)) {
        return true;
    }
    !excluded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_s3_url_with_prefix() {
        assert_eq!(
            parse_s3_url("s3://ncbi-blast-databases/2026-07-10/").unwrap(),
            S3Location {
                bucket: "ncbi-blast-databases".to_string(),
                key: "2026-07-10/".to_string(),
            }
        );
    }

    #[test]
    fn rejects_non_s3_url() {
        assert!(parse_s3_url("https://example.org/file").is_err());
    }

    #[test]
    fn include_overrides_exclude() {
        assert!(should_download_key("nt.000.nsq", Some("*"), Some("nt.*")));
        assert!(!should_download_key("taxdb.btd", Some("*"), Some("nt.*")));
        assert!(should_download_key("taxdb.btd", None, None));
    }

    #[test]
    fn converts_s3_key_to_encoded_https_url() {
        assert_eq!(
            s3_url_to_https("example-bucket", "folder/file name.gz").unwrap(),
            "https://example-bucket.s3.amazonaws.com/folder/file%20name.gz"
        );
    }
}
