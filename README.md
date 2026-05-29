
[中文文档](./docs/README_zh.md) | English

# EBIDownload

EBIDownload is a command-line tool developed in Rust for efficiently downloading sequencing data from the European Bioinformatics Institute (EBI) FTP server and NCBI SRA. By default, it utilizes **AWS S3 global acceleration** to achieve ultra-fast download speeds (comparable to IDM/Aspera). **It is capable of downloading 2TB of data from the SRA database to local storage within 24 hours**, while providing full support for **resumable downloads** and **MD5 integrity verification**. It also employs [pigz](https://zlib.net/pigz/) for parallel decompression, significantly improving data acquisition efficiency.

In addition, EBIDownload supports **Aspera CLI (`ascp`)** as an alternative high-speed download method, which can be selected via the `-d ascp` flag. To use this fallback, you must configure the Aspera path and key in the `EBIDownload.yaml` file (see [Configuration File](#3-configuration-file)).

![EBIDownload](./docs/Download.png)

## Features

- **AWS S3 Acceleration (Most Recommended)**: Direct multi-threaded downloading from NCBI SRA AWS S3 buckets, maximizing bandwidth utilization for global high-speed access. This is the fastest and most reliable method for large-scale data acquisition.
- **Aspera Fallback (Alternative)**: Integrates Aspera CLI (`ascp`) as an alternative high-speed channel when AWS S3 is unavailable or not preferred.
- **Parallel Processing**: Supports multi-threaded downloading and decompression.
- **Easy Configuration**: Manages software paths and keys through a simple YAML file.
- **Flexible Usage**: Supports direct downloads via project accession numbers.
- **Resumable Downloads**: Supports resumable downloads in `aws`, `ascp` and `prefetch` modes, ensuring download continuity.
- **Smart Auto-Fallback**: Automatically attempts AWS S3 first and seamlessly switches to Prefetch if the AWS download fails (Mode: `auto`).
- **Advanced Filtering**: Supports Regex-based filtering to precisely include or exclude specific samples or runs.

---

## 1. Prerequisites and Setup

**Why are external dependencies required?**
Since the raw data downloaded from NCBI/EBI is typically in `.sra` format, it must be converted to standard `.fastq` format and compressed. Because there are currently no mature native Rust libraries available for parsing `.sra` files, this tool relies on external third-party command-line tools to handle these steps. Therefore, you must install the following dependencies before using the tool:

- **`sra-tools` (`prefetch` / `fasterq-dump`)**: Required. `prefetch` is used to download SRA data via standard NCBI protocols. `fasterq-dump` is used to extract and convert `.sra` files into `.fastq` format.
- **`pigz`**: Required. A parallel implementation of gzip used to quickly compress massive `.fastq` files into `.fastq.gz`, significantly saving storage space.
- **`aspera-cli` (`ascp`)**: Optional. A high-speed data transfer client by IBM, used as an alternative to traditional FTP/HTTP downloads.

### a. Conda Environment

We recommend using Conda to create an isolated runtime environment to easily install `sra-tools` and `aspera-cli`.

```bash
# Create and activate the conda environment using the provided .yaml file
conda env create -f ./docs/EBIDownload_env.yaml
conda activate EBIDownload_env
```

### b. Install pigz

`pigz` is a parallel implementation of `gzip` that can significantly speed up file compression.

- **For Ubuntu/Debian systems:**
  ```bash
  sudo apt-get update
  sudo apt-get install pigz
  ```

- **For macOS systems (using Homebrew):**
  ```bash
  brew install pigz
  ```

---

## 2. Building the Program

This project is written in Rust. You need to install the [Rust environment](https://www.rust-lang.org/tools/install) first.

```bash
# Clone the repository
# git clone git@github.com:xsx123123/EBIDownload.git
# cd EBIDownload

# Build for development (faster, for debugging)
CC=clang cargo build

# Build for release (optimized for performance, for production)
CC=clang cargo build --release
```

The compiled executable will be located at `target/release/EBIDownload`.

---

## 3. Configuration File

This program uses a YAML file (defaulting to `EBIDownload.yaml`) to configure the paths for external tools, including the Aspera CLI (`ascp`) and `sra-tools` (`prefetch`, `fasterq-dump`).

You need to **manually create** this file and fill in the correct absolute paths according to your system environment. Even if you primarily use the default **AWS S3** mode, it is recommended to configure the Aspera path and key so that the `ascp` fallback is ready when needed.

Below is the standard format for the `EBIDownload.yaml` file:

```yaml
# EBIDownload Setting yaml
software:
  ascp: /path/to/your/ascp
  prefetch: /path/to/your/prefetch
  fasterq_dump: /path/to/your/fasterq-dump
setting:
  openssh: /path/to/your/asperaweb_id_dsa.openssh
```

**Important Notes**:
- The `software` section must point to the absolute paths of the `ascp`, `prefetch`, and `fasterq-dump` executables.
- The `openssh` key in the `setting` section must point to the absolute path of the key file provided by Aspera Connect (`asperaweb_id_dsa.openssh`). This is required when using the Aspera (`ascp`) download mode.
- Ensure all paths are correct, or the program will not run properly in the corresponding download mode.

---

## 4. Usage

### a. Command-Line Arguments

According to the program's help information, the correct usage is as follows:

```
Download EMBL-ENA sequencing data

Usage: EBIDownload [OPTIONS] --output <OUTPUT>
```

| Short | Long             | Description                                      | Default      |
|-------|------------------|--------------------------------------------------|--------------|
| `-A`  | `--accession`    | Download by project Accession ID                 |              |
| `-T`  | `--tsv`          | Download using a TSV file containing Accession IDs |              |
| `-o`  | `--output`       | **Required**, the output directory for downloaded files |              |
| `-p`  | `--multithreads` | Number of files to download in parallel          | 4            |
| `-d`  | `--download`     | Download method (`aws`, `ascp`, `ftp`, `prefetch`, `auto`) | `aws`        |
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
| `-h`  | `--help`         | Print help information                           |              |
| `-V`  | `--version`      | Print version information                        |              |

**Note**: The `-A` and `-T` options are typically mutually exclusive and are used to specify the data source to download.

### b. Example

**1. AWS S3 High-Speed Mode (Most Recommended)**

This mode uses AWS S3 buckets for global acceleration, similar to IDM. It is the best choice for large-scale data acquisition.

```bash
# Download using AWS S3 with 8 threads per file, processing 4 files in parallel
./target/release/EBIDownload -A PRJNA1251654 -o ./data -d aws -p 4 -t 8
```

**2. Filtering Mode**

You can use `--filter-run` or `--filter-sample` to download specific data.

```bash
# Download a specific Run from a project
./target/release/EBIDownload -A PRJNA833659 -o ./ -p 6 -d aws -y ./EBIDownload.yaml --chunk-size 200 --filter-run SRR19019104

# Download multiple specified Runs (separated by spaces)
./target/release/EBIDownload -A PRJNA833659 -o ./ -p 6 -d aws --filter-run SRR19019104 SRR19019105

# Download a list of specific Runs from a project (useful for targeted re-analysis)
./target/release/EBIDownload -A PRJNA259308 -o ./ -p 6 -d aws \
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
./target/release/EBIDownload -A PRJNA1251654 -o ./ --multithreads 6 --yaml ./EBIDownload.yaml -d prefetch
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
## 5. Output Structure

After the script runs, the output directory will contain the following files and directories:

```
.
├── EBIDownload_{ACCESSION}_YYYY-MM-DD_HH-MM-SS.log
├── ena_metadata_{ACCESSION}.tsv
├── R1_fastq_md5_{ACCESSION}.tsv
├── R2_fastq_md5_{ACCESSION}.tsv
├── SRRXXXXXX/
│   └── ... (downloaded files)
└── ...
```

- **Log File**: `EBIDownload_{ACCESSION}_YYYY-MM-DD_HH-MM-SS.log`
  - Records the detailed execution log of the script, with the Accession ID in the filename for easy identification.

- **Metadata File**: `ena_metadata_{ACCESSION}.tsv`
  - Contains all fetched and filtered metadata from the EBI API, with a header comment indicating the source project.

- **MD5 Checksum Files**: `R1_fastq_md5_{ACCESSION}.tsv` and `R2_fastq_md5_{ACCESSION}.tsv`
  - These files contain the official MD5 checksums and sample names retrieved from the EBI database for the downloaded FASTQ files (R1 and R2 reads, respectively). You can use these files to verify the integrity of your downloaded data.

- **Sample Directories**: `SRRXXXXXX/`
  - Each directory corresponds to a downloaded sample (Run ID) and contains the actual sequencing data files.
