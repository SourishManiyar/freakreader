//! Page layout analysis for typeset papers: the columns of a page, its lines and blocks
//! (paragraphs, headings, displayed equations, floats, captions, footnotes, list items, the title,
//! the abstract, running heads and page numbers), their reading order, and across pages the running
//! elements and the section tree.
//!
//! It reads a [`MathPage`] (every glyph with its font, size, ink and baseline, every rule) and works
//! from geometry, not from text: columns from vertical whitespace alleys between line fragments,
//! lines from baselines (a sub- or superscript stays on the line it rides on), paragraphs from the
//! indent of a first line, a short last line, a gap or a change of size or weight, displays from a
//! centred or indented line made mostly of mathematics fonts and rules, floats from the frames and
//! table rules drawn around them. Text is read only for a few labels a block announces itself by
//! (`Figure 2`, `Abstract`, a section number, a list bullet, an equation number). Running heads are
//! decided across pages by what repeats at the same place ([`analyse_document`]).
//!
//! It sits beside the text layer (`PageText`, `text_extract`, search, export), which it does not
//! change. Each block carries a confidence, and the page a list of [`Diagnostic`]s (columns not
//! decided, a heading level guessed, a page with no text), so a caller can say when it is unsure.
//! A page with no text, or only invisible (OCR) text, gives an empty layout with a diagnostic.
//!
//! Work is bounded by the input: sorting and sweeps, `O(n log n)` in the glyphs; rule clustering is
//! skipped past [`MAX_FRAME_RULES`] rules, and a page's text past [`MAX_BLOCK_TEXT_BYTES`] per block
//! is cut. Non-finite coordinates, zero-size glyphs and text in other directions are left out with a
//! diagnostic. The measures are scored against TeX's own answer by the layout oracle
//! (`crates/render/tests/data/layout/oracle`).

use super::math::MathPage;

mod analyse;
pub mod bib;
mod document;
pub mod refs;

pub use document::{DocumentStructure, SectionNode, analyse_document};

/// Most rules clustered into float frames on one page (the clustering is quadratic in them).
pub const MAX_FRAME_RULES: usize = 2_000;
/// Longest text kept for one block, in bytes (cut at a character boundary).
pub const MAX_BLOCK_TEXT_BYTES: usize = 16 * 1024;
/// Most columns side by side.
pub const MAX_COLUMNS: usize = 4;

/// What a block is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum BlockKind {
    Title,
    Abstract,
    Heading,
    Paragraph,
    DisplayEquation,
    /// A figure or table region (found by its frame or rules).
    Float,
    Caption,
    Footnote,
    RunningHeader,
    RunningFooter,
    PageNumber,
    ListItem,
    Other,
}

impl BlockKind {
    pub const ALL: [BlockKind; 13] = [
        BlockKind::Title,
        BlockKind::Abstract,
        BlockKind::Heading,
        BlockKind::Paragraph,
        BlockKind::DisplayEquation,
        BlockKind::Float,
        BlockKind::Caption,
        BlockKind::Footnote,
        BlockKind::RunningHeader,
        BlockKind::RunningFooter,
        BlockKind::PageNumber,
        BlockKind::ListItem,
        BlockKind::Other,
    ];

    /// The name the tools use.
    pub fn as_str(self) -> &'static str {
        match self {
            BlockKind::Title => "title",
            BlockKind::Abstract => "abstract",
            BlockKind::Heading => "heading",
            BlockKind::Paragraph => "paragraph",
            BlockKind::DisplayEquation => "display_equation",
            BlockKind::Float => "float",
            BlockKind::Caption => "caption",
            BlockKind::Footnote => "footnote",
            BlockKind::RunningHeader => "running_header",
            BlockKind::RunningFooter => "running_footer",
            BlockKind::PageNumber => "page_number",
            BlockKind::ListItem => "list_item",
            BlockKind::Other => "other",
        }
    }

    pub fn from_name(name: &str) -> Option<BlockKind> {
        BlockKind::ALL.into_iter().find(|k| k.as_str() == name)
    }

    /// Page furniture that repeats from page to page.
    pub fn is_running(self) -> bool {
        matches!(self, BlockKind::RunningHeader | BlockKind::RunningFooter | BlockKind::PageNumber)
    }
}

