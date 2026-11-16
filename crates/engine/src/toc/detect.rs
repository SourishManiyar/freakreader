//! Finding a document's table of contents among its pages and reading its entries: which pages
//! are contents pages (a "Contents" heading, or many lines ending in page numbers after dot
//! leaders), the entries on them in reading order (titles that wrap onto a second row joined),
//! and how deep each entry is nested (from its numbering, else its indent, else bold for the
//! upper level).

use super::layout::Row;
use super::parse::{self, NumberKind, PageRef, ParsedLine};

/// Most pages one table of contents runs over.
pub(crate) const MAX_TOC_PAGES: usize = 40;
/// Most entries read from a table of contents.
pub const MAX_ENTRIES: usize = 5000;
/// Most rows one wrapped entry joins.
const MAX_WRAP_ROWS: usize = 4;
/// Deepest level given to an entry (0 is the top).
pub(crate) const MAX_LEVEL: usize = 7;
/// Longest entry title kept, in characters.
pub(crate) const MAX_TITLE_CHARS: usize = 300;

/// Headings that start a table of contents, lower case, in several languages.
const CONTENTS: [&str; 24] = [
    "contents",
    "table of contents",
    "brief contents",
    "contents in brief",
    "contents at a glance",
    "summary of contents",
    "detailed contents",
    "inhalt",
    "inhaltsverzeichnis",
    "table des matières",
    "table des matieres",
    "sommaire",
    "índice",
    "indice",
    "índice general",
    "sommario",
    "spis treści",
    "inhoud",
    "innehåll",
    "目次",
    "目录",
    "目錄",
    "содержание",
    "оглавление",
];

/// Headings of pages that list page numbers but are not a table of contents.
const NOT_CONTENTS: [&str; 16] = [
    "index",
    "subject index",
    "author index",
    "name index",
    "general index",
    "register",
    "stichwortverzeichnis",
    "list of figures",
    "list of tables",
    "list of illustrations",
    "list of abbreviations",
    "list of symbols",
    "list of plates",
    "figures",
    "tables",
    "illustrations",
];

/// `text` as a heading is matched: lower case, without numbering, punctuation or "(continued)".
fn heading_key(text: &str) -> String {
    let lower = parse::clean(text).to_lowercase();
    let lower = lower.replace("(continued)", "").replace("(cont.)", "").replace("(cont'd)", "");
    let trimmed = lower.trim_matches(|c: char| !c.is_alphanumeric());
    let mut words: Vec<&str> = trimmed.split_whitespace().collect();
    // Letter-spaced headings ("C O N T E N T S") read as one word.
    if words.len() > 3 && words.iter().all(|w| w.chars().count() == 1) {
        return words.concat();
    }
    // A running head with the page's number before or after it ("Table of Contents XI").
    let folio = |w: &&str| parse::page_token(w, true).is_some_and(|p| !matches!(p, PageRef::Label(_)));
    if words.len() > 1 && words.last().is_some_and(folio) {
        words.pop();
    }
    if words.len() > 1 && words.first().is_some_and(folio) {
        words.remove(0);
    }
    words.join(" ")
}

fn is_contents_heading(text: &str) -> bool {
    let key = heading_key(text);
    CONTENTS.contains(&key.as_str())
}

fn is_other_list_heading(text: &str) -> bool {
    let key = heading_key(text);
    NOT_CONTENTS.contains(&key.as_str())
}

/// A row with what it says as a contents line.
#[derive(Clone, Debug)]
pub(crate) struct Line {
    pub(crate) row: Row,
    pub(crate) parsed: Option<ParsedLine>,
}

