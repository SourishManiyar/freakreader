//! The committed truth: per equation its source, page, box and the glyphs and rules TeX set for
//! it, and per page every glyph and rule with the equation (if any) it belongs to.
//!
//! One tab-separated row per record; `#` starts a comment line.
//!
//! ```text
//! E  id  base  doc  page  category  tier  env  context  x0  y0  x1  y1  source
//! M  id  base  mutation  source
//! G  doc  page  owner  font  size  code  h  v
//! R  doc  page  owner  h  v  width  height
//! ```
//!
//! `E` is one occurrence of an equation of the list (`base`) on page `page` (counted from 0, as
//! `PageRenderer::glyph_details` counts) of the PDF `doc`; `x0 y0 x1 y1` is its box in the page's
//! view space, in bp ([`super::view_bp`]). `M` is a deliberately broken variant of the equation
//! `base` (`mutants.tsv`). `G` and `R` are a glyph and a rule as `dvitype` lists them, in sp from
//! the DVI origin; `owner` is the `E` or `M` id they belong to, `tag` for an equation number, or
//! `-` for anything else on the page.

use super::{Env, Item, Mark, TexError};
use std::collections::HashMap;
use std::fmt::Write as _;

/// The context of an equation on a page of its own.
pub const ALONE: &str = "alone";
/// Most rows read from one truth file.
pub const MAX_ROWS: usize = 2_000_000;
/// Longest LaTeX source kept for one equation, in bytes.
pub const MAX_SOURCE_BYTES: usize = 8192;

/// One occurrence of an equation, with everything TeX set for it.
#[derive(Clone, Debug, PartialEq)]
pub struct EquationTruth {
    /// Unique in the file: the list's id for an equation on a page of its own, `base@page-id` for
    /// one inside running text.
    pub id: String,
    /// The equation's id in the list.
    pub base: String,
    /// The PDF the equation is in, without extension.
    pub doc: String,
    /// Page index in `doc`, from 0.
    pub page: usize,
    pub category: String,
    /// 1 (plain), 2 (two-dimensional structure) or 3 (expected to be hard to recover).
    pub tier: u8,
    pub env: Env,
    /// `alone` (a page of its own), `1col` or `2col` (inside running text).
    pub context: String,
    /// The box of the equation's glyphs and rules, `[x0, y0, x1, y1]` in view-space bp.
    pub bbox: [f64; 4],
    pub source: String,
    /// The glyphs and rules TeX set for it on its page, in sp from the DVI origin, in the order it
    /// set them.
    pub items: Vec<Item>,
    /// What TeX sets for the source alone, in the single-equation template: the equation's own
    /// `items` when it is `alone`, else those of the `alone` occurrence of `base`. In running text
    /// the glue around operators and relations stretches with the line, so placed items are not a
    /// translation of these; [`super::typeset_equal`] compares candidates with these.
    pub reference: Vec<Item>,
}

/// A deliberately broken variant of an equation and what TeX set for it.
#[derive(Clone, Debug, PartialEq)]
pub struct MutantTruth {
    pub id: String,
    pub base: String,
    pub mutation: String,
    pub source: String,
    pub items: Vec<Item>,
}

/// Everything on one page, with owners.
#[derive(Clone, Debug, PartialEq)]
pub struct PageTruth {
    pub doc: String,
    pub page: usize,
    pub items: Vec<(Item, String)>,
}

/// A truth file.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TruthSet {
    pub equations: Vec<EquationTruth>,
    pub mutants: Vec<MutantTruth>,
    pub pages: Vec<PageTruth>,
}

fn field<'a>(f: &[&'a str], i: usize, line: usize) -> Result<&'a str, TexError> {
    f.get(i).copied().ok_or_else(|| TexError::Listing(format!("truth line {line}: column {} is missing", i.saturating_add(1))))
}

fn number<T: std::str::FromStr>(f: &[&str], i: usize, line: usize) -> Result<T, TexError> {
    let s = field(f, i, line)?;
    s.parse().map_err(|_| TexError::Listing(format!("truth line {line}: `{s}` is not a number")))
}

fn text(f: &[&str], i: usize, line: usize) -> Result<String, TexError> {
    let s = field(f, i, line)?;
    if s.len() > MAX_SOURCE_BYTES {
        return Err(TexError::Listing(format!("truth line {line}: a field is longer than {MAX_SOURCE_BYTES} bytes")));
    }
    Ok(s.to_owned())
}

