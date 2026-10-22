//! What is shown: text and colour per device.

use crate::hidpp::Battery;

pub type Rgb = [u8; 3];

pub const GREEN: Rgb = [0x4C, 0xD9, 0x64];
pub const YELLOW: Rgb = [0xFF, 0xC8, 0x00];
pub const RED: Rgb = [0xFF, 0x45, 0x3A];
pub const BLUE: Rgb = [0x3F, 0x9B, 0xFF];
pub const GREY: Rgb = [0xA0, 0xA0, 0xA0];

/// Green above 50 %, yellow 20..=50 %, red below 20 %, blue while charging,
/// grey when unknown / disconnected / asleep.
pub fn colour(b: Option<&Battery>) -> Rgb {
    match b {
        None => GREY,
        Some(b) if b.charging => BLUE,
        Some(b) if b.percent > 50 => GREEN,
        Some(b) if b.percent >= 20 => YELLOW,
        Some(_) => RED,
    }
}

/// "M 85%", "H --", "M ~50%" (coarse level only).
pub fn text(label: char, b: Option<&Battery>) -> String {
    match b {
        None => format!("{} --", label),
        Some(b) if b.approx => format!("{} ~{}%", label, b.percent),
        Some(b) => format!("{} {}%", label, b.percent),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub text: String,
    pub colour: Rgb,
}

pub fn line(label: char, b: Option<&Battery>) -> Line {
    Line {
        text: text(label, b),
        colour: colour(b),
    }
}

/// Lines to draw given the toggles.
pub fn lines(show_mouse: bool, mouse: Option<&Battery>, show_headset: bool, headset: Option<&Battery>) -> Vec<Line> {
    let mut v = Vec::new();
    if show_mouse {
        v.push(line('M', mouse));
    }
    if show_headset {
        v.push(line('H', headset));
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(p: u8, c: bool) -> Battery {
        Battery {
            percent: p,
            approx: false,
            charging: c,
        }
    }

    #[test]
    fn colour_thresholds() {
        assert_eq!(colour(Some(&b(100, false))), GREEN);
        assert_eq!(colour(Some(&b(51, false))), GREEN);
        assert_eq!(colour(Some(&b(50, false))), YELLOW);
        assert_eq!(colour(Some(&b(20, false))), YELLOW);
        assert_eq!(colour(Some(&b(19, false))), RED);
        assert_eq!(colour(Some(&b(0, false))), RED);
        assert_eq!(colour(Some(&b(10, true))), BLUE);
        assert_eq!(colour(Some(&b(90, true))), BLUE);
        assert_eq!(colour(None), GREY);
    }

    #[test]
    fn text_formatting() {
        assert_eq!(text('M', Some(&b(85, false))), "M 85%");
        assert_eq!(text('H', Some(&b(60, true))), "H 60%");
        assert_eq!(text('H', None), "H --");
        assert_eq!(text('M', Some(&b(100, false))), "M 100%");
        let a = Battery {
            percent: 50,
            approx: true,
            charging: false,
        };
        assert_eq!(text('M', Some(&a)), "M ~50%");
    }

    #[test]
    fn lines_respect_toggles() {
        assert_eq!(lines(true, None, true, None).len(), 2);
        let l = lines(false, None, true, Some(&b(5, false)));
        assert_eq!(
            l,
            vec![Line {
                text: "H 5%".into(),
                colour: RED
            }]
        );
        assert!(lines(false, None, false, None).is_empty());
    }
}
