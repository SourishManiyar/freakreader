//! The bibliography of a typeset paper and its citations, read from its page layouts and words
//! ([`super::refs::page_words`]), with no help from link annotations: where the reference list is,
//! its entries, what each entry's fields are, where the text cites an entry and which one.
//!
//! 1. *Region* ([`region`]): the list starts after its heading ("References", "Bibliography",
//!    "Literature", "Works cited", numbered or not, in any case) and runs to the next heading or the
//!    end, across pages and columns. With no such heading, a list at the very end of the paper whose
//!    lines start `[1]`, `[2]`, `[3]`, … in order is taken, less surely. Nothing else: no list is
//!    guessed from lone `[1]`s in the text, and a document with none says so.
//! 2. *Entries* ([`entries`]): a line starts an entry when it starts with a label at the list's
//!    label edge (`[12]`, `12.`, `[BKM20]`), or, in a list without labels, when it starts at the
//!    left edge where the other lines of an entry hang indented, or after a gap larger than the
//!    list's line spacing, or (lines all flush) when the line before ended an entry and this one
//!    starts with a family name and a comma. A line at the top of a column or a page goes on the
//!    entry before it unless it starts one by these rules. A URL or DOI broken over lines is joined.
//! 3. *Fields* ([`fields`]): from an entry's words and their fonts, each with a confidence:
//!    authors (family names and given initials, `and`, commas, `et al.`), title (italic, quoted, or
//!    the sentence after the authors), venue, year (with a suffix), volume, number, pages, DOI,
//!    arXiv id, URL, MR and Zbl numbers. A field not found is left out, never guessed; an entry
//!    whose authors, title and year are all missing says it is unparsed and keeps its text.
//! 4. *Citations* ([`cites`]): a bracket in running text whose parts are entry labels (`[12]`,
//!    `[1, 3–5]`, `[BKM20]`) with an optional locator after a comma (`[12, Thm. 3.1]`), or, for a
//!    list without labels, a family name (one, two with `and`, or `et al.`) with a year (`Smith
//!    (2020)`, `(Smith, 2020; Jones and Lee, 2019a)`). A bracket set in a mathematics font, in a
//!    display, after `Section`, `Figure`, … or opening a list item is no citation, nor is a bracket
//!    whose parts are not labels of the list. Each citation of a group has the glyphs of its own
//!    label (or names and year) and resolves to its entry: by label, or by first family name and
//!    year (a suffix decides between `2020a` and `2020b`). A key with no entry, or more than one,
//!    gives no target and a diagnostic: a wrong link is worse than none. The keys inside a range
//!    are citations with no glyphs of their own, at most [`MAX_RANGE`] of them.
//!
//! Work is linear in the document's words and lines, with every count capped by the constants;
//! there is no recursion. The measures are scored against TeX's own answer by the bibliography
//! oracle (`crates/render/tests/data/bib/oracle`).

mod cites;
mod entries;
mod fields;
mod region;

#[cfg(test)]
mod tests;

use super::refs::{LineRef, PageWords, Piece};
use super::{Diagnostic, DocumentStructure, PageLayout};

/// Most entries read from one list; the rest are left out with a diagnostic.
pub const MAX_ENTRIES: usize = 20_000;
/// Most lines one entry runs on for; a longer one is cut with a diagnostic.
pub const MAX_ENTRY_LINES: usize = 80;
/// Most citations found in one document.
pub const MAX_CITATIONS: usize = 200_000;
/// Most entries one range (`[1–5]`) cites inside it; a longer range cites its two ends only.
pub const MAX_RANGE: usize = 200;
/// Longest label kept, in bytes (`[BKM+20a]`); a longer bracket is no label.
pub const MAX_LABEL_BYTES: usize = 24;
/// Longest text kept for an entry, in bytes (cut at a character boundary).
pub const MAX_ENTRY_TEXT_BYTES: usize = 8 * 1024;
/// Most authors read from one entry.
pub const MAX_AUTHORS: usize = 200;
/// Longest bracket read as one group of citations, in characters.
pub const MAX_GROUP_CHARS: usize = 400;

