use beacon::{
    Manager, ToolReport, ToolStatus,
    command::CommandSpec,
    runner::sanitize_verbose_line,
    ui::{FeedbackMode, spinner_template, status_text},
    upgrade::{resolve_targets, upgrade_candidates},
};

fn report(id: &str, status: ToolStatus) -> ToolReport {
    ToolReport {
        id: id.into(),
        name: id.into(),
        current: (status != ToolStatus::Missing).then(|| "1.0.0".into()),
        latest: Some("2.0.0".into()),
        status,
        manager: Manager::Homebrew,
        executable: None,
        other_sources: vec![],
        detail: None,
        action: Some(CommandSpec::new("brew", ["upgrade", id])),
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
