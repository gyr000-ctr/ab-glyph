//! Paragraph layout: line metrics, word-wrap line breaking with backtracking
//! and kerning accumulation.
//!
//! ```
//! # use ab_glyph::*;
//! # fn main() -> Result<(), ab_glyph::InvalidFont> {
//! let font = FontRef::try_from_slice(include_bytes!("../../dev/fonts/Exo2-Light.otf"))?;
//! let scaled = font.as_scaled(24.0);
//!
//! let paragraph = layout_paragraph(&scaled, point(10.0, 10.0), 200.0, "Hello world!").unwrap();
//!
//! for glyph in &paragraph.glyphs {
//!     // draw glyph at glyph.position
//! }
//! # Ok(()) }
//! ```
use crate::{point, Font, Glyph, GlyphId, Point, ScaleFont};
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

/// Vertical line metrics used for paragraph layout.
///
/// See [glyph layout concepts](Font#glyph-layout-concepts).
#[derive(Copy, Clone, Default, Debug, PartialEq, PartialOrd)]
pub struct LineMetrics {
    /// Pixel scaled glyph ascent.
    pub ascent: f32,
    /// Pixel scaled glyph descent.
    pub descent: f32,
    /// Pixel scaled line gap.
    pub line_gap: f32,
}

impl LineMetrics {
    /// Pixel scaled line metrics of a font at its associated scale.
    #[inline]
    pub fn new<F: Font, SF: ScaleFont<F>>(font: &SF) -> Self {
        Self {
            ascent: font.ascent(),
            descent: font.descent(),
            line_gap: font.line_gap(),
        }
    }

    /// Line height: `ascent - descent`.
    #[inline]
    pub fn height(&self) -> f32 {
        self.ascent - self.descent
    }

    /// Baseline-to-baseline distance: `height + line_gap`.
    #[inline]
    pub fn line_advance(&self) -> f32 {
        self.height() + self.line_gap
    }
}

/// Paragraph layout error.
#[derive(Clone, Debug, PartialEq)]
pub enum ParagraphError {
    /// `max_width` was not positive (or `NaN`).
    InvalidMaxWidth(f32),
    /// A word without any break opportunity is wider than `max_width`,
    /// so it cannot be laid out without overflowing.
    UnbreakableWord {
        /// The offending word.
        word: String,
        /// Pixel width of the word including internal kerning.
        width: f32,
        /// The `max_width` the word did not fit into.
        max_width: f32,
    },
}

impl fmt::Display for ParagraphError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidMaxWidth(max_width) => {
                write!(f, "ParagraphError: invalid max_width `{max_width}`")
            }
            Self::UnbreakableWord {
                word,
                width,
                max_width,
            } => write!(
                f,
                "ParagraphError: unbreakable word `{word}` (width `{width}`) \
                 exceeds max_width `{max_width}`"
            ),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for ParagraphError {}

/// A single laid out line of a [`Paragraph`].
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd)]
pub struct Line {
    /// Index of the line's first glyph in [`Paragraph::glyphs`].
    pub glyph_start: usize,
    /// Index one past the line's last glyph in [`Paragraph::glyphs`].
    pub glyph_end: usize,
    /// Baseline y-position of this line.
    pub baseline_y: f32,
    /// Horizontal advance used by this line, including any trailing whitespace.
    pub width: f32,
}

/// Result of laying out a paragraph with [`layout_paragraph`].
///
/// An empty text (or one containing only control characters) lays out to an
/// empty `Paragraph`: no glyphs and no lines.
#[derive(Clone, Debug, Default)]
pub struct Paragraph {
    /// Positioned glyphs in text order. Every non-control character of the
    /// input produces exactly one glyph, including whitespace.
    pub glyphs: Vec<Glyph>,
    /// Lines in top-to-bottom order. Ranges over [`Self::glyphs`] are
    /// contiguous and cover every glyph.
    pub lines: Vec<Line>,
    /// Line metrics used for this layout.
    pub metrics: LineMetrics,
}

