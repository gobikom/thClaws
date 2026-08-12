# Chapter 30 — Job Artifacts

*Since v0.88.0.*

Dispatch work to a thClaws worker over HTTP, then collect the **files**
it produced — declared per job, frozen with a hash the moment the run
finishes, downloadable with the same Bearer token you dispatched with.
Push input files the same way. Together these turn any `--serve`
instance into a worker node you can chain into pipelines:

```
coder (machine A) ──artifacts──▶ your orchestrator ──inputs──▶ reviewer (machine B)
```

If you've used GitHub Actions or GitLab CI artifacts, this is the same
idea — declared outputs of one job, retrievable by id — applied to
agent runs. The "job" is the `session_id` every `/agent/run` returns.

## Why not workspace sync?

The sync surface (`/workspace/sync/*`, Chapter 27) mirrors a *whole
workspace* and was designed for trusted networks (a tunnel or
ForwardAuth in front). An external orchestrator holding only an API
token had three gaps: no supported auth, no way to know which files a
given job actually produced, and a race — the file list and the file
download were separate requests, so a file could change in between.
Job Artifacts closes all three: Bearer auth, per-job scoping, and a
manifest whose hashes are fixed at collection time.

## Quick start

Start a worker:

```bash
THCLAWS_API_TOKEN=secret thclaws --serve --port 8443
```

**1. (Optional) push input files** before dispatching:

```bash
curl -X POST http://worker:8443/v1/inputs \
  -H "Authorization: Bearer secret" -H "Content-Type: application/json" \
  -d '{"files":[{"path":"inputs/brief.txt","content_base64":"'"$(base64 < brief.txt)"'"}]}'
```