/// One line of a block: glyphs on one baseline, with the scripts that ride on it.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    /// Indices into [`MathPage::glyphs`], left to right.
    pub glyphs: Vec<u32>,
    /// The y of the line's main baseline (view space).
    pub baseline: f32,
    pub rect: [f32; 4],
}

/// What a heading says about itself.
#[derive(Clone, Debug, PartialEq)]
pub struct HeadingInfo {
    /// 1 for a section, 2 for a subsection, …
    pub level: u8,
    /// The number it starts with (`3.2`, `A`, `A.1`), if any.
    pub number: Option<String>,
    /// The level comes from size and weight, not from a number.
    pub level_guessed: bool,
    /// An appendix heading (lettered, or under one).
    pub appendix: bool,
}

/// The look of a block's text: its most common size and whether most of it is bold or small caps.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TextStyle {
    pub size: f32,
    pub bold: bool,
    pub small_caps: bool,
}

/// One block.
#[derive(Clone, Debug, PartialEq)]
pub struct Block {
    pub kind: BlockKind,
    /// `[x0, y0, x1, y1]` in view space (points from the page's top-left, y down).
    pub rect: [f32; 4],
    pub lines: Vec<Line>,
    /// Indices into [`MathPage::rules`] of the rules that belong to it (a fraction bar, a frame).
    pub rules: Vec<u32>,
    /// Its text, lines joined by spaces (glyphs with no Unicode left out).
    pub text: String,
    /// Its place in [`PageLayout::reading_order`].
    pub order: usize,
    /// The column it sits in (index into [`PageLayout::columns`]), `None` when it spans columns.
    pub column: Option<usize>,
    /// How sure the analysis is of its boundary and kind, from 0 to 1.
    pub confidence: f32,
    pub style: TextStyle,
    /// For a heading.
    pub heading: Option<HeadingInfo>,
    /// For a displayed equation: its number as printed in the margin (`(3.2)`).
    pub equation_tag: Option<String>,
    /// For a float: its caption block. For a caption: its float block.
    pub partner: Option<usize>,
}

impl Block {
    /// Every glyph of the block, line by line.
    pub fn glyphs(&self) -> impl Iterator<Item = u32> + '_ {
        self.lines.iter().flat_map(|l| l.glyphs.iter().copied())
    }
}

/// A column: a stretch of the page between whitespace alleys, within one zone (a band of the page
/// with one column layout; a block spanning the columns starts a zone of its own).
#[derive(Clone, Debug, PartialEq)]
pub struct Column {
    pub rect: [f32; 4],
    /// Zones are numbered top to bottom.
    pub zone: usize,
    /// Left to right within the zone, from 0.
    pub index: usize,
    /// How many columns its zone has.
    pub of: usize,
}

/// Something the analysis is unsure of or left out.
#[derive(Clone, Debug, PartialEq)]
pub struct Diagnostic {
    /// A stable code: `no_text`, `invisible_text`, `other_direction`, `bad_glyphs`, `capped`,
    /// `truncated`, `columns_undecided`, `heading_level_guessed`, `running_unconfirmed`,
    /// `too_many_rules`, `float_without_caption`.
    pub code: &'static str,
    pub message: String,
    /// The block it is about, if one.
    pub block: Option<usize>,
}

/// The layout of one page.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PageLayout {
    /// Page index, from 0.
    pub page: usize,
    /// The displayed page's width and height in points.
    pub size: [f32; 2],
    /// The most common text size on the page (the body text's), if any text.
    pub body_size: Option<f32>,
    pub columns: Vec<Column>,
    pub blocks: Vec<Block>,
    /// Indices into `blocks`, in reading order.
    pub reading_order: Vec<usize>,
    pub diagnostics: Vec<Diagnostic>,
}

/// Where a page sits: its index and displayed size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PageContext {
    pub index: usize,
    pub width: f32,
    pub height: f32,
}

/// The layout of one page: columns, blocks, reading order and diagnostics. Never fails; a page the
/// analysis cannot read gives an empty layout that says why.
pub fn analyse_page(page: &MathPage, ctx: PageContext) -> PageLayout {
    analyse::run(page, ctx)
}

/// `s` cut to at most `max` bytes, at a character boundary.
fn cut(s: &mut String, max: usize) {
    if s.len() > max {
        let at = s.floor_char_boundary(max);
        s.truncate(at);
    }
}

#[cfg(test)]
mod tests;
