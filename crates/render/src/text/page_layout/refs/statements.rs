//! Statements and proofs: run-in heads, extents, end marks, and what each proof proves.

use super::mentions::{canonical_statement, number_core};
use super::{
    EMPTY, EndMark, FlowLine, HeadStyle, LineRef, MAX_LABEL_BYTES, MAX_NUMBER_BYTES, MAX_STATEMENT_LINES, MAX_STATEMENTS, PageWords, Piece, ProvesBy,
    References, Statement, Style, Word, union,
};
use crate::text::page_layout::{BlockKind, Diagnostic, cut};
use std::collections::HashSet;

/// Words a run-in head may not be: they open headings, captions and front matter.
const NOT_STATEMENTS: [&str; 14] = [
    "appendix",
    "section",
    "chapter",
    "part",
    "figure",
    "fig",
    "table",
    "abstract",
    "keywords",
    "acknowledgements",
    "acknowledgments",
    "references",
    "contents",
    "bibliography",
];
/// Most words a head runs on for before its period.
const MAX_HEAD_WORDS: usize = 14;
/// A share of letters at least this large makes text italic.
const ITALIC_SHARE: f32 = 0.6;

/// A run-in head found at the start of a line.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Head {
    pub(super) kind: String,
    pub(super) proof: bool,
    pub(super) number: Option<String>,
    pub(super) name: Option<String>,
    pub(super) style: HeadStyle,
    /// How many of the line's words the head takes, and how many its kind takes.
    pub(super) words: usize,
    pub(super) kind_words: usize,
}

fn alphabetic(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_alphabetic() || c == '-' || c == '\'')
}

fn head_style(s: Style) -> Option<HeadStyle> {
    if s.math {
        None
    } else if s.bold {
        Some(HeadStyle::Bold)
    } else if s.small_caps {
        Some(HeadStyle::SmallCaps)
    } else if s.italic {
        Some(HeadStyle::Italic)
    } else {
        None
    }
}

/// Strips one terminator (`.` or `:`) from the end: the text and whether there was one.
fn terminated(s: &str) -> (&str, bool) {
    match s.strip_suffix('.').or_else(|| s.strip_suffix(':')) {
        Some(t) => (t, true),
        None => (s, false),
    }
}

/// The run-in head that `words` (a line's) start with, if any.
pub(super) fn parse_head(words: &[Word]) -> Option<Head> {
    let first = words.first()?;
    let style = head_style(first.style)?;
    let (w0, mut done) = terminated(first.text.as_str());
    if !alphabetic(w0) || w0.chars().count() < 3 || !w0.chars().next().is_some_and(char::is_uppercase) {
        return None;
    }
    let lower0 = w0.to_lowercase();
    if NOT_STATEMENTS.contains(&lower0.as_str()) {
        return None;
    }
    let proof = lower0 == "proof";
    let mut kind = w0.to_owned();
    let mut i = 1;
    // More words of the kind (`Main Theorem`), capitalised and in the head's style.
    while !done && !proof && i < 3 {
        let Some(w) = words.get(i) else { break };
        let (t, end) = terminated(w.text.as_str());
        if alphabetic(t) && t.chars().next().is_some_and(char::is_uppercase) && head_style(w.style) == Some(style) {
            kind.push(' ');
            kind.push_str(t);
            done = end;
            i += 1;
        } else {
            break;
        }
    }
    let kind_words = i;
    let mut number = None;
    if !done
        && !proof
        && let Some(w) = words.get(i)
    {
        let (t, end) = terminated(w.text.as_str());
        if let Some(n) = number_core(t).filter(|n| n.len() == t.len() && n.len() <= MAX_NUMBER_BYTES) {
            number = Some(n.to_owned());
            done = end;
            i += 1;
        }
    }
    // The name in parentheses, or for a proof the rest of its head.
    let mut name = None;
    if !done {
        let opens = words.get(i).is_some_and(|w| w.text.starts_with('('));
        if proof || opens {
            let mut parts: Vec<&str> = Vec::new();
            while let Some(w) = words.get(i) {
                if i >= MAX_HEAD_WORDS {
                    return None;
                }
                let t = w.text.as_str();
                i += 1;
                let (body, end) = terminated(t);
                parts.push(body);
                if end && (proof || body.ends_with(')')) {
                    done = true;
                    break;
                }
                if !proof && body.ends_with(')') {
                    // The period may stand apart: `(Key bound) .`
                    if words.get(i).is_some_and(|w| matches!(w.text.as_str(), "." | ":")) {
                        i += 1;
                        done = true;
                    }
                    break;
                }
            }
            let mut joined = parts.join(" ");
            if !proof {
                joined = joined.trim_start_matches('(').trim_end_matches(')').trim().to_owned();
            }
            cut(&mut joined, MAX_LABEL_BYTES);
            name = (!joined.is_empty()).then_some(joined);
        } else if words.get(i).is_some_and(|w| matches!(w.text.as_str(), "." | ":")) {
            i += 1;
            done = true;
        }
    }
    if !done {
        return None;
    }
    // A kind no one knows is a head only with a number.
    if number.is_none() && !proof && canonical_statement(&kind).is_none() {
        return None;
    }
    // What follows the head must not look like it: an italic head runs into upright text, a
    // bold or small-caps one into anything but more of the same.
    if let Some(next) = words.get(i).filter(|w| !w.style.math) {
        let same = match style {
            HeadStyle::Italic => next.style.italic,
            HeadStyle::Bold => next.style.bold,
            HeadStyle::SmallCaps => next.style.small_caps,
        };
        if same {
            return None;
        }
    }
    cut(&mut kind, MAX_LABEL_BYTES);
    Some(Head { kind, proof, number, name, style, words: i, kind_words })
}

