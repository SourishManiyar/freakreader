//! The entries of the list: where each starts, its label, its lines and glyphs.

use super::lines::BibLine;
use super::text::{composed, is_dash, word_flags};
use super::{Entry, Fields, LabelStyle, MAX_ENTRIES, MAX_ENTRY_LINES, MAX_ENTRY_TEXT_BYTES, MAX_LABEL_BYTES};
use crate::text::page_layout::Diagnostic;
use crate::text::page_layout::refs::{GLYPH_MATH, GLYPH_MONO, Piece, Word};

/// A label at the start of a line: its style, its text, and how many words it takes.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct LineLabel {
    pub style: LabelStyle,
    pub text: String,
    pub words: usize,
}

/// The label a line (its words, `words`) starts with, if any: `[12]`, `[BKM20]` (up to three words
/// to the `]`), `12.`, `(12)`. Not one set in a mathematics font.
pub(super) fn line_label<'a>(words: impl IntoIterator<Item = &'a Word>) -> Option<LineLabel> {
    let words: Vec<&Word> = words.into_iter().take(3).collect();
    let first = *words.first()?;
    if word_flags(first) & GLYPH_MATH != 0 {
        return None;
    }
    let t = first.text.as_str();
    if t.starts_with('[') {
        let mut text = String::new();
        for (k, w) in words.iter().enumerate() {
            if k > 0 {
                text.push(' ');
            }
            text.push_str(&composed(w));
            if let Some(close) = text.find(']') {
                // The label is the bracket alone: what follows `]` in its word is the entry's.
                let inner = text.get(1..close).unwrap_or("").trim().to_owned();
                let whole = text.get(close + 1..).is_some_and(str::is_empty);
                if inner.is_empty() || inner.len() > MAX_LABEL_BYTES || !whole {
                    return None;
                }
                return Some(LineLabel { style: LabelStyle::Bracket, text: inner, words: k + 1 });
            }
            if text.len() > MAX_LABEL_BYTES + 2 {
                return None;
            }
        }
        return None;
    }
    let digits = |s: &str| !s.is_empty() && s.len() <= 6 && s.bytes().all(|b| b.is_ascii_digit());
    if let Some(n) = t.strip_suffix('.').filter(|n| digits(n)) {
        return Some(LineLabel { style: LabelStyle::Dot, text: n.to_owned(), words: 1 });
    }
    if let Some(n) = t.strip_prefix('(').and_then(|r| r.strip_suffix(')')).filter(|n| digits(n)) {
        return Some(LineLabel { style: LabelStyle::Paren, text: n.to_owned(), words: 1 });
    }
    None
}

/// A line of the list as segmentation sees it.
struct Row<'a, 'b> {
    /// Its index in the list's lines.
    index: usize,
    line: &'b BibLine<'a>,
    label: Option<LineLabel>,
    /// Its left edge from its column's.
    x: f32,
}

/// The x most of `xs` sit at (to half a point), the smallest of equals.
fn mode(xs: &[f32]) -> Option<f32> {
    let mut v: Vec<i64> = xs.iter().filter(|x| x.is_finite()).map(|x| (x * 2.0).round() as i64).collect();
    v.sort_unstable();
    let mut best: Option<(usize, i64)> = None;
    let mut i = 0;
    while let Some(&a) = v.get(i) {
        let j = v.iter().skip(i).take_while(|&&b| b == a).count();
        if best.is_none_or(|(n, _)| j > n) {
            best = Some((j, a));
        }
        i += j.max(1);
    }
    best.map(|(_, a)| a as f32 / 2.0)
}

/// Whether line `l` looks like it ends an entry: its last word ends with a period (or is a URL or
/// DOI), and the line stops short of its column's right edge.
fn ends_entry(l: &BibLine<'_>) -> bool {
    let Some(last) = l.word_list().last() else { return false };
    let short = l.rect[2] < l.column[1] - 2.0 * l.size;
    (last.text.ends_with('.') || word_flags(last) & GLYPH_MONO != 0) && short
}

/// Whether line `l` starts like an author-year entry: a capitalised name and a comma (`Okafor,`,
/// `van der Berg,`), within its first three words.
fn starts_with_name(l: &BibLine<'_>) -> bool {
    let first_lower = l.word_list().next().is_some_and(|f| f.text.starts_with(char::is_lowercase));
    for w in l.word_list().take(3) {
        let t = composed(w);
        let name = t.trim_end_matches(',');
        if !name.chars().next().is_some_and(char::is_alphabetic) {
            return false;
        }
        if t.ends_with(',') {
            return name.chars().next().is_some_and(char::is_uppercase) || first_lower;
        }
    }
    false
}

