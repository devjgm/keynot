//! Turns a parsed [`Slide`] into styled ratatui text.

use super::highlight::Highlighter;
use super::wrap::{spans_width, wrap_spans};
use crate::markdown::{Block, InlineSpan, ListBlock, Slide};
use crate::theme::Theme;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};

/// Returns the (columns, rows) an image should occupy, or `None` to
/// render a text placeholder instead.
pub type ImageSizer<'a> = &'a dyn Fn(&str) -> Option<(u16, u16)>;

/// Everything needed to render slides.
pub struct RenderContext<'a> {
    pub theme: &'a Theme,
    pub highlighter: &'a Highlighter,
    /// `None` (e.g. in tests or when the terminal supports no graphics)
    /// disables image layout entirely.
    pub image_sizer: Option<ImageSizer<'a>>,
}

/// Where an image goes within rendered slide text: `height` blank lines
/// are reserved starting at line index `line`, and the player draws the
/// picture over them at horizontal offset `x` (relative to the slide
/// text area; already centered within the image's column).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImagePlacement {
    pub line: usize,
    pub source: String,
    pub x: u16,
    pub width: u16,
    pub height: u16,
}

/// A rendered slide: styled text plus the image areas reserved in it.
#[derive(Debug, Clone)]
pub struct RenderedSlide {
    pub text: Text<'static>,
    pub images: Vec<ImagePlacement>,
}

/// The gap between columns of a multi-column slide, in cells.
const COLUMN_GUTTER: usize = 3;

/// Render a slide as text wrapped to `width` cells. Multi-column slides
/// ([`Slide::columns`]) render each column independently and join them
/// side by side, top-aligned, with equal widths. The caller places (and
/// vertically centers) the returned text and draws the images.
pub fn render_slide(slide: &Slide, ctx: &RenderContext, width: usize) -> RenderedSlide {
    let width = width.max(10);
    let n = slide.columns.len().max(1);
    if n == 1 {
        let empty = Vec::new();
        let blocks = slide.columns.first().unwrap_or(&empty);
        let (lines, images) = render_column(blocks, ctx, width);
        return RenderedSlide {
            text: Text::from(lines),
            images,
        };
    }

    let usable = width.saturating_sub(COLUMN_GUTTER * (n - 1));
    let base_width = (usable / n).max(1);
    let last_width = usable.saturating_sub(base_width * (n - 1)).max(1);

    let mut rendered: Vec<(Vec<Line>, usize)> = Vec::with_capacity(n);
    let mut images = Vec::new();
    let mut x_offset = 0usize;
    for (i, blocks) in slide.columns.iter().enumerate() {
        let col_width = if i == n - 1 { last_width } else { base_width };
        let (lines, mut placements) = render_column(blocks, ctx, col_width);
        for placement in &mut placements {
            placement.x += x_offset as u16;
        }
        images.extend(placements);
        rendered.push((lines, col_width));
        x_offset += col_width + COLUMN_GUTTER;
    }

    // Join row-wise: pad every column's row to its exact width so the
    // next column starts at a fixed x. Per-line styles (e.g. code block
    // backgrounds) are pushed down into the spans, since the joined line
    // can only carry one line-level style.
    let rows = rendered
        .iter()
        .map(|(lines, _)| lines.len())
        .max()
        .unwrap_or(0);
    let mut joined = Vec::with_capacity(rows);
    for row in 0..rows {
        let mut spans: Vec<Span<'static>> = Vec::new();
        for (i, (lines, col_width)) in rendered.iter().enumerate() {
            if i > 0 {
                spans.push(Span::raw(" ".repeat(COLUMN_GUTTER)));
            }
            let mut row_width = 0;
            if let Some(line) = lines.get(row) {
                row_width = line.width();
                let base = line.style;
                spans.extend(
                    line.spans
                        .iter()
                        .map(|s| Span::styled(s.content.clone(), base.patch(s.style))),
                );
            }
            if i < n - 1 {
                let pad = col_width.saturating_sub(row_width);
                if pad > 0 {
                    spans.push(Span::raw(" ".repeat(pad)));
                }
            }
        }
        // Drop trailing padding (rows where the right columns are
        // empty). Styled spans stay: a code panel's background padding
        // is blank text but visible.
        while spans
            .last()
            .is_some_and(|s| s.content.trim().is_empty() && s.style == Style::default())
        {
            spans.pop();
        }
        joined.push(Line::from(spans));
    }
    RenderedSlide {
        text: Text::from(joined),
        images,
    }
}

