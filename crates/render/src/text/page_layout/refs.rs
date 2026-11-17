//! Theorem-like statements, proofs and in-text references of a typeset paper, read from its page
//! layouts: which blocks are a Theorem, Lemma, Definition, Remark, … (the printed word, an open set
//! read from the text), with their number and name; which are proofs and what each proves; where
//! the text mentions a block of the document ("by Lemma 2.1", "see (3.4)", "Section 3") and which
//! block each mention points at. No link annotation is read here: this is the inferred half of a
//! document's reference graph (the engine adds the links).
//!
//! 1. *Words* ([`page_words`]): each page's lines in reading order (from [`PageLayout`]), cut into
//!    words at word gaps, each with its glyphs and the style of its font (bold, italic, small caps,
//!    mathematics), and each line's end-of-proof mark, if it has one (a box of rules, a filled or
//!    open square, `QED`).
//! 2. *Statements* ([`analyse_references`]): a run-in head at the start of a paragraph, set in bold,
//!    small caps or italic and unlike the body after it (`Lemma 2.1.`, `Theorem 3 (Main theorem).`,
//!    `Proof of Lemma 2.1.`), ending with a period or a colon. The kind is the printed word, never
//!    guessed from the number. The extent runs on through the head's paragraph, displays and list
//!    items that follow it, a paragraph that goes on after a display, and (for a statement set in
//!    italic) further italic paragraphs; it ends at the next head, a heading, or upright text that
//!    starts a paragraph. A proof ends at its end mark, or, without one, at the next head (and is
//!    less sure).
//! 3. *Mentions*: a kind word (`Lemma`, `Lemmas`, `Thm.`, `Section`, `§`, `Figure`, `Eq.`,
//!    `equation`) and a number; further numbers after `and`, a comma or a dash; an equation number
//!    alone (`(3.4)`) only after a word that cites (`by`, `see`, `from`, `in`, …) and only where a
//!    display has that number. A mention followed by `in [5]`, `of [Smith]`, `of the appendix of …`
//!    points outside the document. A mention resolves to the one block of its kind and number, and
//!    to nothing when there is none or more than one: a wrong link is worse than none.
//!
//! Work is linear in the words and lines of the document, with every count bounded (see the
//! constants); recursion is none. The measures are scored against TeX's own answer by the reference
//! oracle (`crates/render/tests/data/refs/oracle`).

mod mentions;
mod statements;
mod words;

#[cfg(test)]
mod tests;

pub use words::page_words;

use super::{BlockKind, DocumentStructure, PageLayout};

/// Most words read from one page; the rest are left out with a diagnostic.
pub const MAX_WORDS_PER_PAGE: usize = 60_000;
/// Most statements and proofs found in one document.
pub const MAX_STATEMENTS: usize = 20_000;
/// Most mentions found in one document.
pub const MAX_MENTIONS: usize = 200_000;
/// Most lines one statement or proof runs on for.
pub const MAX_STATEMENT_LINES: usize = 600;
/// Longest number a mention or a head carries, in bytes (`2.1.3.4`).
pub const MAX_NUMBER_BYTES: usize = 16;
/// Longest kind word or name kept, in bytes.
pub const MAX_LABEL_BYTES: usize = 120;

/// The look of a word, from its fonts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Style {
    pub bold: bool,
    pub italic: bool,
    pub small_caps: bool,
    /// Set in a mathematics font.
    pub math: bool,
}

/// A word of a line: glyphs with no word gap between them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Word {
    /// Its text (glyphs with no Unicode left out; ligatures spelled out).
    pub text: String,
    /// Indices into the page's glyphs, left to right.
    pub glyphs: Vec<u32>,
    /// Where each glyph's text starts in `text`, in bytes (one per glyph).
    pub starts: Vec<usize>,
    /// Each glyph's left and right edge (one per glyph).
    pub spans: Vec<[f32; 2]>,
    /// `[x0, y0, x1, y1]` in view space.
    pub rect: [f32; 4],
    /// The style most of its letters have (mathematics when most are).
    pub style: Style,
}

