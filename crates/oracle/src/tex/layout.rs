//! TeX as the oracle for page layout: which glyphs and rules make up each block of a page (a
//! paragraph, a heading, a display, a footnote, the running head…), of what kind, and in which
//! order TeX shipped them.
//!
//! The method is the math oracle's ([`super`]): the document is compiled to DVI with the
//! structure bracketed by `\special`s, and `dvitype` lists every glyph and rule with the brackets in
//! shipping order. The brackets come from [`package`], a LaTeX package that hooks paragraphs
//! (`para/begin`, `para/end`), `\@startsection` (headings and their level), `\@maketitle`, the
//! `abstract` environment, `\@footnotetext`, `\footnoterule` and the header and footer of
//! `\@outputpage`. Floats, captions and lists are bracketed in the source by `\pdfcraftopen`,
//! `\pdfcraftclose` and `\pdfcraftenv`, and display equations by the math oracle's own
//! `\pdfcraftbegin`/`\pdfcraftend` and its equation-number patch. With `specials` false the package
//! only defines those source macros as nothing: the generator compiles the PDF both ways and
//! requires identical bytes, which proves neither the specials nor the patches moved anything.
//!
//! A block of the truth is a maximal run of items, in shipping order, with one owner, cut again
//! where TeX moved up the page by more than [`COLUMN_JUMP_SP`] (the next column) or to the next
//! page. So a paragraph broken by a display is two blocks with the display between them, and a
//! paragraph that runs on into the next column is two blocks; that is what a reader of the page sees.

use super::{DviPage, Item, Mark, Owner, TexError};
use std::collections::HashMap;
use std::fmt::{self, Write as _};

/// How far up the page the next item of the same owner may go before it starts a new block, in sp
/// (30 pt): a column's top. A superscript or a radical's index rises less.
pub const COLUMN_JUMP_SP: i64 = 30 * 65536;
/// A block of the truth is the one a found block stands for when their item sets match with an F1
/// at least this high.
pub const MATCH_F1: f64 = 0.9;
/// Most open brackets at once.
pub const MAX_DEPTH: usize = 64;
/// Most rows read from one layout truth file.
pub const MAX_ROWS: usize = 2_000_000;

/// The kind of a block.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kind {
    Title,
    Abstract,
    /// A sectioning heading of a level (1 section, 2 subsection, 3 subsubsection).
    Heading(u8),
    Paragraph,
    Display,
    Footnote,
    ListItem,
    Float,
    Caption,
    RunningHeader,
    RunningFooter,
    PageNumber,
    Other,
    /// Drawn, but not a block of its own (the footnote rule): left out of the scores.
    Skip,
}

impl Kind {
    /// Every kind but the heading levels and `Skip`, in report order; [`Kind::family`] maps the
    /// levels onto `Heading(0)`.
    pub const REPORTED: [Kind; 13] = [
        Kind::Title,
        Kind::Abstract,
        Kind::Heading(0),
        Kind::Paragraph,
        Kind::Display,
        Kind::Footnote,
        Kind::ListItem,
        Kind::Float,
        Kind::Caption,
        Kind::RunningHeader,
        Kind::RunningFooter,
        Kind::PageNumber,
        Kind::Other,
    ];

    pub fn as_str(self) -> String {
        match self {
            Kind::Heading(0) => "heading".to_owned(),
            Kind::Heading(n) => format!("heading{n}"),
            k => k.base_name().to_owned(),
        }
    }

    fn base_name(self) -> &'static str {
        match self {
            Kind::Title => "title",
            Kind::Abstract => "abstract",
            Kind::Heading(_) => "heading",
            Kind::Paragraph => "paragraph",
            Kind::Display => "display",
            Kind::Footnote => "footnote",
            Kind::ListItem => "item",
            Kind::Float => "float",
            Kind::Caption => "caption",
            Kind::RunningHeader => "head",
            Kind::RunningFooter => "foot",
            Kind::PageNumber => "pagenum",
            Kind::Other => "other",
            Kind::Skip => "skip",
        }
    }

    pub fn parse(s: &str) -> Option<Kind> {
        if let Some(level) = s.strip_prefix("heading") {
            return if level.is_empty() { Some(Kind::Heading(0)) } else { level.parse().ok().filter(|l| (1..=9).contains(l)).map(Kind::Heading) };
        }
        Some(match s {
            "title" => Kind::Title,
            "abstract" => Kind::Abstract,
            "paragraph" | "par" => Kind::Paragraph,
            "display" => Kind::Display,
            "footnote" => Kind::Footnote,
            "item" => Kind::ListItem,
            "float" => Kind::Float,
            "caption" => Kind::Caption,
            "head" => Kind::RunningHeader,
            "foot" => Kind::RunningFooter,
            "pagenum" => Kind::PageNumber,
            "other" => Kind::Other,
            "skip" => Kind::Skip,
            _ => return None,
        })
    }

    /// The kind with a heading's level dropped: what kind accuracy compares.
    pub fn family(self) -> Kind {
        match self {
            Kind::Heading(_) => Kind::Heading(0),
            k => k,
        }
    }

    pub fn level(self) -> Option<u8> {
        match self {
            Kind::Heading(n) if n > 0 => Some(n),
            _ => None,
        }
    }

    /// A running element: repeated page furniture.
    pub fn is_running(self) -> bool {
        matches!(self, Kind::RunningHeader | Kind::RunningFooter | Kind::PageNumber)
    }
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.as_str())
    }
}