/// What one page looks like as a contents page.
#[derive(Clone, Debug, Default)]
pub(crate) struct PageScore {
    /// Rows that are entries (a readable title and a page number).
    pub(crate) entries: usize,
    /// Of those, the ones whose page number is set apart by a leader or a gap.
    pub(crate) strong: usize,
    pub(crate) rows: usize,
    /// A "Contents" heading near the top.
    pub(crate) heading: bool,
    /// A heading of another kind of list (index, list of figures) near the top.
    pub(crate) other_heading: bool,
    /// Rows ending in several comma-separated page numbers, as an index's do.
    pub(crate) index_rows: usize,
    /// Arabic page numbers in reading order that don't go down from the one before.
    pub(crate) rising: usize,
    pub(crate) arabic: usize,
    /// The first and last arabic page number on the page.
    pub(crate) first_number: Option<u32>,
    pub(crate) last_number: Option<u32>,
}

impl PageScore {
    /// A page that starts a table of contents on its own.
    pub(crate) fn starts(&self) -> bool {
        if self.other_heading || self.index_rows.saturating_mul(3) > self.rows.max(1) {
            return false;
        }
        if self.heading {
            return self.entries >= 2;
        }
        let ordered = self.arabic < 3 || self.rising.saturating_mul(10) >= self.arabic.saturating_mul(8);
        self.entries >= 8
            && self.strong.saturating_mul(10) >= self.entries.saturating_mul(6)
            && self.entries.saturating_mul(2) >= self.rows
            && ordered
    }

    /// A page that carries on a table of contents begun before it.
    pub(crate) fn continues(&self, previous_last: Option<u32>) -> bool {
        if self.other_heading || self.index_rows.saturating_mul(3) > self.rows.max(1) {
            return false;
        }
        let in_order = match (previous_last, self.first_number) {
            (Some(prev), Some(first)) => first.saturating_add(2) >= prev,
            _ => true,
        };
        self.entries >= 2 && self.entries.saturating_mul(10) >= self.rows.saturating_mul(3) && in_order
    }
}

/// The page's rows read as contents lines, and how much it looks like a contents page.
pub(crate) fn score(rows: &[Row]) -> (Vec<Line>, PageScore) {
    let mut s = PageScore { rows: rows.len(), ..Default::default() };
    let mut lines = Vec::with_capacity(rows.len());
    let mut last_arabic: Option<u32> = None;
    // The heading is among the rows highest on the page, whichever column they are in.
    let mut by_height: Vec<&Row> = rows.iter().collect();
    by_height.sort_by(|a, b| b.top.total_cmp(&a.top));
    for row in by_height.into_iter().take(4) {
        s.heading |= is_contents_heading(&row.text);
        s.other_heading |= is_other_list_heading(&row.text);
    }
    s.other_heading &= !s.heading;
    for row in rows {
        if parse::index_numbers(&row.text) >= 2 {
            s.index_rows += 1;
        }
        let parsed = parse::parse_line(&row.text);
        if let Some(p) = parsed.as_ref()
            && let Some(page) = &p.page
            && !is_contents_heading(&p.title)
        {
            s.entries += 1;
            if p.strong {
                s.strong += 1;
            }
            if let PageRef::Arabic(n) = page {
                s.arabic += 1;
                if last_arabic.is_none_or(|l| *n >= l) {
                    s.rising += 1;
                }
                last_arabic = Some(*n);
                s.first_number.get_or_insert(*n);
                s.last_number = Some(*n);
            }
        }
        lines.push(Line { row: row.clone(), parsed });
    }
    (lines, s)
}

