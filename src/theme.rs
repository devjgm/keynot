//! Themes: colors used to draw slides.
//!
//! A theme starts from a named base (`dark` or `light`) and is then
//! adjusted by the `colors:` overrides in the frontmatter. Color values
//! accept anything ratatui's `Color::from_str` understands: hex strings
//! (`#rrggbb`), ANSI names (`red`, `lightcyan`), or indexed colors (`42`).

use crate::markdown::{BackgroundSpec, CodeStyle, GradientDirection, Metadata};
use ratatui::style::Color;
use std::str::FromStr;

/// A resolved background: one color, or a gradient between hex stops.
#[derive(Debug, Clone, PartialEq)]
pub enum Background {
    Solid(Color),
    Gradient {
        /// RGB stops, top/left/center first. Always at least two.
        stops: Vec<(u8, u8, u8)>,
        direction: GradientDirection,
    },
}

impl Background {
    /// The color at normalized position `t` in `[0, 1]` along the
    /// gradient (multi-stop, evenly spaced, linear RGB interpolation).
    /// A solid background is the same color everywhere.
    pub fn color_at(&self, t: f64) -> Color {
        match self {
            Background::Solid(color) => *color,
            Background::Gradient { stops, .. } => {
                let t = t.clamp(0.0, 1.0);
                let segments = (stops.len() - 1) as f64;
                let position = t * segments;
                let i = (position.floor() as usize).min(stops.len() - 2);
                let frac = position - i as f64;
                let (r0, g0, b0) = stops[i];
                let (r1, g1, b1) = stops[i + 1];
                let lerp = |a: u8, b: u8| (a as f64 + (b as f64 - a as f64) * frac).round() as u8;
                Color::Rgb(lerp(r0, r1), lerp(g0, g1), lerp(b0, b1))
            }
        }
    }

    /// The representative single color, for the places that can only
    /// take one (the highlight bar's foreground, transition fills): the
    /// solid color itself, or the gradient's midpoint.
    pub fn base(&self) -> Color {
        self.color_at(0.5)
    }

    pub fn direction(&self) -> GradientDirection {
        match self {
            Background::Solid(_) => GradientDirection::Vertical,
            Background::Gradient { direction, .. } => *direction,
        }
    }
}

/// Resolved colors and code theme for a presentation.
#[derive(Debug, Clone, PartialEq)]
pub struct Theme {
    pub background: Background,
    pub text: Color,
    pub heading: Color,
    pub accent: Color,
    pub link: Color,
    pub blockquote: Color,
    pub code_background: Color,
    /// The terminal-window border drawn around code blocks.
    pub code_border: Color,
    /// How code blocks are framed (`code_style:` in the frontmatter).
    pub code_style: CodeStyle,
    /// Syntect theme name for code highlighting.
    pub code_theme: String,
}

#[derive(Debug, thiserror::Error)]
pub enum ThemeError {
    #[error("unknown theme `{0}` (available: dark, light)")]
    UnknownTheme(String),
    #[error("invalid color `{value}` for `colors.{field}`")]
    InvalidColor { field: String, value: String },
    #[error("a background gradient needs at least 2 colors, got {0}")]
    GradientTooShort(usize),
    #[error("invalid gradient stop `{0}`: stops must be hex like `#rrggbb`")]
    GradientStopNotHex(String),
}

impl Theme {
    /// The VS Code Dark+ palette: charcoal background, keyword-blue
    /// headings, function-yellow accents, comment-green quote bars. The
    /// background is a vertical gradient through the Dark+ grays (menu
    /// gray down to near-black); code blocks sit on a darker panel with
    /// a terminal-window border so they stand out against it.
    pub fn dark() -> Self {
        Theme {
            background: Background::Gradient {
                stops: vec![(0x2d, 0x2d, 0x30), (0x18, 0x18, 0x18)],
                direction: GradientDirection::Vertical,
            },
            text: Color::from_u32(0x00d4d4d4),
            heading: Color::from_u32(0x00569cd6),
            accent: Color::from_u32(0x00dcdcaa),
            link: Color::from_u32(0x003794ff),
            blockquote: Color::from_u32(0x006a9955),
            code_background: Color::from_u32(0x00141414),
            code_border: Color::from_u32(0x00454545),
            code_style: CodeStyle::Window,
            code_theme: "Dark+".to_string(),
        }
    }

    pub fn light() -> Self {
        Theme {
            background: Background::Solid(Color::from_u32(0x00fafafa)),
            text: Color::from_u32(0x00383a42),
            heading: Color::from_u32(0x000550ae),
            accent: Color::from_u32(0x00a626a4),
            link: Color::from_u32(0x000969da),
            blockquote: Color::from_u32(0x0050a14f),
            code_background: Color::from_u32(0x00eaeaeb),
            code_border: Color::from_u32(0x00c4c4c8),
            code_style: CodeStyle::Window,
            code_theme: "InspiredGitHub".to_string(),
        }
    }