/// Render one column of blocks at `width`, returning its lines (trailing
/// blanks trimmed, image rows preserved) and image placements.
fn render_column(
    blocks: &[Block],
    ctx: &RenderContext,
    width: usize,
) -> (Vec<Line<'static>>, Vec<ImagePlacement>) {
    let mut lines = Vec::new();
    let mut images = Vec::new();
    render_blocks_spaced(blocks, ctx, width, &mut lines, true, Some(&mut images));
    // Trim trailing blanks, but never into rows reserved for images.
    let reserved = images
        .iter()
        .map(|p: &ImagePlacement| p.line + p.height as usize)
        .max()
        .unwrap_or(0);
    while lines.len() > reserved && lines.last().is_some_and(|l: &Line| l.width() == 0) {
        lines.pop();
    }
    // The renderer reserved blank rows for each image; if a renderer
    // change breaks that contract, fail loudly in debug builds instead
    // of drawing pictures over text.
    #[cfg(debug_assertions)]
    for p in &images {
        for line in lines.iter().skip(p.line).take(p.height as usize) {
            assert_eq!(line.width(), 0, "image rows must be blank");
        }
    }
    (lines, images)
}

/// Drop empty lines from the end.
fn trim_trailing_blanks(lines: &mut Vec<Line<'static>>) {
    while lines.last().is_some_and(|l| l.width() == 0) {
        lines.pop();
    }
}

fn render_blocks(
    blocks: &[Block],
    ctx: &RenderContext,
    width: usize,
    out: &mut Vec<Line<'static>>,
) {
    render_blocks_spaced(blocks, ctx, width, out, true, None);
}

/// Render blocks, optionally separated by blank lines. List items render
/// tight (no separators) so nested lists hug their parent item.
/// `images` is `Some` only at the top level: nested blocks (quotes, list
/// items) shift and re-indent lines, which would invalidate placements,
/// so images there fall back to a text placeholder.
fn render_blocks_spaced(
    blocks: &[Block],
    ctx: &RenderContext,
    width: usize,
    out: &mut Vec<Line<'static>>,
    spaced: bool,
    mut images: Option<&mut Vec<ImagePlacement>>,
) {
    for block in blocks {
        match block {
            Block::Heading { level, content } => render_heading(*level, content, ctx, width, out),
            Block::Paragraph(spans) => {
                out.extend(wrap_spans(convert_inline(spans, ctx.theme), width));
            }
            Block::List(list) => render_list(list, ctx, width, out),
            Block::CodeBlock { language, code } => {
                render_code(language.as_deref(), code, ctx, width, out);
            }
            Block::BlockQuote(inner) => render_quote(inner, ctx, width, out),
            Block::Image { source, alt } => {
                render_image(source, alt, ctx, width, out, images.as_deref_mut());
            }
            Block::Rule => {
                out.push(Line::styled(
                    "-".repeat(width),
                    Style::default().fg(ctx.theme.accent).dim(),
                ));
            }
        }
        if spaced {
            out.push(Line::raw(""));
        }
    }
}

/// Reserve blank lines for an image (recording its placement, centered
/// within `width`), or render a `[image: alt]` placeholder when no size
/// is available.
fn render_image(
    source: &str,
    alt: &str,
    ctx: &RenderContext,
    width: usize,
    out: &mut Vec<Line<'static>>,
    images: Option<&mut Vec<ImagePlacement>>,
) {
    if let Some(images) = images
        && let Some(sizer) = ctx.image_sizer
        && let Some((cols, rows)) = sizer(source)
        && rows > 0
    {
        let cols = cols.min(width as u16).max(1);
        images.push(ImagePlacement {
            line: out.len(),
            source: source.to_string(),
            x: (width as u16 - cols) / 2,
            width: cols,
            height: rows,
        });
        out.extend(std::iter::repeat_n(Line::raw(""), rows as usize));
    } else {
        out.push(Line::styled(
            format!("[image: {alt}]"),
            Style::default().fg(ctx.theme.text).italic().dim(),
        ));
    }
}

