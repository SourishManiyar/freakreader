//! What the TeX oracles (`layout-oracle`, `refs-oracle`, `bib-oracle`) share: running cargo for
//! their reports and tests, and typesetting a document both with and without its `\special`
//! brackets.

use crate::gates::root;
use anyhow::{Context, bail};
use pdfcraft_oracle::tex::{DviPage, Output, Tex};
use std::path::Path;
use std::process::Command;
use std::time::Duration;

/// Where pdfTeX must have found every font it embedded: the AMS Type 1 fonts (OFL-1.1,
/// `crates/render/tests/data/math/OFL-AMSFonts.txt`).
pub(crate) const AMS_FONT_DIR: &str = "/fonts/type1/public/amsfonts/";

pub(crate) fn cargo(fixed: &[&str], extra: &[String]) -> anyhow::Result<()> {
    let mut c = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
    c.current_dir(root()).args(fixed).args(extra);
    let status = c.status().context("running cargo")?;
    if !status.success() {
        bail!("cargo {} exited with {status}", fixed.join(" "));
    }
    Ok(())
}

/// A document typeset to DVI (listed) and to PDF, with and without brackets.
pub(crate) struct Built {
    pub(crate) pages: Vec<DviPage>,
    pub(crate) pdf: Vec<u8>,
}

/// Typesets `source` (bracketed) and `plain_source` (the brackets expanding to nothing) in
/// `work/stem`, with more files beside the source: `(name, bracketed, plain)` (the layout oracle's
/// package, with and without its specials). The two PDFs must be identical, and every embedded
/// font an AMS Type 1 font.
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
