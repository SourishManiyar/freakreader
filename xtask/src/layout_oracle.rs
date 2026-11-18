//! `cargo xtask layout-oracle`: the page-layout oracle (`crates/render/tests/data/layout/oracle`).
//!
//! - `generate [--check] [--keep]` writes each paper of `papers.toml` as LaTeX (the prose is made up
//!   here, from a seeded generator and word lists written for this repository), typesets it with
//!   pdfTeX to DVI and PDF, and writes the PDFs, the sources, the bracket package and `truth.tsv`.
//!   `--check` writes to a temporary directory and fails unless every file is byte-identical with
//!   the committed one. Needs TeX Live; nothing else does.
//! - `report [ARGS]` scores the analysis (`crates/render/examples/layout_oracle.rs`).
//! - `test` runs the layout oracle's tests.
//!
//! The truth comes from DVI ([`pdfcraft_oracle::tex::layout`]): structure is bracketed by
//! `\special`s that `dvitype` lists in shipping order. Each PDF is compiled again with the bracket
//! package reduced to empty macros and must come out byte-identical, so the specials and the patches
//! that place them moved nothing; every embedded font must be an AMS Type 1 font (OFL-1.1).

use crate::gates::root;
use crate::tex_oracle::{build_with, cargo};
use anyhow::{Context, bail};
use pdfcraft_oracle::tex::layout::{self, Kind, LayoutTruth, PaperInfo};
use pdfcraft_oracle::tex::{Tex, WorkDir};
use serde::Deserialize;
use std::collections::HashSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// The paper list and the generated files, relative to the repository root.
pub const DATA_DIR: &str = "crates/render/tests/data/layout/oracle";
pub(crate) const PACKAGE: &str = "pdfcraftlayout.sty";
/// Most pages one paper may have.
const MAX_PAGES: usize = 12;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct List {
    paper: Vec<Paper>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Paper {
    id: String,
    /// Held out: written before the analysis, scored only by its rates. The id starts `ho_`.
    holdout: bool,
    class: String,
    options: Vec<String>,
    geometry: Option<String>,
    pagestyle: String,
    seed: u64,
    body: String,
}

pub fn run(args: &[String]) -> anyhow::Result<()> {
    let rest = args.get(1..).unwrap_or_default();
    match args.first().map(String::as_str) {
        Some("generate") => generate(rest),
        Some("report") => cargo(&["run", "-q", "-p", "pdfcraft-render", "--example", "layout_oracle", "--"], rest),
        Some("test") => {
            let extra: Vec<String> = std::iter::once("--include-ignored".to_owned()).chain(rest.iter().cloned()).collect();
            cargo(&["test", "-p", "pdfcraft-render", "--test", "it", "layout_", "--"], &extra)
        }
        _ => bail!("usage: cargo xtask layout-oracle generate [--check] [--keep] | report [ARGS] | test"),
    }
}

/// xorshift64*, so every run writes the same papers.
pub(crate) struct Rng(u64);

impl Rng {
    pub(crate) fn new(seed: u64) -> Rng {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    pub(crate) fn below(&mut self, n: usize) -> usize {
        if n == 0 { 0 } else { (self.next() % n as u64) as usize }
    }
    pub(crate) fn pick<'a>(&mut self, list: &[&'a str]) -> &'a str {
        list.get(self.below(list.len())).copied().unwrap_or("")
    }
    pub(crate) fn chance(&mut self, percent: usize) -> bool {
        self.below(100) < percent
    }
}

