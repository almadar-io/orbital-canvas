//! CSS colours as the web hosts paint them.

/// Deterministic CSS color subset: `#rgb`, `#rrggbb`, `#rrggbbaa`,
/// `rgb(r,g,b)`, `rgba(r,g,b,a)` (a in 0..1). Anything else → None (the caller
/// skips the fill — no heuristics).
pub fn parse_css_color(s: &str) -> Option<[f32; 4]> {
    let s = s.trim();
    if let Some(hex) = s.strip_prefix('#') {
        let v = u32::from_str_radix(hex, 16).ok()?;
        return match hex.len() {
            3 => Some([
                ((v >> 8) & 0xf) as f32 * 17.0 / 255.0,
                ((v >> 4) & 0xf) as f32 * 17.0 / 255.0,
                (v & 0xf) as f32 * 17.0 / 255.0,
                1.0,
            ]),
            6 => Some([
                ((v >> 16) & 0xff) as f32 / 255.0,
                ((v >> 8) & 0xff) as f32 / 255.0,
                (v & 0xff) as f32 / 255.0,
                1.0,
            ]),
            8 => Some([
                ((v >> 24) & 0xff) as f32 / 255.0,
                ((v >> 16) & 0xff) as f32 / 255.0,
                ((v >> 8) & 0xff) as f32 / 255.0,
                (v & 0xff) as f32 / 255.0,
            ]),
            _ => None,
        };
    }
    let rgb = s
        .strip_prefix("rgba(")
        .or_else(|| s.strip_prefix("rgb("))
        .and_then(|r| r.strip_suffix(')'))
        .and_then(|inner| {
            inner
                .split(',')
                .map(|p| p.trim().parse::<f32>())
                .collect::<Result<Vec<f32>, _>>()
                .ok()
        });
    match rgb.as_deref() {
        Some([r, g, b]) => Some([r / 255.0, g / 255.0, b / 255.0, 1.0]),
        Some([r, g, b, a]) => Some([r / 255.0, g / 255.0, b / 255.0, *a]),
        // every other CSS color the browser paints: named colors, `transparent`, hsl(), hwb(), space-separated rgb()
        _ => csscolorparser::parse(s).ok().map(|c| [c.r, c.g, c.b, c.a]),
    }
}

#[cfg(test)]
mod tests {
    use super::parse_css_color;

    #[test]
    fn parses_css_colors() {
        assert_eq!(parse_css_color("#1a1a2e").unwrap()[0], 26.0 / 255.0);
        assert_eq!(parse_css_color("#fff").unwrap(), [1.0, 1.0, 1.0, 1.0]);
        assert_eq!(
            parse_css_color("rgba(0,150,255,0.6)").unwrap(),
            [0.0, 150.0 / 255.0, 1.0, 0.6]
        );
        assert_eq!(
            parse_css_color("rgb(255, 0, 0)").unwrap(),
            [1.0, 0.0, 0.0, 1.0]
        );
        assert_eq!(parse_css_color("#ff000080").unwrap()[3], 128.0 / 255.0);
        let purple = parse_css_color("rebeccapurple").unwrap();
        assert_eq!(
            purple,
            [
                0x66 as f32 / 255.0,
                0x33 as f32 / 255.0,
                0x99 as f32 / 255.0,
                1.0
            ],
            "named colors, as browsers paint them"
        );
        assert_eq!(parse_css_color("transparent").unwrap()[3], 0.0);
        assert!(
            parse_css_color("var(--tone)").is_none(),
            "control: a theme variable is the web host's to resolve"
        );
        assert!(parse_css_color("not a color").is_none());
    }
}
