//! Recovering a formula's structure from the glyphs and rules of a region.
//!
//! The glyphs say what and where; TeX's own rules of layout say how the pieces relate, and are run in
//! reverse (written from the public description of how typeset mathematics is laid out, TeXbook
//! chapters 17 and 18):
//!
//! - a **fraction** is a horizontal rule with something above and something below it; a **radical** is
//!   a radical sign with a rule leaving its top; an **overline** is a rule with something under it
//!   and nothing over it. They are found first, outermost first, and what lies in them is read by
//!   the same procedure, to a bounded depth;
//! - a **large operator** with things above and below it, centred on it, has **limits**;
//! - **delimiters** that grow (the `\big` and `\left` sizes, and tall ones built from pieces) enclose what
//!   lies between a left one and its right one;
//! - what is left of a row is read left to right; a mark that is smaller than the one before it and
//!   shifted up or down from it is a **script**. Scripts are told from other small marks by their size
//!   relative to the base (one em of the script style is about 0.7 of the text style's, 0.5 of the
//!   next) and by how far their baseline is raised or lowered, never by the order the glyphs were
//!   drawn in.
//!
//! Every glyph must be accounted for and every symbol known. A region that holds something this does
//! not read (a matrix, an alignment, stacked material with no rule) is not guessed at: it is an error
//! naming what was met.

use super::MathPage;
use super::ast::{FractionStyle, Limits, Node, Sizing, Space, Sym};
use super::symbols::{self, Alphabet, Class, Compound, Kind, Lookup, Role, Symbol, SymbolTable};
use std::collections::HashMap;

/// Most glyphs read from one region.
pub const MAX_GLYPHS: usize = 4_000;
/// Most rules read from one region.
pub const MAX_RULES: usize = 2_000;
/// Deepest nesting of fractions, radicals, scripts and delimiters.
pub const MAX_DEPTH: usize = 32;
/// Most structures built in one region: bounds the work an adversarial layout can cause.
const MAX_STEPS: usize = 100_000;

/// The style the formula is set in, when the caller knows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    /// A display: `\[…\]`, `equation`, `align`.
    Display,
    /// Inline in a paragraph.
    Text,
}

/// What the caller can say about the formula.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RecoverOptions {
    /// Display or inline. Without it a fraction is `\frac` whatever the size of its numerator.
    pub style: Option<Style>,
}

/// How much one diagnostic matters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    /// Something noted that does not doubt the answer.
    Info,
    /// A choice was made between readings the glyphs cannot tell apart.
    Ambiguous,
    /// Something the reader does not handle; the region is not answered.
    Unsupported,
    /// A glyph whose font or code the symbol table does not know.
    Unknown,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Diagnostic {
    pub severity: Severity,
    pub message: String,
}

impl Diagnostic {
    fn new(severity: Severity, message: impl Into<String>) -> Diagnostic {
        Diagnostic { severity, message: message.into() }
    }
}

/// How far to trust a recovered formula.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Confidence {
    /// Every glyph was read as a known symbol, every mark was explained, and no choice was made
    /// between readings.
    Confident,
    /// An answer, but a choice was made that the glyphs do not settle.
    Tentative,
}

/// A formula recovered from a region.
#[derive(Clone, Debug, PartialEq)]
pub struct Recovered {
    /// The LaTeX body of the formula.
    pub latex: String,
    pub ast: Node,
    pub confidence: Confidence,
    pub diagnostics: Vec<Diagnostic>,
}

/// Why a region was not answered.
#[derive(Clone, Debug, PartialEq)]
pub struct RecoverError {
    pub kind: ErrorKind,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    /// The region is not a finite box with positive area.
    BadRegion,
    /// There is no glyph in the region.
    NoGlyphs,
    /// More glyphs or rules than [`MAX_GLYPHS`] or [`MAX_RULES`].
    TooLarge,
    /// A glyph with a size, position or direction that is not that of typeset text, or drawn twice.
    InvalidGlyph,
    /// A glyph of a font or code the symbol table does not know.
    UnknownSymbol,
    /// A layout this reader does not handle.
    Unsupported,
    /// Nesting deeper than [`MAX_DEPTH`], or more work than a region is allowed.
    TooComplex,
}

impl std::fmt::Display for RecoverError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let first = self.diagnostics.first().map_or("", |d| d.message.as_str());
        write!(f, "{:?}: {first}", self.kind)
    }
}

impl std::error::Error for RecoverError {}

fn fail(kind: ErrorKind, message: impl Into<String>) -> RecoverError {
    RecoverError { kind, diagnostics: vec![Diagnostic::new(severity_of(kind), message)] }
}

fn severity_of(kind: ErrorKind) -> Severity {
    if kind == ErrorKind::UnknownSymbol { Severity::Unknown } else { Severity::Unsupported }
}

type R<T> = Result<T, RecoverError>;

// ---- elements ------------------------------------------------------------------------------------

/// A mark on the page: a symbol, several glyphs drawn as one thing, or a structure already read.
#[derive(Clone, Debug)]
struct El {
    what: What,
    /// The layout box's left and right edges: the glyph's origin and the end of its advance.
    x0: f32,
    x1: f32,
    /// The ink, `[x0, y0, x1, y1]`, y down.
    ink: [f32; 4],
    /// The baseline the mark sits on (for a structure, the baseline of the row it stands in).
    base: f32,
    /// One em of the mark's own style, in points.
    size: f32,
    /// What TeX adds to the width of the mark when no subscript follows (a letter's italic
    /// correction; none for a font with a word space).
    ic: f32,
}

