//! The Unicode path: what a glyph of a font the symbol table does not know is, read from its Unicode
//! text.
//!
//! The symbol table knows Computer Modern and the AMS fonts by font and character code. A formula set
//! in an OpenType math font (STIX Two, TeX Gyre, Latin Modern Math, Fira Math, Libertinus Math,
//! Cambria Math, with `unicode-math`) or in a Times-style package (newtx, mathptmx, txfonts) uses
//! fonts it has never seen; their glyphs carry Unicode text instead (`ToUnicode`, glyph names). This
//! module turns that text back into symbols:
//!
//! - **symbols** by inverting the table's own Unicode column (no other data: every command named here
//!   is one the table generator typeset), preferring the plain size of a symbol that has several and,
//!   where the table has several commands for one character, the first by family and noting the others
//!   ([`UnicodeSymbol::others`]);
//! - **Mathematical Alphanumeric Symbols** (U+1D400–1D7FF) and the letter-like symbols that fill their
//!   holes, by the alphabet the block is: math italic letters and Greek, `\mathbb`, `\mathfrak`,
//!   `\mathcal`, and `unicode-math`'s `\symbf`, `\symbfit`, `\symsf`, `\symtt`;
//! - **plain letters and digits** (U+0041…), whose alphabet the character does not say: it comes from
//!   the font's style ([`FontRole`], read from its name) and from what the page shows ([`PageKind`]).
//!   A letter of a font whose style the name does not tell (a symbol font whose `R` is a blackboard R)
//!   is not guessed at.
//!
//! What only the glyph's size settles (a large operator in text or display size, a delimiter grown
//! past its plain size) the reader decides from the glyph's ink; nothing here has the font's metrics.

use super::symbols::{Alphabet, Class, Kind, Role, Symbol, SymbolTable, table};
use std::collections::HashMap;
use std::sync::OnceLock;

/// A symbol the Unicode path knows, with what it cannot tell apart.
#[derive(Debug)]
pub struct UnicodeSymbol {
    pub symbol: Symbol,
    /// The display size of a large operator (rank 1 of its own size family).
    pub display: Option<Symbol>,
    /// A delimiter grown past its plain size (rank 1 of its size family, no `\big` size known).
    pub grown: Option<Symbol>,
    /// Other commands the table has for the same character (`\varnothing` beside `\emptyset`): the
    /// glyph does not say which was meant.
    pub others: Vec<String>,
}

/// The style of a font, as its PDF name says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FontRole {
    /// An OpenType math font (`STIXTwoMath-Regular`, `LatinModernMath-Regular`): its letters are
    /// Mathematical Alphanumeric Symbols; a plain letter in it is an upright letter of the math font.
    Math,
    TextRoman,
    TextBold,
    TextItalic,
    TextBoldItalic,
    /// A name that says no style (`NewTXMI`, `txsys`, `rsfs10`, `StandardSymL`).
    Other,
}

/// The name without subset prefix and XeTeX's `-Identity-H`, lower case.
fn plain_name(name: &str) -> String {
    let name = match name.split_once('+') {
        Some((prefix, rest)) if prefix.len() == 6 && prefix.bytes().all(|b| b.is_ascii_uppercase()) => rest,
        _ => name,
    };
    let lower = name.to_ascii_lowercase();
    lower.strip_suffix("-identity-h").map(str::to_owned).unwrap_or(lower)
}

/// The role of a font by its name.
pub fn font_role(name: &str) -> FontRole {
    let n = plain_name(name);
    if n.contains("math") {
        return FontRole::Math;
    }
    // The style is the part after the last hyphen: `NimbusRomNo9L-ReguItal`, `STIXTwoText-BoldItalic`.
    let Some((_, style)) = n.rsplit_once('-') else { return FontRole::Other };
    let italic = ["italic", "ital", "oblique"].iter().any(|s| style.contains(s)) || style == "it" || style.ends_with("it");
    let bold = ["bold", "medi", "black", "heavy", "demi"].iter().any(|s| style.contains(s)) || style == "bd";
    let regular = ["regular", "regu", "roman", "book", "normal", "text"].iter().any(|s| style.contains(s));
    match (bold, italic) {
        (true, true) => FontRole::TextBoldItalic,
        (true, false) => FontRole::TextBold,
        (false, true) => FontRole::TextItalic,
        (false, false) if regular => FontRole::TextRoman,
        _ => FontRole::Other,
    }
}

