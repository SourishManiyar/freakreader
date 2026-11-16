//! The math oracle's report: scores LaTeX recoverers against the committed equation truth, per
//! category, tier and context. Run it as `cargo xtask math-oracle report [ARGS]`.
//!
//! ```text
//! --recoverer NAME   score only NAME (repeatable); the default is every registered recoverer
//! --no-tex           skip the typeset check (edit distance only)
//! --list             list the registered recoverers
//! --failures        also list the development equations (alone) the typeset check rejects: truth and answer
//! --explain ID       for a development equation: what the layout reader says (answer, confidence,
//!                    diagnostics) next to the truth; refuses a held-out equation (`ho_…`)
//!
//! The scores are printed for the development equations and for the held-out ones (`ho_…`) apart.
//! `precision` is how often an answer given is typeset-equal; for `layout-confident` it is the
//! precision of the confident answers.
//! ```
//!
//! The typeset check needs `pdflatex` and `dvitype`. Without them the report says so and shows edit
//! distances only, unless `PDFCRAFT_ORACLES_REQUIRED` is set, when it fails.

#[path = "../tests/it/math_oracle/harness.rs"]
mod harness;

use harness::{Broken, Identity, Nothing, Pages, answers, load, outcomes, region_of};
use pdfcraft_oracle::tex::Env;
use pdfcraft_oracle::tex::{EquationTruth, Mutation, Report, Tex};
use pdfcraft_render::text::math::MathPage;
use pdfcraft_render::text::math::recover::{Confidence, ConfidentRecoverer, LatexRecoverer, LayoutRecoverer, RecoverOptions, Style, recover_latex};
use std::process::ExitCode;

/// Every recoverer the report knows: name, what it is, the recoverer.
#[allow(clippy::vec_init_then_push)] // a list written one entry at a time, with comments
fn registry(truths: &[EquationTruth]) -> Vec<(String, &'static str, Box<dyn LatexRecoverer + '_>)> {
    // Real recoverers go first, here.
    let mut out: Vec<(String, &'static str, Box<dyn LatexRecoverer + '_>)> = Vec::new();
    out.push(("layout".to_owned(), "the layout reader: its best answer, sure or not", Box::new(LayoutRecoverer::default())));
    out.push(("layout-confident".to_owned(), "the layout reader: only the answers it is confident of", Box::new(ConfidentRecoverer::default())));
    out.push((
        "layout+env".to_owned(),
        "the layout reader told whether the equation is inline or displayed (read from the truth: what a caller that knows its environment passes)",
        Box::new(WithEnvironment { truths, confident_only: false }),
    ));
    out.push((
        "layout+env-confident".to_owned(),
        "as layout+env, only the answers it is confident of",
        Box::new(WithEnvironment { truths, confident_only: true }),
    ));
    out.push(("none".to_owned(), "baseline: answers nothing", Box::new(Nothing)));
    out.push(("identity".to_owned(), "test: answers the truth's source, found through the page", Box::new(Identity { truths })));
    for m in Mutation::ALL {
        out.push((
            m.name().to_owned(),
            "test: the identity with one deliberate mistake",
            Box::new(Broken { identity: Identity { truths }, mutation: m }),
        ));
    }
    out
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("math oracle: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut wanted = Vec::new();
    let mut use_tex = true;
    let mut list = false;
    let mut explain: Vec<String> = Vec::new();
    let mut failures = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--recoverer" => wanted.push(it.next().ok_or("--recoverer needs a name")?.clone()),
            "--no-tex" => use_tex = false,
            "--list" => list = true,
            "--failures" => failures = true,
            "--explain" => explain.push(it.next().ok_or("--explain needs an equation id")?.clone()),
            other => return Err(format!("unknown argument `{other}` (--recoverer NAME, --no-tex, --list, --explain ID)")),
        }
    }
    let truth = load("truth.tsv")?;
    if !explain.is_empty() {
        return explain_equations(&truth.equations, &explain);
    }
    let registry = registry(&truth.equations);
    if list {
        for (name, about, _) in &registry {
            println!("{name:<18} {about}");
        }
        return Ok(());
    }
    if let Some(unknown) = wanted.iter().find(|w| !registry.iter().any(|(n, _, _)| n == *w)) {
        return Err(format!("no recoverer `{unknown}` (--list shows them)"));
    }
    let tex = if use_tex {
        match Tex::find() {
            Ok(t) => Some(t),
            Err(absent) if absent.required() => return Err(absent.to_string()),
            Err(absent) => {
                eprintln!("{absent}");
                None
            }
        }
    } else {
        None
    };
    let alone = truth.equations.iter().filter(|e| e.context == "alone").count();
    println!(
        "math oracle: {} equations, {} occurrences ({} in running text); typeset check: {}",
        alone,
        truth.equations.len(),
        truth.equations.len() - alone,
        tex.as_ref().map_or("not run", |t| t.version())
    );
    let mut pages = Pages::default();
    for (name, about, recoverer) in &registry {
        if !wanted.is_empty() && !wanted.contains(name) {
            continue;
        }
        let got = answers(recoverer.as_ref(), &truth.equations, &mut pages)?;
        let scored = outcomes(&truth.equations, got, tex.as_ref()).map_err(|e| e.to_string())?;
        println!("\n{about}");
        // Development and held-out equations are scored apart: a recoverer is developed against the
        // first and the second says whether that fitted it to them.
        let (held_out, development): (Vec<_>, Vec<_>) = scored.into_iter().partition(|o| o.truth.base.starts_with("ho_"));
        println!("development equations");
        print!("{}", Report::new(&development).render(name));
        if failures {
            for o in development
                .iter()
                .filter(|o| o.truth.context == "alone" && o.verdict.as_ref().is_some_and(|v| *v != pdfcraft_oracle::tex::Verdict::Equal))
            {
                println!(
                    "  ✗ {:<22} {}\n      truth:  {}\n      answer: {}",
                    o.truth.id,
                    o.truth.category,
                    o.truth.source,
                    o.answer.as_deref().unwrap_or("(none)")
                );
            }
        }
        if !held_out.is_empty() {
            println!("held-out equations");
            print!("{}", Report::new(&held_out).render(name));
        }
    }
    Ok(())
}