/// How an entry's label is set.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum LabelStyle {
    /// `[12]`, `[BKM20]`.
    Bracket,
    /// `12.`
    Dot,
    /// `(12)`
    Paren,
    /// No label: an author-year list.
    #[default]
    None,
}

impl LabelStyle {
    pub fn as_str(self) -> &'static str {
        match self {
            LabelStyle::Bracket => "bracket",
            LabelStyle::Dot => "dot",
            LabelStyle::Paren => "paren",
            LabelStyle::None => "none",
        }
    }
}

/// Where the list is.
#[derive(Clone, Debug, PartialEq)]
pub struct Region {
    /// The heading's glyphs, when it has one.
    pub heading: Option<Piece>,
    /// The heading's text (`References`).
    pub title: Option<String>,
    /// Its first and last line (after the heading).
    pub first: LineRef,
    pub last: LineRef,
    /// How sure the analysis is that this is the list, from 0 to 1.
    pub confidence: f32,
}

/// An author of an entry.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Author {
    /// As printed (`van der Berg`, `Müller-Okafor`).
    pub family: String,
    /// Given names or initials as printed (`A.-L.`, `Bérénice`), possibly empty.
    pub given: String,
}

/// A field read from an entry, and how sure the reading is (0 to 1).
#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    pub value: String,
    pub confidence: f32,
}

/// The fields of an entry: each only where the entry shows it. Shaped so that a BibTeX record can
/// be built from it (`author`, `title`, `journal`/`booktitle`/`publisher`, `year`, `volume`,
/// `number`, `pages`, `doi`, `eprint`, `url`, `mrnumber`, `zbl`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Fields {
    pub authors: Vec<Author>,
    /// How sure the author list is; 0 when there is none.
    pub authors_confidence: f32,
    /// The list ends `et al.`: more authors than are named.
    pub et_al: bool,
    pub title: Option<Field>,
    /// The journal, the book a chapter is in, or a book's publisher.
    pub venue: Option<Field>,
    /// As printed, with a suffix (`2020a`).
    pub year: Option<Field>,
    pub volume: Option<Field>,
    pub number: Option<Field>,
    /// `1–20` with an en dash.
    pub pages: Option<Field>,
    pub doi: Option<Field>,
    pub arxiv: Option<Field>,
    pub url: Option<Field>,
    /// The MR number, digits only.
    pub mr: Option<Field>,
    /// The Zbl number (`1234.56789`).
    pub zbl: Option<Field>,
}

impl Fields {
    /// Each field that is there, by its name: `title`, `venue`, `year`, `volume`, `number`,
    /// `pages`, `doi`, `arxiv`, `url`, `mr`, `zbl`.
    pub fn named(&self) -> impl Iterator<Item = (&'static str, &Field)> {
        [
            ("title", &self.title),
            ("venue", &self.venue),
            ("year", &self.year),
            ("volume", &self.volume),
            ("number", &self.number),
            ("pages", &self.pages),
            ("doi", &self.doi),
            ("arxiv", &self.arxiv),
            ("url", &self.url),
            ("mr", &self.mr),
            ("zbl", &self.zbl),
        ]
        .into_iter()
        .filter_map(|(n, f)| f.as_ref().map(|f| (n, f)))
    }
}

/// An entry of the list.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    /// Its index in [`Bibliography::entries`].
    pub id: usize,
    /// The label as printed, without its brackets or period (`12`, `BKM20`).
    pub label: Option<String>,
    pub label_style: LabelStyle,
    /// Its lines, in order.
    pub lines: Vec<LineRef>,
    /// Its glyphs: one piece per page and column, each with the box of its lines.
    pub pieces: Vec<Piece>,
    /// Its text after the label, lines joined (a word broken at a line end by a hyphen stays
    /// broken; a URL or DOI broken over lines is joined).
    pub text: String,
    pub fields: Fields,
    /// Authors, title or year was read; `false` keeps the text alone.
    pub parsed: bool,
    /// How sure the boundary is, from 0 to 1.
    pub confidence: f32,
}

