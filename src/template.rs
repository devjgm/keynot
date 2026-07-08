//! The skeleton presentation written by `keynot new`: the shipped tour
//! example (embedded at compile time), personalized with the new file's
//! title.

use crate::markdown::Presentation;

/// The tour example, embedded so `keynot new` works from any directory
/// and always matches the documented feature set.
const TOUR: &str = include_str!("../examples/tour.keynot");

/// The starter presentation: the tour with its title (frontmatter and
/// title slide) replaced by `title`, a placeholder author, and the date
/// dropped.
///
/// The values to replace are read from the tour's own parsed metadata
/// rather than hardcoded, so editing the tour cannot silently break
/// this personalization; if a field disappears from the tour its
/// replacement is simply skipped.
pub fn skeleton(title: &str) -> String {
    // Normalize first: a CRLF checkout (Windows) would otherwise defeat
    // the line-exact replacements below.
    let tour = TOUR.replace("\r\n", "\n");
    let metadata = Presentation::parse(&tour)
        .expect("embedded tour example must parse")
        .metadata;

    let mut out = tour;
    if let Some(tour_title) = &metadata.title {
        // Replace everywhere: the frontmatter and the title slide.
        out = out.replace(tour_title, title);
    }
    if let Some(author) = &metadata.author {
        out = out.replacen(&format!("author: {author}\n"), "author: Your Name\n", 1);
    }
    if let Some(date) = &metadata.date {
        out = out.replacen(&format!("date: {date}\n"), "", 1);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skeleton_parses_cleanly() {
        let src = skeleton("Demo Talk");
        let p = Presentation::parse(&src).expect("skeleton must parse");
        assert_eq!(p.metadata.title.as_deref(), Some("Demo Talk"));
        assert_eq!(p.metadata.author.as_deref(), Some("Your Name"));
        assert_eq!(p.metadata.date, None);
        assert!(!p.slides.is_empty());
    }

    #[test]
    fn tour_personalization_is_complete() {
        let tour_metadata = Presentation::parse(TOUR).unwrap().metadata;
        let tour_title = tour_metadata.title.expect("tour has a title");
        let tour_author = tour_metadata.author.expect("tour has an author");

        let src = skeleton("Demo Talk");
        assert!(!src.contains(&tour_title), "tour title must be replaced");
        assert!(!src.contains(&tour_author), "tour author must be replaced");
        assert!(!src.contains("date:"), "tour date must be dropped");
        // The title lands on the title slide too, not just the metadata.
        assert!(src.contains("# Demo Talk"));
    }

    /// The replacements assume the tour's frontmatter fields are plain,
    /// single-line, unquoted scalars; this pins that assumption.
    #[test]
    fn tour_frontmatter_fields_are_plain_scalars() {
        let metadata = Presentation::parse(TOUR).unwrap().metadata;
        for value in [metadata.title, metadata.author, metadata.date] {
            let value = value.expect("tour sets title, author, and date");
            assert!(
                TOUR.contains(&format!(": {value}\n")),
                "field value {value:?} must appear as a plain scalar line"
            );
        }
    }

    #[test]
    fn skeleton_code_fence_survives_splitting() {
        let src = skeleton("T");
        let p = Presentation::parse(&src).unwrap();
        let has_rust_block = p.slides.iter().any(|s| {
            s.columns.iter().flatten().any(|b| {
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
