//! TeX as the oracle for bibliographies and citations: which glyphs make up each entry of a
//! paper's reference list (with its key and printed label), which glyphs of the text make up each
//! citation of an entry ("[12]", the `12` of "[1, 12]", "Smith and Jones" and "2020" of "Smith and
//! Jones (2020)"), and which entry each citation names. What each entry's fields are (authors,
//! title, year, DOI, …) is the `.bib` file's, which the generator wrote.
//!
//! The method is the layout oracle's ([`super::layout`]), on top of it: the document is compiled to
//! DVI with brackets, `dvitype` lists every glyph and rule with the brackets in shipping order, and
//! the layout truth is built as before. The brackets are `\special{pdfcraft:layout:x:…}`, which the
//! layout truth passes over:
//!
//! - `x:bib:head` at the start of the paragraph after `\begin{thebibliography}`: the list's heading.
//! - `x:bib:begin:KEY` at the start of the paragraph an entry's `\bibitem` opens (from the
//!   paragraph hook, before the label's box that `\everypar` puts there), and `x:bib:end` at that
//!   paragraph's end. The kernel's `\@bibitem` and `\@lbibitem` (or natbib's and hyperref's, at the
//!   start of the document) are wrapped to say which key comes next; nothing is typeset there.
//! - `x:cite:begin:N:KEY` and `x:cite:end` around the part of a citation that is its link:
//!   the kernel's `\@cite@ofmt` box for `\cite` (the label alone, not the locator of `\cite[Thm.
//!   2]{k}`), natbib's `\hyper@natlinkstart`/`\hyper@natlinkend` for `\citep`, `\citet` and
//!   `\citeauthor` (where hyperref would put its link), and `x:cite:resume:N` where natbib breaks
//!   one link in two (`Smith` and `2020` of `Smith (2020)`). `N` counts citations through the
//!   document, one per key: the generator's [`CiteSpec`]s say what each is.
//! - `x:trap:begin:ID`, `x:trap:end` around text that looks like a citation and is none (an
//!   interval `[a, b]`, `[x]`, "Section [3]"), from the source.
//!
//! Each PDF is compiled again with the package reduced to the same source macros without specials
//! and must come out byte-identical (`cargo xtask bib-oracle generate`).

use super::layout::{Kind, LayoutPage, LayoutTruth};
use super::{DviPage, TexError};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;

/// A found entry is the detection of a truth entry when their glyph sets agree with an F1 at least
/// this high.
pub const DETECT_F1: f64 = 0.5;
/// A found entry's boundary is right when its glyphs agree with the truth's at an F1 at least this
/// high (a wrapped DOI cut in two, or an entry merged with its neighbour, falls below).
pub const ENTRY_F1: f64 = 0.95;
/// A found citation is a truth citation when their clickable glyphs agree with an F1 at least this
/// high.
pub const CITE_F1: f64 = 0.8;
/// A found title is right when its tokens agree with the truth's at an F1 at least this high.
pub const TITLE_F1: f64 = 0.8;
/// Longest key, label or field value read from a bracket or a truth row.
pub const MAX_VALUE_BYTES: usize = 1024;

/// The LaTeX package `pdfcraftbib` (load it last; it loads `pdfcraftlayout`). With `specials`
/// false it defines the source macros so that they typeset the same and say nothing.
pub fn package(specials: bool) -> String {
    let head = "\\NeedsTeXFormat{LaTeX2e}\n\\ProvidesPackage{pdfcraftbib}[2026/11/18 PdfCraft bibliography oracle brackets]\n\\RequirePackage{pdfcraftlayout}\n";
    // A range of numeric citations, `[1--4]`, from the labels of its first and last key, each
    // bracketed as a citation. The keys are cited (`\nocite`) so BibTeX lists them.
    let range = |x: bool| {
        let (b1, e1, b2, e2) = if x {
            (
                "\\global\\advance\\pdfcraft@citeno\\@ne\\pdfcraft@x{cite:begin:\\the\\pdfcraft@citeno:#1}",
                "\\pdfcraft@x{cite:end}",
                "\\global\\advance\\pdfcraft@citeno\\@ne\\pdfcraft@x{cite:begin:\\the\\pdfcraft@citeno:#2}",
                "\\pdfcraft@x{cite:end}",
            )
        } else {
            ("", "", "", "")
        };
        format!(
            "\\newcommand*\\pdfcraftrange[2]{{\\nocite{{#1,#2}}\\leavevmode[{{{b1}\\hbox{{\\csname b@#1\\endcsname}}{e1}--{b2}\\hbox{{\\csname b@#2\\endcsname}}{e2}}}]}}\n"
        )
    };
    if !specials {
        return format!(
            "{head}% The brackets expand to nothing: the PDF must come out byte-identical with them.\n\
             \\newcommand*\\pdfcrafttrap[1]{{}}\n\\newcommand*\\pdfcrafttrapend{{}}\n\\newcommand*\\pdfcrafthand[2]{{#2}}\n{}",
            range(false)
        );
    }
    format!(
        "{head}\\def\\pdfcraft@x#1{{\\special{{pdfcraft:layout:x:#1}}}}\n\
         \\newcount\\pdfcraft@citeno\n\
         \\global\\let\\pdfcraft@bibnext\\@empty\n\
         \\global\\let\\pdfcraft@bibopen\\@empty\n\
         \\global\\let\\pdfcraft@headnext\\@empty\n\
         % The list's heading is the first paragraph in it; an entry is the paragraph its \\bibitem opens.\n\
         \\AddToHook{{para/begin}}{{\\ifx\\pdfcraft@headnext\\@empty\\else\\pdfcraft@x{{bib:head}}\\global\\let\\pdfcraft@headnext\\@empty\\fi\n\
         \\ifx\\pdfcraft@bibnext\\@empty\\else\\pdfcraft@x{{bib:begin:\\pdfcraft@bibnext}}\\global\\let\\pdfcraft@bibnext\\@empty\\gdef\\pdfcraft@bibopen{{1}}\\fi}}\n\
         \\AddToHook{{para/end}}{{\\ifx\\pdfcraft@bibopen\\@empty\\else\\pdfcraft@x{{bib:end}}\\global\\let\\pdfcraft@bibopen\\@empty\\fi}}\n\
         \\AtBeginEnvironment{{thebibliography}}{{\\gdef\\pdfcraft@headnext{{1}}}}\n\
         \\def\\pdfcraft@bibitem#1{{\\gdef\\pdfcraft@bibnext{{#1}}\\pdfcraft@obibitem{{#1}}}}\n\
         \\def\\pdfcraft@lbibitem[#1]#2{{\\gdef\\pdfcraft@bibnext{{#2}}\\pdfcraft@olbibitem[#1]{{#2}}}}\n\
         % \\cite: the box of each key's label.\n\
         \\def\\pdfcraft@citefmt#1{{\\global\\advance\\pdfcraft@citeno\\@ne\\pdfcraft@x{{cite:begin:\\the\\pdfcraft@citeno:\\@citeb}}\\pdfcraft@ofmt{{#1}}\\pdfcraft@x{{cite:end}}}}\n\
         % natbib: where hyperref puts its links.\n\
         \\def\\pdfcraft@nls#1{{\\global\\advance\\pdfcraft@citeno\\@ne\\pdfcraft@x{{cite:begin:\\the\\pdfcraft@citeno:#1}}\\pdfcraft@onls{{#1}}}}\n\
         \\def\\pdfcraft@nle{{\\pdfcraft@onle\\pdfcraft@x{{cite:end}}}}\n\
         \\def\\pdfcraft@nlb#1#2{{\\pdfcraft@x{{cite:end}}\\pdfcraft@onlb{{#1}}{{#2}}\\pdfcraft@x{{cite:resume:\\the\\pdfcraft@citeno}}}}\n\
         \\AtBeginDocument{{%\n\
         \\let\\pdfcraft@obibitem\\@bibitem\\let\\@bibitem\\pdfcraft@bibitem\n\
         \\let\\pdfcraft@olbibitem\\@lbibitem\\let\\@lbibitem\\pdfcraft@lbibitem\n\
         \\@ifpackageloaded{{natbib}}{{%\n\
         \\let\\pdfcraft@onls\\hyper@natlinkstart\\let\\hyper@natlinkstart\\pdfcraft@nls\n\
         \\let\\pdfcraft@onle\\hyper@natlinkend\\let\\hyper@natlinkend\\pdfcraft@nle\n\
         \\let\\pdfcraft@onlb\\hyper@natlinkbreak\\let\\hyper@natlinkbreak\\pdfcraft@nlb}}%\n\
         {{\\let\\pdfcraft@ofmt\\@cite@ofmt\\let\\@cite@ofmt\\pdfcraft@citefmt}}}}\n\
         \\newcommand*\\pdfcrafttrap[1]{{\\pdfcraft@x{{trap:begin:#1}}}}\n\
         \\newcommand*\\pdfcrafttrapend{{\\pdfcraft@x{{trap:end}}}}\n\
         % A citation typed by hand: its key and its text.\n\
         \\newcommand*\\pdfcrafthand[2]{{\\global\\advance\\pdfcraft@citeno\\@ne\\pdfcraft@x{{cite:begin:\\the\\pdfcraft@citeno:#1}}#2\\pdfcraft@x{{cite:end}}}}\n{}",
        range(true)
    )
}

