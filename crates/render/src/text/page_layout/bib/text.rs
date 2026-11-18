//! Words as the bibliography reads them: text with TeX's separate accent glyphs put on their letters,
//! and each word's font style.

use crate::text::page_layout::refs::{GLYPH_BOLD, GLYPH_ITALIC, GLYPH_MATH, GLYPH_MONO, Word};

/// The combining mark a spacing accent glyph stands for.
fn mark_of(c: char) -> Option<char> {
    Some(match c {
        '´' | '\u{2CA}' => '\u{301}',
        '`' | '\u{2CB}' => '\u{300}',
        '¨' => '\u{308}',
        'ˆ' | '^' => '\u{302}',
        '˜' | '~' => '\u{303}',
        '¸' => '\u{327}',
        'ˇ' => '\u{30C}',
        '˘' => '\u{306}',
        '˙' => '\u{307}',
        '¯' => '\u{304}',
        '˝' => '\u{30B}',
        '˚' => '\u{30A}',
        '˛' => '\u{328}',
        _ => return None,
    })
}

/// `base` with `mark` as one character, for the letters Latin accents make; `None` when there is no
/// such character (the pair is then written as the letter and the combining mark).
fn precompose(base: char, mark: char) -> Option<char> {
    // Rows: a mark, the letters it takes, the letters it makes (one to one).
    const TABLE: [(char, &str, &str); 12] = [
        ('\u{301}', "aeiouyAEIOUYcnszrlgCNSZRLG", "áéíóúýÁÉÍÓÚÝćńśźŕĺǵĆŃŚŹŔĹǴ"),
        ('\u{300}', "aeiouAEIOUn", "àèìòùÀÈÌÒÙǹ"),
        ('\u{302}', "aeiouAEIOUcgshjwyCGSHJWY", "âêîôûÂÊÎÔÛĉĝŝĥĵŵŷĈĜŜĤĴŴŶ"),
        ('\u{308}', "aeiouyAEIOUY", "äëïöüÿÄËÏÖÜŸ"),
        ('\u{303}', "anoAINOUiu", "ãñõÃĨÑÕŨĩũ"),
        ('\u{327}', "cstgklnrCSTGKLNR", "çşţģķļņŗÇŞŢĢĶĻŅŖ"),
        ('\u{30C}', "cdenrstzCDENRSTZ", "čďěňřšťžČĎĚŇŘŠŤŽ"),
        ('\u{306}', "agouAGOU", "ăğŏŭĂĞŎŬ"),
        ('\u{307}', "czegCZEGI", "ċżėġĊŻĖĠİ"),
        ('\u{304}', "aeiouAEIOU", "āēīōūĀĒĪŌŪ"),
        ('\u{30B}', "ouOU", "őűŐŰ"),
        ('\u{30A}', "auAU", "åůÅŮ"),
    ];
    // A dotless i takes the accent as an i does.
    let base = match base {
        'ı' => 'i',
        'ȷ' => 'j',
        b => b,
    };
    let (_, from, to) = TABLE.iter().find(|(m, _, _)| *m == mark)?;
    let at = from.chars().position(|c| c == base)?;
    to.chars().nth(at)
}

/// The text of `word` with each spacing accent glyph put on the letter under it (the letter whose
/// box holds the accent's middle, else the nearer one): TeX sets `\"u` as a `u` and a `¨` of its
/// own, in either order.
pub(super) fn composed(word: &Word) -> String {
    let n = word.glyphs.len();
    // Each glyph's text, from its start to the next one's.
    let piece = |i: usize| -> &str {
        let from = word.starts.get(i).copied().unwrap_or(word.text.len());
        let to = word.starts.get(i + 1).copied().unwrap_or(word.text.len());
        word.text.get(from..to.max(from)).unwrap_or("")
    };
    if n == 0 || word.starts.len() != n || word.spans.len() != n {
        return word.text.clone();
    }
    // For each letter glyph, the marks that go on it.
    let mut marks: Vec<Vec<char>> = vec![Vec::new(); n];
    let mut is_mark = vec![false; n];
    for i in 0..n {
        let mut chars = piece(i).chars();
        let (Some(c), None) = (chars.next(), chars.next()) else { continue };
        let Some(mark) = mark_of(c) else { continue };
        let Some(span) = word.spans.get(i) else { continue };
        let mid = (span[0] + span[1]) / 2.0;
        let letter = |k: usize| piece(k).chars().next().is_some_and(char::is_alphabetic);
        let dist =
            |k: usize| word.spans.get(k).map_or(f32::INFINITY, |s| if mid >= s[0] && mid <= s[1] { 0.0 } else { (mid - (s[0] + s[1]) / 2.0).abs() });
        let prev = i.checked_sub(1).filter(|&k| letter(k));
        let next = Some(i + 1).filter(|&k| k < n && letter(k));
        let to = match (prev, next) {
            (Some(p), Some(q)) => Some(if dist(q) < dist(p) { q } else { p }),
            (p, q) => p.or(q),
        };
        if let Some(t) = to.and_then(|t| marks.get_mut(t)) {
            t.push(mark);
            if let Some(m) = is_mark.get_mut(i) {
                *m = true;
            }
        }
    }
    let mut out = String::with_capacity(word.text.len());
    for i in 0..n {
        if is_mark.get(i).copied().unwrap_or(false) {
            continue;
        }
        let text = piece(i);
        let ms = marks.get(i).map(Vec::as_slice).unwrap_or(&[]);
        let mut cs = text.chars();
        match (cs.next(), ms) {
            (Some(c), [m]) if cs.clone().next().is_none() => match precompose(c, *m) {
                Some(p) => out.push(p),
                None => {
                    out.push(c);
                    out.push(*m);
                }
            },
            _ => {
                out.push_str(text);
                out.extend(ms.iter());
            }
        }
    }
    out
}

