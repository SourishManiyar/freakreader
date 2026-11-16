//! `text_recover_latex` end to end: equations of the math oracle's pages (TeX-typeset, with known
//! sources) read back as LaTeX through the tool, what it says of a region it cannot read, and what it
//! refuses.

use pdfcraft_automation::{Automation, Content, tools};
use serde_json::{Value, json};

fn ok(a: &mut Automation, tool: &str, args: Value) -> Value {
    match a.call(tool, &args) {
        Ok(mut c) => match c.remove(0) {
            Content::Json(v) => v,
            other => panic!("{tool}: expected JSON, got {other:?}"),
        },
        Err(e) => panic!("{tool} {args}: {e}"),
    }
}

fn oracle(name: &str) -> String {
    format!("{}/../render/tests/data/math/oracle/{name}", env!("CARGO_MANIFEST_DIR"))
}

/// An equation of the oracle's truth: the document, its 1-based page, and its box.
struct Place {
    doc: &'static str,
    page: u64,
    rect: [f64; 4],
}

fn place(id: &str) -> Place {
    let truth = std::fs::read_to_string(oracle("truth.tsv")).unwrap();
    let line = truth.lines().find(|l| l.starts_with(&format!("E\t{id}\t"))).unwrap_or_else(|| panic!("no equation {id}"));
    let f: Vec<&str> = line.split('\t').collect();
    // E id base doc page category tier env context x0 y0 x1 y1 source
    let doc = if f[3] == "equations" { "equations.pdf" } else { "paragraphs.pdf" };
    let n = |k: usize| f[k].parse::<f64>().unwrap();
    Place { doc, page: f[4].parse::<u64>().unwrap() + 1, rect: [n(9), n(10), n(11), n(12)] }
}

fn open(a: &mut Automation, doc: &str) -> u64 {
    ok(a, "doc_open", json!({ "path": oracle(doc) }))["doc"].as_u64().unwrap()
}

fn recover(a: &mut Automation, docs: &mut std::collections::HashMap<&'static str, u64>, id: &str, display: Option<bool>) -> Value {
    let p = place(id);
    let doc = *docs.entry(p.doc).or_insert_with(|| open(a, p.doc));
    let mut args = json!({ "doc": doc, "page": p.page, "rect": p.rect });
    if let Some(d) = display {
        args["display"] = json!(d);
    }
    ok(a, "text_recover_latex", args)
}

#[test]
fn typeset_equations_come_back_as_latex_with_a_confidence() {
    let mut a = Automation::new();
    let mut docs = std::collections::HashMap::new();
    for (id, display, latex) in [
        ("sup_squares", Some(false), r"x^{2} + y^{2} = r^{2}"),
        ("sqrt_cube", Some(true), r"\sqrt[3]{u + v}"),
        ("int_power", Some(true), r"\int_{0}^{1}x^{2}\,dx = \frac{1}{3}"),
        ("lim_inline", Some(false), r"\lim_{x \rightarrow 0}\frac{\sin x}{x} = 1"),
        ("hat_letters", Some(false), r"\hat{x} + \hat{\theta}"),
        ("text_and", Some(false), r"x \in A\text{ and }y \notin A"),
        ("bold_vectors", None, r"\mathbf{x} \cdot \mathbf{y} = \mathbf{0}"),
    ] {
        let r = recover(&mut a, &mut docs, id, display);
        assert_eq!((r["latex"].as_str(), r["confidence"].as_str()), (Some(latex), Some("confident")), "{id}: {r}");
        assert!(r["diagnostics"].as_array().unwrap().iter().all(|d| d["severity"] == "info"), "{id}: {r}");
        assert_eq!(r["page"], place(id).page);
    }
}

/// The answer depends on the environment where the glyphs do not settle it: a numerator the size of
/// the text is `\frac` in a display and `\dfrac` in text.
#[test]
fn a_choice_the_glyphs_do_not_settle_is_tentative_until_the_environment_is_told() {
    let mut a = Automation::new();
    let mut docs = std::collections::HashMap::new();
    let unknown = recover(&mut a, &mut docs, "frac_simple", None);
    assert_eq!((unknown["latex"].as_str(), unknown["confidence"].as_str()), (Some(r"\frac{a + b}{c}"), Some("tentative")), "{unknown}");
    let ambiguous: Vec<&Value> = unknown["diagnostics"].as_array().unwrap().iter().filter(|d| d["severity"] == "ambiguous").collect();
    assert!(ambiguous.len() == 1 && ambiguous[0]["message"].as_str().unwrap().contains("environment"), "{unknown}");
    let display = recover(&mut a, &mut docs, "frac_simple", Some(true));
    assert_eq!((display["latex"].as_str(), display["confidence"].as_str()), (Some(r"\frac{a + b}{c}"), Some("confident")), "{display}");
    // The same kind of glyphs, told it is inline: \dfrac.
    let inline = recover(&mut a, &mut docs, "dfrac_inline", Some(false));
    assert_eq!((inline["latex"].as_str(), inline["confidence"].as_str()), (Some(r"\dfrac{p}{q}"), Some("confident")), "{inline}");
}

