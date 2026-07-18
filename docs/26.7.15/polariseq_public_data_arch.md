# Polariseq 公共数据库下载模块架构文档

> 版本：v1.0
> 日期：2026-07-15
> 作者：基于 Rust 的 Polariseq 项目

---

## 1. 背景与目标

Polariseq 是一个 Rust workspace 项目，由核心库 (`polariseq-core`)、CLI (`polariseq-cli`) 和 Tauri GUI (`polariseq-gui`) 组成，用于从 ENA、NCBI 等公共数据源下载生物信息学数据。

**本方案目标**：在现有架构基础上，新增一个 `public-data` 子命令，支持通过 YAML 配置文件对 NCBI BLAST 数据库（nt、nr）、Kraken 索引等公共参考数据库进行一键式下载。充分利用现有 `ResumableDownloader` 的分片续传、进度追踪能力，以及已有的 AWS SDK 依赖。

---

## 2. 架构总览

### 2.1 Workspace 结构（更新后）

```text
polariseq-cli ─┐
                  ├── polariseq-core ──┬── ftp.rs          (FTP FASTQ 下载)
polariseq-gui ─┘          │           ├── prefetch.rs     (SRA Toolkit 路径)
                              │           ├── aws_s3.rs       (SRA AWS 分片下载)
                              │           ├── upload/         (AWS SDK 上传)
                              │           ├── public_data/    ← 新增
                              │           │   ├── config.rs   (YAML 解析)
                              │           │   ├── s3.rs       (S3 工具)
                              │           │   └── downloader.rs (调度器)
                              │           └── lib.rs          (公共模型)
```

### 2.2 数据流

```text
CLI: public-data 子命令
  ├─ 读取 polariseq.yaml
  ├─ 解析 public_data HashMap<String, PublicDatabase>
  ├─ 根据 --name 过滤（或全部下载）
  ├─ PublicDataDownloader::new() 创建匿名 S3 客户端
  ├─ 对 folder 类型:
  │   └─ list_objects_v2 → include/exclude 过滤 → 批量 ResumableDownloader
  └─ 对 file 类型:
      └─ 直接 ResumableDownloader 单文件下载
```

---

## 3. 新增模块设计

### 3.1 模块职责

| 模块 | 文件 | 职责 |
|------|------|------|
| `public_data` | `mod.rs` | 模块入口，导出公共接口 |
| `config` | `config.rs` | 解析 YAML 中的 `public_data` 段落，定义数据模型 |
| `s3` | `s3.rs` | S3 URL 解析、匿名 HTTPS 转换、include/exclude 过滤 |
| `downloader` | `downloader.rs` | 下载调度器：区分 folder/file 类型，并发控制，错误处理 |

### 3.2 核心数据模型

```rust
// config.rs
#[derive(Debug, Clone, Deserialize)]
pub struct PublicDatabase {
    pub s3_url: String,           // s3://bucket/prefix 或 s3://bucket/file.tar.gz
    pub description: String,      // 人类可读描述
    pub db_type: DatabaseType,    // folder / file
    pub exclude: Option<String>,  // 排除模式（如 "*"）
    pub include: Option<String>,  // 包含模式（如 "nt.*"）
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DatabaseType {
    Folder,  // 遍历前缀下所有对象，过滤后批量下载
    File,    // 单文件直接下载
}
```

---

## 4. 下载策略详解

### 4.1 Folder 类型（以 NCBI nt 为例）

**输入**：`s3://ncbi-blast-databases/2026-07-10-12-55-02/`

**步骤**：

1. **解析 URL**：提取 `bucket = "ncbi-blast-databases"`，`prefix = "2026-07-10-12-55-02/"`
2. **列举对象**：调用 `ListObjectsV2`，获取前缀下所有对象（含大小）
3. **过滤**：
   - `exclude = "*"` → 默认排除所有
   - `include = "nt.*"` → 仅保留 `nt.` 开头的文件
   - 跳过目录标记对象（0 字节且以 `/` 结尾）
4. **并发下载**：
   - 文件级并发：默认 8 个文件同时下载（`Semaphore` 控制）
   - 分片级并发：单个文件内部 4 个分片并发（`ResumableDownloader` 控制）
5. **断点续传**：每个文件独立维护 `.meta.json`，记录已完成分片
6. **校验**：下载完成后可选进行完整性校验（文件大小或 MD5）

**输出**：本地目录 `./dbs/nt.000.nsq`, `nt.000.nhr`, `nt.000.nin` ... 等全部裸文件

### 4.2 File 类型（以 Kraken viral 为例）

**输入**：`s3://genome-idx/kraken/k2_viral_20240112.tar.gz`

**步骤**：

