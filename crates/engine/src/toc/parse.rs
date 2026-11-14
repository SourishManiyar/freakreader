//! Reading one line of a table of contents: the printed page number at its end (arabic, roman
//! or a label such as `A-3`), the dot leader before it, and the section numbering at its start
//! (`1.2.3`, `A.1`, `IV.`, `Chapter 3`, `Part II`). Also the plain-text outlines a user pastes.
//!
//! Everything here is pure text work, so it is tested with synthetic strings, and every loop is
//! bounded by the length of its input (which callers cap).

/// The layout marks a wide gap between two pieces of a row with this character, so a page number
/// set far to the right of its title counts as one even without a dot leader.
pub(crate) const GAP: char = '\u{2003}';

/// Longest page token read (`MMMDCCCLXXXVIII` is 15; arabic numbers have at most 5 digits).
const MAX_PAGE_TOKEN: usize = 16;

/// Deepest section numbering read (`1.2.3.4.5.6.7.8`).
const MAX_NUMBERING_DEPTH: usize = 8;

/// A printed page number as a table of contents gives it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum PageRef {
    Arabic(u32),
    Roman(u32),
    /// Anything else that names a page, such as `A-3` or `2-14`: only page labels can map it.
    Label(String),
}

impl PageRef {
    /// How the page is printed, for showing it.
    pub fn printed(&self) -> String {
        match self {
            PageRef::Arabic(n) => n.to_string(),
            PageRef::Roman(n) => to_roman(*n),
            PageRef::Label(s) => s.clone(),
        }
    }
}

/// How a section is numbered at the start of its line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NumberKind {
    /// "Part I", "Book 2".
    Part,
    /// "Chapter 3", "Lecture 4", "Appendix A".
    Chapter,
    /// `1`, `1.2`, `1.2.3`.
    Arabic,
    /// `A.`, `A.1`, `B.2.3`.
    Letter,
    /// `IV.`
    Roman,
    /// `§ 2.`, `§3.1`: sections, under chapters.
    Section,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Numbering {
    pub(crate) kind: NumberKind,
    /// How many numbers it has: 1 for `3` or `A.`, 2 for `3.1`.
    pub(crate) depth: usize,
}

/// One line of a table of contents taken apart.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ParsedLine {
    /// The title with the leader and page number taken off (numbering kept, as printed).
    pub(crate) title: String,
    pub(crate) page: Option<PageRef>,
    /// The page number was set apart by a dot leader or a wide gap, not just a space.
    pub(crate) strong: bool,
    pub(crate) numbering: Option<Numbering>,
}

fn is_leader(c: char) -> bool {
    matches!(c, '.' | '·' | '…' | '•' | '_' | '‥' | '\u{2024}' | '\u{2027}' | '\u{22c5}' | '\u{2219}' | '\u{30fb}' | '\u{ff0e}')
}

/// Words after which a number names the section rather than its page ("Chapter 3").
fn names_a_number(word: &str) -> bool {
    const WORDS: [&str; 22] = [
        "chapter", "part", "section", "appendix", "figure", "fig", "table", "lecture", "unit", "book", "volume", "vol", "no", "step", "phase",
        "level", "lesson", "module", "week", "day", "article", "exercise",
    ];
    let w = word.trim_end_matches(['.', ':']).to_lowercase();
    WORDS.contains(&w.as_str())
}

/// The value of a roman numeral written in one case, if it is a well-formed one up to 3999.
pub(crate) fn roman_value(s: &str) -> Option<u32> {
    if s.is_empty() || s.len() > MAX_PAGE_TOKEN {
        return None;
    }
    let lower = s.chars().all(|c| c.is_ascii_lowercase());
    let upper = s.chars().all(|c| c.is_ascii_uppercase());
    if !lower && !upper {
        return None;
    }
    let digit = |c: char| match c.to_ascii_lowercase() {
        'i' => Some(1),
        'v' => Some(5),
        'x' => Some(10),
        'l' => Some(50),
        'c' => Some(100),
        'd' => Some(500),
        'm' => Some(1000),
        _ => None,
    };
    let values: Vec<i64> = s.chars().map(digit).collect::<Option<_>>()?;
    let mut total: i64 = 0;
    for (i, v) in values.iter().enumerate() {
        match values.get(i + 1) {
            Some(next) if next > v => total -= v,
            _ => total += v,
        }
    }
    // Only the canonical spelling: "iiii" or "ic" are words or noise, not numerals.
    let total = u32::try_from(total).ok().filter(|t| *t > 0 && *t < 4000)?;
    to_roman(total).eq_ignore_ascii_case(s).then_some(total)
}