/// One entry of a `.bbl` file (or a hand-written list) as the generator read it.
#[derive(Clone, Debug, PartialEq)]
pub struct EntrySpec {
    pub key: String,
    /// The label as printed, without its brackets or period (`12`, `BKM20`); `None` where the
    /// style prints none (author-year lists).
    pub label: Option<String>,
    /// The fields as the `.bib` file (or the hand-written list) gives them, each only where the
    /// entry prints it.
    pub fields: FieldsTruth,
}

/// An entry's fields, as truth. Values are the source's (TeX accents and braces left in); scoring
/// compares them after [`fold`]. A field is `None` (an author list empty) where the printed entry
/// does not show it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FieldsTruth {
    /// Family names, in order.
    pub authors: Vec<String>,
    pub title: Option<String>,
    /// The journal, the book a chapter is in, or the publisher of a book.
    pub venue: Option<String>,
    /// As printed, with a suffix (`2020a`) where the style adds one.
    pub year: Option<String>,
    pub volume: Option<String>,
    pub number: Option<String>,
    pub pages: Option<String>,
    pub doi: Option<String>,
    pub arxiv: Option<String>,
    pub url: Option<String>,
    pub mr: Option<String>,
    pub zbl: Option<String>,
}

/// The names of the fields scored, in report order.
pub const FIELDS: [&str; 12] = ["authors", "title", "venue", "year", "volume", "number", "pages", "doi", "arxiv", "url", "mr", "zbl"];

impl FieldsTruth {
    /// The value of field `name` ([`FIELDS`]); authors joined by `|`.
    pub fn get(&self, name: &str) -> Option<String> {
        match name {
            "authors" => (!self.authors.is_empty()).then(|| self.authors.join("|")),
            "title" => self.title.clone(),
            "venue" => self.venue.clone(),
            "year" => self.year.clone(),
            "volume" => self.volume.clone(),
            "number" => self.number.clone(),
            "pages" => self.pages.clone(),
            "doi" => self.doi.clone(),
            "arxiv" => self.arxiv.clone(),
            "url" => self.url.clone(),
            "mr" => self.mr.clone(),
            "zbl" => self.zbl.clone(),
            _ => None,
        }
    }

    fn set(&mut self, name: &str, value: &str) -> bool {
        let v = Some(value.to_owned());
        match name {
            "authors" => self.authors = value.split('|').filter(|s| !s.is_empty()).map(str::to_owned).collect(),
            "title" => self.title = v,
            "venue" => self.venue = v,
            "year" => self.year = v,
            "volume" => self.volume = v,
            "number" => self.number = v,
            "pages" => self.pages = v,
            "doi" => self.doi = v,
            "arxiv" => self.arxiv = v,
            "url" => self.url = v,
            "mr" => self.mr = v,
            "zbl" => self.zbl = v,
            _ => return false,
        }
        true
    }
}

/// A citation as the generator wrote it.
#[derive(Clone, Debug, PartialEq)]
pub struct CiteSpec {
    /// The citation's number in the document (`N` of its bracket), from 1; `None` for a key inside
    /// a range (`2` and `3` of `[1--4]`), which has no glyphs of its own.
    pub serial: Option<usize>,
    pub key: String,
    /// How it is written: `cite`, `cite_locator`, `cite_group`, `range`, `range_inner`, `citep`,
    /// `citet`, `citeauthor`, `citep_locator`, `citep_group`, `hand`.
    pub form: String,
}

/// Text that looks like a citation and is none.
#[derive(Clone, Debug, PartialEq)]
pub struct TrapSpec {
    pub id: String,
    /// `interval`, `class`, `commutator`, `section`, `display`, …
    pub kind: String,
}

/// Items of a document: per page (from 0) the indices into that page's [`LayoutPage::items`].
pub type Items = Vec<(usize, Vec<usize>)>;

/// What kind of paper a document is beyond its layout, for grouping scores.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BibPaper {
    pub doc: String,
    /// The BibTeX style (`plain`, `alpha`, `abbrv`, `unsrt`, `amsplain`, `apalike`, `plainnat`) or
    /// `hand` for a list written by hand.
    pub style: String,
    /// `numeric`, `alpha` or `authoryear`.
    pub mode: String,
    pub hyperref: bool,
}

/// An entry of the truth.
#[derive(Clone, Debug, PartialEq)]
pub struct EntryTruth {
    pub doc: String,
    pub spec: EntrySpec,
    /// Its place in the list, from 0.
    pub order: usize,
    pub items: Items,
}

/// A citation of the truth.
#[derive(Clone, Debug, PartialEq)]
pub struct CiteTruth {
    pub doc: String,
    pub spec: CiteSpec,
    pub items: Items,
}

/// A trap of the truth.
#[derive(Clone, Debug, PartialEq)]
pub struct TrapTruth {
    pub doc: String,
    pub spec: TrapSpec,
    pub items: Items,
}

/// A bibliography truth file: the layout truth of the papers, and their entries, citations and
/// traps.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BibTruth {
    pub layout: LayoutTruth,
    pub papers: Vec<BibPaper>,
    /// Per document, the items of the list's heading.
    pub headings: Vec<(String, Items)>,
    pub entries: Vec<EntryTruth>,
    pub cites: Vec<CiteTruth>,
    pub traps: Vec<TrapTruth>,
}

fn bad(what: impl std::fmt::Display) -> TexError {
    TexError::Listing(format!("bibliography truth: {what}"))
}

fn push_item(items: &mut Items, page: usize, item: usize) {
    match items.last_mut() {
        Some((p, list)) if *p == page => list.push(item),
        _ => items.push((page, vec![item])),
    }
}

/// The layout block kinds an entry's or a citation's glyphs may be in (not a running head or a
/// footnote shipped meanwhile).
fn text_kind(kind: Kind) -> bool {
    matches!(kind, Kind::Paragraph | Kind::ListItem | Kind::Other | Kind::Abstract)
}

/// What [`build`] gives: a document's heading items, entries, citations and traps.
pub type DocTruth = (Items, Vec<EntryTruth>, Vec<CiteTruth>, Vec<TrapTruth>);