/// The LaTeX package `pdfcraftlayout` (load it after `amsmath`). With `specials` false it defines
/// the source macros and nothing else.
pub fn package(specials: bool) -> String {
    let head = "\\NeedsTeXFormat{LaTeX2e}\n\\ProvidesPackage{pdfcraftlayout}[2026/11/17 PdfCraft layout oracle brackets]\n";
    if !specials {
        return format!(
            "{head}% The brackets expand to nothing: the PDF must come out byte-identical with them.\n\
             \\newcommand*\\pdfcraftopen[1]{{}}\n\\newcommand*\\pdfcraftclose[1]{{}}\n\\newcommand*\\pdfcraftenv[1]{{}}\n\
             \\newcommand*\\pdfcraftbegin[1]{{}}\n\\newcommand*\\pdfcraftend[1]{{}}\n"
        );
    }
    format!(
        "{head}\\RequirePackage{{etoolbox}}\n\
         \\def\\pdfcraft@s#1{{\\special{{pdfcraft:layout:#1}}}}\n\
         \\newcommand*\\pdfcraftopen[1]{{\\pdfcraft@s{{open:#1}}}}\n\
         \\newcommand*\\pdfcraftclose[1]{{\\pdfcraft@s{{close:#1}}}}\n\
         \\newcommand*\\pdfcraftenv[1]{{\\def\\pdfcraft@env{{#1}}}}\n\
         \\newcommand*\\pdfcraftbegin[1]{{\\special{{pdfcraft:eq:#1:begin}}}}\n\
         \\newcommand*\\pdfcraftend[1]{{\\special{{pdfcraft:eq:#1:end}}}}\n\
         % The kind of the next paragraph: a one-shot (heading, title, footnote) or the environment's.\n\
         \\def\\pdfcraft@env{{par}}\n\
         \\global\\let\\pdfcraft@next\\@empty\n\
         \\def\\pdfcraft@kind{{\\ifx\\pdfcraft@next\\@empty\\pdfcraft@env\\else\\pdfcraft@next\\fi}}\n\
         \\AddToHook{{para/begin}}{{\\pdfcraft@s{{par:begin:\\pdfcraft@kind:\\the\\currentgrouplevel}}\\global\\let\\pdfcraft@next\\@empty}}\n\
         \\AddToHook{{para/end}}{{\\pdfcraft@s{{par:end:\\the\\currentgrouplevel}}}}\n\
         \\def\\pdfcraft@fail#1{{\\PackageError{{pdfcraftlayout}}{{cannot patch #1}}{{}}}}\n\
         \\pretocmd\\@startsection{{\\gdef\\pdfcraft@next{{heading#2}}}}{{}}{{\\pdfcraft@fail{{startsection}}}}\n\
         \\pretocmd\\@maketitle{{\\gdef\\pdfcraft@next{{title}}\\def\\pdfcraft@env{{other}}}}{{}}{{\\pdfcraft@fail{{maketitle}}}}\n\
         \\pretocmd\\@footnotetext{{\\gdef\\pdfcraft@next{{footnote}}}}{{}}{{\\pdfcraft@fail{{footnotetext}}}}\n\
         \\AtBeginEnvironment{{abstract}}{{\\def\\pdfcraft@env{{abstract}}}}\n\
         \\let\\pdfcraft@footnoterule\\footnoterule\n\
         \\def\\footnoterule{{\\pdfcraft@s{{open:skip}}\\pdfcraft@footnoterule\\pdfcraft@s{{close:skip}}}}\n\
         \\patchcmd\\@outputpage{{{{\\@thehead}}}}{{{{\\pdfcraft@s{{open:head}}\\pdfcraft@numbered\\@thehead\\pdfcraft@s{{close:head}}}}}}{{}}{{\\pdfcraft@fail{{head}}}}\n\
         \\patchcmd\\@outputpage{{{{\\@thefoot}}}}{{{{\\pdfcraft@s{{open:foot}}\\pdfcraft@numbered\\@thefoot\\pdfcraft@s{{close:foot}}}}}}{{}}{{\\pdfcraft@fail{{foot}}}}\n\
         \\def\\pdfcraft@numbered{{\\let\\pdfcraft@thepage\\thepage\\def\\thepage{{\\pdfcraft@s{{open:pagenum}}\\pdfcraft@thepage\\pdfcraft@s{{close:pagenum}}}}}}\n\
         % amsmath's \\maketag@@@, with the equation number bracketed as the math oracle brackets it.\n\
         \\@ifpackageloaded{{amsmath}}{{\\def\\maketag@@@#1{{\\hbox{{\\m@th\\normalfont\\special{{pdfcraft:tag:begin}}#1\\special{{pdfcraft:tag:end}}}}}}}}{{}}\n"
    )
}

/// One block of the truth.
#[derive(Clone, Debug, PartialEq)]
pub struct TruthBlock {
    /// Unique within the document: `b0`, `b1`, … in shipping order.
    pub id: String,
    pub kind: Kind,
    /// Indices into the page's [`LayoutPage::items`], in shipping order.
    pub items: Vec<usize>,
}

/// One page of a document with its blocks, in shipping order.
#[derive(Clone, Debug, PartialEq)]
pub struct LayoutPage {
    pub doc: String,
    /// From 0, as `PageRenderer::glyph_details` counts.
    pub page: usize,
    pub items: Vec<Item>,
    pub blocks: Vec<TruthBlock>,
    /// Items no bracket owns (counted, not scored).
    pub unowned: Vec<usize>,
}

/// What kind of paper a document is, for grouping scores.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaperInfo {
    pub doc: String,
    /// Held out: written before the analysis and read only by its rates.
    pub holdout: bool,
    /// `article`, `amsart`.
    pub class: String,
    /// `1col` or `2col`.
    pub columns: String,
    /// `10pt`, `11pt`, `12pt`.
    pub size: String,
}

/// A layout truth file: the papers and their pages.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LayoutTruth {
    pub papers: Vec<PaperInfo>,
    pub pages: Vec<LayoutPage>,
}

#[derive(Clone, Debug)]
enum Frame {
    /// A paragraph of a kind, opened at a TeX group level.
    Par { level: u32, key: usize },
    /// A bracket opened in the source or by a patch.
    Explicit { kind: Kind, key: usize },
}

fn listing(what: impl fmt::Display) -> TexError {
    TexError::Listing(format!("layout truth: {what}"))
}