    fn named(name: &str) -> Result<Self, ThemeError> {
        match name {
            "dark" | "default" => Ok(Theme::dark()),
            "light" => Ok(Theme::light()),
            other => Err(ThemeError::UnknownTheme(other.to_string())),
        }
    }

    /// Build the effective theme for a presentation: named base theme plus
    /// any color overrides from the frontmatter.
    pub fn from_metadata(metadata: &Metadata) -> Result<Self, ThemeError> {
        let mut theme = match &metadata.theme {
            Some(name) => Theme::named(name)?,
            None => Theme::dark(),
        };
        let colors = &metadata.colors;
        if let Some(spec) = &colors.background {
            theme.background = resolve_background(spec)?;
        }
        apply(&mut theme.text, "text", &colors.text)?;
        apply(&mut theme.heading, "heading", &colors.heading)?;
        apply(&mut theme.accent, "accent", &colors.accent)?;
        apply(&mut theme.link, "link", &colors.link)?;
        apply(&mut theme.blockquote, "blockquote", &colors.blockquote)?;
        apply(
            &mut theme.code_background,
            "code_background",
            &colors.code_background,
        )?;
        apply(&mut theme.code_border, "code_border", &colors.code_border)?;
        if let Some(code_theme) = &metadata.code_theme {
            theme.code_theme = code_theme.clone();
        }
        theme.code_style = metadata.code_style;
        Ok(theme)
    }
}

/// Resolve the frontmatter background spec: a solid color (full color
/// syntax) or a gradient (hex-only stops -- ANSI names and palette
/// indexes have no fixed RGB to interpolate).
fn resolve_background(spec: &BackgroundSpec) -> Result<Background, ThemeError> {
    let (stops, direction) = match spec {
        BackgroundSpec::Solid(value) => {
            let color = Color::from_str(value).map_err(|_| ThemeError::InvalidColor {
                field: "background".to_string(),
                value: value.clone(),
            })?;
            return Ok(Background::Solid(color));
        }
        BackgroundSpec::Gradient(spec) => (&spec.gradient, spec.direction),
    };
    if stops.len() < 2 {
        return Err(ThemeError::GradientTooShort(stops.len()));
    }
    let stops = stops
        .iter()
        .map(|value| parse_hex(value).ok_or_else(|| ThemeError::GradientStopNotHex(value.clone())))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Background::Gradient { stops, direction })
}

/// Parse `#rrggbb` into RGB components.
fn parse_hex(value: &str) -> Option<(u8, u8, u8)> {
    let hex = value.strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    let n = u32::from_str_radix(hex, 16).ok()?;
    Some(((n >> 16) as u8, (n >> 8) as u8, n as u8))
}

