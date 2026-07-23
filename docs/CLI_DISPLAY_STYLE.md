# Polariseq 命令行显示样式规范

> 本文基于 Polariseq `v1.4.2` 当前 Rust 实现整理，目标是把现有 CLI 的视觉语言、输出层级和实现方式沉淀为一套可直接迁移到其他 Rust 工具的规范。

## 1. 样式定位

Polariseq 的 CLI 风格可以概括为：

- **品牌明确**：顶层帮助和正常运行都显示大幅 ASCII Logo、产品副标题、版本号和一句品牌文案。
- **信息密度高但层级清晰**：帮助页按输入、下载、过滤、高级、全局选项分组；运行日志固定为“时间 + 级别 + 模块 + 消息”。
- **动态信息稳定**：并发任务使用多进度条，最底部固定一条全局状态栏；普通日志始终打印在进度条上方。
- **颜色具有语义**：绿色表示正常或成功，黄色表示警告或速度，红色表示失败，青色表示结构、对象名和活动状态。
- **完成后自动收拢**：单文件任务成功后清除进度条，只保留必要日志和最终摘要，避免终端堆积大量已完成任务。
- **适合生物信息下载场景**：重点展示 accession/run ID、字节数、速度、ETA、校验状态和并发队列状态。

## 2. 技术组成

| 职责 | Rust 组件 | Polariseq 中的用途 |
|---|---|---|
| 参数解析和帮助页 | `clap` | 子命令、参数分组、帮助模板、帮助页颜色 |
| 动态进度显示 | `indicatif` | 单任务进度条、spinner、`MultiProgress`、底部状态栏 |
| 结构化日志 | `tracing` | `TRACE/DEBUG/INFO/WARN/ERROR` 事件 |
| 日志订阅与过滤 | `tracing-subscriber` | 文本/JSON 输出、终端与文件双写、日志级别过滤 |
| ANSI 样式 | `nu-ansi-term` | Banner、日志级别、最终摘要着色 |
| 时间 | `chrono` | 终端日志时间和日志文件名时间戳 |

当前核心实现位置：

- `crates/polariseq-cli/src/main.rs`：帮助页、Banner、日志格式、最终摘要、全局 `MultiProgress`。
- `crates/polariseq-cli/src/ui_manager.rs`：底部全局状态栏和聚合速度。
- `crates/polariseq-core/src/progress.rs`：下载、校验、普通 spinner 的统一模板。
- `crates/polariseq-core/src/observer.rs`：core 与 CLI UI 之间的观察者接口。

main.rs 中几个值得直接抽取的实现单元是：`HELP_STYLES`、`ColoredFormatter`、`MpWriter`、`GLOBAL_MP`、`BARS_ACTIVE` 和 `print_summary_line()`；详细下载事件使用 `target: "download_detail"`，终端通过 `download_detail=off` 默认隐藏，文件日志仍然保留。

## 3. 终端信息层级

一次正常运行从上到下分为五层：

```text
┌─────────────────────────────────────────────────────────────┐
│ 1. 品牌层：ASCII Logo、产品名、版本、品牌文案              │
├─────────────────────────────────────────────────────────────┤
│ 2. 日志层：时间、级别、模块、事件消息                      │
│    新日志通过 MultiProgress 打印到动态区域上方             │
├─────────────────────────────────────────────────────────────┤
│ 3. 活动任务层：每个正在下载/校验的任务一行                 │
│    任务完成后自动清除，不长期占用终端                      │
├─────────────────────────────────────────────────────────────┤
│ 4. 全局状态层：完成、活动、排队、失败、速度、流量          │
│    固定在所有活动任务下方，每 100 ms 刷新                  │
├─────────────────────────────────────────────────────────────┤
│ 5. 收尾层：成功日志、失败错误或单行验证摘要                │
└─────────────────────────────────────────────────────────────┘
```

核心原则是：**静态信息向上沉淀，动态信息留在下方刷新，成功任务及时折叠，失败信息明确保留。**

## 4. 颜色令牌

### 4.1 语义色

