//! TeX as the oracle for theorem-like statements, proofs and in-text references: which glyphs make
//! up each statement (Theorem, Lemma, Definition, Remark, Proof, …) with its printed word and
//! number, which glyphs make up each mention of a block ("Lemma 2.1", "(3.4)", "Section 3"), and
//! which block each mention points at, or that it points outside the document.
//!
//! The method is the layout oracle's ([`super::layout`]), on top of it: the document is compiled to
//! DVI with brackets, `dvitype` lists every glyph and rule with the brackets in shipping order, and
//! the layout truth is built as before. The brackets here are `\special{pdfcraft:layout:x:…}`,
//! which the layout truth ignores:
//!
//! - `x:thm:begin:ID:LABEL` and `x:thm:end` around a statement or proof, emitted at the start of the
//!   paragraph that follows `\pdfcraftthm{ID}` and `\pdfcraftthmend` in the source (or at the next
//!   display), so no whatsit lands anywhere a paragraph's hook does not already put one. `LABEL` is
//!   `\@currentlabel` there: the number amsthm just stepped.
//! - `x:target:ID` at the start of the paragraph after `\pdfcraftmark{ID}`: a heading a label
//!   names; or where `\pdfcraftmarkhere{ID}` stands, beside a caption's own layout bracket (a short
//!   caption is a box, not a paragraph). Equations are known by their own `pdfcraft:eq:ID` brackets.
//! - `x:ref:begin:ID`, `x:ref:end` around the part of a mention that should be clickable, and
//!   `x:extra:begin`, `x:extra:end` around what may go with it either way (the `(ii)` of
//!   "Proposition 2.1(ii)").
//!
//! What each id stands for (a statement's printed word, whether it is numbered, its name, what a
//! proof proves; a mention's target) is the generator's, which wrote the source: [`StatementSpec`],
//! [`TargetSpec`], [`MentionSpec`]. A statement's glyphs are those shipped between its brackets that
//! the layout truth gives to a paragraph, display, list item or other text (not a float, footnote or
//! running head shipped meanwhile). A target's glyphs are its layout block's, or its statement's.
//!
//! The package also guards one thing the layout package did not meet: a paragraph that is empty
//! after a display (a statement or proof ending in one) is dropped by TeX, unless the
//! paragraph-end bracket is put in it. Its hook is replaced by one that leaves an empty paragraph
//! alone.

use super::layout::{Kind, LayoutPage, LayoutTruth};
use super::{DviPage, Owner, TexError};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;

/// A found statement stands for a truth statement when their glyph sets agree with an F1 at least
/// this high (its extent is right).
pub const EXTENT_F1: f64 = 0.9;
/// A found statement is the detection of a truth statement (its head was found, the extent may be
/// off) when their glyph sets agree with an F1 at least this high.
pub const DETECT_F1: f64 = 0.5;
/// A found mention is a truth mention when their clickable glyphs agree with an F1 at least this
/// high (the truth's optional extra, the `(ii)`, counts on neither side).
pub const MENTION_F1: f64 = 0.8;
/// A found target is a truth target when their glyph sets agree with an F1 at least this high: the
/// question is which block, not where it ends.
pub const TARGET_F1: f64 = 0.5;
/// Longest id or label read from a bracket.
pub const MAX_ID_BYTES: usize = 64;

/// The LaTeX package `pdfcraftrefs` (load it after `amsmath`, `amsthm` and `hyperref`; it
/// loads `pdfcraftlayout`). With `specials` false it defines the source macros as nothing.
pub fn package(specials: bool) -> String {
    let head = "\\NeedsTeXFormat{LaTeX2e}\n\\ProvidesPackage{pdfcraftrefs}[2026/11/17 PdfCraft reference oracle brackets]\n\\RequirePackage{pdfcraftlayout}\n";
    if !specials {
        return format!(
            "{head}% The brackets expand to nothing: the PDF must come out byte-identical with them.\n\
             \\newcommand*\\pdfcraftthm[1]{{}}\n\\newcommand*\\pdfcraftthmend{{}}\n\\newcommand*\\pdfcraftmark[1]{{}}\n\
             \\newcommand*\\pdfcraftmarkhere[1]{{}}\n\
             \\newcommand*\\pdfcraftref[1]{{}}\n\\newcommand*\\pdfcraftrefend{{}}\n\
             \\newcommand*\\pdfcraftextra{{}}\n\\newcommand*\\pdfcraftextraend{{}}\n"
        );
    }
    format!(
        "{head}\\def\\pdfcraft@x#1{{\\special{{pdfcraft:layout:x:#1}}}}\n\
         \\global\\let\\pdfcraft@thmnext\\@empty\n\
         \\global\\let\\pdfcraft@leave\\@empty\n\
         \\global\\let\\pdfcraft@marknext\\@empty\n\
         % A statement ends where the next paragraph or display begins.\n\
         \\def\\pdfcraft@flush{{\\ifx\\pdfcraft@leave\\@empty\\else\\pdfcraft@x{{thm:end}}\\global\\let\\pdfcraft@leave\\@empty\\fi}}\n\
         \\AddToHook{{para/begin}}{{\\pdfcraft@flush\n\
         \\ifx\\pdfcraft@marknext\\@empty\\else\\pdfcraft@x{{target:\\pdfcraft@marknext}}\\global\\let\\pdfcraft@marknext\\@empty\\fi\n\
         \\ifx\\pdfcraft@thmnext\\@empty\\else\\pdfcraft@x{{thm:begin:\\pdfcraft@thmnext:\\@currentlabel}}\\global\\let\\pdfcraft@thmnext\\@empty\\fi}}\n\
         \\newcommand*\\pdfcraftthm[1]{{\\gdef\\pdfcraft@thmnext{{#1}}}}\n\
         \\newcommand*\\pdfcraftthmend{{\\gdef\\pdfcraft@leave{{1}}}}\n\
         \\newcommand*\\pdfcraftmark[1]{{\\gdef\\pdfcraft@marknext{{#1}}}}\n\
         \\newcommand*\\pdfcraftmarkhere[1]{{\\pdfcraft@x{{target:#1}}}}\n\
         \\newcommand*\\pdfcraftref[1]{{\\pdfcraft@x{{ref:begin:#1}}}}\n\
         \\newcommand*\\pdfcraftrefend{{\\pdfcraft@x{{ref:end}}}}\n\
         \\newcommand*\\pdfcraftextra{{\\pdfcraft@x{{extra:begin}}}}\n\
         \\newcommand*\\pdfcraftextraend{{\\pdfcraft@x{{extra:end}}}}\n\
         \\renewcommand*\\pdfcraftbegin[1]{{\\pdfcraft@flush\\special{{pdfcraft:eq:#1:begin}}}}\n\
         % An empty paragraph after a display is dropped by TeX unless something is in it.\n\
         \\RemoveFromHook{{para/end}}[pdfcraftlayout]\n\
         \\AddToHook{{para/end}}{{\\ifnum\\lastnodetype=-1 \\else\\pdfcraft@s{{par:end:\\the\\currentgrouplevel}}\\fi}}\n\
         % \\eqref's number is a mention, not an equation's tag: amsmath's own \\eqref with \\maketag@@@ as it was.\n\
         \\expandafter\\let\\expandafter\\pdfcraft@eqref\\csname eqref \\endcsname\n\
         \\def\\pdfcraft@plaintag#1{{\\hbox{{\\m@th\\normalfont#1}}}}\n\
         \\DeclareRobustCommand{{\\eqref}}[1]{{{{\\let\\maketag@@@\\pdfcraft@plaintag\\pdfcraft@eqref{{#1}}}}}}\n"
    )
}

