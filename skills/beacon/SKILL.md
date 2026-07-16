---
name: beacon
description: Manage a macOS development toolchain and third-party Agent Skills with the Beacon CLI. Use when the user mentions Beacon or asks to check, diagnose, or safely upgrade Homebrew, Agent Skills, Rust, Node, npm, pnpm, Go, Bun, Deno, or uv installations. Do not use for project dependency upgrades, ordinary macOS application updates, or unrelated software management.
---

# Beacon

Use Beacon's JSON interface to inspect the active development toolchain, explain findings in the user's language, and prepare explicit upgrades without silently changing the environment.

## Establish availability

Run `command -v beacon` before invoking Beacon. If it is missing, explain the supported platform and ask for explicit confirmation before installing it. Check that a channel is published before offering it, then use the first available channel in this order:

```bash
brew install LioRael/tap/beacon
npm install --global @liorael/beacon
cargo install beacon-cli --locked
```

Do not install Beacon automatically. The Homebrew tap and npm package support Apple Silicon macOS 15 or newer.

## Inspect the environment

Run read-only commands without additional confirmation:

```bash
beacon check --json
beacon doctor --json
beacon doctor <targets> --json
beacon history --json
beacon config show --json
beacon config tools --json
beacon config inventories --json
```

Always prefer `--json` machine output. Require `schema_version: 2` and parse the JSON envelope (`status`, `data`, `errors`). Never scrape colored or human terminal tables for decisions. Traverse `data.tools` and `data.inventories` separately. Treat `installation` and `update` as explicitly nullable.

Summarize current, outdated, missing, unmanaged, and failed resources. Preserve installation source separately from update manager in every summary and confirmation; never collapse them into a single "manager". For tools, read `installation.source` and `update.manager`. For inventories, read `installation_source` and `update_manager`, plus `scope` and redacted `source_locator` when present. `check` reports configured resources, not every supported resource: a missing report means the user explicitly chose to monitor it. Report missing resources for diagnosis only. Never pass missing or unmanaged resources to `upgrade`.

When the user asks to change the monitored scope, use the domain commands rather than editing TOML or using list-valued `config set` calls:

```bash
beacon config tools enable <tools> --json
beacon config tools disable <tools> --json
beacon config tools sync --json
beacon config tools reset --json
beacon config inventories enable <inventories> --json
beacon config inventories disable <inventories> --json
beacon config inventories reset --json
```

Treat these as state-changing commands and get confirmation when the user has not already requested the change. `sync` adds tools that are executable on the current `PATH` and pass a version probe, but never re-enables an explicitly disabled tool. Configuration schema v4 is distinct from the schema v2 JSON envelope.

The `skills` inventory uses a package runner rather than a globally installed CLI. Beacon prefers `npx --yes skills@^1.5.18` and falls back to `bunx skills@^1.5.18`; the resolved CLI must satisfy `>=1.5.18,<2.0.0`. Beacon does not install a global `skills` executable, and there is no separate `beacon skills` command. Agent Skill items use `skill:global:<name>` and `skill:project:<name>` IDs. Treat a bare Skill name as valid only when it is unique in the current result. Do not infer or manage agent-specific copies or links; the Skills CLI owns that topology.

Interpret envelope outcomes:

- `status: "ok"` with exit 0 is complete success.
- `status: "partial"` with exit 2 is valid partial data plus structured, redacted `errors`; report successful items and failures separately.
- `status: "error"` with exit 1 is fatal; do not invent tool state from human output.

Use `doctor` when the user asks why a tool, version, path, or manager was detected. Use `history` only when prior Beacon activity is relevant.

## Prepare an upgrade

Always run `beacon check --json` immediately before preparing upgrades. Select only installed reports whose status is `outdated` and whose `action` is present.

Show the user:

- Each exact target.
- Current and latest versions.
- Detected installation source and update manager, kept separate.
- For an Agent Skill, its global/project scope and every path in its added/modified/removed `changes` manifest. Do not request or display file contents unless the user separately asks to inspect the source.
- The exact action Beacon reports.
- Whether the action has an `exact`, `floating`, or `rolling` target mode:
  - `exact`: post-upgrade version must equal the confirmed expected version.
  - `floating`: manager policy is preserved; post-upgrade version must be newer than the old version and no lower than the confirmed candidate.
  - `rolling`: a moving channel is preserved; the observed revision must change, but it may advance beyond the revision seen during confirmation.

Ask for explicit confirmation of the final target set after showing this information. A prior general request such as "update my environment" is not the required final confirmation.

After confirmation, execute exactly:

```bash
beacon upgrade <targets> --yes --json
```

Do not add targets, use an untargeted `brew upgrade`, install missing tools through `upgrade`, or retry a failed upgrade without a new diagnosis. Beacon stops at the first failed command or verification. Always parse the JSON envelope afterward:

- If some targets succeeded before the failure, expect `status: "partial"` and exit 2.
- If no target succeeded, expect `status: "error"` and exit 1.

In both cases report recovery guidance from the structured `errors` field and wait for direction. A failed Agent Skill update may retain a home-redacted recovery path containing only the canonical Skill and applicable receipt; do not treat it as an automatic agent-topology rollback. Do not scrape human terminal output or auto-retry.

## Stay within scope

Do not use Beacon for project dependencies such as Cargo crates, npm dependencies in a repository, Go modules, CocoaPods, or Flutter packages. Use the project's own dependency workflow instead. Do not use Beacon to update ordinary macOS applications or tools outside its reports.