| 语义 | 颜色/效果 | 使用位置 |
|---|---|---|
| 品牌主视觉 | 粗体白色 | ASCII Logo |
| 结构强调 | 粗体绿色 | 帮助页区块标题、`INFO`、成功状态 |
| 交互对象 | 粗体蓝色 | 命令名、选项名、字面量 |
| 参数占位 | 青色 | `<FILE>`、`<DIR>`、`<COMMAND>` 等 |
| 活动状态 | 粗体青色 | 进度条前缀、active 数量 |
| 次要结构 | 暗青色 | 日志模块名 |
| 时间/弱提示 | 暗紫色或 dim | 时间戳、排队、零失败、进度条尾部消息 |
| 速度/校验中 | 黄色 | `WARN`、聚合速度、校验 spinner |
| 错误/失败 | 粗体红色 | `ERROR`、失败摘要、非零失败数量 |

### 4.2 ANSI 对照

底部状态栏直接使用 ANSI code：

```text
绿色粗体  32;1  完成数量
青色粗体  36;1  活动数量
黄色粗体  33;1  实时速度
红色粗体  31;1  非零失败数量
白色粗体  37;1  当前/总字节数
弱化显示  2     排队数量、零失败数量
```

迁移到其他工具时，应优先保留“颜色语义”，不必机械保留具体颜色。例如品牌色可以替换，但成功、警告、失败之间必须保持稳定区分。

## 5. 品牌 Banner

### 5.1 组成

Banner 由四部分组成：

1. 顶部空行，用于和 shell prompt 拉开距离。
2. 六行粗体白色 ASCII Logo。
3. 青色居中副标题：`Sequencing Data Toolkit  │  v1.4.2`。
4. 青色居中双行品牌文案，结束后再留一个空行。

视觉宽度以 **72 个字符**为基准，副标题和文案按字符数计算左侧填充。

### 5.2 两种使用场景

Polariseq 为 Banner 保留了两套入口：

- **帮助页 Banner**：通过 `clap` 的 `before_help` 注入，直接内嵌 ANSI escape。
- **正常运行 Banner**：在参数解析后调用 `print_banner()`，使用 `nu-ansi-term` 着色。

这样做可以保证 `polariseq --help` 和真正执行命令时都有完整品牌识别。

### 5.3 可复用模板

```rust
const VERSION: &str = env!("CARGO_PKG_VERSION");
const LOGO_WIDTH: usize = 72;

fn center(text: &str) -> String {
    let padding = LOGO_WIDTH.saturating_sub(text.chars().count()) / 2;
    format!("{}{}", " ".repeat(padding), text)
}

fn print_banner() {
    println!();
    for line in LOGO_LINES {
        println!("{}", Color::White.bold().paint(line));
    }
    println!(
        "{}",
        Color::Cyan.paint(center(&format!("Your Toolkit  │  v{VERSION}")))
    );
    println!();
    println!("{}", Color::Cyan.paint(center("Your product sentence.")));
    println!();
}
```

### 5.4 复用注意事项

- Logo 建议控制在 **60–80 列**，否则小终端容易换行。
- 居中时使用 `chars().count()`，不要直接使用字节长度；如果包含中文或宽字符，建议改用 `unicode-width`。
- 版本号应统一使用 `env!("CARGO_PKG_VERSION")`，避免源码常量和 `Cargo.toml` 不一致。
- 在非交互输出、管道或 CI 中，可以考虑通过 `std::io::IsTerminal` 关闭大 Banner。

## 6. 帮助页样式

### 6.1 页面顺序

顶层帮助页采用以下顺序：

```text
[Banner]

[一句话产品说明]

Usage: program [OPTIONS] <COMMAND>

Commands:
  ...

Options:
  ...

Global Options:
  ...
```

子命令帮助页不重复大 Banner，直接展示：

```text
[子命令说明]

Usage: program subcommand [OPTIONS] --required <VALUE>

Options:
Input Options:
Download Options:
Filters:
Advanced Options:
Global Options:
```

这种设计既保证首次进入时有品牌感，也避免每个子命令帮助页过于冗长。

### 6.2 帮助页颜色

