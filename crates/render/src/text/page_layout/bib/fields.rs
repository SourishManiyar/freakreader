//! The fields of an entry, read from its words and their fonts.
//!
//! Identifiers first, by their shape (a DOI `10.NNNN/…`, an arXiv id, a URL, `MR NNNNNNN`, `Zbl
//! NNNN.NNNNN`); then the year (in parentheses early on: an author-year entry; else the last one);
//! the authors (before the year in an author-year entry, else up to the italic title or the first
//! period that ends a name rather than an initial); the title (an italic or quoted run, else the
//! sentence after the authors); the venue (the italic run after a roman title, else the words after
//! an italic one up to a volume, a year or a comma); volume, number and pages by their shapes
//! (`12(3):45–67`, a bold volume, `no. 3`, `pp. 45–67`). What is not found is left out.

use super::entries::line_label;
use super::lines::BibLine;
use super::text::{composed, is_dash, key, word_flags};
use super::{Author, Entry, Field, Fields, MAX_AUTHORS};
use crate::text::page_layout::refs::{GLYPH_BOLD, GLYPH_ITALIC, GLYPH_MATH, GLYPH_MONO};

/// Most words of an entry read for its fields.
const MAX_TOKENS: usize = 1_500;

#[derive(Clone, Debug)]
struct Tok {
    text: String,
    flags: u8,
    /// Taken by an identifier.
    used: bool,
}

impl Tok {
    fn italic(&self) -> bool {
        self.flags & GLYPH_ITALIC != 0 && self.flags & GLYPH_MATH == 0
    }
    fn bold(&self) -> bool {
        self.flags & GLYPH_BOLD != 0
    }
    /// The text without the punctuation around it.
    fn bare(&self) -> &str {
        self.text.trim_matches(|c: char| matches!(c, ',' | '.' | ';' | ':' | '(' | ')' | '"' | '“' | '”' | '‘' | '’' | '\''))
    }
}

