## Agent skills

### Issue tracker

Issues and specs are tracked in this repository's GitHub Issues. See `docs/agents/issue-tracker.md`.

### Triage labels

Triage uses the five default Matt Pocock skill labels. See `docs/agents/triage-labels.md`.

### Domain docs

This is a single-context repository. See `docs/agents/domain.md`.

## Code Review Rules

### Domain invariants

- Use the terms and ownership boundaries defined in `CONTEXT.md`. Flag changes that make a Pane own a Terminal Session's identity or lifetime, or that make a UI Client own Spaces or terminal processes instead of the Resident Core.
- Agent Integration must remain optional. A recognized agent program must still work as a normal interactive Terminal Session when richer integration is absent, incompatible, or fails.

### Terminal correctness

- Flag code that can alter, decode, truncate, or reorder PTY bytes before libghostty-vt consumes them, or that bypasses libghostty-vt as the terminal-state authority.
- Terminal dimensions must be derived from the rendered viewport and verified fixed-cell metrics. Resizes must update both terminal state and the platform PTY/ConPTY transport.
- Preserve wide-cell semantics: a wide glyph occupies two cells and its trailing cell is not rendered independently.
- Platform-specific terminal transport changes must preserve macOS and Linux PTY behavior and Windows ConPTY behavior. Resource and process handles must be closed on every success, error, and shutdown path.

### Review signal

- Report correctness, process-lifetime, resource-safety, security, and supported-platform regressions introduced by the change. Do not report style preferences, naming opinions, or speculative abstractions without a concrete failure mode.