/// Letters of `words` set in italic, and all letters (mathematics left out).
fn italic_letters<'a>(words: impl Iterator<Item = &'a Word>) -> (usize, usize) {
    let (mut italic, mut all) = (0, 0);
    for w in words.filter(|w| !w.style.math) {
        let n = w.text.chars().filter(|c| c.is_alphabetic()).count();
        all += n;
        if w.style.italic {
            italic += n;
        }
    }
    (italic, all)
}

fn italic(words: &[Word]) -> Option<bool> {
    let (i, a) = italic_letters(words.iter());
    (a >= 3).then_some(i as f32 >= ITALIC_SHARE * a as f32)
}

/// Lines that hold body text a statement can run through; the rest (running heads, footnotes,
/// floats) are passed over.
fn text_kind(k: BlockKind) -> bool {
    matches!(k, BlockKind::Paragraph | BlockKind::ListItem | BlockKind::DisplayEquation | BlockKind::Other)
}

struct Flow<'a> {
    pages: &'a [PageWords],
    order: Vec<LineRef>,
}

impl<'a> Flow<'a> {
    fn line(&self, at: usize) -> Option<&'a FlowLine> {
        let &(p, l) = self.order.get(at)?;
        self.pages.get(p)?.lines.get(l)
    }

    /// Whether the line at `at` starts a paragraph: the first of its block, after a short line or a
    /// mark set apart, or after a gap.
    fn starts_paragraph(&self, at: usize) -> bool {
        let Some(l) = self.line(at) else { return false };
        if l.first {
            return true;
        }
        let Some(prev) = at.checked_sub(1).and_then(|p| self.line(p)) else { return true };
        if prev.page != l.page || prev.block != l.block {
            return true;
        }
        let short = prev.rect[2] < l.column[1] - 2.0 * l.size;
        short || prev.mark_only() || self.spaced(prev, l)
    }

    /// Whether more than a paragraph's space stands between `prev` and `l` (the space a list or a
    /// theorem-like environment leaves around itself).
    fn spaced(&self, prev: &FlowLine, l: &FlowLine) -> bool {
        if prev.page != l.page || prev.column != l.column {
            return false;
        }
        let lead = self.pages.get(l.page).and_then(|p| p.leading).filter(|v| v.is_finite() && *v > 0.0);
        let gap = l.baseline - prev.baseline;
        match lead {
            Some(lead) => gap > 1.15 * lead,
            None => gap > 1.6 * l.size.max(prev.size),
        }
    }
}

/// A mark set apart from the line at `at` (on its baseline, to its right, a few lines away in
/// reading order), and its place.
fn apart_mark(flow: &Flow<'_>, at: usize, used: &HashSet<usize>) -> Option<usize> {
    let l = flow.line(at)?;
    let lo = at.saturating_sub(3);
    (lo..at.saturating_add(4)).filter(|&k| k != at && !used.contains(&k)).find(|&k| {
        flow.line(k).is_some_and(|m| {
            m.page == l.page
                && m.mark_only()
                && (m.baseline - l.baseline).abs() < 0.5 * l.size.max(1.0)
                && m.rect[0] >= l.rect[0]
                && m.rect[0] <= l.column[1] + l.size
                && m.rect[2] >= l.column[0]
        })
    })
}

#[derive(Default)]
struct Extent {
    pieces: Vec<Piece>,
    lines: usize,
}

