use beacon::{CheckData, config::Config, store::Store, versions::version_number};

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
fn sqlite_fresh_database_is_user_version_2_without_manager_column() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("beacon.db");
    let store = Store::open(&path).unwrap();

    assert_eq!(store.schema_version().unwrap(), 2);
    store
        .record(
            "upgrade",
            "node",
            Some("1"),
            Some("2"),
            "mise",
            "npm",
            "success",
            "fresh",
        )
        .unwrap();
    let history = store.history(1).unwrap();
    assert_eq!(history[0].installation_source, "mise");
    assert_eq!(history[0].update_manager, "npm");
    assert!(!history_json_has_manager_field(&history[0]));

    let columns = table_columns(&path, "history");
    assert!(columns.contains(&"installation_source".into()));
    assert!(columns.contains(&"update_manager".into()));
    assert!(!columns.contains(&"manager".into()));
    assert!(table_columns(&path, "snapshots").contains(&"payload_schema_version".into()));
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
fn sqlite_v1_migration_backfills_source_unknown_updater_and_preserves_snapshots() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("beacon.db");
    let legacy_payload = r#"[{"id":"node","legacy":true}]"#;
    seed_v1_database(
        &path,
        &[
            ("t1", "check", "node", "mise", "success", "legacy-check"),
            ("t2", "upgrade", "node", "homebrew", "failed", "legacy-fail"),
            ("t3", "upgrade", "go", "mise", "success", "legacy-ok"),
        ],
        legacy_payload,
    );

    let store = Store::open(&path).unwrap();
    let history = store.history(10).unwrap();
    let snapshots = store.snapshots(10).unwrap();

    assert_eq!(store.schema_version().unwrap(), 2);
    assert_eq!(history.len(), 3);
    assert_eq!(history[0].summary, "legacy-ok");
    assert_eq!(history[0].installation_source, "mise");
    assert_eq!(history[0].update_manager, "unknown");
    assert_eq!(history[1].installation_source, "homebrew");
    assert_eq!(history[1].update_manager, "unknown");
    assert_eq!(history[2].installation_source, "mise");
    assert_eq!(history[2].update_manager, "unknown");
    assert!(!history_json_has_manager_field(&history[0]));

    assert_eq!(snapshots.len(), 1);
    assert_eq!(snapshots[0].payload, legacy_payload);
    assert_eq!(snapshots[0].payload_schema_version, 1);

    let columns = table_columns(&path, "history");
    assert!(!columns.contains(&"manager".into()));
    assert!(columns.contains(&"installation_source".into()));
    assert!(columns.contains(&"update_manager".into()));
}