/// What the page as a whole shows about how its formulas are set.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PageKind {
    /// A font of the page is an OpenType math font.
    pub math_font: bool,
    /// Some glyph of the page is a math italic letter by its Unicode text (U+1D434…): math italic is
    /// not the text italic font, so a letter of that font is `\mathit`.
    pub math_italic_text: bool,
}

/// Whether `c` is a Mathematical Alphanumeric italic letter (Latin or Greek, plain italic or bold
/// italic), or the italic h (U+210E) that fills that block's hole.
pub fn is_math_italic(c: char) -> bool {
    matches!(c as u32, 0x1D434..=0x1D467 | 0x1D6E2..=0x1D71B | 0x210E)
}

/// The answer for one glyph.
#[derive(Debug)]
pub enum Reading {
    Known {
        symbol: &'static UnicodeSymbol,
        /// Why the reading is not certain, when it is not.
        doubt: Option<String>,
    },
    Unknown(String),
}

#[derive(Default)]
struct Index {
    /// By Unicode text, for every character whose reading does not depend on the font.
    by_text: HashMap<String, UnicodeSymbol>,
    /// Plain letters and digits by role: `\mathrm{a}`, `a` (math italic), `\mathit{a}`, `\mathbf{a}`.
    roman: HashMap<char, UnicodeSymbol>,
    math_italic: HashMap<char, UnicodeSymbol>,
    text_italic: HashMap<char, UnicodeSymbol>,
    bold: HashMap<char, UnicodeSymbol>,
    digit: HashMap<char, UnicodeSymbol>,
    digit_roman: HashMap<char, UnicodeSymbol>,
}

fn plain(symbol: Symbol) -> UnicodeSymbol {
    UnicodeSymbol { symbol: unmeasured(symbol), display: None, grown: None, others: Vec::new() }
}

/// A table symbol read from another font: the table's metrics are Computer Modern's, not that font's,
/// so it has none.
fn unmeasured(symbol: Symbol) -> Symbol {
    Symbol { width: 0, italic: 0, height: 0, depth: 0, from_unicode: true, ..symbol }
}

/// A symbol made here (an alphabet the table has no font for), with no metrics.
fn made(latex: String, unicode: String, kind: Kind, alphabet: Option<Alphabet>) -> Symbol {
    Symbol {
        latex,
        unicode,
        class: Class::Ord,
        kind,
        alphabet,
        variants: Vec::new(),
        pieces: Vec::new(),
        big: None,
        accent: false,
        width: 0,
        italic: 0,
        height: 0,
        depth: 0,
        from_unicode: true,
    }
}

/// The table's preference among families for one character: Computer Modern before the AMS fonts.
fn family_rank(f: &str) -> usize {
    ["cmmi", "cmr", "cmsy", "cmex", "msam", "msbm", "eufm", "cmbx", "cmti"].iter().position(|x| *x == f).unwrap_or(99)
}

/// Spacing accents and other forms text fonts use, as the character the table has.
const ALIASES: [(char, char); 14] = [
    // The table has `|` and `\\|` only as `\\mid` and `\\parallel` (one glyph, two commands, which the
    // reader tells apart by the glue), and `\\colon` only as `:`.
    ('\u{007C}', '\u{2223}'),
    ('\u{2016}', '\u{2225}'),
    ('\u{2236}', '\u{003A}'),
    ('\u{02C6}', '\u{0302}'),
    ('\u{02DC}', '\u{0303}'),
    ('\u{00AF}', '\u{0304}'),
    ('\u{02C9}', '\u{0304}'),
    ('\u{02C7}', '\u{030C}'),
    ('\u{02D8}', '\u{0306}'),
    ('\u{02D9}', '\u{0307}'),
    ('\u{00A8}', '\u{0308}'),
    ('\u{00B4}', '\u{0301}'),
    ('\u{02DA}', '\u{030A}'),
    ('\u{00B7}', '\u{22C5}'),
];

