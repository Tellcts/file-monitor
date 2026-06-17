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
