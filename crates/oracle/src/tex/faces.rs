//! Faces: the equation list typeset in fonts other than Computer Modern.
//!
//! The truth of the oracle's main set comes from DVI, which only pdfTeX with TFM fonts writes in a
//! form `dvitype` reads. XeTeX writes XDV and LuaTeX's DVI names OpenType fonts no TFM describes, so a
//! face's truth is its PDF instead: each equation is set alone on a page of its own (nothing else on
//! the page, no equation number), and a candidate is typeset the same way, by the same engine, with
//! the same fonts. Two typesets are compared as the main set compares DVI items
//! ([`super::same_typeset`]), with the glyphs and rules read from the two PDFs by the caller (the
//! renderer's glyph details: font name, size, code, baseline origin).
//!
//! Licences decide where a face's PDF may live. A PDF embeds subsets of the fonts it uses, so only a
//! face whose fonts are all OFL-1.1 ([`Face::committed`]) is committed
//! (`crates/render/tests/data/math/oracle/faces/`); the others (TeX Gyre and Latin Modern are under
//! the GUST Font License, the Times clones under the GPL or AGPL with a font exception, none of them
//! on the allowlist) are generated into the git-ignored `target/math-oracle-faces/` for `#[ignore]`d
//! tests on a machine with TeX Live, and never committed.

use super::{Env, RUN_TIMEOUT, TEX_ENV, TexError, WorkDir, log_tail, run, single_page_geometry};
use crate::{Absent, Oracle};
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

/// The TeX engine a face is typeset with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Engine {
    PdfLatex,
    LuaLatex,
    XeLatex,
}

impl Engine {
    pub fn program(self) -> &'static str {
        match self {
            Engine::PdfLatex => "pdflatex",
            Engine::LuaLatex => "lualatex",
            Engine::XeLatex => "xelatex",
        }
    }

    fn find(self) -> Result<Oracle, Absent> {
        match self {
            Engine::PdfLatex => crate::pdflatex(),
            Engine::LuaLatex => crate::lualatex(),
            Engine::XeLatex => crate::xelatex(),
        }
    }
}

/// One way of setting the equation list in other fonts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Face {
    /// Short name: `[a-z0-9-]`, the stem of its files.
    pub id: &'static str,
    pub engine: Engine,
    /// What it is, for the report.
    pub about: &'static str,
    /// The preamble lines that choose its fonts, after `\documentclass{article}`.
    pub setup: &'static str,
    /// Every font it embeds is OFL-1.1, so its PDF is committed; otherwise it is generated locally only.
    pub committed: bool,
    /// The fonts its PDF may embed, by the start of their PDF names (subset prefix removed) or of
    /// their file names as TeX's log gives them, ignoring case. The generator and the tests refuse a
    /// PDF that embeds anything else: for a committed face this is the licence check.
    pub fonts: &'static [&'static str],
    /// Looked at equation by equation while the reader's Unicode path was written. The others were
    /// scored only as a whole.
    pub development: bool,
}

