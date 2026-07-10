use beacon::{config::Config, store::Store, versions::version_number};

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
    assert_eq!(config.schema_version, 2);
    assert!(config.enabled_tools.contains(&"pnpm".into()));
    assert!(config.enabled_tools.contains(&"bun".into()));
    assert_eq!(config.enabled_inventories, ["homebrew"]);
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
            "homebrew",
            "unknown",
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
            "homebrew",
            "homebrew",
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
fn config_v1_migration_preserves_comments_and_unknown_keys() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.toml");
    std::fs::write(
        &path,
        "# keep this comment\nenabled_tools = [\"homebrew\", \"node\"]\npreferred_install_manager = \"homebrew\"\ncustom_key = \"keep\"\n",
    )
    .unwrap();

    let (config, _) = beacon::config::ensure_at(&path).unwrap();
    let migrated = std::fs::read_to_string(&path).unwrap();
    let backup = std::fs::read_to_string(directory.path().join("config.toml.v1.bak")).unwrap();

    assert_eq!(config.schema_version, 2);
    assert_eq!(config.enabled_tools, ["node"]);
    assert_eq!(config.enabled_inventories, ["homebrew"]);
    assert!(migrated.contains("# keep this comment"));
    assert!(migrated.contains("custom_key = \"keep\""));
    assert!(!migrated.contains("preferred_install_manager"));
    assert!(backup.contains("preferred_install_manager"));

    beacon::config::ensure_at(&path).unwrap();
    assert_eq!(
        std::fs::read_to_string(directory.path().join("config.toml.v1.bak")).unwrap(),
        backup
    );
}

#[test]
fn config_v1_migration_preserves_disabled_homebrew_inventory() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.toml");
    std::fs::write(&path, "enabled_tools = [\"node\"]\n").unwrap();

    let (config, _) = beacon::config::ensure_at(&path).unwrap();

    assert!(config.enabled_inventories.is_empty());
}

#[test]
fn sqlite_v1_migration_backfills_source_and_unknown_updater() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("beacon.db");
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection.execute_batch("CREATE TABLE history (id INTEGER PRIMARY KEY, created_at TEXT NOT NULL, operation TEXT NOT NULL, tool TEXT NOT NULL, old_version TEXT, new_version TEXT, manager TEXT NOT NULL, status TEXT NOT NULL, summary TEXT NOT NULL); CREATE TABLE snapshots (id INTEGER PRIMARY KEY, created_at TEXT NOT NULL, payload TEXT NOT NULL); INSERT INTO history(created_at,operation,tool,manager,status,summary) VALUES('now','check','node','mise','success','legacy'); INSERT INTO snapshots(created_at,payload) VALUES('now','[]');").unwrap();
    drop(connection);

    let store = Store::open(&path).unwrap();
    let history = store.history(1).unwrap();

    assert_eq!(store.schema_version().unwrap(), 2);
    assert_eq!(history[0].installation_source, "mise");
    assert_eq!(history[0].update_manager, "unknown");
}
