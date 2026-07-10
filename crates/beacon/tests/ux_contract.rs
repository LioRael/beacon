use beacon::{
    Diagnostics, InstallationReport, ToolReport, ToolStatus, UpdateReport,
    command::CommandSpec,
    providers::{ManagerId, SourceId, TargetMode, ToolVersion, UpgradeAction},
    runner::sanitize_verbose_line,
    ui::{FeedbackMode, spinner_template, status_text},
    upgrade::{resolve_targets, upgrade_candidates},
};

fn report(id: &str, status: ToolStatus) -> ToolReport {
    let current = ToolVersion::new("1.0.0", Some("1.0.0".into())).unwrap();
    let latest = ToolVersion::new("2.0.0", Some("2.0.0".into())).unwrap();
    let manager = ManagerId::new("homebrew").unwrap();
    ToolReport {
        id: id.into(),
        name: id.into(),
        status,
        detail: None,
        installation: (status != ToolStatus::Missing).then(|| InstallationReport {
            current,
            executable: format!("/opt/homebrew/bin/{id}"),
            source: Some(SourceId::new("homebrew").unwrap()),
            alternatives: vec![],
        }),
        update: Some(UpdateReport {
            manager: manager.clone(),
            latest: latest.clone(),
            action: UpgradeAction {
                manager,
                command: CommandSpec::new("brew", ["upgrade", "--formula", id]),
                expected_version: latest,
                target_mode: TargetMode::Floating,
            },
        }),
        diagnostics: Diagnostics::default(),
    }
}

#[test]
fn missing_tools_are_not_upgrade_candidates() {
    let reports = vec![
        report("node", ToolStatus::Outdated),
        report("pnpm", ToolStatus::Missing),
    ];

    let candidates = upgrade_candidates(&reports);

    assert_eq!(
        candidates
            .iter()
            .map(|item| item.id.as_str())
            .collect::<Vec<_>>(),
        ["node"]
    );
}

#[test]
fn explicitly_upgrading_a_missing_tool_explains_the_boundary() {
    let reports = vec![report("pnpm", ToolStatus::Missing)];

    let error = resolve_targets(&reports, &["pnpm".into()]).unwrap_err();

    assert!(error.to_string().contains("not installed"));
    assert!(error.to_string().contains("does not install"));
}

#[test]
fn machine_reports_keep_missing_unmanaged_and_failed_states_distinct() {
    let mut missing = report("missing", ToolStatus::Missing);
    missing.update = None;
    let mut unmanaged = report("unmanaged", ToolStatus::Unmanaged);
    unmanaged.update = None;
    let mut failed = report("failed", ToolStatus::Failed);
    failed.update = None;
    failed.detail = Some("redacted diagnostic".into());

    let value = serde_json::to_value([missing, unmanaged, failed]).unwrap();

    assert_eq!(value[0]["status"], "missing");
    assert!(value[0]["installation"].is_null());
    assert!(value[0]["update"].is_null());
    assert!(value[0]["diagnostics"].is_object());

    assert_eq!(value[1]["status"], "unmanaged");
    assert!(value[1]["installation"].is_object());
    assert!(value[1]["update"].is_null());

    assert_eq!(value[2]["status"], "failed");
    assert_ne!(value[2]["status"], "current");
    assert_eq!(value[2]["detail"], "redacted diagnostic");
}

#[test]
fn status_colors_can_be_disabled() {
    let plain = status_text(ToolStatus::Outdated, false, 8);
    let colored = status_text(ToolStatus::Outdated, true, 8);

    assert_eq!(plain, "outdated");
    assert!(!plain.contains("\u{1b}["));
    assert!(colored.contains("\u{1b}["));
}

#[test]
fn feedback_mode_preserves_machine_output() {
    assert_eq!(
        FeedbackMode::select(true, false, false),
        FeedbackMode::Spinner
    );
    assert_eq!(
        FeedbackMode::select(false, false, false),
        FeedbackMode::Plain
    );
    assert_eq!(
        FeedbackMode::select(true, true, false),
        FeedbackMode::Silent
    );
    assert_eq!(
        FeedbackMode::select(true, true, true),
        FeedbackMode::Verbose
    );
}

#[test]
fn no_color_spinner_template_has_no_ansi_style_directive() {
    assert!(!spinner_template(false).contains(".cyan"));
    assert!(spinner_template(true).contains(".cyan"));
}

#[test]
fn verbose_lines_are_redacted_before_streaming() {
    let line = "token=secret /Users/alice/private\n";
    let safe = sanitize_verbose_line(line, Some("/Users/alice"));

    assert!(!safe.contains("secret"));
    assert!(!safe.contains("/Users/alice"));
    assert!(safe.ends_with('\n'));
}
