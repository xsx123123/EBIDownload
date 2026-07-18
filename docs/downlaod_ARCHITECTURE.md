# 当前 Rust 下载模块架构

本文仅汇总当前实现的下载架构与模块职责，不包含新增功能的设计或实现方案。项目是一个 Rust workspace，由核心库、CLI 和 Tauri GUI 组成：

```text
polariseq-cli ─┐
                  ├── polariseq-core ── ENA / NCBI / EBI 网络服务
polariseq-gui ─┘          │
                              ├── wget（FTP FASTQ 下载）
                              ├── prefetch + fasterq-dump（SRA Toolkit）
                              └── HTTP Range（NCBI SRA 的 AWS 公共副本）
```

## 1. Workspace 与分层

| 层级 | 位置 | 当前职责 |
| --- | --- | --- |
| 核心库 | `crates/polariseq-core` | 领域模型、元数据读取与筛选、三个下载后端、压缩、校验、进度模型、SRA Toolkit 依赖管理及 S3 上传。 |
| 命令行入口 | `crates/polariseq-cli` | 使用 `clap` 解析参数；加载配置；组织下载流水线；显示日志/终端进度；可选提供进度 HTTP API。 |
| GUI 入口 | `crates/polariseq-gui/src-tauri` | 提供 Tauri 命令、下载状态事件以及暂停/取消控制；其中保留了一部分下载后处理调度代码。 |

`polariseq-core` 是可复用的主要实现层。CLI 和 GUI 都依赖它，但两端并非只调用一个统一的高层 `download` 服务：它们各自完成方法分派以及部分「下载 SRA → `fasterq-dump` 转换 → gzip 压缩」流程。

## 2. 主要数据模型与输入准备

核心定义位于 `crates/polariseq-core/src/lib.rs`：

| 模型/函数 | 作用 |
| --- | --- |
| `Config` / `SoftwarePaths` | 保存 `prefetch` 和 `fasterq-dump` 的可执行文件路径。 |
| `DownloadOptions` / `DownloadMethod` | 下载的公共参数模型；当前方法为 `Ftp`、`Prefetch`、`Aws` 和 `Auto`。 |
| `EnaRecord` | ENA Portal API 或用户 TSV 返回的一整条原始 run 元数据。 |
| `ProcessedRecord` | 从 `EnaRecord.fastq_ftp`、`fastq_md5`、`fastq_bytes` 解析出的、面向 FASTQ 下载的简化记录；最多保存双端的两个 FASTQ 文件。 |
| `fetch_ena_data` | 请求 ENA Portal API，按 accession 获得 TSV 格式的 run 元数据。 |
| `read_tsv_data` | 从本地 TSV 读取同一结构的 run 元数据。 |
| `RegexFilters` / `process_records` | 对样本名或 run accession 做包含/排除正则筛选，并构造 `ProcessedRecord`；可通过 `pe_only` 排除单端数据。 |

CLI 的标准前置流程为：

```text
--accession → ENA Portal API ─┐
                              ├→ 正则筛选 → 保存原始 metadata TSV / md5 文件
--tsv → 本地 TSV ──────────────┘                 ↓
                                           ProcessedRecord 列表
```

这里的 `ProcessedRecord` 以 ENA 提供的 FASTQ FTP 字段为中心。因此它直接服务 FTP FASTQ 下载；AWS/SRA 与 Prefetch 路径仍使用其中的 `run_accession`，再分别获取或下载对应的 SRA 数据。

## 3. 下载方法与实际数据流

### 3.1 FTP FASTQ：`ftp.rs`

入口是 `ftp::process_downloads`，输入为 `ProcessedRecord` 列表。

```text
ProcessedRecord 中的 fastq_ftp URL
  → 为 R1/R2 建立独立任务
  → Tokio Semaphore 控制文件级并发
  → 调用外部 wget -c <url>
  → 轮询本地文件大小更新 indicatif 进度条
  → 计算本地 MD5 并与 ENA 元数据核对
```

当前特征：

- 实际传输命令是系统中的 `wget`，`-c` 用于续传；`Config` 和 `Protocol` 参数目前没有参与具体的传输实现。
- 并发度由 `multithreads` 控制，粒度是 FASTQ 文件而非 run。
- 已存在同名文件时，若文件大小相同，会先进行 MD5 校验；否则由 `wget -c` 尝试续传。
- 此路径下载的是 ENA FASTQ 文件，不涉及 `.sra` 转换或 Rust 原生 gzip 压缩。

