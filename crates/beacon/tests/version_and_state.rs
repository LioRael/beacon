use beacon::{
    Manager,
    config::Config,
    store::Store,
    versions::{manager_for_executable, version_number},
};

#[test]
fn parses_versions_from_supported_tool_outputs() {
    assert_eq!(version_number("node v26.5.0"), Some("26.5.0".into()));
    assert_eq!(
        version_number("go version go1.26.5 darwin/arm64"),
        Some("1.26.5".into())
    );
    assert_eq!(
        version_number("rustc 1.98.0-nightly (abc)"),
        Some("1.98.0-nightly".into())
    );
}

#[test]
fn default_config_matches_v01_policy() {
    let config = Config::default();
    assert!(config.enabled_tools.contains(&"pnpm".into()));
    assert_eq!(config.preferred_install_manager, "homebrew");
    assert!(config.command_timeout_seconds > 0);
}

#[test]
fn sqlite_history_is_newest_first_and_prunable() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(&directory.path().join("beacon.db")).unwrap();
    store
        .record(
            "check",
            "node",
            None,
            Some("1"),
            Manager::Homebrew,
            "success",
            "first",
        )
        .unwrap();
    store
        .record(
            "upgrade",
            "node",
            Some("1"),
            Some("2"),
            Manager::Homebrew,
            "success",
            "second",
        )
        .unwrap();
    store.prune(1).unwrap();
    let history = store.history(10).unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].summary, "second");
}

#[test]
fn manager_detection_resolves_executable_symlinks() {
    let directory = tempfile::tempdir().unwrap();
    let target_dir = directory.path().join("homebrew/bin");
    let link_dir = directory.path().join("usr/local/bin");
    std::fs::create_dir_all(&target_dir).unwrap();
    std::fs::create_dir_all(&link_dir).unwrap();
    let target = target_dir.join("npm");
    let link = link_dir.join("npm");
    std::fs::write(&target, "").unwrap();
    std::os::unix::fs::symlink(&target, &link).unwrap();

    assert_eq!(manager_for_executable(&link), Manager::Homebrew);
}
