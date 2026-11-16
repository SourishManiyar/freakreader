//! The math oracle's report: scores LaTeX recoverers against the committed equation truth, per
//! category, tier and context. Run it as `cargo xtask math-oracle report [ARGS]`.
//!
//! ```text
//! --recoverer NAME   score only NAME (repeatable); the default is every registered recoverer
//! --no-tex           skip the typeset check (edit distance only)
//! --list             list the registered recoverers
//! ```
//!
//! The typeset check needs `pdflatex` and `dvitype`. Without them the report says so and shows edit
//! distances only, unless `PDFCRAFT_ORACLES_REQUIRED` is set, when it fails.

#[path = "../tests/it/math_oracle/harness.rs"]
mod harness;

use harness::{Broken, Identity, Nothing, Pages, answers, load, outcomes};
use pdfcraft_oracle::tex::{EquationTruth, Mutation, Report, Tex};
use pdfcraft_render::text::math::recover::LatexRecoverer;
use std::process::ExitCode;

/// Every recoverer the report knows: name, what it is, the recoverer.
fn registry(truths: &[EquationTruth]) -> Vec<(String, &'static str, Box<dyn LatexRecoverer + '_>)> {
    // Real recoverers go first, here. None exists yet.
    let mut out: Vec<(String, &'static str, Box<dyn LatexRecoverer + '_>)> = Vec::new();
    out.push(("none".to_owned(), "baseline: answers nothing (no recoverer exists yet)", Box::new(Nothing)));
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
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--recoverer" => wanted.push(it.next().ok_or("--recoverer needs a name")?.clone()),
            "--no-tex" => use_tex = false,
            "--list" => list = true,
            other => return Err(format!("unknown argument `{other}` (--recoverer NAME, --no-tex, --list)")),
        }
    }
    let truth = load("truth.tsv")?;
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
        print!("{}", Report::new(&scored).render(name));
    }
    Ok(())
}
