//! The design brief: the **only** thing the AI produces.
//!
//! It is small on purpose. Models are good at words, colours and intent, and bad at coordinates,
//! so a brief carries what a person would say out loud — a title, some copy, a palette, a mood —
//! and never a layout. The layout is this crate's job, in code that can be tested.
//!
//! Everything here is untrusted input: every field is length-capped, control characters are
//! rejected, colours must be `#rrggbb`, sizes and resolutions are bounded, and unknown fields are
//! ignored so a newer model's extra keys are not a failure.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::color::Rgb;
use crate::error::{Error, Result};
use crate::format::{self, Format};
use crate::template;

/// The brief version this build reads.
pub const VERSION: u32 = 1;
/// Longest title, in characters.
pub const MAX_TITLE: usize = 200;
/// Longest subtitle/body/footer, in characters.
pub const MAX_TEXT: usize = 2000;
/// Most style hints a brief may carry.
pub const MAX_STYLE: usize = 8;

fn one() -> u32 {
    VERSION
}

/// A three-colour palette. Templates derive tints and readable inks from it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Palette {
    /// The paper: the document background.
    pub background: String,
    /// Body and headline colour.
    pub ink: String,
    /// The one saturated colour: rules, bands, panels.
    pub accent: String,
}

impl Palette {
    /// Parse all three, naming the field that is wrong.
    pub fn parsed(&self) -> Result<ParsedPalette> {
        Ok(ParsedPalette {
            background: Rgb::parse("palette.background", &self.background)?,
            ink: Rgb::parse("palette.ink", &self.ink)?,
            accent: Rgb::parse("palette.accent", &self.accent)?,
        })
    }

    /// Reject an all-one-tone palette, where text would vanish into the paper.
    pub fn check_contrast(&self) -> Result<()> {
        let p = self.parsed()?;
        if (p.background.luminance() - p.ink.luminance()).abs() < 0.08 {
            return Err(Error::field("palette", "ink and background are too close in brightness to read"));
        }
        Ok(())
    }
}

/// The palette with its colours parsed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ParsedPalette {
    pub background: Rgb,
    pub ink: Rgb,
    pub accent: Rgb,
}

impl ParsedPalette {
    /// The ink to draw on `background`: the palette's own ink where it can be read, otherwise
    /// whichever of white/black contrasts most.
    pub fn on(self, background: Rgb) -> Rgb {
        if (background.luminance() - self.ink.luminance()).abs() >= 0.2 { self.ink } else { background.readable_on(Rgb::WHITE, Rgb::BLACK) }
    }

    /// The ink to draw on the accent colour (a band or a panel).
    pub fn on_accent(self) -> Rgb {
        self.accent.readable_on(Rgb::WHITE, Rgb::BLACK)
    }
}

/// What the AI writes: words, a palette, and intent.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Brief {
    /// Schema version. Only [`VERSION`] is read.
    #[serde(default = "one")]
    pub version: u32,
    /// The headline. Required, and the only required field.
    pub title: String,
    #[serde(default)]
    pub subtitle: Option<String>,
    #[serde(default)]
    pub body: Option<String>,
    /// A line along the bottom: date, venue, url, price.
    #[serde(default)]
    pub footer: Option<String>,
    /// A [`format::PRESETS`] id. Defaults to [`format::DEFAULT_PRESET`].
    #[serde(default)]
    pub format: Option<String>,
    /// `[width, height]` in pixels, when `format` is `"custom"` (or absent).
    #[serde(default)]
    pub size: Option<[u32; 2]>,
    #[serde(default)]
    pub dpi: Option<f32>,
    /// A [`template::ids`] id, or `"auto"` (the default) to choose from the format.
    #[serde(default)]
    pub template: Option<String>,
    #[serde(default)]
    pub palette: Option<Palette>,
    /// Mood words: `bold`, `minimal`, `editorial`, `retro`, `warm`, `mono`, … Unknown hints are
    /// ignored, so a newer model may send more.
    #[serde(default)]
    pub style: Vec<String>,
}

impl Brief {
    /// Read a brief from an AI's JSON answer, validating every field.
    pub fn from_json(value: &Value) -> Result<Self> {
        let brief: Self = serde_json::from_value(value.clone()).map_err(|e| Error::Json(e.to_string()))?;
        brief.validate()?;
        Ok(brief)
    }

    /// Read a brief from JSON text (an HTTP answer, say).
    pub fn from_json_str(text: &str) -> Result<Self> {
        let value: Value = serde_json::from_str(text).map_err(|e| Error::Json(e.to_string()))?;
        Self::from_json(&value)
    }

