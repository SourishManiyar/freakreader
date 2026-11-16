//! Edit ▸ Copy as LaTeX end to end on the math oracle's TeX-typeset equations: drag a rectangle
//! around a formula, and its LaTeX is on the clipboard with a notice saying how sure the reader is;
//! an area it cannot read copies nothing; a document that withholds copying reads nothing.

use egui_kittest::Harness;
use pdfcraft_ui_egui::PdfCraftApp;

fn oracle(name: &str) -> String {
    format!("{}/../render/tests/data/math/oracle/{name}", env!("CARGO_MANIFEST_DIR"))
}

/// An equation of the oracle's truth: its 0-based page of `equations.pdf` and its view-space box.
fn place(id: &str) -> (usize, [f32; 4]) {
    let truth = std::fs::read_to_string(oracle("truth.tsv")).unwrap();
    let line = truth.lines().find(|l| l.starts_with(&format!("E\t{id}\t"))).unwrap_or_else(|| panic!("no equation {id}"));
    let f: Vec<&str> = line.split('\t').collect();
    assert_eq!(f[3], "equations");
    let n = |k: usize| f[k].parse::<f32>().unwrap();
    (f[4].parse().unwrap(), [n(9), n(10), n(11), n(12)])
}

fn harness(bytes: Vec<u8>) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).with_step_dt(1.0 / 60.0).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("equations.pdf", None, bytes).expect("opens");
        app.set_option("left", "closed").unwrap();
        app.system_clipboard = false;
        app
    });
    crate::support::settle(&mut h);
    h
}

/// Show page `page` (0-based) and return where view-space point (x, y) of it is on screen.
fn show(h: &mut Harness<'static, PdfCraftApp>, page: usize) -> impl Fn(f32, f32) -> egui::Pos2 + use<> {
    h.state_mut().set_option("page", &(page + 1).to_string()).unwrap();
    let shown = crate::support::step_until(h, |h| h.state().views[0].page_screen_rect(page).is_some() && !h.state().render_pending());
    assert!(shown, "page {} on screen", page + 1);
    h.run_steps(2);
    let r = h.state().views[0].page_screen_rect(page).expect("on screen");
    // The oracle's pages are 420 × 200 points.
    move |x, y| egui::pos2(r.left() + x / 420.0 * r.width(), r.top() + y / 200.0 * r.height())
}

/// Drag from `a` to `b` with the primary button, then wait for the answer. Returns every text put on
/// the clipboard meanwhile.
fn drag(h: &mut Harness<'static, PdfCraftApp>, a: egui::Pos2, b: egui::Pos2) -> Vec<String> {
    let mut copied = Vec::new();
    let mut take = |h: &Harness<'static, PdfCraftApp>| {
        for c in &h.output().platform_output.commands {
            if let egui::OutputCommand::CopyText(t) = c {
                copied.push(t.clone());
            }
        }
    };
    h.hover_at(a);
    h.run_steps(1);
    h.drag_at(a);
    h.run_steps(1);
    for k in 1..=4 {
        h.hover_at(a + (b - a) * (k as f32 / 4.0));
        h.run();
        take(h);
    }
    h.drop_at(b);
    h.step();
    take(h);
    let answered = crate::support::step_until(h, |h| {
        take(h);
        !h.state().latex_copy_running()
    });
    assert!(answered, "the formula was read");
    h.step();
    take(h);
    copied
}

/// Drag around equation `id`, a point outside its box on every side.
fn copy_equation(h: &mut Harness<'static, PdfCraftApp>, id: &str) -> Vec<String> {
    let (page, [x0, y0, x1, y1]) = place(id);
    let at = show(h, page);
    drag(h, at(x0 - 1.0, y0 - 1.0), at(x1 + 1.0, y1 + 1.0))
}

fn notice(h: &Harness<'static, PdfCraftApp>) -> String {
    h.state().toast.as_ref().map(|t| t.0.clone()).unwrap_or_default()
}

