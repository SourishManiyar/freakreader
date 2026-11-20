//! `pdfcraft-cli run --script` splits a scanned two-page spread with `page_split_spreads` and saves
//! it: the saved file has one page per book page, cropped to its half, and the scan stored once.

use std::process::Command;
use std::sync::Arc;

use pdfcraft_cos::{Document, ObjRef, Object};

const BIN: &str = env!("CARGO_BIN_EXE_pdfcraft-cli");

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

#[test]
fn run_script_splits_a_spread_and_saves_it() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-cli-spreads-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let (src, out, script) = (dir.join("scan.pdf"), dir.join("book.pdf"), dir.join("split.json"));
    std::fs::write(&src, fixture()).unwrap();
    let steps = serde_json::json!([
        { "tool": "doc_open", "args": { "path": src } },
        { "tool": "page_split_spreads", "args": { "doc": 1 } },
        { "tool": "doc_save", "args": { "doc": 1, "path": out } },
    ]);
    std::fs::write(&script, steps.to_string()).unwrap();
    let run = Command::new(BIN).args(["run", "--script", script.to_str().unwrap()]).output().unwrap();
    assert!(run.status.success(), "{}", String::from_utf8_lossy(&run.stderr));
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert!(stdout.contains("\"gutter\""), "{stdout}");

    let doc = Document::open(Arc::new(std::fs::read(&out).unwrap())).unwrap();
    // The split writes a flat page tree; each page carries its own crop box.
    let root = doc.get(doc.root().unwrap());
    let tree = doc.get(root.as_dict().unwrap().reference(b"Pages").unwrap());
    let kids: Vec<ObjRef> = tree.as_dict().unwrap().get(b"Kids").and_then(Object::as_array).unwrap().iter().filter_map(Object::as_ref).collect();
    assert_eq!(kids.len(), 3, "the spread's two halves, then the portrait page");
    let crop = |k: ObjRef| -> Option<Vec<f64>> {
        Some(doc.get(k).as_dict()?.get(b"CropBox")?.as_array()?.iter().filter_map(|v| doc.resolve(v).as_f64()).collect())
    };
    let (a, b) = (crop(kids[0]).unwrap(), crop(kids[1]).unwrap());
    assert!((a[2] - b[0]).abs() < 1e-6 && (a[2] / 792.0 - 0.45).abs() < 0.015, "{a:?} {b:?}");
    assert_eq!((a[0], a[1], a[3], b[2]), (0.0, 0.0, 612.0, 792.0));
    assert_eq!(crop(kids[2]), None, "the portrait page is left as it was");
    let images = doc
        .object_numbers()
        .into_iter()
        .filter(|n| matches!(&*doc.get(ObjRef::new(*n, doc.generation(*n))), Object::Stream(s) if s.dict.name(b"Subtype") == Some(b"Image")))
        .count();
    assert_eq!(images, 1, "the scan is stored once");
    let _ = std::fs::remove_dir_all(&dir);
}
