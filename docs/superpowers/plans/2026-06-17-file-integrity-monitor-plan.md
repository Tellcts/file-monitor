# File Integrity Monitor — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a daemon that monitors file integrity via SHA-256 hashing with mtime+size fast-path filtering, detects changes, and sends email alerts via Tencent SMTP.

**Architecture:** Single-crate binary with 5 modules — config (TOML parsing), store (baseline persistence with per-file hash+mtime+size records), monitor (mtime+size quick check → SHA-256 confirmation → change detection), mailer (SMTP via lettre + tokio bridge for async), and main (CLI + init + polling loop + signal handling).

**Tech Stack:** Rust 2024 edition, clap 4, serde + toml, serde_json, sha2, lettre (rustls-tls, tokio1), chrono, log + env_logger, ctrlc, tokio (rt only), tempfile (dev).

## Global Constraints

- Rust edition 2024, binary crate `file_monitor`
- Default config path: `~/.file_monitor/config.toml`
- Baseline + config stored under `~/.file_monitor/` (auto-created on first run)
- Baseline file: `~/.file_monitor/store.json`
- SMTP: port 465 SSL (Tencent email), port configured in TOML
- mtime+size first-pass filter; SHA-256 hash for confirmation
- SIGTERM/SIGINT: flush baseline, exit cleanly (no extra email)
- Baseline JSON format: `{ "/path/to/file": { "hash": "sha256hex", "mtime": 1234567890, "size": 1024 } }`
- Startup: build baseline silently, send startup report email listing all monitored files

---

### Task 1: Project Setup + Config Module

**Files:**
- Modify: `Cargo.toml`
- Create: `src/config.rs`

**Interfaces:**
- Produces structs (all `Debug, Deserialize`): `Config`, `SmtpConfig`, `NotificationConfig`, `MonitorConfig`, `FileEntry`
- Produces: `pub fn load_config(path: &Path) -> Result<Config, Box<dyn std::error::Error>>`

- [ ] **Step 1: Add dependencies to Cargo.toml**

Overwrite `Cargo.toml`:

```toml
[package]
name = "file_monitor"
version = "0.1.0"
edition = "2024"

[dependencies]
clap = { version = "4", features = ["derive"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
toml = "0.8"
sha2 = "0.10"
hex = "0.4"
chrono = { version = "0.4", features = ["serde"] }
log = "0.4"
env_logger = "0.11"
ctrlc = "3"
tokio = { version = "1", features = ["rt-multi-thread"] }
lettre = { version = "0.11", default-features = false, features = [
    "builder",
    "rustls-tls",
    "smtp-transport",
    "tokio1-rustls-tls",
] }

[dev-dependencies]
tempfile = "3"
```

- [ ] **Step 2: Create `src/config.rs` with tests then implement**

Write `src/config.rs`:

```rust
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize)]
pub struct SmtpConfig {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub auth_code: String,
    pub from_name: String,
}

#[derive(Debug, Deserialize)]
pub struct NotificationConfig {
    pub to: Vec<String>,
    pub subject_prefix: String,
}

#[derive(Debug, Deserialize)]
pub struct MonitorConfig {
    pub interval_seconds: u64,
}

#[derive(Debug, Deserialize)]
pub struct FileEntry {
    pub path: PathBuf,
}

#[derive(Debug, Deserialize)]
pub struct Config {
    pub smtp: SmtpConfig,
    pub notification: NotificationConfig,
    pub monitor: MonitorConfig,
    pub files: Vec<FileEntry>,
}

pub fn load_config(path: &Path) -> Result<Config, Box<dyn std::error::Error>> {
    let content = std::fs::read_to_string(path)?;
    let config: Config = toml::from_str(&content)?;
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_valid_config() {
        let toml_content = r#"
[smtp]
host = "smtp.exmail.qq.com"
port = 465
username = "test@domain.com"
auth_code = "test-auth-code"
from_name = "File Monitor"

[notification]
to = ["admin@domain.com"]
subject_prefix = "[FileMonitor]"

[monitor]
interval_seconds = 30

[[files]]
path = "/etc/nginx/nginx.conf"

[[files]]
path = "/var/www/config.json"
"#;
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        std::fs::write(&config_path, toml_content).unwrap();

        let config = load_config(&config_path).unwrap();

        assert_eq!(config.smtp.host, "smtp.exmail.qq.com");
        assert_eq!(config.smtp.port, 465);
        assert_eq!(config.files.len(), 2);
        assert_eq!(config.files[0].path, PathBuf::from("/etc/nginx/nginx.conf"));
        assert_eq!(config.files[1].path, PathBuf::from("/var/www/config.json"));
    }

    #[test]
    fn test_missing_file_returns_err() {
        let result = load_config(Path::new("/nonexistent/path/config.toml"));
        assert!(result.is_err());
    }

    #[test]
    fn test_invalid_toml_returns_err() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        std::fs::write(&config_path, "not valid toml {{{").unwrap();
        let result = load_config(&config_path);
        assert!(result.is_err());
    }
}
```

