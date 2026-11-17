//! `cargo xtask math-oracle`: the equation-level test oracle for LaTeX recovery.
//!
//! - `generate [--out DIR] [--check] [--keep]` typesets the equation list
//!   (`crates/render/tests/data/math/oracle/equations.toml`) with pdfTeX and writes the PDFs and
//!   the truth next to it. `--check` writes to a temporary directory and fails unless every file
//!   is byte-identical with the committed one; `--keep` leaves TeX's files in place. Needs TeX Live (`pdflatex`, `dvitype`,
//!   `kpsewhich`); nothing else does.
//! - `report [ARGS]` scores recoverers per category, tier and context
//!   (`crates/render/examples/math_oracle.rs`; `--help` there lists the arguments).
//! - `faces [--only ID] [--check]` typesets the list in other fonts (`math_oracle/faces.rs`): OpenType
//!   math fonts with LuaLaTeX and XeLaTeX, Times with pdfLaTeX. Only faces whose fonts are all
//!   OFL-1.1 are committed; the others go to the git-ignored `target/math-oracle-faces/`.
//! - `test` runs the oracle tests that need TeX (`#[ignore]`d in the plain test run, because the CI
//!   runners require every oracle and do not install TeX Live).
//!
//! The truth comes from DVI, not from the PDF: each document is run twice, to DVI (where
//! `dvitype` lists every glyph and rule, and the `\special` brackets around each equation say
//! which ones it owns) and to PDF. The generator checks that the PDF is byte-identical when the
//! brackets expand to nothing, so pdfTeX in PDF mode really ignores them; that every embedded font
//! is an AMS Type 1 font (OFL-1.1); that every box is on its page and holds no glyph of anything
//! else; that an equation in running text typesets as it does alone; and that every deliberate
//! breakage changes the typeset.

pub(crate) mod faces;
pub(crate) mod tfm;

use crate::gates::root;
use anyhow::{Context, bail};
use pdfcraft_oracle::tex::{
    self, DviPage, Env, EquationTruth, Item, Mark, MutantTruth, Mutation, Output, Owner, PageTruth, Tex, TruthSet, Verdict, WorkDir,
};
use serde::Deserialize;
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

/// The equation list and the generated files, relative to the repository root.
pub const DATA_DIR: &str = "crates/render/tests/data/math/oracle";
/// The files `generate` writes, all byte-reproducible.
pub const OUTPUTS: [&str; 6] = ["equations.tex", "equations.pdf", "paragraphs.tex", "paragraphs.pdf", "truth.tsv", "mutants.tsv"];
/// Paper of the paragraph pages, in bp.
const PARAGRAPH_PAGE_BP: [f64; 2] = [440.0, 340.0];
const CATEGORIES: [&str; 19] = [
    "scripts",
    "fractions",
    "radicals",
    "bigops",
    "integrals",
    "operators",
    "delimiters",
    "matrices",
    "cases",
    "alignment",
    "accents",
    "decorations",
    "alphabets",
    "text",
    "greek",
    "relations",
    "arrows",
    "dots_primes",
    "spacing",
];
/// Where pdfTeX must have found every font it embedded: the AMS Type 1 fonts (OFL-1.1,
/// `crates/render/tests/data/math/OFL-AMSFonts.txt`).
const AMS_FONT_DIR: &str = "/fonts/type1/public/amsfonts/";
/// How far each equation's box reaches past TeX's, in sp (0.05 bp): pdfTeX rounds positions in the
/// PDF to 0.001 bp, so a glyph on the edge of TeX's box can land just outside it.
const BOX_MARGIN_SP: i64 = 3289;
/// Longest equation source accepted from the list.
const MAX_TEX_BYTES: usize = 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct List {
    equation: Vec<Equation>,
    paragraph_page: Vec<ParagraphPage>,
}

