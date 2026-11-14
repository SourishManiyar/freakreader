//! Headings found by how they look, for documents with neither a table of contents nor tagged
//! headings (most research papers): rows set larger or bolder than the body text, on a line of
//! their own, that read as headings. When several are numbered (`1 Introduction`, `2.1 Data`,
//! `IV. RESULTS`), numbering decides the levels and only numbered rows and the usual unnumbered
//! section names (Abstract, References, Appendix…) are kept; otherwise each heading style is a
//! level, the largest first.

use super::layout::Row;
use super::parse::{self, NumberKind};

/// Most headings kept.
pub(crate) const MAX_HEADINGS: usize = 2000;
/// Longest row that can be a heading, in characters and words.
const MAX_HEADING_CHARS: usize = 150;
const MAX_HEADING_WORDS: usize = 18;
/// Most heading styles turned into levels when nothing is numbered.
const MAX_STYLE_LEVELS: usize = 3;

/// Section names that head a part of a paper without a number.
const SECTION_NAMES: [&str; 30] = [
    "abstract",
    "introduction",
    "background",
    "related work",
    "related works",
    "preliminaries",
    "method",
    "methods",
    "methodology",
    "materials and methods",
    "experiments",
    "experimental results",
    "results",
    "discussion",
    "conclusion",
    "conclusions",
    "future work",
    "limitations",
    "acknowledgments",
    "acknowledgements",
    "acknowledgment",
    "acknowledgement",
    "references",
    "bibliography",
    "appendix",
    "appendices",
    "supplementary material",
    "summary",
    "notation",
    "contents",
];

/// Captions and other numbered labels that are set in bold but are not headings.
const NOT_HEADINGS: [&str; 12] =
    ["figure", "fig", "table", "algorithm", "listing", "equation", "theorem", "lemma", "proof", "definition", "proposition", "corollary"];

/// A heading found on a page.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Heading {
    pub(crate) level: usize,
    pub(crate) title: String,
    pub(crate) page: usize,
    pub(crate) top: f64,
}

fn key(text: &str) -> String {
    let t = parse::without_numbering(text);
    t.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase()
}

fn is_section_name(text: &str) -> bool {
    let k = key(text);
    SECTION_NAMES.contains(&k.as_str()) || k.starts_with("appendix ")
}

fn is_caption(text: &str) -> bool {
    let first = text.split_whitespace().next().unwrap_or("").trim_end_matches(['.', ':']).to_lowercase();
    NOT_HEADINGS.contains(&first.as_str())
}

/// The size of a style, rounded to half a point.
fn size_key(size: f64) -> i64 {
    (size * 2.0).round() as i64
}

/// Running heads and feet: the first and last row of a page whose text (numbers aside) repeats on
/// at least three pages.
fn running(pages: &[(usize, Vec<Row>)]) -> std::collections::HashSet<String> {
    let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for (_, rows) in pages {
        for row in [rows.first(), rows.last()].into_iter().flatten() {
            let k: String = row.text.chars().filter(|c| c.is_alphabetic()).flat_map(char::to_lowercase).collect();
            if !k.is_empty() {
                *counts.entry(k).or_default() += 1;
            }
        }
    }
    counts.into_iter().filter(|(_, n)| *n >= 3).map(|(k, _)| k).collect()
}