#[derive(Clone, Debug)]
enum What {
    Sym(&'static Symbol),
    Comp(&'static Compound),
    /// A delimiter that is a glyph of a size ladder, or tall and built of pieces.
    Delim(Delim),
    /// A function name drawn as a run of upright letters (`\sin`, `\lim`).
    Func(String),
    /// A glyph the symbol table does not know; kept only until compounds are joined, since it may be a
    /// part of one (the slash of `\ne`).
    Unknown(String),
    /// A structure already read.
    Block(Box<Node>, Class),
}

#[derive(Clone, Debug)]
struct Delim {
    sym: &'static Symbol,
    /// The delimiter as LaTeX (`(`, `\lfloor`).
    group: String,
    class: Class,
    /// Rank on the size ladder, or `None` for one built of pieces.
    rank: Option<u8>,
    /// One em of the glyph, in points.
    size: f32,
}

impl El {
    fn cx(&self) -> f32 {
        (self.ink[0] + self.ink[2]) / 2.0
    }
    fn cy(&self) -> f32 {
        (self.ink[1] + self.ink[3]) / 2.0
    }
    fn boxy(&self) -> bool {
        match &self.what {
            What::Sym(s) => s.class == Class::Large,
            What::Delim(_) | What::Block(..) => true,
            What::Comp(c) => c.class == Class::Large,
            What::Func(_) | What::Unknown(_) => false,
        }
    }
    fn class(&self) -> Class {
        match &self.what {
            What::Sym(s) => s.class,
            What::Comp(c) => c.class,
            What::Delim(d) => d.class,
            What::Func(_) => Class::Op,
            What::Unknown(_) => Class::Ord,
            What::Block(_, c) => *c,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct RuleEl {
    rect: [f32; 4],
}

impl RuleEl {
    fn x0(&self) -> f32 {
        self.rect[0]
    }
    fn x1(&self) -> f32 {
        self.rect[2]
    }
    fn yc(&self) -> f32 {
        (self.rect[1] + self.rect[3]) / 2.0
    }
}

#[derive(Default)]
struct Pool {
    els: Vec<El>,
    rules: Vec<RuleEl>,
}

fn union(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])]
}

fn union_all(els: &[El]) -> Option<[f32; 4]> {
    els.iter().map(|e| e.ink).reduce(union)
}

/// The math axis above the baseline, in ems: the height of a minus sign's bar.
const AXIS: f32 = 0.25;
/// `\nulldelimiterspace` on each side of a fraction, in big points (1.2 TeX points).
const NULL_DELIMITER: f32 = 1.195;
/// `\scriptspace` after a script, in big points (0.5 TeX points).
const SCRIPT_SPACE: f32 = 0.4981;

/// The row's size and baseline.
#[derive(Clone, Copy, Debug)]
struct RowInfo {
    size: f32,
    base: f32,
    /// Where the row's box ends.
    right: f32,
}

// ---- the reader ----------------------------------------------------------------------------------

struct Reader {
    table: &'static SymbolTable,
    diags: Vec<Diagnostic>,
    steps: usize,
    /// The largest glyph size in the region: one em of the formula's own style.
    top: f32,
    /// Display or text style, from the caller or from what the glyphs show (a large operator's size).
    style: Option<Style>,
    /// How many fractions the part being read is inside.
    frac_depth: usize,
}

/// Recovers the formula drawn in `region` of `page`.
pub fn recover_latex(page: &MathPage, region: [f32; 4], opts: &RecoverOptions) -> Result<Recovered, RecoverError> {
    let table = symbols::table();
    let pool = collect(page, region, table)?;
    let glyphs = pool.els.len();
    let top = pool.els.iter().filter(|e| !matches!(e.what, What::Block(..))).map(|e| e.size).fold(0.0, f32::max);
    let style = opts.style.or_else(|| style_evidence(&pool.els));
    let mut reader = Reader { table, diags: Vec::new(), steps: 0, top, style, frac_depth: 0 };
    let (node, _) = reader.parse_pool(pool, 0)?;
    if node.is_empty() {
        return Err(fail(ErrorKind::NoGlyphs, "nothing was read from the region"));
    }
    let confidence = if reader.diags.iter().any(|d| matches!(d.severity, Severity::Ambiguous | Severity::Unsupported | Severity::Unknown)) {
        Confidence::Tentative
    } else {
        Confidence::Confident
    };
    reader.diags.push(Diagnostic::new(Severity::Info, format!("read {glyphs} marks")));
    Ok(Recovered { latex: node.to_latex(), ast: node, confidence, diagnostics: reader.diags })
}

/// The style a large operator's size shows: its text-size glyph is the smaller one.
fn style_evidence(els: &[El]) -> Option<Style> {
    els.iter().find_map(|e| match &e.what {
        What::Sym(s) if s.class == Class::Large => s.variants.iter().find(|(g, _)| *g == s.latex).and_then(|(_, r)| match r {
            0 => Some(Style::Text),
            1 => Some(Style::Display),
            _ => None,
        }),
        _ => None,
    })
}

/// The glyphs and rules of `region`, as elements; the glyphs of several parts of one thing drawn as
/// one (`\ne`, `\sin`) are joined, and delimiters and radical signs built of pieces assembled.
fn collect(page: &MathPage, region: [f32; 4], table: &'static SymbolTable) -> R<Pool> {
    if !region.iter().all(|v| v.is_finite()) || region[0] >= region[2] || region[1] >= region[3] {
        return Err(fail(ErrorKind::BadRegion, "the region is not a finite box with positive area"));
    }
    let [rx0, ry0, rx1, ry1] = region;
    let mut inside: Vec<&super::GlyphDetail> = Vec::new();
    for g in &page.glyphs {
        let [ox, oy] = g.baseline[0];
        if !(rx0..=rx1).contains(&ox) || !(ry0..=ry1).contains(&oy) || g.invisible {
            continue;
        }
        if inside.len() >= MAX_GLYPHS {
            return Err(fail(ErrorKind::TooLarge, format!("more than {MAX_GLYPHS} glyphs in the region")));
        }
        inside.push(g);
    }
    if inside.is_empty() {
        return Err(fail(ErrorKind::NoGlyphs, "no glyph in the region"));
    }
    let mut singles: Vec<(El, u16, u32)> = Vec::with_capacity(inside.len());
    for g in &inside {
        let ink = g.ink.unwrap_or(g.rect);
        let [ex, ey] = g.baseline[1];
        let oy = g.baseline[0][1];
        if !(g.size.is_finite() && g.size > 0.0 && ex.is_finite() && ey.is_finite() && ink.iter().all(|v| v.is_finite())) {
            return Err(fail(ErrorKind::InvalidGlyph, format!("a glyph (code {}) has a size or position that is not finite and positive", g.code)));
        }
        if (g.direction[0] - 1.0).abs() > 0.02 || g.direction[1].abs() > 0.02 || (ey - oy).abs() > 0.02 * g.size {
            return Err(fail(ErrorKind::InvalidGlyph, format!("a glyph (code {}) is not set left to right", g.code)));
        }
        let font = page.fonts.get(g.font as usize);
        let name = font.and_then(|f| f.name.as_deref().or(f.base_font.as_deref())).unwrap_or("");
        let Some(family) = symbols::family_of(name).and_then(|f| table.family(&f)) else {
            return Err(RecoverError {
                kind: ErrorKind::UnknownSymbol,
                diagnostics: vec![Diagnostic::new(
                    Severity::Unknown,
                    format!("font `{name}` is not a math font the symbol table knows (code {})", g.code),
                )],
            });
        };
        let what = match table.lookup_id(family, g.code) {
            Lookup::Known(sym) => What::Sym(sym),
            _ => What::Unknown(format!("`{name}` has no symbol with code {} in the symbol table", g.code)),
        };
        // A font with a word space (the text fonts) has no italic correction added in a formula.
        let text_font =
            table.family_name(family).is_some_and(|n| ["cmr", "cmbx", "cmti", "cmsl", "cmss", "cmtt", "cmcsc"].iter().any(|p| n.starts_with(p)));
        // The last letter of a run in a text font gets the correction too (`\mathit{rank}`); the others do not.
        let ic = match &what {
            What::Sym(sym) if !text_font || matches!(sym.alphabet, Some(a) if a != Alphabet::Math) => {
                f32::from(sym.italic).max(0.0) / 1000.0 * g.size
            }
            _ => 0.0,
        };
        singles.push((El { what, x0: g.baseline[0][0], x1: ex, ink, base: oy, size: g.size, ic }, family, g.code));
    }
    // A glyph drawn twice at one place (a faked bold, an overstrike) is not typeset mathematics.
    for (k, g) in inside.iter().enumerate() {
        for h in inside.iter().skip(k + 1) {
            if g.code == h.code
                && g.font == h.font
                && (g.baseline[0][0] - h.baseline[0][0]).abs() < 0.01
                && (g.baseline[0][1] - h.baseline[0][1]).abs() < 0.01
                && (g.size - h.size).abs() < 0.01
            {
                return Err(fail(ErrorKind::InvalidGlyph, format!("glyph code {} is drawn twice at one place", g.code)));
            }
        }
    }
    let mut els = join_compounds(singles, table);
    join_function_runs(&mut els);
    let unknown: Vec<Diagnostic> =
        els.iter().filter_map(|e| if let What::Unknown(m) = &e.what { Some(Diagnostic::new(Severity::Unknown, m.clone())) } else { None }).collect();
    if !unknown.is_empty() {
        let mut unknown = unknown;
        unknown.dedup();
        return Err(RecoverError { kind: ErrorKind::UnknownSymbol, diagnostics: unknown });
    }
    assemble_stacks(&mut els);
    let mut rules = Vec::new();
    let tol = 0.5;
    for r in &page.rules {
        if !r.rect.iter().all(|v| v.is_finite()) {
            continue;
        }
        let within = r.rect[0] >= rx0 - tol && r.rect[2] <= rx1 + tol && r.rect[1] >= ry0 - tol && r.rect[3] <= ry1 + tol;
        if !within {
            continue;
        }
        if !r.horizontal {
            return Err(fail(ErrorKind::Unsupported, "a vertical rule (a table or array line)"));
        }
        if rules.len() >= MAX_RULES {
            return Err(fail(ErrorKind::TooLarge, format!("more than {MAX_RULES} rules in the region")));
        }
        rules.push(RuleEl { rect: r.rect });
    }
    Ok(Pool { els, rules })
}

/// Names of functions that are written as upright letters.
const FUNCTION_NAMES: [&str; 29] = [
    "arccos", "arcsin", "arctan", "arg", "cos", "cosh", "cot", "coth", "csc", "deg", "det", "dim", "exp", "gcd", "hom", "inf", "ker", "lg", "lim",
    "liminf", "limsup", "ln", "log", "max", "min", "Pr", "sec", "sin", "sinh",
];

/// The letter of an upright-letter symbol (`\mathrm{s}` is `s`).
fn roman_letter(e: &El) -> Option<char> {
    let What::Sym(s) = &e.what else { return None };
    if s.alphabet != Some(Alphabet::Roman) || s.kind != Kind::Latin {
        return None;
    }
    s.latex.strip_suffix('}').and_then(|t| t.chars().last()).filter(char::is_ascii_alphabetic)
}

/// Runs of upright letters that spell a function name become one element (`\sin`, `\lim`).
fn join_function_runs(els: &mut Vec<El>) {
    let mut letters: Vec<usize> = els.iter().enumerate().filter(|(_, e)| roman_letter(e).is_some()).map(|(k, _)| k).collect();
    letters.sort_by(|&a, &b| els.get(a).map_or(0.0, |e| e.x0).total_cmp(&els.get(b).map_or(0.0, |e| e.x0)));
    let mut remove: Vec<usize> = Vec::new();
    let mut made: Vec<El> = Vec::new();
    let mut k = 0;
    while k < letters.len() {
        let mut run = vec![letters[k]];
        let mut j = k + 1;
        while let (Some(&prev), Some(&next)) = (run.last(), letters.get(j)) {
            let (Some(p), Some(n)) = (els.get(prev), els.get(next)) else { break };
            if (n.size - p.size).abs() > 0.05 * p.size || (n.base - p.base).abs() > 0.05 * p.size || n.x0 - p.x1 > 0.15 * p.size || n.x0 < p.x0 {
                break;
            }
            run.push(next);
            j += 1;
        }
        let text: String = run.iter().filter_map(|&i| els.get(i)).filter_map(roman_letter).collect();
        if FUNCTION_NAMES.contains(&text.as_str()) && run.len() > 1 {
            let members: Vec<&El> = run.iter().filter_map(|&i| els.get(i)).collect();
            if let Some(first) = members.first() {
                let ink = members.iter().map(|e| e.ink).reduce(union).unwrap_or(first.ink);
                let x1 = members.iter().map(|e| e.x1).fold(f32::NEG_INFINITY, f32::max);
                made.push(El { what: What::Func(format!("\\{text}")), x0: first.x0, x1, ink, base: first.base, size: first.size, ic: 0.0 });
                remove.extend(run.iter().copied());
            }
        }
        k = j;
    }
    if made.is_empty() {
        return;
    }
    remove.sort_unstable();
    let mut idx = 0usize;
    els.retain(|_| {
        let keep = remove.binary_search(&idx).is_err();
        idx += 1;
        keep
    });
    els.extend(made);
}

/// Joins glyphs that TeX draws together into compounds.
fn join_compounds(singles: Vec<(El, u16, u32)>, table: &'static SymbolTable) -> Vec<El> {
    let mut by_code: HashMap<(u16, u32), Vec<usize>> = HashMap::new();
    for (k, (_, family, code)) in singles.iter().enumerate() {
        by_code.entry((*family, *code)).or_default().push(k);
    }
    let mut used = vec![false; singles.len()];
    let mut out: Vec<El> = Vec::new();
    for k in 0..singles.len() {
        if used.get(k) != Some(&false) {
            continue;
        }
        let Some((first, family, code)) = singles.get(k) else { continue };
        let mut joined = false;
        for c in table.compounds_from(*family, *code) {
            let mut members = vec![k];
            let mut ok = true;
            for part in c.parts.iter().skip(1) {
                let want_size = first.size * part.scale;
                let found = by_code.get(&(part.family, part.code)).and_then(|list| {
                    list.iter().copied().find(|&j| {
                        let Some((e, _, _)) = singles.get(j) else { return false };
                        used.get(j) == Some(&false)
                            && !members.contains(&j)
                            && (e.size - want_size).abs() <= 0.02 * first.size
                            // Another design size of the font (cmr7, cmr5) is a little wider or narrower.
                            && ((e.x0 - first.x0) / first.size - part.dx).abs() <= 0.03 + 0.1 * part.dx.abs()
                            && ((e.base - first.base) / first.size - part.dy).abs() <= 0.03 + 0.1 * part.dy.abs()
                    })
                });
                match found {
                    Some(j) => members.push(j),
                    None => {
                        ok = false;
                        break;
                    }
                }
            }
            if ok && members.len() == c.parts.len() {
                let mut ink = first.ink;
                let (mut x1, mut x0) = (first.x1, first.x0);
                for &j in &members {
                    if let Some((e, _, _)) = singles.get(j) {
                        ink = union(ink, e.ink);
                        x1 = x1.max(e.x1);
                        x0 = x0.min(e.x0);
                    }
                    if let Some(u) = used.get_mut(j) {
                        *u = true;
                    }
                }
                out.push(El { what: What::Comp(c), x0, x1, ink, base: first.base + c.base * first.size, size: first.size, ic: 0.0 });
                joined = true;
                break;
            }
        }
        if !joined {
            if let Some(u) = used.get_mut(k) {
                *u = true;
            }
            out.push(first.clone());
        }
    }
    out
}

/// The delimiter group a glyph is a *size* of (not the smallest) or a piece of.
fn delimiter_groups(sym: &Symbol) -> Vec<(&str, Option<u8>, Option<Role>)> {
    let is_delim_group = |g: &str| !matches!(g, "\\sqrt" | "\\widehat" | "\\widetilde" | "\\overbrace" | "\\underbrace");
    let mut out: Vec<(&str, Option<u8>, Option<Role>)> = Vec::new();
    if !matches!(sym.class, Class::Open | Class::Close | Class::Ord) {
        return out;
    }
    for (g, r) in &sym.variants {
        if is_delim_group(g) && *r >= 1 {
            out.push((g.as_str(), Some(*r), None));
        }
    }
    for (g, role) in &sym.pieces {
        if is_delim_group(g) {
            out.push((g.as_str(), None, Some(*role)));
        }
    }
    out
}

/// Single-glyph delimiters of a larger size become [`What::Delim`]s; stacks of pieces are joined into
/// one.
fn assemble_stacks(els: &mut Vec<El>) {
    // A piece of a tall delimiter: collected into columns by x.
    let mut piece_idx: Vec<usize> = Vec::new();
    for (k, e) in els.iter().enumerate() {
        if let What::Sym(s) = &e.what
            && !s.pieces.is_empty()
            && !delimiter_groups(s).is_empty()
        {
            piece_idx.push(k);
        }
    }
    let mut consumed = vec![false; els.len()];
    let mut made: Vec<El> = Vec::new();
    for &k in &piece_idx {
        if consumed.get(k) != Some(&false) {
            continue;
        }
        let Some(top) = els.get(k) else { continue };
        let What::Sym(top_sym) = &top.what else { continue };
        let top_pieces = top_sym.pieces.clone();
        // The stack: pieces of the same column, vertically touching, from the one at the top down.
        let mut stack: Vec<usize> = vec![k];
        while let Some(last) = stack.last().and_then(|&i| els.get(i)) {
            let next = piece_idx.iter().copied().find(|&j| {
                j != k
                    && consumed.get(j) == Some(&false)
                    && !stack.contains(&j)
                    && els.get(j).is_some_and(|e| {
                        (e.x0 - last.x0).abs() < 0.3
                            && (e.size - last.size).abs() < 0.1
                            && e.ink[1] >= last.ink[1]
                            && e.ink[1] - last.ink[3] < 1.0
                            && e.ink[1] >= last.ink[3] - 3.0 * last.size
                    })
            });
            match next {
                Some(j) => stack.push(j),
                None => break,
            }
        }
        // The group all pieces share, and the roles in order.
        let mut group: Option<&str> = None;
        for (g, _) in &top_pieces {
            let all = stack.iter().all(|&i| matches!(els.get(i).map(|e| &e.what), Some(What::Sym(s)) if s.pieces.iter().any(|(sg, _)| sg == g)));
            if all {
                let has_bot = stack
                    .iter()
                    .any(|&i| matches!(els.get(i).map(|e| &e.what), Some(What::Sym(s)) if s.pieces.iter().any(|(sg, r)| sg == g && *r == Role::Bot)));
                let last_is_bot = stack
                    .last()
                    .and_then(|&i| els.get(i))
                    .is_some_and(|e| matches!(&e.what, What::Sym(s) if s.pieces.iter().any(|(sg, r)| sg == g && *r == Role::Bot)));
                let first_is_top =
                    els.get(k).is_some_and(|e| matches!(&e.what, What::Sym(s) if s.pieces.iter().any(|(sg, r)| sg == g && *r == Role::Top)));
                if has_bot == last_is_bot && (first_is_top || stack.len() == 1) {
                    group = Some(g.as_str());
                    break;
                }
            }
        }
        let Some(group) = group else { continue };
        let mut ink = top.ink;
        let (mut x1, mut x0) = (top.x1, top.x0);
        for &i in &stack {
            if let Some(e) = els.get(i) {
                ink = union(ink, e.ink);
                x1 = x1.max(e.x1);
                x0 = x0.min(e.x0);
            }
            if let Some(c) = consumed.get_mut(i) {
                *c = true;
            }
        }
        made.push(El {
            what: What::Delim(Delim { sym: top_sym, group: group.to_owned(), class: top_sym.class, rank: None, size: top.size }),
            x0,
            x1,
            ink,
            base: top.base,
            size: top.size,
            ic: 0.0,
        });
    }
    let mut out: Vec<El> = Vec::with_capacity(els.len());
    for (k, e) in els.drain(..).enumerate() {
        if consumed.get(k) == Some(&true) {
            continue;
        }
        if let What::Sym(s) = &e.what
            && let Some((g, Some(rank), _)) = delimiter_groups(s).into_iter().find(|(_, r, _)| r.is_some())
        {
            out.push(El { what: What::Delim(Delim { sym: s, group: g.to_owned(), class: s.class, rank: Some(rank), size: e.size }), ..e });
            continue;
        }
        out.push(e);
    }
    out.extend(made);
    *els = out;
}

// ---- structures -----------------------------------------------------------------------------------

/// A structure found by its rule.
#[derive(Clone, Debug)]
enum Cand {
    Fraction { rule: usize },
    Radical { rule: usize, signs: Vec<usize> },
    Overline { rule: usize },
}

/// Whether `e` is (a piece of) a radical sign.
fn is_radical_sign(e: &El) -> bool {
    match &e.what {
        What::Sym(s) => s.variants.iter().any(|(g, _)| g == "\\sqrt") || s.pieces.iter().any(|(g, _)| g == "\\sqrt"),
        _ => false,
    }
}

impl Reader {
    fn tick(&mut self) -> R<()> {
        self.steps += 1;
        if self.steps > MAX_STEPS {
            return Err(fail(ErrorKind::TooComplex, "the region needs more work than a formula should"));
        }
        Ok(())
    }

    fn note(&mut self, severity: Severity, message: impl Into<String>) {
        if self.diags.len() < 64 {
            self.diags.push(Diagnostic::new(severity, message));
        }
    }

    /// The structures the rules of `pool` make, with the elements each uses.
    fn candidates(&self, pool: &Pool, ctx_size: f32) -> Vec<Cand> {
        let mut out = Vec::new();
        let mut radical_rule = vec![false; pool.rules.len()];
        // Radicals: a sign whose top meets a rule that starts at its right edge.
        let signs: Vec<usize> = pool.els.iter().enumerate().filter(|(_, e)| is_radical_sign(e)).map(|(k, _)| k).collect();
        for (ri, r) in pool.rules.iter().enumerate() {
            let hit = signs.iter().copied().find(|&k| {
                pool.els.get(k).is_some_and(|e| {
                    // The radical's overline starts at the right edge of its sign and rises to the sign's top.
                    (r.x0() - e.x1).abs() <= 1.2 && (r.rect[1] - e.ink[1]).abs() <= 1.6
                })
            });
            if let Some(k) = hit {
                // A tall sign is several pieces; the top one is where the rule leaves.
                let mut all = vec![k];
                if let Some(top) = pool.els.get(k) {
                    for (j, e) in pool.els.iter().enumerate() {
                        if j != k && is_radical_sign(e) && (e.x0 - top.x0).abs() < 1.0 && e.ink[1] >= top.ink[1] {
                            all.push(j);
                        }
                    }
                }
                if let Some(flag) = radical_rule.get_mut(ri) {
                    *flag = true;
                }
                out.push(Cand::Radical { rule: ri, signs: all });
            }
        }
        for (ri, r) in pool.rules.iter().enumerate() {
            if radical_rule.get(ri) == Some(&true) {
                continue;
            }
            let reach = 1.3 * ctx_size.max(1.0);
            let above =
                pool.els.iter().any(|e| e.cx() >= r.x0() - 1.0 && e.cx() <= r.x1() + 1.0 && e.ink[3] <= r.yc() + 0.3 && r.yc() - e.ink[3] <= reach);
            let below =
                pool.els.iter().any(|e| e.cx() >= r.x0() - 1.0 && e.cx() <= r.x1() + 1.0 && e.ink[1] >= r.yc() - 0.3 && e.ink[1] - r.yc() <= reach);
            if above && below {
                out.push(Cand::Fraction { rule: ri });
            } else if below {
                out.push(Cand::Overline { rule: ri });
            }
        }
        out
    }

    /// Reads the structure that is not inside another, and replaces what it used by one block.
    /// Returns `false` when there is none.
    fn extract_one(&mut self, pool: &mut Pool, ctx_size: f32, depth: usize) -> R<bool> {
        let cands = self.candidates(pool, ctx_size);
        if cands.is_empty() {
            return Ok(false);
        }
        // Anchors and extents, to tell which structure lies inside which.
        let anchor = |c: &Cand| -> Option<[f32; 2]> {
            let rule = match c {
                Cand::Fraction { rule } | Cand::Radical { rule, .. } | Cand::Overline { rule } => *rule,
            };
            pool.rules.get(rule).map(|r| [(r.x0() + r.x1()) / 2.0, r.yc()])
        };
        let contains = |outer: &Cand, p: [f32; 2]| -> bool {
            match outer {
                Cand::Fraction { rule } | Cand::Overline { rule } => {
                    pool.rules.get(*rule).is_some_and(|r| p[0] >= r.x0() - 1.0 && p[0] <= r.x1() + 1.0 && (p[1] - r.yc()).abs() > 0.1)
                }
                Cand::Radical { rule, signs } => pool.rules.get(*rule).is_some_and(|r| {
                    let bottom = signs.iter().filter_map(|&k| pool.els.get(k)).map(|e| e.ink[3]).fold(r.yc(), f32::max);
                    p[0] >= r.x0() - 0.2 && p[0] <= r.x1() + 0.2 && p[1] > r.rect[3] - 0.2 && p[1] < bottom + 1.0
                }),
            }
        };
        let mut chosen: Option<usize> = None;
        let mut best_depth = usize::MAX;
        for (a, c) in cands.iter().enumerate() {
            let Some(p) = anchor(c) else { continue };
            let inside = cands.iter().enumerate().filter(|(b, o)| *b != a && contains(o, p)).count();
            if inside < best_depth {
                best_depth = inside;
                chosen = Some(a);
            }
        }
        let Some(cand) = chosen.and_then(|a| cands.get(a)).cloned() else { return Ok(false) };
        self.tick()?;
        match cand {
            Cand::Fraction { rule } => self.build_fraction(pool, rule, ctx_size, depth)?,
            Cand::Radical { rule, signs } => self.build_radical(pool, rule, &signs, ctx_size, depth)?,
            Cand::Overline { rule } => self.build_overline(pool, rule, depth)?,
        }
        Ok(true)
    }

    /// Takes from `pool` the elements and rules for which `inside` holds.
    fn take(pool: &mut Pool, mut inside_el: impl FnMut(&El) -> bool, mut inside_rule: impl FnMut(&RuleEl) -> bool) -> Pool {
        let mut taken = Pool::default();
        let mut keep = Vec::with_capacity(pool.els.len());
        for e in pool.els.drain(..) {
            if inside_el(&e) { taken.els.push(e) } else { keep.push(e) }
        }
        pool.els = keep;
        let mut keep = Vec::with_capacity(pool.rules.len());
        for r in pool.rules.drain(..) {
            if inside_rule(&r) { taken.rules.push(r) } else { keep.push(r) }
        }
        pool.rules = keep;
        taken
    }

    fn build_fraction(&mut self, pool: &mut Pool, rule: usize, ctx_size: f32, depth: usize) -> R<()> {
        let Some(r) = pool.rules.get(rule).copied() else { return Ok(()) };
        let yc = r.yc();
        let in_x = |x: f32| x >= r.x0() - 1.0 && x <= r.x1() + 1.0;
        // Nothing here is the bar itself: take it out first.
        pool.rules.remove(rule);
        let mut above = Self::take(pool, |e| in_x(e.cx()) && e.cy() < yc, |q| in_x(q.x0()) && in_x(q.x1()) && q.yc() < yc);
        let mut below = Self::take(pool, |e| in_x(e.cx()) && e.cy() >= yc, |q| in_x(q.x0()) && in_x(q.x1()) && q.yc() >= yc);
        let ink = union_all(&above.els).into_iter().chain(union_all(&below.els)).reduce(union).unwrap_or(r.rect);
        let ink = union(ink, r.rect);
        self.frac_depth += 1;
        let parts =
            self.parse_pool(std::mem::take(&mut above), depth + 1).and_then(|n| Ok((n, self.parse_pool(std::mem::take(&mut below), depth + 1)?)));
        self.frac_depth -= 1;
        let ((num, num_info), (den, den_info)) = parts?;
        // The style of the fraction's own row: its numerator is one style smaller in text and script
        // style, and as large in display style.
        let numer_size = num_info.size.max(den_info.size);
        // A numerator half the size of its row's text is a fraction in a script: `\frac` whatever the
        // environment. Otherwise it is as large as the row (display style) or 0.7 of it (text style).
        let script_row = numer_size < 0.6 * ctx_size;
        let full = numer_size >= 0.85 * ctx_size;
        let style_size = if script_row { ctx_size * 0.7 } else { ctx_size };
        let top_level = self.frac_depth == 0;
        if !script_row && full && !top_level {
            // Inside a fraction the next one is smaller, unless forced (`\\dfrac`, `\\cfrac`).
            self.note(Severity::Ambiguous, "a fraction at full size inside a fraction: \\dfrac or \\cfrac, which differ in their struts");
        }
        // `\frac` is as large as its surroundings make it; only the environment tells `\dfrac` and
        // `\tfrac` apart from that.
        let style = match (self.style, script_row, full, top_level) {
            (Some(Style::Display), false, false, true) => FractionStyle::Text,
            (Some(Style::Text), false, true, true) => FractionStyle::Display,
            (None, false, true, true) => {
                self.note(
                    Severity::Ambiguous,
                    "a fraction with a full-size numerator: \\frac in a display, or \\dfrac in text (the environment is not known)",
                );
                FractionStyle::Plain
            }
            _ => FractionStyle::Plain,
        };
        let node = Node::Fraction { numerator: Box::new(num), denominator: Box::new(den), style };
        pool.els.push(El {
            what: What::Block(Box::new(node), Class::Inner),
            x0: r.x0() - NULL_DELIMITER,
            x1: r.x1() + NULL_DELIMITER,
            ink,
            base: yc + AXIS * style_size,
            size: style_size,
            ic: 0.0,
        });
        Ok(())
    }

    fn build_overline(&mut self, pool: &mut Pool, rule: usize, depth: usize) -> R<()> {
        let Some(r) = pool.rules.get(rule).copied() else { return Ok(()) };
        let yc = r.yc();
        let in_x = |x: f32| x >= r.x0() - 1.0 && x <= r.x1() + 1.0;
        pool.rules.remove(rule);
        let body = Self::take(pool, |e| in_x(e.cx()) && e.cy() >= yc, |q| in_x(q.x0()) && in_x(q.x1()) && q.yc() >= yc);
        let ink = union_all(&body.els).map_or(r.rect, |i| union(i, r.rect));
        let (node, info) = self.parse_pool(body, depth + 1)?;
        pool.els.push(El {
            what: What::Block(Box::new(Node::Overline(Box::new(node))), Class::Ord),
            x0: r.x0(),
            x1: r.x1(),
            ink,
            base: info.base,
            size: info.size,
            ic: 0.0,
        });
        Ok(())
    }

    fn build_radical(&mut self, pool: &mut Pool, rule: usize, signs: &[usize], ctx_size: f32, depth: usize) -> R<()> {
        let Some(r) = pool.rules.get(rule).copied() else { return Ok(()) };
        let sign_els: Vec<El> = signs.iter().filter_map(|&k| pool.els.get(k).cloned()).collect();
        let Some(sign_ink) = union_all(&sign_els) else { return Ok(()) };
        let sign_x0 = sign_els.iter().map(|e| e.x0).fold(f32::INFINITY, f32::min);
        // The radicand: under the rule, between its ends. The index: small, up and to the left.
        let in_rule_x = |x: f32| x >= r.x0() - 0.2 && x <= r.x1() + 0.2;
        let sign_height = sign_ink[3] - sign_ink[1];
        // Take out the sign first (it is not part of either), then the radicand, then the index.
        let signs_set: Vec<usize> = signs.to_vec();
        let mut keep = Vec::with_capacity(pool.els.len());
        for (idx, e) in pool.els.drain(..).enumerate() {
            if !signs_set.contains(&idx) {
                keep.push(e);
            }
        }
        pool.els = keep;
        pool.rules.remove(rule);
        let body = Self::take(
            pool,
            |e| in_rule_x(e.cx()) && e.cy() > r.rect[3] - 0.2 && e.cy() < sign_ink[3] + 1.0,
            |q| in_rule_x(q.x0()) && in_rule_x(q.x1()) && q.yc() > r.rect[3],
        );
        let index = Self::take(
            pool,
            |e| {
                e.ink[2] <= r.x0() + 1.0
                    && e.cx() >= sign_ink[0] - 1.0 * ctx_size
                    && e.cy() <= sign_ink[1] + 0.65 * sign_height
                    && e.size < 0.75 * ctx_size
            },
            |_| false,
        );
        let ink = union(union(sign_ink, r.rect), union_all(&body.els).unwrap_or(r.rect));
        let ink = union_all(&index.els).map_or(ink, |i| union(ink, i));
        let x0 = index.els.iter().map(|e| e.x0).fold(sign_x0, f32::min);
        let (node, info) = self.parse_pool(body, depth + 1)?;
        let index_node = if index.els.is_empty() {
            None
        } else {
            let (n, _) = self.parse_pool(index, depth + 1)?;
            Some(Box::new(n))
        };
        let node = Node::Radical { index: index_node, body: Box::new(node) };
        pool.els.push(El { what: What::Block(Box::new(node), Class::Ord), x0, x1: r.x1(), ink, base: info.base, size: info.size, ic: 0.0 });
        Ok(())
    }

    // ---- rows ------------------------------------------------------------------------------------

    /// Reads everything in `pool` as one row.
    fn parse_pool(&mut self, mut pool: Pool, depth: usize) -> R<(Node, RowInfo)> {
        if depth > MAX_DEPTH {
            return Err(fail(ErrorKind::TooComplex, format!("nesting deeper than {MAX_DEPTH}")));
        }
        self.tick()?;
        if pool.els.is_empty() {
            return Err(fail(ErrorKind::Unsupported, "a structure with nothing in it"));
        }
        let glyph_size = |e: &El| if matches!(e.what, What::Block(..)) { 0.0 } else { e.size };
        let mut ctx_size = pool.els.iter().map(glyph_size).fold(0.0, f32::max);
        if ctx_size <= 0.0 {
            ctx_size = pool.els.iter().map(|e| e.size).fold(0.0, f32::max);
        }
        while self.extract_one(&mut pool, ctx_size, depth)? {}
        if !pool.rules.is_empty() {
            let r = pool.rules.first().map(|r| r.rect).unwrap_or_default();
            return Err(fail(
                ErrorKind::Unsupported,
                format!("a rule at x {:.1}..{:.1}, y {:.1} that is not a fraction bar, a radical's overline or an overline", r[0], r[2], r[1]),
            ));
        }
        self.group_limits(&mut pool.els, depth)?;
        self.join_accents(&mut pool.els, depth)?;
        let mut els = std::mem::take(&mut pool.els);
        els.sort_by(|a, b| a.x0.total_cmp(&b.x0).then_with(|| a.ink[1].total_cmp(&b.ink[1])));
        let size = els.iter().map(glyph_size).fold(0.0, f32::max);
        let size = if size > 0.0 { size } else { ctx_size };
        let base = els.iter().find(|e| e.size >= 0.9 * size && !e.boxy()).or_else(|| els.first()).map_or(0.0, |e| e.base);
        let info = RowInfo { size, base, right: 0.0 };
        let (nodes, right) = self.fold(els, info, depth)?;
        Ok((one_or_row(nodes), RowInfo { right, ..info }))
    }

    /// Accents over what lies under them, lowest first (`\\hat{\\bar{x}}`).
    fn join_accents(&mut self, els: &mut Vec<El>, depth: usize) -> R<()> {
        loop {
            // The accent lowest on the page that has something under it.
            let pick = els
                .iter()
                .enumerate()
                .filter(|(_, e)| matches!(&e.what, What::Sym(s) if s.accent))
                .filter_map(|(k, a)| {
                    let under: Vec<usize> = els
                        .iter()
                        .enumerate()
                        .filter(|(j, b)| {
                            *j != k
                                && !matches!(&b.what, What::Sym(s) if s.accent)
                                && b.cx() >= a.ink[0] - 0.15 * a.size
                                && b.cx() <= a.ink[2] + 0.15 * a.size
                                && b.cy() > a.cy()
                                && b.cy() - a.cy() < 1.6 * a.size
                        })
                        .map(|(j, _)| j)
                        .collect();
                    (!under.is_empty()).then_some((k, under, a.ink[3]))
                })
                .max_by(|x, y| x.2.total_cmp(&y.2));
            let Some((k, under, _)) = pick else { return Ok(()) };
            self.tick()?;
            let mut accent: Option<El> = None;
            let mut base = Pool::default();
            let mut rest: Vec<El> = Vec::with_capacity(els.len());
            for (j, e) in els.drain(..).enumerate() {
                if j == k {
                    accent = Some(e);
                } else if under.contains(&j) {
                    base.els.push(e);
                } else {
                    rest.push(e);
                }
            }
            let Some(accent) = accent else { return Ok(()) };
            let What::Sym(sym) = &accent.what else { return Ok(()) };
            let mut ink = accent.ink;
            let (mut x0, mut x1) = (f32::INFINITY, f32::NEG_INFINITY);
            for e in &base.els {
                ink = union(ink, e.ink);
                x0 = x0.min(e.x0);
                x1 = x1.max(e.x1 + e.ic);
            }
            let (node, info) = self.parse_pool(base, depth + 1)?;
            let latex = sym.latex.clone();
            rest.push(El {
                what: What::Block(Box::new(Node::Accent { accent: latex, base: Box::new(node) }), Class::Ord),
                x0,
                x1,
                ink,
                base: info.base,
                size: info.size,
                ic: 0.0,
            });
            *els = rest;
        }
    }

    /// Large operators and limit-taking functions with things centred above or below them.
    fn group_limits(&mut self, els: &mut Vec<El>, depth: usize) -> R<()> {
        let mut k = 0;
        while k < els.len() {
            let Some(op) = els.get(k).cloned() else { break };
            let takes = match &op.what {
                What::Sym(s) => s.class == Class::Large,
                What::Comp(c) if c.class == Class::Large => true,
                What::Comp(c) => c.function && LIMIT_FUNCTIONS.contains(&c.latex.as_str()),
                What::Func(name) => LIMIT_FUNCTIONS.contains(&name.as_str()),
                _ => false,
            };
            if !takes {
                k += 1;
                continue;
            }
            // Small marks well above or below the operator.
            let small: Vec<usize> = els.iter().enumerate().filter(|(j, e)| *j != k && e.size < 0.86 * op.size).map(|(j, _)| j).collect();
            let above: Vec<usize> = small
                .iter()
                .copied()
                .filter(|&j| els.get(j).is_some_and(|e| e.ink[3] <= op.ink[1] + 0.2 * op.size && e.base < op.ink[1] + 0.2 * op.size))
                .collect();
            let below: Vec<usize> = small
                .iter()
                .copied()
                .filter(|&j| els.get(j).is_some_and(|e| e.ink[1] >= op.ink[3] - 0.2 * op.size && e.base > op.base + 0.2 * op.size))
                .collect();
            let op_c = (op.x0 + op.x1) / 2.0;
            let tol = 0.1 * op.size + op_italic(&op) * op.size;
            let chain_of = |band: &[usize]| -> Vec<usize> {
                // The run of marks, close to each other, that overlaps the operator's width.
                let mut v: Vec<usize> = band.to_vec();
                v.sort_by(|&a, &b| els.get(a).map_or(0.0, |e| e.x0).total_cmp(&els.get(b).map_or(0.0, |e| e.x0)));
                let overlaps = |e: &El| e.x1 > op.x0 && e.x0 < op.x1;
                let Some(seed) = v.iter().position(|&j| els.get(j).is_some_and(overlaps)) else { return Vec::new() };
                let (mut lo, mut hi) = (seed, seed);
                loop {
                    let mut grew = false;
                    if lo > 0
                        && let (Some(&a), Some(&b)) = (v.get(lo - 1), v.get(lo))
                        && let (Some(ea), Some(eb)) = (els.get(a), els.get(b))
                        && eb.x0 - ea.x1 <= 0.6 * eb.size
                    {
                        lo -= 1;
                        grew = true;
                    }
                    if let (Some(&a), Some(&b)) = (v.get(hi), v.get(hi + 1))
                        && let (Some(ea), Some(eb)) = (els.get(a), els.get(b))
                        && eb.x0 - ea.x1 <= 0.6 * eb.size
                    {
                        hi += 1;
                        grew = true;
                    }
                    if !grew {
                        break;
                    }
                }
                v.get(lo..=hi).map(<[usize]>::to_vec).unwrap_or_default()
            };
            let centred = |chain: &[usize]| -> bool {
                let (mut x0, mut x1) = (f32::INFINITY, f32::NEG_INFINITY);
                for &j in chain {
                    if let Some(e) = els.get(j) {
                        x0 = x0.min(e.x0);
                        x1 = x1.max(e.x1);
                    }
                }
                x0.is_finite() && ((x0 + x1) / 2.0 - op_c).abs() <= tol
            };
            let up = chain_of(&above);
            let down = chain_of(&below);
            let up = if centred(&up) { up } else { Vec::new() };
            let down = if centred(&down) { down } else { Vec::new() };
            if up.is_empty() && down.is_empty() {
                k += 1;
                continue;
            }
            self.tick()?;
            let mut member: Vec<usize> = up.iter().chain(down.iter()).copied().collect();
            member.sort_unstable();
            member.dedup();
            let mut taken_up = Pool::default();
            let mut taken_down = Pool::default();
            let mut rest: Vec<El> = Vec::with_capacity(els.len());
            let mut op_el: Option<El> = None;
            for (j, e) in els.drain(..).enumerate() {
                if j == k {
                    op_el = Some(e);
                } else if up.contains(&j) {
                    taken_up.els.push(e);
                } else if down.contains(&j) {
                    taken_down.els.push(e);
                } else {
                    rest.push(e);
                }
            }
            let Some(op_el) = op_el else { break };
            let mut ink = op_el.ink;
            let (mut x0, mut x1) = (op_el.x0, op_el.x1 + op_el.ic);
            for e in taken_up.els.iter().chain(taken_down.els.iter()) {
                ink = union(ink, e.ink);
                x0 = x0.min(e.x0);
                x1 = x1.max(e.x1);
            }
            let sup = if taken_up.els.is_empty() { None } else { Some(Box::new(self.parse_pool(taken_up, depth + 1)?.0)) };
            let sub = if taken_down.els.is_empty() { None } else { Some(Box::new(self.parse_pool(taken_down, depth + 1)?.0)) };
            let (sym, rank) = op_symbol(&op_el);
            let default_limits = DEFAULT_LIMITS.contains(&sym.latex.as_str()) && rank != Some(0);
            let node = Node::BigOp {
                op: sym,
                sub,
                sup,
                limits: Limits::Above,
                forced: !default_limits && !LIMIT_FUNCTIONS.contains(&op_latex(&op_el).as_str()),
            };
            rest.push(El { what: What::Block(Box::new(node), Class::Op), x0, x1, ink, base: op_el.base, size: op_el.size, ic: 0.0 });
            *els = rest;
            k = 0;
        }
        Ok(())
    }

    /// Reads a sorted row of elements left to right.
    fn fold(&mut self, els: Vec<El>, info: RowInfo, depth: usize) -> R<(Vec<Node>, f32)> {
        let n = els.len();
        let mut used = vec![false; n];
        let mut atoms: Vec<Atom> = Vec::new();
        let mut i = 0;
        while i < n {
            if used.get(i) != Some(&false) {
                i += 1;
                continue;
            }
            self.tick()?;
            let Some(el) = els.get(i).cloned() else { break };
            if let Some(u) = used.get_mut(i) {
                *u = true;
            }
            // A big delimiter opens a group.
            let mut either: Option<u8> = None;
            let (nucleus, nucleus_node) = if let What::Delim(d) = &el.what {
                match self.delimited(&els, &mut used, i, d, &el, info, depth)? {
                    Some((block, node, e)) => {
                        either = e;
                        (block, node)
                    }
                    None => (el.clone(), self.leaf(&el)?),
                }
            } else {
                let node = self.leaf(&el)?;
                (el.clone(), node)
            };
            // A mark on the row's baseline, as large as the row: anything else here is not part of a
            // row (stacked material, a script of something that is not there, an accent's base).
            if !nucleus.boxy() && !matches!(nucleus.what, What::Block(..)) {
                let shift = (nucleus.base - info.base).abs();
                if shift > 0.15 * info.size || nucleus.size < 0.86 * info.size {
                    return Err(fail(
                        ErrorKind::Unsupported,
                        format!(
                            "a mark at x {:.1} that is neither on the row's baseline nor a script of anything (stacked material, or a script with no base)",
                            nucleus.x0
                        ),
                    ));
                }
            }
            // Scripts: what follows, smaller and shifted.
            let mut region: Vec<usize> = Vec::new();
            let mut right = nucleus.x1;
            let mut k = i + 1;
            while k < n {
                let Some(c) = els.get(k) else { break };
                if used.get(k) != Some(&false) {
                    k += 1;
                    continue;
                }
                if !is_script(&nucleus, c, right, region.is_empty(), self.top) {
                    break;
                }
                right = right.max(c.x1);
                region.push(k);
                k += 1;
            }
            // What a script is set beside, TeX sets after it: the next mark of the row starts where the
            // scripts end. A "script" with row marks beside it is stacked material (limits under a word).
            if let Some(next) = els.iter().enumerate().skip(k).find(|(j, _)| used.get(*j) == Some(&false)).map(|(_, e)| e)
                && !region.is_empty()
                && next.x0 < right - 0.6
            {
                return Err(fail(
                    ErrorKind::Unsupported,
                    format!("something under or over a mark at x {:.1} that is not its script (stacked material)", nucleus.x0),
                ));
            }
            let (sub_idx, sup_idx) = partition_scripts(&nucleus, &els, &region);
            for &j in &region {
                if let Some(u) = used.get_mut(j) {
                    *u = true;
                }
            }
            let pick = |idx: &[usize]| -> Vec<El> { idx.iter().filter_map(|&j| els.get(j).cloned()).collect() };
            let (subs, sups) = (pick(&sub_idx), pick(&sup_idx));
            let mut group_right = f32::NEG_INFINITY;
            let mut read_group = |this: &mut Self, group: Vec<El>| -> R<Option<Box<Node>>> {
                if group.is_empty() {
                    return Ok(None);
                }
                let (node, g) = this.parse_pool(Pool { els: group, rules: Vec::new() }, depth + 1)?;
                group_right = group_right.max(g.right);
                Ok(Some(Box::new(node)))
            };
            let sub = read_group(self, subs)?;
            let sup = read_group(self, sups)?;
            let has_scripts = sub.is_some() || sup.is_some();
            // The atom's box ends after its scripts (and a half point), or after its italic correction.
            let box_right = if has_scripts { group_right.max(nucleus.x1) + SCRIPT_SPACE } else { nucleus.x1 + nucleus.ic };
            let class = match nucleus.class() {
                Class::Large => Class::Op,
                c => c,
            };
            let node = if sub.is_none() && sup.is_none() {
                nucleus_node
            } else if let (What::Sym(s), true) = (&nucleus.what, nucleus.boxy()) {
                // A large operator with scripts at its side.
                let rank = s.variants.iter().find(|(g, _)| *g == s.latex).map(|(_, r)| *r);
                let display_default = DEFAULT_LIMITS.contains(&s.latex.as_str()) && rank == Some(1);
                Node::BigOp { op: sym_of(s), sub, sup, limits: Limits::Side, forced: display_default }
            } else {
                Node::Scripts { base: Box::new(nucleus_node), sub, sup }
            };
            let alts = match (&node, &nucleus.what) {
                (Node::Symbol(_), What::Sym(s)) => alternatives(&s.latex),
                _ => &[],
            };
            // `\not` is drawn over the relation after it: the font kerns back by its own width.
            let (pre_glue, post_glue) = match &nucleus.what {
                What::Sym(s) if s.latex == "\\not" => (0.0, -9.0),
                _ => (0.0, 0.0),
            };
            let either = if has_scripts { None } else { either };
            atoms.push(Atom {
                node,
                x0: nucleus.x0,
                right: box_right,
                plain: nucleus.x1,
                size: nucleus.size,
                class_l: class,
                class_r: class,
                either,
                alts,
                pre: pre_glue,
                post: post_glue,
            });
            i += 1;
        }
        let atoms = merge_runs(atoms);
        let right = atoms.last().map_or(0.0, |a| a.right);
        Ok((self.space(atoms, info), right))
    }

    /// The node of one element that is not a delimiter group.
    fn leaf(&mut self, el: &El) -> R<Node> {
        Ok(match &el.what {
            What::Sym(s) if s.pieces.iter().any(|(g, _)| g == "\\overbrace" || g == "\\underbrace") => {
                return Err(fail(ErrorKind::Unsupported, "a brace over or under a formula"));
            }
            What::Sym(s) if s.accent => {
                return Err(fail(ErrorKind::Unsupported, format!("an accent {} with nothing under it", s.latex)));
            }
            What::Sym(s) => symbol_node(s),
            What::Func(name) => Node::Function(name.clone()),
            What::Unknown(m) => return Err(fail(ErrorKind::UnknownSymbol, m.clone())),
            What::Comp(c) if c.function => Node::Function(c.latex.clone()),
            What::Comp(c) => Node::Symbol(Sym { latex: c.latex.clone(), unicode: c.unicode.clone(), class: c.class }),
            What::Delim(d) => {
                // A delimiter with no partner: written as a delimiter sized by hand when it has such a name.
                let sym = sym_of(d.sym);
                match d.sym.big {
                    Some(k) => {
                        let names = if d.class == Class::Close {
                            ["\\bigr", "\\Bigr", "\\biggr", "\\Biggr"]
                        } else {
                            ["\\bigl", "\\Bigl", "\\biggl", "\\Biggl"]
                        };
                        let cmd = names.get(usize::from(k.saturating_sub(1))).copied().unwrap_or("\\bigl");
                        self.note(Severity::Ambiguous, format!("a delimiter {} with no partner", d.group));
                        Node::Symbol(Sym { latex: format!("{cmd}{}", d.group), unicode: sym.unicode, class: d.class })
                    }
                    None => {
                        return Err(fail(ErrorKind::Unsupported, format!("a tall delimiter {} with no partner", d.group)));
                    }
                }
            }
            What::Block(n, _) => (**n).clone(),
        })
    }

    /// The group a big delimiter at `i` opens or closes: a block that stands for the whole.
    #[allow(clippy::too_many_arguments)]
    fn delimited(
        &mut self,
        els: &[El],
        used: &mut [bool],
        i: usize,
        d: &Delim,
        el: &El,
        info: RowInfo,
        depth: usize,
    ) -> R<Option<(El, Node, Option<u8>)>> {
        let opens = |e: &El| matches!(&e.what, What::Delim(x) if x.class == Class::Open || (x.class == Class::Ord && x.group == d.group));
        let closes = |e: &El| matches!(&e.what, What::Delim(x) if x.class == Class::Close);
        if d.class == Class::Close || (d.class != Class::Open && d.class != Class::Ord) {
            return Ok(None);
        }
        // The partner: the first closer at the same nesting level.
        let mut level = 0usize;
        let mut partner: Option<usize> = None;
        for (j, e) in els.iter().enumerate().skip(i + 1) {
            if used.get(j) != Some(&false) {
                continue;
            }
            let is_same_ord = matches!(&e.what, What::Delim(x) if x.class == Class::Ord && x.group == d.group);
            if closes(e) || (d.class == Class::Ord && is_same_ord && level == 0) {
                if level == 0 {
                    partner = Some(j);
                    break;
                }
                level -= 1;
            } else if opens(e) {
                level += 1;
            }
        }
        let Some(j) = partner else { return Ok(None) };
        let Some(right) = els.get(j).cloned() else { return Ok(None) };
        let body_idx: Vec<usize> = (i + 1..j).filter(|&k| used.get(k) == Some(&false)).collect();
        for &k in &body_idx {
            if let Some(u) = used.get_mut(k) {
                *u = true;
            }
        }
        if let Some(u) = used.get_mut(j) {
            *u = true;
        }
        let body_els: Vec<El> = body_idx.iter().filter_map(|&k| els.get(k).cloned()).collect();
        if body_els.is_empty() {
            return Ok(None);
        }
        let body_ink = union_all(&body_els).unwrap_or(el.ink);
        let (body, body_info) = self.parse_pool(Pool { els: body_els, rules: Vec::new() }, depth + 1)?;
        let What::Delim(rd) = &right.what else { return Ok(None) };
        let (sizing, either) = self.sizing(d, rd, body_ink, body_info);
        let node = Node::Delimited { left: Some(sym_of_delim(d)), body: Box::new(body), right: Some(sym_of_delim(rd)), sizing };
        let ink = union(union(el.ink, right.ink), body_ink);
        let block =
            El { what: What::Block(Box::new(node.clone()), Class::Inner), x0: el.x0, x1: right.x1, ink, base: info.base, size: info.size, ic: 0.0 };
        Ok(Some((block, node, either)))
    }

    /// Whether a pair of delimiters is `\left…\right` (as tall as TeX makes them for the contents) or
    /// sized by hand. Where both fit, the glyphs are the same and only the glue around the pair differs:
    /// the second value is the by-hand size, for the spacing to decide.
    fn sizing(&mut self, left: &Delim, right: &Delim, body_ink: [f32; 4], body: RowInfo) -> (Sizing, Option<u8>) {
        let axis = AXIS * body.size;
        let (h, dp) = (body.base - body_ink[1], body_ink[3] - body.base);
        let delta = (h - axis).max(dp + axis).max(0.0);
        let needed = (delta * 1.802).max(2.0 * delta - 4.98);
        let tex_rank = |d: &Delim| -> Option<u8> {
            // What \left would pick: the smallest member of the ladder at least as tall as needed.
            let ladder = self.table.ladder(&d.group);
            let size = d.size;
            let pick = ladder.iter().find(|(_, hd)| f32::from(*hd) / 1000.0 * size >= needed - 0.5).map(|(r, _)| *r);
            pick.or_else(|| ladder.last().map(|(r, _)| *r))
        };
        let auto_ok = |d: &Delim| match d.rank {
            None => true,
            Some(r) => tex_rank(d) == Some(r),
        };
        let auto = auto_ok(left) && auto_ok(right);
        let manual = match (left.sym.big, right.sym.big) {
            (Some(a), Some(b)) if a == b => Some(a),
            _ => None,
        };
        match (auto, manual) {
            (true, m) => (Sizing::Auto, m),
            (false, Some(k)) => (Sizing::Manual(k), None),
            (false, None) => {
                self.note(Severity::Ambiguous, "delimiters of a size neither \\left…\\right nor \\big… gives");
                (Sizing::Auto, None)
            }
        }
    }
}

/// Operators that put their limits above and below in display style.
const DEFAULT_LIMITS: [&str; 12] = [
    "\\sum",
    "\\prod",
    "\\coprod",
    "\\bigcup",
    "\\bigcap",
    "\\bigsqcup",
    "\\bigvee",
    "\\bigwedge",
    "\\bigodot",
    "\\bigotimes",
    "\\bigoplus",
    "\\biguplus",
];
/// Function names that take limits.
const LIMIT_FUNCTIONS: [&str; 13] =
    ["\\lim", "\\liminf", "\\limsup", "\\max", "\\min", "\\sup", "\\inf", "\\det", "\\gcd", "\\Pr", "\\projlim", "\\injlim", "\\varlimsup"];

fn op_italic(e: &El) -> f32 {
    match &e.what {
        What::Sym(s) => f32::from(s.italic) / 1000.0,
        _ => 0.0,
    }
}

fn op_latex(e: &El) -> String {
    match &e.what {
        What::Sym(s) => s.latex.clone(),
        What::Comp(c) => c.latex.clone(),
        What::Func(n) => n.clone(),
        _ => String::new(),
    }
}

fn op_symbol(e: &El) -> (Sym, Option<u8>) {
    match &e.what {
        What::Sym(s) => (sym_of(s), s.variants.iter().find(|(g, _)| *g == s.latex).map(|(_, r)| *r)),
        What::Comp(c) => (Sym { latex: c.latex.clone(), unicode: c.unicode.clone(), class: c.class }, None),
        What::Func(n) => (Sym { latex: n.clone(), unicode: String::new(), class: Class::Op }, None),
        _ => (Sym { latex: String::new(), unicode: String::new(), class: Class::Ord }, None),
    }
}

fn sym_of(s: &Symbol) -> Sym {
    Sym { latex: s.latex.clone(), unicode: s.unicode.clone(), class: s.class }
}

fn sym_of_delim(d: &Delim) -> Sym {
    Sym { latex: d.group.clone(), unicode: d.sym.unicode.clone(), class: d.class }
}

fn symbol_node(s: &Symbol) -> Node {
    if let Some(a) = s.alphabet
        && a != Alphabet::Math
        && s.kind != Kind::Greek
        // `\mathbb{R}`: the letter is what is inside the braces.
        && let Some(ch) = s.latex.strip_suffix('}').and_then(|t| t.chars().last()).filter(char::is_ascii_alphanumeric)
    {
        return Node::Styled { alphabet: a, text: ch.to_string() };
    }
    Node::Symbol(sym_of(s))
}

fn one_or_row(mut nodes: Vec<Node>) -> Node {
    if nodes.len() == 1 { nodes.pop().unwrap_or(Node::Row(Vec::new())) } else { Node::Row(nodes) }
}

/// A mark with the box it fills in a row.
#[derive(Clone, Debug)]
struct Atom {
    node: Node,
    /// Left edge of its box, and right edge (after its italic correction or its scripts).
    x0: f32,
    right: f32,
    /// Where the glyphs end, before any italic correction.
    plain: f32,
    size: f32,
    /// TeX's math class, which decides the glue before and after it.
    class_l: Class,
    class_r: Class,
    /// A pair of delimiters that read as `\left…\right` or as `\bigl…\bigr` (this size): they differ
    /// only in the glue around them, which the gaps decide.
    either: Option<u8>,
    /// Other readings of the same glyph (`|` and `\mid` are one glyph).
    alts: &'static [Alt],
    /// Glue in math units that the reading adds on each side of the atom (what `\colon` adds).
    pre: f32,
    post: f32,
}

/// One reading of a glyph that is more than one command.
#[derive(Clone, Copy, Debug)]
struct Alt {
    latex: &'static str,
    class: Class,
    /// Glue beyond the class's own, before and after, in math units.
    pre: f32,
    post: f32,
}

const fn alt(latex: &'static str, class: Class, pre: f32, post: f32) -> Alt {
    Alt { latex, class, pre, post }
}

/// The readings of a glyph that is more than one command. amsmath's `\colon` is a colon with 2mu
/// before it and 6mu after, whatever its neighbours.
fn alternatives(latex: &str) -> &'static [Alt] {
    const MID: [Alt; 2] = [alt("|", Class::Ord, 0.0, 0.0), alt("\\mid", Class::Rel, 0.0, 0.0)];
    const PARALLEL: [Alt; 2] = [alt("\\|", Class::Ord, 0.0, 0.0), alt("\\parallel", Class::Rel, 0.0, 0.0)];
    const COLON: [Alt; 2] = [alt(":", Class::Rel, 0.0, 0.0), alt("\\colon", Class::Ord, 2.0, 6.0)];
    match latex {
        "\\mid" => &MID,
        "\\parallel" => &PARALLEL,
        ":" => &COLON,
        _ => &[],
    }
}

/// Joins neighbouring upright letters that stand close together (`\mathrm{t}\mathrm{r}`); letters a
/// word space apart make a `\text`.
fn merge_runs(atoms: Vec<Atom>) -> Vec<Atom> {
    let mut out: Vec<Atom> = Vec::with_capacity(atoms.len());
    for atom in atoms {
        if let Some(prev) = out.last_mut() {
            let gap = atom.x0 - prev.plain;
            let tight = gap <= 0.15 * atom.size;
            let word = (gap - WORD_SPACE * atom.size).abs() <= 0.08 * atom.size;
            let joined = match (&mut prev.node, &atom.node) {
                (Node::Styled { alphabet, text }, Node::Styled { alphabet: a2, text: t2 }) if alphabet == a2 && tight => {
                    text.push_str(t2);
                    true
                }
                (Node::Text(text), Node::Styled { alphabet: Alphabet::Roman, text: t2 }) if tight || word => {
                    if word {
                        text.push(' ');
                    }
                    text.push_str(t2);
                    true
                }
                _ => false,
            };
            if joined {
                prev.right = atom.right;
                prev.plain = atom.plain;
                continue;
            }
            if word
                && let (Node::Styled { alphabet: Alphabet::Roman, text }, Node::Styled { alphabet: Alphabet::Roman, text: t2 }) =
                    (&prev.node, &atom.node)
            {
                prev.node = Node::Text(format!("{text} {t2}"));
                prev.right = atom.right;
                prev.plain = atom.plain;
                continue;
            }
        }
        out.push(atom);
    }
    out
}

/// The word space of a text font, in ems (cmr10's is 1/3).
const WORD_SPACE: f32 = 1.0 / 3.0;

/// TeX's glue between two atoms (the TeXbook's table of spacing, p. 170), in math units, and how much of
/// it can stretch. `script` is a row in script or scriptscript style, where the entries the table
/// puts in parentheses are dropped.
fn glue(a: Class, b: Class, script: bool) -> (f32, f32) {
    use Class::{Bin, Close, Inner, Op, Open, Ord, Punct, Rel};
    // 1 thin, 2 medium, 3 thick; the flag is "in parentheses".
    let (level, parenthesised): (usize, bool) = match (a, b) {
        (Ord, Op) | (Op, Ord) | (Op, Op) | (Close, Op) | (Inner, Op) => (1, false),
        (Ord, Bin) | (Close, Bin) | (Inner, Bin) => (2, true),
        (Bin, Ord | Op | Open | Inner) => (2, true),
        (Ord, Rel) | (Op, Rel) | (Close, Rel) | (Inner, Rel) => (3, true),
        (Rel, Ord | Op | Open | Inner) => (3, true),
        (Ord, Inner) | (Op, Inner) | (Close, Inner) | (Inner, Ord) | (Inner, Open) | (Inner, Punct) | (Inner, Inner) => (1, true),
        (Punct, Ord | Op | Rel | Open | Close | Punct | Inner) => (1, true),
        _ => (0, false),
    };
    if script && parenthesised {
        return (0.0, 0.0);
    }
    ([0.0, 3.0, 4.0, 5.0][level], [0.0, 0.0, 2.0, 5.0][level])
}

/// The classes the atoms are spaced as, on their left and right: a binary operator that has nothing to
/// operate on is ordinary.
fn effective_classes(sides: &[(Class, Class)]) -> Vec<(Class, Class)> {
    let mut out: Vec<(Class, Class)> = Vec::with_capacity(sides.len());
    for (k, &(l, r)) in sides.iter().enumerate() {
        let (mut l, mut r) = (l, r);
        if l == Class::Bin && r == Class::Bin {
            let before = out.last().map(|c| c.1);
            let after = sides.get(k + 1).map(|c| c.0);
            let lonely_before = matches!(before, None | Some(Class::Bin | Class::Op | Class::Rel | Class::Open | Class::Punct));
            let lonely_after = matches!(after, None | Some(Class::Rel | Class::Close | Class::Punct));
            if lonely_before || lonely_after {
                (l, r) = (Class::Ord, Class::Ord);
            }
        }
        out.push((l, r));
    }
    out
}

/// The explicit spaces that make up `amount` (big points): one space, or up to three of one kind
/// (`\!\!`, `\,\,`); `None` when the amount is none of those.
fn explicit_spaces(amount: f32, mu: f32, em: f32) -> Option<Vec<Space>> {
    let kinds: [(Space, f32); 6] = [
        (Space::Qquad, 2.0 * em),
        (Space::Quad, em),
        (Space::Thick, 5.0 * mu),
        (Space::Medium, 4.0 * mu),
        (Space::Thin, 3.0 * mu),
        (Space::NegThin, -3.0 * mu),
    ];
    for n in 1..=3usize {
        for (space, width) in kinds {
            if (amount - n as f32 * width).abs() <= 0.15 && !(n > 1 && matches!(space, Space::Quad | Space::Qquad)) {
                return Some(vec![space; n]);
            }
        }
    }
    None
}

impl Reader {
    /// Puts the spaces the gaps between atoms show into the row: TeX's own glue is expected, and what is
    /// left over is an explicit space (`\\,`, `\\quad`, …) or the word space of a `\\text`.
    fn space(&mut self, mut atoms: Vec<Atom>, info: RowInfo) -> Vec<Node> {
        let mu = info.size / 18.0;
        let em = info.size;
        let script = info.size < 0.86 * self.top;
        // How far the gap before atom `i` is from TeX's own glue, and how much of that glue stretches.
        let leftover = |atoms: &[Atom], i: usize| -> Option<(f32, f32)> {
            let (a, b) = (atoms.get(i.checked_sub(1)?)?, atoms.get(i)?);
            let sides: Vec<(Class, Class)> = atoms.iter().map(|x| (x.class_l, x.class_r)).collect();
            let eff = effective_classes(&sides);
            let (ca, cb) = (eff.get(i - 1)?.1, eff.get(i)?.0);
            let (g, stretch) = glue(ca, cb, script);
            Some((b.x0 - a.right - (g + a.post + b.pre) * mu, stretch))
        };
        let cost_of = |atoms: &[Atom], i: usize| -> f32 {
            [i, i + 1]
                .iter()
                .filter_map(|&g| leftover(atoms, g))
                .map(|(l, _)| {
                    if l.abs() <= TOL {
                        0.0
                    } else if explicit_spaces(l, mu, em).is_some() {
                        1.0
                    } else {
                        3.0
                    }
                })
                .sum()
        };
        for i in 0..atoms.len() {
            // A glyph that is two commands (`|` and `\mid`) is the one the gaps around it fit.
            let alts = atoms.get(i).map_or(&[][..], |a| a.alts);
            if !alts.is_empty() {
                let mut best: Option<(f32, usize)> = None;
                for (k, alt) in alts.iter().enumerate() {
                    if let Some(a) = atoms.get_mut(i) {
                        (a.class_l, a.class_r, a.pre, a.post) = (alt.class, alt.class, alt.pre, alt.post);
                    }
                    let cost = cost_of(&atoms, i);
                    if best.is_none_or(|(c, _)| cost < c) {
                        best = Some((cost, k));
                    }
                }
                if let (Some((_, k)), Some(a)) = (best, atoms.get_mut(i))
                    && let Some(alt) = alts.get(k)
                {
                    (a.class_l, a.class_r, a.pre, a.post) = (alt.class, alt.class, alt.pre, alt.post);
                    if let Node::Symbol(sym) = &mut a.node {
                        sym.latex = alt.latex.to_owned();
                        sym.class = alt.class;
                    }
                }
            }
            // Delimiters that fit `\left…\right` and `\bigl…\bigr` alike: the glue around them says which.
            if let Some(n) = atoms.get(i).and_then(|a| a.either) {
                let set = |atoms: &mut Vec<Atom>, l: Class, r: Class| {
                    if let Some(a) = atoms.get_mut(i) {
                        (a.class_l, a.class_r) = (l, r);
                    }
                };
                set(&mut atoms, Class::Inner, Class::Inner);
                let auto = cost_of(&atoms, i);
                set(&mut atoms, Class::Open, Class::Close);
                let manual = cost_of(&atoms, i);
                if manual < auto {
                    if let Some(a) = atoms.get_mut(i)
                        && let Node::Delimited { sizing, .. } = &mut a.node
                    {
                        *sizing = Sizing::Manual(n);
                    }
                } else {
                    set(&mut atoms, Class::Inner, Class::Inner);
                }
            }
        }
        // Glue that stretches (around binary operators and relations) is stretched alike in a justified
        // line: its excess is then the line's, not the author's.
        let gaps: Vec<Option<(f32, f32)>> = (0..atoms.len()).map(|i| leftover(&atoms, i)).collect();
        let ratios: Vec<f32> = gaps.iter().flatten().filter(|(_, st)| *st > 0.0).map(|(l, st)| l / (st * mu)).collect();
        // One ratio, small, for every stretchable gap: a justified line. Anything else is the author's.
        let (lo, hi) = ratios.iter().fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), r| (lo.min(*r), hi.max(*r)));
        let stretched = ratios.iter().any(|r| r.abs() > 0.03) && hi - lo <= 0.1 && ratios.iter().all(|r| r.abs() <= 0.5);
        if stretched {
            self.note(Severity::Info, "the glue around operators and relations is stretched, as in a justified line");
        }
        let mut inserts: Vec<Vec<Space>> = vec![Vec::new(); atoms.len()];
        let mut word_before: Vec<bool> = vec![false; atoms.len()];
        let mut word_after: Vec<bool> = vec![false; atoms.len()];
        for (i, gap) in gaps.iter().enumerate() {
            let Some((l, st)) = *gap else { continue };
            if l.abs() <= TOL || (stretched && st > 0.0) {
                continue;
            }
            let size = atoms.get(i).map_or(em, |a| a.size);
            let word = (l - WORD_SPACE * size).abs() <= 0.15;
            // Upright letters a word space away from their neighbour are a `\text`.
            let as_text = |atoms: &mut Vec<Atom>, k: usize| -> bool {
                match atoms.get_mut(k).map(|a| &mut a.node) {
                    Some(Node::Text(_)) => true,
                    Some(node @ Node::Styled { alphabet: Alphabet::Roman, .. }) => {
                        if let Node::Styled { text, .. } = node {
                            let t = std::mem::take(text);
                            *node = Node::Text(t);
                        }
                        true
                    }
                    _ => false,
                }
            };
            if word && as_text(&mut atoms, i) {
                if let Some(w) = word_before.get_mut(i) {
                    *w = true;
                }
            } else if word && as_text(&mut atoms, i.saturating_sub(1)) {
                if let Some(w) = word_after.get_mut(i.saturating_sub(1)) {
                    *w = true;
                }
            } else if let Some(spaces) = explicit_spaces(l, mu, em) {
                // Glue that can stretch may be a justified line's, and spaces in a script are unusual.
                let thin_run = spaces.iter().all(|s| matches!(s, Space::Thin | Space::NegThin));
                if st > 0.0 || script || (spaces.len() > 1 && !thin_run) {
                    self.note(Severity::Ambiguous, format!("a gap of {l:.2} pt beyond TeX's glue, read as {} explicit space(s)", spaces.len()));
                }
                if let Some(slot) = inserts.get_mut(i) {
                    *slot = spaces;
                }
            } else {
                self.note(Severity::Ambiguous, format!("a gap of {l:.2} pt that is neither TeX's own glue nor an explicit space"));
            }
        }
        let mut out: Vec<Node> = Vec::with_capacity(atoms.len());
        for (i, a) in atoms.into_iter().enumerate() {
            if let Some(spaces) = inserts.get(i) {
                out.extend(spaces.iter().map(|s| Node::Space(*s)));
            }
            let mut node = a.node;
            if let Node::Text(t) = &mut node {
                if word_before.get(i) == Some(&true) {
                    t.insert(0, ' ');
                }
                if word_after.get(i) == Some(&true) {
                    t.push(' ');
                }
            }
            out.push(node);
        }
        out
    }
}

