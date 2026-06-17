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
