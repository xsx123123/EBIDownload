# Polariseq TODO List

## ✅ 已修复 Bug（记录备忘）

### ~~保存配置报错 `missing field 'prefetch_path'`~~

**原因**：前端传 camelCase（`prefetchPath`），后端 `ConfigInput` serde 默认认 snake_case（`prefetch_path`）。

**修复**：给 `ConfigInput` 添加 `#[serde(rename_all = "camelCase")]`。

---

## ✅ 已完成（记录备忘）

### ~~GUI 界面美化~~
- [x] 优化整体配色方案（采用现代生物信息学蓝/绿/红配色）
- [x] 改善表单布局和间距（采用卡片式布局）
- [x] 为按钮、输入框添加 hover/focus 状态样式
- [x] 优化进度条视觉效果（动态颜色区分：下载中/转换中/完成）
- [x] 添加空状态提示（无任务或无元数据时的引导界面）
- [x] 优化日志面板的可读性（带时间戳和级别颜色）
- [x] 响应式布局适配

### ~~GUI 正则过滤功能~~
- [x] 在 Download 页面添加过滤配置区域（卡片布局）
- [x] 添加输入框：Include Sample, Include Run, Exclude Sample, Exclude Run
- [x] 实时显示过滤结果（"N of M records selected"）
- [x] 前端预览过滤效果（在元数据表格中高亮/暗淡显示）
- [x] 后端复用 CLI 的 `RegexFilters` 逻辑

### ~~pigz 替换为 Rust 原生并行压缩~~

**实现**：引入 `gzp` crate（默认 `deflate_default` + `libdeflate` 后端），新增 `polariseq_core::compress_fastq_files()`；替换 `prefetch.rs`、GUI AWS 路径、CLI AWS/Prefetch 路径中的外部 `pigz` 调用；移除 `which` 与 `check_pigz_dependency()`。

### ~~上传功能真正打通~~

**实现**：`core::upload::run_upload()` 增加可选进度回调参数 `UploadProgressCallback`；GUI `run_upload_async()` 调用真实上传函数并通过 `upload-event` 向前端推送每文件进度；CLI 调用传入 `None` 保持原行为。

### ~~日志输出到前端~~

**实现**：GUI Tauri 侧新增自定义 `tracing_subscriber::Layer`（`logger.rs`），通过 channel 将 `tracing::info!/warn!/error!` 日志统一 emit 到 `app-log` 事件；前端 `App.tsx` 监听该事件并追加到日志面板。

### ~~自动检测并安装外部依赖~~

**实现**：`polariseq-core` 新增 `deps` 模块，支持检测当前平台、从 NCBI 官方源下载 `sra-tools` 预编译包、MD5 校验、解压、安装到托管目录并写入 `polariseq.yaml`；CLI 新增 `deps` 子命令（`install/check/list/remove`）；GUI 启动时自动调用 `check_deps_command`，缺失时弹出模态框一键安装，安装完成后自动刷新配置。

### ~~CLI 终端日志 / 进度条美化~~

**实现**（`feature/log-ui-beautify` → `main`）：对齐并简化 ASCII banner；日志 target 居中对齐；全局状态栏分段着色；进度条 Unicode 细化；validate/md5 汇总行去双 emoji。

---

## 🔥 高优先级（P0 / P1）— 审计 2026-07-16

> 来源：全库审计。测试目前全部通过，但存在「假成功」与路径不一致等逻辑问题。

### P0 — 会假成功 / 丢数据

#### 1. 任务失败被 join 循环吞掉；CLI/GUI 仍报成功

**现象**：`tokio::spawn` 返回 `Result<Result<T,E>, JoinError>`，当前 join 循环多数只处理 join panic，不处理任务内部的 `Ok(Err(...))`。