/// The entries, citations and traps of document `doc` from its DVI pages and its layout truth
/// (`layout`, built from the same pages by [`super::layout::build`]). `entries` are in list order.
///
/// Fails on brackets that do not follow (one opened inside another of its kind, one never closed,
/// a key or number the specs do not know, a citation whose key is not the spec's), on an entry or a
/// citation with no glyphs, and on a list with no heading.
pub fn build(
    doc: &str,
    pages: &[DviPage],
    layout: &[LayoutPage],
    entries: &[EntrySpec],
    cites: &[CiteSpec],
    traps: &[TrapSpec],
) -> Result<DocTruth, TexError> {
    let mut et: Vec<EntryTruth> =
        entries.iter().enumerate().map(|(order, e)| EntryTruth { doc: doc.to_owned(), spec: e.clone(), order, items: Vec::new() }).collect();
    let mut ct: Vec<CiteTruth> = cites.iter().map(|c| CiteTruth { doc: doc.to_owned(), spec: c.clone(), items: Vec::new() }).collect();
    let mut tt: Vec<TrapTruth> = traps.iter().map(|t| TrapTruth { doc: doc.to_owned(), spec: t.clone(), items: Vec::new() }).collect();
    let mut seen_entry = vec![false; et.len()];
    let mut seen_cite = vec![false; ct.len()];
    let mut open_entry: Option<usize> = None;
    let mut open_cite: Option<usize> = None;
    let mut open_trap: Option<usize> = None;
    let mut head_next = false;
    let mut head: Option<(usize, usize)> = None;
    for (pi, page) in pages.iter().enumerate() {
        let lp = layout.get(pi).ok_or_else(|| bad(format!("{doc}: page {pi} has no layout truth")))?;
        let mut block_of = vec![usize::MAX; page.items.len()];
        for (bi, b) in lp.blocks.iter().enumerate() {
            for &i in &b.items {
                if let Some(slot) = block_of.get_mut(i) {
                    *slot = bi;
                }
            }
        }
        let mut events = page.events.iter().peekable();
        for i in 0..=page.items.len() {
            while let Some((_, text)) = events.next_if(|(at, _)| *at <= i) {
                let Some(x) = text.strip_prefix("x:") else { continue };
                let (what, rest) = x.split_once(':').unwrap_or((x, ""));
                match (what, rest) {
                    ("bib", "head") => head_next = true,
                    ("bib", "end") => {
                        if open_entry.take().is_none() {
                            return Err(bad(format!("{doc} page {pi}: an entry ends that never began")));
                        }
                    }
                    ("bib", rest) => {
                        let Some(key) = rest.strip_prefix("begin:") else { return Err(bad(format!("{doc}: unknown bracket `{text}`"))) };
                        if open_entry.is_some() {
                            return Err(bad(format!("{doc} page {pi}: entry `{key}` begins inside another")));
                        }
                        let k = et.iter().position(|e| e.spec.key == key).ok_or_else(|| bad(format!("{doc}: unknown entry `{key}`")))?;
                        if std::mem::replace(&mut seen_entry[k], true) {
                            return Err(bad(format!("{doc}: entry `{key}` begins twice")));
                        }
                        open_entry = Some(k);
                    }
                    ("cite", "end") => {
                        if open_cite.take().is_none() {
                            return Err(bad(format!("{doc} page {pi}: a citation ends that never began")));
                        }
                    }
                    ("cite", rest) => {
                        if open_cite.is_some() {
                            return Err(bad(format!("{doc} page {pi}: citation `{rest}` begins inside another")));
                        }
                        let (serial, key) = if let Some(r) = rest.strip_prefix("begin:") {
                            let (n, key) = r.split_once(':').ok_or_else(|| bad(format!("{doc}: bad bracket `{text}`")))?;
                            (n, Some(key))
                        } else if let Some(n) = rest.strip_prefix("resume:") {
                            (n, None)
                        } else {
                            return Err(bad(format!("{doc}: unknown bracket `{text}`")));
                        };
                        let serial: usize = serial.parse().map_err(|_| bad(format!("{doc}: bad citation number in `{text}`")))?;
                        let k = ct
                            .iter()
                            .position(|c| c.spec.serial == Some(serial))
                            .ok_or_else(|| bad(format!("{doc}: citation {serial} is not one the generator wrote")))?;
                        if let Some(key) = key {
                            if ct.get(k).is_some_and(|c| c.spec.key != key) {
                                return Err(bad(format!("{doc}: citation {serial} cites `{key}`, the generator wrote another key")));
                            }
                            if std::mem::replace(&mut seen_cite[k], true) {
                                return Err(bad(format!("{doc}: citation {serial} begins twice")));
                            }
                        } else if !seen_cite.get(k).copied().unwrap_or(false) {
                            return Err(bad(format!("{doc}: citation {serial} resumes before it began")));
                        }
                        open_cite = Some(k);
                    }
                    ("trap", "end") => {
                        if open_trap.take().is_none() {
                            return Err(bad(format!("{doc} page {pi}: a trap ends that never began")));
                        }
                    }
                    ("trap", rest) => {
                        let Some(id) = rest.strip_prefix("begin:") else { return Err(bad(format!("{doc}: unknown bracket `{text}`"))) };
                        if open_trap.is_some() {
                            return Err(bad(format!("{doc} page {pi}: trap `{id}` begins inside another")));
                        }
                        open_trap = Some(tt.iter().position(|t| t.spec.id == id).ok_or_else(|| bad(format!("{doc}: unknown trap `{id}`")))?);
                    }
                    // The statement oracle's brackets, if a paper uses them, are not ours.
                    _ => {}
                }
            }
            if i == page.items.len() {
                break;
            }
            if head_next {
                head.get_or_insert((pi, i));
                head_next = false;
            }
            let kind = lp.blocks.get(block_of.get(i).copied().unwrap_or(usize::MAX)).map(|b| b.kind);
            // A trap may be a display; entries and citations are text.
            if let Some(t) = open_trap.and_then(|k| tt.get_mut(k))
                && kind.is_some_and(|k| text_kind(k) || k == Kind::Display)
            {
                push_item(&mut t.items, pi, i);
            }
            if !kind.is_some_and(text_kind) {
                continue;
            }
            if let Some(e) = open_entry.and_then(|k| et.get_mut(k)) {
                push_item(&mut e.items, pi, i);
            }
            if let Some(c) = open_cite.and_then(|k| ct.get_mut(k)) {
                push_item(&mut c.items, pi, i);
            }
        }
    }
    if open_cite.is_some() || open_trap.is_some() {
        return Err(bad(format!("{doc}: a citation or trap never ends")));
    }
    if let Some(e) = et.iter().zip(&seen_entry).find(|(_, s)| !**s) {
        return Err(bad(format!("{doc}: entry `{}` is never bracketed", e.0.spec.key)));
    }
    if let Some(e) = et.iter().find(|e| e.items.is_empty()) {
        return Err(bad(format!("{doc}: entry `{}` has no glyphs", e.spec.key)));
    }
    if let Some((c, _)) = ct.iter().zip(&seen_cite).find(|(c, seen)| c.spec.serial.is_some() && !**seen) {
        return Err(bad(format!("{doc}: citation {:?} of `{}` is never bracketed", c.spec.serial, c.spec.key)));
    }
    if let Some(c) = ct.iter().find(|c| c.spec.serial.is_some() && c.items.is_empty()) {
        return Err(bad(format!("{doc}: citation {:?} of `{}` has no glyphs", c.spec.serial, c.spec.key)));
    }
    if let Some(t) = tt.iter().find(|t| t.items.is_empty()) {
        return Err(bad(format!("{doc}: trap `{}` has no glyphs", t.spec.id)));
    }
    for c in &ct {
        if !et.iter().any(|e| e.spec.key == c.spec.key) {
            return Err(bad(format!("{doc}: citation of `{}`, which the list does not hold", c.spec.key)));
        }
    }
    let heading = match head {
        Some((p, i)) => {
            let lp = layout.get(p).ok_or_else(|| bad(format!("{doc}: no layout page {p}")))?;
            let b = lp.blocks.iter().find(|b| b.items.contains(&i)).ok_or_else(|| bad(format!("{doc}: the heading is in no block")))?;
            vec![(p, b.items.clone())]
        }
        None if et.is_empty() => Vec::new(),
        None => return Err(bad(format!("{doc}: the list has no heading"))),
    };
    Ok((heading, et, ct, tt))
}

fn ranges(list: &[usize]) -> String {
    let mut sorted = list.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    let mut out = String::new();
    let mut i = 0;
    while let Some(&a) = sorted.get(i) {
        let mut j = i;
        while sorted.get(j.saturating_add(1)).is_some_and(|&n| Some(n) == sorted.get(j).map(|v| v.saturating_add(1))) {
            j = j.saturating_add(1);
        }
        let b = sorted.get(j).copied().unwrap_or(a);
        if !out.is_empty() {
            out.push(',');
        }
        if a == b {
            let _ = write!(out, "{a}");
        } else {
            let _ = write!(out, "{a}-{b}");
        }
        i = j.saturating_add(1);
    }
    out
}

/// Most items one range of a truth file may stand for.
const MAX_RANGE: usize = 200_000;

fn parse_ranges(s: &str, line: usize) -> Result<Vec<usize>, TexError> {
    let mut out = Vec::new();
    for part in s.split(',').filter(|p| !p.is_empty()) {
        let (a, b) = part.split_once('-').unwrap_or((part, part));
        let a: usize = a.parse().map_err(|_| bad(format!("line {line}: `{part}` is not a range")))?;
        let b: usize = b.parse().map_err(|_| bad(format!("line {line}: `{part}` is not a range")))?;
        if b < a || b - a > MAX_RANGE || out.len() > MAX_RANGE {
            return Err(bad(format!("line {line}: range `{part}` is out of bounds")));
        }
        out.extend(a..=b);
    }
    Ok(out)
}

