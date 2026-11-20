//! `page_split_spreads` end to end: a synthetic scanned spread split by the gutter it finds, both
//! halves rendered with the renderer `page_render` uses, undo, and arguments it refuses.

use pdfcraft_automation::{Automation, Content, ToolError};
use serde_json::{Value, json};

const SW: usize = 396;
const SH: usize = 306;

/// A grey scan of two book pages: lines of word-like dashes in a left column ending at 42% of the
/// width and a right column starting at 48%, blank between.
fn scan() -> Vec<u8> {
    let mut g = vec![255u8; SW * SH];
    for y in 0..SH {
        for x in 0..SW {
            let f = x as f64 / SW as f64;
            let text_row = y > SH / 10 && y < SH * 9 / 10 && y % 9 < 3;
            if text_row && ((f > 0.08 && f < 0.42) || (f > 0.48 && f < 0.92)) && (x / 5) % 6 != 5 {
                g[y * SW + x] = 20;
            }
        }
    }
    g
}

/// Page 1: the 792 × 612 spread. Page 2: a portrait page, which must be left alone.
fn fixture() -> Vec<u8> {
    let hex: String = scan().iter().map(|b| format!("{b:02x}")).collect::<String>() + ">";
    let draw = "q 792 0 0 612 0 0 cm /Im0 Do Q";
    let objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R 6 0 R] /Count 2 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 792 612] /Contents 4 0 R /Resources << /XObject << /Im0 5 0 R >> >> >>".into(),
        format!("<< /Length {} >>\nstream\n{draw}\nendstream", draw.len()),
        format!(
            "<< /Type /XObject /Subtype /Image /Width {SW} /Height {SH} /ColorSpace /DeviceGray /BitsPerComponent 8 /Filter /ASCIIHexDecode /Length {} >>\nstream\n{hex}\nendstream",
            hex.len()
        ),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>".into(),
    ];
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

fn ok(a: &mut Automation, tool: &str, args: Value) -> Value {
    match a.call(tool, &args) {
        Ok(mut c) => match c.remove(0) {
            Content::Json(v) => v,
            other => panic!("{tool}: expected JSON, got {other:?}"),
        },
        Err(e) => panic!("{tool} {args}: {e}"),
    }
}

fn open(a: &mut Automation, tag: &str) -> u64 {
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("spread-{tag}.pdf"));
    std::fs::write(&path, fixture()).unwrap();
    ok(a, "doc_open", json!({ "path": path }))["doc"].as_u64().unwrap()
}

/// Dark pixels in each column of page `page` (1-based) rendered at 72 dpi, and its size.
fn ink(a: &mut Automation, doc: u64, page: usize) -> (usize, usize, Vec<usize>) {
    let (w, h, px) = a.render_pixels(doc, page, 1.0).unwrap();
    let (w, h) = (w as usize, h as usize);
    let bytes: &[u8] = &px;
    let mut cols = vec![0usize; w];
    for (i, p) in bytes.as_chunks::<4>().0.iter().enumerate() {
        if p[0] < 128 {
            cols[i % w] += 1;
        }
    }
    (w, h, cols)
}

fn pages(a: &mut Automation, doc: u64) -> u64 {
    ok(a, "doc_info", json!({ "doc": doc }))["document"]["pages"].as_u64().unwrap()
}

