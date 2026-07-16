use crate::{
    InventoryChange, InventoryChangeKind, InventoryReport, InventoryRuntime, ResourceScope,
    ToolStatus,
    command::CommandSpec,
    providers::{ManagerId, TargetMode, ToolVersion, UpgradeAction},
    runner,
    store::Store,
};
use anyhow::{Context, Result, bail};
use futures::{StreamExt, stream};
use semver::{Version, VersionReq};
use serde::Deserialize;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Component, Path, PathBuf},
};
use tempfile::TempDir;

const MANAGER_ID: &str = "skills";
const PACKAGE_SPEC: &str = "skills@^1.5.18";
const GLOBAL_RECEIPT_VERSION: u64 = 3;
const PROJECT_RECEIPT_VERSION: u64 = 1;
const MAX_CONCURRENCY: usize = 4;

#[derive(Debug, Clone)]
pub struct Capability {
    pub runner_path: PathBuf,
    pub runner: PackageRunner,
    pub runner_version: Version,
    pub skills_version: Version,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageRunner {
    Npx,
    Bunx,
}

impl PackageRunner {
    fn executable(self) -> &'static str {
        match self {
            Self::Npx => "npx",
            Self::Bunx => "bunx",
        }
    }
}

impl Capability {
    fn command(&self, args: impl IntoIterator<Item = impl Into<String>>) -> CommandSpec {
        package_command(&self.runner_path, self.runner, args)
    }

    fn identity(&self) -> String {
        format!(
            "{} {}; skills {}; {}",
            self.runner.executable(),
            self.runner_version,
            self.skills_version,
            PACKAGE_SPEC
        )
    }
}

fn package_command(
    runner_path: &Path,
    package_runner: PackageRunner,
    args: impl IntoIterator<Item = impl Into<String>>,
) -> CommandSpec {
    let mut invocation = Vec::new();
    if package_runner == PackageRunner::Npx {
        invocation.push("--yes".into());
    }
    invocation.push(PACKAGE_SPEC.into());
    invocation.extend(args.into_iter().map(Into::into));
    telemetry_disabled(CommandSpec::new(runner_path.to_string_lossy(), invocation))
}

#[derive(Debug)]
pub struct RecoveryCopy {
    root: PathBuf,
}

impl RecoveryCopy {
    pub fn path(&self) -> &Path {
        &self.root
    }