/// How a proof's end is marked.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EndMarkKind {
    /// An open box drawn with rules (amsthm's `\qedsymbol`).
    RuleBox,
    /// A filled square (■, `\blacksquare`).
    Filled,
    /// An open square glyph (□, `\square`).
    Open,
    /// The letters `QED` or `Q.E.D.`.
    Qed,
}

impl EndMarkKind {
    pub fn as_str(self) -> &'static str {
        match self {
            EndMarkKind::RuleBox => "box",
            EndMarkKind::Filled => "filled_square",
            EndMarkKind::Open => "square",
            EndMarkKind::Qed => "qed",
        }
    }
}

/// An end-of-proof mark at the end of a line.
#[derive(Clone, Debug, PartialEq)]
pub struct EndMark {
    pub kind: EndMarkKind,
    /// Its glyphs (none for a box of rules) and rules (indices into the page's).
    pub glyphs: Vec<u32>,
    pub rules: Vec<u32>,
    pub rect: [f32; 4],
}

/// A line of a page, in reading order.
#[derive(Clone, Debug, PartialEq)]
pub struct FlowLine {
    pub page: usize,
    /// Its block (index into the page's [`PageLayout::blocks`]) and the block's kind.
    pub block: usize,
    pub kind: BlockKind,
    /// The first line of its block.
    pub first: bool,
    pub rect: [f32; 4],
    pub baseline: f32,
    /// The size of its text (its words' most common em).
    pub size: f32,
    /// The left and right edge of the column it is in.
    pub column: [f32; 2],
    pub words: Vec<Word>,
    /// An end-of-proof mark at its end (its glyphs are also the last word's, or none for rules).
    pub mark: Option<EndMark>,
}

impl FlowLine {
    /// The line holds an end mark and nothing else (a mark the layout set apart from its line).
    pub fn mark_only(&self) -> bool {
        self.mark.as_ref().is_some_and(|m| self.words.iter().all(|w| w.glyphs.iter().all(|g| m.glyphs.contains(g))))
    }
}

/// The words of one page, line by line in reading order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PageWords {
    pub page: usize,
    pub lines: Vec<FlowLine>,
    /// The usual distance between the baselines of a paragraph, when the page has paragraphs.
    pub leading: Option<f32>,
    /// `capped` when the page had more than [`MAX_WORDS_PER_PAGE`] words.
    pub diagnostics: Vec<super::Diagnostic>,
}

/// Part of a statement or a mention on one page.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Piece {
    pub page: usize,
    /// Indices into the page's glyphs and rules.
    pub glyphs: Vec<u32>,
    pub rules: Vec<u32>,
    /// The box of its lines (a statement) or of its glyphs (a mention), in view space.
    pub rect: [f32; 4],
}

/// Where a line is: its page and its index in that page's [`PageWords::lines`].
pub type LineRef = (usize, usize);

/// How a statement's head is set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeadStyle {
    Bold,
    SmallCaps,
    Italic,
}

impl HeadStyle {
    pub fn as_str(self) -> &'static str {
        match self {
            HeadStyle::Bold => "bold",
            HeadStyle::SmallCaps => "small_caps",
            HeadStyle::Italic => "italic",
        }
    }
}

/// Why a proof proves what it does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProvesBy {
    /// Its head names the statement (`Proof of Lemma 2.1`).
    Named,
    /// It comes right after the statement.
    Previous,
}

/// A theorem-like statement or a proof.
#[derive(Clone, Debug, PartialEq)]
pub struct Statement {
    /// The printed word (`Theorem`, `Main Theorem`, `Proof`), as set.
    pub kind: String,
    pub proof: bool,
    /// The number as printed (`2.1`, `3`, `A.4`), without the period after it.
    pub number: Option<String>,
    /// The name in parentheses (`Main theorem`), without them; for a proof, the rest of its head
    /// (`of Lemma 2.1`, `sketch`).
    pub name: Option<String>,
    pub head_style: HeadStyle,
    /// The body is set in italic (amsthm's plain style), not upright (definition, remark, proof).
    pub italic_body: bool,
    /// The head's glyphs.
    pub head: Piece,
    /// Every glyph and rule of the statement, page by page.
    pub extent: Vec<Piece>,
    /// Its first and last line.
    pub first_line: LineRef,
    pub last_line: LineRef,
    /// For a proof: the statement it proves (index into [`References::statements`]) and why.
    pub proves: Option<(usize, ProvesBy)>,
    /// For a proof: its end mark and the page it is on.
    pub end_mark: Option<(usize, EndMark)>,
    pub confidence: f32,
}