fn apply(slot: &mut Color, field: &str, value: &Option<String>) -> Result<(), ThemeError> {
    if let Some(value) = value {
        *slot = Color::from_str(value).map_err(|_| ThemeError::InvalidColor {
            field: field.to_string(),
            value: value.clone(),
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metadata(yaml: &str) -> Metadata {
        Metadata::from_yaml(yaml).unwrap()
    }

    #[test]
    fn default_is_dark() {
        let theme = Theme::from_metadata(&Metadata::default()).unwrap();
        assert_eq!(theme, Theme::dark());
    }

    #[test]
    fn named_light_theme() {
        let theme = Theme::from_metadata(&metadata("theme: light")).unwrap();
        assert_eq!(theme, Theme::light());
    }

    #[test]
    fn dark_theme_palette() {
        // VS Code Dark+.
        let theme = Theme::dark();
        assert_eq!(
            theme.background,
            Background::Gradient {
                stops: vec![(0x2d, 0x2d, 0x30), (0x18, 0x18, 0x18)],
                direction: GradientDirection::Vertical,
            }
        );
        // The midpoint stays in the Dark+ charcoal family, so
        // everything keyed off base() stays in character.
        assert_eq!(theme.background.base(), Color::Rgb(0x23, 0x23, 0x24));
        assert_eq!(theme.heading, Color::Rgb(0x56, 0x9c, 0xd6));
        assert_eq!(theme.accent, Color::Rgb(0xdc, 0xdc, 0xaa));
    }

    #[test]
    fn unknown_theme_errors() {
        let err = Theme::from_metadata(&metadata("theme: neon")).unwrap_err();
        assert!(matches!(err, ThemeError::UnknownTheme(name) if name == "neon"));
    }

    #[test]
    fn hex_color_override() {
        let theme = Theme::from_metadata(&metadata("colors:\n  heading: '#ff0000'")).unwrap();
        assert_eq!(theme.heading, Color::Rgb(255, 0, 0));
        // Other colors keep their base values.
        assert_eq!(theme.text, Theme::dark().text);
    }

    #[test]
    fn named_color_override() {
        let theme = Theme::from_metadata(&metadata("colors:\n  accent: red")).unwrap();
        assert_eq!(theme.accent, Color::Red);
    }

    /// FORMAT.md's defaults tables must state what the code actually
    /// does; this fails the suite if a default changes without the doc.
    #[test]
    fn format_doc_lists_the_real_defaults() {
        let doc = include_str!("../FORMAT.md");
        let hex = |color: Color| match color {
            Color::Rgb(r, g, b) => format!("`#{r:02x}{g:02x}{b:02x}`"),
            other => panic!("theme colors are RGB: {other:?}"),
        };
        for theme in [Theme::dark(), Theme::light()] {
            match &theme.background {
                Background::Solid(color) => {
                    assert!(
                        doc.contains(&hex(*color)),
                        "FORMAT.md lacks {}",
                        hex(*color)
                    );
                }
                Background::Gradient { stops, .. } => {
                    for &(r, g, b) in stops {
                        let stop = format!("`#{r:02x}{g:02x}{b:02x}`");
                        assert!(doc.contains(&stop), "FORMAT.md lacks stop {stop}");
                    }
                }
            }
            for color in [
                theme.text,
                theme.heading,
                theme.accent,
                theme.link,
                theme.blockquote,
                theme.code_background,
                theme.code_border,
            ] {
                assert!(doc.contains(&hex(color)), "FORMAT.md lacks {}", hex(color));
            }
            assert!(doc.contains(&format!("`{}`", theme.code_theme)));
        }
        let transition = format!("{:?}", crate::markdown::Transition::default()).to_lowercase();
        assert!(
            doc.contains(&format!("| `transition` | `{transition}`")),
            "FORMAT.md defaults table lacks transition = {transition}"
        );
        let highlight = format!("{:?}", crate::markdown::HighlightStyle::default()).to_lowercase();
        assert!(
            doc.contains(&format!("| `highlight`  | `{highlight}`")),
            "FORMAT.md defaults table lacks highlight = {highlight}"
        );
        assert!(doc.contains("| `theme`      | `dark`"));
        let code_style = format!("{:?}", crate::markdown::CodeStyle::default()).to_lowercase();
        assert!(
            doc.contains(&format!("| `code_style` | `{code_style}`")),
            "FORMAT.md defaults table lacks code_style = {code_style}"
        );
    }

    #[test]
    fn gradient_background_resolves() {
        let metadata = crate::markdown::Metadata::from_yaml(
            "colors:\n  background:\n    gradient: ['#000000', '#ffffff']\n",
        )
        .unwrap();
        let theme = Theme::from_metadata(&metadata).unwrap();
        assert_eq!(theme.background.color_at(0.0), Color::Rgb(0, 0, 0));
        assert_eq!(theme.background.color_at(1.0), Color::Rgb(255, 255, 255));
        assert_eq!(theme.background.base(), Color::Rgb(128, 128, 128));
        assert_eq!(theme.background.direction(), GradientDirection::Vertical);
    }

    #[test]
    fn gradient_with_one_stop_errors() {
        let metadata = crate::markdown::Metadata::from_yaml(
            "colors:\n  background:\n    gradient: ['#000000']\n",
        )
        .unwrap();
        assert!(matches!(
            Theme::from_metadata(&metadata),
            Err(ThemeError::GradientTooShort(1))
        ));
    }

    #[test]
    fn gradient_stops_must_be_hex() {
        let metadata = crate::markdown::Metadata::from_yaml(
            "colors:\n  background:\n    gradient: ['red', '#ffffff']\n",
        )
        .unwrap();
        assert!(matches!(
            Theme::from_metadata(&metadata),
            Err(ThemeError::GradientStopNotHex(_))
        ));
    }

    #[test]
    fn multi_stop_gradient_interpolates_between_neighbors() {
        let background = Background::Gradient {
            stops: vec![(0, 0, 0), (100, 100, 100), (200, 0, 0)],
            direction: GradientDirection::Vertical,
        };
        // Midpoint = exactly the middle stop.
        assert_eq!(background.color_at(0.5), Color::Rgb(100, 100, 100));
        // A quarter in = halfway through the first segment.
        assert_eq!(background.color_at(0.25), Color::Rgb(50, 50, 50));
    }

    #[test]
    fn solid_background_is_uniform() {
        let solid = Background::Solid(Color::Rgb(9, 9, 9));
        assert_eq!(solid.color_at(0.0), solid.color_at(1.0));
        assert_eq!(solid.base(), Color::Rgb(9, 9, 9));
    }

    #[test]
    fn invalid_color_errors_with_field_name() {
        let err = Theme::from_metadata(&metadata("colors:\n  link: notacolor")).unwrap_err();
        match err {
            ThemeError::InvalidColor { field, value } => {
                assert_eq!(field, "link");
                assert_eq!(value, "notacolor");
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn code_theme_override() {
        let theme = Theme::from_metadata(&metadata("code_theme: Solarized (dark)")).unwrap();
        assert_eq!(theme.code_theme, "Solarized (dark)");
    }

    #[test]
    fn overrides_compose_with_named_theme() {
        let theme =
            Theme::from_metadata(&metadata("theme: light\ncolors:\n  background: '#000000'"))
                .unwrap();
        assert_eq!(theme.background, Background::Solid(Color::Rgb(0, 0, 0)));
        assert_eq!(theme.text, Theme::light().text);
    }
}
