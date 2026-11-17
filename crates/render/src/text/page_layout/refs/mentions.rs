//! Mentions: a kind word and a number (or an equation number alone where a citing word comes
//! before it), and the block each resolves to.

use super::statements::parse_head;
use super::words::glyphs_in;
use super::{
    EMPTY, MAX_MENTIONS, MAX_NUMBER_BYTES, Mention, MentionKind, PageWords, Piece, References, Target, Unresolved, Word, finite_or_zero, union,
};
use crate::text::page_layout::{BlockKind, Diagnostic, DocumentStructure, PageLayout};
use std::collections::{HashMap, HashSet};

/// Statement words every document may use, singular and lower case.
const STATEMENT_WORDS: [&str; 20] = [
    "theorem",
    "lemma",
    "proposition",
    "corollary",
    "claim",
    "conjecture",
    "definition",
    "remark",
    "example",
    "fact",
    "assumption",
    "notation",
    "observation",
    "hypothesis",
    "problem",
    "question",
    "exercise",
    "property",
    "convention",
    "axiom",
];
/// Abbreviations of statement words, without their period.
const ABBREVIATIONS: [(&str, &str); 14] = [
    ("thm", "theorem"),
    ("thms", "theorem"),
    ("lem", "lemma"),
    ("lems", "lemma"),
    ("prop", "proposition"),
    ("props", "proposition"),
    ("cor", "corollary"),
    ("cors", "corollary"),
    ("def", "definition"),
    ("defn", "definition"),
    ("defs", "definition"),
    ("rem", "remark"),
    ("ex", "example"),
    ("conj", "conjecture"),
];
/// Words before an equation number alone that make it a citation.
const CITING: [&str; 12] = ["by", "see", "from", "in", "using", "cf", "cf.", "via", "and", "with", "to", "or"];
/// After a mention, words that say it is of another work when a citation follows them.
const ELSEWHERE: [&str; 11] = ["paper", "papers", "book", "article", "monograph", "thesis", "survey", "preprint", "notes", "lectures", "work"];
/// How far after a mention the words that send it elsewhere are looked for.
const ELSEWHERE_WINDOW: usize = 8;
/// Most numbers one mention lists (`Lemmas 2.1, 2.3 and 2.5`).
const MAX_LIST: usize = 20;

/// The leading number of `t`: `2.1`, `A.4`, `3`, `12a` (the `.` before a digit only; ASCII
/// digits only). `None` when `t` does not start with one.
pub(super) fn number_core(t: &str) -> Option<&str> {
    let b = t.as_bytes();
    let mut i = 0;
    // An appendix letter: `A.4`.
    if b.first().is_some_and(u8::is_ascii_uppercase) && b.get(1) == Some(&b'.') && b.get(2).is_some_and(u8::is_ascii_digit) {
        i = 2;
    }
    let start_digits = i;
    while b.get(i).is_some_and(u8::is_ascii_digit) {
        i += 1;
    }
    if i == start_digits {
        return None;
    }
    while b.get(i) == Some(&b'.') && b.get(i + 1).is_some_and(u8::is_ascii_digit) {
        i += 1;
        while b.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
    }
    // One letter right after (`12a`), not the start of a word.
    if b.get(i).is_some_and(u8::is_ascii_lowercase) && !b.get(i + 1).is_some_and(u8::is_ascii_alphabetic) {
        i += 1;
    }
    let n = t.get(..i)?;
    (n.len() <= MAX_NUMBER_BYTES).then_some(n)
}

