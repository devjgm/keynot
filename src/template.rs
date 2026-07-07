//! The skeleton presentation written by `keynot new`: the shipped tour
//! example (embedded at compile time), personalized with the new file's
//! title.

/// The tour example, embedded so `keynot new` works from any directory
/// and always matches the documented feature set.
const TOUR: &str = include_str!("../examples/tour.keynot");

/// The starter presentation: the tour, with its title (frontmatter and
/// title slide) replaced by `title`, a placeholder author, and the date
/// dropped.
pub fn skeleton(title: &str) -> String {
    TOUR.replace("A Tour of keynot", title)
        .replacen("author: The keynot Authors\n", "author: Your Name\n", 1)
        .replacen("date: 2026-07-07\n", "", 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown::Presentation;

    #[test]
    fn skeleton_parses_cleanly() {
        let src = skeleton("Demo Talk");
        let p = Presentation::parse(&src).expect("skeleton must parse");
        assert_eq!(p.metadata.title.as_deref(), Some("Demo Talk"));
        assert_eq!(p.metadata.author.as_deref(), Some("Your Name"));
        assert_eq!(p.metadata.date, None);
        assert_eq!(p.slides.len(), 8);
    }

    /// If these fail, the tour's frontmatter changed and the replacement
    /// markers in [`skeleton`] need updating to match.
    #[test]
    fn tour_placeholders_are_fully_replaced() {
        let src = skeleton("Demo Talk");
        assert!(!src.contains("A Tour of keynot"));
        assert!(!src.contains("The keynot Authors"));
        assert!(!src.contains("date:"));
        // The title lands on the title slide too, not just the metadata.
        assert!(src.contains("# Demo Talk"));
    }

    #[test]
    fn skeleton_code_fence_survives_splitting() {
        let src = skeleton("T");
        let p = Presentation::parse(&src).unwrap();
        let has_rust_block = p.slides.iter().any(|s| {
            s.blocks.iter().any(|b| {
                matches!(
                    b,
                    crate::markdown::Block::CodeBlock { language: Some(l), .. } if l == "rust"
                )
            })
        });
        assert!(has_rust_block);
    }

    #[test]
    fn skeleton_has_speaker_notes() {
        let src = skeleton("T");
        let p = Presentation::parse(&src).unwrap();
        assert!(p.slides.iter().any(|s| !s.notes.is_empty()));
    }

    #[test]
    fn skeleton_theme_resolves() {
        let src = skeleton("T");
        let p = Presentation::parse(&src).unwrap();
        assert!(crate::theme::Theme::from_metadata(&p.metadata).is_ok());
    }
}
