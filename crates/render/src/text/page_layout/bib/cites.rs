//! Citations in the text: brackets of labels (`[12]`, `[1, 3–5]`, `[BKM20]`, `[12, Thm. 3.1]`) and
//! names with years (`Smith (2020)`, `(Smith and Jones, 2020a; Kay et al., 2019)`), each resolved
//! to its entry or to nothing.

use super::text::{composed, is_dash, key};
use super::{Citation, CiteKind, CiteUnresolved, Entry, LabelStyle, MAX_CITATIONS, MAX_GROUP_CHARS, MAX_LABEL_BYTES, MAX_RANGE, Region};
use crate::text::page_layout::refs::{GLYPH_MATH, LineRef, PageWords, Piece, Word};
use crate::text::page_layout::{BlockKind, Diagnostic};
use std::collections::{HashMap, HashSet};

/// Words before a bracket that make it no citation: it numbers a part of this text.
const NOT_CITING: [&str; 26] = [
    "section",
    "sections",
    "sec",
    "secs",
    "§",
    "chapter",
    "chapters",
    "step",
    "steps",
    "item",
    "items",
    "part",
    "parts",
    "figure",
    "fig",
    "figs",
    "table",
    "tab",
    "eq",
    "eqs",
    "equation",
    "equations",
    "line",
    "lines",
    "case",
    "cases",
];
/// Most words a bracket may span.
const MAX_GROUP_WORDS: usize = 16;
/// Most words read back from a year for its names.
const MAX_NAME_WORDS: usize = 8;

/// A word of the text where citations may be.
struct Tok<'a> {
    page: usize,
    word: &'a Word,
    /// A list item's label (`[2]` opening an item): never a citation.
    item_label: bool,
}

/// The glyphs of `word` whose text starts in the byte range `from..to`, and their box.
fn glyphs_in(word: &Word, from: usize, to: usize) -> (Vec<u32>, [f32; 4]) {
    let mut rect = [f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY];
    let mut glyphs = Vec::new();
    for ((g, s), x) in word.glyphs.iter().zip(&word.starts).zip(&word.spans) {
        if *s >= from && *s < to {
            glyphs.push(*g);
            rect = [rect[0].min(x[0]), rect[1].min(word.rect[1]), rect[2].max(x[1]), rect[3].max(word.rect[3])];
        }
    }
    (glyphs, rect)
}

/// Whether a glyph of `word` whose text starts in `from..to` is set in a mathematics font.
fn math_in(word: &Word, from: usize, to: usize) -> bool {
    word.starts.iter().zip(&word.flags).any(|(s, f)| *s >= from && *s < to && f & GLYPH_MATH != 0)
}

/// Adds the glyphs of `word` in `from..to` to `pieces`, one piece per page.
fn add_part(pieces: &mut Vec<Piece>, page: usize, word: &Word, from: usize, to: usize) {
    let (glyphs, rect) = glyphs_in(word, from, to);
    if glyphs.is_empty() || !rect.iter().all(|v| v.is_finite()) {
        return;
    }
    match pieces.last_mut().filter(|p| p.page == page) {
        Some(p) => {
            p.glyphs.extend(glyphs);
            p.rect = [p.rect[0].min(rect[0]), p.rect[1].min(rect[1]), p.rect[2].max(rect[2]), p.rect[3].max(rect[3])];
        }
        None => pieces.push(Piece { page, glyphs, rules: Vec::new(), rect }),
    }
}

/// The entries by label, as printed and as [`key`] reads them.
struct Labels {
    exact: HashMap<String, Vec<usize>>,
    loose: HashMap<String, Vec<usize>>,
    numeric: bool,
}

fn labels(entries: &[Entry]) -> Labels {
    let mut exact: HashMap<String, Vec<usize>> = HashMap::new();
    let mut loose: HashMap<String, Vec<usize>> = HashMap::new();
    for e in entries {
        if let Some(l) = &e.label {
            exact.entry(l.clone()).or_default().push(e.id);
            loose.entry(key(l)).or_default().push(e.id);
        }
    }
    let numeric = !exact.is_empty() && exact.keys().all(|k| k.bytes().all(|b| b.is_ascii_digit()));
    Labels { exact, loose, numeric }
}