/// The entry's words, the label left out, a URL or DOI broken over lines joined.
fn tokens(lines: &[BibLine<'_>], members: &[usize], e: &Entry) -> Vec<Tok> {
    let mut out: Vec<Tok> = Vec::new();
    for (k, &m) in members.iter().enumerate() {
        let Some(line) = lines.get(m) else { continue };
        let skip = if k == 0 && e.label.is_some() { line_label(line.word_list()).map_or(0, |l| l.words) } else { 0 };
        for (j, w) in line.word_list().enumerate().skip(skip) {
            if out.len() >= MAX_TOKENS {
                return out;
            }
            let text = composed(w);
            let flags = word_flags(w);
            if j == skip
                && k > 0
                && let Some(prev) = out.last_mut()
            {
                let pl = prev.text.as_str();
                let id = pl.contains("://") || pl.contains("10.") && pl.contains('/') || prev.flags & GLYPH_MONO != 0 && flags & GLYPH_MONO != 0;
                let goes_on = text.chars().next().is_some_and(|c| c.is_ascii_digit() || c.is_lowercase() || c == '/');
                if id && (goes_on || flags & GLYPH_MONO != 0) && !pl.ends_with([',', ';']) {
                    prev.text.push_str(&text);
                    continue;
                }
                let before = pl.chars().rev().nth(1);
                let word_break = before.is_some_and(char::is_lowercase) && text.starts_with(char::is_lowercase);
                let range_break = before.is_some_and(|c| c.is_ascii_digit()) && text.starts_with(|c: char| c.is_ascii_digit());
                // `Vasquez-` `Pell`: a name with a hyphen, broken at it.
                let name_break = before.is_some_and(char::is_alphabetic) && text.starts_with(char::is_uppercase);
                if pl.ends_with(is_dash) && (range_break || name_break) {
                    prev.text.push_str(&text);
                    continue;
                }
                if pl.ends_with(is_dash) && word_break {
                    prev.text.pop();
                    prev.text.push_str(&text);
                    continue;
                }
            }
            out.push(Tok { text, flags, used: false });
        }
    }
    out
}

fn field(value: &str, confidence: f32) -> Option<Field> {
    let v = value.trim().trim_end_matches([',', ';', ':']).trim();
    (!v.is_empty()).then(|| Field { value: v.to_owned(), confidence })
}

fn trim_end_punct(s: &str) -> &str {
    s.trim_end_matches(['.', ',', ';', ':', ')']).trim_end()
}

/// A DOI in `t`: `10.` digits `/` something, after `doi:` or a resolver if any.
fn doi_in(t: &str) -> Option<String> {
    let at = t.find("10.")?;
    let rest = t.get(at..)?;
    let slash = rest.find('/')?;
    let prefix = rest.get(3..slash)?;
    if prefix.len() < 4 || !prefix.bytes().all(|b| b.is_ascii_digit() || b == b'.') {
        return None;
    }
    let doi = trim_end_punct(rest);
    (doi.len() > slash + 1).then(|| doi.to_owned())
}

/// An arXiv id: `2101.01234`, `2101.01234v2`, or the old `math/0601001`.
fn arxiv_id(t: &str) -> Option<String> {
    let t = trim_end_punct(t);
    let (a, b) = t.split_once(['.', '/'])?;
    let new = a.len() == 4 && a.bytes().all(|c| c.is_ascii_digit()) && {
        let digits: String = b.chars().take_while(char::is_ascii_digit).collect();
        let rest = b.get(digits.len()..).unwrap_or("");
        (4..=5).contains(&digits.len())
            && (rest.is_empty() || rest.strip_prefix('v').is_some_and(|v| !v.is_empty() && v.bytes().all(|c| c.is_ascii_digit())))
    };
    let old = t.contains('/')
        && a.bytes().all(|c| c.is_ascii_lowercase() || c == b'-' || c == b'.')
        && b.len() == 7
        && b.bytes().all(|c| c.is_ascii_digit());
    (new || old).then(|| t.to_owned())
}

/// Takes the identifiers: DOI, arXiv id, URL, MR and Zbl numbers.
fn identifiers(toks: &mut [Tok], f: &mut Fields) {
    let n = toks.len();
    for i in 0..n {
        let Some(t) = toks.get(i).map(|t| t.text.clone()) else { continue };
        let lower = t.to_lowercase();
        let next = toks.get(i + 1).map(|t| t.text.clone()).unwrap_or_default();
        let take = |toks: &mut [Tok], k: usize| {
            if let Some(x) = toks.get_mut(k) {
                x.used = true;
            }
        };
        if t.contains("://") || lower.starts_with("www.") {
            let url = trim_end_punct(t.trim_start_matches(['(', '<'])).trim_end_matches('>');
            if f.url.is_none() {
                f.url = field(url, 0.9);
            }
            if f.doi.is_none() && lower.contains("doi.org/") {
                f.doi = doi_in(url).and_then(|d| field(&d, 0.9));
            }
            if f.arxiv.is_none()
                && let Some(abs) = lower.find("arxiv.org/abs/")
            {
                f.arxiv = url.get(abs + 14..).and_then(arxiv_id).and_then(|a| field(&a, 0.9));
            }
            take(toks, i);
            continue;
        }
        if f.doi.is_none() && (lower.starts_with("doi") || lower.contains("10.")) {
            let joined = if matches!(lower.trim_end_matches(':'), "doi") { next.clone() } else { t.clone() };
            if let Some(d) = doi_in(&joined) {
                f.doi = field(&d, 0.95);
                take(toks, i);
                if matches!(lower.trim_end_matches(':'), "doi") {
                    take(toks, i + 1);
                }
                continue;
            }
        }
        if f.arxiv.is_none() && lower.starts_with("arxiv") {
            let rest = t.get(5..).unwrap_or("").trim_start_matches([':', ' ']);
            let id = if rest.is_empty() { arxiv_id(&next) } else { arxiv_id(rest) };
            if let Some(a) = id {
                f.arxiv = field(&a, 0.95);
                take(toks, i);
                if rest.is_empty() {
                    take(toks, i + 1);
                }
                continue;
            }
        }
        let number = |s: &str, lo: usize, hi: usize| {
            let s = trim_end_punct(s);
            (lo..=hi).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_digit())
        };
        if f.mr.is_none() && t.starts_with("MR") {
            let rest = t.get(2..).unwrap_or("");
            if number(rest, 5, 8) {
                f.mr = field(trim_end_punct(rest), 0.95);
                take(toks, i);
            } else if rest.is_empty() && number(&next, 5, 8) {
                f.mr = field(trim_end_punct(&next), 0.95);
                take(toks, i);
                take(toks, i + 1);
            }
            continue;
        }
        if f.zbl.is_none() && (t == "Zbl" || t == "Zbl." || t == "zbMATH") {
            let z = trim_end_punct(&next);
            let ok = z
                .split_once('.')
                .is_some_and(|(a, b)| (3..=4).contains(&a.len()) && b.len() == 5 && a.bytes().chain(b.bytes()).all(|c| c.is_ascii_digit()));
            if ok {
                f.zbl = field(z, 0.95);
                take(toks, i);
                take(toks, i + 1);
            }
        }
    }
}