#[test]
fn sqlite_v1_migration_allows_new_records_and_is_idempotent_on_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("beacon.db");
    seed_v1_database(
        &path,
        &[("t1", "check", "node", "mise", "success", "legacy")],
        r#"{"tools":[]}"#,
    );

    let store = Store::open(&path).unwrap();
    store
        .record(
            "upgrade",
            "node",
            Some("1.0.0"),
            Some("1.1.0"),
            "mise",
            "npm",
            "success",
            "post-migration",
        )
        .unwrap();
    store
        .record(
            "upgrade",
            "node",
            Some("1.0.0"),
            None,
            "mise",
            "npm",
            "failed",
            "upgrade failed: drift. Inspect PATH.",
        )
        .unwrap();
    store
        .snapshot(&CheckData {
            tools: vec![],
            inventories: vec![],
        })
        .unwrap();

    let history = store.history(10).unwrap();
    assert_eq!(history.len(), 3);
    assert_eq!(history[0].status, "failed");
    assert_eq!(history[0].installation_source, "mise");
    assert_eq!(history[0].update_manager, "npm");
    assert_eq!(history[1].status, "success");
    assert_eq!(history[1].installation_source, "mise");
    assert_eq!(history[1].update_manager, "npm");
    assert_eq!(history[2].installation_source, "mise");
    assert_eq!(history[2].update_manager, "unknown");

    let snapshots = store.snapshots(10).unwrap();
    assert_eq!(snapshots.len(), 2);
    assert_eq!(snapshots[0].payload_schema_version, 2);
    assert_eq!(snapshots[1].payload_schema_version, 1);
    assert_eq!(snapshots[1].payload, r#"{"tools":[]}"#);

    drop(store);
    let reopened = Store::open(&path).unwrap();
    assert_eq!(reopened.schema_version().unwrap(), 2);
    let history_again = reopened.history(10).unwrap();
    assert_eq!(history_again.len(), 3);
    assert_eq!(history_again[0].summary, history[0].summary);
    assert_eq!(
        history_again[2].installation_source,
        history[2].installation_source
    );
    assert_eq!(history_again[2].update_manager, history[2].update_manager);
    let snapshots_again = reopened.snapshots(10).unwrap();
    assert_eq!(snapshots_again[1].payload, r#"{"tools":[]}"#);
    assert_eq!(snapshots_again[1].payload_schema_version, 1);
}

#[test]
fn sqlite_v1_migration_preserves_history_order_limit_and_pruning() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("beacon.db");
    seed_v1_database(
        &path,
        &[
            ("t1", "check", "node", "homebrew", "success", "one"),
            ("t2", "check", "node", "homebrew", "success", "two"),
            ("t3", "check", "node", "homebrew", "success", "three"),
        ],
        "[]",
    );

    let store = Store::open(&path).unwrap();
    let limited = store.history(2).unwrap();
    assert_eq!(limited.len(), 2);
    assert_eq!(limited[0].summary, "three");
    assert_eq!(limited[1].summary, "two");

    store.prune(1).unwrap();
    let pruned = store.history(10).unwrap();
    assert_eq!(pruned.len(), 1);
    assert_eq!(pruned[0].summary, "three");
    assert_eq!(pruned[0].installation_source, "homebrew");
    assert_eq!(pruned[0].update_manager, "unknown");
}

#[test]
fn sqlite_heals_broken_v2_stamp_that_still_has_manager_column() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("beacon.db");
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE history (
                id INTEGER PRIMARY KEY,
                created_at TEXT NOT NULL,
                operation TEXT NOT NULL,
                tool TEXT NOT NULL,
                old_version TEXT,
                new_version TEXT,
                manager TEXT NOT NULL,
                status TEXT NOT NULL,
                summary TEXT NOT NULL,
                installation_source TEXT NOT NULL DEFAULT 'unknown',
                update_manager TEXT NOT NULL DEFAULT 'unknown'
            );
            CREATE TABLE snapshots (
                id INTEGER PRIMARY KEY,
                created_at TEXT NOT NULL,
                payload TEXT NOT NULL,
                payload_schema_version INTEGER NOT NULL DEFAULT 1
            );
            INSERT INTO history(
                created_at,operation,tool,manager,status,summary,installation_source,update_manager
            ) VALUES(
                'now','check','node','mise','success','hybrid','unknown','unknown'
            );
            INSERT INTO snapshots(created_at,payload,payload_schema_version)
            VALUES('now','[]',1);
            PRAGMA user_version = 2;",
        )
        .unwrap();
    drop(connection);

    let store = Store::open(&path).unwrap();
    assert_eq!(store.schema_version().unwrap(), 2);
    let history = store.history(1).unwrap();
    assert_eq!(history[0].installation_source, "mise");
    assert_eq!(history[0].update_manager, "unknown");
    store
        .record(
            "upgrade",
            "node",
            Some("1"),
            Some("2"),
            "mise",
            "npm",
            "success",
            "healed",
        )
        .expect("healed v2 schema must accept new records without manager column");
    assert!(!table_columns(&path, "history").contains(&"manager".into()));
    assert_eq!(store.history(1).unwrap()[0].summary, "healed");
}

