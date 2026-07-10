# Beacon

Beacon is a conservative macOS CLI for checking, upgrading, and diagnosing a development toolchain. It manages only the executable currently active on `PATH`, reports duplicate mise installations, previews every command, and never performs an untargeted `brew upgrade`.

## Install

Install the published crate with Cargo:

```bash
cargo install beacon-cli
```

You can also download the Apple Silicon archive from the
[latest GitHub Release](https://github.com/liorael/beacon/releases/latest). Verify it before
installing:

```bash
shasum -a 256 -c beacon-v0.1.0-aarch64-apple-darwin.tar.gz.sha256
tar -xzf beacon-v0.1.0-aarch64-apple-darwin.tar.gz
install -m 755 beacon-v0.1.0-aarch64-apple-darwin/beacon /usr/local/bin/beacon
```

Prebuilt binaries require Apple Silicon and macOS 15 Sequoia or newer. Installing from Cargo
requires Rust 1.85 or newer. Beacon v0.1 manages Homebrew formulae/casks, rustup toolchains,
Node.js, npm, pnpm, and Go.

## Commands

```bash
beacon check                    # refresh remote metadata and show updates
beacon check --json             # stable schema_version: 1 output
beacon upgrade                  # interactively select and confirm updates
beacon upgrade npm --yes        # explicit non-interactive update
beacon doctor --json            # inspect PATH, managers, and duplicate sources
beacon history --limit 20
beacon config show
beacon config set command_timeout_seconds 180
```

`upgrade` stops on the first command or verification failure and prints manager-specific recovery guidance. Missing pnpm is offered as an installation through the configured preferred manager.

## Local data

- Configuration: `~/Library/Application Support/Beacon/config.toml`
- History: `~/Library/Application Support/Beacon/beacon.db`
- Logs: `~/Library/Logs/Beacon/beacon.log`

Logs and history redact bearer/basic credentials, common secret assignments, and the absolute home directory. Beacon does not edit project toolchain files, lockfiles, shell configuration, or request administrator privileges.

## Development

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```
