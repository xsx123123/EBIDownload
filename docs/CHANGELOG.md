# Changelog

## [Unreleased]

## [1.4.2] - 2026-07-18

### Changed
- **Renamed to Polariseq**: Project rebranded from **EBIDownload** to **Polariseq** (`Polaris` + `seq`). The name is inspired by the night sky and Polaris (the North Star) — a guide for navigating vast sequencing data from ENA, NCBI SRA, GEO, and public reference databases.
- **Binary & crates**: CLI binary is now `polariseq`; crates are `polariseq-core`, `polariseq-cli`, and `polariseq` (GUI).
- **Config paths**: Default config is `polariseq.yaml` (beside the executable or under `~/.polariseq/`); managed deps live under `~/.local/share/polariseq/`.
- **Environment variable**: Progress API key env var is now `POLARISEQ_PROGRESS_KEY` (was `EBIDOWNLOAD_PROGRESS_KEY`).
- **Repository**: GitHub repository renamed to [xsx123123/polariseq](https://github.com/xsx123123/polariseq).
- **CLI banner**: Polariseq ASCII logo with centered star-themed tagline.
- **S3 resume fix**: Incomplete preallocated downloads no longer trigger early MD5 wipe of `.meta.json`.
- **Default chunk size**: Raised to **200 MiB** for faster large-file transfers.
- **Status-bar speed**: 3-second sliding-window average for a more stable aggregate rate.
- **Log UI**: Removed emoji prefixes from log lines; status bar styling kept.

### Migration
| Old | New |
|-----|-----|
| `EBIDownload` binary | `polariseq` |
| `EBIDownload.yaml` / `~/.EBIDownload/` | `polariseq.yaml` / `~/.polariseq/` |
| `~/.local/share/EBIDownload/` | `~/.local/share/polariseq/` |
| `EBIDOWNLOAD_PROGRESS_KEY` | `POLARISEQ_PROGRESS_KEY` |
| github.com/xsx123123/EBIDownload | github.com/xsx123123/polariseq |

## [1.4.1] - 2026-07-16

### Added
- **Public Data Download**: New `public-data` subcommand to download public reference databases (e.g., NCBI BLAST databases, Kraken indices) from configured S3 sources.
- **Colored Terminal Logging**: Added a custom `tracing-subscriber` formatter that colorizes timestamps, log levels, and module targets while keeping log files free of ANSI escape codes.

### Changed
- **Cleaner Progress Messages**: Re-routed ad-hoc `println!` / `MultiProgress::println` messages in the AWS S3 downloader through `tracing` for consistent formatting, and removed awkward leading spaces.
- **Simplified Network Health Warnings**: Removed verbose `(error sending request for url (...): ...)` details from the `NCBI API is NOT reachable!` warning; only the concise warning and DNS/Proxy hint remain.
- **Version Bump**: Bumped version to `1.4.1` across all crates, the GUI package, and documentation.

## [1.4.0] - 2026-06-12

### Added
- **Automatic Dependency Management**: New `deps` subcommand to download, verify, and install NCBI `sra-tools` automatically:
  - `polariseq deps install` downloads the correct pre-built release for the current platform
  - `polariseq deps check` verifies that `prefetch` / `fasterq-dump` are available
  - `polariseq deps list` shows managed installations
  - `polariseq deps remove` removes a managed installation
- **GUI Dependency Auto-Detection**: The GUI now checks for `sra-tools` on startup and offers a one-click install dialog if it is missing.
- **CLI Install Progress Bar**: `deps install` now shows a real-time progress bar for download, checksum verification, and extraction.

### Changed
- **Version Bump**: Bumped version to `1.4.0` across all crates, the GUI package, and documentation.
- **YAML Path Logging**: `deps install` now reports the absolute path of the updated `polariseq.yaml` file.

## [1.3.7] - 2026-06-05

### Added
- **Upload Subcommand**: New `upload` subcommand for uploading sequencing data to AWS S3 for fast NCBI SRA submission. Includes:
  - Concurrent S3 file uploads with progress tracking
  - Automatic NCBI SRA bucket policy configuration (`--apply-policy`)
  - SRA metadata template generation (`--metadata-template`)
  - Region validation with `us-east-1` warning (NCBI hard requirement)
  - Dry-run preview mode (`--dry-run`)
- **Subcommand Architecture**: Refactored CLI from flat arguments to `download` / `upload` subcommands using `clap::Subcommand`

### Changed
- **CLI Structure**: All download commands now require the `download` subcommand prefix (e.g., `polariseq download -A PRJNA1251654 -o ./data -d aws`)
- **Banner Update**: Renamed from "EMBL-ENA Data Downloader" to "EMBL-ENA Data Toolkit" to reflect both download and upload capabilities
- **Global Options**: `--yaml`, `--log-level`, `--log-format` are now global options shared across subcommands

## [1.3.6] - 2026-05-25

### Added
- **Colorful ASCII Logo & Help**: Added a vibrant, multi-colored ASCII art logo to the CLI help output. Help sections are now color-coded (green headers, blue options, cyan placeholders) for better readability.
- **Unicode Progress Bars**: Replaced plain ASCII progress bars with modern Unicode block characters (`█▓░`) and added spinner animations for a smoother download experience.
- **Smart File Naming**: Log files, metadata TSV, and MD5 checksum files now automatically include the project Accession ID in their filenames (e.g., `Polariseq_PRJNA123_...log`, `ena_metadata_PRJNA123.tsv`).
- **Project Annotation**: `ena_metadata.tsv` now includes a `# Project Accession:` comment header for easy traceability.

### Changed
- **Progress Bar Layout**: Redesigned progress bar template to show percentage, aligned byte counters, speed, and ETA in a clean, compact format.
- **Download Completion Messages**: Results (speed, MD5 verification) are now written to both the terminal and the log file via `tracing`.
- **Help Output Grouping**: CLI arguments are now organized into `Input Options`, `Download Options`, `Filters`, and `Advanced Options` sections.

### Fixed
- **Cargo.toml Localization**: Removed Chinese comments from `Cargo.toml` in favor of English.

## [1.3.5] - 2025-12-27

### Added
- **AWS S3 下载预校验优化**：在启动 AWS 下载前增加本地文件检查逻辑。若存在大小一致的文件，则优先进行 MD5 校验。
- **智能跳过机制**：MD5 校验通过后自动跳过下载阶段，直接进入后续的提取与压缩流程，大幅提升断点续传和重复运行的效率。

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
- Fixed environment configuration in `polariseq_env.yaml`.
- Improved log printing logic and user feedback.

## [0.0.2] - 2025-11-06

### Added
- Initial release of Polariseq.
- Basic support for Aspera, FTP, and Prefetch download methods.
- Documentation and Usage examples.
