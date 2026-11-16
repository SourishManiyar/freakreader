//! The reference preview's blocks on real LaTeX output: the papers in
//! `crates/render/tests/data/hyperref` (one and two columns, from one source), every
//! cross-reference a hyperref link to a named destination. Each block is checked by the text it
//! shows: all of what it names, none of its neighbours.

use std::sync::Arc;

use pdfcraft_engine::link_preview::{LinkPreview, RefKind, dest_preview, link_preview};
use pdfcraft_render::text::math::MathPage;
use pdfcraft_render::{DocInfo, PageRenderer, RenderConfig, inspect, named_destinations};

struct Paper {
    info: DocInfo,
    renderer: PageRenderer,
    bytes: Arc<Vec<u8>>,
}

fn paper(name: &str) -> Paper {
    let path = format!("{}/../render/tests/data/hyperref/{name}.pdf", env!("CARGO_MANIFEST_DIR"));
    let bytes = Arc::new(std::fs::read(&path).expect("the test paper is committed"));
    Paper { info: inspect(bytes.clone(), None).expect("inspects"), renderer: PageRenderer::new(bytes.clone(), RenderConfig::default()), bytes }
}

impl Paper {
    /// The preview of the named destination `name`, fitted to its block, and the target page's glyphs.
    fn dest(&mut self, name: &str) -> (LinkPreview, MathPage) {
        let dests = named_destinations(self.bytes.clone(), None).expect("reads");
        let d = dests.list.iter().find(|d| d.name == name).unwrap_or_else(|| panic!("{name} is a destination"));
        let mut preview = dest_preview(&self.info, d.page, d.view, Some(name)).expect("a page of the document");
        let math = self.renderer.glyph_details(preview.page).expect("glyphs");
        preview.fit(&self.info.pages[preview.page], &math);
        (preview, math)
    }

    /// The text the block `name` shows: the glyphs whose ink is inside its area, line by line,
    /// without spaces.
    fn shows(&mut self, name: &str) -> String {
        let (preview, math) = self.dest(name);
        assert!(preview.fitted, "{name}: no block was found ({:?})", preview.area);
        text_in(&math, preview.area)
    }
}

fn text_in(math: &MathPage, [x0, y0, x1, y1]: [f32; 4]) -> String {
    let mut inside: Vec<_> = math
        .glyphs
        .iter()
        .filter_map(|g| {
            let ink = g.ink?;
            let (cx, cy) = ((ink[0] + ink[2]) / 2.0, (ink[1] + ink[3]) / 2.0);
            ((x0..=x1).contains(&cx) && (y0..=y1).contains(&cy))
                .then(|| (g.baseline[0][1].round() as i64, ink[0], g.text.clone().unwrap_or_default()))
        })
        .collect();
    inside.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)));
    inside.into_iter().map(|g| g.2).collect()
}

/// `shown` has every one of `all` and none of `none`.
fn check(name: &str, shown: &str, all: &[&str], none: &[&str]) {
    for want in all {
        assert!(shown.contains(want), "{name} should show {want:?}: {shown}");
    }
    for unwanted in none {
        assert!(!shown.contains(unwanted), "{name} should not show {unwanted:?}: {shown}");
    }
}

fn blocks_read_whole(name: &str) {
    let mut p = paper(name);
    let mut is = |dest: &str, all: &[&str], none: &[&str]| {
        let shown = p.shows(dest);
        check(&format!("{name} {dest}"), &shown, all, none);
    };
    // An equation: its display, limits included, and nothing of the statement around it.
    is("equation.2.1", &["(2.1)", "n>N", "lim"], &["Lemma", "Then", "Proposition"]);
    is("equation.2.2", &["(2.2)", "π"], &["Moreover", "Corollary"]);
    // A row of an align: the whole align, whichever row is named.
    for row in ["equation.3.1", "equation.3.2"] {
        is(row, &["(3.1)", "(3.2)"], &["Compare", "By(3.2)"]);
    }
    // A statement: from its heading to its end, a display inside it included.
    is("theorem.2.1", &["Lemma2.1", "Then", "(2.1)"], &["Statements", "Proposition"]);
    is("theorem.2.3", &["Theorem2.3", "Moreover", "(2.2)"], &["Proposition", "Corollary"]);
    is("theorem.2.4", &["Corollary2.4", "converges"], &["(2.2)"]);
    is("definition.1.1", &["Definition1.1", "partialsum"], &["Remark", "Figure1shows"]);
    is("remark.1.1", &["Remark1.1", "tion1.1."], &["Definition1.1(", "Statements"]);
    // One bibliography entry, its hanging lines included.
    is("cite.alpha", &["[1]", "2001"], &["[2]", "References"]);
    is("cite.beta", &["[2]", "1–20"], &["[1]", "[3]"]);
    is("cite.gamma", &["[3]"], &["[2]"]);
    // A heading and its first lines.
    is("section.3", &["3Proofs", "ProofofLemma2.1"], &["References"]);
    // A float with its caption: the figure's picture above it, the table's rows below.
    is("figure.1", &["Figure1:", "sN", "N"], &["ProofofCorollary", "rem2.3", "Table1"]);
    is("table.1", &["Table1:", "1.36"], &["Figure1:", "References"]);
}

#[test]
fn one_column_blocks_read_whole() {
    blocks_read_whole("onecol");
}

#[test]
fn two_column_blocks_read_whole_within_their_column() {
    blocks_read_whole("twocol");
    // The right column's align stays in the right column.
    let mut p = paper("twocol");
    let (preview, _) = p.dest("equation.3.2");
    assert!(preview.area[0] > 290.0, "{:?}", preview.area);
    let (preview, _) = p.dest("equation.2.1");
    assert!(preview.area[2] < 320.0, "{:?}", preview.area);
}

#[test]
fn names_without_a_block_keep_the_band() {
    let mut p = paper("onecol");
    for name in ["page.2", "Doc-Start"] {
        let (preview, _) = p.dest(name);
        assert!(!preview.fitted && preview.area[0] == 0.0 && preview.area[2] == 612.0, "{name}: {preview:?}");
    }
}

#[test]
fn every_link_in_the_papers_reads_as_its_kind_with_its_number() {
    for name in ["onecol", "twocol"] {
        let p = paper(name);
        let mut kinds = std::collections::BTreeSet::new();
        for link in &p.info.links {
            let preview = link_preview(&p.info, link.page, link.index).expect("every link goes into the paper");
            let dest = preview.name.clone().expect("hyperref names every destination");
            if dest.starts_with("equation.") {
                assert_eq!(preview.reference.number.as_deref(), dest.strip_prefix("equation."));
                assert_eq!(
                    preview.label(&p.info.pages[preview.page].label),
                    format!("Equation {} · page {}", dest.strip_prefix("equation.").unwrap_or(""), preview.page + 1)
                );
            }
            kinds.insert(format!("{:?}", preview.reference.kind));
        }
        let expected: std::collections::BTreeSet<String> =
            [RefKind::Equation, RefKind::Citation, RefKind::Section, RefKind::TheoremLike, RefKind::Figure]
                .iter()
                .map(|k| format!("{k:?}"))
                .collect();
        assert_eq!(kinds, expected, "{name}");
    }
}