    pub fn discard(self) -> Result<()> {
        fs::remove_dir_all(self.root)?;
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
struct ListedSkill {
    name: String,
    path: PathBuf,
    scope: String,
}

#[derive(Debug, Clone)]
struct Candidate {
    name: String,
    scope: ResourceScope,
    canonical_path: PathBuf,
    receipt_path: PathBuf,
    project_root: Option<PathBuf>,
    receipt: Value,
    receipt_entry: Value,
    source: String,
    source_locator: Option<String>,
    current: ToolVersion,
    locally_modified: bool,
    local_detail: Option<String>,
}

pub async fn probe(timeout_seconds: u64) -> Result<Capability> {
    let mut failures = Vec::new();
    for package_runner in [PackageRunner::Npx, PackageRunner::Bunx] {
        match probe_runner(package_runner, timeout_seconds).await {
            Ok(capability) => return Ok(capability),
            Err(error) => failures.push(format!("{}: {error}", package_runner.executable())),
        }
    }
    bail!(
        "no usable package runner for `{PACKAGE_SPEC}`; {}",
        failures.join("; ")
    )
}

async fn probe_runner(package_runner: PackageRunner, timeout_seconds: u64) -> Result<Capability> {
    let executable = package_runner.executable();
    let which = runner::run(
        &telemetry_disabled(CommandSpec::new("/usr/bin/which", [executable])),
        timeout_seconds,
    )
    .await
    .with_context(|| format!("`{executable}` is not installed on PATH"))?;
    let runner_path = PathBuf::from(which.stdout.trim());
    if runner_path.as_os_str().is_empty() {
        bail!("`{executable}` is not installed on PATH");
    }
    let runner_version_output = runner::run(
        &telemetry_disabled(CommandSpec::new(
            runner_path.to_string_lossy(),
            ["--version"],
        )),
        timeout_seconds,
    )
    .await
    .with_context(|| format!("failed to read `{executable} --version`"))?;
    let runner_version = parse_version(&runner_version_output.stdout, executable)?;

    let version_output = runner::run(
        &package_command(&runner_path, package_runner, ["--version"]),
        timeout_seconds,
    )
    .await
    .with_context(|| format!("failed to execute `{executable} {PACKAGE_SPEC} --version`"))?;
    let skills_version = parse_version(&version_output.stdout, "skills")?;
    let requirement = VersionReq::parse(">=1.5.18, <2.0.0")?;
    if !requirement.matches(&skills_version) {
        bail!("unsupported skills version {skills_version}; Beacon requires >=1.5.18,<2.0.0");
    }
    let probe_dir = TempDir::new()?;
    let list_command = package_command(&runner_path, package_runner, ["list", "--json"])
        .in_directory(probe_dir.path());
    let list_output = runner::run(&list_command, timeout_seconds)
        .await
        .with_context(|| format!("`{executable} {PACKAGE_SPEC}` does not support `list --json`"))?;
    serde_json::from_str::<Vec<ListedSkill>>(&list_output.stdout)
        .context("`skills list --json` returned an unsupported shape")?;
    Ok(Capability {
        runner_path,
        runner: package_runner,
        runner_version,
        skills_version,
    })
}

pub async fn inventory(timeout_seconds: u64, store: Option<&Store>) -> Vec<InventoryReport> {
    let capability = match probe(timeout_seconds).await {
        Ok(capability) => capability,
        Err(error) => return vec![manager_failure(error.to_string())],
    };
    let project_root = match resolve_project_root(timeout_seconds).await {
        Ok(root) => root,
        Err(error) => return vec![manager_failure(error.to_string())],
    };
    let global_output = match runner::run_machine_output(
        &capability.command(["list", "--global", "--json"]),
        timeout_seconds,
    )
    .await
    {
        Ok(output) => output,
        Err(error) => return vec![manager_failure(error.to_string())],
    };
    let mut listed: Vec<ListedSkill> = match serde_json::from_str(&global_output.stdout) {
        Ok(listed) => listed,
        Err(error) => {
            return vec![manager_failure(format!(
                "invalid global skills list JSON: {error}"
            ))];
        }
    };

    let mut reports = Vec::new();
    if let Some(root) = &project_root {
        let command = capability.command(["list", "--json"]).in_directory(root);
        match runner::run_machine_output(&command, timeout_seconds).await {
            Ok(output) => match serde_json::from_str::<Vec<ListedSkill>>(&output.stdout) {
                Ok(mut project) => listed.append(&mut project),
                Err(error) => reports.push(scope_failure(format!(
                    "invalid project skills list JSON: {error}"
                ))),
            },
            Err(error) => reports.push(scope_failure(format!(
                "project skills list failed: {error}"
            ))),
        }
    }

    let mut candidates = Vec::new();
    for listed_skill in listed {
        match prepare_candidate(listed_skill, project_root.as_deref(), store) {
            Ok(Prepared::Candidate(candidate)) => candidates.push(*candidate),
            Ok(Prepared::Report(report)) => reports.push(*report),
            Err(error) => reports.push(scope_failure(error.to_string())),
        }
    }

    let mut checked = stream::iter(candidates.into_iter().map(|candidate| {
        let capability = capability.clone();
        async move { check_candidate(candidate, &capability, timeout_seconds).await }
    }))
    .buffered(MAX_CONCURRENCY)
    .collect::<Vec<_>>()
    .await;
    reports.append(&mut checked);
    reports.sort_by(|left, right| left.id.cmp(&right.id));
    reports
}

pub fn prepare_recovery(report: &InventoryReport, recovery_root: &Path) -> Result<RecoveryCopy> {
    if report.kind != "agent-skill" {
        bail!("recovery copies are only supported for Agent Skills");
    }
    let canonical = report
        .runtime
        .canonical_path
        .as_deref()
        .context("Skill report has no canonical path")?;
    let receipt = report
        .runtime
        .receipt_path
        .as_deref()
        .context("Skill report has no receipt path")?;
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%S%.fZ");
    let safe_id = report
        .id
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '-'
            }
        })
        .collect::<String>();
    let root = recovery_root.join(format!("{stamp}-{safe_id}"));
    copy_directory(canonical, &root.join("skill"))?;
    fs::create_dir_all(&root)?;
    fs::copy(receipt, root.join("receipt.json"))?;
    fs::write(
        root.join("RECOVERY.txt"),
        format!(
            "Beacon recovery copy for {}\nscope={}\ncanonical={}\nreceipt={}\n",
            report.id,
            report.scope.as_str(),
            display_redacted(canonical),
            display_redacted(receipt)
        ),
    )?;
    Ok(RecoveryCopy { root })
}