/// `n` as a lower-case roman numeral (`n` from 1 to 3999).
pub(crate) fn to_roman(mut n: u32) -> String {
    const TABLE: [(u32, &str); 13] = [
        (1000, "m"),
        (900, "cm"),
        (500, "d"),
        (400, "cd"),
        (100, "c"),
        (90, "xc"),
        (50, "l"),
        (40, "xl"),
        (10, "x"),
        (9, "ix"),
        (5, "v"),
        (4, "iv"),
        (1, "i"),
    ];
    let mut out = String::new();
    for (value, s) in TABLE {
        while n >= value {
            out.push_str(s);
            n -= value;
        }
    }
    out
}

/// A page token: digits, a roman numeral, or a label like `A-3`, `2-14` or `S12`.
pub(crate) fn page_token(token: &str, roman_ok: bool) -> Option<PageRef> {
    if token.is_empty() || token.chars().count() > MAX_PAGE_TOKEN {
        return None;
    }
    if token.chars().all(|c| c.is_ascii_digit()) {
        if token.len() > 5 {
            return None;
        }
        return token.parse::<u32>().ok().filter(|n| *n > 0).map(PageRef::Arabic);
    }
    if roman_ok && let Some(n) = roman_value(token) {
        return Some(PageRef::Roman(n));
    }
    // A label: letters and digits joined by one hyphen, or a letter prefix and digits, ending
    // in a digit ("A-3", "2-14", "S12"). Words are not labels.
    let ends_digit = token.chars().last().is_some_and(|c| c.is_ascii_digit());
    let digits = token.chars().filter(char::is_ascii_digit).count();
    let letters = token.chars().filter(char::is_ascii_alphabetic).count();
    let hyphens = token.chars().filter(|c| *c == '-' || *c == '–').count();
    let shape = token.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '–');
    (shape && ends_digit && letters <= 3 && hyphens <= 1 && digits >= 1 && token.len() <= 8 && !token.starts_with(['-', '–']))
        .then(|| PageRef::Label(token.replace('–', "-")))
}

/// Section numbering at the start of `title`, if any.
pub(crate) fn numbering(title: &str) -> Option<Numbering> {
    // "§ 2. Title", "§2. Title", "§3.1 Title": the section sign, then a number.
    if let Some(rest) = title.trim_start().strip_prefix('§') {
        let mut words = rest.split_whitespace();
        let number = words.next()?.trim_end_matches('.');
        words.next()?;
        let parts: Vec<&str> = number.split('.').collect();
        let ok = !parts.is_empty()
            && parts.len() <= MAX_NUMBERING_DEPTH
            && parts.iter().all(|p| !p.is_empty() && p.len() <= 3 && p.chars().all(|c| c.is_ascii_digit()));
        return ok.then_some(Numbering { kind: NumberKind::Section, depth: parts.len() });
    }
    let mut words = title.split_whitespace();
    let first = words.next()?;
    let second = words.next();
    let lower = first.trim_end_matches(['.', ':']).to_lowercase();
    // "Part I", "Chapter 3", "Appendix A": the keyword and a number or letter after it.
    let keyword = match lower.as_str() {
        "part" | "book" => Some(NumberKind::Part),
        "chapter" | "lecture" | "appendix" | "unit" | "kapitel" | "chapitre" | "capítulo" | "capitolo" | "teil" => Some(NumberKind::Chapter),
        _ => None,
    };
    if let Some(kind) = keyword {
        let numbered = second.is_some_and(|w| {
            let w = w.trim_end_matches(['.', ':', ')']);
            !w.is_empty()
                && (w.chars().all(|c| c.is_ascii_digit())
                    || roman_value(w).is_some()
                    || (w.chars().count() == 1 && w.chars().all(char::is_alphabetic)))
                || ["one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten"].contains(&w.to_lowercase().as_str())
        });
        return numbered.then_some(Numbering { kind, depth: 1 });
    }
    // Something must follow the number: "1.2" alone is not a numbered title.
    second?;
    let token = first.trim_end_matches(')');
    let (body, dotted) = match token.strip_suffix('.') {
        Some(b) => (b, true),
        None => (token, false),
    };
    if body.is_empty() {
        return None;
    }
    let parts: Vec<&str> = body.split('.').collect();
    if parts.len() > MAX_NUMBERING_DEPTH || parts.iter().any(|p| p.is_empty()) {
        return None;
    }
    let all_digits = |p: &&str| p.len() <= 3 && p.chars().all(|c| c.is_ascii_digit());
    if parts.iter().all(all_digits) {
        // A lone number over 99 is more likely a year or a quantity ("2019 in review").
        let lone_big = parts.len() == 1 && parts.first().and_then(|p| p.parse::<u32>().ok()).is_none_or(|n| n > 99);
        return (!lone_big).then_some(Numbering { kind: NumberKind::Arabic, depth: parts.len() });
    }
    let (head, rest) = parts.split_first()?;
    // "A.", "A.1", "B.2.3" — a bare capital needs its dot or a number after it ("A Study" is a title).
    let one_capital = head.chars().count() == 1 && head.chars().all(|c| c.is_ascii_uppercase());
    if one_capital && (dotted || !rest.is_empty()) && rest.iter().all(all_digits) {
        return Some(Numbering { kind: NumberKind::Letter, depth: parts.len() });
    }
    // "IV." — a roman numeral needs its dot; "I" is a word.
    if rest.is_empty() && dotted && head.chars().all(|c| c.is_ascii_uppercase()) && roman_value(head).is_some() {
        return Some(Numbering { kind: NumberKind::Roman, depth: 1 });
    }
    None
}