/// Pieces of extensible delimiters (Miscellaneous Technical): the delimiters and roles each is.
const PIECES: [(char, &[(&str, Role)]); 19] = [
    ('\u{239B}', &[("(", Role::Top)]),
    ('\u{239C}', &[("(", Role::Rep)]),
    ('\u{239D}', &[("(", Role::Bot)]),
    ('\u{239E}', &[(")", Role::Top)]),
    ('\u{239F}', &[(")", Role::Rep)]),
    ('\u{23A0}', &[(")", Role::Bot)]),
    ('\u{23A1}', &[("[", Role::Top)]),
    ('\u{23A2}', &[("[", Role::Rep)]),
    ('\u{23A3}', &[("[", Role::Bot)]),
    ('\u{23A4}', &[("]", Role::Top)]),
    ('\u{23A5}', &[("]", Role::Rep)]),
    ('\u{23A6}', &[("]", Role::Bot)]),
    ('\u{23A7}', &[("\\{", Role::Top)]),
    ('\u{23A8}', &[("\\{", Role::Mid)]),
    ('\u{23A9}', &[("\\{", Role::Bot)]),
    ('\u{23AA}', &[("\\{", Role::Rep), ("\\}", Role::Rep)]),
    ('\u{23AB}', &[("\\}", Role::Top)]),
    ('\u{23AC}', &[("\\}", Role::Mid)]),
    ('\u{23AD}', &[("\\}", Role::Bot)]),
];

impl Index {
    fn build(t: &SymbolTable) -> Index {
        let mut ix = Index::default();
        // Every single-glyph symbol of the table with Unicode text, at its plain size, grouped by text.
        let mut rows: Vec<(&str, &Symbol)> = t
            .symbols()
            .filter(|(_, _, s)| !s.unicode.is_empty() && s.big.is_none() && s.pieces.is_empty() && s.variants.iter().all(|(_, r)| *r == 0))
            .map(|(f, _, s)| (f, s))
            .collect();
        rows.sort_by(|a, b| family_rank(a.0).cmp(&family_rank(b.0)).then_with(|| a.1.latex.cmp(&b.1.latex)));
        let mut by_text: HashMap<String, Vec<&Symbol>> = HashMap::new();
        for (family, s) in &rows {
            // Letters in an alphabet other than math italic are read from their Unicode block below;
            // the table's text italic and bold rows serve the plain letters of text fonts.
            match s.alphabet {
                Some(Alphabet::Italic) => {
                    if let Some(c) = styled_char(s) {
                        ix.text_italic.entry(c).or_insert_with(|| plain(Symbol { from_unicode: true, ..(*s).clone() }));
                    }
                    continue;
                }
                Some(Alphabet::Bold) => {
                    if let Some(c) = styled_char(s) {
                        ix.bold.entry(c).or_insert_with(|| plain(Symbol { from_unicode: true, ..(*s).clone() }));
                    }
                    continue;
                }
                Some(Alphabet::Roman) => {
                    if let Some(c) = styled_char(s) {
                        ix.roman.entry(c).or_insert_with(|| plain(Symbol { from_unicode: true, ..(*s).clone() }));
                    }
                    continue;
                }
                _ => {}
            }
            if *family == "cmr" && s.kind == Kind::Digit {
                if let Some(c) = s.latex.chars().next() {
                    ix.digit.entry(c).or_insert_with(|| plain(Symbol { from_unicode: true, ..(*s).clone() }));
                    let roman = made(format!("\\mathrm{{{c}}}"), c.to_string(), Kind::Digit, Some(Alphabet::Roman));
                    ix.digit_roman.entry(c).or_insert_with(|| plain(roman));
                }
                continue;
            }
            if s.alphabet == Some(Alphabet::Math)
                && let Some(c) = s.latex.chars().next().filter(char::is_ascii_alphabetic)
            {
                ix.math_italic.entry(c).or_insert_with(|| plain(Symbol { from_unicode: true, ..(*s).clone() }));
            }
            by_text.entry(s.unicode.clone()).or_default().push(s);
        }
        for (text, list) in by_text {
            // A `\var` form is the second choice: `\Gamma` and `\varGamma` are both U+0393 in the table, and
            // the upright one is the character.
            let Some(first) = list.iter().find(|s| !s.latex.starts_with("\\var")).or_else(|| list.first()) else { continue };
            let mut others: Vec<String> = list.iter().map(|s| s.latex.clone()).filter(|l| *l != first.latex).collect();
            others.sort();
            others.dedup();
            if first.kind == Kind::Greek {
                others.retain(|o| !o.starts_with("\\var"));
            }
            // An accent's plain and wide forms are told apart by its width (the layout reader).
            if first.accent {
                others.retain(|o| !o.starts_with("\\wide"));
            }
            let symbol = unmeasured((*first).clone());
            let display = (symbol.class == Class::Large).then(|| Symbol { variants: vec![(symbol.latex.clone(), 1)], ..symbol.clone() });
            let grown = matches!(symbol.class, Class::Open | Class::Close | Class::Ord)
                .then(|| symbol.variants.iter().find(|(g, r)| *r == 0 && g == &symbol.latex).map(|(g, _)| g.clone()))
                .flatten()
                .filter(|g| g != "\\sqrt")
                .map(|g| Symbol { variants: vec![(g, 1)], ..symbol.clone() });
            let large_variants = if symbol.class == Class::Large { vec![(symbol.latex.clone(), 0)] } else { symbol.variants.clone() };
            ix.by_text.insert(text, UnicodeSymbol { symbol: Symbol { variants: large_variants, ..symbol }, display, grown, others });
        }
        // Several glyphs in the table (`\ne` is a slash over `=`), one character in a Unicode font.
        for c in t.all_compounds() {
            if c.function || c.unicode.chars().count() != 1 || ix.by_text.contains_key(&c.unicode) {
                continue;
            }
            let mut s = made(c.latex.clone(), c.unicode.clone(), Kind::Sym, None);
            s.class = c.class;
            let display = (s.class == Class::Large).then(|| Symbol { variants: vec![(s.latex.clone(), 1)], ..s.clone() });
            if s.class == Class::Large {
                s.variants = vec![(s.latex.clone(), 0)];
            }
            ix.by_text.insert(c.unicode.clone(), UnicodeSymbol { symbol: s, display, grown: None, others: Vec::new() });
        }
        ix.add_alphanumerics();
        ix.add_aliases_and_pieces();
        ix
    }