    /// Check the version, the lengths, the characters, the size and the palette.
    pub fn validate(&self) -> Result<()> {
        if self.version != VERSION {
            return Err(Error::Version { got: self.version, expected: VERSION });
        }
        if clean("title", &self.title, MAX_TITLE, true)?.is_empty() {
            return Err(Error::field("title", "is empty"));
        }
        for (field, text) in [("subtitle", &self.subtitle), ("body", &self.body), ("footer", &self.footer)] {
            if let Some(text) = text {
                clean(field, text, MAX_TEXT, false)?;
            }
        }
        if self.style.len() > MAX_STYLE {
            return Err(Error::field("style", format!("{} hints; at most {MAX_STYLE} are read", self.style.len())));
        }
        self.format()?;
        if let Some(palette) = &self.palette {
            palette.check_contrast()?;
        }
        self.template_id()?;
        Ok(())
    }

    /// The canvas: the named preset, or the custom size and resolution. A brief that names no
    /// format gets the default preset.
    pub fn format(&self) -> Result<Format> {
        let preset_id = self.format.as_deref().map(str::trim).unwrap_or("");
        let custom = preset_id.eq_ignore_ascii_case("custom") || (preset_id.is_empty() && self.size.is_some());
        if custom {
            return match self.size {
                Some([w, h]) => Format::new(w, h, self.dpi.unwrap_or(72.0)),
                None => Err(Error::Format("\"custom\" needs a size: [width, height]".into())),
            };
        }
        let preset = format::preset(if preset_id.is_empty() { format::DEFAULT_PRESET } else { preset_id })
            .ok_or_else(|| Error::Format(format!("{preset_id:?} is not a preset; use one of {} or \"custom\" with a size", format::preset_ids().join(", "))))?;
        let mut format = preset.format;
        if let Some(dpi) = self.dpi {
            format = Format::new(format.width, format.height, dpi)?;
        }
        Ok(format)
    }

    /// The template id: an explicit one, or `"auto"`.
    pub fn template_id(&self) -> Result<&'static str> {
        let given = self.template.as_deref().unwrap_or("auto").trim().to_ascii_lowercase();
        if given.is_empty() {
            return Ok("auto");
        }
        template::resolve(&given).ok_or_else(|| Error::Template(format!("{given:?} is not a template; use \"auto\" or one of {}", template::ids().join(", "))))
    }

    /// The texts, cleaned for layout (single-line fields collapsed to one line).
    pub fn texts(&self) -> Texts {
        Texts {
            title: clean("title", &self.title, MAX_TITLE, true).unwrap_or_else(|_| self.title.trim().to_string()),
            subtitle: self.subtitle.as_deref().and_then(|t| clean("subtitle", t, MAX_TEXT, true).ok()).filter(|t| !t.is_empty()),
            body: self.body.as_deref().and_then(|t| clean("body", t, MAX_TEXT, false).ok()).filter(|t| !t.is_empty()),
            footer: self.footer.as_deref().and_then(|t| clean("footer", t, MAX_TEXT, true).ok()).filter(|t| !t.is_empty()),
        }
    }
}

/// The brief's words, cleaned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Texts {
    pub title: String,
    pub subtitle: Option<String>,
    pub body: Option<String>,
    pub footer: Option<String>,
}

impl Texts {
    /// Every character count the layout needs, without re-asking the brief.
    pub fn chars_title(&self) -> usize {
        self.title.chars().count()
    }
}

/// Trim a field and check its characters and length. Control characters other than newline (and
/// tab, in a multi-line field) are rejected: they would reach the type engine and the status bar.
fn clean(field: &str, text: &str, max: usize, single_line: bool) -> Result<String> {
    let mut out = String::with_capacity(text.len().min(max * 4));
    for c in text.chars() {
        match c {
            '\n' | '\r' | '\t' if single_line => out.push(' '),
            '\r' => out.push('\n'),
            '\n' | '\t' => out.push(c),
            c if c.is_control() => return Err(Error::field(field, "contains a control character")),
            _ => out.push(c),
        }
    }
    let out = if single_line { out.split_whitespace().collect::<Vec<_>>().join(" ") } else { out.trim().to_string() };
    if out.chars().count() > max {
        return Err(Error::field(field, format!("is {} characters; at most {max} are read", out.chars().count())));
    }
    Ok(out)
}

