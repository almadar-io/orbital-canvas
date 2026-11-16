use super::drawable_track;
use serde_json::json;

#[test]
fn drawable_tracks_interpolate_numbers_and_hold_colors() {
    let animation = json!({
        "durationMs": 1000, "loop": false,
        "keyframes": [
            {"at": 0, "width": 0, "fill": "#ff0000", "shadow": {"blur": 1}},
            {"at": 1, "width": 2, "fill": "#0000ff", "shadow": {"blur": 5}}
        ]
    });
    assert_eq!(drawable_track(&animation, 500.0, "width"), Some(json!(1.0)));
    assert_eq!(
        drawable_track(&animation, 500.0, "fill"),
        Some(json!("#ff0000"))
    );
    assert_eq!(
        drawable_track(&animation, 500.0, "shadow.blur"),
        Some(json!(3.0))
    );
    assert_eq!(drawable_track(&animation, 1500.0, "width"), Some(json!(2)));
}
