//! A page's lines in reading order, cut into words with their glyphs and styles, and each line's
//! end-of-proof mark.

use super::{
    EMPTY, EndMark, EndMarkKind, FlowLine, GLYPH_BOLD, GLYPH_ITALIC, GLYPH_MATH, GLYPH_MONO, MAX_WORDS_PER_PAGE, PageWords, Style, Word, union,
};
use crate::text::math::MathPage;
use crate::text::page_layout::{BlockKind, Diagnostic, Line, PageLayout};

/// A word gap: the space between two glyphs, in ems of the larger (as the layout reads text).
const WORD_GAP: f32 = 0.18;

/// The style a font's name says: bold, italic, small caps, mathematics.
pub(super) fn font_style(name: Option<&str>) -> Style {
    let n = name.unwrap_or("").to_ascii_uppercase();
    const MATH: [&str; 16] =
        ["CMMI", "CMSY", "CMEX", "CMBSY", "MSAM", "MSBM", "EUFM", "EUFB", "EURM", "EURB", "EUSM", "EUSB", "EUEX", "RSFS", "WASY", "STMARY"];
    let math = MATH.iter().any(|m| n.starts_with(m)) || n.contains("MATH");
    let bold = (n.starts_with("CMB") && !n.starts_with("CMBSY")) || n.contains("BOLD") || n.contains("-BD") || n.contains("BX") || n.contains("-SB");
    let italic = ["CMTI", "CMSL", "CMBXTI", "CMBXSL", "CMITT", "CMSSI"].iter().any(|p| n.starts_with(p))
        || n.contains("ITALIC")
        || n.contains("OBLIQUE")
        || n.ends_with("-IT")
        || n.ends_with("-BI")
        || n.contains("ITAL");
    let small_caps = n.starts_with("CMCSC") || n.contains("SMALLCAP") || n.ends_with("-SC") || n.contains("CAPS");
    Style { bold, italic, small_caps, math }
}

/// A glyph's [`Word::flags`] from its font's style and name.
fn glyph_flags(style: Style, font_name: &str) -> u8 {
    let n = font_name.to_ascii_uppercase();
    // A subset prefix (`ABCDEF+CMTT10`) goes.
    let n = n.rsplit('+').next().unwrap_or("");
    let mono = n.starts_with("CMTT")
        || n.starts_with("CMITT")
        || n.starts_with("CMSLTT")
        || n.contains("MONO")
        || n.contains("COURIER")
        || n.contains("TYPEWRITER");
    let mut f = 0;
    if style.math {
        f |= GLYPH_MATH;
    }
    if style.italic {
        f |= GLYPH_ITALIC;
    }
    if style.bold {
        f |= GLYPH_BOLD;
    }
    if mono {
        f |= GLYPH_MONO;
    }
    f
}

