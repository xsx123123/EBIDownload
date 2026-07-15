
[中文文档](./docs/README_zh.md) | English

# EBIDownload

EBIDownload is a Rust-based toolkit for efficiently downloading and uploading sequencing data from the European Bioinformatics Institute (EBI), NCBI SRA, and GEO (Gene Expression Omnibus). It provides both a **command-line interface (CLI)** and a **cross-platform desktop GUI** (powered by Tauri), making it accessible to both bioinformatics engineers and wet-lab researchers.

For GEO datasets, simply obtain the associated **BioProject ID** (e.g., `PRJNAxxxxxx`) and pass it to EBIDownload to fetch the underlying sequencing data at high speed.

By default, EBIDownload utilizes **AWS S3 global acceleration** to achieve ultra-fast download speeds (comparable to IDM/Aspera). **It is capable of downloading 2TB of data from the SRA database to local storage within 24 hours**, while providing full support for **resumable downloads** and **MD5 integrity verification**. It also uses Rust-native parallel gzip compression (via the [`gzp`](https://crates.io/crates/gzp) crate with the `libdeflate` backend), eliminating the need to install external compression tools.

## What's New in v1.4.1

- **Public Data Download**: New `public-data` subcommand to download public reference databases from configured S3 sources.
- **Colored Terminal Logging**: Logs now use a colorized format (timestamp purple, level auto-colored, module cyan) while log files remain plain text.
- **Cleaner Progress Messages**: Progress messages are now consistently formatted through `tracing`.

## What's New in v1.4.0

- **Light / Dark theme toggle**: A new theme button in the GUI header switches between dark and light modes. The choice is persisted across sessions (`localStorage`) and respects the system's `prefers-color-scheme` on first launch.
- **GitHub link in About tab**: The About tab now includes a "View on GitHub" link that opens the project's [GitHub repository](https://github.com/xsx123123/EBIDownload) in your default browser.
- **Default config moved to `~/.EBIDownload/EBIDownload.yaml`**: The GUI and CLI now look for `EBIDownload.yaml` under `~/.EBIDownload/` by default, and auto-load it on startup.
- **Automatic Dependency Management**: EBIDownload can now automatically download, verify, and install NCBI `sra-tools` via the new `EBIDownload deps install` command. The GUI also checks for dependencies on startup and offers one-click installation if `sra-tools` is missing.
- **GUI logs written to output directory**: While a download is running, logs are mirrored to `output/EBIDownload.log` for offline review, and `TRACE` noise is filtered to keep log files small.
- **Pause / Stop downloads in GUI**: AWS downloads can now be paused and resumed; any in-progress download can be stopped.
- **Real-time speed in Status column**: AWS downloads show live `MB/s` speed next to each run's progress bar.
- **Smooth Overall Progress**: Overall progress is now computed from the average percentage across all runs instead of counting completed runs.
- **Upload marked experimental**: Both CLI and GUI now warn that the upload subcommand is still under testing.
- **Clean script rewritten in Python**: `clean.sh` has been replaced with `clean.py` for cross-platform cleanup.
- **Public S3 reference databases**: The `public-data` command downloads one YAML-selected public database at a time, with anonymous S3 access, resumable HTTP ranges, object filtering, and dry-run preview.

![EBIDownload GUI](./docs/GUI.png)

*The EBIDownload desktop GUI: Download, Upload, Settings, and About tabs.*

## Features

- **Dual Interface (CLI + GUI)**: Choose between a powerful command-line tool for scripting and automation, or an intuitive desktop GUI (Windows/macOS/Linux) for visual operation.
- **AWS S3 Acceleration (Most Recommended)**: Direct multi-threaded downloading from NCBI SRA AWS S3 buckets, maximizing bandwidth utilization for global high-speed access. This is the fastest and most reliable method for large-scale data acquisition.
- **Parallel Processing**: Supports multi-threaded downloading, conversion, and native parallel gzip compression.
- **Easy Configuration**: Manages software paths and keys through a simple YAML file. The GUI provides a visual settings panel for path configuration.
- **GEO Dataset Support**: Download sequencing data associated with GEO records using the corresponding **BioProject ID** (`PRJNAxxxxxx`).
- **Flexible Usage**: Supports direct downloads via project accession numbers or TSV file lists.
- **Resumable Downloads**: Supports resumable downloads in `aws` and `prefetch` modes, ensuring download continuity.
- **Smart Auto-Fallback**: Automatically attempts AWS S3 first and seamlessly switches to Prefetch if the AWS download fails (Mode: `auto`).
- **Public Reference Data Downloads**: Download configured NCBI BLAST, Kraken, or other public S3 databases with file-level and range-level concurrency.
- **Advanced Filtering**: Supports Regex-based filtering to precisely include or exclude specific samples or runs.
- **Real-time Progress (GUI)**: Visual progress bars, per-run download speed, smooth overall progress, download queue management, and live log streaming in the desktop application.
- **Pause / Stop Downloads (GUI)**: Pause and resume AWS downloads, or stop any in-progress download.
- **Light / Dark Themes (GUI)**: One-click theme toggle in the header; choice is persisted across sessions and follows the system theme on first launch.
- **Automatic Dependency Management**: One-click or CLI-driven installation of `sra-tools`; the GUI checks for dependencies on startup.
- **Auto-Collapse UI During Download**: Configuration cards fold away and the progress panel expands automatically when a download starts.
- **HTTP Progress API (CLI)**: Optional encrypted HTTP endpoint for external platforms to query real-time download progress (AES-256-GCM).
- **md5.txt Generation**: Automatically generates md5sum-compatible checksum file for all compressed outputs after download completes.

---

## Quick Start

After [building](#3-building-the-program) the CLI, you can start downloading data in just a few commands:

```bash
# 1. Install the required sra-tools dependency automatically
./target/release/EBIDownload deps install

# 2. Download a project by BioProject ID using AWS S3 acceleration (default)
./target/release/EBIDownload download -A PRJNA1251654 -o ./data

# 3. Download with more parallelism for large datasets
./target/release/EBIDownload download -A PRJNA1251654 -o ./data -d aws -p 4 -t 8
```

For **GEO datasets**, find the associated **BioProject ID** on the GEO record page (usually under "SRA" or "BioProject" links) and pass it to EBIDownload:

```bash
# Download GEO data via its BioProject ID at high speed
./target/release/EBIDownload download -A PRJNA833659 -o ./data -p 4 -t 8
```

Run `./target/release/EBIDownload --help` or `./target/release/EBIDownload download --help` for the full list of options.

---

## 1. Prerequisites and Setup

**Why are external dependencies required?**
Since the raw data downloaded from NCBI/EBI is typically in `.sra` format, it must be converted to standard `.fastq` format. Because there are currently no mature native Rust libraries available for parsing `.sra` files, this tool relies on the external `sra-tools` toolkit for this conversion step. The subsequent `.fastq` → `.fastq.gz` compression is handled internally by Rust-native parallel gzip (no external compression tool needed).

Only one external dependency is required:

- **`sra-tools` (`prefetch` / `fasterq-dump`)**: Required for `prefetch` downloads and `.sra` → `.fastq` conversion. EBIDownload can **automatically download and install** this for you (see [Dependency Management](#3b-dependency-management)). You can also install it manually if you prefer.

### a. Conda Environment (optional manual install)

If you prefer Conda, you can create an isolated runtime environment to install `sra-tools`.

```bash
# Create and activate the conda environment using the provided .yaml file
conda env create -f ./docs/EBIDownload_env.yaml
conda activate EBIDownload_env
```

---

## 2. Project Structure

This project is organized as a Rust workspace with three crates:

```
crates/
├── ebidownload-core/     # Shared library: download/upload logic + data types
├── ebidownload-cli/      # Command-line tool
└── ebidownload-gui/      # Tauri desktop application (Rust backend + React frontend)
```

The **core** crate contains all shared business logic (AWS S3, FTP, Aspera, Prefetch, S3 Upload). Both CLI and GUI depend on it, ensuring consistent behavior across interfaces.

---

## 3. Building the Program

### Prerequisites

- [Rust](https://www.rust-lang.org/tools/install) toolchain
- [Node.js](https://nodejs.org/) 18+ (only for GUI build)
- `sra-tools` is **optional at build time** — you can let EBIDownload install it automatically later (see [Dependency Management](#3b-dependency-management))

### a. Build CLI Only

```bash
# Build CLI for development
CC=clang cargo build -p ebidownload-cli

# Build CLI for release
CC=clang cargo build -p ebidownload-cli --release

# Run CLI
./target/release/EBIDownload --help

# Automatically install the required sra-tools dependency
./target/release/EBIDownload deps install
```

### b. Dependency Management

EBIDownload can automatically download, verify, and configure NCBI `sra-tools` so you do not have to install it manually.

```bash
# Install sra-tools into a managed directory and write the paths to EBIDownload.yaml
./target/release/EBIDownload deps install

# Check whether sra-tools is available (config → managed dir → PATH)
./target/release/EBIDownload deps check

# List detected sra-tools paths
./target/release/EBIDownload deps list

# Remove the managed sra-tools installation
./target/release/EBIDownload deps remove
```

The GUI also performs this check automatically on startup. If `sra-tools` is missing, it shows a one-click install dialog and downloads the correct pre-built release for your platform.

### c. Build GUI (Desktop App)

Tauri **does not support cross-compilation**. You must build on the target platform:

| Target Platform | Build Host | Output Format |
|-----------------|------------|---------------|
| macOS | macOS (Intel or Apple Silicon) | `.dmg`, `.app` |
| Windows | Windows | `.msi`, `.exe` |
| Linux | Linux | `.AppImage`, `.deb` |

#### macOS

```bash
# 1. Install system dependencies
xcode-select --install

# 2. Build
cd crates/ebidownload-gui
npm install
npm run tauri build

# 3. Output
#   Intel Mac:    src-tauri/target/release/bundle/dmg/EBIDownload_1.4.0_x64.dmg
#   Apple Silicon: src-tauri/target/release/bundle/dmg/EBIDownload_1.4.0_aarch64.dmg
```

`sra-tools` will be installed automatically on first GUI launch if it is not found. If you prefer to use your own installation, create `EBIDownload.yaml` before running:

```yaml
# Apple Silicon Mac (M1/M2/M3)
software:
  prefetch: /opt/homebrew/bin/prefetch
  fasterq_dump: /opt/homebrew/bin/fasterq-dump
```

#### Windows

```powershell
# 1. Build (in PowerShell or CMD)
cd crates/ebidownload-gui
npm install
npm run tauri build

# 2. Output
#    src-tauri\target\release\bundle\msi\EBIDownload_1.4.0_x64_en-US.msi
#    src-tauri\target\release\bundle\nsis\EBIDownload_1.4.0_x64-setup.exe
```

`sra-tools` will be installed automatically on first GUI launch if it is not found.

**Note**: Windows may show a SmartScreen warning on first run because the binary is not code-signed. This is expected for unsigned applications.

#### Linux

```bash
# 1. Install system dependencies (Ubuntu/Debian example)
sudo apt-get update
sudo apt-get install libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev patchelf

# 2. Build
cd crates/ebidownload-gui
npm install
npm run tauri build

# 3. Output
#    src-tauri/target/release/bundle/appimage/ebidownload_1.4.0_amd64.AppImage
#    src-tauri/target/release/bundle/deb/ebidownload_1.4.0_amd64.deb
```

`sra-tools` will be installed automatically on first GUI launch if it is not found.

### d. Clean Build Artifacts

To remove all build artifacts (Rust `target/`, `node_modules`, `dist`, etc.), run the provided Python script from the project root:

```bash
python3 clean.py
```

After cleaning, rebuild the GUI with:

```bash
cd crates/ebidownload-gui
npm install
npm run tauri dev
```

### e. CI/CD Automatic Multi-Platform Build

To build for all three platforms automatically, use GitHub Actions. See [`.github/workflows/build.yml`](./.github/workflows/build.yml) for a ready-to-use workflow that produces `.dmg`, `.msi`, and `.AppImage` on every release.

```yaml
# .github/workflows/build.yml (example)
name: Release Build
on:
  push:
    tags: [ 'v*' ]
jobs:
  build:
    strategy:
      matrix:
        platform: [macos-latest, ubuntu-latest, windows-latest]
    runs-on: ${{ matrix.platform }}
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-action@stable
      - uses: actions/setup-node@v4
        with:
          node-version: 20
      - run: cd crates/ebidownload-gui && npm install && npm run tauri build
      - uses: actions/upload-artifact@v4
        with:
          name: bundle-${{ matrix.platform }}
          path: crates/ebidownload-gui/src-tauri/target/release/bundle/*
```

---

## 4. Configuration File

This program uses a YAML file to configure the paths for external tools, including `sra-tools` (`prefetch`, `fasterq-dump`).

**CLI default location**: `EBIDownload.yaml` beside the `EBIDownload` executable. Use the global `-y, --yaml <FILE>` option to choose another path. If the default file is absent, the CLI reports the exact path it attempted to load.

`sra-tools` paths are optional: if they are not present in the YAML file, EBIDownload falls back to a managed installation (created by `EBIDownload deps install` or the GUI's startup install dialog) and finally to executables found in your `PATH`.

You can **manually create** this file if you want to use your own installations.

Below is the standard format for the `EBIDownload.yaml` file:

```yaml
# EBIDownload Setting yaml
software:
  prefetch: /path/to/your/prefetch
  fasterq_dump: /path/to/your/fasterq-dump

public_data:
  ncbi_nt:
    s3_url: s3://ncbi-blast-databases/2026-07-10-12-55-02/
    description: "NCBI nt database"
    database_type: folder
    exclude: "*"
    include: "nt.*"

  k2_viral:
    s3_url: s3://genome-idx/kraken/k2_viral_20240112.tar.gz
    description: "Kraken2 viral database"
    database_type: file
```

**Important Notes**:
- The `software` section must point to the absolute paths of the `prefetch` and `fasterq-dump` executables.
- Ensure all paths are correct, or the program will not run properly in the corresponding download mode.

---

## 5. Usage

### GUI (Desktop Application)

The GUI provides an intuitive interface for users who prefer visual operation over command-line tools.

![EBIDownload GUI Tabs](./docs/GUI.png)

```bash
cd crates/ebidownload-gui
npm run tauri dev
```

**Interface Overview:**

| Tab | Function |
|-----|----------|
| **Download** | Enter Accession ID, select output directory, choose download method (AWS/FTP/Prefetch/Auto), set parallel threads, and start downloading. Supports fetching metadata preview before download. |
| **Upload** | Select files, enter S3 bucket name, configure upload settings, and submit to NCBI SRA via AWS S3. Shows real per-file upload progress and forwards core logs to the live log panel. |
| **Settings** | Visually configure paths for `prefetch`, `fasterq-dump`, and other software executables. |
| **About** | Software information, version, a "View on GitHub" link to the [project repository](https://github.com/xsx123123/EBIDownload), and a reflection on the atoms that make us all. |

A circular **theme toggle button** in the top-right corner of the header switches between dark and light modes. Your preference is stored locally and restored on the next launch.

**Features:**
- Automatic dependency detection: checks for `sra-tools` on startup and offers one-click installation if missing
- Real-time download progress bars for each run, with live `MB/s` speed in the Status column
- Smooth Overall Progress based on the average completion across all runs
- Pause / Stop controls for AWS downloads
- Live log panel showing download/conversion/compression/upload status, including logs emitted from the Rust core
- Logs mirrored to `output/EBIDownload.log` during each download run
- Auto-collapse configuration cards and auto-expand progress panel when a download starts
- Dry-run mode to preview what would be downloaded
- Support for TSV file input (batch download)

---

### CLI (Command-Line Tool)

#### a. Command-Line Arguments

```
./target/release/EBIDownload download -h
```

| Short | Long             | Description                                      | Default      |
|-------|------------------|--------------------------------------------------|--------------|
| `-A`  | `--accession`    | Download by project Accession ID                 |              |
| `-T`  | `--tsv`          | Download using a TSV file containing Accession IDs |              |
| `-o`  | `--output`       | **Required**, the output directory for downloaded files |              |
| `-p`  | `--multithreads` | Number of files to download in parallel          | 4            |
| `-d`  | `--download`     | Download method (`aws`, `ftp`, `prefetch`, `auto`) | `aws`        |
| `-y`  | `--yaml`         | Specify the path to the `EBIDownload.yaml` config file | `EBIDownload.yaml` |
|       | `--log-level`    | Log level (`debug`, `info`, `warn`, `error`)     | `info`       |
|       | `--log-format`   | Log output format (`text`, `json`)               | `text`       |
| `-t`  | `--aws-threads`  | **AWS/Prefetch**: Threads for internal chunk download or conversion per file | 8            |
|       | `--chunk-size`   | **AWS Only**: Chunk size in MB                   | 20           |
|       | `--max-size`     | **Prefetch Only**: Max download size limit (e.g., `100G`) | `100G`       |
|       | `--pe-only`      | Only download Paired-End data, ignore Single-End | `false`      |
|       | `--filter-sample`| Regex pattern to include samples matching this   |              |
|       | `--filter-run`   | Regex pattern to include runs matching this      |              |
|       | `--exclude-sample`| Regex pattern to exclude samples matching this   |              |
|       | `--exclude-run`  | Regex pattern to exclude runs matching this      |              |
|       | `--cleanup-sra`  | Remove intermediate .sra files after conversion | `false`      |
|       | `--dry-run`      | Show what would be downloaded without actually downloading | `false` |
|       | `--progress-port`| Enable HTTP progress API on this port (AES-256-GCM encrypted) | — |
|       | `--write-progress-key` | Write encryption key to `progress.key` in output directory (default: not written) | `false` |
| `-h`  | `--help`         | Print help information                           |              |
| `-V`  | `--version`      | Print version information                        |              |

**Note**: The `-A` and `-T` options are typically mutually exclusive and are used to specify the data source to download.

#### b. Public Reference Data from S3

`public-data` reads the `public_data` map from `EBIDownload.yaml`. You must select exactly one YAML identifier with `--name`; running `public-data` without arguments prints help and never downloads every configured entry.

```bash
# Download the YAML entry named ncbi_nt
./target/release/EBIDownload public-data --name ncbi_nt --output ./dbs

# Override the default YAML location
./target/release/EBIDownload public-data -y /path/to/databases.yaml --name k2_viral --output ./dbs

# Preview matching objects without downloading data
./target/release/EBIDownload public-data --name ncbi_nt --dry-run

# Tune file concurrency, per-file HTTP ranges, and range size (MiB)
./target/release/EBIDownload public-data --name k2_viral -p 4 -t 2 --chunk-size 32 --output ./dbs
```

| Option | Description | Default |
|--------|-------------|---------|
| `-n`, `--name` | Required key in the YAML `public_data` map, such as `ncbi_nt` | — |
| `-o`, `--output` | Directory for downloaded database files | `.` |
| `-p`, `--multithreads` | Concurrent files for folder sources | `8` |
| `-t`, `--aws-threads` | Concurrent HTTP range requests per file | `4` |
| `--chunk-size` | HTTP range size in MiB | `64` |
| `--dry-run` | List matching objects and sizes without downloading | `false` |
| `-y`, `--yaml` | Global override for the YAML configuration path | Executable directory |

Folder sources use `exclude` first and then `include` as an override. For example, `exclude: "*"` together with `include: "nt.*"` downloads only `nt.*` objects below the configured S3 prefix. Plain 32-hex S3 ETags are verified as MD5 values; other object types retain size and completed-range checks. Each completed database also writes `<name>.md5`, an `md5sum`-compatible manifest containing every downloaded file. Each object uses its own `.meta.json` file for resumable transfer; it is removed when the download completes.

#### c. Download Examples

**1. AWS S3 High-Speed Mode (Most Recommended)**

This mode uses AWS S3 buckets for global acceleration, similar to IDM. It is the best choice for large-scale data acquisition.

```bash
# Download using AWS S3 with 8 threads per file, processing 4 files in parallel
./target/release/EBIDownload download -A PRJNA1251654 -o ./data -d aws -p 4 -t 8
```

**2. Filtering Mode**

You can use `--filter-run` or `--filter-sample` to download specific data.

```bash
# Download a specific Run from a project
./target/release/EBIDownload download -A PRJNA833659 -o ./ -p 6 -d aws -y ./EBIDownload.yaml --chunk-size 200 --filter-run SRR19019104

# Download multiple specified Runs (separated by spaces)
./target/release/EBIDownload download -A PRJNA833659 -o ./ -p 6 -d aws --filter-run SRR19019104 SRR19019105

# Download a list of specific Runs from a project (useful for targeted re-analysis)
./target/release/EBIDownload download -A PRJNA259308 -o ./ -p 6 -d aws \
  -y ./EBIDownload.yaml \
  --chunk-size 200 \
  --filter-run SRR1572540 SRR1572541 SRR1572542 
```

**3. Standard Mode (Prefetch)**

The following example demonstrates how to download data for project `PRJNA1251654`, using 6 threads, and saving the files to the current directory.

```bash
# Make sure you have activated the conda environment and the config file is set up correctly
# conda activate EBIDownload_env

# Example command:
./target/release/EBIDownload download -A PRJNA1251654 -o ./ --multithreads 6 --yaml ./EBIDownload.yaml -d prefetch
```

---

## Important Notes on AWS S3 High-Speed Download Mode

This tool leverages the AWS S3 open data pool (`s3://sra-pub-run-odp/`) for high-speed downloads, which is only applicable to data that has already been archived in SRA. Due to the inherent timing of NCBI data processing workflows, the following important limitations apply:

1. **Data Availability Delay**
   GEO metadata release (obtaining GSE/GSM IDs) ≠ SRA data availability.
   Raw sequencing data must undergo quality control, format conversion, and indexing before it is transferred from GEO to SRA and synchronized to AWS S3. This process usually takes **1–4 weeks**.
   During this period, even if the GEO page is public, the S3 path may not yet exist, and the tool will return a 404 error.

2. **How to Check if Data is Ready**
   Before downloading, please confirm:
   - The GEO page shows an "SRA Run Selector" link (rather than "Data coming soon").
   - Or verify via the command line: `esearch -db sra -query "GSEXXXXXX" | efetch -format runinfo` returns a list of SRR accessions.

3. **Alternatives for Data Not Yet in SRA**
   If the data has not yet entered SRA:
   - **Use SRA Toolkit**: Submit a download request with the `prefetch` command; the system will automatically fetch the data once it becomes available (may require queuing).
   - **Contact the original authors**: The GEO page provides contact information for the corresponding authors, who can usually provide a direct download link (FTP, Google Drive, etc.) within 24–48 hours.
   - **Check the ENA mirror**: The European Nucleotide Archive (ENA) is sometimes available 1–3 days earlier than SRA. Try `ftp://ftp.sra.ebi.ac.uk/`.

4. **Recommended Download Strategy**
   We recommend implementing a tiered download logic: first check SRA availability; if ready, use this tool for high-speed S3 downloads; if not ready, automatically fall back to SRA Toolkit or prompt the user to wait/contact the authors.

> **Note**: This limitation stems from the NCBI data archiving architecture, not a technical defect of this tool. For urgent needs, we recommend contacting the data submitter to obtain the original files directly.

---
## 6. Output Structure

After the script runs, the output directory will contain the following files and directories:

```
.
├── EBIDownload_{ACCESSION}_YYYY-MM-DD_HH-MM-SS.log
├── EBIDownload.log                         (GUI per-run log, when using the GUI)
├── ena_metadata_{ACCESSION}.tsv
├── R1_fastq_md5_{ACCESSION}.tsv
├── R2_fastq_md5_{ACCESSION}.tsv
├── SRRXXXXXX/
│   └── ... (downloaded files)
└── ...
```

- **Log File**: `EBIDownload_{ACCESSION}_YYYY-MM-DD_HH-MM-SS.log`
  - Records the detailed execution log of the script, with the Accession ID in the filename for easy identification.

- **GUI Log File**: `EBIDownload.log`
  - Created in the selected output directory when a GUI download starts. It mirrors the same logs shown in the live log panel and is overwritten on each new download.

- **Metadata File**: `ena_metadata_{ACCESSION}.tsv`
  - Contains all fetched and filtered metadata from the EBI API, with a header comment indicating the source project.

- **MD5 Checksum Files**: `R1_fastq_md5_{ACCESSION}.tsv` and `R2_fastq_md5_{ACCESSION}.tsv`
  - These files contain the official MD5 checksums and sample names retrieved from the EBI database for the downloaded FASTQ files (R1 and R2 reads, respectively). You can use these files to verify the integrity of your downloaded data.

- **Sample Directories**: `SRRXXXXXX/`
  - Each directory corresponds to a downloaded sample (Run ID) and contains the actual sequencing data files.

---

## 7. Upload to NCBI SRA via AWS S3

> **⚠️ Experimental**: The upload subcommand / GUI tab is still under testing. Please verify your uploads and use with caution.

In addition to downloading, EBIDownload supports **uploading sequencing data to AWS S3** for fast NCBI SRA submission. This is useful when you need to submit large volumes of data (hundreds of GB to TB scale) and want to leverage AWS's enterprise-grade bandwidth for reliable, high-speed uploads.

### a. Prerequisites

- **Your own AWS S3 Bucket**: You must create an S3 bucket **in the `us-east-1` (US East - N. Virginia) region**. This is a [hard requirement from NCBI](https://www.ncbi.nlm.nih.gov/sra/docs/data-delivery) — buckets in other regions will not work.
- **AWS Credentials**: Configure your AWS credentials via `aws configure` or environment variables (`AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`). These are used locally and are **never shared with NCBI**.

### b. How It Works

The S3-based SRA submission uses a **read-only permission model** — you don't give NCBI any credentials:

1. **Upload files** to your S3 bucket using your own AWS key (handled by `EBIDownload upload`)
2. **Apply Bucket Policy** to authorize NCBI's IAM user (`arn:aws:iam::228184908524:user/SA-SubmissionPortal-S3`) with read-only access (handled by `--apply-policy`)
3. **Submit on the SRA Portal** ([https://submit.ncbi.nlm.nih.gov/subs/sra/](https://submit.ncbi.nlm.nih.gov/subs/sra/)), select "Upload from Amazon S3 storage" and provide your S3 paths

```
You (Bucket Owner)                    NCBI SRA Portal
       │                                    │
       │  1. Upload files (your AWS key)     │
       │  ──────────────────► S3 Bucket      │
       │                                    │
       │  2. Add Bucket Policy               │
       │     (read-only for NCBI IAM user)   │
       │  ──────────────────► S3 Bucket      │
       │                                    │
       │  3. Submit S3 paths on Portal       │
       │  ──────────────────────────────────►│
       │                                    │
       │              NCBI reads files       │
       │              (their own IAM key)    ├────► S3 Bucket (read-only)
```

### c. Cost

| Item | Cost |
|------|------|
| S3 Storage | ~$0.023/GB/month |
| Upload traffic (into AWS) | **Free** |
| NCBI reading traffic (same region) | **Free** |

The actual cost is **storage only**. For example, 100 GB of data stored for 2 weeks costs less than **$1**. Once SRA confirms your submission has been processed, you can **delete the bucket** to stop all charges. AWS Free Tier also includes 5 GB of S3 storage for the first 12 months.

### d. Usage

```bash
# Basic upload to S3
EBIDownload upload -b my-sra-bucket -f sample_R1.fastq.gz sample_R2.fastq.gz

# Upload with NCBI Bucket Policy + metadata template generation
EBIDownload upload -b my-sra-bucket \
    -f sample_R1.fastq.gz sample_R2.fastq.gz \
    --apply-policy \
    --metadata-template sra_metadata.tsv

# Dry run: preview files without uploading
EBIDownload upload -b my-sra-bucket -f *.fastq.gz --dry-run

# Upload with S3 key prefix (subdirectory)
EBIDownload upload -b my-sra-bucket --prefix project_001 -f *.fastq.gz
```

| Option | Description | Default |
|--------|-------------|---------|
| `-b`, `--bucket` | **Required**, AWS S3 bucket name | — |
| `--prefix` | S3 key prefix (subdirectory) | — |
| `-f`, `--files` | Files to upload | — |
| `--region` | AWS region (must be `us-east-1` for NCBI) | `us-east-1` |
| `-c`, `--concurrent` | Concurrent file uploads | 4 |
| `--apply-policy` | Apply NCBI SRA submission bucket policy | `false` |
| `--metadata-template` | Generate SRA metadata template TSV | — |
| `--dry-run` | Show what would be uploaded without uploading | `false` |

### e. When to Use S3 Upload vs. Alternatives

| Method | Cost | Speed | Best For |
|--------|------|-------|----------|
| **S3 Upload** (`EBIDownload upload`) | ~$0.023/GB/month | Fastest, most reliable | Large datasets (100 GB+), unstable networks |
| **NCBI Web Upload** | Free | Slow, unreliable for large files | Small datasets (< 10 GB) |

> **Tip**: If your data is small, use the free NCBI Web Upload. S3 upload is a "pay a little for speed and reliability" option — ideal when you have hundreds of GB to submit and want enterprise-grade bandwidth with resumable transfers.

### f. Complete Workflow Example

```bash
# Step 1: Create an S3 bucket in us-east-1 (one-time setup)
aws s3 mb s3://my-sra-bucket --region us-east-1

# Step 2: Upload files + apply NCBI policy + generate metadata template
EBIDownload upload -b my-sra-bucket \
    -f sample1_R1.fastq.gz sample1_R2.fastq.gz \
       sample2_R1.fastq.gz sample2_R2.fastq.gz \
    --apply-policy \
    --metadata-template sra_metadata.tsv \
    --region us-east-1

# Step 3: Fill in the empty columns in sra_metadata.tsv
#   (library_strategy, library_source, platform, instrument_model, etc.)

# Step 4: Go to https://submit.ncbi.nlm.nih.gov/subs/sra/
#   - Create a new submission
#   - At the "Files" step, select "Upload from Amazon S3 storage"
#   - Enter your S3 paths: s3://my-sra-bucket/sample1_R1.fastq.gz etc.

# Step 5: Wait for SRA confirmation email, then delete the bucket
aws s3 rb s3://my-sra-bucket --force
```

---

## 8. HTTP Progress API (Encrypted)

EBIDownload CLI provides an optional **HTTP Progress API** that allows external platforms to query real-time download progress. The progress data is encrypted with **AES-256-GCM** to ensure security.

### a. Overview

When enabled via `--progress-port`, EBIDownload starts an HTTP server that serves encrypted progress data. The encryption key is:
- **Generated at compile time** (via `EBIDOWNLOAD_PROGRESS_KEY` env var) and embedded in the binary
- **NOT written to disk by default** — add `--write-progress-key` to write it to `progress.key` in the output directory
- **Never exposed via HTTP** — no HTTP endpoint returns the key

The platform can obtain the key in two ways:
1. **Known at compile time**: If your platform set `EBIDOWNLOAD_PROGRESS_KEY` before compilation, it already knows the key — no file needed
2. **Read from file**: Use `--write-progress-key` at runtime to write the key to `progress.key`, then read it from the output directory

### b. Compile with Custom Key

You can set a custom encryption key at compile time using the `EBIDOWNLOAD_PROGRESS_KEY` environment variable:

```bash
# Generate a random 32-character key (or use your own)
export EBIDOWNLOAD_PROGRESS_KEY=$(openssl rand -hex 16)
echo "Your key: $EBIDOWNLOAD_PROGRESS_KEY"

# Build with the custom key embedded
CC=clang cargo build -p ebidownload-cli --release

# The key is now embedded in the binary
./target/release/EBIDownload --version
```

If `EBIDOWNLOAD_PROGRESS_KEY` is not set, a deterministic key is derived from the crate version.

### c. Enable Progress API at Runtime

```bash
# Start download with progress API on port 8080
./target/release/EBIDownload download -A PRJNA1251654 -o ./data --progress-port 8080

# If external platforms need to read the key from file, add --write-progress-key
./target/release/EBIDownload download -A PRJNA1251654 -o ./data --progress-port 8080 --write-progress-key
```

By default, the encryption key is **not** written to disk. Add `--write-progress-key` to write it to `./data/progress.key`. This will:
1. Start an HTTP server on `0.0.0.0:8080`
2. (Optional) Write the encryption key to `./data/progress.key`
3. Track progress for all 3 stages: download → extraction → compression

### d. Query Progress from External Platform

The platform reads the key file and queries the HTTP endpoint:

```bash
# 1. Read the encryption key (shared securely, e.g., via mounted volume)
KEY=$(cat ./data/progress.key)

# 2. Query the encrypted progress
RESPONSE=$(curl -s http://localhost:8080/progress)
echo "$RESPONSE"
# {"ciphertext":"...","nonce":"..."}
```

### e. Decrypt Progress Data (Python Example)

```python
import json
import base64
from cryptography.hazmat.primitives.ciphers.aead import AESGCM

# Read the key (32 bytes hex-encoded)
with open("./data/progress.key", "r") as f:
    key = bytes.fromhex(f.read().strip())

# Query the API
import requests
resp = requests.get("http://localhost:8080/progress").json()
ciphertext = base64.b64decode(resp["ciphertext"])
nonce = base64.b64decode(resp["nonce"])

# Decrypt
aesgcm = AESGCM(key)
plaintext = aesgcm.decrypt(nonce, ciphertext, None)
progress = json.loads(plaintext)

# Example output
for run_id, data in progress.items():
    print(f"{run_id}: {data['stage']} ({data['overall_percent']:.1f}%)")
    print(f"  Download:    {data['download']['percent']:.1f}%")
    print(f"  Extraction:  {data['extraction']['percent']:.1f}%")
    print(f"  Compression: {data['compression']['percent']:.1f}%")
```

### f. Progress Data Structure

The decrypted JSON contains per-run progress with 3-stage weighted tracking:

```json
{
  "SRR12345678": {
    "run_id": "SRR12345678",
    "stage": "compressing",
    "overall_percent": 75.5,
    "download": {
      "bytes_done": 1073741824,
      "bytes_total": 1073741824,
      "weight": 1073741824,
      "percent": 100.0
    },
    "extraction": {
      "bytes_done": 3221225472,
      "bytes_total": 3221225472,
      "weight": 3221225472,
      "percent": 100.0
    },
    "compression": {
      "bytes_done": 1610612736,
      "bytes_total": 3221225472,
      "weight": 3221225472,
      "percent": 50.0
    }
  }
}
```

**Stage values**: `pending` → `downloading` → `extracting` → `compressing` → `completed` (or `failed`)

### g. Security Considerations

| Aspect | Implementation |
|--------|----------------|
| **Encryption** | AES-256-GCM (authenticated encryption) |
| **Key storage** | Embedded in binary at compile time |
| **Key file** | NOT written by default; opt-in via `--write-progress-key` |
| **Key distribution** | Platform uses compile-time key, or reads `progress.key` if written |
| **HTTP exposure** | Key is never exposed via HTTP |
| **Nonce** | Random 12-byte nonce per request (prevents replay attacks) |

> **Warning**: The key file (`progress.key`) must be shared securely with the consuming platform. Treat it like a password — anyone with the key can decrypt the progress data.

### h. Complete Workflow Example

```bash
# 1. Compile with custom key
export EBIDOWNLOAD_PROGRESS_KEY="my-secret-key-1234567890abcdef"
CC=clang cargo build -p ebidownload-cli --release

# 2. Start download with progress API (key NOT written to disk by default)
./target/release/EBIDownload download -A PRJNA1251654 -o ./data --progress-port 8080

# 3. If platform needs to read key from file, use --write-progress-key
./target/release/EBIDownload download -A PRJNA1251654 -o ./data --progress-port 8080 --write-progress-key

# 4. In another terminal / platform, query progress
#    (platform already knows the key from compile time, or reads progress.key)
curl -s http://localhost:8080/progress | jq .

# 5. Decrypt using Python (see example above)
python3 decrypt_progress.py
```

### i. md5.txt Output

After all downloads complete, EBIDownload automatically generates `md5.txt` in the output directory:

```bash
$ cat ./data/md5.txt
a1b2c3d4e5f6...  SRR12345678_1.fastq.gz
f6e5d4c3b2a1...  SRR12345678_2.fastq.gz
```

This file uses the standard **md5sum format** and can be verified with:

```bash
cd ./data
md5sum -c md5.txt
# SRR12345678_1.fastq.gz: OK
# SRR12345678_2.fastq.gz: OK
```

---

**Author**: JZHANG | **Version**: v1.4.1

## 🔗 Links
- GitHub: [repository](https://github.com/xsx123123/EBIDownload)
- LINUX DO: [Announcement](https://linux.do/) (Original)
