//! The "reading colours" view setting (Acrobat's Replace Document Colors): solid text and
//! line-art paints are remapped for readability while rendering. Display only — the document
//! itself never changes. The setting flows into [`hayro::RenderSettings`] through
//! `RenderConfig`.

/// The reading-colours setting for one document. `params: None` means off.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ReadingColors {
    /// The colours and flags, or `None` when the setting is off.
    pub params: Option<ReadingParams>,
}

/// Colours and flags of the reading-colours mapping.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ReadingParams {
    /// The sRGB colour that dark paints are mapped towards.
    pub text: [u8; 3],
    /// The sRGB colour that light paints are mapped towards.
    pub background: [u8; 3],
    /// When `true`, only near-black paints are mapped; lighter ones keep their colour.
    pub only_black: bool,
    /// When `true`, line-art paints are mapped in addition to text paints.
    pub line_art: bool,
    /// Dark mode: when set, paints keep their hue and only their lightness turns over (white to
    /// `background`, black to `text`), and this gamma, in hundredths ([`DARK_GAMMA_RANGE`]),
    /// then shapes the rendered tones between the two, pictures aside ([`dark_gamma`]). `None`
    /// maps every paint between the two colours.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gamma: Option<u16>,
}

/// The dark mode's gamma, in hundredths: 0.50 (thinner, dimmer text and middle tones) to 2.50
/// (bolder, brighter ones).
pub const DARK_GAMMA_RANGE: (u16, u16) = (50, 250);
/// The dark mode's gamma unless one is chosen: lightness turned over as it is.
pub const DARK_GAMMA_DEFAULT: u16 = 100;

impl ReadingColors {
    /// Black text on a white page.
    pub const BLACK_ON_WHITE: Self = Self {
        params: Some(ReadingParams { text: [0x00, 0x00, 0x00], background: [0xff, 0xff, 0xff], only_black: false, line_art: true, gamma: None }),
    };
    /// White text on a black page.
    pub const WHITE_ON_BLACK: Self = Self {
        params: Some(ReadingParams { text: [0xff, 0xff, 0xff], background: [0x00, 0x00, 0x00], only_black: false, line_art: true, gamma: None }),
    };
    /// Yellow text on a black page.
    pub const YELLOW_ON_BLACK: Self = Self {
        params: Some(ReadingParams { text: [0xff, 0xff, 0x00], background: [0x00, 0x00, 0x00], only_black: false, line_art: true, gamma: None }),
    };
    /// Green text on a black page.
    pub const GREEN_ON_BLACK: Self = Self {
        params: Some(ReadingParams { text: [0x00, 0xff, 0x00], background: [0x00, 0x00, 0x00], only_black: false, line_art: true, gamma: None }),
    };

    /// The document's dark mode: light text on a dark page, every paint keeping its hue, with
    /// `gamma` (hundredths, kept to [`DARK_GAMMA_RANGE`]) shaping the rendered tones between. Pictures,
    /// shadings and comments keep their own colours.
    pub const fn dark(gamma: u16) -> Self {
        let gamma = if gamma < DARK_GAMMA_RANGE.0 {
            DARK_GAMMA_RANGE.0
        } else if gamma > DARK_GAMMA_RANGE.1 {
            DARK_GAMMA_RANGE.1
        } else {
            gamma
        };
        Self {
            params: Some(ReadingParams {
                text: [0xe6, 0xe6, 0xe6],
                background: [0x1e, 0x1e, 0x1e],
                only_black: false,
                line_art: true,
                gamma: Some(gamma),
            }),
        }
    }

    /// This setting with a dark mode's gamma kept to [`DARK_GAMMA_RANGE`]: for settings read
    /// back from a file, which are untrusted.
    pub fn checked(mut self) -> Self {
        if let Some(p) = self.params.as_mut() {
            p.gamma = p.gamma.map(|g| g.clamp(DARK_GAMMA_RANGE.0, DARK_GAMMA_RANGE.1));
        }
        self
    }

    /// The dark mode's gamma in hundredths, when this is the dark mode.
    pub fn dark_gamma(&self) -> Option<u16> {
        self.params.and_then(|p| p.gamma)
    }

    /// Parses tool/settings input. `mode` is one of `off | black-on-white | white-on-black |
    /// yellow-on-black | green-on-black | custom`; `custom` needs `text` and `background` as
    /// `#rrggbb`; the flags override the preset defaults when given. `off` ignores colours.
    /// Errors are actionable strings. Use [`Self::parse_dark`] for `dark`.
    pub fn parse(mode: &str, text: Option<&str>, background: Option<&str>, only_black: Option<bool>, line_art: Option<bool>) -> Result<Self, String> {
        Self::parse_dark(mode, text, background, only_black, line_art, None)
    }