/// Text cleaned of control characters, with runs of whitespace as one space.
pub(crate) fn clean(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut space = false;
    for c in text.chars() {
        if c.is_whitespace() || c.is_control() {
            space = true;
        } else {
            if space && !out.is_empty() {
                out.push(' ');
            }
            space = false;
            out.push(c);
        }
    }
    out
}

/// Whether `title` reads as a title: it has letters, and is not mostly replacement characters
/// (a font whose text could not be decoded).
pub(crate) fn readable(title: &str) -> bool {
    let total = title.chars().count();
    let letters = title.chars().filter(|c| c.is_alphabetic()).count();
    let broken = title.chars().filter(|c| *c == '\u{fffd}' || ('\u{e000}'..='\u{f8ff}').contains(c)).count();
    letters > 0 && broken.saturating_mul(4) < total.max(1)
}

/// Take a table-of-contents line apart: the page number at its end and what sets it apart, the
/// numbering at its start. `None` when nothing readable is left for a title.
pub(crate) fn parse_line(raw: &str) -> Option<ParsedLine> {
    let text: String = raw.chars().filter(|c| !c.is_control() || *c == '\t').collect();
    let text = text.trim_end_matches(|c: char| c.is_whitespace() && c != GAP);
    // The token at the end: letters, digits and hyphens.
    let chars: Vec<char> = text.chars().collect();
    let mut end = chars.len();
    while end > 0 && chars.get(end - 1).is_some_and(|c| c.is_whitespace() && *c != GAP) {
        end -= 1;
    }
    let mut start = end;
    while start > 0 && chars.get(start - 1).is_some_and(|c| c.is_alphanumeric() || *c == '-' || *c == '–') && end - start < MAX_PAGE_TOKEN + 1 {
        start -= 1;
    }
    let token: String = chars.get(start..end).unwrap_or_default().iter().collect();
    // What separates it from the title: leaders and spaces.
    let mut before = start;
    let (mut dots, mut gap, mut spaces) = (0usize, false, 0usize);
    while before > 0 {
        let Some(&c) = chars.get(before - 1) else { break };
        if is_leader(c) {
            // An ellipsis is a leader of three dots on its own.
            dots += if matches!(c, '…' | '‥') { 3 } else { 1 };
        } else if c == GAP || c == '\t' {
            gap = true;
        } else if c.is_whitespace() {
            spaces += 1;
        } else {
            break;
        }
        before -= 1;
    }
    let title_part: String = chars.get(..before).unwrap_or_default().iter().collect();
    let title_part = clean(&title_part);
    let separated = dots >= 2 || gap || spaces > 0 || (dots == 1 && spaces > 0);
    let strong = dots >= 2 || gap;
    let last_word = title_part.split_whitespace().last().unwrap_or("");
    // A roman page needs a leader or a gap: "Part I" and "World War II" end in numerals.
    let page = (separated && start > 0 && start < end && readable(&title_part))
        .then(|| page_token(&token, strong))
        .flatten()
        // "Chapter 3" with only a space is the chapter's number, not its page.
        .filter(|p| strong || !(matches!(p, PageRef::Arabic(_)) && names_a_number(last_word)))
        // A label (letters and digits) needs a leader or gap too: "Model T2" is a title.
        .filter(|p| strong || !matches!(p, PageRef::Label(_)));
    let title = if page.is_some() {
        title_part
    } else {
        // No page: the whole line is the title, without a trailing leader.
        clean(text.trim_end_matches(|c: char| is_leader(c) || c.is_whitespace()))
    };
    let title = title.trim_end_matches(|c: char| is_leader(c) || c.is_whitespace()).to_string();
    // A page number on its own (a page's folio) is not a title.
    if !readable(&title) || (page.is_none() && page_token(&title, true).is_some()) {
        return None;
    }
    let numbering = numbering(&title);
    let strong = page.is_some() && strong;
    Some(ParsedLine { title, page, strong, numbering })
}