/// What a label names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum TargetKind {
    Statement,
    Heading,
    Equation,
    Float,
}

impl TargetKind {
    pub const ALL: [TargetKind; 4] = [TargetKind::Statement, TargetKind::Heading, TargetKind::Equation, TargetKind::Float];

    pub fn as_str(self) -> &'static str {
        match self {
            TargetKind::Statement => "statement",
            TargetKind::Heading => "heading",
            TargetKind::Equation => "equation",
            TargetKind::Float => "float",
        }
    }

    pub fn parse(s: &str) -> Option<TargetKind> {
        TargetKind::ALL.into_iter().find(|k| k.as_str() == s)
    }
}

/// A statement or proof as the generator wrote it.
#[derive(Clone, Debug, PartialEq)]
pub struct StatementSpec {
    pub id: String,
    /// The printed word: `Theorem`, `Lemma`, `Proof`, `Main Theorem`.
    pub kind: String,
    pub numbered: bool,
    /// The name in parentheses, as printed.
    pub name: Option<String>,
    /// The theorem style: `plain`, `definition`, `remark`, `proof`.
    pub style: String,
    pub proof: bool,
    /// For a proof: the statement id it proves.
    pub proves: Option<String>,
    /// For a proof: whether it ends with a mark (a box, a square, `QED`).
    pub end_mark: bool,
}

/// How a target's glyphs are found.
#[derive(Clone, Debug, PartialEq)]
pub enum Anchor {
    /// The statement with this id.
    Statement(String),
    /// The layout block that holds the first item after `x:target:ID` for the target's id.
    Mark,
    /// The layout block that holds the equation bracketed `pdfcraft:eq:ID`.
    Equation(String),
}

/// A label as the generator wrote it.
#[derive(Clone, Debug, PartialEq)]
pub struct TargetSpec {
    /// The label key.
    pub id: String,
    pub kind: TargetKind,
    pub anchor: Anchor,
}

/// A mention as the generator wrote it.
#[derive(Clone, Debug, PartialEq)]
pub struct MentionSpec {
    pub id: String,
    /// The label key it points at; `None` for one that points outside the document or at nothing.
    pub target: Option<String>,
    /// How it is written (`word`, `bare`, `plural`, `range`, `extra`, `external`, `missing`, `head`).
    pub form: String,
}

/// Items of a document: per page (from 0) the indices into that page's [`LayoutPage::items`].
pub type Items = Vec<(usize, Vec<usize>)>;

/// A statement of the truth.
#[derive(Clone, Debug, PartialEq)]
pub struct StatementTruth {
    pub doc: String,
    pub spec: StatementSpec,
    /// The number as printed (`\@currentlabel` when its paragraph began), for a numbered one.
    pub number: Option<String>,
    pub items: Items,
}

/// A target of the truth.
#[derive(Clone, Debug, PartialEq)]
pub struct TargetTruth {
    pub doc: String,
    pub id: String,
    pub kind: TargetKind,
    /// For a statement target: the statement's id.
    pub statement: Option<String>,
    pub items: Items,
}

/// A mention of the truth.
#[derive(Clone, Debug, PartialEq)]
pub struct MentionTruth {
    pub doc: String,
    pub id: String,
    pub form: String,
    pub target: Option<String>,
    pub items: Items,
    pub extra: Items,
}

/// What kind of paper a document is beyond its layout, for grouping scores.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefsPaper {
    pub doc: String,
    /// How statement heads are set: `bold`, `italic`, `smallcaps`.
    pub heads: String,
    /// `section` or `global`.
    pub numbering: String,
    pub hyperref: bool,
}

