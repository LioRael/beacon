---
name: beacon
description: Manage a macOS development toolchain with the Beacon CLI. Use when the user mentions Beacon or asks to check, diagnose, or safely upgrade Homebrew, Rust, Node, npm, pnpm, Go, Bun, Deno, or uv installations. Do not use for project dependency upgrades, ordinary macOS application updates, or unrelated software management.
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
```

Always prefer `--json` machine output. Require `schema_version: 2` and parse the JSON envelope (`status`, `data`, `errors`). Never scrape colored or human terminal tables for decisions. Traverse `data.tools` and `data.inventories` separately. Treat `installation` and `update` as explicitly nullable.

Summarize current, outdated, missing, unmanaged, and failed tools. Preserve `installation.source` separately from `update.manager` in every summary and confirmation; never collapse them into a single "manager". Report missing tools for diagnosis only. Never pass missing or unmanaged tools to `upgrade`.

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
- The exact action Beacon reports.
- Whether the action has an `exact` or `floating` target mode:
  - `exact`: post-upgrade version must equal the confirmed expected version.
  - `floating`: manager policy is preserved; post-upgrade version must be newer than the old version and no lower than the confirmed candidate.

Ask for explicit confirmation of the final target set after showing this information. A prior general request such as "update my environment" is not the required final confirmation.

After confirmation, execute exactly:

```bash
beacon upgrade <targets> --yes --json
```

Do not add targets, use an untargeted `brew upgrade`, install missing tools through `upgrade`, or retry a failed upgrade without a new diagnosis. Beacon stops at the first failed command or verification. Always parse the JSON envelope afterward:

- If some targets succeeded before the failure, expect `status: "partial"` and exit 2.
- If no target succeeded, expect `status: "error"` and exit 1.

In both cases report recovery guidance from the structured `errors` field and wait for direction. Do not scrape human terminal output or auto-retry.

## Stay within scope

Do not use Beacon for project dependencies such as Cargo crates, npm dependencies in a repository, Go modules, CocoaPods, or Flutter packages. Use the project's own dependency workflow instead. Do not use Beacon to update ordinary macOS applications or tools outside its reports.