/// The pages of the best table of contents among `scored` (page index, score), in order: runs of
/// a starting page and the pages that carry it on, the run with the most entries winning.
pub(crate) fn best_run(scored: &[(usize, PageScore)]) -> Vec<usize> {
    let mut runs: Vec<(Vec<usize>, usize)> = Vec::new();
    let mut i = 0;
    while let Some((page, s)) = scored.get(i) {
        if !s.starts() {
            i += 1;
            continue;
        }
        let mut run = vec![*page];
        let mut entries = s.entries;
        let mut last = s.last_number;
        let mut j = i + 1;
        while let Some((next_page, next)) = scored.get(j) {
            // A run is pages next to each other that carry it on. A "Contents" heading on the next
            // page is a running head when its numbers carry on from this page's, and a second
            // table (detailed contents after brief contents) when they start again.
            let carries_on = next.first_number.zip(last).is_some_and(|(first, prev)| first.saturating_add(2) >= prev);
            if *next_page != run.last().map_or(usize::MAX, |p| p.saturating_add(1))
                || run.len() >= MAX_TOC_PAGES
                || (next.heading && !carries_on)
                || !next.continues(last)
            {
                break;
            }
            run.push(*next_page);
            entries = entries.saturating_add(next.entries);
            last = next.last_number.or(last);
            j += 1;
        }
        runs.push((run, entries));
        i = j;
    }
    // The most entries; the first of equals (a brief table of contents comes before a detailed
    // one, which has more).
    let mut best: Option<(Vec<usize>, usize)> = None;
    for (run, entries) in runs {
        if best.as_ref().is_none_or(|(_, b)| entries > *b) {
            best = Some((run, entries));
        }
    }
    best.map(|(run, _)| run).unwrap_or_default()
}

/// One entry read from a table of contents, before its page is found.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RawEntry {
    pub(crate) title: String,
    pub(crate) page: Option<PageRef>,
    pub(crate) level: usize,
    /// The contents page it was read on.
    pub(crate) toc_page: usize,
}

/// A row of the table of contents on its way to an entry.
struct Pending {
    title: String,
    page: Option<PageRef>,
    numbering: Option<parse::Numbering>,
    /// Indent from the left of its column on its page.
    indent: f64,
    bold: bool,
    size: f64,
    toc_page: usize,
}

/// The entries of the table of contents on `pages` (each page's index and its lines), in order,
/// nested by level.
pub(crate) fn entries(pages: &[(usize, Vec<Line>)]) -> Vec<RawEntry> {
    let mut pending: Vec<Pending> = Vec::new();
    for (toc_page, lines) in pages {
        // The left edge of each column on this page, from its entries.
        let mut column_left: Vec<(usize, f64)> = Vec::new();
        for l in lines.iter().filter(|l| l.parsed.as_ref().is_some_and(|p| p.page.is_some())) {
            match column_left.iter_mut().find(|(c, _)| *c == l.row.column) {
                Some((_, x)) => *x = x.min(l.row.x0),
                None => column_left.push((l.row.column, l.row.x0)),
            }
        }
        let left_of = |column: usize, x0: f64| column_left.iter().find(|(c, _)| *c == column).map_or(x0, |(_, x)| *x);
        // On the page with the "Contents" heading, the table starts under it: a title page or an
        // abstract above it is not part of it.
        let heading_top = lines.iter().find(|l| is_contents_heading(&l.row.text)).map(|l| l.row.top);
        let mut i = 0;
        while let Some(line) = lines.get(i) {
            i += 1;
            if heading_top.is_some_and(|top| line.row.bottom > top) {
                continue;
            }
            let Some(parsed) = line.parsed.as_ref() else { continue };
            if is_contents_heading(&parsed.title) || is_contents_heading(&line.row.text) {
                continue;
            }
            // A running head on a contents page ("viii   CONTENTS") names the page, not an entry.
            if heading_key(&line.row.text).split_whitespace().any(|w| w == "contents") && parsed.page.is_none() {
                continue;
            }
            let mut title = parsed.title.clone();
            let mut page = parsed.page.clone();
            let mut row = line.row.clone();
            // A title without a page that wraps: the rows after it, indented under it or carrying
            // on a sentence, until one ends in the page.
            let mut joined = 0;
            while page.is_none() && joined < MAX_WRAP_ROWS {
                let Some(next) = lines.get(i) else { break };
                let Some(next_parsed) = next.parsed.as_ref() else { break };
                let same_column = next.row.column == row.column;
                let close = row.bottom - next.row.top < row.size.max(next.row.size) * 1.2;
                let indented = next.row.x0 > row.x0 + row.size * 0.4;
                let flush = (next.row.x0 - row.x0).abs() <= row.size * 0.4;
                let carries_on = title.ends_with(['-', ',', ':', ';'])
                    || title.split_whitespace().last().is_some_and(|w| {
                        ["and", "of", "the", "for", "in", "to", "a", "an", "on", "with", "und", "et", "de"].contains(&w.to_lowercase().as_str())
                    });
                let wraps = same_column && close && next_parsed.numbering.is_none() && (indented || (flush && carries_on));
                if !wraps {
                    break;
                }
                if title.ends_with('-') && next_parsed.title.chars().next().is_some_and(char::is_lowercase) {
                    title.pop();
                } else {
                    title.push(' ');
                }
                title.push_str(&next_parsed.title);
                page = next_parsed.page.clone();
                row.bottom = next.row.bottom;
                i += 1;
                joined += 1;
            }
            let title: String = title.chars().take(MAX_TITLE_CHARS).collect();
            if !parse::readable(&title) {
                continue;
            }
            pending.push(Pending {
                numbering: parse::numbering(&title),
                title,
                page,
                indent: (line.row.x0 - left_of(line.row.column, line.row.x0)).max(0.0),
                bold: line.row.bold,
                size: line.row.size,
                toc_page: *toc_page,
            });
            if pending.len() >= MAX_ENTRIES {
                break;
            }
        }
        if pending.len() >= MAX_ENTRIES {
            break;
        }
    }
    drop_prose(&mut pending);
    let levels = levels(&pending);
    pending.into_iter().zip(levels).map(|(p, level)| RawEntry { title: p.title, page: p.page, level, toc_page: p.toc_page }).collect()
}

