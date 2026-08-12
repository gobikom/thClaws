# GUI Shells — the `window.thclaws.*` bridge

How a marketplace / catalog agent ships its own HTML+JS UI ("GUI shell") and how that UI talks to the engine. A shell is a folder of static assets (`index.html` / `main.js` / `manifest.json`) that the engine serves inside an iframe and injects a single global into: `window.thclaws`. The bridge is the **only** capability surface — a shell has no direct filesystem, no network beyond declared hosts, and no access to another shell's storage.

This doc covers: the two transports (Mode A desktop iframe vs Mode B standalone `--serve`), bridge injection, the request/reply + event wire format, the host-side IPC handlers, per-shell storage, the full method surface and **what is actually wired vs. on-the-object-but-stubbed**, theme / full-screen integration, the permission model, and the preview mock.

Related: [`app-architecture.md`](app-architecture.md) (the underlying Rust↔JS IPC bridge this rides on), [`mcp.md`](mcp.md) (MCP-Apps widgets — a different host↔widget postMessage protocol), [`serve-mode.md`](serve-mode.md) + [`multi-tenant-serve.md`](multi-tenant-serve.md) (Mode B / per-user storage roots), [`built-in-tools.md`](built-in-tools.md) (the Media Studio shell + media-tool gating).

## 1. Module layout

| File | Role |
|---|---|
| `crates/core/assets/gui-shell-bridge.js` | The bridge runtime injected into every shell. Builds `window.thclaws`, marshals JSON to the host, fans events out to subscribers. Embedded via `gui_shell/mod.rs:29` `BRIDGE_RUNTIME = include_str!(…)`. |
| `crates/core/src/gui_shell/mod.rs` | Module root; serves the bridge at `thclaws://localhost/gui-shell-bridge.js`. |
| `crates/core/src/gui_shell/{manifest,registry,router,serve,storage,tokens,shell_cli,shell_preview}.rs` | Manifest schema, installed-shell registry, URL routing, Mode B serve handler, per-shell storage backend, serve-token mint/verify, `thclaws shell …` CLI, and the preview mock. |
| `crates/core/src/ipc.rs` | Host-side `gui_shell_*` dispatch arms (shared GUI + `--serve`). |
| `frontend/src/components/UIView.tsx` | Mode A only: the React iframe host that marshals between the shell's `postMessage` and the backend `window.ipc` bridge. |
| `frontend/src/App.tsx` | Handles parent-only signals (`hotkey` / `ui`) the iframe forwards (`App.tsx:539`, `ns === "thclaws-shell"`). |

## 2. Two transports

`thclaws.transport` is `"tauri"` (Mode A) or `"ws"` (Mode B). The bridge picks the mode from `window.__thclaws_shell_mode` (the Mode B serve handler sets it to `"ws"` before the bridge script runs).

- **Mode A — desktop iframe (`transport: "tauri"`).** URL `thclaws://localhost/gui-shell/<id>/<path>?session=<sid>`. The `thclaws://` protocol handler (`gui.rs:852`, `req_path == "/gui-shell-bridge.js"`) serves the bridge and the shell assets; the bridge tag is injected into `<head>` at serve time by the inject helper at `gui.rs:468`. The shell `postMessage`s to its parent (the React `UIView` iframe host), which relays to/from the Rust backend over the existing `window.ipc` / `__thclaws_dispatch` bridge.
- **Mode B — standalone serve (`transport: "ws"`).** URL `/t/<token>/<path>?session=<sid>` under `thclaws --serve --gui-shell`. There is no parent React app: the bridge opens a WebSocket directly to the engine (`window.__thclaws_shell_ws_url`, default `/__ws`), opened lazily on first send with exponential backoff reconnection (500 ms → 30 s cap). Identity (`shellId`, `sessionId`) is injected as `window.__thclaws_shell_id` / `__thclaws_shell_session_id` at HTML render time because the `/t/<token>/` URL carries neither. The cloud `--serve`-over-https case nests the iframe under the same Traefik-stripped prefix as the parent workspace URL.