/// A reference truth file: the layout truth of the papers, and their statements, targets and
/// mentions.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RefsTruth {
    pub layout: LayoutTruth,
    pub papers: Vec<RefsPaper>,
    pub statements: Vec<StatementTruth>,
    pub targets: Vec<TargetTruth>,
    pub mentions: Vec<MentionTruth>,
}

fn bad(what: impl std::fmt::Display) -> TexError {
    TexError::Listing(format!("reference truth: {what}"))
}

fn push_item(items: &mut Items, page: usize, item: usize) {
    match items.last_mut() {
        Some((p, list)) if *p == page => list.push(item),
        _ => items.push((page, vec![item])),
    }
}

/// The layout block kinds a statement's glyphs may be in.
fn statement_text(kind: Kind) -> bool {
    matches!(kind, Kind::Paragraph | Kind::Display | Kind::ListItem | Kind::Other)
}

/// What [`build`] gives: a document's statements, targets and mentions.
pub type DocTruth = (Vec<StatementTruth>, Vec<TargetTruth>, Vec<MentionTruth>);

/// The statements, targets and mentions of document `doc` from its DVI pages and its layout truth
/// (`layout`, built from the same pages by [`super::layout::build`]).
///
/// Fails on brackets that do not follow (a statement or mention opened inside another of its kind,
/// one never closed, an id the specs do not know), and on a target whose anchor is not found.
pub fn build(
    doc: &str,
    pages: &[DviPage],
    layout: &[LayoutPage],
    statements: &[StatementSpec],
    targets: &[TargetSpec],
    mentions: &[MentionSpec],
) -> Result<DocTruth, TexError> {
    let mut st: Vec<StatementTruth> =
        statements.iter().map(|s| StatementTruth { doc: doc.to_owned(), spec: s.clone(), number: None, items: Vec::new() }).collect();
    let mut mt: Vec<MentionTruth> = mentions
        .iter()
        .map(|m| MentionTruth {
            doc: doc.to_owned(),
            id: m.id.clone(),
            form: m.form.clone(),
            target: m.target.clone(),
            items: Vec::new(),
            extra: Vec::new(),
        })
        .collect();
    let mut seen_statement = vec![false; st.len()];
    let mut seen_mention = vec![false; mt.len()];
    let mut open_statement: Option<usize> = None;
    let mut open_mention: Option<usize> = None;
    let mut last_mention: Option<usize> = None;
    let mut in_extra = false;
    let mut pending_marks: Vec<String> = Vec::new();
    // Mark id -> (page, item) of the first item after it.
    let mut marks: HashMap<String, (usize, usize)> = HashMap::new();
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
                    ("thm", "end") => {
                        if open_statement.take().is_none() {
                            return Err(bad(format!("{doc} page {pi}: a statement ends that never began")));
                        }
                    }
                    ("thm", rest) => {
                        let Some(rest) = rest.strip_prefix("begin:") else { return Err(bad(format!("{doc}: unknown bracket `{text}`"))) };
                        let (id, label) = rest.split_once(':').unwrap_or((rest, ""));
                        if open_statement.is_some() {
                            return Err(bad(format!("{doc} page {pi}: statement `{id}` begins inside another")));
                        }
                        let k = st.iter().position(|s| s.spec.id == id).ok_or_else(|| bad(format!("{doc}: unknown statement `{id}`")))?;
                        if std::mem::replace(&mut seen_statement[k], true) {
                            return Err(bad(format!("{doc}: statement `{id}` begins twice")));
                        }
                        if let Some(s) = st.get_mut(k)
                            && s.spec.numbered
                        {
                            if label.is_empty() || label.len() > MAX_ID_BYTES {
                                return Err(bad(format!("{doc}: numbered statement `{id}` has no number")));
                            }
                            s.number = Some(label.to_owned());
                        }
                        open_statement = Some(k);
                    }
                    ("target", id) => {
                        if id.is_empty() || id.len() > MAX_ID_BYTES {
                            return Err(bad(format!("{doc}: bad target bracket `{text}`")));
                        }
                        pending_marks.push(id.to_owned());
                    }
                    ("ref", "end") => {
                        if open_mention.take().is_none() {
                            return Err(bad(format!("{doc} page {pi}: a mention ends that never began")));
                        }
                    }
                    ("ref", rest) => {
                        let Some(id) = rest.strip_prefix("begin:") else { return Err(bad(format!("{doc}: unknown bracket `{text}`"))) };
                        if open_mention.is_some() {
                            return Err(bad(format!("{doc} page {pi}: mention `{id}` begins inside another")));
                        }
                        let k = mt.iter().position(|m| m.id == id).ok_or_else(|| bad(format!("{doc}: unknown mention `{id}`")))?;
                        if std::mem::replace(&mut seen_mention[k], true) {
                            return Err(bad(format!("{doc}: mention `{id}` is bracketed twice")));
                        }
                        open_mention = Some(k);
                        last_mention = Some(k);
                    }
                    ("extra", "begin") if !in_extra && open_mention.is_none() && last_mention.is_some() => in_extra = true,
                    ("extra", "end") if in_extra => in_extra = false,
                    _ => return Err(bad(format!("{doc} page {pi}: unexpected bracket `{text}`"))),
                }
            }
            if i == page.items.len() {
                break;
            }
            for id in pending_marks.drain(..) {
                marks.entry(id).or_insert((pi, i));
            }
            let kind = lp.blocks.get(block_of.get(i).copied().unwrap_or(usize::MAX)).map(|b| b.kind);
            if let Some(k) = open_statement
                && kind.is_some_and(statement_text)
                && let Some(s) = st.get_mut(k)
            {
                push_item(&mut s.items, pi, i);
            }
            if let Some(k) = open_mention
                && let Some(m) = mt.get_mut(k)
            {
                push_item(&mut m.items, pi, i);
            }
            if in_extra && let Some(m) = last_mention.and_then(|k| mt.get_mut(k)) {
                push_item(&mut m.extra, pi, i);
            }
        }
    }
    if let Some(k) = open_mention {
        return Err(bad(format!("{doc}: mention `{}` never ends", mt.get(k).map_or("?", |m| m.id.as_str()))));
    }
    if let Some(s) = st.iter().zip(&seen_statement).find(|(_, seen)| !**seen) {
        return Err(bad(format!("{doc}: statement `{}` is never bracketed", s.0.spec.id)));
    }
    if let Some(s) = st.iter().find(|s| s.items.is_empty()) {
        return Err(bad(format!("{doc}: statement `{}` has no glyphs", s.spec.id)));
    }
    if let Some(m) = mt.iter().find(|m| m.items.is_empty()) {
        return Err(bad(format!("{doc}: mention `{}` has no glyphs", m.id)));
    }
    // Targets: a layout block, or a statement.
    let block_items = |page: usize, item: usize| -> Option<Items> {
        let lp = layout.get(page)?;
        let b = lp.blocks.iter().find(|b| b.items.contains(&item))?;
        Some(vec![(page, b.items.clone())])
    };
    let mut tt = Vec::with_capacity(targets.len());
    for t in targets {
        let items = match &t.anchor {
            Anchor::Statement(id) => st.iter().find(|s| &s.spec.id == id).map(|s| s.items.clone()),
            Anchor::Mark => marks.get(&t.id).and_then(|&(p, i)| block_items(p, i)),
            Anchor::Equation(eq) => pages.iter().enumerate().find_map(|(pi, page)| {
                let e = page.equations.iter().position(|x| x == eq)?;
                let first = page.items.iter().position(|(_, o)| *o == Owner::Equation(e))?;
                block_items(pi, first)
            }),
        };
        let items = items.ok_or_else(|| bad(format!("{doc}: target `{}` ({:?}) is not found", t.id, t.anchor)))?;
        let statement = match &t.anchor {
            Anchor::Statement(id) => Some(id.clone()),
            _ => None,
        };
        tt.push(TargetTruth { doc: doc.to_owned(), id: t.id.clone(), kind: t.kind, statement, items });
    }
    Ok((st, tt, mt))
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

