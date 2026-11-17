//! `text_glyph_details` end to end: a page LaTeX typeset, and a synthetic page with a glyph the text
//! layer cannot read.

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

fn open(a: &mut Automation, path: &str) -> u64 {
    ok(a, "doc_open", json!({ "path": path }))["doc"].as_u64().unwrap()
}

fn latex(name: &str) -> String {
    format!("{}/../render/tests/data/math/{name}", env!("CARGO_MANIFEST_DIR"))
}

fn glyph<'a>(page: &'a Value, font: &str, code: u64) -> &'a Value {
    let fonts = page["fonts"].as_array().unwrap();
    page["glyphs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["code"] == code && fonts[g["font"].as_u64().unwrap() as usize]["name"] == font)
        .unwrap_or_else(|| panic!("no glyph {font} {code}"))
}

#[test]
fn a_latex_page_is_read_glyph_by_glyph() {
    let mut a = Automation::new();
    let doc = open(&mut a, &latex("scripts_fractions.pdf"));
    let out = ok(&mut a, "text_glyph_details", json!({ "doc": doc }));
    assert!(out.get("next_page").is_none());
    let page = &out["pages"][0];
    assert_eq!((page["page"].clone(), page["truncated"].clone(), page["capped"].clone()), (json!(1), json!(false), json!(false)));
    // `generate.sh`'s DVI truth for this page: 36 glyphs and one rule.
    assert_eq!((page["glyphs"].as_array().unwrap().len(), page["rules"].as_array().unwrap().len()), (36, 1));
    let alpha = glyph(page, "CMMI10", 11);
    assert_eq!((alpha["name"].as_str(), alpha["text"].as_str(), alpha["unmapped"].clone()), (Some("alpha"), Some("α"), json!(false)));
    assert!((alpha["size"].as_f64().unwrap() - 9.963).abs() < 0.001, "{alpha}");
    // Boxes are [x0, y0, x1, y1]; the ink lies inside the em box's height.
    let (rect, ink) = (alpha["rect"].as_array().unwrap(), alpha["ink"].as_array().unwrap());
    assert!(rect.len() == 4 && ink.len() == 4);
    assert!(ink[1].as_f64() >= rect[1].as_f64() && ink[3].as_f64() <= rect[3].as_f64(), "{alpha}");
    let fonts = page["fonts"].as_array().unwrap();
    let cmmi = fonts.iter().find(|f| f["name"] == "CMMI10").unwrap();
    assert_eq!(cmmi["kind"], "type1");
    assert!(cmmi["program_bytes"].as_u64().unwrap() > 1000 && cmmi["base_font"].as_str().unwrap().ends_with("+CMMI10"), "{cmmi}");
    assert_eq!(page["rules"][0]["horizontal"], true);
    // Read-only: the document is not changed by reading it.
    let info = ok(&mut a, "doc_list", json!({}));
    ok(&mut a, "text_glyph_details", json!({ "doc": doc, "pages": [1] }));
    assert_eq!(ok(&mut a, "doc_list", json!({})), info);
    let def = tools().into_iter().find(|t| t.name == "text_glyph_details").unwrap();
    assert!(def.read_only && !def.destructive);
}

/// One page whose font names its first code `/foo`, which no glyph list knows, and its second
/// `/alpha`.
fn unmapped_pdf() -> Vec<u8> {
    let content = "BT /F1 12 Tf 20 100 Td (AB) Tj ET";
    let objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 200] >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".to_vec(),
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()).into_bytes(),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /ABCDEF+CMMI10 /Encoding << /Type /Encoding /Differences [65 /foo /alpha] >> >>".to_vec(),
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(o);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
    out
}

#[test]
fn a_glyph_the_text_layer_leaves_out_is_kept_and_flagged() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-math-details-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("m.pdf"), unmapped_pdf()).unwrap();
    let mut a = Automation::new().with_root(&dir).unwrap();
    let doc = open(&mut a, "m.pdf");
    let details = ok(&mut a, "text_glyph_details", json!({ "doc": doc }));
    let glyphs = details["pages"][0]["glyphs"].as_array().unwrap();
    assert_eq!(glyphs.len(), 2, "{details}");
    assert_eq!(
        (glyphs[0]["code"].clone(), glyphs[0]["name"].clone(), glyphs[0]["text"].clone(), glyphs[0]["unmapped"].clone()),
        (json!(65), json!("foo"), Value::Null, json!(true))
    );
    assert_eq!((glyphs[1]["name"].clone(), glyphs[1]["text"].clone(), glyphs[1]["unmapped"].clone()), (json!("alpha"), json!("α"), json!(false)));
    // Text extraction reads only the glyph that has Unicode.
    assert_eq!(ok(&mut a, "text_extract", json!({ "doc": doc }))["pages"][0]["text"], "α");
    // A page that does not exist is refused, and a document that is not open.
    assert!(a.call("text_glyph_details", &json!({ "doc": doc, "pages": [2] })).is_err());
    assert!(a.call("text_glyph_details", &json!({ "doc": 99 })).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}