/// The headings of `pages` (each page's index and rows in reading order), in order.
pub(crate) fn headings(pages: &[(usize, Vec<Row>)]) -> Vec<Heading> {
    // The body size: the size most characters of regular (not bold) text are set in.
    let mut by_size: std::collections::HashMap<i64, usize> = std::collections::HashMap::new();
    let mut total_rows = 0usize;
    let any_regular = pages.iter().any(|(_, rows)| rows.iter().any(|r| !r.bold));
    for (_, rows) in pages {
        for r in rows {
            if !r.bold || !any_regular {
                *by_size.entry(size_key(r.size)).or_default() += r.text.chars().count();
            }
            total_rows += 1;
        }
    }
    let Some(body) = by_size.iter().max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0))).map(|(s, _)| *s as f64 / 2.0) else {
        return Vec::new();
    };
    let running = running(pages);
    // Candidates: (page, row, numbering).
    // Candidates: (page, row, numbering, set apart by position rather than by style).
    let mut found: Vec<(usize, &Row, Option<parse::Numbering>, bool)> = Vec::new();
    for (page, rows) in pages {
        let apart = set_apart(rows);
        for (k, r) in rows.iter().enumerate() {
            let text = r.text.trim();
            let words = text.split_whitespace().count();
            let readable = words <= MAX_HEADING_WORDS
                && wordy(text)
                && text.chars().count() <= MAX_HEADING_CHARS
                && parse::without_numbering(text).chars().filter(|c| c.is_alphabetic()).count() >= 2
                && parse::readable(text)
                && !is_caption(text)
                && !text.ends_with([':', ',', ';'])
                && !folio_head(text)
                && !running.contains(&text.chars().filter(|c| c.is_alphabetic()).flat_map(char::to_lowercase).collect::<String>());
            if !readable {
                continue;
            }
            let numbering = parse::numbering(text);
            let by_style = r.size >= body + 0.8 || (r.bold && r.size >= body - 0.5);
            // A numbered heading in the body's own style, as article classes set them ("1. Setup"
            // centred): on its own short line, centred or with space above it, not a sentence.
            let by_place = numbering.is_some_and(|n| n.kind != NumberKind::Part)
                && words <= 12
                && !text.ends_with('.')
                && apart.get(k).copied().unwrap_or(false);
            if by_style || by_place {
                found.push((*page, r, numbering, by_place && !by_style));
            }
        }
    }
    // A style that much of the text is set in is the body, however it looks (in a document long
    // enough to tell).
    let style = |r: &Row| (size_key(r.size), r.bold);
    let mut style_rows: std::collections::HashMap<(i64, bool), usize> = std::collections::HashMap::new();
    for (_, rows) in pages {
        for r in rows {
            *style_rows.entry(style(r)).or_default() += 1;
        }
    }
    if total_rows >= 20 {
        found.retain(|(_, r, _, by_place)| *by_place || style_rows.get(&style(r)).is_some_and(|n| n.saturating_mul(4) <= total_rows));
    }
    // On the first page, what comes before the first heading is the title block (title, authors,
    // affiliations), not headings.
    if let Some(first_page) = pages.first().map(|(p, _)| *p) {
        let first = found.iter().position(|(p, r, n, _)| *p == first_page && (n.is_some() || is_section_name(&r.text)));
        if let Some(first) = first {
            let mut i = 0;
            found.retain(|(p, _, _, _)| {
                let keep = *p != first_page || i >= first;
                i += 1;
                keep
            });
        }
    }
    let numbered = found
        .iter()
        .filter(|(_, _, n, _)| n.is_some_and(|n| matches!(n.kind, NumberKind::Arabic | NumberKind::Roman | NumberKind::Letter | NumberKind::Chapter)))
        .count();
    let mut out: Vec<Heading> = Vec::new();
    if numbered >= 3 {
        let roman = found.iter().any(|(_, _, n, _)| n.is_some_and(|n| n.kind == NumberKind::Roman));
        let arabic = found.iter().any(|(_, _, n, _)| n.is_some_and(|n| n.kind == NumberKind::Arabic));
        for (page, r, n, _) in &found {
            let level = match n {
                Some(n) => match n.kind {
                    NumberKind::Part => 0,
                    NumberKind::Chapter | NumberKind::Roman => 0,
                    NumberKind::Letter if roman && !arabic => n.depth,
                    NumberKind::Arabic | NumberKind::Letter | NumberKind::Section => n.depth.saturating_sub(1),
                },
                None if is_section_name(&r.text) => 0,
                None => continue,
            };
            push(&mut out, level, *page, r);
        }
    } else {
        // Each style a level, the largest (then bold) first.
        let mut styles: Vec<(i64, bool)> = found.iter().filter(|f| !f.3).map(|(_, r, _, _)| style(r)).collect();
        styles.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
        styles.dedup();
        styles.truncate(MAX_STYLE_LEVELS);
        for (page, r, _, by_place) in &found {
            if *by_place {
                continue;
            }
            if let Some(level) = styles.iter().position(|s| *s == style(r)) {
                push(&mut out, level, *page, r);
            }
        }
    }
    // A list nests at most one level below the entry before it.
    let mut previous: Option<usize> = None;
    for h in &mut out {
        h.level = h.level.min(previous.map_or(0, |p| p.saturating_add(1)));
        previous = Some(h.level);
    }
    out
}

/// Whether `text` reads as words rather than a formula: after its numbering it starts with a
/// letter, most of it is letters, and it doesn't end in an equation number ("(7.29)").
fn wordy(text: &str) -> bool {
    let rest = parse::without_numbering(text).trim();
    let letters = rest.chars().filter(|c| c.is_alphabetic()).count();
    let marks = rest.chars().filter(|c| !c.is_whitespace()).count();
    let equation = rest
        .strip_suffix(')')
        .and_then(|r| r.rsplit_once('('))
        .is_some_and(|(_, n)| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit() || c == '.'));
    rest.chars().next().is_some_and(char::is_alphabetic) && letters.saturating_mul(10) >= marks.saturating_mul(6) && !equation
}