Both modes converge on one `handleShellEvent(data)` fan-out in the bridge, so shell code is transport-agnostic.

## 3. Wire format

The bridge's `send(type, payload)` allocates a `requestId`, stores `{resolve, reject}` in a `pending` map, and emits a frame:

**Request (shell → host).**
- Mode A `postMessage` to parent: `{ ns: "thclaws-shell", requestId, type, payload, shellId, sessionId }`. `UIView.tsx:71` forwards it to the backend as `{ type: "gui_shell_<type>", id: requestId, sessionId, shellId, ...payload }` (`UIView.tsx:95`).
- Mode B WS frame: identical `{ type: "gui_shell_<type>", id: requestId, sessionId, shellId, ...payload }`.

**Reply (host → shell).** The host dispatches a `gui_shell_event` envelope correlated by `replyTo`:

```json
{ "type": "gui_shell_event", "sessionId": "<sid>", "replyTo": 7, "result": <any> }
{ "type": "gui_shell_event", "sessionId": "<sid>", "replyTo": 7, "error": "<msg>" }
```

`handleShellEvent` matches `replyTo` against `pending` and resolves/rejects the promise. In Mode A, `UIView.tsx:122` subscribes to backend dispatches and re-posts any `gui_shell_event` into the iframe as `{ ns: "thclaws-shell-event", ... }`; in Mode B the bridge reads it straight off the WS.

**Events (host → shell, unsolicited).** Same `gui_shell_event` envelope but carrying `event` + `payload` instead of `replyTo`. The bridge fans these out to `thclaws.on(event, …)` subscribers. Event names: `ready`, `text`, `done`, `error`, `tool_call`, `tool_result`, `fullscreen`, `theme`.

> **Tier 1 has no per-tab session filtering** (`UIView.tsx:124`): one shared session, so every active shell tab receives every event. Per-tab `sessionId` filtering is Tier 2.

## 4. Host-side handlers (`ipc.rs`)

`handle_ipc` returns `bool` (the [`running-modes.md`](running-modes.md) invariant); the `gui_shell_*` arms are shared by GUI and `--serve`. Implemented arms:

| Backend type | `ipc.rs` | Bridge method |
|---|---|---|
| `gui_shell_run` | `:275` | `thclaws.run(prompt, opts?)` |
| `gui_shell_cancel` | `:304` | `thclaws.cancel(runId?)` (fire-and-forget, no reply) |
| `gui_shell_tool_invoke` | `:334` | `thclaws.callTool(name, args)` / `thclaws.tools.invoke(name, args)` |
| `gui_shell_storage_get` | `:425` | `thclaws.storage.get(key)` |
| `gui_shell_storage_set` | `:523` | `thclaws.storage.set(key, value)` |
| `gui_shell_list` | `:488` | *(frontend shell-list, not a bridge method)* |

**`gui_shell_tool_invoke` gating** (`ipc.rs`, the `gui_shell_tool_invoke` arm): read-only tools (`ls` / `read` / `glob` / `grep` / `web_fetch` / `web_search` / `kms_read` / …) run directly; mutating tools (`Bash` / `Write` / `Edit` / …) go through approval — the shell's own inline widget when it registered one (§6a), else the system modal. **Manifest allowlist:** a shell that declares any `tools.invoke:<tool>` permission may invoke only those tools (`tools.invoke:*` = all); one that declares none runs unfettered (legacy). An undeclared tool is rejected before it runs (`shell_tool_invoke_allowed`). MCP-contributed tools are **not** reachable in this arm — it builds a fresh built-ins-only `ToolRegistry`. Media/generation tools are force-enabled for the built-in **`media-studio`** and **`film-studio`** shells (the latter also activates the FilmScript gate) regardless of the workspace `image_tools_enabled` flag, and `hal_enabled` additionally registers the HAL tools — making those built-in shells zero-config on-ramps (see [`built-in-tools.md`](built-in-tools.md)).

## 5. Storage