- [ ] **Step 3: Build and run tests**

Run: `cargo build 2>&1 && cargo test 2>&1`

Expected: all 3 config tests pass.

- [ ] **Step 4: Commit**

```bash
git add Cargo.toml src/config.rs
git commit -m "feat: add config module with TOML parsing"
```

---

### Task 2: Store Module (Baseline Persistence)

**Files:**
- Create: `src/store.rs`

**Interfaces:**
- Produces: `pub struct FileRecord { pub hash: String, pub mtime: u64, pub size: u64 }` (derive Debug, Clone, Serialize, Deserialize)
- Produces: `pub fn load_baseline(path: &Path) -> Result<HashMap<PathBuf, FileRecord>, Box<dyn std::error::Error>>`
- Produces: `pub fn save_baseline(path: &Path, data: &HashMap<PathBuf, FileRecord>) -> Result<(), Box<dyn std::error::Error>>`
- `load_baseline` returns empty HashMap on missing/corrupt file (with WARN log for corruption)
- `save_baseline` creates parent directories if missing

- [ ] **Step 1: Create `src/store.rs` with tests then implement**

Write `src/store.rs`:

```rust
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileRecord {
    pub hash: String,
    pub mtime: u64,
    pub size: u64,
}

pub fn load_baseline(
    path: &Path,
) -> Result<HashMap<PathBuf, FileRecord>, Box<dyn std::error::Error>> {
    match std::fs::read_to_string(path) {
        Ok(content) => match serde_json::from_str(&content) {
            Ok(data) => Ok(data),
            Err(_) => {
                log::warn!("基线文件损坏，将重新初始化");
                Ok(HashMap::new())
            }
        },
        Err(_) => Ok(HashMap::new()),
    }
}

pub fn save_baseline(
    path: &Path,
    data: &HashMap<PathBuf, FileRecord>,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(data)?;
    std::fs::write(path, json)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_save_and_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let store_path = dir.path().join("store.json");

        let mut data = HashMap::new();
        data.insert(
            PathBuf::from("/etc/nginx/nginx.conf"),
            FileRecord { hash: "abc123".into(), mtime: 1718000000, size: 1024 },
        );

        save_baseline(&store_path, &data).unwrap();
        let loaded = load_baseline(&store_path).unwrap();

        assert_eq!(loaded.len(), 1);
        let record = loaded.get(&PathBuf::from("/etc/nginx/nginx.conf")).unwrap();
        assert_eq!(record.hash, "abc123");
        assert_eq!(record.mtime, 1718000000);
        assert_eq!(record.size, 1024);
    }

    #[test]
    fn test_load_nonexistent_file_returns_empty() {
        let result = load_baseline(Path::new("/nonexistent/store.json")).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_load_corrupted_json_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        let store_path = dir.path().join("store.json");
        std::fs::write(&store_path, "not json {{{").unwrap();
        let result = load_baseline(&store_path).unwrap();
        assert!(result.is_empty());
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test 2>&1`

Expected: 3 store tests pass.

- [ ] **Step 3: Commit**