```rust
use clap::builder::styling::{AnsiColor, Effects, Styles};

const HELP_STYLES: Styles = Styles::styled()
    .header(AnsiColor::Green.on_default().effects(Effects::BOLD))
    .usage(AnsiColor::Cyan.on_default().effects(Effects::BOLD))
    .literal(AnsiColor::Blue.on_default().effects(Effects::BOLD))
    .placeholder(AnsiColor::Cyan.on_default())
    .error(AnsiColor::Red.on_default().effects(Effects::BOLD))
    .valid(AnsiColor::Green.on_default())
    .invalid(AnsiColor::Yellow.on_default());
```

具体含义：

- `header`：`Commands:`、`Options:`、自定义参数分组标题。
- `usage`：`Usage:`。
- `literal`：程序名、子命令、`--option`、`-o`。
- `placeholder`：`<FILE>`、`<DIR>`、`<VALUE>`。
- `error`：clap 参数错误。
- `valid/invalid`：候选值提示和非法值提示。

### 6.3 帮助模板

```rust
#[derive(Parser)]
#[command(
    version,
    about = "One-line product description",
    color = clap::ColorChoice::Always,
    styles = HELP_STYLES,
    before_help = HELP_LOGO,
    help_template = r#"{before-help}
{about-with-newline}
{usage-heading} {usage}

{all-args}
"#
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}
```

### 6.4 参数分组规则

Polariseq 使用 `help_heading` 给参数建立语义分组：

| 分组 | 放置内容 |
|---|---|
| `Input Options` | accession、TSV、输入文件、输出目录、数据集名称 |
| `Download Options` | 下载方式、并发数、线程数、chunk 大小、尺寸限制 |
| `Filters` | include/exclude 正则过滤条件 |
| `Advanced Options` | dry-run、清理中间文件、HTTP API、调试型开关 |
| `Global Options` | YAML、日志级别、日志格式等所有子命令共享参数 |

分组原则：

- 一个子命令有 6 个以上参数时应分组。
- 最常用、最影响执行结果的参数放在前面。
- 全局参数永远放在最后，减少对主流程参数的干扰。
- 必填参数应设置清晰的 `value_name`，并让 `Usage` 自动展示约束。
- 布尔开关不显示 `[default: false]`，保持帮助页简洁。

## 7. 日志行样式

### 7.1 文本格式

终端文本日志固定为：

```text
[HH:MM:SS] LEVEL [   module   ] message
```

示例：

```text
[14:32:08] INFO  [    main    ] Log file created: output/polariseq_....log
[14:32:09] INFO  [   aws_s3   ] Starting AWS S3 downloads...
[14:32:12] WARN  [   aws_s3   ] Connection failed. Retrying in 10s (1/5)...
[14:32:20] ERROR [    main    ] Application failed: ...
```

### 7.2 字段规则

| 字段 | 宽度 | 对齐 | 样式 |
|---|---:|---|---|
| 时间 | 10 字符 | 固定 `[HH:MM:SS]` | 暗紫色 |
| 级别 | 5 字符 | 左对齐 | 粗体语义色 |
| 模块 | 方括号内 12 字符 | 居中 | 暗青色 |
| 消息 | 不限 | 左对齐 | 默认终端色 |

日志级别映射：

```text
TRACE  灰色、dim
DEBUG  青色、bold
INFO   绿色、bold
WARN   黄色、bold
ERROR  红色、bold
```

模块名只取 `target` 最后一个 `::` 片段，超过 12 字符时截断。固定宽度让连续日志形成稳定的视觉列。

### 7.3 日志与进度条共存

不能直接让 `tracing` 写普通 stderr，否则日志会切碎正在刷新的进度条。Polariseq 使用以下策略：

```text
无活动进度条：日志直接写 stderr
有活动进度条：日志交给 MultiProgress::println()
```

`MultiProgress::println()` 会临时上移动态区域、写入日志、再恢复进度条，因此终端不会出现重影或错行。

可复用的 writer 骨架：

```rust
static GLOBAL_MP: LazyLock<MultiProgress> = LazyLock::new(MultiProgress::new);
static BARS_ACTIVE: AtomicBool = AtomicBool::new(false);

struct ProgressAwareWriter {
    buffer: Vec<u8>,
}

impl Write for ProgressAwareWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.buffer.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        let message = String::from_utf8_lossy(&self.buffer);
        let message = message.trim_end_matches('\n');
        if !message.is_empty() {
            if BARS_ACTIVE.load(Ordering::Relaxed) {
                let _ = GLOBAL_MP.println(message);
            } else {
                eprintln!("{message}");
            }
        }
        self.buffer.clear();
        Ok(())
    }
}
```

