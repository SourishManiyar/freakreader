//! A document's reference graph for the viewer ([`pdfcraft_engine::references`]): where its text
//! mentions its statements, equations, sections and floats, so the reference preview
//! ([`super::link_peek`]) can open on a mention that has no link annotation.
//!
//! The graph is built off the UI thread, once per edit generation of the document, and only while
//! inferred previews are wanted (Preferences); it is dropped when the document changes and built
//! again for the new state. Nothing waits for it: until it is there, inferred mentions show no
//! preview. On the web build there are no threads, and no graph is built.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError};

use pdfcraft_engine::references::ReferenceGraph;

/// The viewer's copy of a document's reference graph, and the build under way.
#[derive(Default)]
pub struct RefGraphCache {
    /// The graph of an edit generation.
    ready: Option<(u64, Arc<ReferenceGraph>)>,
    /// The build under way, for an edit generation. `None` from it: the build failed.
    job: Option<(u64, Receiver<Option<ReferenceGraph>>)>,
    /// A generation whose build failed: not tried again.
    failed: Option<u64>,
}

impl std::fmt::Debug for RefGraphCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RefGraphCache")
            .field("ready", &self.ready.as_ref().map(|(g, r)| (*g, r.mentions.len())))
            .field("building", &self.job.as_ref().map(|(g, _)| *g))
            .finish()
    }
}

impl RefGraphCache {
    /// The graph of edit generation `generation`, when it has been built.
    pub fn graph(&self, generation: u64) -> Option<&Arc<ReferenceGraph>> {
        self.ready.as_ref().filter(|(g, _)| *g == generation).map(|(_, r)| r)
    }

    /// Whether a build is under way.
    pub fn building(&self) -> bool {
        self.job.is_some()
    }

    /// Once a frame: take a finished build; drop a graph or build of another generation; start a
    /// build of `doc`'s current state when `wanted` and none is there. A repaint follows the
    /// answer.
    pub(crate) fn update(&mut self, ctx: &egui::Context, doc: &pdfcraft_engine::Document, wanted: bool) {
        let generation = doc.edit_generation();
        if self.ready.as_ref().is_some_and(|(g, _)| *g != generation) {
            self.ready = None;
        }
        if let Some((g, answer)) = &self.job {
            let g = *g;
            match answer.try_recv() {
                Ok(Some(graph)) => {
                    if g == generation {
                        self.ready = Some((g, Arc::new(graph)));
                    }
                    self.job = None;
                }
                Ok(None) | Err(TryRecvError::Disconnected) => {
                    self.failed = Some(g);
                    self.job = None;
                }
                Err(TryRecvError::Empty) => {
                    // A build of a state that is gone is let go: its answer is never read.
                    if g != generation {
                        self.job = None;
                    }
                }
            }
        }
        if !wanted || self.ready.is_some() || self.job.is_some() || self.failed == Some(generation) {
            return;
        }
        self.start(ctx, doc, generation);
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn start(&mut self, ctx: &egui::Context, doc: &pdfcraft_engine::Document, generation: u64) {
        let (send, answer) = std::sync::mpsc::channel();
        let source = doc.export_source();
        let links = doc.info.links.clone();
        let pages = doc.info.pages.clone();
        let ctx = ctx.clone();
        let run = move || {
            let graph = pdfcraft_engine::references::reference_graph(source, &links, &pages).ok();
            let _ = send.send(graph);
            ctx.request_repaint();
        };
        match std::thread::Builder::new().name("reference-graph".into()).spawn(run) {
            Ok(_) => self.job = Some((generation, answer)),
            Err(e) => {
                log::warn!("reference graph thread: {e}");
                self.failed = Some(generation);
            }
        }
    }

    #[cfg(target_arch = "wasm32")]
    fn start(&mut self, _ctx: &egui::Context, _doc: &pdfcraft_engine::Document, generation: u64) {
        // No threads on the web: building the graph would hold the page; inferred previews are off.
        self.failed = Some(generation);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The graph is built off the UI thread, kept for its generation, and dropped when the
    /// document changes.
    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn the_graph_is_built_in_the_background_and_dropped_on_change() {
        let mut app = crate::PdfCraftApp::new();
        app.open_bytes("notes.txt", None, b"Lemma 1. Every set is small.".to_vec()).expect("opens");
        let (_, d) = app.active_ids().expect("a document");
        let ctx = egui::Context::default();
        let mut cache = RefGraphCache::default();
        let doc = app.session.get(d).expect("open");
        cache.update(&ctx, doc, false);
        assert!(!cache.building(), "not wanted, not built");
        cache.update(&ctx, doc, true);
        for _ in 0..1000 {
            if cache.graph(doc.edit_generation()).is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
            cache.update(&ctx, doc, true);
        }
        assert!(cache.graph(doc.edit_generation()).is_some(), "built");
        assert!(cache.graph(doc.edit_generation() + 1).is_none());
        // Another generation: the old graph is gone.
        cache.ready = cache.ready.take().map(|(g, r)| (g.wrapping_add(7), r));
        cache.update(&ctx, doc, false);
        assert!(cache.ready.is_none());
    }
}
