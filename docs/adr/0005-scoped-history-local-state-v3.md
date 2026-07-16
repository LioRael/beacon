# ADR 0005: Add scoped history in local state v3

Status: accepted during the Agent Skill package-management grilling session.

Beacon upgrades its SQLite local state to `user_version = 3` so history entries can distinguish resources with the same target ID across scopes. The history table adds `resource_scope` (`system`, `global`, or `project`) and nullable `scope_locator`; project locators store the home-redacted Skill Project root, while system and global entries store no locator. Existing rows backfill as system-scoped because Agent Skill history did not exist before this migration.

Local state v3 also adds `skill_baselines`, keyed by scope, scope locator, and Skill name. Global GitHub receipts contain a remote Git tree identity rather than an installed-content hash, so Beacon records the content SHA-256 and receipt fingerprint on first observation. A later content change with the same receipt fingerprint is locally modified; a receipt change establishes a new trusted baseline. This cannot identify edits made before Beacon's first observation and does not replace the Skills CLI receipt. The migration is transactional and idempotent, snapshots keep their independent payload version, and the public machine envelope remains schema v2 with additive optional history fields.
