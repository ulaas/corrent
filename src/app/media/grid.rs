//! Card grids that fill the width and wrap downwards: no row ever needs a horizontal scroll bar.

/// Columns and card width for a grid `avail` points wide.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GridSpec {
    pub cols: usize,
    pub card_w: f32,
    pub gap: f32,
}

/// As many columns as fit cards of at least `min` points; the cards then grow to fill the row,
/// up to `max` (a very wide window keeps a few more columns instead of huge cards).
pub fn spec(avail: f32, min: f32, max: f32, gap: f32) -> GridSpec {
    let avail = avail.max(1.0);
    let cols = (((avail + gap) / (min + gap)).floor() as usize).max(1);
    let card_w = ((avail - (cols as f32 - 1.0) * gap) / cols as f32).min(max).max(1.0);
    GridSpec { cols, card_w, gap }
}

/// Card size limits (points) for portrait cards (posters) and landscape ones (episodes, videos).
pub const PORTRAIT: (f32, f32) = (140.0, 200.0);
pub const LANDSCAPE: (f32, f32) = (220.0, 330.0);
/// Narrow windows (and the picture-in-picture layout) get smaller cards.
pub const NARROW: f32 = 560.0;

pub fn limits(landscape: bool, avail: f32) -> (f32, f32) {
    let (min, max) = if landscape { LANDSCAPE } else { PORTRAIT };
    if avail < NARROW { (min * 0.75, max) } else { (min, max) }
}

/// "AB" for "A Bug's Life", for a card without a picture.
pub fn initials(title: &str) -> String {
    title
        .split(|c: char| c.is_whitespace() || c == '.' || c == '_' || c == '-')
        .filter_map(|w| w.chars().find(|c| c.is_alphanumeric()))
        .take(2)
        .flat_map(char::to_uppercase)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cards_fill_the_width_and_never_overflow() {
        for avail in [120.0, 320.0, 600.0, 1280.0, 2560.0] {
            for (min, max) in [PORTRAIT, LANDSCAPE] {
                let s = spec(avail, min, max, 16.0);
                assert!(s.cols >= 1);
                let used = s.cols as f32 * s.card_w + (s.cols as f32 - 1.0) * s.gap;
                assert!(used <= avail + 0.01, "{avail} {min}: {s:?}");
                assert!(s.card_w <= max);
                if s.cols > 1 {
                    assert!(s.card_w >= min, "{avail} {min}: {s:?}");
                }
            }
        }
        // A wide window gets more columns, not giant cards.
        let s = spec(2560.0, 140.0, 200.0, 16.0);
        assert!(s.cols >= 13);
        // Narrower than one card: one column, as wide as there is room.
        assert_eq!(spec(100.0, 140.0, 200.0, 16.0), GridSpec { cols: 1, card_w: 100.0, gap: 16.0 });
    }

    #[test]
    fn initials_of_titles() {
        assert_eq!(initials("A Bug's Life"), "AB");
        assert_eq!(initials("Sintel.2010.1080p"), "S2");
        assert_eq!(initials("élan vital"), "ÉV");
        assert_eq!(initials(""), "");
        assert_eq!(initials("  "), "");
    }
}
