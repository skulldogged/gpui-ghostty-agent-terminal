## Agent skills

### Issue tracker

Issues and specs are tracked in this repository's GitHub Issues. See `docs/agents/issue-tracker.md`.

### Resuming work

When starting, resuming, or moving implementation between development machines, follow `docs/agents/cross-machine-work.md`.

### Native GitHub stacks

Call pull requests a native GitHub stack only after GitHub has created a stack object. A compatible base/head branch chain is a prerequisite, not proof of membership. Create or link the stack with GitHub's stack UI, `gh stack`, or the Stacks REST API; then read it back with API version `2026-03-10` and verify the pull request's non-null `stack` field plus the ordered pull request list from `/repos/{owner}/{repo}/stacks/{stack_number}`. Report the stack number, position, and size when declaring the stack complete.

### Triage labels

Triage uses the five default Matt Pocock skill labels. See `docs/agents/triage-labels.md`.

### Domain docs

This is a single-context repository. See `docs/agents/domain.md`.

## Code Review Rules

### Domain invariants

- Use the terms and ownership boundaries defined in `CONTEXT.md`. Flag changes that make a Pane own a Terminal Session's identity or lifetime, or that make a native window own Spaces or terminal processes instead of the tray-resident Application.
- Closing every native window must leave the Application and its Terminal Sessions running when desktop presence is available. Explicit Quit ends the Application and its Terminal Sessions; process-restart survival is not a requirement.
- Agent Integration must remain optional. A recognized agent program must still work as a normal interactive Terminal Session when richer integration is absent, incompatible, or fails.

### Terminal correctness

- Flag code that can alter, decode, truncate, or reorder PTY bytes before libghostty-vt consumes them, or that bypasses libghostty-vt as the terminal-state authority.
- Terminal dimensions must be derived from the rendered viewport and verified fixed-cell metrics. Resizes must update both terminal state and the platform PTY/ConPTY transport.
- Preserve wide-cell semantics: a wide glyph occupies two cells and its trailing cell is not rendered independently.
- Platform-specific terminal transport changes must preserve macOS and Linux PTY behavior and Windows ConPTY behavior. Resource and process handles must be closed on every success, error, and shutdown path.

### Review signal

- Report correctness, process-lifetime, resource-safety, security, and supported-platform regressions introduced by the change. Do not report style preferences, naming opinions, or speculative abstractions without a concrete failure mode.
- Use local Codex review proportionally. Run one when a material PR becomes a merge candidate or when the change carries meaningful correctness, lifetime, resource-safety, security, or supported-platform risk. For a trivial low-risk change—such as a few documentation lines or an obvious small single-file edit—use targeted validation and omit the full review. Do not request a GitHub `@codex review` unless the operator explicitly asks for one. Run another local review only after a material finding required changes.
