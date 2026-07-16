# Beacon

Beacon observes and safely manages developer-tooling resources selected by the user. Its language distinguishes the resource being managed from the external mechanism that performs lifecycle operations.

## Language

### Resources

**Tool**:
A command selected by the user's logical `PATH`. Beacon observes only the active executable.
_Avoid_: Package, application

**Inventory**:
A collection of installed resources that are not logical `PATH` tools, such as Homebrew items or Agent Skills.
_Avoid_: Tool list, package list

**Agent Skill**:
A reusable instruction set packaged around a `SKILL.md` file and installable into one or more coding agents.
_Avoid_: Prompt, plugin

**Third-party Skill**:
An Agent Skill whose source and release lifecycle are not owned by Beacon.
_Avoid_: Beacon Skill, external plugin

**Skill Inventory Item**:
An installed Agent Skill reported through Beacon's Inventory model. It is not a Tool because the Skill itself is not an executable selected from `PATH`.
_Avoid_: Skill tool, Skill executable

### Ownership and updates

**Tool Adapter**:
The role that discovers a Tool, interprets one version observation, and compares versions.
_Avoid_: Provider, manager

**Installation Source**:
The origin of an installed resource. It is evidence of provenance, not permission to update.
_Avoid_: Update manager, owner

**Install Manager**:
The role that understands receipts, channel policy, latest-revision semantics, and safe update actions for installed resources.
_Avoid_: Tool adapter, installation source

**Update Manager**:
The unique Install Manager authorized to update an installed resource. It may differ from the Installation Source.
_Avoid_: Installation source, provider

**Receipt**:
Package-manager-owned metadata that records the provenance and installed revision of a managed resource. Beacon may use a Receipt as evidence but does not own or rewrite it.
_Avoid_: Beacon lock, cache

**Exact Action**:
An update whose resulting version must equal the confirmed expected version.
_Avoid_: Pinned update

**Floating Action**:
An update that preserves manager policy and must produce a newer result no lower than the confirmed candidate.
_Avoid_: Latest update

**Rolling Action**:
An update that preserves a moving channel and must change the observed revision while allowing the channel head to advance after confirmation.
_Avoid_: Floating action

**Unmanaged**:
An installed resource for which no unique safe update action exists. Beacon diagnoses it but does not update it.
_Avoid_: Unsupported, failed

**Missing**:
A selected Tool that is absent from the logical `PATH`.
_Avoid_: Unmanaged, disabled

### Agent Skill management

**Skill Package Manager**:
The Beacon capability that presents third-party Agent Skills as managed resources throughout their lifecycle.
_Avoid_: Beacon Skill updater, plugin manager

**Skill Scope**:
The installation boundary that distinguishes a global Agent Skill from an Agent Skill belonging to the current project. A Skill name is unique only within its scope.
_Avoid_: Skill location, agent scope

**Skill Project**:
The project boundary whose Agent Skills form the project Skill Scope for the current Beacon operation.
_Avoid_: Current directory, repository

**Skill Revision**:
The content identity of an installed Agent Skill, represented by a SHA-256 hash of its files rather than a semantic version.
_Avoid_: Skill version, package version

**Locally Modified Skill**:
An installed Agent Skill whose current revision differs from the revision recorded by its package manager before any remote update is considered.
_Avoid_: Dirty Skill, forked Skill

### Results

**Partial Result**:
A command outcome containing valid data alongside isolated structured errors.
_Avoid_: Failure, best effort
