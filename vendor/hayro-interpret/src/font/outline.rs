use crate::CacheKey;
use crate::font::cid::Type0Font;
use crate::font::true_type::TrueTypeFont;
use crate::font::type1::Type1Font;
use hayro_cmap::BfString;
use kurbo::BezPath;
use skrifa::GlyphId;
use skrifa::outline::OutlinePen;
use std::rc::Rc;

/// Font data and metadata for downstream use.
#[derive(Clone)]
pub struct OutlineFontData {
    /// Raw font bytes (TrueType/OpenType/CFF data).
    pub data: crate::font::FontData,
    /// Cache key for font deduplication.
    pub cache_key: u128,
    /// PostScript name (e.g., "TimesNewRomanPS-BoldMT").
    pub postscript_name: Option<String>,
    /// Font weight (100-900, 400=normal, 700=bold).
    pub weight: Option<u32>,
    /// Whether the font is italic/oblique.
    pub is_italic: bool,
    /// Whether the font is serif (vs sans-serif).
    pub is_serif: bool,
    /// Whether the font is monospace.
    pub is_monospace: bool,
}

/// PdfCraft patch (math extraction): which kind of program a [`FontProgram`] holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FontProgramKind {
    /// A Type 1 program (`/FontFile`: PFA/PFB-style cleartext and an eexec section). Not an sfnt.
    Type1,
    /// A bare CFF program (`/FontFile3`, `Type1C` or `CIDFontType0C`).
    Cff,
    /// An sfnt: TrueType or OpenType (with `glyf` or `CFF ` outlines).
    OpenType,
    /// Not embedded: one of the standard 14 faces, or the substitute for a missing font.
    Standard,
}

/// PdfCraft patch (math extraction): the program a glyph's font draws from.
#[derive(Clone)]
pub struct FontProgram {
    /// What `data` is.
    pub kind: FontProgramKind,
    /// The program's bytes as embedded (decoded from the stream filters). `None` when the font
    /// is not embedded ([`FontProgramKind::Standard`]).
    pub data: Option<crate::font::FontData>,
}

impl std::fmt::Debug for FontProgram {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let len = self.data.as_ref().map(|d| (**d).as_ref().len());
        f.debug_struct("FontProgram").field("kind", &self.kind).field("len", &len).finish()
    }
}

pub(crate) struct OutlinePath(BezPath);

impl OutlinePath {
    pub(crate) fn new() -> Self {
        Self(BezPath::new())
    }

    pub(crate) fn take(self) -> BezPath {
        self.0
    }
}

impl OutlinePen for OutlinePath {
    #[inline]
    fn move_to(&mut self, x: f32, y: f32) {
        self.0.move_to((x, y));
    }

    #[inline]
    fn line_to(&mut self, x: f32, y: f32) {
        if !self.0.elements().is_empty() {
            self.0.line_to((x, y));
        }
    }

    #[inline]
    fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) {
        if !self.0.elements().is_empty() {
            self.0.quad_to((cx, cy), (x, y));
        }
    }

    #[inline]
    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        if !self.0.elements().is_empty() {
            self.0.curve_to((cx0, cy0), (cx1, cy1), (x, y));
        }
    }

    #[inline]
    fn close(&mut self) {
        if !self.0.elements().is_empty() {
            self.0.close_path();
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) enum OutlineFont {
    Type1(Rc<Type1Font>),
    TrueType(Rc<TrueTypeFont>),
    Type0(Rc<Type0Font>),
}

impl CacheKey for OutlineFont {
    fn cache_key(&self) -> u128 {
        match self {
            Self::Type1(f) => f.cache_key(),
            Self::TrueType(t) => t.cache_key(),
            Self::Type0(t0) => t0.cache_key(),
        }
    }
}

impl OutlineFont {
    pub(crate) fn outline_glyph(&self, glyph: GlyphId, code: u32) -> BezPath {
        match self {
            Self::Type1(t) => t.outline_glyph(glyph),
            Self::TrueType(t) => t.outline_glyph(glyph),
            Self::Type0(t) => t.outline_glyph(glyph, code),
        }
    }

    pub(crate) fn char_code_to_unicode(&self, char_code: u32) -> Option<BfString> {
        match self {
            Self::Type1(t) => t.char_code_to_unicode(char_code),
            Self::TrueType(t) => t.char_code_to_unicode(char_code),
            Self::Type0(t) => t.char_code_to_unicode(char_code),
        }
    }

    /// Get the advance width for a glyph by character code.
    pub(crate) fn glyph_advance_width(&self, char_code: u32) -> Option<f32> {
        match self {
            Self::Type1(t) => t.glyph_width(char_code as u8),
            Self::TrueType(t) => Some(t.glyph_width(char_code as u8)),
            Self::Type0(t) => Some(t.code_advance(char_code).x as f32),
        }
    }

    /// PdfCraft patch (math extraction): the glyph's name, see `Glyph::glyph_name`.
    pub(crate) fn glyph_name(&self, code: u32, gid: GlyphId) -> Option<String> {
        match self {
            Self::Type1(t) => t.glyph_name(code, gid),
            Self::TrueType(t) => t.glyph_name(code),
            Self::Type0(_) => None,
        }
    }

    /// PdfCraft patch (math extraction): the font program of whatever kind, see
    /// `OutlineGlyph::font_program`.
    pub(crate) fn font_program(&self) -> FontProgram {
        match self {
            Self::Type1(t) => t.font_program(),
            Self::TrueType(t) => t.font_program(),
            Self::Type0(t) => t.font_program(),
        }
    }

    /// Get raw font bytes and metadata.
    ///
    /// Returns None for Type1 fonts and non-embedded TrueType fonts.
    pub(crate) fn font_data(&self) -> Option<OutlineFontData> {
        match self {
            Self::Type1(_) => None,
            Self::TrueType(t) => Some(OutlineFontData {
                data: t.font_data()?,
                cache_key: t.cache_key(),
                postscript_name: t.postscript_name().map(|s| s.to_string()),
                weight: t.weight(),
                is_italic: t.is_italic(),
                is_serif: t.is_serif(),
                is_monospace: t.is_monospace(),
            }),
            Self::Type0(t) => Some(OutlineFontData {
                data: t.font_data()?,
                cache_key: t.cache_key(),
                postscript_name: t.postscript_name().map(|s| s.to_string()),
                weight: t.weight(),
                is_italic: t.is_italic(),
                is_serif: t.is_serif(),
                is_monospace: t.is_monospace(),
            }),
        }
    }
}