/// Lay out `text` as a paragraph starting at `position`, wrapping lines at
/// `max_width`.
///
/// The first baseline is at `position.y + ascent`. Lines break at whitespace
/// (greedy word wrap): when a word does not fit on the current line the
/// layout backtracks and moves the whole word to the next line. `\n` forces a
/// line break. Kerning is accumulated between adjacent glyphs on the same
/// line; no kerning is applied across a line break.
///
/// Break positions are deterministic for a given `(font, scale, position,
/// max_width, text)` and no character is dropped: every non-control character
/// yields one glyph in [`Paragraph::glyphs`]. Memory usage is `O(text)`.
///
/// # Errors
/// * [`ParagraphError::InvalidMaxWidth`] if `max_width <= 0.0` or `NaN`.
/// * [`ParagraphError::UnbreakableWord`] if a single word is wider than
///   `max_width`.
pub fn layout_paragraph<F, SF>(
    font: SF,
    position: Point,
    max_width: f32,
    text: &str,
) -> Result<Paragraph, ParagraphError>
where
    F: Font,
    SF: ScaleFont<F>,
{
    if !(max_width > 0.0) {
        return Err(ParagraphError::InvalidMaxWidth(max_width));
    }

    let metrics = LineMetrics::new(&font);
    let mut layouter = Layouter {
        font,
        _font_marker: core::marker::PhantomData,
        x_start: position.x,
        max_width,
        caret: point(position.x, position.y + metrics.ascent),
        v_advance: metrics.line_advance(),
        last_id: None,
        line_start: 0,
        glyphs: Vec::new(),
        lines: Vec::new(),
        word: Vec::new(),
        word_text: String::new(),
        word_width: 0.0,
        word_last: None,
    };

    for c in text.chars() {
        if c == '\n' {
            layouter.flush_word()?;
            layouter.new_line();
            continue;
        }
        if c.is_control() {
            continue;
        }
        if c.is_whitespace() {
            layouter.flush_word()?;
            layouter.push_space(c);
            continue;
        }
        layouter.push_word_char(c);
    }
    layouter.flush_word()?;

    Ok(layouter.finish(metrics))
}

/// Greedy word-wrap layout state.
struct Layouter<F: Font, SF: ScaleFont<F>> {
    font: SF,
    _font_marker: core::marker::PhantomData<fn() -> F>,
    /// Left edge of every line.
    x_start: f32,
    max_width: f32,
    caret: Point,
    /// Baseline-to-baseline distance.
    v_advance: f32,
    /// Last glyph placed on the current line, for kerning.
    last_id: Option<GlyphId>,
    /// Index in `glyphs` where the current line starts.
    line_start: usize,
    glyphs: Vec<Glyph>,
    lines: Vec<Line>,
    /// Unpositioned glyphs of the word currently being accumulated.
    word: Vec<Glyph>,
    /// Text of the current word, for error reporting.
    word_text: String,
    /// Width of the current word including internal kerning.
    word_width: f32,
    word_last: Option<GlyphId>,
}

impl<F: Font, SF: ScaleFont<F>> Layouter<F, SF> {
    fn line_width(&self) -> f32 {
        self.caret.x - self.x_start
    }

    /// Close the current line and move the caret to the start of the next.
    fn new_line(&mut self) {
        self.lines.push(Line {
            glyph_start: self.line_start,
            glyph_end: self.glyphs.len(),
            baseline_y: self.caret.y,
            width: self.line_width(),
        });
        self.caret = point(self.x_start, self.caret.y + self.v_advance);
        self.last_id = None;
        self.line_start = self.glyphs.len();
    }

    /// Buffer a non-whitespace character into the current word.
    fn push_word_char(&mut self, c: char) {
        let id = self.font.glyph_id(c);
        if let Some(prev) = self.word_last {
            self.word_width += self.font.kern(prev, id);
        }
        self.word_width += self.font.h_advance(id);
        self.word.push(id.with_scale(self.font.scale()));
        self.word_text.push(c);
        self.word_last = Some(id);
    }

    /// Place a whitespace glyph at the caret on the current line.
    fn push_space(&mut self, c: char) {
        let id = self.font.glyph_id(c);
        if let Some(prev) = self.last_id {
            self.caret.x += self.font.kern(prev, id);
        }
        let glyph = id.with_scale_and_position(self.font.scale(), self.caret);
        self.caret.x += self.font.h_advance(id);
        self.last_id = Some(id);
        self.glyphs.push(glyph);
    }

    /// Position the buffered word onto the current line, applying
    /// `leading_kern` before its first glyph, and append it to `glyphs`.
    fn place_word(&mut self, leading_kern: f32) {
        self.caret.x += leading_kern;
        let mut prev = None;
        for mut glyph in self.word.drain(..) {
            if let Some(prev) = prev {
                self.caret.x += self.font.kern(prev, glyph.id);
            }
            glyph.position = self.caret;
            self.caret.x += self.font.h_advance(glyph.id);
            prev = Some(glyph.id);
            self.glyphs.push(glyph);
        }
        self.last_id = prev;
        self.word_text.clear();
        self.word_width = 0.0;
        self.word_last = None;
    }