fn render_heading(
    level: u8,
    content: &[InlineSpan],
    ctx: &RenderContext,
    width: usize,
    out: &mut Vec<Line<'static>>,
) {
    let theme = ctx.theme;
    let style = match level {
        1 | 2 => Style::default().fg(theme.heading).bold(),
        _ => Style::default().fg(theme.text).bold(),
    };
    let spans: Vec<Span<'static>> = convert_inline(content, theme)
        .into_iter()
        .map(|s| s.patch_style(style))
        .collect();
    let title_width = spans_width(&spans).min(width);
    let mut lines = wrap_spans(spans, width);
    if level == 1 {
        // An underline rule exactly as wide as the title.
        lines.push(Line::styled(
            "-".repeat(title_width.max(1)),
            Style::default().fg(theme.accent).dim(),
        ));
    }
    out.extend(lines);
}

fn render_list(list: &ListBlock, ctx: &RenderContext, width: usize, out: &mut Vec<Line<'static>>) {
    // Right-align ordered markers to the widest number in the list, so
    // item text stays in one column when crossing 9 -> 10.
    let number_width = if list.ordered {
        (list.start + list.items.len() as u64 - 1).to_string().len()
    } else {
        1
    };
    for (i, item) in list.items.iter().enumerate() {
        // Task items show their checkbox as the marker; the dash would be
        // redundant next to it.
        let (marker, marker_style) = match item.task {
            Some(true) => (
                "[x] ".to_string(),
                Style::default().fg(ctx.theme.accent).bold(),
            ),
            Some(false) => (
                "[ ] ".to_string(),
                Style::default().fg(ctx.theme.text).dim(),
            ),
            None if list.ordered => (
                format!("{:>number_width$}. ", list.start + i as u64),
                Style::default().fg(ctx.theme.accent),
            ),
            None => ("- ".to_string(), Style::default().fg(ctx.theme.accent)),
        };
        let indent = marker.len();
        let inner_width = width.saturating_sub(indent).max(1);

        // Render the item's blocks, then attach the marker to the first
        // line and indent the rest (hanging indent).
        let mut item_lines = Vec::new();
        render_blocks_spaced(&item.blocks, ctx, inner_width, &mut item_lines, false, None);
        trim_trailing_blanks(&mut item_lines);
        for (j, line) in item_lines.into_iter().enumerate() {
            let prefix = if j == 0 {
                Span::styled(marker.clone(), marker_style)
            } else {
                Span::raw(" ".repeat(indent))
            };
            out.push(prefix_line(prefix, line));
        }
    }
}

fn render_code(
    language: Option<&str>,
    code: &str,
    ctx: &RenderContext,
    width: usize,
    out: &mut Vec<Line<'static>>,
) {
    let theme = ctx.theme;
    let bg = Style::default().bg(theme.code_background);
    let code_lines = ctx.highlighter.highlight(code, language, &theme.code_theme);

    // Pad the block to a uniform width: the longest line plus one column of
    // padding either side, capped to the available width.
    let content_width = code_lines.iter().map(|l| l.width()).max().unwrap_or(0);
    let box_width = (content_width + 2).min(width).max(4);

    for line in code_lines {
        let line = clip_line(line, box_width - 2);
        let pad = box_width.saturating_sub(line.width() + 1);
        let mut spans = vec![Span::raw(" ")];
        spans.extend(line.spans);
        spans.push(Span::raw(" ".repeat(pad)));
        out.push(Line::from(spans).style(bg));
    }
}

fn render_quote(inner: &[Block], ctx: &RenderContext, width: usize, out: &mut Vec<Line<'static>>) {
    let bar = Span::styled("| ", Style::default().fg(ctx.theme.blockquote));
    let mut lines = Vec::new();
    render_blocks(inner, ctx, width.saturating_sub(2).max(1), &mut lines);
    trim_trailing_blanks(&mut lines);
    for line in lines {
        let styled = Line::from(
            line.spans
                .into_iter()
                .map(|s| s.patch_style(Style::default().add_modifier(Modifier::ITALIC)))
                .collect::<Vec<_>>(),
        );
        out.push(prefix_line(bar.clone(), styled));
    }
}