pub async fn verify(report: &InventoryReport, timeout_seconds: u64) -> Result<String> {
    let old = report
        .current
        .as_ref()
        .context("Skill report has no current revision")?;
    let canonical = report
        .runtime
        .canonical_path
        .as_deref()
        .context("Skill report has no canonical path")?;
    let expected_path = report
        .runtime
        .manager_path
        .as_deref()
        .context("Skill report has no manager path")?;
    let expected_version = report
        .runtime
        .manager_version
        .as_deref()
        .context("Skill report has no manager version")?;
    let capability = probe(timeout_seconds).await?;
    if capability.runner_path != expected_path || capability.identity() != expected_version {
        bail!("skills package runner or resolved version drifted during update");
    }
    let actual_revision = directory_revision(canonical)?;
    let actual = ToolVersion::new(&actual_revision, Some(actual_revision.clone()))?;
    crate::providers::verify_versions(TargetMode::Rolling, old, old, &actual, |left, right| {
        Ok(left.display().cmp(right.display()))
    })?;
    Ok(actual_revision)
}

enum Prepared {
    Candidate(Box<Candidate>),
    Report(Box<InventoryReport>),
}

fn prepare_candidate(
    listed: ListedSkill,
    project_root: Option<&Path>,
    store: Option<&Store>,
) -> Result<Prepared> {
    let scope = match listed.scope.as_str() {
        "global" => ResourceScope::Global,
        "project" => ResourceScope::Project,
        other => {
            return Ok(Prepared::Report(Box::new(unmanaged_report(
                listed.name,
                ResourceScope::System,
                None,
                format!("skills reported unsupported scope `{other}`"),
            ))));
        }
    };
    let canonical_path = listed
        .path
        .canonicalize()
        .with_context(|| format!("cannot resolve Skill path {}", listed.path.display()))?;
    let (receipt_path, expected_version, root) = match scope {
        ResourceScope::Global => (global_receipt_path()?, GLOBAL_RECEIPT_VERSION, None),
        ResourceScope::Project => {
            let root =
                project_root.context("skills reported a project Skill without a project root")?;
            (
                root.join("skills-lock.json"),
                PROJECT_RECEIPT_VERSION,
                Some(root.to_path_buf()),
            )
        }
        ResourceScope::System => unreachable!(),
    };
    if !receipt_path.is_file() {
        return Ok(Prepared::Report(Box::new(unmanaged_report(
            listed.name,
            scope,
            Some(canonical_path),
            format!(
                "manager receipt is missing at {}",
                display_redacted(&receipt_path)
            ),
        ))));
    }
    let receipt: Value = serde_json::from_slice(&fs::read(&receipt_path)?).with_context(|| {
        format!(
            "invalid manager receipt {}",
            display_redacted(&receipt_path)
        )
    })?;
    let version = receipt.get("version").and_then(Value::as_u64);
    if version != Some(expected_version) {
        bail!(
            "unsupported {} receipt version {}; expected {}",
            scope.as_str(),
            version.map_or_else(|| "missing".into(), |value| value.to_string()),
            expected_version
        );
    }
    let receipt_entry = receipt
        .get("skills")
        .and_then(Value::as_object)
        .and_then(|skills| skills.get(&listed.name))
        .cloned();
    let Some(receipt_entry) = receipt_entry else {
        return Ok(Prepared::Report(Box::new(unmanaged_report(
            listed.name,
            scope,
            Some(canonical_path),
            "Skill is listable but absent from the manager receipt".into(),
        ))));
    };
    let source_type = field(&receipt_entry, "sourceType")
        .unwrap_or("unknown")
        .to_string();
    if matches!(source_type.as_str(), "local" | "node_modules" | "legacy")
        || !matches!(source_type.as_str(), "github" | "git")
    {
        return Ok(Prepared::Report(Box::new(unmanaged_report(
            listed.name,
            scope,
            Some(canonical_path),
            format!("source type `{source_type}` is not safely updateable"),
        ))));
    }
    ensure_safe_location(scope, &canonical_path, root.as_deref())?;
    let revision = directory_revision(&canonical_path)?;
    let current = ToolVersion::new(&revision, Some(revision.clone()))?;
    let source_locator = field(&receipt_entry, "sourceUrl")
        .or_else(|| field(&receipt_entry, "source"))
        .map(sanitize_locator);

    let (locally_modified, local_detail) = match scope {
        ResourceScope::Project => {
            let expected = field(&receipt_entry, "computedHash")
                .or_else(|| field(&receipt_entry, "skillFolderHash"));
            let hash_mismatch =
                expected.is_none_or(|value| value != strip_revision_prefix(&revision));
            let git_overlap = root.as_deref().is_some_and(|root| {
                project_git_overlap(root, &canonical_path, timeout_seconds_fallback())
                    .unwrap_or(false)
            });
            let modified = hash_mismatch || git_overlap;
            (
                modified,
                modified.then(|| {
                    "project Skill or its skills-lock.json has overlapping local changes".into()
                }),
            )
        }
        ResourceScope::Global => {
            global_baseline_state(store, &listed.name, &receipt_entry, &revision)?
        }
        ResourceScope::System => unreachable!(),
    };

    Ok(Prepared::Candidate(Box::new(Candidate {
        name: listed.name,
        scope,
        canonical_path,
        receipt_path,
        project_root: root,
        receipt,
        receipt_entry,
        source: source_type,
        source_locator,
        current,
        locally_modified,
        local_detail,
    })))
}

