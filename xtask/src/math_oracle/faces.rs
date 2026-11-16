//! `cargo xtask math-oracle faces [--only ID] [--check]`: the equation list typeset in other fonts
//! (`pdfcraft_oracle::tex::faces`).
//!
//! Each face gets `<id>.tex`, `<id>.pdf` (one equation per page) and `<id>.tsv` (which equation is on
//! which page, and those its fonts cannot set). Faces whose fonts are all OFL-1.1 are written to
//! `crates/render/tests/data/math/oracle/faces/` and committed; the others to the git-ignored
//! `target/math-oracle-faces/`, never committed. `--check` regenerates the committed faces and fails
//! unless every file is byte-identical. A face whose engine is not installed is skipped with a notice.

use super::{DATA_DIR, Equation, read_list};
use crate::gates::root;
use anyhow::{Context, bail};
use pdfcraft_oracle::tex::faces::{FACES, Face, FaceTex, embedded_font_files, face_source, pages_written};
use pdfcraft_oracle::tex::{Env, RUN_TIMEOUT, WorkDir};
use std::path::{Path, PathBuf};

/// Where committed faces live, relative to the repository root.
pub const COMMITTED_DIR: &str = "crates/render/tests/data/math/oracle/faces";
/// Where the faces that may not be committed are written, relative to the repository root.
pub const LOCAL_DIR: &str = "target/math-oracle-faces";

pub fn run(args: &[String]) -> anyhow::Result<()> {
    let only: Vec<&str> = args.iter().enumerate().filter(|(_, a)| *a == "--only").filter_map(|(i, _)| args.get(i + 1)).map(String::as_str).collect();
    let check = args.iter().any(|a| a == "--check");
    if let Some(bad) = only.iter().find(|id| !FACES.iter().any(|f| f.id == **id)) {
        bail!("no face `{bad}`; the faces are {:?}", FACES.iter().map(|f| f.id).collect::<Vec<_>>());
    }
    let list = read_list(&root().join(DATA_DIR).join("equations.toml"))?;
    let scratch = WorkDir::new("math-faces")?;
    let mut differ = Vec::new();
    for face in FACES.iter().filter(|f| only.is_empty() || only.contains(&f.id)).filter(|f| !check || f.committed) {
        let tex = match FaceTex::find(face) {
            Ok(t) => t,
            Err(absent) if absent.required() => bail!("{absent}"),
            Err(absent) => {
                eprintln!("math-oracle faces: {}: {absent}", face.id);
                continue;
            }
        };
        let dir = scratch.path().join(face.id);
        std::fs::create_dir_all(&dir)?;
        let files = generate_face(&tex, face, &list.equation, &dir).with_context(|| format!("face `{}`", face.id))?;
        let out: PathBuf = match (check, face.committed) {
            (true, _) => dir.join("out"),
            (false, true) => root().join(COMMITTED_DIR),
            (false, false) => root().join(LOCAL_DIR),
        };
        std::fs::create_dir_all(&out)?;
        for (name, bytes) in &files {
            std::fs::write(out.join(name), bytes).with_context(|| format!("writing {name}"))?;
            if check && std::fs::read(root().join(COMMITTED_DIR).join(name)).ok().as_ref() != Some(bytes) {
                differ.push(name.clone());
            }
        }
        if !check {
            println!("math-oracle faces: {} → {}", face.id, out.display());
        }
    }
    if !differ.is_empty() {
        bail!("generated face files differ from the committed ones: {differ:?}");
    }
    if check {
        println!("math-oracle faces: every committed face file is byte-identical with {COMMITTED_DIR}");
    }
    Ok(())
}