/// The entries of the list whose lines are `lines`, in order, and for each the lines it has.
pub(super) fn split(lines: &[BibLine<'_>], diagnostics: &mut Vec<Diagnostic>) -> (Vec<Entry>, Vec<Vec<usize>>) {
    let rows: Vec<Row<'_, '_>> = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| !l.words.is_empty())
        .map(|(index, line)| Row { index, line, label: line_label(line.word_list()), x: line.rect[0] - line.column[0] })
        .collect();
    // The list's label style: the first line's, when at least two lines carry it; when the first
    // line has none it can read (a label too long), the one at least a third of the lines start with.
    let count = |s: LabelStyle| rows.iter().filter(|r| r.label.as_ref().is_some_and(|l| l.style == s)).count();
    let style = match rows.first().map(|r| r.label.as_ref().map(|l| l.style)) {
        Some(Some(s)) => Some(s).filter(|s| count(*s) >= 2),
        Some(None) => [LabelStyle::Bracket, LabelStyle::Dot, LabelStyle::Paren]
            .into_iter()
            .map(|s| (count(s), s))
            .max_by_key(|(n, _)| *n)
            .filter(|(n, _)| *n >= 2 && *n * 3 >= rows.len())
            .map(|(_, s)| s),
        None => None,
    };
    let starts = match style {
        Some(s) => labelled_starts(&rows, s),
        None => unlabelled_starts(&rows),
    };
    let mut out: Vec<Entry> = Vec::new();
    let mut members: Vec<Vec<usize>> = Vec::new();
    let mut cut = 0usize;
    let mut capped = false;
    for (k, row) in rows.iter().enumerate() {
        let start = starts.get(k).copied().unwrap_or(false) || out.is_empty();
        let too_long = members.last().is_some_and(|m| m.len() >= MAX_ENTRY_LINES);
        if too_long && !start {
            cut += 1;
        }
        if start || too_long {
            if out.len() >= MAX_ENTRIES {
                capped = true;
                break;
            }
            let label = row.label.clone().filter(|l| Some(l.style) == style && start);
            out.push(Entry {
                id: out.len(),
                label: label.as_ref().map(|l| l.text.clone()),
                label_style: style.unwrap_or(LabelStyle::None),
                lines: Vec::new(),
                pieces: Vec::new(),
                text: String::new(),
                fields: Fields::default(),
                parsed: false,
                confidence: if style.is_some() { 0.9 } else { 0.75 },
            });
            members.push(Vec::new());
        }
        let (Some(e), Some(m)) = (out.last_mut(), members.last_mut()) else { continue };
        let skip = if m.is_empty() { e.label.as_ref().and(row.label.as_ref()).map_or(0, |l| l.words) } else { 0 };
        m.push(row.index);
        add_line(e, row.line, skip);
    }
    if cut > 0 {
        diagnostics.push(Diagnostic {
            code: "entry_cut",
            message: format!("an entry ran on for more than {MAX_ENTRY_LINES} lines and was cut ({cut} lines start new entries)"),
            block: None,
        });
    }
    if capped {
        diagnostics.push(Diagnostic {
            code: "entries_capped",
            message: format!("more than {MAX_ENTRIES} entries: the rest are left out"),
            block: None,
        });
    }
    if style.is_some_and(|s| s != LabelStyle::None) {
        let numbers: Vec<u64> = out.iter().filter_map(|e| e.label.as_deref()?.parse().ok()).collect();
        if numbers.len() == out.len() && numbers.windows(2).any(|w| matches!(w, [a, b] if *b != a.saturating_add(1))) {
            diagnostics.push(Diagnostic {
                code: "labels_out_of_order",
                message: "the list's numbers do not run 1, 2, 3, … in order".into(),
                block: None,
            });
        }
    }
    (out, members)
}

/// In a labelled list, a line starts an entry when it starts with a label of the list's style, at
/// the labels' edge (left of where the list's other lines start), and, for numbers, with the next
/// number.
fn labelled_starts(rows: &[Row<'_, '_>], style: LabelStyle) -> Vec<bool> {
    let labelled: Vec<f32> = rows.iter().filter(|r| r.label.as_ref().is_some_and(|l| l.style == style)).map(|r| r.x).collect();
    let others: Vec<f32> = rows.iter().filter(|r| r.label.is_none()).map(|r| r.x).collect();
    let label_edge = labelled.iter().copied().fold(f32::INFINITY, f32::min);
    let hang = mode(&others);
    let mut last_number: Option<u64> = None;
    rows.iter()
        .map(|r| {
            let Some(l) = r.label.as_ref().filter(|l| l.style == style) else { return false };
            // A continuation line that happens to start with a bracket sits at the hanging indent.
            if let Some(h) = hang
                && h > label_edge + 1.0
                && r.x >= h - 0.5
            {
                return false;
            }
            let n: Option<u64> = l.text.parse().ok();
            match (style, n, last_number) {
                (LabelStyle::Dot | LabelStyle::Paren, Some(n), Some(prev)) if n != prev.saturating_add(1) => false,
                (_, n, _) => {
                    if n.is_some() {
                        last_number = n;
                    }
                    true
                }
            }
        })
        .collect()
}

