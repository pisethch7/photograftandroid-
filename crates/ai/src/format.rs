//! Canvas formats: the poster presets, and the pixel/point conversion the type sizes need.
//!
//! Sizes are kept in **pixels** everywhere in this crate (a layout is geometry), and turned into
//! points only when a `type.create` step is written, because type sizes are points at the
//! document's resolution: a 100 px headline on a 300 dpi A4 is 24 pt, the same headline on a
//! 72 dpi web poster is 100 pt.

use crate::error::{Error, Result};

/// A canvas: pixels, and the resolution that gives them a physical size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Format {
    pub width: u32,
    pub height: u32,
    pub dpi: f32,
}

/// Narrowest or tallest side a brief may ask for, in pixels.
pub const MIN_SIDE: u32 = 64;
/// Largest side a brief may ask for: `file.new`'s own cap is 300000, but a poster this large is a
/// mistake (and the allocation a hostile brief could reach for), so the brief stops well short.
pub const MAX_SIDE: u32 = 20_000;
/// Resolution range. Below 36 dpi a print poster is a thumbnail; above 1200 dpi nothing is gained.
pub const MIN_DPI: f32 = 36.0;
pub const MAX_DPI: f32 = 1200.0;

impl Format {
    /// Validate a format a brief asked for.
    pub fn new(width: u32, height: u32, dpi: f32) -> Result<Self> {
        if !(MIN_SIDE..=MAX_SIDE).contains(&width) || !(MIN_SIDE..=MAX_SIDE).contains(&height) {
            return Err(Error::Format(format!("size {width}×{height} px is outside {MIN_SIDE}..={MAX_SIDE} px")));
        }
        if !dpi.is_finite() || !(MIN_DPI..=MAX_DPI).contains(&dpi) {
            return Err(Error::Format(format!("resolution {dpi} dpi is outside {MIN_DPI}..={MAX_DPI}")));
        }
        Ok(Self { width, height, dpi })
    }

    /// The shorter side: the unit every template scales from.
    pub fn short_side(&self) -> f32 {
        self.width.min(self.height) as f32
    }

    /// The longer side.
    pub fn long_side(&self) -> f32 {
        self.width.max(self.height) as f32
    }

    /// Points for a length in pixels at this resolution.
    pub fn pt(&self, px: f32) -> f32 {
        px * 72.0 / self.dpi
    }

    /// Pixels for a length in points at this resolution.
    pub fn px(&self, pt: f32) -> f32 {
        pt * self.dpi / 72.0
    }

    /// Page proportions: wider than tall, taller than wide, or square.
    pub fn orientation(&self) -> Orientation {
        if self.width > self.height {
            Orientation::Landscape
        } else if self.height > self.width {
            Orientation::Portrait
        } else {
            Orientation::Square
        }
    }

    /// The format as `(width, height, dpi)` rounded for a JSON answer.
    pub fn describe(&self) -> serde_json::Value {
        serde_json::json!({
            "width": self.width,
            "height": self.height,
            "dpi": (self.dpi * 10.0).round() / 10.0,
            "orientation": self.orientation().name(),
        })
    }
}

/// Which way a canvas runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Orientation {
    Landscape,
    Portrait,
    Square,
}

impl Orientation {
    pub fn name(self) -> &'static str {
        match self {
            Self::Landscape => "landscape",
            Self::Portrait => "portrait",
            Self::Square => "square",
        }
    }
}

/// A named canvas size, as offered to a brief (and to the UI's preset menu).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Preset {
    pub id: &'static str,
    pub label: &'static str,
    pub format: Format,
}

const fn make(id: &'static str, label: &'static str, width: u32, height: u32, dpi: f32) -> Preset {
    Preset { id, label, format: Format { width, height, dpi } }
}

/// The poster presets. Print sizes are at 300 dpi, screen sizes at 72.
pub const PRESETS: &[Preset] = &[
    make("a4", "A4 portrait · 210×297 mm", 2480, 3508, 300.0),
    make("a4-landscape", "A4 landscape · 297×210 mm", 3508, 2480, 300.0),
    make("a3", "A3 portrait · 297×420 mm", 3508, 4961, 300.0),
    make("letter", "US Letter · 8.5×11 in", 2550, 3300, 300.0),
    make("square", "Square · 1080×1080", 1080, 1080, 72.0),
    make("story", "Story · 1080×1920", 1080, 1920, 72.0),
    make("wide", "Wide · 1920×1080", 1920, 1080, 72.0),
];

/// The default canvas when a brief names none: A4 portrait, the most poster-like of the set.
pub const DEFAULT_PRESET: &str = "a4";

/// Look a preset up by id (case-insensitive, `_` read as `-`).
pub fn preset(id: &str) -> Option<Preset> {
    let want = id.trim().to_ascii_lowercase().replace('_', "-");
    PRESETS.iter().copied().find(|p| p.id == want)
}

/// Every preset id, for the brief schema this crate documents.
pub fn preset_ids() -> Vec<&'static str> {
    PRESETS.iter().map(|p| p.id).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_are_valid_formats() {
        for p in PRESETS {
            assert!(Format::new(p.format.width, p.format.height, p.format.dpi).is_ok(), "{}", p.id);
            assert!(!p.id.is_empty() && !p.label.is_empty(), "{}", p.id);
        }
    }

    #[test]
    fn preset_lookup_is_lenient_about_case_and_underscores() {
        assert!(preset("A4").is_some());
        assert!(preset(" a4_LANDSCAPE ").is_some_and(|p| p.format.width > p.format.height));
        assert!(preset("poster").is_none());
        assert!(preset("").is_none());
    }

    #[test]
    fn points_and_pixels_round_trip_at_each_resolution() {
        let print = Format::new(2480, 3508, 300.0).expect("valid");
        assert!((print.pt(300.0) - 72.0).abs() < 0.01);
        assert!((print.px(print.pt(250.0)) - 250.0).abs() < 0.01);
        let screen = Format::new(1080, 1080, 72.0).expect("valid");
        assert!((screen.pt(48.0) - 48.0).abs() < 0.01);
    }

    #[test]
    fn a_format_outside_bounds_is_rejected() {
        assert!(Format::new(0, 1080, 72.0).is_err());
        assert!(Format::new(MAX_SIDE + 1, 1080, 72.0).is_err());
        assert!(Format::new(1080, 1080, 0.0).is_err());
        assert!(Format::new(1080, 1080, f32::NAN).is_err());
        assert!(Format::new(1080, 1080, f32::INFINITY).is_err());
        assert!(Format::new(MIN_SIDE, MIN_SIDE, MIN_DPI).is_ok());
    }

    #[test]
    fn orientation_reads_the_sides() {
        assert_eq!(Format { width: 1920, height: 1080, dpi: 72.0 }.orientation(), Orientation::Landscape);
        assert_eq!(Format { width: 1080, height: 1920, dpi: 72.0 }.orientation(), Orientation::Portrait);
        assert_eq!(Format { width: 1080, height: 1080, dpi: 72.0 }.orientation(), Orientation::Square);
    }
}
