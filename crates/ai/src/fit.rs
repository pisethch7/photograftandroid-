//! Choosing a type size that fits a box, and knowing how tall the result will be.
//!
//! The type engine does the real wrapping; this only has to be close enough to pick a size that
//! will not overflow before the engine sees it, and close enough that the block below can follow
//! the words instead of an empty box. It estimates a greedy word wrap at an average glyph advance
//! for a sans face (narrow letters, capitals and wide letters each scaled from it), which lands
//! within a couple of percent of a real line on ordinary copy.
//!
//! A model cannot do this: a headline of 40 characters and one of 4 need very different sizes in
//! the same box, and the size has to change with the canvas. Doing it here keeps the result
//! predictable and testable.

/// What the fitting needs to know about a text box.
#[derive(Clone, Copy, Debug)]
pub struct Fit {
    /// The box width, in pixels.
    pub box_w: f32,
    /// The box height, in pixels.
    pub box_h: f32,
    /// Most lines the box may hold.
    pub max_lines: usize,
    /// Average glyph advance as a fraction of the type size (0.56 for a mixed-case sans face,
    /// more for a capitals-heavy line).
    pub advance: f32,
    /// Line height as a multiple of the size.
    pub leading: f32,
    /// Smallest size to use, whatever happens.
    pub min_px: f32,
    /// Largest size to use, whatever happens.
    pub max_px: f32,
}

/// How a block of text is expected to set at a size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Estimated {
    pub lines: f32,
    pub height: f32,
}