#[test]
fn sqlite_future_schema_is_rejected_without_rewrite() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("beacon.db");
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE history (
                id INTEGER PRIMARY KEY,
                created_at TEXT NOT NULL,
                operation TEXT NOT NULL,
                tool TEXT NOT NULL,
                old_version TEXT,
                new_version TEXT,
                manager TEXT NOT NULL,
                status TEXT NOT NULL,
                summary TEXT NOT NULL
            );
            PRAGMA user_version = 99;
            INSERT INTO history(created_at,operation,tool,manager,status,summary)
            VALUES('now','check','node','mise','success','future');",
        )
        .unwrap();
    drop(connection);

    let error = match Store::open(&path) {
        Ok(_) => panic!("future schema must be rejected"),
        Err(error) => error.to_string(),
    };
    assert!(
        error.contains("unsupported Beacon database schema version 99"),
        "unexpected error: {error}"
    );

    let connection = rusqlite::Connection::open(&path).unwrap();
    let version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, 99);
    let manager: String = connection
        .query_row(
            "SELECT manager FROM history WHERE summary = 'future'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(manager, "mise");
    let columns = table_columns(&path, "history");
    assert!(columns.contains(&"manager".into()));
    assert!(!columns.contains(&"installation_source".into()));
}

#[test]
fn sqlite_migration_rolls_back_when_history_table_is_corrupt() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("beacon.db");
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE history (id INTEGER PRIMARY KEY);
             CREATE TABLE snapshots (id INTEGER PRIMARY KEY, created_at TEXT NOT NULL, payload TEXT NOT NULL);
             INSERT INTO history(id) VALUES(1);",
        )
        .unwrap();
    drop(connection);

    let error = match Store::open(&path) {
        Ok(_) => panic!("corrupt history must fail migration"),
        Err(error) => error.to_string(),
    };
    assert!(
        error.contains("history") || error.contains("column") || error.contains("migrate"),
        "expected migration failure, got: {error}"
    );

    let connection = rusqlite::Connection::open(&path).unwrap();
    let version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, 0);
    let count: i64 = connection
        .query_row("SELECT COUNT(*) FROM history", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 1);
    let columns = table_columns(&path, "history");
    assert_eq!(columns, vec!["id".to_string()]);
}

fn seed_v1_database(
    path: &std::path::Path,
    rows: &[(&str, &str, &str, &str, &str, &str)],
    snapshot_payload: &str,
) {
    let connection = rusqlite::Connection::open(path).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE history (
                id INTEGER PRIMARY KEY,
                created_at TEXT NOT NULL,
                operation TEXT NOT NULL,
                tool TEXT NOT NULL,
                old_version TEXT,
                new_version TEXT,
                manager TEXT NOT NULL,
                status TEXT NOT NULL,
                summary TEXT NOT NULL
            );
            CREATE TABLE snapshots (
                id INTEGER PRIMARY KEY,
                created_at TEXT NOT NULL,
                payload TEXT NOT NULL
            );",
        )
        .unwrap();
    for (created_at, operation, tool, manager, status, summary) in rows {
        connection
            .execute(
                "INSERT INTO history(created_at,operation,tool,manager,status,summary)
                 VALUES(?,?,?,?,?,?)",
                rusqlite::params![created_at, operation, tool, manager, status, summary],
            )
            .unwrap();
    }
    connection
        .execute(
            "INSERT INTO snapshots(created_at,payload) VALUES('now',?)",
            [snapshot_payload],
        )
        .unwrap();
}

fn table_columns(path: &std::path::Path, table: &str) -> Vec<String> {
    let connection = rusqlite::Connection::open(path).unwrap();
    let mut statement = connection
        .prepare(&format!("PRAGMA table_info({table})"))
        .unwrap();
    statement
        .query_map([], |row| row.get::<_, String>(1))
        .unwrap()
        .map(|value| value.unwrap())
        .collect()
}

fn history_json_has_manager_field(entry: &beacon::store::HistoryEntry) -> bool {
    let value = serde_json::to_value(entry).unwrap();
    value.get("manager").is_some()
}
