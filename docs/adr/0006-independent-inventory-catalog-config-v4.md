# ADR 0006: Version the inventory catalog independently in config v4

Status: accepted during the Agent Skill package-management grilling session.

Beacon configuration schema v4 retains `tool_catalog_version` and adds `inventory_catalog_version`, allowing new inventory providers to migrate independently from logical PATH tools. Migration from v3 preserves existing selections, comments, and unknown keys, writes `config.toml.v3.bak` once, and probes only the newly introduced `skills` inventory. A compatible Skills CLI resolved through `npx` or `bunx` enables that inventory; an unavailable runner or incompatible resolved CLI does not. Configuration writes remain atomic, and the public machine envelope remains schema v2.
