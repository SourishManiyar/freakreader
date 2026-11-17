//! Statements and references as tools: `doc_blocks` (theorem-like statements, proofs, numbered
//! headings, displays and floats) and `doc_references` (where the text mentions them, from links
//! and from the text), see `pdfcraft_engine::references`.

use pdfcraft_engine::references::{BlockKind, Extent, RefBlock, RefMention, ReferenceGraph, Source, reference_graph};
use serde_json::{Value, json};

use crate::{Args, Automation, Result, ToolError, failed};

fn r2(v: f32) -> f64 {
    (f64::from(v) * 100.0).round() / 100.0
}

fn extent_json(e: &Extent) -> Value {
    json!({ "page": e.page + 1, "rect": e.rect.iter().map(|v| r2(*v)).collect::<Vec<_>>() })
}

fn block_json(b: &RefBlock) -> Value {
    json!({
        "id": b.id,
        "kind": b.kind.as_str(),
        "label": b.label,
        "number": b.number,
        "name": b.name,
        "title": b.title(),
        "page": b.page() + 1,
        "extent": b.extent.iter().map(extent_json).collect::<Vec<_>>(),
        "head": b.head.as_ref().map(extent_json),
        "proves": b.proves,
        "end_mark": b.end_mark,
        "confidence": r2(b.confidence),
    })
}

fn mention_json(g: &ReferenceGraph, m: &RefMention) -> Value {
    let target = m.target.and_then(|t| g.block(t));
    json!({
        "id": m.id,
        "kind": m.kind,
        "text": m.text,
        "page": m.page() + 1,
        "extent": m.extent.iter().map(extent_json).collect::<Vec<_>>(),
        "target": m.target,
        "target_title": target.map(RefBlock::title),
        "target_page": target.map(|b| b.page() + 1),
        "source": m.source.as_str(),
        "confidence": r2(m.confidence),
        "unresolved": m.unresolved,
        "link": m.link.map(|(page, index)| json!({ "page": page + 1, "index": index })),
    })
}

fn diagnostics(g: &ReferenceGraph) -> Vec<Value> {
    g.diagnostics.iter().map(|d| json!({ "code": d.code, "message": d.message, "page": d.page.map(|p| p + 1) })).collect()
}

impl Automation {
    fn graph(&self, a: &Args) -> Result<ReferenceGraph> {
        let doc = self.doc_to_copy(a)?;
        reference_graph(doc.export_source(), &doc.info.links, &doc.info.pages).map_err(failed)
    }

    /// The document's referable blocks (`doc_blocks`).
    pub(crate) fn doc_blocks(&self, a: &Args) -> Result<Value> {
        let kinds: Option<Vec<BlockKind>> = match a.get("kinds") {
            None => None,
            Some(_) => Some(
                a.strs("kinds")?
                    .into_iter()
                    .map(|k| {
                        BlockKind::from_name(k)
                            .ok_or_else(|| ToolError::InvalidArgs(format!("kinds: `{k}` is none of statement, proof, heading, equation, float")))
                    })
                    .collect::<Result<_>>()?,
            ),
        };
        let g = self.graph(a)?;
        let blocks: Vec<Value> = g.blocks.iter().filter(|b| kinds.as_ref().is_none_or(|k| k.contains(&b.kind))).map(block_json).collect();
        Ok(json!({ "blocks": blocks, "pages_read": g.pages_read, "truncated": g.truncated, "diagnostics": diagnostics(&g) }))
    }

    /// Where the text mentions the blocks (`doc_references`).
    pub(crate) fn doc_references(&self, a: &Args) -> Result<Value> {
        let source = match a.opt_str("source")?.unwrap_or("all") {
            "all" => None,
            "link" => Some(Source::Link),
            "inferred" => Some(Source::Inferred),
            other => return Err(ToolError::InvalidArgs(format!("source must be all, link or inferred, not `{other}`"))),
        };
        let least = a.opt_num("min_confidence")?.unwrap_or(0.0);
        if !(0.0..=1.0).contains(&least) {
            return Err(ToolError::InvalidArgs(format!("min_confidence must be from 0 to 1, not {least}")));
        }
        let page = match a.opt_int("page")? {
            None => None,
            Some(p) if p >= 1 => Some((p - 1) as usize),
            Some(p) => return Err(ToolError::InvalidArgs(format!("page must be 1 or more, not {p}"))),
        };
        let resolved_only = a.opt_bool("resolved")?.unwrap_or(false);
        let g = self.graph(a)?;
        if let Some(p) = page.filter(|p| *p >= g.pages_read.max(1)) {
            return Err(ToolError::InvalidArgs(format!("page {} was not read: the graph has {} pages", p + 1, g.pages_read)));
        }
        let mentions: Vec<Value> = g
            .mentions
            .iter()
            .filter(|m| source.is_none_or(|s| m.source == s))
            .filter(|m| f64::from(m.confidence) >= least)
            .filter(|m| !resolved_only || m.target.is_some())
            .filter(|m| page.is_none_or(|p| m.page() == p))
            .map(|m| mention_json(&g, m))
            .collect();
        Ok(json!({
            "mentions": mentions,
            "blocks": g.blocks.iter().map(|b| json!({ "id": b.id, "kind": b.kind.as_str(), "title": b.title(), "page": b.page() + 1 })).collect::<Vec<_>>(),
            "pages_read": g.pages_read,
            "truncated": g.truncated,
            "diagnostics": diagnostics(&g),
        }))
    }
}
