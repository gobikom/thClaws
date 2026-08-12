# Chapter 29 — Movie Maker (make an AI film from a screenplay)

**Movie Maker** turns a short screenplay you write — in a tiny `.film`
language — into a finished video: consistent characters and scenes, spoken
Thai (or English) dialogue, music, and subtitles. You describe the shots; the
tools run the generation pipeline and show you the cost before spending
anything.

This is the task-oriented guide. The engine internals (the `.film` grammar in
full, the compiler, backends) are in the technical manual's `filmscript.md`.

## 1. Install the Movie Maker agent

Movie Maker ships as a **catalog agent**, not a built-in — its tools stay
hidden until you install it (this keeps the paid video tools off by default):

```
/cloud get movie-maker-2
```

Installing it into a folder brings the `.film` skill + the **Film Studio** GUI
shell (a point-and-click front end). You'll need thClaws.cloud gateway access
(or provider keys) since generation is paid.

## 2. Prepare your character / scene art

The backends keep a character looking the same across shots by anchoring to a
**reference image**. Generate character art first (TextToImage, or bring your
own — but note some backends reject real human faces as references; use
generated images for people). Import assets so `.film` can reference them:

- Drop them in `.thclaws/film/assets/` (or use `FilmAssetImport` for base64).
- Reference them as `@./assets/hero.png`.

## 3. Write a `.film` script

A `.film` file is a screenplay with a little structure. Minimal example:

```
film "Morning" {
  aspect: 16:9
  resolution: 720p
  char $mai = @./assets/mai.png voice:th-female-warm desc:"a young Thai woman"

  sequence "Kitchen" {
    scene: a sunny kitchen
    shot dialogue {
      $mai say "อรุณสวัสดิ์ค่ะ"
      camera: medium close-up
    }
  }
}
```

You write **characters/scenes/props** (with a reference image + a voice),
group **shots** into **sequences**, and inside each shot write actions,
dialogue (`$who say "…"`), and directives (`@duration: 6`, `@backend: veo`,
`@continue_from: …`). Dialogue is spoken by the backend natively by default; a
`narrator` voice reads voice-over.

## 4. Preview the cost, then generate

Always compile first — it's free and instant, and it tells you what the film
will cost:

- **`FilmCompile`** validates the script + returns a per-shot **cost estimate**
  in USD.
- **`FilmGenerate`** actually renders it. It **requires a `budgetUsd`** — this
  is both your consent and a hard cap: generation stops before any shot that
  would push spend over the budget (already-rendered shots are kept; raise the
  budget and re-run with `resume` to continue).

In the Film Studio shell this is a "Preview cost → Generate" button; from chat
the agent calls the tools for you.

The finished film + subtitles land in `.thclaws/film/<job>/out/`
(`final.mp4`, `final.srt`).

## 5. Review and re-roll

Not happy with a shot? Ask the agent to **watch** it (`WatchVideo` pulls key
frames + a transcript so the model can actually see the result), edit that
shot in the `.film`, and re-generate — only the **changed** shots re-render
(the rest are cached), so iterating is cheap.

## 6. Voices

Voice ids (`th-female-warm`, `th-male-low`, `narrator`, …) map to TTS providers
in `.thclaws/film/voices.json`, which you can edit to pick a different provider
or voice. The default Thai narrator is Gemini's "Charon" voice. **The model,
not just the provider, decides Thai quality** — the defaults are chosen to
speak Thai well.

## Tips

- Start small (one sequence, 2–3 short shots) and preview the cost before
  scaling up — video is billed per output second.
- Use generated images (not photos of real people) for character references.
- Keep continuations short: a `@continue_from` shot bills for the source clip's
  length *plus* the new length.
