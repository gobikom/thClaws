# FilmScript / Movie Maker

FilmScript turns a `.film` screenplay into a finished AI video: a **pure
two-phase compiler** (`filmscript/`) produces per-shot video-backend payloads
+ a declarative ffmpeg assembly plan + a cost estimate, and a **harness**
(`filmscript/harness/`) executes the side effects (generation, TTS, upload,
assembly). The user-facing surface is the **Movie Maker** catalog agent (skill
`tool-gate: filmscript`) + the Film Studio gui-shell; this doc is the engine.

**Source:** `crates/core/src/filmscript/` (`ast.rs`, `parser.rs`, `resolve.rs`,
`phase1.rs`, `phase2.rs`, `backend.rs`, `harness/{mod,job,dispatch,kie,tts,upload,assemble}.rs`)
+ the tool family in `crates/core/src/tools/filmscript.rs`. Design specs:
`docs/filmscript-dsl-design.md` (+ `-v2-th.md`), `dev-plan/52-*`.

**Companion:** built-in tool gating in [`built-in-tools.md`](built-in-tools.md) §9g.

---

## 1. The purity contract

The load-bearing invariant (`filmscript/mod.rs`): **phase 1 is pure** — it
emits `AssetRequest`s and never touches the filesystem, network, or clock; the
**harness** fulfils them into `ResolvedAsset`s; **phase 2** is pure again,
consuming the resolved assets to codegen backend payloads. This keeps compile
deterministic + unit-testable and confines all money-spending / IO to the
harness.

Diagnostics are model-repair input: `CompileError` carries a stable `code`
(e.g. `E_ASSET_UNRESOLVED`, `E_AUDIO_OVERRUN`, `E_MIXED_VARIANT`,
`E_PHASE1_ERRORS`) + a language-matched `message` — a Thai source produces Thai
error messages (`is_thai_text`), so the authoring agent can fix and retry.

---

## 2. The `.film` DSL

Line-oriented grammar (`parser.rs`, context stack Top → Film → Sequence →
Shot). The AST (`ast.rs`):

- **`film "title" { … }`** header fields: `aspect`, `resolution`, `style`,
  `lighting`, `genre`, `fps_look`, `audio_default`, `dialogue_sync` (`native` |
  `overlay` | `lipsync`), `subtitle_default`, `subtitle_burn`, `backend`.
- **Declarations:** `char|scene|prop $handle = @./img.png voice:… desc:… time:…`
  (`EntityKind`); `variant $base#tag = @img` / `view $base#tag = @img`;
  `music $x = @f.mp3` / `sfx $y = @f.wav`.
- **`sequence "title" { scene:/style:/lighting:/music: … <shots> }`**.
- **Shot blocks** with shot types `dialogue | action | establishing | insert`.
- **Line kinds** (syntactically unambiguous): Action (with `$refs`), Property
  (`key: value`), Directive (`@key: value`), Dialogue (`$a say "…"`), Beat
  (`[t1-t2] …`).
- **Property keys:** `camera, expression, gesture, voice_tone, mood, lighting,
  style, scene, ambient, sfx`. **Directive keys:** `continue_from, match_cut,
  duration, resolution, aspect, model, audio, seed, takes, hold, transition,
  subtitle, subtitle_burn, dialogue_sync, backend`.
- **v1-deferred** (parse but reject/ignore): `@takes>1`, `@hold`, voice cloning
  (`voice:@sample`), `dialogue_sync: post`.

Voice ids resolve against `voices.json` (§9).

---

## 3. Resolution & the style cascade

`resolve.rs`: bind `$refs` / `$base#tag`, assign `@ImageN` slots by declaration
order, apply the **film ⊕ sequence ⊕ shot** cascade (inner wins), pick a
generation `Mode` (`Reference` | `FirstFrame` | `TextOnly`), and normalize
directives. Determinism rules produce `E_MIXED_VARIANT` when a shot mixes
incompatible variant tags. Output is `ResolvedShot`.

---

## 4. Two-phase compiler

**Phase 1** (`phase1.rs`): parse → resolve → validate, then emit shot skeletons,
deduped **content-addressed `AssetRequest`s** (`File | Tts | Video | Frame`),
the ffmpeg **`AssemblyPlan`** (`build_assembly_plan`), and the **`CostEstimate`**.
Dialogue routing: **native by default** (the video backend generates dialogue
audio from the prompt); TTS is synthesized only in `overlay` / `lipsync` modes.

**Phase 2** (`phase2.rs`): codegen. The prompt is assembled in a fixed order
(subject → action → camera → lighting/style → ambient → dialogue → negatives);
identity is image-led, disambiguation text-led. It refuses on any phase-1 error
(`E_PHASE1_ERRORS`) and gates on `E_ASSET_UNRESOLVED` / `E_AUDIO_OVERRUN`. Emits
`ShotPayload { backend, payload, depends_on, seed }`.

---

## 5. Cost model & budget gate

Estimation uses a T0-calibrated per-second rate table `rate_usd_per_s(fast,
resolution, with_video)` (`phase1.rs`). Continuation quirk: `@continue_from`
bills `(source_dur + new_dur) × with-video rate`, so a 6 s continuation costs
*more* than a fresh 6 s shot. Currency basis: **Kie 1 credit = $0.005**, shared
by the estimate and the runtime ceiling (`harness/job.rs`).

Two gates:
1. **Up-front** — `FilmGenerate` refuses if `estimate.total_usd > budgetUsd`
   (`job.rs::start`).
