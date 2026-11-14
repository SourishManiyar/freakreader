//! The outline as a list and Create Table of Contents through the tools: `bookmark_rows`,
//! `bookmark_edit_rows` and `bookmark_create_toc`.

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

fn failure(a: &mut Automation, tool: &str, args: Value) -> String {
    match a.call(tool, &args) {
        Err(ToolError::InvalidArgs(m) | ToolError::Failed(m)) => m,
        other => panic!("{tool} {args}: expected an error, got {other:?}"),
    }
}

/// A PDF of `pages` content streams, Helvetica as /F1 and Helvetica-Bold as /F2, 612 × 792.
fn pdf(pages: &[String]) -> Vec<u8> {
    let n = pages.len();
    let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", 5 + 2 * i)).collect();
    let mut objs = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        format!("<< /Type /Pages /Kids [{}] /Count {n} /MediaBox [0 0 612 792] /Resources << /Font << /F1 3 0 R /F2 4 0 R >> >> >>", kids.join(" ")),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".into(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica-Bold /Encoding /WinAnsiEncoding >>".into(),
    ];
    for (i, content) in pages.iter().enumerate() {
        objs.push(format!("<< /Type /Page /Parent 2 0 R /Contents {} 0 R >>", 6 + 2 * i));
        objs.push(format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()));
    }
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

fn text(font: &str, size: u32, x: u32, y: u32, s: &str) -> String {
    format!("BT /{font} {size} Tf {x} {y} Td ({s}) Tj ET ")
}

/// A book: title page, a contents page over two columns of the page (title left, number far
/// right), front matter printed ii, chapters printed 1–3 from the fourth page on.
fn book() -> Vec<u8> {
    let entry = |y: u32, title: &str, page: &str, x: u32| format!("{}{}", text("F1", 11, x, y, title), text("F1", 11, 520, y, page));
    let mut contents = text("F2", 18, 72, 720, "Table of Contents");
    contents += &entry(680, "Foreword", "ii", 72);
    contents += &entry(664, "1 Getting started", "1", 72);
    contents += &entry(648, "1.1 Installing the tools", "2", 90);
    contents += &entry(632, "2 Using them", "3", 72);
    let chapter = |heading: &str, folio: &str| {
        format!("{}{}{}", text("F2", 16, 72, 700, heading), text("F1", 10, 72, 670, "Body text."), text("F1", 9, 300, 40, folio))
    };
    pdf(&[
        text("F2", 24, 72, 600, "Manual"),
        contents,
        chapter("Foreword", "ii"),
        chapter("1 Getting started", "1"),
        chapter("1.1 Installing the tools", "2"),
        chapter("2 Using them", "3"),
    ])
}

