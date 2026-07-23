# EBIDownload 详细调查报告

> 报告生成时间：2026-07-15  
> 调查版本：v1.4.1  
> 范围：CLI（`crates/ebidownload-cli`）、Core（`crates/ebidownload-core`）、GUI（`crates/ebidownload-gui/src-tauri`）及依赖链。

---

## 一、安全性调查

### 1.1 子进程命令执行路径未经充分校验（中-高风险）

**位置**：

- `crates/ebidownload-core/src/prefetch.rs:58`（`Command::new(&prefetch)`）
- `crates/ebidownload-core/src/prefetch.rs:92`（`Command::new(&fasterq_dump)`）
- `crates/ebidownload-cli/src/main.rs:1251`（AWS 模式调用 `fasterq-dump`）
- `crates/ebidownload-gui/src-tauri/src/app.rs:556`（GUI 调用 `fasterq-dump`）

**问题**：`prefetch` 与 `fasterq-dump` 路径来自 `EBIDownload.yaml` 配置、托管依赖目录或系统 `PATH`。`validate_config`（`lib.rs:508-531`）仅检查文件是否存在，未验证：

- 是否真正为 NCBI 官方二进制（签名/哈希）；
- 是否位于用户可写目录（易被替换）；
- 文件是否有可执行权限。

若配置文件被篡改或环境被污染，工具会直接执行攻击者指定的二进制。

**建议**：

- 对 managed dependency 目录中的二进制做 SHA256 校验；
- 对配置路径增加文件名白名单（`prefetch`、`fasterq-dump` 等），并提示用户风险；
- 考虑使用 OS 级代码签名验证（macOS `codesign`、Windows 签名、Linux 可选）。

### 1.2 FTP 模式依赖外部 `wget`，URL 未经白名单校验（中风险）

**位置**：`crates/ebidownload-core/src/ftp.rs:74-78`、`132-138`

**问题**：FTP 模式硬编码 `wget -c <url>`，其中 URL 来自 EBI Portal API 返回的 `fastq_ftp` 字段。代码未对 URL 协议、主机做白名单校验。若 API 返回恶意 URL（如 `file:///etc/passwd` 或内网地址），`wget` 可能被诱导访问非预期目标。

**建议**：

- 限制协议为 `ftp://`、`http://`、`https://`；
- 对主机名做白名单或后缀校验（如 `ebi.ac.uk`、`sra.ebi.ac.uk`）；
- 长期用 Rust 原生 HTTP/FTP 客户端替换 `wget`。

### 1.3 HTTP 进度 API 密钥派生过弱且可预测（高风险）

**位置**：

- `crates/ebidownload-cli/build.rs:11-35`
- `crates/ebidownload-cli/src/http_server.rs:14`

**问题**：

- 使用 `std::collections::hash_map::DefaultHasher`（SipHash，非加密安全）对 `EBIDOWNLOAD_PROGRESS_KEY` 或固定字符串 `"EBIDownload-progress-{version}"` 派生 32 字节密钥；
- 若未设置 `EBIDOWNLOAD_PROGRESS_KEY`，密钥完全由版本号决定，任何人可复现派生；
- `include_bytes!` 将密钥硬编码进二进制，易被静态提取。

**建议**：

- 废弃固定派生与 `DefaultHasher`；
- 强制要求运行时通过安全通道传入 32 字节随机密钥（环境变量/文件），未提供则禁用进度 API；
- 或弃用该加密 API，改用本地 Unix socket / named pipe。

### 1.4 HTTP 进度 API 默认监听 `0.0.0.0`（中风险）

**位置**：`crates/ebidownload-cli/src/http_server.rs:31`

**问题**：`TcpListener::bind(format!("0.0.0.0:{}", port))` 默认监听所有网络接口。虽然数据经 AES-256-GCM 加密，但端口暴露增加了被扫描、DoS、重放攻击的风险。

**建议**：默认绑定 `127.0.0.1`，并增加可选的访问 token / mTLS。

### 1.5 TLS 使用 `native-tls`，未显式加固（中风险）

**位置**：

