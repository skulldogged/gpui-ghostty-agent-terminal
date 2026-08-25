# How Herdr detects Nix-wrapped Claude Code

Research basis: the current official `herdrdev/herdr` `master` at commit
[`6e8b138`](https://github.com/herdrdev/herdr/tree/6e8b138d0f7d7d695530657a6d8dc475bd3fba2b)
(2026-08-25, package version `0.8.2`) and isolated Linux runs of that exact
revision with Claude Code `2.1.234`. Source-proven behavior and live verification
are separated below.

## Source-proven behavior

On Linux, Herdr asks the kernel for the pane shell's terminal foreground process
group. It walks descendants from the shell and foreground group leader, retains
only live members of that same process group, and records each member's `comm`
from `/proc/<pid>/stat` plus its NUL-delimited argument vector from
`/proc/<pid>/cmdline`
([foreground-job collection](https://github.com/herdrdev/herdr/blob/6e8b138d0f7d7d695530657a6d8dc475bd3fba2b/src/platform/linux.rs#L142-L178),
[tree and process-group filtering](https://github.com/herdrdev/herdr/blob/6e8b138d0f7d7d695530657a6d8dc475bd3fba2b/src/platform/linux.rs#L229-L281),
[`comm` and `cmdline` reads](https://github.com/herdrdev/herdr/blob/6e8b138d0f7d7d695530657a6d8dc475bd3fba2b/src/platform/linux.rs#L318-L378)).
Detection tries the foreground group leader first, then every member of the
foreground job and retains the strongest recognized candidate
([job classification](https://github.com/herdrdev/herdr/blob/6e8b138d0f7d7d695530657a6d8dc475bd3fba2b/src/detect/mod.rs#L222-L249),
[leader/full-job probing](https://github.com/herdrdev/herdr/blob/6e8b138d0f7d7d695530657a6d8dc475bd3fba2b/src/pane.rs#L614-L676)).

Claude's exact process aliases are `claude` and `claude-code`. Herdr does not
substring-match arbitrary command lines. For each process it checks the effective
process name, understands common runtime wrappers, and then falls back to the exact
basename of `argv[0]` (or the first command-line token). The result is canonicalized
to `claude`
([agent aliases](https://github.com/herdrdev/herdr/blob/6e8b138d0f7d7d695530657a6d8dc475bd3fba2b/src/detect/mod.rs#L178-L220),
[normalization and fallback](https://github.com/herdrdev/herdr/blob/6e8b138d0f7d7d695530657a6d8dc475bd3fba2b/src/detect/mod.rs#L338-L390),
[exact path-token matching](https://github.com/herdrdev/herdr/blob/6e8b138d0f7d7d695530657a6d8dc475bd3fba2b/src/detect/mod.rs#L566-L630)).

Therefore, Linux metadata with `comm = ".claude-wrapped"` and
`argv[0] = "claude"` is recognized as Claude. The wrapper name itself is not a
special case; the exact known entrypoint in `argv[0]` is the evidence. Upstream has
a regression test for the equivalent `comm = ".claude-code-wrapped"` and
`argv[0] = "/nix/store/example/bin/claude-code"` case
([regression test](https://github.com/herdrdev/herdr/blob/6e8b138d0f7d7d695530657a6d8dc475bd3fba2b/src/detect/mod.rs#L1040-L1070)).
Host-visible sandbox or VM wrappers that conceal both the real process and a known
entrypoint remain a separate limitation; Herdr documents `HERDR_AGENT=claude` as an
explicit hint for those cases
([wrapper guidance](https://github.com/herdrdev/herdr/blob/6e8b138d0f7d7d695530657a6d8dc475bd3fba2b/docs/next/website/src/content/docs/agents.mdx#L39-L55)).

Process recognition and state classification are separate. Once Claude is known,
Herdr evaluates the live bottom-buffer screen and OSC evidence against Claude's
screen manifest to report `idle`, `working`, or `blocked`; Claude's optional
integration supplies session identity rather than replacing screen-state detection
([status authority](https://github.com/herdrdev/herdr/blob/6e8b138d0f7d7d695530657a6d8dc475bd3fba2b/docs/next/website/src/content/docs/agents.mdx#L12-L49)).

## Live verification

The exact upstream revision was built and run as `herdr 0.8.2` with isolated Herdr
configuration, state, runtime, and socket paths. A temporary workspace launched the
installed `claude` command without submitting a prompt. Herdr reported the live
foreground process as:

```json
{
  "foreground_process_group_id": 301758,
  "foreground_processes": [
    {
      "argv": ["claude"],
      "cmdline": "claude",
      "name": ".claude-wrapped",
      "pid": 301758
    }
  ]
}
```

Despite that wrapper `comm`, `herdr agent list` returned `agent: "claude"` and,
after Claude's UI settled, `agent_status: "idle"`. `herdr agent explain` confirmed
that screen detection was active and matched Claude's `live_prompt_box` idle rule.
An independent `herdr agent start claudecheck --kind claude` run likewise returned
`agent: "claude"`, `interactive_ready: true`, and an idle Claude Code title. No
agent prompt was submitted in either run; the temporary Claude processes and Herdr
servers were stopped and their isolated state removed afterward.

The pinned source regression also passed directly:

```text
cargo test --locked \
  identify_agent_in_job_canonicalizes_nix_wrapped_aliases_from_cmdline_argv0 \
  -- --nocapture

test result: ok. 1 passed; 0 failed
```

## Implication for Agent Terminal

The behavior to model is a structured two-stage seam:

1. discover the terminal's foreground process group and retain structured process
   metadata (`pid`, process-group identity, `comm`/executable, and argv);
2. identify a known agent from exact executable or `argv[0]` entrypoint semantics,
   including interpreter wrappers, before applying terminal-screen state rules.

For the observed failure, matching only Linux `comm` cannot work because the current
Nix package intentionally exposes `.claude-wrapped`. Treating the exact `argv[0]`
basename `claude` as the agent identity fixes that case while avoiding the false
positives that would come from searching every argument for the word `claude`.