/// A readable description of the brief, for an AI prompt or for `docs/`.
pub fn schema() -> String {
    let presets = format::preset_ids().join(" | ");
    let templates = template::ids().join(" | ");
    let mut s = String::with_capacity(1200);
    s.push_str("Poster brief (JSON). Only `title` is required.\n{\n");
    s.push_str("  \"version\": 1,\n");
    s.push_str(&format!("  \"title\": \"string, 1..={MAX_TITLE} characters: the headline\",\n"));
    s.push_str("  \"subtitle\": \"string, optional\",\n");
    s.push_str("  \"body\": \"string, optional, may contain newlines\",\n");
    s.push_str("  \"footer\": \"string, optional: date, venue, price, url\",\n");
    s.push_str(&format!("  \"format\": \"one of {presets}, or \\\"custom\\\" with \\\"size\\\"\",\n"));
    s.push_str("  \"size\": [width, height],\n");
    s.push_str("  \"dpi\": \"number, optional\",\n");
    s.push_str(&format!("  \"template\": \"one of {templates}, or \\\"auto\\\"\",\n"));
    s.push_str("  \"palette\": { \"background\": \"#rrggbb\", \"ink\": \"#rrggbb\", \"accent\": \"#rrggbb\" },\n");
    s.push_str("  \"style\": [\"bold\", \"minimal\", \"editorial\", \"retro\", \"warm\", \"mono\"]\n");
    s.push('}');
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn brief(v: Value) -> Brief {
        Brief::from_json(&v).expect("valid brief")
    }

    #[test]
    fn a_minimal_brief_is_enough() {
        let b = brief(json!({"title": "Open Studio"}));
        assert_eq!(b.version, 1);
        assert_eq!(b.format().expect("default format").width, 2480);
        assert_eq!(b.template_id().expect("auto"), "auto");
    }

    #[test]
    fn unknown_fields_are_ignored_but_known_ones_are_checked() {
        let b = brief(json!({"title": "T", "mood": "calm", "template": "stack"}));
        assert_eq!(b.template_id().expect("stack"), "stack");
        assert!(Brief::from_json(&json!({"title": "", "mood": "calm"})).is_err());
        assert!(Brief::from_json(&json!({"title": 12})).is_err());
        assert!(Brief::from_json(&json!({"subtitle": "no title"})).is_err());
    }

    #[test]
    fn version_must_be_the_one_we_read() {
        let e = Brief::from_json(&json!({"title": "T", "version": 2})).expect_err("rejected");
        assert!(matches!(e, Error::Version { got: 2, .. }));
    }

    #[test]
    fn text_is_length_capped_and_controls_rejected() {
        let long = "a".repeat(MAX_TITLE + 1);
        assert!(Brief::from_json(&json!({"title": long})).is_err());
        assert!(Brief::from_json(&json!({"title": "T", "body": "bell \u{7}"})).is_err());
        let ok = brief(json!({"title": "T", "body": "line one\nline two\tindented"}));
        assert_eq!(ok.texts().body.expect("body"), "line one\nline two\tindented");
        let collapsed = brief(json!({"title": "  Two   Words \n"}));
        assert_eq!(collapsed.texts().title, "Two Words");
    }

    #[test]
    fn formats_resolve_from_presets_and_custom_sizes() {
        let b = brief(json!({"title": "T", "format": "square"}));
        assert_eq!(b.format().expect("square").width, 1080);
        let c = brief(json!({"title": "T", "format": "custom", "size": [1200, 800], "dpi": 150}));
        let f = c.format().expect("custom");
        assert_eq!((f.width, f.height), (1200, 800));
        assert!((f.dpi - 150.0).abs() < 0.01);
        assert!(Brief::from_json(&json!({"title": "T", "format": "nope"})).is_err());
        assert!(Brief::from_json(&json!({"title": "T", "format": "custom"})).is_err());
        assert!(Brief::from_json(&json!({"title": "T", "size": [10, 10]})).is_err());
        assert!(Brief::from_json(&json!({"title": "T", "format": "a4", "dpi": 100000})).is_err());
    }

    #[test]
    fn palettes_must_parse_and_contrast() {
        let ok = brief(json!({"title": "T", "palette": {"background": "#f4f1ea", "ink": "#17161a", "accent": "#e2402c"}}));
        assert!(ok.palette.is_some());
        assert!(Brief::from_json(&json!({"title": "T", "palette": {"background": "#f4f1ea", "ink": "#zzzzzz", "accent": "#e2402c"}})).is_err());
        let flat = json!({"title": "T", "palette": {"background": "#f0f0f0", "ink": "#f2f2f2", "accent": "#e2402c"}});
        assert!(Brief::from_json(&flat).is_err(), "unreadable ink must be rejected");
    }

    #[test]
    fn templates_and_style_hints_are_bounded() {
        let b = brief(json!({"title": "T", "template": "HERO "}));
        assert_eq!(b.template_id().expect("hero"), "hero");
        assert!(Brief::from_json(&json!({"title": "T", "template": "spiral"})).is_err());
        let many = json!({"title": "T", "style": vec!["a"; MAX_STYLE + 1]});
        assert!(Brief::from_json(&many).is_err());
    }

    #[test]
    fn schema_lists_every_preset_and_template() {
        let s = schema();
        for id in format::preset_ids() {
            assert!(s.contains(id), "{id} missing from the schema text");
        }
        for id in template::ids() {
            assert!(s.contains(id), "{id} missing from the schema text");
        }
    }

    #[test]
    fn json_text_and_value_agree() {
        let b1 = Brief::from_json_str(r##"{"title": "T"}"##).expect("valid");
        let b2 = brief(json!({"title": "T"}));
        assert_eq!(b1, b2);
        assert!(Brief::from_json_str("not json").is_err());
        assert!(Brief::from_json_str("[1,2,3]").is_err());
    }
}
