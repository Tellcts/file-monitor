use file_monitor::{config, mailer, monitor, store};

use clap::{Parser, Subcommand};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Parser)]
#[command(name = "file_monitor", about = "文件完整性校验守护进程")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// 生成默认配置文件模板
    Init,
    /// 快捷添加收件人或监控文件到配置文件
    Add {
        /// 添加收件人邮箱
        #[arg(short = 'r', long = "receiver")]
        receiver: Option<String>,

        /// 添加监控文件路径
        #[arg(short = 'f', long = "file")]
        file: Option<String>,
    },
    /// 快捷移除收件人或监控文件
    Remove {
        /// 移除收件人邮箱
        #[arg(short = 'r', long = "receiver")]
        receiver: Option<String>,

        /// 移除监控文件路径
        #[arg(short = 'f', long = "file")]
        file: Option<String>,
    },
    /// 设置轮询时间间隔（秒）
    Interval {
        /// 间隔秒数
        seconds: u64,
    },
    /// 显示配置文件及哈希基线文件路径
    Paths,
    /// 显示当前监控的文件列表
    Files,
    /// 启动文件监控守护进程
    Run {
        /// 详细日志输出
        #[arg(short = 'v', long = "verbose")]
        verbose: bool,
    },
}

fn default_config_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(format!("{}/.file_monitor", home))
}

fn default_config_path() -> PathBuf {
    default_config_dir().join("config.toml")
}

fn data_dir_from_config(config_path: &std::path::Path) -> PathBuf {
    config_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .to_path_buf()
}

const DEFAULT_CONFIG_TEMPLATE: &str = r#"# ===== 邮件配置 =====
[smtp]
host = "smtp.qq.com"
port = 465
username = "your-email@qq.com"
auth_code = "your-authorization-code"
from_name = "File Monitor"

# ===== 通知对象 =====
[notification]
to = ["admin@domain.com"]
subject_prefix = "[FileMonitor]"

# ===== 监控参数 =====
[monitor]
interval_seconds = 30

# ===== 监控文件列表 =====
[[files]]
path = "/home/tzcan/.bashrc"

[[files]]
path = "/home/tzcan/.bash_profile"
"#;

fn load_config_value() -> toml::Value {
    let config_path = default_config_path();

    if !config_path.exists() {
        eprintln!("配置文件不存在: {}", config_path.display());
        eprintln!("请先运行 'file_monitor init' 生成默认配置");
        std::process::exit(1);
    }

    let content = std::fs::read_to_string(&config_path).unwrap_or_else(|e| {
        eprintln!("无法读取配置文件: {}", e);
        std::process::exit(1);
    });

    toml::from_str(&content).unwrap_or_else(|e| {
        eprintln!("配置文件格式错误: {}", e);
        std::process::exit(1);
    })
}

fn save_config_value(value: &toml::Value) {
    let config_path = default_config_path();
    let new_content = toml::to_string_pretty(value).unwrap_or_else(|e| {
        eprintln!("序列化配置失败: {}", e);
        std::process::exit(1);
    });
    std::fs::write(&config_path, new_content).unwrap_or_else(|e| {
        eprintln!("无法写入配置文件: {}", e);
        std::process::exit(1);
    });
    // Preserve secure permission after write
    let perm = std::fs::Permissions::from_mode(0o600);
    let _ = std::fs::set_permissions(&config_path, perm);
}

fn cmd_add(receiver: Option<String>, file: Option<String>) {
    if receiver.is_none() && file.is_none() {
        eprintln!("请指定 --receiver 或 --file（至少一个）");
        eprintln!("示例: file_monitor add --receiver admin@qq.com");
        eprintln!("示例: file_monitor add --file /etc/nginx/nginx.conf");
        std::process::exit(1);
    }

    let mut value = load_config_value();

    if let Some(email) = &receiver {
        let to_array = value
            .get_mut("notification")
            .and_then(|n| n.get_mut("to"))
            .and_then(|t| t.as_array_mut());

        match to_array {
            Some(arr) => {
                if arr.iter().any(|v| v.as_str() == Some(email)) {
                    println!("收件人已存在，跳过: {}", email);
                } else {
                    arr.push(toml::Value::String(email.clone()));
                    println!("已添加收件人: {}", email);
                }
            }
            None => {
                eprintln!("配置文件中未找到 [notification] to 数组");
                std::process::exit(1);
            }
        }
    }

    if let Some(path) = &file {
        let files_array = value.get_mut("files").and_then(|f| f.as_array_mut());
        match files_array {
            Some(arr) => {
                if arr
                    .iter()
                    .any(|v| v.get("path").and_then(|p| p.as_str()) == Some(path))
                {
                    println!("文件已存在，跳过: {}", path);
                } else {
                    let mut table = toml::Table::new();
                    table.insert("path".to_string(), toml::Value::String(path.clone()));
                    arr.push(toml::Value::Table(table));
                    println!("已添加文件: {}", path);
                }
            }
            None => {
                eprintln!("配置文件中未找到 [[files]] 列表");
                std::process::exit(1);
            }
        }
    }

    save_config_value(&value);
}