fn global_baseline_state(
    store: Option<&Store>,
    name: &str,
    receipt_entry: &Value,
    revision: &str,
) -> Result<(bool, Option<String>)> {
    let Some(store) = store else {
        return Ok((false, None));
    };
    let fingerprint = value_fingerprint(receipt_entry)?;
    match store.skill_baseline("global", "", name)? {
        Some(baseline) if baseline.receipt_fingerprint == fingerprint => {
            let changed = baseline.content_revision != revision;
            Ok((
                changed,
                changed.then(|| "global Skill content changed without a receipt change".into()),
            ))
        }
        _ => {
            store.upsert_skill_baseline("global", "", name, &fingerprint, revision)?;
            Ok((false, None))
        }
    }
}

async fn check_candidate(
    candidate: Candidate,
    capability: &Capability,
    timeout_seconds: u64,
) -> InventoryReport {
    if candidate.locally_modified {
        return unmanaged_candidate(candidate);
    }
    match sandbox_check(&candidate, capability, timeout_seconds).await {
        Ok((latest_revision, changes)) => {
            let status = if latest_revision == candidate.current {
                ToolStatus::Current
            } else {
                ToolStatus::Outdated
            };
            let action = (status == ToolStatus::Outdated).then(|| UpgradeAction {
                manager: ManagerId::new(MANAGER_ID).expect("static manager id is valid"),
                command: real_update_command(&candidate, capability),
                expected_version: latest_revision.clone(),
                target_mode: TargetMode::Rolling,
            });
            let runtime = runtime(&candidate, capability);
            InventoryReport {
                id: skill_id(candidate.scope, &candidate.name),
                name: candidate.name,
                kind: "agent-skill".into(),
                status,
                current: Some(candidate.current),
                latest: Some(latest_revision),
                action,
                detail: None,
                scope: candidate.scope,
                installation_source: Some(candidate.source),
                source_locator: candidate.source_locator,
                update_manager: Some(MANAGER_ID.into()),
                changes,
                runtime,
            }
        }
        Err(error) => failed_candidate(candidate, error.to_string(), capability),
    }
}