1. **解析 URL**：提取 `bucket = "genome-idx"`，`key = "kraken/k2_viral_20240112.tar.gz"`
2. **获取元数据**：`HeadObject` 获取文件大小
3. **单文件下载**：直接构造 `ResumableDownloader` 进行分片续传下载
4. **无需过滤**：`include` / `exclude` 对 `file` 类型不生效

**输出**：本地文件 `./dbs/k2_viral_20240112.tar.gz`

---

## 5. 核心实现细节

### 5.1 匿名 S3 客户端

```rust
let creds = aws_credential_types::Credentials::new(
    "anonymous", "anonymous", None, None, "anonymous"
);
let sdk_config = aws_config::defaults(aws_config::BehaviorVersion::latest())
    .credentials_provider(creds)
    .region(aws_sdk_s3::config::Region::new("us-east-1"))
    .load()
    .await;
let client = Client::new(&sdk_config);
```

> **注意**：NCBI 的 `ncbi-blast-databases` 和 Kraken 的 `genome-idx` 均位于 `us-east-1`。若未来扩展其他区域，建议将 `region` 加入 YAML 配置。

### 5.2 S3 URL 解析

| 输入 | bucket | prefix/key |
|------|--------|-----------|
| `s3://ncbi-blast-databases/2026-07-10-12-55-02/` | `ncbi-blast-databases` | `2026-07-10-12-55-02/` |
| `s3://genome-idx/kraken/k2_viral_20240112.tar.gz` | `genome-idx` | `kraken/k2_viral_20240112.tar.gz` |

### 5.3 过滤规则

兼容 AWS CLI `s3 sync` 的 `--exclude` / `--include` 语义：

- 先应用 `exclude`：匹配的文件被排除
- 再应用 `include`：匹配的文件被重新包含
- 使用 `wildmatch` crate 支持通配符（`*`、`?`）

**示例**：`exclude="*"`, `include="nt.*"` → 仅下载 `nt.` 开头的文件

### 5.4 并发模型

```
folder 类型:
  └─ 文件级并发: max_workers=8 (Semaphore)
      └─ 文件 A: ResumableDownloader (chunk_size=64MB, inner_workers=4)
      └─ 文件 B: ResumableDownloader (chunk_size=64MB, inner_workers=4)
      └─ ...

file 类型:
  └─ 单文件: ResumableDownloader (chunk_size=64MB, inner_workers=4)
```

---

## 6. YAML 配置规范

### 6.1 完整示例

```yaml
# polariseq.yaml
software:
  prefetch: /home/zj/.local/share/mamba/envs/sra/bin/prefetch
  fasterq_dump: /home/zj/.local/share/mamba/envs/sra/bin/fasterq-dump

# 公共数据库配置
public_data:
  ncbi_nt:
    s3_url: s3://ncbi-blast-databases/2026-07-10-12-55-02/
    description: "NCBI nt nucleotide database"
    database_type: folder
    exclude: "*"
    include: "nt.*"

  ncbi_nr:
    s3_url: s3://ncbi-blast-databases/2026-07-10-12-55-02/
    description: "NCBI nr protein database"
    database_type: folder
    exclude: "*"
    include: "nr.*"

  k2_viral:
    s3_url: s3://genome-idx/kraken/k2_viral_20240112.tar.gz
    description: "Kraken2 viral database"
    database_type: file

  k2_nt:
    s3_url: s3://genome-idx/kraken/k2_nt_20231129.tar.gz
    description: "Kraken2 nt database"
    database_type: file
```

### 6.2 字段说明

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `s3_url` | `string` | ✅ | S3 地址，格式 `s3://bucket/prefix/` 或 `s3://bucket/key` |
| `description` | `string` | ✅ | 数据库描述，用于日志输出 |
| `database_type` | `string` | ✅ | `folder`（遍历下载）或 `file`（单文件下载） |
| `exclude` | `string` | ❌ | 排除模式，支持通配符 |
| `include` | `string` | ❌ | 包含模式，支持通配符 |

---

## 7. CLI 使用方式

### 7.1 命令格式

```bash
polariseq public-data [OPTIONS] --config <YAML> [--name <NAME>] --output <DIR>
```

### 7.2 参数说明

| 参数 | 短选项 | 说明 |
|------|--------|------|
| `--config` | `-c` | YAML 配置文件路径 |
| `--name` | `-n` | 指定下载的数据库名称（如 `ncbi_nt`），不填则下载全部 |
| `--output` | `-o` | 输出目录，默认当前目录 |

### 7.3 使用示例

