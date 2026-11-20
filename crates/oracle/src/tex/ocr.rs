//! The OCR benchmark's truth: the words TeX set on each page, read from the DVI listing, and the
//! word and character error rates a recogniser's reading scores against them.
//!
//! A benchmark page wraps each paragraph in `\ocrpar{…}` ([`MACROS`]), which ships a
//! `\special{pdfcraft:layout:w}` after every word. The words are then the glyph runs between those
//! specials, in the order TeX shipped them (so a two-column page's truth reads the first column,
//! then the second), and each glyph's character comes from its font's encoding: Computer Modern's
//! OT1 text fonts and the math italic's Greek. An accent glyph combines with the letter shipped
//! after it, as TeX's `\accent` places it (`\'e` ships the acute, then the e). Nothing is read
//! from the PDF.

use super::{DviPage, Mark, TexError};

/// The macros a benchmark document defines before `\begin{document}`: `\ocrpar{words}` sets one
/// paragraph and marks the end of each word. Words are split at spaces, so a word must not hold
/// one (math and accents are fine: `$\alpha\beta$`, `Erd\H{o}s`).
pub const MACROS: &str = r"\def\ocrpar#1{\ocrwords#1 \ocrend\par}
\def\ocrwords#1 #2\ocrend{#1\special{pdfcraft:layout:w}\def\ocrrest{#2}\ifx\ocrrest\empty\else\space\ocrwords#2\ocrend\fi}
";

/// The special [`MACROS`] ship after each word, as [`DviPage::events`] keeps it.
const WORD_END: &str = "w";

/// Most words read from one page.
pub const MAX_WORDS: usize = 20_000;

/// What one glyph contributes to the text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Glyph {
    Text(&'static str),
    Char(char),
    /// An accent, as the combining character it puts on the next letter.
    Accent(char),
    /// A glyph this module does not know: the truth would be wrong, so it is an error.
    Unknown,
}

/// Computer Modern's OT1 text encoding (cmr, cmbx, cmti, cmsl, cmss …), codes 0–127.
fn ot1(code: u32) -> Glyph {
    use Glyph::*;
    const GREEK: [char; 11] = ['Γ', 'Δ', 'Θ', 'Λ', 'Ξ', 'Π', 'Σ', 'Υ', 'Φ', 'Ψ', 'Ω'];
    match code {
        0..=10 => GREEK.get(code as usize).map_or(Unknown, |c| Char(*c)),
        11 => Text("ff"),
        12 => Text("fi"),
        13 => Text("fl"),
        14 => Text("ffi"),
        15 => Text("ffl"),
        16 => Char('ı'),
        17 => Char('ȷ'),
        18 => Accent('\u{300}'),
        19 => Accent('\u{301}'),
        20 => Accent('\u{30C}'),
        21 => Accent('\u{306}'),
        22 => Accent('\u{304}'),
        23 => Accent('\u{30A}'),
        24 => Accent('\u{327}'),
        25 => Char('ß'),
        26 => Char('æ'),
        27 => Char('œ'),
        28 => Char('ø'),
        29 => Char('Æ'),
        30 => Char('Œ'),
        31 => Char('Ø'),
        34 => Char('”'),
        39 => Char('’'),
        60 => Char('¡'),
        62 => Char('¿'),
        92 => Char('“'),
        94 => Accent('\u{302}'),
        95 => Accent('\u{307}'),
        96 => Char('‘'),
        123 => Char('–'),
        124 => Char('—'),
        125 => Accent('\u{30B}'),
        126 => Accent('\u{303}'),
        127 => Accent('\u{308}'),
        33..=122 => char::from_u32(code).map_or(Unknown, Char),
        _ => Unknown,
    }
}

/// Computer Modern math italic (cmmi): lower-case Greek at 11–39, Latin letters and digits where
/// ASCII has them.
fn math_italic(code: u32) -> Glyph {
    use Glyph::*;
    const GREEK: [char; 29] = [
        'α', 'β', 'γ', 'δ', 'ϵ', 'ζ', 'η', 'θ', 'ι', 'κ', 'λ', 'μ', 'ν', 'ξ', 'π', 'ρ', 'σ', 'τ', 'υ', 'ϕ', 'χ', 'ψ', 'ω', 'ε', 'ϑ', 'ϖ', 'ϱ', 'ς',
        'φ',
    ];
    const CAPITALS: [char; 11] = ['Γ', 'Δ', 'Θ', 'Λ', 'Ξ', 'Π', 'Σ', 'Υ', 'Φ', 'Ψ', 'Ω'];
    match code {
        0..=10 => CAPITALS.get(code as usize).map_or(Unknown, |c| Char(*c)),
        11..=39 => GREEK.get(code as usize - 11).map_or(Unknown, |c| Char(*c)),
        48..=57 | 65..=90 | 97..=122 => char::from_u32(code).map_or(Unknown, Char),
        58 => Char('.'),
        59 => Char(','),
        _ => Unknown,
    }
}