```bash
git add src/store.rs
git commit -m "feat: add store module for baseline persistence with FileRecord"
```

---

### Task 3: Monitor Module (Scanning, Hashing, Comparison)

**Files:**
- Create: `src/monitor.rs`

**Interfaces:**
- Consumes: `store::FileRecord`
- Produces: `pub enum ChangeType { Modified, Deleted }` (derive Debug, Clone)
- Produces: `pub struct FileChange { pub path: PathBuf, pub change_type: ChangeType, pub old_hash: String, pub new_hash: String, pub timestamp: DateTime<Utc> }` (derive Debug, Clone)
- Produces: `pub fn compute_hash(path: &Path) -> Result<String, Box<dyn std::error::Error>>`
- Produces: `pub fn init_baseline(files: &[PathBuf]) -> Result<HashMap<PathBuf, FileRecord>, Box<dyn std::error::Error>>`
- Produces: `pub fn scan_and_compare(files: &[PathBuf], old_baseline: &HashMap<PathBuf, FileRecord>) -> (Vec<FileChange>, HashMap<PathBuf, FileRecord>)`
  - Returns (changes, new_baseline) so caller gets the updated baseline for persistence
  - For each file: stat() → compare mtime+size → if changed, compute SHA-256 → compare hash
  - File not on disk but in baseline → Deleted
  - File on disk but not in baseline → include in new_baseline (no change reported)
  - File permissions unreadable → log ERROR, send alert, skip
- Internal: `fn get_mtime_size(path: &Path) -> Result<(u64, u64), ...>`

- [ ] **Step 1: Create `src/monitor.rs` with tests then implement**

Write `src/monitor.rs`:

```rust
use crate::store::FileRecord;
use chrono::Utc;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub enum ChangeType {
    Modified,
    Deleted,
}

#[derive(Debug, Clone)]
pub struct FileChange {
    pub path: PathBuf,
    pub change_type: ChangeType,
    pub old_hash: String,
    pub new_hash: String,
    pub timestamp: chrono::DateTime<Utc>,
}

/// Returns (mtime_seconds, file_size) from file metadata.
fn get_mtime_size(path: &Path) -> Result<(u64, u64), Box<dyn std::error::Error>> {
    let metadata = std::fs::metadata(path)?;
    let mtime = metadata
        .modified()?
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();
    let size = metadata.len();
    Ok((mtime, size))
}

pub fn compute_hash(path: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let content = std::fs::read(path)?;
    let mut hasher = Sha256::new();
    hasher.update(&content);
    let result = hasher.finalize();
    Ok(hex::encode(result))
}

/// Build initial baseline for all files. Logs warnings for files that can't be read.
pub fn init_baseline(
    files: &[PathBuf],
) -> Result<HashMap<PathBuf, FileRecord>, Box<dyn std::error::Error>> {
    let mut baseline = HashMap::new();
    for path in files {
        match get_mtime_size(path) {
            Ok((mtime, size)) => match compute_hash(path) {
                Ok(hash) => {
                    baseline.insert(path.clone(), FileRecord { hash, mtime, size });
                }
                Err(e) => log::warn!("无法计算文件哈希 {}: {}", path.display(), e),
            },
            Err(e) => log::warn!("无法读取文件 {}: {}", path.display(), e),
        }
    }
    Ok(baseline)
}

/// Scan files and compare against baseline. Returns (changes, updated_baseline).
///
/// Strategy:
/// 1. stat() each file for (mtime, size)
/// 2. If mtime+size match the stored record → skip (no change)
/// 3. If mtime+size differ → compute SHA-256, compare against stored hash
/// 4. File missing from disk but in baseline → Deleted
/// 5. File on disk but not in baseline → add to new baseline silently
pub fn scan_and_compare(
    files: &[PathBuf],
    old_baseline: &HashMap<PathBuf, FileRecord>,
) -> (Vec<FileChange>, HashMap<PathBuf, FileRecord>) {
    let mut changes = Vec::new();
    let mut new_baseline = HashMap::new();
    let now = Utc::now();

    for path in files {
        match std::fs::metadata(path) {
            Ok(metadata) => {
                let current_mtime = metadata
                    .modified()
                    .map(|t| t.duration_since(std::time::UNIX_EPOCH).unwrap().as_secs())
                    .unwrap_or(0);
                let current_size = metadata.len();

                if let Some(old_record) = old_baseline.get(path) {
                    // mtime+size fast path
                    if current_mtime == old_record.mtime && current_size == old_record.size {
                        // Unchanged — carry forward old record
                        new_baseline.insert(path.clone(), old_record.clone());
                        continue;
                    }

                    // mtime or size changed — compute hash to confirm
                    match compute_hash(path) {
                        Ok(current_hash) => {
                            if current_hash != old_record.hash {
                                changes.push(FileChange {
                                    path: path.clone(),
                                    change_type: ChangeType::Modified,
                                    old_hash: old_record.hash.clone(),
                                    new_hash: current_hash.clone(),
                                    timestamp: now,
                                });
                            }
                            new_baseline.insert(
                                path.clone(),
                                FileRecord {
                                    hash: current_hash,
                                    mtime: current_mtime,
                                    size: current_size,
                                },
                            );
                        }
                        Err(e) => {
                            log::error!("无法计算文件哈希 {}: {}", path.display(), e);
                            // Alert for unreadable file
                            changes.push(FileChange {
                                path: path.clone(),
                                change_type: ChangeType::Modified,
                                old_hash: old_record.hash.clone(),
                                new_hash: "<unreadable>".to_string(),
                                timestamp: now,
                            });
                            // Keep old record in baseline
                            new_baseline.insert(path.clone(), old_record.clone());
                        }
                    }
                } else {
                    // New file not previously tracked — add silently
                    if let Ok(hash) = compute_hash(path) {
                        new_baseline.insert(
                            path.clone(),
                            FileRecord {
                                hash,
                                mtime: current_mtime,
                                size: current_size,
                            },
                        );
                    }
                }
            }
            Err(_) => {
                // File doesn't exist on disk
                if let Some(old_record) = old_baseline.get(path) {
                    changes.push(FileChange {
                        path: path.clone(),
                        change_type: ChangeType::Deleted,
                        old_hash: old_record.hash.clone(),
                        new_hash: "<deleted>".to_string(),
                        timestamp: now,
                    });
                    // Don't add deleted file to new_baseline
                }
            }
        }
    }

    (changes, new_baseline)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_hash_sha256_hex() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        std::fs::write(&file_path, b"hello world").unwrap();

        let hash = compute_hash(&file_path).unwrap();
        assert_eq!(hash.len(), 64);
        assert_eq!(
            hash,
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
    }

    #[test]
    fn test_compute_hash_nonexistent_file_err() {
        assert!(compute_hash(Path::new("/nonexistent/file.txt")).is_err());
    }

    #[test]
    fn test_init_baseline_hashes_all_readable_files() {
        let dir = tempfile::tempdir().unwrap();
        let f1 = dir.path().join("a.txt");
        let f2 = dir.path().join("b.txt");
        std::fs::write(&f1, b"aaa").unwrap();
        std::fs::write(&f2, b"bbb").unwrap();

        let baseline = init_baseline(&[f1.clone(), f2.clone()]).unwrap();
        assert_eq!(baseline.len(), 2);
        assert!(baseline.get(&f1).is_some());
        assert!(baseline.get(&f2).is_some());
        assert_ne!(baseline.get(&f1).unwrap().hash, baseline.get(&f2).unwrap().hash);
    }

    #[test]
    fn test_scan_and_compare_detects_modified() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("f.txt");
        std::fs::write(&file, b"original").unwrap();

        let old_baseline = init_baseline(&[file.clone()]).unwrap();
        let old_hash = old_baseline.get(&file).unwrap().hash.clone();

        std::fs::write(&file, b"modified").unwrap();

        let (changes, new_baseline) = scan_and_compare(&[file.clone()], &old_baseline);
        assert_eq!(changes.len(), 1);
        assert!(matches!(changes[0].change_type, ChangeType::Modified));
        assert_eq!(changes[0].old_hash, old_hash);
        assert_ne!(changes[0].new_hash, old_hash);
    }

    #[test]
    fn test_scan_and_compare_detects_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("f.txt");
        std::fs::write(&file, b"temp").unwrap();

        let old_baseline = init_baseline(&[file.clone()]).unwrap();
        let old_hash = old_baseline.get(&file).unwrap().hash.clone();

        std::fs::remove_file(&file).unwrap();

        let (changes, new_baseline) = scan_and_compare(&[file.clone()], &old_baseline);
        assert_eq!(changes.len(), 1);
        assert!(matches!(changes[0].change_type, ChangeType::Deleted));
        assert_eq!(changes[0].old_hash, old_hash);
        assert_eq!(changes[0].new_hash, "<deleted>");
        // Deleted file removed from baseline
        assert!(!new_baseline.contains_key(&file));
    }

    #[test]
    fn test_scan_and_compare_no_changes_empty() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("f.txt");
        std::fs::write(&file, b"stable").unwrap();

        let old_baseline = init_baseline(&[file.clone()]).unwrap();
        let (changes, _) = scan_and_compare(&[file.clone()], &old_baseline);
        assert!(changes.is_empty());
    }

    #[test]
    fn test_scan_unchanged_file_skips_hash_via_mtime() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("f.txt");
        std::fs::write(&file, b"content").unwrap();

        let baseline = init_baseline(&[file.clone()]).unwrap();
        // Second scan without modifying — should produce no changes
        let (changes, new_baseline) = scan_and_compare(&[file.clone()], &baseline);
        assert!(changes.is_empty());
        assert_eq!(
            new_baseline.get(&file).unwrap().hash,
            baseline.get(&file).unwrap().hash
        );
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test 2>&1`

