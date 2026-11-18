//! `doc_blocks`, `doc_references` and `doc_bibliography` end to end, on the reference oracle's papers
//! (TeX-typeset with amsthm, statements and mentions known; `crates/render/tests/data/refs/oracle`)
//! and the bibliography oracle's (BibTeX lists and citations known;
//! `crates/render/tests/data/bib/oracle`).

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
        ("doc_blocks", json!({ "doc": doc, "kinds": ["lemma"] }), "statement, proof, heading, equation, float, bib_entry"),
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

fn open_bib(a: &mut Automation, paper: &str) -> u64 {
    let path = format!("{}/../render/tests/data/bib/oracle/{paper}.pdf", env!("CARGO_MANIFEST_DIR"));
    ok(a, "doc_open", json!({ "path": path }))["doc"].as_u64().unwrap()
}

#[test]
fn the_bibliography_comes_with_its_entries_and_fields() {
    let mut a = Automation::new();
    let doc = open_bib(&mut a, "art11_plainnat");
    let r = ok(&mut a, "doc_bibliography", json!({ "doc": doc, "citations": true }));
    assert_eq!((r["found"].as_bool(), r["region"]["heading"].as_str()), (Some(true), Some("References")));
    let entries = r["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 30);
    let first = &entries[0];
    assert_eq!((first["label"].as_str(), first["printed_label"].as_str(), first["label_style"].as_str()), (Some("1"), Some("[1]"), Some("bracket")));
    assert!(first["fields"]["authors"].as_array().is_some_and(|a| !a.is_empty() && a[0]["family"].is_string()));
    assert!(first["fields"]["year"]["value"].is_string() && first["fields"]["title"]["confidence"].as_f64().unwrap() > 0.5);
    assert!(entries.iter().any(|e| e["fields"]["doi"]["value"].as_str().is_some_and(|d| d.starts_with("10.5555/"))), "plainnat prints DOIs");
    assert!(entries.iter().all(|e| e["parsed"] == true && e["extent"].as_array().is_some_and(|x| !x.is_empty())));
    // Every citation names an entry; the entries say how often they are cited.
    let cites = r["citations"].as_array().unwrap();
    assert_eq!(cites.len() as u64, r["citation_count"].as_u64().unwrap());
    assert!(cites.len() >= 15 && cites.iter().all(|c| c["kind"] == "citation" && c["entry"].is_u64()));
    let cited: u64 = entries.iter().map(|e| e["cited"].as_u64().unwrap()).sum();
    assert_eq!(cited as usize, cites.len());
    // One entry, and its citations.
    let one = ok(&mut a, "doc_bibliography", json!({ "doc": doc, "entry": 0, "citations": true }));
    assert_eq!(one["entries"].as_array().unwrap().len(), 1);
    assert!(one["citations"].as_array().unwrap().iter().all(|c| c["entry"] == 0));
    // An author-year list: no labels, years with their suffixes, entries called by author and year.
    let doc = open_bib(&mut a, "art12_apalike");
    let r = ok(&mut a, "doc_bibliography", json!({ "doc": doc }));
    let entries = r["entries"].as_array().unwrap();
    assert!(entries.iter().all(|e| e["label"].is_null() && e["label_style"] == "none"));
    assert!(entries.iter().any(|e| e["fields"]["year"]["value"] == "2020a"));
    assert!(entries.iter().any(|e| e["title"].as_str().is_some_and(|t| t.contains("Okafor 2020"))));
}

#[test]
fn citations_come_through_doc_references() {
    let mut a = Automation::new();
    let doc = open_bib(&mut a, "art10_plain");
    let r = ok(&mut a, "doc_references", json!({ "doc": doc }));
    let cites: Vec<&Value> = r["mentions"].as_array().unwrap().iter().filter(|m| m["kind"] == "citation").collect();
    assert!(cites.len() >= 20);
    assert!(cites.iter().all(|c| c["source"] == "inferred" && c["target_title"].as_str().is_some_and(|t| t.starts_with('['))));
    assert!(cites.iter().any(|c| c["locator"].as_str().is_some_and(|l| l.contains("Theorem"))));
    // citations: false gives the mentions of blocks alone.
    let without = ok(&mut a, "doc_references", json!({ "doc": doc, "citations": false }));
    assert!(without["mentions"].as_array().unwrap().iter().all(|m| m["kind"] != "citation"));
    // Entries are blocks only when asked for.
    let blocks = ok(&mut a, "doc_blocks", json!({ "doc": doc }));
    assert!(blocks["blocks"].as_array().unwrap().iter().all(|b| b["kind"] != "bib_entry"));
    let entries = ok(&mut a, "doc_blocks", json!({ "doc": doc, "kinds": ["bib_entry"] }));
    assert_eq!(entries["blocks"].as_array().unwrap().len(), 28);
}

#[test]
fn a_paper_without_a_list_says_so() {
    let mut a = Automation::new();
    // The statement oracle's papers cite other works with literal brackets, and list none.
    let doc = open(&mut a, "art10_traps");
    let r = ok(&mut a, "doc_bibliography", json!({ "doc": doc, "citations": true }));
    assert_eq!((r["found"].as_bool(), r["entries"].as_array().map(Vec::len)), (Some(false), Some(0)));
    assert!(r["diagnostics"].as_array().unwrap().iter().any(|d| d["code"] == "no_bibliography"));
    assert!(r["citations"].as_array().unwrap().is_empty());
    for (args, says) in [
        (json!({ "doc": doc, "entry": -1 }), "0 or more"),
        (json!({ "doc": doc, "entry": 3 }), "does not exist"),
        (json!({ "doc": doc, "citations": "yes" }), "true or false"),
    ] {
        match a.call("doc_bibliography", &args) {
            Err(ToolError::InvalidArgs(m)) => assert!(m.contains(says), "{args}: {m}"),
            other => panic!("{args}: {other:?}"),
        }
    }
}
