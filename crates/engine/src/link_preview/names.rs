//! What a named destination names, read from the name LaTeX's hyperref gives it: the counter
//! the target was numbered with, then that counter's value (`equation.4.7`, `theorem.2.1`,
//! `section.3`, `cite.knuth84`, `page.12`).

/// The longest number taken from a destination name, in bytes. hyperref's are a few
/// characters (`4.7`, `A.2`); anything longer is not shown.
pub const MAX_REF_NUMBER_BYTES: usize = 16;

/// What kind of thing a destination names, by its hyperref name.
///
/// `TheoremLike` covers every amsthm-style statement: theorem, lemma, proposition, corollary,
/// definition, remark, example and their kin. When environments share one counter (as
/// `\newtheorem{lemma}[theorem]{Lemma}` makes them), hyperref names them all after that
/// counter, so `theorem.2.1` can be Lemma 2.1: the name cannot tell which statement it is, and
/// this does not guess. The statement's own text says what it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RefKind {
    /// A numbered display: `equation`, `align` and the other amsmath environments (`equation.N`;
    /// `AMS.N` for one `\tag`ged by hand).
    Equation,
    /// A bibliography entry (`cite.KEY`).
    Citation,
    /// A sectioning heading: part, chapter, section down to subparagraph, an appendix.
    Section,
    /// A theorem, lemma, definition, remark or other amsthm statement (see the type's notes).
    TheoremLike,
    /// A figure float (`figure.N`, `subfigure.N`).
    Figure,
    /// A table float (`table.N`, `subtable.N`).
    Table,
    /// An item of a list (`Item.N`, `enumi.N`).
    Item,
    /// The top of a page (`page.N`).
    Page,
    /// Any other name, or a destination with none.
    Unknown,
}

impl RefKind {
    /// The kind's name in automation output: `equation`, `citation`, `section`, `theorem_like`,
    /// `figure`, `table`, `item`, `page` or `unknown`.
    pub fn as_str(self) -> &'static str {
        match self {
            RefKind::Equation => "equation",
            RefKind::Citation => "citation",
            RefKind::Section => "section",
            RefKind::TheoremLike => "theorem_like",
            RefKind::Figure => "figure",
            RefKind::Table => "table",
            RefKind::Item => "item",
            RefKind::Page => "page",
            RefKind::Unknown => "unknown",
        }
    }
}

/// A destination name read as a reference: its kind, and the number it was given when the name
/// follows hyperref's pattern (`counter.number`, the number dot-separated letters and digits).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefName {
    pub kind: RefKind,
    /// The target's number as the name has it (`4.7` of `equation.4.7`). `None` for a citation
    /// (its key is not a number), a starred heading (`section*.2` counts unnumbered ones), an
    /// equation tagged by hand (`AMS.3` counts tags, not what they say), and any name that does
    /// not follow the pattern.
    pub number: Option<String>,
}

/// amsthm counters papers commonly use, and their abbreviations, lower case.
const THEOREM_LIKE: &[&str] = &[
    "theorem",
    "lemma",
    "proposition",
    "corollary",
    "definition",
    "remark",
    "example",
    "conjecture",
    "claim",
    "fact",
    "exercise",
    "problem",
    "assumption",
    "hypothesis",
    "notation",
    "observation",
    "question",
    "note",
    "property",
    "axiom",
    "criterion",
    "condition",
    "construction",
    "convention",
    "statement",
    "result",
    "thm",
    "lem",
    "lma",
    "prop",
    "prp",
    "cor",
    "coro",
    "defn",
    "def",
    "dfn",
    "rem",
    "rmk",
    "ex",
    "exa",
    "exm",
    "conj",
    "obs",
    "hyp",
    "ass",
    "asm",
    "nota",
    "prob",
    "quest",
    "propty",
];