Expected: 6 monitor tests pass.

- [ ] **Step 3: Commit**

```bash
git add src/monitor.rs
git commit -m "feat: add monitor module with mtime+size fast path and SHA-256 comparison"
```

---

### Task 4: Mailer Module (SMTP Email via lettre)

**Files:**
- Create: `src/mailer.rs`

**Interfaces:**
- Consumes: `config::SmtpConfig`, `config::NotificationConfig`, `monitor::FileChange`
- Produces: `pub struct Mailer { ... }` (private fields)
- Produces: `impl Mailer { pub fn new(smtp: SmtpConfig, notif: NotificationConfig) -> Self }`
- Produces: `pub async fn send_alert(&self, changes: &[FileChange]) -> Result<(), String>`
- Produces: `pub async fn send_startup_report(&self, file_hashes: &[(PathBuf, String)]) -> Result<(), String>`
- Email body: plain text with one line per changed file (path, status, old/new hash, timestamp)
- Uses `lettre` with tokio1-rustls-tls for port 465 SSL
- Returns `Result<(), String>` with human-readable SMTP error messages

- [ ] **Step 1: Create `src/mailer.rs` with tests then implement**

Write `src/mailer.rs`:

```rust
use crate::config::{NotificationConfig, SmtpConfig};
use crate::monitor::{ChangeType, FileChange};
use lettre::{
    message::Mailbox,
    transport::smtp::authentication::Credentials,
    AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
};
use std::path::PathBuf;

pub struct Mailer {
    smtp: SmtpConfig,
    notification: NotificationConfig,
}

impl Mailer {
    pub fn new(smtp: SmtpConfig, notification: NotificationConfig) -> Self {
        Self { smtp, notification }
    }

    /// Build alert email subject and body from a list of changes.
    fn build_alert_body(&self, changes: &[FileChange]) -> (String, String) {
        let count = changes.len();
        let subject = format!(
            "{} 文件完整性告警 - {} 个文件发生变化",
            self.notification.subject_prefix, count
        );

        let mut body = String::new();
        for change in changes {
            body.push_str(&format!("文件: {}\n", change.path.display()));
            match change.change_type {
                ChangeType::Modified => body.push_str("状态: 已修改\n"),
                ChangeType::Deleted => body.push_str("状态: 已删除\n"),
            }
            body.push_str(&format!("旧哈希: {}\n", change.old_hash));
            body.push_str(&format!("新哈希: {}\n", change.new_hash));
            body.push_str(&format!("时间: {}\n\n", change.timestamp.format("%Y-%m-%d %H:%M:%S UTC")));
        }
        (subject, body)
    }

    fn build_startup_body(&self, file_hashes: &[(PathBuf, String)]) -> (String, String) {
        let subject = format!(
            "{} 监控已启动 - {} 个文件",
            self.notification.subject_prefix,
            file_hashes.len()
        );
        let mut body = String::from("文件清单:\n");
        for (path, hash) in file_hashes {
            body.push_str(&format!("  - {} (SHA256: {})\n", path.display(), hash));
        }
        (subject, body)
    }

    pub async fn send_alert(&self, changes: &[FileChange]) -> Result<(), String> {
        if changes.is_empty() {
            return Ok(());
        }
        let (subject, body) = self.build_alert_body(changes);
        self.send_email(&subject, &body).await
    }

    pub async fn send_startup_report(
        &self,
        file_hashes: &[(PathBuf, String)],
    ) -> Result<(), String> {
        let (subject, body) = self.build_startup_body(file_hashes);
        self.send_email(&subject, &body).await
    }

    async fn send_email(&self, subject: &str, body: &str) -> Result<(), String> {
        let from: Mailbox = format!("{} <{}>", self.smtp.from_name, self.smtp.username)
            .parse()
            .map_err(|e| format!("无效的发件人地址: {}", e))?;

        let to_addresses: Vec<Mailbox> = self
            .notification
            .to
            .iter()
            .filter_map(|addr| addr.parse().ok())
            .collect();

        if to_addresses.is_empty() {
            return Err("没有有效的收件人地址".to_string());
        }

        let mut email_builder = Message::builder()
            .from(from.clone())
            .subject(subject);

        for to in &to_addresses {
            email_builder = email_builder.to(to.clone());
        }

        let email = email_builder
            .body(body.to_string())
            .map_err(|e| format!("邮件构建失败: {}", e))?;

        let creds = Credentials::new(
            self.smtp.username.clone(),
            self.smtp.auth_code.clone(),
        );

        let mailer: AsyncSmtpTransport<Tokio1Executor> =
            AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&self.smtp.host)
                .port(self.smtp.port)
                .credentials(creds)
                .build();

        mailer
            .send(email)
            .await
            .map_err(|e| format!("SMTP 发送失败: {}", e))?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::monitor::ChangeType;
    use chrono::Utc;

    fn make_mailer() -> Mailer {
        Mailer::new(
            SmtpConfig {
                host: "smtp.test.com".into(),
                port: 465,
                username: "test@test.com".into(),
                auth_code: "secret".into(),
                from_name: "Test".into(),
            },
            NotificationConfig {
                to: vec!["admin@test.com".into()],
                subject_prefix: "[Test]".into(),
            },
        )
    }

    #[test]
    fn test_alert_body_contains_file_info() {
        let mailer = make_mailer();
        let changes = vec![FileChange {
            path: PathBuf::from("/etc/hosts"),
            change_type: ChangeType::Modified,
            old_hash: "aaa".into(),
            new_hash: "bbb".into(),
            timestamp: Utc::now(),
        }];
        let (subject, body) = mailer.build_alert_body(&changes);
        assert!(subject.contains("[Test]"));
        assert!(subject.contains("1 个文件"));
        assert!(body.contains("/etc/hosts"));
        assert!(body.contains("已修改"));
        assert!(body.contains("aaa"));
        assert!(body.contains("bbb"));
    }

    #[test]
    fn test_alert_body_deleted_file() {
        let mailer = make_mailer();
        let changes = vec![FileChange {
            path: PathBuf::from("/tmp/gone.txt"),
            change_type: ChangeType::Deleted,
            old_hash: "oldhash".into(),
            new_hash: "<deleted>".into(),
            timestamp: Utc::now(),
        }];
        let (_, body) = mailer.build_alert_body(&changes);
        assert!(body.contains("已删除"));
    }

    #[test]
    fn test_startup_report_contains_file_list() {
        let mailer = make_mailer();
        let files = vec![
            (PathBuf::from("/etc/a"), "hashA".into()),
            (PathBuf::from("/etc/b"), "hashB".into()),
        ];
        let (subject, body) = mailer.build_startup_body(&files);
        assert!(subject.contains("监控已启动"));
        assert!(subject.contains("2 个文件"));
        assert!(body.contains("/etc/a"));
        assert!(body.contains("hashA"));
        assert!(body.contains("/etc/b"));
        assert!(body.contains("hashB"));
    }
}
```