// Word lists for the made-up prose: plain mathematical English, written for this repository.
pub(crate) const ADJ: [&str; 24] = [
    "bounded",
    "compact",
    "separable",
    "positive",
    "measurable",
    "finite",
    "smooth",
    "convex",
    "symmetric",
    "regular",
    "uniform",
    "local",
    "closed",
    "dense",
    "invariant",
    "normal",
    "stable",
    "linear",
    "periodic",
    "weak",
    "strong",
    "admissible",
    "reduced",
    "natural",
];
pub(crate) const NOUN: [&str; 24] = [
    "operator",
    "space",
    "sequence",
    "measure",
    "function",
    "bound",
    "estimate",
    "kernel",
    "domain",
    "norm",
    "partition",
    "family",
    "set",
    "map",
    "solution",
    "series",
    "integral",
    "subspace",
    "graph",
    "process",
    "matrix",
    "group",
    "condition",
    "constant",
];
const VERB: [&str; 14] = [
    "is bounded by",
    "converges to",
    "depends only on",
    "is controlled by",
    "agrees with",
    "is dominated by",
    "is close to",
    "extends",
    "determines",
    "is equivalent to",
    "contains",
    "acts on",
    "preserves",
    "approximates",
];
const OPENER: [&str; 14] = [
    "In particular",
    "Moreover",
    "By the previous remark",
    "For this reason",
    "On the other hand",
    "As a consequence",
    "In the same way",
    "Conversely",
    "By construction",
    "Under this assumption",
    "In general",
    "For the same reason",
    "As before",
    "By symmetry",
];
pub(crate) const MATH: [&str; 24] = [
    "$T$",
    "$f$",
    "$x_n$",
    "$\\lambda$",
    "$\\|T\\|$",
    "$L^2(\\Omega)$",
    "$\\mu(A)$",
    "$k \\ge 1$",
    "$\\varepsilon > 0$",
    "$a_{ij}$",
    "$\\sigma(T)$",
    "$n^2$",
    "$f(x)$",
    "$\\phi_k$",
    "$C_0$",
    "$\\alpha + \\beta$",
    "$\\mathbb{R}^d$",
    "$s_n(T)$",
    "$u_t$",
    "$\\frac{1}{n}$",
    "$e^{-t}$",
    "$\\Delta u$",
    "$p < q$",
    "$\\langle x, y \\rangle$",
];
pub(crate) const TITLE_WORDS: [&str; 12] =
    ["Remarks", "Bounds", "Estimates", "Notes", "Results", "Questions", "Methods", "Examples", "Limits", "Properties", "Spectra", "Averages"];

pub(crate) fn sentence(rng: &mut Rng) -> String {
    let mut s = String::new();
    if rng.chance(35) {
        let _ = write!(s, "{}, ", rng.pick(&OPENER));
    }
    let subject = if rng.chance(50) {
        format!("the {} {} {}", rng.pick(&ADJ), rng.pick(&NOUN), rng.pick(&MATH))
    } else {
        format!("every {} {}", rng.pick(&ADJ), rng.pick(&NOUN))
    };
    let object = if rng.chance(60) {
        format!("a {} {} of {}", rng.pick(&ADJ), rng.pick(&NOUN), rng.pick(&MATH))
    } else {
        format!("the {} {}", rng.pick(&NOUN), rng.pick(&MATH))
    };
    let _ = write!(s, "{subject} {} {object}", rng.pick(&VERB));
    if rng.chance(45) {
        let _ = write!(s, " whenever {} {} {}", rng.pick(&MATH), rng.pick(&VERB), rng.pick(&MATH));
    }
    if rng.chance(30) {
        let _ = write!(s, ", and the {} is {}", rng.pick(&NOUN), rng.pick(&ADJ));
    }
    s.push('.');
    let mut chars = s.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().chain(chars).collect(),
        None => s,
    }
}

pub(crate) fn sentences(rng: &mut Rng, n: usize) -> String {
    (0..n).map(|_| sentence(rng)).collect::<Vec<_>>().join(" ")
}