    /// Flush the buffered word, breaking the line before it (backtracking)
    /// when it does not fit.
    fn flush_word(&mut self) -> Result<(), ParagraphError> {
        while !self.word.is_empty() {
            if self.line_start == self.glyphs.len() {
                // Empty line: the word must fit on its own.
                if self.word_width > self.max_width {
                    return Err(ParagraphError::UnbreakableWord {
                        word: core::mem::take(&mut self.word_text),
                        width: self.word_width,
                        max_width: self.max_width,
                    });
                }
                self.place_word(0.0);
            } else {
                let kern = self.font.kern(self.last_id.unwrap(), self.word[0].id);
                if self.line_width() + kern + self.word_width <= self.max_width {
                    self.place_word(kern);
                } else {
                    // Backtrack: leave the word unplaced and retry on a new line.
                    self.new_line();
                }
            }
        }
        Ok(())
    }

    fn finish(mut self, metrics: LineMetrics) -> Paragraph {
        if self.glyphs.len() > self.line_start {
            self.lines.push(Line {
                glyph_start: self.line_start,
                glyph_end: self.glyphs.len(),
                baseline_y: self.caret.y,
                width: self.line_width(),
            });
        }
        Paragraph {
            glyphs: self.glyphs,
            lines: self.lines,
            metrics,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FontRef, PxScale};

    const OPEN_SANS_ITALIC: &[u8] = include_bytes!("../../dev/fonts/OpenSans-Italic.ttf");

    fn font() -> FontRef<'static> {
        FontRef::try_from_slice(OPEN_SANS_ITALIC).unwrap()
    }

    fn layout(
        scale: f32,
        max_width: f32,
        text: &str,
    ) -> Result<Paragraph, ParagraphError> {
        let font = font().into_scaled(scale);
        layout_paragraph(&font, point(0.0, 0.0), max_width, text)
    }

    #[test]
    fn paragraph_metrics() {
        let font = font();
        let font = font.as_scaled(PxScale::from(30.0));
        let metrics = LineMetrics::new(&font);
        assert_eq!(metrics.ascent, font.ascent());
        assert_eq!(metrics.descent, font.descent());
        assert_eq!(metrics.line_gap, font.line_gap());
        assert_eq!(metrics.height(), metrics.ascent - metrics.descent);
        assert_eq!(metrics.height(), 30.0);
        assert_eq!(metrics.line_advance(), metrics.height() + metrics.line_gap);
    }

    #[test]
    fn paragraph_empty_text() {
        let paragraph = layout(24.0, 100.0, "").unwrap();
        assert!(paragraph.glyphs.is_empty());
        assert!(paragraph.lines.is_empty());

        // Control-only text also lays out empty.
        let paragraph = layout(24.0, 100.0, "\u{0}\u{7}").unwrap();
        assert!(paragraph.glyphs.is_empty());
        assert!(paragraph.lines.is_empty());
    }

    #[test]
    fn paragraph_invalid_max_width() {
        for &bad in &[0.0, -1.0, f32::NAN] {
            match layout(24.0, bad, "hello") {
                Err(ParagraphError::InvalidMaxWidth(w)) => {
                    assert!(w == bad || (w.is_nan() && bad.is_nan()))
                }
                other => panic!("expected InvalidMaxWidth for {bad}, got {other:?}"),
            }
        }
        // Distinct from UnbreakableWord: tiny but positive width errors differently.
        assert!(matches!(
            layout(24.0, 1.0, "hello"),
            Err(ParagraphError::UnbreakableWord { .. })
        ));
    }

    #[test]
    fn paragraph_unbreakable_word() {
        let scale = 24.0;
        let font = font().into_scaled(scale);
        let word = "supercalifragilistic";
        let word_width: f32 = {
            let mut w = 0.0;
            let mut prev = None;
            for c in word.chars() {
                let id = font.glyph_id(c);
                if let Some(prev) = prev {
                    w += font.kern(prev, id);
                }
                w += font.h_advance(id);
                prev = Some(id);
            }
            w
        };
        let max_width = word_width - 1.0;
        match layout(scale, max_width, word) {
            Err(ParagraphError::UnbreakableWord {
                word: w,
                width,
                max_width: m,
            }) => {
                assert_eq!(w, word);
                assert_eq!(width, word_width);
                assert_eq!(m, max_width);
            }
            other => panic!("expected UnbreakableWord, got {other:?}"),
        }
    }