impl Labels {
    fn find(&self, text: &str) -> Result<usize, CiteUnresolved> {
        let one = |v: Option<&Vec<usize>>| match v.map(Vec::as_slice) {
            Some([one]) => Ok(*one),
            Some([]) | None => Err(CiteUnresolved::NoEntry),
            Some(_) => Err(CiteUnresolved::Ambiguous),
        };
        match one(self.exact.get(text)) {
            Err(CiteUnresolved::NoEntry) if !self.numeric => one(self.loose.get(&key(text))),
            r => r,
        }
    }
}

/// What the part of a bracket between commas is.
enum Part {
    Number(String),
    Range(String, String),
    Label(String),
    Other,
}

fn digits(s: &str) -> bool {
    !s.is_empty() && s.len() <= 40 && s.bytes().all(|b| b.is_ascii_digit())
}

fn part_of(text: &str, labels: &Labels) -> Part {
    if digits(text) {
        return Part::Number(text.to_owned());
    }
    if let Some((a, b)) = text.split_once(is_dash) {
        let (a, b) = (a.trim(), b.trim_start_matches(is_dash).trim());
        if digits(a) && digits(b) {
            return Part::Range(a.to_owned(), b.to_owned());
        }
    }
    if text.len() <= MAX_LABEL_BYTES && !labels.numeric && labels.find(text).is_ok() {
        return Part::Label(text.to_owned());
    }
    Part::Other
}

/// A character of a bracket's inside: its word (index into the tokens) and byte offset, or a space
/// between words.
#[derive(Clone, Copy)]
struct At {
    tok: usize,
    byte: usize,
    c: char,
}

struct Out {
    cites: Vec<Citation>,
    unresolved: usize,
    ambiguous: usize,
    ranges_capped: usize,
    capped: bool,
    group: usize,
}

impl Out {
    fn push(&mut self, c: Citation) {
        if self.cites.len() >= MAX_CITATIONS {
            self.capped = true;
            return;
        }
        match c.unresolved {
            Some(CiteUnresolved::NoEntry) => self.unresolved += 1,
            Some(CiteUnresolved::Ambiguous) => self.ambiguous += 1,
            None => {}
        }
        self.cites.push(c);
    }
}

fn resolved(r: Result<usize, CiteUnresolved>, confidence: f32) -> (Option<usize>, Option<CiteUnresolved>, f32) {
    match r {
        Ok(t) => (Some(t), None, confidence),
        Err(u) => (None, Some(u), 0.0),
    }
}

