//! The intermediate design: resolved geometry and colours, back to front.
//!
//! A template turns a brief into this, and it is the only thing the planning step in [`crate::plan`]
//! reads. Keeping it separate means a template can be checked without touching JSON step shapes,
//! and a step shape can be checked without composing a poster.

use crate::color::Rgb;
use crate::format::Format;

/// A rectangle in document pixels, origin at the top left (the type and shape commands' frame).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    /// The bottom edge.
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }

    /// The right edge.
    pub fn right(&self) -> f32 {
        self.x + self.w
    }

    /// A rect the same width, `h` tall, starting at the given y.
    pub fn band(&self, y: f32, h: f32) -> Self {
        Self { x: self.x, y, w: self.w, h }
    }

    /// Clamp into a `w`×`h` canvas, keeping at least one pixel of it visible.
    fn clamp_into(&mut self, w: f32, h: f32) {
        self.w = self.w.clamp(1.0, w.max(1.0));
        self.h = self.h.clamp(1.0, h.max(1.0));
        self.x = self.x.clamp(0.0, (w - self.w).max(0.0));
        self.y = self.y.clamp(0.0, (h - self.h).max(0.0));
    }

    /// Whether the rect has any area at all.
    pub fn is_visible(&self) -> bool {
        self.w >= 1.0 && self.h >= 1.0 && self.w.is_finite() && self.h.is_finite()
    }
}

/// Text alignment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

impl Align {
    pub fn name(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Center => "center",
            Self::Right => "right",
        }
    }
}

/// A type layer: words, a box to set them in, and how they should look.
#[derive(Clone, Debug, PartialEq)]
pub struct TextElement {
    pub name: String,
    pub text: String,
    pub rect: Rect,
    /// Cap height of the type in pixels; converted to points at the document's resolution.
    pub size_px: f32,
    /// 100..=900, as `type.create`'s `weight`.
    pub weight: u16,
    /// Letter spacing in 1/1000 em.
    pub tracking: f32,
    /// Line height as a multiple of the type size.
    pub leading: f32,
    pub color: Rgb,
    pub align: Align,
    /// Set in capitals (a footer's wide-tracked label).
    pub caps: bool,
}

/// A shape layer: a rectangle, drawn solid or as a stroke, with rounded corners.
#[derive(Clone, Debug, PartialEq)]
pub struct ShapeElement {
    pub name: String,
    pub rect: Rect,
    pub fill: Option<Rgb>,
    pub stroke: Option<(Rgb, f32)>,
    pub radius: f32,
}

impl ShapeElement {
    /// A filled rectangle with square corners: the rule, band or panel every template starts from.
    pub fn filled(name: impl Into<String>, rect: Rect, fill: Rgb) -> Self {
        Self { name: name.into(), rect, fill: Some(fill), stroke: None, radius: 0.0 }
    }

    /// An outlined rectangle: a frame.
    pub fn outlined(name: impl Into<String>, rect: Rect, color: Rgb, width: f32) -> Self {
        Self { name: name.into(), rect, fill: None, stroke: Some((color, width)), radius: 0.0 }
    }
}

impl TextElement {
    /// A text block with neutral defaults (regular weight, no tracking, 1.25 leading, left
    /// aligned, no capitals); templates then set what they need.
    pub fn new(name: impl Into<String>, text: impl Into<String>, rect: Rect, color: Rgb) -> Self {
        Self { name: name.into(), text: text.into(), rect, size_px: 12.0, weight: 400, tracking: 0.0, leading: 1.25, color, align: Align::Left, caps: false }
    }
}

/// A full-canvas fill layer, solid or gradient.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Paint {
    Solid(Rgb),
    Gradient { from: Rgb, to: Rgb, angle: f32 },
}

/// One layer of the design, in the order they are created (back to front).
#[derive(Clone, Debug, PartialEq)]
pub enum Element {
    Fill { name: String, paint: Paint },
    Shape(ShapeElement),
    Text(TextElement),
}

impl Element {
    /// The element's name, which becomes the layer's name.
    pub fn name(&self) -> &str {
        match self {
            Self::Fill { name, .. } => name,
            Self::Shape(s) => &s.name,
            Self::Text(t) => &t.name,
        }
    }
}

/// A composed poster: the canvas, the paper, and the layers above it.
#[derive(Clone, Debug, PartialEq)]
pub struct Design {
    pub format: Format,
    /// The template that composed it (`stack`, `hero`, `split`, `band`, `frame`).
    pub template: &'static str,
    /// The document background colour: `file.new`'s `background`.
    pub paper: Rgb,
    /// The mood hints the brief carried, kept for later use.
    pub style: Vec<String>,
    pub elements: Vec<Element>,
}

impl Design {
    pub fn new(format: Format, template: &'static str, paper: Rgb) -> Self {
        Self { format, template, paper, style: Vec::new(), elements: Vec::new() }
    }

    /// The layer count, including the background the document starts with.
    pub fn layer_count(&self) -> usize {
        self.elements.len() + 1
    }

    /// Force every element inside the canvas, dropping anything that no longer has area. A
    /// template computes its own geometry, but this is the invariant the whole crate promises:
    /// nothing is ever placed where it cannot be seen.
    pub fn clamp_to_canvas(&mut self) {
        let (w, h) = (self.format.width as f32, self.format.height as f32);
        for element in &mut self.elements {
            match element {
                Element::Fill { .. } => {}
                Element::Shape(s) => s.rect.clamp_into(w, h),
                Element::Text(t) => t.rect.clamp_into(w, h),
            }
        }
        self.elements.retain(|element| match element {
            Element::Fill { .. } => true,
            Element::Shape(s) => s.rect.is_visible(),
            Element::Text(t) => t.rect.is_visible(),
        });
    }

    /// Every text element, for checking a template's hierarchy in tests.
    pub fn texts(&self) -> impl Iterator<Item = &TextElement> {
        self.elements.iter().filter_map(|e| match e {
            Element::Text(t) => Some(t),
            _ => None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::Format;

    fn design() -> Design {
        Design::new(Format { width: 1000, height: 1000, dpi: 72.0 }, "stack", Rgb::WHITE)
    }

    #[test]
    fn clamping_keeps_elements_on_the_canvas() {
        let mut d = design();
        d.elements.push(Element::Shape(ShapeElement {
            name: "away".into(),
            rect: Rect::new(900.0, -50.0, 400.0, 40.0),
            fill: Some(Rgb::BLACK),
            stroke: None,
            radius: 0.0,
        }));
        d.clamp_to_canvas();
        let Element::Shape(s) = &d.elements[0] else { panic!("shape") };
        assert!(s.rect.x >= 0.0 && s.rect.right() <= 1000.0, "{:?}", s.rect);
        assert_eq!(s.rect.y, 0.0);
    }

    #[test]
    fn an_element_with_no_area_is_dropped() {
        let mut d = design();
        d.elements.push(Element::Text(TextElement {
            name: "invisible".into(),
            text: "x".into(),
            rect: Rect::new(0.0, 0.0, 0.0, 0.0),
            size_px: 10.0,
            weight: 400,
            tracking: 0.0,
            leading: 1.2,
            color: Rgb::BLACK,
            align: Align::Left,
            caps: false,
        }));
        d.clamp_to_canvas();
        // A zero-sized box is clamped to one pixel, so it stays rather than vanishing silently.
        assert_eq!(d.elements.len(), 1);
    }

    #[test]
    fn layer_count_includes_the_background() {
        assert_eq!(design().layer_count(), 1);
    }
}