fn dash(v: Option<&str>) -> &str {
    match v {
        Some(s) if !s.is_empty() => s,
        _ => "-",
    }
}

fn undash(v: &str) -> Option<String> {
    (v != "-").then(|| v.to_owned())
}

fn clean(s: &str) -> String {
    s.replace(['\t', '\n', '\r'], " ")
}

impl RefsTruth {
    /// The file [`RefsTruth::parse`] reads: the layout truth's rows ([`LayoutTruth::to_tsv`]), then
    ///
    /// ```text
    /// Y  doc  heads  numbering  hyperref                               a paper
    /// S  doc  id  kind  number  numbered  name  style  proof  proves  end_mark   a statement
    /// T  doc  id  kind  statement                                      a target (label key)
    /// M  doc  id  target  form                                         a mention
    /// I  doc  id  page  ranges                                         items of a statement, target or mention
    /// E  doc  id  page  ranges                                         a mention's optional extra
    /// ```
    ///
    /// `-` is none; a statement target's items are its statement's (no `I` rows).
    pub fn to_tsv(&self, header: &str) -> String {
        let mut s = self.layout.to_tsv(header);
        let items = |s: &mut String, row: &str, doc: &str, id: &str, list: &Items| {
            for (page, l) in list {
                let _ = writeln!(s, "{row}\t{doc}\t{id}\t{page}\t{}", ranges(l));
            }
        };
        for p in &self.papers {
            let _ = writeln!(s, "Y\t{}\t{}\t{}\t{}", p.doc, p.heads, p.numbering, u8::from(p.hyperref));
        }
        for t in &self.statements {
            let sp = &t.spec;
            let _ = writeln!(
                s,
                "S\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                t.doc,
                sp.id,
                clean(&sp.kind),
                dash(t.number.as_deref()),
                u8::from(sp.numbered),
                dash(sp.name.as_deref().map(clean).as_deref()),
                sp.style,
                u8::from(sp.proof),
                dash(sp.proves.as_deref()),
                u8::from(sp.end_mark)
            );
            items(&mut s, "I", &t.doc, &sp.id, &t.items);
        }
        for t in &self.targets {
            let _ = writeln!(s, "T\t{}\t{}\t{}\t{}", t.doc, t.id, t.kind.as_str(), dash(t.statement.as_deref()));
            if t.kind != TargetKind::Statement {
                items(&mut s, "I", &t.doc, &t.id, &t.items);
            }
        }
        for m in &self.mentions {
            let _ = writeln!(s, "M\t{}\t{}\t{}\t{}", m.doc, m.id, dash(m.target.as_deref()), m.form);
            items(&mut s, "I", &m.doc, &m.id, &m.items);
            items(&mut s, "E", &m.doc, &m.id, &m.extra);
        }
        s
    }