/// Shows what the layout reader makes of development equations, for fixing it. Held-out equations are
/// refused: a recoverer must not be fitted to them.
fn explain_equations(truths: &[EquationTruth], ids: &[String]) -> Result<(), String> {
    use pdfcraft_render::text::math::recover::{RecoverOptions, recover_latex};
    let mut pages = Pages::default();
    for id in ids {
        if id.starts_with("ho_") || id.split('@').next().is_some_and(|b| b.starts_with("ho_")) {
            return Err(format!("`{id}` is a held-out equation: it is not to be read one by one"));
        }
        let t = truths.iter().find(|t| &t.id == id).ok_or_else(|| format!("no equation `{id}`"))?;
        let page = pages.get(&t.doc, t.page)?;
        println!("{id}  [{} tier {} {}]", t.category, t.tier, t.env.as_str());
        println!("  truth:  {}", t.source);
        match recover_latex(page, region_of(t), &RecoverOptions::default()) {
            Ok(r) => {
                println!("  answer: {}   ({:?})", r.latex, r.confidence);
                for d in &r.diagnostics {
                    println!("    {:?}: {}", d.severity, d.message);
                }
            }
            Err(e) => {
                println!("  no answer: {e}");
                for d in &e.diagnostics {
                    println!("    {:?}: {}", d.severity, d.message);
                }
            }
        }
    }
    Ok(())
}

/// The layout reader given the environment (inline or a display) of the equation it is asked about,
/// which a caller that knows where the region came from can say. The environment is the only thing
/// taken from the truth, found by the region as `Identity` finds an equation.
struct WithEnvironment<'a> {
    truths: &'a [EquationTruth],
    confident_only: bool,
}

impl LatexRecoverer for WithEnvironment<'_> {
    fn recover(&self, page: &MathPage, region: [f32; 4]) -> Option<String> {
        let env = self.truths.iter().find(|t| region_of(t) == region)?.env;
        let style = if env == Env::Inline { Style::Text } else { Style::Display };
        let got = recover_latex(page, region, &RecoverOptions { style: Some(style) }).ok()?;
        (!self.confident_only || got.confidence == Confidence::Confident).then_some(got.latex)
    }
}