Per-shell, per-session JSON, namespaced by shell id so two shells can't read each other's state even on a shared session. Default path `~/.config/thclaws/gui-shell/<shellId>/state/<sessionId>.json` (`ipc.rs:422`, atomic per-set). Under multi-tenant `--serve`, the `SessionRoots` override relocates it into the per-user subtree `<project>/.thclaws/users/<id>/storage/…` — see [`multi-tenant-serve.md`](multi-tenant-serve.md). There is no `delete` handler; the documented delete idiom is `storage.set(key, null)`.

## 6. Method surface

As of dev-plan/39 Tier 3 the full bridge surface is backed end-to-end (there are no
longer "on-the-object-but-stubbed" methods). Each maps to a `gui_shell_*` IPC arm
unless noted as pure client-side.

- `thclaws.shell.{id,sessionId}`, `thclaws.transport` — identity (client-side).
- `thclaws.run(prompt, opts?) → Promise<{runId}>`, `thclaws.cancel(runId?)`.
- `thclaws.on(event, cb) → unsubscribe`.
- `thclaws.streamTurn(prompt, opts?)` — async-iterable over the live turn built on
  the same `gui_shell_event` stream. Yields **`{type:"text", delta}`**,
  **`{type:"tool_call", name, label, input}`**, and **`{type:"tool_result", name, output}`**
  in arrival order, terminated by the turn's `done` (or surfaced as a rejection on
  `error`). The iterator binds `ev` to the payload directly, so a consumer reads
  `ev.type` / `ev.delta` (not `ev.value.*`).
- `thclaws.callTool(name, args)` / `thclaws.tools.invoke(name, args)` → `gui_shell_tool_invoke`.
- `thclaws.storage.get/set/delete` → `gui_shell_storage_get/set/delete`. `delete(key)`
  removes the key (distinct from `set(key, null)`, which stores an explicit null).
- `thclaws.approvals.subscribe(cb) / respond(id, decision)` — inline tool approvals (§6a).
- `thclaws.awaitApproval(request) → {approved}` → `gui_shell_await_approval`; the shell
  asks the user to sign off on its OWN action, routed through the session approver.
- `thclaws.uploadFile(blob, name?) → assetUrl` → `gui_shell_upload_file` (§6b).
- `thclaws.permissions.list() / has(action)` → `gui_shell_permissions_list` (the shell's
  declared manifest permissions, so it can grey out UI for un-granted actions).
- `thclaws.model.* / kms.* / research.*` → `gui_shell_model_*` / `gui_shell_kms_*` /
  `gui_shell_research_*`, each gated by the matching manifest permission (§8).
- `thclaws.fileUrl(path) → string|null` — pure client-side path→URL mapping. Mode B accepts a path relative to the shell's project root (`/t/<token>/file-asset/…`); Mode A requires an absolute path (`thclaws://localhost/file-asset/…`), else `null`.
- `thclaws.ui.*` — `theme`, `isFullscreen`, `onTheme`, `onFullscreen`, `exitFullscreen`, `claimExitControl` (see §7).

**Timeout:** every `send()` self-rejects with `no reply for '<type>' after 900s` if the
host never answers (`SEND_TIMEOUT_MS = 15 min` in the bridge) — an unimplemented or
renamed command fails loud instead of hanging the shell's promise.

### 6a. Inline tool approvals

A shell that hosts its own approve/deny UI subscribes with `thclaws.approvals.subscribe(cb)`;
the bridge then sends `preferInline: true` on every `tools.invoke`/`callTool`. When a
mutating tool needs approval the engine, instead of popping the full-screen system modal
over the shell, dispatches an `approval_request` `gui_shell_event`
(`{approvalId, toolName, input, summary}`) and **awaits the shell's verdict** — the shell
renders its widget and calls `thclaws.approvals.respond(id, "allow"|"allow_for_session"|"deny")`,
which lands on the `gui_shell_approval_respond` arm and resolves the pending decision. A
process-global hub (`gui_shell/inline_approval.rs`) holds the pending oneshot; it is
fail-closed — a **5-minute timeout → deny**, and an unknown/missing decision string →
deny. A shell with no approval handler keeps the modal path unchanged.

