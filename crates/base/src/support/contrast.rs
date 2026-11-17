//! WCAG 2.2 contrast of sRGB colours (components 0..1).

/// Relative luminance (0 black, 1 white); alpha is ignored.
pub fn relative_luminance(color: [f32; 4]) -> f32 {
    let linear = |c: f32| {
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(color[0]) + 0.7152 * linear(color[1]) + 0.0722 * linear(color[2])
}

/// The contrast ratio (1 to 21) of `fg` against an opaque `bg`; a translucent `fg` is composited over `bg` first.
pub fn wcag_ratio(fg: [f32; 4], bg: [f32; 4]) -> f32 {
    let a = fg[3].clamp(0.0, 1.0);
    let shown = [
        fg[0] * a + bg[0] * (1.0 - a),
        fg[1] * a + bg[1] * (1.0 - a),
        fg[2] * a + bg[2] * (1.0 - a),
        1.0,
    ];
    let (l1, l2) = (relative_luminance(shown), relative_luminance(bg));
    let (light, dark) = if l1 >= l2 { (l1, l2) } else { (l2, l1) };
    (light + 0.05) / (dark + 0.05)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLACK: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
    const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];

    #[test]
    fn black_on_white_is_21_and_the_same_colour_is_1() {
        assert!((wcag_ratio(BLACK, WHITE) - 21.0).abs() < 1e-4);
        assert!((wcag_ratio(WHITE, BLACK) - 21.0).abs() < 1e-4);
        assert!((wcag_ratio(WHITE, WHITE) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn grey_999_on_white_is_2_85_and_767676_passes_4_5() {
        let grey = |v: f32| [v / 255.0, v / 255.0, v / 255.0, 1.0];
        assert!((wcag_ratio(grey(153.0), WHITE) - 2.848).abs() < 0.01);
        assert!(wcag_ratio(grey(118.0), WHITE) >= 4.5);
    }

    #[test]
    fn a_translucent_foreground_is_composited_over_the_background() {
        let half = [0.0, 0.0, 0.0, 0.5];
        let composited = wcag_ratio(half, WHITE);
        assert!(composited < 21.0 && composited > 1.0);
        assert!((composited - wcag_ratio([0.5, 0.5, 0.5, 1.0], WHITE)).abs() < 1e-4);
    }
}
