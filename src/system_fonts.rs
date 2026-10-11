//! The desktop twin of the mobile hosts' platform-font source: a script the bundled faces lack
//! ([`crate::vector::take_missing_scripts`]) is supplied from the installed system fonts.

use std::sync::OnceLock;

/// The first installed face, in (family, PostScript name) order, that draws every non-space
/// character of `sample`: its bytes and collection index.
pub fn face_for(db: &fontdb::Database, sample: &str) -> Option<(Vec<u8>, u32)> {
    let mut faces: Vec<&fontdb::FaceInfo> = db.faces().collect();
    faces.sort_by(|a, b| {
        let family = |f: &fontdb::FaceInfo| {
            f.families
                .first()
                .map(|(name, _)| name.clone())
                .unwrap_or_default()
        };
        (family(a), &a.post_script_name).cmp(&(family(b), &b.post_script_name))
    });
    faces.into_iter().find_map(|info| {
        db.with_face_data(info.id, |data, index| {
            let face = ttf_parser::Face::parse(data, index).ok()?;
            sample
                .chars()
                .filter(|c| !c.is_whitespace())
                .all(|c| face.glyph_index(c).is_some())
                .then(|| (data.to_vec(), index))
        })
        .flatten()
    })
}

fn system() -> &'static fontdb::Database {
    static DB: OnceLock<fontdb::Database> = OnceLock::new();
    DB.get_or_init(|| {
        let mut db = fontdb::Database::new();
        db.load_system_fonts();
        db
    })
}

/// Supply every script the renderer has asked for since the last call from the system fonts;
/// returns how many were supplied. A script no installed face covers is logged and still draws
/// `.notdef`.
pub fn supply_missing_from_system() -> usize {
    let mut supplied = 0;
    for missing in crate::vector::take_missing_scripts() {
        match face_for(system(), &missing.sample) {
            Some((bytes, index)) => {
                match crate::vector::add_font_face(&missing.script, bytes, index) {
                    Ok(()) => supplied += 1,
                    Err(error) => {
                        tracing::warn!(script = %missing.script, %error, "system face refused")
                    }
                }
            }
            None => {
                tracing::warn!(script = %missing.script, sample = %missing.sample, "no installed face covers the script")
            }
        }
    }
    supplied
}

#[cfg(test)]
mod tests {
    use super::face_for;

    fn mono_only() -> fontdb::Database {
        let mut db = fontdb::Database::new();
        db.load_font_data(include_bytes!("../assets/fonts/DejaVuSansMono-subset.ttf").to_vec());
        db
    }

    #[test]
    fn a_face_that_draws_the_sample_is_supplied() {
        let (bytes, index) =
            face_for(&mono_only(), "abc 123").expect("the mono subset covers Latin");
        assert_eq!(index, 0);
        assert!(ttf_parser::Face::parse(&bytes, index).is_ok());
    }

    #[test]
    fn control_no_face_for_an_uncovered_script() {
        assert!(face_for(&mono_only(), "שלום").is_none());
        assert!(
            face_for(&fontdb::Database::new(), "abc").is_none(),
            "an empty database supplies nothing"
        );
    }
}
