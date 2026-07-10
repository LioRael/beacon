use crate::{
    Manager, ToolReport, ToolStatus,
    command::CommandSpec,
    config::Config,
    ui::Ui,
    versions::{manager_for_executable, version_number},
};
use anyhow::{Context, Result};
use serde::Deserialize;
use std::{
    collections::{HashMap, HashSet},
    env,
    path::{Path, PathBuf},
};

#[derive(Debug, Default, Deserialize)]
struct BrewOutdated {
    #[serde(default)]
    formulae: Vec<BrewItem>,
    #[serde(default)]
    casks: Vec<BrewItem>,
}
#[derive(Debug, Deserialize)]
struct BrewItem {
    name: String,
    #[serde(default)]
    installed_versions: Vec<String>,
    current_version: Option<String>,
}

pub fn find_executable(name: &str) -> Option<PathBuf> {
    env::var_os("PATH")?
        .to_string_lossy()
        .split(':')
        .map(|dir| Path::new(dir).join(name))
        .find(|path| path.is_file())
}

async fn output(
    program: &str,
    args: &[&str],
    config: &Config,
    ui: &Ui,
    label: &str,
) -> Result<String> {
    Ok(ui
        .run_command(
            label,
            &CommandSpec::new(program, args.iter().copied()),
            config.command_timeout_seconds,
        )
        .await?
        .stdout)
}

async fn version(
    program: &str,
    args: &[&str],
    config: &Config,
    ui: &Ui,
    label: &str,
) -> Option<String> {
    output(program, args, config, ui, label)
        .await
        .ok()
        .and_then(|text| version_number(&text))
}

async fn other_source(tool: &str, active: Manager, config: &Config, ui: &Ui) -> Vec<String> {
    if active == Manager::Mise || find_executable("mise").is_none() {
        return vec![];
    }
    match output(
        "mise",
        &["ls", tool],
        config,
        ui,
        &format!("Checking alternate {tool} installations"),
    )
    .await
    {
        Ok(text) if !text.trim().is_empty() && !text.contains("No versions") => vec![format!(
            "mise: {}",
            text.lines().next().unwrap_or_default().trim()
        )],
        _ => vec![],
    }
}

fn status(current: Option<&String>, latest: Option<&String>) -> ToolStatus {
    match (current, latest) {
        (None, _) => ToolStatus::Missing,
        (Some(a), Some(b)) if a != b => ToolStatus::Outdated,
        (Some(_), _) => ToolStatus::Current,
    }
}

#[allow(clippy::too_many_arguments)]
fn report(
    id: &str,
    name: &str,
    current: Option<String>,
    latest: Option<String>,
    manager: Manager,
    executable: Option<PathBuf>,
    other_sources: Vec<String>,
    action: Option<CommandSpec>,
) -> ToolReport {
    let status = status(current.as_ref(), latest.as_ref());
    ToolReport {
        id: id.into(),
        name: name.into(),
        status,
        current,
        latest,
        manager,
        executable: executable.map(|p| p.to_string_lossy().to_string()),
        other_sources,
        detail: None,
        action: (status != ToolStatus::Missing).then_some(action).flatten(),
    }
}

fn npm_report(
    executable: Option<PathBuf>,
    current: Option<String>,
    latest: Option<String>,
) -> ToolReport {
    let manager = executable
        .as_deref()
        .map(manager_for_executable)
        .unwrap_or(Manager::Unknown);
    report(
        "npm",
        "npm",
        current,
        latest,
        manager,
        executable,
        vec![],
        Some(CommandSpec::new(
            "npm",
            ["install", "--global", "npm@latest"],
        )),
    )
}