- `crates/ebidownload-core/Cargo.toml:14`
- `crates/ebidownload-cli/Cargo.toml:24`
- `reqwest` 调用：`lib.rs:204`、`aws_s3.rs:93/259/330/404`、`deps/mod.rs:329/404`

**问题**：`reqwest` 启用 `native-tls`，依赖系统证书库。在企业环境中易被中间人证书劫持，且代码未显式配置根证书校验策略。

**建议**：

- 改用 `rustls-tls` 并固定 WebPKI 根证书；
- 显式禁止 `danger_accept_invalid_certs` 类配置。

### 1.6 YAML 解析依赖 C 库 `unsafe-libyaml`（中风险）

**位置**：`Cargo.toml` 中 `serde_yaml = "0.9"`

**问题**：`serde_yaml 0.9` 底层使用 `libyaml` C 库（`unsafe-libyaml`），历史上存在内存安全问题。配置文件路径由用户指定，属于受攻击面。

**建议**：

- 限制 YAML 文件大小；
- 禁用 YAML 标签/别名解析（若可行）；
- 长期迁移到纯 Rust YAML 解析器（如 `yaml-rust2`）。

### 1.7 S3 上传 Bucket Policy 合并逻辑粗放（中风险）

**位置**：`crates/ebidownload-core/src/upload/mod.rs:370-389`

**问题**：

- `get_bucket_policy` 失败时直接忽略所有错误（`Err(_) => None`），可能掩盖权限不足问题；
- 合并 policy 时未对现有 `Principal` 做校验，也未提示用户 policy 已变更。

**建议**：

- 对 `get_bucket_policy` 错误分类处理（NoSuchBucketPolicy 可忽略，AccessDenied 需报错）；
- 应用 policy 前向用户展示变更内容。

### 1.8 生成脚本权限过宽（低风险）

**位置**：`crates/ebidownload-cli/src/main.rs:1099-1102`

**问题**：`create_script` 将脚本权限设置为 `0o755`，任何本地用户均可执行。

**建议**：按最小权限原则设置为 `0o700` 或继承 `umask`。

### 1.9 未发现显式 `unsafe` 块

代码层面没有手写 `unsafe` 块；`unsafe-libyaml` 的调用由第三方 crate 封装。

---

## 二、稳定性调查

### 2.1 生产路径中存在大量 `unwrap`/`expect`（中高风险）

**位置**：

- `crates/ebidownload-core/src/aws_s3.rs:547`：`self.metadata.md5.as_ref().unwrap()`
- `crates/ebidownload-core/src/lib.rs:500`：`path.file_name().unwrap()`
- `crates/ebidownload-gui/src-tauri/src/app.rs`：大量 `Mutex::lock().unwrap()`（如 `160`、`265`、`779`、`879`）
- `crates/ebidownload-gui/src-tauri/src/logger.rs:125`、`132`
- `crates/ebidownload-gui/src-tauri/src/main.rs:42`：`expect("error while running tauri application")`

**问题**：任何 panic 都会直接终止整个进程，GUI 会直接退出。`Mutex` 被污染后 `unwrap` 会崩溃，且异步多线程场景下风险更高。

**建议**：

- 将 `unwrap`/`expect` 替换为 `?` 或显式错误处理；
- 在 GUI 入口捕获 panic 并向前端报告。

### 2.2 `tokio::spawn` 任务错误被静默吞掉（中风险）

**位置**：

- `crates/ebidownload-core/src/ftp.rs:189-191`
- `crates/ebidownload-core/src/prefetch.rs:160-164`
- `crates/ebidownload-cli/src/main.rs:1411-1415`
- `crates/ebidownload-gui/src-tauri/src/app.rs:651-660`

**问题**：`if let Err(_e) = handle.await {}` 等形式忽略任务错误，用户无法感知部分文件下载/转换失败，尤其在并发下载时。

**建议**：

- 汇总所有任务结果，任一失败即整体失败并返回详细错误；
- 或在最后输出失败文件清单。

### 2.3 取消下载时缺乏优雅关闭（中风险）

**位置**：