- [ ] **Step 2: Build**

Run: `cargo build 2>&1`

Expected: compilation succeeds.

- [ ] **Step 3: Run tests**

Run: `cargo test 2>&1`

Expected: 3 mailer tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/mailer.rs
git commit -m "feat: add mailer module with SMTP email via lettre"
```

---

### Task 5: Main Entry Point (CLI, Init, Loop, Signal Handling)

**Files:**
- Create: `src/main.rs`

**Interfaces:**
- Consumes: `config::load_config`, `store::load_baseline / save_baseline`, `monitor::init_baseline / scan_and_compare`, `mailer::Mailer`
- Produces: binary entry point
- CLI: `file_monitor [-c <path>] [-v]`, default config `~/.file_monitor/config.toml`
- Logging: env_logger, INFO default, DEBUG with `-v`
- Startup: load config → ensure `~/.file_monitor/` exists → init baseline → save → send startup report
- Loop: sleep interval → scan_and_compare → if changes, send_alert → save new baseline → repeat
- Signal: ctrlc handler sets AtomicBool, main loop checks it, flushes baseline, exits

- [ ] **Step 1: Create `src/main.rs`**

Write `src/main.rs`:

```rust
mod config;
mod mailer;
mod monitor;
mod store;

use clap::Parser;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[derive(Parser)]
#[command(name = "file_monitor", about = "文件完整性校验守护进程")]
struct Cli {
    /// 配置文件路径
    #[arg(short = 'c', long = "config", default_value = default_config())]
    config: PathBuf,