/// Builds the blocks of document `doc` from its DVI pages (see the module documentation).
///
/// Fails on brackets that do not follow: a close with nothing open, an unknown kind, more than
/// [`MAX_DEPTH`] open, or anything left open at the end.
pub fn build(doc: &str, pages: &[DviPage]) -> Result<Vec<LayoutPage>, TexError> {
    let mut stack: Vec<Frame> = Vec::new();
    let mut next_key = 0usize;
    // Frame key -> kind, for every frame ever opened.
    let mut kinds: Vec<Kind> = Vec::new();
    let mut new_key = |kind: Kind, kinds: &mut Vec<Kind>| {
        kinds.push(kind);
        next_key = next_key.saturating_add(1);
        next_key.saturating_sub(1)
    };
    let mut out = Vec::with_capacity(pages.len());
    let mut block_no = 0usize;
    for (page_index, page) in pages.iter().enumerate() {
        // The owner (frame key) of each item; equations get keys of their own.
        let mut owner: Vec<Option<usize>> = vec![None; page.items.len()];
        let mut events = page.events.iter().peekable();
        let mut eq_keys: HashMap<usize, usize> = HashMap::new();
        for (i, (_, dvi_owner)) in page.items.iter().enumerate() {
            while let Some((_, text)) = events.next_if(|(at, _)| *at <= i) {
                apply(&mut stack, text, &mut kinds, &mut new_key)?;
            }
            let o = match dvi_owner {
                Owner::Equation(e) => Some(*eq_keys.entry(*e).or_insert_with(|| new_key(Kind::Display, &mut kinds))),
                // Resolved below: the number belongs to its equation.
                Owner::Tag => None,
                Owner::Outside => current(&stack),
            };
            if let Some(slot) = owner.get_mut(i) {
                *slot = o;
            }
        }
        for (_, text) in events {
            apply(&mut stack, text, &mut kinds, &mut new_key)?;
        }
        // An equation number joins the equation set nearest to it on the page: on its row. (On the
        // left, amsart's `leqno`, it is shipped before its equation's bracket opens.)
        let eq_items: Vec<usize> = (0..page.items.len()).filter(|&k| matches!(page.items.get(k), Some((_, Owner::Equation(_))))).collect();
        for i in 0..page.items.len() {
            let Some((tag, Owner::Tag)) = page.items.get(i) else { continue };
            let nearest = eq_items.iter().copied().min_by_key(|&k| {
                let v = page.items.get(k).map_or(i64::MAX, |(item, _)| item.v.saturating_sub(tag.v).saturating_abs());
                (v, k.abs_diff(i))
            });
            let found = nearest.and_then(|k| owner.get(k).copied().flatten());
            if let Some(slot) = owner.get_mut(i) {
                *slot = found;
            }
        }
        // Pieces: runs of one owner, cut at a jump up the page.
        let mut blocks: Vec<TruthBlock> = Vec::new();
        let mut open_piece: HashMap<usize, usize> = HashMap::new();
        let mut last_key: Option<usize> = None;
        let mut unowned = Vec::new();
        for (i, (item, _)) in page.items.iter().enumerate() {
            let Some(key) = owner.get(i).copied().flatten() else {
                unowned.push(i);
                continue;
            };
            // A float, a caption or a display is drawn in any order (a frame's rules after its
            // text) and never breaks across columns: only text flows on into the next one.
            let flows = matches!(kinds.get(key), Some(Kind::Paragraph | Kind::ListItem | Kind::Footnote | Kind::Abstract | Kind::Other));
            let continues = last_key == Some(key)
                && (!flows
                    || open_piece
                        .get(&key)
                        .and_then(|&b| blocks.get(b))
                        .and_then(|b| b.items.last())
                        .and_then(|&k| page.items.get(k))
                        .is_some_and(|(prev, _)| item.v >= prev.v.saturating_sub(COLUMN_JUMP_SP)));
            if continues {
                if let Some(b) = open_piece.get(&key).and_then(|&b| blocks.get_mut(b)) {
                    b.items.push(i);
                }
            } else {
                let kind = kinds.get(key).copied().unwrap_or(Kind::Other);
                open_piece.insert(key, blocks.len());
                blocks.push(TruthBlock { id: format!("b{block_no}"), kind, items: vec![i] });
                block_no = block_no.saturating_add(1);
            }
            last_key = Some(key);
        }
        out.push(LayoutPage { doc: doc.to_owned(), page: page_index, items: page.items.iter().map(|(i, _)| i.clone()).collect(), blocks, unowned });
    }
    if let Some(open) = stack.iter().find(|f| matches!(f, Frame::Explicit { .. })) {
        return Err(listing(format!("{doc}: a bracket is still open at the end: {open:?}")));
    }
    Ok(out)
}

/// The frame that owns what is shipped now: the innermost source bracket if any is open (a
/// paragraph inside a caption or a header is the caption's or the header's), else the innermost
/// paragraph.
fn current(stack: &[Frame]) -> Option<usize> {
    if let Some(Frame::Explicit { key, .. }) = stack.iter().rev().find(|f| matches!(f, Frame::Explicit { .. })) {
        return Some(*key);
    }
    stack.iter().rev().find_map(|f| match f {
        Frame::Par { key, .. } => Some(*key),
        Frame::Explicit { .. } => None,
    })
}

fn apply(stack: &mut Vec<Frame>, text: &str, kinds: &mut Vec<Kind>, new_key: &mut impl FnMut(Kind, &mut Vec<Kind>) -> usize) -> Result<(), TexError> {
    let parts: Vec<&str> = text.split(':').collect();
    match parts[..] {
        ["par", "begin", kind, level] => {
            let kind = Kind::parse(kind).ok_or_else(|| listing(format!("unknown paragraph kind in `{text}`")))?;
            let level: u32 = level.parse().map_err(|_| listing(format!("bad group level in `{text}`")))?;
            // A paragraph never holds another at its own group level or outside it: one that was
            // not closed ends here. A footnote's paragraph opens deeper, inside the insert.
            while let Some(Frame::Par { level: l, .. }) = stack.last() {
                if *l >= level {
                    stack.pop();
                } else {
                    break;
                }
            }
            if stack.len() >= MAX_DEPTH {
                return Err(listing(format!("more than {MAX_DEPTH} brackets open")));
            }
            let key = new_key(kind, kinds);
            stack.push(Frame::Par { level, key });
        }
        ["par", "end", _] => {
            // Only a paragraph opened inside the innermost bracket: an extra end inside a caption
            // must not end the body paragraph the float interrupted.
            if let Some(Frame::Par { .. }) = stack.last() {
                stack.pop();
            }
        }
        ["open", kind] => {
            let kind = Kind::parse(kind).ok_or_else(|| listing(format!("unknown bracket `{text}`")))?;
            if stack.len() >= MAX_DEPTH {
                return Err(listing(format!("more than {MAX_DEPTH} brackets open")));
            }
            let key = new_key(kind, kinds);
            stack.push(Frame::Explicit { kind, key });
        }
        ["close", kind] => {
            let kind = Kind::parse(kind).ok_or_else(|| listing(format!("unknown bracket `{text}`")))?;
            match stack.iter().rposition(|f| matches!(f, Frame::Explicit { .. })) {
                Some(at) if matches!(stack.get(at), Some(Frame::Explicit { kind: k, .. }) if *k == kind) => {
                    // Paragraphs opened inside the bracket end with it.
                    stack.truncate(at);
                }
                _ => return Err(listing(format!("`{text}` closes nothing open"))),
            }
        }
        _ => return Err(listing(format!("unknown special `{text}`"))),
    }
    Ok(())
}