    /// [`Self::parse`], with `dark` too: the dark mode, whose `gamma` (0.5 to 2.5, default 1)
    /// shapes the rendered tones between page and text and whose `text` and `background` colours, when given, replace
    /// its own. `gamma` belongs to `dark` alone.
    pub fn parse_dark(
        mode: &str,
        text: Option<&str>,
        background: Option<&str>,
        only_black: Option<bool>,
        line_art: Option<bool>,
        gamma: Option<f64>,
    ) -> Result<Self, String> {
        if gamma.is_some() && mode != "dark" {
            return Err(format!("gamma only applies to mode dark, not {mode:?}"));
        }
        // Every mode accepts the flags: they override the preset defaults, and are the custom
        // mode's own values.
        let with_flags = |mut colors: Self| {
            if let Some(p) = colors.params.as_mut() {
                if let Some(v) = only_black {
                    p.only_black = v;
                }
                if let Some(v) = line_art {
                    p.line_art = v;
                }
            }
            colors
        };
        match mode {
            "off" => Ok(Self::default()),
            "dark" => {
                let (lo, hi) = (f64::from(DARK_GAMMA_RANGE.0) / 100.0, f64::from(DARK_GAMMA_RANGE.1) / 100.0);
                let gamma = match gamma {
                    None => DARK_GAMMA_DEFAULT,
                    Some(g) if g.is_finite() && (lo..=hi).contains(&g) => (g * 100.0).round() as u16,
                    Some(g) => return Err(format!("gamma {g} is out of range; expected a number from {lo} to {hi} (1 keeps the tones as they are)")),
                };
                let mut colors = with_flags(Self::dark(gamma));
                if let Some(p) = colors.params.as_mut() {
                    if let Some(text) = text {
                        p.text = parse_hex("text", text)?;
                    }
                    if let Some(background) = background {
                        p.background = parse_hex("background", background)?;
                    }
                }
                Ok(colors)
            }
            "black-on-white" => Ok(with_flags(Self::BLACK_ON_WHITE)),
            "white-on-black" => Ok(with_flags(Self::WHITE_ON_BLACK)),
            "yellow-on-black" => Ok(with_flags(Self::YELLOW_ON_BLACK)),
            "green-on-black" => Ok(with_flags(Self::GREEN_ON_BLACK)),
            "custom" => match (text, background) {
                (None, None) => Err("custom needs text and background as #rrggbb".into()),
                (None, Some(_)) => Err("custom needs text as #rrggbb".into()),
                (Some(_), None) => Err("custom needs background as #rrggbb".into()),
                (Some(text), Some(background)) => Ok(with_flags(Self {
                    params: Some(ReadingParams {
                        text: parse_hex("text", text)?,
                        background: parse_hex("background", background)?,
                        only_black: false,
                        line_art: true,
                        gamma: None,
                    }),
                })),
            },
            other => Err(format!(
                "unknown mode {other:?}; expected one of: off, dark, black-on-white, white-on-black, yellow-on-black, green-on-black, custom"
            )),
        }
    }

    /// The mode name for echoes and display: `"off"`, `"dark"` (whatever its gamma and
    /// colours), a preset name, or `"custom"`.
    pub fn mode_name(&self) -> &'static str {
        match self.params {
            None => "off",
            Some(params) => {
                if params.gamma.is_some() {
                    "dark"
                } else if Some(params) == Self::BLACK_ON_WHITE.params {
                    "black-on-white"
                } else if Some(params) == Self::WHITE_ON_BLACK.params {
                    "white-on-black"
                } else if Some(params) == Self::YELLOW_ON_BLACK.params {
                    "yellow-on-black"
                } else if Some(params) == Self::GREEN_ON_BLACK.params {
                    "green-on-black"
                } else {
                    "custom"
                }
            }
        }
    }

    /// The text colour as lowercase `#rrggbb`, or `None` when off.
    pub fn text_hex(&self) -> Option<String> {
        self.params.map(|p| hex(p.text))
    }

    /// The background colour as lowercase `#rrggbb`, or `None` when off.
    pub fn background_hex(&self) -> Option<String> {
        self.params.map(|p| hex(p.background))
    }
}

impl From<ReadingParams> for hayro::ColorReplacement {
    fn from(p: ReadingParams) -> Self {
        hayro::ColorReplacement {
            text: p.text,
            background: p.background,
            only_black: p.only_black,
            line_art: p.line_art,
            invert_lightness: p.gamma.is_some(),
            hide_paints: false,
        }
    }
}