/// Displays of our own, by kind.
pub(crate) const EQUATIONS: [&str; 8] = [
    "\\sum_{n=1}^{\\infty} \\frac{s_n(T)^{2}}{n} \\le C \\|T\\|_{2}^{2}",
    "\\int_{0}^{1} |f(t)|^{2} \\, dt = \\lim_{k \\to \\infty} \\frac{1}{k} \\sum_{j=1}^{k} a_j",
    "u(x) = \\frac{1}{2\\pi} \\int_{\\mathbb{R}} \\hat{u}(\\xi) e^{i x \\xi} \\, d\\xi",
    "\\|T x\\| \\le \\left( \\sum_{k=1}^{n} \\lambda_k^{2} \\right)^{1/2} \\|x\\|",
    "\\mu(A \\cap B) \\ge \\mu(A) + \\mu(B) - 1",
    "\\phi_k(t) = \\sqrt{\\frac{2}{\\pi}} \\sin(k t)",
    "\\det(I - zT) = \\prod_{n \\ge 1} (1 - z \\lambda_n)",
    "E_n = \\max_{1 \\le j \\le n} |x_j - y_j|",
];
pub(crate) const ALIGNS: [&str; 3] = [
    "a_n &= \\sum_{k=0}^{n} \\binom{n}{k} b_k \\\\ b_n &= \\frac{1}{n+1} \\sum_{k=0}^{n} a_k",
    "\\|u\\|_{1} &\\le \\|u\\|_{2} \\\\ &\\le C \\|\\nabla u\\|_{2}",
    "f(x) &= x^{2} + 1 \\\\ g(x) &= \\frac{f(x)}{1 + x^{2}} \\\\ h(x) &= f(g(x))",
];
const WIDE: [&str; 2] = [
    "\\sum_{j=1}^{m} \\sum_{k=1}^{n} a_{jk} x_j y_k \\le \\left( \\sum_{j=1}^{m} x_j^{2} \\right)^{1/2} \\left( \\sum_{k=1}^{n} y_k^{2} \\right)^{1/2} \\max_{j,k} |a_{jk}|",
    "\\int_{0}^{\\infty} \\int_{0}^{\\infty} \\frac{f(s) g(t)}{s + t} \\, ds \\, dt \\le \\pi \\left( \\int_{0}^{\\infty} f^{2} \\right)^{1/2} \\left( \\int_{0}^{\\infty} g^{2} \\right)^{1/2}",
];

struct Writer<'a> {
    paper: &'a Paper,
    rng: Rng,
    out: String,
    displays: usize,
    twocolumn: bool,
}