/// A statement word as a head prints it, lower case and singular (`lemma` for `Lemmas`, `Lem.`),
/// or `None` for a word no statement uses. A phrase (`Main Theorem`) keeps its first words.
pub(super) fn canonical_statement(phrase: &str) -> Option<String> {
    let mut words: Vec<String> = phrase.split_whitespace().map(str::to_lowercase).collect();
    let last = words.pop()?;
    let last = last.trim_end_matches('.');
    let single = if let Some((_, full)) = ABBREVIATIONS.iter().find(|(a, _)| *a == last) {
        (*full).to_owned()
    } else if STATEMENT_WORDS.contains(&last) {
        last.to_owned()
    } else if last == "lemmata" {
        "lemma".to_owned()
    } else if let Some(s) = last.strip_suffix("es").filter(|s| STATEMENT_WORDS.contains(s)) {
        s.to_owned()
    } else {
        last.strip_suffix('s').filter(|s| STATEMENT_WORDS.contains(s))?.to_owned()
    };
    words.push(single);
    Some(words.join(" "))
}

/// The key a statement is found by: its canonical word, or its word in lower case.
pub(super) fn kind_key(kind: &str) -> String {
    canonical_statement(kind).unwrap_or_else(|| kind.to_lowercase())
}

#[derive(Clone, Debug, PartialEq)]
enum WordKind {
    Statement(String),
    Section,
    Equation,
    Figure,
    Table,
}

impl WordKind {
    fn mention_kind(&self) -> MentionKind {
        match self {
            WordKind::Statement(_) => MentionKind::Statement,
            WordKind::Section => MentionKind::Section,
            WordKind::Equation => MentionKind::Equation,
            WordKind::Figure => MentionKind::Figure,
            WordKind::Table => MentionKind::Table,
        }
    }
}

/// What a kind word names, and whether it is plural. `doc_kinds`: the keys of the document's own
/// statements (an open set: `Hypothesis`, `Problem`).
fn mention_word(word: &str, doc_kinds: &HashSet<String>) -> Option<(WordKind, bool)> {
    let lower = word.to_lowercase();
    let bare = lower.trim_end_matches('.');
    let fixed = match bare {
        "section" | "sec" | "subsection" | "chapter" | "appendix" | "§" => Some((WordKind::Section, false)),
        "sections" | "secs" | "subsections" | "chapters" | "appendices" | "§§" => Some((WordKind::Section, true)),
        "figure" | "fig" => Some((WordKind::Figure, false)),
        "figures" | "figs" => Some((WordKind::Figure, true)),
        "table" | "tab" => Some((WordKind::Table, false)),
        "tables" | "tabs" => Some((WordKind::Table, true)),
        "equation" | "eq" | "eqn" | "formula" => Some((WordKind::Equation, false)),
        "equations" | "eqs" | "eqns" | "formulas" | "formulae" => Some((WordKind::Equation, true)),
        _ => None,
    };
    if fixed.is_some() {
        // An abbreviation has its period; a full word has none.
        let abbreviated = matches!(bare, "sec" | "secs" | "fig" | "figs" | "tab" | "tabs" | "eq" | "eqs" | "eqn" | "eqns");
        return (abbreviated == lower.ends_with('.') || bare.starts_with('§')).then_some(fixed).flatten();
    }
    // A statement word: capitalised, or a known one in lower case.
    let capital = word.chars().next().is_some_and(char::is_uppercase);
    let key = canonical_statement(word).or_else(|| {
        let k = bare.to_owned();
        let single = k.strip_suffix('s').map(str::to_owned);
        if doc_kinds.contains(&k) { Some(k) } else { single.filter(|s| doc_kinds.contains(s)) }
    })?;
    if !capital && !STATEMENT_WORDS.contains(&key.as_str()) {
        return None;
    }
    // An abbreviation needs its period, a full word must not end the sentence.
    let abbreviation = ABBREVIATIONS.iter().any(|(a, _)| *a == bare);
    if abbreviation != lower.ends_with('.') {
        return None;
    }
    let plural = key != bare && !abbreviation || matches!(bare, "thms" | "lems" | "props" | "cors" | "defs");
    Some((WordKind::Statement(key), plural))
}