impl Extent {
    fn add(&mut self, l: &FlowLine, only: Option<&EndMark>) {
        if self.pieces.last().is_none_or(|p| p.page != l.page) {
            self.pieces.push(Piece { page: l.page, rect: EMPTY, ..Default::default() });
        }
        let Some(p) = self.pieces.last_mut() else { return };
        match only {
            Some(m) => {
                p.glyphs.extend(&m.glyphs);
                p.rules.extend(&m.rules);
                union(&mut p.rect, &m.rect);
            }
            None => {
                for w in &l.words {
                    p.glyphs.extend(&w.glyphs);
                }
                if let Some(m) = &l.mark {
                    p.rules.extend(&m.rules);
                    union(&mut p.rect, &m.rect);
                }
                union(&mut p.rect, &l.rect);
            }
        }
        self.lines += 1;
    }
}

/// Finds the statements and proofs of `pages` (see the module notes).
pub(super) fn find(pages: &[PageWords], out: &mut References) {
    let order: Vec<LineRef> = pages
        .iter()
        .enumerate()
        .flat_map(|(p, pw)| (0..pw.lines.len()).map(move |l| (p, l)))
        .filter(|&(p, l)| pages.get(p).and_then(|pw| pw.lines.get(l)).is_some_and(|l| text_kind(l.kind) || l.kind == BlockKind::Heading))
        .collect();
    let flow = Flow { pages, order };
    let n = flow.order.len();
    // Lines a statement took whole, and marks set apart that a proof took.
    let mut used: HashSet<usize> = HashSet::new();
    let head_at = |at: usize| -> Option<Head> {
        let l = flow.line(at)?;
        if !text_kind(l.kind) || l.kind == BlockKind::DisplayEquation || !flow.starts_paragraph(at) {
            return None;
        }
        parse_head(&l.words)
    };
    let mut at = 0;
    while at < n {
        let Some(head) = head_at(at) else {
            at += 1;
            continue;
        };
        if out.statements.len() >= MAX_STATEMENTS {
            out.diagnostics.push(Diagnostic {
                code: "statements_capped",
                message: format!("more than {MAX_STATEMENTS} statements and proofs: the rest are left out"),
                block: None,
            });
            break;
        }
        let Some(first) = flow.line(at) else { break };
        let mut extent = Extent::default();
        extent.add(first, None);
        let mut head_piece = Piece { page: first.page, rect: EMPTY, ..Default::default() };
        for w in first.words.iter().take(head.words) {
            head_piece.glyphs.extend(&w.glyphs);
            union(&mut head_piece.rect, &w.rect);
        }
        // The body's look: the head's paragraph after the head.
        let mut body: Vec<&Word> = first.words.iter().skip(head.words).collect();
        let mut k = at + 1;
        while let Some(l) = flow.line(k).filter(|l| l.page == first.page && l.block == first.block && !flow.starts_paragraph(k)) {
            body.extend(l.words.iter());
            k += 1;
        }
        let (it, all) = italic_letters(body.into_iter());
        let italic_body = all >= 3 && it as f32 >= ITALIC_SHARE * all as f32;
        let mut last = at;
        let mut mark: Option<(usize, EndMark)> = None;
        let mut cut_short = false;
        // Lines taken after the head (flow index, and the mark alone when only that is taken),
        // and for a proof where the first spaced paragraph begins: it ends there if it has no mark.
        let mut taken: Vec<(usize, Option<EndMark>)> = Vec::new();
        let mut spaced_at: Option<(usize, usize)> = None;
        let mut k = at;
        loop {
            // The current last line's end mark ends a proof.
            if head.proof {
                if let Some(m) = flow.line(k).and_then(|l| l.mark.clone()) {
                    mark = Some((flow.line(k).map_or(0, |l| l.page), m));
                    break;
                }
                if let Some(a) = apart_mark(&flow, k, &used)
                    && let Some(ml) = flow.line(a)
                    && let Some(m) = ml.mark.clone()
                {
                    taken.push((a, Some(m.clone())));
                    used.insert(a);
                    mark = Some((ml.page, m));
                    break;
                }
            }
            k += 1;
            let Some(l) = flow.line(k) else { break };
            if used.contains(&k) {
                continue;
            }
            if l.kind == BlockKind::Heading || head_at(k).is_some() {
                break;
            }
            if taken.len() >= MAX_STATEMENT_LINES {
                cut_short = true;
                break;
            }
            let Some(prev) = flow.line(last) else { break };
            let same_block = l.page == prev.page && l.block == prev.block;
            let starts = flow.starts_paragraph(k);
            let take = if (same_block && !starts) || l.kind == BlockKind::DisplayEquation || (prev.kind == BlockKind::DisplayEquation && same_block) {
                true
            } else if l.kind == BlockKind::ListItem {
                !italic_body || italic(&l.words).unwrap_or(true)
            } else if head.proof {
                // A proof runs on to its end mark (or the next head).
                if spaced_at.is_none() && starts && prev.kind != BlockKind::DisplayEquation && flow.spaced(prev, l) {
                    spaced_at = Some((taken.len(), last));
                }
                true
            } else {
                let indented = l.rect[0] - l.column[0] > 0.5 * l.size;
                // A paragraph that goes on after a display, or into the next column or page.
                let goes_on = !indented
                    && (prev.kind == BlockKind::DisplayEquation
                        || (l.first && (l.page != prev.page || l.column != prev.column) && prev.rect[2] >= prev.column[1] - 2.0 * prev.size));
                goes_on || (italic_body && italic(&l.words) == Some(true) && (prev.kind == BlockKind::DisplayEquation || !flow.spaced(prev, l)))
            };
            if !take {
                break;
            }
            taken.push((k, None));
            last = k;
        }
        // A proof with no mark ends where a paragraph after it is set apart by more space.
        if head.proof
            && mark.is_none()
            && let Some((n, before)) = spaced_at
        {
            taken.truncate(n);
            last = before;
        }
        for (idx, only) in &taken {
            if let Some(l) = flow.line(*idx) {
                extent.add(l, only.as_ref());
            }
        }
        // A statement does not end on a display that only follows it: a display is its own when
        // text of the statement comes after it or it is the statement's last thing before a
        // paragraph of other text (it then belongs, as LaTeX sets displays inside environments).
        let proof_unended = head.proof && mark.is_none();
        let number = head.number.clone();
        let confidence = match (head.proof, proof_unended, number.is_some()) {
            (true, true, _) => 0.55,
            (true, false, _) => 0.9,
            (false, _, true) => 0.9,
            (false, _, false) => 0.8,
        };
        if cut_short {
            out.diagnostics.push(Diagnostic {
                code: "statement_cut",
                message: format!("{} runs on for more than {MAX_STATEMENT_LINES} lines: cut there", head.kind),
                block: Some(first.block),
            });
        }
        out.statements.push(Statement {
            kind: head.kind.clone(),
            proof: head.proof,
            number,
            name: head.name.clone(),
            head_style: head.style,
            italic_body,
            head: Piece { rect: super::finite_or_zero(head_piece.rect), ..head_piece },
            extent: extent.pieces.into_iter().map(|p| Piece { rect: super::finite_or_zero(p.rect), ..p }).collect(),
            first_line: flow.order.get(at).copied().unwrap_or((0, 0)),
            last_line: flow.order.get(last).copied().unwrap_or((0, 0)),
            proves: None,
            end_mark: mark,
            confidence,
        });
        if proof_unended {
            out.diagnostics.push(Diagnostic {
                code: "proof_unended",
                message: format!("page {}: a proof with no end mark: it is taken to run to the next head", first.page.saturating_add(1)),
                block: Some(first.block),
            });
        }
        at = last.max(at) + 1;
    }
    attach_proofs(out);
}