/// What a mention is of.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MentionKind {
    Statement,
    Section,
    Equation,
    Figure,
    Table,
}

impl MentionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            MentionKind::Statement => "statement",
            MentionKind::Section => "section",
            MentionKind::Equation => "equation",
            MentionKind::Figure => "figure",
            MentionKind::Table => "table",
        }
    }
}

/// A block a mention points at.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Target {
    /// Index into [`References::statements`].
    Statement(usize),
    /// A heading block, a display block, a caption block (its float is the caption's partner).
    Heading {
        page: usize,
        block: usize,
    },
    Equation {
        page: usize,
        block: usize,
    },
    Float {
        page: usize,
        block: usize,
    },
}

/// Why a mention points at nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unresolved {
    /// It names another work (`Theorem 3.1 in [5]`).
    External,
    /// No block of its kind has its number.
    Missing,
    /// More than one does.
    Ambiguous,
}

impl Unresolved {
    pub fn as_str(self) -> &'static str {
        match self {
            Unresolved::External => "external",
            Unresolved::Missing => "missing",
            Unresolved::Ambiguous => "ambiguous",
        }
    }
}

/// A place where the text mentions a block.
#[derive(Clone, Debug, PartialEq)]
pub struct Mention {
    pub kind: MentionKind,
    /// The kind word as printed (`Lemmas`, `Thm.`, `equation`); empty for a number alone (`(3.4)`)
    /// or a later number of a list (`2.3` of `Lemmas 2.1 and 2.3`).
    pub word: String,
    /// For a statement: the kind it names, as a head prints it (`Lemma` for `Lemmas`, `Theorem`
    /// for `Thm.`).
    pub statement_kind: Option<String>,
    /// The number as printed (`2.1`, `(3.4)`).
    pub number: String,
    /// What follows the number that is not part of the link (`(ii)`).
    pub extra: Option<String>,
    /// The glyphs that should be clickable: the word and the number.
    pub pieces: Vec<Piece>,
    pub target: Option<Target>,
    pub unresolved: Option<Unresolved>,
    /// How sure the resolution is, from 0 to 1 (0 when there is no target).
    pub confidence: f32,
}

/// The statements, proofs and mentions of a document.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct References {
    pub statements: Vec<Statement>,
    pub mentions: Vec<Mention>,
    /// `statements_capped`, `mentions_capped`, `statement_cut` (a statement longer than
    /// [`MAX_STATEMENT_LINES`] lines), `proof_unended` (a proof with no end mark),
    /// `proof_unattached` (a proof that proves nothing found).
    pub diagnostics: Vec<super::Diagnostic>,
}

/// The statements, proofs and mentions of a document from its pages' layouts and words (the same
/// pages, in order) and its section tree ([`super::analyse_document`]). Never fails.
pub fn analyse_references(layouts: &[PageLayout], words: &[PageWords], structure: &DocumentStructure) -> References {
    let mut out = References::default();
    statements::find(words, &mut out);
    mentions::find(layouts, words, structure, &mut out);
    out
}

fn union(a: &mut [f32; 4], b: &[f32; 4]) {
    if !b.iter().all(|v| v.is_finite()) {
        return;
    }
    a[0] = a[0].min(b[0]);
    a[1] = a[1].min(b[1]);
    a[2] = a[2].max(b[2]);
    a[3] = a[3].max(b[3]);
}

const EMPTY: [f32; 4] = [f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY];

fn finite_or_zero(r: [f32; 4]) -> [f32; 4] {
    if r.iter().all(|v| v.is_finite()) { r } else { [0.0; 4] }
}
