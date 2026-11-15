//! AI poster planning for PhotoCraft.
//!
//! The AI writes a **brief** — a title, some copy, a palette, a mood, a page. This crate turns it
//! into a **design** (geometry and colour, from one of five templates) and then into a **plan**: the
//! ordinary engine commands that build the document. Nothing here touches the network, a document
//! or the UI: the model call goes through a [`Transport`] the app supplies ([`http`]), so a whole
//! poster — prompt, request, answer, brief, design, plan — is exercised in a unit test with no key
//! and no connection.
//!
//! Why a plan rather than a picture: a generated image is one flat layer. A plan builds real type
//! layers, real shape layers and real fill layers, so the poster that arrives on the canvas can be
//! retyped, restyled, re-laid-out and filtered with a plug-in exactly like a hand-made document.
//! That is what makes "create a poster, then keep editing it" work.
//!
//! The brief is untrusted input. Every field is bounded ([`brief`]), every colour is parsed
//! ([`color`]), every text size is fitted to its box ([`fit`]), and every element is clamped onto
//! the canvas ([`design`]). A hostile or broken brief is an [`Error`], never a panic and never a
//! document that cannot be edited.
//!
//! ```no_run
//! use photocraft_ai::{Brief, plan};
//!
//! let brief = Brief::from_json_str(r#"{"title": "Autumn Print Fair", "format": "a4"}"#)?;
//! let steps = plan::steps(&photocraft_ai::compose(&brief)?, "Autumn Print Fair");
//! for step in &steps {
//!     // session.execute(step.command, step.params.clone())?
//!     let _ = step;
//! }
//! # Ok::<(), photocraft_ai::Error>(())
//! ```
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod brief;
pub mod color;
pub mod design;
pub mod fit;
pub mod format;
pub mod http;
pub mod openai;
pub mod plan;
pub mod provider;
pub mod settings;
pub mod template;

mod error;

pub use brief::{Brief, Palette, Texts, VERSION};
pub use color::Rgb;
pub use design::{Align, Design, Element, Paint, Rect, ShapeElement, TextElement};
pub use error::{Error, Result};
pub use format::{Format, Orientation, PRESETS, Preset};
pub use http::{Endpoint, Key, Transport};
pub use openai::OpenAi;
pub use plan::{Created, Ref, Step};
pub use provider::{Planner, Request, Scripted, build_params, system_prompt, user_prompt};
pub use settings::Settings;

/// Compose a brief into a design: the layers, placed and coloured.
pub fn compose(brief: &Brief) -> Result<Design> {
    template::compose(brief)
}

/// The whole job in one call: a brief becomes the steps that build its document, named `name`.
pub fn poster(brief: &Brief, name: &str) -> Result<(Design, Vec<Step>)> {
    let design = compose(brief)?;
    let steps = plan::steps(&design, name);
    Ok((design, steps))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn brief(json: serde_json::Value) -> Brief {
        Brief::from_json(&json).expect("valid brief")
    }

    #[test]
    fn a_brief_becomes_one_editable_document() {
        let b = brief(serde_json::json!({
            "title": "Autumn Print Fair",
            "subtitle": "Twelve studios open late",
            "body": "Prints, posters and new work, with the studios open until nine.",
            "footer": "Sat 14 Nov · 6-9 pm · Mill Lane",
            "format": "a4",
        }));
        let (design, steps) = poster(&b, "Autumn Print Fair").expect("a poster");
        assert_eq!(design.template, "stack");
        assert!(design.layer_count() >= 6, "{} layers", design.layer_count());
        assert_eq!(steps.first().map(|s| s.command), Some("file.new"));
        // Every text becomes its own type layer, so every word can be edited afterwards.
        let type_layers = steps.iter().filter(|s| s.command == "type.create").count();
        assert_eq!(type_layers, design.texts().count());
        assert!(type_layers >= 4);
    }

    #[test]
    fn a_hostile_brief_never_panics() {
        let cases = [
            serde_json::json!({}),
            serde_json::json!({"title": ""}),
            serde_json::json!({"title": "\u{0}\u{1}\u{2}"}),
            serde_json::json!({"title": "x".repeat(100_000)}),
            serde_json::json!({"title": "T", "format": "custom", "size": [0, u32::MAX]}),
            serde_json::json!({"title": "T", "format": "custom", "size": [1, 1], "dpi": 0}),
            serde_json::json!({"title": "T", "template": "../../etc/passwd"}),
            serde_json::json!({"title": "T", "palette": {"background": "#fff", "ink": "red", "accent": "#000000"}}),
            serde_json::json!({"title": "T", "style": vec!["x"; 1000]}),
            serde_json::json!({"title": "T", "body": "a".repeat(1_000_000)}),
            serde_json::json!([1, 2, 3]),
        ];
        for case in cases {
            // Either a poster or a typed error: never a panic, never a half-built plan.
            match Brief::from_json(&case) {
                Ok(b) => {
                    let (design, steps) = poster(&b, "P").expect("a brief that validated must compose");
                    assert!(!steps.is_empty());
                    assert!(design.layer_count() >= 2);
                }
                Err(error) => {
                    let message = error.to_string();
                    assert!(!message.is_empty());
                }
            }
        }
    }

    #[test]
    fn every_preset_and_template_pairs_up() {
        for preset in PRESETS {
            for template in template::IDS {
                let b = brief(serde_json::json!({"title": "Pair", "format": preset.id, "template": template}));
                let (design, steps) = poster(&b, "Pair").expect("a poster");
                assert_eq!(design.template, *template);
                assert!(steps.len() >= 3);
            }
        }
    }
}
