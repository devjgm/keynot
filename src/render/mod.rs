//! Rendering: from the parsed slide AST to styled ratatui text.

mod highlight;
mod slide;
mod wrap;

pub use highlight::Highlighter;
pub(crate) use slide::{ColumnSpan, RenderContext, RenderedSlide, render_slide};
pub(crate) use wrap::split_spans_at;
