# EBIDownload TODO List

## ✅ 已修复 Bug（记录备忘）

### ~~保存配置报错 `missing field 'prefetch_path'`~~

**原因**：前端传 camelCase（`prefetchPath`），后端 `ConfigInput` serde 默认认 snake_case（`prefetch_path`）。

**修复**：给 `ConfigInput` 添加 `#[serde(rename_all = "camelCase")]`。

---

## 🔥 高优先级

### 1. GUI 界面美化

**问题**：当前 GUI 界面较为简陋，用户体验不佳。

**目标**：
- [ ] 优化整体配色方案（当前暗色主题可以保留，但需要更精致的调色板）
- [ ] 改善表单布局和间距
- [ ] 为按钮、输入框添加 hover/focus 状态样式
- [ ] 优化进度条视觉效果（添加颜色区分：下载中/转换中/压缩中/完成）
- [ ] 添加空状态提示（如无下载任务时的引导界面）
- [ ] 优化日志面板的可读性（不同级别用不同颜色：info/warn/error）
- [ ] 响应式布局适配（窗口缩小时的排版）
- [ ] 考虑使用 UI 组件库（如 Tailwind CSS / Ant Design / shadcn/ui）

**参考方向**：
```
现代生物信息学工具风格：
- 主色调：深蓝/科技蓝 (#0ea5e9, #3b82f6)
- 成功色：翡翠绿 (#10b981)
- 警告色：琥珀黄 (#f59e0b)
- 错误色：玫瑰红 (#f43f5e)
- 卡片式布局 + 微阴影
```

---

### 2. GUI 正则过滤功能

**问题**：CLI 支持 `--filter-sample`、`--filter-run`、`--exclude-sample`、`--exclude-run` 正则过滤，但 GUI 中完全没有实现。

**目标**：
- [ ] 在 Download 页面添加过滤配置区域（可折叠面板）
- [ ] 添加输入框：
  - Include Sample（正则，匹配 sample_title）
  - Include Run（正则，匹配 run_accession）
  - Exclude Sample（正则）
  - Exclude Run（正则）
- [ ] Fetch Metadata 后，在记录列表上方显示过滤条件
- [ ] 实时显示过滤结果（"共 N 条记录，过滤后 M 条"）
- [ ] 支持多个正则模式（逗号分隔或换行分隔）
- [ ] 前端预览过滤效果（在元数据表格中高亮被过滤掉的行）

**技术要点**：
- 前端用 JavaScript RegExp 做预览验证
- 后端复用 CLI 的 `RegexFilters` 逻辑
- `DownloadOptions` 中 `filter_sample` / `filter_run` 等字段已经存在，只需前端传值

**UI 布局建议**：
```
┌─ 过滤条件 ──────────────┐
│ Include Sample: [______] │
│ Include Run:    [______] │
│ Exclude Sample: [______] │
│ Exclude Run:    [______] │
│ ✅ 实时预览过滤结果      │
└──────────────────────────┘
```

---

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