    pub fn parse(tsv: &str) -> Result<RefsTruth, TexError> {
        let mut layout_rows = String::new();
        let mut out = RefsTruth::default();
        // (doc, id) -> where its items go.
        #[derive(Clone, Copy)]
        enum Slot {
            Statement(usize),
            Target(usize),
            Mention(usize),
        }
        let mut slots: HashMap<(String, String), Slot> = HashMap::new();
        for (n, line) in tsv.lines().enumerate() {
            let line_no = n.saturating_add(1);
            if line_no > super::layout::MAX_ROWS {
                return Err(bad(format!("more than {} rows", super::layout::MAX_ROWS)));
            }
            let f: Vec<&str> = line.split('\t').collect();
            let get = |i: usize| f.get(i).copied().ok_or_else(|| bad(format!("line {line_no}: column {} is missing", i.saturating_add(1))));
            match f.first().copied() {
                Some("Y") => out.papers.push(RefsPaper {
                    doc: get(1)?.to_owned(),
                    heads: get(2)?.to_owned(),
                    numbering: get(3)?.to_owned(),
                    hyperref: get(4)? == "1",
                }),
                Some("S") => {
                    let doc = get(1)?.to_owned();
                    let id = get(2)?.to_owned();
                    let spec = StatementSpec {
                        id: id.clone(),
                        kind: get(3)?.to_owned(),
                        numbered: get(5)? == "1",
                        name: undash(get(6)?),
                        style: get(7)?.to_owned(),
                        proof: get(8)? == "1",
                        proves: undash(get(9)?),
                        end_mark: get(10)? == "1",
                    };
                    if slots.insert((doc.clone(), id.clone()), Slot::Statement(out.statements.len())).is_some() {
                        return Err(bad(format!("line {line_no}: `{id}` is listed twice")));
                    }
                    out.statements.push(StatementTruth { doc, spec, number: undash(get(4)?), items: Vec::new() });
                }
                Some("T") => {
                    let doc = get(1)?.to_owned();
                    let id = get(2)?.to_owned();
                    let kind = TargetKind::parse(get(3)?).ok_or_else(|| bad(format!("line {line_no}: unknown target kind")))?;
                    let statement = undash(get(4)?);
                    let items = if let Some(sid) = statement.as_ref().filter(|_| kind == TargetKind::Statement) {
                        match slots.get(&(doc.clone(), sid.clone())) {
                            Some(Slot::Statement(k)) => out.statements.get(*k).map(|s| s.items.clone()).unwrap_or_default(),
                            _ => Vec::new(),
                        }
                    } else {
                        Vec::new()
                    };
                    let key = (doc.clone(), format!("target:{id}"));
                    if slots.insert(key, Slot::Target(out.targets.len())).is_some() {
                        return Err(bad(format!("line {line_no}: target `{id}` is listed twice")));
                    }
                    out.targets.push(TargetTruth { doc, id, kind, statement, items });
                }
                Some("M") => {
                    let doc = get(1)?.to_owned();
                    let id = get(2)?.to_owned();
                    if slots.insert((doc.clone(), id.clone()), Slot::Mention(out.mentions.len())).is_some() {
                        return Err(bad(format!("line {line_no}: `{id}` is listed twice")));
                    }
                    out.mentions.push(MentionTruth {
                        doc,
                        id,
                        target: undash(get(3)?),
                        form: get(4)?.to_owned(),
                        items: Vec::new(),
                        extra: Vec::new(),
                    });
                }
                Some(row @ ("I" | "E")) => {
                    let doc = get(1)?.to_owned();
                    let id = get(2)?.to_owned();
                    let page: usize = get(3)?.parse().map_err(|_| bad(format!("line {line_no}: bad page")))?;
                    let list = parse_ranges(get(4)?, line_no)?;
                    // Statement and mention ids are the generator's (`s3`, `m7`), label keys the
                    // source's (`lem:main`): they never meet.
                    let slot = slots
                        .get(&(doc.clone(), id.clone()))
                        .or_else(|| slots.get(&(doc.clone(), format!("target:{id}"))))
                        .copied()
                        .ok_or_else(|| bad(format!("line {line_no}: `{id}` is not listed before its items")))?;
                    let dest = match (row, slot) {
                        ("I", Slot::Statement(k)) => out.statements.get_mut(k).map(|s| &mut s.items),
                        ("I", Slot::Target(k)) => out.targets.get_mut(k).map(|t| &mut t.items),
                        ("I", Slot::Mention(k)) => out.mentions.get_mut(k).map(|m| &mut m.items),
                        ("E", Slot::Mention(k)) => out.mentions.get_mut(k).map(|m| &mut m.extra),
                        _ => None,
                    };
                    let dest = dest.ok_or_else(|| bad(format!("line {line_no}: `{id}` takes no such rows")))?;
                    dest.push((page, list));
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

    pub fn paper(&self, doc: &str) -> Option<&RefsPaper> {
        self.papers.iter().find(|p| p.doc == doc)
    }
}

// --- Scoring -------------------------------------------------------------------------------------

/// An item of a document: (page, index into that page's [`LayoutPage::items`]).
pub type ItemRef = (usize, usize);

fn flat(items: &Items) -> HashSet<ItemRef> {
    items.iter().flat_map(|(p, l)| l.iter().map(move |&i| (*p, i))).collect()
}

/// A statement an analysis found.
#[derive(Clone, Debug, PartialEq)]
pub struct FoundStatement {
    pub kind: String,
    pub number: Option<String>,
    pub name: Option<String>,
    pub proof: bool,
    /// The truth items its glyphs and rules are.
    pub items: Vec<ItemRef>,
    /// Its glyphs and rules that are no item of the truth.
    pub unmatched: usize,
    /// For a proof: the index of the found statement it proves.
    pub proves: Option<usize>,
    pub end_mark: bool,
}

/// The block a found mention points at, as a set of glyphs.
#[derive(Clone, Debug, PartialEq)]
pub struct FoundTarget {
    pub items: Vec<ItemRef>,
    pub unmatched: usize,
}

/// A mention an analysis found.
#[derive(Clone, Debug, PartialEq)]
pub struct FoundMention {
    pub items: Vec<ItemRef>,
    pub unmatched: usize,
    pub target: Option<FoundTarget>,
}

fn f1(common: usize, a: usize, b: usize) -> f64 {
    if a + b == 0 { 0.0 } else { 2.0 * common as f64 / (a + b) as f64 }
}

/// Counts for statements of one kind (or all).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct StatementCounts {
    pub truth: usize,
    /// Found with their glyphs (F1 ≥ [`DETECT_F1`]).
    pub detected: usize,
    /// Of those: the printed word, the number, the name right; the extent (F1 ≥ [`EXTENT_F1`]).
    pub kind_right: usize,
    pub number_right: usize,
    pub name_right: usize,
    pub extent_right: usize,
    /// Found statements of this kind that stand for no truth statement.
    pub found: usize,
    pub spurious: usize,
}

/// Counts for proofs.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ProofCounts {
    /// Detected proofs, and of those whose `proves` is the right statement, a wrong one, none.
    pub detected: usize,
    pub proves_right: usize,
    pub proves_wrong: usize,
    pub proves_none: usize,
    /// Detected proofs with an end mark in the truth, and of those found with one; without one in
    /// the truth, and of those found with one.
    pub marked: usize,
    pub marked_found: usize,
    pub unmarked: usize,
    pub unmarked_found: usize,
}

/// Counts for mentions of one kind of target (or all).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MentionCounts {
    pub truth: usize,
    /// Found with their clickable glyphs (F1 ≥ [`MENTION_F1`]).
    pub matched: usize,
    /// Of the matched: the right target; none where the truth has none; a target where the truth
    /// has a different one or none (a wrong link); none where the truth has one.
    pub right: usize,
    pub none_right: usize,
    pub wrong: usize,
    pub missed: usize,
}

/// The scores of some documents.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RefsTally {
    pub docs: usize,
    /// By the printed word, lower case (`theorem`, `proof`), and all under `*`.
    pub statements: BTreeMap<String, StatementCounts>,
    pub proofs: ProofCounts,
    /// By what the truth mention points at: a statement's word, `heading`, `equation`, `float`,
    /// `none` (outside the document or at nothing); all under `*`.
    pub mentions: BTreeMap<String, MentionCounts>,
    /// Found mentions, those that stand for a truth mention, and those that stand for none but
    /// point somewhere: links where the text has no mention (wrong links too).
    pub mentions_found: usize,
    pub mentions_found_matched: usize,
    pub spurious_links: usize,
}