    /// 详细日志输出
    #[arg(short = 'v', long = "verbose")]
    verbose: bool,
}

fn default_config() -> String {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    format!("{}/.file_monitor/config.toml", home)
}

fn data_dir_from_config(config_path: &std::path::Path) -> PathBuf {
    config_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .to_path_buf()
}

fn main() {
    let cli = Cli::parse();

    // Init logging
    let log_level = if cli.verbose { "debug" } else { "info" };
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or(log_level),
    )
    .format_timestamp_secs()
    .init();

    // Load config
    let cfg = config::load_config(&cli.config).unwrap_or_else(|e| {
        log::error!("无法加载配置文件 {}: {}", cli.config.display(), e);
        std::process::exit(1);
    });

    let data_dir = data_dir_from_config(&cli.config);
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
```

- [ ] **Step 2: Build**

Run: `cargo build 2>&1`

Expected: compilation succeeds.

- [ ] **Step 3: Commit**

```bash
git add src/main.rs
git commit -m "feat: add main entry point with CLI, polling loop, and signal handling"
```

---

### Task 6: Integration Test

**Files:**
- Create: `tests/integration_test.rs`

**Interfaces:**
- End-to-end test: create temp config + temp files → run init baseline → modify a file → scan → verify change detected → verify baseline updated
- Does NOT test real SMTP sending (mail body verification is in mailer unit tests)

- [ ] **Step 1: Create integration test**

Write `tests/integration_test.rs`:

```rust
use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;