## 8. 日志输出通道

Polariseq 同时维护终端日志和文件日志：

| 通道 | 格式 | 颜色 | 默认级别 | 目的 |
|---|---|---|---|---|
| 终端 | 自定义紧凑文本或 JSON | 文本模式有颜色 | 用户指定，默认 `info` | 实时可读 |
| 文件 | tracing 标准文本 | 无 ANSI | `debug` | 完整审计和排错 |

文件日志包含 RFC 3339 时间、target 和 thread ID。终端默认隐藏 `download_detail` target，避免大量分块下载细节淹没主界面；文件仍保留这些 debug 信息。

日志文件名规则：

```text
普通命令：polariseq_YYYY-MM-DD_HH-MM-SS.log
下载命令：polariseq_<accession>_YYYY-MM-DD_HH-MM-SS.log
MD5 命令：polariseq_md5_YYYY-MM-DD_HH-MM-SS.log
```

复用建议：终端面向人，文件面向排错；不要为了让终端简洁而同时丢掉文件中的细节。

## 9. 单任务进度条

### 9.1 下载进度条

模板：

```text
{spinner} {prefix:<14} {bar:28} {percent:>3}% {bytes:>9}/{total:<9} {speed:>10} ETA {eta:>8} {message}
```

实际视觉示例：

```text
⠹ SRR12345678    █████████████▋░░░░░░░░░░░░░░  46%   4.6 GiB/10.0 GiB  82.4 MiB/s ETA 00:01:07 Downloading
```

样式定义：

```rust
ProgressStyle::with_template(
    "{spinner:.green} \
     {prefix:<14.bold.cyan} \
     {bar:28.cyan/bright_black} \
     {percent:>3}% \
     {binary_bytes:>9}/{binary_total_bytes:<9} \
     {binary_bytes_per_sec:>10} \
     ETA {eta_precise:>8} \
     {msg:.dim}",
)?
.progress_chars("█▉▊▋▌▍▎▏░")
.tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏ ");
```

设计细节：

- prefix 固定 14 列，适合大多数 SRA run ID。
- bar 固定 28 列，在 100 列左右终端仍可完整显示。
- 百分比右对齐 3 位，使 `  1%`、` 99%`、`100%` 不跳动。
- 字节字段左右对齐组合，斜杠位置尽量稳定。
- 速度宽 10，ETA 宽 8，减少刷新时的横向抖动。
- 尾部 message 使用 dim，因为它是状态补充，不应抢过核心数字。

### 9.2 校验进度条

```rust
ProgressStyle::with_template(
    "{spinner:.yellow} \
     {prefix:<14.bold.yellow} \
     {bar:28.green/bright_black} \
     {percent:>3}% \
     {binary_bytes:>9}/{binary_total_bytes:<9} \
     {msg:.dim}",
)?;
```

校验阶段移除速度和 ETA，视觉上更短；黄色 spinner/prefix 表示“正在检查”，绿色 bar 表示已经校验通过的字节区域。

### 9.3 非定长任务 spinner

```rust
ProgressStyle::with_template(
    "{spinner:.green} {prefix:<18.bold.cyan} {msg:.dim}"
)?;
```

适用于安装依赖、提取压缩包、等待外部命令等没有可靠 total 的任务。

### 9.4 动画字符

统一 spinner：

```text
⠋ ⠙ ⠹ ⠸ ⠼ ⠴ ⠦ ⠧ ⠇ ⠏
```

统一细粒度进度字符：

```text
█ ▉ ▊ ▋ ▌ ▍ ▎ ▏ ░
```

相比只用 `#` 和 `-`，八分之一块字符可以让较短的 28 列进度条仍然显得平滑。

## 10. 全局底部状态栏

### 10.1 固定布局

状态栏位于 `MultiProgress` 最后一行，活动任务插入到它的上一行。模板为：

```text
{spinner} ✓ N done · ↓ N active · … N queued · ! N failed · ⚡ X.X MiB/s · 📦 current/total
```