/// The kind and number `name` gives, by hyperref's naming (see [`RefName`]). Never fails: a name
/// that is not hyperref's is [`RefKind::Unknown`] with no number.
pub fn ref_name(name: &str) -> RefName {
    let unknown = RefName { kind: RefKind::Unknown, number: None };
    let Some((counter, value)) = name.split_once('.') else { return unknown };
    let starred = counter.ends_with('*');
    let base = counter.trim_end_matches('*').to_ascii_lowercase();
    let kind = match base.as_str() {
        "equation" | "ams" => RefKind::Equation,
        "cite" => RefKind::Citation,
        "part" | "chapter" | "section" | "subsection" | "subsubsection" | "paragraph" | "subparagraph" | "appendix" => RefKind::Section,
        "figure" | "subfigure" => RefKind::Figure,
        "table" | "subtable" => RefKind::Table,
        "item" | "enumi" | "enumii" | "enumiii" | "enumiv" => RefKind::Item,
        "page" => RefKind::Page,
        other if THEOREM_LIKE.contains(&other) => RefKind::TheoremLike,
        _ => return unknown,
    };
    let numbered = !starred && !matches!(kind, RefKind::Citation) && base != "ams";
    let number = (numbered && is_number(value)).then(|| value.to_owned());
    RefName { kind, number }
}

/// Whether `value` is a number as hyperref writes one: letters and digits in dot-separated parts,
/// with at least one digit or a single letter (`4.7`, `A`, `A.2`, `iv`), and short.
fn is_number(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_REF_NUMBER_BYTES
        && value.split('.').all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_alphanumeric()))
        && (value.chars().any(|c| c.is_ascii_digit()) || value.split('.').all(|part| part.len() <= 4))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kind(name: &str) -> (RefKind, Option<String>) {
        let r = ref_name(name);
        (r.kind, r.number)
    }

    #[test]
    fn hyperref_names_read_as_their_kind_and_number() {
        assert_eq!(kind("equation.4.7"), (RefKind::Equation, Some("4.7".into())));
        assert_eq!(kind("cite.knuth84"), (RefKind::Citation, None));
        assert_eq!(kind("section.3"), (RefKind::Section, Some("3".into())));
        assert_eq!(kind("subsection.3.1"), (RefKind::Section, Some("3.1".into())));
        assert_eq!(kind("appendix.A"), (RefKind::Section, Some("A".into())));
        assert_eq!(kind("theorem.2.1"), (RefKind::TheoremLike, Some("2.1".into())));
        assert_eq!(kind("lemma.5"), (RefKind::TheoremLike, Some("5".into())));
        assert_eq!(kind("figure.2"), (RefKind::Figure, Some("2".into())));
        assert_eq!(kind("table.1"), (RefKind::Table, Some("1".into())));
        assert_eq!(kind("Item.5"), (RefKind::Item, Some("5".into())));
        assert_eq!(kind("page.3"), (RefKind::Page, Some("3".into())));
    }

    #[test]
    fn names_with_no_printed_number_have_none() {
        // Unnumbered headings, hand tags, and anything not hyperref's.
        assert_eq!(kind("section*.1"), (RefKind::Section, None));
        assert_eq!(kind("AMS.3"), (RefKind::Equation, None));
        assert_eq!(kind("Doc-Start"), (RefKind::Unknown, None));
        assert_eq!(kind("mylabel"), (RefKind::Unknown, None));
        assert_eq!(kind("chapter.intro.notes"), (RefKind::Section, None));
        assert_eq!(kind("equation."), (RefKind::Equation, None));
        assert_eq!(kind("equation.1..2"), (RefKind::Equation, None));
        assert_eq!(kind(&format!("equation.{}", "1".repeat(MAX_REF_NUMBER_BYTES + 1))), (RefKind::Equation, None));
        assert_eq!(kind("équation.1"), (RefKind::Unknown, None));
        assert_eq!(kind("equation.4·7"), (RefKind::Equation, None));
        assert_eq!(kind(""), (RefKind::Unknown, None));
        assert_eq!(kind("."), (RefKind::Unknown, None));
    }
}
