//! Font families a theme names with a file: each family name is interned once to a [`TextFamily::Face`] id the
//! painter resolves to that file, falling back to the family's generic face until the file has loaded.

use crate::packet::TextFamily;
use std::sync::{OnceLock, RwLock};

/// One theme family: its CSS name (lower case), the font file URL and the generic face drawn until it loads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FaceFile {
    pub name: String,
    pub url: String,
    pub fallback: TextFamily,
}

fn registry() -> &'static RwLock<Vec<FaceFile>> {
    static FACES: OnceLock<RwLock<Vec<FaceFile>>> = OnceLock::new();
    FACES.get_or_init(|| RwLock::new(Vec::new()))
}

/// Register `name` as drawn from `url`, returning its family. A name already registered keeps its id and takes
/// the new file and fallback, so a theme switch that reuses a family name points it at the new theme's file.
pub fn register_face(name: &str, url: &str, fallback: TextFamily) -> Option<TextFamily> {
    let name = name.trim().to_ascii_lowercase();
    if name.is_empty() || url.is_empty() {
        return None;
    }
    let mut faces = registry().write().ok()?;
    let entry = FaceFile {
        name: name.clone(),
        url: url.to_string(),
        fallback: generic(fallback),
    };
    let id = match faces.iter().position(|f| f.name == name) {
        Some(i) => {
            faces[i] = entry;
            i
        }
        None => {
            faces.push(entry);
            faces.len() - 1
        }
    };
    u16::try_from(id).ok().map(TextFamily::Face)
}

/// The family registered under the CSS family `name`, if any.
pub fn face_named(name: &str) -> Option<TextFamily> {
    let name = name.to_ascii_lowercase();
    let faces = registry().read().ok()?;
    let id = faces.iter().position(|f| f.name == name)?;
    u16::try_from(id).ok().map(TextFamily::Face)
}

/// The file behind a [`TextFamily::Face`] id.
pub fn face_file(id: u16) -> Option<FaceFile> {
    registry().read().ok()?.get(usize::from(id)).cloned()
}

/// The generic family a face falls back to; a face never falls back to another face.
fn generic(family: TextFamily) -> TextFamily {
    match family {
        TextFamily::Face(id) => face_file(id).map_or(TextFamily::Sans, |f| f.fallback),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_registered_name_is_one_face_id_and_takes_the_latest_file() {
        let first = register_face(
            "Faces Test Limelight",
            "https://a/one.ttf",
            TextFamily::Serif,
        )
        .unwrap();
        let again = register_face(
            "faces test limelight",
            "https://a/two.ttf",
            TextFamily::Sans,
        )
        .unwrap();
        assert_eq!(first, again);
        assert_eq!(face_named("FACES TEST LIMELIGHT"), Some(first));
        let TextFamily::Face(id) = first else {
            panic!("not a face: {first:?}")
        };
        let file = face_file(id).unwrap();
        assert_eq!(file.url, "https://a/two.ttf");
        assert_eq!(file.fallback, TextFamily::Sans);
    }

    #[test]
    fn an_empty_name_or_url_registers_nothing() {
        assert_eq!(register_face("", "https://a/x.ttf", TextFamily::Sans), None);
        assert_eq!(
            register_face("faces test empty", "", TextFamily::Sans),
            None
        );
        assert_eq!(face_named("faces test empty"), None);
    }
}