| 位置 | 模式 |
|------|------|
| `crates/polariseq-cli/src/main.rs`（AWS 批处理 join） | `if let Err(e) = handle.await` 仅处理 JoinError |
| `crates/polariseq-core/src/prefetch.rs` | 同上；最后仍 `Ok(())` + “All Prefetch tasks completed” |
| `crates/polariseq-gui/src-tauri/src/app.rs` | 同上；随后仍 emit `DownloadEvent::Completed` |
| `crates/polariseq-core/src/ftp.rs` | `if let Err(_e) = handle.await {}` 完全静默 |

**影响**：
- 部分 / 全部 run 失败时仍打印 “download completed successfully!”，退出码 0
- Auto 模式几乎不会因单 run 失败而回退 Prefetch（因为 `download_with_aws` 几乎总是 `Ok(())`）

**修复方向**：
```rust
match handle.await {
    Ok(Ok(())) => {}
    Ok(Err(e)) => { /* 计数失败 / 聚合 */ }
    Err(e) => { /* join panic */ }
}
```
任一侧失败则最终 `Err` / 非 0 退出；GUI 不在有失败时 emit `Completed`。

- [ ] CLI AWS join 聚合失败并非 0 退出
- [ ] Prefetch / FTP join 聚合失败
- [ ] GUI 失败时不 emit `Completed`，正确标记失败 run

#### 2. AWS 分片下载失败被丢弃、不重入队

**位置**：`crates/polariseq-core/src/aws_s3.rs`（chunk 结果处理）

**现象**：chunk `Err` 后错误被丢弃，当前 run 内该分片不会重入队；进度可能不完整。

**修复方向**：失败 chunk 重试 / 重入队，或 `start()` fail-fast 返回 `Err`；日志明确记录 chunk 错误。

- [ ] 分片失败可重试或 fail-fast
- [ ] 不再静默 `Err(_e) => {}`

#### 3. Auto 回退在实践中失效

**原因**：根因是 #1——批处理总返回 `Ok(())`，外层 auto 判断整批 `Err` 才 fallback。

- [ ] 批处理有任意 run 失败时返回 `Err`（或结构化失败计数）
- [ ] 补充 auto fallback 集成测试

---

### P1 — 行为不一致 / 体验差

#### 4. Prefetch vs AWS SRA 目录布局不一致

| 路径 | SRA 位置 | FASTQ 位置 |
|------|----------|------------|
| AWS（CLI） | `{output}/{run_id}.sra`（扁平） | `{output}/{run_id}_1.fastq` |
| Prefetch | `{output}/{run_id}/{run_id}.sra` | `{output}/{run_id}_1.fastq`（扁平） |

**影响**：Auto / 续传时互相找不到已有 SRA；`fasterq-dump` 参数路径不一致；cleanup 行为分叉。

- [ ] 统一约定（推荐 `{output}/{run_id}/{run_id}.sra` 或统一扁平，二选一）
- [ ] CLI / GUI / Prefetch 共用同一布局与路径计算

#### 5. GUI 在 run 失败时仍标完成

- join 忽略 `Err` 后仍 `DownloadEvent::Completed`
- Prefetch GUI 路径在 `download_all` 后可能把所有 run 标为 100% Completed

- [ ] 按实际成功/失败更新 stage
- [ ] 有失败时前端展示失败态而非 100% 成功

#### 6. `fasterq-dump` 非 0 退出仅 soft-warn

| 路径 | 行为 |
|------|------|
| CLI AWS | `warn!` 后继续 |
| Prefetch | warn / 忽略 exec 错误 |
| GUI AWS | 仅日志 warn |

仅当最终找不到 FASTQ 才判失败；中间非 0 仍可能压缩并“成功”。

- [ ] 无有效 FASTQ 时 hard fail
- [ ] 非 0 且无输出时立即失败，不进入 compress

#### 7. NCBI eutils 限流（429）与未使用 API key

**位置**：`aws_s3.rs` 元数据/链接解析相关请求

- `_api_key` 未真正发给 NCBI（无 key 约 ~3 req/s）
- 非成功状态（含 429）固定 sleep 重试，无 `Retry-After` / 指数退避
- 高并发会放大 eutils 压力（用户日志已出现 429）

