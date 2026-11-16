//! The reference preview's tools end to end on a hyperref paper: `link_list` names each link's
//! destination, `dest_list` lists them all, and `link_preview` shows the block one names, by link
//! or by name.

use pdfcraft_automation::{Automation, Content, ToolError};
use serde_json::{Value, json};

fn call(a: &mut Automation, tool: &str, args: Value) -> Vec<Content> {
    a.call(tool, &args).unwrap_or_else(|e| panic!("{tool} {args}: {e}"))
}

fn ok(a: &mut Automation, tool: &str, args: Value) -> Value {
    match call(a, tool, args).remove(0) {
        Content::Json(v) => v,
        other => panic!("{tool}: expected JSON, got {other:?}"),
    }
}

/// `link_preview`'s place: the JSON after its picture.
fn preview(a: &mut Automation, args: Value) -> Value {
    let out = call(a, "link_preview", args);
    assert!(matches!(out.first(), Some(Content::Png { .. })), "a picture first");
    match out.last() {
        Some(Content::Json(v)) => v.clone(),
        other => panic!("expected the place, got {other:?}"),
    }
}

fn open(a: &mut Automation, name: &str) -> u64 {
    let path = format!("{}/../render/tests/data/hyperref/{name}.pdf", env!("CARGO_MANIFEST_DIR"));
    ok(a, "doc_open", json!({ "path": path }))["doc"].as_u64().unwrap()
}

#[test]
fn link_list_names_each_links_destination() {
    let mut a = Automation::new();
    let doc = open(&mut a, "onecol");
    let links = ok(&mut a, "link_list", json!({ "doc": doc }));
    let named: Vec<&Value> = links["links"].as_array().unwrap().iter().filter(|l| l["dest_name"] == "equation.2.2").collect();
    assert!(!named.is_empty(), "{links}");
    for l in named {
        assert_eq!(
            (l["named"].clone(), l["dest_kind"].clone(), l["dest_number"].clone(), l["to_page"].clone()),
            (json!(true), json!("equation"), json!("2.2"), json!(1)),
            "{l}"
        );
        assert_eq!(l["destination"]["fit"], "XYZ", "{l}");
    }
    let cite = links["links"].as_array().unwrap().iter().find(|l| l["dest_name"] == "cite.alpha").expect("a citation");
    assert_eq!((cite["dest_kind"].clone(), cite.get("dest_number")), (json!("citation"), None));
    // Every link of the paper goes through a name.
    assert!(links["links"].as_array().unwrap().iter().all(|l| l["dest_name"].is_string()), "{links}");
}

#[test]
fn dest_list_lists_named_destinations_in_page_order() {
    let mut a = Automation::new();
    let doc = open(&mut a, "onecol");
    let all = ok(&mut a, "dest_list", json!({ "doc": doc }));
    let names: Vec<&str> = all["destinations"].as_array().unwrap().iter().map(|d| d["name"].as_str().unwrap()).collect();
    assert_eq!((all["count"].clone(), all["left_out"].clone(), all["unresolved"].clone()), (json!(names.len()), json!(0), json!(0)));
    let at = |n: &str| names.iter().position(|x| *x == n).unwrap_or_else(|| panic!("{n} in {names:?}"));
    // Page order, then top to bottom.
    assert!(
        at("page.1") < at("theorem.2.3")
            && at("theorem.2.3") < at("equation.2.2")
            && at("equation.2.2") < at("figure.1")
            && at("figure.1") < at("cite.gamma")
    );
    let theorem = &all["destinations"][at("theorem.2.1")];
    assert_eq!(
        (theorem["kind"].clone(), theorem["number"].clone(), theorem["page"].clone(), theorem["page_label"].clone()),
        (json!("theorem_like"), json!("2.1"), json!(1), json!("1"))
    );
    assert_eq!(all["destinations"][at("section*.1")].get("number"), None, "an unnumbered heading");
    // Paged.
    let first = ok(&mut a, "dest_list", json!({ "doc": doc, "limit": 3 }));
    assert_eq!((first["destinations"].as_array().unwrap().len(), first["next_from"].clone()), (3, json!(4)));
    let rest = ok(&mut a, "dest_list", json!({ "doc": doc, "from": 4, "limit": 10000 }));
    assert_eq!(rest["destinations"][0]["name"], all["destinations"][3]["name"]);
    assert!(rest.get("next_from").is_none());
    for bad in [json!({ "doc": doc, "limit": 0 }), json!({ "doc": doc, "limit": 10001 }), json!({ "doc": doc, "from": 0 })] {
        assert!(matches!(a.call("dest_list", &bad), Err(ToolError::InvalidArgs(_))), "{bad}");
    }
}

#[test]
fn link_preview_shows_the_block_a_destination_names() {
    let mut a = Automation::new();
    for name in ["onecol", "twocol"] {
        let doc = open(&mut a, name);
        let place = preview(&mut a, json!({ "doc": doc, "dest": "equation.3.2", "dpi": 72 }));
        assert_eq!(
            (place["kind"].clone(), place["number"].clone(), place["fitted"].clone()),
            (json!("equation"), json!("3.2"), json!(true)),
            "{place}"
        );
        assert!(place["caption"].as_str().unwrap().starts_with("Equation 3.2 · page "), "{place}");
        // The align's two rows: about 70 points tall, not the band's 245.
        let area: Vec<f64> = place["area"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
        assert!(area[3] - area[1] < 90.0 && area[3] - area[1] > 50.0, "{name}: {area:?}");
        // fit: false keeps the band.
        let band = preview(&mut a, json!({ "doc": doc, "dest": "equation.3.2", "fit": false, "dpi": 72 }));
        assert_eq!(band["fitted"], false);
        assert!(band["area"][3].as_f64().unwrap() - band["area"][1].as_f64().unwrap() > 200.0, "{band}");
        // By link, as the viewer finds it.
        let links = ok(&mut a, "link_list", json!({ "doc": doc }));
        let link = links["links"].as_array().unwrap().iter().find(|l| l["dest_name"] == "cite.beta").expect("a link to cite.beta");
        let by_link = preview(&mut a, json!({ "doc": doc, "page": link["page"], "index": link["index"], "dpi": 72 }));
        let by_name = preview(&mut a, json!({ "doc": doc, "dest": "cite.beta", "dpi": 72 }));
        assert_eq!(
            (by_link["area"].clone(), by_link["kind"].clone(), by_link["name"].clone()),
            (by_name["area"].clone(), json!("citation"), json!("cite.beta"))
        );
        // A page destination keeps its band, and says so.
        let page = preview(&mut a, json!({ "doc": doc, "dest": "page.1", "dpi": 72 }));
        assert_eq!((page["fitted"].clone(), page["kind"].clone(), page["caption"].clone()), (json!(false), json!("page"), json!("Page 1")));
    }
}

#[test]
fn link_preview_says_what_is_wrong_with_a_name() {
    let mut a = Automation::new();
    let doc = open(&mut a, "onecol");
    for args in [
        json!({ "doc": doc, "dest": "equation.9.9" }),
        json!({ "doc": doc, "dest": "equation.2.1", "page": 1, "index": 1 }),
        json!({ "doc": doc }),
        json!({ "doc": doc, "dest": "x".repeat(100_000) }),
    ] {
        match a.call("link_preview", &args) {
            Err(ToolError::InvalidArgs(m)) => assert!(!m.is_empty()),
            other => panic!("{args}: {other:?}"),
        }
    }
}