示例：

```text
⠼ ✓ 12 done · ↓ 4 active · … 27 queued · ! 0 failed · ⚡ 318.7 MiB/s · 📦 9.3 GiB/41.8 GiB
```

### 10.2 分段语义

| 分段 | 图标 | 颜色 | 说明 |
|---|---|---|---|
| 完成 | `✓` | 绿色粗体 | 已成功结束的任务数 |
| 活动 | `↓` | 青色粗体 | 正在下载/解压/压缩的任务数 |
| 排队 | `…` | dim | 尚未开始的任务数 |
| 失败 | `!` | 0 时 dim，非 0 时红色粗体 | 已失败任务数 |
| 速度 | `⚡` | 黄色粗体 | 所有活动下载的聚合速度 |
| 流量 | `📦` | 白色粗体 | 当前活动任务累计字节/总字节 |

### 10.3 刷新与速度平滑

- UI 刷新周期：**100 ms**。
- 速度采样窗口：**3 秒**。
- 速度按活动任务共享字节计数器求和后计算。
- 当活动字节总和下降时，说明某个任务已结束并被移除，立即清空旧采样，避免出现负速度。
- 使用 MiB/s，保留 1 位小数。
- 字节数使用二进制单位：`B/KiB/MiB/GiB/TiB/PiB`。

### 10.4 生命周期

```text
UiManager::start()
  ├─ 在 MultiProgress 最后插入状态栏
  ├─ 启动 spinner
  └─ 启动 100 ms 异步刷新任务

任务开始
  └─ observer.register(id, total) -> 返回共享 AtomicU64

下载推进
  └─ core 更新 AtomicU64

任务成功/失败
  ├─ unregister(id)
  └─ complete(info) 或 fail(id)

命令结束
  └─ UiManager::stop() -> abort 刷新任务并清除状态栏
```

这个观察者结构值得复用：核心下载库只依赖 trait，不依赖 CLI crate 或具体终端组件，因此同一核心可以接 CLI、GUI 或测试观察器。

## 11. 成功、失败与最终摘要

### 11.1 普通命令完成

普通下载和公共数据命令用 `INFO` 日志结束：

```text
polariseq download completed successfully!
Public data download completed successfully!
```

不再额外打印大型成功框，避免与 Banner 和进度区域形成重复视觉噪音。

### 11.2 Validate / MD5 Verify 摘要

验证类命令使用单独的一行摘要，并在摘要前留一个空行：

```text
✓ Verification finished  ·  18 passed  ·  0 failed
✗ Validation finished  ·  7 passed  ·  2 corrupted
```

颜色规则：

- 全部通过：标题绿色粗体，passed 绿色粗体，`0 failed` 普通绿色。
- 有失败：标题红色粗体，passed 仍为绿色粗体，失败数字红色粗体。
- 分隔符固定为两侧双空格的 `·`，不用多组 emoji，保持一行干净。

可复用实现：

```rust
fn print_summary(label: &str, passed: usize, failed: usize, fail_word: &str) {
    let ok = Color::Green.bold().paint(format!("{passed} passed"));
    let bad = if failed > 0 {
        Color::Red.bold().paint(format!("{failed} {fail_word}"))
    } else {
        Color::Green.paint(format!("0 {fail_word}"))
    };
    let head = if failed > 0 {
        Color::Red.bold().paint(format!("✗ {label}"))
    } else {
        Color::Green.bold().paint(format!("✓ {label}"))
    };
    eprintln!("\n{head}  ·  {ok}  ·  {bad}");
}
```

### 11.3 顶层错误

运行失败时采用两层输出：

1. `tracing::error!` 写入完整错误链，供日志和高级用户排查。
2. `eprintln!` 输出简明用户提示，并返回失败退出码。

复用时应确保：

- 错误走 stderr。
- 失败返回非零 exit code。
- 用户提示短，完整上下文进日志。
- 不要只依赖红色；文本中必须明确包含 `failed/error/invalid` 等语义。

## 12. 输出文案规则

### 12.1 动词时态