- `crates/ebidownload-core/src/aws_s3.rs:411-425`（monitor `abort()`）
- `crates/ebidownload-core/src/ftp.rs:119-129`、`141`（monitor `abort()`）
- `crates/ebidownload-cli/src/main.rs:1291`、`1371`
- `crates/ebidownload-gui/src-tauri/src/app.rs:884`、`896`（`handle.abort()`）

**问题**：`abort()` 不会等待任务完成，可能在文件写入、校验、压缩中途被中断，导致 `.sra`、`.fastq`、`.meta.json` 处于不一致状态。

**建议**：实现 graceful shutdown，如使用 `tokio::select!` + 取消 token，确保落盘/校验完成后再退出。

### 2.4 无磁盘空间检查（中高风险）

**位置**：所有下载/转换/压缩入口

**问题**：代码未在下载前、转换前、压缩前检查可用磁盘空间。SRA → FASTQ 膨胀约 3 倍，`fasterq-dump` 临时文件也可能占满磁盘，导致任务中途失败且文件损坏。

**建议**：

- 下载 SRA 前检查 `metadata.size * 4` 左右空间；
- 转换前检查 `sra_size * 3`；
- 压缩前检查 `fastq_size` 左右空间；
- 空间不足时提前报错并给出清理建议。

### 2.5 网络重试策略粗糙（中风险）

**位置**：

- `crates/ebidownload-core/src/aws_s3.rs:95-137`：NCBI API 固定 10 秒重试 10 次；
- `crates/ebidownload-core/src/aws_s3.rs:582-658`：chunk 重试使用指数退避，但未按 HTTP 状态码分类。

**问题**：

- 对 404 等不可重试错误也重试 10 次；
- 对 429/503 未读取 `Retry-After`；
- 无总超时控制。

**建议**：

- 404/401 立即失败；
- 429/503 读取 `Retry-After` 并配合指数退避；
- 增加整体下载超时或 max-download-time 参数。

### 2.6 `PauseToken` 使用 `Ordering::Relaxed`（低风险）

**位置**：`crates/ebidownload-core/src/aws_s3.rs:48-56`

**问题**：`Relaxed` 序保证不足，多 worker 看到 pause 状态的时间点可能不一致，理论上会导致 pause 后仍有少量数据写入。

**建议**：改为 `Acquire/Release` 或 `SeqCst`。

### 2.7 多 worker 并发写入同一文件缺少文件锁（中风险）

**位置**：`crates/ebidownload-core/src/aws_s3.rs:612-630`

**问题**：多个 worker 同时打开同一文件并按 offset 写入，未加文件级锁。本地文件系统通常安全，但在 NFS 等网络文件系统上可能出现写冲突或数据损坏。

**建议**：

- 使用 `fs2` 文件锁；
- 或每个 chunk 写入临时文件，最后合并。

### 2.8 `fasterq-dump` 非零退出码被降级为警告（中风险）

**位置**：

- `crates/ebidownload-core/src/prefetch.rs:106-116`
- `crates/ebidownload-gui/src-tauri/src/app.rs:570-594`
- `crates/ebidownload-cli/src/main.rs:1294-1299`

**问题**：`fasterq-dump` 非零退出码仅打印 warning，随后检查 FASTQ 文件是否存在；若存在则视为成功。这可能掩盖部分 reads 丢失或损坏。

**建议**：

- 默认将非零退出码视为失败；
- 或至少检查 stderr 中的错误关键字并提示用户。

---

## 三、完整性调查

### 3.1 ETag 作为 MD5 的校验逻辑不严谨（中风险）

**位置**：`crates/ebidownload-core/src/public_data/downloader.rs:332-336`

**问题**：仅当 ETag 为 32 位十六进制且不含 `-` 时才当作 MD5。S3 多部分上传的 ETag 形如 `md5-n`，会被丢弃，公共数据下载只能做 size 校验。

**建议**：

- 对分段 ETag 单独处理（需本地计算各段 MD5 再合并验证）；
- 或在上传/源端提供独立 checksum 文件。

### 3.2 FTP 断点续传未校验部分文件完整性（中风险）

**位置**：`crates/ebidownload-core/src/ftp.rs:99-115`

**问题**：当本地文件大小与远程不一致但大于 0 时，直接设置进度并继续 `wget -c`，未对已下载部分做校验。若之前下载损坏，会继续追加导致最终 MD5 失败。