/// The glyph `code` of TFM font `font` (`cmr10`, `cmmi7` …).
fn glyph(font: &str, code: u32) -> Glyph {
    let family = font.trim_end_matches(|c: char| c.is_ascii_digit());
    match family {
        "cmmi" | "cmmib" => math_italic(code),
        "cmr" | "cmbx" | "cmti" | "cmsl" | "cmss" | "cmssbx" | "cmbxti" | "cmcsc" | "cmb" | "cmssi" => ot1(code),
        _ => Glyph::Unknown,
    }
}

/// `base` with the combining `accent` on it, precomposed (the benchmark's names and words need
/// these; anything else is reported).
fn compose(base: char, accent: char) -> Option<char> {
    const TABLE: &[(char, char, char)] = &[
        ('a', '\u{301}', 'á'),
        ('e', '\u{301}', 'é'),
        ('ı', '\u{301}', 'í'),
        ('i', '\u{301}', 'í'),
        ('o', '\u{301}', 'ó'),
        ('u', '\u{301}', 'ú'),
        ('y', '\u{301}', 'ý'),
        ('c', '\u{301}', 'ć'),
        ('n', '\u{301}', 'ń'),
        ('s', '\u{301}', 'ś'),
        ('E', '\u{301}', 'É'),
        ('a', '\u{300}', 'à'),
        ('e', '\u{300}', 'è'),
        ('u', '\u{300}', 'ù'),
        ('a', '\u{308}', 'ä'),
        ('e', '\u{308}', 'ë'),
        ('ı', '\u{308}', 'ï'),
        ('o', '\u{308}', 'ö'),
        ('u', '\u{308}', 'ü'),
        ('A', '\u{308}', 'Ä'),
        ('O', '\u{308}', 'Ö'),
        ('U', '\u{308}', 'Ü'),
        ('o', '\u{30B}', 'ő'),
        ('u', '\u{30B}', 'ű'),
        ('a', '\u{302}', 'â'),
        ('e', '\u{302}', 'ê'),
        ('ı', '\u{302}', 'î'),
        ('o', '\u{302}', 'ô'),
        ('c', '\u{30C}', 'č'),
        ('e', '\u{30C}', 'ě'),
        ('r', '\u{30C}', 'ř'),
        ('s', '\u{30C}', 'š'),
        ('z', '\u{30C}', 'ž'),
        ('C', '\u{30C}', 'Č'),
        ('S', '\u{30C}', 'Š'),
        ('c', '\u{327}', 'ç'),
        ('n', '\u{303}', 'ñ'),
        ('a', '\u{303}', 'ã'),
        ('o', '\u{303}', 'õ'),
        ('a', '\u{30A}', 'å'),
        ('u', '\u{30A}', 'ů'),
    ];
    TABLE.iter().find(|(b, a, _)| *b == base && *a == accent).map(|(_, _, c)| *c)
}