/// The flags most of a word's letters have, and any glyph's mathematics or typewriter flag.
pub(super) fn word_flags(word: &Word) -> u8 {
    // Letters and digits decide: the period after an italic title is set upright.
    let alnum: Vec<bool> = (0..word.flags.len())
        .map(|i| {
            let from = word.starts.get(i).copied().unwrap_or(0);
            word.text.get(from..).and_then(|r| r.chars().next()).is_some_and(char::is_alphanumeric)
        })
        .collect();
    let letters = alnum.iter().filter(|a| **a).count();
    let n = if letters > 0 { letters } else { word.flags.len().max(1) };
    let count = |f: u8| word.flags.iter().zip(&alnum).filter(|(x, a)| **x & f != 0 && (**a || letters == 0)).count();
    let mut out = 0;
    if count(GLYPH_ITALIC) * 2 > n {
        out |= GLYPH_ITALIC;
    }
    if count(GLYPH_BOLD) * 2 > n {
        out |= GLYPH_BOLD;
    }
    if count(GLYPH_MONO) * 2 > n {
        out |= GLYPH_MONO;
    }
    if word.flags.iter().any(|x| x & GLYPH_MATH != 0) {
        out |= GLYPH_MATH;
    }
    out
}

/// A family name, title or label compared loosely: letters and digits only, accents dropped, lower
/// case.
pub(super) fn key(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if let Some(b) = base_letter(c) {
            out.push_str(b);
        } else if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
        }
    }
    out
}

/// The base letters of an accented Latin letter (and the ligatures).
fn base_letter(c: char) -> Option<&'static str> {
    Some(match c {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' | 'À' | 'Á' | 'Â' | 'Ã' | 'Ä' | 'Å' | 'Ā' | 'Ă' => "a",
        'ç' | 'ć' | 'č' | 'ĉ' | 'ċ' | 'Ç' | 'Ć' | 'Č' | 'Ĉ' | 'Ċ' => "c",
        'ď' | 'đ' | 'Ď' | 'Đ' => "d",
        'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ė' | 'ę' | 'ě' | 'È' | 'É' | 'Ê' | 'Ë' | 'Ē' | 'Ė' | 'Ě' => "e",
        'ğ' | 'ĝ' | 'ġ' | 'ģ' | 'ǵ' | 'Ğ' | 'Ĝ' | 'Ġ' | 'Ģ' | 'Ǵ' => "g",
        'ì' | 'í' | 'î' | 'ï' | 'ı' | 'ĩ' | 'ī' | 'Ì' | 'Í' | 'Î' | 'Ï' | 'İ' | 'Ĩ' | 'Ī' => "i",
        'ł' | 'ľ' | 'ĺ' | 'ļ' | 'Ł' | 'Ľ' | 'Ĺ' | 'Ļ' => "l",
        'ñ' | 'ń' | 'ň' | 'ņ' | 'ǹ' | 'Ñ' | 'Ń' | 'Ň' | 'Ņ' => "n",
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ő' | 'Ò' | 'Ó' | 'Ô' | 'Õ' | 'Ö' | 'Ø' | 'Ō' | 'Ő' => "o",
        'ŕ' | 'ř' | 'ŗ' | 'Ŕ' | 'Ř' | 'Ŗ' => "r",
        'ś' | 'š' | 'ş' | 'ŝ' | 'Ś' | 'Š' | 'Ş' | 'Ŝ' => "s",
        'ß' => "ss",
        'ţ' | 'ť' | 'Ţ' | 'Ť' => "t",
        'ù' | 'ú' | 'û' | 'ü' | 'ũ' | 'ū' | 'ů' | 'ű' | 'Ù' | 'Ú' | 'Û' | 'Ü' | 'Ũ' | 'Ū' | 'Ů' | 'Ű' => "u",
        'ý' | 'ÿ' | 'Ý' | 'Ÿ' => "y",
        'ź' | 'ż' | 'ž' | 'Ź' | 'Ż' | 'Ž' => "z",
        'æ' | 'Æ' => "ae",
        'œ' | 'Œ' => "oe",
        'ﬁ' => "fi",
        'ﬂ' => "fl",
        'ﬀ' => "ff",
        'ﬃ' => "ffi",
        'ﬄ' => "ffl",
        _ => return None,
    })
}

/// A dash of any kind: `-`, `–`, `—`, `‐`, `‑`, `−`.
pub(super) fn is_dash(c: char) -> bool {
    matches!(c, '-' | '–' | '—' | '‐' | '‑' | '−')
}