fn dash(v: Option<&str>) -> String {
    match v {
        Some(s) if !s.is_empty() => s.replace(['\t', '\n', '\r'], " "),
        _ => "-".to_owned(),
    }
}

fn undash(v: &str) -> Option<String> {
    (v != "-").then(|| v.to_owned())
}

impl BibTruth {
    /// The file [`BibTruth::parse`] reads: the layout truth's rows ([`LayoutTruth::to_tsv`]), then
    ///
    /// ```text
    /// Q  doc  style  mode  hyperref        a paper
    /// N  doc  key  order  label            an entry
    /// F  doc  key  field  value            a field of an entry (authors: family names joined by |)
    /// C  doc  id  serial  key  form        a citation (id c1, c2, …; serial - inside a range)
    /// P  doc  id  kind                     a trap
    /// I  doc  id  page  ranges             items of an entry (id `e:KEY`), citation, trap, or the heading (`head`)
    /// ```
    ///
    /// `-` is none.
    pub fn to_tsv(&self, header: &str) -> String {
        let mut s = self.layout.to_tsv(header);
        let items = |s: &mut String, doc: &str, id: &str, list: &Items| {
            for (page, l) in list {
                let _ = writeln!(s, "I\t{doc}\t{id}\t{page}\t{}", ranges(l));
            }
        };
        for p in &self.papers {
            let _ = writeln!(s, "Q\t{}\t{}\t{}\t{}", p.doc, p.style, p.mode, u8::from(p.hyperref));
        }
        for (doc, list) in &self.headings {
            items(&mut s, doc, "head", list);
        }
        for e in &self.entries {
            let _ = writeln!(s, "N\t{}\t{}\t{}\t{}", e.doc, e.spec.key, e.order, dash(e.spec.label.as_deref()));
            for f in FIELDS {
                if let Some(v) = e.spec.fields.get(f) {
                    let _ = writeln!(s, "F\t{}\t{}\t{f}\t{}", e.doc, e.spec.key, dash(Some(&v)));
                }
            }
            items(&mut s, &e.doc, &format!("e:{}", e.spec.key), &e.items);
        }
        for (i, c) in self.cites.iter().enumerate() {
            let id = format!("c{i}");
            let serial = c.spec.serial.map(|n| n.to_string());
            let _ = writeln!(s, "C\t{}\t{id}\t{}\t{}\t{}", c.doc, dash(serial.as_deref()), c.spec.key, c.spec.form);
            items(&mut s, &c.doc, &id, &c.items);
        }
        for t in &self.traps {
            let _ = writeln!(s, "P\t{}\tt:{}\t{}", t.doc, t.spec.id, t.spec.kind);
            items(&mut s, &t.doc, &format!("t:{}", t.spec.id), &t.items);
        }
        s
    }

    pub fn parse(tsv: &str) -> Result<BibTruth, TexError> {
        let mut layout_rows = String::new();
        let mut out = BibTruth::default();
        #[derive(Clone, Copy)]
        enum Slot {
            Entry(usize),
            Cite(usize),
            Trap(usize),
        }
        let mut slots: HashMap<(String, String), Slot> = HashMap::new();
        for (n, line) in tsv.lines().enumerate() {
            let line_no = n.saturating_add(1);
            if line_no > super::layout::MAX_ROWS {
                return Err(bad(format!("more than {} rows", super::layout::MAX_ROWS)));
            }
            let f: Vec<&str> = line.split('\t').collect();
            let get = |i: usize| {
                let v = f.get(i).copied().ok_or_else(|| bad(format!("line {line_no}: column {} is missing", i.saturating_add(1))))?;
                if v.len() > MAX_VALUE_BYTES {
                    return Err(bad(format!("line {line_no}: a value is longer than {MAX_VALUE_BYTES} bytes")));
                }
                Ok(v)
            };
            match f.first().copied() {
                Some("Q") => out.papers.push(BibPaper {
                    doc: get(1)?.to_owned(),
                    style: get(2)?.to_owned(),
                    mode: get(3)?.to_owned(),
                    hyperref: get(4)? == "1",
                }),
                Some("N") => {
                    let doc = get(1)?.to_owned();
                    let key = get(2)?.to_owned();
                    let order: usize = get(3)?.parse().map_err(|_| bad(format!("line {line_no}: bad order")))?;
                    if slots.insert((doc.clone(), format!("e:{key}")), Slot::Entry(out.entries.len())).is_some() {
                        return Err(bad(format!("line {line_no}: entry `{key}` is listed twice")));
                    }
                    let spec = EntrySpec { key, label: undash(get(4)?), fields: FieldsTruth::default() };
                    out.entries.push(EntryTruth { doc, spec, order, items: Vec::new() });
                }
                Some("F") => {
                    let doc = get(1)?;
                    let key = format!("e:{}", get(2)?);
                    let Some(Slot::Entry(k)) = slots.get(&(doc.to_owned(), key)).copied() else {
                        return Err(bad(format!("line {line_no}: a field of an entry not listed before it")));
                    };
                    let (name, value) = (get(3)?, get(4)?);
                    let e = out.entries.get_mut(k).ok_or_else(|| bad(format!("line {line_no}: no entry")))?;
                    if !e.spec.fields.set(name, value) {
                        return Err(bad(format!("line {line_no}: unknown field `{name}`")));
                    }
                }
                Some("C") => {
                    let doc = get(1)?.to_owned();
                    let id = get(2)?.to_owned();
                    let serial = match get(3)? {
                        "-" => None,
                        n => Some(n.parse().map_err(|_| bad(format!("line {line_no}: bad serial")))?),
                    };
                    if slots.insert((doc.clone(), id.clone()), Slot::Cite(out.cites.len())).is_some() {
                        return Err(bad(format!("line {line_no}: `{id}` is listed twice")));
                    }
                    out.cites.push(CiteTruth {
                        doc,
                        spec: CiteSpec { serial, key: get(4)?.to_owned(), form: get(5)?.to_owned() },
                        items: Vec::new(),
                    });
                }
                Some("P") => {
                    let doc = get(1)?.to_owned();
                    let id = get(2)?.to_owned();
                    if slots.insert((doc.clone(), id.clone()), Slot::Trap(out.traps.len())).is_some() {
                        return Err(bad(format!("line {line_no}: `{id}` is listed twice")));
                    }
                    let spec = TrapSpec { id: id.trim_start_matches("t:").to_owned(), kind: get(3)?.to_owned() };
                    out.traps.push(TrapTruth { doc, spec, items: Vec::new() });
                }
                Some("I") => {
                    let doc = get(1)?.to_owned();
                    let id = get(2)?.to_owned();
                    let page: usize = get(3)?.parse().map_err(|_| bad(format!("line {line_no}: bad page")))?;
                    let list = parse_ranges(get(4)?, line_no)?;
                    if id == "head" {
                        match out.headings.iter_mut().find(|(d, _)| *d == doc) {
                            Some((_, items)) => items.push((page, list)),
                            None => out.headings.push((doc, vec![(page, list)])),
                        }
                        continue;
                    }
                    let slot = slots
                        .get(&(doc.clone(), id.clone()))
                        .copied()
                        .ok_or_else(|| bad(format!("line {line_no}: `{id}` is not listed before its items")))?;
                    let dest = match slot {
                        Slot::Entry(k) => out.entries.get_mut(k).map(|e| &mut e.items),
                        Slot::Cite(k) => out.cites.get_mut(k).map(|c| &mut c.items),
                        Slot::Trap(k) => out.traps.get_mut(k).map(|t| &mut t.items),
                    };
                    dest.ok_or_else(|| bad(format!("line {line_no}: `{id}` takes no items")))?.push((page, list));
                }
                _ => {
                    layout_rows.push_str(line);
                    layout_rows.push('\n');
                }
            }
        }
        out.layout = LayoutTruth::parse(&layout_rows)?;
        Ok(out)
    }

    pub fn paper(&self, doc: &str) -> Option<&BibPaper> {
        self.papers.iter().find(|p| p.doc == doc)
    }
}

// --- Normalisation -------------------------------------------------------------------------------