fn setup(test: &str, files: &[(&str, Vec<u8>)]) -> (std::path::PathBuf, Automation) {
    let dir = std::env::temp_dir().join(format!("pdfcraft-automation-bookmark-rows-{test}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for (name, bytes) in files {
        std::fs::write(dir.join(name), bytes).unwrap();
    }
    let a = Automation::new().with_root(&dir).unwrap().with_clock(|| 1_700_000_000);
    (dir, a)
}

#[test]
fn create_toc_reports_then_applies_the_contents_as_one_step() {
    let (_dir, mut a) = setup("toc", &[("book.pdf", book())]);
    let doc = ok(&mut a, "doc_open", json!({ "path": "book.pdf" }))["doc"].as_u64().unwrap();
    let found = ok(&mut a, "bookmark_create_toc", json!({ "doc": doc }));
    assert_eq!(found["source"], "contents");
    assert_eq!(found["toc_pages"], json!([2]));
    type Entry<'a> = (u64, &'a str, Option<u64>, &'a str, bool);
    let entries: Vec<Entry<'_>> = found["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            (
                e["level"].as_u64().unwrap(),
                e["title"].as_str().unwrap(),
                e["page"].as_u64(),
                e["printed"].as_str().unwrap(),
                e["verified"].as_bool().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        entries,
        [
            (1, "Foreword", Some(3), "ii", true),
            (1, "1 Getting started", Some(4), "1", true),
            (2, "1.1 Installing the tools", Some(5), "2", true),
            (1, "2 Using them", Some(6), "3", true)
        ]
    );
    let top = found["entries"][1]["top"].as_f64().unwrap();
    assert!((70.0..100.0).contains(&top), "points from the top of the page to the heading: {top}");
    assert_eq!(ok(&mut a, "bookmark_rows", json!({ "doc": doc }))["rows"], json!([]), "only reported");

    ok(&mut a, "bookmark_add", json!({ "doc": doc, "title": "Kept", "page": 1 }));
    let applied = ok(&mut a, "bookmark_create_toc", json!({ "doc": doc, "apply": true, "keep": true }));
    assert_eq!(applied["source"], "contents");
    let rows = ok(&mut a, "bookmark_rows", json!({ "doc": doc }))["rows"].clone();
    let titles: Vec<(u64, &str)> = rows.as_array().unwrap().iter().map(|r| (r["level"].as_u64().unwrap(), r["title"].as_str().unwrap())).collect();
    assert_eq!(titles, [(1, "Kept"), (1, "Foreword"), (1, "1 Getting started"), (2, "1.1 Installing the tools"), (1, "2 Using them")]);
    assert_eq!(rows[2]["destination"]["fit"], "XYZ");
    assert_eq!(ok(&mut a, "edit_undo", json!({ "doc": doc }))["undone"], "Create table of contents");
    let rows = ok(&mut a, "bookmark_rows", json!({ "doc": doc }))["rows"].clone();
    assert_eq!(rows.as_array().unwrap().len(), 1);

    assert!(failure(&mut a, "bookmark_create_toc", json!({ "doc": doc, "keep": true })).contains("keep goes with apply"));
    let (_dir2, mut b) = setup("toc-plain", &[("plain.pdf", pdf(&["".into(), "".into()]))]);
    let plain = ok(&mut b, "doc_open", json!({ "path": "plain.pdf" }))["doc"].as_u64().unwrap();
    assert!(failure(&mut b, "bookmark_create_toc", json!({ "doc": plain })).contains("no table of contents, tagged headings or headings were found"));
}

#[test]
fn rows_rewrite_the_outline_keeping_what_they_do_not_change() {
    let (_dir, mut a) = setup("rows", &[("book.pdf", book())]);
    let doc = ok(&mut a, "doc_open", json!({ "path": "book.pdf" }))["doc"].as_u64().unwrap();
    ok(&mut a, "bookmark_add", json!({ "doc": doc, "title": "Alpha", "page": 2, "top": 100 }));
    ok(&mut a, "bookmark_add", json!({ "doc": doc, "title": "Beta", "page": 3 }));
    ok(&mut a, "bookmark_set_style", json!({ "doc": doc, "path": [1], "color": [1, 0, 0], "bold": true }));
    let rows = ok(&mut a, "bookmark_rows", json!({ "doc": doc }))["rows"].clone();
    assert_eq!(rows[0]["path"], json!([1]));
    assert_eq!(rows[0]["color"], json!([1.0, 0.0, 0.0]));
    assert_eq!(rows[0]["destination"]["top"], json!(100.0));

    // Beta first and renamed, Alpha nested under it keeping its style and destination, a new child.
    let r = ok(
        &mut a,
        "bookmark_edit_rows",
        json!({ "doc": doc, "rows": [
            { "level": 1, "title": "Beta renamed", "from": [2] },
            { "level": 2, "title": "Alpha", "from": [1] },
            { "level": 2, "title": "New", "page": 6, "top": 50 },
        ] }),
    );
    assert_eq!(r["rows"], 3);
    let rows = ok(&mut a, "bookmark_rows", json!({ "doc": doc }))["rows"].clone();
    let got: Vec<(u64, &str, u64)> =
        rows.as_array().unwrap().iter().map(|r| (r["level"].as_u64().unwrap(), r["title"].as_str().unwrap(), r["page"].as_u64().unwrap())).collect();
    assert_eq!(got, [(1, "Beta renamed", 3), (2, "Alpha", 2), (2, "New", 6)]);
    assert_eq!(
        (rows[1]["color"].clone(), rows[1]["bold"].clone(), rows[1]["destination"]["top"].clone()),
        (json!([1.0, 0.0, 0.0]), json!(true), json!(100.0))
    );
    assert_eq!(rows[2]["destination"]["top"], json!(50.0));
    assert_eq!(ok(&mut a, "edit_undo", json!({ "doc": doc }))["undone"], "Edit bookmarks");
    assert_eq!(ok(&mut a, "bookmark_rows", json!({ "doc": doc }))["rows"].as_array().unwrap().len(), 2);

    // Refusals name the row and change nothing.
    for (rows, why) in [
        (json!([{ "level": 2, "title": "x" }]), "row 1: the first bookmark must be at the top level"),
        (json!([{ "level": 1, "title": "x" }, { "level": 3, "title": "y" }]), "row 2: a bookmark can be at most one level deeper"),
        (json!([{ "level": 1, "title": "x", "page": 99 }]), "row 1: page 99 is past the last page (6)"),
        (json!([{ "level": 1, "title": "x", "from": [9] }]), "row 1: there is no bookmark at [9]"),
        (
            json!([{ "level": 1, "title": "x", "from": [1] }, { "level": 1, "title": "y", "from": [1] }]),
            "row 2: another row already stands for the same bookmark",
        ),
        (json!([{ "level": 1, "title": "x", "top": 5 }]), "row 1: top and left go with a page"),
        (json!([{ "level": 1, "title": "x", "colour": 1 }]), "row 1: unknown key \"colour\""),
        (json!([{ "level": 1, "title": " " }]), "row 1: the title must not be empty"),
        (json!([{ "level": 0, "title": "x" }]), "row 1: level must be 1 or more"),
    ] {
        let m = failure(&mut a, "bookmark_edit_rows", json!({ "doc": doc, "rows": rows }));
        assert!(m.contains(why), "{m:?} should say {why:?}");
    }
    assert_eq!(ok(&mut a, "bookmark_rows", json!({ "doc": doc }))["rows"].as_array().unwrap().len(), 2);
    assert!(failure(&mut a, "bookmark_edit_rows", json!({ "doc": doc })).contains("give either rows or text"));
    assert!(failure(&mut a, "bookmark_edit_rows", json!({ "doc": doc, "rows": [], "keep": true })).contains("keep and printed go with text"));

    // An empty list removes every bookmark.
    ok(&mut a, "bookmark_edit_rows", json!({ "doc": doc, "rows": [] }));
    assert_eq!(ok(&mut a, "bookmark_rows", json!({ "doc": doc }))["rows"], json!([]));
}