impl Writer<'_> {
    fn display(&mut self, env: &str) -> anyhow::Result<()> {
        self.displays += 1;
        let id = format!("{}_d{}", self.paper.id, self.displays);
        let (open, close, body) = match env {
            "equation" | "equation*" | "gather" => {
                (format!("\\begin{{{env}}}"), format!("\\end{{{env}}}"), EQUATIONS.get(self.rng.below(EQUATIONS.len())).copied().unwrap_or(""))
            }
            "display" => ("\\[".to_owned(), "\\]".to_owned(), EQUATIONS.get(self.rng.below(EQUATIONS.len())).copied().unwrap_or("")),
            "align" | "align*" => {
                (format!("\\begin{{{env}}}"), format!("\\end{{{env}}}"), ALIGNS.get(self.rng.below(ALIGNS.len())).copied().unwrap_or(""))
            }
            "wide" => {
                let env = if self.twocolumn { "multline" } else { "equation" };
                (format!("\\begin{{{env}}}"), format!("\\end{{{env}}}"), WIDE.get(self.rng.below(WIDE.len())).copied().unwrap_or(""))
            }
            other => bail!("paper `{}`: unknown display `{other}`", self.paper.id),
        };
        let body = if env == "wide" && self.twocolumn { body.replacen("\\le", "\\\\ \\le", 1) } else { body.to_owned() };
        // On lines of their own so that no space lands beside a bracket.
        let _ = write!(self.out, "%\n{open}\\pdfcraftbegin{{{id}}}%\n{body}%\n\\pdfcraftend{{{id}}}{close}\n");
        Ok(())
    }

    fn float(&mut self, what: &str, args: &[&str]) -> anyhow::Result<()> {
        let placement = args.first().copied().unwrap_or("t");
        let wide = args.contains(&"wide");
        let env = if wide && self.twocolumn { format!("{what}*") } else { what.to_owned() };
        let width = if wide { "\\textwidth" } else { "\\columnwidth" };
        let caption = format!("{} {}", sentence(&mut self.rng).trim_end_matches('.'), "in the setting above.");
        let content = if what == "figure" {
            let labels: Vec<String> = (0..3).map(|_| format!("{} {}", self.rng.pick(&ADJ), self.rng.pick(&NOUN))).collect();
            format!(
                "\\fbox{{\\parbox{{0.8{width}}}{{\\centering\\vspace{{4ex}}{}\\\\[3ex]{}\\quad {}\\vspace{{4ex}}}}}}",
                labels.join(" "),
                self.rng.pick(&MATH),
                self.rng.pick(&MATH)
            )
        } else {
            let mut rows = String::new();
            for _ in 0..4 {
                let _ = writeln!(rows, "{} & {} & {} \\\\", self.rng.pick(&NOUN), 10 + self.rng.below(90), self.rng.below(1000));
            }
            format!("\\begin{{tabular}}{{lrr}}\\hline\nname & size & count \\\\ \\hline\n{rows}\\hline\n\\end{{tabular}}")
        };
        let cap = format!("\\pdfcraftopen{{caption}}%\n\\caption{{{caption}}}\\par\n\\pdfcraftclose{{caption}}%\n");
        let body = format!("\\pdfcraftopen{{float}}%\n{content}\\par\n\\pdfcraftclose{{float}}%\n");
        // A table's caption goes above it, a figure's below.
        let inner = if what == "table" { format!("{cap}{body}") } else { format!("{body}{cap}") };
        let _ = write!(self.out, "\\begin{{{env}}}[{placement}]\\centering\n{inner}\\end{{{env}}}\n");
        Ok(())
    }

    fn line(&mut self, line: &str) -> anyhow::Result<()> {
        let (word, rest) = line.split_once(' ').unwrap_or((line, ""));
        let args: Vec<&str> = rest.split_whitespace().collect();
        let count = || args.first().and_then(|n| n.parse::<usize>().ok()).with_context(|| format!("`{line}`: a count is missing"));
        match word {
            "title" => {
                let _ = writeln!(self.out, "\\title{{{rest}}}");
            }
            "author" => {
                let _ = writeln!(self.out, "\\author{{{rest}}}");
            }
            "date" => {
                let _ = writeln!(self.out, "\\date{{{rest}}}");
            }
            "maketitle" => self.out.push_str("\\maketitle\n"),
            "markright" => {
                let _ = writeln!(self.out, "\\markright{{{rest}}}");
            }
            "abstract" => {
                let n = count()?;
                let text = sentences(&mut self.rng, n);
                let _ = write!(self.out, "\\begin{{abstract}}\n{text}\n\\end{{abstract}}\n");
            }
            "section" | "subsection" | "subsubsection" | "section*" | "subsection*" => {
                let title = if rest.is_empty() { format!("{} {}", self.rng.pick(&TITLE_WORDS), "on the problem") } else { rest.to_owned() };
                let _ = write!(self.out, "\n\\{word}{{{title}}}\n");
            }
            "appendix" => self.out.push_str("\n\\appendix\n"),
            "par" | "cont" => {
                let n = count()?;
                let mut text = sentences(&mut self.rng, n);
                if args.contains(&"footnote") {
                    let note = sentence(&mut self.rng);
                    let at = text.find(". ").map_or(text.len(), |i| i + 1);
                    text.insert_str(at, &format!("\\footnote{{{note}}}"));
                }
                if word == "par" {
                    self.out.push('\n');
                }
                if args.contains(&"noindent") {
                    self.out.push_str("\\noindent ");
                }
                let _ = writeln!(self.out, "{text}");
            }
            "display" => self.display(args.first().copied().unwrap_or("equation"))?,
            "list" => {
                let env = args.first().copied().unwrap_or("itemize");
                if !matches!(env, "itemize" | "enumerate") {
                    bail!("`{line}`: itemize or enumerate");
                }
                let n = args.get(1).and_then(|n| n.parse::<usize>().ok()).unwrap_or(3);
                let _ = writeln!(self.out, "\\begin{{{env}}}\\pdfcraftenv{{item}}");
                for _ in 0..n {
                    let k = 1 + self.rng.below(2);
                    let _ = writeln!(self.out, "\\item {}", sentences(&mut self.rng, k));
                }
                let _ = writeln!(self.out, "\\end{{{env}}}");
            }
            "figure" | "table" => self.float(word, &args)?,
            "newpage" => self.out.push_str("\\newpage\n"),
            other => bail!("paper `{}`: unknown directive `{other}`", self.paper.id),
        }
        Ok(())
    }
}

