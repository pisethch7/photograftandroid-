//! `#rrggbb` colours, their contrast, and the few transformations the templates need.
//!
//! Templates never choose a colour blindly: text is drawn in the palette's ink or in whichever of
//! ink/paper contrasts with the colour behind it, so a dark palette and a light one both stay
//! readable without the AI having to reason about contrast.

use crate::error::{Error, Result};

/// A colour as normalised sRGB plus alpha, the shape `type.create` and `shape.create` take.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rgb {
    pub r: f32,
    pub g: f32,
    pub b: f32,
}

impl Rgb {
    pub const WHITE: Self = Self { r: 1.0, g: 1.0, b: 1.0 };
    pub const BLACK: Self = Self { r: 0.0, g: 0.0, b: 0.0 };

    /// Parse `#rrggbb` (the `#` is required, as the command parameters use it).
    pub fn parse(field: &str, value: &str) -> Result<Self> {
        let hex = value.trim();
        let digits = hex.strip_prefix('#').filter(|d| d.len() == 6);
        let Some(digits) = digits else {
            return Err(Error::Color { field: field.to_string(), value: hex.to_string() });
        };
        let channel = |at: usize| -> Option<u8> { u8::from_str_radix(digits.get(at..at + 2)?, 16).ok() };
        let (Some(r), Some(g), Some(b)) = (channel(0), channel(2), channel(4)) else {
            return Err(Error::Color { field: field.to_string(), value: hex.to_string() });
        };
        Ok(Self::from_u8(r, g, b))
    }

    pub fn from_u8(r: u8, g: u8, b: u8) -> Self {
        Self { r: r as f32 / 255.0, g: g as f32 / 255.0, b: b as f32 / 255.0 }
    }

    /// `#rrggbb`, lower case, as the commands take it.
    pub fn hex(self) -> String {
        let c = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        format!("#{:02x}{:02x}{:02x}", c(self.r), c(self.g), c(self.b))
    }

    /// WCAG relative luminance: how bright the colour is, weighted for the eye.
    pub fn luminance(self) -> f32 {
        let lin = |v: f32| if v <= 0.040_45 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) };
        0.2126 * lin(self.r) + 0.7152 * lin(self.g) + 0.0722 * lin(self.b)
    }

    /// Whether text on this colour should be light.
    pub fn is_dark(self) -> bool {
        self.luminance() < 0.4
    }

    /// Move the colour towards black by `amount` (0 = unchanged, 1 = black).
    pub fn shade(self, amount: f32) -> Self {
        let a = amount.clamp(0.0, 1.0);
        Self { r: self.r * (1.0 - a), g: self.g * (1.0 - a), b: self.b * (1.0 - a) }
    }

    /// Move the colour towards white by `amount` (0 = unchanged, 1 = white).
    pub fn tint(self, amount: f32) -> Self {
        let a = amount.clamp(0.0, 1.0);
        Self { r: self.r + (1.0 - self.r) * a, g: self.g + (1.0 - self.g) * a, b: self.b + (1.0 - self.b) * a }
    }

    /// The better of two inks on this background: whichever is further from it in luminance.
    pub fn readable_on(self, light: Self, dark: Self) -> Self {
        if (self.luminance() - light.luminance()).abs() >= (self.luminance() - dark.luminance()).abs() { light } else { dark }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_prints_hex() {
        let c = Rgb::parse("accent", "#E2402C").expect("valid");
        assert_eq!(c.hex(), "#e2402c");
        assert_eq!(Rgb::parse("ink", "  #ffffff ").expect("valid"), Rgb::WHITE);
    }

    #[test]
    fn rejects_anything_but_six_hex_digits() {
        for bad in ["", "#", "fff", "#fff", "#gggggg", "e2402c", "#e2402ca", "#１２３４５６"] {
            assert!(Rgb::parse("accent", bad).is_err(), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn luminance_orders_black_white_and_midtones() {
        assert!(Rgb::BLACK.luminance() < Rgb::parse("c", "#777777").expect("valid").luminance());
        assert!(Rgb::parse("c", "#777777").expect("valid").luminance() < Rgb::WHITE.luminance());
        assert!(Rgb::BLACK.is_dark());
        assert!(!Rgb::WHITE.is_dark());
    }

    #[test]
    fn shade_tint_and_readable_on_stay_in_range() {
        let c = Rgb::parse("c", "#3366cc").expect("valid");
        assert_eq!(c.shade(0.0), c);
        assert_eq!(c.shade(1.0), Rgb::BLACK);
        assert_eq!(c.tint(1.0), Rgb::WHITE);
        assert!(c.shade(2.0).hex() == "#000000");
        assert_eq!(Rgb::BLACK.readable_on(Rgb::WHITE, Rgb::BLACK), Rgb::WHITE);
        assert_eq!(Rgb::WHITE.readable_on(Rgb::WHITE, Rgb::BLACK), Rgb::BLACK);
    }
}