/// How many page numbers a line ends with, separated by commas (an index line such as
/// "graphs, 12, 45–47, 103" has several; a contents line has one).
pub(crate) fn index_numbers(line: &str) -> usize {
    line.split(',')
        .skip(1)
        .filter(|part| {
            let p = part.trim().trim_end_matches('.');
            let p = p.split(['–', '-']).next().unwrap_or("");
            !p.is_empty() && p.len() <= 5 && p.chars().all(|c| c.is_ascii_digit())
        })
        .count()
}

/// Lower-case letters and digits only, every other character dropped: what titles are matched
/// on, so hyphenation, spacing, punctuation and letter-spaced headings don't matter.
pub(crate) fn compact(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
        }
    }
    out
}

/// `title` without its section numbering ("2.3 Methods" → "Methods").
pub(crate) fn without_numbering(title: &str) -> &str {
    match numbering(title) {
        Some(n) => {
            let spaced_section =
                n.kind == NumberKind::Section && title.trim_start().strip_prefix('§').is_some_and(|r| r.starts_with(char::is_whitespace));
            let skip = if matches!(n.kind, NumberKind::Part | NumberKind::Chapter) || spaced_section { 2 } else { 1 };
            let mut rest = title;
            for _ in 0..skip {
                rest = rest.trim_start();
                rest = rest.find(char::is_whitespace).map_or("", |i| rest.get(i..).unwrap_or(""));
            }
            let rest = rest.trim_start_matches(|c: char| c.is_whitespace() || matches!(c, ':' | '—' | '–' | '-' | '.'));
            if rest.is_empty() { title } else { rest }
        }
        None => title,
    }
}

/// One line of a pasted outline.
#[derive(Clone, Debug, PartialEq)]
pub struct PlainEntry {
    /// 0 for the top level; at most one deeper than the line before.
    pub level: usize,
    pub title: String,
    /// The page number at its end, as written.
    pub page: Option<PageRef>,
}

/// Most lines a pasted outline gives.
pub const MAX_PLAIN_LINES: usize = 10_000;
/// Longest line of a pasted outline read, in characters.
const MAX_PLAIN_LINE_CHARS: usize = 2000;