/// A numbered heading run into its paragraph ("3.1. Surplus. To obtain the…") cut after its
/// first sentence; any other title as it is.
fn run_in(text: &str) -> String {
    let text = parse::clean(text);
    if parse::numbering(&text).is_none() {
        return text;
    }
    let number_end = text.find(char::is_whitespace).unwrap_or(0);
    let Some(after) = text.get(number_end..) else { return text };
    match after.find(". ") {
        Some(cut) if after.get(cut + 2..).is_some_and(|rest| rest.split_whitespace().count() >= 2) => {
            let head = text.get(..number_end + cut).unwrap_or(&text);
            // The heading is words, not initials ("A. B. C.").
            let words = head.get(number_end..).is_some_and(|h| h.split_whitespace().any(|w| w.chars().filter(|c| c.is_alphabetic()).count() >= 3));
            if words { head.to_string() } else { text }
        }
        _ => text,
    }
}

/// Which rows of a page stand apart from the text around them: centred in the text column, or
/// with more space above than the page's lines usually have.
fn set_apart(rows: &[Row]) -> Vec<bool> {
    // The text column, from the page's long lines.
    let mut lefts: Vec<f64> = rows.iter().filter(|r| r.text.split_whitespace().count() >= 8).map(|r| r.x0).collect();
    let mut rights: Vec<f64> = rows.iter().filter(|r| r.text.split_whitespace().count() >= 8).map(|r| r.x1).collect();
    lefts.sort_by(f64::total_cmp);
    rights.sort_by(f64::total_cmp);
    let column = lefts.get(lefts.len() / 2).copied().zip(rights.get(rights.len() / 2).copied()).filter(|(l, r)| r > l);
    let mut gaps: Vec<f64> = rows.windows(2).filter_map(|w| Some(w.first()?.bottom - w.get(1)?.top)).filter(|g| g.is_finite() && *g >= 0.0).collect();
    gaps.sort_by(f64::total_cmp);
    let usual = gaps.get(gaps.len() / 2).copied().unwrap_or(0.0);
    rows.iter()
        .enumerate()
        .map(|(k, r)| {
            let centred = column.is_some_and(|(left, right)| {
                let width = right - left;
                r.x0 > left + width * 0.05 && ((r.x0 + r.x1) / 2.0 - (left + right) / 2.0).abs() < width * 0.08
            });
            let above = k.checked_sub(1).and_then(|j| rows.get(j)).map(|p| p.bottom - r.top);
            let spaced = above.is_some_and(|g| g > usual * 1.8 + r.size * 0.3) || k == 0;
            centred || (spaced && column.is_some_and(|(left, _)| (r.x0 - left).abs() < r.size))
        })
        .collect()
}

/// A running head with the page number set apart before or after it ("2   B. AUTHOR").
fn folio_head(text: &str) -> bool {
    let pieces: Vec<&str> = text.split(parse::GAP).map(str::trim).filter(|p| !p.is_empty()).collect();
    pieces.len() >= 2
        && [pieces.first(), pieces.last()].into_iter().flatten().any(|p| p.chars().all(|c| c.is_ascii_digit()) || parse::roman_value(p).is_some())
}

/// Add a heading, or join it to the one before when it carries on that heading on the next row.
fn push(out: &mut Vec<Heading>, level: usize, page: usize, r: &Row) {
    if let Some(last) = out.last_mut()
        && last.page == page
        && last.level == level
        && parse::numbering(&r.text).is_none()
        && last.top - r.top < r.size * 2.0
        && last.top > r.top
        && last.title.chars().count() < super::detect::MAX_TITLE_CHARS
    {
        last.title.push(' ');
        last.title.push_str(r.text.trim());
        return;
    }
    if out.len() < MAX_HEADINGS {
        out.push(Heading { level, title: run_in(&r.text).chars().take(super::detect::MAX_TITLE_CHARS).collect(), page, top: r.top });
    }
}

#[cfg(test)]
mod tests {
    use super::super::layout::{Seg, rows, seg};
    use super::*;

    /// A page of rows: (text, size, bold).
    fn page(lines: &[(&str, f64, bool)]) -> Vec<Row> {
        let segs: Vec<Seg> =
            lines.iter().enumerate().map(|(i, (t, size, bold))| Seg { bold: *bold, ..seg(t, 72.0, 750.0 - i as f64 * 16.0, *size) }).collect();
        rows(&segs)
    }

    fn body(n: usize) -> Vec<(&'static str, f64, bool)> {
        vec![("Body text that goes on and on about the matter at hand.", 10.0, false); n]
    }

    fn found(pages: Vec<Vec<Row>>) -> Vec<(usize, String, usize)> {
        let pages: Vec<(usize, Vec<Row>)> = pages.into_iter().enumerate().collect();
        headings(&pages).into_iter().map(|h| (h.level, h.title, h.page)).collect()
    }

