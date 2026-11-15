# photocraft-ai

Turns an AI-written **poster brief** into a validated design and the engine commands that build it.

No network, no UI, no document: a whole poster is composed and inspected in a unit test.

```rust
use photocraft_ai::{Brief, poster};

let brief = Brief::from_json_str(r#"{"title": "Autumn Print Fair", "format": "a4"}"#)?;
let (design, steps) = poster(&brief, "Autumn Print Fair")?;
for step in &steps {
    // session.execute(step.command, step.params.clone())?;
}
# Ok::<(), photocraft_ai::Error>(())
```

## Modules

| module | what it holds |
|---|---|
| `brief` | `Brief`, the untrusted input: limits, colours, formats, the schema text |
| `format` | `Format` and the presets (A4/A3/Letter at 300 dpi, square/story/wide at 72), px ↔ pt |
| `design` | `Design`, `Element`, `TextElement`, `ShapeElement`, `Rect`, `Paint`, canvas clamping |
| `fit` | `Fit`: the largest type size whose estimated wrapping fits a box |
| `template` | the five templates (`stack`, `hero`, `split`, `band`, `frame`) and curated palettes |
| `plan` | `steps()`: a design as engine commands; `Created`/`Ref` for naming a layer in a later step |
| `color` | `Rgb`: `#rrggbb`, luminance, shade/tint, the readable ink on a colour |
| `provider` | `Planner` (what the app implements), `Request`, the system/user prompts, `Scripted` |
| `http` | `Transport` (what the app implements), `Fake`, `Endpoint`/`Key` checks, `extract_json` |
| `openai` | `OpenAi`: the OpenAI-compatible chat-completions request and answer |
| `settings` | `Settings`: base URL, model and key, with a redacted `Debug` |

## Rules this crate keeps

- The AI produces a brief, never coordinates and never commands.
- Everything is bounded: text lengths, sizes, resolutions, colour syntax, style hints.
- Every element is clamped onto the canvas before it is planned; nothing is placed where it cannot
  be seen.
- Nothing here can panic: a broken brief is an `Error`.
