//! Where the list is: its heading, and the lines from it to the next heading or the end.

use super::Region;
use super::text::{composed, key, word_flags};
use crate::text::page_layout::refs::{FlowLine, GLYPH_BOLD, LineRef, PageWords, Piece};
use crate::text::page_layout::{BlockKind, Diagnostic, DocumentStructure, PageLayout};
use std::collections::HashMap;

/// What a reference list's heading says, as [`key`] reads it.
const NAMES: [&str; 13] = [
    "references",
    "reference",
    "bibliography",
    "literature",
    "literaturecited",
    "workscited",
    "citedworks",
    "referencesandnotes",
    "selectedreferences",
    "bibliographyandreferences",
    "literatur",
    "literaturverzeichnis",
    "bibliographie",
];
/// Fewest entries a list found by its labels alone (no heading) must have.
const HEADLESS_LEAST: usize = 3;

/// The kinds of line a list's text can be in.
pub(super) fn list_kind(kind: BlockKind) -> bool {
    matches!(kind, BlockKind::Paragraph | BlockKind::ListItem | BlockKind::Other | BlockKind::Heading)
}

/// The text of a line, words joined by spaces, accents on their letters.
pub(super) fn line_text(line: &FlowLine) -> String {
    line.words.iter().map(composed).collect::<Vec<_>>().join(" ")
}

/// The heading's words without a number in front (`7`, `7.`, `VII.`, `A.`, `§ 7`) or a colon after.
fn heading_name(text: &str) -> String {
    let mut words: Vec<&str> = text.split_whitespace().collect();
    if let Some(first) = words.first().copied() {
        let bare = first.trim_end_matches(['.', ')', ':']).trim_start_matches('§');
        let numeral = !bare.is_empty()
            && (bare.chars().all(|c| c.is_ascii_digit() || c == '.')
                || (bare.len() <= 4 && bare.chars().all(|c| matches!(c, 'I' | 'V' | 'X' | 'L')))
                || (bare.len() == 1 && bare.chars().all(|c| c.is_ascii_uppercase()) && words.len() > 1));
        if numeral || first == "§" {
            words.remove(0);
        }
    }
    key(&words.join(" "))
}

/// Whether line `l` reads as a reference list's heading: a short block of its own whose words are
/// one of [`NAMES`], set as a heading (the layout says so, or it is bold, bigger than the text, or
/// in capitals).
fn is_list_heading(line: &FlowLine, lines_in_block: usize, body: f32) -> bool {
    if !line.first || lines_in_block > 2 || line.words.len() > 6 {
        return false;
    }
    let text = line_text(line);
    if !NAMES.contains(&heading_name(&text).as_str()) {
        return false;
    }
    let bold = line.words.iter().all(|w| word_flags(w) & GLYPH_BOLD != 0);
    let capitals = text.chars().filter(|c| c.is_alphabetic()).all(char::is_uppercase);
    line.kind == BlockKind::Heading || bold || capitals || line.size > body * 1.1
}

/// Whether line `l`, after the list began, starts what follows it: a heading, an appendix.
fn ends_list(line: &FlowLine, size: f32) -> bool {
    let text = line_text(line);
    let first = text.split_whitespace().next().unwrap_or("");
    let lower = first.to_lowercase();
    if line.first && (lower == "appendix" || lower == "appendices") && line.words.len() <= 12 {
        return true;
    }
    let label = first.starts_with('[') || first.trim_end_matches('.').chars().all(|c| c.is_ascii_digit()) && first.ends_with('.');
    // A heading the layout found, set bigger than the list or apart from it (not an entry the
    // layout took for one: it starts with no label and is no smaller than the list).
    // A page number the layout took for a heading has no word to it.
    let words = text.chars().filter(|c| c.is_alphabetic()).count() >= 2;
    line.kind == BlockKind::Heading && line.first && !label && words && line.size >= size * 0.95
}

/// The most common text size of `lines`, from the middle.
fn middle_size(sizes: &mut [f32]) -> f32 {
    sizes.sort_by(f32::total_cmp);
    sizes.get(sizes.len() / 2).copied().filter(|v| v.is_finite() && *v > 0.0).unwrap_or(10.0)
}

/// Every line in reading order, as `(page, line)`, of the kinds a list can be in.
fn all_lines(words: &[PageWords]) -> Vec<LineRef> {
    words.iter().enumerate().flat_map(|(p, w)| w.lines.iter().enumerate().filter(|(_, l)| list_kind(l.kind)).map(move |(i, _)| (p, i))).collect()
}

fn line(words: &[PageWords], at: LineRef) -> Option<&FlowLine> {
    words.get(at.0).and_then(|w| w.lines.get(at.1))
}