pub async fn check_all(config: &Config, refresh: bool, ui: &Ui) -> Result<Vec<ToolReport>> {
    let mut reports = Vec::new();
    let brew_available = find_executable("brew").is_some();
    let mut brew_items = HashMap::new();
    if brew_available {
        if refresh {
            output(
                "brew",
                &["update"],
                config,
                ui,
                "Refreshing Homebrew metadata",
            )
            .await
            .context("Homebrew refresh failed")?;
        }
        let json = output(
            "brew",
            &["outdated", "--json=v2"],
            config,
            ui,
            "Checking Homebrew packages",
        )
        .await?;
        let outdated: BrewOutdated =
            serde_json::from_str(&json).context("invalid Homebrew outdated response")?;
        for item in outdated.formulae.into_iter().chain(outdated.casks) {
            brew_items.insert(item.name.clone(), item);
        }
    }

    if config.enabled_tools.iter().any(|t| t == "homebrew") {
        if brew_available {
            for item in brew_items.values() {
                let current = item.installed_versions.last().cloned();
                reports.push(report(
                    &format!("brew:{}", item.name),
                    &item.name,
                    current,
                    item.current_version.clone(),
                    Manager::Homebrew,
                    find_executable("brew"),
                    vec![],
                    CommandSpec::brew_upgrade(&item.name).ok(),
                ));
            }
            if brew_items.is_empty() {
                reports.push(report(
                    "homebrew",
                    "Homebrew packages",
                    Some("installed".into()),
                    Some("installed".into()),
                    Manager::Homebrew,
                    find_executable("brew"),
                    vec![],
                    None,
                ));
            }
        } else {
            reports.push(ToolReport {
                id: "homebrew".into(),
                name: "Homebrew".into(),
                current: None,
                latest: None,
                status: ToolStatus::Unavailable,
                manager: Manager::Unknown,
                executable: None,
                other_sources: vec![],
                detail: Some("brew is not available on PATH".into()),
                action: None,
            });
        }
    }

    if config.enabled_tools.iter().any(|t| t == "rust") {
        let executable = find_executable("rustc");
        if executable.is_some() && find_executable("rustup").is_some() {
            let current =
                version("rustc", &["--version"], config, ui, "Reading Rust version").await;
            let active = output(
                "rustup",
                &["show", "active-toolchain"],
                config,
                ui,
                "Reading active Rust toolchain",
            )
            .await
            .unwrap_or_else(|_| "stable".into());
            let channel = active
                .split_whitespace()
                .next()
                .unwrap_or("stable")
                .split('-')
                .next()
                .unwrap_or("stable");
            let check = output("rustup", &["check"], config, ui, "Checking Rust updates")
                .await
                .unwrap_or_default();
            let latest = check
                .lines()
                .find(|line| line.starts_with(channel))
                .and_then(version_number)
                .or_else(|| current.clone());
            let action = Some(CommandSpec::new("rustup", ["update", channel]));
            reports.push(report(
                "rust",
                "Rust",
                current,
                latest,
                Manager::Rustup,
                executable,
                vec![],
                action,
            ));
        } else {
            reports.push(report(
                "rust",
                "Rust",
                None,
                None,
                Manager::Unknown,
                executable,
                vec![],
                None,
            ));
        }
    }

    for (id, display, executable_name, version_args, brew_name) in [
        ("node", "Node.js", "node", vec!["--version"], "node"),
        ("go", "Go", "go", vec!["version"], "go"),
    ] {
        if !config.enabled_tools.iter().any(|t| t == id) {
            continue;
        }
        let executable = find_executable(executable_name);
        let manager = executable
            .as_deref()
            .map(manager_for_executable)
            .unwrap_or(Manager::Unknown);
        let current = version(
            executable_name,
            &version_args,
            config,
            ui,
            &format!("Reading {display} version"),
        )
        .await;
        let latest = if manager == Manager::Homebrew {
            brew_items
                .get(brew_name)
                .and_then(|i| i.current_version.clone())
                .or_else(|| current.clone())
        } else if manager == Manager::Mise {
            output(
                "mise",
                &["latest", id],
                config,
                ui,
                &format!("Checking latest {display} version"),
            )
            .await
            .ok()
            .and_then(|s| version_number(&s).or(Some(s)))
        } else {
            current.clone()
        };
        let action = match manager {
            Manager::Homebrew => CommandSpec::brew_upgrade(brew_name).ok(),
            Manager::Mise => Some(CommandSpec::new(
                "mise",
                ["use", "-g", &format!("{id}@latest")],
            )),
            _ => None,
        };
        reports.push(report(
            id,
            display,
            current,
            latest,
            manager,
            executable,
            other_source(id, manager, config, ui).await,
            action,
        ));
    }

    if config.enabled_tools.iter().any(|t| t == "npm") {
        let executable = find_executable("npm");
        let current = version("npm", &["--version"], config, ui, "Reading npm version").await;
        let latest = output(
            "npm",
            &["view", "npm", "version"],
            config,
            ui,
            "Checking latest npm version",
        )
        .await
        .ok()
        .and_then(|s| version_number(&s).or(Some(s)));
        reports.push(npm_report(executable, current, latest));
    }

    if config.enabled_tools.iter().any(|t| t == "pnpm") {
        let executable = find_executable("pnpm");
        let current = version("pnpm", &["--version"], config, ui, "Reading pnpm version").await;
        let latest = output(
            "npm",
            &["view", "pnpm", "version"],
            config,
            ui,
            "Checking latest pnpm version",
        )
        .await
        .ok()
        .and_then(|s| version_number(&s).or(Some(s)));
        let (manager, action) = if let Some(path) = executable.as_deref() {
            let manager = manager_for_executable(path);
            let action = match manager {
                Manager::Homebrew => CommandSpec::brew_upgrade("pnpm").ok(),
                Manager::Mise => Some(CommandSpec::new("mise", ["use", "-g", "pnpm@latest"])),
                _ => Some(CommandSpec::new(
                    "npm",
                    ["install", "--global", "pnpm@latest"],
                )),
            };
            (manager, action)
        } else {
            (Manager::Unknown, None)
        };
        reports.push(report(
            "pnpm",
            "pnpm",
            current,
            latest,
            manager,
            executable,
            other_source("pnpm", manager, config, ui).await,
            action,
        ));
    }

    let mut seen = HashSet::new();
    reports.retain(|item| seen.insert(item.id.clone()));
    reports.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(reports)
}