fn source(paper: &Paper) -> anyhow::Result<String> {
    let twocolumn = paper.options.iter().any(|o| o == "twocolumn");
    let mut w = Writer { paper, rng: Rng::new(paper.seed), out: String::new(), displays: 0, twocolumn };
    let _ = writeln!(w.out, "% Generated by `cargo xtask layout-oracle generate` from papers.toml. Do not edit.");
    let _ = writeln!(w.out, "\\documentclass[{}]{{{}}}", paper.options.join(","), paper.class);
    w.out.push_str("\\usepackage{amsmath,amssymb}\n");
    if let Some(g) = &paper.geometry {
        let _ = writeln!(w.out, "\\usepackage[{g}]{{geometry}}");
    }
    let _ = write!(w.out, "\\usepackage{{pdfcraftlayout}}\n\\pagestyle{{{}}}\n", paper.pagestyle);
    // No inline formula breaks across lines, so a line's mathematics stays on it.
    // The item bullet from CMSY: the kernel's \\textbullet would load a TS1 (cm-super) font.
    w.out.push_str("\\renewcommand\\labelitemi{$\\bullet$}\n");
    w.out.push_str("\\relpenalty=10000\n\\binoppenalty=10000\n\\sloppy\n\\begin{document}\n");
    for line in paper.body.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
        w.line(line).with_context(|| format!("paper `{}`", paper.id))?;
    }
    w.out.push_str("\n\\end{document}\n");
    Ok(w.out)
}

fn read_list(path: &Path) -> anyhow::Result<List> {
    let list: List = toml::from_str(&std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?)
        .with_context(|| format!("parsing {}", path.display()))?;
    let mut ids = HashSet::new();
    for p in &list.paper {
        let ok_id = !p.id.is_empty() && p.id.len() <= 40 && p.id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
        if !ok_id || !ids.insert(p.id.as_str()) {
            bail!("paper `{}`: the id must be unique, [a-z0-9_]", p.id);
        }
        if p.holdout != p.id.starts_with("ho_") {
            bail!("paper `{}`: a held-out paper says `holdout = true` and its id starts with `ho_`; no other does", p.id);
        }
        if !matches!(p.class.as_str(), "article" | "amsart") {
            bail!("paper `{}`: class article or amsart", p.id);
        }
        if p.body.contains(['%', '\\', '$', '&', '#', '_', '^', '~']) {
            bail!("paper `{}`: the body is directives and plain words only", p.id);
        }
    }
    let holdout = list.paper.iter().filter(|p| p.holdout).count();
    if holdout * 3 < list.paper.len() {
        bail!("{holdout} of {} papers are held out; at least a third must be", list.paper.len());
    }
    Ok(list)
}

fn info(paper: &Paper) -> PaperInfo {
    let size = paper.options.iter().find(|o| o.ends_with("pt")).cloned().unwrap_or_else(|| "10pt".to_owned());
    let columns = if paper.options.iter().any(|o| o == "twocolumn") { "2col" } else { "1col" };
    PaperInfo { doc: paper.id.clone(), holdout: paper.holdout, class: paper.class.clone(), columns: columns.to_owned(), size }
}

