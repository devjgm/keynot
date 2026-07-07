//! Rendering: from the parsed slide AST to styled ratatui text.

mod highlight;
mod slide;
mod wrap;

pub use highlight::Highlighter;
pub(crate) use slide::{RenderContext, RenderedSlide, render_slide};