async fn sandbox_check(
    candidate: &Candidate,
    capability: &Capability,
    timeout_seconds: u64,
) -> Result<(ToolVersion, Vec<InventoryChange>)> {
    let temp = TempDir::new().context("create isolated Skill update mirror")?;
    let temp_home = temp.path().join("home");
    let temp_state = temp.path().join("state");
    let temp_project = temp.path().join("project");
    fs::create_dir_all(&temp_home)?;
    fs::create_dir_all(&temp_state)?;
    let target = match candidate.scope {
        ResourceScope::Global => {
            let home = home_dir()?.canonicalize().context("cannot resolve HOME")?;
            let relative = candidate
                .canonical_path
                .strip_prefix(&home)
                .context("global Skill path is outside HOME")?;
            temp_home.join(relative)
        }
        ResourceScope::Project => {
            let root = candidate
                .project_root
                .as_deref()
                .context("project Skill has no project root")?;
            let relative = candidate
                .canonical_path
                .strip_prefix(root)
                .context("project Skill path is outside its project root")?;
            temp_project.join(relative)
        }
        ResourceScope::System => bail!("system scope is not valid for Agent Skills"),
    };
    copy_directory(&candidate.canonical_path, &target)?;
    let before = directory_manifest(&target)?;
    let isolated_receipt = receipt_with_only_target(
        &candidate.receipt,
        &candidate.name,
        &candidate.receipt_entry,
    )?;
    match candidate.scope {
        ResourceScope::Global => {
            let path = temp_state.join("skills/.skill-lock.json");
            write_json(&path, &isolated_receipt)?;
        }
        ResourceScope::Project => {
            write_json(&temp_project.join("skills-lock.json"), &isolated_receipt)?
        }
        ResourceScope::System => unreachable!(),
    }

    let mut command = capability
        .command(update_args(&candidate.name, candidate.scope))
        .with_environment("HOME", temp_home.to_string_lossy())
        .with_environment("XDG_STATE_HOME", temp_state.to_string_lossy());
    if candidate.scope == ResourceScope::Project {
        command = command.in_directory(&temp_project);
    }
    runner::run(&command, timeout_seconds).await?;
    let after = directory_manifest(&target)?;
    let revision = directory_revision(&target)?;
    let latest = ToolVersion::new(&revision, Some(revision.clone()))?;
    Ok((latest, compare_manifests(&before, &after)))
}

fn real_update_command(candidate: &Candidate, capability: &Capability) -> CommandSpec {
    let mut command = capability.command(update_args(&candidate.name, candidate.scope));
    if let Some(root) = &candidate.project_root {
        command = command.in_directory(root);
    }
    command
}

fn update_args(name: &str, scope: ResourceScope) -> Vec<String> {
    vec![
        "update".into(),
        name.into(),
        match scope {
            ResourceScope::Global => "--global".into(),
            ResourceScope::Project => "--project".into(),
            ResourceScope::System => unreachable!(),
        },
        "--yes".into(),
    ]
}

