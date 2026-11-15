//! The templates: a brief becomes a real composition here.
//!
//! Five layouts, each a grid worked out from the canvas, so the same brief looks right on an A3
//! sheet and on a 1080 square. The AI picks one by name or leaves it to `auto`, which follows the
//! page's orientation.
//!
//! A template only decides *where* things go and *how big* the type is; the words, the palette and
//! the mood come from the brief. That split is what keeps a model's output from turning into a
//! pile of overlapping layers.

use crate::brief::{Brief, ParsedPalette, Texts};
use crate::color::Rgb;
use crate::design::{Align, Design, Element, Rect, ShapeElement, TextElement};
use crate::error::Result;
use crate::fit::Fit;
use crate::format::{Format, Orientation};

/// The template ids, most general first.
pub const IDS: &[&str] = &["stack", "hero", "split", "band", "frame"];

/// Every template id.
pub fn ids() -> Vec<&'static str> {
    IDS.to_vec()
}

/// Resolve a template id (lower case), or `auto`.
pub fn resolve(id: &str) -> Option<&'static str> {
    if id.eq_ignore_ascii_case("auto") {
        return Some("auto");
    }
    IDS.iter().copied().find(|known| known.eq_ignore_ascii_case(id))
}

/// `auto` follows the page: a tall page stacks, a wide one splits, a square one makes the headline
/// the picture.
fn choose(requested: &'static str, format: Format) -> &'static str {
    if requested != "auto" {
        return requested;
    }
    match format.orientation() {
        Orientation::Portrait => "stack",
        Orientation::Landscape => "split",
        Orientation::Square => "hero",
    }
}

/// The canvas' own measurements: one unit is 1% of the short side, so every template scales with
/// the page rather than with a resolution.
#[derive(Clone, Copy, Debug)]
struct Metrics {
    w: f32,
    h: f32,
    unit: f32,
    /// The page margin.
    m: f32,
    /// Half a margin: the gap between related blocks.
    gutter: f32,
    /// The text column: the canvas less its margins.
    col: f32,
    /// The height between the margins.
    content_h: f32,
}

impl Metrics {
    fn new(format: Format) -> Self {
        let w = format.width as f32;
        let h = format.height as f32;
        let short = format.short_side();
        // A margin of 7% of the short side, never under 10 px and never more than 16%: a margin
        // has to exist on a 64 px thumbnail and must not swallow a small canvas.
        let m = (short * 0.07).max(10.0).min(short * 0.16);
        Self { w, h, unit: short * 0.01, m, gutter: m * 0.5, col: w - 2.0 * m, content_h: h - 2.0 * m }
    }

    /// Fit a text size into `rect`, between `min_units` and `max_units` of the unit.
    #[allow(clippy::too_many_arguments)]
    fn fit(&self, text: &str, rect: Rect, max_lines: usize, advance: f32, leading: f32, min_units: f32, max_units: f32) -> f32 {
        self.fitter(rect, max_lines, advance, leading, min_units, max_units).size(text)
    }

    /// The fitted size, and the height the type will actually take in the box.
    ///
    /// A text box is generous so the type engine never has to clip; that means its box is taller
    /// than its words whenever the copy is short. Blocks below follow the *words*, or a
    /// two-word headline leaves the void of three it never used.
    #[allow(clippy::too_many_arguments)]
    fn fitted(&self, text: &str, rect: Rect, max_lines: usize, advance: f32, leading: f32, min_units: f32, max_units: f32) -> (f32, f32) {
        let fitter = self.fitter(rect, max_lines, advance, leading, min_units, max_units);
        let size = fitter.size(text);
        (size, fitter.estimate(text, size).height.min(rect.h).max(1.0))
    }

    fn fitter(&self, rect: Rect, max_lines: usize, advance: f32, leading: f32, min_units: f32, max_units: f32) -> Fit {
        Fit { box_w: rect.w, box_h: rect.h, max_lines, advance, leading, min_px: self.unit * min_units, max_px: self.unit * max_units }
    }
}

/// Curated palettes: the AI may send its own, but when it does not, one of these is picked by the
/// title, so two posters with different words do not look like the same poster twice.
const PALETTES: &[(&str, &str, &str)] = &[
    ("#f4f1ea", "#17161a", "#e2402c"),
    ("#101418", "#f2f4f7", "#7bd3f7"),
    ("#efe6d8", "#2b2118", "#c2703c"),
    ("#0f1f18", "#eaf3ee", "#7dc9a0"),
    ("#eef1f8", "#0f1b3d", "#2f4fd8"),
    ("#ffffff", "#111111", "#111111"),
];

/// The palette a design uses: the brief's own, or a curated one chosen by title and template.
fn palette_for(brief: &Brief, template: &str) -> Result<ParsedPalette> {
    let base = match &brief.palette {
        Some(palette) => palette.parsed()?,
        None => {
            let (background, ink, accent) = PALETTES[hash(&brief.title, template) % PALETTES.len()];
            ParsedPalette {
                background: Rgb::parse("palette.background", background)?,
                ink: Rgb::parse("palette.ink", ink)?,
                accent: Rgb::parse("palette.accent", accent)?,
            }
        }
    };
    Ok(if mono(brief) { ParsedPalette { accent: base.ink, ..base } } else { base })
}

/// Whether the brief asked for a single-hue look.
fn mono(brief: &Brief) -> bool {
    brief.style.iter().any(|hint| hint.trim().eq_ignore_ascii_case("mono"))
}

/// FNV-1a over the title and template: stable across runs and platforms, unlike a hasher seeded
/// per process.
fn hash(title: &str, template: &str) -> usize {
    let mut h: u32 = 0x811c_9dc5;
    for byte in title.trim().to_lowercase().bytes().chain(b"\0".iter().copied()).chain(template.bytes()) {
        h ^= u32::from(byte);
        h = h.wrapping_mul(0x0100_0193);
    }
    h as usize
}

/// Compose `brief` into a design: format, template, palette and every element, back to front.
pub fn compose(brief: &Brief) -> Result<Design> {
    let format = brief.format()?;
    let template = choose(brief.template_id()?, format);
    let palette = palette_for(brief, template)?;
    let metrics = Metrics::new(format);
    let texts = brief.texts();
    let mut design = Design::new(format, template, palette.background);
    design.style = brief.style.iter().map(|hint| hint.trim().to_ascii_lowercase()).filter(|hint| !hint.is_empty()).collect();
    match template {
        "hero" => hero(&metrics, &palette, &texts, &mut design),
        "split" => split(&metrics, &palette, &texts, &mut design),
        "band" => band(&metrics, &palette, &texts, &mut design),
        "frame" => frame(&metrics, &palette, &texts, &mut design),
        _ => stack(&metrics, &palette, &texts, &mut design),
    }
    design.clamp_to_canvas();
    Ok(design)
}

/// Rule of the given thickness, the small accent line templates use as a divider.
fn rule(name: &str, x: f32, y: f32, w: f32, color: Rgb, thickness: f32) -> Element {
    Element::Shape(ShapeElement::filled(name, Rect::new(x, y, w, thickness.max(2.0)), color))
}

/// Stack: a print poster. Accent rule, headline, subtitle, a short rule, the body low on the page,
/// the details along the bottom.
fn stack(m: &Metrics, p: &ParsedPalette, t: &Texts, d: &mut Design) {
    let top = Rect::new(m.m, m.m, m.col, (m.unit * 0.9).max(4.0));
    d.elements.push(Element::Shape(ShapeElement::filled("Accent Rule", top, p.accent)));

    let title_box = Rect::new(m.m, top.bottom() + m.gutter * 0.9, m.col, m.content_h * 0.34);
    let (title_size, title_height) = m.fitted(&t.title, title_box, 3, 0.56, 1.06, 2.4, 10.0);
    let mut title = TextElement::new("Headline", t.title.clone(), title_box, p.ink);
    title.size_px = title_size;
    title.weight = 800;
    title.leading = 1.06;
    title.tracking = -20.0;
    d.elements.push(Element::Text(title));

    let mut cursor = title_box.y + title_height + m.gutter * 0.9;
    if let Some(subtitle) = &t.subtitle {
        let area = Rect::new(m.m, cursor, m.col * 0.88, m.content_h * 0.13);
        let (size, height) = m.fitted(subtitle, area, 2, 0.5, 1.3, 1.7, (title_size / m.unit * 0.4).clamp(2.0, 5.0));
        let mut text = TextElement::new("Subhead", subtitle.clone(), area, p.ink);
        text.size_px = size;
        text.tracking = 10.0;
        cursor = area.y + height + m.gutter * 1.6;
        d.elements.push(Element::Text(text));
    }

    // A print poster reads from the top: the body sits in the lower half, and the short accent
    // rule travels with it as the label above the paragraph.
    let body_y = if t.body.is_some() { (m.h * 0.60).max(cursor + m.gutter * 1.4) } else { cursor };
    d.elements.push(rule("Divider", m.m, body_y, (m.col * 0.16).max(8.0), p.accent, m.unit * 0.35));

    if let Some(body) = &t.body {
        let area = Rect::new(m.m, body_y + m.gutter * 1.2, m.col * 0.72, ((m.h - m.m - m.unit * 4.6) - body_y).max(m.unit * 2.0));
        let mut text = TextElement::new("Body", body.clone(), area, p.ink);
        text.size_px = m.fit(body, area, 8, 0.5, 1.5, 1.7, 3.2);
        text.leading = 1.5;
        d.elements.push(Element::Text(text));
    }
    if let Some(footer) = &t.footer {
        let area = Rect::new(m.m, m.h - m.m - m.unit * 3.6, m.col, m.unit * 3.6);
        let mut text = TextElement::new("Details", footer.clone(), area, p.accent);
        text.size_px = m.fit(footer, area, 1, 0.62, 1.2, 1.5, 2.6);
        text.weight = 600;
        text.tracking = 120.0;
        text.caps = true;
        d.elements.push(Element::Text(text));
    }
}

/// Hero: a square or story post. A deep gradient, the headline centred in it, everything reversed
/// out so the colour carries the picture.
fn hero(m: &Metrics, p: &ParsedPalette, t: &Texts, d: &mut Design) {
    let ink = p.on_accent();
    d.elements.push(Element::Fill {
        name: "Background Gradient".into(),
        paint: crate::design::Paint::Gradient { from: p.accent, to: p.accent.shade(0.5), angle: 135.0 },
    });
    let title_box = Rect::new(m.m * 1.15, m.h * 0.26, m.w - 2.3 * m.m, m.h * 0.30);
    let (title_size, title_height) = m.fitted(&t.title, title_box, 4, 0.58, 1.04, 3.0, 12.0);
    let mut title = TextElement::new("Headline", t.title.clone(), title_box, ink);
    title.size_px = title_size;
    title.weight = 900;
    title.leading = 1.04;
    title.tracking = -25.0;
    title.align = Align::Center;
    d.elements.push(Element::Text(title));

    let rule_w = (m.col * 0.28).min(m.unit * 14.0).max(8.0);
    let rule_y = title_box.y + title_height + m.gutter;
    d.elements.push(rule("Divider", (m.w - rule_w) * 0.5, rule_y, rule_w, ink, m.unit * 0.35));

    if let Some(subtitle) = &t.subtitle {
        let area = Rect::new(m.m * 1.5, rule_y + m.gutter, m.w - 3.0 * m.m, m.h * 0.11);
        let mut text = TextElement::new("Subhead", subtitle.clone(), area, ink);
        text.size_px = m.fit(subtitle, area, 2, 0.5, 1.35, 1.8, 4.2);
        text.align = Align::Center;
        text.tracking = 15.0;
        d.elements.push(Element::Text(text));
    }
    if let Some(footer) = &t.footer {
        let area = Rect::new(m.m, m.h - m.m - m.unit * 3.4, m.col, m.unit * 3.4);
        let mut text = TextElement::new("Details", footer.clone(), area, ink);
        text.size_px = m.fit(footer, area, 1, 0.62, 1.2, 1.5, 2.4);
        text.weight = 600;
        text.tracking = 140.0;
        text.align = Align::Center;
        text.caps = true;
        d.elements.push(Element::Text(text));
    }
}

/// Split: a wide poster. Text on the left in a column, a full-height accent panel on the right with
/// a placeholder where an image goes.
fn split(m: &Metrics, p: &ParsedPalette, t: &Texts, d: &mut Design) {
    let panel_x = m.w * 0.60;
    d.elements.push(Element::Shape(ShapeElement::filled("Accent Panel", Rect::new(panel_x, 0.0, m.w - panel_x, m.h), p.accent)));
    let pad = m.m * 0.55;
    d.elements.push(Element::Shape(ShapeElement::filled(
        "Image Placeholder",
        Rect::new(panel_x + pad, pad, (m.w - panel_x) - 2.0 * pad, m.h - 2.0 * pad),
        p.accent.shade(0.22),
    )));

    let col = panel_x - 2.0 * m.m;
    let title_box = Rect::new(m.m, m.m * 1.1, col, m.h * 0.34);
    let (title_size, title_height) = m.fitted(&t.title, title_box, 3, 0.56, 1.06, 2.4, 9.0);
    let mut title = TextElement::new("Headline", t.title.clone(), title_box, p.ink);
    title.size_px = title_size;
    title.weight = 800;
    title.leading = 1.06;
    title.tracking = -18.0;
    d.elements.push(Element::Text(title));

    let mut cursor = title_box.y + title_height + m.gutter * 0.9;
    if let Some(subtitle) = &t.subtitle {
        let area = Rect::new(m.m, cursor, col * 0.9, m.h * 0.12);
        let (size, height) = m.fitted(subtitle, area, 2, 0.5, 1.3, 1.7, (title_size / m.unit * 0.4).clamp(2.0, 4.5));
        let mut text = TextElement::new("Subhead", subtitle.clone(), area, p.ink);
        text.size_px = size;
        cursor = area.y + height + m.gutter * 1.6;
        d.elements.push(Element::Text(text));
    }

    let body_y = if t.body.is_some() { (m.h * 0.56).max(cursor + m.gutter * 1.4) } else { cursor };
    d.elements.push(rule("Divider", m.m, body_y, (col * 0.2).max(8.0), p.accent, m.unit * 0.35));

    if let Some(body) = &t.body {
        let area = Rect::new(m.m, body_y + m.gutter * 1.2, col * 0.92, ((m.h - m.m - m.unit * 4.4) - body_y).max(m.unit * 2.0));
        let mut text = TextElement::new("Body", body.clone(), area, p.ink);
        text.size_px = m.fit(body, area, 10, 0.5, 1.5, 1.7, 3.0);
        text.leading = 1.5;
        d.elements.push(Element::Text(text));
    }
    if let Some(footer) = &t.footer {
        let area = Rect::new(m.m, m.h - m.m - m.unit * 3.4, col, m.unit * 3.4);
        let mut text = TextElement::new("Details", footer.clone(), area, p.accent);
        text.size_px = m.fit(footer, area, 1, 0.62, 1.2, 1.5, 2.4);
        text.weight = 600;
        text.tracking = 120.0;
        text.caps = true;
        d.elements.push(Element::Text(text));
    }
}

/// Band: the headline sits inside a full-width colour band, with a subhead above it and the body
/// below. Reads well from a distance and on a phone.
fn band(m: &Metrics, p: &ParsedPalette, t: &Texts, d: &mut Design) {
    let band_y = m.h * 0.36;
    let band_h = m.h * 0.28;
    d.elements.insert(0, Element::Shape(ShapeElement::filled("Accent Band", Rect::new(0.0, band_y, m.w, band_h), p.accent)));

    if let Some(subtitle) = &t.subtitle {
        let area = Rect::new(m.m, m.m * 1.2, m.col * 0.8, band_y - m.m * 2.2);
        let mut text = TextElement::new("Subhead", subtitle.clone(), area, p.ink);
        text.size_px = m.fit(subtitle, area, 3, 0.5, 1.35, 1.7, 4.5);
        text.weight = 500;
        d.elements.push(Element::Text(text));
    }

    let inner_pad = band_h * 0.18;
    let title_box = Rect::new(m.m, band_y + inner_pad, m.col, band_h - 2.0 * inner_pad);
    let mut title = TextElement::new("Headline", t.title.clone(), title_box, p.on_accent());
    title.size_px = m.fit(&t.title, title_box, 2, 0.58, 1.04, 2.6, 11.0);
    title.weight = 900;
    title.leading = 1.04;
    title.tracking = -20.0;
    title.align = Align::Center;
    d.elements.push(Element::Text(title));

    if let Some(body) = &t.body {
        let area = Rect::new(m.m, band_y + band_h + m.gutter * 1.4, m.col * 0.62, m.h - (band_y + band_h) - m.m * 1.6);
        let mut text = TextElement::new("Body", body.clone(), area, p.ink);
        text.size_px = m.fit(body, area, 8, 0.5, 1.5, 1.7, 3.0);
        text.leading = 1.5;
        d.elements.push(Element::Text(text));
    }
    if let Some(footer) = &t.footer {
        let area = Rect::new(m.m, m.h - m.m - m.unit * 3.2, m.col, m.unit * 3.2);
        let mut text = TextElement::new("Details", footer.clone(), area, p.accent);
        text.size_px = m.fit(footer, area, 1, 0.62, 1.2, 1.5, 2.3);
        text.weight = 600;
        text.tracking = 120.0;
        text.caps = true;
        d.elements.push(Element::Text(text));
    }
}

/// Frame: a gallery notice. Hairline frame, centred headline with wide tracking, everything quiet.
fn frame(m: &Metrics, p: &ParsedPalette, t: &Texts, d: &mut Design) {
    let inset = (m.m * 0.62).max(6.0);
    d.elements.push(Element::Shape(ShapeElement::outlined(
        "Frame",
        Rect::new(inset, inset, m.w - 2.0 * inset, m.h - 2.0 * inset),
        p.accent,
        (m.unit * 0.22).max(2.0),
    )));
    let title_box = Rect::new(m.m * 1.5, m.h * 0.34, m.w - 3.0 * m.m, m.h * 0.24);
    let (title_size, title_height) = m.fitted(&t.title, title_box, 3, 0.58, 1.08, 2.4, 9.0);
    let mut title = TextElement::new("Headline", t.title.clone(), title_box, p.ink);
    title.size_px = title_size;
    title.weight = 700;
    title.leading = 1.08;
    title.tracking = 30.0;
    title.align = Align::Center;
    d.elements.push(Element::Text(title));

    let rule_w = (m.col * 0.18).min(m.unit * 12.0).max(6.0);
    let rule_y = title_box.y + title_height + m.gutter;
    d.elements.push(rule("Divider", (m.w - rule_w) * 0.5, rule_y, rule_w, p.accent, m.unit * 0.3));

    if let Some(subtitle) = &t.subtitle {
        let area = Rect::new(m.m * 1.5, rule_y + m.gutter, m.w - 3.0 * m.m, m.h * 0.12);
        let mut text = TextElement::new("Subhead", subtitle.clone(), area, p.ink);
        text.size_px = m.fit(subtitle, area, 2, 0.5, 1.35, 1.7, 4.0);
        text.align = Align::Center;
        d.elements.push(Element::Text(text));
    }
    if let Some(body) = &t.body {
        let area = Rect::new(m.m * 1.9, m.h * 0.68, m.w - 3.8 * m.m, m.h * 0.16);
        let mut text = TextElement::new("Body", body.clone(), area, p.ink);
        text.size_px = m.fit(body, area, 6, 0.5, 1.5, 1.7, 2.8);
        text.leading = 1.5;
        text.align = Align::Center;
        d.elements.push(Element::Text(text));
    }
    if let Some(footer) = &t.footer {
        let area = Rect::new(m.m, m.h - m.m - m.unit * 3.2, m.col, m.unit * 3.2);
        let mut text = TextElement::new("Details", footer.clone(), area, p.accent);
        text.size_px = m.fit(footer, area, 1, 0.62, 1.2, 1.4, 2.2);
        text.weight = 600;
        text.tracking = 160.0;
        text.align = Align::Center;
        text.caps = true;
        d.elements.push(Element::Text(text));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brief::Brief;
    use serde_json::json;

    fn brief(v: serde_json::Value) -> Brief {
        Brief::from_json(&v).expect("valid brief")
    }

    fn full(template: &str, format: &str) -> Design {
        compose(&brief(json!({
            "title": "Autumn Print Fair",
            "subtitle": "Twelve studios open late",
            "body": "Prints, posters and new work from twelve artists, with the studios open until nine.",
            "footer": "Sat 14 Nov · 6-9 pm · Mill Lane",
            "format": format,
            "template": template,
        })))
        .expect("composes")
    }

    #[test]
    fn auto_follows_the_page_orientation() {
        assert_eq!(full("auto", "a4").template, "stack");
        assert_eq!(full("auto", "wide").template, "split");
        assert_eq!(full("auto", "square").template, "hero");
    }

    #[test]
    fn every_template_on_every_preset_composes_inside_the_canvas() {
        for preset in crate::format::PRESETS {
            for template in IDS {
                let d = full(template, preset.id);
                assert_eq!(d.template, *template);
                assert!(d.layer_count() >= 3, "{template} on {} made {} layers", preset.id, d.layer_count());
                let (w, h) = (d.format.width as f32, d.format.height as f32);
                for element in &d.elements {
                    let rect = match element {
                        Element::Fill { .. } => continue,
                        Element::Shape(s) => s.rect,
                        Element::Text(t) => t.rect,
                    };
                    assert!(rect.x >= 0.0 && rect.y >= 0.0, "{template}/{} off canvas: {rect:?}", preset.id);
                    assert!(rect.right() <= w + 0.01 && rect.bottom() <= h + 0.01, "{template}/{} over edge: {rect:?}", preset.id);
                    assert!(rect.w >= 1.0 && rect.h >= 1.0, "{template}/{} empty area: {rect:?}", preset.id);
                }
                for text in d.texts() {
                    assert!(text.size_px >= 1.0 && text.size_px.is_finite(), "{template}: {:?} size {}", text.name, text.size_px);
                    assert!(text.text.chars().count() > 0);
                }
            }
        }
    }

    #[test]
    fn the_headline_is_the_largest_type() {
        for template in IDS {
            let d = full(template, "a4");
            let title = d.texts().find(|t| t.name == "Headline").expect("a headline");
            for other in d.texts().filter(|t| t.name != "Headline") {
                assert!(other.size_px <= title.size_px, "{template}: {} is bigger than the headline", other.name);
            }
            assert!(title.weight >= 700, "{template}: headline is not bold");
        }
    }

    #[test]
    fn the_subhead_follows_the_headline_rather_than_its_empty_box() {
        // A short headline set in a generous box used to leave the height of three lines under
        // it; the block below must follow the words.
        for preset in crate::format::PRESETS {
            let d = full("stack", preset.id);
            let title = d.texts().find(|t| t.name == "Headline").expect("a headline");
            let sub = d.texts().find(|t| t.name == "Subhead").expect("a subhead");
            let gap = sub.rect.y - title.rect.y;
            assert!(gap > title.size_px, "{}: the subhead overlaps the headline ({gap} for {} px type)", preset.id, title.size_px);
            assert!(gap < d.format.height as f32 * 0.22, "{}: a {gap} px void under the headline", preset.id);
        }
    }

    #[test]
    fn the_divider_sits_above_the_body_not_in_the_void() {
        for preset in crate::format::PRESETS {
            let d = full("split", preset.id);
            let divider = d.elements.iter().find(|e| e.name() == "Divider").expect("a divider");
            let body = d.texts().find(|t| t.name == "Body").expect("a body");
            let Element::Shape(rule) = divider else { panic!("the divider is a rule") };
            assert!(rule.rect.y < body.rect.y, "{}: the divider is below the body", preset.id);
            assert!(body.rect.y - rule.rect.y < body.rect.h, "{}: the divider floats far from the body", preset.id);
        }
    }

    #[test]
    fn extended_latin_and_long_titles_still_fit() {
        let d = compose(&brief(json!({
            "title": "Festival International des Arts — Édition Numéro Douze",
            "subtitle": "Avec la participation de douze ateliers",
            "body": "Des estampes, des affiches, et des œuvres nouvelles.",
            "format": "a4",
            "template": "stack",
        })))
        .expect("composes");
        assert!(d.texts().all(|t| t.size_px >= 1.0 && t.size_px.is_finite()));
    }

    #[test]
    fn a_brief_with_only_a_title_composes_in_every_template() {
        for template in IDS {
            let d = compose(&brief(json!({"title": "Solo", "template": template, "format": "square"}))).expect("composes");
            assert!(d.texts().count() >= 1, "{template}");
            assert_eq!(d.texts().count(), 1, "{template} invented text");
        }
    }

    #[test]
    fn the_brief_controls_the_palette() {
        let d = compose(&brief(json!({
            "title": "Mono",
            "palette": {"background": "#ffffff", "ink": "#111111", "accent": "#ff0000"},
        })))
        .expect("composes");
        assert_eq!(d.paper.hex(), "#ffffff");
        let Element::Shape(rule) = &d.elements[0] else { panic!("the first layer is the accent rule") };
        assert_eq!(rule.fill.expect("filled").hex(), "#ff0000");
    }

    #[test]
    fn the_mono_hint_desaturates_the_accent() {
        let d = compose(&brief(json!({
            "title": "Mono",
            "style": ["mono"],
            "palette": {"background": "#ffffff", "ink": "#111111", "accent": "#ff0000"},
        })))
        .expect("composes");
        let Element::Shape(rule) = &d.elements[0] else { panic!("the first layer is the accent rule") };
        assert_eq!(rule.fill.expect("filled").hex(), "#111111");
    }

    #[test]
    fn an_unstated_palette_is_stable_and_varies_with_the_title() {
        let a = compose(&brief(json!({"title": "Autumn Print Fair"}))).expect("composes");
        let b = compose(&brief(json!({"title": "Autumn Print Fair"}))).expect("composes");
        assert_eq!(a.paper, b.paper);
        let titles: Vec<String> = ["Autumn Print Fair", "Summer Jam", "Night Market", "Open Studios"]
            .iter()
            .map(|title| compose(&brief(json!({"title": title}))).expect("composes").paper.hex())
            .collect();
        assert!(titles.iter().any(|c| c != &titles[0]), "four titles all took the same palette");
    }

    #[test]
    fn hero_reverses_text_out_of_its_gradient() {
        let d = full("hero", "square");
        let Element::Fill { paint, .. } = &d.elements[0] else { panic!("hero starts with a gradient") };
        assert!(matches!(paint, crate::design::Paint::Gradient { .. }));
        let title = d.texts().next().expect("headline");
        assert!(title.color.luminance() > 0.5, "headline should be light on the dark accent");
    }

    #[test]
    fn split_puts_its_panel_on_the_right_and_a_wide_poster_stays_wide() {
        let d = full("split", "wide");
        assert!(d.elements.iter().any(|e| e.name() == "Accent Panel"));
        assert!(d.elements.iter().any(|e| e.name() == "Image Placeholder"));
        let Element::Shape(panel) = d.elements.iter().find(|e| e.name() == "Accent Panel").expect("panel") else { panic!("shape") };
        assert!(panel.rect.x > d.format.width as f32 * 0.5);
    }

    #[test]
    fn composing_is_pure() {
        let b = brief(json!({"title": "Repeatable", "body": "Body text"}));
        assert_eq!(compose(&b).expect("composes"), compose(&b).expect("composes"));
    }
}