fn field<'a>(f: &[&'a str], i: usize, line: usize) -> Result<&'a str, TexError> {
    f.get(i).copied().ok_or_else(|| listing(format!("line {line}: column {} is missing", i.saturating_add(1))))
}

fn number<T: std::str::FromStr>(f: &[&str], i: usize, line: usize) -> Result<T, TexError> {
    let s = field(f, i, line)?;
    s.parse().map_err(|_| listing(format!("line {line}: `{s}` is not a number")))
}

impl LayoutTruth {
    /// The file [`LayoutTruth::parse`] reads, one tab-separated row per record, headed by `header`:
    ///
    /// ```text
    /// D  doc  set  class  columns  size        a paper (set: dev | holdout)
    /// B  doc  page  block  kind                a block, in shipping order
    /// G  doc  page  owner  font  size  code  h  v
    /// R  doc  page  owner  h  v  width  height
    /// ```
    ///
    /// `G` and `R` rows list every item of a page in shipping order; `owner` is a `B` id or `-`.
    pub fn to_tsv(&self, header: &str) -> String {
        let mut s = String::new();
        for line in header.lines() {
            let _ = writeln!(s, "# {line}");
        }
        for p in &self.papers {
            let set = if p.holdout { "holdout" } else { "dev" };
            let _ = writeln!(s, "D\t{}\t{set}\t{}\t{}\t{}", p.doc, p.class, p.columns, p.size);
        }
        for page in &self.pages {
            let mut owner_of: Vec<&str> = vec!["-"; page.items.len()];
            for b in &page.blocks {
                let _ = writeln!(s, "B\t{}\t{}\t{}\t{}", page.doc, page.page, b.id, b.kind);
                for &i in &b.items {
                    if let Some(o) = owner_of.get_mut(i) {
                        *o = &b.id;
                    }
                }
            }
            for (item, owner) in page.items.iter().zip(owner_of) {
                match &item.mark {
                    Mark::Glyph { font, size, code } => {
                        let _ = writeln!(s, "G\t{}\t{}\t{owner}\t{font}\t{size}\t{code}\t{}\t{}", page.doc, page.page, item.h, item.v);
                    }
                    Mark::Rule { width, height } => {
                        let _ = writeln!(s, "R\t{}\t{}\t{owner}\t{}\t{}\t{width}\t{height}", page.doc, page.page, item.h, item.v);
                    }
                }
            }
        }
        s
    }

    pub fn parse(tsv: &str) -> Result<LayoutTruth, TexError> {
        let mut set = LayoutTruth::default();
        let mut page_at: HashMap<(String, usize), usize> = HashMap::new();
        for (n, line) in tsv.lines().enumerate() {
            let line_no = n.saturating_add(1);
            if line_no > MAX_ROWS {
                return Err(listing(format!("more than {MAX_ROWS} rows")));
            }
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let f: Vec<&str> = line.split('\t').collect();
            match f.first().copied() {
                Some("D") => set.papers.push(PaperInfo {
                    doc: field(&f, 1, line_no)?.to_owned(),
                    holdout: match field(&f, 2, line_no)? {
                        "dev" => false,
                        "holdout" => true,
                        other => return Err(listing(format!("line {line_no}: set `{other}` is neither dev nor holdout"))),
                    },
                    class: field(&f, 3, line_no)?.to_owned(),
                    columns: field(&f, 4, line_no)?.to_owned(),
                    size: field(&f, 5, line_no)?.to_owned(),
                }),
                Some(row @ ("B" | "G" | "R")) => {
                    let doc = field(&f, 1, line_no)?.to_owned();
                    let page: usize = number(&f, 2, line_no)?;
                    let at = match page_at.get(&(doc.clone(), page)) {
                        Some(&at) => at,
                        None => {
                            page_at.insert((doc.clone(), page), set.pages.len());
                            set.pages.push(LayoutPage { doc, page, items: Vec::new(), blocks: Vec::new(), unowned: Vec::new() });
                            set.pages.len().saturating_sub(1)
                        }
                    };
                    let Some(p) = set.pages.get_mut(at) else { continue };
                    let owner = field(&f, 3, line_no)?;
                    if row == "B" {
                        let kind = field(&f, 4, line_no)?;
                        let kind = Kind::parse(kind).ok_or_else(|| listing(format!("line {line_no}: unknown kind `{kind}`")))?;
                        if p.blocks.iter().any(|b| b.id == owner) {
                            return Err(listing(format!("line {line_no}: block `{owner}` is listed twice")));
                        }
                        p.blocks.push(TruthBlock { id: owner.to_owned(), kind, items: Vec::new() });
                        continue;
                    }
                    let item = if row == "G" {
                        let mark =
                            Mark::Glyph { font: field(&f, 4, line_no)?.to_owned(), size: number(&f, 5, line_no)?, code: number(&f, 6, line_no)? };
                        Item { mark, h: number(&f, 7, line_no)?, v: number(&f, 8, line_no)? }
                    } else {
                        let mark = Mark::Rule { width: number(&f, 6, line_no)?, height: number(&f, 7, line_no)? };
                        Item { mark, h: number(&f, 4, line_no)?, v: number(&f, 5, line_no)? }
                    };
                    let index = p.items.len();
                    p.items.push(item);
                    if owner == "-" {
                        p.unowned.push(index);
                    } else {
                        let b =
                            p.blocks.iter_mut().find(|b| b.id == owner).ok_or_else(|| listing(format!("line {line_no}: unknown block `{owner}`")))?;
                        b.items.push(index);
                    }
                }
                _ => return Err(listing(format!("line {line_no}: unknown row"))),
            }
        }
        if let Some((p, b)) = set.pages.iter().find_map(|p| p.blocks.iter().find(|b| b.items.is_empty()).map(|b| (p, b))) {
            return Err(listing(format!("{} page {}: block `{}` has no items", p.doc, p.page, b.id)));
        }
        Ok(set)
    }

    pub fn paper(&self, doc: &str) -> Option<&PaperInfo> {
        self.papers.iter().find(|p| p.doc == doc)
    }
}

// --- Scoring -------------------------------------------------------------------------------------