#[test]
fn a_pasted_outline_becomes_bookmarks_by_page_number_or_printed_number() {
    let (_dir, mut a) = setup("text", &[("book.pdf", book())]);
    let doc = ok(&mut a, "doc_open", json!({ "path": "book.pdf" }))["doc"].as_u64().unwrap();
    let outline = "Start\t1\n  Inner ..... 2\n  Deep 3\nEnd";
    ok(&mut a, "bookmark_edit_rows", json!({ "doc": doc, "text": outline }));
    let rows = ok(&mut a, "bookmark_rows", json!({ "doc": doc }))["rows"].clone();
    let got: Vec<(u64, &str, Option<u64>)> =
        rows.as_array().unwrap().iter().map(|r| (r["level"].as_u64().unwrap(), r["title"].as_str().unwrap(), r["page"].as_u64())).collect();
    assert_eq!(got, [(1, "Start", Some(1)), (2, "Inner", Some(2)), (2, "Deep", Some(3)), (1, "End", None)]);

    // Printed numbers, after the bookmarks already there: mapped like the contents are.
    let printed = "Foreword ..... ii\n1 Getting started 1\n  1.1 Installing the tools 2\nGhost 40";
    let r = ok(&mut a, "bookmark_edit_rows", json!({ "doc": doc, "text": printed, "printed": true, "keep": true }));
    assert_eq!(r["unplaced"], json!([4]), "printed 40 is not in the book");
    let rows = ok(&mut a, "bookmark_rows", json!({ "doc": doc }))["rows"].clone();
    let got: Vec<(&str, Option<u64>)> = rows.as_array().unwrap().iter().skip(4).map(|r| (r["title"].as_str().unwrap(), r["page"].as_u64())).collect();
    assert_eq!(got, [("Foreword", Some(3)), ("1 Getting started", Some(4)), ("1.1 Installing the tools", Some(5)), ("Ghost", None)]);

    assert!(failure(&mut a, "bookmark_edit_rows", json!({ "doc": doc, "text": "Too far 70" })).contains("row 1: page 70 is past the last page (6)"));
    assert!(failure(&mut a, "bookmark_edit_rows", json!({ "doc": doc, "text": "Roman ..... xii" })).contains("no page is labelled \"xii\""));
    assert!(failure(&mut a, "bookmark_edit_rows", json!({ "doc": doc, "text": "\n\n  \n" })).contains("text has no outline lines"));
}