Files land under `inputs/` in the worker's workspace (see
[Input placement rules](#input-rules)).

**2. Dispatch with `collect_files`** — glob patterns naming what counts
as this job's output:

```bash
curl -X POST http://worker:8443/agent/run \
  -H "Authorization: Bearer secret" -H "Content-Type: application/json" \
  -d '{"prompt":"Read inputs/brief.txt and write a report to reports/summary.md",
       "collect_files":["reports/*.md"]}'
# → { "session_id": "sess-abc…", "summary": "...", ... }
```

When the run finishes, matching files are **copied into a per-session
snapshot** and sha256-hashed. From that moment the artifact is immutable
— editing the live file later changes nothing you can download.

**3. Fetch the manifest**, then the files:

```bash
curl -H "Authorization: Bearer secret" \
  http://worker:8443/v1/sessions/sess-abc…/artifacts
# → { "artifacts": [ { "id":"a1", "path":"reports/summary.md",
#                      "size":180, "sha256":"67aeef…" } ],
#     "patterns": ["reports/*.md"], "collected_at": "…" }

curl -H "Authorization: Bearer secret" -o summary.md \
  http://worker:8443/v1/sessions/sess-abc…/artifacts/a1
# response carries an `x-sha256` header — verify it matches the manifest
```

## Endpoint reference

All under the same Bearer auth as the rest of `/v1`
(`THCLAWS_API_TOKEN`).

| Endpoint | What it does |
|---|---|
| `POST /agent/run` + `"collect_files": ["glob", …]` | Declare the run's outputs; snapshot + hash at completion. Works on the sync, streaming (SSE), and async (`x_callback`) paths alike. |
| `GET /v1/sessions/{sid}/artifacts` | The frozen manifest: `id`, `path`, `size`, `sha256` per artifact, plus the patterns used and a `skipped` list if caps truncated the collection. |
| `GET /v1/sessions/{sid}/artifacts/{aid}` | One file, served from the snapshot (never the live workspace). `aid` is the manifest id (`a1`, `a2`, …) or the exact path. |
| `POST /v1/inputs` | Place files into the workspace ahead of a dispatch. Body: `{"workspace_dir"?: "...", "files": [{"path", "content_base64"}]}`. |

Both GETs and `/v1/inputs` accept an optional `workspace_dir`
(query parameter for the GETs) with the same validation as
`/agent/run`; omitted, the daemon's working directory is used.

## Input placement rules {#input-rules}

`POST /v1/inputs` is deliberately conservative:

- Paths must be **relative**, may not contain `..`, and may never touch
  `.thclaws/` or `.git/`.
- By default files may only land under **`inputs/`**. Widen with an
  env allowlist on the worker: `THCLAWS_INPUTS_PREFIXES="inputs/,data/"`
  — or `THCLAWS_INPUTS_PREFIXES="*"` for anywhere in the workspace
  (still excluding `.thclaws/` and `.git/`).
- Limits: ≤ 100 files per request, ≤ 64 MB decoded total. The response
  echoes each written file's `sha256` so the sender can verify.

## Collection limits

Artifact snapshots cap at **256 files / 300 MB** per run. Files that
matched your globs but fell past a cap appear in the manifest's
`skipped` list — a truncated collection is always visible, never
silent. Collection skips `.thclaws/`, `.git/`, and `node_modules/`
entirely.

## Securing workspace sync for orchestrators (Tier 1)

If you *do* want whole-workspace mirroring (Chapter 27) from an
orchestrator that only holds the API token, opt the sync routes into
Bearer auth on the worker:

```bash
THCLAWS_SYNC_REQUIRE_AUTH=1 THCLAWS_API_TOKEN=secret thclaws --serve
```

Every `/workspace/sync/*` request must then carry
`Authorization: Bearer <token>` — no tunnel or ForwardAuth needed.
Unset, sync behaves exactly as before (trusted-network model), so
existing deployments are unaffected.

## A two-worker pipeline, end to end

```bash
# machine A writes code
SID_A=$(curl -s -X POST http://worker-a:8443/agent/run \
  -H "Authorization: Bearer $TOK" -H "Content-Type: application/json" \
  -d '{"prompt":"Implement the parser per spec.md","collect_files":["src/**/*.py"]}' \
  | jq -r .session_id)

# pull A's artifacts, push them to B as inputs
curl -s -H "Authorization: Bearer $TOK" \
  http://worker-a:8443/v1/sessions/$SID_A/artifacts | jq -c '.artifacts[]' |
while read -r art; do
  aid=$(jq -r .id <<<"$art"); path=$(jq -r .path <<<"$art")
  curl -s -H "Authorization: Bearer $TOK" \
    http://worker-a:8443/v1/sessions/$SID_A/artifacts/$aid -o /tmp/f
  curl -s -X POST http://worker-b:8443/v1/inputs \
    -H "Authorization: Bearer $TOK" -H "Content-Type: application/json" \
    -d "{\"files\":[{\"path\":\"inputs/$path\",\"content_base64\":\"$(base64 < /tmp/f)\"}]}"
done

# machine B reviews the actual files
curl -s -X POST http://worker-b:8443/agent/run \
  -H "Authorization: Bearer $TOK" -H "Content-Type: application/json" \
  -d '{"prompt":"Review the code under inputs/ and write findings to review.md",
       "collect_files":["review.md"]}'
```

## Where artifacts live on disk

Snapshots are stored inside the worker's workspace at
`.thclaws/state/artifacts/<session_id>/` (`manifest.json` + `files/`).
Like everything under `.thclaws/state/`, they're gitignored and never
included when an agent is packed or published. They persist until you
delete them — prune old sessions' directories if a long-lived worker
accumulates too many.

## See also

- [Chapter 3](ch03-working-directory-and-modes.md) — `--serve` mode and
  `THCLAWS_API_TOKEN`.
- [Chapter 19](ch19-scheduling.md#heartbeats) — heartbeat schedules;
  combine with artifacts for continuous produce-and-collect loops.
- [Chapter 27](ch27-thclaws-cloud.md) — workspace sync, the
  whole-workspace alternative.
- Technical manual: `job-artifacts.md` for wire-level details.