/// Convert AST inline spans to styled ratatui spans.
pub fn convert_inline(spans: &[InlineSpan], theme: &Theme) -> Vec<Span<'static>> {
    let mut out = Vec::with_capacity(spans.len());
    for (i, span) in spans.iter().enumerate() {
        let mut style = Style::default().fg(theme.text);
        if span.style.code {
            style = style.fg(theme.accent).bg(theme.code_background);
        }
        if span.link.is_some() {
            style = style.fg(theme.link).add_modifier(Modifier::UNDERLINED);
        }
        if span.style.bold {
            style = style.add_modifier(Modifier::BOLD);
        }
        if span.style.italic {
            style = style.add_modifier(Modifier::ITALIC);
        }
        if span.style.strikethrough {
            style = style.add_modifier(Modifier::CROSSED_OUT);
        }
        if span.style.underline {
            style = style.add_modifier(Modifier::UNDERLINED);
        }
        if span.image.is_some() {
            // An image mixed into text: show its label as a placeholder.
            out.push(Span::styled(
                format!("[image: {}]", span.text),
                style.italic().dim(),
            ));
        } else {
            out.push(Span::styled(span.text.clone(), style));
        }

        // Terminals can only open URLs they can see, so show the
        // destination after the last span of each link (unless the link
        // text already is the URL, as in autolinks).
        if let Some(url) = &span.link {
            let last_of_link = spans
                .get(i + 1)
                .is_none_or(|next| next.link.as_deref() != Some(url));
            if last_of_link && span.text != *url {
                out.push(Span::styled(
                    format!(" ({url})"),
                    Style::default().fg(theme.link).dim(),
                ));
            }
        }
    }
    out
}

fn prefix_line(prefix: Span<'static>, line: Line<'static>) -> Line<'static> {
    let mut spans = vec![prefix];
    spans.extend(line.spans);
    Line::from(spans)
}

