use serde::{Deserialize, Serialize};

/// Configuration for validating a downloaded public database volume.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ValidateConfig {
    /// Whether to run validation after each volume finishes downloading.
    pub enabled: bool,
    /// Name of the tool in `software:` whose path should be used.
    pub tool: String,
    /// BLAST database type: `nucl` for nucleotide, `prot` for protein.
    pub dbtype: String,
    /// Maximum number of retry attempts after a validation failure.
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
    /// Seconds to wait before re-downloading a corrupted volume.
    #[serde(default = "default_retry_delay")]
    pub retry_delay_seconds: u64,
}

fn default_max_retries() -> u32 {
    2
}

fn default_retry_delay() -> u64 {
    10
}

/// Configuration for one downloadable public reference database.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PublicDatabase {
    /// Public object URL or object-prefix URL, such as `s3://bucket/path/`.
    pub s3_url: String,
    /// Human-readable description written to progress logs.
    pub description: String,
    /// Whether `s3_url` addresses one object or a prefix to enumerate.
    #[serde(alias = "db_type")]
    pub database_type: DatabaseType,
    /// Optional wildcard exclusion pattern applied before `include`.
    pub exclude: Option<String>,
    /// Optional wildcard inclusion pattern applied after `exclude`.
    pub include: Option<String>,
    /// Optional post-download validation configuration.
    pub validate: Option<ValidateConfig>,
}

/// The shape of the public database source in the YAML configuration.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DatabaseType {
    Folder,
    File,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn deserializes_folder_database_from_yaml() {
        let databases: HashMap<String, PublicDatabase> = serde_yaml::from_str(
            r#"
ncbi_nt:
  s3_url: s3://ncbi-blast-databases/current/
  description: NCBI nt database
  database_type: folder
  exclude: "*"
  include: "nt.*"
"#,
        )
        .unwrap();

        let database = databases.get("ncbi_nt").unwrap();
        assert_eq!(database.database_type, DatabaseType::Folder);
        assert_eq!(database.include.as_deref(), Some("nt.*"));
        assert!(database.validate.is_none());
    }

    #[test]
    fn deserializes_database_with_validate_config() {
        let databases: HashMap<String, PublicDatabase> = serde_yaml::from_str(
            r#"
ncbi_nr:
  s3_url: s3://ncbi-blast-databases/current/
  description: NCBI nr database
  database_type: folder
  exclude: "*"
  include: "nr.*"
  validate:
    enabled: true
    tool: blastdbcmd
    dbtype: prot
    max_retries: 2
    retry_delay_seconds: 10
"#,
        )
        .unwrap();

        let database = databases.get("ncbi_nr").unwrap();
        let validate = database.validate.as_ref().unwrap();
        assert!(validate.enabled);
        assert_eq!(validate.tool, "blastdbcmd");
        assert_eq!(validate.dbtype, "prot");
        assert_eq!(validate.max_retries, 2);
        assert_eq!(validate.retry_delay_seconds, 10);
    }
}