fn generate(args: &[String]) -> anyhow::Result<()> {
    let scratch = WorkDir::new("layout-oracle")?;
    if args.iter().any(|a| a == "--keep") {
        let kept = scratch.keep();
        eprintln!("layout-oracle: keeping TeX's files in {}", kept.display());
        return generate_in(args, &kept);
    }
    generate_in(args, scratch.path())
}

fn generate_in(args: &[String], scratch: &Path) -> anyhow::Result<()> {
    let data = root().join(DATA_DIR);
    let check = args.iter().any(|a| a == "--check");
    let out: PathBuf = if check { scratch.join("out") } else { data.clone() };
    let list = read_list(&data.join("papers.toml"))?;
    let tex = Tex::find().map_err(|absent| anyhow::anyhow!("{absent}"))?;
    let (on, off) = (layout::package(true), layout::package(false));
    let mut truth = LayoutTruth::default();
    let mut files: Vec<(String, Vec<u8>)> = vec![(PACKAGE.to_owned(), on.clone().into_bytes())];
    for paper in &list.paper {
        let src = source(paper)?;
        let built = build_with(&tex, scratch, &paper.id, &src, &src, &[(PACKAGE, &on, &off)]).with_context(|| format!("paper `{}`", paper.id))?;
        if built.pages.len() > MAX_PAGES {
            bail!("paper `{}` has {} pages; at most {MAX_PAGES}", paper.id, built.pages.len());
        }
        let pages = layout::build(&paper.id, &built.pages).with_context(|| format!("paper `{}`", paper.id))?;
        for p in &pages {
            let glyphs_unowned =
                p.unowned.iter().filter(|&&i| matches!(p.items.get(i).map(|i| &i.mark), Some(pdfcraft_oracle::tex::Mark::Glyph { .. }))).count();
            if glyphs_unowned > 0 {
                bail!("paper `{}` page {}: {glyphs_unowned} glyphs belong to no block", paper.id, p.page);
            }
            // A heading or title is never more than a few lines: a run-in heading would swallow its paragraph.
            if let Some(b) = p.blocks.iter().find(|b| matches!(b.kind, Kind::Heading(_) | Kind::Title) && b.items.len() > 120) {
                bail!("paper `{}` page {}: {} `{}` has {} items; a run-in heading?", paper.id, p.page, b.kind, b.id, b.items.len());
            }
        }
        let mut counts: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
        for b in pages.iter().flat_map(|p| &p.blocks) {
            *counts.entry(b.kind.as_str()).or_default() += 1;
        }
        println!("{:<16} {} pages {counts:?}", paper.id, pages.len());
        truth.papers.push(info(paper));
        truth.pages.extend(pages);
        files.push((format!("{}.tex", paper.id), src.into_bytes()));
        files.push((format!("{}.pdf", paper.id), built.pdf));
    }
    let header = format!(
        "Generated by `cargo xtask layout-oracle generate` from papers.toml with {}. Do not edit.\n\
         Rows: see pdfcraft_oracle::tex::layout::LayoutTruth. Positions in sp from the DVI origin.",
        tex.version()
    );
    files.push(("truth.tsv".to_owned(), truth.to_tsv(&header).into_bytes()));
    std::fs::create_dir_all(&out)?;
    for (name, bytes) in &files {
        std::fs::write(out.join(name), bytes).with_context(|| format!("writing {name}"))?;
    }
    if check {
        let differ: Vec<&str> =
            files.iter().map(|(n, _)| n.as_str()).filter(|n| std::fs::read(out.join(n)).ok() != std::fs::read(data.join(n)).ok()).collect();
        if !differ.is_empty() {
            bail!("generated files differ from the committed ones: {differ:?}");
        }
        println!("layout-oracle: every generated file is byte-identical with {DATA_DIR}");
    } else {
        println!("layout-oracle: wrote {} files to {}", files.len(), out.display());
    }
    Ok(())
}