| 场景 | 推荐形式 | 示例 |
|---|---|---|
| 即将开始 | 动名词 | `Starting AWS S3 downloads...` |
| 正在执行 | 动名词/现在分词 | `Downloading`、`Verifying MD5` |
| 已经完成 | 过去式或 completed | `Metadata saved`、`download completed successfully` |
| 可恢复警告 | 原因 + retry | `Connection failed. Retrying in 10s...` |
| 用户可操作错误 | 错误 + hint | `NOT reachable` + `Hint: check DNS or proxy` |

### 12.2 标识符格式

- run/accession ID 放在行首方括号中：`[SRR12345678] Step 1: ...`。
- 文件列表使用 3–6 个空格缩进和 `-`。
- 网络检查使用 `✓`、`✗` 和缩进箭头 `→ Hint:`。
- 状态消息首字母大写：`Downloading`、`Verifying`、`Checking existing file...`。
- 普通日志尽量不用 emoji；图标集中用于状态栏、网络检查和最终摘要。

### 12.3 标点

- 持续过程使用 `...`。
- 已完成事件通常不加句号。
- 状态栏分段使用 ` · `。
- 结构化步骤使用 `Step 1:`、`Step 2:`。
- 错误中需要补充技术细节时使用冒号。

## 13. 推荐的模块拆分

把这套样式迁移到其他工具时，建议不要继续全部放在 `main.rs`，而是拆成：

```text
src/
├── main.rs
└── terminal/
    ├── mod.rs
    ├── theme.rs       # 颜色、Styles、宽度、spinner 字符
    ├── banner.rs      # Logo 和 print_banner
    ├── logging.rs     # tracing formatter、双输出层、MpWriter
    ├── progress.rs    # transfer/verify/spinner ProgressStyle
    ├── status.rs      # 全局底部状态栏
    └── summary.rs     # validate/md5 等最终摘要
```

建议对外暴露：

```rust
pub use banner::print_banner;
pub use logging::setup_logging;
pub use progress::{spinner_style, transfer_bar_style, verify_bar_style};
pub use status::{StatusManager, TaskObserver};
pub use summary::print_summary;
pub use theme::HELP_STYLES;
```

这样其他工具迁移时可以只替换 Logo、颜色令牌和业务字段，而不复制一整个大型 `main.rs`。

## 14. 最小复用依赖

```toml
[dependencies]
chrono = "0.4"
clap = { version = "4", features = ["derive"] }
indicatif = "0.17"
nu-ansi-term = "0.50"
tokio = { version = "1", features = ["rt-multi-thread", "macros", "time"] }
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = [
    "ansi",
    "env-filter",
    "fmt",
    "json",
    "local-time",
    "time",
] }
```

如果不需要异步全局状态栏，可以移除 `tokio`，改为由业务线程主动刷新。

## 15. 跨平台与降级策略

当前样式大量使用 ANSI、Braille spinner、Unicode 方块和 emoji。迁移时建议加入以下降级：

| 环境 | 建议行为 |
|---|---|
| 交互式 TTY | 完整 Banner、颜色、动态进度条、状态栏 |
| 输出重定向到文件 | 禁用动态进度条，只保留普通日志和阶段完成消息 |
| `NO_COLOR` 存在 | 禁用所有 ANSI 颜色 |
| `TERM=dumb` | 使用 ASCII spinner 或不显示 spinner |
| 非 UTF-8/旧 Windows 终端 | `✓/✗/…/⚡/📦` 降级为 `OK/ERR/.../SPD/BYTES` |
| JSON 模式 | stdout 仅输出 JSON；人类提示和进度应写 stderr 或关闭 |

特别注意：`clap::ColorChoice::Always` 会让重定向后的帮助文本仍包含 ANSI escape。通用工具更推荐：

```rust
color = clap::ColorChoice::Auto
```

如果品牌要求必须始终着色，再保留 `Always`。

## 16. 推荐统一常量

为避免多个文件逐渐产生不一致，建议集中定义：

```rust
pub const PREFIX_WIDTH: usize = 14;
pub const MODULE_WIDTH: usize = 12;
pub const BAR_WIDTH: usize = 28;
pub const STATUS_REFRESH_MS: u64 = 100;
pub const SPEED_WINDOW_SECS: u64 = 3;
pub const SPINNER_TICKS: &str = "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏ ";
pub const PROGRESS_CHARS: &str = "█▉▊▋▌▍▎▏░";
```

