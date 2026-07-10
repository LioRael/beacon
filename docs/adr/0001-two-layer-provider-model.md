# ADR 0001: Two-layer provider model

Status: accepted for Beacon 0.2.

Beacon models tools with compile-time `ToolAdapter` trait objects and installation channels with compile-time `InstallManager` trait objects. Adapters own discovery, parsing, and comparison. Managers own snapshots, source/updater claims, latest semantics, and action planning. The orchestrator alone executes confirmed actions.

Installation source and update manager are independent. Claims rank receipt above canonical path above path heuristic. Equal top-confidence claims are a conflict: the report is unmanaged and has no update action. An unknown source does not by itself block a unique updater.

This design keeps execution injectable, commands argument-separated, and extension built-in. A dynamic plugin ABI and arbitrary user-defined commands remain out of scope.
