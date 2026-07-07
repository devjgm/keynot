//! Presentation-wide metadata parsed from the YAML frontmatter.

use serde::Deserialize;

/// Metadata from the frontmatter at the top of a `.keynot` file.
///
/// All fields are optional; unknown fields are ignored so files stay
/// forward-compatible with newer keynot versions. Known fields with
/// invalid values (e.g. a misspelled transition) are parse errors.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(default)]
pub struct Metadata {
    /// Presentation title, shown in the footer.
    pub title: Option<String>,
    /// Author name, shown in the footer.
    pub author: Option<String>,
    /// Free-form date string, shown in the footer next to the author.
    pub date: Option<String>,
    /// Base theme name: `dark` (default) or `light`.
    pub theme: Option<String>,
    /// Per-element color overrides applied on top of the base theme.
    pub colors: ColorOverrides,
    /// Syntect theme used for code blocks (e.g. `base16-ocean.dark`).
    pub code_theme: Option<String>,
    /// How slides change.
    pub transition: Transition,
    /// How the speaker's line highlight (up/down keys) is drawn.
    pub highlight: HighlightStyle,
    /// Whether to draw the footer (title, author, slide counter).
    pub footer: Option<bool>,
}

/// Slide transition style, the `transition:` frontmatter key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Transition {
    /// Push: the old slide slides out, the new one slides in.
    #[default]
    Slide,
    /// Characters dissolve into place.
    Coalesce,
    /// Fade in from the background color.
    Fade,
    /// Wipe across in the direction of navigation.
    Sweep,
    /// Instant switch.
    None,
}

/// How the speaker's line highlight is drawn, the `highlight:` key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HighlightStyle {
    /// An accent-colored bar across the content, behind the line.
    #[default]
    Bar,
    /// The highlighted line keeps full brightness; everything else dims.
    Dim,
}

/// Optional color overrides. Values accept hex (`"#rrggbb"`), ANSI names
/// (`"red"`, `"lightcyan"`), or indexed colors (`"42"`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(default)]
pub struct ColorOverrides {
    pub background: Option<String>,
    pub text: Option<String>,
    pub heading: Option<String>,
    pub accent: Option<String>,
    pub link: Option<String>,
    pub blockquote: Option<String>,
    pub code_background: Option<String>,
}

impl Metadata {
    /// Parse frontmatter YAML. Empty (or comment-only) input yields defaults.
    pub fn from_yaml(yaml: &str) -> Result<Self, serde_norway::Error> {
        if yaml.trim().is_empty() {
            return Ok(Metadata::default());
        }
        match serde_norway::from_str(yaml) {
            Ok(metadata) => Ok(metadata),
            // A frontmatter of only YAML comments deserializes as null;
            // map that to defaults rather than an error.
            Err(err) => match serde_norway::from_str::<serde_norway::Value>(yaml) {
                Ok(value) if value.is_null() => Ok(Metadata::default()),
                _ => Err(err),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_all_fields() {
        let yaml = "\
title: My Talk
author: Alice
date: 2026-07-07
theme: light
code_theme: InspiredGitHub
transition: fade
highlight: dim
footer: false
colors:
  background: '#101020'
  heading: red
";
        let m = Metadata::from_yaml(yaml).unwrap();
        assert_eq!(m.title.as_deref(), Some("My Talk"));
        assert_eq!(m.author.as_deref(), Some("Alice"));
        assert_eq!(m.date.as_deref(), Some("2026-07-07"));
        assert_eq!(m.theme.as_deref(), Some("light"));
        assert_eq!(m.code_theme.as_deref(), Some("InspiredGitHub"));
        assert_eq!(m.transition, Transition::Fade);
        assert_eq!(m.highlight, HighlightStyle::Dim);
        assert_eq!(m.footer, Some(false));
        assert_eq!(m.colors.background.as_deref(), Some("#101020"));
        assert_eq!(m.colors.heading.as_deref(), Some("red"));
        assert_eq!(m.colors.text, None);
    }

    #[test]
    fn empty_yaml_is_default() {
        assert_eq!(Metadata::from_yaml("").unwrap(), Metadata::default());
        assert_eq!(Metadata::from_yaml("   \n").unwrap(), Metadata::default());
    }

    #[test]
    fn comment_only_yaml_is_default() {
        let m = Metadata::from_yaml("# just a comment\n").unwrap();
        assert_eq!(m, Metadata::default());
    }

    #[test]
    fn default_transition_is_slide() {
        assert_eq!(Metadata::default().transition, Transition::Slide);
    }

    #[test]
    fn named_transitions_parse() {
        for (name, expected) in [
            ("slide", Transition::Slide),
            ("coalesce", Transition::Coalesce),
            ("fade", Transition::Fade),
            ("sweep", Transition::Sweep),
            ("none", Transition::None),
        ] {
            let m = Metadata::from_yaml(&format!("transition: {name}")).unwrap();
            assert_eq!(m.transition, expected);
        }
    }

    #[test]
    fn unknown_transition_is_an_error_listing_variants() {
        let err = Metadata::from_yaml("transition: spiral").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("unknown variant `spiral`"), "got: {msg}");
        assert!(msg.contains("slide"), "got: {msg}");
    }

    #[test]
    fn default_highlight_is_bar() {
        assert_eq!(Metadata::default().highlight, HighlightStyle::Bar);
    }

    #[test]
    fn named_highlight_styles_parse() {
        for (name, expected) in [("bar", HighlightStyle::Bar), ("dim", HighlightStyle::Dim)] {
            let m = Metadata::from_yaml(&format!("highlight: {name}")).unwrap();
            assert_eq!(m.highlight, expected);
        }
    }

    #[test]
    fn unknown_highlight_style_is_an_error() {
        assert!(Metadata::from_yaml("highlight: sparkles").is_err());
    }

    #[test]
    fn unknown_fields_are_ignored() {
        let m = Metadata::from_yaml("title: Hi\nfuture_option: 42\n").unwrap();
        assert_eq!(m.title.as_deref(), Some("Hi"));
    }

    #[test]
    fn unquoted_date_parses_as_string() {
        let m = Metadata::from_yaml("date: 2026-07-07").unwrap();
        assert_eq!(m.date.as_deref(), Some("2026-07-07"));
    }

    #[test]
    fn invalid_yaml_errors() {
        assert!(Metadata::from_yaml("title: [unclosed").is_err());
    }

    #[test]
    fn wrong_type_errors() {
        assert!(Metadata::from_yaml("colors: nope").is_err());
    }
}