**建议**：

- 非完整 resume 文件先校验已下载部分 MD5；
- 或删除重下，并提供 `--no-resume` 选项。

### 3.3 `.meta.json` 与数据文件一致性未强制同步（中风险）

**位置**：

- `crates/ebidownload-core/src/aws_s3.rs:291-308`
- `crates/ebidownload-core/src/aws_s3.rs:480-482`

**问题**：

- `save_progress` 在每次 chunk 完成时写入 `.meta.json`，但 `std::fs::write` 不 fsync；
- 若进程在 chunk 写入后、meta 写入前崩溃，或 meta 写入后 chunk 实际未落盘，会导致 resume 时认为已完成但实际数据缺失。

**建议**：

- chunk 写入后调用 `file.sync_all()`；
- `.meta.json` 先写临时文件再 `rename`。

### 3.4 AWS 模式未校验最终 FASTQ/MD5（中高风险）

**位置**：

- `crates/ebidownload-cli/src/main.rs:1418-1426`
- `crates/ebidownload-core/src/lib.rs:393-450`

**问题**：AWS 路径下载的 `.sra` 仅校验 archive MD5；转换压缩后生成的 `.fastq.gz` 的 `md5.txt` 是本地重新计算的，未与 ENA 元数据中的 `fastq_md5` 比对。

**建议**：转换完成后与 `ProcessedRecord.fastq_md5_1/2` 比对，不匹配则标记失败。

### 3.5 `generate_md5sum_file` 可能 panic 且路径信息丢失（中风险）

**位置**：`crates/ebidownload-core/src/lib.rs:500`

**问题**：

- `path.file_name().unwrap()` 在路径异常时会 panic；
- manifest 中仅保留 basename，对 public_data 等子目录结构会丢失相对路径。

**建议**：

- 使用 `ok_or` 处理；
- public_data 场景按相对路径写入 manifest。

### 3.6 公共数据目录结构丢失（中风险）

**位置**：`crates/ebidownload-core/src/public_data/downloader.rs:308-329`、`275-306`

**问题**：

- `download_object` 将 S3 key 的 basename 作为本地文件名；
- `generate_md5_manifest` 假设所有文件在 `output_dir` 根目录；
- 若 S3 key 含子目录且不同目录下有同名文件，会互相覆盖。

**建议**：按 S3 key 的目录结构保存文件，manifest 使用相对路径。

### 3.7 压缩后未校验 `.fastq.gz` 完整性（中风险）

**位置**：`crates/ebidownload-core/src/lib.rs:393-450`

**问题**：`compress_fastq_files` 在删除原始 `.fastq` 前未对生成的 `.gz` 做完整性校验。测试用例验证了可解压，但生产代码没有。

**建议**：

- 压缩后读取 gzip 尾部校验；
- 或 decompress 一小部分验证。

### 3.8 `download_chunk_http` 边界判断不精确（低风险）

**位置**：`crates/ebidownload-core/src/aws_s3.rs:642`

**问题**：使用 `current_offset > chunk.end` 而非 `>=`，理论上可能多写 1 字节边界，实际影响较小。

**建议**：改为 `>=`。

### 3.9 `process_records` 中 size 与 url/md5 长度可能错位（中风险）

**位置**：`crates/ebidownload-core/src/lib.rs:345-370`

**问题**：`sizes` 通过 `parse::<u64>().ok()` 过滤无效值，因此长度可能小于 `ftp_urls`/`md5s`。后续使用 `sizes.first()`/`sizes.get(1)` 可能导致 size 错位。

**建议**：对长度不一致的三元组做明确处理，如过滤同时无效的记录。

### 3.10 `current_file_size` 解析失败时静默设为 0（低风险）

**位置**：`crates/ebidownload-core/src/aws_s3.rs:183`

**问题**：`v.parse().unwrap_or(0)` 在 size 属性非法时静默使用 0，后续可能导致预分配 0 字节文件。

**建议**：解析失败时返回错误。

---

## 四、用户体验可提高的方面

### 4.1 CLI/GUI 默认配置路径不统一（中优先级）

**位置**：

