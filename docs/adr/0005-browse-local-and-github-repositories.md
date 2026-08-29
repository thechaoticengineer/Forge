# ADR-0005: Browse Local and GitHub Repositories Through the Engine

- Status: Accepted
- Date: 2026-08-29
- Decision owners: Project maintainers

## Context

Creating a draft currently requires typing an absolute local repository path. That is safe but does not provide the workshop-like entry point described by the product vision. Developers commonly keep projects below a small set of roots and also have repositories on GitHub that are not cloned on the current machine.

Repository discovery, GitHub authentication, identity matching, and cloning are filesystem and process operations. Running them from QML would violate the established engine boundary and could block or destabilize the long-lived `omarchy-shell` process. The product must also avoid exposing GitHub credentials, silently overwriting directories, following unbounded directory trees, or cloning merely because selection focus moved.

## Decision

The Rust engine will provide one repository catalog over the existing versioned IPC boundary. The initial catalog will:

- scan configured project roots for local Git worktrees, defaulting to `~/Projects`;
- allow project roots to be supplied as repeatable engine command-line options;
- bound scan time, depth, and directory count, avoid symbolic links, and skip hidden non-repository, dependency, build, and cache directories;
- identify GitHub-backed local repositories from common `origin` URL forms;
- use the developer's existing authenticated GitHub CLI installation to list repositories available through owner, collaborator, or organization membership affiliations;
- keep GitHub authentication material outside the orchestrator and disable interactive prompts;
- merge matching local and GitHub identities so the remote column contains only repositories that are not already local;
- return GitHub failure separately so local discovery remains usable offline or without `gh` authentication.

Cloning will be an explicit engine request for a validated `owner/name` identity. The engine will clone into the first configured project root, using `<root>/<repository>` normally and `<root>/<owner>/<repository>` when the simple name already exists. It will atomically reserve the selected directory, refuse symlink or dual-destination collisions, supervise `gh` and its Git child in one process group, and roll back the reserved destination when cloning fails. A repository already discovered locally with the requested GitHub identity will be opened instead of cloned again.

The QML panel will render the catalog as keyboard-navigable Local and GitHub areas with shared search. `Enter` opens a local repository or explicitly starts cloning the selected remote repository. Manual absolute-path entry and engine-backed path completion remain available for repositories outside configured roots.

The catalog is refreshed on request and is not durable workflow state. Drafts continue to persist the canonical local repository identity through the existing storage model.

## Consequences

### Positive

- Starting work no longer depends on remembering or typing an absolute path.
- Local and remote repositories form one entry workflow without duplicating local clones.
- Private and organization repositories use the user's existing GitHub CLI authentication.
- Offline and unauthenticated use still supports local repositories and manual paths.
- QML remains a thin presentation client.
- Clone destinations are predictable and collision-safe.

### Negative

- Repository discovery adds bounded filesystem and subprocess work each time the catalog refreshes.
- GitHub metadata depends on the installed `gh` CLI and its API compatibility.
- The initial project-root configuration is engine-startup configuration rather than editable durable UI settings.
- Matching depends on a recognizable GitHub `origin`; repositories with renamed remotes or non-GitHub mirrors may appear remote-only.
- Failed-clone rollback can itself fail after external filesystem interference; that condition is reported for manual inspection rather than deleting an unverified path.

### Follow-up

- Add durable project-root settings and management in the panel when configuration work is scheduled.
- Add owned, collaborating, forked, and archived filters if the catalog becomes difficult to navigate.
- Add cancellation and richer progress evidence for long-running clones.
- Consider cached metadata and incremental local refresh if observed catalog latency requires it.

## Alternatives Considered

### Keep only absolute-path entry

This preserves the smallest protocol surface but makes ordinary project selection unnecessarily manual and provides no path from a GitHub repository to a local workflow.

### Let QML scan directories and invoke GitHub or Git directly

This would make the interface initially self-contained but places blocking filesystem, authentication, and mutation work inside the unsandboxed shell process and duplicates logic needed by other clients.

### Always clone into `<root>/<owner>/<repository>`

This eliminates name collisions but changes the common flat `~/Projects/<repository>` layout. Owner namespacing only on collision preserves the simpler default while retaining a deterministic fallback.

### Store a durable repository inventory

A durable inventory could make startup faster, but it immediately introduces staleness, file watching, reconciliation, and migration concerns. On-demand bounded discovery is sufficient until measurements justify that complexity.

## References

- [ADR-0001](0001-separate-engine-from-shell-frontend.md)
- [ADR-0002](0002-use-versioned-json-over-a-unix-socket.md)
- [ADR-0003](0003-store-state-in-sqlite.md)
- [Product repository selection](../../README.md#ui-and-keyboard-interaction)