#[test]
fn a_formula_is_copied_with_how_sure_the_reader_is() {
    let mut h = harness(std::fs::read(oracle("equations.pdf")).unwrap());
    assert!(h.state_mut().execute("edit.copy_as_latex"));
    assert_eq!(h.state().quick_tool, pdfcraft_ui_egui::QuickTool::CopyAsLatex);
    assert_eq!(notice(&h), "Drag a rectangle around a formula to copy its LaTeX");

    // Confident: the bare body, and the notice says so with a preview.
    let copied = copy_equation(&mut h, "sup_squares");
    assert_eq!(copied, vec![r"x^{2} + y^{2} = r^{2}".to_string()]);
    assert_eq!(notice(&h), r"Copied LaTeX: x^{2} + y^{2} = r^{2}");

    // Tentative (\frac or \dfrac cannot be told apart without the environment): copied, wrapped as
    // the preference says, and never presented as certain.
    h.state_mut().set_option("latex-wrapper", "inline").unwrap();
    let copied = copy_equation(&mut h, "frac_simple");
    assert_eq!(copied, vec![r"$\frac{a + b}{c}$".to_string()]);
    let said = notice(&h);
    assert!(said.starts_with(r"Copied LaTeX with low confidence, check it: \frac{a + b}{c} ("), "{said}");

    // Not read (a matrix): nothing is copied and the notice says why.
    let copied = copy_equation(&mut h, "pmatrix_two");
    assert!(copied.is_empty(), "{copied:?}");
    let said = notice(&h);
    assert!(said.starts_with("Nothing recognised:") && said.ends_with("The clipboard is unchanged."), "{said}");

    // An empty part of a page already read is answered at once, without reading the page again.
    let (page, _) = place("pmatrix_two");
    let at = show(&mut h, page);
    h.hover_at(at(20.0, 120.0));
    h.run_steps(1);
    h.drag_at(at(20.0, 120.0));
    h.run_steps(1);
    h.hover_at(at(120.0, 180.0));
    h.run_steps(1);
    h.drop_at(at(120.0, 180.0));
    h.run_steps(2);
    assert!(!h.state().latex_copy_running());
    assert_eq!(notice(&h), "Nothing recognised: there is no text or symbol in the area. The clipboard is unchanged.");

    // A click is not a rectangle.
    h.hover_at(at(200.0, 100.0));
    h.run_steps(1);
    h.drag_at(at(200.0, 100.0));
    h.run_steps(1);
    h.drop_at(at(200.0, 100.0));
    h.run_steps(2);
    assert_eq!(notice(&h), "Drag a rectangle around a formula to copy its LaTeX");
    assert!(!h.state().latex_copy_running());
}

#[test]
fn a_document_that_withholds_copying_is_not_read() {
    let mut doc = pdfcraft_cos::Document::open(std::sync::Arc::new(std::fs::read(oracle("equations.pdf")).unwrap())).unwrap();
    doc.set_encryption(&pdfcraft_cos::NewEncryption {
        algorithm: pdfcraft_cos::Algorithm::Aes256,
        user_password: "",
        owner_password: "owner",
        permissions: -1 ^ 0b1_0000,
        encrypt_metadata: true,
        seed: [4; 32],
    })
    .unwrap();
    let bytes = pdfcraft_cos::write_full(&doc, &Default::default()).unwrap();
    let mut h = harness(bytes);
    let id = h.state().views[0].id;
    assert!(!h.state().session.get(id).unwrap().allows_copying());
    // The command is disabled; the tool chosen on another tab still refuses here.
    assert!(!h.state_mut().execute("edit.copy_as_latex"));
    h.state_mut().set_option("quick", "copy-as-latex").unwrap();
    let copied = copy_equation(&mut h, "sup_squares");
    assert!(copied.is_empty(), "{copied:?}");
    assert_eq!(notice(&h), "The document's security settings don't allow copying text and images");
}
