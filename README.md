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

### systemd 部署（推荐）

```bash
# 1. 创建专用用户
sudo useradd -r -s /sbin/nologin filemon
sudo chown -R filemon:filemon ~/.file_monitor

# 2. 安装服务
sudo cp file-monitor.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable file-monitor   # 开机自启
sudo systemctl start file-monitor    # 立即启动

# 3. 查看状态和日志
sudo systemctl status file-monitor
sudo journalctl -u file-monitor -f
```

## 构建要求

- Rust 1.85+（edition 2024）
- Linux x86-64