    #[test]
    fn a_numbered_paper_nests_its_sections_and_skips_its_title_block() {
        let mut p1 = vec![("A Grand Title", 18.0, true), ("Jane Doe and John Roe", 12.0, false), ("Abstract", 12.0, true)];
        p1.extend(body(6));
        p1.push(("1 Introduction", 12.0, true));
        p1.extend(body(8));
        let mut p2 = vec![("2 Model", 12.0, true)];
        p2.extend(body(5));
        p2.push(("2.1 Assumptions", 11.0, true));
        p2.extend(body(5));
        p2.push(("Figure 1: A bold caption", 10.0, true));
        p2.push(("References", 12.0, true));
        p2.extend(body(4));
        let got = found(vec![page(&p1), page(&p2)]);
        assert_eq!(
            got,
            [
                (0, "Abstract".into(), 0),
                (0, "1 Introduction".into(), 0),
                (0, "2 Model".into(), 1),
                (1, "2.1 Assumptions".into(), 1),
                (0, "References".into(), 1)
            ]
        );
    }

    #[test]
    fn roman_sections_and_lettered_subsections_nest() {
        let mut p = vec![("I. INTRODUCTION", 10.0, true)];
        p.extend(body(10));
        p.push(("A. Background", 10.0, true));
        p.extend(body(10));
        p.push(("II. METHOD", 10.0, true));
        p.extend(body(10));
        let got = found(vec![page(&p)]);
        assert_eq!(
            got.iter().map(|(l, t, _)| (*l, t.as_str())).collect::<Vec<_>>(),
            [(0, "I. INTRODUCTION"), (1, "A. Background"), (0, "II. METHOD")]
        );
    }

    #[test]
    fn numbered_headings_in_the_body_style_are_found_by_their_place() {
        // An article class: "1. Setup" centred in the body's own size and weight.
        let long = "Body text that runs the whole width of the text column on the page.";
        let mut pages = Vec::new();
        for (k, h) in ["1. Setup", "2. To criticality via percolation", "3. The main estimate"].iter().enumerate() {
            // Centred over the text column (72 to 446).
            let x0 = 259.0 - h.chars().count() as f64 * 11.0 * 0.25;
            let mut segs = vec![seg(h, x0, 750.0, 11.0)];
            for i in 0..12 {
                segs.push(seg(long, 72.0, 720.0 - f64::from(i) * 14.0, 11.0));
            }
            // A running head with the page number set apart, and a numbered list item in the text.
            segs.push(seg(&format!("{} AUTHOR NAME", k + 2), 72.0, 790.0, 8.0));
            segs.push(seg("1. We first show that the estimate holds in general.", 72.0, 500.0, 11.0));
            pages.push(rows(&segs));
        }
        let got = found(pages);
        assert_eq!(
            got.iter().map(|(l, t, p)| (*l, t.as_str(), *p)).collect::<Vec<_>>(),
            [(0, "1. Setup", 0), (0, "2. To criticality via percolation", 1), (0, "3. The main estimate", 2)]
        );
    }

    #[test]
    fn formulas_are_not_headings_and_run_in_headings_end_at_their_sentence() {
        assert!(!wordy("2 exp(−mx2/8). (7.29)"));
        assert!(!wordy("1 ∑ 1 R∑(n,i) 1 L∑(n,i)"));
        assert!(!wordy("1 − pq(f(du) + f(dv))"));
        assert!(wordy("2.1 Neighborhood sampling"));
        assert!(wordy("Appendix A. Proofs of Lemma 3"));
        assert_eq!(run_in("3.1. Obtain surplus convergence. To obtain convergence of the"), "3.1. Obtain surplus convergence");
        assert_eq!(run_in("1.1. The multiplicative coalescent. The multiplicative coale"), "1.1. The multiplicative coalescent");
        assert_eq!(run_in("2. Main results"), "2. Main results");
        assert_eq!(run_in("A. B. C. Details here"), "A. B. C. Details here");
        assert_eq!(run_in("Introduction. We study"), "Introduction. We study");
    }

    #[test]
    fn unnumbered_headings_take_a_level_per_style() {
        let mut p = vec![("Overview", 16.0, true)];
        p.extend(body(10));
        p.push(("Details", 12.0, true));
        p.extend(body(10));
        p.push(("A heading set on", 12.0, true));
        p.push(("two lines", 12.0, true));
        p.extend(body(10));
        let got = found(vec![page(&p)]);
        assert_eq!(
            got.iter().map(|(l, t, _)| (*l, t.as_str())).collect::<Vec<_>>(),
            [(0, "Overview"), (1, "Details"), (1, "A heading set on two lines")]
        );
    }

    #[test]
    fn running_heads_and_a_page_of_body_text_give_no_headings() {
        let pages: Vec<Vec<Row>> = (0..4)
            .map(|_| {
                let mut p = vec![("JOURNAL OF THINGS", 12.0, true)];
                p.extend(body(10));
                page(&p)
            })
            .collect();
        assert!(found(pages).is_empty());
        assert!(found(vec![]).is_empty());
    }
}