/// A block an analysis found, in the analysis's reading order, as the scorer sees it.
#[derive(Clone, Debug, PartialEq)]
pub struct FoundBlock {
    pub kind: Kind,
    /// The truth items (indices into [`LayoutPage::items`]) its glyphs and rules are.
    pub items: Vec<usize>,
    /// Its glyphs and rules that are no item of the truth (nothing should be).
    pub unmatched: usize,
}

/// One found heading of a document's section tree.
#[derive(Clone, Debug, PartialEq)]
pub struct FoundSection {
    /// Page index, and the index of the heading's block in that page's found blocks.
    pub page: usize,
    pub block: usize,
    pub level: u8,
    /// Index into the same list.
    pub parent: Option<usize>,
}

/// Counts for one kind (or all).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct KindTally {
    /// Truth blocks.
    pub truth: usize,
    /// Truth blocks found with the right boundary (F1 at least [`MATCH_F1`]).
    pub boundary: usize,
    /// … and of the right kind (heading level not compared).
    pub kind_right: usize,
    /// Truth blocks merged with another into one found block.
    pub merged: usize,
    /// Truth blocks split over several found blocks.
    pub split: usize,
    /// Truth blocks with no found block holding half of them.
    pub missed: usize,
    /// Found blocks of this kind.
    pub found: usize,
    /// Found blocks of this kind that stand for no truth block.
    pub spurious: usize,
    /// Found blocks of this kind whose matched truth block is of this kind.
    pub found_right: usize,
}

impl KindTally {
    fn add(&mut self, o: &KindTally) {
        self.truth += o.truth;
        self.boundary += o.boundary;
        self.kind_right += o.kind_right;
        self.merged += o.merged;
        self.split += o.split;
        self.missed += o.missed;
        self.found += o.found;
        self.spurious += o.spurious;
        self.found_right += o.found_right;
    }
}

/// The scores of some pages.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LayoutTally {
    pub pages: usize,
    pub by_kind: std::collections::BTreeMap<Kind, KindTally>,
    /// Pages whose matched blocks come in exactly the truth's order, with every block matched.
    pub order_exact: usize,
    /// Sum of Kendall's tau over pages with at least two matched blocks, and how many.
    pub tau_sum: f64,
    pub tau_pages: usize,
    /// Running elements: found ones that are truth running elements, all found, all truth, and
    /// truth ones found.
    pub running_tp: usize,
    pub running_found: usize,
    pub running_truth: usize,
    pub running_recalled: usize,
    /// Section tree: truth headings, matched nodes, of those with the right level and the right parent.
    pub sections_truth: usize,
    pub sections_found: usize,
    pub sections_matched: usize,
    pub sections_level: usize,
    pub sections_parent: usize,
}

fn f1(common: usize, a: usize, b: usize) -> f64 {
    if a + b == 0 { 0.0 } else { 2.0 * common as f64 / (a + b) as f64 }
}

/// For each truth block, the found block that matches it (F1 at least [`MATCH_F1`]), if any.
pub fn matches(truth: &LayoutPage, found: &[FoundBlock]) -> Vec<Option<usize>> {
    let skip: std::collections::HashSet<usize> = truth.blocks.iter().filter(|b| b.kind == Kind::Skip).flat_map(|b| b.items.iter().copied()).collect();
    let mut owner = vec![usize::MAX; truth.items.len()];
    for (bi, b) in truth.blocks.iter().enumerate() {
        for &i in &b.items {
            if let Some(o) = owner.get_mut(i) {
                *o = bi;
            }
        }
    }
    let size = |f: &FoundBlock| f.items.iter().filter(|i| !skip.contains(i)).count() + f.unmatched;
    let mut overlap: HashMap<(usize, usize), usize> = HashMap::new();
    for (fi, f) in found.iter().enumerate() {
        for &i in &f.items {
            if let Some(&t) = owner.get(i).filter(|&&t| t != usize::MAX) {
                *overlap.entry((t, fi)).or_default() += 1;
            }
        }
    }
    truth
        .blocks
        .iter()
        .enumerate()
        .map(|(ti, t)| {
            found.iter().enumerate().find_map(|(fi, f)| {
                let c = overlap.get(&(ti, fi)).copied().unwrap_or(0);
                (t.kind != Kind::Skip && f1(c, t.items.len(), size(f)) >= MATCH_F1).then_some(fi)
            })
        })
        .collect()
}

/// Kendall's tau-a of a sequence against its sorted order (1: in order, -1: reversed).
pub fn kendall_tau(seq: &[usize]) -> Option<f64> {
    let n = seq.len();
    if n < 2 {
        return None;
    }
    // O(n²) on the blocks of one page (a few hundred at most).
    let (mut concordant, mut discordant) = (0i64, 0i64);
    for i in 0..n {
        for j in i.saturating_add(1)..n {
            match seq.get(i).cmp(&seq.get(j)) {
                std::cmp::Ordering::Less => concordant += 1,
                std::cmp::Ordering::Greater => discordant += 1,
                std::cmp::Ordering::Equal => {}
            }
        }
    }
    let pairs = (n * (n - 1) / 2) as f64;
    Some((concordant - discordant) as f64 / pairs)
}

