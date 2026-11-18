//! Previews of citations: resting the pointer on a citation the text makes without a link (`[12]`,
//! one of `[1, 2]`, `Okafor (2020c)`) shows its bibliography entry's own extent, captioned with the
//! entry's printed label and "inferred"; a citation link (`cite.KEY`) shows its entry captioned with
//! its label; a look-alike (`[0,1]`, `Section [3]`) shows nothing. The papers are the bibliography
//! oracle's (`crates/render/tests/data/bib/oracle`).

use egui::Pos2;
use egui_kittest::Harness;
use pdfcraft_engine::references::{BlockKind, ReferenceGraph, Source};
use pdfcraft_ui_egui::PdfCraftApp;
use pdfcraft_ui_egui::canvas::link_peek::{HOVER_DELAY, HOVER_GRACE, PeekSource, Trigger};

fn harness(paper: &str) -> Harness<'static, PdfCraftApp> {
    let path = format!("{}/../render/tests/data/bib/oracle/{paper}.pdf", env!("CARGO_MANIFEST_DIR"));
    let bytes = std::fs::read(path).expect("the oracle paper is committed");
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 1000.0)).with_step_dt(1.0 / 60.0).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("paper.pdf", None, bytes.clone()).unwrap();
        app.set_option("zoom", "100").unwrap();
        app
    });
    h.run_steps(6);
    h
}