- [ ] 接通配置中的 NCBI API key
- [ ] 429 / 5xx：`Retry-After` + 指数退避
- [ ] 必要时限制 eutils 并发

---

## 📌 中优先级（P2）

### 8. CLI / GUI 默认配置路径不一致

| 端 | 默认路径 |
|----|----------|
| GUI | `~/.polariseq/polariseq.yaml` |
| CLI | 可执行文件旁 `polariseq.yaml` |

`deps install` 写入传入路径，容易出现一端更新、另一端仍缺配置。

- [ ] 对齐默认路径（或文档明确双路径策略）
- [ ] README / help 文案与实现一致

### 9. Progress API 绑定 `0.0.0.0`

**位置**：`http_server.rs` — 全网卡监听 + AES，无鉴权。拿到 key 即可轮询进度。

- [ ] 默认 `127.0.0.1`，或增加鉴权 / 可选绑定地址

### 10. Prefetch 存在 SRA 即跳过下载（无完整性校验）

**位置**：`prefetch.rs` — `exists && len > 0` 即 skip，无 size/MD5（AWS 路径会校验）。

- [ ] 截断 / 损坏 SRA 应触发重新 prefetch 或校验失败

### 11. Prefetch / FTP 未接入全局状态栏

- `BARS_ACTIVE` 主要在 AWS 路径设置；Prefetch/FTP 日志与底部 `UiManager` 状态栏集成不完整

- [ ] Prefetch / FTP 接入 `UiManager` 或等价状态汇总
- [ ] 统一 `BARS_ACTIVE` 生命周期

### 12. HTTP Range 重试与进度计数

- 中途写失败时 `global_bytes` 可能与真实文件不一致，进度条可能超计

- [ ] 重试时校正进度计数
- [ ] 明确 stream 错误回滚策略

### 13. Upload 仍标 experimental

- CLI 启动有 under-testing 警告；功能已打通但仍需稳健性与文档收尾

- [ ] 失败聚合与 CLI/GUI 一致
- [ ] 去掉 experimental 或补全限制说明

---

## 💡 低优先级 / 未来规划（P3）

### 14. 下载历史记录

- 保存每次下载的任务历史到 SQLite/JSON
- 支持查看历史下载记录、重新下载、导出报告

### 15. 批量任务队列

- 支持添加多个下载任务到队列，按顺序或并行执行
- 显示队列状态（等待中 / 进行中 / 已完成 / 失败）

### 16. 国际化 (i18n)

- 支持中英文切换
- 使用 react-i18next 实现

### 17. 主题切换

- 支持亮色 / 暗色主题切换（GUI 侧已有部分实现，文档需同步）

### 18. 技术债与文档

- [ ] 抽取 CLI/GUI 共用的 AWS 下载 + convert + compress 流水线，消除复制漂移
- [ ] 为失败聚合、auto fallback、SRA 布局、chunk 重试补集成测试
- [ ] README 版本号 / What's New 与 CHANGELOG 对齐（避免 1.4.0/1.4.1 漂移）
- [ ] 修正 `docs/downlaod_ARCHITECTURE.md` 文件名拼写
- [ ] 更新 `docs/external_deps_problem.md`（deps 自动安装已落地）
- [ ] 平台：Linux aarch64 / 非 x86_64 Windows 的 sra-tools 托管安装支持
- [ ] Aspera：元数据字段存在但无下载实现（历史 CHANGELOG 曾提及）

---

## 建议修复顺序

1. **Join 失败聚合** → CLI 非 0 退出 + GUI 不假完成 + Auto 可回退  
2. **Chunk 失败重试 / fail-fast**  
3. **统一 SRA 目录布局**  
4. **fasterq-dump hard fail**  
5. **NCBI API key + 429 退避**  
6. **CLI/GUI 默认 yaml 对齐**  
7. **共享流水线 + 集成测试 + 文档收尾**
