use crate::{Font, FontRef, FontVec, ScaleFont};

const EXO2_OTF: &[u8] = include_bytes!("../../../../dev/fonts/Exo2-Light.otf");
const EXO2_TTF: &[u8] = include_bytes!("../../../../dev/fonts/Exo2-Light.ttf");
const OPENS_SANS_ITALIC: &[u8] = include_bytes!("../../../../dev/fonts/OpenSans-Italic.ttf");
const DEJA_VU_MONO: &[u8] = include_bytes!("../../../../dev/fonts/DejaVuSansMono.ttf");
const CANTARELL_VF: &[u8] = include_bytes!("../../../../dev/fonts/Cantarell-VF.otf");

fn pair<F: Font>(font: &F, first: char, second: char) -> f32 {
    font.kern_unscaled(font.glyph_id(first), font.glyph_id(second))
}

/// Exo2-Light has no legacy `kern` table, only GPOS pair adjustments.
///
/// Reference values from harfbuzz shaping of the same pairs.
#[test]
fn gpos_kern_exo2_otf() {
    let font = FontRef::try_from_slice(EXO2_OTF).unwrap();
    assert_eq!(pair(&font, 'A', 'V'), -34.0);
    assert_eq!(pair(&font, 'T', 'o'), -113.0);
    assert_eq!(pair(&font, 'L', 'T'), -108.0);
    assert_eq!(pair(&font, 'o', 'x'), -11.0);
    assert_eq!(pair(&font, 'r', '.'), -55.0);
    assert_eq!(pair(&font, 'e', 'r'), 0.0);
}

#[test]
fn gpos_kern_exo2_ttf() {
    let font = FontRef::try_from_slice(EXO2_TTF).unwrap();
    assert_eq!(pair(&font, 'A', 'V'), -34.0);
    assert_eq!(pair(&font, 'T', 'o'), -113.0);
    assert_eq!(pair(&font, 'L', 'T'), -108.0);
    assert_eq!(pair(&font, 'o', 'x'), -11.0);
}

#[test]
fn gpos_kern_font_vec_matches_ref() {
    let font = FontVec::try_from_vec(EXO2_OTF.to_vec()).unwrap();
    assert_eq!(pair(&font, 'A', 'V'), -34.0);
    assert_eq!(pair(&font, 'T', 'o'), -113.0);
    assert_eq!(pair(&font, 'o', 'x'), -11.0);
}

/// Fonts with a legacy `kern` table must return exactly those values, ignoring
/// any GPOS pair adjustments.
#[test]
fn gpos_kern_legacy_table_takes_precedence() {
    let font = FontRef::try_from_slice(OPENS_SANS_ITALIC).unwrap();
    assert_eq!(pair(&font, 'A', 'V'), -82.0);
    assert_eq!(pair(&font, 'T', 'o'), -143.0);
    assert_eq!(pair(&font, 'V', 'a'), -41.0);
    assert_eq!(pair(&font, 'Y', 'o'), -102.0);
    assert_eq!(pair(&font, 'e', 'r'), 0.0);
}

/// Fonts with neither pair adjustments nor legacy kerning stay at zero.
#[test]
fn gpos_kern_without_kern_data_is_zero() {
    let deja_vu = FontRef::try_from_slice(DEJA_VU_MONO).unwrap();
    for (a, b) in [('A', 'V'), ('T', 'o'), ('V', 'a'), ('f', 'i')] {
        assert_eq!(pair(&deja_vu, a, b), 0.0, "DejaVuSansMono {a}{b}");
    }

    let cantarell = FontRef::try_from_slice(CANTARELL_VF).unwrap();
    for (a, b) in [('A', 'V'), ('T', 'o'), ('f', 'i')] {
        assert_eq!(pair(&cantarell, a, b), 0.0, "Cantarell-VF {a}{b}");
    }
}

#[test]
fn gpos_kern_scaled() {
    let font = FontRef::try_from_slice(EXO2_OTF).unwrap();
    let scaled = font.as_scaled(48.0);
    let (a, v) = (font.glyph_id('A'), font.glyph_id('V'));
    assert_eq!(
        scaled.kern(a, v),
        scaled.h_scale_factor() * font.kern_unscaled(a, v)
    );
    assert!(scaled.kern(a, v) < 0.0);
}

#[test]
fn gpos_kern_repeatable_queries() {
    let font = FontRef::try_from_slice(EXO2_OTF).unwrap();
    let (a, v) = (font.glyph_id('A'), font.glyph_id('V'));
    for _ in 0..10_000 {
        assert_eq!(font.kern_unscaled(a, v), -34.0);
    }
}
