# Beacon 0.3

Beacon is a conservative macOS CLI for checking, upgrading, and diagnosing a development toolchain. It manages only the executable currently active on `PATH`, keeps installation source separate from update manager, previews every command, and never performs an untargeted `brew upgrade`.

## Install

Install the published crate with Cargo today:

```bash
cargo install beacon-cli
```

The repository also prepares these future distribution channels:

```bash
brew install LioRael/tap/beacon
npm install --global @liorael/beacon
```

The `LioRael/homebrew-tap` repository and `@liorael/beacon` package have not been created or
published yet. Do not rely on those commands until their first release is announced.

You can also download the Apple Silicon archive from the
[latest GitHub Release](https://github.com/liorael/beacon/releases/latest). Verify it before
installing:

```bash
shasum -a 256 -c beacon-v0.3.1-aarch64-apple-darwin.tar.gz.sha256
tar -xzf beacon-v0.3.1-aarch64-apple-darwin.tar.gz
install -m 755 beacon-v0.3.1-aarch64-apple-darwin/beacon /usr/local/bin/beacon
```

Prebuilt binaries require Apple Silicon and macOS 15 Sequoia or newer. Installing from Cargo
requires Rust 1.85 or newer. Beacon v0.2 covers Rust, Node.js, npm, pnpm, Go, Bun, Deno, and uv. Homebrew formulae and casks are reported as a separate inventory.

## Commands

```bash
beacon check                    # refresh remote metadata with progress feedback
beacon check --json             # stable schema_version: 2 output
beacon upgrade                  # interactively select and confirm updates
beacon upgrade npm --yes        # explicit non-interactive update
beacon --verbose upgrade npm     # stream the underlying command output
beacon --no-color doctor         # disable ANSI colors
beacon doctor --json            # inspect PATH, managers, and duplicate sources
beacon history --limit 20
beacon config show
beacon config tools                         # list support, selection, and install state
beacon config tools enable bun uv
beacon config tools disable pnpm
beacon config tools sync                    # add installed tools unless explicitly disabled
beacon config tools reset                   # redetect installed tools and clear disabled choices
beacon config tools edit                    # interactive selection
beacon config inventories
beacon config inventories disable homebrew
beacon config set command_timeout_seconds 180
```

On first run, Beacon enables only supported tools that are executable on the current `PATH` and pass their version probe. `check` is read-only: tools installed later are added only by `config tools sync`, `enable`, `edit`, or `reset`. A missing tool remains visible only when it was explicitly enabled. `upgrade` lists only installed, outdated tools and qualified Homebrew inventory targets; it never installs missing tools. An upgrade stops on the first command or verification failure and prints manager-specific recovery guidance.

Configuration uses schema v3 and records enabled and explicitly disabled tools separately. Migrating an older configuration removes missing legacy defaults, adds installed supported tools, preserves comments and unknown keys, and writes `config.toml.v2.bak`. Configuration schema versions are independent from the machine-output envelope version.

Machine output always uses a schema v2 envelope with `tools` and `inventories` arrays. Tool `installation` and `update` sections are independently nullable. A successful partial check returns valid JSON and exit code 2 with structured, redacted errors; fatal failures return exit code 1.

Interactive terminals use color and a spinner with the current stage and elapsed time. Redirected human output uses plain stage lines, while `--json` keeps stdout machine-readable and suppresses progress. Set `NO_COLOR` or pass `--no-color` to disable ANSI styling. Verbose child-process output is streamed to stderr after Beacon redacts common credentials and the absolute home directory.

## Local data

- Configuration: `~/Library/Application Support/Beacon/config.toml`
- History: `~/Library/Application Support/Beacon/beacon.db`
- Logs: `~/Library/Logs/Beacon/beacon.log`

Logs and history redact bearer/basic credentials, common secret assignments, and the absolute home directory. Beacon does not edit project toolchain files, lockfiles, shell configuration, or request administrator privileges.

## Architecture and domain model

Beacon v0.2 uses a two-layer provider model: **ToolAdapter** discovers tools and versions;
**InstallManager** owns receipts, latest semantics, and update actions. Installation source and
update manager stay independent fields across human output, JSON, and local history.

- Domain vocabulary: [`docs/domain-glossary.md`](docs/domain-glossary.md)
- ADR 0001 — two-layer provider model: [`docs/adr/0001-two-layer-provider-model.md`](docs/adr/0001-two-layer-provider-model.md)
- ADR 0002 — JSON and local-state v2: [`docs/adr/0002-schema-and-local-state-v2.md`](docs/adr/0002-schema-and-local-state-v2.md)
- ADR 0003 — configuration selection and catalog v3: [`docs/adr/0003-config-selection-and-catalog-v3.md`](docs/adr/0003-config-selection-and-catalog-v3.md)
- Agent Skill (schema v2 contracts): [`skills/beacon/SKILL.md`](skills/beacon/SKILL.md)

## Development

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
node --test packages/npm/test/*.test.mjs tests/*.test.mjs
node scripts/verify-release-version.mjs v0.3.1
```

## Distribution setup

Tag releases always build one Apple Silicon archive and publish it to GitHub Releases. The npm
and Homebrew jobs reuse that archive and skip themselves until their credentials exist, so missing
distribution credentials do not block the primary release.

Before enabling the prepared channels:

1. Create the public `LioRael/homebrew-tap` repository with a default branch and `Formula/`
   directory.
2. Confirm the npm account can publish the public `@liorael/beacon` package.
3. Add an npm automation token as the `NPM_TOKEN` repository secret.
4. Add a fine-grained token with write access to the tap repository as
   `HOMEBREW_TAP_TOKEN`.

The Rust crate, npm package, Git tag, GitHub Release, and generated Formula must use the same
semantic version. The tag workflow stages the npm tarball and generates the Formula; it does not
rebuild the native binary for either channel.