/// A word holding a number: where the number is (bytes, with an equation's parentheses), what
/// follows it that is not linked (`(ii)`), and a second number after a dash (`2.1–2.3`).
#[derive(Clone, Debug, PartialEq)]
struct NumberWord {
    number: (usize, usize),
    extra: Option<(usize, usize)>,
    tail: Option<(usize, usize)>,
}

/// An equation number in parentheses at `from` in `t`: its byte range.
fn tag_at(t: &str, from: usize) -> Option<(usize, usize)> {
    let rest = t.get(from..)?.strip_prefix('(')?;
    let core = number_core(rest.trim_start())?;
    let pad = rest.len() - rest.trim_start().len();
    let after = rest.get(pad + core.len()..)?;
    let close = after.len() - after.trim_start().len();
    after.trim_start().starts_with(')').then(|| (from, from + 1 + pad + core.len() + close + 1))
}

fn only_punctuation(s: &str) -> bool {
    s.chars().all(|c| matches!(c, '.' | ',' | ';' | ':' | ')' | '\'' | '’'))
}

fn dash_len(s: &str) -> usize {
    ["--", "–", "—", "-"].iter().find(|d| s.starts_with(*d)).map_or(0, |d| d.len())
}

/// Reads a number word for a mention of `kind`: `2.1`, `2.1,`, `2.1(ii).`, `1–3`, `(3.4)`.
fn number_word(t: &str, kind: &WordKind) -> Option<NumberWord> {
    let eq = matches!(kind, WordKind::Equation);
    let (number, rest_at) = if eq && t.starts_with('(') {
        let r = tag_at(t, 0)?;
        (r, r.1)
    } else if matches!(kind, WordKind::Section) && t.starts_with(|c: char| c.is_ascii_uppercase()) && number_core(t).is_none() {
        // An appendix by its letter: `Appendix A`, `Section B`.
        let len = 1;
        if t.get(len..).is_some_and(|r| r.is_empty() || only_punctuation(r)) {
            ((0, len), len)
        } else {
            return None;
        }
    } else {
        let n = number_core(t)?;
        ((0, n.len()), n.len())
    };
    let mut at = rest_at;
    let mut extra = None;
    let mut tail = None;
    let rest = t.get(at..).unwrap_or("");
    if matches!(kind, WordKind::Statement(_)) && rest.starts_with('(') {
        let close = rest.find(')')?;
        let inner = rest.get(1..close).unwrap_or("");
        if !inner.is_empty() && inner.len() <= 6 && inner.chars().all(|c| c.is_ascii_alphanumeric()) {
            extra = Some((at, at + close + 1));
            at += close + 1;
        } else {
            return None;
        }
    }
    let rest = t.get(at..).unwrap_or("");
    let d = dash_len(rest);
    if d > 0 {
        let from = at + d;
        let second = if eq && t.get(from..).is_some_and(|r| r.starts_with('(')) {
            tag_at(t, from)
        } else {
            t.get(from..).and_then(number_core).map(|n| (from, from + n.len()))
        };
        let second = second?;
        tail = Some(second);
        at = second.1;
    }
    only_punctuation(t.get(at..).unwrap_or("")).then_some(NumberWord { number, extra, tail })
}

/// A token of the text: a word of a line where mentions may be.
struct Tok<'a> {
    page: usize,
    word: &'a Word,
    /// A list item's label (`(1)` opening an item): never a citation.
    label: bool,
    /// The last word of its line (a word broken by a hyphen goes on in the next token).
    line_end: bool,
}

/// Where each kind of block is, by number.
#[derive(Default)]
struct Index {
    statements: HashMap<(String, String), Vec<usize>>,
    sections: HashMap<String, Vec<(usize, usize)>>,
    equations: HashMap<String, Vec<(usize, usize)>>,
    figures: HashMap<String, Vec<(usize, usize)>>,
    tables: HashMap<String, Vec<(usize, usize)>>,
}

fn add<K: std::hash::Hash + Eq, V: PartialEq>(map: &mut HashMap<K, Vec<V>>, k: K, v: V) {
    let list = map.entry(k).or_default();
    if !list.contains(&v) {
        list.push(v);
    }
}