/// The bracket opening at byte `open` of token `i`, read as a group of citations: `Some(next
/// token)` when it was one.
fn bracket(toks: &[Tok<'_>], i: usize, open: usize, labels: &Labels, out: &mut Out) -> Option<usize> {
    let t = toks.get(i)?;
    // `f[x]`: a bracket inside a word is no citation; nor one after a word that numbers this text.
    let before = t.word.text.get(..open).unwrap_or("");
    if !before.chars().all(|c| matches!(c, '(' | '~')) || math_in(t.word, 0, open + 1) || t.item_label {
        return None;
    }
    if open == 0
        && let Some(p) = i.checked_sub(1).and_then(|p| toks.get(p))
    {
        let w = p.word.text.to_lowercase();
        let w = w.trim_end_matches(['~', '.']);
        if NOT_CITING.contains(&w) && p.page == t.page {
            return None;
        }
    }
    // The inside, to the closing bracket.
    let mut inside: Vec<At> = Vec::new();
    let mut close: Option<(usize, usize)> = None;
    let mut k = i;
    let mut start = open + 1;
    'words: while let Some(w) = toks.get(k) {
        if k > i {
            inside.push(At { tok: usize::MAX, byte: 0, c: ' ' });
        }
        for (b, c) in w.word.text.get(start..).unwrap_or("").char_indices() {
            let byte = start + b;
            match c {
                ']' => {
                    close = Some((k, byte));
                    break 'words;
                }
                '[' => return None,
                _ => inside.push(At { tok: k, byte, c }),
            }
            if inside.len() > MAX_GROUP_CHARS {
                return None;
            }
        }
        k += 1;
        start = 0;
        if k - i >= MAX_GROUP_WORDS {
            return None;
        }
    }
    let (ck, cb) = close?;
    // No glyph of the inside, the brackets or what is stuck to the closing one set in mathematics.
    for k2 in i..=ck {
        let w = toks.get(k2)?.word;
        let from = if k2 == i { open } else { 0 };
        if math_in(w, from, w.text.len()) {
            return None;
        }
    }
    let tail = toks.get(ck)?.word.text.get(cb + 1..).unwrap_or("");
    if tail.starts_with(|c: char| c.is_alphanumeric()) {
        return None;
    }
    // Parts between commas and semicolons, spaces trimmed.
    let mut parts: Vec<Vec<At>> = vec![Vec::new()];
    for a in inside {
        if matches!(a.c, ',' | ';') {
            parts.push(Vec::new());
        } else if let Some(p) = parts.last_mut() {
            p.push(a);
        }
    }
    let text_of = |p: &[At]| p.iter().map(|a| a.c).collect::<String>().trim().to_owned();
    let kinds: Vec<(Part, &[At])> = parts.iter().map(|p| (part_of(&text_of(p), labels), p.as_slice())).collect();
    let labelled = kinds.iter().take_while(|(k, _)| !matches!(k, Part::Other)).count();
    if labelled == 0 {
        return None;
    }
    let known = |s: &str| labels.find(s).is_ok();
    let any_known = kinds.iter().take(labelled).any(|(k, _)| match k {
        Part::Number(n) | Part::Label(n) => known(n),
        Part::Range(a, b) => known(a) || known(b),
        Part::Other => false,
    });
    if !any_known
        || (matches!(kinds.first(), Some((Part::Number(_) | Part::Range(..), _)))
            && !labels.numeric
            && labels.find(&text_of(kinds.first()?.1)).is_err())
    {
        return None;
    }
    let locator = {
        let rest: Vec<String> = kinds.iter().skip(labelled).map(|(_, p)| text_of(p)).collect();
        (!rest.is_empty()).then(|| rest.join(", ")).filter(|s| !s.is_empty())
    };
    out.group += 1;
    let group = out.group;
    let kind = if labels.numeric { CiteKind::Numeric } else { CiteKind::Alpha };
    // The glyphs of the trimmed characters `chars` (a part, or one end of a range).
    let pieces_of = |chars: &[At]| {
        let mut pieces = Vec::new();
        let first = chars.iter().position(|a| !a.c.is_whitespace());
        let last = chars.iter().rposition(|a| !a.c.is_whitespace());
        let (Some(f), Some(l)) = (first, last) else { return pieces };
        let slice = chars.get(f..=l).unwrap_or_default();
        // Per word: the byte range of its characters.
        let mut by_word: Vec<(usize, usize, usize)> = Vec::new();
        for a in slice.iter().filter(|a| a.tok != usize::MAX) {
            let end = a.byte + a.c.len_utf8();
            match by_word.last_mut().filter(|(t, _, _)| *t == a.tok) {
                Some(w) => w.2 = end,
                None => by_word.push((a.tok, a.byte, end)),
            }
        }
        for (t, from, to) in by_word {
            if let Some(tok) = toks.get(t) {
                add_part(&mut pieces, tok.page, tok.word, from, to);
            }
        }
        pieces
    };
    for (part, chars) in kinds.iter().take(labelled) {
        match part {
            Part::Number(n) | Part::Label(n) => {
                let (target, unresolved, confidence) = resolved(labels.find(n), 0.9);
                out.push(Citation {
                    kind,
                    text: n.clone(),
                    locator: locator.clone(),
                    pieces: pieces_of(chars),
                    group,
                    target,
                    unresolved,
                    in_range: false,
                    confidence,
                });
            }
            Part::Range(a, b) => {
                // The two ends, each with its own glyphs.
                let dash = chars.iter().position(|x| is_dash(x.c)).unwrap_or(0);
                let (left, right) = chars.split_at(dash);
                let right = right.get(1..).unwrap_or_default();
                let right = right.get(right.iter().position(|x| !is_dash(x.c)).unwrap_or(0)..).unwrap_or_default();
                for (n, cs) in [(a, left), (b, right)] {
                    let (target, unresolved, confidence) = resolved(labels.find(n), 0.9);
                    out.push(Citation {
                        kind,
                        text: n.clone(),
                        locator: None,
                        pieces: pieces_of(cs),
                        group,
                        target,
                        unresolved,
                        in_range: false,
                        confidence,
                    });
                }
                // The numbers between, lazily: only a short range of numbers the list has.
                let (lo, hi) = (a.parse::<u64>().ok(), b.parse::<u64>().ok());
                match (lo, hi) {
                    (Some(lo), Some(hi)) if hi > lo.saturating_add(1) && hi - lo - 1 <= MAX_RANGE as u64 => {
                        for m in lo + 1..hi {
                            let n = m.to_string();
                            let (target, unresolved, confidence) = resolved(labels.find(&n), 0.85);
                            out.push(Citation {
                                kind,
                                text: n,
                                locator: None,
                                pieces: Vec::new(),
                                group,
                                target,
                                unresolved,
                                in_range: true,
                                confidence,
                            });
                        }
                    }
                    (Some(lo), Some(hi)) if hi > lo.saturating_add(1) => out.ranges_capped += 1,
                    (None, _) | (_, None) => out.ranges_capped += 1,
                    _ => {}
                }
            }
            Part::Other => {}
        }
    }
    Some(ck + 1)
}