- CLI：`crates/ebidownload-cli/src/main.rs:625-636`（可执行文件同级目录）
- GUI：`crates/ebidownload-gui/src-tauri/src/app.rs:44-49`（`~/.EBIDownload/EBIDownload.yaml`）

**问题**：README 说默认在 `~/.EBIDownload/`，但 CLI 实际不是，导致用户困惑。

**建议**：CLI 也优先查找 `~/.EBIDownload/EBIDownload.yaml`，再回退到可执行文件目录。

### 4.2 错误信息不够具体（中优先级）

**位置**：

- `crates/ebidownload-core/src/prefetch.rs:77`：`"Prefetch failed"` 丢失 stderr
- `crates/ebidownload-core/src/aws_s3.rs:121`：未包含具体 URL
- `crates/ebidownload-core/src/ftp.rs:152`：`"Download failed"` 无 URL/文件名

**建议**：在错误中保留 run_id、URL、退出码、stderr 摘要。

### 4.3 FTP/Prefetch 模式未接入统一进度存储（中优先级）

**位置**：`progress_store.rs`

**问题**：仅 AWS 模式写入 `ProgressStore`，HTTP 进度 API 与 GUI 在 FTP/Prefetch 模式下无实时进度。

**建议**：三种下载模式统一向 `ProgressStore` 报告阶段和字节进度。

### 4.4 `dry_run` 与实际下载逻辑不一致（中优先级）

**位置**：`crates/ebidownload-cli/src/main.rs:716-731`

**问题**：dry_run 只列出 `ProcessedRecord` 中的文件，不反映 AWS/SRA 是否真正可用、是否会 fallback。

**建议**：dry_run 时调用 metadata discovery，提示哪些 run 在 SRA/ENA 上不可下载。

### 4.5 GUI 日志文件被覆盖/追加，缺少按任务分文件（低优先级）

**位置**：

- `crates/ebidownload-gui/src-tauri/src/app.rs:294`
- `crates/ebidownload-gui/src-tauri/src/logger.rs:109-127`

**问题**：每次下载都将日志写入 `output/EBIDownload.log`。

**建议**：按任务时间命名日志文件，或支持滚动追加。

### 4.6 上传失败时只返回第一个错误（低优先级）

**位置**：`crates/ebidownload-core/src/upload/mod.rs:261-270`

**建议**：聚合所有失败文件名及原因后返回。

### 4.7 网络健康检查对 `public-data` 跳过但无提示（低优先级）

**位置**：`crates/ebidownload-cli/src/main.rs:600-602`

**建议**：输出一条 info 说明跳过原因。

### 4.8 上传进度条不精确（低优先级）

**位置**：`crates/ebidownload-core/src/upload/mod.rs:294-309`

**问题**：`ByteStream::from_path` 不支持进度回调，进度条在 put 完成前一直为 0，完成后直接跳到 100%。

**建议**：使用 `ByteStream::read_from` 配合自定义 reader 实现进度，或分块上传。

### 4.9 Windows 上默认配置路径不符合惯例（低优先级）

**位置**：`crates/ebidownload-gui/src-tauri/src/app.rs:44-49`

**问题**：GUI 使用 `~/.EBIDownload/EBIDownload.yaml`，Windows 上应优先使用 `%APPDATA%\EBIDownload\EBIDownload.yaml`。

**建议**：使用 `dirs::config_dir()` 或 `dirs::data_dir()`。

### 4.10 `deps/mod.rs` 解析 YAML 失败时静默回退（中优先级）

**位置**：`crates/ebidownload-core/src/deps/mod.rs:496-504`

**问题**：`serde_yaml::from_str` 失败时直接构造新 Config，会静默丢弃用户原有的 `public_data` 等配置。

**建议**：解析失败时返回错误，而不是覆盖。

---

## 五、可新增功能

1. **磁盘空间预估与检查**  
   下载前、转换前、压缩前分别检查磁盘空间，空间不足时提前失败并提示清理。

2. **SRA 可用性预检**  
   AWS 下载前主动查询 NCBI SRA XML/ODP，避免 404 后大量重试，并在 GUI/CLI 中提示数据是否已同步。