fn runtime(candidate: &Candidate, capability: &Capability) -> InventoryRuntime {
    InventoryRuntime {
        canonical_path: Some(candidate.canonical_path.clone()),
        receipt_path: Some(candidate.receipt_path.clone()),
        project_root: candidate.project_root.clone(),
        manager_path: Some(capability.runner_path.clone()),
        manager_version: Some(capability.identity()),
    }
}

fn unmanaged_candidate(candidate: Candidate) -> InventoryReport {
    let detail = candidate
        .local_detail
        .clone()
        .unwrap_or_else(|| "Skill is not safely updateable".into());
    InventoryReport {
        id: skill_id(candidate.scope, &candidate.name),
        name: candidate.name,
        kind: "agent-skill".into(),
        status: ToolStatus::Unmanaged,
        current: Some(candidate.current.clone()),
        latest: None,
        action: None,
        detail: Some(detail),
        scope: candidate.scope,
        installation_source: Some(candidate.source.clone()),
        source_locator: candidate.source_locator.clone(),
        update_manager: None,
        changes: Vec::new(),
        runtime: InventoryRuntime::default(),
    }
}

fn failed_candidate(
    candidate: Candidate,
    detail: String,
    capability: &Capability,
) -> InventoryReport {
    let runtime = runtime(&candidate, capability);
    InventoryReport {
        id: skill_id(candidate.scope, &candidate.name),
        name: candidate.name,
        kind: "agent-skill".into(),
        status: ToolStatus::Failed,
        current: Some(candidate.current.clone()),
        latest: None,
        action: None,
        detail: Some(detail),
        scope: candidate.scope,
        installation_source: Some(candidate.source.clone()),
        source_locator: candidate.source_locator.clone(),
        update_manager: Some(MANAGER_ID.into()),
        changes: Vec::new(),
        runtime,
    }
}

fn unmanaged_report(
    name: String,
    scope: ResourceScope,
    canonical_path: Option<PathBuf>,
    detail: String,
) -> InventoryReport {
    InventoryReport {
        id: skill_id(scope, &name),
        name,
        kind: "agent-skill".into(),
        status: ToolStatus::Unmanaged,
        current: canonical_path
            .as_deref()
            .and_then(|path| directory_revision(path).ok())
            .and_then(|revision| ToolVersion::new(&revision, Some(revision.clone())).ok()),
        latest: None,
        action: None,
        detail: Some(detail),
        scope,
        installation_source: None,
        source_locator: None,
        update_manager: None,
        changes: Vec::new(),
        runtime: InventoryRuntime::default(),
    }
}

fn manager_failure(detail: String) -> InventoryReport {
    InventoryReport {
        id: MANAGER_ID.into(),
        name: "Agent Skills".into(),
        kind: "agent-skill-manager".into(),
        status: ToolStatus::Failed,
        current: None,
        latest: None,
        action: None,
        detail: Some(format!(
            "{detail}. Install Node.js with npx or Bun with bunx, ensure the package runner can execute `{PACKAGE_SPEC}`, or disable the skills inventory"
        )),
        scope: ResourceScope::System,
        installation_source: None,
        source_locator: None,
        update_manager: Some(MANAGER_ID.into()),
        changes: Vec::new(),
        runtime: InventoryRuntime::default(),
    }
}

fn scope_failure(detail: String) -> InventoryReport {
    InventoryReport {
        id: "skills:receipt".into(),
        name: "Agent Skills receipt".into(),
        kind: "agent-skill-manager".into(),
        status: ToolStatus::Failed,
        current: None,
        latest: None,
        action: None,
        detail: Some(detail),
        scope: ResourceScope::System,
        installation_source: None,
        source_locator: None,
        update_manager: Some(MANAGER_ID.into()),
        changes: Vec::new(),
        runtime: InventoryRuntime::default(),
    }
}

fn skill_id(scope: ResourceScope, name: &str) -> String {
    format!("skill:{}:{name}", scope.as_str())
}

fn telemetry_disabled(command: CommandSpec) -> CommandSpec {
    command.with_environment("DISABLE_TELEMETRY", "1")
}