    #[test]
    fn paragraph_no_missing_chars() {
        let text = "The quick brown fox\njumps over  the\tlazy dog!";
        let paragraph = layout(18.0, 120.0, text).unwrap();
        // One glyph per non-control character, none swallowed.
        let expected = text.chars().filter(|c| !c.is_control()).count();
        assert_eq!(paragraph.glyphs.len(), expected);
        // Line ranges are contiguous and cover every glyph.
        let mut covered = 0;
        for line in &paragraph.lines {
            assert_eq!(line.glyph_start, covered);
            covered = line.glyph_end;
        }
        assert_eq!(covered, paragraph.glyphs.len());
    }

    #[test]
    fn paragraph_wrap_reproducible() {
        let text = "The quick brown fox jumps over the lazy dog. \
                    Pack my box with five dozen liquor jugs.";
        let a = layout(20.0, 150.0, text).unwrap();
        let b = layout(20.0, 150.0, text).unwrap();
        assert_eq!(a.glyphs, b.glyphs, "same input must break identically");
        assert_eq!(a.lines, b.lines);
        assert!(a.lines.len() > 1, "expected wrapping at max_width 150");

        // Every word fits: line width minus trailing whitespace <= max_width.
        let font = font().into_scaled(20.0);
        for line in &a.lines {
            let mut width = line.width;
            for glyph in a.glyphs[line.glyph_start..line.glyph_end].iter().rev() {
                let c_width = font.h_advance(glyph.id);
                // Only trailing whitespace is excluded from the fit check.
                if glyph.id != font.glyph_id(' ') {
                    break;
                }
                width -= c_width;
            }
            assert!(
                width <= 150.0 + f32::EPSILON,
                "line overflowed: {width} > 150"
            );
        }
    }

    #[test]
    fn paragraph_wrap_backtracks_word() {
        let scale = 24.0;
        let font = font().into_scaled(scale);
        let text = "aaa bbb";
        let w = |s: &str| -> f32 {
            s.chars()
                .map(|c| font.h_advance(font.glyph_id(c)))
                .sum::<f32>()
        };
        // Fits "aaa " but not "aaa bbb" on one line.
        let max_width = w("aaa ") + 1.0;
        let paragraph = layout(scale, max_width, text).unwrap();
        assert_eq!(paragraph.lines.len(), 2);
        // The whole word moved to line 2; no kerning across the break.
        let second = &paragraph.lines[1];
        let first_glyph = paragraph.glyphs[second.glyph_start].clone();
        assert_eq!(first_glyph.id, font.glyph_id('b'));
        assert_eq!(first_glyph.position.x, 0.0);
        assert_eq!(second.baseline_y, paragraph.metrics.ascent + paragraph.metrics.line_advance());
    }

    #[test]
    fn paragraph_newline_forces_break() {
        let paragraph = layout(24.0, 9999.0, "ab\ncd").unwrap();
        assert_eq!(paragraph.lines.len(), 2);
        let advance = paragraph.metrics.line_advance();
        let dy = paragraph.lines[1].baseline_y - paragraph.lines[0].baseline_y;
        assert!((dy - advance).abs() < 0.001, "{dy} != {advance}");
        // First baseline sits at ascent.
        assert_eq!(paragraph.lines[0].baseline_y, paragraph.metrics.ascent);
    }

    #[test]
    fn paragraph_kerning_accumulation() {
        let scale = 40.0;
        let font = font().into_scaled(scale);
        let (a, v) = (font.glyph_id('A'), font.glyph_id('V'));
        let kern = font.kern(a, v);
        assert!(kern != 0.0, "test font should kern `AV`");

        let paragraph = layout(scale, 9999.0, "AV").unwrap();
        assert_eq!(paragraph.glyphs.len(), 2);
        assert_eq!(paragraph.glyphs[0].position.x, 0.0);
        // Second glyph x accumulates advance + kern of the pair.
        assert_eq!(
            paragraph.glyphs[1].position.x,
            font.h_advance(a) + kern
        );
    }

    #[test]
    fn paragraph_wide_max_width_single_line() {
        let paragraph = layout(24.0, f32::INFINITY, "one two three").unwrap();
        assert_eq!(paragraph.lines.len(), 1);
        assert_eq!(paragraph.glyphs.len(), "one two three".len());
    }

    #[test]
    fn paragraph_memory_is_linear() {
        // Layout must not blow up super-linearly; a long text lays out fine.
        let text: String = "lorem ipsum dolor sit amet ".repeat(2000);
        let paragraph = layout(12.0, 200.0, &text).unwrap();
        let expected = text.chars().filter(|c| !c.is_control()).count();
        assert_eq!(paragraph.glyphs.len(), expected);
        assert!(paragraph.lines.len() > 100);
    }
}