/// Rows without a page number that are not entries: the text after the table on its last page
/// (the trailing rows without one), and runs of three or more such rows (a paragraph, not a part
/// heading over its chapters).
fn drop_prose(pending: &mut Vec<Pending>) {
    while pending.last().is_some_and(|p| p.page.is_none()) {
        pending.pop();
    }
    let mut keep = vec![true; pending.len()];
    let mut i = 0;
    while i < pending.len() {
        if pending.get(i).is_some_and(|p| p.page.is_none()) {
            let start = i;
            while pending.get(i).is_some_and(|p| p.page.is_none()) {
                i += 1;
            }
            if i - start >= 3 {
                for k in keep.iter_mut().take(i).skip(start) {
                    *k = false;
                }
            }
        } else {
            i += 1;
        }
    }
    let mut it = keep.into_iter();
    pending.retain(|_| it.next().unwrap_or(true));
}

/// Front and back matter, which a table of contents lists at its top level.
fn is_matter(title: &str) -> bool {
    const MATTER: [&str; 24] = [
        "preface",
        "foreword",
        "introduction",
        "acknowledgements",
        "acknowledgments",
        "contents",
        "notation",
        "list of figures",
        "list of tables",
        "abstract",
        "summary",
        "bibliography",
        "references",
        "index",
        "index of terms",
        "index of notation",
        "subject index",
        "author index",
        "appendix",
        "appendices",
        "glossary",
        "epilogue",
        "afterword",
        "catalogue",
    ];
    let key = heading_key(title);
    MATTER.contains(&key.as_str()) || key.starts_with("appendix ") || key.starts_with("index ")
}

/// The rank numbering gives: parts above chapters above sections, deeper numbers deeper.
fn rank(n: &parse::Numbering, roman_sections: bool) -> usize {
    match n.kind {
        NumberKind::Part => 0,
        NumberKind::Chapter => 1,
        NumberKind::Roman => 1,
        NumberKind::Section => n.depth.saturating_add(1),
        // Under roman sections ("I. INTRODUCTION"), lettered ones are their subsections.
        NumberKind::Letter if roman_sections => n.depth.saturating_add(1),
        NumberKind::Arabic | NumberKind::Letter => n.depth,
    }
}

