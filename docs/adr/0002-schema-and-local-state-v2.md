# ADR 0002: JSON and local state v2

Status: accepted for Beacon 0.2.

Every machine command returns a schema v2 envelope with `status`, `data`, and structured redacted `errors`. Check and doctor data separate logical `tools` from `inventories`; nullable `installation` and `update` sections make missing and unmanaged states deterministic. Complete, fatal, and partial outcomes use exit codes 0, 1, and 2.

Configuration v2 separated `enabled_tools` and `enabled_inventories`; ADR 0003 supersedes that configuration model with configuration schema v3. The JSON envelope and SQLite state remain at schema v2. Migration preserves comments and unknown keys, writes versioned backups once, and atomically replaces the active file. SQLite migration is transactional and idempotent through `user_version = 2`; historical manager data becomes installation source while historical updater remains `unknown`. Snapshots record their payload schema version so v1 payloads remain identifiable.
