# 文件日志功能 — 设计文档

**日期**: 2026-06-18
**项目**: file_monitor

## 概述

将日志从终端输出（stderr）改为写入 `~/.file_monitor/` 目录下的日志文件，支持按天滚动，文件权限与配置文件保持一致（0o600）。

---

## 方案

使用 **`flexi_logger`** 替代 `env_logger`：

- 与 `log` 门面无缝集成，现有 `log::info!` 等宏无需改动
- 内置按天滚动（`Age::Day`）
- 支持文件权限设置
- 成熟稳定

## 改动范围

| 文件 | 改动 |
|------|------|
| `Cargo.toml` | `env_logger = "0.11"` → `flexi_logger = "0.29"` |
| `src/main.rs` `cmd_run()` | 替换 logger 初始化逻辑，去掉终端输出 |

不涉及 config.rs、monitor.rs、store.rs、mailer.rs，不新增 CLI 参数。

## 日志行为

- **目标**：仅写入文件 `~/.file_monitor/file_monitor.log`，不输出到终端
- **格式**：`[2026-06-18 15:30:00] INFO 监控已启动，共 3 个文件`
- **滚动**：每天午夜自动切分，旧文件命名 `file_monitor_r2026-06-17.log`
- **保留**：无限保留
- **权限**：0o600（与 config.toml 一致）
- **级别**：默认 INFO，`--verbose` 启用 DEBUG

## 初始化逻辑

```
cmd_run():
  1. 确保 ~/.file_monitor/ 目录存在
  2. flexi_logger 初始化：
     - log_target = LogTarget::File (不输出到终端)
     - directory = ~/.file_monitor/
     - file_spec = FileSpec::default().basename("file_monitor")
     - rotate = Criterion::Age(Age::Day)
     - file_mode = 0o600
  3. log::info! / warn! / error! 自动写入文件
```