/// A year in `t`: `2020`, `2020a`, `(2020)`, `(2020a).`, from 1000 to 2999; and whether it was in
/// parentheses.
fn year_in(t: &Tok) -> Option<(String, bool)> {
    if t.used || t.flags & GLYPH_MATH != 0 {
        return None;
    }
    let paren = t.text.starts_with('(');
    let s = t.text.trim_start_matches('(').trim_end_matches(['.', ',', ';', ':']).trim_end_matches(')').trim_end_matches(['.', ',']);
    let digits: String = s.chars().take_while(char::is_ascii_digit).collect();
    let rest = s.get(digits.len()..).unwrap_or("");
    let y: u32 = digits.parse().ok().filter(|_| digits.len() == 4)?;
    if !(1000..=2999).contains(&y) || rest.chars().count() > 1 || !rest.chars().all(|c| c.is_ascii_lowercase()) {
        return None;
    }
    Some((s.to_owned(), paren || t.text.ends_with(')') || t.text.ends_with(").")))
}

/// Whether `w` (with its period) is an initial or an abbreviation that does not end a name list:
/// `A.`, `A.-L.`, `Jr.`, `É.`.
fn initial(w: &str) -> bool {
    let core = w.trim_end_matches([',', ';']);
    let Some(stem) = core.strip_suffix('.') else { return false };
    if !stem.chars().any(char::is_alphabetic) || stem.chars().any(|c| c.is_ascii_digit()) {
        return false;
    }
    let letters = stem.chars().filter(|c| c.is_alphabetic()).count();
    letters <= 1 || stem.split(['-', '.']).all(|p| p.chars().filter(|c| c.is_alphabetic()).count() <= 1) || matches!(stem, "Jr" | "Sr" | "St")
}

/// Lower-case words that belong to a family name (`van der Berg`, `de la Cruz`).
pub(super) const PARTICLES: [&str; 12] = ["van", "von", "der", "den", "de", "del", "della", "di", "da", "du", "la", "le"];

/// The authors in `toks` (the author block): names joined by `,` `and` `&`, each `Given Family`
/// or `Family, G.`; `et al.` at the end.
fn authors(toks: &[Tok]) -> (Vec<Author>, bool) {
    // Words without the joiners, cut into parts at commas and `and`.
    let mut parts: Vec<Vec<String>> = vec![Vec::new()];
    let mut et_al = false;
    let mut i = 0;
    while let Some(t) = toks.get(i) {
        let w = t.text.trim_end_matches(';');
        let lw = w.to_lowercase();
        if lw == "et" && toks.get(i + 1).is_some_and(|n| n.text.to_lowercase().starts_with("al")) {
            et_al = true;
            i += 2;
            parts.push(Vec::new());
            continue;
        }
        if matches!(lw.as_str(), "and" | "&" | "and," | "with") {
            parts.push(Vec::new());
            i += 1;
            continue;
        }
        let comma = w.ends_with(',');
        if let Some(p) = parts.last_mut() {
            p.push(w.trim_end_matches(',').to_owned());
        }
        if comma {
            parts.push(Vec::new());
        }
        i += 1;
    }
    parts.retain(|p| !p.is_empty());
    let is_initials = |p: &[String]| p.iter().all(|w| initial(w));
    let mut out = Vec::new();
    let mut k = 0;
    while let Some(p) = parts.get(k) {
        if out.len() >= MAX_AUTHORS {
            break;
        }
        // `Family, G.`: a name part followed by a part of initials.
        if !is_initials(p) && parts.get(k + 1).is_some_and(|n| is_initials(n)) {
            out.push(Author { family: p.join(" "), given: parts.get(k + 1).map(|n| n.join(" ")).unwrap_or_default() });
            k += 2;
            continue;
        }
        if is_initials(p) {
            k += 1;
            continue;
        }
        // `Given von Family`: the family name is the last word with the particles before it.
        let mut at = p.len().saturating_sub(1);
        while at > 1 && p.get(at - 1).is_some_and(|w| PARTICLES.contains(&w.as_str())) {
            at -= 1;
        }
        let family = p.get(at..).unwrap_or_default().join(" ");
        let given = p.get(..at).unwrap_or_default().join(" ");
        if family.chars().next().is_some_and(|c| c.is_alphabetic()) {
            out.push(Author { family, given });
        }
        k += 1;
    }
    (out, et_al)
}

