# ADR 0003: Configuration selection and catalog v3

Status: accepted after the configuration UX grilling session.

Beacon configuration schema v3 treats tool selection as an explicit watchlist. A missing tool is reported by `check` and `doctor` only when the user explicitly enabled it. Fresh configuration initializes from supported executables on the current `PATH` whose version probes succeed. Ordinary checks are read-only and never change selection.

`enabled_tools` and `disabled_tools` preserve the distinction between monitored tools and deliberate opt-outs. `tool_catalog_version` lets a future Beacon release probe only newly supported catalog entries: installed new entries may be enabled during migration, while existing user choices remain stable. `config tools sync` adds currently runnable tools but respects `disabled_tools`; `reset` redetects installed tools and clears explicit opt-outs. Inventories use the symmetric enabled/disabled representation, without a sync operation.

Configuration lists are managed through `config tools` and `config inventories` list, enable, disable, edit, and reset commands. The generic `config set` command remains only for scalar settings. IDs are normalized and strictly validated before any mutation, so batch changes are atomic. Non-interactive operations support the existing schema v2 JSON envelope; interactive edit requires a TTY.

Migration from v2 writes `config.toml.v2.bak`, preserves comments and unknown keys, removes missing legacy defaults, and enables installed supported tools. Configuration writes remain atomic. The configuration schema version does not change the JSON envelope or SQLite state schema versions.
