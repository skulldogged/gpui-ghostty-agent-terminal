# Agent integration surfaces for a terminal-first multiplexer

Research date: 2026-08-19

This report answers [research ticket #6](https://github.com/skulldogged/gpui-ghostty-agent-terminal/issues/6) using pinned first-party documentation and source repositories.

## Executive conclusion

The application should treat every pane as a real terminal first and layer an optional, capability-negotiated agent adapter beside it. There is no universal integration protocol across the priority agents:

- OpenCode has the best terminal-preserving integration: its normal TUI is already a client of an HTTP server, so a second client can consume typed state and events without replacing the TUI.
- Claude Code can preserve its TUI while command or localhost HTTP hooks report lifecycle and permission events.
- Pi can preserve its TUI through an explicitly loaded extension. Its RPC and SDK modes are substantially richer, but then the host owns the interaction UI.
- Codex has the strongest rich-client protocol in `codex app-server`, including typed lifecycle events and bidirectional approvals. Its hook implementation can report events from the regular CLI, but the public compatibility promise is clearest for app-server, not for hooks as a third-party observer API.
- ACP is real first-party support for OpenCode and Gemini CLI. It is not a common denominator for Codex, Claude Code, or Pi at the revisions surveyed, and ACP mode is an alternative client-rendered mode rather than a way to observe an already-running TUI.

Accordingly, the product should expose a capability ladder rather than label a pane "integrated" or "not integrated." An arbitrary CLI remains fully usable at level 0; cooperative hooks, plugins, servers, and protocols progressively add authoritative state, resume, control, and approvals. Screen scraping, prompt matching, process names, CPU idleness, and transcript tailing may improve discovery, but they must remain explicitly low-confidence heuristics and must never override live cooperative data.

## Scope and source basis

This survey prioritizes first-party documentation and source. Repository observations are pinned to:

- [OpenAI Codex `e741cd9`](https://github.com/openai/codex/tree/e741cd9ace0286bf0be3d5e1c7f569b345e33ee5)
- [Claude Code `c3d2e35`](https://github.com/anthropics/claude-code/tree/c3d2e35e554060b5a20ee6b28140fbdbd4eb0048), supplemented by the live first-party Claude Code documentation because the public repository does not contain the full product source
- [Pi `1355cd3`](https://github.com/earendil-works/pi/tree/1355cd36e0b10a3e71c6c78f713b7b36458db27f)
- [OpenCode `da4730e`](https://github.com/anomalyco/opencode/tree/da4730e4a41dcbb2cb2d907dd2b06ac481b8f962)
- [Gemini CLI `571851b`](https://github.com/google-gemini/gemini-cli/tree/571851b1077a51cef757146ce13f9da887326bec)

"Authoritative" below means that the running agent cooperatively emitted or returned the datum. It does not mean that every surface has a long-term compatibility guarantee.

## Surface matrix

| Agent | Keep native TUI and add a side channel | Rich control surface | Resume identity | Approval interface | Machine-readable output | First-party ACP | Stability signal |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Codex | Lifecycle command hooks exist in current source; public observer-contract maturity is unclear | `codex app-server`, bidirectional JSON-RPC-like JSONL | Thread/session UUID or thread name; `--last` | App-server server requests with correlated decisions | `codex exec --json` JSONL | No documented entry point found | App-server explicitly separates stable default and experimental opt-in; schemas are binary-version-specific |
| Claude Code | Command or localhost HTTP hooks while the TUI remains in the PTY | Agent SDK or `-p` stream-JSON mode | `session_id`; `--resume`, `--continue`, optional fork | `PermissionRequest` hook; SDK `canUseTool` | JSON and stream-JSON | No documented entry point found | Stream `system/init` advertises capabilities; feature-detect rather than version-guess |
| Pi | Global or CLI `-e` extension receives in-process lifecycle/tool events | JSONL RPC subprocess or in-process TypeScript SDK | Session UUID and session file/path; partial IDs accepted by CLI | No general built-in execution approval; extension may block tools and render a confirmation | JSON event mode and RPC JSONL | No documented entry point found | Package is pre-1.0; session files are versioned/migrated, RPC has no documented handshake version |
| OpenCode | Normal TUI starts a server; connect a second SDK/SSE client to a chosen loopback port | OpenAPI HTTP server/SDK; TUI-control endpoints; ACP alternative | Session ID; `--session`, `--continue`, optional fork | Permission events plus reply endpoint; ACP maps these to `requestPermission` | OpenAPI responses and SSE events | Yes, `opencode acp` over JSON-RPC stdio | Health reports server version; SDK is generated from OpenAPI and released with the app; pin/probe |
| Gemini CLI (reference adapter) | Command hooks can report lifecycle while TUI remains in PTY | `gemini --acp` | Full UUID or latest/index via `--resume`; ACP `loadSession` | `BeforeTool` can block; notification hook observes permission prompts; ACP owns client permissions | JSON and stream-JSON | Yes | ACP includes an explicitly `unstable_` model method; pin and negotiate |

## What the terminal host knows without cooperation

When this application launches the PTY occupant, these are facts owned by the application itself:

- launch profile, executable and arguments chosen by the user;
- initial working directory and environment supplied by the host;
- PTY/process identifiers, bytes transferred, focus and user input;
- process start, stop, signal, and exit status.

Those facts establish that a process is alive, not that an agent is thinking, idle, compacting, waiting for approval, or safe to interrupt. A shell may replace or wrap a process, agents launch children, and users may start an agent manually after the pane starts. Process-tree inspection, executable matching, terminal titles, OSC metadata, prompt regexes, screen text, filesystem changes, transcript tails, and CPU utilization are therefore **heuristics**. They should carry a source and confidence, expire quickly, and never override cooperative state.

Agent-provided environment variables are also usually the wrong direction for host discovery. For example, Codex inserts `CODEX_THREAD_ID` into the environment it constructs for model-reachable child commands, not into the already-running parent terminal host ([source](https://github.com/openai/codex/blob/e741cd9ace0286bf0be3d5e1c7f569b345e33ee5/codex-rs/protocol/src/shell_environment.rs#L52-L69), [insertion](https://github.com/openai/codex/blob/e741cd9ace0286bf0be3d5e1c7f569b345e33ee5/codex-rs/protocol/src/shell_environment.rs#L148-L155)). OpenCode's `shell.env` plugin hook likewise customizes child shell environments rather than identifying the parent TUI ([plugin events](https://github.com/anomalyco/opencode/blob/da4730e4a41dcbb2cb2d907dd2b06ac481b8f962/packages/web/src/content/docs/plugins.mdx#L142-L208)). Treat such variables as correlation aids inside agent-launched commands, not as proof visible from the multiplexer.

The verified process/environment picture is therefore:

| Agent | Reliable host-visible locator | Environment qualification |
| --- | --- | --- |
| Codex | Host-owned process lifetime; app-server stdio/socket after a successful handshake | `CODEX_THREAD_ID` is injected into agent-launched commands, not exported backward to the parent host |
| Claude Code | Host-owned process lifetime; hook delivery carrying `session_id` | `CLAUDE_ENV_FILE` lets `SessionStart` hooks persist variables for later Bash commands; it is not a parent-process discovery signal ([hooks reference](https://code.claude.com/docs/en/hooks)) |
| Pi | Host-owned process lifetime; explicit extension bridge, RPC stdio, or SDK object | No documented environment variable identifies a live Pi session to its parent; use the extension/RPC session ID |
| OpenCode | Host-chosen loopback URL plus per-launch server credentials | `OPENCODE_SERVER_PASSWORD` is a real launch/authentication contract; `shell.env` applies to child shells |
| Gemini CLI | Host-owned process lifetime; hook delivery or ACP stdio handshake | Hook `session_id` is cooperative identity; no documented environment variable identifies the live session to its parent |

Absence in this table is a scoped research finding at the pinned revisions, not a guarantee that an undocumented variable never exists. Depending on undocumented variables would still be unsuitable for a supported adapter.

## OpenAI Codex

### Verified facts

`codex app-server` is the interface used for rich Codex clients. Its default transport is newline-delimited, bidirectional JSON-RPC-style messages over stdio; Unix sockets are supported, while WebSocket transport is explicitly experimental and unsupported ([protocol](https://github.com/openai/codex/blob/e741cd9ace0286bf0be3d5e1c7f569b345e33ee5/codex-rs/app-server/README.md#L20-L44)). A connection must initialize, then start, resume, or fork a thread, start a turn, consume item/turn events, and receive `turn/completed` ([lifecycle](https://github.com/openai/codex/blob/e741cd9ace0286bf0be3d5e1c7f569b345e33ee5/codex-rs/app-server/README.md#L76-L87)).

The event stream is authoritative for rich-client activity: `turn/started` carries `inProgress`; `turn/completed` carries `completed`, `interrupted`, or `failed`; item lifecycles run from `item/started` through deltas to `item/completed` ([events](https://github.com/openai/codex/blob/e741cd9ace0286bf0be3d5e1c7f569b345e33ee5/codex-rs/app-server/README.md#L1585-L1599)). Approvals are server-initiated requests correlated by thread, turn, item, and request identifiers. The client receives the proposed command or diff, returns a decision, observes `serverRequest/resolved`, and then treats the terminal item status as the authoritative outcome ([command approvals](https://github.com/openai/codex/blob/e741cd9ace0286bf0be3d5e1c7f569b345e33ee5/codex-rs/app-server/README.md#L1683-L1710), [permission profiles](https://github.com/openai/codex/blob/e741cd9ace0286bf0be3d5e1c7f569b345e33ee5/codex-rs/app-server/README.md#L1750-L1794)).

For non-interactive execution, `codex exec --json` emits JSONL ([CLI flag](https://github.com/openai/codex/blob/e741cd9ace0286bf0be3d5e1c7f569b345e33ee5/codex-rs/exec/src/cli.rs#L42-L69)). Its first `thread.started` event contains the thread ID needed to resume; subsequent events describe turns and item terminal states ([event types](https://github.com/openai/codex/blob/e741cd9ace0286bf0be3d5e1c7f569b345e33ee5/codex-rs/exec/src/exec_events.rs#L8-L43)). The exec CLI can resume by UUID or thread name, choose the most recent thread, or fork a named thread ([resume arguments](https://github.com/openai/codex/blob/e741cd9ace0286bf0be3d5e1c7f569b345e33ee5/codex-rs/exec/src/cli.rs#L143-L187)).

Current source also defines command-hook events for `SessionStart`, `UserPromptSubmit`, `PreToolUse`, `PermissionRequest`, `PostToolUse`, `Stop`, compaction, and subagents ([hook event union](https://github.com/openai/codex/blob/e741cd9ace0286bf0be3d5e1c7f569b345e33ee5/codex-rs/hooks/src/schema.rs#L85-L121)). Inputs include a session ID, and turn-scoped hooks include a turn ID ([lifecycle input schemas](https://github.com/openai/codex/blob/e741cd9ace0286bf0be3d5e1c7f569b345e33ee5/codex-rs/hooks/src/schema.rs#L483-L588)). This is evidence that a terminal-preserving bridge is technically possible, but the inspected first-party materials do not give these hooks the same explicit stable-client promise as app-server.

App-server's stability boundary is unusually clear: stable-only is the default, experimental fields require capability opt-in, and generated TypeScript or JSON Schema can exclude experiments. Generated schemas are exact for the installed Codex version ([schema generation](https://github.com/openai/codex/blob/e741cd9ace0286bf0be3d5e1c7f569b345e33ee5/codex-rs/app-server/README.md#L57-L64), [stability contract](https://github.com/openai/codex/blob/e741cd9ace0286bf0be3d5e1c7f569b345e33ee5/codex-rs/app-server/README.md#L2458-L2509)). At the pinned revision, no first-party ACP command or ACP documentation was found; Codex's native rich-client surface is app-server.

### Integration conclusion

Build an app-server adapter for a later host-rendered rich pane and generate bindings from the exact installed binary. For the first terminal-preserving release, either use a narrowly scoped lifecycle-hook bridge behind version checks or provide only launch/exit plus explicit user association. Do not parse Codex rollout/transcript files for live status and do not claim a pending approval from screen text.

## Claude Code

### Verified facts

Claude Code supports `--continue` and `--resume <session>`; `--fork-session` branches instead of continuing in place. Programmatic mode can emit `json` or `stream-json`, and its initialization message advertises metadata and optional capabilities. Anthropic explicitly recommends feature detection from `system/init` rather than comparing version strings ([CLI reference](https://code.claude.com/docs/en/cli-reference), [programmatic mode](https://code.claude.com/docs/en/headless)). The JSON result contains a session ID, which can be passed back to resume a specific session ([session workflow](https://code.claude.com/docs/en/agent-sdk/sessions)).

Hooks are the strongest terminal-preserving side channel. They run in terminal, IDE, desktop, and web sessions and can be command, HTTP, MCP-tool, prompt, or agent hooks depending on event. Command hooks receive JSON on stdin; HTTP hooks receive the same JSON in a POST. Common fields include `session_id`, transcript path, cwd, permission mode, and event name. The transcript can lag the live event, so it is useful for later context but is not the event authority ([hooks reference](https://code.claude.com/docs/en/hooks)).

The lifecycle includes `SessionStart`, `UserPromptSubmit`, tool pre/post events, `PermissionRequest`, `Stop`, `StopFailure`, and `SessionEnd`. `PermissionRequest` fires when permission is needed and may return a structured allow or deny decision; notification hooks are useful for observation but are not the decision channel. `Stop` means the main agent has finished responding, while `StopFailure` covers API failures and `SessionEnd` identifies teardown reason ([hooks reference](https://code.claude.com/docs/en/hooks)). This makes hook payloads authoritative for the event that fired. A host-derived label such as “idle” remains an inference from the latest event and should become unknown if delivery fails.

The Claude Agent SDK provides the richer alternate mode: typed sessions, hooks, tools, permission modes, and a runtime `canUseTool` callback for approvals ([SDK overview](https://code.claude.com/docs/en/agent-sdk/overview), [SDK permissions](https://code.claude.com/docs/en/agent-sdk/permissions)). This is a host-rendered agent session, not passive observation of the normal TUI. Anthropic's SDK documentation also imposes an authentication/product boundary: third-party products generally use API-key authentication unless separately approved for Claude.ai login. Retaining the user's installed CLI in a PTY avoids silently changing that product model ([SDK overview](https://code.claude.com/docs/en/agent-sdk/overview)).

At the pinned revision and in the current first-party docs, no Claude Code ACP entry point was found. Hooks, stream-JSON, and the Agent SDK are the supported first-party paths.

### Integration conclusion

Supply a per-launch localhost HTTP hook configuration and random bearer-like path/token, or a command hook that writes to a private local socket. This preserves the native TUI and yields session, lifecycle, tool, and permission events. Keep approval forwarding opt-in and synchronous. Add an Agent SDK adapter only for a distinct host-rendered experience with explicit authentication expectations.

## Pi

### Verified facts

Pi stores each session as a tree-structured JSONL file and supports continue, browse, explicit path or partial ID, and fork from the CLI ([sessions](https://github.com/earendil-works/pi/blob/1355cd36e0b10a3e71c6c78f713b7b36458db27f/packages/coding-agent/docs/sessions.md#L1-L20)). The file header contains a format version; legacy v1 and v2 sessions are migrated to v3 on load ([format versions](https://github.com/earendil-works/pi/blob/1355cd36e0b10a3e71c6c78f713b7b36458db27f/packages/coding-agent/docs/session-format.md#L19-L37)).

`pi --mode json` emits a one-way JSON event stream whose first record includes the session UUID and format version, followed by agent, turn, message, and tool events. `message_end` is the final authoritative message ([JSON mode](https://github.com/earendil-works/pi/blob/1355cd36e0b10a3e71c6c78f713b7b36458db27f/packages/coding-agent/docs/json.md#L1-L31), [wire example](https://github.com/earendil-works/pi/blob/1355cd36e0b10a3e71c6c78f713b7b36458db27f/packages/coding-agent/docs/json.md#L67-L92)).

`pi --mode rpc` is a bidirectional, strict LF-delimited JSONL protocol over stdin/stdout with correlated commands, responses, and asynchronous events ([RPC framing](https://github.com/earendil-works/pi/blob/1355cd36e0b10a3e71c6c78f713b7b36458db27f/packages/coding-agent/docs/rpc.md#L1-L37)). It supports prompt, steer, follow-up, abort, new session, and state reads; `get_state` returns `isStreaming`, `isCompacting`, session file, session ID/name, and pending queue counts ([commands and state](https://github.com/earendil-works/pi/blob/1355cd36e0b10a3e71c6c78f713b7b36458db27f/packages/coding-agent/docs/rpc.md#L39-L76), [state response](https://github.com/earendil-works/pi/blob/1355cd36e0b10a3e71c6c78f713b7b36458db27f/packages/coding-agent/docs/rpc.md#L160-L195)). Its event stream includes retries, compaction, tools, queues, and `agent_settled`; unlike low-level `agent_end`, `agent_settled` means Pi will not automatically retry, compact-and-retry, or process a queued follow-up ([RPC event](https://github.com/earendil-works/pi/blob/1355cd36e0b10a3e71c6c78f713b7b36458db27f/packages/coding-agent/docs/rpc.md#L832-L887), [extension semantics](https://github.com/earendil-works/pi/blob/1355cd36e0b10a3e71c6c78f713b7b36458db27f/packages/coding-agent/docs/extensions.md#L567-L580)). RPC mode also contains a correlated UI request/response subprotocol for extension selects, confirms, inputs, and editors, so an extension can ask the embedding host for input ([RPC extension UI](https://github.com/earendil-works/pi/blob/1355cd36e0b10a3e71c6c78f713b7b36458db27f/packages/coding-agent/docs/rpc.md#L1161-L1188)).

The same events are available to extensions inside the normal TUI. A `tool_call` extension handler runs before execution and can mutate or block the call ([tool interception](https://github.com/earendil-works/pi/blob/1355cd36e0b10a3e71c6c78f713b7b36458db27f/packages/coding-agent/docs/extensions.md#L758-L799)). This enables an optional permission extension, but Pi itself deliberately has no general sandbox or execution-approval boundary: it runs with the user's permissions, and project trust only gates loading project resources ([security model](https://github.com/earendil-works/pi/blob/1355cd36e0b10a3e71c6c78f713b7b36458db27f/packages/coding-agent/docs/security.md#L1-L37)). Do not describe project trust as tool approval.

For in-process TypeScript integrations, the SDK's `AgentSession` exposes prompts, steering, subscriptions, session file, and session ID ([SDK](https://github.com/earendil-works/pi/blob/1355cd36e0b10a3e71c6c78f713b7b36458db27f/packages/coding-agent/docs/sdk.md#L16-L42), [interface](https://github.com/earendil-works/pi/blob/1355cd36e0b10a3e71c6c78f713b7b36458db27f/packages/coding-agent/docs/sdk.md#L66-L85)). The pinned package is version `0.84.2`, and the RPC documentation does not define a protocol-version handshake. The versioned, migrated session file is a more explicit persistence contract than the RPC wire contract. No first-party ACP mode was found.

### Integration conclusion

Ship a small global or explicitly supplied `-e` extension that emits a versioned envelope to a private local bridge. It can preserve the native TUI and report exact session/activity/tool events. Treat extension-based permission prompts as an application policy layer, not a native Pi safety feature. Pin and integration-test Pi versions. Use RPC or the SDK only when the application is prepared to own the complete interaction UI.

## OpenCode

### Verified facts

The normal `opencode` command starts the TUI, and its flags include continue, explicit session ID, fork, auto-approval, hostname, and port ([CLI](https://github.com/anomalyco/opencode/blob/da4730e4a41dcbb2cb2d907dd2b06ac481b8f962/packages/web/src/content/docs/cli.mdx#L8-L45)). Internally, the TUI is already a client of an HTTP server. Supplying hostname and port makes that server discoverable by another local client, and TUI endpoints can prefill or execute prompts ([server architecture](https://github.com/anomalyco/opencode/blob/da4730e4a41dcbb2cb2d907dd2b06ac481b8f962/packages/web/src/content/docs/server.mdx#L47-L68)). This is the cleanest verified way to retain an agent's native TUI while adding rich integration.

The server publishes OpenAPI 3.1, exposes health plus version, and provides SSE events. Session endpoints list/create/fork/abort sessions, return per-session status, diffs, and todos, and reply to a specific permission request ([server contract](https://github.com/anomalyco/opencode/blob/da4730e4a41dcbb2cb2d907dd2b06ac481b8f962/packages/web/src/content/docs/server.mdx#L72-L95), [session endpoints](https://github.com/anomalyco/opencode/blob/da4730e4a41dcbb2cb2d907dd2b06ac481b8f962/packages/web/src/content/docs/server.mdx#L146-L168)). Generated types define authoritative session statuses `idle`, `retry`, and `busy`, plus permission update/reply events with session and request IDs ([generated event types](https://github.com/anomalyco/opencode/blob/da4730e4a41dcbb2cb2d907dd2b06ac481b8f962/packages/sdk/js/src/gen/types.gen.ts#L423-L480)). Plugins can additionally subscribe to permission, session, message, todo, tool, and shell events ([plugin events](https://github.com/anomalyco/opencode/blob/da4730e4a41dcbb2cb2d907dd2b06ac481b8f962/packages/web/src/content/docs/plugins.mdx#L142-L208)).

OpenCode permissions resolve to allow, ask, or deny. An ask offers once, always-for-session patterns, or reject ([permission policy](https://github.com/anomalyco/opencode/blob/da4730e4a41dcbb2cb2d907dd2b06ac481b8f962/packages/web/src/content/docs/permissions.mdx#L6-L36), [prompt outcomes](https://github.com/anomalyco/opencode/blob/da4730e4a41dcbb2cb2d907dd2b06ac481b8f962/packages/web/src/content/docs/permissions.mdx#L191-L199)). The server can be protected with HTTP Basic authentication through `OPENCODE_SERVER_PASSWORD`; it defaults to loopback ([authentication](https://github.com/anomalyco/opencode/blob/da4730e4a41dcbb2cb2d907dd2b06ac481b8f962/packages/web/src/content/docs/server.mdx#L13-L43)). The application should choose a port and per-launch random password rather than trying to discover the TUI's random port.

OpenCode also has actual first-party ACP support: `opencode acp` starts a JSON-RPC stdio subprocess for an ACP client ([ACP](https://github.com/anomalyco/opencode/blob/da4730e4a41dcbb2cb2d907dd2b06ac481b8f962/packages/web/src/content/docs/acp.mdx#L1-L21)). The implementation translates OpenCode permission requests into ACP `requestPermission` choices and fails closed when the client cannot handle them ([permission bridge](https://github.com/anomalyco/opencode/blob/da4730e4a41dcbb2cb2d907dd2b06ac481b8f962/packages/opencode/src/acp/permission.ts#L16-L96)). Some built-in slash commands remain unsupported over ACP ([support note](https://github.com/anomalyco/opencode/blob/da4730e4a41dcbb2cb2d907dd2b06ac481b8f962/packages/web/src/content/docs/acp.mdx#L143-L149)). ACP is therefore a rich alternate client mode, not the preferred observer for the existing TUI.

The pinned OpenCode and generated SDK packages are both `1.18.18`; the pinned ACP SDK dependency is `0.21.0`. OpenAPI generation and `/global/health` provide good version/probe points, but the surveyed documentation does not promise cross-version compatibility. Pin a tested range and derive capabilities from the live spec/health response.

### Integration conclusion

Make OpenCode the first deep adapter. Launch its ordinary TUI with a reserved loopback port and random server password, consume SSE, query status after reconnect/gaps, and reply to approvals by exact request ID. This gives authoritative state without altering the terminal experience. Keep ACP as a separate future host-rendered mode.

## Gemini CLI as an ACP reference

Gemini CLI is not required for the first four adapters, but it is a useful second first-party ACP implementation. `gemini --acp` runs JSON-RPC 2.0 over stdio and supports initialize, authentication, new/load session, prompt, cancel, approval modes, and a proxied filesystem ([ACP protocol](https://github.com/google-gemini/gemini-cli/blob/571851b1077a51cef757146ce13f9da887326bec/docs/cli/acp-mode.md#L1-L20), [capabilities](https://github.com/google-gemini/gemini-cli/blob/571851b1077a51cef757146ce13f9da887326bec/docs/cli/acp-mode.md#L40-L99)). Its normal CLI resumes by latest, list index, or full session UUID ([sessions](https://github.com/google-gemini/gemini-cli/blob/571851b1077a51cef757146ce13f9da887326bec/docs/cli/session-management.md#L24-L53)) and offers text, JSON, and stream-JSON output.

Terminal-preserving command hooks receive a session ID and lifecycle/tool event names. `BeforeTool` may block or rewrite input, while the `Notification` `ToolPermission` event is observability-only and cannot grant permission ([hook schema](https://github.com/google-gemini/gemini-cli/blob/571851b1077a51cef757146ce13f9da887326bec/docs/hooks/reference.md#L1-L58), [tool hook](https://github.com/google-gemini/gemini-cli/blob/571851b1077a51cef757146ce13f9da887326bec/docs/hooks/reference.md#L92-L112), [notification hook](https://github.com/google-gemini/gemini-cli/blob/571851b1077a51cef757146ce13f9da887326bec/docs/hooks/reference.md#L272-L285)). The ACP surface includes a method named `unstable_setSessionModel`, so clients must negotiate and avoid assuming all methods have equal stability.

This confirms that a shared ACP adapter is worthwhile after the agent-specific terminal side channels, but it does not justify wrapping non-ACP agents with unverified third-party shims.

## Capability ladder

### Level 0 — terminal

Every command works as a PTY occupant. The application knows launch/process/PTY facts and preserves ordinary terminal input, output, resize, copy, scrollback, tabs, panes, and spaces. Agent state is `unknown`.

### Level 1 — identified launch

An explicit launch profile associates a pane with an adapter and records executable/version/cwd. The adapter may advertise detected capabilities, but no semantic lifecycle is claimed until a cooperative channel connects. Identifying a manually launched process by name is heuristic and should be shown as such.

### Level 2 — cooperative observer, native TUI retained

A hook, extension, plugin, or built-in side server reports session identity, lifecycle, and attention events while the agent still renders into the PTY:

- Claude Code HTTP/command hooks;
- Pi extension;
- OpenCode's TUI server/SSE;
- Codex lifecycle hooks, behind explicit compatibility/version gates;
- Gemini CLI command hooks.

This should be the default target for an agent-focused terminal. It enables badges, attention routing, notifications, workspace/session association, and resume affordances without replacing the user's chosen CLI.

### Level 3 — native rich control

The host speaks an agent-specific protocol or SDK: Codex app-server, Claude Agent SDK/stream-JSON, Pi RPC/SDK, or OpenCode HTTP SDK. It can prompt, steer, interrupt, render structured tools/diffs, and answer correlated approvals. Except for OpenCode's side server, this usually replaces the agent's native TUI; the product must present it as a distinct host-rendered session, not pretend it is passive terminal enrichment.

### Level 4 — standardized rich control

An ACP client can host agents that actually advertise ACP, currently verified here for OpenCode and Gemini CLI. The adapter must use protocol initialization/capability negotiation. ACP is one adapter family, not the base abstraction and not a fallback assumption.

## Recommended adapter architecture

```text
Pane / PTY -------------------------------------------------- always available
   |
   +-- Process witness (launch, pid, exit) ------------------ host facts
   |
   +-- Agent adapter (optional, capability-negotiated)
          |
          +-- observer: hook / extension / SSE -------------- native TUI retained
          |
          +-- controller: app-server / SDK / RPC / ACP ------ host-rendered mode
          |
          +-- normalized evidence -> session registry
                                   -> activity/attention model
                                   -> actions allowed by capabilities
```

Each adapter should return capabilities, not force core code to branch on agent names:

```text
observe_activity       observe_items          obtain_resume_id
resume                 fork                   prompt
steer                  interrupt              request_approval
answer_approval        read_history           read_diff
read_todos             structured_output      reconnect_snapshot
```

Core UI actions are enabled only when the current connection advertises the corresponding capability. An adapter may lose capabilities after disconnect or version mismatch without affecting terminal usability.

## Normalized evidence and state

Do not compress everything into one "agent status" enum. Store three orthogonal dimensions:

| Dimension | Suggested values | Highest-authority examples |
| --- | --- | --- |
| Process | `starting`, `running`, `exited` | The application's child/process witness |
| Activity | `unknown`, `idle`, `running`, `retrying`, `compacting`, `failed` | OpenCode `session.status`; Pi `agent_settled`; Codex turn events; Claude lifecycle hooks |
| Attention | `none`, `input`, `approval` | Correlated permission/input requests, never prompt text |

Every observation should preserve at least:

```text
source                  authority              observed_at
connection_generation   agent_session_id?      turn_or_message_id?
raw_agent_version?      capabilities           payload_reference?
```

Precedence is per dimension: live protocol response/event, then cooperative hook/plugin, then app-owned process fact, then heuristic. A protocol event about activity cannot override the app-owned fact that its process exited; a process being alive cannot clear an authoritative pending approval. On side-channel disconnect, preserve process state but degrade semantic activity to `unknown` after a short grace period and resnapshot when supported.

The mappings are adapter-owned and documented as either fact or inference. For example:

- OpenCode `status: busy` → activity `running` is a direct normalized fact.
- Pi `agent_settled` → activity `idle` is a direct normalized fact because the event explicitly excludes automatic continuation.
- Codex `turn/started` → activity `running`; completion → `idle` or `failed` according to final status.
- Claude `Stop` → inferred `idle` unless a background task/continuation signal says otherwise; `PermissionRequest` → attention `approval` is direct.
- PTY output silence for ten seconds → no semantic state change.

## Security and failure behavior

- Bind bridges to a private Unix socket or loopback only. Use a fresh random secret per pane/launch; a session ID is correlation identity, not authorization.
- Prefer ephemeral CLI/global configuration supplied by the application over writing a hook or plugin into an untrusted project. Project code and project-local extensions execute with the user's authority.
- Persist the minimum event data. Tool inputs, outputs, transcripts, prompts, diffs, paths, and environment snapshots may contain credentials or proprietary code; redact by default and make history retention explicit.
- Forward approvals only through a request-ID-bearing cooperative interface. Render the exact proposed operation and agent-provided choices, expire requests on completion/disconnect, and fail closed. Keystroke injection into a terminal is normal user-directed terminal control, not a semantic approval API.
- Treat transcript/session files as durable history, not a live bus. They may lag, be concurrently written, change format, and expose secrets.
- If an adapter crashes, rejects the version, misses an event, or cannot authenticate, leave the PTY untouched and degrade to the lower capability level. Integration failure must never terminate the user's agent.
- On event-stream gaps, use a snapshot endpoint when available: OpenCode session status, Pi `get_state`, or Codex thread read/status. Otherwise mark state unknown rather than guessing.

## Versioning and conformance policy

1. Probe the executable and record its exact version before enabling an adapter.
2. Perform a real initialization/health handshake and derive the live capability set. Do not infer support solely from a version string.
3. Generate or vendor types from the exact protocol source where offered: Codex app-server schemas and OpenCode OpenAPI.
4. Keep experimental methods disabled unless a feature explicitly needs them. Experimental capability names must remain visible in adapter diagnostics.
5. Maintain fixture traces plus launch/resume/approval/reconnect conformance tests for the minimum and maximum supported version of every adapter.
6. Unknown event variants are logged safely and ignored; missing required fields cause semantic state to become unknown, never a permissive approval.
7. Pin pre-1.0 surfaces, notably Pi and the ACP SDK used by the surveyed OpenCode implementation, until their conformance suite passes.

## Recommended delivery order

1. Build the PTY-first pane model, evidence envelope, three-dimensional state model, session registry, and capability-driven UI.
2. Implement OpenCode's authenticated loopback server/SSE adapter. It exercises resume, status, events, approvals, reconnect, and control while retaining the real TUI.
3. Implement Claude Code hooks and a Pi extension as terminal-preserving observers. Start with session/activity/attention; add approvals only after fail-closed tests.
4. Implement Codex app-server as the first host-rendered rich adapter. Keep ordinary Codex CLI panes at level 0/1 until the hook bridge has a tested compatibility boundary.
5. Add an ACP controller shared by OpenCode and Gemini CLI, while retaining agent-specific resume and capability quirks.
6. Add heuristic discovery last and keep it visibly non-authoritative.

## Bottom line

The correct universal abstraction is the terminal pane plus evidence, not ACP and not a unified agent process. The terminal is the durable baseline. Cooperative integrations enrich it only with claims they can authoritatively support, and the UI is driven by negotiated capabilities. This architecture preserves arbitrary CLIs today, supports deeply integrated OpenCode/Claude/Pi/Codex experiences incrementally, and avoids turning brittle screen or process heuristics into false product truth.