```bash
# 下载配置文件中定义的全部公共数据库
polariseq public-data --config polariseq.yaml --output ./dbs/

# 只下载 NCBI nt 数据库
polariseq public-data --config polariseq.yaml --name ncbi_nt --output ./dbs/

# 只下载 Kraken viral 数据库（单文件）
polariseq public-data --config polariseq.yaml --name k2_viral --output ./dbs/

# 下载 NCBI nt 和 nr
polariseq public-data --config polariseq.yaml --name ncbi_nt --output ./dbs/
polariseq public-data --config polariseq.yaml --name ncbi_nr --output ./dbs/
```

---

## 8. 关键注意事项

### 8.1 taxdb 文件遗漏

BLAST 运行时需要 `taxdb.btd` 和 `taxdb.bti` 进行 taxonomy 解析，但这两个文件**不在** `nt.*` 或 `nr.*` 的匹配范围内。

**建议方案**：
- 将 `include` 字段扩展为 `Vec<String>`，支持多模式匹配：
  ```yaml
  include:
    - "nt.*"
    - "taxdb.*"
  ```
- 或在 YAML 中单独添加一个 `ncbi_taxdb` 条目

### 8.2 存储空间预估

| 数据库 | 类型 | 预估大小 | 说明 |
|--------|------|---------|------|
| NCBI nt | folder | ~160 GB | 预格式化裸文件，无需解压 |
| NCBI nr | folder | ~80–100 GB | 预格式化裸文件 |
| Kraken2 nt | file | ~70 GB | 压缩包，需额外解压空间 |
| Kraken2 viral | file | ~2 GB | 压缩包 |

**建议**：下载前检查磁盘空间，预留 1.5 倍于下载目标的空间。

### 8.3 网络与限流

- NCBI S3 公开 bucket 无认证限制，但高并发可能被限流
- 当前默认配置（8 文件并发 × 4 分片并发 = 32 并发请求）在大多数网络环境下安全
- 若遇限流，可降低 `max_workers` 或 `inner_workers`

### 8.4 断点续传

- 完全复用 `ResumableDownloader` 的 `.meta.json` 机制
- 中断后重新运行相同命令，会自动跳过已完成的文件和分片
- 手动删除 `.meta.json` 可强制重新下载

### 8.5 日期目录硬编码问题

NCBI 的 `ncbi-blast-databases` 每日更新，日期目录会滚动。当前 YAML 中硬编码了 `2026-07-10-12-55-02/`，建议：

1. 先通过 `aws s3 cp --no-sign-request s3://ncbi-blast-databases/latest-dir -` 获取最新版本
2. 在 Rust 工具中自动拼接 URL，YAML 中只写 `s3://ncbi-blast-databases/`（不含日期）
3. 或定期手动更新 YAML 中的日期路径

---

## 9. 文件变更清单

### 9.1 新增文件

| 文件 | 说明 |
|------|------|
| `crates/polariseq-core/src/public_data/mod.rs` | 模块入口 |
| `crates/polariseq-core/src/public_data/config.rs` | YAML 解析与数据模型 |
| `crates/polariseq-core/src/public_data/s3.rs` | S3 URL 解析与过滤工具 |
| `crates/polariseq-core/src/public_data/downloader.rs` | 下载调度器 |

### 9.2 修改文件

| 文件 | 修改内容 |
|------|---------|
| `crates/polariseq-core/src/lib.rs` | 注册 `pub mod public_data;`，扩展 `AppConfig` |
| `crates/polariseq-core/src/aws_s3.rs` | 确保 `ResumableDownloader` 为 `pub` |
| `crates/polariseq-cli/src/main.rs` | 新增 `public-data` 子命令及参数解析 |
| `Cargo.toml` (core) | 新增 `wildmatch = "2"` 依赖 |

### 9.3 新增依赖

```toml
# crates/polariseq-core/Cargo.toml
[dependencies]
wildmatch = "2"
```

---

## 10. 后续优化方向

1. **动态版本获取**：自动读取 `latest-dir`，避免 YAML 中硬编码日期目录
2. **多区域支持**：将 `region` 加入 YAML 配置，支持非 `us-east-1` 的 bucket
3. **校验增强**：下载完成后自动运行 `blastdbcmd -db nt -info` 验证数据库完整性
4. **GUI 集成**：在 Tauri GUI 中新增 "Public Data" 面板，可视化选择数据库并显示下载进度
5. **增量更新**：基于文件大小或 ETag 实现数据库的增量更新（只下载变更的文件）

---

## 附录：参考链接

- NCBI BLAST S3 文档：https://github.com/ncbi/blast_plus_docs
- AWS S3 公开数据集：https://registry.opendata.aws/ncbi-blast-databases/
- Kraken2 数据库索引：https://benlangmead.github.io/aws-indexes/k2
