# Beacon domain glossary

- **Tool**: a command selected by the user's logical `PATH`. Beacon observes only that active executable.
- **Tool adapter**: built-in code that discovers a tool, parses one immutable version observation, and owns version comparison.
- **Installation source**: where the active executable came from. This is evidence, not permission to update it.
- **Install manager**: built-in code that understands receipts, channel policy, latest-version semantics, and safe update actions.
- **Update manager**: the unique install manager authorized to update the active installation. It may differ from the installation source; npm inside a mise Node runtime is `mise → npm`.
- **Receipt**: manager-owned evidence that outranks canonical-path and path-heuristic evidence.
- **Inventory**: installed items that are not logical PATH tools. Homebrew formulae and casks use qualified IDs such as `brew:formula:wget` and `brew:cask:firefox`.
- **Exact action**: verification requires the resulting version to equal the confirmed expected version.
- **Floating action**: manager policy is preserved; verification requires a newer result no lower than the confirmed candidate.
- **Unmanaged**: the tool is installed but no unique safe updater exists. Beacon diagnoses it and emits no action.
- **Missing**: the tool is not present on `PATH`. Beacon reports it without latest lookup or upgrade action.
- **Partial result**: machine output with valid data and structured errors (`status: "partial"`, exit 2). Failures stay isolated; agents must not invent missing fields from human terminal output.
