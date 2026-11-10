//! The shared text model: Taffy's Ahem test model, 10px per character, `U+200B` the only break
//! opportunity. Every adapter routes its engine's measure callback through [`measure_text`], so
//! all engines see identical leaf sizes and the equal-output gate compares layout, not text.
//!
//! This is a port of `test_measure_function` in Taffy v0.15.1's `tests/common`, as used by
//! hooke's Chrome fixture harness (`tests/support/mod.rs`, `measure_ahem`): the inline size is the
//! known size, else the min-content, max-content or `min(definite available, max-content)`
//! size, and never below min-content; the block size is 10 times the number of lines from
//! greedily packing the segments into `floor(inline / 10)` characters.

/// Available space on one axis for the text model.
#[derive(Copy, Clone, PartialEq, Debug)]
pub enum TextAvail {
    Definite(f32),
    MinContent,
    MaxContent,
}

/// Measures `text` given known sizes and available space, returning `[width, height]` of the
/// content box.
pub fn measure_text(text: &str, known: [Option<f32>; 2], available: [TextAvail; 2]) -> [f32; 2] {
    const ZWS: char = '\u{200B}';
    const H: f32 = 10.0;
    if let [Some(w), Some(h)] = known {
        return [w, h];
    }
    let lines: Vec<&str> = text.split(ZWS).collect();
    let min_line = lines.iter().map(|l| l.len()).max().unwrap_or(0) as f32 * H;
    let max_line = lines.iter().map(|l| l.len()).sum::<usize>() as f32 * H;
    let width = known[0]
        .unwrap_or_else(|| match available[0] {
            TextAvail::MinContent => min_line,
            TextAvail::MaxContent => max_line,
            TextAvail::Definite(a) => a.min(max_line),
        })
        .max(min_line);
    let height = known[1].unwrap_or_else(|| {
        let per_line = (width / H).floor() as usize;
        let mut count = 1;
        let mut current = 0;
        for l in &lines {
            if current + l.len() > per_line {
                if current > 0 {
                    count += 1;
                }
                current = l.len();
            } else {
                current += l.len();
            }
        }
        count as f32 * H
    });
    [width, height]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_at_zero_width_spaces_only() {
        let t = "HHHH\u{200B}HHHH";
        assert_eq!(measure_text(t, [None, None], [TextAvail::MaxContent, TextAvail::MaxContent]), [80.0, 10.0]);
        assert_eq!(measure_text(t, [None, None], [TextAvail::MinContent, TextAvail::MaxContent]), [40.0, 20.0]);
        assert_eq!(measure_text(t, [None, None], [TextAvail::Definite(50.0), TextAvail::MaxContent]), [50.0, 20.0]);
        assert_eq!(measure_text(t, [Some(45.0), None], [TextAvail::MaxContent, TextAvail::MaxContent]), [45.0, 20.0]);
        assert_eq!(measure_text(t, [None, None], [TextAvail::Definite(10.0), TextAvail::MaxContent]), [40.0, 20.0]);
    }
}