fn index(layouts: &[PageLayout], pages: &[PageWords], structure: &DocumentStructure, out: &References) -> Index {
    let mut ix = Index::default();
    for (i, s) in out.statements.iter().enumerate() {
        if let (false, Some(n)) = (s.proof, &s.number) {
            add(&mut ix.statements, (kind_key(&s.kind), n.clone()), i);
        }
    }
    for s in &structure.sections {
        if let Some(n) = &s.number {
            add(&mut ix.sections, n.clone(), (s.page, s.block));
        }
    }
    for l in layouts {
        for (bi, b) in l.blocks.iter().enumerate() {
            match b.kind {
                BlockKind::DisplayEquation => {
                    if let Some(tag) =
                        b.equation_tag.as_deref().and_then(|t| tag_at(t.trim(), 0).and_then(|_| number_core(t.trim().get(1..)?.trim_start())))
                    {
                        add(&mut ix.equations, tag.to_owned(), (l.page, bi));
                    }
                }
                BlockKind::Caption => {
                    let t = b.text.trim_start();
                    let (map, rest) = if let Some(r) = t.strip_prefix("Figure").or_else(|| t.strip_prefix("Fig.")) {
                        (&mut ix.figures, r)
                    } else if let Some(r) = t.strip_prefix("Table") {
                        (&mut ix.tables, r)
                    } else {
                        continue;
                    };
                    if let Some(n) = number_core(rest.trim_start()) {
                        add(map, n.to_owned(), (l.page, bi));
                    }
                }
                _ => {}
            }
        }
    }
    // Every row of a display carries its number at a margin: an alignment has several.
    for pw in pages {
        for line in pw.lines.iter().filter(|l| l.kind == BlockKind::DisplayEquation) {
            for w in [line.words.first(), line.words.last()].into_iter().flatten() {
                let t = w.text.trim();
                if tag_at(t, 0).is_some_and(|r| r.1 == t.len())
                    && let Some(n) = t.get(1..).and_then(|r| number_core(r.trim_start()))
                {
                    add(&mut ix.equations, n.to_owned(), (line.page, line.block));
                }
            }
        }
    }
    ix
}

struct Found {
    kind: WordKind,
    word: String,
    number: String,
    extra: Option<String>,
    /// (page, glyphs, box) of the clickable part.
    parts: Vec<(usize, Vec<u32>, [f32; 4])>,
    bare: bool,
}

fn part(tok: &Tok<'_>, range: (usize, usize)) -> (usize, Vec<u32>, [f32; 4]) {
    let (g, r) = glyphs_in(tok.word, range.0, range.1);
    (tok.page, g, r)
}

fn text_of(tok: &Tok<'_>, range: (usize, usize)) -> String {
    tok.word.text.get(range.0..range.1).unwrap_or("").to_owned()
}