/// Entries by their first author's family name and year (without a suffix), for author-year
/// citations; and every family name of the list.
struct Names {
    by: HashMap<(String, String), Vec<usize>>,
    families: HashSet<String>,
}

fn year_base(y: &str) -> (String, String) {
    let digits: String = y.chars().take_while(char::is_ascii_digit).collect();
    let suffix = y.get(digits.len()..).unwrap_or("").to_owned();
    (digits, suffix)
}

fn names(entries: &[Entry]) -> Names {
    let mut by: HashMap<(String, String), Vec<usize>> = HashMap::new();
    let mut families = HashSet::new();
    for e in entries {
        for a in &e.fields.authors {
            families.insert(key(&a.family));
        }
        let (Some(first), Some(year)) = (e.fields.authors.first(), &e.fields.year) else { continue };
        by.entry((key(&first.family), year_base(&year.value).0)).or_default().push(e.id);
    }
    Names { by, families }
}

/// The years in a word of running text: `2020`, `(2020a)`, `2020,` `2019;`, and natbib's
/// `2020c,a)` (the same names' `2020a` after `2020c`). Each with where it is in the word's text.
fn text_years(t: &str) -> Vec<(String, usize, usize)> {
    // natbib sets them in parentheses, or in brackets (amsart).
    let start = t.len() - t.trim_start_matches(['(', '[']).len();
    let core = t.get(start..).unwrap_or("").trim_end_matches([')', ']', ',', ';', '.', ':']);
    let mut out: Vec<(String, usize, usize)> = Vec::new();
    let mut at = start;
    for (k, item) in core.split(',').enumerate() {
        let from = at;
        at = at.saturating_add(item.len()).saturating_add(1);
        let (d, s) = year_base(item);
        if k > 0 && d.is_empty() && s.chars().count() == 1 && s.chars().all(|c| c.is_ascii_lowercase()) {
            // A suffix alone: the year before it, with this letter.
            let Some(base) = out.first().map(|(y, _, _)| year_base(y).0) else { break };
            out.push((format!("{base}{s}"), from, from + item.len()));
            continue;
        }
        let ok = d.len() == 4
            && d.parse::<u32>().is_ok_and(|y| (1000..=2999).contains(&y))
            && s.chars().count() <= 1
            && s.chars().all(|c| c.is_ascii_lowercase());
        if !ok {
            break;
        }
        out.push((item.to_owned(), from, from + item.len()));
    }
    out
}