impl Entry {
    /// The label as the list prints it: `[12]`, `12.`, `(12)`, or none.
    pub fn printed_label(&self) -> Option<String> {
        let l = self.label.as_deref()?;
        Some(match self.label_style {
            LabelStyle::Bracket | LabelStyle::None => format!("[{l}]"),
            LabelStyle::Dot => format!("{l}."),
            LabelStyle::Paren => format!("({l})"),
        })
    }
}

/// How a citation names its entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CiteKind {
    /// A number in brackets: `[12]`.
    Numeric,
    /// A label of letters and digits in brackets: `[BKM20]`.
    Alpha,
    /// A family name and a year: `Smith (2020)`, `(Smith, 2020)`.
    AuthorYear,
}

impl CiteKind {
    pub fn as_str(self) -> &'static str {
        match self {
            CiteKind::Numeric => "numeric",
            CiteKind::Alpha => "alpha",
            CiteKind::AuthorYear => "author_year",
        }
    }
}

/// Why a citation has no entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CiteUnresolved {
    /// No entry has its label, or its first author and year.
    NoEntry,
    /// More than one has.
    Ambiguous,
}

impl CiteUnresolved {
    pub fn as_str(self) -> &'static str {
        match self {
            CiteUnresolved::NoEntry => "no_entry",
            CiteUnresolved::Ambiguous => "ambiguous",
        }
    }
}

/// A place where the text cites an entry.
#[derive(Clone, Debug, PartialEq)]
pub struct Citation {
    pub kind: CiteKind,
    /// What names the entry, as printed: `12`, `BKM20`, `Smith and Jones 2020a`.
    pub text: String,
    /// What follows it in its bracket that is not a label (`Thm. 3.1`).
    pub locator: Option<String>,
    /// The clickable glyphs: the label, or the names and the year. None for a key inside a range.
    pub pieces: Vec<Piece>,
    /// Citations of one bracket or parenthesis share a group number.
    pub group: usize,
    /// The entry (index into [`Bibliography::entries`]).
    pub target: Option<usize>,
    pub unresolved: Option<CiteUnresolved>,
    /// A key inside a range (`2` of `[1–3]`), cited without glyphs of its own.
    pub in_range: bool,
    /// How sure the target is, from 0 to 1 (0 with none).
    pub confidence: f32,
}

impl Citation {
    /// The page it is on (the first piece's), if it has glyphs.
    pub fn page(&self) -> Option<usize> {
        self.pieces.first().map(|p| p.page)
    }
}

/// The bibliography of a document and the citations of it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Bibliography {
    pub region: Option<Region>,
    pub entries: Vec<Entry>,
    /// In reading order.
    pub citations: Vec<Citation>,
    /// `no_bibliography`, `no_heading` (a list found by its labels alone), `entries_capped`,
    /// `entry_cut`, `entry_unparsed`, `citations_capped`, `range_capped`, `citation_unresolved`,
    /// `citation_ambiguous`, `labels_out_of_order`.
    pub diagnostics: Vec<Diagnostic>,
}

/// The bibliography of a document and its citations, from its pages' layouts and words (the same
/// pages, in order) and its section tree ([`super::analyse_document`]). Never fails.
pub fn analyse_bibliography(layouts: &[PageLayout], words: &[PageWords], structure: &DocumentStructure) -> Bibliography {
    let mut out = Bibliography::default();
    let Some(region) = region::find(layouts, words, structure, &mut out.diagnostics) else { return out };
    out.entries = entries::split(words, &region, &mut out.diagnostics);
    for e in &mut out.entries {
        e.fields = fields::parse(words, e);
        e.parsed = !e.fields.authors.is_empty() || e.fields.title.is_some() || e.fields.year.is_some();
    }
    let unparsed = out.entries.iter().filter(|e| !e.parsed).count();
    if unparsed > 0 {
        out.diagnostics.push(Diagnostic {
            code: "entry_unparsed",
            message: format!("{unparsed} entries read as text only: no authors, title or year was found"),
            block: None,
        });
    }
    out.citations = cites::find(words, &region, &out.entries, &mut out.diagnostics);
    out.region = Some(region);
    out
}