/// Every face, development faces first.
pub const FACES: [Face; 11] = [
    Face {
        id: "stix2",
        engine: Engine::LuaLatex,
        about: "STIX Two Math and STIX Two Text, unicode-math, LuaLaTeX",
        setup: "\\usepackage{amsmath}\n\\usepackage{unicode-math}\n\
                \\setmainfont{STIXTwoText}[Extension=.otf,UprightFont=*-Regular,BoldFont=*-Bold,ItalicFont=*-Italic,BoldItalicFont=*-BoldItalic]\n\
                \\setsansfont{FiraSans}[Extension=.otf,UprightFont=*-Regular,BoldFont=*-Bold,ItalicFont=*-Italic,BoldItalicFont=*-BoldItalic]\n\
                \\setmonofont{FiraMono}[Extension=.otf,UprightFont=*-Regular,BoldFont=*-Bold]\n\
                \\setmathfont{STIXTwoMath-Regular.otf}\n",
        committed: true,
        fonts: &["STIXTwoMath-", "STIXTwoText-", "FiraSans-", "FiraMono-"],
        development: true,
    },
    Face {
        id: "newtx",
        engine: Engine::PdfLatex,
        about: "newtxtext and newtxmath (Times), pdfLaTeX",
        setup: "\\usepackage{amsmath}\n\\usepackage{newtxtext,newtxmath}\n",
        committed: false,
        fonts: &["NewTX", "TeXGyreTermesX-", "ztm", "tx", "zhv", "zi4"],
        development: true,
    },
    Face {
        id: "fira",
        engine: Engine::LuaLatex,
        about: "Fira Math and Fira Sans, unicode-math, LuaLaTeX",
        setup: "\\usepackage{amsmath}\n\\usepackage{unicode-math}\n\
                \\setmainfont{FiraSans}[Extension=.otf,UprightFont=*-Regular,BoldFont=*-Bold,ItalicFont=*-Italic,BoldItalicFont=*-BoldItalic]\n\
                \\setsansfont{FiraSans}[Extension=.otf,UprightFont=*-Regular,BoldFont=*-Bold,ItalicFont=*-Italic,BoldItalicFont=*-BoldItalic]\n\
                \\setmonofont{FiraMono}[Extension=.otf,UprightFont=*-Regular,BoldFont=*-Bold]\n\
                \\setmathfont{FiraMath-Regular.otf}\n",
        committed: true,
        fonts: &["FiraMath-", "FiraSans-", "FiraMono-"],
        development: false,
    },
    Face {
        id: "libertinus",
        engine: Engine::LuaLatex,
        about: "Libertinus Math and Libertinus Serif, unicode-math, LuaLaTeX",
        setup: "\\usepackage{amsmath}\n\\usepackage{unicode-math}\n\
                \\setmainfont{LibertinusSerif}[Extension=.otf,UprightFont=*-Regular,BoldFont=*-Bold,ItalicFont=*-Italic,BoldItalicFont=*-BoldItalic]\n\
                \\setsansfont{LibertinusSans}[Extension=.otf,UprightFont=*-Regular,BoldFont=*-Bold,ItalicFont=*-Italic]\n\
                \\setmonofont{LibertinusMono-Regular.otf}\n\
                \\setmathfont{LibertinusMath-Regular.otf}\n",
        committed: true,
        fonts: &["LibertinusMath-", "LibertinusSerif-", "LibertinusSans-", "LibertinusMono-"],
        development: false,
    },
    Face {
        id: "stix2-xe",
        engine: Engine::XeLatex,
        about: "STIX Two Math and STIX Two Text, unicode-math, XeLaTeX",
        setup: "\\usepackage{amsmath}\n\\usepackage{unicode-math}\n\
                \\setmainfont{STIXTwoText}[Extension=.otf,UprightFont=*-Regular,BoldFont=*-Bold,ItalicFont=*-Italic,BoldItalicFont=*-BoldItalic]\n\
                \\setsansfont{FiraSans}[Extension=.otf,UprightFont=*-Regular,BoldFont=*-Bold,ItalicFont=*-Italic,BoldItalicFont=*-BoldItalic]\n\
                \\setmonofont{FiraMono}[Extension=.otf,UprightFont=*-Regular,BoldFont=*-Bold]\n\
                \\setmathfont{STIXTwoMath-Regular.otf}\n",
        committed: false,
        fonts: &["STIXTwoMath-", "STIXTwoText-", "FiraSans-", "FiraMono-"],
        development: false,
    },
    Face {
        id: "termes",
        engine: Engine::LuaLatex,
        about: "TeX Gyre Termes Math and TeX Gyre Termes, unicode-math, LuaLaTeX (GUST Font License: local only)",
        setup: "\\usepackage{amsmath}\n\\usepackage{unicode-math}\n\
                \\setmainfont{texgyretermes}[Extension=.otf,UprightFont=*-regular,BoldFont=*-bold,ItalicFont=*-italic,BoldItalicFont=*-bolditalic]\n\
                \\setmathfont{texgyretermes-math.otf}\n",
        committed: false,
        fonts: &["TeXGyreTermes", "LMSans", "LMMono"],
        development: false,
    },
    Face {
        id: "pagella",
        engine: Engine::LuaLatex,
        about: "TeX Gyre Pagella Math and TeX Gyre Pagella, unicode-math, LuaLaTeX (GUST Font License: local only)",
        setup: "\\usepackage{amsmath}\n\\usepackage{unicode-math}\n\
                \\setmainfont{texgyrepagella}[Extension=.otf,UprightFont=*-regular,BoldFont=*-bold,ItalicFont=*-italic,BoldItalicFont=*-bolditalic]\n\
                \\setmathfont{texgyrepagella-math.otf}\n",
        committed: false,
        fonts: &["TeXGyrePagella", "LMSans", "LMMono"],
        development: false,
    },
    Face {
        id: "bonum",
        engine: Engine::LuaLatex,
        about: "TeX Gyre Bonum Math and TeX Gyre Bonum, unicode-math, LuaLaTeX (GUST Font License: local only)",
        setup: "\\usepackage{amsmath}\n\\usepackage{unicode-math}\n\
                \\setmainfont{texgyrebonum}[Extension=.otf,UprightFont=*-regular,BoldFont=*-bold,ItalicFont=*-italic,BoldItalicFont=*-bolditalic]\n\
                \\setmathfont{texgyrebonum-math.otf}\n",
        committed: false,
        fonts: &["TeXGyreBonum", "LMSans", "LMMono"],
        development: false,
    },
    Face {
        id: "lm-math",
        engine: Engine::LuaLatex,
        about: "Latin Modern Math and Latin Modern Roman, unicode-math, LuaLaTeX (GUST Font License: local only)",
        setup: "\\usepackage{amsmath}\n\\usepackage{unicode-math}\n\\setmathfont{latinmodern-math.otf}\n",
        committed: false,
        fonts: &["LatinModernMath-", "latinmodern-math", "LMRoman", "LMSans", "LMMono"],
        development: false,
    },
    Face {
        id: "mathptmx",
        engine: Engine::PdfLatex,
        about: "mathptmx (Times with Computer Modern and symbol-font math), pdfLaTeX (local only)",
        setup: "\\usepackage{amsmath,amssymb}\n\\usepackage{mathptmx}\n",
        committed: false,
        fonts: &["Nimbus", "utm", "StandardSym", "usy", "CM", "MSAM", "MSBM", "rsfs", "EUF", "EUR"],
        development: false,
    },
    Face {
        id: "txfonts",
        engine: Engine::PdfLatex,
        about: "txfonts (Times), pdfLaTeX (local only)",
        setup: "\\usepackage{amsmath}\n\\usepackage{txfonts}\n",
        committed: false,
        fonts: &["Nimbus", "utm", "rtx", "tx", "CM", "EUF"],
        development: false,
    },
];