/// Ligatures spelled out, so `Definition` reads as such whatever the font drew for `fi`.
pub(super) fn spell(text: &str, out: &mut String) {
    for c in text.chars() {
        match c {
            '\u{FB00}' => out.push_str("ff"),
            '\u{FB01}' => out.push_str("fi"),
            '\u{FB02}' => out.push_str("fl"),
            '\u{FB03}' => out.push_str("ffi"),
            '\u{FB04}' => out.push_str("ffl"),
            '\u{FB05}' | '\u{FB06}' => out.push_str("st"),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
}

/// A glyph that marks the end of a proof by its text or its name.
fn mark_glyph(text: &str, name: Option<&str>) -> Option<EndMarkKind> {
    match text {
        "■" | "∎" | "◼" | "▪" | "⬛" => return Some(EndMarkKind::Filled),
        "□" | "◻" | "▫" | "⬜" | "☐" => return Some(EndMarkKind::Open),
        _ => {}
    }
    match name? {
        "squaresolid" | "blacksquare" | "filledbox" | "uni25A0" | "uni220E" | "qed" => Some(EndMarkKind::Filled),
        "square" | "openbox" | "uni25A1" | "Box" => Some(EndMarkKind::Open),
        _ => None,
    }
}

struct Built {
    words: Vec<Word>,
    /// The line's text size: its glyphs' middle em.
    size: f32,
}

fn words_of(line: &Line, page: &MathPage, budget: &mut usize) -> Built {
    let mut words: Vec<Word> = Vec::new();
    let mut prev: Option<(f32, f32)> = None;
    let mut sizes: Vec<f32> = Vec::new();
    // Per word: bold, italic, small-caps and mathematics glyphs, all glyphs, letters not in mathematics.
    let mut tallies: Vec<[usize; 6]> = Vec::new();
    for &g in &line.glyphs {
        let Some(gd) = page.glyphs.get(g as usize) else { continue };
        let r = gd.rect;
        if !r.iter().all(|v| v.is_finite()) || !gd.size.is_finite() || gd.size <= 0.0 {
            continue;
        }
        let style = font_style(page.fonts.get(gd.font as usize).and_then(|f| f.name.as_deref()));
        let new_word = match prev {
            Some((x1, size)) => r[0] - x1 > WORD_GAP * size.max(gd.size),
            None => true,
        };
        if new_word {
            if *budget == 0 {
                break;
            }
            *budget -= 1;
            words.push(Word { rect: EMPTY, ..Default::default() });
            tallies.push([0; 6]);
        }
        let (Some(w), Some(t)) = (words.last_mut(), tallies.last_mut()) else { continue };
        w.starts.push(w.text.len());
        w.glyphs.push(g);
        w.spans.push([r[0], r[2]]);
        // A big operator or delimiter (CMEX) between letters is no part of the word's text: an
        // inline sum the layout drew into a number (`2∑.2.`) would hide it.
        let font_name = page.fonts.get(gd.font as usize).and_then(|f| f.name.as_deref()).unwrap_or("");
        w.flags.push(glyph_flags(style, font_name));
        let big = font_name.get(..4).is_some_and(|p| p.eq_ignore_ascii_case("CMEX"));
        if let Some(text) = gd.text.as_deref().filter(|_| !big) {
            spell(text, &mut w.text);
        }
        union(&mut w.rect, &r);
        t[4] += 1;
        if style.math {
            t[3] += 1;
        } else if gd.text.as_deref().is_some_and(|s| s.chars().any(char::is_alphanumeric)) {
            t[0] += usize::from(style.bold);
            t[1] += usize::from(style.italic);
            t[2] += usize::from(style.small_caps);
            t[5] += 1;
        }
        sizes.push(gd.size);
        prev = Some((r[2], gd.size));
    }
    for (w, t) in words.iter_mut().zip(&tallies) {
        let letters = t[5].max(1);
        w.style = Style { bold: t[0] * 2 > letters, italic: t[1] * 2 > letters, small_caps: t[2] * 2 > letters, math: t[3] * 2 > t[4] };
    }
    sizes.sort_by(f32::total_cmp);
    let size = sizes.get(sizes.len() / 2).copied().unwrap_or(10.0);
    Built { words, size }
}

/// The end mark at the end of `line` (words `words`, text size `size`): its last word when that is
/// a mark glyph or `QED`. A box of rules is found for the whole page ([`rule_boxes`]).
fn end_mark(line: &Line, words: &[Word], size: f32, page: &MathPage) -> Option<EndMark> {
    let mid = (line.rect[0] + line.rect[2]) / 2.0;
    if let Some(last) = words.last() {
        let right_side = last.rect[0] >= mid || words.len() == 1;
        let glyph = last.glyphs.last().copied();
        let gd = glyph.and_then(|g| page.glyphs.get(g as usize));
        if right_side
            && let (Some(g), Some(gd)) = (glyph, gd)
            && let Some(kind) = mark_glyph(gd.text.as_deref().unwrap_or(""), gd.name.as_deref())
        {
            return Some(EndMark { kind, glyphs: vec![g], rules: Vec::new(), rect: last.rect });
        }
        let bare: String = last.text.chars().filter(|c| *c != '.').collect();
        if right_side && bare.eq_ignore_ascii_case("qed") && (words.len() == 1 || last.rect[0] - line.rect[0] > 4.0 * size) {
            return Some(EndMark { kind: EndMarkKind::Qed, glyphs: last.glyphs.clone(), rules: Vec::new(), rect: last.rect });
        }
    }
    None
}

/// The words of one page, line by line in the layout's reading order. At most
/// [`MAX_WORDS_PER_PAGE`] words; past that the rest of the page is left out and it says so.
pub fn page_words(page: &MathPage, layout: &PageLayout) -> PageWords {
    let mut out = PageWords { page: layout.page, ..Default::default() };
    let mut budget = MAX_WORDS_PER_PAGE;
    'blocks: for &bi in &layout.reading_order {
        let Some(block) = layout.blocks.get(bi) else { continue };
        let column = block.column.and_then(|c| layout.columns.get(c)).map_or([block.rect[0], block.rect[2]], |c| [c.rect[0], c.rect[2]]);
        for (li, line) in block.lines.iter().enumerate() {
            if budget == 0 {
                out.diagnostics.push(Diagnostic {
                    code: "capped",
                    message: format!("more than {MAX_WORDS_PER_PAGE} words: the rest of the page is not read for statements and references"),
                    block: Some(bi),
                });
                break 'blocks;
            }
            let built = words_of(line, page, &mut budget);
            let mark = end_mark(line, &built.words, built.size, page);
            out.lines.push(FlowLine {
                page: layout.page,
                block: bi,
                kind: block.kind,
                first: li == 0,
                rect: line.rect,
                baseline: line.baseline,
                size: built.size,
                column,
                words: built.words,
                mark,
            });
        }
    }
    attach_boxes(page, &mut out);
    out.leading = leading(&out);
    out
}

/// Most rules looked at for boxes on one page.
const MAX_BOX_RULES: usize = 20_000;

/// Boxes of thin rules drawn one after another that touch, about an em square: amsthm's end
/// mark, wherever it is (beside a line, or in a display's margin for `\qedhere`). Linear in the
/// rules: a box's rules are drawn in a row.
fn rule_boxes(page: &MathPage) -> Vec<(Vec<u32>, [f32; 4])> {
    let mut out = Vec::new();
    let mut run: Vec<u32> = Vec::new();
    let mut rect = EMPTY;
    let thin = |r: &[f32; 4]| (r[2] - r[0]).min(r[3] - r[1]) <= 1.5;
    let touches = |a: &[f32; 4], b: &[f32; 4]| a[0] <= b[2] + 0.5 && b[0] <= a[2] + 0.5 && a[1] <= b[3] + 0.5 && b[1] <= a[3] + 0.5;
    let flush = |run: &mut Vec<u32>, rect: &mut [f32; 4], out: &mut Vec<(Vec<u32>, [f32; 4])>| {
        let (w, h) = (rect[2] - rect[0], rect[3] - rect[1]);
        if run.len() >= 3 && (3.0..=14.0).contains(&w) && (3.0..=14.0).contains(&h) && (w / h.max(0.1) - 1.0).abs() < 0.5 {
            out.push((std::mem::take(run), *rect));
        }
        run.clear();
        *rect = EMPTY;
    };
    for (i, r) in page.rules.iter().enumerate().take(MAX_BOX_RULES) {
        if !r.rect.iter().all(|v| v.is_finite()) || !thin(&r.rect) {
            flush(&mut run, &mut rect, &mut out);
            continue;
        }
        if !run.is_empty() && !touches(&rect, &r.rect) {
            flush(&mut run, &mut rect, &mut out);
        }
        run.push(i as u32);
        union(&mut rect, &r.rect);
    }
    flush(&mut run, &mut rect, &mut out);
    out
}

/// Gives each box of rules to the line it ends: the nearest baseline at its foot, in its column,
/// left of it.
fn attach_boxes(page: &MathPage, out: &mut PageWords) {
    for (rules, rect) in rule_boxes(page) {
        let best = out
            .lines
            .iter()
            .enumerate()
            .filter(|(_, l)| {
                l.mark.is_none()
                    && l.rect[0] <= rect[0]
                    && rect[0] <= l.column[1] + l.size
                    && rect[2] >= l.column[0]
                    && (l.baseline - rect[3]).abs() <= 0.6 * l.size
            })
            .min_by(|a, b| (a.1.baseline - rect[3]).abs().total_cmp(&(b.1.baseline - rect[3]).abs()))
            .map(|(i, _)| i);
        if let Some(l) = best.and_then(|i| out.lines.get_mut(i)) {
            l.mark = Some(EndMark { kind: EndMarkKind::RuleBox, glyphs: Vec::new(), rules, rect });
        }
    }
}

/// The page's usual distance between baselines in a paragraph: the middle of those between lines
/// of one paragraph block.
fn leading(out: &PageWords) -> Option<f32> {
    let mut d: Vec<f32> = out
        .lines
        .windows(2)
        .filter_map(|w| match w {
            [a, b] if a.block == b.block && a.kind == BlockKind::Paragraph => Some(b.baseline - a.baseline),
            _ => None,
        })
        .filter(|v| v.is_finite() && *v > 0.0)
        .collect();
    d.sort_by(f32::total_cmp);
    d.get(d.len() / 2).copied()
}

/// The glyphs of `word` whose text starts inside the byte range `from..to` of its text, and their box.
pub(super) fn glyphs_in(word: &Word, from: usize, to: usize) -> (Vec<u32>, [f32; 4]) {
    let mut rect = EMPTY;
    let mut glyphs = Vec::new();
    for ((g, s), x) in word.glyphs.iter().zip(&word.starts).zip(&word.spans) {
        if *s >= from && *s < to {
            glyphs.push(*g);
            union(&mut rect, &[x[0], word.rect[1], x[1], word.rect[3]]);
        }
    }
    (glyphs, rect)
}