impl LayoutTally {
    /// Adds one page: `found` in the analysis's reading order.
    pub fn add_page(&mut self, truth: &LayoutPage, found: &[FoundBlock]) {
        self.pages += 1;
        let m = matches(truth, found);
        let mut owner = vec![usize::MAX; truth.items.len()];
        for (bi, b) in truth.blocks.iter().enumerate() {
            for &i in &b.items {
                if let Some(o) = owner.get_mut(i) {
                    *o = bi;
                }
            }
        }
        // overlap[truth][found]
        let mut share: HashMap<(usize, usize), usize> = HashMap::new();
        for (fi, f) in found.iter().enumerate() {
            for &i in &f.items {
                if let Some(&t) = owner.get(i).filter(|&&t| t != usize::MAX) {
                    *share.entry((t, fi)).or_default() += 1;
                }
            }
        }
        let mut matched_found = vec![None; found.len()];
        for (ti, t) in truth.blocks.iter().enumerate() {
            if t.kind == Kind::Skip {
                continue;
            }
            let k = self.by_kind.entry(t.kind.family()).or_default();
            k.truth += 1;
            let n = t.items.len().max(1);
            let holders: Vec<(usize, usize)> = (0..found.len()).filter_map(|fi| share.get(&(ti, fi)).map(|&c| (fi, c))).collect();
            if let Some(fi) = m.get(ti).copied().flatten() {
                k.boundary += 1;
                if found.get(fi).is_some_and(|f| f.kind.family() == t.kind.family()) {
                    k.kind_right += 1;
                }
                if let Some(slot) = matched_found.get_mut(fi) {
                    *slot = Some(ti);
                }
                continue;
            }
            let big: Vec<usize> = holders.iter().filter(|(_, c)| c * 5 >= n).map(|(fi, _)| *fi).collect();
            let half = holders.iter().find(|(_, c)| c * 2 >= n).map(|(fi, _)| *fi);
            let merged = half.is_some_and(|fi| {
                truth
                    .blocks
                    .iter()
                    .enumerate()
                    .any(|(oi, o)| oi != ti && o.kind != Kind::Skip && share.get(&(oi, fi)).is_some_and(|&c| c * 2 >= o.items.len().max(1)))
            });
            if merged {
                k.merged += 1;
            } else if big.len() >= 2 {
                k.split += 1;
            } else if half.is_none() {
                k.missed += 1;
            } else {
                // Held mostly by one found block that also holds too much else, or that misses a piece.
                k.split += 1;
            }
        }
        for (fi, f) in found.iter().enumerate() {
            let k = self.by_kind.entry(f.kind.family()).or_default();
            k.found += 1;
            let size = f.items.len() + f.unmatched;
            let best = (0..truth.blocks.len()).filter_map(|ti| share.get(&(ti, fi))).max().copied().unwrap_or(0);
            if best * 2 < size.max(1) {
                k.spurious += 1;
            }
            if matched_found.get(fi).copied().flatten().and_then(|ti| truth.blocks.get(ti)).is_some_and(|t| t.kind.family() == f.kind.family()) {
                k.found_right += 1;
            }
        }
        // Reading order over the matched blocks.
        let seq: Vec<usize> = matched_found.iter().flatten().copied().collect();
        let scored = truth.blocks.iter().filter(|b| b.kind != Kind::Skip).count();
        if seq.len() == scored && seq.windows(2).all(|w| w.first() < w.get(1)) {
            self.order_exact += 1;
        }
        if let Some(tau) = kendall_tau(&seq) {
            self.tau_sum += tau;
            self.tau_pages += 1;
        }
        // Running elements.
        for (fi, f) in found.iter().enumerate() {
            if f.kind.is_running() {
                self.running_found += 1;
                if matched_found.get(fi).copied().flatten().and_then(|ti| truth.blocks.get(ti)).is_some_and(|t| t.kind.is_running()) {
                    self.running_tp += 1;
                }
            }
        }
        for (ti, t) in truth.blocks.iter().enumerate() {
            if t.kind.is_running() {
                self.running_truth += 1;
                if m.get(ti).copied().flatten().and_then(|fi| found.get(fi)).is_some_and(|f| f.kind.is_running()) {
                    self.running_recalled += 1;
                }
            }
        }
    }

    /// Adds one document's section tree. `pages` are the document's truth pages in order and
    /// `found` the found blocks of each (the same order); `sections` the found tree.
    pub fn add_sections(&mut self, pages: &[&LayoutPage], found: &[Vec<FoundBlock>], sections: &[FoundSection]) {
        // Truth headings in document order, with their parent.
        let mut truth_nodes: Vec<(usize, usize, u8)> = Vec::new();
        for (pi, p) in pages.iter().enumerate() {
            for (bi, b) in p.blocks.iter().enumerate() {
                if let Kind::Heading(level) = b.kind {
                    truth_nodes.push((pi, bi, level));
                }
            }
        }
        let truth_parent: Vec<Option<usize>> = (0..truth_nodes.len())
            .map(|i| {
                let level = truth_nodes.get(i).map_or(0, |n| n.2);
                (0..i).rev().find(|&j| truth_nodes.get(j).is_some_and(|n| n.2 < level))
            })
            .collect();
        let page_matches: Vec<Vec<Option<usize>>> = pages.iter().zip(found).map(|(p, f)| matches(p, f)).collect();
        self.sections_truth += truth_nodes.len();
        self.sections_found += sections.len();
        // The found section standing for each truth node.
        let found_of = |node: &(usize, usize, u8)| -> Option<usize> {
            let fb = page_matches.get(node.0)?.get(node.1).copied().flatten()?;
            sections.iter().position(|s| s.page == node.0 && s.block == fb)
        };
        let node_found: Vec<Option<usize>> = truth_nodes.iter().map(found_of).collect();
        for (i, node) in truth_nodes.iter().enumerate() {
            let Some(si) = node_found.get(i).copied().flatten() else { continue };
            let Some(s) = sections.get(si) else { continue };
            self.sections_matched += 1;
            if s.level == node.2 {
                self.sections_level += 1;
            }
            let want = truth_parent.get(i).copied().flatten().and_then(|p| node_found.get(p).copied().flatten());
            let parent_ok = match (truth_parent.get(i).copied().flatten(), s.parent) {
                (None, None) => true,
                (Some(_), Some(p)) => want == Some(p),
                _ => false,
            };
            if parent_ok {
                self.sections_parent += 1;
            }
        }
    }

    pub fn add(&mut self, o: &LayoutTally) {
        self.pages += o.pages;
        for (k, t) in &o.by_kind {
            self.by_kind.entry(*k).or_default().add(t);
        }
        self.order_exact += o.order_exact;
        self.tau_sum += o.tau_sum;
        self.tau_pages += o.tau_pages;
        self.running_tp += o.running_tp;
        self.running_found += o.running_found;
        self.running_truth += o.running_truth;
        self.running_recalled += o.running_recalled;
        self.sections_truth += o.sections_truth;
        self.sections_found += o.sections_found;
        self.sections_matched += o.sections_matched;
        self.sections_level += o.sections_level;
        self.sections_parent += o.sections_parent;
    }

    /// All kinds together.
    pub fn overall(&self) -> KindTally {
        let mut all = KindTally::default();
        for t in self.by_kind.values() {
            all.add(t);
        }
        all
    }

    /// Blocks found with the right boundary and kind, over all truth blocks.
    pub fn block_rate(&self) -> Option<f64> {
        let a = self.overall();
        (a.truth > 0).then(|| a.kind_right as f64 / a.truth as f64)
    }

    pub fn mean_tau(&self) -> Option<f64> {
        (self.tau_pages > 0).then(|| self.tau_sum / self.tau_pages as f64)
    }