/// Typesets the list in `face`: finds the equations the face cannot set, then sets all the others in
/// one document.
fn generate_face(tex: &FaceTex, face: &Face, equations: &[Equation], dir: &Path) -> anyhow::Result<Vec<(String, Vec<u8>)>> {
    let entries: Vec<(Env, &str)> =
        equations.iter().map(|e| Ok((Env::parse(&e.env).context("env")?, e.tex.as_str()))).collect::<anyhow::Result<_>>()?;
    let mut excluded: Vec<(&str, String)> = Vec::new();
    let mut kept: Vec<usize> = Vec::new();
    // Those it cannot set are found by halving: one TeX run at a time (a LuaTeX run takes over half a
    // gigabyte, so running several at once can exhaust a machine's memory), and only a failing half is
    // split again.
    let mut runs = 0usize;
    let mut todo: Vec<Vec<usize>> = vec![(0..equations.len()).collect()];
    while let Some(part) = todo.pop() {
        let part_entries: Vec<(Env, &str)> = part.iter().filter_map(|&i| entries.get(i).copied()).collect();
        runs += 1;
        match try_document(tex, face, dir, &format!("part{runs}"), &part_entries) {
            Ok(_) => kept.extend(&part),
            Err(why) if part.len() == 1 => {
                if let Some(e) = part.first().and_then(|&i| equations.get(i)) {
                    excluded.push((e.id.as_str(), why.to_string().lines().find(|l| l.starts_with('!')).unwrap_or("does not typeset").to_owned()));
                }
            }
            Err(_) => {
                let (lo, hi) = part.split_at(part.len() / 2);
                todo.push(hi.to_vec());
                todo.push(lo.to_vec());
            }
        }
    }
    kept.sort_unstable();
    excluded.sort();
    if kept.is_empty() {
        bail!("the face sets none of the equations; the first reason: {}", excluded.first().map_or("none", |(_, why)| why.as_str()));
    }
    let kept_entries: Vec<(Env, &str)> = kept.iter().filter_map(|&i| entries.get(i).copied()).collect();
    let (pdf, log) = try_document(tex, face, dir, face.id, &kept_entries)?;
    let fonts = embedded_font_files(&log);
    if face.committed && fonts.is_empty() {
        bail!("the log names no embedded font file");
    }
    let mut tsv = format!(
        "# Generated by `cargo xtask math-oracle faces` from equations.toml: {}, typeset with {}. Do not edit.\n\
         # page\tid\tcategory\ttier\tenv\tsource   (env as in the list; the page sets it unnumbered)\n",
        face.about,
        tex.version().trim_start_matches("This is ")
    );
    for (page, &i) in kept.iter().enumerate() {
        let Some(e) = equations.get(i) else { continue };
        tsv.push_str(&format!("{page}\t{}\t{}\t{}\t{}\t{}\n", e.id, e.category, e.tier, e.env, e.tex));
    }
    for (id, why) in &excluded {
        tsv.push_str(&format!("# excluded\t{id}\t{why}\n"));
    }
    println!("{}: {} equations, {} the face cannot set; fonts {:?}", face.id, kept.len(), excluded.len(), fonts);
    Ok(vec![
        (format!("{}.tex", face.id), face_source(face, &kept_entries).into_bytes()),
        (format!("{}.pdf", face.id), pdf),
        (format!("{}.tsv", face.id), tsv.into_bytes()),
    ])
}

/// One document of `entries`: its PDF and log, provided it has one page per entry.
fn try_document(tex: &FaceTex, face: &Face, dir: &Path, stem: &str, entries: &[(Env, &str)]) -> anyhow::Result<(Vec<u8>, String)> {
    std::fs::write(dir.join(format!("{stem}.tex")), face_source(face, entries))?;
    let log = tex.compile(dir, stem, RUN_TIMEOUT).map_err(|e| anyhow::anyhow!("{e}"))?;
    if let Some(line) = log.lines().find(|l| l.starts_with("Overfull")) {
        bail!("! {line}");
    }
    if let Some(line) = log.lines().find(|l| l.contains("Missing character")) {
        bail!("! {line}");
    }
    // XeTeX's log names no font files; its PDFs are checked by name in the tests.
    let fonts = embedded_font_files(&log);
    let allowed = |f: &str| face.fonts.iter().any(|p| f.to_ascii_lowercase().starts_with(&p.to_ascii_lowercase()));
    if let Some(f) = fonts.iter().find(|f| !allowed(f)) {
        bail!("! embeds {f}, which is not one of the face's fonts {:?}", face.fonts);
    }
    let pages = pages_written(&log);
    if pages != Some(entries.len()) {
        bail!("! {pages:?} pages for {} equations", entries.len());
    }
    Ok((std::fs::read(dir.join(format!("{stem}.pdf")))?, log))
}
