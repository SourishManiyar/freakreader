//! Organize ▸ Split two-page spreads in the real shell (egui_kittest).

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::{Dialog, PdfCraftApp};

/// Page 1 a landscape spread (two blocks of text either side of the middle), page 2 a portrait page.
const FIXTURE: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 800 500] /Contents 5 0 R /Resources << /Font << /F1 6 0 R >> >> >> endobj
4 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] >> endobj
5 0 obj << /Length 65 >> stream
BT /F1 24 Tf 60 250 Td (Left page) Tj 400 0 Td (Right page) Tj ET
endstream endobj
6 0 obj << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> endobj
trailer << /Root 1 0 R >>
%%EOF";

fn harness() -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("spread.pdf", None, FIXTURE.to_vec()).unwrap();
        app.set_option("left", "closed").unwrap();
        app
    });
    h.run_steps(6);
    h
}

fn sizes(h: &Harness<'static, PdfCraftApp>) -> Vec<(f32, f32)> {
    let s = h.state();
    s.session.get(s.views[0].id).unwrap().info.pages.iter().map(|p| (p.width, p.height)).collect()
}

#[test]
fn split_two_page_spreads_dialog_splits_the_landscape_page_only() {
    let mut h = harness();
    assert!(h.state_mut().execute("page.split_spreads"));
    h.run_steps(3);
    assert_eq!(h.state().dialog, Some(Dialog::SplitSpreads));
    h.get_by_label("Split Two-Page Spreads");
    // The dialog says the hidden half stays in the file, and where bookmarks and labels go.
    h.get_by_label_contains("stays in the file behind the crop box");
    h.get_by_label_contains("go to its first half");
    // The preview shows the portrait page next to the spread, left alone.
    h.get_by_label("Left as it is");
    h.get_by_label("A fixed position").click();
    h.run_steps(2);
    assert!(!h.state().spreads_draft.auto);
    h.get_by_label("Split pages").click();
    h.run_steps(4);
    assert_eq!(h.state().dialog, None);
    assert_eq!(sizes(&h), [(400.0, 500.0), (400.0, 500.0), (300.0, 400.0)]);
    let s = h.state();
    assert_eq!(s.session.get(s.views[0].id).unwrap().can_undo(), Some("Split page spread"));
}

#[test]
fn the_split_is_refused_when_page_assembly_is_forbidden() {
    let mut doc = pdfcraft_cos::Document::open(std::sync::Arc::new(FIXTURE.to_vec())).unwrap();
    doc.set_encryption(&pdfcraft_cos::NewEncryption {
        algorithm: pdfcraft_cos::Algorithm::Aes256,
        user_password: "",
        owner_password: "owner",
        permissions: 0b0100,
        encrypt_metadata: true,
        seed: [5; 32],
    })
    .unwrap();
    let bytes = pdfcraft_cos::write_full(&doc, &Default::default()).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("restricted.pdf", None, bytes).unwrap();
        app
    });
    h.run_steps(6);
    assert!(!h.state_mut().execute("page.split_spreads"));
    assert_eq!(h.state().dialog, None);
}