/// The words TeX set on `page`, in the order it shipped them: the glyphs between consecutive
/// [`MACROS`] word ends. Glyphs after the last word end (a page number, a heading set outside
/// `\ocrpar`) are not words of the truth. A glyph whose character is unknown, or an accent with
/// nothing to sit on, is an error naming it, so a document the truth cannot spell fails loudly.
pub fn page_words(page: &DviPage) -> Result<Vec<String>, TexError> {
    let mut ends: Vec<usize> = page.events.iter().filter(|(_, e)| e == WORD_END).map(|(n, _)| *n).collect();
    ends.dedup();
    let mut words = Vec::new();
    let mut start = 0usize;
    for end in ends {
        if words.len() >= MAX_WORDS {
            return Err(TexError::Listing(format!("more than {MAX_WORDS} words on one page")));
        }
        let items = page.items.get(start..end).ok_or_else(|| TexError::Listing(format!("word end {end} is past the page's items")))?;
        start = end;
        let mut word = String::new();
        let mut accent: Option<char> = None;
        for (item, _) in items {
            let Mark::Glyph { font, code, .. } = &item.mark else { continue };
            match glyph(font, *code) {
                Glyph::Accent(a) => {
                    if accent.replace(a).is_some() {
                        return Err(TexError::Listing(format!("two accents in a row in {font} (code {code})")));
                    }
                }
                Glyph::Char(c) => match accent.take() {
                    None => word.push(c),
                    Some(a) => {
                        word.push(compose(c, a).ok_or_else(|| TexError::Listing(format!("no precomposed form of {c} with U+{:04X}", a as u32)))?)
                    }
                },
                Glyph::Text(s) if accent.is_none() => word.push_str(s),
                Glyph::Text(s) => return Err(TexError::Listing(format!("an accent on the ligature {s}"))),
                Glyph::Unknown => return Err(TexError::Listing(format!("{font} code {code} has no character in the OCR truth"))),
            }
        }
        if accent.is_some() {
            return Err(TexError::Listing(format!("an accent ends the word {word:?}")));
        }
        if !word.is_empty() {
            words.push(word);
        }
    }
    Ok(words)
}

/// Fold what OCR and typesetting spell differently without the reader caring: curly quotes to
/// straight ones, en and em dashes to hyphens, the ligature-free spellings TeX's glyphs already
/// give, dotless i to i, and the math italic's ϵ/ϕ to ε/φ. Applied to both sides before scoring.
pub fn fold(word: &str) -> String {
    word.chars()
        .map(|c| match c {
            '“' | '”' | '„' => '"',
            '‘' | '’' | '‚' => '\'',
            '–' | '—' | '−' => '-',
            'ı' => 'i',
            'ȷ' => 'j',
            'ϵ' => 'ε',
            'ϕ' => 'φ',
            'ϑ' => 'θ',
            'ϱ' => 'ρ',
            'ϖ' => 'π',
            other => other,
        })
        .collect()
}

/// A reading's text split into words the way [`page_words`] counts them: at whitespace, folded.
pub fn split_words(text: &str) -> Vec<String> {
    text.split_whitespace().map(fold).collect()
}

/// Edits against a reference: an error rate is `edits / len`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Score {
    /// Substitutions, insertions and deletions turning the reading into the truth.
    pub edits: usize,
    /// The truth's length (words or characters).
    pub len: usize,
}

impl Score {
    /// The error rate, `None` for an empty truth.
    pub fn rate(&self) -> Option<f64> {
        (self.len > 0).then(|| self.edits as f64 / self.len as f64)
    }

    /// Two scores pooled (the rate of a set of pages is its total edits over its total length).
    pub fn add(&mut self, other: Score) {
        self.edits = self.edits.saturating_add(other.edits);
        self.len = self.len.saturating_add(other.len);
    }
}

/// Levenshtein distance between two sequences, two rows at a time (O(len(b)) memory).
fn distance<T: PartialEq>(a: &[T], b: &[T]) -> usize {
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut row = vec![0usize; b.len() + 1];
    for (i, x) in a.iter().enumerate() {
        if let Some(first) = row.first_mut() {
            *first = i + 1;
        }
        for (j, y) in b.iter().enumerate() {
            let sub = prev.get(j).copied().unwrap_or(0) + usize::from(x != y);
            let del = prev.get(j + 1).copied().unwrap_or(0) + 1;
            let ins = row.get(j).copied().unwrap_or(0) + 1;
            if let Some(cell) = row.get_mut(j + 1) {
                *cell = sub.min(del).min(ins);
            }
        }
        std::mem::swap(&mut prev, &mut row);
    }
    prev.last().copied().unwrap_or(0)
}

/// Word error rate's counts: word-level edit distance from `read` to `truth` (both folded, in
/// reading order, so a page read in the wrong column order scores badly).
pub fn word_errors(truth: &[String], read: &[String]) -> Score {
    let truth: Vec<String> = truth.iter().map(|w| fold(w)).collect();
    let read: Vec<String> = read.iter().map(|w| fold(w)).collect();
    Score { edits: distance(&truth, &read), len: truth.len() }
}

/// Character error rate's counts: the edit distance between the words joined by single spaces.
pub fn char_errors(truth: &[String], read: &[String]) -> Score {
    let join = |w: &[String]| -> Vec<char> { w.iter().map(|w| fold(w)).collect::<Vec<_>>().join(" ").chars().collect() };
    let (t, r) = (join(truth), join(read));
    Score { edits: distance(&t, &r), len: t.len() }
}