impl RefsTally {
    /// Every wrong link: a matched mention linked to the wrong block or to one where it points
    /// nowhere, and a link where there is no mention.
    pub fn wrong_links(&self) -> usize {
        self.mentions.get("*").map_or(0, |c| c.wrong).saturating_add(self.spurious_links)
    }

    /// Adds one document: `truth` (all of the truth, the document's rows are picked by `doc`) and
    /// what the analysis found in it.
    pub fn add_document(&mut self, truth: &RefsTruth, doc: &str, statements: &[FoundStatement], mentions: &[FoundMention]) {
        self.docs += 1;
        self.add_statements(truth, doc, statements);
        self.add_mentions(truth, doc, mentions);
    }

    fn add_statements(&mut self, truth: &RefsTruth, doc: &str, statements: &[FoundStatement]) {
        let ts: Vec<&StatementTruth> = truth.statements.iter().filter(|s| s.doc == doc).collect();
        let sets: Vec<HashSet<ItemRef>> = ts.iter().map(|s| flat(&s.items)).collect();
        // Truth statement -> found statement: greedy by overlap.
        let mut pairs: Vec<(usize, usize, f64)> = Vec::new();
        for (ti, set) in sets.iter().enumerate() {
            for (fi, f) in statements.iter().enumerate() {
                let common = f.items.iter().filter(|i| set.contains(i)).count();
                if common > 0 {
                    pairs.push((ti, fi, f1(common, set.len(), f.items.len().saturating_add(f.unmatched))));
                }
            }
        }
        pairs.sort_by(|a, b| b.2.total_cmp(&a.2));
        let mut t_found: Vec<Option<(usize, f64)>> = vec![None; ts.len()];
        let mut f_truth: Vec<Option<usize>> = vec![None; statements.len()];
        for (ti, fi, score) in pairs {
            if score < DETECT_F1 || t_found.get(ti).is_some_and(Option::is_some) || f_truth.get(fi).is_some_and(Option::is_some) {
                continue;
            }
            if let Some(slot) = t_found.get_mut(ti) {
                *slot = Some((fi, score));
            }
            if let Some(slot) = f_truth.get_mut(fi) {
                *slot = Some(ti);
            }
        }
        let norm = |s: &str| s.trim().to_lowercase();
        for (ti, t) in ts.iter().enumerate() {
            let key = norm(&t.spec.kind);
            let found = t_found.get(ti).copied().flatten();
            for k in [key.as_str(), "*"] {
                let c = self.statements.entry(k.to_owned()).or_default();
                c.truth += 1;
                let Some((fi, score)) = found else { continue };
                let Some(f) = statements.get(fi) else { continue };
                c.detected += 1;
                c.kind_right += usize::from(norm(&f.kind) == key);
                c.number_right += usize::from(f.number == t.number);
                c.name_right += usize::from(f.name.as_deref().map(norm) == t.spec.name.as_deref().map(norm));
                c.extent_right += usize::from(score >= EXTENT_F1);
            }
            if t.spec.proof
                && let Some((fi, _)) = found
                && let Some(f) = statements.get(fi)
            {
                let p = &mut self.proofs;
                p.detected += 1;
                let want = t.spec.proves.as_deref().and_then(|id| ts.iter().position(|s| s.spec.id == id));
                let got = f.proves.and_then(|k| f_truth.get(k).copied().flatten());
                match (f.proves, want) {
                    (None, _) => p.proves_none += 1,
                    (Some(_), Some(w)) if got == Some(w) => p.proves_right += 1,
                    _ => p.proves_wrong += 1,
                }
                if t.spec.end_mark {
                    p.marked += 1;
                    p.marked_found += usize::from(f.end_mark);
                } else {
                    p.unmarked += 1;
                    p.unmarked_found += usize::from(f.end_mark);
                }
            }
        }
        for (fi, f) in statements.iter().enumerate() {
            let key = norm(&f.kind);
            for k in [key.as_str(), "*"] {
                let c = self.statements.entry(k.to_owned()).or_default();
                c.found += 1;
                c.spurious += usize::from(f_truth.get(fi).is_some_and(Option::is_none));
            }
        }
    }

