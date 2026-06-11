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


## 📌 中优先级

### 3. pigz 替换为 Rust 原生并行压缩

**背景**：当前依赖外部 `pigz` 命令，增加用户安装负担。

**方案**：评估 `gzp` crate（libdeflater 后端）替代 pigz，消除外部依赖。

---

### 4. 上传功能真正打通

**问题**：`run_upload_async()` 当前还是 sleep 模拟，没有调用真实的 `core::upload::run_upload()`。

---

### 5. 日志输出到前端

**问题**：下载函数内部用 `tracing::info!()` 打印日志，前端日志面板看不到这些输出。

**方案**：配置 tracing subscriber 把日志也 emit 到前端，或在前端轮询日志文件。

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

### 9. 自动检测外部依赖

- 启动时自动检测系统是否安装了 pigz / sra-tools
- 未安装时弹出引导安装提示

### 10. 主题切换

- 支持亮色 / 暗色主题切换