/// The base letter of a Latin letter with a diacritic, the letters TeX's accents make.
fn base_letter(c: char) -> Option<&'static str> {
    Some(match c {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' => "a",
        'ç' | 'ć' | 'č' | 'ĉ' | 'ċ' => "c",
        'ď' | 'đ' => "d",
        'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ĕ' | 'ė' | 'ę' | 'ě' => "e",
        'ğ' | 'ĝ' | 'ġ' | 'ģ' => "g",
        'ì' | 'í' | 'î' | 'ï' | 'ı' | 'ĩ' | 'ī' | 'į' => "i",
        'ł' | 'ľ' | 'ĺ' | 'ļ' => "l",
        'ñ' | 'ń' | 'ň' | 'ņ' => "n",
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ő' => "o",
        'ŕ' | 'ř' => "r",
        'ś' | 'š' | 'ş' | 'ŝ' => "s",
        'ß' => "ss",
        'ţ' | 'ť' => "t",
        'ù' | 'ú' | 'û' | 'ü' | 'ũ' | 'ū' | 'ů' | 'ű' | 'ų' => "u",
        'ý' | 'ÿ' => "y",
        'ź' | 'ż' | 'ž' => "z",
        'æ' => "ae",
        'œ' => "oe",
        'ﬁ' => "fi",
        'ﬂ' => "fl",
        'ﬀ' => "ff",
        'ﬃ' => "ffi",
        'ﬄ' => "ffl",
        _ => return None,
    })
}

/// A string as scoring compares it: TeX accents and the accents of Unicode letters dropped to the
/// base letter, braces, `$`, `^`, `_` and `~` dropped, `--` and Unicode dashes made `-`, lower
/// case, whitespace collapsed. Used on both sides: the `.bib` value and what was read from the PDF.
pub fn fold(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                // A control word (`\ss`, `\o`, `\textbf`) or an accent (`\'`, `\"`, `\c`).
                let mut name = String::new();
                while let Some(&n) = chars.peek() {
                    if n.is_ascii_alphabetic() {
                        name.push(n);
                        chars.next();
                    } else {
                        break;
                    }
                }
                if name.is_empty() {
                    match chars.next() {
                        Some('&') => out.push('&'),
                        Some('%') => out.push('%'),
                        Some('_') => out.push('_'),
                        Some('#') => out.push('#'),
                        // An accent: its letter follows.
                        _ => {}
                    }
                } else {
                    match name.as_str() {
                        "ss" => out.push_str("ss"),
                        "o" | "O" => out.push('o'),
                        "l" | "L" => out.push('l'),
                        "ae" | "AE" => out.push_str("ae"),
                        "oe" | "OE" => out.push_str("oe"),
                        "aa" | "AA" => out.push('a'),
                        "i" => out.push('i'),
                        "j" => out.push('j'),
                        // `\c c`, `\v s`: an accent named by a letter; the space after it goes.
                        _ => {
                            while chars.peek() == Some(&' ') {
                                chars.next();
                            }
                        }
                    }
                }
            }
            '{' | '}' | '$' | '^' | '_' => {}
            '~' => out.push(' '),
            '–' | '—' | '‐' | '‑' | '−' => out.push('-'),
            // Spacing accents a font draws as glyphs of their own.
            '´' | '`' | '¨' | '˜' | '¸' | 'ˇ' | '˘' | '˙' | '¯' | '˝' | '˚' | '˛' | 'ˆ' => {}
            c if ('\u{300}'..='\u{36F}').contains(&c) => {}
            c => match base_letter(c.to_lowercase().next().unwrap_or(c)) {
                Some(b) => out.push_str(b),
                None => out.extend(c.to_lowercase()),
            },
        }
    }
    let collapsed = out.replace("--", "-").replace("--", "-");
    collapsed.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The words of a string after [`fold`]: runs of letters and digits.
pub fn tokens(s: &str) -> Vec<String> {
    fold(s).split(|c: char| !c.is_alphanumeric()).filter(|t| !t.is_empty()).map(str::to_owned).collect()
}

/// The F1 of two token lists, as multisets.
pub fn token_f1(a: &[String], b: &[String]) -> f64 {
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    let mut pool: HashMap<&str, usize> = HashMap::new();
    for t in b {
        *pool.entry(t.as_str()).or_default() += 1;
    }
    let mut common = 0usize;
    for t in a {
        if let Some(n) = pool.get_mut(t.as_str()).filter(|n| **n > 0) {
            *n -= 1;
            common += 1;
        }
    }
    f1(common, a.len(), b.len())
}

/// Whether a found value of field `name` is the truth's, after [`fold`]: authors as a set of family
/// names, a title by its tokens ([`TITLE_F1`]), a venue by its tokens (half of them), a DOI, an
/// arXiv id, a URL and an MR number without spaces, the rest exactly.
pub fn field_matches(name: &str, truth: &str, found: &str) -> bool {
    match name {
        "authors" => {
            let set =
                |s: &str| -> std::collections::BTreeSet<String> { s.split('|').map(|a| tokens(a).join(" ")).filter(|a| !a.is_empty()).collect() };
            set(truth) == set(found)
        }
        "title" => token_f1(&tokens(truth), &tokens(found)) >= TITLE_F1,
        "venue" => token_f1(&tokens(truth), &tokens(found)) >= 0.5,
        "doi" | "arxiv" | "url" | "mr" | "zbl" => {
            let squash = |s: &str| fold(s).chars().filter(|c| !c.is_whitespace()).collect::<String>().trim_end_matches(['.', ',']).to_owned();
            let (t, f) = (squash(truth), squash(found));
            let strip =
                |s: &str| s.trim_start_matches("arxiv:").trim_start_matches("doi:").trim_start_matches("mr").trim_start_matches("zbl").to_owned();
            let (t, f) = (strip(&t), strip(&f));
            t == f
        }
        _ => fold(truth).replace(' ', "") == fold(found).replace(' ', ""),
    }
}

// --- Scoring -------------------------------------------------------------------------------------

/// An item of a document: (page, index into that page's [`LayoutPage::items`]).
pub type ItemRef = (usize, usize);

fn flat(items: &Items) -> HashSet<ItemRef> {
    items.iter().flat_map(|(p, l)| l.iter().map(move |&i| (*p, i))).collect()
}

fn f1(common: usize, a: usize, b: usize) -> f64 {
    if a + b == 0 { 0.0 } else { 2.0 * common as f64 / (a + b) as f64 }
}

/// What [`pair_up`] gives: per truth set its found one and F1, per found one its truth set, per
/// truth set its best F1.
type Pairing = (Vec<Option<(usize, f64)>>, Vec<Option<usize>>, Vec<f64>);

/// Pairs truth glyph sets with found ones (their items and how many of their glyphs are no item),
/// best F1 first, one to one, at F1 ≥ `least`. Returns per truth set its found one and F1, per found
/// one its truth set, and per truth set its best F1 with any found one.
fn pair_up(sets: &[HashSet<ItemRef>], found: &[(&[ItemRef], usize)], least: f64) -> Pairing {
    let mut owner: HashMap<ItemRef, usize> = HashMap::new();
    for (ti, set) in sets.iter().enumerate() {
        for i in set {
            owner.insert(*i, ti);
        }
    }
    let mut pairs: Vec<(usize, usize, f64)> = Vec::new();
    for (fi, (items, unmatched)) in found.iter().enumerate() {
        let mut common: HashMap<usize, usize> = HashMap::new();
        for i in *items {
            if let Some(&ti) = owner.get(i) {
                *common.entry(ti).or_default() += 1;
            }
        }
        for (ti, c) in common {
            let n = sets.get(ti).map_or(0, HashSet::len);
            pairs.push((ti, fi, f1(c, n, items.len().saturating_add(*unmatched))));
        }
    }
    pairs.sort_by(|a, b| b.2.total_cmp(&a.2).then(a.0.cmp(&b.0)).then(a.1.cmp(&b.1)));
    let mut t_found: Vec<Option<(usize, f64)>> = vec![None; sets.len()];
    let mut f_truth: Vec<Option<usize>> = vec![None; found.len()];
    let mut best: Vec<f64> = vec![0.0; sets.len()];
    for &(ti, _, score) in &pairs {
        if let Some(b) = best.get_mut(ti) {
            *b = b.max(score);
        }
    }
    for (ti, fi, score) in pairs {
        if score < least || t_found.get(ti).is_some_and(Option::is_some) || f_truth.get(fi).is_some_and(Option::is_some) {
            continue;
        }
        if let Some(slot) = t_found.get_mut(ti) {
            *slot = Some((fi, score));
        }
        if let Some(slot) = f_truth.get_mut(fi) {
            *slot = Some(ti);
        }
    }
    (t_found, f_truth, best)
}