/// Words that open what is not a venue's name.
const NOT_VENUE: [&str; 20] = [
    "in",
    "editor",
    "editors",
    "eds",
    "ed",
    "preprint",
    "phd",
    "ph.d",
    "thesis",
    "master's",
    "technical",
    "tech",
    "report",
    "reports",
    "pages",
    "pp",
    "submitted",
    "doctoral",
    "dissertation",
    "habilitation",
];

/// The fields of entry `e`.
pub(super) fn parse(lines: &[BibLine<'_>], members: &[usize], e: &Entry) -> Fields {
    let mut toks = tokens(lines, members, e);
    let mut f = Fields::default();
    identifiers(&mut toks, &mut f);
    let n = toks.len();
    // The year: one in parentheses near the start (author-year), else the last one.
    let years: Vec<(usize, String, bool)> = toks.iter().enumerate().filter_map(|(i, t)| year_in(t).map(|(y, p)| (i, y, p))).collect();
    let early = years.iter().find(|(i, _, p)| *p && *i <= 40 && *i < n.saturating_sub(1).max(1));
    // Author-year: only names, initials and joiners come before the year in parentheses.
    let name_like = |t: &Tok| {
        let b = t.bare();
        let lower = b.to_lowercase();
        !t.italic()
            && !t.used
            && (b.starts_with(char::is_uppercase)
                || initial(&t.text)
                || matches!(lower.as_str(), "and" | "et" | "al" | "&")
                || PARTICLES.contains(&lower.as_str()))
    };
    let author_year = early.is_some_and(|(i, _, _)| *i > 0 && toks.get(..*i).is_some_and(|before| before.iter().all(name_like)));
    let year = if author_year { early } else { years.iter().rfind(|(_, _, p)| *p).or(years.last()) };
    if let Some((_, y, p)) = year {
        f.year = field(y, if *p { 0.95 } else { 0.85 });
    }
    // The authors.
    let (author_end, after) = if author_year {
        let at = early.map_or(0, |(i, _, _)| *i);
        (at, at + 1)
    } else {
        let mut end = None;
        for (i, t) in toks.iter().enumerate() {
            if t.used {
                break;
            }
            if i > 0 && t.italic() && toks.get(i - 1).is_some_and(|p| p.text.ends_with([',', '.', ':'])) {
                end = Some((i, i));
                break;
            }
            if t.text.ends_with('.') && !initial(&t.text) && t.text.chars().filter(|c| c.is_alphabetic()).count() > 1 {
                let next_capital =
                    toks.get(i + 1).is_none_or(|x| x.text.starts_with(|c: char| c.is_uppercase() || !c.is_alphanumeric()) || x.italic());
                if next_capital {
                    end = Some((i + 1, i + 1));
                    break;
                }
            }
        }
        end.unwrap_or((0, 0))
    };
    let mut block: Vec<Tok> = toks.get(..author_end).unwrap_or_default().to_vec();
    // The period that ended the list is not the last name's, unless that is an initial.
    if let Some(last) = block.last_mut().filter(|l| !initial(&l.text)) {
        last.text = last.text.trim_end_matches('.').to_owned();
    }
    block.retain(|t| !t.text.is_empty());
    if !block.is_empty() && block.len() < n && block.len() <= 4 * MAX_AUTHORS && block.iter().any(|t| t.text.starts_with(char::is_uppercase)) {
        let (list, et_al) = authors(&block);
        if !list.is_empty() {
            f.authors_confidence = if author_year { 0.85 } else { 0.75 };
            f.authors = list;
            f.et_al = et_al;
        }
    }
    // The title.
    let mut at = after;
    if author_year {
        // `(2020).` may be followed by a period of its own.
        while toks.get(at).is_some_and(|t| t.bare().is_empty()) {
            at += 1;
        }
    }
    let (title, title_italic, title_end) = title_from(&toks, at);
    if let Some(t) = title {
        f.title = field(&t, if title_italic { 0.85 } else { 0.7 });
    }
    // The venue.
    f.venue = venue_from(&toks, title_end, title_italic).and_then(|v| field(&v, if title_italic { 0.6 } else { 0.7 }));
    numbers(&toks, title_end, &mut f, year.map(|(i, _, _)| *i));
    f
}

/// The title from token `at`: a quoted or italic run, else the words up to the first period that
/// ends a sentence (or `, YEAR.` or an italic venue). Returns it, whether it was italic, and the
/// token after it.
fn title_from(toks: &[Tok], at: usize) -> (Option<String>, bool, usize) {
    let Some(first) = toks.get(at) else { return (None, false, at) };
    let join = |ts: &[Tok]| ts.iter().map(|t| t.text.as_str()).collect::<Vec<_>>().join(" ");
    let clean = |s: String| {
        let s = s.trim_matches(|c: char| matches!(c, '"' | '“' | '”' | '‘' | '’')).to_owned();
        trim_end_punct(&s).trim_matches(|c: char| matches!(c, '"' | '“' | '”' | '‘' | '’')).to_owned()
    };
    // Quoted.
    if first.text.starts_with(['"', '“', '‘']) {
        let end = toks.iter().skip(at).position(|t| t.text.trim_end_matches([',', '.', ';']).ends_with(['"', '”', '’'])).map(|k| at + k);
        if let Some(e) = end {
            return (Some(clean(join(toks.get(at..=e).unwrap_or_default()))), true, e + 1);
        }
    }
    // Italic: the run of italic words (math inside it stays).
    if first.italic() {
        let mut e = at;
        while toks.get(e + 1).is_some_and(|t| (t.italic() || t.flags & GLYPH_MATH != 0) && !t.used) {
            e += 1;
            if toks.get(e).is_some_and(|t| t.text.ends_with('.') && !initial(&t.text)) {
                break;
            }
        }
        return (Some(clean(join(toks.get(at..=e).unwrap_or_default()))), true, e + 1);
    }
    // Roman: up to a period that ends a sentence, a year after a comma, or an italic word.
    let mut e = at;
    while let Some(t) = toks.get(e) {
        if t.used {
            e = e.saturating_sub(1).max(at);
            break;
        }
        let next = toks.get(e + 1);
        if next.is_some_and(|x| x.italic() && !t.italic()) && e > at {
            break;
        }
        if t.text.ends_with(',') && next.is_some_and(|x| year_in(x).is_some()) {
            break;
        }
        if t.text.ends_with('.') && !initial(&t.text) && next.is_none_or(|x| x.text.starts_with(char::is_uppercase) || x.italic() || x.used) {
            break;
        }
        if next.is_none() {
            break;
        }
        e += 1;
    }
    let ts = toks.get(at..=e.min(toks.len().saturating_sub(1))).unwrap_or_default();
    if ts.is_empty() || ts.len() > 60 {
        return (None, false, at);
    }
    (Some(clean(join(ts))), false, e + 1)
}

/// The venue after the title: the italic run after a roman title (after `In`), else the words up to
/// a bold volume, a number, a year in parentheses or a comma.
fn venue_from(toks: &[Tok], at: usize, title_italic: bool) -> Option<String> {
    let join = |ts: &[Tok]| trim_end_punct(&ts.iter().map(|t| t.text.as_str()).collect::<Vec<_>>().join(" ")).to_owned();
    if !title_italic && let Some(start) = toks.iter().enumerate().skip(at).take(12).find(|(_, t)| t.italic() && !t.used).map(|(i, _)| i) {
        let mut e = start;
        while toks.get(e + 1).is_some_and(|t| t.italic() && !t.used) && !toks.get(e).is_some_and(|t| t.text.ends_with(',')) {
            e += 1;
        }
        let v = join(toks.get(start..=e)?);
        return (!v.is_empty()).then_some(v);
    }
    // After an italic title: skip what names no venue (`preprint,`, `PhD thesis,`, `In`).
    let mut i = at;
    while let Some(t) = toks.get(i) {
        let b = t.bare().to_lowercase();
        let number_after =
            i > at && b.bytes().all(|c| c.is_ascii_digit()) && toks.get(i - 1).is_some_and(|p| NOT_VENUE.contains(&p.bare().to_lowercase().as_str()));
        if NOT_VENUE.contains(&b.as_str()) || b.is_empty() || number_after {
            i += 1;
            continue;
        }
        break;
    }
    let mut e = i;
    while let Some(t) = toks.get(e) {
        let stop = t.used || t.bold() || t.text.starts_with('(') || t.text.starts_with(|c: char| c.is_ascii_digit()) || year_in(t).is_some();
        if stop {
            break;
        }
        e += 1;
        if t.text.ends_with([',', ';']) || t.text.ends_with('.') && !initial(&t.text) && t.bare().chars().count() > 4 {
            break;
        }
    }
    let ts = toks.get(i..e)?;
    if ts.is_empty() || ts.len() > 20 || !ts.first().is_some_and(|t| t.text.starts_with(char::is_uppercase)) {
        return None;
    }
    Some(join(ts))
}

/// A page range: `45–67`, `45--67` or `45-67`, digits only on both sides.
fn page_range(s: &str) -> Option<String> {
    let s = trim_end_punct(s.trim_start_matches(['(', ':']));
    let (a, b) = s.split_once(is_dash)?;
    let b = b.trim_start_matches(is_dash);
    let ok = |x: &str| !x.is_empty() && x.len() <= 7 && x.bytes().all(|c| c.is_ascii_digit());
    (ok(a) && ok(b)).then(|| format!("{a}–{b}"))
}

/// Volume, number and pages: `12(3):45–67`, `12:45–67`, a bold volume, `no. 3`, `pp. 45–67`, a
/// page range alone.
fn numbers(toks: &[Tok], from: usize, f: &mut Fields, year_at: Option<usize>) {
    for (i, t) in toks.iter().enumerate().skip(from) {
        if t.used || Some(i) == year_at {
            continue;
        }
        let s = t.text.trim_end_matches(['.', ',', ';']);
        // `V(N):P–Q`, `V:P–Q`, `V(N)`.
        let vol: String = s.chars().take_while(char::is_ascii_digit).collect();
        if !vol.is_empty() && vol.len() <= 5 && f.volume.is_none() && f.pages.is_none() {
            let rest = s.get(vol.len()..).unwrap_or("");
            let (num, rest) = match rest.strip_prefix('(').and_then(|r| r.split_once(')')) {
                Some((n, r)) if !n.is_empty() && n.len() <= 6 => (Some(n), r),
                _ => (None, rest),
            };
            // `68:` with the pages in the next word.
            let pages_next = rest.strip_prefix(':').filter(|p| p.is_empty()).and_then(|_| toks.get(i + 1)).and_then(|x| page_range(&x.text));
            if let Some(p) = pages_next {
                f.volume = field(&vol, 0.8);
                f.number = num.and_then(|n| field(n, 0.8));
                f.pages = field(&p, 0.8);
                continue;
            }
            if let Some(pages) = rest.strip_prefix(':') {
                f.volume = field(&vol, 0.8);
                f.number = num.and_then(|n| field(n, 0.8));
                f.pages = page_range(pages).and_then(|p| field(&p, 0.8)).or_else(|| field(pages, 0.6));
                continue;
            }
            if num.is_some() && rest.is_empty() {
                f.volume = field(&vol, 0.75);
                f.number = num.and_then(|n| field(n, 0.75));
                continue;
            }
        }
        if t.bold() && f.volume.is_none() && !vol.is_empty() && vol.len() == trim_end_punct(s).len() {
            f.volume = field(&vol, 0.8);
            continue;
        }
        let lower = t.bare().to_lowercase();
        if matches!(lower.as_str(), "no" | "nr" | "issue" | "report") && f.number.is_none() {
            if let Some(n) = toks
                .get(i + 1)
                .map(|x| trim_end_punct(&x.text).to_owned())
                .filter(|n| !n.is_empty() && n.len() <= 6 && n.bytes().all(|b| b.is_ascii_digit()))
            {
                f.number = field(&n, 0.75);
            }
            continue;
        }
        if matches!(lower.as_str(), "vol" | "volume") && f.volume.is_none() {
            if let Some(v) = toks
                .get(i + 1)
                .map(|x| trim_end_punct(&x.text).to_owned())
                .filter(|n| !n.is_empty() && n.len() <= 5 && n.bytes().all(|b| b.is_ascii_digit()))
            {
                f.volume = field(&v, 0.7);
            }
            continue;
        }
        if f.pages.is_none()
            && let Some(p) = page_range(&t.text)
        {
            f.pages = field(&p, 0.75);
        }
    }
    // A key read so far that holds no letter or digit is no field.
    for slot in [&mut f.title, &mut f.venue] {
        if slot.as_ref().is_some_and(|x| key(&x.value).is_empty()) {
            *slot = None;
        }
    }
}