/// How far a gap may be from TeX's glue and still be it, in big points: positions are written to a
/// thousandth, and the glue to the sp.
const TOL: f32 = 0.22;

/// Italic correction of an element in ems (what TeX adds to a letter's width, and what a superscript
/// is shifted right by).
fn italic_of(e: &El) -> f32 {
    match &e.what {
        What::Sym(s) => f32::from(s.italic).max(0.0) / 1000.0,
        _ => 0.0,
    }
}

/// Whether `c`, after `nucleus`, is a script of it. `first` says nothing of the nucleus's scripts has
/// been seen yet; `right` is how far the scripts seen so far reach; `top` is one em of the formula's
/// own style.
fn is_script(nucleus: &El, c: &El, right: f32, first: bool, top: f32) -> bool {
    let smaller = c.size <= 0.86 * nucleus.size;
    if matches!(c.what, What::Delim(_)) {
        return false;
    }
    let gap = c.x0 - right;
    if first {
        // A script follows its base at once: after the base's italic correction and a half point.
        let reach = (italic_of(nucleus) + 0.12) * nucleus.size + 0.5;
        if gap > reach || c.x0 - nucleus.x1 < -(if nucleus.boxy() { 0.7 } else { 0.12 }) * nucleus.size {
            return false;
        }
    } else if gap > 0.45 * c.size {
        return false;
    }
    if smaller {
        if nucleus.boxy() {
            return true;
        }
        // The first mark must really be raised or lowered; the rest follow it.
        return !first || (c.base - nucleus.base).abs() >= 0.05 * nucleus.size;
    }
    // The same size: a script only in the smallest style, and only when it is clearly shifted.
    nucleus.size <= 0.6 * top && (c.base - nucleus.base).abs() >= 0.1 * nucleus.size && !nucleus.boxy()
}

