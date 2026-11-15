# AI posters

**Status: the panel works end to end on desktop, and the Android wiring is written (Phase 2).** A
prompt becomes a brief, a brief becomes a real, editable PhotoCraft document, and the whole path is
drivable from the panel, the CLI, the control channel and MCP. Image generation is not wired yet;
see [Not built yet](#not-built-yet).

`cargo xtask` target: none (the crate is pure Rust; `cargo test -p photocraft-ai` covers it).

## What it does

The AI writes a **brief**, never a picture and never commands. PhotoCraft composes the brief into
one of five templates and runs the ordinary engine commands that build it, so what arrives on the
canvas is a normal document:

| What you get | Why it matters |
|---|---|
| A type layer per block (`Headline`, `Subhead`, `Body`, `Details`) | Retype or restyle any word; nothing is flattened |
| A shape layer per panel, band, rule and frame | `shape.edit`, layer styles, masks, vector masks all apply |
| A fill layer for a gradient background | Recolour or replace it; it is not baked into pixels |
| An image placeholder (split template) | Phase 3 replaces it with a generated smart object |
| Empty history, clean document | The poster arrives like File › Open, not as twelve undo steps |

A generated *image* would be one flat raster: no retyping, no moving the logo, no per-layer
plug-in. That is why the plan builds layers instead.

## The panel

**Window › AI › Create Poster…** opens a modeless window (like Glyphs and the Measurement Log, so it
never blocks a frame):

1. **Prompt** — what the poster is for, capped at 4000 characters.
2. **Page** and **Layout** — a preset and a template, or `Auto` for both.
3. **Mood** — chips (`warm`, `minimal`, `bold`, …) that become the brief's style hints. Any word is
   accepted by the engine; the chips are the usual ones.
4. **Create** — asks the model on a worker thread; the window shows `Asking the model…` and a
   Cancel button, and repaints while it waits.
5. **Preview** — a wireframe painted from the plan's own steps: the paper, every fill, every shape
   rectangle and every type box, in build order. It is painted from the same step list
   `ai.poster.build` runs, so the preview cannot drift from the document.
6. **Build into a new document** — runs the plan; the panel closes and the poster is the active
   document.

Provider settings are collapsed behind **Provider**: base URL, model and API key, with an explicit
Save. The endpoint is any OpenAI-compatible one — OpenAI, Gemini's compat endpoint, Azure OpenAI,
OpenRouter, Groq, vLLM, LM Studio or Ollama. A blank key is fine for a local server.

The prompt, the answer and the settings draft live in the panel's runtime, **not** in `ui.ron`: only
`ai_open` is persisted (and controllable through `ui.set`/`ui.inspect`). A prompt is the person's
words and does not belong in a UI state file.

## The brief

The only thing a model produces. Untrusted: every field is length-capped, colours must be
`#rrggbb`, control characters are rejected, sizes and resolutions are bounded, and unknown fields
are ignored so a newer model may send more.

```json
{
  "version": 1,
  "title": "Autumn Print Fair",
  "subtitle": "Twelve studios open late",
  "body": "Prints, posters and new work, from twelve artists.",
  "footer": "Sat 14 Nov · 6-9 pm · Mill Lane",
  "format": "a4",
  "template": "auto",
  "palette": { "background": "#f4f1ea", "ink": "#17161a", "accent": "#e2402c" },
  "style": ["editorial"]
}
```

Only `title` is required. `photocraft_ai::brief::schema()` prints this description with the
preset and template ids filled in, so the prompt and the docs cannot drift apart.

**Presets:** `a4`, `a4-landscape`, `a3`, `letter` (300 dpi) · `square`, `story`, `wide` (72 dpi),
or `"custom"` with `size: [w, h]` (64–20000 px, 36–1200 dpi).

**Templates:** `stack` (print), `hero` (square/story), `split` (wide), `band`, `frame`. `auto`
follows the page: tall stacks, wide splits, square heroes.

**Style hints** are accepted and recorded; `mono` desaturates the accent today. Unknown hints are
ignored rather than rejected.

## Why the model does not place anything

Models are good at words and bad at coordinates. So the brief carries no positions: the template
owns the grid, and the type size is *fitted* — the largest size (bounded) at which an estimate of
the wrapping stays inside the box's line and height budget (`photocraft_ai::fit`). The estimate
wraps greedily at word boundaries with per-character advances, so it lands within a couple of
percent of a real line. A 4-character headline and a 40-character headline therefore both look
right in the same box, on any canvas, without asking the model to do arithmetic. Text is set as
paragraph text in a box, so the type engine reflows rather than overflows, and every element is
clamped onto the canvas before it is planned.

The box is deliberately generous, so the type engine never has to clip; the fitting therefore also
reports *how tall the words will actually be*, and each block below follows that height instead of
the box. Without it a two-word headline leaves the empty height of three lines under it — a void
that reads as a layout bug even though every element is on the canvas. Two tests keep it honest
(`the_subhead_follows_the_headline_rather_than_its_empty_box`,
`the_divider_sits_above_the_body_not_in_the_void`).

Type sizes are computed in **pixels** and converted to **points** at the document's resolution: a
100 px headline is 24 pt on a 300 dpi A4 and 100 pt on a 72 dpi square. Getting this wrong is the
usual way a scripted poster ends up unreadably small. The plan's answer carries the document's
`dpi` for exactly this reason: the panel's preview converts points back to pixels with it.

## The model call

`crates/ai` never opens a socket. A `Transport` does the POST, and the crate still compiles for
`wasm32` and every test runs offline. The two targets differ only in that implementation:

| Target | Transport |
|---|---|
| desktop | `ureq` (already in the tree through `craft-update`): one blocking POST, rustls with the platform's certificate store, 120 s timeout, a 1 MiB body cap |
| Android | `ai-request.json` / `ai-answer.json` in the bridge, POSTed by Java `HttpURLConnection`: Android's own trust store, no TLS cross-compilation, no `unsafe` |
| tests | `Fake` / `Scripted`: the whole path — prompt, request, answer, brief, design, plan — with no key and no connection |

The request is deliberately plain: `model`, the system and user messages, `stream: false`. No
`temperature` (reasoning models reject it) and no `response_format` (some compatible servers answer
400 for it). `extract_json` unwraps a fence or a stray sentence instead.

A non-2xx answer is read rather than discarded: the server's own `{"error":{"message":…}}` becomes
the sentence beside the panel's buttons, because "Incorrect API key provided" is actionable and
`400` is not.

### Where the key lives

Never in a command parameter. Parameters are journaled, replayed by actions and readable through
the control channel; `ai.poster.create` refuses a `key`, `apiKey`, `api_key`, `token`,
`authorization` or `secret` parameter **by name**, with an error that does not echo the value.

The key lives in `ai.json` in the app's config directory (beside `preferences.json`), written
through the same atomic write as the preferences and created `0600` on Unix. On Android it sits in
`ai.json` in the app's private files directory, which is already per-app. `Settings`' `Debug` is
redacted, so a settings value can be logged or printed in a failing test without printing the key.
The panel states where the file is rather than hiding it.

### Why the engine owns the answer

`ai.poster.create` starts the request on `Session::ai`'s background seam and returns at once;
`ai.poster.status` polls it. The engine composes the brief in `poll()`, so the panel, the CLI, the
control channel and MCP read **one** state, and prompt → document is three calls for an agent. The
seam is the same shape as the update check (`updates_cmds`), so there is one background pattern in
the engine rather than two.

## Commands

Everything user-visible is an engine command, so the UI, the CLI, the control channel and MCP all
reach it by id:

| id | does |
|---|---|
| `ai.poster.presets` | presets, templates and the brief schema (for menus and prompts) |
| `ai.poster.plan` | `{"brief": …}` → the design and its steps, changing nothing |
| `ai.poster.build` | `{"brief": …, "name": …}` → one new document, empty history |
| `ai.poster.create` | `{"prompt": …}` → starts the model call; returns at once |
| `ai.poster.status` | what the background request came to: `idle` / `planning` / `ready` / `failed` |
| `ai.poster.cancel` | drops the request; a late answer is discarded |

```sh
# No model needed: a brief becomes a document.
cargo run -p photocraft-cli -- run --new '{"width":1,"height":1}' \
  --cmd ai.poster.build --params '{"brief":{"title":"Autumn Print Fair","format":"a4"}}'

# With a provider configured: prompt -> answer -> document.
cargo run -p photocraft-cli -- run --new '{"width":1,"height":1}' \
  --cmd ai.poster.create --params '{"prompt":"a night market for ceramics, Saturday"}'
cargo run -p photocraft-cli -- run --new '{"width":1,"height":1}' --cmd ai.poster.status
```

`ai.poster.build` is atomic: if any step fails the half-built document is closed, so a bad brief
leaves the open documents exactly as they were. It never panics on hostile parameters (the
`panic_hunt` gate covers every command).

## Layout of the code

```text
crates/ai (photocraft-ai, layer 2, no network, no egui)
  brief.rs     untrusted JSON -> validated Brief (limits, colours, formats)
  format.rs    presets; px <-> pt at the document's resolution
  design.rs    Rect/Text/Shape/Fill elements, clamped to the canvas
  fit.rs       the largest type size that fits a box
  template.rs  five templates + curated palettes (deterministic by title)
  plan.rs      design -> engine steps, with $layer references for renames
  color.rs     #rrggbb, luminance, readable ink on a colour
  provider.rs  Planner trait, the prompts, Scripted
  http.rs      Transport trait, Fake, Endpoint/Key checks, extract_json, one_line/refusal
  openai.rs    the OpenAI-compatible request and answer
  settings.rs  base URL, model, key (redacted Debug)
crates/engine/src/cmd/ai_cmds.rs
  ai.poster.*  validate, compose, execute, roll back on failure, clear history,
               and the background request (create/status/cancel)
crates/ui-egui/src/ai_ui.rs
  the panel, the preview painter, and the PlanFn over the app's planner
apps/photocraft/src/ai.rs
  the desktop transport (ureq) and ai.json
apps/photocraft-android/src/ai.rs
  the Android transport: ai-request.json / ai-answer.json through the bridge
apps/photocraft-android/android/.../PhotoCraftActivity.java
  checkAiRequest / runAiRequest: the POST with HttpURLConnection
```

The plan uses `file.new`, `layer.newFillLayer.solidColor|gradient`, `layer.renameLayer`,
`shape.create` and `type.create` — nothing bespoke. A test asserts the step list contains only
these ids, so a typo fails in the crate rather than halfway through building a document.

## Not built yet

1. **Android, built and run on a device.** The bridge pair and the Java `HttpURLConnection` handler
   are written, and the Rust half (transport, settings, bridge round trip) passes its tests on the
   host. This machine has no SDK/NDK/JDK, so neither the APK nor the Java has been compiled here:
   run `apps/photocraft-android/build.sh` on a machine with the toolchain before relying on it. The
   `INTERNET` permission is already in the manifest (used by the updater). The key is in the app's
   private `ai.json`, not the Keystore yet.
2. **The web build.** No model call there: the engine is happy to plan and build, but nothing can
   ask a model.
3. **Image generation** into the imagery slots (smart objects), so a hero image is one generated
   layer and the rest stays editable.
4. **Release wiring**: a release tag once 1–3 land.