/// The words of `truth` that hold a letter outside ASCII (an accent, Greek), and how many of
/// them `read` has, as a multiset: whether accented words survive, whatever happens to the rest.
pub fn special_words_found(truth: &[String], read: &[String]) -> (usize, usize) {
    let mut pool: Vec<String> = read.iter().map(|w| fold(w)).collect();
    let mut total = 0;
    let mut found = 0;
    for w in truth.iter().map(|w| fold(w)) {
        let bare = w.trim_matches(|c: char| !c.is_alphanumeric());
        if bare.is_ascii() || bare.is_empty() {
            continue;
        }
        total += 1;
        if let Some(i) = pool.iter().position(|r| r.trim_matches(|c: char| !c.is_alphanumeric()) == bare) {
            pool.swap_remove(i);
            found += 1;
        }
    }
    (total, found)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tex::{Item, Owner};

    fn page(glyphs: &[(&str, u32)], ends: &[usize]) -> DviPage {
        let items = glyphs
            .iter()
            .enumerate()
            .map(|(i, (font, code))| {
                (Item { mark: Mark::Glyph { font: (*font).into(), size: 655360, code: *code }, h: i as i64, v: 0 }, Owner::Outside)
            })
            .collect();
        DviPage { items, equations: Vec::new(), events: ends.iter().map(|n| (*n, WORD_END.to_string())).collect() }
    }

    /// Accents combine with the letter shipped after them; ligatures spell out; math italic
    /// Greek is Greek.
    #[test]
    fn words_are_spelled_from_the_glyphs_tex_set() {
        // E r d ˝ o s | H ¨ o l d e r | α λ | ﬁ x
        let p = page(
            &[
                ("cmr10", 69),
                ("cmr10", 114),
                ("cmr10", 100),
                ("cmr10", 125),
                ("cmr10", 111),
                ("cmr10", 115),
                ("cmr10", 72),
                ("cmr10", 127),
                ("cmr10", 111),
                ("cmr10", 108),
                ("cmr10", 100),
                ("cmr10", 101),
                ("cmr10", 114),
                ("cmmi10", 11),
                ("cmmi10", 21),
                ("cmr10", 12),
                ("cmr10", 120),
                ("cmr10", 49),
            ],
            &[6, 13, 15, 17],
        );
        assert_eq!(page_words(&p).unwrap(), ["Erdős", "Hölder", "αλ", "fix"], "the trailing 1 is after the last word end");
    }

    #[test]
    fn unknown_glyphs_and_stray_accents_are_errors_not_guesses() {
        assert!(page_words(&page(&[("cmsy10", 3)], &[1])).is_err());
        assert!(page_words(&page(&[("cmr10", 101), ("cmr10", 19)], &[2])).is_err(), "an accent with no letter after it");
        assert!(page_words(&page(&[("cmr10", 19), ("cmr10", 12)], &[2])).is_err(), "an accent on a ligature");
        assert!(page_words(&page(&[("cmr10", 19), ("cmr10", 120)], &[2])).is_err(), "x has no precomposed acute");
        // A word end past the items is a listing error, not a panic.
        assert!(page_words(&page(&[("cmr10", 101)], &[5])).is_err());
    }

    #[test]
    fn error_rates_count_edits_against_the_truth() {
        let w = |s: &str| s.split_whitespace().map(String::from).collect::<Vec<_>>();
        assert_eq!(word_errors(&w("the Hölder norm"), &w("the Hölder norm")), Score { edits: 0, len: 3 });
        assert_eq!(word_errors(&w("the Hölder norm"), &w("the H?lder norm is")), Score { edits: 2, len: 3 });
        assert_eq!(char_errors(&w("Hölder"), &w("Holder")), Score { edits: 1, len: 6 });
        // Typographic quotes and dashes fold to what OCR writes.
        assert_eq!(word_errors(&w("“first” —"), &w("\"first\" -")).edits, 0);
        assert_eq!(Score { edits: 1, len: 4 }.rate(), Some(0.25));
        assert_eq!(Score::default().rate(), None);
        assert_eq!(special_words_found(&w("Erdős and Rényi, αλ"), &w("Erdos and Rényi αλ")), (3, 2));
    }
}