3. **统一断点续传抽象**  
   将 FTP/Prefetch/AWS 三种 resume 机制抽象为 `DownloadBackend` trait，统一状态文件、校验与恢复逻辑。

4. **下载任务队列与历史**  
   保存任务元数据到 SQLite/JSON，支持查看历史、重新下载、导出报告。`docs/TODO.md` 中已列为未来规划。

5. **上传分片/多段上传（multipart）**  
   当前 `put_object` 一次性上传大文件，不稳定网络下不可靠。可改为 multipart upload 并支持断点续传。

6. **并行 gzip 后 MD5 校验**  
   压缩后可选计算 `.fastq.gz` 的 MD5 并与 ENA 元数据比对，确保端到端完整性。

7. **原生 Aspera/ascp 支持**  
   文档提到 ascp 但代码中未实现，仅使用 `wget`。可新增 ascp 下载后端以提升速度。

8. **配置向导 / CLI 初始化命令**  
   `EBIDownload init` 自动生成配置文件并检测依赖，降低新用户上手门槛。

9. **国际化（i18n）**  
   CLI 与 GUI 支持中英文切换。`docs/TODO.md` 中已列为未来规划。

10. **指标/遥测导出**  
    支持 Prometheus/OpenTelemetry 格式的下载指标，便于集群/服务器端监控。

11. **下载后自动验证并生成报告**  
    下载完成后自动生成包含文件数、字节数、校验结果、耗时、平均速度的 HTML/JSON 报告。

12. **邮件/ webhook 通知**  
    长任务完成后通过邮件或 webhook 通知用户。

---

## 六、代码潜在问题 / Bug 汇总

| 编号 | 文件 | 行号 | 问题 | 建议 |
|------|------|------|------|------|
| 1 | `ebidownload-core/src/aws_s3.rs` | 547 | `unwrap()` 风格不佳 | 使用 `if let Some(expected)` |
| 2 | `ebidownload-cli/src/main.rs` | 657-684 | `RegexFilters` 逻辑与 core 重复 | 复用 `RegexFilters::new` |
| 3 | `ebidownload-gui/src-tauri/src/app.rs` | 245-256 | `fetch_metadata_command` 使用 `String` 而非 `PathBuf` | 统一类型 |
| 4 | `ebidownload-core/src/deps/mod.rs` | 497-498 | YAML 解析失败静默回退 | 解析失败返回错误 |
| 5 | `ebidownload-core/src/upload/mod.rs` | 294-309 | 上传进度条不精确 | 使用自定义 ByteStream reader |
| 6 | `ebidownload-core/src/public_data/downloader.rs` | 275-306 | 同名文件会覆盖 | 保留 S3 key 目录结构 |
| 7 | `ebidownload-core/src/public_data/s3.rs` | 19-23 | `s3://bucket` 空 key 边界 | 增加测试 |
| 8 | `ebidownload-core/src/lib.rs` | 345-370 | size/url/md5 长度可能错位 | 过滤无效三元组 |
| 9 | `ebidownload-core/src/aws_s3.rs` | 183 | size 解析失败设为 0 | 返回错误 |
| 10 | `ebidownload-core/src/lib.rs` | 486 | clippy warning（needless borrow） | 按 clippy 建议修改 |
| 11 | `ebidownload-cli/src/http_server.rs` | 68 | clippy warning（needless borrow） | 按 clippy 建议修改 |

---

## 七、静态检查结论

- `cargo check -p ebidownload-cli`：通过。
- `cargo clippy -p ebidownload-cli`：通过，仅 2 个 `needless_borrows_for_generic_args` warning（`lib.rs:486`、`http_server.rs:68`），无错误。
- `cargo audit`：当前环境未安装，建议后续在 CI 中引入 `cargo audit` 检查已知漏洞。

---

## 八、整改优先级建议

### 高优先级（建议立即处理）

1. 重写 HTTP 进度 API 密钥派生逻辑，禁止固定密钥入二进制。
2. HTTP 进度 API 默认绑定 `127.0.0.1`。
3. 对外部 `prefetch`/`fasterq-dump` 路径增加可执行文件校验（至少校验文件名与是否位于 managed dir）。
4. 清理生产路径中的 `unwrap`/`expect`，尤其是 GUI 中的 `Mutex::lock().unwrap()`。
5. 实现磁盘空间检查。
6. AWS 模式增加最终 FASTQ MD5 与 ENA 元数据比对。

