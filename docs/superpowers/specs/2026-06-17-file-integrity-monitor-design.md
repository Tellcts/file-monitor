# 文件完整性校验监控工具 — 设计文档

**日期**: 2026-06-17
**项目**: file_monitor

## 概述

用 Rust 开发一个文件完整性校验守护进程。通过 TOML 配置文件指定需要监控的文件列表，程序按固定间隔轮询文件，使用 mtime+size 快速初筛、SHA-256 哈希二次确认的方式检测文件变化，变化时通过腾讯企业邮箱 SMTP 发送告警邮件。

---

## 架构

**方案 A：单文件单体**，5 个模块文件，每个负责一个明确职责。

```
src/
├── main.rs        # 入口：参数解析、信号处理、主循环
├── config.rs      # Config 结构体 + load(path) 函数
├── store.rs       # 哈希基线读取/写入（JSON 文件）
├── monitor.rs     # 扫描文件、计算 mtime+size、计算 SHA-256、判断变化
└── mailer.rs      # SMTP 连接、组装邮件内容、发送
```

### 数据流

```
启动: ~/.file_monitor/config.toml → config.load() → Config
      ↓
初始化: Config.files → monitor.init() → 计算所有文件 SHA-256 → store.save()（静默建基线）
      → mailer.send_startup_report()（发送"监控启动"报告邮件）
      ↓
主循环 (每 N 秒):
      store.load()  →  旧基线
      monitor.scan() → HashMap<路径, (mtime, size, hash)>
      ↓ mtime+size 未变 → 跳过
      ↓ mtime+size 变化 → 计算 SHA-256
      monitor.compare() → Vec<FileChange>
      ↓
      有变化? ──Yes→ mailer.send_alert(changes) → store.save()
      │
      No──→ 等待 N 秒，继续循环
```

### 关键结构体

```rust
struct FileChange {
    path: PathBuf,
    change_type: ChangeType,  // Modified | Deleted
    old_hash: String,
    new_hash: String,
    timestamp: DateTime<Utc>,
}

enum ChangeType { Modified, Deleted }
```

---

## 配置文件设计

**默认路径**: `~/.file_monitor/config.toml`，可通过 `-c / --config` 覆盖。
**基线文件**: `~/.file_monitor/store.json`（同一目录）。
**首次启动**: 若 `~/.file_monitor/` 目录不存在，自动创建。

```toml
# ===== 邮件配置 =====
[smtp]
host = "smtp.exmail.qq.com"
port = 465                          # SSL
username = "your-email@domain.com"
auth_code = "your-authorization-code"
from_name = "File Monitor"

# ===== 通知对象 =====
[notification]
to = ["admin@domain.com", "ops@domain.com"]
subject_prefix = "[FileMonitor]"

# ===== 监控参数 =====
[monitor]
interval_seconds = 30

# ===== 监控文件列表 =====
[[files]]
path = "/etc/nginx/nginx.conf"

[[files]]
path = "/var/www/app/config.json"
```

**安全注意**: 授权码明文存储，建议配置文件权限设为 600。

---

## CLI 设计

```
file_monitor [OPTIONS]

Options:
  -c, --config <PATH>    配置文件路径 [default: ~/.file_monitor/config.toml]
  -v, --verbose          详细日志输出（DEBUG 级别）
  -h, --help             打印帮助信息
  -V, --version          打印版本号
```

---

## 日志

- 输出到 stdout/stderr，使用 `env_logger`
- 默认 INFO 级别，`--verbose` 启用 DEBUG 级别
- 格式: `[2026-06-17T15:30:00Z INFO] 检测到文件变化: /etc/nginx/nginx.conf`

---

## 邮件格式

### 告警邮件

```
主题: [FileMonitor] 文件完整性告警 - 2 个文件发生变化

文件: /etc/nginx/nginx.conf
状态: 已修改
旧哈希: abc123...
新哈希: def456...
时间: 2026-06-17 15:30:00 UTC

文件: /etc/ssh/sshd_config
状态: 已删除
旧哈希: ghi789...
时间: 2026-06-17 15:30:00 UTC
```

### 启动报告邮件

```
主题: [FileMonitor] 监控已启动 - N 个文件

文件清单:
  - /etc/nginx/nginx.conf (SHA256: abc123...)
  - /var/www/app/config.json (SHA256: def456...)
```

---

## 错误处理与边界情况

| 场景 | 处理方式 |
|------|----------|
| 被监控文件不存在 | WARN 日志，跳过该文件（不视为变化），继续检查下一个 |
| 文件权限不可读 | ERROR 日志，发送告警邮件通知 |
| 文件被删除 | 视为 Deleted，哈希记为 `<deleted>`，发邮件通知 |
| 配置文件语法错误 | 启动时立即报错退出，返回非 0 退出码 |
| SMTP 连接失败 | ERROR 日志，不重试（等下个周期再试），不丢失基线数据 |
| store.json 损坏 | 视为初始化，重新建基线，发告警邮件 |
| 磁盘满/IO 错误 | ERROR 日志，跳过本轮检查，不清除基线 |

### 优雅退出

捕获 SIGTERM / SIGINT，挂起当前轮询，flush 基线数据到 store.json 后退出（不发送额外邮件）。

---

## 依赖 Crate

```toml
[dependencies]
clap = { version = "4", features = ["derive"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
toml = "0.8"
sha2 = "0.10"
hex = "0.4"
lettre = "0.11"
chrono = { version = "0.4", features = ["serde"] }
log = "0.4"
env_logger = "0.11"
ctrlc = "3"
```

---

## 测试策略

| 层级 | 测试内容 | 方式 |
|------|----------|------|
| config.rs | 解析合法/非法 TOML | 单元测试，测试用 .toml 字符串 |
| store.rs | 读写基线 JSON、损坏 JSON 恢复 | 单元测试，临时目录 |
| monitor.rs | 哈希计算、变化检测、文件不存在 | 单元测试，临时文件 |
| mailer.rs | 邮件内容组装、字段正确性 | 单元测试，不实际发送 |
| 集成测试 | 完整流程 | tests/ 目录，临时文件 + fake SMTP |
