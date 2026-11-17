//! Page layout analysis as tools: `page_layout` (columns, blocks and reading order of pages) and
//! `doc_sections` (the section tree), see `pdfcraft_render::text::page_layout`.

use pdfcraft_render::text::page_layout::{Block, Diagnostic, PageLayout};
use serde_json::{Value, json};

use crate::{Args, Automation, Result, ToolError, failed};

fn r4(r: [f32; 4]) -> Vec<f64> {
    r.iter().map(|v| (f64::from(*v) * 100.0).round() / 100.0).collect()
}

fn diagnostics(ds: &[Diagnostic]) -> Vec<Value> {
    ds.iter().map(|d| json!({ "code": d.code, "message": d.message, "block": d.block })).collect()
}

fn block_json(i: usize, b: &Block, text: bool, lines: bool) -> Value {
    let mut v = json!({
        "id": i,
        "kind": b.kind.as_str(),
        "rect": r4(b.rect),
        "order": b.order,
        "column": b.column,
        "confidence": (f64::from(b.confidence) * 100.0).round() / 100.0,
        "line_count": b.lines.len(),
        "size": (f64::from(b.style.size) * 10.0).round() / 10.0,
    });
    if text {
        v["text"] = json!(b.text);
    }
    if lines {
        v["lines"] = b
            .lines
            .iter()
            .map(|l| json!({ "baseline": (f64::from(l.baseline) * 100.0).round() / 100.0, "rect": r4(l.rect), "glyphs": l.glyphs }))
            .collect();
        v["rules"] = json!(b.rules);
    }
    if let Some(h) = &b.heading {
        v["heading"] = json!({ "level": h.level, "number": h.number, "level_guessed": h.level_guessed, "appendix": h.appendix });
    }
    if let Some(t) = &b.equation_tag {
        v["equation_tag"] = json!(t);
    }
    if let Some(p) = b.partner {
        v[if b.kind == pdfcraft_render::text::page_layout::BlockKind::Float { "caption" } else { "float" }] = json!(p);
    }
    v
}

fn page_json(l: &PageLayout, text: bool, lines: bool) -> Value {
    json!({
        "page": l.page + 1,
        "size": [f64::from(l.size[0]), f64::from(l.size[1])],
        "body_size": l.body_size.map(|s| (f64::from(s) * 10.0).round() / 10.0),
        "columns": l.columns.iter().map(|c| json!({ "rect": r4(c.rect), "zone": c.zone, "index": c.index, "of": c.of })).collect::<Vec<_>>(),
        "blocks": l.blocks.iter().enumerate().map(|(i, b)| block_json(i, b, text, lines)).collect::<Vec<_>>(),
        "reading_order": l.reading_order,
        "diagnostics": diagnostics(&l.diagnostics),
    })
}

impl Automation {
    /// Columns, blocks and reading order of pages (`page_layout`).
    pub(crate) fn page_layout(&self, a: &Args) -> Result<Value> {
        let doc = self.doc_to_copy(a)?;
        let pages = match a.opt_ints("pages")? {
            Some(_) => self.pages(a, "pages")?,
            None => (0..doc.info.pages.len()).collect(),
        };
        let max = pdfcraft_engine::page_layout::MAX_LAYOUT_PAGES;
        if pages.len() > max {
            return Err(ToolError::InvalidArgs(format!("at most {max} pages at a time; the document has {} (pass pages to choose)", pages.len())));
        }
        let text = a.opt_bool("text")?.unwrap_or(true);
        let lines = a.opt_bool("lines")?.unwrap_or(false);
        let layouts = pdfcraft_engine::page_layout::page_layouts(doc.export_source(), &pages).map_err(failed)?;
        Ok(json!({ "pages": layouts.iter().map(|l| page_json(l, text, lines)).collect::<Vec<_>>() }))
    }

    /// The section tree inferred from the headings (`doc_sections`).
    pub(crate) fn doc_sections(&self, a: &Args) -> Result<Value> {
        let doc = self.doc_to_copy(a)?;
        let (layouts, structure, truncated) = pdfcraft_engine::page_layout::document_structure(doc.export_source()).map_err(failed)?;
        let running: Vec<Value> = layouts
            .iter()
            .flat_map(|l| {
                l.blocks.iter().enumerate().filter(|(_, b)| b.kind.is_running()).map(move |(i, b)| {
                    json!({ "page": l.page + 1, "block": i, "kind": b.kind.as_str(), "text": b.text, "confidence": (f64::from(b.confidence) * 100.0).round() / 100.0 })
                })
            })
            .collect();
        Ok(json!({
            "sections": structure.sections.iter().map(|s| json!({
                "page": s.page + 1, "block": s.block, "level": s.level, "number": s.number, "title": s.title,
                "parent": s.parent, "appendix": s.appendix, "level_guessed": s.level_guessed,
            })).collect::<Vec<_>>(),
            "running": running,
            "pages_read": layouts.len(),
            "truncated": truncated,
            "diagnostics": diagnostics(&structure.diagnostics),
        }))
    }
}