2. **Running ceiling** — the shot loop aborts *before* the next paid shot once
   accumulated `spent_credits × $0.005` passes the confirmed `budgetUsd`
   (`job.rs` shot loop); already-rendered shots are cached, so raising the
   budget + `resume:true` continues.

---

## 6. Video-backend abstraction

`backend.rs`. `BackendId` = `Grok` (default) | `Ltx` | `Seedance` | `Veo` |
`HappyHorse`. A `VideoCaps` matrix (native_audio, audio_ref, thai_native,
max_image_refs, identity mode, continuation mode, voice_control, max_duration)
drives per-backend payload builders (`grok_payload` / `seedance_payload` /
`ltx_payload` / `veo_payload` / `happyhorse_payload`). Payload JSON is
golden-test-frozen so a backend contract change is caught in CI.

---

## 7. Harness — job lifecycle

`harness/job.rs`. One OS thread + a current-thread tokio runtime per job; the
member-attribution task-local is **re-scoped inside that thread** (dev-plan/45
A2) so every gateway call carries `X-Thclaws-Member`. Job id = the script hash,
so "same script" = "same job"; **one active job per workspace**. State
(`job.json`) is written atomically; `FilmJobStatus` is a read-only snapshot.

A **shot-result cache** keyed by payload-hash → `{task_id, clip_url, credits}`
makes resume + edit-rerender incremental and never double-spends. The shot loop
(`run_job`): synthesize upfront File + TTS assets → phase-2 pre-check for
`E_AUDIO_OVERRUN` → topo-order the shots → per-shot ref-video / frame prep →
dispatch → finalize → assemble. A chain-drift warning fires at continuation
depth ≥ 4. `ShotState` = `pending | assets | generating | polling | done |
failed | skipped`. ffmpeg/ffprobe presence is preflighted. Dialogue finalize
swaps the backend's regenerated timbre for the real TTS aligned to the detected
speech onset; ref-video prep strips audio + normalizes to Kie's constraints;
match-cut captures the last frame for the next shot.

---

## 8. Harness — dispatch & backends

`harness/dispatch.rs`: Grok/Seedance → Kie jobs, Veo → Kie Veo route, LTX →
native `api.ltx.video` sync (raw MP4, no poll; `LTX_BASE_URL` override), Happy
Horse → DashScope async. Poll interval 15 s, timeout 20 min. Kie client
(`harness/kie.rs`): sends a browser User-Agent (Cloudflare 403s default UAs),
classifies success on `data.state` (not the top-level `code`), and debits
credits at submit.

---

## 9. Harness — TTS

`harness/tts.rs`. Providers: Gemini `gemini-3.1-flash-tts-preview` (quality
winner; direct or the OpenRouter pcm route), MiniMax `speech-02-hd`, ElevenLabs
`eleven_v3`, OpenAI `gpt-4o-mini-tts`. **The model, not the provider, decides
Thai.** A `voices.json` registry (built-in `th-female-warm` / `th-male-low` /
`narrator` = Gemini Kore/Charon; user-editable at `.thclaws/film/voices.json`)
maps voice ids to `(provider, voice, model)`. Output contract: mp3, padded to
≥ 2 s (Kie's audio-ref floor), `duration_ms` from ffprobe. All four providers
route **BYOK-or-gateway** with per-member metering (Gemini meters through the
`/google` token path; ElevenLabs/OpenAI/MiniMax through the gateway's media
routes), closing the old hosted-TTS-bypass hole.

---

## 10. Harness — upload & assembly

**Upload** (`harness/upload.rs`): the Kie File Upload API (free, but ~3-day
auto-delete) behind a content-hash URL cache with a 60 h TTL + transparent
re-upload on expiry. **Assembly** (`harness/assemble.rs`): normalize each clip
(24 fps / 1280×720 / 44.1 k), trim boundary-dup frames, apply transition fades,
concat-demux the clips, mix in the music bed + SFX, build an SRT from the final
durations, and either soft-mux the subtitles (default) or burn them
(`subtitle_burn:`). Artifacts land in `.thclaws/film/<jobId>/out/`
(`final.mp4` / `final.srt` / `manifest.json` with per-shot task/credit/sha).

---

## 11. Tool family (Tier 3)

`tools/filmscript.rs` — all five gated behind the `filmscript` tool-gate
(dormant + invisible until the Movie Maker skill or the Film Studio gui-shell
opens the gate):

| Tool | Approval | Notes |
|---|---|---|
| `FilmCompile` | no | Pure + instant; adds fs `exists` checks + voice-registry validation on top of phase 1. |
| `FilmGenerate` | **yes** | `budgetUsd` **required** — the real spend consent (hosted multiuser force-auto-approves, so the budget is the gate). `resume: true` continues a job. |
| `FilmJobStatus` | no | Read-only snapshot + disk usage. |
| `FilmJobCancel` | no | Cancels a running job. |
| `FilmAssetImport` | **yes** | Path-jailed base64 import to `.thclaws/film/assets/`, 30 MB cap; warns that Seedance rejects real-face reference uploads. |

The **review loop**: [`WatchVideo`](built-in-tools.md) lets a vision model watch
a generated clip (scene-aware frames + transcript); edit the `.film` → recompile
→ `FilmGenerate resume:true` re-fires **only the changed shots** via the
payload-hash cache.

---

## 12. Known gaps / v1 boundaries

- `@takes>1`, `@hold`, voice cloning (`voice:@sample`), and `dialogue_sync: post`
  parse but are deferred.
- A user-manual companion (task-oriented "install Movie Maker, prep art, write a
  `.film`, preview cost, generate, review") is a separate deliverable
  (`user-manual/ch29-movie-maker.md`).