### 3.2 Prefetch：`prefetch.rs`

入口是 `prefetch::download_all`，依赖配置中的 SRA Toolkit 程序。

```text
run_accession
  → prefetch <run>（生成 <run>/<run>.sra）
  → fasterq-dump --split-3（生成 FASTQ）
  → compress_fastq_files（并行 gzip，删除成功压缩后的 .fastq）
  → 可选 cleanup_sra 删除中间 .sra
```

当前特征：

- `multithreads` 控制 run 级并发，`aws_threads` 被作为每次 `fasterq-dump -e` 的线程数传入。
- 对已有且非空的 `.sra` 或 FASTQ 会跳过相应步骤。
- `prefetch` 使用 `--max-size`、`--verify yes` 和 `--force no`。
- 转换命令报错后会检查是否仍然生成 FASTQ；确认输出不存在才将该 run 标记为失败。

### 3.3 AWS/S3 SRA：`aws_s3.rs`

该模块的名称含有 S3，但当前下载实现并**不**使用 AWS SDK 的 `GetObject`。实际流程如下：

```text
run_accession
  → NCBI E-utilities efetch（SRA XML）
  → 解析带 org=AWS 且 free_egress=worldwide 的 Alternatives URL
  → 得到 s3://bucket/key 和对应 https://bucket.s3.amazonaws.com/key
  → ResumableDownloader 以 HTTP Range 分片下载 .sra
  → MD5 校验
  → fasterq-dump --split-3
  → compress_fastq_files
  → 可选删除 .sra
```

`SraUtils::get_metadata` 负责元数据发现；它会请求 NCBI XML、重试网络错误，并解析出 `SraMetadata`：`s3_uri`、可下载的 `http_url`、可选 MD5 与文件大小。`s3_uri` 当前主要用于记录来源与推导文件名，实际字节传输使用 `reqwest` 对 `http_url` 发起请求。

`ResumableDownloader` 是当前最独立、最接近通用下载器的组件，包含：

- 根据 `chunk_size`（MB）将文件切为 HTTP byte-range 分片；`max_workers` 控制单文件分片并发。
- 预分配目标文件，多个任务按 offset 写入同一文件。
- 以 `<filename>.meta.json` 记录已完成分片 ID；下次运行可跳过已完成分片。
- 已有完整尺寸文件会先执行 MD5 验证；所有分片完成后再做完整 MD5 验证，成功后删除 `.meta.json`。
- 请求失败进行重试与退避；支持 GUI 注入的 `PauseToken` 暂停/恢复。
- 可以把字节数写入 `ProgressStore`，供 CLI 进度 API 使用。

此方法仍要求 `fasterq-dump`，因为下载对象是 NCBI SRA archive，而不是最终 FASTQ。

### 3.4 Auto：入口层的回退策略

`DownloadMethod::Auto` 的分派逻辑在 CLI 与 GUI 中实现：先尝试 AWS/S3 SRA 路径；当该路径返回错误时，转为 Prefetch 路径。FTP 不是 Auto 的当前回退目标。

## 4. 后处理、校验与进度

### 4.1 压缩与文件校验

`lib.rs` 中的 `compress_fastq_files` 使用 `gzp` 的并行 gzip 实现压缩。它会查找当前 run 的单端或双端 `.fastq`，写出 `.fastq.gz`，成功后删除原 `.fastq`。

- FTP 路径对每个下载的 FASTQ 使用 ENA 提供的 MD5 做校验。
- AWS/S3 SRA 路径对下载的 `.sra` 使用 NCBI SRA XML 中的 MD5 做校验。
- CLI 在 AWS/S3 工作流完成后还会为输出目录中的 `.gz` 文件生成汇总 MD5 文件。

### 4.2 进度模型

`progress_store.rs` 定义共享的 `ProgressStore`：`run_id → RunProgress`。每个 run 包含下载、转换（`extraction`）和压缩三个阶段及其加权的整体百分比；状态为 `Pending`、`Downloading`、`Extracting`、`Compressing`、`Completed` 或 `Failed`。

- AWS/S3 路径会把分片累计字节写入该 store；CLI 对 `fasterq-dump` 与压缩也更新对应阶段。
- CLI 可选启动 `http_server.rs` 的 `/progress` 服务，返回经 AES-256-GCM 加密的进度 JSON。
- GUI 主要通过 Tauri 的 `download-event` 向前端发送进度；AWS 路径额外使用 `PauseToken` 支持暂停/继续。
- FTP 和 Prefetch 核心模块主要使用 `indicatif` 终端进度条，未统一写入 `ProgressStore`。

