# Cross-machine work

GitHub is the durable coordination surface. A development machine is an
interchangeable executor, not the owner of project context.

## Sources of truth

- The issue owns scope, accepted decisions, and completion criteria.
- The draft pull request owns live implementation status, validation results,
  and the next concrete step.
- The branch owns code and reviewable checkpoints.
- `CONTEXT.md` and the domain docs own stable architecture and vocabulary.

Keep hostnames, private paths, credentials, and descriptions of the testing
environment out of issues and pull requests. Record only the operating system,
relevant configuration, command, result, and user-visible evidence.

## Start or resume

1. Read `AGENTS.md` and `CONTEXT.md`.
2. Fetch the relevant issue with its comments and find its linked pull request.
3. Read the pull-request description, review threads, checks, and latest branch
   commit before editing.
4. Fetch the branch and verify that the working tree starts clean.
5. Before the first write, run `agent-gh identity` and `agent-git identity`.
   Repository registration and write access must both succeed.

The work is resumed when the local checkout matches the pull-request head and
the agent can state the remaining acceptance criteria and next step from GitHub
alone.

## Publish a checkpoint

Create a draft pull request after the first coherent, signed commit. Keep each
pull request focused on one independently reviewable slice. Before leaving a
machine:

1. Run the validation appropriate to the current checkpoint.
2. Commit through `agent-git` and push the branch through `agent-git`.
3. Update the draft pull-request description with completed criteria, exact
   validation results, known gaps, and the next concrete step.
4. Leave the working tree clean.

Use an issue or pull-request comment only for a durable decision, blocker, or
review response. Update the description for routine progress so resuming does
not require reconstructing state from a comment stream.

## Review and merge

When the implementation and required platform evidence are complete, mark the
pull request ready. Request a GitHub `@codex review` only when the operator
explicitly asks for one; that request is the sole GitHub mutation made with the
operator's session, while all other agent-authored GitHub writes use `agent-gh`.

Merge after required checks and issue acceptance criteria are green. Treat a
review as an additional merge gate only when branch protection requires it, the
operator requests it, or a relevant unresolved risk justifies it under the
repository's review rules. The merged pull request and closed issue become the
permanent handoff record.
