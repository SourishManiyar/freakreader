//! The deliberate breakages of [`super::Mutation`], on LaTeX tokens.

/// Byte spans of the tokens of `latex` (see [`super::tokens`]).
pub(crate) fn spans(latex: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut at = 0usize;
    while let Some(c) = latex.get(at..).and_then(|r| r.chars().next()) {
        if c.is_whitespace() {
            at = at.saturating_add(c.len_utf8());
            continue;
        }
        let len = if c == '\\' {
            let after = latex.get(at.saturating_add(1)..).unwrap_or("");
            let letters = after.find(|x: char| !x.is_ascii_alphabetic()).unwrap_or(after.len());
            if letters > 0 { letters.saturating_add(1) } else { after.chars().next().map_or(1, |x| x.len_utf8().saturating_add(1)) }
        } else {
            c.len_utf8()
        };
        let end = at.saturating_add(len).min(latex.len());
        out.push((at, end));
        at = end;
    }
    out
}

/// An argument: the token at `spans[i]`, or the balanced group it opens. Returns the index of the
/// span after the argument and the byte range of its content (without the braces).
fn argument(latex: &str, spans: &[(usize, usize)], i: usize) -> Option<(usize, (usize, usize))> {
    let &(start, end) = spans.get(i)?;
    if latex.get(start..end)? != "{" {
        return (latex.get(start..end)? != "}").then_some((i.saturating_add(1), (start, end)));
    }
    let mut depth = 0usize;
    for (j, &(s, e)) in spans.iter().enumerate().skip(i) {
        match latex.get(s..e)? {
            "{" => depth = depth.saturating_add(1),
            "}" => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some((j.saturating_add(1), (end, s)));
                }
            }
            _ => {}
        }
    }
    None
}

pub(crate) fn drop_superscript(latex: &str) -> Option<String> {
    let spans = spans(latex);
    let i = spans.iter().position(|&(s, e)| latex.get(s..e) == Some("^"))?;
    let (next, _) = argument(latex, &spans, i.saturating_add(1))?;
    let cut_start = spans.get(i)?.0;
    let cut_end = spans.get(next.saturating_sub(1))?.1;
    Some(format!("{}{}", latex.get(..cut_start)?, latex.get(cut_end..)?))
}

pub(crate) fn swap_fraction(latex: &str) -> Option<String> {
    let spans = spans(latex);
    for (i, &(s, e)) in spans.iter().enumerate() {
        if !matches!(latex.get(s..e)?, r"\frac" | r"\dfrac" | r"\tfrac" | r"\cfrac") {
            continue;
        }
        let Some((after_first, first)) = argument(latex, &spans, i.saturating_add(1)) else { continue };
        let Some((after_second, second)) = argument(latex, &spans, after_first) else { continue };
        let (a, b) = (latex.get(first.0..first.1)?.trim(), latex.get(second.0..second.1)?.trim());
        if a == b {
            continue;
        }
        let tail = spans.get(after_second.saturating_sub(1))?.1;
        return Some(format!("{}{{{b}}}{{{a}}}{}", latex.get(..e)?, latex.get(tail..)?));
    }
    None
}

const GREEK_LOWER: &[&str] = &[
    "alpha",
    "beta",
    "gamma",
    "delta",
    "epsilon",
    "varepsilon",
    "zeta",
    "eta",
    "theta",
    "vartheta",
    "iota",
    "kappa",
    "lambda",
    "mu",
    "nu",
    "xi",
    "pi",
    "varpi",
    "rho",
    "varrho",
    "sigma",
    "varsigma",
    "tau",
    "upsilon",
    "phi",
    "varphi",
    "chi",
    "psi",
    "omega",
];
const GREEK_UPPER: &[&str] = &[
    "Gamma",
    "Delta",
    "Theta",
    "Lambda",
    "Xi",
    "Pi",
    "Sigma",
    "Upsilon",
    "Phi",
    "Psi",
    "Omega",
    "varGamma",
    "varDelta",
    "varTheta",
    "varLambda",
    "varXi",
    "varPi",
    "varSigma",
    "varUpsilon",
    "varPhi",
    "varPsi",
    "varOmega",
];

pub(crate) fn change_greek(latex: &str) -> Option<String> {
    for (s, e) in spans(latex) {
        let Some(word) = latex.get(s..e).and_then(|t| t.strip_prefix('\\')) else { continue };
        let replacement = if GREEK_LOWER.contains(&word) {
            if word == "alpha" { "beta" } else { "alpha" }
        } else if GREEK_UPPER.contains(&word) {
            if word == "Delta" { "Gamma" } else { "Delta" }
        } else {
            continue;
        };
        return Some(format!("{}\\{replacement}{}", latex.get(..s)?, latex.get(e..)?));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::super::Mutation;

    #[test]
    fn each_breakage_changes_what_it_says() {
        let drop = |s| Mutation::DropSuperscript.apply(s);
        assert_eq!(drop("x^2 + y^2").as_deref(), Some("x + y^2"));
        assert_eq!(drop(r"e^{i\pi} + 1").as_deref(), Some(r"e + 1"));
        assert_eq!(drop(r"\sum_{i}^{\infty} a").as_deref(), Some(r"\sum_{i} a"));
        assert_eq!(drop(r"x^\alpha y").as_deref(), Some(r"x y"));
        assert_eq!(drop(r"x_1 \^{}"), None);
        assert_eq!(drop("x^"), None);
        assert_eq!(drop("x^{1"), None);

        let swap = |s| Mutation::SwapFraction.apply(s);
        assert_eq!(swap(r"\frac{a+b}{c}").as_deref(), Some(r"\frac{c}{a+b}"));
        assert_eq!(swap(r"\frac12").as_deref(), Some(r"\frac{2}{1}"));
        assert_eq!(swap(r"\frac\pi x").as_deref(), Some(r"\frac{x}{\pi}"));
        assert_eq!(swap(r"\frac{1}{1} + \dfrac{x}{y}").as_deref(), Some(r"\frac{1}{1} + \dfrac{y}{x}"));
        assert_eq!(swap(r"\frac{a}{a}"), None);
        assert_eq!(swap(r"\fraction{a}{b}"), None);
        assert_eq!(swap(r"\frac{a}"), None);

        let greek = |s| Mutation::ChangeGreek.apply(s);
        assert_eq!(greek(r"\alpha+\beta").as_deref(), Some(r"\beta+\beta"));
        assert_eq!(greek(r"x\pi").as_deref(), Some(r"x\alpha"));
        assert_eq!(greek(r"\Gamma(n)").as_deref(), Some(r"\Delta(n)"));
        assert_eq!(greek(r"\Delta").as_deref(), Some(r"\Gamma"));
        assert_eq!(greek(r"\alphabet + \pin"), None);
        assert_eq!(greek("alpha"), None);
    }

    #[test]
    fn hostile_strings_do_not_panic() {
        for s in ["", "\\", "^", "{", "}", "^}", r"\frac{", r"\frac}{", "é^é", r"\frac{é}{è}", "{{{{{{", "}}}}", r"\\\\\\^"] {
            for m in Mutation::ALL {
                let _ = m.apply(s);
            }
        }
    }
}
