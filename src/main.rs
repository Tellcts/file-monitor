mod config;
mod mailer;
mod monitor;
mod store;

use clap::Parser;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[derive(Parser)]
#[command(name = "file_monitor", about = "文件完整性校验守护进程")]
struct Cli {
    /// 配置文件路径
    #[arg(short = 'c', long = "config")]
    config: Option<PathBuf>,

    /// 详细日志输出
    #[arg(short = 'v', long = "verbose")]
    verbose: bool,
}

fn default_config() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(format!("{}/.file_monitor/config.toml", home))
}

fn data_dir_from_config(config_path: &std::path::Path) -> PathBuf {
    config_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .to_path_buf()
}

fn main() {
    let cli = Cli::parse();
    let config_path = cli.config.unwrap_or_else(default_config);

    // Init logging
    let log_level = if cli.verbose { "debug" } else { "info" };
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or(log_level),
    )
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

    log::info!("监控已启动，共 {} 个文件，间隔 {} 秒", file_paths.len(), cfg.monitor.interval_seconds);

    // Init baseline
    let baseline = monitor::init_baseline(&file_paths).unwrap_or_else(|e| {
        log::error!("初始化基线失败: {}", e);
        std::process::exit(1);
    });
    store::save_baseline(&store_path, &baseline).unwrap_or_else(|e| {
        log::error!("保存基线失败: {}", e);
    });

    // Send startup report
    let mailer = mailer::Mailer::new(cfg.smtp, cfg.notification);
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    let startup_files: Vec<(PathBuf, String)> = baseline
        .iter()
        .map(|(k, v)| (k.clone(), v.hash.clone()))
        .collect();
    rt.block_on(async {
        if let Err(e) = mailer.send_startup_report(&startup_files).await {
            log::error!("发送启动报告失败: {}", e);
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

        // Always update in-memory baseline so mtime values stay fresh,
        // avoiding unnecessary hash recomputation on subsequent scans.
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

            // Persist updated baseline after changes
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
    log::info!("已退出");
}