/// Whether the words after `end` send the mention elsewhere (`in [5]`, `of [Smith]`, `of the
/// appendix of [3]`, `in the book [2]`), or the words before it open a citation (`[5, Theorem 3]`).
fn elsewhere(toks: &[Tok<'_>], start: usize, end: usize) -> bool {
    if let Some(prev) = start.checked_sub(1).and_then(|p| toks.get(p)) {
        let t = prev.word.text.as_str();
        if t.starts_with('[') && !t.contains(']') {
            return true;
        }
    }
    let Some(next) = toks.get(end + 1) else { return false };
    let lead = next.word.text.to_lowercase();
    if !matches!(lead.as_str(), "in" | "of" | "from") {
        return false;
    }
    for k in end + 2..end + 2 + ELSEWHERE_WINDOW {
        let Some(t) = toks.get(k) else { break };
        let w = t.word.text.as_str();
        let lower = w.to_lowercase();
        let bare = lower.trim_matches(|c: char| !c.is_alphanumeric());
        if w.starts_with('[') || ELSEWHERE.contains(&bare) || lower.ends_with("'s") || lower.ends_with("’s") {
            return true;
        }
        if bare == "et" && toks.get(k + 1).is_some_and(|n| n.word.text.starts_with("al")) {
            return true;
        }
        if (w.ends_with('.') && !matches!(bare, "al" | "eq" | "thm" | "sec" | "fig")) || w.ends_with(';') {
            break;
        }
    }
    false
}

/// The words where mentions may be, in reading order: body text, list items, footnotes, captions,
/// without statement heads (a proof's word `Proof` only: what follows it may name a statement) and
/// a caption's own label.
fn tokens<'a>(pages: &'a [PageWords], out: &References) -> Vec<Tok<'a>> {
    let mut skip: HashSet<(usize, usize, usize)> = HashSet::new();
    for s in &out.statements {
        let (p, l) = s.first_line;
        let Some(line) = pages.get(p).and_then(|pw| pw.lines.get(l)) else { continue };
        let Some(head) = parse_head(&line.words) else { continue };
        let n = if s.proof { 1 } else { head.words };
        for w in 0..n {
            skip.insert((p, l, w));
        }
    }
    let mut toks: Vec<Tok<'_>> = Vec::new();
    for (pi, pw) in pages.iter().enumerate() {
        for (li, line) in pw.lines.iter().enumerate() {
            if !matches!(
                line.kind,
                BlockKind::Paragraph | BlockKind::ListItem | BlockKind::Footnote | BlockKind::Caption | BlockKind::Other | BlockKind::Abstract
            ) {
                continue;
            }
            // A caption's own label (`Figure 2:`) names the float it is under, not another.
            let caption_label = line.kind == BlockKind::Caption
                && line.first
                && line.words.first().is_some_and(|w| matches!(w.text.to_lowercase().trim_end_matches('.'), "figure" | "fig" | "table" | "tab"));
            let last = line.words.len().saturating_sub(1);
            for (wi, w) in line.words.iter().enumerate() {
                if skip.contains(&(pi, li, wi)) || w.text.is_empty() || (caption_label && wi < 2) {
                    continue;
                }
                toks.push(Tok { page: line.page, word: w, label: line.kind == BlockKind::ListItem && line.first && wi == 0, line_end: wi == last });
            }
        }
    }
    toks
}