/// In a list without labels, a line starts an entry at the left edge where the others hang
/// indented; with all lines flush, after a gap larger than the list's line spacing, or at a column
/// or page top when the line before ended an entry and this one starts with a name.
fn unlabelled_starts(rows: &[Row<'_, '_>]) -> Vec<bool> {
    let xs: Vec<f32> = rows.iter().map(|r| r.x).collect();
    let left = xs.iter().copied().fold(f32::INFINITY, f32::min);
    let size = rows.first().map_or(10.0, |r| r.line.size.max(1.0));
    let at_left = |r: &Row<'_, '_>| r.x <= left + 0.25 * size;
    let indented = rows.iter().filter(|r| r.x > left + 0.5 * size && r.x < left + 6.0 * size).count();
    let flush = rows.iter().filter(|r| at_left(r)).count();
    if indented >= 2 && flush >= 2 {
        return rows.iter().map(at_left).collect();
    }
    // Lines all flush: gaps.
    let mut gaps: Vec<f32> = rows
        .windows(2)
        .filter_map(|w| match w {
            [a, b] if a.line.page == b.line.page && (a.line.column[0] - b.line.column[0]).abs() < 1.0 => Some(b.line.baseline - a.line.baseline),
            _ => None,
        })
        .filter(|g| g.is_finite() && *g > 0.0)
        .collect();
    gaps.sort_by(f32::total_cmp);
    let leading = gaps.first().copied().unwrap_or(size * 1.2);
    let mut out = Vec::with_capacity(rows.len());
    for (k, r) in rows.iter().enumerate() {
        let prev = k.checked_sub(1).and_then(|p| rows.get(p));
        let start = match prev {
            None => true,
            Some(p) if p.line.page == r.line.page && (p.line.column[0] - r.line.column[0]).abs() < 1.0 && r.line.baseline > p.line.baseline => {
                r.line.baseline - p.line.baseline > leading * 1.25
            }
            Some(p) => ends_entry(p.line) && starts_with_name(r.line),
        };
        out.push(start);
    }
    out
}

/// Adds `line` to entry `e`: its words to its glyphs, the words after the first `skip` (the label)
/// to its text, and the page lines they come from to its lines.
fn add_line(e: &mut Entry, line: &BibLine<'_>, skip: usize) {
    for (r, _, _) in &line.words {
        if !e.lines.contains(r) {
            e.lines.push(*r);
        }
    }
    let glyphs = line.word_list().flat_map(|w| w.glyphs.iter().copied());
    // One piece per page and column: a line of the same column overlaps the piece across.
    match e.pieces.last_mut().filter(|p| p.page == line.page && line.rect[0] < p.rect[2] && p.rect[0] < line.rect[2]) {
        Some(p) => {
            p.glyphs.extend(glyphs);
            p.rect = [p.rect[0].min(line.rect[0]), p.rect[1].min(line.rect[1]), p.rect[2].max(line.rect[2]), p.rect[3].max(line.rect[3])];
        }
        None => e.pieces.push(Piece { page: line.page, glyphs: glyphs.collect(), rules: Vec::new(), rect: line.rect }),
    }
    for (k, w) in line.word_list().enumerate().skip(skip) {
        let t = composed(w);
        if e.text.len() + t.len() > MAX_ENTRY_TEXT_BYTES {
            break;
        }
        // A word broken at the end of the last line goes on without a space: a URL or DOI, a
        // name with a hyphen; a lower-case word broken by TeX loses its hyphen.
        let joined = k == skip && !e.text.is_empty() && joins(&e.text, &t, w.flags.iter().any(|f| f & GLYPH_MONO != 0));
        if joined {
            let ends_hyphen = e.text.ends_with(is_dash);
            let lower_break = ends_hyphen
                && e.text.chars().rev().nth(1).is_some_and(char::is_lowercase)
                && t.chars().next().is_some_and(char::is_lowercase)
                && !last_word(&e.text).contains(['/', ':', '.']);
            if lower_break {
                e.text.pop();
            }
        } else if !e.text.is_empty() {
            e.text.push(' ');
        }
        e.text.push_str(&t);
    }
}

fn last_word(text: &str) -> &str {
    text.rsplit(' ').next().unwrap_or("")
}

/// Whether the next line's first word `next` goes on the text so far without a space: a URL or a
/// DOI broken over lines (the last word one, the next one going on without a capital), or a word
/// broken with a hyphen.
fn joins(text: &str, next: &str, next_mono: bool) -> bool {
    let last = last_word(text);
    let url = last.contains("://") || last.starts_with("www.");
    let doi = last.starts_with("10.") && last.contains('/') || last.to_lowercase().starts_with("doi:10.");
    let open = last.ends_with(['/', '.', '-', '_', '=', '?', '&', '#']) || !last.ends_with([',', ';', ')']);
    let goes_on = next.chars().next().is_some_and(|c| c.is_ascii_digit() || c.is_lowercase() || c == '/') && !next.contains(char::is_whitespace);
    if (url || doi) && open && (goes_on || next_mono) {
        return true;
    }
    last.ends_with(is_dash) && last.chars().count() > 2 && next.chars().next().is_some_and(char::is_alphabetic)
}
