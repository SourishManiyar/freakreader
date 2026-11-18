//! The reference graph on typeset papers: the reference oracle's (`crates/render/tests/data/refs/
//! oracle`, statements and mentions known from the DVI) and the hyperref papers of the reference
//! previews (`crates/render/tests/data/hyperref`, written apart from the oracle, every
//! cross-reference a link): the text's inferred mentions go where the links go.

use std::sync::Arc;

use pdfcraft_engine::export::ExportSource;
use pdfcraft_engine::references::{BlockKind, PREVIEW_CONFIDENCE, ReferenceGraph, Source, reference_graph};
use pdfcraft_render::{DocInfo, RenderConfig, inspect};

fn graph(path: &str) -> (DocInfo, ReferenceGraph) {
    let path = format!("{}/../render/tests/data/{path}.pdf", env!("CARGO_MANIFEST_DIR"));
    let bytes = Arc::new(std::fs::read(&path).expect("the test paper is committed"));
    let info = inspect(bytes.clone(), None).expect("inspects");
    let source = ExportSource {
        bytes,
        config: RenderConfig::default(),
        pages: info.pages.len(),
        sizes: info.pages.iter().map(|p| (p.width, p.height)).collect(),
    };
    let g = reference_graph(source, &info.links, &info.pages).expect("no panic");
    (info, g)
}

#[test]
fn the_word_decides_where_a_mention_goes() {
    let (_, g) = graph("refs/oracle/art10_traps");
    let title = |id: Option<usize>| id.and_then(|i| g.block(i)).map(|b| b.title());
    // Lemma 1, Theorem 1, Proposition 1 and Corollary 1 all exist; each mention goes to its own.
    for (text, want) in [("Lemma 1", "Lemma 1"), ("Theorem 1", "Theorem 1"), ("Proposition 1", "Proposition 1"), ("Corollary 1", "Corollary 1")] {
        let m = g.mentions.iter().find(|m| m.text == text).unwrap_or_else(|| panic!("{text} is mentioned"));
        assert_eq!(title(m.target).as_deref(), Some(want), "{text}");
        assert_eq!(m.source, Source::Inferred);
        assert!(m.confidence >= PREVIEW_CONFIDENCE);
    }
    // Nothing is a link here; mentions of other works and of missing blocks point at nothing.
    assert!(g.mentions.iter().all(|m| m.link.is_none()));
    assert!(g.mentions.iter().any(|m| m.unresolved == Some("external") && m.target.is_none()));
    assert!(g.mentions.iter().any(|m| m.text == "Proposition 3" && m.unresolved == Some("missing")));
    // Proofs prove the statement before them or the one they name.
    let proofs: Vec<_> = g.blocks.iter().filter(|b| b.kind == BlockKind::Proof).collect();
    assert_eq!(proofs.len(), 5);
    assert!(proofs.iter().all(|p| p.proves.is_some() && p.end_mark.is_none()));
    // Hover finds a mention by a point inside it.
    let m = g.mentions.iter().find(|m| m.target.is_some()).expect("a resolved mention");
    let e = m.extent[0];
    let hit = g.mention_at(e.page, (e.rect[0] + e.rect[2]) / 2.0, (e.rect[1] + e.rect[3]) / 2.0, PREVIEW_CONFIDENCE);
    assert_eq!(hit.map(|h| h.id), Some(m.id));
}

/// Every link to a statement, equation, section or float is a mention of the graph, with the link
/// as its source; where the text was read too, the text says the same, apart from the mentions
/// the counts below name.
fn links_and_text(path: &str) -> (usize, usize, Vec<String>) {
    let (info, g) = graph(path);
    let linked: Vec<_> = g.mentions.iter().filter(|m| m.source == Source::Link).collect();
    let to_blocks = info.links.iter().filter(|l| !l.dest_name.as_deref().is_some_and(|n| n.starts_with("cite."))).count();
    assert_eq!(linked.len(), to_blocks, "{path}: every link to a block is in the graph");
    let read: Vec<_> = linked.iter().filter(|m| m.kind != "link").collect();
    let disagree: Vec<String> = g.diagnostics.iter().filter(|d| d.code == "link_disagrees").map(|d| d.message.clone()).collect();
    (linked.len(), read.len(), disagree)
}

#[test]
fn inferred_mentions_agree_with_the_links() {
    let (links, read, disagree) = links_and_text("refs/oracle/art11_hyper");
    assert_eq!((links, read), (9, 9));
    // `Theorem 1.1 of [Tanaka]` is set with \ref: the link is real, the text says another work.
    assert_eq!(disagree.len(), 1, "{disagree:?}");
    assert!(disagree[0].contains("Theorem 1.1"), "{disagree:?}");
    // Written apart from the oracle: every link to a block is in the graph, the text was read
    // under all but two (an equation number after `is`, which is not taken for a citation alone),
    // and nowhere does the text say something else than the link.
    for paper in ["hyperref/onecol", "hyperref/twocol"] {
        let (links, read, disagree) = links_and_text(paper);
        assert_eq!((links, read), (16, 14), "{paper}");
        assert!(disagree.is_empty(), "{paper}: {disagree:#?}");
    }
}
