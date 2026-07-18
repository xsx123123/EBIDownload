//! Download support for publicly available reference databases stored in S3.

mod config;
mod downloader;
mod s3;
pub mod validator;

pub use config::{DatabaseType, PublicDatabase, ValidateConfig};
pub use downloader::PublicDataDownloader;
pub use s3::{parse_s3_url, s3_url_to_https, should_download_key, S3Location};