### 6b. uploadFile

`uploadFile(blob, name?)` chunk-base64s the blob and sends it over the IPC channel
(works in both Mode A and Mode B). The `gui_shell_upload_file` arm decodes it, writes it
under `<workspace>/_uploads/` — the **per-user** workspace root in a multiuser pod, so
one member's uploads stay isolated — caps at `UPLOAD_MAX_BYTES` (25 MB; the bridge also
rejects oversize blobs early), and replies `{path, url}` where `url` is a shell-base-
relative `file-asset/<rel>` the shell can drop into `<img src>` / `<a href>`.

## 7. Theme & full-screen integration

The host pushes its resolved state as events the bridge intercepts in `handleShellEvent` before fanning out:

- `theme` (`{mode: "light"|"dark"}`) — the bridge sets `thclaws.ui.theme`, plus `data-theme` + `color-scheme` on `<html>`, so a shell can theme in **CSS alone** (`:root[data-theme="light"]{…}`) with no JS. `thclaws.ui.onTheme(cb)` is for JS-driven styling (canvas/charts) and fires immediately with the current value.
- `fullscreen` (`{active}`) — updates `thclaws.ui.isFullscreen`; `onFullscreen(cb)` fires immediately + on change.

`thclaws.ui.exitFullscreen()` and `claimExitControl()` post **parent-only** envelopes (`type: "hotkey" | "ui"`) that `App.tsx` handles on the window; both are no-ops in Mode B (the standalone shell owns the whole page). `UIView.tsx` replays the current fullscreen + theme to a freshly-loaded shell on the `ready` signal (`UIView.tsx:80`) so subscribers added before the first host event still get an initial value.

## 8. Permission model

`manifest.json::permissions` declares what a shell may do; the user sees the list before install, and anything undeclared throws at call time. The bridge is the only API — no workspace FS, no network beyond declared hosts (CSP injected at serve time).

| Permission | Allows |
|---|---|
| `agent.run` | `thclaws.run()` + event subscription |
| `tools.invoke:<name>` | `thclaws.callTool("<name>", …)` / `tools.invoke(…)` per tool. Declaring any `tools.invoke:` **gates** the shell to only the listed tools (`tools.invoke:*` = all); a shell that declares none runs unfettered (legacy). See §4. |
| `session.read` / `session.list` | read sidecar session data |
| `fs.shell-scoped` | read/write inside the shell's resolved root |
| `network.outbound:<host>` | `fetch()` to that host |
| `approval.inline` | marketplace signal that the shell renders its own approve/deny UI (`thclaws.approvals.*`, §6a) |
| `model.read` / `model.write` | `thclaws.model.*` — view current + list / switch the active model |
| `kms.read` | `thclaws.kms.*` — read the knowledge base directly (no LLM) |
| `research.read` | `thclaws.research.*` — read the research-job registry directly |

The full allowlist lives in `gui_shell/manifest.rs::ALLOWED_PERMISSION_PREFIXES`.

Publish-safety: `cloud/pack.rs` strips `.thclaws/sessions/`, KMS data, and browser-profile cookies so a shell's local state never leaks into a catalog tarball ([`thclaws-cloud-client.md`](thclaws-cloud-client.md)).

## 9. Preview & doctor

`thclaws shell doctor <dir>` validates the manifest, entry file, permission sanity, and flags Tauri-only APIs that would break in Mode B. `gui_shell/shell_preview.rs` provides a mock `window.thclaws` for design-time preview that replies to unimplemented methods with `"preview mock doesn't implement '<method>'"`.

## 10. Known gaps

- The `gui_shell_tool_invoke` IPC arm builds a **built-ins-only** `ToolRegistry`, so
  MCP-contributed tools aren't reachable from a shell's `callTool` yet.
- No per-tab session filtering — every shell tab gets every event.
- Full per-unit metering aside, the rest of Tier 3 (a structured `permissions` object,
  in-workspace shell authoring) is future work; the runtime surface above is complete.