fn parse_version(output: &str, label: &str) -> Result<Version> {
    let token = output
        .split_whitespace()
        .find(|token| {
            token
                .trim_start_matches('v')
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_digit())
        })
        .with_context(|| format!("{label} --version returned no semantic version"))?;
    Version::parse(token.trim_start_matches('v'))
        .with_context(|| format!("invalid {label} semantic version"))
}

async fn resolve_project_root(timeout_seconds: u64) -> Result<Option<PathBuf>> {
    let cwd = std::env::current_dir()?;
    for ancestor in cwd.ancestors() {
        if ancestor.join("skills-lock.json").is_file() {
            return Ok(Some(ancestor.to_path_buf()));
        }
    }
    let command = telemetry_disabled(CommandSpec::new("git", ["rev-parse", "--show-toplevel"]))
        .in_directory(&cwd);
    match runner::run(&command, timeout_seconds).await {
        Ok(output) if !output.stdout.trim().is_empty() => {
            Ok(Some(PathBuf::from(output.stdout.trim())))
        }
        Ok(_) | Err(_) => Ok(None),
    }
}

fn global_receipt_path() -> Result<PathBuf> {
    if let Some(state) = std::env::var_os("XDG_STATE_HOME") {
        Ok(PathBuf::from(state).join("skills/.skill-lock.json"))
    } else {
        Ok(home_dir()?.join(".agents/.skill-lock.json"))
    }
}

fn home_dir() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .context("HOME is not set")
}

fn ensure_safe_location(
    scope: ResourceScope,
    path: &Path,
    project_root: Option<&Path>,
) -> Result<()> {
    let base = match scope {
        ResourceScope::Global => home_dir()?,
        ResourceScope::Project => project_root
            .context("project root is missing")?
            .to_path_buf(),
        ResourceScope::System => bail!("unsupported Agent Skill scope"),
    };
    let base = base.canonicalize().unwrap_or(base);
    if !path.starts_with(&base) {
        bail!(
            "Skill path is outside its declared {} scope",
            scope.as_str()
        );
    }
    Ok(())
}

fn field<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

fn sanitize_locator(locator: &str) -> String {
    let without_query = locator.split(['?', '#']).next().unwrap_or(locator);
    let sanitized = if let Some(scheme_end) = without_query.find("://") {
        let after_scheme = scheme_end + 3;
        let tail = &without_query[after_scheme..];
        if let Some(at) = tail.find('@') {
            format!("{}{}", &without_query[..after_scheme], &tail[at + 1..])
        } else {
            without_query.to_string()
        }
    } else {
        without_query.to_string()
    };
    crate::redact::redact(&sanitized, std::env::var("HOME").ok().as_deref())
}

fn display_redacted(path: &Path) -> String {
    crate::redact::redact(
        &path.display().to_string(),
        std::env::var("HOME").ok().as_deref(),
    )
}

fn timeout_seconds_fallback() -> u64 {
    5
}

fn project_git_overlap(root: &Path, canonical_path: &Path, timeout_seconds: u64) -> Result<bool> {
    let relative = canonical_path.strip_prefix(root)?;
    let output = std::process::Command::new("git")
        .current_dir(root)
        .args(["status", "--porcelain", "--"])
        .arg("skills-lock.json")
        .arg(relative)
        .env("DISABLE_TELEMETRY", "1")
        .output()?;
    if !output.status.success() {
        return Ok(false);
    }
    let _ = timeout_seconds;
    Ok(!output.stdout.is_empty())
}

fn directory_manifest(root: &Path) -> Result<BTreeMap<String, String>> {
    let mut files = Vec::new();
    collect_files(root, root, &mut files)?;
    files.sort();
    let mut manifest = BTreeMap::new();
    for relative in files {
        let bytes = fs::read(root.join(&relative))?;
        manifest.insert(path_key(&relative)?, format!("{:x}", Sha256::digest(bytes)));
    }
    Ok(manifest)
}