#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields)]
struct Equation {
    id: String,
    category: String,
    tier: u8,
    env: String,
    tex: String,
    hard: Option<String>,
    /// Held out: not used to develop a recoverer. The id starts with `ho_` exactly when this is true.
    holdout: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ParagraphPage {
    id: String,
    columns: u8,
    text: String,
}

pub fn run(args: &[String]) -> anyhow::Result<()> {
    let rest = args.get(1..).unwrap_or_default();
    match args.first().map(String::as_str) {
        Some("generate") => generate(rest),
        Some("faces") => faces::run(rest),
        Some("report") => cargo(&["run", "-q", "-p", "pdfcraft-render", "--example", "math_oracle", "--"], rest),
        Some("test") => {
            let extra: Vec<String> = std::iter::once("--include-ignored".to_owned()).chain(rest.iter().cloned()).collect();
            cargo(&["test", "-p", "pdfcraft-render", "--test", "it", "math_", "--"], &extra)
        }
        _ => bail!("usage: cargo xtask math-oracle generate [--out DIR] [--check] | faces [--only ID] [--check] | report [ARGS] | test"),
    }
}

pub(crate) fn cargo(fixed: &[&str], extra: &[String]) -> anyhow::Result<()> {
    let mut c = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
    c.current_dir(root()).args(fixed).args(extra);
    let status = c.status().context("running cargo")?;
    if !status.success() {
        bail!("cargo {} exited with {status}", fixed.join(" "));
    }
    Ok(())
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

fn read_list(path: &Path) -> anyhow::Result<List> {
    let list: List = toml::from_str(&std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?)
        .with_context(|| format!("parsing {}", path.display()))?;
    let mut ids = HashSet::new();
    for e in &list.equation {
        let at = |what: &str| format!("equation `{}`: {what}", e.id);
        if !valid_id(&e.id) || !ids.insert(e.id.as_str()) {
            bail!("{}", at("the id must be unique, [a-z0-9_]"));
        }
        if e.holdout.unwrap_or(false) != e.id.starts_with("ho_") {
            bail!("{}", at("a held-out equation says `holdout = true` and its id starts with `ho_`; no other does"));
        }
        if !CATEGORIES.contains(&e.category.as_str()) {
            bail!("{}", at(&format!("unknown category; one of {CATEGORIES:?}")));
        }
        if !(1..=3).contains(&e.tier) || (e.tier == 3) != e.hard.is_some() {
            bail!("{}", at("tier is 1, 2 or 3, and `hard` says why exactly when it is 3"));
        }
        if Env::parse(&e.env).is_none() {
            bail!("{}", at("unknown env"));
        }
        if e.tex.trim().is_empty() || e.tex.len() > MAX_TEX_BYTES || e.tex.contains(['\t', '\n', '\r', '%']) || e.tex.contains("pdfcraft") {
            bail!("{}", at("tex is one line, without tabs, `%` or the oracle's own macros"));
        }
    }
    let mut pages = HashSet::new();
    for p in &list.paragraph_page {
        if !valid_id(&p.id) || !pages.insert(p.id.as_str()) || !(1..=2).contains(&p.columns) {
            bail!("paragraph page `{}`: unique [a-z0-9_] id, 1 or 2 columns", p.id);
        }
    }
    Ok(list)
}

/// The paragraph page's text with each `{{id}}` set as that equation, bracketed as `id@page`.
fn paragraph_body(page: &ParagraphPage, equations: &BTreeMap<&str, &Equation>) -> anyhow::Result<(String, Vec<String>)> {
    let mut out = String::new();
    let mut placed = Vec::new();
    let mut rest = page.text.as_str();
    loop {
        let (prose, after) = match rest.split_once("{{") {
            Some((p, a)) => (p, Some(a)),
            None => (rest, None),
        };
        if let Some(c) = prose.chars().find(|c| "\\$%#&^_{}~".contains(*c)) {
            bail!("paragraph page `{}`: the prose has `{c}`; it must be plain text", page.id);
        }
        out.push_str(prose);
        let Some(after) = after else { break };
        let (name, tail) = after.split_once("}}").with_context(|| format!("paragraph page `{}`: an unclosed {{{{", page.id))?;
        if name == "columnbreak" {
            if page.columns != 2 {
                bail!("paragraph page `{}`: a column break on a one-column page", page.id);
            }
            out.push_str("\\newpage\n");
        } else {
            let e = equations.get(name).with_context(|| format!("paragraph page `{}`: no equation `{name}`", page.id))?;
            let env = Env::parse(&e.env).context("env")?;
            let occurrence = format!("{name}@{}", page.id);
            out.push_str(&tex::wrap(env, &occurrence, &e.tex));
            placed.push(occurrence);
        }
        rest = tail;
    }
    Ok((out, placed))
}

fn paragraphs_source(pages: &[ParagraphPage], equations: &BTreeMap<&str, &Equation>, specials: bool) -> anyhow::Result<(String, Vec<Vec<String>>)> {
    let [w, h] = PARAGRAPH_PAGE_BP;
    let mut s = tex::preamble(&format!("paperwidth={w}bp,paperheight={h}bp,margin=24bp"), specials);
    // No line breaks inside inline equations (TeX's default allows them after relations and binary
    // operators), so each occurrence stays on one line; its glue still stretches with the line.
    // `\sloppy` lets the narrow columns break before such a block instead of running over.
    s.push_str("\\relpenalty=10000\n\\binoppenalty=10000\n\\sloppy\n\\begin{document}\n");
    let mut placed = Vec::new();
    for p in pages {
        s.push_str(if p.columns == 2 { "\\twocolumn\n" } else { "\\onecolumn\n" });
        let (body, ids) = paragraph_body(p, equations)?;
        s.push_str(&body);
        s.push_str("\n\\clearpage\n");
        placed.push(ids);
    }
    s.push_str("\\end{document}\n");
    Ok((s, placed))
}

/// A document typeset to DVI (listed) and to PDF, with and without brackets.
pub(crate) struct Built {
    pub(crate) pages: Vec<DviPage>,
    pub(crate) pdf: Vec<u8>,
}

fn build(tex: &Tex, work: &Path, stem: &str, source: &str, plain_source: &str) -> anyhow::Result<Built> {
    build_with(tex, work, stem, source, plain_source, &[])
}

/// [`build`] with more files beside the source: `(name, bracketed, plain)` (the layout oracle's
/// package, with and without its specials).
pub(crate) fn build_with(
    tex: &Tex,
    work: &Path,
    stem: &str,
    source: &str,
    plain_source: &str,
    extra: &[(&str, &str, &str)],
) -> anyhow::Result<Built> {
    let dir = work.join(stem);
    let plain = dir.join("plain");
    std::fs::create_dir_all(&plain)?;
    std::fs::write(dir.join(format!("{stem}.tex")), source)?;
    std::fs::write(plain.join(format!("{stem}.tex")), plain_source)?;
    for (name, bracketed, unbracketed) in extra {
        std::fs::write(dir.join(name), bracketed)?;
        std::fs::write(plain.join(name), unbracketed)?;
    }
    let timeout = Duration::from_secs(300);
    tex.compile(&dir, stem, Output::Dvi, timeout)?;
    let pages = tex.list(&dir, stem)?;
    tex.compile(&dir, stem, Output::Pdf, timeout)?;
    tex.compile(&plain, stem, Output::Pdf, timeout)?;
    let pdf = std::fs::read(dir.join(format!("{stem}.pdf")))?;
    if pdf != std::fs::read(plain.join(format!("{stem}.pdf")))? {
        bail!("{stem}.pdf differs when the \\special brackets expand to nothing: pdfTeX did not ignore them");
    }
    let log = std::fs::read_to_string(dir.join(format!("{stem}.log")))?;
    let fonts: Vec<&str> = log.split('<').filter_map(|s| s.split_once('>').map(|(f, _)| f)).filter(|f| f.ends_with(".pfb")).collect();
    if fonts.is_empty() {
        bail!("{stem}.log names no embedded Type 1 font");
    }
    if let Some(f) = fonts.iter().find(|f| !f.contains(AMS_FONT_DIR)) {
        bail!("{stem}.pdf embeds {f}, which is not an AMS Type 1 font (OFL-1.1)");
    }
    Ok(Built { pages, pdf })
}

/// The box of `items` in sp: glyphs by their TFM boxes (italic correction included, negative
/// dimensions taken as zero), rules as set, widened by [`BOX_MARGIN_SP`].
fn bbox_sp(items: &[Item], fonts: &mut tfm::Fonts) -> anyhow::Result<[i64; 4]> {
    let mut b = [i64::MAX, i64::MAX, i64::MIN, i64::MIN];
    for i in items {
        let [x0, y0, x1, y1] = match &i.mark {
            Mark::Glyph { font, size, code } => {
                let c = fonts.get(font)?.char_box(*code, *size).with_context(|| format!("{font} has no character {code}"))?;
                // A dimension can be negative (cmsy7's `\sim` has a negative depth: it sits above its
                // baseline); the box still holds the glyph's reference point.
                let (w, ht, dp) = (c.width.max(0), c.height.max(0), c.depth.max(0));
                [i.h, i.v.saturating_sub(ht), i.h.saturating_add(w).saturating_add(c.italic.max(0)), i.v.saturating_add(dp)]
            }
            Mark::Rule { width, height } => [i.h, i.v.saturating_sub(*height), i.h.saturating_add(*width), i.v],
        };
        b = [b[0].min(x0), b[1].min(y0), b[2].max(x1), b[3].max(y1)];
    }
    if b[0] > b[2] {
        bail!("an equation with nothing in it");
    }
    let m = BOX_MARGIN_SP;
    Ok([b[0].saturating_sub(m), b[1].saturating_sub(m), b[2].saturating_add(m), b[3].saturating_add(m)])
}

/// The box in view-space bp, rounded outwards to the thousandth the truth file keeps.
fn view_box(sp: [i64; 4]) -> [f64; 4] {
    let [x0, y0, x1, y1] = sp.map(tex::view_bp);
    [(x0 * 1000.0).floor() / 1000.0, (y0 * 1000.0).floor() / 1000.0, (x1 * 1000.0).ceil() / 1000.0, (y1 * 1000.0).ceil() / 1000.0]
}

fn owner_name(page: &DviPage, owner: Owner) -> String {
    match owner {
        Owner::Outside => "-".to_owned(),
        Owner::Tag => "tag".to_owned(),
        Owner::Equation(i) => page.equations.get(i).cloned().unwrap_or_default(),
    }
}

/// Checks that the box of equation `id` is on the page and that nothing else on it sits inside:
/// no other glyph's reference point, no other rule's ink.
fn check_isolated(page: &DviPage, id: &str, sp: [i64; 4], view: [f64; 4], paper: [f64; 2]) -> anyhow::Result<()> {
    if view[0] < 0.0 || view[1] < 0.0 || view[2] > paper[0] || view[3] > paper[1] {
        bail!("`{id}`: its box {view:?} is not inside the {paper:?} bp page");
    }
    let index = page.equations.iter().position(|e| e == id);
    for (item, owner) in &page.items {
        if index.is_some_and(|i| *owner == Owner::Equation(i)) {
            continue;
        }
        let inside = match item.mark {
            Mark::Glyph { .. } => item.h > sp[0] && item.h < sp[2] && item.v > sp[1] && item.v < sp[3],
            Mark::Rule { width, height } => {
                item.h < sp[2] && item.h.saturating_add(width) > sp[0] && item.v.saturating_sub(height) < sp[3] && item.v > sp[1]
            }
        };
        if inside {
            bail!("`{id}`: {} ({}) lies inside its box", item.mark, owner_name(page, *owner));
        }
    }
    Ok(())
}

fn page_truth(doc: &str, index: usize, page: &DviPage) -> PageTruth {
    PageTruth { doc: doc.to_owned(), page: index, items: page.items.iter().map(|(i, o)| (i.clone(), owner_name(page, *o))).collect() }
}

fn generate(args: &[String]) -> anyhow::Result<()> {
    let scratch = WorkDir::new("math-oracle")?;
    if args.iter().any(|a| a == "--keep") {
        let kept = scratch.keep();
        eprintln!("math-oracle: keeping TeX's files in {}", kept.display());
        return generate_in(args, &kept);
    }
    generate_in(args, scratch.path())
}

fn generate_in(args: &[String], scratch: &Path) -> anyhow::Result<()> {
    let data = root().join(DATA_DIR);
    let check = args.iter().any(|a| a == "--check");
    let out: PathBuf = match args.iter().position(|a| a == "--out").and_then(|i| args.get(i + 1)) {
        Some(dir) => PathBuf::from(dir),
        None if check => scratch.join("out"),
        None => data.clone(),
    };
    let list = read_list(&data.join("equations.toml"))?;
    let tex = Tex::find().map_err(|absent| anyhow::anyhow!("{absent}"))?;
    let by_id: BTreeMap<&str, &Equation> = list.equation.iter().map(|e| (e.id.as_str(), e)).collect();
    let mut fonts = tfm::Fonts::default();

    // The equations, one per page.
    let mut entries = Vec::new();
    for e in &list.equation {
        entries.push((e.id.as_str(), Env::parse(&e.env).context("env")?, e.tex.as_str()));
    }
    let single = build(&tex, scratch, "equations", &tex::single_pages_source(&entries, true), &tex::single_pages_source(&entries, false))?;
    if single.pages.len() != entries.len() {
        bail!("equations.tex made {} pages for {} equations: one does not fit its page", single.pages.len(), entries.len());
    }
    let mut set = TruthSet::default();
    let mut alone: BTreeMap<&str, Vec<Item>> = BTreeMap::new();
    for (index, (e, page)) in list.equation.iter().zip(&single.pages).enumerate() {
        if page.equations != [e.id.as_str()] {
            bail!("page {index} of equations.pdf brackets {:?}, not `{}`", page.equations, e.id);
        }
        if let Some((item, _)) = page.items.iter().find(|(_, o)| *o == Owner::Outside) {
            bail!("`{}`: {} is set outside the equation", e.id, item.mark);
        }
        let items = page.equation_items(&e.id).unwrap_or_default();
        let sp = bbox_sp(&items, &mut fonts).with_context(|| e.id.clone())?;
        let bbox = view_box(sp);
        check_isolated(page, &e.id, sp, bbox, tex::SINGLE_PAGE_BP)?;
        alone.insert(e.id.as_str(), items.clone());
        set.equations.push(truth_of(e, &e.id, "equations", index, "alone", bbox, items)?);
        set.pages.push(page_truth("equations", index, page));
    }

    // The paragraph pages.
    let (para_source, placed) = paragraphs_source(&list.paragraph_page, &by_id, true)?;
    let (plain_source, _) = paragraphs_source(&list.paragraph_page, &by_id, false)?;
    let para = build(&tex, scratch, "paragraphs", &para_source, &plain_source)?;
    let log = std::fs::read_to_string(scratch.join("paragraphs").join("paragraphs.log"))?;
    if let Some(line) = log.lines().find(|l| l.starts_with("Overfull")) {
        bail!("paragraphs.tex: {line}; reword the prose so the line breaks");
    }
    if para.pages.len() != list.paragraph_page.len() {
        bail!("paragraphs.tex made {} pages for {} paragraph pages: one overflows", para.pages.len(), list.paragraph_page.len());
    }
    for (index, ((p, ids), page)) in list.paragraph_page.iter().zip(&placed).zip(&para.pages).enumerate() {
        if &page.equations != ids {
            bail!("page {index} of paragraphs.pdf brackets {:?}, not {ids:?}", page.equations);
        }
        for occurrence in ids {
            let base = occurrence.split('@').next().unwrap_or_default();
            let e = by_id.get(base).context("base equation")?;
            let items = page.equation_items(occurrence).unwrap_or_default();
            if let Verdict::Different(why) = tex::same_up_to_glue(alone.get(base).map_or(&[][..], Vec::as_slice), &items) {
                bail!("`{occurrence}` typesets differently in running text than alone ({why}); a line may break inside it");
            }
            let sp = bbox_sp(&items, &mut fonts).with_context(|| occurrence.clone())?;
            let bbox = view_box(sp);
            check_isolated(page, occurrence, sp, bbox, PARAGRAPH_PAGE_BP)?;
            let context = if p.columns == 2 { "2col" } else { "1col" };
            set.equations.push(truth_of(e, occurrence, "paragraphs", index, context, bbox, items)?);
        }
        set.pages.push(page_truth("paragraphs", index, page));
    }

    // The deliberate breakages, typeset the way a candidate is.
    let mut mutants: Vec<(String, &Equation, Mutation, String)> = Vec::new();
    for e in &list.equation {
        for m in Mutation::ALL {
            if let Some(src) = m.apply(&e.tex) {
                mutants.push((format!("{}/{}", e.id, m.name()), e, m, src));
            }
        }
    }
    let typeset_entries: Vec<(Env, &str)> = mutants.iter().map(|(_, e, _, src)| (Env::parse(&e.env).unwrap_or(Env::Display), src.as_str())).collect();
    let typeset = tex.typeset(&typeset_entries)?;
    let mut mutant_set = TruthSet::default();
    for (k, ((id, e, m, src), result)) in mutants.iter().zip(typeset).enumerate() {
        let items = result.map_err(|why| anyhow::anyhow!("mutant `{id}` ({src}) does not typeset: {why}"))?;
        if tex::same_typeset(alone.get(e.id.as_str()).map_or(&[][..], Vec::as_slice), &items) == Verdict::Equal {
            bail!("mutant `{id}` ({src}) typesets like the original: the check could not tell them apart");
        }
        mutant_set.mutants.push(MutantTruth {
            id: id.clone(),
            base: e.id.clone(),
            mutation: m.name().to_owned(),
            source: src.clone(),
            items: items.clone(),
        });
        mutant_set.pages.push(PageTruth { doc: "mutants".to_owned(), page: k, items: items.into_iter().map(|i| (i, id.clone())).collect() });
    }

    // Write.
    std::fs::create_dir_all(&out)?;
    let header = format!(
        "Generated by `cargo xtask math-oracle generate` from equations.toml with {}. Do not edit.\n\
         Rows: see pdfcraft_oracle::tex::TruthSet. Positions in sp from the DVI origin; boxes in view-space bp.",
        tex.version()
    );
    let files: [(&str, Vec<u8>); 6] = [
        ("equations.tex", tex::single_pages_source(&entries, true).into_bytes()),
        ("equations.pdf", single.pdf),
        ("paragraphs.tex", para_source.into_bytes()),
        ("paragraphs.pdf", para.pdf),
        ("truth.tsv", set.to_tsv(&header).into_bytes()),
        ("mutants.tsv", mutant_set.to_tsv(&header).into_bytes()),
    ];
    for (name, bytes) in &files {
        std::fs::write(out.join(name), bytes).with_context(|| format!("writing {name}"))?;
    }
    print_counts(&list, &set, &mutant_set);
    if check {
        let differ: Vec<&str> =
            OUTPUTS.iter().copied().filter(|name| std::fs::read(out.join(name)).ok() != std::fs::read(data.join(name)).ok()).collect();
        if !differ.is_empty() {
            bail!("generated files differ from the committed ones: {differ:?}");
        }
        println!("math-oracle: every generated file is byte-identical with {DATA_DIR}");
    } else {
        println!("math-oracle: wrote {} files to {}", files.len(), out.display());
    }
    Ok(())
}

fn truth_of(e: &Equation, id: &str, doc: &str, page: usize, context: &str, bbox: [f64; 4], items: Vec<Item>) -> anyhow::Result<EquationTruth> {
    Ok(EquationTruth {
        id: id.to_owned(),
        base: e.id.clone(),
        doc: doc.to_owned(),
        page,
        category: e.category.clone(),
        tier: e.tier,
        env: Env::parse(&e.env).context("env")?,
        context: context.to_owned(),
        bbox,
        source: e.tex.clone(),
        reference: Vec::new(),
        items,
    })
}

fn print_counts(list: &List, set: &TruthSet, mutants: &TruthSet) {
    let mut table: BTreeMap<&str, [usize; 3]> = BTreeMap::new();
    for e in &list.equation {
        if let Some(slot) = table.entry(e.category.as_str()).or_default().get_mut(usize::from(e.tier.saturating_sub(1))) {
            *slot += 1;
        }
    }
    println!("{:<14} {:>6} {:>6} {:>6} {:>6}", "category", "tier 1", "tier 2", "tier 3", "all");
    let mut totals = [0usize; 3];
    for (category, tiers) in &table {
        println!("{category:<14} {:>6} {:>6} {:>6} {:>6}", tiers[0], tiers[1], tiers[2], tiers.iter().sum::<usize>());
        for (t, n) in totals.iter_mut().zip(tiers) {
            *t += n;
        }
    }
    println!("{:<14} {:>6} {:>6} {:>6} {:>6}", "all", totals[0], totals[1], totals[2], totals.iter().sum::<usize>());
    let in_text = set.equations.iter().filter(|e| e.context != "alone").count();
    let mut by_mutation: BTreeMap<&str, usize> = BTreeMap::new();
    for m in &mutants.mutants {
        *by_mutation.entry(m.mutation.as_str()).or_default() += 1;
    }
    println!("{} occurrences ({in_text} in running text); mutants: {by_mutation:?}", set.equations.len());
}