    /// The Mathematical Alphanumeric Symbols the table has no font for, and math italic Greek.
    fn add_alphanumerics(&mut self) {
        // Math italic Greek: the table's upright character, moved to the italic block.
        // α…ω, final sigma included, then the symbol forms and ∂.
        let mut pairs: Vec<(u32, u32)> = (0..25u32).map(|k| (0x1D6FC + k, 0x03B1 + k)).collect();
        for (italic, upright) in [
            (0x1D716u32, 0x03F5u32),
            (0x1D717, 0x03D1),
            (0x1D718, 0x03F0),
            (0x1D719, 0x03D5),
            (0x1D71A, 0x03F1),
            (0x1D71B, 0x03D6),
            (0x1D715, 0x2202),
        ] {
            pairs.push((italic, upright));
        }
        for (italic, upright) in pairs {
            let (Some(i), Some(u)) = (char::from_u32(italic), char::from_u32(upright)) else { continue };
            if let Some(found) = self.by_text.get(&u.to_string()).filter(|s| s.symbol.kind == Kind::Greek || u == '\u{2202}') {
                let symbol = Symbol { unicode: i.to_string(), ..found.symbol.clone() };
                self.by_text.entry(i.to_string()).or_insert_with(|| plain(symbol));
            }
        }
        // Alphabets: block start (capital A), the command, the alphabet, whether digits follow.
        let blocks: [(u32, &str, Alphabet); 7] = [
            (0x1D400, "\\symbf", Alphabet::SymBold),
            (0x1D468, "\\symbfit", Alphabet::SymBoldItalic),
            (0x1D5A0, "\\symsf", Alphabet::SymSans),
            (0x1D670, "\\symtt", Alphabet::SymMono),
            (0x1D538, "\\mathbb", Alphabet::Blackboard),
            (0x1D504, "\\mathfrak", Alphabet::Fraktur),
            (0x1D49C, "\\mathcal", Alphabet::Calligraphic),
        ];
        for (start, command, alphabet) in blocks {
            for (k, letter) in ('A'..='Z').chain('a'..='z').enumerate() {
                let Some(c) = u32::try_from(k).ok().and_then(|k| char::from_u32(start + k)) else { continue };
                if self.by_text.contains_key(&c.to_string()) {
                    continue;
                }
                let s = made(format!("{command}{{{letter}}}"), c.to_string(), Kind::Latin, Some(alphabet));
                self.by_text.insert(c.to_string(), plain(s));
            }
        }
        // Digits: bold, double-struck, sans, monospace.
        for (start, command, alphabet) in [
            (0x1D7CEu32, "\\symbf", Alphabet::SymBold),
            (0x1D7D8, "\\mathbb", Alphabet::Blackboard),
            (0x1D7E2, "\\symsf", Alphabet::SymSans),
            (0x1D7F6, "\\symtt", Alphabet::SymMono),
        ] {
            for (k, digit) in ('0'..='9').enumerate() {
                let Some(c) = u32::try_from(k).ok().and_then(|k| char::from_u32(start + k)) else { continue };
                if self.by_text.get(&c.to_string()).is_some_and(|s| s.symbol.alphabet != Some(Alphabet::Bold)) {
                    continue;
                }
                let s = made(format!("{command}{{{digit}}}"), c.to_string(), Kind::Digit, Some(alphabet));
                self.by_text.insert(c.to_string(), plain(s));
            }
        }
        // The table's `\mathbf` letters are U+1D400…: in a Unicode math font those are `\symbf`.
        for (k, letter) in ('A'..='Z').chain('a'..='z').enumerate() {
            let Some(c) = u32::try_from(k).ok().and_then(|k| char::from_u32(0x1D400 + k)) else { continue };
            let s = made(format!("\\symbf{{{letter}}}"), c.to_string(), Kind::Latin, Some(Alphabet::SymBold));
            self.by_text.insert(c.to_string(), plain(s));
        }
    }