/// The face called `id`.
pub fn face(id: &str) -> Option<&'static Face> {
    FACES.iter().find(|f| f.id == id)
}

/// The environment an equation is set in on a face's page: numbered environments lose their
/// numbers, so the page holds the equation and nothing else.
pub fn face_env(env: Env) -> Env {
    match env {
        Env::Equation => Env::Display,
        Env::Align => Env::AlignStar,
        Env::Gather => Env::GatherStar,
        e => e,
    }
}

/// A document of the face with one page per `(env, body)`.
pub fn face_source(face: &Face, entries: &[(Env, &str)]) -> String {
    let mut s = String::from("\\documentclass{article}\n");
    // The trailer's /ID is made from the job's path, which differs on every run: a fixed one keeps the
    // committed PDFs reproducible (`cargo xtask math-oracle faces --check`).
    match face.engine {
        Engine::LuaLatex => s.push_str("\\pdfvariable trailerid {[<00000000000000000000000000000000><00000000000000000000000000000000>]}\n"),
        Engine::PdfLatex => s.push_str("\\pdftrailerid{}\n"),
        Engine::XeLatex => {}
    }
    s.push_str(face.setup);
    s.push_str(&format!("\\usepackage[{}]{{geometry}}\n", single_page_geometry()));
    // A glyph the fonts lack is logged ("Missing character"), so the generator leaves the equation out.
    s.push_str("\\pagestyle{empty}\n\\setlength{\\parindent}{0pt}\n\\tracinglostchars=1\n\\begin{document}\n");
    for (i, (env, body)) in entries.iter().enumerate() {
        if i > 0 {
            s.push_str("\\newpage\n");
        }
        let (open, close) = face_env(*env).delimiters();
        s.push_str(&format!("{open}\n{body}\n{close}\n\\par\n"));
    }
    s.push_str("\\end{document}\n");
    s
}