/// A plain-text outline as entries: one per non-blank line, nested by indentation (a tab counts
/// as four spaces), Markdown heading marks (`#`, `##`) or, when no line is indented, section
/// numbering (`1`, `1.1`). Bullets (`-`, `*`, `•`) are dropped, and a page number at the end of
/// a line (after a space, a tab or a dot leader) is taken as its page.
pub fn parse_outline_text(text: &str) -> Vec<PlainEntry> {
    // (indent, hashes, title, page, numbering depth)
    type Line = (usize, usize, String, Option<PageRef>, Option<usize>);
    let mut lines: Vec<Line> = Vec::new();
    for raw in text.lines().take(MAX_PLAIN_LINES.saturating_mul(4)) {
        if lines.len() >= MAX_PLAIN_LINES {
            break;
        }
        let raw: String = raw.chars().take(MAX_PLAIN_LINE_CHARS).collect();
        let mut indent = 0usize;
        for c in raw.chars() {
            match c {
                '\t' => indent = indent.saturating_add(4),
                ' ' | '\u{a0}' | '\u{3000}' => indent = indent.saturating_add(1),
                _ => break,
            }
        }
        let mut rest = raw.trim();
        if rest.is_empty() {
            continue;
        }
        let hashes = rest.chars().take_while(|c| *c == '#').count();
        if hashes > 0 && rest.chars().nth(hashes).is_some_and(char::is_whitespace) {
            rest = rest.get(hashes..).unwrap_or("").trim_start();
        }
        let hashes = if rest.len() < raw.trim().len() { hashes } else { 0 };
        for bullet in ["- ", "* ", "• ", "+ ", "– ", "◦ ", "▪ "] {
            if let Some(r) = rest.strip_prefix(bullet) {
                rest = r.trim_start();
                break;
            }
        }
        // A tab before the page counts as a gap, like a wide space in a page's layout.
        let line = rest.replace('\t', &GAP.to_string());
        let Some(parsed) = parse_line(&line) else { continue };
        let depth = parsed.numbering.filter(|n| matches!(n.kind, NumberKind::Arabic | NumberKind::Letter)).map(|n| n.depth);
        lines.push((indent, hashes, parsed.title, parsed.page, depth));
    }
    let any_hashes = lines.iter().any(|l| l.1 > 0);
    let any_indent = lines.iter().map(|l| l.0).collect::<std::collections::HashSet<_>>().len() > 1;
    let mut out = Vec::with_capacity(lines.len());
    // The indents of the open levels, outermost first.
    let mut indents: Vec<usize> = Vec::new();
    for (indent, hashes, title, page, depth) in lines {
        let wanted = if any_hashes {
            hashes.saturating_sub(1)
        } else if any_indent {
            while indents.last().is_some_and(|&i| i > indent) {
                indents.pop();
            }
            if indents.last().is_none_or(|&i| i < indent) {
                indents.push(indent);
            }
            indents.len().saturating_sub(1)
        } else {
            depth.map_or(0, |d| d.saturating_sub(1))
        };
        let deepest = out.last().map_or(0, |e: &PlainEntry| e.level.saturating_add(1));
        out.push(PlainEntry { level: wanted.min(deepest), title, page });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(line: &str) -> Option<PageRef> {
        parse_line(line).and_then(|p| p.page)
    }

    #[test]
    fn page_numbers_after_leaders_gaps_and_spaces() {
        assert_eq!(page("1.2 Methods ........ 14"), Some(PageRef::Arabic(14)));
        assert_eq!(page("Introduction.......3"), Some(PageRef::Arabic(3)));
        assert_eq!(page("Preface . . . . . . xi"), Some(PageRef::Roman(11)));
        assert_eq!(page("Preface\u{2003}vii"), Some(PageRef::Roman(7)));
        assert_eq!(page("Results 27"), Some(PageRef::Arabic(27)));
        assert_eq!(page("Appendix tables ···· A-3"), Some(PageRef::Label("A-3".into())));
        assert_eq!(page("Werkzeuge … 2–14"), Some(PageRef::Label("2-14".into())));
        let p = parse_line("2.3 Very long title ..... 45").unwrap();
        assert_eq!((p.title.as_str(), p.strong), ("2.3 Very long title", true));
    }

    #[test]
    fn numbers_that_are_part_of_the_title_are_not_pages() {
        assert_eq!(page("Chapter 3"), None, "the chapter's number");
        assert_eq!(page("Part II"), None, "a roman numeral needs a leader");
        assert_eq!(page("World War II"), None);
        assert_eq!(page("Model T2"), None, "a label needs a leader");
        assert_eq!(page("Section3"), None, "glued to the word");
        assert_eq!(page("pp. 123–145"), None, "a range in a reference");
        assert_eq!(page("Results 000"), None, "page 0");
        assert_eq!(page("Totals 1234567"), None, "too many digits");
        assert_eq!(page("Chapter 3 ........ 41"), Some(PageRef::Arabic(41)), "with a leader it is the page");
        assert_eq!(parse_line("vii"), None, "a page number alone has no title");
        assert_eq!(parse_line("12 ....."), None);
    }

    #[test]
    fn roman_numerals_are_read_only_in_their_canonical_spelling() {
        assert_eq!(roman_value("xiv"), Some(14));
        assert_eq!(roman_value("MCMXCIV"), Some(1994));
        assert_eq!(roman_value("iiii"), None);
        assert_eq!(roman_value("ic"), None);
        assert_eq!(roman_value("Xiv"), None, "one case");
        assert_eq!(roman_value("mix"), Some(1009), "a word that happens to be a numeral");
        assert_eq!(roman_value(&"m".repeat(40)), None);
        assert_eq!(to_roman(3999), "mmmcmxcix");
    }

    #[test]
    fn section_numbering_and_its_depth() {
        let n = |t: &str| numbering(t).map(|n| (n.kind, n.depth));
        assert_eq!(n("1 Introduction"), Some((NumberKind::Arabic, 1)));
        assert_eq!(n("1. Introduction"), Some((NumberKind::Arabic, 1)));
        assert_eq!(n("2.3.1 Data"), Some((NumberKind::Arabic, 3)));
        assert_eq!(n("A.2 Proofs"), Some((NumberKind::Letter, 2)));
        assert_eq!(n("B. Details"), Some((NumberKind::Letter, 1)));
        assert_eq!(n("A Study of Things"), None, "a bare capital is a word");
        assert_eq!(n("IV. RESULTS"), Some((NumberKind::Roman, 1)));
        assert_eq!(n("I think"), None);
        assert_eq!(n("Part II Foundations"), Some((NumberKind::Part, 1)));
        assert_eq!(n("Chapter 12: Tests"), Some((NumberKind::Chapter, 1)));
        assert_eq!(n("Appendix A"), Some((NumberKind::Chapter, 1)));
        assert_eq!(n("Chapters of life"), None);
        assert_eq!(n("2019 in review"), None, "a year");
        assert_eq!(n("1.2"), None, "nothing after the number");
        assert_eq!(n("1..2 x"), None);
        assert_eq!(without_numbering("2.3 Methods"), "Methods");
        assert_eq!(without_numbering("Chapter 3: Results"), "Results");
        assert_eq!(n("§ 2. Ray-Knight Theorems"), Some((NumberKind::Section, 1)));
        assert_eq!(n("§3.1 Bridges"), Some((NumberKind::Section, 2)));
        assert_eq!(n("§ x. y"), None);
        assert_eq!(without_numbering("§ 2. Ray-Knight Theorems"), "Ray-Knight Theorems");
        assert_eq!(without_numbering("§2. Ray-Knight Theorems"), "Ray-Knight Theorems");
        assert_eq!(without_numbering("Overview"), "Overview");
    }

    #[test]
    fn garbled_and_hostile_lines_are_refused_without_panicking() {
        assert_eq!(parse_line("\u{fffd}\u{fffd}\u{fffd}\u{fffd} 12"), None);
        assert_eq!(parse_line(""), None);
        assert_eq!(parse_line("........"), None);
        assert!(parse_line(&"x".repeat(100_000)).is_some());
        assert!(parse_line(&format!("{} 7", "é".repeat(5000))).is_some());
        for s in ["\u{2003}", "-", "––– 3", "a\u{0}b 4", "ⅳ", "١٢", "目次 ……… 3", "\u{202e}abc 5"] {
            let _ = parse_line(s);
        }
        assert_eq!(page("目次 ……… 3"), Some(PageRef::Arabic(3)));
        assert_eq!(index_numbers("graphs, 12, 45–47, 103"), 3);
        assert_eq!(index_numbers("1.2 Methods ..... 14"), 0);
        assert_eq!(compact("Intro-\nduc tion: Ünï"), "introductionünï");
    }

    #[test]
    fn a_pasted_outline_nests_by_indent_hashes_or_numbering() {
        let e = parse_outline_text("Intro\t1\n  Background ... 2\n    Detail 3\n  Aims\nResults 9\n\n");
        let got: Vec<_> = e.iter().map(|e| (e.level, e.title.as_str(), e.page.clone())).collect();
        assert_eq!(
            got,
            [
                (0, "Intro", Some(PageRef::Arabic(1))),
                (1, "Background", Some(PageRef::Arabic(2))),
                (2, "Detail", Some(PageRef::Arabic(3))),
                (1, "Aims", None),
                (0, "Results", Some(PageRef::Arabic(9)))
            ]
        );
        let md = parse_outline_text("# One\n### Too deep\n## Two\n- bullet");
        assert_eq!(md.iter().map(|e| e.level).collect::<Vec<_>>(), [0, 1, 1, 0]);
        let numbered = parse_outline_text("1 Intro\n1.1 Scope\n1.1.1 Terms\n2 Method\nReferences");
        assert_eq!(numbered.iter().map(|e| e.level).collect::<Vec<_>>(), [0, 1, 2, 0, 0]);
        let odd = parse_outline_text("      starts deep\nshallow\n        deep again");
        assert_eq!(odd.iter().map(|e| e.level).collect::<Vec<_>>(), [0, 0, 1]);
        let many = "line\n".repeat(MAX_PLAIN_LINES + 50);
        assert_eq!(parse_outline_text(&many).len(), MAX_PLAIN_LINES);
    }
}