    fn add_aliases_and_pieces(&mut self) {
        for (from, to) in ALIASES {
            if self.by_text.contains_key(&from.to_string()) {
                continue;
            }
            if let Some(found) = self.by_text.get(&to.to_string()) {
                let copy = UnicodeSymbol {
                    symbol: Symbol { unicode: from.to_string(), ..found.symbol.clone() },
                    display: None,
                    grown: None,
                    others: found.others.clone(),
                };
                self.by_text.insert(from.to_string(), copy);
            }
        }
        for (c, roles) in PIECES {
            let Some(group) = roles.first().map(|(g, _)| *g) else { continue };
            let Some(base) = self.by_text.get(group_char(group)).map(|s| s.symbol.clone()) else { continue };
            let pieces = roles.iter().map(|(g, r)| ((*g).to_owned(), *r)).collect();
            let s = Symbol { unicode: c.to_string(), pieces, variants: Vec::new(), ..base };
            self.by_text.insert(c.to_string(), plain(s));
        }
    }
}

/// The text of a delimiter group's plain glyph.
fn group_char(group: &str) -> &str {
    match group {
        "\\{" => "{",
        "\\}" => "}",
        g => g,
    }
}

/// The letter or digit inside `\mathrm{a}`.
fn styled_char(s: &Symbol) -> Option<char> {
    s.latex.strip_suffix('}').and_then(|t| t.chars().last()).filter(char::is_ascii_alphanumeric)
}

fn index() -> &'static Index {
    static INDEX: OnceLock<Index> = OnceLock::new();
    INDEX.get_or_init(|| Index::build(table()))
}

/// Every symbol the Unicode path can read by text alone (for tests and tools).
pub fn known_texts() -> impl Iterator<Item = (&'static str, &'static UnicodeSymbol)> {
    index().by_text.iter().map(|(k, v)| (k.as_str(), v))
}

/// Whether `text` is a large operator (`∑`, `∫`) by the Unicode path's reading.
pub fn is_large(text: &str) -> bool {
    index().by_text.get(text).is_some_and(|s| s.symbol.class == Class::Large)
}