// Integration test: full pipeline from config → monitor → baseline persistence
#[test]
fn test_full_monitor_pipeline() {
    // Setup temp directory structure
    let dir = tempfile::tempdir().unwrap();
    let config_dir = dir.path().join(".file_monitor");
    std::fs::create_dir_all(&config_dir).unwrap();
    let store_path = config_dir.join("store.json");

    // Create monitored files
    let file_a = dir.path().join("a.txt");
    let file_b = dir.path().join("b.txt");
    std::fs::write(&file_a, b"content A").unwrap();
    std::fs::write(&file_b, b"content B").unwrap();

    let file_paths = vec![file_a.clone(), file_b.clone()];

    // --- Phase 1: Init baseline ---
    let baseline =
        file_monitor::monitor::init_baseline(&file_paths).expect("init baseline should succeed");
    assert_eq!(baseline.len(), 2);

    file_monitor::store::save_baseline(&store_path, &baseline)
        .expect("save baseline should succeed");

    // Reload and verify
    let loaded = file_monitor::store::load_baseline(&store_path)
        .expect("load baseline should succeed");
    assert_eq!(loaded.len(), 2);
    assert!(loaded.contains_key(&file_a));
    assert!(loaded.contains_key(&file_b));

    // --- Phase 2: No changes → empty ---
    let (changes, _) = file_monitor::monitor::scan_and_compare(&file_paths, &loaded);
    assert!(changes.is_empty(), "no changes expected right after init");

    // --- Phase 3: Modify file_a → detected ---
    std::fs::write(&file_a, b"content A modified").unwrap();
    let (changes, new_baseline) =
        file_monitor::monitor::scan_and_compare(&file_paths, &loaded);

    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].path, file_a);
    assert!(matches!(
        changes[0].change_type,
        file_monitor::monitor::ChangeType::Modified
    ));

    // Persist new baseline and verify file_a hash updated
    file_monitor::store::save_baseline(&store_path, &new_baseline)
        .expect("save updated baseline should succeed");
    let reloaded = file_monitor::store::load_baseline(&store_path).unwrap();
    assert_ne!(
        reloaded.get(&file_a).unwrap().hash,
        loaded.get(&file_a).unwrap().hash,
        "hash should differ after modification"
    );
    assert_eq!(
        reloaded.get(&file_b).unwrap().hash,
        loaded.get(&file_b).unwrap().hash,
        "unchanged file hash should stay the same"
    );

    // --- Phase 4: Delete file_b → detected ---
    std::fs::remove_file(&file_b).unwrap();
    let (changes, final_baseline) =
        file_monitor::monitor::scan_and_compare(&file_paths, &reloaded);

    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].path, file_b);
    assert!(matches!(
        changes[0].change_type,
        file_monitor::monitor::ChangeType::Deleted
    ));
    // Deleted file removed from baseline
    assert!(!final_baseline.contains_key(&file_b));

    // --- Phase 5: store.json corruption recovery ---
    std::fs::write(&store_path, "garbage {{{").unwrap();
    let recovered = file_monitor::store::load_baseline(&store_path).unwrap();
    assert!(recovered.is_empty(), "corrupted baseline should return empty");
}
```

Note: integration tests require the library target. Update `Cargo.toml` to include `[lib]`:

```toml
[lib]
name = "file_monitor"
path = "src/main.rs"
```

Actually, main.rs can't serve as both binary and library easily since it has `fn main()`. A cleaner approach: add a `src/lib.rs` that re-exports modules.

- [ ] **Step 2: Create `src/lib.rs` for integration test access**

Write `src/lib.rs`:

```rust
pub mod config;
pub mod mailer;
pub mod monitor;
pub mod store;
```

- [ ] **Step 3: Update `src/main.rs` to use `use file_monitor::*` for modules**

Replace the `mod` declarations at the top of `src/main.rs`:

```rust
use file_monitor::{config, mailer, monitor, store};
```

And remove these 4 lines:
```
mod config;
mod mailer;
mod monitor;
mod store;
```

- [ ] **Step 4: Build**

Run: `cargo build 2>&1`

Expected: compilation succeeds.

- [ ] **Step 5: Run all tests including integration**

Run: `cargo test 2>&1`

Expected: all unit tests + integration test pass.

- [ ] **Step 6: Commit**

```bash
git add src/lib.rs src/main.rs tests/ Cargo.toml
git commit -m "feat: add integration tests and lib target"
```

---

### Final Step: Build Release Binary

```bash
cargo build --release
```

Binary at `target/release/file_monitor`.
