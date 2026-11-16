//! The faces text is drawn in: the bundled faces, faces a host supplies for scripts they lack, and the generation that moves when one is added.

use super::*;

pub(super) static LATIN_BYTES: &[u8] =
    include_bytes!("../../../assets/fonts/DejaVuSans-subset.ttf");

pub(super) static ARABIC_REGULAR_BYTES: &[u8] =
    include_bytes!("../../../assets/fonts/NotoSansArabic-Regular-subset.ttf");

pub(super) static ARABIC_BOLD_BYTES: &[u8] =
    include_bytes!("../../../assets/fonts/NotoSansArabic-Bold-subset.ttf");

pub(super) static MONO_BYTES: &[u8] =
    include_bytes!("../../../assets/fonts/JetBrainsMono-Regular.ttf");

pub(super) static MONO_BOLD_BYTES: &[u8] =
    include_bytes!("../../../assets/fonts/JetBrainsMono-Bold.ttf");

pub(super) static SERIF_REGULAR_BYTES: &[u8] =
    include_bytes!("../../../assets/fonts/Amiri-Regular.ttf");

pub(super) static SERIF_BOLD_BYTES: &[u8] = include_bytes!("../../../assets/fonts/Amiri-Bold.ttf");

pub(super) static DISPLAY_REGULAR_BYTES: &[u8] =
    include_bytes!("../../../assets/fonts/ReemKufi-Regular.ttf");

pub(super) static DISPLAY_BOLD_BYTES: &[u8] =
    include_bytes!("../../../assets/fonts/ReemKufi-Bold.ttf");

pub(super) static BODY_REGULAR_BYTES: &[u8] =
    include_bytes!("../../../assets/fonts/Cairo-Regular.ttf");

pub(super) static BODY_BOLD_BYTES: &[u8] = include_bytes!("../../../assets/fonts/Cairo-Bold.ttf");

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub enum FontId {
    Latin,
    ArabicRegular,
    ArabicBold,
    /// JetBrains Mono Regular: text a theme draws in its `mono` face.
    Mono,
    MonoBold,
    /// Amiri Regular: the generic family `serif` (Latin and Arabic Naskh).
    Serif,
    /// Amiri Bold.
    SerifBold,
    /// Reem Kufi Regular: the family `Display` (Latin and Arabic).
    Display,
    DisplayBold,
    /// Cairo Regular: the family `Body` (Latin and Arabic).
    Body,
    BodyBold,
    /// A face the host supplied for a script no bundled face covers (`add_font_face`).
    Host(u16),
}

impl FontId {
    /// Only the bundled Arabic has a real bold; every other face is emboldened by stroking.
    pub fn faux_bold(self) -> bool {
        !matches!(
            self,
            FontId::ArabicRegular
                | FontId::ArabicBold
                | FontId::Mono
                | FontId::MonoBold
                | FontId::Serif
                | FontId::SerifBold
                | FontId::Display
                | FontId::DisplayBold
                | FontId::Body
                | FontId::BodyBold
        )
    }
}

pub(super) fn font_for(id: FontId) -> Option<&'static rustybuzz::Face<'static>> {
    static FACES: OnceLock<[Option<rustybuzz::Face<'static>>; 11]> = OnceLock::new();
    let faces = FACES.get_or_init(|| {
        [
            rustybuzz::Face::from_slice(LATIN_BYTES, 0),
            rustybuzz::Face::from_slice(ARABIC_REGULAR_BYTES, 0),
            rustybuzz::Face::from_slice(ARABIC_BOLD_BYTES, 0),
            rustybuzz::Face::from_slice(MONO_BYTES, 0),
            rustybuzz::Face::from_slice(SERIF_REGULAR_BYTES, 0),
            rustybuzz::Face::from_slice(SERIF_BOLD_BYTES, 0),
            rustybuzz::Face::from_slice(DISPLAY_REGULAR_BYTES, 0),
            rustybuzz::Face::from_slice(DISPLAY_BOLD_BYTES, 0),
            rustybuzz::Face::from_slice(BODY_REGULAR_BYTES, 0),
            rustybuzz::Face::from_slice(BODY_BOLD_BYTES, 0),
            rustybuzz::Face::from_slice(MONO_BOLD_BYTES, 0),
        ]
    });
    match id {
        FontId::Latin => faces[0].as_ref(),
        FontId::ArabicRegular => faces[1].as_ref(),
        FontId::ArabicBold => faces[2].as_ref(),
        FontId::Mono => faces[3].as_ref(),
        FontId::MonoBold => faces[10].as_ref(),
        FontId::Serif => faces[4].as_ref(),
        FontId::SerifBold => faces[5].as_ref(),
        FontId::Display => faces[6].as_ref(),
        FontId::DisplayBold => faces[7].as_ref(),
        FontId::Body => faces[8].as_ref(),
        FontId::BodyBold => faces[9].as_ref(),
        FontId::Host(i) => host_fonts().read().ok()?.get(i as usize).map(|(_, f)| *f),
    }
}

/// Code is drawn as typed: the mono faces' contextual ligatures (`->` as one arrow) stay off.
pub(super) fn features_for(id: FontId) -> &'static [rustybuzz::Feature] {
    static MONO: OnceLock<[rustybuzz::Feature; 2]> = OnceLock::new();
    match id {
        FontId::Mono | FontId::MonoBold => MONO.get_or_init(|| {
            [
                rustybuzz::Feature::new(rustybuzz::ttf_parser::Tag::from_bytes(b"calt"), 0, ..),
                rustybuzz::Feature::new(rustybuzz::ttf_parser::Tag::from_bytes(b"liga"), 0, ..),
            ]
        }),
        _ => &[],
    }
}

