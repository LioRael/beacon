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
fn default_config_is_fresh_v2_with_tools_and_homebrew_inventory() {
    let config = Config::default();
    assert_eq!(config.schema_version, 2);
    assert_eq!(
        config.enabled_tools,
        ["rust", "node", "npm", "pnpm", "go", "bun", "deno", "uv"]
    );
    assert_eq!(config.enabled_inventories, ["homebrew"]);
    assert_eq!(config.history_limit, 500);
    assert_eq!(config.command_timeout_seconds, 120);
}

#[test]
fn ensure_writes_fresh_v2_config_when_missing() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.toml");

    let (config, returned) = beacon::config::ensure_at(&path).unwrap();
    let written = std::fs::read_to_string(&path).unwrap();

    assert_eq!(returned, path);
    assert_eq!(config.schema_version, 2);
    assert_eq!(
        config.enabled_tools,
        ["rust", "node", "npm", "pnpm", "go", "bun", "deno", "uv"]
    );
    assert_eq!(config.enabled_inventories, ["homebrew"]);
    assert!(written.contains("schema_version = 2"));
    assert!(written.contains("enabled_tools"));
    assert!(written.contains("enabled_inventories"));
    assert!(written.contains("\"bun\""));
    assert!(written.contains("\"deno\""));
    assert!(written.contains("\"uv\""));
    assert!(written.contains("\"homebrew\""));
    assert!(!path.with_file_name("config.toml.tmp").exists());
    assert!(!directory.path().join("config.toml.v1.bak").exists());
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
    assert!(!path.with_file_name("config.toml.tmp").exists());
    assert!(!path.with_file_name("config.toml.v1.bak.tmp").exists());

    let migrated_again = std::fs::read_to_string(&path).unwrap();
    beacon::config::ensure_at(&path).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), migrated_again);
    assert_eq!(
        std::fs::read_to_string(directory.path().join("config.toml.v1.bak")).unwrap(),
        backup
    );
}

#[test]
fn config_v1_migration_moves_homebrew_and_keeps_valid_settings() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.toml");
    let original = "schema_version = 1\nenabled_tools = [\"homebrew\", \"node\", \"rust\"]\npreferred_install_manager = \"mise\"\nhistory_limit = 42\ncommand_timeout_seconds = 30\n";
    std::fs::write(&path, original).unwrap();

    let (config, _) = beacon::config::ensure_at(&path).unwrap();
    let migrated = std::fs::read_to_string(&path).unwrap();
    let backup = std::fs::read_to_string(directory.path().join("config.toml.v1.bak")).unwrap();

    assert_eq!(config.schema_version, 2);
    assert_eq!(config.enabled_tools, ["node", "rust"]);
    assert_eq!(config.enabled_inventories, ["homebrew"]);
    assert_eq!(config.history_limit, 42);
    assert_eq!(config.command_timeout_seconds, 30);
    assert!(migrated.contains("history_limit = 42"));
    assert!(migrated.contains("command_timeout_seconds = 30"));
    assert!(!migrated.contains("preferred_install_manager"));
    assert!(migrated.contains("enabled_inventories = [\"homebrew\"]"));
    assert!(!config.enabled_tools.iter().any(|tool| tool == "homebrew"));
    assert_eq!(backup, original);
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
fn config_v1_migration_without_enabled_tools_uses_fresh_defaults() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.toml");
    let original = "history_limit = 10\n";
    std::fs::write(&path, original).unwrap();

    let load_error = beacon::config::load_from(&path).unwrap_err().to_string();
    assert!(
        load_error.contains("requires migration"),
        "unmigrated v1 must not deserialize as ready v2: {load_error}"
    );

    let (config, _) = beacon::config::ensure_at(&path).unwrap();
    let migrated = std::fs::read_to_string(&path).unwrap();
    let backup = std::fs::read_to_string(directory.path().join("config.toml.v1.bak")).unwrap();

    assert_eq!(config.schema_version, 2);
    assert_eq!(config.enabled_tools, Config::default().enabled_tools);
    assert_eq!(config.enabled_inventories, ["homebrew"]);
    assert_eq!(config.history_limit, 10);
    assert!(migrated.contains("schema_version = 2"));
    assert!(migrated.contains("enabled_tools"));
    assert!(migrated.contains("enabled_inventories = [\"homebrew\"]"));
    assert!(migrated.contains("\"bun\""));
    assert!(migrated.contains("\"deno\""));
    assert!(migrated.contains("\"uv\""));
    assert_eq!(backup, original);
    assert!(!path.with_file_name("config.toml.tmp").exists());
    assert!(!path.with_file_name("config.toml.v1.bak.tmp").exists());
}

#[test]
fn future_config_schema_is_rejected_without_rewrite() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.toml");
    let original = "schema_version = 99\nenabled_tools = [\"node\"]\ncustom_key = \"keep\"\n";
    std::fs::write(&path, original).unwrap();

    let ensure_error = beacon::config::ensure_at(&path).unwrap_err().to_string();
    let load_error = beacon::config::load_from(&path).unwrap_err().to_string();

    assert!(ensure_error.contains("unsupported Beacon config schema version 99"));
    assert!(load_error.contains("unsupported Beacon config schema version 99"));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
    assert!(!directory.path().join("config.toml.v1.bak").exists());
    assert!(!path.with_file_name("config.toml.tmp").exists());
    assert!(!path.with_file_name("config.toml.v1.bak.tmp").exists());
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
