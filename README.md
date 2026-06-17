# File Monitor

文件完整性校验守护进程 — 定时轮询指定文件，SHA-256 哈希比对，变化时通过 QQ 邮箱 SMTP 发送 HTML 告警邮件。

## 安装

```bash
cargo build --release
sudo cp target/release/file_monitor /usr/local/bin/
```

## 快速开始

```bash
# 1. 初始化配置
file_monitor init

# 2. 编辑配置（填入邮箱授权码）
vim ~/.file_monitor/config.toml

# 3. 启动
file_monitor run
```

## 命令一览

| 命令                                     | 说明                                           |
| ---------------------------------------- | ---------------------------------------------- |
| `file_monitor init`                      | 生成配置文件模板 `~/.file_monitor/config.toml` |
| `file_monitor add -r "email"`            | 添加收件人                                     |
| `file_monitor add -f "/path/to/file"`    | 添加监控文件                                   |
| `file_monitor remove -r "email"`         | 移除收件人                                     |
| `file_monitor remove -f "/path/to/file"` | 移除监控文件                                   |
| `file_monitor interval 60`               | 设置轮询间隔（秒）                             |
| `file_monitor paths`                     | 显示配置和基线文件路径                         |
| `file_monitor files`                     | 显示当前监控的文件列表                         |
| `file_monitor run`                       | 启动守护进程                                   |
| `file_monitor run -v`                    | 启动（详细日志）                               |

## 项目结构

```
src/
├── main.rs      # CLI 入口、子命令路由、轮询主循环、信号处理
├── config.rs    # TOML 配置解析（SmtpConfig, NotificationConfig, MonitorConfig）
├── store.rs     # 哈希基线 JSON 持久化（FileRecord: hash + mtime + size）
├── monitor.rs   # 文件扫描、mtime+size 快速筛选、SHA-256 哈希、变化比对
├── mailer.rs    # SMTP 邮件发送（lettre）、HTML 邮件模板
└── lib.rs       # 库根节点，供集成测试引用
tests/
└── integration_test.rs  # 完整流程集成测试
```

## 依赖库及用途

| Crate                | 用途           | 关键特性                                                           |
| -------------------- | -------------- | ------------------------------------------------------------------ |
| `clap` 4             | CLI 参数解析   | `derive` 宏自动生成子命令解析                                      |
| `serde` + `toml`     | 配置文件解析   | `Deserialize` 派生宏将 TOML 自动映射到 Rust 结构体                 |
| `serde_json`         | 基线文件持久化 | 将 `HashMap<PathBuf, FileRecord>` 序列化为 JSON                    |
| `sha2` + `hex`       | SHA-256 哈希   | 计算文件内容摘要，hex 编码为 64 位十六进制字符串                   |
| `lettre` 0.11        | SMTP 邮件发送  | `rustls-tls`（纯 Rust TLS）、`tokio1` 异步传输、`builder` 构建邮件 |
| `chrono`             | 时间戳         | `DateTime<Utc>` 记录变更时间，邮件中转为北京时间显示               |
| `log` + `env_logger` | 日志           | `log` 门面宏 + `env_logger` 输出到 stderr，支持 INFO/DEBUG 级别    |
| `ctrlc` 3            | 信号处理       | 捕获 SIGTERM/SIGINT，设置 `AtomicBool` 标志优雅退出                |
| `tokio` 1            | 异步运行时     | `current_thread` 运行时桥接 lettre 的异步 SMTP 发送                |

## 配置文件

`~/.file_monitor/config.toml`：

```toml
[smtp]
host = "smtp.qq.com"
port = 465
username = "your-email@qq.com"
auth_code = "your-authorization-code"    # QQ 邮箱 SMTP 授权码，非登录密码
from_name = "File Monitor"

[notification]
to = ["admin@qq.com"]
subject_prefix = "[FileMonitor]"

[monitor]
interval_seconds = 30

[[files]]
path = "/home/<username>/.bashrc"

[[files]]
path = "/home/<username>/.bash_profile"
```

### QQ 邮箱授权码获取

登录 QQ 邮箱 → 设置 → 账户 → POP3/SMTP 服务 → 开启 → 生成授权码。

## 工作原理

1. **启动** — 计算所有文件 SHA-256 建立基线，发送启动报告邮件
2. **轮询** — 每隔 N 秒 `stat()` 检查文件 mtime + size
3. **快速筛选** — mtime+size 未变则跳过哈希计算
4. **确认变更** — mtime 或 size 变化才计算 SHA-256 与基线比对
5. **告警** — 检测到变化/删除后发送 HTML 邮件
6. **退出** — SIGTERM/SIGINT 退出时保存基线，发送退出通知

## 邮件

HTML 格式邮件，包含纯文本降级。启动、告警、退出三种邮件均有发送。

## 安全

`init` 命令自动设置安全权限：

```
drwx------  .file_monitor/          # 700
-rw-------  .file_monitor/config.toml  # 600
-rw-------  .file_monitor/store.json   # 600
```

## 构建要求

- Rust 1.85+（edition 2024）
- Linux x86-64