### 中优先级

1. 网络重试按 HTTP 状态码分类处理。
2. `.meta.json` 原子写入并配合 `fsync`。
3. 统一三种下载模式进度存储。
4. 修复公共数据目录结构丢失问题。
5. CLI/GUI 默认配置路径统一。

### 低优先级

1. Windows 默认配置路径优化。
2. 上传 multipart / 进度精确化。
3. i18n、任务队列、历史记录等功能性增强。

---

## 九、本次已修复问题记录

> 修复时间：2026-07-15  
> 验证：`cargo check -p ebidownload-cli` 通过；`cargo clippy -p ebidownload-cli` 通过；`cargo test -p ebidownload-core` 通过（10/10）。

| 编号 | 原报告位置 | 问题简述 | 修复方式 |
|------|-----------|---------|---------|
| 1 | `ebidownload-core/src/aws_s3.rs:547` | `unwrap()` 风格问题 | 改为 `let Some(expected_md5) = &self.metadata.md5 else { return Ok(false); };` |
| 2 | `ebidownload-cli/src/main.rs:657-684` | CLI 重复构造 `RegexFilters` | 改为复用 `RegexFilters::new(&filter_options)`，并移除未使用的 `regex::Regex` import |
| 3 | `ebidownload-gui/src-tauri/src/app.rs:245-256` | `fetch_metadata_command` 使用 `String` 而非 `PathBuf` | 参数改为 `tsv: Option<PathBuf>`，直接传给 `read_tsv_data` |
| 4 | `ebidownload-core/src/deps/mod.rs:497-498` | YAML 解析失败静默回退 | 解析失败时返回 `Err`，不再覆盖用户原有配置 |
| 5 | `ebidownload-core/src/upload/mod.rs:294-309` | 上传进度条不精确 | 新增 multipart 上传（>100 MiB），每完成一个 part 更新进度条与回调；小文件保持原 `put_object` |
| 6 | `ebidownload-core/src/public_data/downloader.rs:275-306` | 公共数据同名文件覆盖、目录结构丢失 | `ResumableDownloader` 新增 `with_local_path`；公共数据下载按 S3 key 前缀保留目录结构；manifest 路径同步修正 |
| 7 | `ebidownload-core/src/public_data/s3.rs:19-23` | `s3://bucket` 空 key 边界 | 新增 `parses_s3_url_with_empty_key` 测试 |
| 8 | `ebidownload-core/src/lib.rs:345-370` | `size`/`url`/`md5` 长度可能错位 | `sizes` 解析改为保留位置（`unwrap_or(0)`），并增加 `ftp_urls.len() != md5s.len()` 跳过逻辑 |
| 9 | `ebidownload-core/src/aws_s3.rs:183` | size 属性解析失败静默设为 0 | 解析失败时返回 `Err`；`parse_sra_xml` 增加 `run_id` 参数以输出更具体的错误 |
| 10 | `ebidownload-core/src/lib.rs:486` | clippy `needless_borrows_for_generic_args` | `File::create(&md5_path)` → `File::create(md5_path)` |
| 11 | `ebidownload-cli/src/http_server.rs:68` | clippy `needless_borrows_for_generic_args` | `encode(&ciphertext)` / `encode(&nonce_bytes)` → 移除多余借用 |

### 修复涉及的关键文件变更

- `crates/ebidownload-core/src/aws_s3.rs`
- `crates/ebidownload-core/src/lib.rs`
- `crates/ebidownload-core/src/upload/mod.rs`
- `crates/ebidownload-core/src/deps/mod.rs`
- `crates/ebidownload-core/src/public_data/downloader.rs`
- `crates/ebidownload-core/src/public_data/s3.rs`
- `crates/ebidownload-cli/src/main.rs`
- `crates/ebidownload-cli/src/http_server.rs`
- `crates/ebidownload-gui/src-tauri/src/app.rs`

---

*报告结束。*