impl TruthSet {
    /// Reads a truth file. Every `G` and `R` row's owner must be `-`, `tag` or an `E`/`M` id of the
    /// same file, and every `E` must own something on its page.
    pub fn parse(tsv: &str) -> Result<TruthSet, TexError> {
        let mut set = TruthSet::default();
        let mut page_index: HashMap<(String, usize), usize> = HashMap::new();
        let mut owners: HashMap<String, (bool, usize)> = HashMap::new();
        for (n, line) in tsv.lines().enumerate() {
            let line_no = n.saturating_add(1);
            if line_no > MAX_ROWS {
                return Err(TexError::Listing(format!("truth file has more than {MAX_ROWS} rows")));
            }
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let f: Vec<&str> = line.split('\t').collect();
            match f.first().copied() {
                Some("E") => {
                    let env = field(&f, 7, line_no)?;
                    let e = EquationTruth {
                        id: text(&f, 1, line_no)?,
                        base: text(&f, 2, line_no)?,
                        doc: text(&f, 3, line_no)?,
                        page: number(&f, 4, line_no)?,
                        category: text(&f, 5, line_no)?,
                        tier: number(&f, 6, line_no)?,
                        env: Env::parse(env).ok_or_else(|| TexError::Listing(format!("truth line {line_no}: unknown env `{env}`")))?,
                        context: text(&f, 8, line_no)?,
                        bbox: [number(&f, 9, line_no)?, number(&f, 10, line_no)?, number(&f, 11, line_no)?, number(&f, 12, line_no)?],
                        source: text(&f, 13, line_no)?,
                        items: Vec::new(),
                        reference: Vec::new(),
                    };
                    if owners.insert(e.id.clone(), (true, set.equations.len())).is_some() {
                        return Err(TexError::Listing(format!("truth line {line_no}: `{}` is listed twice", e.id)));
                    }
                    set.equations.push(e);
                }
                Some("M") => {
                    let m = MutantTruth {
                        id: text(&f, 1, line_no)?,
                        base: text(&f, 2, line_no)?,
                        mutation: text(&f, 3, line_no)?,
                        source: text(&f, 4, line_no)?,
                        items: Vec::new(),
                    };
                    if owners.insert(m.id.clone(), (false, set.mutants.len())).is_some() {
                        return Err(TexError::Listing(format!("truth line {line_no}: `{}` is listed twice", m.id)));
                    }
                    set.mutants.push(m);
                }
                Some(kind @ ("G" | "R")) => {
                    let doc = text(&f, 1, line_no)?;
                    let page: usize = number(&f, 2, line_no)?;
                    let owner = text(&f, 3, line_no)?;
                    let item = if kind == "G" {
                        let mark = Mark::Glyph { font: text(&f, 4, line_no)?, size: number(&f, 5, line_no)?, code: number(&f, 6, line_no)? };
                        Item { mark, h: number(&f, 7, line_no)?, v: number(&f, 8, line_no)? }
                    } else {
                        let mark = Mark::Rule { width: number(&f, 6, line_no)?, height: number(&f, 7, line_no)? };
                        Item { mark, h: number(&f, 4, line_no)?, v: number(&f, 5, line_no)? }
                    };
                    match owner.as_str() {
                        "-" | "tag" => {}
                        id => match owners.get(id) {
                            Some(&(true, i)) => match set.equations.get_mut(i) {
                                Some(e) if e.doc == doc && e.page == page => e.items.push(item.clone()),
                                _ => return Err(TexError::Listing(format!("truth line {line_no}: `{id}` is not on {doc} page {page}"))),
                            },
                            Some(&(false, i)) => {
                                if let Some(m) = set.mutants.get_mut(i) {
                                    m.items.push(item.clone());
                                }
                            }
                            None => return Err(TexError::Listing(format!("truth line {line_no}: unknown owner `{id}`"))),
                        },
                    }
                    let key = (doc.clone(), page);
                    let index = match page_index.get(&key) {
                        Some(&i) => i,
                        None => {
                            page_index.insert(key, set.pages.len());
                            set.pages.push(PageTruth { doc, page, items: Vec::new() });
                            set.pages.len().saturating_sub(1)
                        }
                    };
                    if let Some(p) = set.pages.get_mut(index) {
                        p.items.push((item, owner));
                    }
                }
                _ => return Err(TexError::Listing(format!("truth line {line_no}: unknown row"))),
            }
        }
        if let Some(e) = set.equations.iter().find(|e| e.items.is_empty()) {
            return Err(TexError::Listing(format!("truth: equation `{}` has no glyphs or rules", e.id)));
        }
        let alone: HashMap<String, Vec<Item>> =
            set.equations.iter().filter(|e| e.context == ALONE && e.id == e.base).map(|e| (e.id.clone(), e.items.clone())).collect();
        for e in &mut set.equations {
            e.reference = alone.get(&e.base).cloned().ok_or_else(|| TexError::Listing(format!("truth: `{}` has no `alone` occurrence", e.base)))?;
        }
        Ok(set)
    }

