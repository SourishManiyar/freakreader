//! What needs no TeX: a token-level edit distance between LaTeX strings, the deliberate breakages
//! that prove the typeset check discriminates, and the report that tallies a recoverer's results.

use super::truth::EquationTruth;
use super::{Verdict, mutate};
use std::collections::BTreeMap;
use std::fmt::Write as _;

/// Longest LaTeX string, in tokens, that [`edit_distance`] compares; past it the distance is not
/// computed (it takes time and memory proportional to the product of the lengths).
pub const MAX_TOKENS: usize = 4096;

/// The tokens of a LaTeX string as TeX reads them, near enough for a distance: a control word
/// (`\frac`), a control symbol (`\,`, `\\`) or one character. Whitespace separates tokens and is
/// not one, so `\frac{1}{2}` and `\frac {1} {2}` have the same tokens; braces are tokens, so
/// `\frac12` is four edits from `\frac{1}{2}` (which is why the typeset check exists).
pub fn tokens(latex: &str) -> Vec<&str> {
    mutate::spans(latex).into_iter().filter_map(|(s, e)| latex.get(s..e)).collect()
}

/// The Levenshtein distance between the [`tokens`] of `a` and `b`: insertions, deletions and
/// substitutions of whole tokens. `None` when either has more than [`MAX_TOKENS`] tokens.
pub fn edit_distance(a: &str, b: &str) -> Option<usize> {
    let (a, b) = (tokens(a), tokens(b));
    if a.len() > MAX_TOKENS || b.len() > MAX_TOKENS {
        return None;
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len().saturating_add(1)];
    for (i, ta) in a.iter().enumerate() {
        if let Some(first) = cur.first_mut() {
            *first = i.saturating_add(1);
        }
        for (j, tb) in b.iter().enumerate() {
            let substitute = prev.get(j).copied().unwrap_or(0).saturating_add(usize::from(ta != tb));
            let delete = prev.get(j.saturating_add(1)).copied().unwrap_or(0).saturating_add(1);
            let insert = cur.get(j).copied().unwrap_or(0).saturating_add(1);
            if let Some(slot) = cur.get_mut(j.saturating_add(1)) {
                *slot = substitute.min(delete).min(insert);
            }
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev.last().copied()
}

/// A deliberate breakage of a correct LaTeX string, standing in for a recoverer's typical
/// mistakes. Each one changes what TeX typesets whenever it applies, so the typeset check must
/// reject every equation it applies to: that is how the harness proves it discriminates.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Mutation {
    /// Removes the first superscript (`^` and its argument).
    DropSuperscript,
    /// Swaps the numerator and denominator of the first `\frac`, `\dfrac`, `\tfrac` or `\cfrac`
    /// whose two arguments differ.
    SwapFraction,
    /// Replaces the first Greek letter with another (`\alpha` becomes `\beta`, any other becomes
    /// `\alpha`; capitals become `\Delta`, or `\Gamma` for `\Delta`).
    ChangeGreek,
}

impl Mutation {
    pub const ALL: [Mutation; 3] = [Mutation::DropSuperscript, Mutation::SwapFraction, Mutation::ChangeGreek];

    pub fn name(self) -> &'static str {
        match self {
            Mutation::DropSuperscript => "drop-superscript",
            Mutation::SwapFraction => "swap-fraction",
            Mutation::ChangeGreek => "change-greek",
        }
    }

    pub fn from_name(name: &str) -> Option<Mutation> {
        Mutation::ALL.into_iter().find(|m| m.name() == name)
    }

    /// The broken string, or `None` when the breakage does not apply (no superscript, no fraction
    /// with distinct arguments, no Greek letter).
    pub fn apply(self, latex: &str) -> Option<String> {
        match self {
            Mutation::DropSuperscript => mutate::drop_superscript(latex),
            Mutation::SwapFraction => mutate::swap_fraction(latex),
            Mutation::ChangeGreek => mutate::change_greek(latex),
        }
    }
}

/// What one recoverer made of one equation.
#[derive(Clone, Debug)]
pub struct Outcome<'a> {
    pub truth: &'a EquationTruth,
    /// The recoverer's LaTeX; `None` when it gave none.
    pub answer: Option<String>,
    /// The typeset check; `None` when it was not run (no TeX). An equation without an answer is
    /// [`Verdict::Different`] without running TeX.
    pub verdict: Option<Verdict>,
}

