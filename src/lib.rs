//! keynot: terminal slide presentations from a single markdown file.
//!
//! A `.keynot` file is markdown with optional YAML frontmatter and slides
//! separated by `---` lines (outside of code fences), in the spirit of marp.

pub mod app;
pub mod markdown;
pub mod render;
pub mod template;
pub mod theme;

pub use markdown::{Metadata, ParseError, Presentation, Slide};
pub use theme::Theme;