/// What each proof proves: the statement its head names, else the statement right before it.
fn attach_proofs(out: &mut References) {
    let keys: Vec<Option<(String, String)>> = out
        .statements
        .iter()
        .map(|s| {
            if s.proof {
                return None;
            }
            Some((canonical_statement(&s.kind)?, s.number.clone()?))
        })
        .collect();
    let mut index: std::collections::HashMap<(String, String), Vec<usize>> = std::collections::HashMap::new();
    for (i, k) in keys.iter().enumerate() {
        if let Some(k) = k {
            index.entry(k.clone()).or_default().push(i);
        }
    }
    for i in 0..out.statements.len() {
        let Some(s) = out.statements.get(i).filter(|s| s.proof) else { continue };
        let named = s.name.as_deref().and_then(|n| {
            let rest = n.strip_prefix("of ")?;
            let mut parts = rest.split_whitespace();
            let kind = canonical_statement(parts.next()?)?;
            let number = number_core(parts.next()?)?.to_owned();
            Some((kind, number))
        });
        let proves = match named {
            Some(key) => match index.get(&key).map(Vec::as_slice) {
                Some([one]) => Some((*one, ProvesBy::Named)),
                _ => None,
            },
            None => i.checked_sub(1).filter(|&p| out.statements.get(p).is_some_and(|s| !s.proof)).map(|p| (p, ProvesBy::Previous)),
        };
        if let Some(s) = out.statements.get_mut(i) {
            s.proves = proves;
        }
        if proves.is_none() {
            let page = out.statements.get(i).map_or(0, |s| s.first_line.0);
            out.diagnostics.push(Diagnostic {
                code: "proof_unattached",
                message: format!("page {}: a proof whose statement was not found", page.saturating_add(1)),
                block: None,
            });
        }
    }
}