impl Outcome<'_> {
    /// The token edit distance to the truth's source. No answer counts as the empty string: the
    /// distance is the truth's length. `None` past [`MAX_TOKENS`].
    pub fn distance(&self) -> Option<usize> {
        edit_distance(&self.truth.source, self.answer.as_deref().unwrap_or(""))
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tally {
    pub count: usize,
    pub answered: usize,
    /// Equations whose typeset check ran.
    pub checked: usize,
    pub equal: usize,
    /// TeX could not typeset the answer.
    pub failed: usize,
    pub distance_sum: usize,
    pub distance_count: usize,
}

impl Tally {
    fn add(&mut self, o: &Outcome<'_>) {
        self.count = self.count.saturating_add(1);
        self.answered = self.answered.saturating_add(usize::from(o.answer.is_some()));
        if let Some(v) = &o.verdict {
            self.checked = self.checked.saturating_add(1);
            self.equal = self.equal.saturating_add(usize::from(*v == Verdict::Equal));
            self.failed = self.failed.saturating_add(usize::from(matches!(v, Verdict::Failed(_))));
        }
        if let Some(d) = o.distance() {
            self.distance_sum = self.distance_sum.saturating_add(d);
            self.distance_count = self.distance_count.saturating_add(1);
        }
    }

    /// The share of checked equations that typeset equal, in `[0, 1]`; `None` when none was checked.
    pub fn equal_rate(&self) -> Option<f64> {
        (self.checked > 0).then(|| self.equal as f64 / self.checked as f64)
    }

    /// The share of the *answered* equations that typeset equal: how often an answer given is right.
    /// `None` when nothing was answered or nothing checked.
    pub fn precision(&self) -> Option<f64> {
        (self.answered > 0 && self.checked > 0).then(|| self.equal as f64 / self.answered as f64)
    }

    pub fn mean_distance(&self) -> Option<f64> {
        (self.distance_count > 0).then(|| self.distance_sum as f64 / self.distance_count as f64)
    }

    fn row(&self, label: &str) -> String {
        let equal = match self.equal_rate() {
            Some(rate) => format!("{:>4}/{:<4} {:>6.1}%", self.equal, self.checked, rate * 100.0),
            None => format!("{:>17}", "not run"),
        };
        let distance = self.mean_distance().map_or_else(|| "-".to_owned(), |d| format!("{d:.2}"));
        let precision = self.precision().map_or_else(|| "-".to_owned(), |p| format!("{:.1}%", p * 100.0));
        format!("  {label:<22} {:>5} {:>8}  {equal}  {precision:>9}  {:>6}  {distance:>9}\n", self.count, self.answered, self.failed)
    }
}

/// Tallies of a recoverer's outcomes: overall, and by category, tier and context.
#[derive(Clone, Debug, Default)]
pub struct Report {
    pub total: Tally,
    pub by_category: BTreeMap<String, Tally>,
    pub by_tier: BTreeMap<u8, Tally>,
    pub by_context: BTreeMap<String, Tally>,
}

impl Report {
    pub fn new(outcomes: &[Outcome<'_>]) -> Report {
        let mut r = Report::default();
        for o in outcomes {
            r.total.add(o);
            r.by_category.entry(o.truth.category.clone()).or_default().add(o);
            r.by_tier.entry(o.truth.tier).or_default().add(o);
            r.by_context.entry(o.truth.context.clone()).or_default().add(o);
        }
        r
    }

    /// The report as a table, headed by `name`.
    pub fn render(&self, name: &str) -> String {
        let mut s = String::new();
        let _ = writeln!(s, "recoverer `{name}`");
        let header =
            format!("  {:<22} {:>5} {:>8}  {:>17}  {:>9}  {:>6}  {:>9}\n", "", "n", "answered", "typeset-equal", "precision", "failed", "mean edit");
        let mut section = |title: &str, rows: Vec<(String, &Tally)>| {
            let _ = writeln!(s, "  by {title}");
            s.push_str(&header);
            for (label, t) in rows {
                s.push_str(&t.row(&label));
            }
        };
        section("category", self.by_category.iter().map(|(k, t)| (k.clone(), t)).collect());
        section("tier", self.by_tier.iter().map(|(k, t)| (format!("tier {k}"), t)).collect());
        section("context", self.by_context.iter().map(|(k, t)| (k.clone(), t)).collect());
        s.push_str(&self.total.row("all"));
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_follow_tex() {
        assert_eq!(tokens(r"\frac{1}{2}"), [r"\frac", "{", "1", "}", "{", "2", "}"]);
        assert_eq!(tokens(r"\frac 12"), [r"\frac", "1", "2"]);
        assert_eq!(tokens(r"a\,b\\c\alpha2"), ["a", r"\,", "b", r"\\", "c", r"\alpha", "2"]);
        assert_eq!(tokens(r"x\"), ["x", r"\"]);
        assert_eq!(tokens(r"\é x"), [r"\é", "x"]);
        assert_eq!(tokens("  "), Vec::<&str>::new());
    }

    #[test]
    fn distance_counts_token_edits() {
        assert_eq!(edit_distance(r"\frac{1}{2}", r"\frac {1}{2}"), Some(0));
        assert_eq!(edit_distance(r"\frac12", r"\frac{1}{2}"), Some(4));
        assert_eq!(edit_distance(r"\alpha+\beta", r"\alpha+\gamma"), Some(1));
        assert_eq!(edit_distance("", "abc"), Some(3));
        assert_eq!(edit_distance("abc", ""), Some(3));
        assert_eq!(edit_distance("kitten", "sitting"), Some(3));
        let long = "x".repeat(MAX_TOKENS.saturating_add(1));
        assert_eq!(edit_distance(&long, "x"), None);
    }
}