/// An entry an analysis found.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FoundEntry {
    /// The truth items its glyphs are.
    pub items: Vec<ItemRef>,
    /// Its glyphs that are no item of the truth.
    pub unmatched: usize,
    /// Its printed label, without brackets or period.
    pub label: Option<String>,
    /// Field name ([`FIELDS`]) to value; authors' family names joined by `|`.
    pub fields: BTreeMap<String, String>,
}

/// A citation an analysis found.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FoundCite {
    pub items: Vec<ItemRef>,
    pub unmatched: usize,
    /// The found entry it names (index into the found entries).
    pub target: Option<usize>,
}

/// What an analysis found in one document.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FoundBib {
    /// Whether it found a bibliography, and the items of its region.
    pub region: Option<Vec<ItemRef>>,
    pub entries: Vec<FoundEntry>,
    pub cites: Vec<FoundCite>,
}

/// Counts for entries.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct EntryCounts {
    pub truth: usize,
    pub found: usize,
    /// Truth entries found (F1 ≥ [`DETECT_F1`]), and of those with the boundary right (F1 ≥
    /// [`ENTRY_F1`]).
    pub detected: usize,
    pub exact: usize,
    /// Found entries that stand for no truth entry.
    pub spurious: usize,
    /// Truth entries with a label, of those detected with the label right, a wrong one, none.
    pub labelled: usize,
    pub label_right: usize,
    pub label_wrong: usize,
    pub label_missing: usize,
    /// Found entries that give a label where the truth has none.
    pub label_spurious: usize,
    /// Sum of each truth entry's best glyph F1, for the mean.
    pub f1_sum: f64,
}

/// Counts for one field.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FieldCounts {
    /// Detected entries whose truth has the field; of those where it was found right, found wrong,
    /// not found.
    pub truth: usize,
    pub right: usize,
    pub wrong: usize,
    pub missing: usize,
    /// Found where the truth (as printed) has none: an invented field.
    pub invented: usize,
}

impl FieldCounts {
    pub fn precision(&self) -> Option<f64> {
        let found = self.right + self.wrong + self.invented;
        (found > 0).then(|| self.right as f64 / found as f64)
    }
    pub fn recall(&self) -> Option<f64> {
        (self.truth > 0).then(|| self.right as f64 / self.truth as f64)
    }
}

/// Counts for citations.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CiteCounts {
    /// Truth citations with glyphs; found with them (F1 ≥ [`CITE_F1`]).
    pub truth: usize,
    pub matched: usize,
    /// Of the matched: the right entry, a WRONG one, none.
    pub right: usize,
    pub wrong: usize,
    pub unlinked: usize,
    /// Keys inside ranges (no glyphs): cited by the found range, cited wrongly, not cited.
    pub inner: usize,
    pub inner_right: usize,
    pub inner_wrong: usize,
}

/// The scores of some documents.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BibTally {
    pub docs: usize,
    /// Documents with a list, of those whose list was found; documents with one found where there
    /// is none.
    pub with_list: usize,
    pub region_found: usize,
    pub region_spurious: usize,
    /// The found regions' glyphs against the truth list's (heading and entries).
    pub region_common: usize,
    pub region_truth: usize,
    pub region_found_items: usize,
    pub entries: EntryCounts,
    pub fields: BTreeMap<String, FieldCounts>,
    /// By citation form (`cite`, `citep`, …), and all under `*`.
    pub cites: BTreeMap<String, CiteCounts>,
    /// Found citations; of those, the ones that stand for no truth citation; of those, linked
    /// (wrong links where the text cites nothing); and those on a trap.
    pub cites_found: usize,
    pub cites_spurious: usize,
    pub spurious_links: usize,
    pub trap_hits: usize,
    pub traps: usize,
}

impl BibTally {
    /// Every wrong link: a matched citation linked to the wrong entry, a key inside a range linked
    /// wrongly, and a link where the text cites nothing.
    pub fn wrong_links(&self) -> usize {
        self.cites.get("*").map_or(0, |c| c.wrong.saturating_add(c.inner_wrong)).saturating_add(self.spurious_links)
    }

    /// Every wrong field: found but not the truth's, or invented.
    pub fn wrong_fields(&self) -> usize {
        self.fields.values().map(|c| c.wrong.saturating_add(c.invented)).sum()
    }

    pub fn add(&mut self, o: &BibTally) {
        self.docs += o.docs;
        self.with_list += o.with_list;
        self.region_found += o.region_found;
        self.region_spurious += o.region_spurious;
        self.region_common += o.region_common;
        self.region_truth += o.region_truth;
        self.region_found_items += o.region_found_items;
        let e = &mut self.entries;
        let x = &o.entries;
        e.truth += x.truth;
        e.found += x.found;
        e.detected += x.detected;
        e.exact += x.exact;
        e.spurious += x.spurious;
        e.labelled += x.labelled;
        e.label_right += x.label_right;
        e.label_wrong += x.label_wrong;
        e.label_missing += x.label_missing;
        e.label_spurious += x.label_spurious;
        e.f1_sum += x.f1_sum;
        for (k, v) in &o.fields {
            let c = self.fields.entry(k.clone()).or_default();
            c.truth += v.truth;
            c.right += v.right;
            c.wrong += v.wrong;
            c.missing += v.missing;
            c.invented += v.invented;
        }
        for (k, v) in &o.cites {
            let c = self.cites.entry(k.clone()).or_default();
            c.truth += v.truth;
            c.matched += v.matched;
            c.right += v.right;
            c.wrong += v.wrong;
            c.unlinked += v.unlinked;
            c.inner += v.inner;
            c.inner_right += v.inner_right;
            c.inner_wrong += v.inner_wrong;
        }
        self.cites_found += o.cites_found;
        self.cites_spurious += o.cites_spurious;
        self.spurious_links += o.spurious_links;
        self.trap_hits += o.trap_hits;
        self.traps += o.traps;
    }

    /// Adds one document: `truth` (all of the truth; the document's rows are picked by `doc`) and
    /// what the analysis found in it.
    pub fn add_document(&mut self, truth: &BibTruth, doc: &str, found: &FoundBib) {
        self.docs += 1;
        let te: Vec<&EntryTruth> = truth.entries.iter().filter(|e| e.doc == doc).collect();
        self.add_region(truth, doc, &te, found);
        let sets: Vec<HashSet<ItemRef>> = te.iter().map(|e| flat(&e.items)).collect();
        let fsets: Vec<(&[ItemRef], usize)> = found.entries.iter().map(|f| (f.items.as_slice(), f.unmatched)).collect();
        let (t_found, f_truth, best) = pair_up(&sets, &fsets, DETECT_F1);
        self.add_entries(&te, found, &t_found, &f_truth, &best);
        let cf_truth = self.add_cites(truth, doc, &te, &f_truth, found);
        self.add_spurious(truth, doc, found, &cf_truth);
    }

    fn add_region(&mut self, truth: &BibTruth, doc: &str, te: &[&EntryTruth], found: &FoundBib) {
        let heading = truth.headings.iter().find(|(d, _)| d == doc).map(|(_, i)| flat(i)).unwrap_or_default();
        // The region.
        if !te.is_empty() {
            self.with_list += 1;
        }
        if let Some(region) = &found.region {
            if te.is_empty() {
                self.region_spurious += 1;
            } else {
                self.region_found += 1;
            }
            let mut all: HashSet<ItemRef> = heading.clone();
            for e in te {
                all.extend(flat(&e.items));
            }
            self.region_common += region.iter().filter(|i| all.contains(i)).count();
            self.region_truth += all.len();
            self.region_found_items += region.len();
        } else if !te.is_empty() {
            let mut all: HashSet<ItemRef> = heading.clone();
            for e in te {
                all.extend(flat(&e.items));
            }
            self.region_truth += all.len();
        }
    }