pub(super) type HostFaces = RwLock<Vec<(Script, &'static rustybuzz::Face<'static>)>>;

pub(super) fn host_fonts() -> &'static HostFaces {
    static HOST: OnceLock<HostFaces> = OnceLock::new();
    HOST.get_or_init(|| RwLock::new(Vec::new()))
}

/// Scripts text needed that no face covers: `pending` until the host drains them, `samples`
/// (one char per script ever reported) so each is asked for once and a supplied face can be
/// checked against it.
#[derive(Default)]
pub(super) struct MissingScripts {
    pub(super) pending: Vec<MissingScript>,
    pub(super) samples: std::collections::HashMap<Script, char>,
}

/// One script painted text needed, with a char of it the host can ask its font system for.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct MissingScript {
    /// ISO 15924 short name (`Hebr`).
    pub script: String,
    pub sample: String,
}

pub(super) fn missing() -> &'static Mutex<MissingScripts> {
    static MISSING: OnceLock<Mutex<MissingScripts>> = OnceLock::new();
    MISSING.get_or_init(|| Mutex::new(MissingScripts::default()))
}

pub(super) static FONT_GENERATION: AtomicU64 = AtomicU64::new(0);

/// Tests that add a face (moving [`font_generation`]) and tests that depend on it holding still
/// take turns.
#[cfg(any(test, feature = "test-support"))]
pub static FONT_GENERATION_TEST_LOCK: Mutex<()> = Mutex::new(());

/// Register the host platform's face for `script` (ISO 15924 short name, `Hebr`), face `index`
/// of a collection (0 for a single font). Text in that
/// script shapes with it from the next paint; [`font_generation`] moves so hosts repaint. A face
/// with no glyph for the char the script was reported with is refused, so the host can try its
/// next candidate.
pub fn add_font_face(script: &str, bytes: Vec<u8>, index: u32) -> Result<(), String> {
    let script =
        Script::from_short_name(script).ok_or_else(|| format!("unknown script `{script}`"))?;
    let bytes: &'static [u8] = Box::leak(bytes.into_boxed_slice());
    let face = rustybuzz::Face::from_slice(bytes, index).ok_or("the bytes are not a font face")?;
    let sample = missing()
        .lock()
        .ok()
        .and_then(|m| m.samples.get(&script).copied());
    if let Some(c) = sample.filter(|c| face.glyph_index(*c).is_none()) {
        return Err(format!(
            "the face has no glyph for `{c}` ({})",
            script.short_name()
        ));
    }
    let face: &'static rustybuzz::Face<'static> = Box::leak(Box::new(face));
    let mut hosts = host_fonts().write().map_err(|_| "font registry poisoned")?;
    if hosts.iter().all(|(s, _)| *s != script) {
        hosts.push((script, face));
    }
    FONT_GENERATION.fetch_add(1, Ordering::SeqCst);
    Ok(())
}

/// Scripts that text needed and no bundled or host face covers, each returned once. Such text
/// draws the bundled face's `.notdef` until the host supplies a face.
pub fn take_missing_scripts() -> Vec<MissingScript> {
    missing()
        .lock()
        .map(|mut m| std::mem::take(&mut m.pending))
        .unwrap_or_default()
}

/// Moves whenever a host face is added, so a host knows painted text may change.
pub fn font_generation() -> u64 {
    FONT_GENERATION.load(Ordering::SeqCst)
}

pub(super) fn note_missing(script: Script, sample: char) {
    if let Ok(mut m) = missing().lock() {
        if let std::collections::hash_map::Entry::Vacant(slot) = m.samples.entry(script) {
            slot.insert(sample);
            m.pending.push(MissingScript {
                script: script.short_name().to_string(),
                sample: sample.to_string(),
            });
        }
    }
}

pub(super) fn face() -> Option<&'static rustybuzz::Face<'static>> {
    font_for(FontId::Latin)
}