/// Splits the marks after a nucleus into subscript and superscript groups.
fn partition_scripts(nucleus: &El, els: &[El], region: &[usize]) -> (Vec<usize>, Vec<usize>) {
    let s1 = region.iter().filter_map(|&j| els.get(j)).map(|e| e.size).fold(0.0, f32::max);
    let reference = if nucleus.boxy() { nucleus.cy() } else { nucleus.base };
    let eps = 0.02 * nucleus.size;
    let (mut sub, mut sup): (Vec<usize>, Vec<usize>) = (Vec::new(), Vec::new());
    let mut rest: Vec<usize> = Vec::new();
    for &j in region {
        let Some(e) = els.get(j) else { continue };
        if e.size >= 0.9 * s1 {
            if e.base < reference - eps {
                sup.push(j);
            } else {
                sub.push(j);
            }
        } else {
            rest.push(j);
        }
    }
    // Smaller marks join the group of the nearest mark before them.
    for &j in &rest {
        let Some(d) = els.get(j) else { continue };
        let cost = |m: usize| -> Option<f32> {
            let e = els.get(m)?;
            (e.x0 <= d.x0 + 0.5 * d.size).then(|| (d.x0 - e.x1).abs() + 2.0 * (d.base - e.base).abs())
        };
        let best = |group: &[usize]| group.iter().filter_map(|&m| cost(m)).fold(f32::INFINITY, f32::min);
        if best(&sup) < best(&sub) { sup.push(j) } else { sub.push(j) }
    }
    sub.sort_unstable();
    sup.sort_unstable();
    (sub, sup)
}

#[cfg(test)]
#[path = "layout_tests.rs"]
mod tests;
