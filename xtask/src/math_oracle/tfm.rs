//! TeX font metric (TFM) files: the width, height, depth and italic correction TeX gives each
//! character, which bound the box of an equation (the DVI listing gives places, not extents).
//!
//! The format is TeX's own, documented in TFtoPL; this reads only the header and the four
//! dimension tables.

use anyhow::{Context, bail};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;

/// A character's box in sp at the size the font was loaded at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CharBox {
    pub width: i64,
    pub height: i64,
    pub depth: i64,
    pub italic: i64,
}

/// One TFM file's character dimensions, as fix_words (units of 2⁻²⁰ design size).
#[derive(Debug, PartialEq, Eq)]
pub struct Tfm {
    first: u32,
    /// Per code from `first`: width, height, depth, italic; `None` for a code the font lacks.
    chars: Vec<Option<[i32; 4]>>,
}

fn half(bytes: &[u8], i: usize) -> Option<usize> {
    let b = bytes.get(i.checked_mul(2)?..i.checked_mul(2)?.checked_add(2)?)?;
    Some(usize::from(u16::from_be_bytes([*b.first()?, *b.get(1)?])))
}

fn word(bytes: &[u8], i: usize) -> Option<[u8; 4]> {
    let b = bytes.get(i.checked_mul(4)?..i.checked_mul(4)?.checked_add(4)?)?;
    Some([*b.first()?, *b.get(1)?, *b.get(2)?, *b.get(3)?])
}

impl Tfm {
    pub fn parse(bytes: &[u8]) -> anyhow::Result<Tfm> {
        let get = |i| half(bytes, i).context("the TFM file is cut short");
        let (lf, lh, bc, ec, nw, nh, nd, ni) = (get(0)?, get(1)?, get(2)?, get(3)?, get(4)?, get(5)?, get(6)?, get(7)?);
        if lf.saturating_mul(4) > bytes.len() || ec > 255 || bc > ec.saturating_add(1) {
            bail!("not a TFM file (lf {lf}, bc {bc}, ec {ec}, {} bytes)", bytes.len());
        }
        let char_base = 6usize.saturating_add(lh);
        let width_base = char_base.saturating_add(ec.saturating_add(1).saturating_sub(bc));
        let height_base = width_base.saturating_add(nw);
        let depth_base = height_base.saturating_add(nh);
        let italic_base = depth_base.saturating_add(nd);
        let fix = |base: usize, count: usize, index: usize| -> anyhow::Result<i32> {
            if index >= count {
                bail!("a dimension index past its table");
            }
            Ok(i32::from_be_bytes(word(bytes, base.saturating_add(index)).context("the TFM file is cut short")?))
        };
        let mut chars = Vec::new();
        for c in bc..=ec {
            let info = word(bytes, char_base.saturating_add(c.saturating_sub(bc))).context("the TFM file is cut short")?;
            let [w, hd, it, _] = info.map(usize::from);
            chars.push(if w == 0 {
                None
            } else {
                Some([fix(width_base, nw, w)?, fix(height_base, nh, hd >> 4)?, fix(depth_base, nd, hd & 15)?, fix(italic_base, ni, it >> 2)?])
            });
        }
        Ok(Tfm { first: u32::try_from(bc)?, chars })
    }

    /// The box of `code` at `size` sp, or `None` when the font has no such character.
    pub fn char_box(&self, code: u32, size: i64) -> Option<CharBox> {
        let dims = (*self.chars.get(usize::try_from(code.checked_sub(self.first)?).ok()?)?)?;
        let scale = |f: i32| (i64::from(f).saturating_mul(size)) >> 20;
        Some(CharBox { width: scale(dims[0]), height: scale(dims[1]), depth: scale(dims[2]), italic: scale(dims[3]) })
    }
}

/// TFM files found with `kpsewhich`, read once each.
#[derive(Default)]
pub struct Fonts(HashMap<String, Tfm>);

impl Fonts {
    pub fn get(&mut self, name: &str) -> anyhow::Result<&Tfm> {
        if !self.0.contains_key(name) {
            if !name.bytes().all(|b| b.is_ascii_alphanumeric()) {
                bail!("odd font name `{name}`");
            }
            let out = Command::new("kpsewhich").arg(format!("{name}.tfm")).output().context("running kpsewhich")?;
            let path = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
            if !out.status.success() || path.as_os_str().is_empty() {
                bail!("kpsewhich cannot find {name}.tfm");
            }
            let tfm = Tfm::parse(&std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?)
                .with_context(|| path.display().to_string())?;
            self.0.insert(name.to_owned(), tfm);
        }
        self.0.get(name).context("font table")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A two-character TFM by hand: codes 65 and 67 (66 missing), one width, height and depth each.
    fn tiny() -> Vec<u8> {
        let (lh, bc, ec, nw, nh, nd, ni) = (2u16, 65u16, 67u16, 2u16, 2u16, 2u16, 1u16);
        let lf = 6 + lh + (ec - bc + 1) + nw + nh + nd + ni;
        let mut b = Vec::new();
        for v in [lf, lh, bc, ec, nw, nh, nd, ni, 0, 0, 0, 0] {
            b.extend(v.to_be_bytes());
        }
        b.extend([0u8; 8]); // header
        b.extend([1, 0x11, 0, 0]); // 'A': width 1, height 1, depth 1
        b.extend([0, 0, 0, 0]); // 'B' absent
        b.extend([1, 0x10, 0, 0]); // 'C': width 1, height 1, depth 0
        for table in [[0i32, 1 << 19], [0, 1 << 20], [0, 1 << 18]] {
            for v in table {
                b.extend(v.to_be_bytes());
            }
        }
        b.extend(0i32.to_be_bytes()); // italic 0
        b
    }

    #[test]
    fn reads_scaled_boxes() {
        let tfm = Tfm::parse(&tiny()).unwrap();
        let ten = 655_360;
        assert_eq!(tfm.char_box(65, ten), Some(CharBox { width: ten / 2, height: ten, depth: ten / 4, italic: 0 }));
        assert_eq!(tfm.char_box(66, ten), None);
        assert_eq!(tfm.char_box(67, ten).map(|c| c.depth), Some(0));
        assert_eq!(tfm.char_box(64, ten), None);
        assert_eq!(tfm.char_box(68, ten), None);
    }

    #[test]
    fn cut_files_are_errors() {
        let full = tiny();
        for n in [0, 3, 24, 40, full.len() - 1] {
            assert!(Tfm::parse(&full[..n]).is_err(), "{n}");
        }
    }
}
