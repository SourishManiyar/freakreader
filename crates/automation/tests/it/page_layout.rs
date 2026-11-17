//! `page_layout` and `doc_sections` end to end, on the layout oracle's papers (TeX-typeset, with
//! known structure; `crates/render/tests/data/layout/oracle`).

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
    let path = format!("{}/../render/tests/data/layout/oracle/{paper}.pdf", env!("CARGO_MANIFEST_DIR"));
    ok(a, "doc_open", json!({ "path": path }))["doc"].as_u64().unwrap()
}

fn kinds(page: &Value) -> Vec<String> {
    let blocks = page["blocks"].as_array().unwrap();
    page["reading_order"].as_array().unwrap().iter().map(|i| blocks[i.as_u64().unwrap() as usize]["kind"].as_str().unwrap().to_owned()).collect()
}

#[test]
fn a_two_column_page_reads_title_then_left_column_then_right() {
    let mut a = Automation::new();
    let doc = open(&mut a, "art11_twocol");
    let r = ok(&mut a, "page_layout", json!({ "doc": doc, "pages": [1] }));
    let pages = r["pages"].as_array().unwrap();
    assert_eq!(pages.len(), 1);
    let p = &pages[0];
    assert_eq!(p["page"], 1);
    let k = kinds(p);
    assert_eq!(k.first().map(String::as_str), Some("title"), "{k:?}");
    assert!(k.iter().any(|k| k == "heading") && k.iter().any(|k| k == "display_equation") && k.iter().any(|k| k == "footnote"), "{k:?}");
    assert_eq!(k.last().map(String::as_str), Some("page_number"), "{k:?}");
    let columns = p["columns"].as_array().unwrap();
    assert!(columns.iter().any(|c| c["of"] == 2), "{columns:?}");
    // Every block once in the order, and each knows its place; left-column blocks come before right.
    let blocks = p["blocks"].as_array().unwrap();
    let order: Vec<usize> = p["reading_order"].as_array().unwrap().iter().map(|v| v.as_u64().unwrap() as usize).collect();
    let mut sorted = order.clone();
    sorted.sort_unstable();
    assert_eq!(sorted, (0..blocks.len()).collect::<Vec<_>>());
    let col = |i: usize| blocks[i]["column"].as_u64().map(|c| columns[c as usize]["index"].as_u64().unwrap());
    let sides: Vec<u64> = order.iter().filter_map(|&i| col(i)).filter(|_| true).collect();
    let first_right = sides.iter().position(|&c| c == 1).unwrap();
    assert!(sides[first_right..].iter().all(|&c| c == 1), "{sides:?}");
    for (place, &i) in order.iter().enumerate() {
        assert_eq!(blocks[i]["order"], place);
        let c = blocks[i]["confidence"].as_f64().unwrap();
        assert!((0.0..=1.0).contains(&c));
    }
    let heading = blocks.iter().find(|b| b["kind"] == "heading").unwrap();
    assert_eq!(heading["heading"]["number"], "1");
    assert_eq!(heading["heading"]["level"], 1);
    assert!(heading["text"].as_str().unwrap().contains("Introduction"));
    let display = blocks.iter().find(|b| b["kind"] == "display_equation").unwrap();
    assert_eq!(display["equation_tag"], "(1)");
    // Text can be left out, lines asked for.
    let lean = ok(&mut a, "page_layout", json!({ "doc": doc, "pages": [1], "text": false, "lines": true }));
    let b0 = &lean["pages"][0]["blocks"][0];
    assert!(b0.get("text").is_none());
    assert!(!b0["lines"][0]["glyphs"].as_array().unwrap().is_empty());
}

#[test]
fn the_section_tree_has_levels_parents_and_the_appendix() {
    let mut a = Automation::new();
    let doc = open(&mut a, "art12_heads");
    let r = ok(&mut a, "doc_sections", json!({ "doc": doc }));
    assert_eq!(r["truncated"], false);
    assert_eq!(r["pages_read"], 3);
    let s = r["sections"].as_array().unwrap();
    let summary: Vec<(u64, Option<&str>, bool)> =
        s.iter().map(|n| (n["level"].as_u64().unwrap(), n["number"].as_str(), n["appendix"].as_bool().unwrap())).collect();
    assert_eq!(
        summary,
        [
            (1, Some("1"), false),
            (2, Some("1.1"), false),
            (3, Some("1.1.1"), false),
            (3, Some("1.1.2"), false),
            (1, Some("2"), false),
            (1, Some("A"), true),
            (2, Some("A.1"), true),
        ]
    );
    let parents: Vec<Option<u64>> = s.iter().map(|n| n["parent"].as_u64()).collect();
    assert_eq!(parents, [None, Some(0), Some(1), Some(1), None, None, Some(5)]);
    assert_eq!(s[0]["title"], "Setting");
    assert_eq!(s[5]["title"], "Auxiliary estimates");
    // The running head repeats on pages 2 and 3 and is confirmed there.
    let running = r["running"].as_array().unwrap();
    assert!(running.iter().any(|x| x["kind"] == "running_header" && x["page"] == 2 && x["confidence"].as_f64().unwrap() > 0.9), "{running:?}");
}

#[test]
fn a_page_without_text_is_an_empty_layout_that_says_so() {
    let mut a = Automation::new();
    let doc = open(&mut a, "art10_note");
    ok(&mut a, "page_insert_blank", json!({ "doc": doc, "at": 2 }));
    let r = ok(&mut a, "page_layout", json!({ "doc": doc, "pages": [2] }));
    let p = &r["pages"][0];
    assert!(p["blocks"].as_array().unwrap().is_empty());
    assert!(p["diagnostics"].as_array().unwrap().iter().any(|d| d["code"] == "no_text"), "{p}");
    // The unsaved edit is what was read: page 1 still has its text.
    let one = ok(&mut a, "page_layout", json!({ "doc": doc }));
    assert_eq!(one["pages"].as_array().unwrap().len(), 2);
    assert!(!one["pages"][0]["blocks"].as_array().unwrap().is_empty());
}

#[test]
fn bad_arguments_are_refused_and_say_what_is_allowed() {
    let mut a = Automation::new();
    let doc = open(&mut a, "art10_note");
    for (args, why) in [
        (json!({ "doc": doc, "pages": [0] }), ""),
        (json!({ "doc": doc, "pages": [2] }), "out of range"),
        (json!({ "doc": doc, "pages": [] }), "at least one"),
        (json!({ "doc": doc, "text": "yes" }), ""),
        (json!({ "doc": doc, "colour": true }), ""),
        (json!({ "doc": 999 }), ""),
    ] {
        match a.call("page_layout", &args) {
            Err(ToolError::InvalidArgs(m)) | Err(ToolError::Failed(m)) => assert!(m.contains(why), "{args}: {m}"),
            Err(e) => assert!(why.is_empty(), "{args}: {e}"),
            Ok(_) => panic!("{args} is refused"),
        }
    }
    assert!(a.call("doc_sections", &json!({ "doc": doc, "pages": [1] })).is_err());
}