/// What the glyph with Unicode text `text`, of a font with `role`, on a page of `page` kind, is.
pub fn read(text: &str, role: FontRole, page: PageKind) -> Reading {
    let ix = index();
    let mut chars = text.chars();
    let (Some(c), None) = (chars.next(), chars.next()) else {
        return Reading::Unknown(format!("a glyph whose text `{text}` is not one character"));
    };
    let known = |symbol: &'static UnicodeSymbol, doubt: Option<String>| Reading::Known { symbol, doubt };
    let missing = || Reading::Unknown(format!("no symbol for `{text}` (U+{:04X})", c as u32));
    if c.is_ascii_alphabetic() {
        let in_role = match role {
            FontRole::TextRoman => ix.roman.get(&c).map(|s| (s, None)),
            FontRole::TextBold => ix.bold.get(&c).map(|s| (s, None)),
            FontRole::TextItalic if page.math_italic_text => ix.text_italic.get(&c).map(|s| (s, None)),
            FontRole::TextItalic => ix.math_italic.get(&c).map(|s| {
                let doubt = page
                    .math_font
                    .then(|| format!("an italic `{c}` of a text font, on a page set with a Unicode math font: math italic or \\mathit"));
                (s, doubt)
            }),
            FontRole::Math => {
                return Reading::Unknown(format!("an upright `{c}` of a Unicode math font (\\symup or \\mathrm: the font does not say)"));
            }
            FontRole::TextBoldItalic | FontRole::Other => {
                return Reading::Unknown(format!("a letter `{c}` of a font whose name does not say its alphabet"));
            }
        };
        return in_role.map_or_else(missing, |(s, d)| known(s, d));
    }
    if c.is_ascii_digit() {
        let in_role = match role {
            FontRole::Math => ix.digit.get(&c),
            FontRole::TextRoman if page.math_font => ix.roman_digit(c),
            FontRole::TextRoman => ix.digit.get(&c),
            FontRole::TextBold => ix.bold.get(&c),
            FontRole::TextItalic | FontRole::TextBoldItalic | FontRole::Other => {
                return Reading::Unknown(format!("a digit `{c}` of a font whose name does not say its alphabet"));
            }
        };
        return in_role.map_or_else(missing, |s| known(s, None));
    }
    match ix.by_text.get(text) {
        Some(s) => {
            // An upright Greek letter of a Unicode math font is `\upalpha`; math italic is the italic block.
            let doubt = if s.symbol.kind == Kind::Greek && role == FontRole::Math && ('\u{03B1}'..='\u{03D6}').contains(&c) {
                Some(format!("an upright Greek `{c}` of a Unicode math font"))
            } else if s.others.is_empty() {
                None
            } else {
                Some(format!("`{text}` is {} or {} (one character for several commands)", s.symbol.latex, s.others.join(", ")))
            };
            known(s, doubt)
        }
        None => missing(),
    }
}