#[test]
fn a_scanned_spread_becomes_its_two_pages_and_together_they_show_all_its_ink() {
    let mut a = Automation::new();
    let doc = open(&mut a, "halves");
    let (w0, h0, cols0) = ink(&mut a, doc, 1);
    let total0: usize = cols0.iter().sum();
    assert!(total0 > 1000, "the fixture has ink: {total0}");

    // A dry run says where, and changes nothing.
    let plan = ok(&mut a, "page_split_spreads", json!({ "doc": doc, "dry_run": true }));
    let splits = plan["splits"].as_array().unwrap();
    assert_eq!(splits.len(), 1, "the portrait page is left alone: {plan}");
    assert_eq!((splits[0]["page"].as_u64(), splits[0]["source"].as_str()), (Some(1), Some("gutter")), "{plan}");
    let at = splits[0]["position"].as_f64().unwrap();
    assert!((at - 0.45).abs() < 0.015, "{plan}");
    assert_eq!(pages(&mut a, doc), 2);

    let out = ok(&mut a, "page_split_spreads", json!({ "doc": doc }));
    assert_eq!(out["splits"][0]["first_page"], 1);
    assert_eq!(out["splits"][0]["second_page"], 2);
    assert_eq!(pages(&mut a, doc), 3);
    let (w1, h1, cols1) = ink(&mut a, doc, 1);
    let (w2, h2, cols2) = ink(&mut a, doc, 2);
    assert!(h1 == h0 && h2 == h0, "{h0} {h1} {h2}");
    assert!(w1.abs_diff((at * 792.0).round() as usize) <= 1 && (w1 + w2).abs_diff(w0) <= 1, "{w0} = {w1} + {w2}");
    // Side by side, the halves are the page: every column's ink where it was, none twice.
    let joined: Vec<usize> = cols1.iter().chain(&cols2).copied().collect();
    let total: usize = joined.iter().sum();
    assert!(total.abs_diff(total0) <= total0 / 100, "{total} vs {total0}");
    // The second half starts at a fraction of a pixel (x ≈ 359.2 px here), so rounding may shift its
    // columns by one: each column is compared with the original's at the same place or one either
    // side. A misplaced, doubled or missing half fails this by far.
    let moved: usize = joined
        .iter()
        .enumerate()
        .map(|(j, v)| (j.saturating_sub(1)..=j + 1).filter_map(|k| cols0.get(k)).map(|o| v.abs_diff(*o)).min().unwrap_or(*v))
        .sum();
    assert!(moved <= total0 / 100, "ink out of place: {moved} of {total0}");

    // One undo puts the spread back.
    ok(&mut a, "edit_undo", json!({ "doc": doc }));
    assert_eq!(pages(&mut a, doc), 2);
    assert_eq!(ink(&mut a, doc, 1).2, cols0);

    // A margin takes as much from each half, on either side of the gutter, and no ink with it.
    ok(&mut a, "page_split_spreads", json!({ "doc": doc, "margin": 6, "position": 0.45, "order": "ltr" }));
    let (m1, _, c1) = ink(&mut a, doc, 1);
    let (m2, _, c2) = ink(&mut a, doc, 2);
    assert!((m1 + m2 + 12).abs_diff(w0) <= 1, "{m1} + {m2} + 12 ≠ {w0}");
    assert!((c1.iter().sum::<usize>() + c2.iter().sum::<usize>()).abs_diff(total0) <= total0 / 100);
}

#[test]
fn right_to_left_and_saving_keep_the_order() {
    let mut a = Automation::new();
    let doc = open(&mut a, "rtl");
    ok(&mut a, "page_split_spreads", json!({ "doc": doc, "order": "rtl", "pages": [1] }));
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("spread-rtl-out.pdf");
    ok(&mut a, "doc_save", json!({ "doc": doc, "path": path }));
    let again = ok(&mut a, "doc_open", json!({ "path": path }))["doc"].as_u64().unwrap();
    assert_eq!(pages(&mut a, again), 3);
    // The right half (the wider one: the gutter is left of centre) comes first.
    let (w1, _, _) = ink(&mut a, again, 1);
    let (w2, _, _) = ink(&mut a, again, 2);
    assert!(w1 > w2, "{w1} {w2}");
}

#[test]
fn arguments_it_cannot_use_are_refused_with_what_is_allowed() {
    let mut a = Automation::new();
    let doc = open(&mut a, "args");
    for (args, says) in [
        (json!({ "doc": doc, "position": 2 }), "between 0.1 and 0.9"),
        (json!({ "doc": doc, "position": "middle" }), "\"auto\""),
        (json!({ "doc": doc, "margin": -1 }), "between 0 and 1000"),
        (json!({ "doc": doc, "min_ratio": 0.5 }), "between 1 and 10"),
        (json!({ "doc": doc, "order": "up" }), "ltr"),
        (json!({ "doc": doc, "pages": [9] }), "out of range"),
        (json!({ "doc": doc, "pages": [2] }), "nothing was split"),
    ] {
        let e = a.call("page_split_spreads", &args).unwrap_err();
        assert!(e.to_string().contains(says), "{args}: {e}");
        assert!(matches!(e, ToolError::InvalidArgs(_) | ToolError::Failed(_)), "{e:?}");
    }
    assert_eq!(pages(&mut a, doc), 2, "nothing changed");
}
