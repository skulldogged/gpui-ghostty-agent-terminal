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

## GitHub communication

Write issues, pull-request descriptions, reviews, and comments for a human reader. Lead with the conclusion or user-visible result, use short prose paragraphs, and include only the evidence needed to support the decision.

Keep routine updates compact: state what was learned, what it means, and the next step. Do not publish a chronological lab notebook, raw trace narration, or unexplained implementation detail.

Never publish machine-specific or private information such as usernames, hostnames, absolute paths, shell-profile contents, personally installed programs, credentials, or local repository locations. Generalize those details unless they are essential to reproduction and the operator explicitly approves publishing them.

Prefer project-relative paths and portable commands when technical detail is necessary. Summarize long logs and test matrices; link or attach full evidence only when it materially helps review.

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
- Local Codex review is optional and risk-based, not a universal merge gate. Run one only when unresolved correctness, lifetime, resource-safety, security, or supported-platform risk would benefit from independent analysis; do not run one merely because a pull request is sizable, material, or ready to merge. Operator-confirmed testing and targeted validation are sufficient when they cover the change and no relevant risk remains unresolved. Do not request a GitHub `@codex review` unless the operator explicitly asks for one. Run another local review only after a material finding required changes.
- For a quick or small local review, inspect the scoped diff and run only targeted validation that adds new evidence. Do not invoke an unbounded review or repeat broad builds and test suites that already passed.
- `codex review --base` and `codex review --commit` are full independent reviews. The supported CLI cannot combine either selector with a custom scope prompt and may choose broad validation, so do not use them when the requested review must stay bounded.
