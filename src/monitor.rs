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

        // Ensure mtime changes on filesystems with 1-second granularity
        std::thread::sleep(std::time::Duration::from_secs(1));

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