fn cmd_remove(receiver: Option<String>, file: Option<String>) {
    if receiver.is_none() && file.is_none() {
        eprintln!("请指定 --receiver 或 --file（至少一个）");
        eprintln!("示例: file_monitor remove --receiver admin@qq.com");
        eprintln!("示例: file_monitor remove --file /etc/nginx/nginx.conf");
        std::process::exit(1);
    }

    let mut value = load_config_value();

    if let Some(email) = &receiver {
        let to_array = value
            .get_mut("notification")
            .and_then(|n| n.get_mut("to"))
            .and_then(|t| t.as_array_mut());

        match to_array {
            Some(arr) => {
                if let Some(pos) = arr.iter().position(|v| v.as_str() == Some(email)) {
                    arr.remove(pos);
                    println!("已移除收件人: {}", email);
                } else {
                    println!("收件人不存在: {}", email);
                }
            }
            None => {
                eprintln!("配置文件中未找到 [notification] to 数组");
                std::process::exit(1);
            }
        }
    }

    if let Some(path) = &file {
        let files_array = value.get_mut("files").and_then(|f| f.as_array_mut());
        match files_array {
            Some(arr) => {
                if let Some(pos) = arr
                    .iter()
                    .position(|v| v.get("path").and_then(|p| p.as_str()) == Some(path))
                {
                    arr.remove(pos);
                    println!("已移除文件: {}", path);
                } else {
                    println!("文件不存在: {}", path);
                }
            }
            None => {
                eprintln!("配置文件中未找到 [[files]] 列表");
                std::process::exit(1);
            }
        }
    }

    save_config_value(&value);
}

fn cmd_files() {
    let value = load_config_value();
    let files = value.get("files").and_then(|f| f.as_array());
    match files {
        Some(arr) if !arr.is_empty() => {
            println!("当前监控 {} 个文件:", arr.len());
            for entry in arr {
                if let Some(path) = entry.get("path").and_then(|p| p.as_str()) {
                    println!("  {}", path);
                }
            }
        }
        _ => println!("配置文件中暂无监控文件"),
    }
}

fn cmd_paths() {
    let config = default_config_path();
    let store = default_config_dir().join("store.json");
    println!("配置文件: {}", config.display());
    println!("基线文件: {}", store.display());
}

fn cmd_interval(seconds: u64) {
    let mut value = load_config_value();

    value["monitor"]["interval_seconds"] = toml::Value::Integer(seconds as i64);

    save_config_value(&value);
    println!("轮询间隔已设置为 {} 秒", seconds);
}

fn cmd_init() {
    let path = default_config_path();
    let dir = default_config_dir();

    if path.exists() {
        eprintln!("配置文件已存在: {}", path.display());
        eprintln!("如需覆盖，请先删除该文件再重新生成");
        std::process::exit(1);
    }

    // Create directory with 700 (rwx------)
    std::fs::create_dir_all(&dir).unwrap_or_else(|e| {
        eprintln!("无法创建目录 {}: {}", dir.display(), e);
        std::process::exit(1);
    });
    let dir_perm = std::fs::Permissions::from_mode(0o700);
    std::fs::set_permissions(&dir, dir_perm).unwrap_or_else(|e| {
        eprintln!("无法设置目录权限 {}: {}", dir.display(), e);
        std::process::exit(1);
    });

    // Write config with 600 (rw-------)
    std::fs::write(&path, DEFAULT_CONFIG_TEMPLATE).unwrap_or_else(|e| {
        eprintln!("无法写入配置文件 {}: {}", path.display(), e);
        std::process::exit(1);
    });
    let file_perm = std::fs::Permissions::from_mode(0o600);
    std::fs::set_permissions(&path, file_perm).unwrap_or_else(|e| {
        eprintln!("无法设置文件权限 {}: {}", path.display(), e);
        std::process::exit(1);
    });

    println!("配置文件已生成: {}", path.display());
    println!("请编辑该文件，填入真实的邮箱信息和监控文件路径");
}