/// The lines of the list, from its first to its last, in reading order and of the kinds a list can
/// be in.
pub(super) fn region_lines(words: &[PageWords], region: &Region) -> Vec<LineRef> {
    let mut out = Vec::new();
    for (p, w) in words.iter().enumerate().skip(region.first.0) {
        if p > region.last.0 {
            break;
        }
        for (i, l) in w.lines.iter().enumerate() {
            if (p, i) < region.first || (p, i) > region.last || !list_kind(l.kind) {
                continue;
            }
            out.push((p, i));
        }
    }
    out
}

/// The label a line starts with when it is `[n]` (a number, for the list found without heading).
fn bracket_number(line: &FlowLine) -> Option<u64> {
    let first = line.words.first()?.text.as_str();
    let inner = first.strip_prefix('[')?.strip_suffix(']')?;
    if inner.is_empty() || inner.len() > 6 || !inner.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    inner.parse().ok()
}

/// The list: after its heading, to the next heading or the end; or, with no heading, a run of
/// lines at the end of the paper that start `[1]`, `[2]`, … in order. `None` (and a diagnostic)
/// when there is neither.
pub(super) fn find(_layouts: &[PageLayout], words: &[PageWords], _structure: &DocumentStructure, diagnostics: &mut Vec<Diagnostic>) -> Option<Region> {
    let order = all_lines(words);
    let mut block_lines: HashMap<(usize, usize), usize> = HashMap::new();
    for &(p, i) in &order {
        if let Some(l) = line(words, (p, i)) {
            *block_lines.entry((p, l.block)).or_default() += 1;
        }
    }
    let mut sizes: Vec<f32> = order.iter().filter_map(|&r| line(words, r)).filter(|l| l.kind == BlockKind::Paragraph).map(|l| l.size).collect();
    let body = middle_size(&mut sizes);
    // The last heading that names a list and has lines after it.
    let heading = order.iter().enumerate().rev().find(|(k, r)| {
        let Some(l) = line(words, **r) else { return false };
        is_list_heading(l, block_lines.get(&(r.0, l.block)).copied().unwrap_or(0), body) && order.get(k + 1).is_some()
    });
    if let Some((k, &at)) = heading {
        let h = line(words, at)?;
        let rest = order.get(k + 1..).unwrap_or_default();
        // A heading of two lines: the second is no entry.
        let skip = usize::from(rest.first().and_then(|r| line(words, *r)).is_some_and(|l| l.page == h.page && l.block == h.block));
        let rest = rest.get(skip..).unwrap_or_default();
        let first = *rest.first()?;
        let mut size_list: Vec<f32> = rest.iter().take(40).filter_map(|r| line(words, *r)).map(|l| l.size).collect();
        let size = middle_size(&mut size_list);
        let mut last = first;
        for &r in rest {
            let Some(l) = line(words, r) else { continue };
            if r != first && ends_list(l, size) {
                break;
            }
            last = r;
        }
        let glyphs: Vec<u32> = h.words.iter().flat_map(|w| w.glyphs.iter().copied()).collect();
        return Some(Region {
            heading: Some(Piece { page: h.page, glyphs, rules: Vec::new(), rect: h.rect }),
            title: Some(line_text(h)),
            first,
            last,
            confidence: if h.kind == BlockKind::Heading { 0.95 } else { 0.85 },
        });
    }
    // No heading: the last line that starts `[1]`, if `[2]`, `[3]`, … follow it in order to the end.
    let start = order.iter().rposition(|r| line(words, *r).and_then(bracket_number) == Some(1));
    if let Some(s) = start {
        let rest = order.get(s..).unwrap_or_default();
        let mut next = 1u64;
        let mut ok = true;
        for r in rest {
            let Some(l) = line(words, *r) else { continue };
            if l.kind == BlockKind::Heading && l.first {
                ok = false;
                break;
            }
            if let Some(n) = bracket_number(l) {
                if n != next {
                    ok = false;
                    break;
                }
                next += 1;
            }
        }
        let count = usize::try_from(next.saturating_sub(1)).unwrap_or(usize::MAX);
        if let (true, Some(&first), Some(&last)) = (ok && count >= HEADLESS_LEAST, rest.first(), rest.last()) {
            diagnostics.push(Diagnostic {
                code: "no_heading",
                message: format!("a list of {count} entries numbered [1], [2], … ends the document without a heading; taken as its bibliography"),
                block: None,
            });
            return Some(Region { heading: None, title: None, first, last, confidence: 0.6 });
        }
    }
    diagnostics.push(Diagnostic {
        code: "no_bibliography",
        message: "no reference list was found: no heading names one (References, Bibliography, Literature, Works cited), and the document does not end with a numbered list".into(),
        block: None,
    });
    None
}