pub async fn verify(report: &ToolReport, config: &Config, ui: &Ui) -> Result<Option<String>> {
    let (program, args): (&str, &[&str]) = match report.id.as_str() {
        "rust" => ("rustc", &["--version"]),
        "node" => ("node", &["--version"]),
        "npm" => ("npm", &["--version"]),
        "pnpm" => ("pnpm", &["--version"]),
        "go" => ("go", &["version"]),
        id if id.starts_with("brew:") => return Ok(Some("installed".into())),
        _ => return Ok(report.current.clone()),
    };
    let text = output(
        program,
        args,
        config,
        ui,
        &format!("Verifying {}", report.name),
    )
    .await?;
    Ok(version_number(&text))
}

pub fn recovery_hint(report: &ToolReport) -> String {
    match report.manager {
        Manager::Homebrew => format!(
            "Run `brew doctor` and inspect `brew info {}`.",
            report.id.trim_start_matches("brew:")
        ),
        Manager::Rustup => "Run `rustup show` and `rustup check`.".into(),
        Manager::Mise => "Run `mise doctor` and `mise current`.".into(),
        Manager::Npm => "Run `npm doctor` and inspect your global prefix.".into(),
        Manager::Unknown => "Inspect PATH and reinstall the tool with its original manager.".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_reports_never_offer_an_action() {
        let item = report(
            "pnpm",
            "pnpm",
            None,
            Some("11.0.0".into()),
            Manager::Homebrew,
            None,
            vec![],
            Some(CommandSpec::new("brew", ["install", "pnpm"])),
        );

        assert_eq!(item.status, ToolStatus::Missing);
        assert!(item.action.is_none());
    }

    #[test]
    fn npm_report_uses_the_executable_installation_manager() {
        let item = npm_report(
            Some(PathBuf::from(
                "/Users/alice/.local/share/mise/installs/node/26.5.0/bin/npm",
            )),
            Some("11.17.0".into()),
            Some("12.0.0".into()),
        );

        assert_eq!(item.manager, Manager::Mise);
        let action = item.action.unwrap();
        assert_eq!(action.program, "npm");
        assert_eq!(action.args, ["install", "--global", "npm@latest"]);
    }

    #[test]
    fn npm_report_supports_homebrew_and_missing_installations() {
        let homebrew = npm_report(
            Some(PathBuf::from("/opt/homebrew/bin/npm")),
            Some("11.17.0".into()),
            Some("12.0.0".into()),
        );
        let missing = npm_report(None, None, Some("12.0.0".into()));

        assert_eq!(homebrew.manager, Manager::Homebrew);
        assert_eq!(missing.manager, Manager::Unknown);
        assert!(missing.action.is_none());
    }
}