## 5. 外部依赖与配置

| 依赖 | 用途 | 管理位置 |
| --- | --- | --- |
| `wget` | FTP FASTQ 下载与续传。 | `ftp.rs` 直接调用；当前未见核心层的安装/探测逻辑。 |
| SRA Toolkit：`prefetch`、`fasterq-dump` | Prefetch 下载和 SRA→FASTQ 转换；AWS/S3 路径也需要 `fasterq-dump`。 | `deps/mod.rs` 可从受管目录、YAML 配置或系统 `PATH` 发现；可下载、校验并解压官方 sra-tools。 |
| `reqwest` | ENA 元数据请求、NCBI XML 请求、AWS 公共对象的 HTTP Range 下载。 | `lib.rs`、`aws_s3.rs`。 |
| AWS SDK（`aws-config`、`aws-sdk-s3`） | 当前用于 `upload` 模块的对象上传、桶检查与策略操作。 | `upload/mod.rs`，不参与现有下载的对象读取。 |

`validate_config` 的当前行为：

- `Ftp`：不校验 `Config` 中的软件路径。
- `Prefetch`：校验 `prefetch` 与 `fasterq-dump`。
- `Aws` / `Auto`：只校验 `fasterq-dump`。

## 6. 与未来“公共 S3 数据下载”相关的当前边界

以下仅说明当前代码已经具备或尚未提供的边界，避免将现有 NCBI SRA 路径误认为通用 S3 下载功能：

- 已有：`ResumableDownloader` 可针对已知的 HTTP URL、对象大小和可选 MD5 做分片、续传、重试、完整性校验及进度上报；`resolve_urls` 也能在 `s3://bucket/key` 与标准虚拟主机 HTTPS 形式之间转换。
- 已有：项目已经依赖 AWS SDK，但该 SDK 目前只连接在 `upload` 模块，不在下载通路中。
- 未有：面向任意 `s3://` URL 的公开下载命令/输入模型；S3 bucket/key 解析与对象元数据（`HEAD`）发现；目录/前缀列举；统一的下载后端 trait 或单一高层调度器。
- 未有：私有桶认证下载、区域/endpoint 配置、请求签名、或把通用对象下载与 SRA→FASTQ 转换解耦的公共接口。
- 当前 AWS/S3 下载的入口前提是 SRA `run_accession`，且对象元数据由 NCBI E-utilities 的 XML 决定；它不是直接接受用户提供的 S3 公共对象地址。

## 7. 当前调用关系速览

```text
CLI: main.rs
  ├─ fetch_ena_data / read_tsv_data / process_records
  ├─ Ftp      → core::ftp::process_downloads
  ├─ Prefetch → core::prefetch::download_all
  ├─ Aws      → core::aws_s3::{SraUtils, ResumableDownloader}
  │              → fasterq-dump → core::compress_fastq_files
  └─ Auto     → Aws 失败后 Prefetch

GUI: app.rs
  ├─ 与 CLI 相同的元数据准备和方法分派
  ├─ 调用 core::ftp / core::prefetch
  └─ 在 GUI 层直接编排 core::aws_s3 + fasterq-dump + compress_fastq_files，
     并通过 Tauri 事件与 PauseToken 提供交互控制
```

## 8. 关键源码索引

| 主题 | 源码位置 |
| --- | --- |
| 公共模型、ENA 元数据、筛选、压缩、配置校验 | `crates/polariseq-core/src/lib.rs` |
| FTP FASTQ 下载 | `crates/polariseq-core/src/ftp.rs` |
| SRA Toolkit 下载与转换 | `crates/polariseq-core/src/prefetch.rs` |
| NCBI SRA AWS 元数据发现与 HTTP 分片续传 | `crates/polariseq-core/src/aws_s3.rs` |
| 共享进度状态 | `crates/polariseq-core/src/progress_store.rs` |
| 终端进度样式 | `crates/polariseq-core/src/progress.rs` |
| SRA Toolkit 安装、发现和配置写入 | `crates/polariseq-core/src/deps/mod.rs` |
| CLI 编排与进度 HTTP API | `crates/polariseq-cli/src/main.rs`、`crates/polariseq-cli/src/http_server.rs` |
| GUI 编排、Tauri 事件和暂停控制 | `crates/polariseq-gui/src-tauri/src/app.rs` |