    /// The file [`TruthSet::parse`] reads, headed by `header` (each line becomes a comment).
    /// Equations and mutants are written in order, then the pages in order.
    pub fn to_tsv(&self, header: &str) -> String {
        let mut s = String::new();
        for line in header.lines() {
            let _ = writeln!(s, "# {line}");
        }
        for e in &self.equations {
            let [x0, y0, x1, y1] = e.bbox;
            let _ = writeln!(
                s,
                "E\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{x0:.3}\t{y0:.3}\t{x1:.3}\t{y1:.3}\t{}",
                e.id,
                e.base,
                e.doc,
                e.page,
                e.category,
                e.tier,
                e.env.as_str(),
                e.context,
                e.source
            );
        }
        for m in &self.mutants {
            let _ = writeln!(s, "M\t{}\t{}\t{}\t{}", m.id, m.base, m.mutation, m.source);
        }
        for p in &self.pages {
            for (item, owner) in &p.items {
                match &item.mark {
                    Mark::Glyph { font, size, code } => {
                        let _ = writeln!(s, "G\t{}\t{}\t{owner}\t{font}\t{size}\t{code}\t{}\t{}", p.doc, p.page, item.h, item.v);
                    }
                    Mark::Rule { width, height } => {
                        let _ = writeln!(s, "R\t{}\t{}\t{owner}\t{}\t{}\t{width}\t{height}", p.doc, p.page, item.h, item.v);
                    }
                }
            }
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> TruthSet {
        let glyph = Item { mark: Mark::Glyph { font: "cmmi10".into(), size: 655360, code: 120 }, h: 10, v: 20 };
        let rule = Item { mark: Mark::Rule { width: 300, height: 26 }, h: 10, v: 40 };
        let tag = Item { mark: Mark::Glyph { font: "cmr10".into(), size: 655360, code: 49 }, h: 900, v: 20 };
        TruthSet {
            equations: vec![EquationTruth {
                id: "frac".into(),
                base: "frac".into(),
                doc: "equations".into(),
                page: 3,
                category: "fractions".into(),
                tier: 1,
                env: Env::Equation,
                context: "alone".into(),
                bbox: [72.5, 80.25, 100.0, 90.125],
                source: r"\frac{x}{y}".into(),
                items: vec![glyph.clone(), rule.clone()],
                reference: vec![glyph.clone(), rule.clone()],
            }],
            mutants: vec![MutantTruth {
                id: "frac/swap-fraction".into(),
                base: "frac".into(),
                mutation: "swap-fraction".into(),
                source: r"\frac{y}{x}".into(),
                items: vec![rule.clone()],
            }],
            pages: vec![
                PageTruth {
                    doc: "equations".into(),
                    page: 3,
                    items: vec![(glyph, "frac".into()), (rule.clone(), "frac".into()), (tag, "tag".into())],
                },
                PageTruth { doc: "mutants".into(), page: 0, items: vec![(rule, "frac/swap-fraction".into())] },
            ],
        }
    }

    #[test]
    fn writing_and_reading_round_trips() {
        let set = sample();
        let tsv = set.to_tsv("made by a test\nsecond line");
        assert!(tsv.starts_with("# made by a test\n# second line\n"));
        assert_eq!(TruthSet::parse(&tsv).unwrap(), set);
    }

    #[test]
    fn inconsistent_files_are_errors() {
        let tsv = sample().to_tsv("");
        for (from, to, why) in [
            ("\t3\tfrac\tcmmi10", "\t3\tnobody\tcmmi10", "unknown owner"),
            ("G\tequations\t3\tfrac", "G\tequations\t4\tfrac", "not on"),
            ("E\tfrac", "X\tfrac", "unknown row"),
            ("\tequation\t", "\tequations\t", "unknown env"),
            ("\t655360\t120", "\tsix\t120", "not a number"),
        ] {
            let broken = tsv.replacen(from, to, 1);
            assert_ne!(broken, tsv, "{from}");
            let err = TruthSet::parse(&broken).unwrap_err().to_string();
            assert!(err.contains(why), "{from}: {err}");
        }
        let lonely = "E\tx\tx\td\t0\tc\t1\tinline\talone\t0\t0\t1\t1\tx\n";
        assert!(TruthSet::parse(lonely).unwrap_err().to_string().contains("no glyphs"));
        let orphan = tsv.replacen("\talone\t", "\t1col\t", 1);
        assert!(TruthSet::parse(&orphan).unwrap_err().to_string().contains("no `alone` occurrence"));
        assert!(TruthSet::parse("E\tx\n").unwrap_err().to_string().contains("missing"));
    }
}