fn collect_files(root: &Path, current: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(current)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name();
        if entry.file_type()?.is_dir() {
            if name == ".git" || name == "node_modules" {
                continue;
            }
            collect_files(root, &path, files)?;
        } else if entry.file_type()?.is_file() {
            files.push(path.strip_prefix(root)?.to_path_buf());
        }
    }
    Ok(())
}

fn path_key(path: &Path) -> Result<String> {
    if path
        .components()
        .any(|part| matches!(part, Component::ParentDir))
    {
        bail!("unsafe relative path in Skill directory");
    }
    Ok(path.to_string_lossy().replace('\\', "/"))
}

fn directory_revision(root: &Path) -> Result<String> {
    let manifest = directory_manifest(root)?;
    let mut hasher = Sha256::new();
    for path in manifest.keys() {
        hasher.update(path.as_bytes());
        hasher.update(fs::read(root.join(path))?);
    }
    Ok(format!("sha256:{:x}", hasher.finalize()))
}

fn strip_revision_prefix(revision: &str) -> &str {
    revision.strip_prefix("sha256:").unwrap_or(revision)
}

fn value_fingerprint(value: &Value) -> Result<String> {
    Ok(format!(
        "sha256:{:x}",
        Sha256::digest(serde_json::to_vec(value)?)
    ))
}

fn compare_manifests(
    before: &BTreeMap<String, String>,
    after: &BTreeMap<String, String>,
) -> Vec<InventoryChange> {
    let paths = before
        .keys()
        .chain(after.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    paths
        .into_iter()
        .filter_map(|path| {
            let kind = match (before.get(&path), after.get(&path)) {
                (None, Some(_)) => InventoryChangeKind::Added,
                (Some(_), None) => InventoryChangeKind::Removed,
                (Some(left), Some(right)) if left != right => InventoryChangeKind::Modified,
                _ => return None,
            };
            Some(InventoryChange { path, kind })
        })
        .collect()
}

fn copy_directory(source: &Path, target: &Path) -> Result<()> {
    fs::create_dir_all(target)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let destination = target.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            if entry.file_name() == ".git" || entry.file_name() == "node_modules" {
                continue;
            }
            copy_directory(&entry.path(), &destination)?;
        } else if entry.file_type()?.is_file() {
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(entry.path(), destination)?;
        }
    }
    Ok(())
}

fn receipt_with_only_target(receipt: &Value, name: &str, entry: &Value) -> Result<Value> {
    let mut receipt = receipt.clone();
    let object = receipt
        .as_object_mut()
        .context("receipt must be a JSON object")?;
    let mut skills = Map::new();
    skills.insert(name.into(), entry.clone());
    object.insert("skills".into(), Value::Object(skills));
    Ok(receipt)
}

fn write_json(path: &Path, value: &Value) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, serde_json::to_vec_pretty(value)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{compare_manifests, parse_version, sanitize_locator};
    use crate::InventoryChangeKind;
    use std::collections::BTreeMap;

    #[test]
    fn parses_cli_version() {
        assert_eq!(
            parse_version("skills 1.5.18", "skills")
                .unwrap()
                .to_string(),
            "1.5.18"
        );
    }

    #[test]
    fn strips_url_credentials_and_query() {
        assert_eq!(
            sanitize_locator("https://token@example.com/org/repo?token=secret"),
            "https://example.com/org/repo"
        );
    }

    #[test]
    fn reports_manifest_changes_without_contents() {
        let before = BTreeMap::from([("a".into(), "1".into()), ("b".into(), "2".into())]);
        let after = BTreeMap::from([("b".into(), "3".into()), ("c".into(), "4".into())]);
        let changes = compare_manifests(&before, &after);
        assert_eq!(changes.len(), 3);
        assert_eq!(changes[0].kind, InventoryChangeKind::Removed);
        assert_eq!(changes[1].kind, InventoryChangeKind::Modified);
        assert_eq!(changes[2].kind, InventoryChangeKind::Added);
    }
}