fn cmd_run(verbose: bool) {
    let config_path = default_config_path();

    // Init logging
    let log_level = if verbose { "debug" } else { "info" };
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(log_level))
        .format_timestamp_secs()
        .init();

    // Load config
    let cfg = config::load_config(&config_path).unwrap_or_else(|e| {
        log::error!("无法加载配置文件 {}: {}", config_path.display(), e);
        std::process::exit(1);
    });

    let data_dir = data_dir_from_config(&config_path);
    std::fs::create_dir_all(&data_dir).unwrap_or_else(|e| {
        log::error!("无法创建数据目录 {}: {}", data_dir.display(), e);
        std::process::exit(1);
    });

    let store_path = data_dir.join("store.json");
    let file_paths: Vec<PathBuf> = cfg.files.iter().map(|f| f.path.clone()).collect();
    let file_count = file_paths.len();

    log::info!(
        "监控已启动，共 {} 个文件，间隔 {} 秒",
        file_count,
        cfg.monitor.interval_seconds
    );

    // Init baseline
    let baseline = monitor::init_baseline(&file_paths).unwrap_or_else(|e| {
        log::error!("初始化基线失败: {}", e);
        std::process::exit(1);
    });
    store::save_baseline(&store_path, &baseline).unwrap_or_else(|e| {
        log::error!("保存基线失败: {}", e);
    });

    // Setup mailer + tokio runtime
    let mailer = mailer::Mailer::new(cfg.smtp, cfg.notification);
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    // Send startup report
    let startup_files: Vec<(PathBuf, String)> = baseline
        .iter()
        .map(|(k, v)| (k.clone(), v.hash.clone()))
        .collect();
    rt.block_on(async {
        if let Err(e) = mailer.send_startup_report(&startup_files).await {
            log::error!("发送启动报告失败: {}", e);
        } else {
            log::info!("启动报告已发送");
        }
    });

    // Signal handling
    let running = Arc::new(AtomicBool::new(true));
    let r = running.clone();
    ctrlc::set_handler(move || {
        log::info!("收到退出信号，正在关闭...");
        r.store(false, Ordering::SeqCst);
    })
    .expect("无法设置信号处理器");

    // Main loop
    let interval = std::time::Duration::from_secs(cfg.monitor.interval_seconds);
    let mut current_baseline = baseline;

    while running.load(Ordering::SeqCst) {
        std::thread::sleep(interval);

        if !running.load(Ordering::SeqCst) {
            break;
        }

        log::debug!("开始新一轮扫描...");
        let (changes, new_baseline) = monitor::scan_and_compare(&file_paths, &current_baseline);

        // Always update in-memory baseline so mtime values stay fresh
        current_baseline = new_baseline;

        if !changes.is_empty() {
            log::warn!("检测到 {} 个文件发生变化", changes.len());
            for change in &changes {
                log::warn!("  {} — {:?}", change.path.display(), change.change_type);
            }

            rt.block_on(async {
                if let Err(e) = mailer.send_alert(&changes).await {
                    log::error!("发送告警邮件失败: {}", e);
                } else {
                    log::info!("告警邮件已发送");
                }
            });

            store::save_baseline(&store_path, &current_baseline).unwrap_or_else(|e| {
                log::error!("保存基线失败: {}", e);
            });
        }
    }

    // Graceful shutdown
    log::info!("正在保存基线并退出...");
    store::save_baseline(&store_path, &current_baseline).unwrap_or_else(|e| {
        log::error!("退出前保存基线失败: {}", e);
    });

    // Send shutdown report
    rt.block_on(async {
        if let Err(e) = mailer.send_shutdown_report(file_count).await {
            log::error!("发送退出报告失败: {}", e);
        } else {
            log::info!("退出报告已发送");
        }
    });

    log::info!("已退出");
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Command::Init => cmd_init(),
        Command::Add { receiver, file } => cmd_add(receiver, file),
        Command::Remove { receiver, file } => cmd_remove(receiver, file),
        Command::Interval { seconds } => cmd_interval(seconds),
        Command::Paths => cmd_paths(),
        Command::Files => cmd_files(),
        Command::Run { verbose } => cmd_run(verbose),
    }
}
