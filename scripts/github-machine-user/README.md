# GitHub machine-user commands

These wrappers keep agent-authored GitHub writes separate from the operator's
normal `gh` and Git identities:

- `agent-gh` permits pull-request, issue, and guarded REST operations as the
  configured machine account.
- `agent-git` creates signed commits and pushes as the configured machine
  account.
- `token-store` keeps the API token outside the repository and uses an
  owner-only file when Secret Service is unavailable.

Run `scripts/setup-github-machine-user.sh` from the repository root for the
initial account setup. On Windows, run it from Git Bash; it installs `.cmd`
shims so later PowerShell sessions can invoke the same commands. The wizard
preserves the operator's existing GitHub sessions.

Windows stores the API token as a current-user DPAPI credential. Each Windows
machine receives its own SSH authentication and signing keys, while the public
account identity and guarded command interface remain the same across machines.

## Register another repository

The commands infer the repository from the current checkout's GitHub `origin`
and require it to appear in the local registry. To onboard another repository:

1. Ask its owner to grant the configured machine account write access.
2. Accept the invitation as the machine account.
3. Run `agent-gh register OWNER/REPOSITORY`.
4. From that repository's checkout, verify `agent-gh identity` and
   `agent-git identity` before the first write.

Registration verifies write access with the machine token before changing the
local allowlist. Missing registration, missing access, identity mismatches, and
cross-repository overrides fail closed.

## Global Codex instruction

Put the following policy in `~/.codex/AGENTS.md` so newly started Codex
sessions use the machine account consistently:

```md
## GitHub identity

- Route GitHub mutations through `agent-gh`.
- Route agent-authored commits and pushes through `agent-git`.
- Before the first mutation, require `agent-gh identity` and `agent-git identity` to confirm the intended repository and machine account.
- If the repository is not registered or the machine account lacks access, ask the user to grant the reported machine account access to the repository and wait for that access to be accepted before continuing.
- If either identity check fails, stop and request repository onboarding.
- Use the personal `gh` session only for read-only inspection.
- Identity selection does not grant authorization for commits, pushes, pull requests, reviews, merges, or other external changes; retain the task's existing approval requirements.
```