/// Finds the mentions of `pages` and resolves them against the statements already in `out`, the
/// headings of `structure` and the displays and captions of `layouts`.
pub(super) fn find(layouts: &[PageLayout], pages: &[PageWords], structure: &DocumentStructure, out: &mut References) {
    let ix = index(layouts, pages, structure, out);
    let doc_kinds: HashSet<String> = out.statements.iter().filter(|s| !s.proof).map(|s| kind_key(&s.kind)).collect();
    let toks = tokens(pages, out);
    let mut found: Vec<(Found, bool)> = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        if found.len() >= MAX_MENTIONS {
            out.diagnostics.push(Diagnostic {
                code: "mentions_capped",
                message: format!("more than {MAX_MENTIONS} mentions: the rest are left out"),
                block: None,
            });
            break;
        }
        let Some(t) = toks.get(i) else { break };
        let text = t.word.text.as_str();
        // The kind word, with an opening bracket or parenthesis before it left out.
        let lead = text.len() - text.trim_start_matches(['(', '[']).len();
        let core_end = text.trim_end_matches([',', ';', ':', ')']).len().max(lead);
        let core = text.get(lead..core_end).unwrap_or("");
        // `§3`: the word and the number in one.
        let joined = core.strip_prefix('§').filter(|r| number_core(r).is_some_and(|n| n.len() == r.len()));
        let mut word = if joined.is_some() { Some((WordKind::Section, false)) } else { mention_word(core, &doc_kinds) };
        // A kind word broken across lines by a hyphen: `Propo-` `sition`.
        let mut broken: Option<(usize, (usize, usize))> = None;
        if word.is_none()
            && t.line_end
            && let Some(stem) = core.strip_suffix('-').filter(|s| !s.is_empty())
            && let Some(n) = toks.get(i + 1)
        {
            let rest_end = n.word.text.trim_end_matches([',', ';', ':', ')']).len();
            let rest = n.word.text.get(..rest_end).unwrap_or("");
            if rest.chars().all(|c| c.is_alphabetic() || c == '.') && !rest.is_empty() {
                word = mention_word(&format!("{stem}{rest}"), &doc_kinds);
                if word.is_some() {
                    broken = Some((i + 1, (0, rest_end)));
                }
            }
        }
        if let Some((kind, plural)) = word {
            let after_word = broken.map_or(i + 1, |(k, _)| k + 1);
            let (num_tok, num) = if let Some(rest) = joined {
                let from = lead + '§'.len_utf8();
                (Some(i), Some(NumberWord { number: (from, from + rest.len()), extra: None, tail: None }))
            } else {
                let next = toks.get(after_word).and_then(|n| number_word(&n.word.text, &kind));
                (next.as_ref().map(|_| after_word), next)
            };
            if let (Some(ni), Some(num)) = (num_tok, num) {
                let Some(nt) = toks.get(ni) else { break };
                let word_range = if joined.is_some() {
                    (lead, lead + '§'.len_utf8())
                } else if broken.is_some() {
                    (lead, core_end.saturating_sub(1))
                } else {
                    (lead, core_end)
                };
                let mut parts = vec![part(t, word_range)];
                let mut word_text = text_of(t, word_range);
                if let Some((k, r)) = broken
                    && let Some(bt) = toks.get(k)
                {
                    parts.push(part(bt, r));
                    word_text.push_str(&text_of(bt, r));
                }
                parts.push(part(nt, num.number));
                let mut first = Found {
                    kind: kind.clone(),
                    word: word_text,
                    number: text_of(nt, num.number),
                    extra: num.extra.map(|r| text_of(nt, r)),
                    parts,
                    bare: false,
                };
                if joined.is_some() {
                    first.parts = vec![part(t, (word_range.0, num.number.1))];
                }
                let mut list = vec![first];
                let mut end = ni;
                let mut tail = num.tail;
                let mut last_tok = ni;
                // Further numbers of a plural: a range, then `and`, `or` or a comma.
                while plural && list.len() < MAX_LIST {
                    if let Some(r) = tail.take() {
                        if let Some(lt) = toks.get(last_tok) {
                            list.push(Found {
                                kind: kind.clone(),
                                word: String::new(),
                                number: text_of(lt, r),
                                extra: None,
                                parts: vec![part(lt, r)],
                                bare: false,
                            });
                        }
                        continue;
                    }
                    let comma = toks.get(end).is_some_and(|w| w.word.text.ends_with(','));
                    let joiner = toks.get(end + 1).map(|w| w.word.text.as_str());
                    let at = match joiner {
                        Some("and" | "or" | "to") => end + 2,
                        _ if comma => end + 1,
                        _ => break,
                    };
                    let Some(nt) = toks.get(at) else { break };
                    let Some(num) = number_word(&nt.word.text, &kind) else { break };
                    list.push(Found {
                        kind: kind.clone(),
                        word: String::new(),
                        number: text_of(nt, num.number),
                        extra: num.extra.map(|r| text_of(nt, r)),
                        parts: vec![part(nt, num.number)],
                        bare: false,
                    });
                    tail = num.tail;
                    end = at;
                    last_tok = at;
                }
                let away = elsewhere(&toks, i, end);
                found.extend(list.into_iter().map(|f| (f, away)));
                i = end + 1;
                continue;
            }
        }
        // An equation number alone, after a citing word, where a display has it.
        if !t.label && t.word.text.starts_with('(') {
            let num = number_word(text, &WordKind::Equation).filter(|n| n.extra.is_none());
            let cited = i.checked_sub(1).and_then(|p| toks.get(p)).is_some_and(|p| {
                let w = p.word.text.to_lowercase();
                CITING.contains(&w.as_str()) || CITING.contains(&w.trim_end_matches(',').trim_start_matches('('))
            });
            if let Some(num) = num.filter(|_| cited) {
                let key = text_of(t, num.number);
                let inner = key.trim_start_matches('(').trim_end_matches(')').trim().to_owned();
                if ix.equations.contains_key(&inner) {
                    let mut list = vec![Found {
                        kind: WordKind::Equation,
                        word: String::new(),
                        number: key,
                        extra: None,
                        parts: vec![part(t, num.number)],
                        bare: true,
                    }];
                    if let Some(r) = num.tail {
                        list.push(Found {
                            kind: WordKind::Equation,
                            word: String::new(),
                            number: text_of(t, r),
                            extra: None,
                            parts: vec![part(t, r)],
                            bare: true,
                        });
                    }
                    let away = elsewhere(&toks, i, i);
                    found.extend(list.into_iter().map(|f| (f, away)));
                }
            }
        }
        i += 1;
    }
    for (f, away) in found {
        let mention = resolve(f, away, &ix, out);
        out.mentions.push(mention);
    }
}