    fn add_entries(&mut self, te: &[&EntryTruth], found: &FoundBib, t_found: &[Option<(usize, f64)>], f_truth: &[Option<usize>], best: &[f64]) {
        let ec = &mut self.entries;
        ec.truth += te.len();
        ec.found += found.entries.len();
        ec.f1_sum += best.iter().sum::<f64>();
        ec.spurious += f_truth.iter().filter(|t| t.is_none()).count();
        for (ti, t) in te.iter().enumerate() {
            let Some((fi, score)) = t_found.get(ti).copied().flatten() else {
                if t.spec.label.is_some() {
                    self.entries.labelled += 1;
                    self.entries.label_missing += 1;
                }
                continue;
            };
            let Some(f) = found.entries.get(fi) else { continue };
            self.score_entry(t, f, score);
        }
    }

    fn score_entry(&mut self, t: &EntryTruth, f: &FoundEntry, score: f64) {
        let norm_label = |s: &str| fold(s).replace(' ', "");
        let ec = &mut self.entries;
        ec.detected += 1;
        ec.exact += usize::from(score >= ENTRY_F1);
        match (&t.spec.label, &f.label) {
            (Some(want), Some(got)) => {
                ec.labelled += 1;
                if norm_label(want) == norm_label(got) {
                    ec.label_right += 1;
                } else {
                    ec.label_wrong += 1;
                }
            }
            (Some(_), None) => {
                ec.labelled += 1;
                ec.label_missing += 1;
            }
            (None, Some(_)) => ec.label_spurious += 1,
            (None, None) => {}
        }
        for name in FIELDS {
            let want = t.spec.fields.get(name);
            let got = f.fields.get(name).filter(|v| !v.is_empty());
            let c = self.fields.entry(name.to_owned()).or_default();
            match (want, got) {
                (Some(w), Some(g)) => {
                    c.truth += 1;
                    if field_matches(name, &w, g) {
                        c.right += 1;
                    } else {
                        c.wrong += 1;
                    }
                }
                (Some(_), None) => {
                    c.truth += 1;
                    c.missing += 1;
                }
                (None, Some(_)) => c.invented += 1,
                (None, None) => {}
            }
        }
    }

    /// Scores the citations; returns, per found citation, the truth citation it stands for.
    fn add_cites(&mut self, truth: &BibTruth, doc: &str, te: &[&EntryTruth], f_truth: &[Option<usize>], found: &FoundBib) -> Vec<Option<usize>> {
        let tc: Vec<&CiteTruth> = truth.cites.iter().filter(|c| c.doc == doc).collect();
        let key_of_found = |fi: usize| f_truth.get(fi).copied().flatten().and_then(|ti| te.get(ti)).map(|e| e.spec.key.as_str());
        let csets: Vec<HashSet<ItemRef>> = tc.iter().map(|c| flat(&c.items)).collect();
        let fsets: Vec<(&[ItemRef], usize)> = found.cites.iter().map(|f| (f.items.as_slice(), f.unmatched)).collect();
        let (ct_found, cf_truth, _) = pair_up(&csets, &fsets, CITE_F1);
        self.score_cites(&tc, &ct_found, found, &key_of_found);
        cf_truth
    }