/// Parses a colour given as `#rrggbb` — exactly this shape, either case of digits. The error
/// names the argument and the expected shape.
fn parse_hex(name: &str, value: &str) -> Result<[u8; 3], String> {
    let invalid = || format!("invalid {name} colour {value:?}; expected #rrggbb (six hex digits, e.g. #00ff00)");
    let Some(digits) = value.strip_prefix('#') else { return Err(invalid()) };
    if digits.len() != 6 || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(invalid());
    }
    let mut rgb = [0u8; 3];
    for (i, channel) in rgb.iter_mut().enumerate() {
        // Length and charset are checked above, so the parse cannot actually fail; the mapping
        // only keeps it total without panicking.
        *channel = u8::from_str_radix(&digits[i * 2..i * 2 + 2], 16).map_err(|_| invalid())?;
    }
    Ok(rgb)
}

/// The dark mode's gamma on a rendered page: each of `page`'s RGBA pixels whose channels lie
/// between the page colour and the text colour moves along that span as `x^(1/gamma)` (`x` = 0
/// at the page, 1 at the text), so with a gamma over 1 the antialiased edges of text and lines
/// and the middle tones grow brighter and the text reads bolder; under 1, thinner. `mask` is
/// the same page rendered with only what keeps its own colours (pictures, shadings, comments)
/// on a transparent ground: where it covers, the pixel is kept as rendered. Pages must have the
/// same length; a mismatch, or a gamma of 1, leaves `page` as it is.
pub(crate) fn dark_gamma(page: &mut [u8], mask: &[u8], params: &ReadingParams) {
    let Some(gamma) = params.gamma.filter(|g| *g != 100) else { return };
    if page.len() != mask.len() {
        return;
    }
    let exponent = 100.0 / f32::from(gamma.clamp(DARK_GAMMA_RANGE.0, DARK_GAMMA_RANGE.1));
    // One table per channel, between that channel's page and text values.
    let table = |from: u8, to: u8| -> [u8; 256] {
        let mut t = [0u8; 256];
        for (v, out) in (0u8..=255).zip(t.iter_mut()) {
            let (lo, hi) = (from.min(to), from.max(to));
            *out = if from == to || v < lo || v > hi {
                v
            } else {
                let x = (f32::from(v) - f32::from(from)) / (f32::from(to) - f32::from(from));
                let y = f32::from(from) + x.clamp(0.0, 1.0).powf(exponent) * (f32::from(to) - f32::from(from));
                // Between two byte values, so the rounded result fits a byte.
                y.round().clamp(0.0, 255.0) as u8
            };
        }
        t
    };
    let tables = [0, 1, 2].map(|c| table(params.background[c], params.text[c]));
    for (px, m) in page.as_chunks_mut::<4>().0.iter_mut().zip(mask.as_chunks::<4>().0) {
        let [.., mask_alpha] = *m;
        let keep = u32::from(mask_alpha);
        if keep == 255 {
            continue;
        }
        for (slot, t) in px.iter_mut().take(3).zip(&tables) {
            let v = *slot;
            let tv = t.get(usize::from(v)).copied().unwrap_or(v);
            // A blend of two bytes with weights summing to 255 stays a byte.
            *slot = ((u32::from(tv) * (255 - keep) + u32::from(v) * keep + 127) / 255).min(255) as u8;
        }
    }
}

