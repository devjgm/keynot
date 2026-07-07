//! Rendering: from the parsed slide AST to styled ratatui text.

mod highlight;
mod slide;
mod wrap;

pub use highlight::Highlighter;
pub use slide::{
    ImagePlacement, ImageSizer, RenderContext, RenderedSlide, convert_inline, render_slide,
};
pub use wrap::{spans_width, wrap_spans};
