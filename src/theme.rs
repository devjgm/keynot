//! Themes: colors used to draw slides.
//!
//! A theme starts from a named base (`dark` or `light`) and is then
//! adjusted by the `colors:` overrides in the frontmatter. Color values
//! accept anything ratatui's `Color::from_str` understands: hex strings
//! (`#rrggbb`), ANSI names (`red`, `lightcyan`), or indexed colors (`42`).

use crate::markdown::Metadata;
use ratatui::style::Color;
use std::str::FromStr;

/// Resolved colors and code theme for a presentation.
#[derive(Debug, Clone, PartialEq)]
pub struct Theme {
    pub background: Color,
    pub text: Color,
    pub heading: Color,
    pub accent: Color,
    pub link: Color,
    pub blockquote: Color,
    pub code_background: Color,
    /// Syntect theme name for code highlighting.
    pub code_theme: String,
}

#[derive(Debug, thiserror::Error)]
pub enum ThemeError {
    #[error("unknown theme `{0}` (available: dark, light)")]
    UnknownTheme(String),
    #[error("invalid color `{value}` for `colors.{field}`")]
    InvalidColor { field: String, value: String },
}

impl Theme {
    /// Deep navy with electric blue and amber: near-black blue background,
    /// blue headings and links, gold accents for highlights.
    pub fn dark() -> Self {
        Theme {
            background: Color::from_u32(0x000a0e17),
            text: Color::from_u32(0x00b8c4e6),
            heading: Color::from_u32(0x004d9fff),
            accent: Color::from_u32(0x00f7b500),
            link: Color::from_u32(0x006db3ff),
            blockquote: Color::from_u32(0x003d5aa8),
            code_background: Color::from_u32(0x00121a2e),
            code_theme: "base16-ocean.dark".to_string(),
        }
    }

    pub fn light() -> Self {
        Theme {
            background: Color::from_u32(0x00fafafa),
            text: Color::from_u32(0x00383a42),
            heading: Color::from_u32(0x000550ae),
            accent: Color::from_u32(0x00a626a4),
            link: Color::from_u32(0x000969da),
            blockquote: Color::from_u32(0x0050a14f),
            code_background: Color::from_u32(0x00eaeaeb),
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
        apply(&mut theme.background, "background", &colors.background)?;
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
        if let Some(code_theme) = &metadata.code_theme {
            theme.code_theme = code_theme.clone();
        }
        Ok(theme)
    }
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
        let theme = Theme::dark();
        assert_eq!(theme.background, Color::Rgb(0x0a, 0x0e, 0x17));
        assert_eq!(theme.heading, Color::Rgb(0x4d, 0x9f, 0xff));
        assert_eq!(theme.accent, Color::Rgb(0xf7, 0xb5, 0x00));
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
        assert_eq!(theme.background, Color::Rgb(0, 0, 0));
        assert_eq!(theme.text, Theme::light().text);
    }
}