/// Who an author-year citation names: the first author's family name, the second's when there are
/// two, `et al.`; and the words that show it (from the family name on).
#[derive(Clone, Debug)]
struct Who {
    family: String,
    second: Option<String>,
    et_al: bool,
    words: Vec<usize>,
}

/// The names before the year at token `i`: capitalised words, `and`, `&`, `et al.`, particles.
fn who_before(toks: &[Tok<'_>], i: usize, names: &Names, used: &HashSet<usize>) -> Option<Who> {
    let t = toks.get(i)?;
    let mut from = i;
    while from > 0 && i - from < MAX_NAME_WORDS {
        let Some(p) = toks.get(from - 1) else { break };
        if used.contains(&(from - 1)) || p.page != t.page {
            break;
        }
        let raw = p.word.text.as_str();
        if raw.ends_with([';', ')', ']']) || (from == i && raw.ends_with('.') && !raw.ends_with("al.")) {
            break;
        }
        let bare = raw.trim_matches(|c: char| matches!(c, '(' | '[' | ',' | ';' | '~'));
        let lower = bare.to_lowercase();
        // `Hal-` `dane`: a name broken by a hyphen at a line's end.
        let broken = bare.starts_with(char::is_lowercase) && from >= 2 && toks.get(from - 2).is_some_and(|b| b.word.text.ends_with('-'));
        let ok = matches!(lower.as_str(), "and" | "&" | "et" | "al" | "al.")
            || super::fields_particle(&lower)
            || bare.starts_with(char::is_uppercase)
            || broken;
        if !ok {
            break;
        }
        from -= 1;
        if raw.starts_with(['(', '[']) {
            break;
        }
    }
    // Words, a name broken by a hyphen joined again (TeX's hyphen dropped before a lower-case rest).
    let mut words: Vec<(usize, String)> = Vec::new();
    for k in from..i {
        let Some(x) = toks.get(k) else { continue };
        let w = composed(x.word);
        match words.last_mut() {
            Some((_, prev)) if prev.ends_with('-') && w.starts_with(char::is_alphabetic) => {
                if w.starts_with(char::is_lowercase) {
                    prev.pop();
                }
                prev.push_str(&w);
            }
            _ => words.push((k, w)),
        }
    }
    let clean = |s: &str| s.trim_matches(|c: char| matches!(c, '(' | '[' | ',' | ';' | '~' | '.')).to_owned();
    let split = words.iter().position(|(_, w)| matches!(clean(w).to_lowercase().as_str(), "and" | "&" | "et"));
    let first_part = words.get(..split.unwrap_or(words.len())).unwrap_or_default();
    let et_al = words.iter().any(|(_, w)| clean(w).to_lowercase() == "et");
    let second = split.filter(|_| !et_al).and_then(|s| words.get(s + 1..)).filter(|r| !r.is_empty());
    let last = first_part.len().checked_sub(1)?;
    let mut fam_at = last;
    while fam_at > 0 && first_part.get(fam_at - 1).is_some_and(|(_, w)| super::fields_particle(&clean(w).to_lowercase())) {
        fam_at -= 1;
    }
    let family = first_part.get(fam_at..).unwrap_or_default().iter().map(|(_, w)| clean(w)).collect::<Vec<_>>().join(" ");
    if !names.families.contains(&key(&family)) {
        return None;
    }
    Some(Who {
        family,
        second: second.map(|s| s.iter().map(|(_, w)| clean(w)).collect::<Vec<_>>().join(" ")),
        et_al,
        words: words.get(fam_at..).and_then(|w| w.first()).map_or_else(Vec::new, |(k0, _)| (*k0..i).collect()),
    })
}

/// The entry `who` and `year` name.
fn entry_of(who: &Who, year: &str, entries: &[Entry], names: &Names) -> Result<usize, CiteUnresolved> {
    let (base, suffix) = year_base(year);
    let mut cands: Vec<usize> = names.by.get(&(key(&who.family), base)).cloned().unwrap_or_default();
    let year_of = |e: usize| entries.get(e).and_then(|x| x.fields.year.as_ref()).map(|y| y.value.clone()).unwrap_or_default();
    if suffix.is_empty() {
        // `2020` names no entry printed `2020a` when there are several.
        let plain: Vec<usize> = cands.iter().copied().filter(|e| year_base(&year_of(*e)).1.is_empty()).collect();
        if !plain.is_empty() {
            cands = plain;
        }
    } else {
        cands.retain(|e| year_of(*e) == year);
    }
    let count = |e: usize| entries.get(e).map_or(0, |x| x.fields.authors.len());
    let narrow = |cands: &mut Vec<usize>, keep: &dyn Fn(usize) -> bool| {
        let kept: Vec<usize> = cands.iter().copied().filter(|e| keep(*e)).collect();
        if !kept.is_empty() {
            *cands = kept;
        }
    };
    if let Some(second) = &who.second {
        let skey = key(second);
        cands.retain(|e| {
            entries.get(*e).and_then(|x| x.fields.authors.get(1)).is_some_and(|a| !key(&a.family).is_empty() && skey.ends_with(&key(&a.family)))
        });
        narrow(&mut cands, &|e| count(e) == 2);
    } else if who.et_al {
        narrow(&mut cands, &|e| count(e) >= 3);
    } else {
        narrow(&mut cands, &|e| count(e) == 1);
    }
    match cands.as_slice() {
        [one] => Ok(*one),
        [] => Err(CiteUnresolved::NoEntry),
        _ => Err(CiteUnresolved::Ambiguous),
    }
}

/// The author-year citations whose year is token `i`: its names are the words before it, or, for a
/// year right after another citation's (`Okafor, 2019, 2020`), that one's. Returns the names read.
fn author_year(
    toks: &[Tok<'_>],
    i: usize,
    entries: &[Entry],
    names: &Names,
    used: &HashSet<usize>,
    last: Option<(usize, &Who)>,
    out: &mut Out,
) -> Option<Who> {
    let t = toks.get(i)?;
    if math_in(t.word, 0, t.word.text.len()) {
        return None;
    }
    let years = text_years(&t.word.text);
    if years.is_empty() {
        return None;
    }
    let (who, own_names) = match who_before(toks, i, names, used) {
        Some(w) => (w, true),
        None => match last {
            Some((at, w)) if at + 1 == i && toks.get(at).is_some_and(|p| p.word.text.ends_with(',')) => (w.clone(), false),
            _ => return None,
        },
    };
    let opens =
        who.words.first().and_then(|k| toks.get(*k)).is_some_and(|w| w.word.text.starts_with(['(', '['])) || t.word.text.starts_with(['(', '[']);
    if (own_names && opens) || out.cites.last().is_none_or(|c| c.kind != CiteKind::AuthorYear) {
        out.group += 1;
    }
    let locator = if t.word.text.ends_with(',') && !toks.get(i + 1).is_some_and(|n| !text_years(&n.word.text).is_empty()) {
        let mut l = Vec::new();
        for k in i + 1..(i + 6).min(toks.len()) {
            let Some(x) = toks.get(k) else { break };
            l.push(x.word.text.trim_end_matches([')', ']', ';', '.']).to_owned());
            if x.word.text.trim_end_matches('.').ends_with([')', ']', ';']) {
                break;
            }
        }
        Some(l.join(" ")).filter(|s| !s.is_empty() && s.len() < 60)
    } else {
        None
    };
    let mut text = who.family.clone();
    if let Some(s) = &who.second {
        text = format!("{text} and {s}");
    } else if who.et_al {
        text.push_str(" et al.");
    }
    for (k, (year, ys, ye)) in years.iter().enumerate() {
        // The clickable glyphs: the names (the first year's only, when they are its own) and the year.
        let mut pieces = Vec::new();
        if k == 0 && own_names {
            for &w in &who.words {
                let Some(tok) = toks.get(w) else { continue };
                let raw = tok.word.text.as_str();
                let lead = raw.len() - raw.trim_start_matches(['(', '[', '~']).len();
                let tail = raw.trim_end_matches([',', ';', '~']).len();
                add_part(&mut pieces, tok.page, tok.word, lead, tail.max(lead));
            }
        }
        add_part(&mut pieces, t.page, t.word, *ys, *ye);
        let suffix = !year_base(year).1.is_empty();
        let (target, unresolved, confidence) = resolved(entry_of(&who, year, entries, names), if suffix { 0.85 } else { 0.8 });
        out.push(Citation {
            kind: CiteKind::AuthorYear,
            text: format!("{text} {year}"),
            locator: locator.clone().filter(|_| k + 1 == years.len()),
            pieces,
            group: out.group,
            target,
            unresolved,
            in_range: false,
            confidence,
        });
    }
    Some(who)
}

/// The citations of the text outside the list, in reading order.
pub(super) fn find(
    words: &[PageWords],
    region: &Region,
    list: &HashSet<(LineRef, usize)>,
    entries: &[Entry],
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<Citation> {
    let mut toks: Vec<Tok<'_>> = Vec::new();
    for (p, pw) in words.iter().enumerate() {
        for (l, line) in pw.lines.iter().enumerate() {
            // Headings too: the layout may take a short paragraph for one.
            let kind_ok = matches!(
                line.kind,
                BlockKind::Paragraph
                    | BlockKind::ListItem
                    | BlockKind::Footnote
                    | BlockKind::Caption
                    | BlockKind::Abstract
                    | BlockKind::Other
                    | BlockKind::Heading
            );
            let heading = region.heading.as_ref().is_some_and(|h| h.page == p && line.rect == h.rect);
            if !kind_ok || heading {
                continue;
            }
            for (k, w) in line.words.iter().enumerate() {
                if list.contains(&((p, l), k)) {
                    continue;
                }
                toks.push(Tok { page: line.page, word: w, item_label: line.kind == BlockKind::ListItem && line.first && k == 0 });
            }
        }
    }
    let labels = labels(entries);
    let unlabelled = entries.iter().filter(|e| e.label_style == LabelStyle::None || e.label.is_none()).count() * 2 > entries.len();
    let names = names(entries);
    let mut out = Out { cites: Vec::new(), unresolved: 0, ambiguous: 0, ranges_capped: 0, capped: false, group: 0 };
    let mut used: HashSet<usize> = HashSet::new();
    let mut last_who: Option<(usize, Who)> = None;
    let mut i = 0;
    while i < toks.len() && !out.capped {
        let Some(t) = toks.get(i) else { break };
        if !labels.exact.is_empty()
            && let Some(open) = t.word.text.find('[')
            && let Some(next) = bracket(&toks, i, open, &labels, &mut out)
        {
            used.extend(i..next);
            i = next.max(i + 1);
            continue;
        }
        if unlabelled && let Some(who) = author_year(&toks, i, entries, &names, &used, last_who.as_ref().map(|(k, w)| (*k, w)), &mut out) {
            used.insert(i);
            last_who = Some((i, who));
        }
        i += 1;
    }
    let mut say = |code: &'static str, n: usize, what: &str| {
        if n > 0 {
            diagnostics.push(Diagnostic { code, message: format!("{n} {what}"), block: None });
        }
    };
    say("citation_unresolved", out.unresolved, "citations name no entry of the list (no link)");
    say("citation_ambiguous", out.ambiguous, "citations name more than one entry (no link)");
    say("range_capped", out.ranges_capped, &format!("ranges of more than {MAX_RANGE} numbers cite their two ends only"));
    if out.capped {
        diagnostics.push(Diagnostic {
            code: "citations_capped",
            message: format!("more than {MAX_CITATIONS} citations: the rest are left out"),
            block: None,
        });
    }
    out.cites
}