fn unique<T: Copy>(list: Option<&Vec<T>>) -> Result<T, Unresolved> {
    match list.map(Vec::as_slice) {
        Some([one]) => Ok(*one),
        Some([]) | None => Err(Unresolved::Missing),
        Some(_) => Err(Unresolved::Ambiguous),
    }
}

fn resolve(f: Found, away: bool, ix: &Index, out: &References) -> Mention {
    let inner = f.number.trim_start_matches('(').trim_end_matches(')').trim().to_owned();
    let (result, confidence) = if away {
        (Err(Unresolved::External), 0.0)
    } else {
        match &f.kind {
            WordKind::Statement(key) => (unique(ix.statements.get(&(key.clone(), inner.clone()))).map(Target::Statement), 0.9),
            WordKind::Section => (unique(ix.sections.get(&inner)).map(|(page, block)| Target::Heading { page, block }), 0.9),
            WordKind::Equation => {
                (unique(ix.equations.get(&inner)).map(|(page, block)| Target::Equation { page, block }), if f.bare { 0.75 } else { 0.9 })
            }
            WordKind::Figure => (unique(ix.figures.get(&inner)).map(|(page, block)| Target::Float { page, block }), 0.9),
            WordKind::Table => (unique(ix.tables.get(&inner)).map(|(page, block)| Target::Float { page, block }), 0.9),
        }
    };
    // A statement found twice by its own heads is no target either way; a statement word that
    // names a block found nowhere is no target.
    let statement_kind = match &f.kind {
        WordKind::Statement(key) => out.statements.iter().find(|s| !s.proof && kind_key(&s.kind) == *key).map(|s| s.kind.clone()).or_else(|| {
            key.split_whitespace().last().map(|w| {
                let mut c = w.chars();
                c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
            })
        }),
        _ => None,
    };
    let mut pieces: Vec<Piece> = Vec::new();
    for (page, glyphs, rect) in f.parts {
        if glyphs.is_empty() {
            continue;
        }
        match pieces.last_mut().filter(|p| p.page == page) {
            Some(p) => {
                p.glyphs.extend(glyphs);
                union(&mut p.rect, &rect);
            }
            None => pieces.push(Piece { page, glyphs, rules: Vec::new(), rect }),
        }
    }
    for p in &mut pieces {
        p.rect = finite_or_zero(if p.rect == EMPTY { [0.0; 4] } else { p.rect });
    }
    let (target, unresolved, confidence) = match result {
        Ok(t) => (Some(t), None, confidence),
        Err(u) => (None, Some(u), 0.0),
    };
    Mention { kind: f.kind.mention_kind(), word: f.word, statement_kind, number: f.number, extra: f.extra, pieces, target, unresolved, confidence }
}