impl Fit {
    /// The fitted size in pixels: the largest in `min_px..=max_px` whose estimate fits the box.
    pub fn size(&self, text: &str) -> f32 {
        let min = self.min_px.max(1.0);
        let max = self.max_px.max(min);
        let fits = |size: f32| -> bool {
            let estimate = self.estimate(text, size);
            estimate.lines <= self.max_lines.max(1) as f32 && estimate.height <= self.box_h.max(1.0)
        };
        if !fits(min) {
            return min;
        }
        let (mut lo, mut hi) = (min, max);
        // 24 halvings take the range to a ten-thousandth of a pixel; more buys nothing.
        for _ in 0..24 {
            let mid = (lo + hi) * 0.5;
            if fits(mid) {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        lo
    }

    /// How many lines `text` takes at `size`, and how tall its line boxes are.
    ///
    /// Words are placed greedily, as the type engine's paragraph text does: a word starts a new
    /// line when it no longer fits, and a line break in the text always starts one.
    pub fn estimate(&self, text: &str, size: f32) -> Estimated {
        let size = size.max(1.0);
        let leading = self.leading.max(1.0);
        let box_w = self.box_w.max(1.0);
        let advance = self.advance.max(0.1);
        let mut lines = 0.0f32;
        for paragraph in text.split('\n') {
            lines += 1.0;
            // Width of the line being filled, and of the word about to be placed, in ems.
            let mut line = 0.0f32;
            for word in paragraph.split_whitespace() {
                let word_w: f32 = word.chars().map(glyph).sum();
                let space = if line > 0.0 { SPACE } else { 0.0 };
                if line > 0.0 && (line + space + word_w) * advance * size > box_w {
                    lines += 1.0;
                    line = word_w;
                } else {
                    line += space + word_w;
                }
            }
        }
        Estimated { lines: lines.max(1.0), height: lines.max(1.0) * size * leading }
    }
}

/// The space between words, in ems.
const SPACE: f32 = 0.48;
/// A narrow glyph (lower case `i`, punctuation) in ems.
const NARROW: f32 = 0.52;
/// A capital or a digit in ems.
const UPPER: f32 = 1.18;
/// A wide glyph in ems.
const WIDE: f32 = 1.5;

/// The estimated advance of one character, as a multiple of [`Fit::advance`].
fn glyph(c: char) -> f32 {
    match c {
        ' ' => SPACE,
        'i' | 'l' | 'j' | 'f' | 't' | 'r' | 'I' | '.' | ',' | '\'' | '"' | '!' | '|' | ':' | ';' | '(' | ')' | '[' | ']' | '-' => NARROW,
        'm' | 'w' | 'M' | 'W' | '@' | '%' => WIDE,
        '0'..='9' => UPPER,
        c if c.is_uppercase() => UPPER,
        _ => 1.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fit(box_w: f32, box_h: f32, max_lines: usize) -> Fit {
        Fit { box_w, box_h, max_lines, advance: 0.56, leading: 1.1, min_px: 10.0, max_px: 400.0 }
    }

    #[test]
    fn a_long_headline_is_set_smaller_than_a_short_one() {
        let f = fit(800.0, 300.0, 2);
        let short = f.size("Studio");
        let long = f.size("An evening of open studios, prints and new work by twelve artists");
        assert!(short > long, "short {short} should beat long {long}");
    }

    #[test]
    fn the_chosen_size_stays_inside_its_budget() {
        let f = fit(700.0, 260.0, 2);
        let text = "Autumn Print Fair";
        let size = f.size(text);
        let e = f.estimate(text, size);
        assert!(e.lines <= 2.0, "{e:?}");
        assert!(e.height <= 260.0, "{e:?}");
        assert!(size <= 400.0);
    }

    #[test]
    fn a_box_too_small_for_even_the_minimum_returns_the_minimum() {
        let f = Fit { box_w: 12.0, box_h: 8.0, max_lines: 1, advance: 0.6, leading: 1.2, min_px: 9.0, max_px: 100.0 };
        assert_eq!(f.size("A headline that cannot fit"), 9.0);
    }

    #[test]
    fn a_huge_box_does_not_exceed_the_maximum() {
        let f = Fit { box_w: 100_000.0, box_h: 100_000.0, max_lines: 1, advance: 0.5, leading: 1.0, min_px: 8.0, max_px: 300.0 };
        assert!((f.size("Short") - 300.0).abs() < 0.5);
    }

    #[test]
    fn sizes_are_ordered_by_box_width_and_are_deterministic() {
        let text = "Twelve artists, one evening";
        let narrow = fit(300.0, 400.0, 3).size(text);
        let wide = fit(900.0, 400.0, 3).size(text);
        assert!(wide > narrow, "narrow {narrow} wide {wide}");
        assert_eq!(fit(900.0, 400.0, 3).size(text), wide);
    }

    #[test]
    fn degenerate_inputs_do_not_panic() {
        let f = Fit { box_w: 0.0, box_h: 0.0, max_lines: 0, advance: 0.0, leading: 0.0, min_px: -5.0, max_px: f32::NAN };
        let size = f.size("");
        assert!(size.is_finite() && size >= 1.0, "{size}");
    }

    #[test]
    fn a_short_line_breaks_at_words_not_at_characters() {
        let f = fit(1000.0, 5000.0, 10);
        // "Autumn Print Fair" is about 7.9 em wide: one line at 100 px, two at 200.
        assert_eq!(f.estimate("Autumn Print Fair", 100.0).lines, 1.0);
        assert_eq!(f.estimate("Autumn Print Fair", 200.0).lines, 2.0);
    }

    #[test]
    fn the_estimate_matches_a_real_line_on_a_known_headline() {
        // Rendered by the type engine at 248 px in a 2133 px column, "Autumn Print Fair" set as
        // one 1995 px line. The estimate must agree, or every block below it drifts.
        let f = Fit { box_w: 2133.0, box_h: 1200.0, max_lines: 3, advance: 0.56, leading: 1.06, min_px: 1.0, max_px: 1000.0 };
        let e = f.estimate("Autumn Print Fair", 248.0);
        assert_eq!(e.lines, 1.0, "{e:?}");
        assert!((e.height - 263.0).abs() < 1.0, "{e:?}");
    }

    #[test]
    fn every_paragraph_gets_its_own_line() {
        let f = fit(1_000.0, 5_000.0, 20);
        assert_eq!(f.estimate("one", 10.0).lines, 1.0);
        assert_eq!(f.estimate("one\ntwo", 10.0).lines, 2.0);
        assert_eq!(f.estimate("one\ntwo\n", 10.0).lines, 3.0, "a trailing break still ends the block");
        assert_eq!(f.estimate("", 10.0).lines, 1.0);
    }

    #[test]
    fn a_word_wider_than_the_box_still_occupies_one_line() {
        let f = fit(20.0, 5_000.0, 20);
        let e = f.estimate("antidisestablishmentarianism", 10.0);
        assert_eq!(e.lines, 1.0, "{e:?}");
    }

    #[test]
    fn the_height_is_the_lines_times_the_leading() {
        let f = Fit { box_w: 100.0, box_h: 5_000.0, max_lines: 20, advance: 0.5, leading: 1.5, min_px: 1.0, max_px: 100.0 };
        let e = f.estimate("a\nb\nc", 40.0);
        assert_eq!(e.lines, 3.0);
        assert!((e.height - 3.0 * 40.0 * 1.5).abs() < 0.01, "{e:?}");
    }
}
