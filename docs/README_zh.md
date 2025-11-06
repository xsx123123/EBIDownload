# EBIDownload

EBIDownload 是一个基于Rust开发的命令行工具, 用于高效地从欧洲生物信息学研究所 (EBI) 的FTP服务器下载测序数据。本工具通过调用 [IBM Aspera CLI](https://www.ibm.com/aspera/connect/) 实现高速下载, 并利用 [pigz](https://zlib.net/pigz/) 进行并行解压缩, 大大提升了数据获取的效率。

## 主要特性

- **高速下载**: 集成 Aspera CLI, 突破传统FTP/HTTP限速。
- **并行处理**: 支持多线程下载和解压。
- **易于配置**: 通过简单的YAML文件管理Aspera路径和密钥。
- **灵活使用**: 支持通过项目登录号 (Accession) 直接下载。

---

## 1. 安装与环境准备

在运行此程序之前, 请确保你已经完成了以下环境的配置。

### a. Conda 环境

本项目依赖于特定版本的 `aspera-cli`。我们推荐使用 Conda 来创建一个隔离的运行环境。

```bash
# 使用项目提供的 .yaml 文件创建并激活 conda 环境
conda env create -f EBIDownload_env.yaml
conda activate EBIDownload_env
```

### b. 安装 pigz

`pigz` 是一个支持多线程的 `gzip` 实现, 可以显著加快文件解压速度。

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

本项目使用 Rust 编写, 你需要先安装 [Rust 环境](https://www.rust-lang.org/tools/install)。

```bash
# 克隆仓库
# git clone <your-repo-url>
# cd EBIDownload

# 编译开发版 (较快, 用于调试)
cargo build

# 编译发行版 (优化性能, 用于生产)
cargo build --release
```

编译后的可执行文件位于 `target/release/EBIDownload`。

---

## 3. 配置文件

本程序通过一个 YAML 文件 (默认为 `EBIDownload.yaml`) 来配置所需软件的路径和 Aspera 的密钥。

你需要**手动创建**此文件,并根据你的系统环境,填入正确的绝对路径。

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
- 请确保所有路径都是准确的,否则程序将无法正常运行。

---

## 4. 使用方法

### a. 命令行参数

根据程序帮助信息, 正确的用法如下:

```
Download EMBL-ENA sequencing data

Usage: EBIDownload [OPTIONS] --output <OUTPUT>
```

| 短参数 | 长参数             | 描述                                     | 默认值      |
|--------|--------------------|------------------------------------------|-------------|
| `-A`   | `--accession`      | 按项目登录号 (Accession ID) 下载          |             |
| `-T`   | `--tsv`            | 按包含登录号的 TSV 文件下载              |             |
| `-o`   | `--output`         | **必需**, 下载文件的输出目录             |             |
| `-p`   | `--multithreads`   | 下载和处理时使用的线程数                 | 4           |
| `-d`   | `--download`       | 下载方式 (`ascp`, `ftp`, `prefetch`)     | `prefetch`  |
| `-O`   | `--only-scripts`   | 仅生成下载脚本, 不执行下载               |             |
| `-y`   | `--yaml`           | 指定 `EBIDownload.yaml` 配置文件路径     | `EBIDownload.yaml` |
|        | `--filter-sample`  | 仅下载匹配该ID的样本 (sample)            |             |
|        | `--filter-run`     | 仅下载匹配该ID的运行 (run)               |             |
|        | `--exclude-sample` | 排除匹配该ID的样本 (sample)              |             |
|        | `--exclude-run`    | 排除匹配该ID的运行 (run)                 |             |
| `-h`   | `--help`           | 打印帮助信息                             |             |
| `-V`   | `--version`        | 打印版本信息                             |             |

**注意**: `-A` 和 `-T` 参数通常是二选一, 用于指定要下载的数据源。

### b. 使用示例

以下示例演示了如何下载项目 `PRJNA1251654` 的数据, 使用6个线程, 并将文件保存到当前目录。

```bash
# 确保你已经激活了 conda 环境, 并且配置文件已正确设置
# conda activate EBIDownload_env

# 示例命令:
./target/release/EBIDownload -A PRJNA1251654 -o ./ --multithreads 6 --yaml ./EBIDownload.yaml
```

该命令将:
1. 读取 `./EBIDownload.yaml` 中的配置。
2. 查询 `PRJNA1251654` 项目下的所有样本数据。
3. 使用 `prefetch` 方式 (默认), 以最多 6 个线程高速下载数据到当前目录 (`./`)。
4. 下载完成后, 自动进行后续处理 (如解压)。