    pub fn running_precision(&self) -> Option<f64> {
        (self.running_found > 0).then(|| self.running_tp as f64 / self.running_found as f64)
    }

    pub fn running_recall(&self) -> Option<f64> {
        (self.running_truth > 0).then(|| self.running_recalled as f64 / self.running_truth as f64)
    }

    /// A table: per kind, then overall, then order, running elements and sections.
    pub fn render(&self, title: &str, per_kind: bool) -> String {
        let pct = |a: usize, b: usize| if b == 0 { "    -".to_owned() } else { format!("{:5.1}", 100.0 * a as f64 / b as f64) };
        let opt = |v: Option<f64>, scale: f64| v.map_or("    -".to_owned(), |v| format!("{:5.1}", v * scale));
        let mut s = format!("{title}: {} pages\n", self.pages);
        let _ = writeln!(
            s,
            "  {:<10} {:>5} {:>6} {:>6} {:>6} {:>6} {:>6} {:>6} {:>6} {:>6}",
            "kind", "truth", "bound%", "kind%", "merged", "split", "missed", "found", "spur.", "prec%"
        );
        let row = |s: &mut String, name: &str, t: &KindTally| {
            let _ = writeln!(
                s,
                "  {:<10} {:>5} {:>6} {:>6} {:>6} {:>6} {:>6} {:>6} {:>6} {:>6}",
                name,
                t.truth,
                pct(t.boundary, t.truth),
                pct(t.kind_right, t.truth),
                t.merged,
                t.split,
                t.missed,
                t.found,
                t.spurious,
                pct(t.found_right, t.found)
            );
        };
        if per_kind {
            for k in Kind::REPORTED {
                if let Some(t) = self.by_kind.get(&k) {
                    row(&mut s, &k.as_str(), t);
                }
            }
        }
        row(&mut s, "all", &self.overall());
        let _ = writeln!(
            s,
            "  order: exact {}/{} pages, mean tau {}; running P {} R {} ({} found, {} truth); sections {}/{} matched, level {} parent {} ({} found)",
            self.order_exact,
            self.pages,
            opt(self.mean_tau(), 1.0),
            opt(self.running_precision(), 100.0),
            opt(self.running_recall(), 100.0),
            self.running_found,
            self.running_truth,
            self.sections_matched,
            self.sections_truth,
            pct(self.sections_level, self.sections_matched),
            pct(self.sections_parent, self.sections_matched),
            self.sections_found,
        );
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn glyph(code: u32, h: i64, v: i64) -> Item {
        Item { mark: Mark::Glyph { font: "cmr10".into(), size: 655360, code }, h, v }
    }

    const PT: i64 = 65536;

    /// Page 1: a head, a heading, a paragraph broken by a display with its number, a footnote inside
    /// the paragraph, the paragraph running on into the next column; page 2: the paragraph goes on.
    fn pages() -> Vec<DviPage> {
        let mut p1 = DviPage::default();
        let push = |p: &mut DviPage, item: Item, owner: Owner| p.items.push((item, owner));
        let ev = |p: &mut DviPage, s: &str| p.events.push((p.items.len(), s.to_owned()));
        ev(&mut p1, "open:head");
        push(&mut p1, glyph(65, 0, 0), Owner::Outside);
        ev(&mut p1, "open:pagenum");
        push(&mut p1, glyph(49, 300 * PT, 0), Owner::Outside);
        ev(&mut p1, "close:pagenum");
        ev(&mut p1, "close:head");
        ev(&mut p1, "par:begin:heading1:2");
        push(&mut p1, glyph(66, 0, 20 * PT), Owner::Outside);
        ev(&mut p1, "par:end:2");
        ev(&mut p1, "par:begin:par:0");
        push(&mut p1, glyph(67, 0, 40 * PT), Owner::Outside);
        p1.equations.push("e".into());
        push(&mut p1, glyph(68, 50 * PT, 60 * PT), Owner::Equation(0));
        push(&mut p1, glyph(40, 200 * PT, 60 * PT), Owner::Tag);
        push(&mut p1, glyph(69, 0, 80 * PT), Owner::Outside);
        // The next column: up the page.
        push(&mut p1, glyph(70, 200 * PT, 20 * PT), Owner::Outside);
        ev(&mut p1, "open:skip");
        p1.items.push((Item { mark: Mark::Rule { width: 100 * PT, height: PT / 2 }, h: 0, v: 90 * PT }, Owner::Outside));
        ev(&mut p1, "close:skip");
        ev(&mut p1, "par:begin:footnote:3");
        push(&mut p1, glyph(71, 0, 100 * PT), Owner::Outside);
        ev(&mut p1, "par:end:3");
        let mut p2 = DviPage::default();
        p2.items.push((glyph(72, 0, 20 * PT), Owner::Outside));
        p2.events.push((1, "par:end:0".into()));
        vec![p1, p2]
    }

    #[test]
    fn blocks_follow_the_brackets_and_split_where_the_page_does() {
        let out = build("d", &pages()).unwrap();
        let kinds: Vec<(Kind, Vec<usize>)> = out[0].blocks.iter().map(|b| (b.kind, b.items.clone())).collect();
        assert_eq!(
            kinds,
            [
                (Kind::RunningHeader, vec![0]),
                (Kind::PageNumber, vec![1]),
                (Kind::Heading(1), vec![2]),
                (Kind::Paragraph, vec![3]),
                (Kind::Display, vec![4, 5]),
                (Kind::Paragraph, vec![6]),
                (Kind::Paragraph, vec![7]),
                (Kind::Skip, vec![8]),
                (Kind::Footnote, vec![9]),
            ]
        );
        assert_eq!(out[1].blocks.len(), 1);
        assert_eq!(out[1].blocks[0].kind, Kind::Paragraph);
        let ids: Vec<&str> = out.iter().flat_map(|p| p.blocks.iter().map(|b| b.id.as_str())).collect();
        assert_eq!(ids.len(), 10);
        assert!(ids.iter().enumerate().all(|(i, id)| *id == format!("b{i}")));
    }

    #[test]
    fn an_extra_end_in_a_caption_does_not_end_the_paragraph_around_the_float() {
        let mut p = DviPage::default();
        let ev = |p: &mut DviPage, s: &str| p.events.push((p.items.len(), s.to_owned()));
        ev(&mut p, "par:begin:par:0");
        p.items.push((glyph(65, 0, 10 * PT), Owner::Outside));
        ev(&mut p, "open:caption");
        ev(&mut p, "par:begin:par:7");
        ev(&mut p, "par:begin:par:6");
        p.items.push((glyph(66, 0, 2 * PT), Owner::Outside));
        ev(&mut p, "par:end:7");
        ev(&mut p, "par:end:6");
        ev(&mut p, "close:caption");
        p.items.push((glyph(67, 0, 12 * PT), Owner::Outside));
        let out = build("d", &[p]).unwrap();
        let kinds: Vec<Kind> = out[0].blocks.iter().map(|b| b.kind).collect();
        assert_eq!(kinds, [Kind::Paragraph, Kind::Caption, Kind::Paragraph]);
        assert!(out[0].unowned.is_empty());
    }

    #[test]
    fn a_number_on_the_left_joins_the_equation_on_its_row() {
        let mut p = DviPage::default();
        p.equations = vec!["a".into(), "b".into()];
        p.items.push((glyph(65, 100 * PT, 10 * PT), Owner::Equation(0)));
        p.items.push((glyph(40, 0, 40 * PT), Owner::Tag));
        p.items.push((glyph(66, 100 * PT, 40 * PT), Owner::Equation(1)));
        let out = build("d", &[p]).unwrap();
        let blocks: Vec<Vec<usize>> = out[0].blocks.iter().map(|b| b.items.clone()).collect();
        assert_eq!(blocks, [vec![0], vec![1, 2]]);
    }

    #[test]
    fn broken_brackets_are_errors() {
        for bad in ["close:head", "open:nonsense", "par:begin:nonsense:0", "par:begin:par:x", "what"] {
            let mut p = DviPage::default();
            p.events.push((0, bad.into()));
            assert!(build("d", &[p]).is_err(), "{bad}");
        }
        let mut p = DviPage::default();
        p.events.push((0, "open:float".into()));
        assert!(build("d", &[p]).unwrap_err().to_string().contains("still open"));
        let mut deep = DviPage::default();
        for _ in 0..=MAX_DEPTH {
            deep.events.push((0, "open:float".into()));
        }
        assert!(build("d", &[deep]).is_err());
    }

    #[test]
    fn the_file_round_trips_and_rejects_nonsense() {
        let truth = LayoutTruth {
            papers: vec![PaperInfo { doc: "d".into(), holdout: true, class: "article".into(), columns: "2col".into(), size: "10pt".into() }],
            pages: build("d", &pages()).unwrap(),
        };
        let tsv = truth.to_tsv("made by a test");
        assert_eq!(LayoutTruth::parse(&tsv).unwrap(), truth);
        for (from, to, why) in
            [("\tb3\t", "\tb99\t", "unknown block"), ("\tparagraph", "\tpara-graph", "unknown kind"), ("D\t", "X\t", "unknown row")]
        {
            let broken = tsv.replacen(from, to, 1);
            assert_ne!(broken, tsv);
            assert!(LayoutTruth::parse(&broken).unwrap_err().to_string().contains(why), "{from}");
        }
    }

    fn found_from(page: &LayoutPage) -> Vec<FoundBlock> {
        page.blocks.iter().filter(|b| b.kind != Kind::Skip).map(|b| FoundBlock { kind: b.kind, items: b.items.clone(), unmatched: 0 }).collect()
    }

    #[test]
    fn the_truth_scores_perfectly_and_mistakes_cost() {
        let out = build("d", &pages()).unwrap();
        let mut t = LayoutTally::default();
        t.add_page(&out[0], &found_from(&out[0]));
        assert_eq!(t.order_exact, 1);
        assert_eq!(t.block_rate(), Some(1.0));
        assert_eq!(t.mean_tau(), Some(1.0));
        assert_eq!((t.running_precision(), t.running_recall()), (Some(1.0), Some(1.0)));

        // Merge the two paragraph pieces, call the header a paragraph, reverse the order.
        let mut found = found_from(&out[0]);
        let last = found.remove(6);
        found[5].items.extend(last.items);
        found[0].kind = Kind::Paragraph;
        found.reverse();
        let mut bad = LayoutTally::default();
        bad.add_page(&out[0], &found);
        let all = bad.overall();
        assert_eq!(all.merged, 2);
        assert!(all.kind_right < all.truth);
        assert_eq!(bad.order_exact, 0);
        assert!(bad.mean_tau().unwrap() < 0.0);
        assert_eq!(bad.running_recall(), Some(0.5));
    }

    #[test]
    fn section_trees_score_levels_and_parents() {
        let mut p = DviPage::default();
        for (i, k) in ["heading1", "heading2", "heading2", "heading1"].iter().enumerate() {
            p.events.push((p.items.len(), format!("par:begin:{k}:1")));
            p.items.push((glyph(65, 0, (i as i64 + 1) * 20 * PT), Owner::Outside));
        }
        let out = build("d", &[p]).unwrap();
        let found = vec![found_from(&out[0])];
        let right = [(1, None), (2, Some(0)), (2, Some(0)), (1, None)];
        let sections: Vec<FoundSection> =
            right.iter().enumerate().map(|(i, &(level, parent))| FoundSection { page: 0, block: i, level, parent }).collect();
        let mut t = LayoutTally::default();
        t.add_sections(&[&out[0]], &found, &sections);
        assert_eq!((t.sections_matched, t.sections_level, t.sections_parent), (4, 4, 4));
        let flat: Vec<FoundSection> = (0..4).map(|i| FoundSection { page: 0, block: i, level: 1, parent: None }).collect();
        let mut f = LayoutTally::default();
        f.add_sections(&[&out[0]], &found, &flat);
        assert_eq!((f.sections_level, f.sections_parent), (2, 2));
    }

    #[test]
    fn the_package_brackets_only_with_specials() {
        let on = package(true);
        let off = package(false);
        for m in ["\\pdfcraftopen", "\\pdfcraftclose", "\\pdfcraftenv", "\\pdfcraftbegin", "\\pdfcraftend"] {
            assert!(on.contains(m) && off.contains(m), "{m}");
        }
        assert!(!off.contains("special") && !off.contains("Hook") && !off.contains("patchcmd"));
        assert!(on.contains("para/begin") && on.contains("\\@startsection"));
        assert_eq!(kendall_tau(&[2, 1]), Some(-1.0));
        assert_eq!(kendall_tau(&[1]), None);
    }
}