/// A region the reader does not handle is a result, not a failure: no LaTeX, and why.
#[test]
fn a_region_that_is_not_read_says_why_and_is_not_guessed() {
    let mut a = Automation::new();
    let mut docs = std::collections::HashMap::new();
    for (id, reason) in [("pmatrix_two", "unsupported"), ("align_two", "unsupported"), ("underbrace", "unsupported"), ("cases_abs", "unsupported")] {
        let r = recover(&mut a, &mut docs, id, Some(true));
        assert_eq!((r["latex"].clone(), r["confidence"].as_str(), r["reason"].as_str()), (Value::Null, Some("none"), Some(reason)), "{id}: {r}");
        assert!(
            r["diagnostics"].as_array().unwrap().iter().any(|d| d["severity"] == "unsupported" && !d["message"].as_str().unwrap().is_empty()),
            "{id}: {r}"
        );
    }
    // A box with nothing in it.
    let doc = open(&mut a, "equations.pdf");
    let r = ok(&mut a, "text_recover_latex", json!({ "doc": doc, "page": 1, "rect": [300, 150, 400, 190] }));
    assert_eq!((r["latex"].clone(), r["reason"].as_str()), (Value::Null, Some("no_glyphs")), "{r}");
}

/// What the tool refuses: a rectangle that is not a box, a page that is not there, a document that is
/// not open; and it changes nothing.
#[test]
fn bad_arguments_are_refused_and_nothing_changes() {
    let mut a = Automation::new();
    let doc = open(&mut a, "equations.pdf");
    let before = ok(&mut a, "doc_list", json!({}));
    for args in [
        json!({ "doc": doc, "page": 1 }),
        json!({ "doc": doc, "page": 1, "rect": [0, 0, 10] }),
        json!({ "doc": doc, "page": 1, "rect": [10, 10, 10, 50] }),
        json!({ "doc": doc, "page": 1, "rect": [10, 10, 50, 10] }),
        json!({ "doc": doc, "page": 1, "rect": [0, 0, 100, "wide"] }),
        json!({ "doc": doc, "page": 1, "rect": [0, 0, 100, 100], "display": "yes" }),
        json!({ "doc": doc, "page": 0, "rect": [0, 0, 100, 100] }),
        json!({ "doc": doc, "page": 10_000, "rect": [0, 0, 100, 100] }),
        json!({ "doc": 99, "page": 1, "rect": [0, 0, 100, 100] }),
    ] {
        assert!(a.call("text_recover_latex", &args).is_err(), "{args}");
    }
    // A rectangle given corner to corner the other way round is the same rectangle.
    let p = place("sqrt_cube");
    let r = [p.rect[2], p.rect[3], p.rect[0], p.rect[1]];
    let flipped = ok(&mut a, "text_recover_latex", json!({ "doc": doc, "page": p.page, "rect": r, "display": true }));
    assert_eq!(flipped["latex"], r"\sqrt[3]{u + v}");
    assert_eq!(ok(&mut a, "doc_list", json!({})), before, "reading changes nothing");
    let def = tools().into_iter().find(|t| t.name == "text_recover_latex").unwrap();
    assert!(def.read_only && !def.destructive);
}

/// In running text the same equation reads the same: justified lines stretch the glue around
/// operators and relations, which is the line's, not the author's.
#[test]
fn an_equation_in_running_text_reads_as_it_does_alone() {
    let truth = std::fs::read_to_string(oracle("truth.tsv")).unwrap();
    let mut a = Automation::new();
    let mut docs = std::collections::HashMap::new();
    let mut compared = 0;
    for line in truth.lines().filter(|l| l.starts_with("E\t") && l.split('\t').nth(1).is_some_and(|id| id.contains('@'))) {
        let id = line.split('\t').nth(1).unwrap();
        let base = id.split('@').next().unwrap();
        let display = Some(line.split('\t').nth(7).unwrap() != "inline");
        let (inside, alone) = (recover(&mut a, &mut docs, id, display), recover(&mut a, &mut docs, base, display));
        if inside["confidence"] == "confident" && alone["confidence"] == "confident" {
            assert_eq!(inside["latex"], alone["latex"], "{id}");
            compared += 1;
        }
    }
    assert!(compared >= 8, "{compared} equations compared");
}

/// `wrap` returns what Edit ▸ Copy as LaTeX puts on the clipboard with that wrapper: the body, `$…$`
/// or `\[…\]`, and null when nothing was read; a wrapper it does not know is refused.
#[test]
fn wrap_returns_the_text_copy_as_latex_copies() {
    let mut a = Automation::new();
    let doc = open(&mut a, "equations.pdf");
    let p = place("sup_squares");
    let call = |a: &mut Automation, wrap: &str| ok(a, "text_recover_latex", json!({ "doc": doc, "page": p.page, "rect": p.rect, "wrap": wrap }));
    let plain = ok(&mut a, "text_recover_latex", json!({ "doc": doc, "page": p.page, "rect": p.rect }));
    assert!(plain.get("text").is_none(), "only asked for: {plain}");
    for (wrap, text) in [("none", r"x^{2} + y^{2} = r^{2}"), ("inline", r"$x^{2} + y^{2} = r^{2}$"), ("display", r"\[x^{2} + y^{2} = r^{2}\]")] {
        let r = call(&mut a, wrap);
        assert_eq!((r["text"].as_str(), r["latex"].as_str()), (Some(text), Some(r"x^{2} + y^{2} = r^{2}")), "{wrap}: {r}");
    }
    let empty = ok(&mut a, "text_recover_latex", json!({ "doc": doc, "page": 1, "rect": [300, 150, 400, 190], "wrap": "inline" }));
    assert_eq!((empty["text"].clone(), empty["latex"].clone()), (Value::Null, Value::Null), "{empty}");
    for wrap in [json!("dollars"), json!(true)] {
        let args = json!({ "doc": doc, "page": p.page, "rect": p.rect, "wrap": wrap });
        assert!(a.call("text_recover_latex", &args).is_err(), "{args}");
    }
}
