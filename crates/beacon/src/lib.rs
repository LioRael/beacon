use serde::{Deserialize, Serialize};

pub mod command {
    use anyhow::{Result, bail};
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    pub struct CommandSpec {
        pub program: String,
        pub args: Vec<String>,
    }

    impl CommandSpec {
        pub fn new(
            program: impl Into<String>,
            args: impl IntoIterator<Item = impl Into<String>>,
        ) -> Self {
            Self {
                program: program.into(),
                args: args.into_iter().map(Into::into).collect(),
            }
        }

        pub fn brew_upgrade(target: &str) -> Result<Self> {
            if target.trim().is_empty() {
                bail!("Homebrew upgrade requires an explicit target");
            }
            Ok(Self::new("brew", ["upgrade", target]))
        }

        pub fn display(&self) -> String {
            std::iter::once(self.program.as_str())
                .chain(self.args.iter().map(String::as_str))
                .map(|part| {
                    if part.contains([' ', '\'', '"']) {
                        format!("{:?}", part)
                    } else {
                        part.to_string()
                    }
                })
                .collect::<Vec<_>>()
                .join(" ")
        }
    }
}

pub mod envelope {
    use serde::Serialize;

    #[derive(Debug, Serialize)]
    pub struct Envelope<T: Serialize> {
        pub schema_version: u8,
        pub status: &'static str,
        pub data: T,
        pub errors: Vec<String>,
    }

    impl<T: Serialize> Envelope<T> {
        pub fn ok(data: T) -> Self {
            Self {
                schema_version: 1,
                status: "ok",
                data,
                errors: vec![],
            }
        }
        pub fn error(data: T, errors: Vec<String>) -> Self {
            Self {
                schema_version: 1,
                status: "error",
                data,
                errors,
            }
        }
    }
}

pub mod redact {
    pub fn redact(input: &str, home: Option<&str>) -> String {
        let mut output = input.to_string();
        if let Some(home) = home {
            if !home.is_empty() {
                output = output.replace(home, "~");
            }
        }
        for marker in [
            "token=",
            "TOKEN=",
            "password=",
            "PASSWORD=",
            "cookie=",
            "COOKIE=",
        ] {
            let mut cursor = 0;
            while let Some(relative) = output[cursor..].find(marker) {
                let start = cursor + relative;
                let value_start = start + marker.len();
                let end = output[value_start..]
                    .find(char::is_whitespace)
                    .map(|i| value_start + i)
                    .unwrap_or(output.len());
                output.replace_range(value_start..end, "[REDACTED]");
                cursor = value_start + "[REDACTED]".len();
            }
        }
        for marker in ["Bearer ", "Basic "] {
            let mut cursor = 0;
            while let Some(relative) = output[cursor..].find(marker) {
                let start = cursor + relative;
                let value_start = start + marker.len();
                let end = output[value_start..]
                    .find(char::is_whitespace)
                    .map(|i| value_start + i)
                    .unwrap_or(output.len());
                output.replace_range(value_start..end, "[REDACTED]");
                cursor = value_start + "[REDACTED]".len();
            }
        }
        output
    }
}

pub mod versions {
    use crate::Manager;
    use std::path::Path;

    pub fn classify_manager(path: &str) -> Manager {
        if path.contains("/homebrew/") {
            Manager::Homebrew
        } else if path.contains("/mise/") {
            Manager::Mise
        } else if path.contains("/.cargo/") {
            Manager::Rustup
        } else {
            Manager::Unknown
        }
    }

    pub fn version_number(output: &str) -> Option<String> {
        output.split_whitespace().find_map(|word| {
            let trimmed = word
                .trim_start_matches('v')
                .strip_prefix("go")
                .unwrap_or(word.trim_start_matches('v'))
                .trim_matches(|c: char| {
                    !(c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '+')
                });
            (trimmed.chars().next()?.is_ascii_digit() && trimmed.contains('.'))
                .then(|| trimmed.to_string())
        })
    }

    pub fn manager_for_executable(path: &Path) -> Manager {
        classify_manager(&path.to_string_lossy())
    }
}

pub mod orchestrator {
    use anyhow::Result;
    use std::future::Future;

