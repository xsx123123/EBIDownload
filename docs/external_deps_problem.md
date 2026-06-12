# EBIDownload 外部依赖跨平台分发问题描述

## 背景

EBIDownload 是一个用 **Rust + Tauri v2** 构建的跨平台桌面应用（支持 Windows / macOS / Linux），用于从 EBI/NCBI 高速下载生物测序数据。

它的核心下载/处理逻辑依赖以下**外部命令行工具**，这些工具**无法被直接打包进 Rust/Tauri 二进制**：

| 工具 | 来源 | 用途 | 当前获取方式 |
|------|------|------|-------------|
| `prefetch` | NCBI sra-tools | 从 NCBI SRA 数据库下载 `.sra` 文件 | Conda / 官方安装包 |
| `fasterq-dump` | NCBI sra-tools | 将 `.sra` 转换为 `.fastq` 格式 | Conda / 官方安装包 |
| `ascp` (可选) | IBM Aspera CLI | 高速替代下载通道 | Aspera Connect 客户端 |

## 当前困境

### 1. 用户门槛极高
- 终端用户（生物研究员）大多不熟悉命令行和 Conda
- 当前必须手动：装 Conda → 创建环境 → `conda install -c bioconda sra-tools` → 再手动写 `EBIDownload.yaml` 配置绝对路径（`.fastq` → `.fastq.gz` 的压缩已由 Rust 原生并行 gzip 替代，无需再安装 `pigz`）
- 这直接把 90% 的潜在 GUI 用户挡在门外

### 2. 各平台的具体问题

#### Windows
- `sra-tools` 官方提供 Windows 安装包，但安装后路径不固定，且没有自动加入 PATH
- Aspera CLI 的 Windows 版本存在但配置复杂
- Tauri 打包的 `.msi`/`.exe` 安装后，子进程 `Command::new("prefetch")` 经常找不到这些工具（因为用户没配 PATH）

#### macOS
- `sra-tools` 有 macOS 安装包，但路径不固定
- Apple Silicon (ARM64) 和 Intel (x86_64) 需要不同的二进制，增加分发复杂度
- macOS Gatekeeper / 签名问题：从互联网下载的二进制需要公证（notarization），否则被系统阻止运行

#### 通用问题
- 这些工具会**独立更新版本**，无法像 Rust crate 那样固定依赖
- `.sra` 文件格式没有成熟的纯 Rust/Python 解析库，**必须**依赖 NCBI 官方的 C++ 工具
- `fasterq-dump` 运行时还会产生大量临时文件，对磁盘空间有要求

## 期望的解决方向

我们需要一种方案，使得 **Tauri 桌面应用** 在 Windows/macOS 上**开箱即用**，无需用户手动安装任何外部依赖。

### 关键需求
1. **自动分发**：应用首次启动时，自动将所需工具下载/解压到应用私有目录（如 `~/Library/Application Support/EBIDownload/bin/` 或 `%APPDATA%\EBIDownload\bin\`）
2. **无需管理员权限**：用户普通权限即可，不写系统目录
3. **架构自适应**：自动识别 x86_64 / ARM64，下载对应二进制
4. **版本锁定**：固定某个稳定版本的工具链，避免兼容性问题
5. **签名/安全合规**：在 macOS 上不至于被 Gatekeeper 拦截；Windows 上不被 Defender 误报
6. **体积可控**：sra-tools 体积较大（数十 MB 到上百 MB），需要考虑缓存/懒加载

### 待调研的具体问题

1. **sra-tools 是否可以静态链接后随应用分发？**
   - NCBI 是否允许将 `prefetch` + `fasterq-dump` 的二进制直接打包进第三方商业/开源软件？
   - 许可证（公有领域？）是否允许重新分发？

2. **`pigz` 的跨平台替代方案** ✅ 已解决
   - 已引入 `gzp` crate（默认 `deflate_default` + `libdeflate` 后端）作为 Rust 原生并行 gzip 实现，替代外部 `pigz`。
   - 新增 `ebidownload_core::compress_fastq_files()` 统一处理 `.fastq` → `.fastq.gz` 的压缩，并在 async 上下文中通过 `tokio::task::spawn_blocking` 执行。
   - 已从依赖检查和文档中移除 `pigz`。

3. **Tauri / Rust 生态中的依赖管理最佳实践**
   - 是否有类似 `sidecar` / `sidecar-binaries` 的模式，用于将外部二进制和应用一起打包？
   - Tauri v2 的 `sidecar` 或 `resources` 配置是否适合这个场景？
   - 如何实现"首次启动检测 → 自动下载 → 校验签名/MD5 → 解压到应用数据目录"的流程？

4. **应用内 Conda/Miniconda 环境**
   - 是否可以在应用内静默安装一个最小 Conda 环境，然后通过 conda 安装 sra-tools？
   - 这样做体积、启动时间、权限问题如何？

5. **云函数 / 服务端处理（Plan B）**
   - 如果本地分发不可行，是否可以将 `.sra` → `.fastq.gz` 的转换放在云端完成，前端只负责下载最终文件？
   - 但这涉及数据隐私、成本、网络带宽，优先级较低。

## 总结

核心诉求：**如何让一个 Tauri 桌面应用在不依赖用户手动安装 Conda/系统包的前提下，在 Windows 和 macOS 上自动获得 `prefetch`、`fasterq-dump` 的运行能力。** 需要兼顾法律许可、技术可行性、用户体验和平台安全限制。（`pigz` 已由 Rust 原生并行 gzip 替代，不再是外部依赖。）
