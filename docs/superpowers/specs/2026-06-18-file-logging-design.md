# 文件日志功能 — 设计文档

**日期**: 2026-06-18
**项目**: file_monitor

## 概述

将日志写入 `~/.file_monitor/` 目录下的日志文件，支持按天滚动。默认仅写文件，`-v` 模式下终端同步输出。文件权限与配置文件保持一致（0o600）。

---

## 方案

使用 **`flexi_logger`** 替代 `env_logger`：

- 与 `log` 门面无缝集成，现有 `log::info!` 等宏无需改动
- 内置按天滚动（`Age::Day`）
- 支持文件权限设置
- 支持多目标输出（`LogTarget::File` / `LogTarget::FileAndConsole`）
- 成熟稳定

## 改动范围

| 文件 | 改动 |
|------|------|
| `Cargo.toml` | `env_logger = "0.11"` → `flexi_logger = "0.29"` |
| `src/main.rs` `cmd_run()` | 替换 logger 初始化逻辑 |

不涉及 config.rs、monitor.rs、store.rs、mailer.rs，不新增 CLI 参数。

## 日志行为

### 输出目标

| 模式 | 日志目标 | 级别 |
|------|---------|------|
| `fm run`（默认） | 仅文件 | INFO |
| `fm run -v` | 文件 + 终端 | DEBUG |

### 文件

- **路径**：`~/.file_monitor/file_monitor.log`
- **格式**：`[2026-06-18 15:30:00] INFO 监控已启动，共 3 个文件`
- **滚动**：每天午夜自动切分，旧文件命名 `file_monitor_r2026-06-17.log`
- **保留**：无限保留
- **权限**：0o600（与 config.toml 一致）

### 终端（仅 `-v` 模式）

- 终端输出格式与文件完全一致：`[2026-06-18 15:30:00] DEBUG 开始新一轮扫描...`

## 初始化逻辑

```
cmd_run(verbose):
  1. 确保 ~/.file_monitor/ 目录存在（已有逻辑）
  2. 根据 verbose 选择日志目标：
     - verbose=true  → LogTarget::FileAndConsole
     - verbose=false → LogTarget::File
  3. 根据 verbose 选择日志级别：
     - verbose=true  → DEBUG
     - verbose=false → INFO
  4. flexi_logger 初始化：
     - FileSpec::default().basename("file_monitor")
     - directory = ~/.file_monitor/
     - rotate = Criterion::Age(Age::Day)
     - file_mode = 0o600
     - format = [timestamp] LEVEL message
  5. 后续 log::info!/warn!/error!/debug! 自动写入目标
```

现有代码中所有 `log` 宏调用无需改动。

## 边界情况

| 场景 | 处理方式 |
|------|---------|
| `~/.file_monitor/` 目录不存在 | `cmd_run` 中已有 `create_dir_all`，在 logger 初始化前执行 |
| 日志文件无写权限 | `flexi_logger` 初始化失败 → 打印错误到 stderr 并 exit(1) |
| 磁盘满 | `flexi_logger` 内部处理写入失败，不 panic，守护进程继续运行 |