    pub async fn run_until_failure<I, T, F, Fut>(items: I, mut run: F) -> Result<()>
    where
        I: IntoIterator<Item = T>,
        F: FnMut(T) -> Fut,
        Fut: Future<Output = Result<()>>,
    {
        for item in items {
            run(item).await?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Manager {
    Homebrew,
    Rustup,
    Mise,
    Npm,
    Unknown,
}

impl std::fmt::Display for Manager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", format!("{self:?}").to_lowercase())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolStatus {
    Current,
    Outdated,
    Missing,
    Unavailable,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolReport {
    pub id: String,
    pub name: String,
    pub current: Option<String>,
    pub latest: Option<String>,
    pub status: ToolStatus,
    pub manager: Manager,
    pub executable: Option<String>,
    pub other_sources: Vec<String>,
    pub detail: Option<String>,
    pub action: Option<command::CommandSpec>,
}

pub mod config {
    use anyhow::{Context, Result};
    use directories::ProjectDirs;
    use serde::{Deserialize, Serialize};
    use std::{
        fs,
        path::{Path, PathBuf},
    };

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(default)]
    pub struct Config {
        pub enabled_tools: Vec<String>,
        pub history_limit: usize,
        pub command_timeout_seconds: u64,
        pub preferred_install_manager: String,
    }

    impl Default for Config {
        fn default() -> Self {
            Self {
                enabled_tools: vec![
                    "homebrew".into(),
                    "rust".into(),
                    "node".into(),
                    "npm".into(),
                    "pnpm".into(),
                    "go".into(),
                ],
                history_limit: 500,
                command_timeout_seconds: 120,
                preferred_install_manager: "homebrew".into(),
            }
        }
    }

    pub fn app_dir() -> Result<PathBuf> {
        let home = ProjectDirs::from("dev", "Beacon", "Beacon")
            .context("cannot determine application directory")?
            .data_dir()
            .ancestors()
            .nth(1)
            .context("cannot determine Application Support directory")?
            .to_path_buf();
        Ok(home.join("Beacon"))
    }
    pub fn path() -> Result<PathBuf> {
        Ok(app_dir()?.join("config.toml"))
    }
    pub fn load_from(path: &Path) -> Result<Config> {
        if !path.exists() {
            return Ok(Config::default());
        }
        toml::from_str(&fs::read_to_string(path)?).context("invalid Beacon config")
    }
    pub fn load() -> Result<Config> {
        load_from(&path()?)
    }
    pub fn save_to(config: &Config, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, toml::to_string_pretty(config)?)?;
        Ok(())
    }
    pub fn ensure() -> Result<(Config, PathBuf)> {
        let path = path()?;
        let config = load_from(&path)?;
        if !path.exists() {
            save_to(&config, &path)?;
        }
        Ok((config, path))
    }
}

pub mod runner {
    use crate::command::CommandSpec;
    use anyhow::{Context, Result, bail};
    use std::time::Duration;
    use tokio::{process::Command, time::timeout};

    #[derive(Debug)]
    pub struct CommandOutput {
        pub stdout: String,
        pub stderr: String,
    }

    pub async fn run(spec: &CommandSpec, seconds: u64) -> Result<CommandOutput> {
        let mut command = Command::new(&spec.program);
        command.args(&spec.args).kill_on_drop(true);
        let output = timeout(Duration::from_secs(seconds), command.output())
            .await
            .with_context(|| format!("command timed out: {}", spec.display()))??;
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if !output.status.success() {
            bail!("{} failed ({}): {}", spec.display(), output.status, stderr);
        }
        Ok(CommandOutput { stdout, stderr })
    }
}

pub mod store {
    use crate::{Manager, ToolReport};
    use anyhow::Result;
    use chrono::Utc;
    use rusqlite::{Connection, params};
    use serde::Serialize;
    use std::path::Path;

    #[derive(Debug, Serialize)]
    pub struct HistoryEntry {
        pub id: i64,
        pub created_at: String,
        pub operation: String,
        pub tool: String,
        pub old_version: Option<String>,
        pub new_version: Option<String>,
        pub manager: String,
        pub status: String,
        pub summary: String,
    }

    pub struct Store {
        connection: Connection,
    }
    impl Store {
        pub fn open(path: &Path) -> Result<Self> {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let store = Self {
                connection: Connection::open(path)?,
            };
            store.migrate()?;
            Ok(store)
        }
        fn migrate(&self) -> Result<()> {
            self.connection.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE IF NOT EXISTS history (id INTEGER PRIMARY KEY, created_at TEXT NOT NULL, operation TEXT NOT NULL, tool TEXT NOT NULL, old_version TEXT, new_version TEXT, manager TEXT NOT NULL, status TEXT NOT NULL, summary TEXT NOT NULL); CREATE TABLE IF NOT EXISTS snapshots (id INTEGER PRIMARY KEY, created_at TEXT NOT NULL, payload TEXT NOT NULL);")?;
            Ok(())
        }
        #[allow(clippy::too_many_arguments)]
        pub fn record(
            &self,
            operation: &str,
            tool: &str,
            old: Option<&str>,
            new: Option<&str>,
            manager: Manager,
            status: &str,
            summary: &str,
        ) -> Result<()> {
            self.connection.execute("INSERT INTO history(created_at,operation,tool,old_version,new_version,manager,status,summary) VALUES(?,?,?,?,?,?,?,?)", params![Utc::now().to_rfc3339(), operation, tool, old, new, manager.to_string(), status, summary])?;
            Ok(())
        }
        pub fn snapshot(&self, reports: &[ToolReport]) -> Result<()> {
            self.connection.execute(
                "INSERT INTO snapshots(created_at,payload) VALUES(?,?)",
                params![Utc::now().to_rfc3339(), serde_json::to_string(reports)?],
            )?;
            Ok(())
        }
        pub fn history(&self, limit: usize) -> Result<Vec<HistoryEntry>> {
            let mut statement = self.connection.prepare("SELECT id,created_at,operation,tool,old_version,new_version,manager,status,summary FROM history ORDER BY id DESC LIMIT ?")?;
            Ok(statement
                .query_map([limit as i64], |row| {
                    Ok(HistoryEntry {
                        id: row.get(0)?,
                        created_at: row.get(1)?,
                        operation: row.get(2)?,
                        tool: row.get(3)?,
                        old_version: row.get(4)?,
                        new_version: row.get(5)?,
                        manager: row.get(6)?,
                        status: row.get(7)?,
                        summary: row.get(8)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?)
        }
        pub fn prune(&self, limit: usize) -> Result<()> {
            self.connection.execute("DELETE FROM history WHERE id NOT IN (SELECT id FROM history ORDER BY id DESC LIMIT ?)", [limit as i64])?;
            Ok(())
        }
    }
}

pub mod providers;