/// Formats sRGB bytes as lowercase `#rrggbb`.
fn hex(rgb: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_round_trips_every_mode() {
        let off = ReadingColors::parse("off", None, None, None, None).unwrap();
        assert_eq!(off, ReadingColors::default());
        assert_eq!(off.mode_name(), "off");
        assert_eq!(off.text_hex(), None);
        assert_eq!(off.background_hex(), None);
        // `off` ignores colour and flag arguments.
        assert_eq!(ReadingColors::parse("off", Some("zzz"), Some("#000000"), Some(true), Some(false)).unwrap(), off);

        for (mode, text, background) in [
            ("black-on-white", "#000000", "#ffffff"),
            ("white-on-black", "#ffffff", "#000000"),
            ("yellow-on-black", "#ffff00", "#000000"),
            ("green-on-black", "#00ff00", "#000000"),
        ] {
            let colors = ReadingColors::parse(mode, None, None, None, None).unwrap();
            assert_eq!(colors.mode_name(), mode, "{mode}");
            assert_eq!(colors.text_hex().as_deref(), Some(text), "{mode}");
            assert_eq!(colors.background_hex().as_deref(), Some(background), "{mode}");
            let params = colors.params.unwrap();
            assert!(!params.only_black && params.line_art, "{mode}");
        }

        // The flags override the preset defaults; a changed preset is no longer one.
        let overridden = ReadingColors::parse("white-on-black", None, None, Some(true), Some(false)).unwrap();
        let params = overridden.params.unwrap();
        assert!(params.only_black && !params.line_art);
        assert_eq!(overridden.mode_name(), "custom");

        // `custom` takes either case of hex and echoes lowercase.
        let custom = ReadingColors::parse("custom", Some("#AaBbCc"), Some("#123456"), Some(true), None).unwrap();
        assert_eq!(custom.mode_name(), "custom");
        assert_eq!(custom.text_hex().as_deref(), Some("#aabbcc"));
        assert_eq!(custom.background_hex().as_deref(), Some("#123456"));
        let params = custom.params.unwrap();
        assert!(params.only_black && params.line_art);
    }

    #[test]
    fn dark_mode_parses_with_its_gamma_and_colours() {
        let dark = ReadingColors::parse_dark("dark", None, None, None, None, None).unwrap();
        assert_eq!(dark, ReadingColors::dark(DARK_GAMMA_DEFAULT));
        assert_eq!((dark.mode_name(), dark.dark_gamma()), ("dark", Some(100)));
        let tuned = ReadingColors::parse_dark("dark", Some("#FFF8E0"), Some("#101820"), None, None, Some(1.6)).unwrap();
        assert_eq!((tuned.mode_name(), tuned.dark_gamma()), ("dark", Some(160)));
        assert_eq!((tuned.text_hex().as_deref(), tuned.background_hex().as_deref()), (Some("#fff8e0"), Some("#101820")));
        let to = hayro::ColorReplacement::from(tuned.params.unwrap());
        assert!(to.invert_lightness && !to.hide_paints);
        assert!(!hayro::ColorReplacement::from(ReadingColors::WHITE_ON_BLACK.params.unwrap()).invert_lightness);
        for bad in [0.1, 3.0, f64::NAN, f64::INFINITY] {
            let err = ReadingColors::parse_dark("dark", None, None, None, None, Some(bad)).unwrap_err();
            assert!(err.contains("from 0.5 to 2.5"), "{err}");
        }
        assert_eq!(
            ReadingColors::parse_dark("white-on-black", None, None, None, None, Some(1.0)),
            Err("gamma only applies to mode dark, not \"white-on-black\"".to_string())
        );
        // Kept to its range, and saved settings without a gamma read as before.
        assert_eq!(ReadingColors::dark(10).dark_gamma(), Some(DARK_GAMMA_RANGE.0));
        assert_eq!(ReadingColors::dark(999).dark_gamma(), Some(DARK_GAMMA_RANGE.1));
        let mut wild = ReadingColors::dark(100);
        wild.params.as_mut().unwrap().gamma = Some(0);
        assert_eq!(wild.checked().dark_gamma(), Some(DARK_GAMMA_RANGE.0));
        assert_eq!(ReadingColors::WHITE_ON_BLACK.checked(), ReadingColors::WHITE_ON_BLACK);
        let old: ReadingColors =
            serde_json::from_str(r#"{"params":{"text":[255,255,255],"background":[0,0,0],"only_black":false,"line_art":true}}"#).unwrap();
        assert_eq!(old, ReadingColors::WHITE_ON_BLACK);
        assert!(!serde_json::to_string(&ReadingColors::WHITE_ON_BLACK).unwrap().contains("gamma"));
    }

    #[test]
    fn parse_rejects_unknown_mode() {
        assert_eq!(
            ReadingColors::parse("nope", None, None, None, None),
            Err("unknown mode \"nope\"; expected one of: off, dark, black-on-white, white-on-black, yellow-on-black, green-on-black, custom"
                .to_string())
        );
    }

    #[test]
    fn parse_rejects_malformed_hex() {
        for bad in ["zzz", "000000", "#000", "#0000000", "#00g000", ""] {
            assert_eq!(
                ReadingColors::parse("custom", Some(bad), Some("#000000"), None, None),
                Err(format!("invalid text colour {bad:?}; expected #rrggbb (six hex digits, e.g. #00ff00)")),
                "{bad}"
            );
        }
        assert_eq!(
            ReadingColors::parse("custom", Some("#000000"), Some("ffffff"), None, None),
            Err("invalid background colour \"ffffff\"; expected #rrggbb (six hex digits, e.g. #00ff00)".to_string())
        );
    }

    #[test]
    fn parse_rejects_custom_missing_colours() {
        assert_eq!(ReadingColors::parse("custom", None, None, None, None), Err("custom needs text and background as #rrggbb".to_string()));
        assert_eq!(ReadingColors::parse("custom", None, Some("#000000"), None, None), Err("custom needs text as #rrggbb".to_string()));
        assert_eq!(ReadingColors::parse("custom", Some("#000000"), None, None, None), Err("custom needs background as #rrggbb".to_string()));
    }
}