并用语义函数代替散落的 ANSI code：

```rust
fn success(text: impl Display) -> String;
fn warning(text: impl Display) -> String;
fn failure(text: impl Display) -> String;
fn active(text: impl Display) -> String;
fn muted(text: impl Display) -> String;
```

## 17. 迁移清单

### 品牌

- [ ] 替换 ASCII Logo。
- [ ] 替换副标题和品牌文案。
- [ ] 使用 `CARGO_PKG_VERSION` 作为唯一版本源。
- [ ] 检查 Logo 在 80 列和 120 列终端中的显示。

### 帮助页

- [ ] 设置 `HELP_STYLES`。
- [ ] 设置简短 `about`。
- [ ] 按 Input / Operation / Filters / Advanced / Global 分组。
- [ ] 为每个参数设置明确的 `value_name`。
- [ ] 检查必填参数是否正确出现在 `Usage` 中。

### 日志

- [ ] 使用固定格式 `[time] level [module] message`。
- [ ] 终端和文件日志分层。
- [ ] 文件日志关闭 ANSI。
- [ ] 让日志通过 `MultiProgress::println()` 避开进度条。
- [ ] 为详细事件设置独立 target，并默认从终端过滤。

### 进度

- [ ] 下载、校验、非定长任务使用不同模板。
- [ ] 固定 prefix、bar、百分比、速度和 ETA 的宽度。
- [ ] 完成任务使用 `finish_and_clear()` 自动折叠。
- [ ] 底部状态栏始终插入在最后一行。
- [ ] 速度使用滑动窗口，避免瞬时抖动。

### 错误与兼容

- [ ] 错误写 stderr 并返回非零退出码。
- [ ] 颜色之外仍有明确文本语义。
- [ ] 支持 `NO_COLOR`、非 TTY 和 JSON 模式。
- [ ] 对窄终端和非 Unicode 终端提供降级方案。

## 18. Polariseq 当前实现中值得改进的点

这些不影响现有风格总结，但在复制到新工具前建议修正：

1. **版本号存在重复来源**：`VERSION`、`Cargo.toml` 和帮助页 Logo 字符串都包含版本，建议统一为 `env!("CARGO_PKG_VERSION")` 并动态拼接。
2. **帮助页强制颜色**：`ColorChoice::Always` 方便品牌展示，但重定向到文件时会保留 escape，通用 CLI 建议用 `Auto`。
3. **Banner 居中未处理宽字符**：当前按 Unicode scalar 数量计算；中文、全角字符或组合字符可能不是真实终端宽度。
4. **状态栏直接拼 ANSI**：可运行，但语义颜色分散；建议抽取 theme 层，并感知 `NO_COLOR`。
5. **模块名按字节切片**：当前目标通常是 ASCII，因此正常；若 target 含非 ASCII，直接 `[..12]` 可能落在 UTF-8 字符中间。
6. **JSON 模式与动态 UI 需明确边界**：机器消费 stdout 时，最佳实践是 JSON 输出和人类进度显示彻底分流。
7. **窄终端适配不足**：下载进度行信息完整但较宽，可以依据 terminal width 提供 compact 模板。

## 19. 一套可直接复用的最终视觉规范

如果后续工具希望保留 Polariseq 的“家族风格”，可以直接遵循以下规则：

```text
品牌：粗体白 Logo + 青色副标题
帮助：绿色标题 + 蓝色命令/选项 + 青色占位符
日志：[暗紫时间] [语义色级别] [暗青固定宽模块] 默认色消息
下载：绿 spinner + 青 prefix/bar + 固定宽数字 + dim 状态
校验：黄 spinner/prefix + 绿 bar
状态：✓绿 · ↓青 · …暗 · !红/暗 · ⚡黄 · 📦白
完成：成功任务自动清除，最后只留 INFO 或单行摘要
失败：红色明确文本 + stderr + 非零退出码
布局：日志在上，任务条居中，聚合状态固定最下
```

这套样式的重点并不是“多用颜色和图标”，而是让不同输出拥有稳定的位置、宽度和语义。只要保留这种层级，即使替换品牌色、Logo 和业务字段，也能获得相同的专业终端体验。