/// Frames until the document's reference graph is built (in the background).
fn graph(h: &mut Harness<'static, PdfCraftApp>) -> std::sync::Arc<ReferenceGraph> {
    for _ in 0..2000 {
        let app = h.state();
        let generation = app.session.get(app.views[0].id).map(|d| d.edit_generation()).unwrap();
        if let Some(g) = app.views[0].refs.graph(generation) {
            return g.clone();
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
        h.step();
    }
    panic!("the reference graph was not built");
}

/// The point (x, y), in points from the top-left of page 1, on screen.
fn on_page(h: &Harness<'static, PdfCraftApp>, x: f32, y: f32) -> Pos2 {
    let r = h.state().views[0].page_screen_rect(0).unwrap();
    let k = r.width() / 612.0;
    r.min + egui::vec2(x * k, y * k)
}

fn middle(h: &Harness<'static, PdfCraftApp>, rect: [f32; 4]) -> Pos2 {
    on_page(h, (rect[0] + rect[2]) / 2.0, (rect[1] + rect[3]) / 2.0)
}

/// Rest on `p` long enough for a preview to open, after resting away from everything.
fn rest(h: &mut Harness<'static, PdfCraftApp>, p: Pos2) {
    h.hover_at(Pos2::new(5.0, 5.0));
    h.run_steps(((HOVER_GRACE * 60.0) as usize) * 2 + 4);
    h.hover_at(p);
    h.run_steps(((HOVER_DELAY * 60.0) as usize) + 4);
}

/// The preview of a citation shows its entry's own extent on the entry's page.
fn shows_entry(h: &Harness<'static, PdfCraftApp>, g: &ReferenceGraph, entry: usize) {
    let peek = h.state().views[0].link_peek.as_ref().expect("a preview");
    let block = g.block(entry).unwrap();
    assert_eq!(block.kind, BlockKind::BibEntry);
    let [x0, y0, x1, y1] = block.first_rect();
    let a = peek.preview().area;
    assert_eq!(peek.preview().page, block.page());
    assert!(a[0] <= x0 && a[1] <= y0 && a[2] >= x1 && a[3] >= y1 && a[3] - a[1] < y1 - y0 + 20.0, "{a:?} for {:?}", block.first_rect());
}

#[test]
fn each_citation_of_a_group_previews_its_own_entry() {
    let mut h = harness("art10_plain");
    let g = graph(&mut h);
    // A group of two on page 1: each part is a citation of its own.
    let group = g
        .mentions
        .iter()
        .filter(|m| m.kind == "citation" && m.page() == 0 && m.source == Source::Inferred)
        .find_map(|m| {
            let same: Vec<_> = g.mentions.iter().filter(|o| o.kind == "citation" && o.group == m.group && o.page() == 0).collect();
            (same.len() >= 2 && same[0].target != same[1].target).then_some(same)
        })
        .expect("a group of citations on page 1");
    for m in group.iter().take(2) {
        {
            let at = middle(&h, m.extent[0].rect);
            rest(&mut h, at);
        }
        let peek = h.state().views[0].link_peek.as_ref().unwrap_or_else(|| panic!("{}: no preview", m.text));
        assert_eq!((peek.source(), peek.trigger()), (PeekSource::Mention { page: 0, id: m.id }, Trigger::Hover));
        let title = g.block(m.target.unwrap()).unwrap().title();
        assert_eq!(peek.inferred(), Some(title.as_str()));
        assert!(title.starts_with('[') && peek.caption().starts_with(&title) && peek.caption().ends_with("· inferred"), "{}", peek.caption());
        shows_entry(&h, &g, m.target.unwrap());
    }
}

#[test]
fn a_look_alike_previews_nothing() {
    let mut h = harness("art10_plain");
    let g = graph(&mut h);
    // `[0,1]`, an interval in mathematics, on the first page: find it among the page's words.
    let app = h.state();
    let doc = app.session.get(app.views[0].id).unwrap();
    let source = doc.export_source();
    let mut r = pdfcraft_render::PageRenderer::new(source.bytes.clone(), Default::default());
    let math = r.glyph_details(0).unwrap();
    let (w, hgt) = source.sizes[0];
    let layout =
        pdfcraft_render::text::page_layout::analyse_page(&math, pdfcraft_render::text::page_layout::PageContext { index: 0, width: w, height: hgt });
    let words = pdfcraft_render::text::page_layout::refs::page_words(&math, &layout);
    let interval = words.lines.iter().flat_map(|l| l.words.iter()).find(|w| w.text.starts_with("[0,1]")).expect("an interval on page 1").rect;
    assert!(g.mention_at(0, (interval[0] + interval[2]) / 2.0, (interval[1] + interval[3]) / 2.0, 0.0).is_none(), "no citation there");
    {
        let at = middle(&h, interval);
        rest(&mut h, at);
    }
    assert!(h.state().views[0].link_peek.is_none(), "a look-alike opens no preview");
}

#[test]
fn an_author_year_citation_previews_its_entry() {
    let mut h = harness("art12_apalike");
    let g = graph(&mut h);
    let m = g.mentions.iter().find(|m| m.kind == "citation" && m.page() == 0 && m.text.starts_with("Okafor 2020")).expect("Okafor (2020x) on page 1");
    {
        let at = middle(&h, m.extent[0].rect);
        rest(&mut h, at);
    }
    let peek = h.state().views[0].link_peek.as_ref().expect("a preview");
    let title = g.block(m.target.unwrap()).unwrap().title();
    assert!(title.starts_with("Okafor 2020") && peek.caption().starts_with(&title), "{title}: {}", peek.caption());
    shows_entry(&h, &g, m.target.unwrap());
}

#[test]
fn a_citation_link_previews_its_entry_by_its_label() {
    let mut h = harness("art11_hyper");
    let g = graph(&mut h);
    let m = g.mentions.iter().find(|m| m.kind == "citation" && m.source == Source::Link && m.page() == 0).expect("a citation link on page 1");
    let (page, index) = m.link.unwrap();
    {
        let at = middle(&h, m.extent[0].rect);
        rest(&mut h, at);
    }
    let peek = h.state().views[0].link_peek.as_ref().expect("a preview");
    assert_eq!(peek.source(), PeekSource::Link { page, index });
    let title = g.block(m.target.unwrap()).unwrap().title();
    assert_eq!((peek.named(), peek.inferred()), (Some(title.as_str()), None));
    assert!(title.starts_with('[') && peek.caption().starts_with(&title) && !peek.caption().contains("inferred"), "{}", peek.caption());
    shows_entry(&h, &g, m.target.unwrap());
    // With link previews off, a citation link shows nothing; inferred ones are another preference.
    h.state_mut().set_option("link-preview-hover", "off").unwrap();
    {
        let at = middle(&h, m.extent[0].rect);
        rest(&mut h, at);
    }
    assert!(h.state().views[0].link_peek.is_none());
}