/// Each entry's level: from numbering when most entries have it (unnumbered ones take the level
/// of the numbered entries nearest their indent), else from indents, else from bold.
fn levels(entries: &[Pending]) -> Vec<usize> {
    let numbered = entries.iter().filter(|e| e.numbering.is_some()).count();
    let roman_sections = entries.iter().any(|e| e.numbering.is_some_and(|n| n.kind == NumberKind::Roman))
        && !entries.iter().any(|e| e.numbering.is_some_and(|n| n.kind == NumberKind::Arabic));
    let tolerance = |e: &Pending| (e.size * 0.6).max(3.0);
    let mut out = vec![0usize; entries.len()];
    if numbered >= 2 && numbered.saturating_mul(2) >= entries.len() {
        // Ranks in use, compressed to 0, 1, 2…
        let mut ranks: Vec<usize> = entries.iter().filter_map(|e| e.numbering.as_ref().map(|n| rank(n, roman_sections))).collect();
        ranks.sort_unstable();
        ranks.dedup();
        let level_of_rank = |r: usize| ranks.iter().position(|x| *x == r).unwrap_or(0);
        // The mean indent of each level, for the entries without numbering.
        let mut indents: Vec<(usize, f64, usize)> = Vec::new();
        for (i, e) in entries.iter().enumerate() {
            if let Some(n) = &e.numbering {
                let level = level_of_rank(rank(n, roman_sections));
                if let Some(o) = out.get_mut(i) {
                    *o = level;
                }
                match indents.iter_mut().find(|(l, _, _)| *l == level) {
                    Some((_, sum, count)) => {
                        *sum += e.indent;
                        *count += 1;
                    }
                    None => indents.push((level, e.indent, 1)),
                }
            }
        }
        for i in 0..entries.len() {
            let Some(e) = entries.get(i).filter(|e| e.numbering.is_none()) else { continue };
            // The level whose entries sit nearest. When several sit as near (every level flush
            // left), front and back matter ("Preface", "Bibliography", "Index") is at the top
            // and anything else ("Notes and Comments") stays at the level of the entry before.
            let distances: Vec<(usize, f64)> = indents.iter().map(|(l, sum, count)| (*l, (sum / *count as f64 - e.indent).abs())).collect();
            let nearest = distances.iter().map(|d| d.1).fold(f64::INFINITY, f64::min);
            let mut tied: Vec<usize> = distances.iter().filter(|d| d.1 <= nearest + tolerance(e)).map(|d| d.0).collect();
            tied.sort_unstable();
            let previous = i.checked_sub(1).and_then(|k| out.get(k)).copied();
            let level = match (tied.as_slice(), previous) {
                ([], _) => 0,
                ([only], _) => *only,
                (many, Some(p)) if !is_matter(&e.title) => many.iter().copied().filter(|l| *l <= p).max().unwrap_or(0),
                (many, _) => many.first().copied().unwrap_or(0),
            };
            if let Some(o) = out.get_mut(i) {
                *o = level;
            }
        }
    } else {
        // Distinct indents, smallest first, each a level.
        let mut clusters: Vec<f64> = Vec::new();
        for e in entries {
            if !clusters.iter().any(|c| (c - e.indent).abs() <= tolerance(e)) {
                clusters.push(e.indent);
            }
        }
        clusters.sort_by(f64::total_cmp);
        let bold = entries.iter().filter(|e| e.bold).count();
        let by_bold = clusters.len() == 1 && bold > 0 && bold < entries.len() && bold.saturating_mul(2) <= entries.len();
        for (i, e) in entries.iter().enumerate() {
            let level = if by_bold { usize::from(!e.bold) } else { clusters.iter().position(|c| (c - e.indent).abs() <= tolerance(e)).unwrap_or(0) };
            if let Some(o) = out.get_mut(i) {
                *o = level;
            }
        }
    }
    // A list nests at most one level below the entry before it, starting at the top.
    let mut previous: Option<usize> = None;
    for l in &mut out {
        let deepest = previous.map_or(0, |p| p.saturating_add(1));
        *l = (*l).min(deepest).min(MAX_LEVEL);
        previous = Some(*l);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::super::layout::{rows, seg};
    use super::*;

    fn page(lines: &[(&str, f64)]) -> Vec<Row> {
        let segs: Vec<_> = lines.iter().enumerate().map(|(i, (t, x))| seg(t, *x, 700.0 - i as f64 * 14.0, 10.0)).collect();
        rows(&segs)
    }

    fn read(pages: &[Vec<Row>]) -> Vec<(usize, String, Option<PageRef>)> {
        let scored: Vec<(usize, Vec<Line>)> = pages.iter().enumerate().map(|(i, r)| (i, score(r).0)).collect();
        entries(&scored).into_iter().map(|e| (e.level, e.title, e.page)).collect()
    }

    #[test]
    fn a_contents_page_is_found_by_its_heading_or_its_leaders() {
        let headed = page(&[("Contents", 72.0), ("1 Introduction ..... 1", 72.0), ("2 Methods ..... 5", 72.0)]);
        assert!(score(&headed).1.starts());
        let spaced = page(&[("C O N T E N T S", 72.0), ("Intro ..... 1", 72.0), ("End ..... 5", 72.0)]);
        assert!(is_contents_heading("Table of Contents XI") && is_contents_heading("viii CONTENTS") && is_contents_heading("Contents (continued)"));
        assert!(!is_contents_heading("Contents of the box"));
        assert!(score(&spaced).1.starts(), "a letter-spaced heading");
        let lines: Vec<String> = (1..=10).map(|i| format!("Section {i} ......... {}", i * 3)).collect();
        let bare = page(&lines.iter().map(|l| (l.as_str(), 72.0)).collect::<Vec<_>>());
        assert!(score(&bare).1.starts(), "ten leadered entries in order");
        let prose =
            page(&[("This is a paragraph of text that", 72.0), ("runs over several lines and has", 72.0), ("no page numbers in it at all 3", 72.0)]);
        assert!(!score(&prose).1.starts());
    }

    #[test]
    fn an_index_or_a_list_of_figures_is_not_a_table_of_contents() {
        let index = page(&[("Index", 72.0), ("graphs, 12, 45, 103", 72.0), ("trees, 3, 9", 72.0), ("vertices ..... 7", 72.0)]);
        assert!(!score(&index).1.starts());
        let figures = page(&[("List of Figures", 72.0), ("1.1 A plot ..... 3", 72.0), ("1.2 Another ..... 9", 72.0)]);
        assert!(!score(&figures).1.starts());
    }

    #[test]
    fn a_table_runs_over_the_pages_that_carry_it_on() {
        let first = score(&page(&[("Contents", 72.0), ("1 One ..... 1", 72.0), ("2 Two ..... 9", 72.0)])).1;
        let second = score(&page(&[("3 Three ..... 20", 72.0), ("4 Four ..... 31", 72.0), ("v", 300.0)])).1;
        let unrelated = score(&page(&[("Some text", 72.0), ("More text", 72.0)])).1;
        assert_eq!(best_run(&[(4, first.clone()), (5, second.clone()), (6, unrelated.clone())]), [4, 5]);
        assert_eq!(best_run(&[(4, first.clone()), (6, second)]), [4], "not next to each other");
        let detailed =
            score(&page(&[("Contents", 72.0), ("1 One ..... 1", 72.0), ("1.1 A ..... 2", 72.0), ("1.2 B ..... 4", 72.0), ("2 Two ..... 9", 72.0)])).1;
        assert_eq!(best_run(&[(2, first.clone()), (3, detailed)]), [3], "detailed contents after brief contents");
        // A "Contents" running head on the next page, numbers carrying on: the same table.
        let headed = score(&page(&[("Contents", 72.0), ("3 Three ..... 20", 72.0), ("4 Four ..... 31", 72.0)])).1;
        assert_eq!(best_run(&[(4, first), (5, headed)]), [4, 5]);
        assert!(best_run(&[(0, unrelated)]).is_empty());
    }

    #[test]
    fn entries_nest_by_numbering_and_unnumbered_ones_by_indent() {
        let p = page(&[
            ("Contents", 72.0),
            ("Preface ..... vii", 72.0),
            ("1 Introduction ..... 1", 72.0),
            ("1.1 Scope ..... 2", 84.0),
            ("1.1.1 Terms ..... 3", 96.0),
            ("Notes ..... 4", 96.0),
            ("2 Methods ..... 5", 72.0),
            ("Bibliography ..... 99", 72.0),
        ]);
        let got = read(&[p]);
        let levels: Vec<(usize, &str)> = got.iter().map(|(l, t, _)| (*l, t.as_str())).collect();
        assert_eq!(
            levels,
            [(0, "Preface"), (0, "1 Introduction"), (1, "1.1 Scope"), (2, "1.1.1 Terms"), (2, "Notes"), (0, "2 Methods"), (0, "Bibliography")]
        );
        assert_eq!(got[0].2, Some(PageRef::Roman(7)));
    }

    #[test]
    fn parts_and_chapters_nest_and_a_part_without_a_page_is_kept() {
        let p = page(&[
            ("Contents", 72.0),
            ("Part I Foundations", 72.0),
            ("Chapter 1 Sets ..... 3", 72.0),
            ("1.1 Unions ..... 4", 84.0),
            ("Part II Uses", 72.0),
            ("Chapter 2 Maps ..... 40", 72.0),
        ]);
        let got = read(&[p]);
        assert_eq!(got.iter().map(|(l, _, _)| *l).collect::<Vec<_>>(), [0, 1, 2, 0, 1]);
        assert_eq!(got[0], (0, "Part I Foundations".into(), None));
    }

    #[test]
    fn section_signs_nest_under_chapters_and_flush_unnumbered_entries_follow_the_one_before() {
        let p = page(&[
            ("Contents", 72.0),
            ("Chapter I. Introduction ..... 1", 72.0),
            ("§ 1. Examples ..... 1", 72.0),
            ("§2. Local Properties ..... 12", 72.0),
            ("Notes and Comments ..... 30", 72.0),
            ("Chapter II. Martingales ..... 33", 72.0),
            ("§ 1. Definitions ..... 33", 72.0),
            ("Bibliography ..... 90", 72.0),
        ]);
        let got = read(&[p]);
        assert_eq!(got.iter().map(|(l, _, _)| *l).collect::<Vec<_>>(), [0, 1, 1, 1, 0, 1, 0]);
    }

    #[test]
    fn without_numbering_indents_give_the_levels_or_bold_does() {
        let p = page(&[("Contents", 72.0), ("Getting started ..... 1", 72.0), ("Installing ..... 2", 90.0), ("Using it ..... 5", 72.0)]);
        assert_eq!(read(&[p]).iter().map(|(l, _, _)| *l).collect::<Vec<_>>(), [0, 1, 0]);
        let mut segs = vec![seg("Contents", 72.0, 720.0, 10.0)];
        for (i, (t, bold)) in [("Getting started ..... 1", true), ("Installing ..... 2", false), ("Running ..... 3", false)].iter().enumerate() {
            segs.push(super::super::layout::Seg { bold: *bold, ..seg(t, 72.0, 700.0 - i as f64 * 14.0, 10.0) });
        }
        assert_eq!(read(&[rows(&segs)]).iter().map(|(l, _, _)| *l).collect::<Vec<_>>(), [0, 1, 1]);
    }

    #[test]
    fn wrapped_titles_join_their_next_row() {
        let p = page(&[
            ("Contents", 72.0),
            ("1 A very long chapter title that runs onto", 72.0),
            ("the next line ..... 3", 84.0),
            ("2 A title broken by hyphen- ", 72.0),
            ("ation ..... 9", 84.0),
            ("3 Short ..... 12", 72.0),
        ]);
        let got = read(&[p]);
        let titles: Vec<&str> = got.iter().map(|(_, t, _)| t.as_str()).collect();
        assert_eq!(titles, ["1 A very long chapter title that runs onto the next line", "2 A title broken by hyphenation", "3 Short"]);
        assert_eq!(got[0].2, Some(PageRef::Arabic(3)));
    }

    #[test]
    fn running_heads_and_page_numbers_on_contents_pages_are_not_entries() {
        let p = page(&[("viii CONTENTS", 72.0), ("4 Four ..... 31", 72.0), ("5 Five ..... 40", 72.0), ("ix", 300.0)]);
        let got = read(&[p]);
        assert_eq!(got.iter().map(|(_, t, _)| t.as_str()).collect::<Vec<_>>(), ["4 Four", "5 Five"]);
    }

    #[test]
    fn a_two_column_table_is_read_column_by_column() {
        let mut segs = vec![seg("Contents", 250.0, 740.0, 12.0)];
        for i in 0..6 {
            let top = 700.0 - f64::from(i) * 14.0;
            segs.push(seg(&format!("{} Left ..... {}", i + 1, i * 2 + 1), 50.0, top, 10.0));
            segs.push(seg(&format!("{} Right ..... {}", i + 7, i * 2 + 50), 330.0, top, 10.0));
        }
        let got = read(&[rows(&segs)]);
        let titles: Vec<&str> = got.iter().map(|(_, t, _)| t.as_str()).collect();
        assert_eq!(titles.first(), Some(&"1 Left"));
        assert_eq!(titles.get(6), Some(&"7 Right"), "{titles:?}");
        assert_eq!(titles.len(), 12);
    }

    #[test]
    fn a_title_page_above_the_heading_and_text_after_the_table_are_not_entries() {
        let p = page(&[
            ("Fundamentals of a method", 72.0),
            ("A. Author", 72.0),
            ("Contents", 72.0),
            ("1 Introduction ..... 3", 72.0),
            ("2 Background ..... 7", 72.0),
            ("1 Introduction", 72.0),
            ("This survey covers the method", 72.0),
            ("and its uses in probability", 72.0),
        ]);
        let got = read(&[p]);
        assert_eq!(got.iter().map(|(_, t, _)| t.as_str()).collect::<Vec<_>>(), ["1 Introduction", "2 Background"]);
        let q = page(&[
            ("Contents", 72.0),
            ("Part I", 72.0),
            ("1 One ..... 3", 72.0),
            ("Some prose here", 72.0),
            ("that runs over", 72.0),
            ("three rows", 72.0),
            ("2 Two ..... 9", 72.0),
        ]);
        assert_eq!(read(&[q]).iter().map(|(_, t, _)| t.as_str()).collect::<Vec<_>>(), ["Part I", "1 One", "2 Two"]);
    }

    #[test]
    fn many_entries_are_capped() {
        let lines: Vec<String> = (0..(MAX_ENTRIES + 200)).map(|i| format!("Entry {i} ..... {}", i + 1)).collect();
        let segs: Vec<_> = lines.iter().enumerate().map(|(i, t)| seg(t, 72.0, 1e6 - i as f64 * 14.0, 10.0)).collect();
        let r = rows(&segs);
        let (lines, _) = score(&r);
        assert_eq!(entries(&[(0, lines)]).len(), MAX_ENTRIES);
    }
}