impl Index {
    fn roman_digit(&self, c: char) -> Option<&UnicodeSymbol> {
        self.digit_roman.get(&c)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn latex(text: &str, role: FontRole, page: PageKind) -> String {
        match read(text, role, page) {
            Reading::Known { symbol, .. } => symbol.symbol.latex.clone(),
            Reading::Unknown(m) => format!("?{m}"),
        }
    }

    const UNICODE_MATH: PageKind = PageKind { math_font: true, math_italic_text: true };
    const TIMES: PageKind = PageKind { math_font: false, math_italic_text: false };

    #[test]
    fn symbols_are_the_table_read_backwards() {
        for (text, want) in [
            ("+", "+"),
            ("=", "="),
            ("\u{2212}", "-"),
            ("\u{2211}", "\\sum"),
            ("\u{222B}", "\\int"),
            ("\u{2264}", "\\leq"),
            ("\u{2192}", "\\rightarrow"),
            ("\u{221E}", "\\infty"),
            ("\u{2260}", "\\ne"),
            ("\u{2209}", "\\notin"),
            ("\u{221A}", "\\surd"),
            ("\u{0393}", "\\Gamma"),
            ("\u{03B1}", "\\alpha"),
            ("|", "\\mid"),
            ("\u{2016}", "\\parallel"),
            ("\u{2236}", ":"),
            ("\u{02C6}", "\\hat"),
        ] {
            assert_eq!(latex(text, FontRole::Other, TIMES), want, "{text}");
        }
    }

    #[test]
    fn mathematical_alphanumerics_name_their_alphabet() {
        for (text, want) in [
            ("\u{1D465}", "x"),
            ("\u{1D6FC}", "\\alpha"),
            ("\u{1D700}", "\\varepsilon"),
            ("\u{1D716}", "\\epsilon"),
            ("\u{1D715}", "\\partial"),
            ("\u{211D}", "\\mathbb{R}"),
            ("\u{1D53B}", "\\mathbb{D}"),
            ("\u{2131}", "\\mathcal{F}"),
            ("\u{1D49C}", "\\mathcal{A}"),
            ("\u{1D504}", "\\mathfrak{A}"),
            ("\u{1D42F}", "\\symbf{v}"),
            ("\u{1D7CE}", "\\symbf{0}"),
            ("\u{210E}", "h"),
        ] {
            assert_eq!(latex(text, FontRole::Math, UNICODE_MATH), want, "{text}");
        }
    }

    #[test]
    fn plain_letters_take_their_alphabet_from_the_font_and_page() {
        assert_eq!(latex("d", FontRole::TextRoman, UNICODE_MATH), "\\mathrm{d}");
        assert_eq!(latex("v", FontRole::TextBold, UNICODE_MATH), "\\mathbf{v}");
        // Times: math italic is the text italic font, unless the page has math italic letters of its own.
        assert_eq!(latex("x", FontRole::TextItalic, TIMES), "x");
        assert_eq!(latex("x", FontRole::TextItalic, UNICODE_MATH), "\\mathit{x}");
        // Digits: the math font's in unicode-math, the text font's in Times packages.
        assert_eq!(latex("2", FontRole::Math, UNICODE_MATH), "2");
        assert_eq!(latex("2", FontRole::TextRoman, TIMES), "2");
        assert_eq!(latex("2", FontRole::TextRoman, UNICODE_MATH), "\\mathrm{2}");
        // A letter of a symbol font says nothing about its alphabet (txsym's `R` is a blackboard R).
        assert!(latex("R", FontRole::Other, TIMES).starts_with('?'));
        assert!(latex("x", FontRole::Math, UNICODE_MATH).starts_with('?'));
        assert!(latex("fi", FontRole::TextRoman, TIMES).starts_with('?'));
    }

    #[test]
    fn several_commands_for_one_character_are_a_doubt() {
        let Reading::Known { doubt, .. } = read("\u{2205}", FontRole::Math, UNICODE_MATH) else { panic!("emptyset") };
        assert!(doubt.is_some_and(|d| d.contains("\\varnothing")));
        let Reading::Known { doubt, .. } = read("\u{0393}", FontRole::Math, UNICODE_MATH) else { panic!("Gamma") };
        assert_eq!(doubt, None);
    }

    #[test]
    fn sizes_and_pieces_are_there() {
        let Some(sum) = index().by_text.get("\u{2211}") else { panic!("sum") };
        assert_eq!(sum.display.as_ref().map(|d| d.variants.clone()), Some(vec![("\\sum".to_owned(), 1)]));
        let Some(paren) = index().by_text.get("(") else { panic!("paren") };
        assert_eq!(paren.grown.as_ref().map(|d| d.variants.clone()), Some(vec![("(".to_owned(), 1)]));
        let Some(top) = index().by_text.get("\u{239B}") else { panic!("piece") };
        assert_eq!(top.symbol.pieces, vec![("(".to_owned(), Role::Top)]);
        assert!(known_texts().all(|(_, s)| s.symbol.from_unicode));
        // The table's metrics are Computer Modern's: a symbol read from another font has none.
        assert!(known_texts().all(|(_, s)| [s.symbol.width, s.symbol.italic, s.symbol.height, s.symbol.depth] == [0; 4]));
        assert!(sum.display.as_ref().is_some_and(|d| d.italic == 0));
    }

    #[test]
    fn font_roles_come_from_style_names() {
        for (name, role) in [
            ("ABCDEF+STIXTwoMath-Regular", FontRole::Math),
            ("LatinModernMath-Regular-Identity-H", FontRole::Math),
            ("STIXTwoText-Regular", FontRole::TextRoman),
            ("NimbusRomNo9L-Regu", FontRole::TextRoman),
            ("NimbusRomNo9L-ReguItal", FontRole::TextItalic),
            ("NimbusRomNo9L-Medi", FontRole::TextBold),
            ("TeXGyreTermesX-Regular", FontRole::TextRoman),
            ("STIXTwoText-BoldItalic", FontRole::TextBoldItalic),
            ("FiraSans-Italic", FontRole::TextItalic),
            ("NewTXMI", FontRole::Other),
            ("txsys", FontRole::Other),
            ("StandardSymL-Slant_167", FontRole::Other),
        ] {
            assert_eq!(font_role(name), role, "{name}");
        }
    }
}
