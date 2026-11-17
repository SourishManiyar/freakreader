//! `doc_blocks` and `doc_references` end to end, on the reference oracle's papers (TeX-typeset with
//! amsthm, statements and mentions known; `crates/render/tests/data/refs/oracle`).

use pdfcraft_automation::{Automation, Content, ToolError};
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

fn open(a: &mut Automation, paper: &str) -> u64 {
    let path = format!("{}/../render/tests/data/refs/oracle/{paper}.pdf", env!("CARGO_MANIFEST_DIR"));
    ok(a, "doc_open", json!({ "path": path }))["doc"].as_u64().unwrap()
}

#[test]
fn statements_and_proofs_come_with_their_numbers_and_extents() {
    let mut a = Automation::new();
    let doc = open(&mut a, "art11_plain");
    let r = ok(&mut a, "doc_blocks", json!({ "doc": doc, "kinds": ["statement", "proof"] }));
    assert_eq!(r["truncated"], false);
    assert_eq!(r["pages_read"], 3);
    let blocks = r["blocks"].as_array().unwrap();
    let titles: Vec<&str> = blocks.iter().map(|b| b["title"].as_str().unwrap()).collect();
    assert_eq!(
        titles,
        ["Theorem 1.1", "Definition 2.1", "Lemma 2.2", "Proof", "Lemma 2.3", "Proof", "Remark 2.4", "Proof", "Corollary 3.1", "Proof", "Example 3.2"]
    );
    let main = &blocks[0];
    assert_eq!((main["label"].as_str(), main["number"].as_str(), main["name"].as_str()), (Some("Theorem"), Some("1.1"), Some("Main estimate")));
    assert_eq!(main["kind"], "statement");
    // Its extent holds its display: taller than its head.
    let rect = &main["extent"][0]["rect"];
    let head = &main["head"]["rect"];
    assert!(rect[3].as_f64().unwrap() - rect[1].as_f64().unwrap() > 3.0 * (head[3].as_f64().unwrap() - head[1].as_f64().unwrap()), "{main}");
    // Proofs prove: the one after Lemma 2.2, and the one that names Theorem 1.1.
    assert_eq!(blocks[3]["proves"], 2);
    assert_eq!(blocks[7]["proves"], 0);
    assert_eq!(blocks[7]["name"], "of Theorem 1.1");
    assert!(blocks.iter().filter(|b| b["kind"] == "proof").all(|b| b["end_mark"] == "box"));
    // All kinds: the numbered headings, displays and figure join.
    let all = ok(&mut a, "doc_blocks", json!({ "doc": doc }));
    let kinds: Vec<&str> = all["blocks"].as_array().unwrap().iter().map(|b| b["kind"].as_str().unwrap()).collect();
    for k in ["heading", "equation", "float"] {
        assert!(kinds.contains(&k), "{kinds:?}");
    }
}

#[test]
fn mentions_point_at_their_blocks_or_say_why_not() {
    let mut a = Automation::new();
    let doc = open(&mut a, "art10_traps");
    let r = ok(&mut a, "doc_references", json!({ "doc": doc }));
    let mentions = r["mentions"].as_array().unwrap();
    let find = |text: &str| mentions.iter().find(|m| m["text"] == text).unwrap_or_else(|| panic!("{text}: {mentions:#?}"));
    // Lemma 1 and Theorem 1 share a number, not a target.
    assert_eq!(find("Lemma 1")["target_title"], "Lemma 1");
    assert_eq!(find("Theorem 1")["target_title"], "Theorem 1");
    assert_eq!(find("Lemma 1")["source"], "inferred");
    assert!(find("Lemma 1")["confidence"].as_f64().unwrap() >= 0.7);
    let missing = find("Proposition 3");
    assert_eq!((missing["target"].is_null(), missing["unresolved"].as_str()), (true, Some("missing")));
    assert!(mentions.iter().any(|m| m["unresolved"] == "external"));
    // Filters: resolved ones only, a page, a source.
    let resolved = ok(&mut a, "doc_references", json!({ "doc": doc, "resolved": true, "min_confidence": 0.7 }));
    assert!(resolved["mentions"].as_array().unwrap().iter().all(|m| !m["target"].is_null()));
    let page2 = ok(&mut a, "doc_references", json!({ "doc": doc, "page": 2 }));
    assert!(page2["mentions"].as_array().unwrap().iter().all(|m| m["page"] == 2));
    assert!(ok(&mut a, "doc_references", json!({ "doc": doc, "source": "link" }))["mentions"].as_array().unwrap().is_empty());
}

#[test]
fn links_are_the_source_where_the_paper_has_them() {
    let mut a = Automation::new();
    let doc = open(&mut a, "art11_hyper");
    let r = ok(&mut a, "doc_references", json!({ "doc": doc, "source": "link" }));
    let mentions = r["mentions"].as_array().unwrap();
    assert_eq!(mentions.len(), 9);
    assert!(mentions.iter().all(|m| m["link"]["index"].is_u64() && m["confidence"] == 1.0));
    // The one place the text reads otherwise (another work's theorem, set with \ref) is said.
    let d = r["diagnostics"].as_array().unwrap();
    assert_eq!(d.iter().filter(|d| d["code"] == "link_disagrees").count(), 1, "{d:?}");
}

#[test]
fn bad_arguments_say_what_is_allowed() {
    let mut a = Automation::new();
    let doc = open(&mut a, "art10_traps");
    for (tool, args, says) in [
        ("doc_blocks", json!({ "doc": doc, "kinds": ["lemma"] }), "statement, proof, heading, equation, float"),
        ("doc_blocks", json!({ "doc": doc, "kinds": "statement" }), "an array of strings"),
        ("doc_references", json!({ "doc": doc, "source": "both" }), "all, link or inferred"),
        ("doc_references", json!({ "doc": doc, "min_confidence": 2 }), "from 0 to 1"),
        ("doc_references", json!({ "doc": doc, "page": 0 }), "1 or more"),
        ("doc_references", json!({ "doc": doc, "page": 99 }), "was not read"),
    ] {
        match a.call(tool, &args) {
            Err(ToolError::InvalidArgs(m)) => assert!(m.contains(says), "{tool} {args}: {m}"),
            other => panic!("{tool} {args}: {other:?}"),
        }
    }
    assert!(a.call("doc_blocks", &json!({ "doc": 999 })).is_err());
}