    fn add_mentions(&mut self, truth: &RefsTruth, doc: &str, mentions: &[FoundMention]) {
        let norm = |s: &str| s.trim().to_lowercase();
        let targets: HashMap<&str, (&TargetTruth, HashSet<ItemRef>)> =
            truth.targets.iter().filter(|t| t.doc == doc).map(|t| (t.id.as_str(), (t, flat(&t.items)))).collect();
        let tm: Vec<&MentionTruth> = truth.mentions.iter().filter(|m| m.doc == doc).collect();
        let extra: HashSet<ItemRef> = tm.iter().flat_map(|m| flat(&m.extra)).collect();
        let msets: Vec<HashSet<ItemRef>> = tm.iter().map(|m| flat(&m.items)).collect();
        let mut pairs: Vec<(usize, usize, f64)> = Vec::new();
        for (ti, set) in msets.iter().enumerate() {
            for (fi, f) in mentions.iter().enumerate() {
                let common = f.items.iter().filter(|i| set.contains(i)).count();
                if common > 0 {
                    let size = f.items.iter().filter(|i| !extra.contains(i)).count().saturating_add(f.unmatched);
                    pairs.push((ti, fi, f1(common, set.len(), size)));
                }
            }
        }
        pairs.sort_by(|a, b| b.2.total_cmp(&a.2));
        let mut t_found: Vec<Option<usize>> = vec![None; tm.len()];
        let mut f_truth: Vec<Option<usize>> = vec![None; mentions.len()];
        for (ti, fi, score) in pairs {
            if score < MENTION_F1 || t_found.get(ti).is_some_and(Option::is_some) || f_truth.get(fi).is_some_and(Option::is_some) {
                continue;
            }
            if let Some(slot) = t_found.get_mut(ti) {
                *slot = Some(fi);
            }
            if let Some(slot) = f_truth.get_mut(fi) {
                *slot = Some(ti);
            }
        }
        let same_target = |found: &FoundTarget, want: &HashSet<ItemRef>| {
            let common = found.items.iter().filter(|i| want.contains(i)).count();
            f1(common, want.len(), found.items.len().saturating_add(found.unmatched)) >= TARGET_F1
        };
        for (ti, t) in tm.iter().enumerate() {
            let want = t.target.as_deref().and_then(|id| targets.get(id));
            let key = match want {
                None => "none".to_owned(),
                Some((tt, _)) if tt.kind == TargetKind::Statement => truth
                    .statements
                    .iter()
                    .find(|s| s.doc == doc && Some(&s.spec.id) == tt.statement.as_ref())
                    .map_or_else(|| "statement".to_owned(), |s| norm(&s.spec.kind)),
                Some((tt, _)) => tt.kind.as_str().to_owned(),
            };
            let found = t_found.get(ti).copied().flatten().and_then(|fi| mentions.get(fi));
            for k in [key.as_str(), "*"] {
                let c = self.mentions.entry(k.to_owned()).or_default();
                c.truth += 1;
                let Some(f) = found else { continue };
                c.matched += 1;
                match (&f.target, want) {
                    (None, None) => c.none_right += 1,
                    (None, Some(_)) => c.missed += 1,
                    (Some(_), None) => c.wrong += 1,
                    (Some(ft), Some((_, set))) => {
                        if same_target(ft, set) {
                            c.right += 1;
                        } else {
                            c.wrong += 1;
                        }
                    }
                }
            }
        }
        self.mentions_found += mentions.len();
        for (fi, f) in mentions.iter().enumerate() {
            if f_truth.get(fi).is_some_and(Option::is_some) {
                self.mentions_found_matched += 1;
            } else if f.target.is_some() {
                self.spurious_links += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tex::layout::{self, LayoutTruth, PaperInfo};
    use crate::tex::{Item, Mark};

    fn glyph(code: u32, h: i64, v: i64) -> Item {
        Item { mark: Mark::Glyph { font: "cmr10".into(), size: 655360, code }, h, v }
    }

    fn page(n: usize, events: &[(usize, &str)]) -> DviPage {
        DviPage {
            items: (0..n).map(|i| (glyph(65, i as i64 * 400_000, 1_000_000), Owner::Outside)).collect(),
            equations: Vec::new(),
            events: events.iter().map(|(a, t)| (*a, (*t).to_owned())).collect(),
        }
    }

    fn spec(id: &str, kind: &str, proof: bool) -> StatementSpec {
        StatementSpec {
            id: id.into(),
            kind: kind.into(),
            numbered: !proof,
            name: None,
            style: if proof { "proof" } else { "plain" }.into(),
            proof,
            proves: proof.then(|| "s1".to_owned()),
            end_mark: proof,
        }
    }

    #[test]
    fn statements_and_mentions_take_the_items_between_their_brackets() {
        let p = page(
            10,
            &[
                (0, "par:begin:par:0"),
                (0, "x:thm:begin:s1:2.1"),
                (3, "x:ref:begin:m1"),
                (5, "x:ref:end"),
                (5, "x:extra:begin"),
                (6, "x:extra:end"),
                (7, "par:end:0"),
                (7, "par:begin:par:0"),
                (7, "x:thm:end"),
                (7, "x:thm:begin:p1:"),
                (10, "par:end:0"),
                (10, "x:thm:end"),
            ],
        );
        let lp = layout::build("d", std::slice::from_ref(&p)).expect("layout");
        let (st, tt, mt) = build(
            "d",
            &[p],
            &lp,
            &[spec("s1", "Lemma", false), spec("p1", "Proof", true)],
            &[TargetSpec { id: "lem".into(), kind: TargetKind::Statement, anchor: Anchor::Statement("s1".into()) }],
            &[MentionSpec { id: "m1".into(), target: Some("lem".into()), form: "word".into() }],
        )
        .expect("truth");
        assert_eq!(st[0].number.as_deref(), Some("2.1"));
        assert_eq!(st[0].items, vec![(0, (0..7).collect())]);
        assert_eq!(st[1].items, vec![(0, (7..10).collect())]);
        assert_eq!(st[1].number, None);
        assert_eq!(mt[0].items, vec![(0, vec![3, 4])]);
        assert_eq!(mt[0].extra, vec![(0, vec![5])]);
        assert_eq!(tt[0].items, st[0].items);

        // The file says the same.
        let truth = RefsTruth {
            layout: LayoutTruth {
                papers: vec![PaperInfo { doc: "d".into(), holdout: false, class: "article".into(), columns: "1col".into(), size: "10pt".into() }],
                pages: lp,
            },
            papers: vec![RefsPaper { doc: "d".into(), heads: "bold".into(), numbering: "section".into(), hyperref: false }],
            statements: st,
            targets: tt,
            mentions: mt,
        };
        let text = truth.to_tsv("test");
        assert_eq!(RefsTruth::parse(&text).expect("parse"), truth);
    }

    #[test]
    fn brackets_that_do_not_follow_are_errors() {
        let lp = |p: &DviPage| layout::build("d", std::slice::from_ref(p)).expect("layout");
        let nested = page(3, &[(0, "par:begin:par:0"), (0, "x:thm:begin:s1:1"), (1, "x:thm:begin:s1:1")]);
        assert!(build("d", std::slice::from_ref(&nested), &lp(&nested), &[spec("s1", "Lemma", false)], &[], &[]).is_err());
        let unknown = page(3, &[(0, "par:begin:par:0"), (0, "x:ref:begin:zz"), (1, "x:ref:end")]);
        assert!(build("d", std::slice::from_ref(&unknown), &lp(&unknown), &[], &[], &[]).is_err());
        let open = page(3, &[(0, "par:begin:par:0"), (0, "x:ref:begin:m1")]);
        let m = MentionSpec { id: "m1".into(), target: None, form: "missing".into() };
        assert!(build("d", std::slice::from_ref(&open), &lp(&open), &[], &[], &[m]).is_err());
        assert!(parse_ranges("5-2", 1).is_err());
        assert!(parse_ranges("0-99999999", 1).is_err());
        assert_eq!(ranges(&[4, 1, 2, 3, 9]), "1-4,9");
        assert_eq!(parse_ranges("1-4,9", 1).expect("ranges"), vec![1, 2, 3, 4, 9]);
    }

    #[test]
    fn the_tally_counts_wrong_links_apart() {
        let truth = RefsTruth {
            targets: vec![
                TargetTruth { doc: "d".into(), id: "a".into(), kind: TargetKind::Equation, statement: None, items: vec![(0, vec![10, 11, 12])] },
                TargetTruth { doc: "d".into(), id: "b".into(), kind: TargetKind::Equation, statement: None, items: vec![(0, vec![20, 21, 22])] },
            ],
            mentions: vec![
                MentionTruth {
                    doc: "d".into(),
                    id: "m1".into(),
                    form: "bare".into(),
                    target: Some("a".into()),
                    items: vec![(0, vec![1, 2])],
                    extra: vec![],
                },
                MentionTruth { doc: "d".into(), id: "m2".into(), form: "external".into(), target: None, items: vec![(0, vec![3, 4])], extra: vec![] },
            ],
            ..Default::default()
        };
        let to = |v: &[usize]| Some(FoundTarget { items: v.iter().map(|&i| (0, i)).collect(), unmatched: 0 });
        let found = [
            FoundMention { items: vec![(0, 1), (0, 2)], unmatched: 0, target: to(&[20, 21, 22]) },
            FoundMention { items: vec![(0, 3), (0, 4)], unmatched: 0, target: to(&[10, 11]) },
            FoundMention { items: vec![(0, 7)], unmatched: 0, target: to(&[10]) },
        ];
        let mut t = RefsTally::default();
        t.add_document(&truth, "d", &[], &found);
        let all = t.mentions.get("*").copied().unwrap_or_default();
        assert_eq!((all.truth, all.matched, all.right, all.wrong), (2, 2, 0, 2));
        assert_eq!(t.spurious_links, 1);
        assert_eq!(t.wrong_links(), 3);
    }
}
