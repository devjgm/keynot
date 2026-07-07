//! Parsing of `.keynot` files.
//!
//! A `.keynot` file is a markdown document with:
//!
//! - Optional YAML frontmatter delimited by `---` lines at the very top,
//!   holding presentation-wide metadata (title, author, theme, ...).
//! - Slides written in plain markdown, separated by `---` on a line of its
//!   own (code fences are respected, so a `---` inside a fenced block does
//!   not split slides).
//! - HTML comments (`<!-- like this -->`) which never render; they are
//!   collected as per-slide speaker notes.
//!
//! The pipeline is: [`splitter`] cuts the raw text into frontmatter and raw
//! slide sources, [`metadata`] parses the frontmatter, and [`slide`] parses
//! each slide's markdown into a small block-level AST that the renderer
//! consumes.

mod metadata;
mod slide;
mod splitter;

pub use metadata::{ColorOverrides, HighlightStyle, Metadata, Transition};
pub use slide::{Block, InlineSpan, InlineStyle, ListBlock, ListItem, Slide};

/// A fully parsed presentation.
#[derive(Debug, Clone, PartialEq)]
pub struct Presentation {
    pub metadata: Metadata,
    pub slides: Vec<Slide>,
}

/// Errors produced while parsing a `.keynot` file.
#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("unterminated frontmatter: the leading `---` needs a closing `---`")]
    UnterminatedFrontmatter,
    #[error("invalid frontmatter: {0}")]
    Frontmatter(#[from] serde_norway::Error),
    #[error("the file contains no slides")]
    NoSlides,
}

impl Presentation {
    /// Parse the full source of a `.keynot` file.
    pub fn parse(source: &str) -> Result<Self, ParseError> {
        let split = splitter::split(source)?;
        let metadata = match &split.frontmatter {
            Some(yaml) => Metadata::from_yaml(yaml)?,
            None => Metadata::default(),
        };
        let slides: Vec<Slide> = split
            .slides
            .iter()
            .map(|raw| Slide::parse(&raw.content))
            .collect();
        if slides.is_empty() {
            return Err(ParseError::NoSlides);
        }
        Ok(Presentation { metadata, slides })
    }

    /// The presentation title: explicit metadata, or the first heading found.
    pub fn title(&self) -> Option<String> {
        if let Some(title) = &self.metadata.title {
            return Some(title.clone());
        }
        self.slides.iter().find_map(|s| s.title_text())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_frontmatter_and_slides() {
        let src = "---\ntitle: Demo\nauthor: Alice\n---\n\n# One\n\n---\n\n# Two\n";
        let p = Presentation::parse(src).unwrap();
        assert_eq!(p.metadata.title.as_deref(), Some("Demo"));
        assert_eq!(p.metadata.author.as_deref(), Some("Alice"));
        assert_eq!(p.slides.len(), 2);
    }

    #[test]
    fn parses_without_frontmatter() {
        let p = Presentation::parse("# Hello\n\ntext\n").unwrap();
        assert_eq!(p.metadata, Metadata::default());
        assert_eq!(p.slides.len(), 1);
    }

    #[test]
    fn empty_file_is_no_slides() {
        assert!(matches!(
            Presentation::parse("   \n\n"),
            Err(ParseError::NoSlides)
        ));
    }

    #[test]
    fn whitespace_only_slides_are_dropped() {
        let p = Presentation::parse("# A\n\n---\n\n   \n\n---\n\n# B\n").unwrap();
        assert_eq!(p.slides.len(), 2);
    }

    #[test]
    fn title_prefers_metadata() {
        let src = "---\ntitle: Meta Title\n---\n\n# Heading Title\n";
        let p = Presentation::parse(src).unwrap();
        assert_eq!(p.title().as_deref(), Some("Meta Title"));
    }

    #[test]
    fn title_falls_back_to_first_heading() {
        let p = Presentation::parse("some text\n\n---\n\n# Real Title\n").unwrap();
        assert_eq!(p.title().as_deref(), Some("Real Title"));
    }

    #[test]
    fn title_fallback_keeps_styled_heading_text() {
        // The bold tail must not be dropped from the title.
        let p = Presentation::parse("# Hello **World**\n").unwrap();
        assert_eq!(p.title().as_deref(), Some("Hello World"));
    }

    #[test]
    fn title_none_when_no_heading_or_metadata() {
        let p = Presentation::parse("just a paragraph\n").unwrap();
        assert_eq!(p.title(), None);
    }

    #[test]
    fn bad_yaml_is_an_error() {
        let src = "---\ntitle: [unclosed\n---\n\n# Slide\n";
        assert!(matches!(
            Presentation::parse(src),
            Err(ParseError::Frontmatter(_))
        ));
    }
}
