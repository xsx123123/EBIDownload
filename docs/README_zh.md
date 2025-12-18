# EBIDownload

EBIDownload 是一个基于 Rust 开发的命令行工具，用于高效地从欧洲生物信息学研究所 (EBI) FTP 服务器和 NCBI SRA 数据库下载测序数据。本工具集成 **AWS S3 全球加速**与 [IBM Aspera CLI](https://www.ibm.com/aspera/connect/)，可实现媲美 IDM/Aspera 的极速下载。**它支持 24 小时从 SRA 数据库下载 2TB 数据到本地，并提供完善的断点续传与 MD5 完整性校验**。同时利用 [pigz](https://zlib.net/pigz/) 进行并行解压缩，显著提升了数据获取与处理的效率。

![EBIDownload](./download.gif)

## 主要特性

- **AWS S3 全球加速 (个人最推荐)**: 直接从 NCBI SRA 的 AWS S3 存储桶进行多线程下载，充分利用带宽，实现全球范围的高速访问。这是目前获取大规模数据最快、最稳定的方式。
- **极速下载**: 集成 Aspera CLI，突破传统 FTP/HTTP 限速，提供顶级下载性能。
- **并行处理**: 支持文件级与分片级的多线程下载及并行解压缩。
- **易于配置**: 通过简单的 YAML 文件管理软件路径和 Aspera 密钥。
- **灵活使用**: 支持通过项目登录号 (Accession) 或 TSV 文件直接下载。
- **断点续传**: 在 `aws`, `ascp` 和 `prefetch` 下载模式下均支持断点续传，保障大文件下载的连续性。

---

## 1. 安装与环境准备

在运行此程序之前，请确保你已经完成了以下环境的配置。

### a. Conda 环境

本项目依赖于 `sra-tools` (提供 `prefetch` 和 `fasterq-dump`) 和 `aspera-cli`。我们推荐使用 Conda 来创建一个隔离的运行环境。

```bash
# 使用项目提供的 .yaml 文件创建并激活 conda 环境
conda env create -f EBIDownload_env.yaml
conda activate EBIDownload_env
```

### b. 安装 pigz

`pigz` 是一个支持多线程的 `gzip` 实现，可以显著加快文件解压速度。

- **对于 Ubuntu/Debian 系统:**
  ```bash
  sudo apt-get update
  sudo apt-get install pigz
  ```

- **对于 macOS 系统 (使用 Homebrew):**
  ```bash
  brew install pigz
  ```

---

## 2. 编译程序

本项目使用 Rust 编写，你需要先安装 [Rust 环境](https://www.rust-lang.org/tools/install)。

```bash
# 克隆仓库
# git clone git@github.com:xsx123123/EBIDownload.git
# cd EBIDownload

# 编译开发版 (较快, 用于调试)
CC=clang cargo build

# 编译发行版 (优化性能, 用于生产)
CC=clang cargo build --release
```

编译后的可执行文件位于 `target/release/EBIDownload`。

---

## 3. 配置文件

本程序通过一个 YAML 文件 (默认为 `EBIDownload.yaml`) 来配置所需软件的路径和 Aspera 的密钥。

你需要**手动创建**此文件，并根据你的系统环境，填入正确的绝对路径。

以下是 `EBIDownload.yaml` 文件的标准格式:

```yaml
# EBIDownload Setting yaml
software:
  ascp: /path/to/your/ascp
  prefetch: /path/to/your/prefetch
  fasterq_dump: /path/to/your/fasterq-dump
setting:
  openssh: /path/to/your/asperaweb_id_dsa.openssh
```

**重要提示**:
- `software` 部分需要指向 `ascp`, `prefetch`, 和 `fasterq-dump` 这三个可执行文件的绝对路径。
- `setting` 部分的 `openssh` 需要指向 Aspera Connect 提供的密钥文件 (`asperaweb_id_dsa.openssh`) 的绝对路径。
- 请确保所有路径都是准确的，否则程序将无法正常运行。

---

## 4. 使用方法

### a. 命令行参数

```
Download EMBL-ENA sequencing data

Usage: EBIDownload [OPTIONS] --output <OUTPUT>
```

| 短参数 | 长参数             | 描述                                     | 默认值      |
|--------|--------------------|------------------------------------------|-------------|
| `-A`   | `--accession`      | 按项目登录号 (Accession ID) 下载          |             |
| `-T`   | `--tsv`            | 按包含登录号的 TSV 文件下载              |             |
| `-o`  | `--output`       | **必需**, 下载文件的输出目录             |             |
| `-p`  | `--multithreads` | 文件级并发数：同时下载的文件数量            | 4            |
| `-d`  | `--download`     | 下载方式 (`aws`, `ascp`, `ftp`, `prefetch`) | `prefetch`   |
| `-O`  | `--only-scripts` | 仅生成下载脚本，不执行下载               |              |
| `-y`  | `--yaml`         | 指定 `EBIDownload.yaml` 配置文件路径     | `EBIDownload.yaml` |
|       | `--log-level`    | 日志级别 (debug, info, warn, error)      | `info`       |
| `-t`  | `--aws-threads`  | **AWS 专用**: 单文件内部分片下载线程数     | 8            |
|       | `--chunk-size`   | **AWS 专用**: 分片大小 (MB)              | 20           |
|       | `--pe-only`      | 仅下载双端测序(Paired-End)数据，忽略单端数据 |              |
|        | `--filter-sample`  | 仅下载匹配该ID的样本 (sample)            |             |
|        | `--filter-run`     | 仅下载匹配该ID的运行 (run)               |             |
|        | `--exclude-sample` | 排除匹配该ID的样本 (sample)              |             |
|        | `--exclude-run`    | 排除匹配该ID的运行 (run)                 |             |
| `-h`   | `--help`           | 打印帮助信息                             |             |
| `-V`   | `--version`        | 打印版本信息                             |             |

### b. 使用示例

**1. AWS S3 高速模式 (个人最推荐)**

该模式利用 AWS S3 存储桶实现全球加速，下载速度极快，是进行大规模数据获取的首选方案。

```bash
# 使用 AWS S3 模式下载，每个文件开启 8 线程分片下载，同时下载 4 个文件
./target/release/EBIDownload -A PRJNA1251654 -o ./data -d aws -p 4 -t 8
```

**2. 标准模式 (Prefetch)**

```bash
# 示例命令:
./target/release/EBIDownload -A PRJNA1251654 -o ./ --multithreads 6 --yaml ./EBIDownload.yaml
```

**3. 备选方案：Python 脚本 (仅限 AWS)**

如果你更倾向于使用 Python，我们在 `python/` 目录下也提供了一个基于 `boto3` 开发的下载脚本，同样可以实现极速下载。

```bash
# Python 备选方案使用示例
python python/sra_downloader_aws_v2.py -A PRJNA1251654 -o ./data
```

---
## 5. 输出文件结构

```
.
├── EBIDownload_EMBI-ENA_Download_YYYY-MM-DD_HH-MM-SS.log
├── R1_fastq_md5.tsv
├── R2_fastq_md5.tsv
├── SRRXXXXXX/
│   └── ... (下载的数据文件)
└── ...
```
