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
    let loaded =
        file_monitor::store::load_baseline(&store_path).expect("load baseline should succeed");
    assert_eq!(loaded.len(), 2);
    assert!(loaded.contains_key(&file_a));
    assert!(loaded.contains_key(&file_b));

    // --- Phase 2: No changes → empty ---
    let (changes, _) = file_monitor::monitor::scan_and_compare(&file_paths, &loaded);
    assert!(changes.is_empty(), "no changes expected right after init");

    // --- Phase 3: Modify file_a → detected ---
    std::fs::write(&file_a, b"content A modified").unwrap();
    let (changes, new_baseline) = file_monitor::monitor::scan_and_compare(&file_paths, &loaded);

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
    let (changes, final_baseline) = file_monitor::monitor::scan_and_compare(&file_paths, &reloaded);

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
    assert!(
        recovered.is_empty(),
        "corrupted baseline should return empty"
    );
}