/// The number of pages TeX says it wrote (`Output written on x.pdf (N pages, …)`).
pub fn pages_written(log: &str) -> Option<usize> {
    let at = log.rfind("Output written on")?;
    let rest = log.get(at..)?;
    let open = rest.find('(')?;
    let digits: String = rest.get(open + 1..)?.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// The font files TeX names as embedded in its log (`<…/name.pfb>`, `<…/name.otf>`), by file name.
pub fn embedded_font_files(log: &str) -> Vec<String> {
    let mut out: Vec<String> = log
        .split('<')
        .filter_map(|s| s.split_once('>').map(|(f, _)| f))
        .filter(|f| [".pfb", ".otf", ".ttf"].iter().any(|ext| f.to_ascii_lowercase().ends_with(ext)))
        .map(|f| f.rsplit('/').next().unwrap_or(f).to_owned())
        .collect();
    out.sort();
    out.dedup();
    out
}

/// The engine of a face, found on `PATH`.
#[derive(Clone, Debug)]
pub struct FaceTex {
    face: Face,
    engine: Oracle,
}

/// A batch of candidates typeset in a face: the PDFs made, and where each candidate's page is (PDF
/// index, page index) or why it has none.
#[derive(Debug, Default)]
pub struct FaceTypeset {
    pub pdfs: Vec<Vec<u8>>,
    pub placed: Vec<Result<(usize, usize), String>>,
}

impl FaceTex {
    pub fn find(face: &Face) -> Result<FaceTex, Absent> {
        Ok(FaceTex { face: *face, engine: face.engine.find()? })
    }

    pub fn face(&self) -> &Face {
        &self.face
    }

    pub fn version(&self) -> &str {
        self.engine.version()
    }

    /// Runs the engine on `dir/stem.tex` to PDF, stopping at the first error; returns the log.
    pub fn compile(&self, dir: &Path, stem: &str, timeout: Duration) -> Result<String, TexError> {
        let mut c = self.engine.command();
        c.current_dir(dir).args(["-no-shell-escape", "-interaction=batchmode", "-halt-on-error"]);
        c.arg(format!("{stem}.tex")).envs(TEX_ENV).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        let what = format!("{} {stem}.tex", self.face.engine.program());
        let status = run(c, timeout, &what)?;
        let log_path = dir.join(format!("{stem}.log"));
        if !status.success() {
            return Err(TexError::Failed { what: format!("{what} failed ({status})"), log_tail: log_tail(&log_path) });
        }
        std::fs::read(&log_path).map(|b| String::from_utf8_lossy(&b).into_owned()).map_err(|e| TexError::Io(format!("{}: {e}", log_path.display())))
    }

    /// Typesets `entries` one per page: all in one run, and one by one when that run fails or makes
    /// a page count other than one per entry, so one bad candidate does not cost the others theirs.
    pub fn typeset(&self, entries: &[(Env, &str)]) -> Result<FaceTypeset, TexError> {
        let work = WorkDir::new("face")?;
        let mut out = FaceTypeset::default();
        let checked: Vec<Result<(), String>> = entries.iter().map(|(_, b)| super::check_candidate(b)).collect();
        let todo: Vec<usize> = checked.iter().enumerate().filter(|(_, c)| c.is_ok()).map(|(i, _)| i).collect();
        out.placed = checked.into_iter().map(|c| Err(c.err().unwrap_or_else(|| "not typeset".to_owned()))).collect();
        if todo.is_empty() {
            return Ok(out);
        }
        let batch: Vec<(Env, &str)> = todo.iter().filter_map(|&i| entries.get(i).copied()).collect();
        if let Ok(pdf) = self.typeset_in(work.path(), "batch", &batch, RUN_TIMEOUT) {
            out.pdfs.push(pdf);
            for (page, &i) in todo.iter().enumerate() {
                if let Some(slot) = out.placed.get_mut(i) {
                    *slot = Ok((0, page));
                }
            }
            return Ok(out);
        }
        for (k, &i) in todo.iter().enumerate() {
            let Some(&entry) = batch.get(k) else { continue };
            let r = match self.typeset_in(work.path(), &format!("single{k}"), &[entry], super::CANDIDATE_TIMEOUT) {
                Ok(pdf) => {
                    out.pdfs.push(pdf);
                    Ok((out.pdfs.len() - 1, 0))
                }
                Err(TexError::Failed { what, log_tail }) => Err(format!("{what}\n{log_tail}")),
                Err(e @ (TexError::Absent(_) | TexError::Io(_))) => return Err(e),
                Err(TexError::Listing(s)) => Err(s),
            };
            if let Some(slot) = out.placed.get_mut(i) {
                *slot = r;
            }
        }
        Ok(out)
    }

    fn typeset_in(&self, dir: &Path, stem: &str, entries: &[(Env, &str)], timeout: Duration) -> Result<Vec<u8>, TexError> {
        let tex = dir.join(format!("{stem}.tex"));
        std::fs::write(&tex, face_source(&self.face, entries)).map_err(|e| TexError::Io(format!("{}: {e}", tex.display())))?;
        let log = self.compile(dir, stem, timeout)?;
        let pages = pages_written(&log);
        if pages != Some(entries.len()) {
            return Err(TexError::Listing(format!("{pages:?} pages typeset for {} candidates", entries.len())));
        }
        let pdf = dir.join(format!("{stem}.pdf"));
        std::fs::read(&pdf).map_err(|e| TexError::Io(format!("{}: {e}", pdf.display())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn faces_are_named_and_committed_only_with_open_fonts() {
        let mut ids: Vec<&str> = FACES.iter().map(|f| f.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), FACES.len());
        for f in FACES {
            assert!(f.id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'), "{}", f.id);
            assert!(!f.fonts.is_empty(), "{}", f.id);
            if f.committed {
                // OFL-1.1 families only: STIX Two, Fira, Libertinus.
                assert!(
                    f.fonts.iter().all(|p| ["STIXTwo", "Fira", "Libertinus"].iter().any(|ofl| p.starts_with(ofl))),
                    "{} is committed with fonts {:?}",
                    f.id,
                    f.fonts
                );
            }
        }
    }

    #[test]
    fn a_face_page_holds_the_equation_without_a_number() {
        let Some(stix) = face("stix2") else { panic!("stix2") };
        let doc = face_source(stix, &[(Env::Equation, "x"), (Env::Align, "a &= b"), (Env::Inline, "y")]);
        assert!(doc.contains("\\[\nx\n\\]"));
        assert!(doc.contains("\\begin{align*}\na &= b\n\\end{align*}"));
        assert!(doc.contains("$\ny\n$"));
        assert_eq!(doc.matches("\\newpage").count(), 2);
        assert!(!doc.contains("pdfcraft"));
    }

    #[test]
    fn logs_say_pages_and_fonts() {
        let log = "x\nOutput written on batch.pdf (3 pages, 12345 bytes).\n</usr/share/a/STIXTwoMath-Regular.otf></x/cmr10.pfb>";
        assert_eq!(pages_written(log), Some(3));
        assert_eq!(pages_written("no output"), None);
        assert_eq!(embedded_font_files(log), vec!["STIXTwoMath-Regular.otf".to_owned(), "cmr10.pfb".to_owned()]);
    }
}