/// Truncate a line to `width` display columns, dropping what overflows.
fn clip_line(line: Line<'static>, width: usize) -> Line<'static> {
    if line.width() <= width {
        return line;
    }
    let mut spans = Vec::new();
    let mut used = 0;
    for span in line.spans {
        let w = span.width();
        if used + w <= width {
            used += w;
            spans.push(span);
            continue;
        }
        let mut text = String::new();
        for ch in span.content.chars() {
            let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
            if used + cw > width {
                break;
            }
            used += cw;
            text.push(ch);
        }
        if !text.is_empty() {
            spans.push(Span::styled(text, span.style));
        }
        break;
    }
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown::Slide;
    use std::sync::OnceLock;

    fn highlighter() -> &'static Highlighter {
        static H: OnceLock<Highlighter> = OnceLock::new();
        H.get_or_init(Highlighter::new)
    }

    fn render(src: &str, width: usize) -> Text<'static> {
        let theme = Theme::dark();
        let slide = Slide::parse(src);
        let ctx = RenderContext {
            theme: &theme,
            highlighter: highlighter(),
            image_sizer: None,
        };
        render_slide(&slide, &ctx, width).text
    }

    /// Render with a fixed-size image sizer.
    fn render_with_images(src: &str, width: usize, size: (u16, u16)) -> RenderedSlide {
        let theme = Theme::dark();
        let slide = Slide::parse(src);
        let sizer = move |_: &str| Some(size);
        let ctx = RenderContext {
            theme: &theme,
            highlighter: highlighter(),
            image_sizer: Some(&sizer),
        };
        render_slide(&slide, &ctx, width)
    }

    fn strings(text: &Text) -> Vec<String> {
        text.lines.iter().map(|l| l.to_string()).collect()
    }

    #[test]
    fn h1_is_left_aligned_with_underline_rule() {
        let text = render("# Title", 40);
        assert_eq!(text.lines[0].to_string(), "Title");
        assert_eq!(text.lines[0].alignment, None);
        assert_eq!(text.lines[1].to_string(), "-----");
        assert_eq!(text.lines[1].alignment, None);
    }

    #[test]
    fn h1_rule_width_matches_title_exactly() {
        let text = render("# foo", 40);
        assert_eq!(text.lines[0].to_string(), "foo");
        assert_eq!(text.lines[1].to_string(), "---");
        assert_eq!(text.lines[0].width(), text.lines[1].width());
    }

    #[test]
    fn h2_is_left_aligned() {
        let text = render("## Section", 40);
        assert_eq!(text.lines[0].to_string(), "Section");
        assert_eq!(text.lines[0].alignment, None);
    }

    #[test]
    fn paragraph_wraps_to_width() {
        let text = render("one two three four five six", 10);
        assert!(text.lines.len() > 1);
        for line in strings(&text) {
            assert!(line.len() <= 10, "line too long: {line:?}");
        }
    }

    #[test]
    fn blocks_are_separated_by_blank_lines() {
        let text = render("first\n\nsecond", 40);
        assert_eq!(strings(&text), vec!["first", "", "second"]);
    }

    #[test]
    fn unordered_list_uses_dash_markers() {
        let text = render("- alpha\n- beta", 40);
        assert_eq!(strings(&text), vec!["- alpha", "- beta"]);
    }

    #[test]
    fn ordered_list_numbers_from_start() {
        let text = render("4. four\n5. five", 40);
        assert_eq!(strings(&text), vec!["4. four", "5. five"]);
    }

    #[test]
    fn ordered_markers_align_across_width_changes() {
        let text = render("9. nine\n10. ten", 40);
        assert_eq!(strings(&text), vec![" 9. nine", "10. ten"]);
    }

    #[test]
    fn wrapped_list_items_get_hanging_indent() {
        let text = render("- alpha beta gamma delta", 12);
        let lines = strings(&text);
        assert_eq!(lines[0], "- alpha beta");
        assert!(
            lines[1].starts_with("  "),
            "no hanging indent: {:?}",
            lines[1]
        );
    }

    #[test]
    fn nested_lists_are_indented() {
        let text = render("- outer\n  - inner", 40);
        let lines = strings(&text);
        assert_eq!(lines[0], "- outer");
        assert_eq!(lines[1], "  - inner");
    }

    #[test]
    fn code_block_has_uniform_background_width() {
        let text = render("```rust\nlet x = 1;\nlet longer = 22;\n```", 40);
        let lines = &text.lines;
        assert_eq!(lines.len(), 2);
        // Both lines padded to the same width.
        assert_eq!(lines[0].width(), lines[1].width());
        assert!(lines[0].to_string().contains("let x = 1;"));
    }

    #[test]
    fn long_code_lines_are_clipped_not_wrapped() {
        let text = render("```\naaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n```", 12);
        assert_eq!(text.lines.len(), 1);
        assert!(text.lines[0].width() <= 12);
    }

    #[test]
    fn blockquote_gets_bar_prefix_and_italics() {
        let text = render("> wisdom here", 40);
        let line = &text.lines[0];
        assert_eq!(line.to_string(), "| wisdom here");
        assert!(
            line.spans[1..]
                .iter()
                .all(|s| s.style.add_modifier.contains(Modifier::ITALIC))
        );
    }

    #[test]
    fn rule_spans_full_width() {
        let text = render("above\n\n***\n\nbelow", 20);
        assert!(strings(&text).contains(&"-".repeat(20)));
    }

    #[test]
    fn inline_code_gets_accent_and_background() {
        let theme = Theme::dark();
        let slide = Slide::parse("`code`");
        let crate::markdown::Block::Paragraph(spans) = &slide.columns[0][0] else {
            panic!()
        };
        let converted = convert_inline(spans, &theme);
        assert_eq!(converted[0].style.fg, Some(theme.accent));
        assert_eq!(converted[0].style.bg, Some(theme.code_background));
    }

    #[test]
    fn links_are_underlined_in_link_color() {
        let theme = Theme::dark();
        let slide = Slide::parse("[text](https://x.dev)");
        let crate::markdown::Block::Paragraph(spans) = &slide.columns[0][0] else {
            panic!()
        };
        let converted = convert_inline(spans, &theme);
        assert_eq!(converted[0].style.fg, Some(theme.link));
        assert!(
            converted[0]
                .style
                .add_modifier
                .contains(Modifier::UNDERLINED)
        );
    }

    /// Render a multi-column slide built from raw column sources.
    fn render_cols(cols: &[&str], width: usize) -> RenderedSlide {
        let theme = Theme::dark();
        let slide = Slide::parse_columns(&cols.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        let ctx = RenderContext {
            theme: &theme,
            highlighter: highlighter(),
            image_sizer: None,
        };
        render_slide(&slide, &ctx, width)
    }

    #[test]
    fn two_columns_render_side_by_side() {
        // width 23: usable 20, two columns of 10, gutter 3.
        let rendered = render_cols(&["aa", "bb"], 23);
        assert_eq!(
            rendered.text.lines[0].to_string(),
            format!("aa{}bb", " ".repeat(11))
        );
    }

    #[test]
    fn column_text_starts_at_fixed_offsets() {
        // Wrapping happens per column: each column is 10 wide.
        let rendered = render_cols(&["one two three", "x"], 23);
        let lines: Vec<String> = rendered.text.lines.iter().map(|l| l.to_string()).collect();
        assert_eq!(lines[0], format!("one two{}x", " ".repeat(6)));
        assert_eq!(lines[1], "three");
    }

    #[test]
    fn tallest_column_defines_the_row_count() {
        let rendered = render_cols(&["a", "1\n\n2\n\n3"], 23);
        assert_eq!(rendered.text.lines.len(), 5);
        // Short column's missing rows are just absent (left blank).
        assert_eq!(rendered.text.lines[4].to_string().trim(), "3");
    }

    #[test]
    fn three_columns_share_the_width() {
        // width 36: usable 30, three columns of 10.
        let rendered = render_cols(&["a", "b", "c"], 36);
        let row = rendered.text.lines[0].to_string();
        assert_eq!(row, format!("a{}b{}c", " ".repeat(12), " ".repeat(12)));
    }

    #[test]
    fn code_block_background_survives_the_join() {
        let rendered = render_cols(&["left", "```\ncode\n```"], 23);
        let row = &rendered.text.lines[0];
        assert!(row.to_string().contains("code"));
        let code_bg = Theme::dark().code_background;
        assert!(
            row.spans.iter().any(|s| s.style.bg == Some(code_bg)),
            "code panel background must survive joining: {row:?}"
        );
        // The panel's right padding is styled blank space; the join's
        // trailing trim must not eat it.
        let last = row.spans.last().unwrap();
        assert_eq!(last.style.bg, Some(code_bg), "styled padding kept: {row:?}");
    }

    #[test]
    fn image_in_second_column_is_offset() {
        let theme = Theme::dark();
        let slide = Slide::parse_columns(&["left".to_string(), "![c](c.png)".to_string()]);
        let sizer = |_: &str| Some((6u16, 3u16));
        let ctx = RenderContext {
            theme: &theme,
            highlighter: highlighter(),
            image_sizer: Some(&sizer),
        };
        let rendered = render_slide(&slide, &ctx, 23);
        // Column 2 starts at 13; a 6-wide image centered in 10 sits at +2.
        assert_eq!(
            rendered.images,
            vec![ImagePlacement {
                line: 0,
                source: "c.png".to_string(),
                x: 15,
                width: 6,
                height: 3,
            }]
        );
        // Its reserved rows are blank within the column (the join keeps
        // the left column's text on those rows).
        assert!(rendered.text.lines[0].to_string().starts_with("left"));
    }

    #[test]
    fn image_in_third_column_is_offset_past_two_columns() {
        let theme = Theme::dark();
        let slide = Slide::parse_columns(&[
            "one".to_string(),
            "two".to_string(),
            "![c](c.png)".to_string(),
        ]);
        let sizer = |_: &str| Some((4u16, 3u16));
        let ctx = RenderContext {
            theme: &theme,
            highlighter: highlighter(),
            image_sizer: Some(&sizer),
        };
        // width 36: usable 30, three 10-wide columns; column 3 starts at
        // 26, and a 4-wide image centered in 10 sits at +3.
        let rendered = render_slide(&slide, &ctx, 36);
        assert_eq!(rendered.images.len(), 1);
        assert_eq!(rendered.images[0].x, 29, "26 + (10 - 4) / 2");
    }

    #[test]
    fn empty_heading_reserves_a_row_for_column_alignment() {
        // The tour's three-column slide uses a bare `##` so a heading-less
        // column's content aligns with its neighbors' first paragraphs.
        let with_heading = render_cols(&["## Title\n\nbody", "##\n\nbody"], 23);
        let lines: Vec<String> = with_heading
            .text
            .lines
            .iter()
            .map(|l| l.to_string())
            .collect();
        let title_row = lines.iter().position(|l| l.contains("Title")).unwrap();
        let body_row = lines.iter().position(|l| l.contains("body")).unwrap();
        assert!(body_row > title_row, "bodies sit below the heading row");
        // Both columns' bodies share one row.
        assert_eq!(lines[body_row].matches("body").count(), 2, "{lines:?}");
    }

    #[test]
    fn single_column_image_is_centered() {
        let rendered = render_with_images("![c](c.png)", 40, (10, 4));
        assert_eq!(rendered.images[0].x, 15, "(40 - 10) / 2");
    }

    #[test]
    fn image_without_sizer_renders_placeholder() {
        let text = render("![a chart](chart.png)", 40);
        assert_eq!(strings(&text), vec!["[image: a chart]"]);
    }

    #[test]
    fn image_with_sizer_reserves_rows() {
        let rendered = render_with_images("above\n\n![c](c.png)\n\nbelow", 40, (20, 5));
        assert_eq!(
            rendered.images,
            vec![ImagePlacement {
                line: 2,
                source: "c.png".to_string(),
                x: 10,
                width: 20,
                height: 5,
            }]
        );
        let lines = &rendered.text.lines;
        // "above", blank, then 5 reserved blanks, blank, "below".
        assert_eq!(lines[0].to_string(), "above");
        for (i, line) in lines.iter().enumerate().take(7).skip(2) {
            assert_eq!(line.width(), 0, "line {i} should be reserved");
        }
        assert_eq!(lines[8].to_string(), "below");
    }

    #[test]
    fn trailing_image_rows_are_not_trimmed() {
        let rendered = render_with_images("![c](c.png)", 40, (10, 4));
        assert_eq!(rendered.images[0].line, 0);
        assert_eq!(rendered.text.lines.len(), 4);
    }

    #[test]
    fn image_inside_list_renders_placeholder_even_with_sizer() {
        let rendered = render_with_images("- item\n\n  ![c](c.png)\n", 40, (10, 4));
        assert!(rendered.images.is_empty());
        let all = rendered
            .text
            .lines
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(all.contains("[image: c]"), "got: {all}");
    }

    #[test]
    fn task_items_render_checkbox_markers() {
        let text = render("- [x] done\n- [ ] todo\n- plain", 40);
        assert_eq!(strings(&text), vec!["[x] done", "[ ] todo", "- plain"]);
    }

    #[test]
    fn link_url_is_shown_after_the_text() {
        let text = render("see [docs](https://x.dev) now", 60);
        assert_eq!(text.lines[0].to_string(), "see docs (https://x.dev) now");
    }

    #[test]
    fn autolink_url_is_not_repeated() {
        let text = render("go to <https://x.dev> now", 60);
        assert_eq!(text.lines[0].to_string(), "go to https://x.dev now");
    }

    #[test]
    fn comments_do_not_render() {
        let text = render("visible\n\n<!-- hidden -->", 40);
        let all = strings(&text).join("\n");
        assert!(!all.contains("hidden"));
        assert!(all.contains("visible"));
    }

    #[test]
    fn empty_slide_renders_empty() {
        let text = render("", 40);
        assert_eq!(text.lines.len(), 0);
    }

    #[test]
    fn no_trailing_blank_lines() {
        let text = render("# T\n\nbody", 40);
        assert!(text.lines.last().unwrap().width() > 0);
    }
}