    fn score_cites<'a>(
        &mut self,
        tc: &[&CiteTruth],
        ct_found: &[Option<(usize, f64)>],
        found: &FoundBib,
        key_of_found: &dyn Fn(usize) -> Option<&'a str>,
    ) {
        // Found citations with no glyphs of their own (keys inside a range), per found entry.
        let inner_found: Vec<&FoundCite> = found.cites.iter().filter(|c| c.items.is_empty() && c.unmatched == 0).collect();
        let mut inner_used = vec![false; inner_found.len()];
        for (ti, t) in tc.iter().enumerate() {
            let form = t.spec.form.as_str();
            for k in [form, "*"] {
                let c = self.cites.entry(k.to_owned()).or_default();
                if t.spec.serial.is_none() {
                    c.inner += 1;
                    continue;
                }
                c.truth += 1;
                let Some((fi, _)) = ct_found.get(ti).copied().flatten() else { continue };
                c.matched += 1;
                match found.cites.get(fi).and_then(|f| f.target) {
                    None => c.unlinked += 1,
                    Some(e) if key_of_found(e) == Some(t.spec.key.as_str()) => c.right += 1,
                    Some(_) => c.wrong += 1,
                }
            }
            if t.spec.serial.is_none() {
                let hit = inner_found
                    .iter()
                    .enumerate()
                    .find(|(i, f)| !inner_used.get(*i).copied().unwrap_or(true) && f.target.and_then(key_of_found) == Some(t.spec.key.as_str()))
                    .map(|(i, _)| i);
                for k in [form, "*"] {
                    let c = self.cites.entry(k.to_owned()).or_default();
                    c.inner_right += usize::from(hit.is_some());
                }
                if let Some(slot) = hit.and_then(|i| inner_used.get_mut(i)) {
                    *slot = true;
                }
            }
        }
        // Inner citations found that name no truth inner key: wrong.
        let wrong_inner = inner_found.iter().zip(&inner_used).filter(|(f, used)| !**used && f.target.is_some()).count();
        for k in ["range_inner", "*"] {
            self.cites.entry(k.to_owned()).or_default().inner_wrong += wrong_inner;
        }
    }

    /// Found citations that stand for no truth citation, and the traps they fall on.
    fn add_spurious(&mut self, truth: &BibTruth, doc: &str, found: &FoundBib, cf_truth: &[Option<usize>]) {
        let traps: Vec<HashSet<ItemRef>> = truth.traps.iter().filter(|t| t.doc == doc).map(|t| flat(&t.items)).collect();
        self.traps += traps.len();
        let mut trap_hit = vec![false; traps.len()];
        for (fi, f) in found.cites.iter().enumerate() {
            if f.items.is_empty() && f.unmatched == 0 {
                continue;
            }
            self.cites_found += 1;
            if cf_truth.get(fi).copied().flatten().is_some() {
                continue;
            }
            self.cites_spurious += 1;
            if f.target.is_some() {
                self.spurious_links += 1;
            }
            for (k, t) in traps.iter().enumerate() {
                if f.items.iter().any(|i| t.contains(i))
                    && let Some(h) = trap_hit.get_mut(k)
                {
                    *h = true;
                }
            }
        }
        self.trap_hits += trap_hit.iter().filter(|h| **h).count();
    }

    fn pct(a: usize, b: usize) -> String {
        if b == 0 { "-".to_owned() } else { format!("{:.1}%", 100.0 * a as f64 / b as f64) }
    }

    /// A report of the tally, `title` first.
    pub fn render(&self, title: &str) -> String {
        let mut s = String::new();
        let e = &self.entries;
        let all = self.cites.get("*").copied().unwrap_or_default();
        let _ = writeln!(s, "== {title}: {} documents ==", self.docs);
        let _ = writeln!(
            s,
            "WRONG LINKS: {}  (wrong entry {}, wrong range key {}, link where nothing is cited {})   WRONG FIELDS: {}",
            self.wrong_links(),
            all.wrong,
            all.inner_wrong,
            self.spurious_links,
            self.wrong_fields()
        );
        let _ = writeln!(
            s,
            "region: found {}/{} lists, {} where there is none; glyph recall {} precision {}",
            self.region_found,
            self.with_list,
            self.region_spurious,
            Self::pct(self.region_common, self.region_truth),
            Self::pct(self.region_common, self.region_found_items)
        );
        let _ = writeln!(
            s,
            "entries: truth {} found {}; detected {} ({}), boundary F1>={ENTRY_F1} {} ({}), spurious {}, mean best F1 {:.3}",
            e.truth,
            e.found,
            e.detected,
            Self::pct(e.detected, e.truth),
            e.exact,
            Self::pct(e.exact, e.truth),
            e.spurious,
            if e.truth == 0 { 0.0 } else { e.f1_sum / e.truth as f64 }
        );
        let _ = writeln!(
            s,
            "labels: {} labelled; right {} ({}), wrong {}, missing {}; a label where none is printed {}",
            e.labelled,
            e.label_right,
            Self::pct(e.label_right, e.labelled),
            e.label_wrong,
            e.label_missing,
            e.label_spurious
        );
        let _ = writeln!(s, "fields (of detected entries):            truth  right  wrong  missing  invented  precision  recall");
        for name in FIELDS {
            let c = self.fields.get(name).copied().unwrap_or_default();
            let p = c.precision().map_or("-".to_owned(), |v| format!("{:.1}%", 100.0 * v));
            let r = c.recall().map_or("-".to_owned(), |v| format!("{:.1}%", 100.0 * v));
            let _ = writeln!(s, "  {name:<38} {:>6} {:>6} {:>6} {:>8} {:>9} {p:>10} {r:>7}", c.truth, c.right, c.wrong, c.missing, c.invented);
        }
        let _ = writeln!(
            s,
            "citations: truth {} matched {} (recall {}); found {} spurious {} (precision {}); traps {} hit {}",
            all.truth,
            all.matched,
            Self::pct(all.matched, all.truth),
            self.cites_found,
            self.cites_spurious,
            Self::pct(self.cites_found.saturating_sub(self.cites_spurious), self.cites_found),
            self.traps,
            self.trap_hits
        );
        let _ = writeln!(
            s,
            "  {:<16} {:>6} {:>8} {:>6} {:>6} {:>9} {:>6} {:>12}",
            "form", "truth", "matched", "right", "WRONG", "unlinked", "inner", "inner right"
        );
        for (k, c) in &self.cites {
            let _ = writeln!(
                s,
                "  {k:<16} {:>6} {:>8} {:>6} {:>6} {:>9} {:>6} {:>12}",
                c.truth, c.matched, c.right, c.wrong, c.unlinked, c.inner, c.inner_right
            );
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tex::{Item, Mark, Owner};

    fn glyph(code: u32) -> (Item, Owner) {
        (Item { mark: Mark::Glyph { font: "cmr10".into(), size: 655_360, code }, h: 0, v: 0 }, Owner::Outside)
    }

    #[test]
    fn folding_drops_accents_braces_and_dashes() {
        assert_eq!(fold(r#"Ber{\'e}nice M\"uller--Okafor"#), "berenice muller-okafor");
        assert_eq!(fold("Bérénice Müller–Okafor"), "berenice muller-okafor");
        assert_eq!(fold(r"{\c{C}}elik and \v{S}imek"), "celik and simek");
        assert_eq!(fold("Çelik and Šimek"), "celik and simek");
        assert_eq!(fold(r"On $L^2$ bounds"), "on l2 bounds");
        assert!(field_matches("authors", "M\\\"uller|Okafor", "Müller|Okafor"));
        assert!(!field_matches("authors", "Muller|Okafor", "Muller"));
        assert!(field_matches("doi", "10.1234/abc.5", "10.1234/abc.5."));
        assert!(field_matches("arxiv", "2101.01234", "arXiv:2101.01234"));
        assert!(field_matches("title", "On the stability of {Banach} spaces", "On the stability of Banach spaces"));
        assert!(!field_matches("year", "2020a", "2020"));
    }

    #[test]
    fn brackets_give_entries_citations_and_traps() {
        let mut page = DviPage { items: (0..12).map(|c| glyph(65 + c)).collect(), equations: Vec::new(), events: Vec::new() };
        page.events = vec![
            (0, "par:begin:par:0".into()),
            (0, "x:cite:begin:1:a".into()),
            (1, "x:cite:end".into()),
            (2, "x:cite:begin:2:b".into()),
            (3, "x:cite:end".into()),
            (3, "x:cite:resume:2".into()),
            (4, "x:cite:end".into()),
            (4, "x:trap:begin:t1".into()),
            (5, "x:trap:end".into()),
            (6, "par:end:1".into()),
            (6, "x:bib:head".into()),
            (6, "par:begin:heading1:3".into()),
            (7, "par:end:3".into()),
            (7, "par:begin:par:1".into()),
            (7, "x:bib:begin:a".into()),
            (9, "x:bib:end".into()),
            (9, "par:end:2".into()),
            (9, "par:begin:par:1".into()),
            (9, "x:bib:begin:b".into()),
            (12, "x:bib:end".into()),
            (12, "par:end:2".into()),
        ];
        let layout = super::super::layout::build("d", std::slice::from_ref(&page)).expect("layout");
        let entries = vec![
            EntrySpec { key: "a".into(), label: Some("1".into()), fields: FieldsTruth::default() },
            EntrySpec { key: "b".into(), label: Some("2".into()), fields: FieldsTruth::default() },
        ];
        let cites = vec![
            CiteSpec { serial: Some(1), key: "a".into(), form: "cite".into() },
            CiteSpec { serial: Some(2), key: "b".into(), form: "citet".into() },
        ];
        let traps = vec![TrapSpec { id: "t1".into(), kind: "interval".into() }];
        let (head, et, ct, tt) = build("d", &[page.clone()], &layout, &entries, &cites, &traps).expect("truth");
        assert_eq!(head, vec![(0, vec![6])]);
        assert_eq!(et[0].items, vec![(0, vec![7, 8])]);
        assert_eq!(et[1].items, vec![(0, vec![9, 10, 11])]);
        assert_eq!(ct[0].items, vec![(0, vec![0])]);
        assert_eq!(ct[1].items, vec![(0, vec![2, 3])], "resumed");
        assert_eq!(tt[0].items, vec![(0, vec![4])]);
        // A key the specs do not know, or one the generator wrote otherwise, fails.
        let wrong = vec![CiteSpec { serial: Some(1), key: "b".into(), form: "cite".into() }, cites[1].clone()];
        assert!(build("d", &[page.clone()], &layout, &entries, &wrong, &traps).is_err());
        assert!(build("d", &[page], &layout, &entries[..1], &cites, &traps).is_err());
        // The file round-trips.
        let truth = BibTruth {
            layout: LayoutTruth::default(),
            papers: vec![BibPaper { doc: "d".into(), style: "plain".into(), mode: "numeric".into(), hyperref: false }],
            headings: vec![("d".into(), head)],
            entries: et,
            cites: ct,
            traps: tt,
        };
        let back = BibTruth::parse(&truth.to_tsv("test")).expect("parses");
        assert_eq!(back.entries, truth.entries);
        assert_eq!(back.cites, truth.cites);
        assert_eq!(back.traps, truth.traps);
        assert_eq!(back.headings, truth.headings);
    }

    #[test]
    fn scoring_counts_wrong_links_and_fields_apart() {
        let entry = |doc: &str, key: &str, items: Vec<usize>, year: &str| EntryTruth {
            doc: doc.into(),
            spec: EntrySpec { key: key.into(), label: Some(key.into()), fields: FieldsTruth { year: Some(year.into()), ..Default::default() } },
            order: 0,
            items: vec![(0, items)],
        };
        let truth = BibTruth {
            entries: vec![entry("d", "1", vec![10, 11, 12], "2020"), entry("d", "2", vec![13, 14, 15], "2019")],
            cites: vec![
                CiteTruth { doc: "d".into(), spec: CiteSpec { serial: Some(1), key: "1".into(), form: "cite".into() }, items: vec![(0, vec![1])] },
                CiteTruth { doc: "d".into(), spec: CiteSpec { serial: Some(2), key: "2".into(), form: "cite".into() }, items: vec![(0, vec![3])] },
            ],
            traps: vec![TrapTruth { doc: "d".into(), spec: TrapSpec { id: "t".into(), kind: "interval".into() }, items: vec![(0, vec![5, 6])] }],
            ..Default::default()
        };
        let mut fields = BTreeMap::new();
        fields.insert("year".to_owned(), "2020".to_owned());
        let found = FoundBib {
            region: Some((10..16).map(|i| (0, i)).collect()),
            entries: vec![
                FoundEntry { items: vec![(0, 10), (0, 11), (0, 12)], label: Some("1".into()), fields: fields.clone(), ..Default::default() },
                FoundEntry { items: vec![(0, 13), (0, 14), (0, 15)], label: Some("2".into()), fields, ..Default::default() },
            ],
            cites: vec![
                FoundCite { items: vec![(0, 1)], unmatched: 0, target: Some(0) },
                FoundCite { items: vec![(0, 3)], unmatched: 0, target: Some(0) },
                FoundCite { items: vec![(0, 5)], unmatched: 0, target: Some(1) },
            ],
        };
        let mut t = BibTally::default();
        t.add_document(&truth, "d", &found);
        let all = t.cites["*"];
        assert_eq!((all.truth, all.matched, all.right, all.wrong), (2, 2, 1, 1));
        assert_eq!((t.cites_spurious, t.spurious_links, t.trap_hits), (1, 1, 1));
        assert_eq!(t.wrong_links(), 2);
        assert_eq!((t.fields["year"].right, t.fields["year"].wrong), (1, 1));
        assert_eq!((t.entries.detected, t.entries.exact, t.entries.label_right), (2, 2, 2));
        assert!(t.render("test").contains("WRONG LINKS: 2"));
    }
}
