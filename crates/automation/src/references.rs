//! Statements, references and the bibliography as tools: `doc_blocks` (theorem-like statements,
//! proofs, numbered headings, displays and floats, and bibliography entries when asked),
//! `doc_references` (where the text mentions or cites them, from links and from the text) and
//! `doc_bibliography` (the reference list, its entries and their fields, and the citations of each),
//! see `pdfcraft_engine::references`.

use pdfcraft_engine::references::{BlockKind, Extent, RefBlock, RefMention, ReferenceGraph, Source, reference_graph};
use pdfcraft_render::text::page_layout::bib::{Entry, Field};
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
    let mut v = json!({
        "id": m.id,
        "kind": m.kind,
        "text": m.text,
        // A key inside a range of citations has no extent, and no page.
        "page": m.extent.first().map(|e| e.page + 1),
        "extent": m.extent.iter().map(extent_json).collect::<Vec<_>>(),
        "target": m.target,
        "target_title": target.map(RefBlock::title),
        "target_page": target.map(|b| b.page() + 1),
        "source": m.source.as_str(),
        "confidence": r2(m.confidence),
        "unresolved": m.unresolved,
        "link": m.link.map(|(page, index)| json!({ "page": page + 1, "index": index })),
    });
    if m.kind == "citation"
        && let Some(o) = v.as_object_mut()
    {
        o.insert("locator".into(), json!(m.locator));
        o.insert("group".into(), json!(m.group));
        o.insert("entry".into(), json!(m.target.and_then(|t| g.entry_of_block(t))));
    }
    v
}

fn field_json(f: &Option<Field>) -> Value {
    f.as_ref().map_or(Value::Null, |f| json!({ "value": f.value, "confidence": r2(f.confidence) }))
}

fn entry_json(g: &ReferenceGraph, e: &Entry, cited: usize) -> Value {
    let f = &e.fields;
    json!({
        "id": e.id,
        "block": g.entry_blocks.get(e.id),
        "label": e.label,
        "printed_label": e.printed_label(),
        "label_style": e.label_style.as_str(),
        "title": g.entry_blocks.get(e.id).and_then(|b| g.block(*b)).map(RefBlock::title),
        "page": e.pieces.first().map(|p| p.page + 1),
        "extent": e.pieces.iter().map(|p| extent_json(&Extent { page: p.page, rect: p.rect })).collect::<Vec<_>>(),
        "text": e.text,
        "parsed": e.parsed,
        "confidence": r2(e.confidence),
        "cited": cited,
        "fields": {
            "authors": f.authors.iter().map(|a| json!({ "family": a.family, "given": a.given })).collect::<Vec<_>>(),
            "authors_confidence": r2(f.authors_confidence),
            "et_al": f.et_al,
            "title": field_json(&f.title),
            "venue": field_json(&f.venue),
            "year": field_json(&f.year),
            "volume": field_json(&f.volume),
            "number": field_json(&f.number),
            "pages": field_json(&f.pages),
            "doi": field_json(&f.doi),
            "arxiv": field_json(&f.arxiv),
            "url": field_json(&f.url),
            "mr": field_json(&f.mr),
            "zbl": field_json(&f.zbl),
        },
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

    /// The document's referable blocks (`doc_blocks`). Bibliography entries only when `kinds` asks
    /// for them (`doc_bibliography` lists them with their fields).
    pub(crate) fn doc_blocks(&self, a: &Args) -> Result<Value> {
        let kinds: Option<Vec<BlockKind>> = match a.get("kinds") {
            None => None,
            Some(_) => Some(
                a.strs("kinds")?
                    .into_iter()
                    .map(|k| {
                        BlockKind::from_name(k).ok_or_else(|| {
                            ToolError::InvalidArgs(format!("kinds: `{k}` is none of statement, proof, heading, equation, float, bib_entry"))
                        })
                    })
                    .collect::<Result<_>>()?,
            ),
        };
        let g = self.graph(a)?;
        let wanted = |k: BlockKind| kinds.as_ref().map_or(k != BlockKind::BibEntry, |ks| ks.contains(&k));
        let blocks: Vec<Value> = g.blocks.iter().filter(|b| wanted(b.kind)).map(block_json).collect();
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
        let citations = a.opt_bool("citations")?.unwrap_or(true);
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
            .filter(|m| page.is_none_or(|p| !m.extent.is_empty() && m.page() == p))
            .filter(|m| citations || m.kind != "citation")
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

    /// The document's bibliography (`doc_bibliography`).
    pub(crate) fn doc_bibliography(&self, a: &Args) -> Result<Value> {
        let with_citations = a.opt_bool("citations")?.unwrap_or(false);
        let only = match a.opt_int("entry")? {
            None => None,
            Some(n) if n >= 0 => Some(n as usize),
            Some(n) => return Err(ToolError::InvalidArgs(format!("entry must be 0 or more, not {n}"))),
        };
        let g = self.graph(a)?;
        let b = &g.bibliography;
        if let Some(n) = only.filter(|n| *n >= b.entries.len()) {
            return Err(ToolError::InvalidArgs(format!("entry {n} does not exist: the bibliography has {} entries (from 0)", b.entries.len())));
        }
        let cites: Vec<&RefMention> = g.mentions.iter().filter(|m| m.kind == "citation").collect();
        let entry_of = |m: &RefMention| m.target.and_then(|t| g.entry_of_block(t));
        let entries: Vec<Value> = b
            .entries
            .iter()
            .filter(|e| only.is_none_or(|n| n == e.id))
            .map(|e| entry_json(&g, e, cites.iter().filter(|m| entry_of(m) == Some(e.id)).count()))
            .collect();
        let region = b.region.as_ref().map(|r| {
            json!({
                "heading": r.title,
                "heading_extent": r.heading.as_ref().map(|h| extent_json(&Extent { page: h.page, rect: h.rect })),
                "first_page": r.first.0 + 1,
                "last_page": r.last.0 + 1,
                "confidence": r2(r.confidence),
            })
        });
        let mut out = json!({
            "found": b.region.is_some(),
            "region": region,
            "entries": entries,
            "citation_count": cites.len(),
            "pages_read": g.pages_read,
            "truncated": g.truncated,
            "diagnostics": diagnostics(&g),
        });
        if with_citations && let Some(o) = out.as_object_mut() {
            let list: Vec<Value> = cites.iter().filter(|m| only.is_none_or(|n| entry_of(m) == Some(n))).map(|m| mention_json(&g, m)).collect();
            o.insert("citations".into(), json!(list));
        }
        Ok(out)
    }
}
