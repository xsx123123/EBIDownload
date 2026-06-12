# EBIDownload TODO List

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

---

## 🔥 高优先级


## ✅ 已完成（记录备忘）

### ~~pigz 替换为 Rust 原生并行压缩~~

**实现**：引入 `gzp` crate（默认 `deflate_default` + `libdeflate` 后端），新增 `ebidownload_core::compress_fastq_files()`；替换 `prefetch.rs`、GUI AWS 路径、CLI AWS/Prefetch 路径中的外部 `pigz` 调用；移除 `which` 与 `check_pigz_dependency()`。

### ~~上传功能真正打通~~

**实现**：`core::upload::run_upload()` 增加可选进度回调参数 `UploadProgressCallback`；GUI `run_upload_async()` 调用真实上传函数并通过 `upload-event` 向前端推送每文件进度；CLI 调用传入 `None` 保持原行为。

### ~~日志输出到前端~~

**实现**：GUI Tauri 侧新增自定义 `tracing_subscriber::Layer`（`logger.rs`），通过 channel 将 `tracing::info!/warn!/error!` 日志统一 emit 到 `app-log` 事件；前端 `App.tsx` 监听该事件并追加到日志面板。

### ~~自动检测并安装外部依赖~~

**实现**：`ebidownload-core` 新增 `deps` 模块，支持检测当前平台、从 NCBI 官方源下载 `sra-tools` 预编译包、MD5 校验、解压、安装到托管目录并写入 `EBIDownload.yaml`；CLI 新增 `deps` 子命令（`install/check/list/remove`）；GUI 启动时自动调用 `check_deps_command`，缺失时弹出模态框一键安装，安装完成后自动刷新配置。

---

## 📌 中优先级

当前暂无。

---

## 💡 低优先级 / 未来规划

### 6. 下载历史记录

- 保存每次下载的任务历史到 SQLite/JSON
- 支持查看历史下载记录、重新下载、导出报告

### 7. 批量任务队列

- 支持添加多个下载任务到队列，按顺序或并行执行
- 显示队列状态（等待中 / 进行中 / 已完成 / 失败）

### 8. 国际化 (i18n)

- 支持中英文切换
- 使用 react-i18next 实现

### 9. ~~自动检测外部依赖~~

**已完成**。`ebidownload-core` 新增 `deps` 模块：自动检测平台、下载 NCBI 官方 `sra-tools` 预编译包、MD5 校验、解压并写入 `EBIDownload.yaml`；CLI 新增 `deps` 子命令（`install/check/list/remove`）；GUI 启动时自动调用 `check_deps_command`，缺失时弹出模态框一键安装。

### 10. 主题切换

- 支持亮色 / 暗色主题切换
