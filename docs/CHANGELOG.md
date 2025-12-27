# Changelog

## [1.3.4] - 2025-12-27

### Added
- **Full Metadata Support**: Expanded `EnaRecord` to capture all 49 fields from the EBI API (e.g., `study_accession`, `tax_id`, `instrument_model`, `read_count`), providing comprehensive dataset details.
- **Metadata Export**: Automatically saves all fetched and filtered metadata to `ena_metadata.tsv` in the output directory.
- **Output Organization**: `R1/R2_fastq_md5.tsv` files are now saved directly to the specified output directory instead of the working directory.
- **Log Management Improvement**: Logs are now automatically saved in the user-specified output directory (`--output`) for better organization and management.
- **Multi-thread Progress Coordination**: Integrated `indicatif::MultiProgress` to resolve display conflicts when downloading multiple files concurrently.

### Fixed
- **Progress Bar Rendering**: Fixed an issue where multiple download threads would overwrite each other's progress bars in the terminal.
- **Terminal Output Cleanliness**: Used `pb.println` to ensure metadata details and status messages do not interfere with active progress bars.

## [1.3.3] - 2025-12-19

### Fixed
- Fixed network connectivity issues related to Ensembl IP resolution.
- Improved bash command execution reliability.
- Optimized retry mechanism for network requests.

## [1.3.2] - 2025-12-19

### Added
- **Smart Auto-Fallback**: Introduced `auto` download mode (`-d auto`), which attempts AWS S3 download first and automatically falls back to Prefetch if it fails.
- **Advanced Filtering**: Added support for Regex-based filtering (`--filter-sample`, `--filter-run`, `--exclude-sample`, `--exclude-run`) for precise data selection.
- **Log Formatting**: Added `--log-format` option to support JSON log output for better integration with other tools.
- **Prefetch Limits**: Added `--max-size` parameter to limit file sizes in Prefetch mode.

### Changed
- **Default Behavior**: Changed the default download method from `prefetch` to `aws` for better performance.
- **Splice Function**: Enhanced file splicing logic for multipart downloads.

## [1.2.6] - 2025-12-18

### Added
- **AWS S3 Module**: Implemented the initial AWS S3 high-speed download module using `aws-sdk-s3`.
- **Global Acceleration**: Enabled direct multi-threaded downloading from NCBI SRA AWS S3 buckets.

## [0.0.3] - 2025-11-07

### Added
- **Logging**: Added configurable log levels (`--log-level`) and debug output.
- **MD5 Verification**: Added warning notifications for MD5 mismatches between SRA and EBI data.
- **CI/CD**: Added GitHub Actions workflow for automated Rust builds.

### Fixed
- Fixed environment configuration in `EBIDownload_env.yaml`.
- Improved log printing logic and user feedback.

## [0.0.2] - 2025-11-06

### Added
- Initial release of EBIDownload.
- Basic support for Aspera, FTP, and Prefetch download methods.
- Documentation and Usage examples.
