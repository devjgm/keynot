//! Turns a parsed [`Slide`] into styled ratatui text.

use super::highlight::Highlighter;
use super::wrap::{spans_width, split_spans_at, wrap_spans};
use crate::markdown::{
    AlertKind, Block, DefinitionItem, Footnote, InlineSpan, ListBlock, Slide, TableAlign,
    TableBlock,
};
use crate::theme::Theme;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use unicode_width::UnicodeWidthStr;

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

/// One column's placement within rendered slide text: its horizontal
/// extent and which rows it can highlight. Single-column slides have
/// exactly one span covering the full width.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnSpan {
    /// Left edge in cells, relative to the slide text area.
    pub x: usize,
    pub width: usize,
    /// Rows (indices into the slide text) where this column has visible
    /// content: the domain of the speaker's line highlight.
    pub non_blank: Vec<usize>,
    /// `(first_row, source_line)` of each top-level block, in row
    /// order, so the player can map a screen row back to the file.
    pub block_rows: Vec<(usize, usize)>,
}

/// A rendered slide: styled text plus the image areas reserved in it
/// and the column geometry the player's highlight cursor navigates.
#[derive(Debug, Clone)]
pub struct RenderedSlide {
    pub text: Text<'static>,
    pub images: Vec<ImagePlacement>,
    pub columns: Vec<ColumnSpan>,
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
    let no_lines = Vec::new();
    if n == 1 {
        let empty = Vec::new();
        let blocks = slide.columns.first().unwrap_or(&empty);
        let block_lines = slide.block_lines.first().unwrap_or(&no_lines);
        let (lines, images, block_rows) = render_column(blocks, block_lines, ctx, width);
        let columns = vec![ColumnSpan {
            x: 0,
            width,
            non_blank: non_blank_rows(&lines),
            block_rows,
        }];
        return RenderedSlide {
            text: Text::from(lines),
            images,
            columns,
        };
    }

    let usable = width.saturating_sub(COLUMN_GUTTER * (n - 1));
    let base_width = (usable / n).max(1);
    let last_width = usable.saturating_sub(base_width * (n - 1)).max(1);

    let mut rendered: Vec<(Vec<Line>, usize)> = Vec::with_capacity(n);
    let mut images = Vec::new();
    let mut columns = Vec::with_capacity(n);
    let mut x_offset = 0usize;
    for (i, blocks) in slide.columns.iter().enumerate() {
        let col_width = if i == n - 1 { last_width } else { base_width };
        let block_lines = slide.block_lines.get(i).unwrap_or(&no_lines);
        let (lines, mut placements, block_rows) =
            render_column(blocks, block_lines, ctx, col_width);
        for placement in &mut placements {
            placement.x += x_offset as u16;
        }
        images.extend(placements);
        columns.push(ColumnSpan {
            x: x_offset,
            width: col_width,
            non_blank: non_blank_rows(&lines),
            block_rows,
        });
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
        columns,
    }
}

/// The tallest slide when rendered `width` cells wide with no terminal
/// (each picture counts as its one-line placeholder): `(rows, index)`
/// with a 1-based slide index. `keynot check` reports this so an
/// author knows what to expect at show time.
pub fn tallest_slide(
    slides: &[Slide],
    theme: &Theme,
    highlighter: &Highlighter,
    width: usize,
) -> (usize, usize) {
    let ctx = RenderContext {
        theme,
        highlighter,
        image_sizer: None,
    };
    slides
        .iter()
        .enumerate()
        .map(|(i, slide)| (render_slide(slide, &ctx, width).text.height(), i + 1))
        // The first slide wins ties, so the report reads naturally.
        .fold((0, 0), |best, cur| if cur.0 > best.0 { cur } else { best })
}

/// Rows where a column has visible content (its trimmed lines are
/// top-aligned, so a row index within the column is a row index in the
/// joined slide text).
fn non_blank_rows(lines: &[Line<'static>]) -> Vec<usize> {
    lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.width() > 0)
        .map(|(i, _)| i)
        .collect()
}

/// Render one column of blocks at `width`, returning its lines (trailing
/// blanks trimmed, image rows preserved), image placements, and each
/// top-level block's `(first_row, source_line)` when lines are known.
fn render_column(
    blocks: &[Block],
    block_lines: &[usize],
    ctx: &RenderContext,
    width: usize,
) -> (Vec<Line<'static>>, Vec<ImagePlacement>, Vec<(usize, usize)>) {
    let mut lines = Vec::new();
    let mut images = Vec::new();
    let mut block_rows = Vec::new();
    for (i, block) in blocks.iter().enumerate() {
        if let Some(line) = block_lines.get(i) {
            block_rows.push((lines.len(), *line));
        }
        render_blocks_spaced(
            std::slice::from_ref(block),
            ctx,
            width,
            &mut lines,
            true,
            Some(&mut images),
        );
    }
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
    (lines, images, block_rows)
}

/// Drop empty lines from the end.
fn trim_trailing_blanks(lines: &mut Vec<Line<'static>>) {
    while lines.last().is_some_and(|l| l.width() == 0) {
        lines.pop();
    }
}

/// Table cells word-wrap to negotiated column widths: each column asks
/// for its widest cell, and when that does not fit the available width,
/// the widest columns shrink (down to a small floor) until it does.
/// Borders match the code windows' rounded style.
fn render_table(
    table: &TableBlock,
    ctx: &RenderContext,
    width: usize,
    out: &mut Vec<Line<'static>>,
) {
    let columns = table
        .header
        .len()
        .max(table.rows.iter().map(Vec::len).max().unwrap_or(0));
    if columns == 0 {
        return;
    }
    let border = Style::default().fg(ctx.theme.code_border);
    // Tables sit on the code panel color, standing out from any
    // background gradient the same way code windows do.
    let bg = Style::default().bg(ctx.theme.code_background);

    // Convert every cell once; header cells render bold.
    let embolden = |spans: Vec<Span<'static>>| -> Vec<Span<'static>> {
        spans
            .into_iter()
            .map(|mut s| {
                s.style = s.style.add_modifier(Modifier::BOLD);
                s
            })
            .collect()
    };
    let convert_row = |cells: &[Vec<InlineSpan>], bold: bool| -> Vec<Vec<Span<'static>>> {
        (0..columns)
            .map(|i| {
                let spans = cells
                    .get(i)
                    .map(|cell| convert_inline(cell, ctx.theme))
                    .unwrap_or_default();
                if bold { embolden(spans) } else { spans }
            })
            .collect()
    };
    let header = convert_row(&table.header, true);
    let body: Vec<Vec<Vec<Span<'static>>>> = table
        .rows
        .iter()
        .map(|row| convert_row(row, false))
        .collect();

    // Column widths: natural, then shrink the widest until it fits.
    const MIN_COLUMN: usize = 4;
    let mut widths: Vec<usize> = (0..columns)
        .map(|i| {
            std::iter::once(&header)
                .chain(body.iter())
                .map(|row| spans_width(&row[i]))
                .max()
                .unwrap_or(1)
                .max(1)
        })
        .collect();
    let overhead = 3 * columns + 1;
    let available = width.saturating_sub(overhead).max(columns * MIN_COLUMN);
    while widths.iter().sum::<usize>() > available {
        let (widest, width) = widths
            .iter()
            .copied()
            .enumerate()
            .max_by_key(|&(_, w)| w)
            .expect("tables have at least one column");
        if width <= MIN_COLUMN {
            break;
        }
        widths[widest] -= 1;
    }

    let edge = |left: char, mid: char, right: char| -> Line<'static> {
        let mut spans = vec![Span::styled(left.to_string(), border)];
        for (i, w) in widths.iter().enumerate() {
            if i > 0 {
                spans.push(Span::styled(mid.to_string(), border));
            }
            spans.push(Span::styled("\u{2500}".repeat(w + 2), border));
        }
        spans.push(Span::styled(right.to_string(), border));
        Line::from(spans)
    };

    // Emit one row: wrap each cell to its width; the tallest cell sets
    // the row height and shorter cells pad with blank lines.
    let emit_row = |cells: &[Vec<Span<'static>>], out: &mut Vec<Line<'static>>| {
        let wrapped: Vec<Vec<Line<'static>>> = cells
            .iter()
            .zip(&widths)
            .map(|(cell, w)| wrap_spans(cell.clone(), *w))
            .collect();
        let rows = wrapped.iter().map(Vec::len).max().unwrap_or(1);
        for line_index in 0..rows {
            let mut spans = vec![Span::styled("\u{2502}", border), Span::raw(" ")];
            for (i, cell_lines) in wrapped.iter().enumerate() {
                if i > 0 {
                    spans.push(Span::raw(" "));
                    spans.push(Span::styled("\u{2502}", border));
                    spans.push(Span::raw(" "));
                }
                let (content, used) = match cell_lines.get(line_index) {
                    Some(line) => (line.spans.clone(), line.width()),
                    None => (Vec::new(), 0),
                };
                let pad = widths[i].saturating_sub(used);
                let (before, after) = match table.alignments.get(i).copied().unwrap_or_default() {
                    TableAlign::Left => (0, pad),
                    TableAlign::Right => (pad, 0),
                    TableAlign::Center => (pad / 2, pad - pad / 2),
                };
                if before > 0 {
                    spans.push(Span::raw(" ".repeat(before)));
                }
                spans.extend(content);
                if after > 0 {
                    spans.push(Span::raw(" ".repeat(after)));
                }
            }
            spans.push(Span::raw(" "));
            spans.push(Span::styled("\u{2502}", border));
            out.push(clip_line(Line::from(spans).style(bg), width));
        }
    };

    out.push(clip_line(
        edge('\u{256d}', '\u{252c}', '\u{256e}').style(bg),
        width,
    ));
    emit_row(&header, out);
    out.push(clip_line(
        edge('\u{251c}', '\u{253c}', '\u{2524}').style(bg),
        width,
    ));
    for row in &body {
        emit_row(row, out);
    }
    out.push(clip_line(
        edge('\u{2570}', '\u{2534}', '\u{256f}').style(bg),
        width,
    ));
}

/// Definition lists: bold terms with their definitions indented below.
fn render_definitions(
    items: &[DefinitionItem],
    ctx: &RenderContext,
    width: usize,
    out: &mut Vec<Line<'static>>,
) {
    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            out.push(Line::raw(""));
        }
        let term: Vec<Span<'static>> = convert_inline(&item.term, ctx.theme)
            .into_iter()
            .map(|mut s| {
                s.style = s.style.add_modifier(Modifier::BOLD);
                s
            })
            .collect();
        out.extend(wrap_spans(term, width));
        for definition in &item.definitions {
            let mut lines = Vec::new();
            render_blocks_spaced(
                definition,
                ctx,
                width.saturating_sub(2).max(1),
                &mut lines,
                true,
                None,
            );
            trim_trailing_blanks(&mut lines);
            for line in lines {
                out.push(prefix_line(Span::raw("  "), line));
            }
        }
    }
}

/// The footnote section a column ends with when it referenced any:
/// a short dim rule, then each note behind its accent `[n]` marker.
fn render_footnotes(
    footnotes: &[Footnote],
    ctx: &RenderContext,
    width: usize,
    out: &mut Vec<Line<'static>>,
) {
    out.push(Line::styled(
        "\u{2500}".repeat(width.min(12)),
        Style::default().fg(ctx.theme.code_border),
    ));
    for footnote in footnotes {
        let marker = format!("[{}] ", footnote.number);
        let indent = marker.width();
        let mut lines = Vec::new();
        render_blocks_spaced(
            &footnote.blocks,
            ctx,
            width.saturating_sub(indent).max(1),
            &mut lines,
            true,
            None,
        );
        trim_trailing_blanks(&mut lines);
        for (i, line) in lines.into_iter().enumerate() {
            let prefix = if i == 0 {
                Span::styled(marker.clone(), Style::default().fg(ctx.theme.accent))
            } else {
                Span::raw(" ".repeat(indent))
            };
            out.push(prefix_line(prefix, line));
        }
    }
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
            Block::BlockQuote { kind, blocks } => render_quote(*kind, blocks, ctx, width, out),
            Block::Image { source, alt } => {
                render_image(source, alt, ctx, width, out, images.as_deref_mut());
            }
            Block::Table(table) => render_table(table, ctx, width, out),
            Block::DefinitionList(items) => render_definitions(items, ctx, width, out),
            Block::Footnotes(footnotes) => render_footnotes(footnotes, ctx, width, out),
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

/// Code blocks render, by default, as a little framed window: a
/// rounded border with the language named in the bottom edge, on a
/// panel darker than any background gradient. `code_style: plain`
/// keeps just the panel.
fn render_code(
    language: Option<&str>,
    code: &str,
    ctx: &RenderContext,
    width: usize,
    out: &mut Vec<Line<'static>>,
) {
    let theme = ctx.theme;
    let bg = Style::default().bg(theme.code_background);
    let border = Style::default()
        .fg(theme.code_border)
        .bg(theme.code_background);
    let code_lines = ctx.highlighter.highlight(code, language, &theme.code_theme);

    if theme.code_style == crate::markdown::CodeStyle::Plain {
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
        return;
    }

    // Pad the block to a uniform width: the longest line plus the
    // border and one column of padding either side, capped to the
    // available width.
    let content_width = code_lines.iter().map(|l| l.width()).max().unwrap_or(0);
    let box_width = (content_width + 4).min(width).max(12);
    let inner = box_width - 2;

    let top = vec![
        Span::styled("╭", border),
        Span::styled("\u{2500}".repeat(box_width.saturating_sub(2)), border),
        Span::styled("╮", border),
    ];
    out.push(Line::from(top));

    for line in code_lines {
        let line = clip_line(line, inner - 2);
        let pad = inner.saturating_sub(line.width() + 1);
        let mut spans = vec![Span::styled("\u{2502}", border), Span::raw(" ")];
        spans.extend(line.spans);
        spans.push(Span::raw(" ".repeat(pad.max(1))));
        spans.push(Span::styled("\u{2502}", border));
        out.push(Line::from(spans).style(bg));
    }

    // Bottom edge, with the language as a label when known.
    let mut bottom = vec![Span::styled("╰", border)];
    match language {
        Some(lang) if lang.width() + 6 <= inner => {
            bottom.push(Span::styled(
                "\u{2500}".repeat(inner - lang.width() - 4),
                border,
            ));
            bottom.push(Span::styled(format!(" {lang} "), border));
            bottom.push(Span::styled("\u{2500}".repeat(2), border));
        }
        _ => bottom.push(Span::styled("\u{2500}".repeat(inner), border)),
    }
    bottom.push(Span::styled("╯", border));
    out.push(Line::from(bottom));
}

/// Plain quotes: italic behind a `|` bar. GFM alerts (`> [!NOTE]`)
/// get a colored bar and a bold label line instead, and their body
/// stays upright -- callouts are information, not quotation.
fn render_quote(
    kind: Option<AlertKind>,
    inner: &[Block],
    ctx: &RenderContext,
    width: usize,
    out: &mut Vec<Line<'static>>,
) {
    let color = match kind {
        None => ctx.theme.blockquote,
        Some(AlertKind::Note) => ctx.theme.link,
        Some(AlertKind::Tip) => ctx.theme.blockquote,
        Some(AlertKind::Important) => ctx.theme.heading,
        Some(AlertKind::Warning) => ctx.theme.accent,
        // No theme slot means "danger"; the terminal's red does.
        Some(AlertKind::Caution) => Color::LightRed,
    };
    let bar = Span::styled("| ", Style::default().fg(color));
    if let Some(kind) = kind {
        out.push(Line::from(vec![
            bar.clone(),
            Span::styled(
                kind.label().to_string(),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
        ]));
    }
    let mut lines = Vec::new();
    render_blocks_spaced(
        inner,
        ctx,
        width.saturating_sub(2).max(1),
        &mut lines,
        true,
        None,
    );
    trim_trailing_blanks(&mut lines);
    for line in lines {
        let styled = if kind.is_some() {
            line
        } else {
            Line::from(
                line.spans
                    .into_iter()
                    .map(|s| s.patch_style(Style::default().add_modifier(Modifier::ITALIC)))
                    .collect::<Vec<_>>(),
            )
        };
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
        if span.style.footnote_ref {
            style = style.fg(theme.accent);
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
    let style = line.style;
    let mut clipped = Line::from(split_spans_at(line.spans, width).0);
    clipped.style = style;
    clipped
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
        // Title bar, two code lines, bottom edge.
        assert_eq!(lines.len(), 4);
        // Every row of the window is the same width.
        assert_eq!(lines[0].width(), lines[1].width());
        assert_eq!(lines[1].width(), lines[2].width());
        assert_eq!(lines[2].width(), lines[3].width());
        assert!(lines[1].to_string().contains("let x = 1;"));
    }

    #[test]
    fn code_block_is_a_terminal_window() {
        let text = render("```rust\nlet x = 1;\n```", 40);
        let top = text.lines[0].to_string();
        let bottom = text.lines[2].to_string();
        assert!(
            top.starts_with('\u{256d}') && top.ends_with('\u{256e}'),
            "{top:?}"
        );
        assert!(
            top.chars()
                .skip(1)
                .take(top.chars().count().saturating_sub(2))
                .all(|c| c == '\u{2500}'),
            "a plain top edge, no adornments: {top:?}"
        );
        assert!(bottom.contains(" rust "), "language label: {bottom:?}");
        assert!(
            bottom.starts_with('\u{2570}') && bottom.ends_with('\u{256f}'),
            "{bottom:?}"
        );
        // Side borders on the code row.
        let mid = text.lines[1].to_string();
        assert!(
            mid.starts_with('\u{2502}') && mid.ends_with('\u{2502}'),
            "{mid:?}"
        );
    }

    #[test]
    fn plain_code_style_has_no_frame() {
        let theme = Theme {
            code_style: crate::markdown::CodeStyle::Plain,
            ..Theme::dark()
        };
        let slide = Slide::parse("```rust\nlet x = 1;\n```");
        let ctx = RenderContext {
            theme: &theme,
            highlighter: highlighter(),
            image_sizer: None,
        };
        let rendered = render_slide(&slide, &ctx, 40);
        assert_eq!(rendered.text.lines.len(), 1, "just the code line");
        let row = rendered.text.lines[0].to_string();
        assert!(row.contains("let x = 1;"));
        assert!(
            !row.contains('\u{256d}') && !row.contains('\u{25cf}'),
            "{row:?}"
        );
    }

    #[test]
    fn code_block_without_language_has_no_label() {
        let text = render("```\nplain\n```", 40);
        let bottom = text.lines[2].to_string();
        assert!(!bottom.contains(char::is_alphabetic), "{bottom:?}");
    }

    #[test]
    fn long_code_lines_are_clipped_not_wrapped() {
        let text = render("```\naaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n```", 12);
        // Title bar, one clipped code line, bottom edge.
        assert_eq!(text.lines.len(), 3);
        assert!(text.lines.iter().all(|l| l.width() <= 12));
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
        // Row 0 is the window's title bar; row 1 holds the code.
        let row = &rendered.text.lines[1];
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
    fn single_column_span_covers_the_full_width() {
        let rendered = render_cols(&["hello\n\nworld"], 40);
        assert_eq!(rendered.columns.len(), 1);
        let span = &rendered.columns[0];
        assert_eq!((span.x, span.width), (0, 40));
        assert_eq!(span.non_blank, vec![0, 2]);
    }

    #[test]
    fn column_spans_carry_offsets_and_rows() {
        // width 23: two 10-wide columns, gutter 3.
        let rendered = render_cols(&["a1\n\na2", "b1"], 23);
        let spans = &rendered.columns;
        assert_eq!(spans.len(), 2);
        assert_eq!((spans[0].x, spans[0].width), (0, 10));
        assert_eq!((spans[1].x, spans[1].width), (13, 10));
        assert_eq!(spans[0].non_blank, vec![0, 2]);
        assert_eq!(spans[1].non_blank, vec![0]);
    }

    #[test]
    fn image_only_column_has_no_highlightable_rows() {
        let theme = Theme::dark();
        let slide = Slide::parse_columns(&["text".to_string(), "![c](c.png)".to_string()]);
        let sizer = |_: &str| Some((4u16, 3u16));
        let ctx = RenderContext {
            theme: &theme,
            highlighter: highlighter(),
            image_sizer: Some(&sizer),
        };
        let rendered = render_slide(&slide, &ctx, 23);
        assert!(rendered.columns[1].non_blank.is_empty());
        assert!(!rendered.columns[0].non_blank.is_empty());
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
    fn definition_lists_render_bold_terms_and_indented_defs() {
        let text = render("Term\n: the meaning\n", 40);
        let rows: Vec<String> = text.lines.iter().map(|l| l.to_string()).collect();
        assert_eq!(rows, vec!["Term", "  the meaning"]);
        let term = &text.lines[0].spans[0];
        assert!(term.style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn footnotes_render_markers_and_a_trailing_section() {
        let text = render("Fact[^1].\n\n[^1]: source here\n", 40);
        let rows: Vec<String> = text.lines.iter().map(|l| l.to_string()).collect();
        assert!(rows[0].contains("Fact[1]."), "{rows:?}");
        assert!(rows.last().unwrap().contains("[1] source here"));
        // The marker and section label take the accent color.
        let accent = Theme::dark().accent;
        let marker_span = text.lines[0]
            .spans
            .iter()
            .find(|s| s.content.contains("[1]"))
            .unwrap();
        assert_eq!(marker_span.style.fg, Some(accent));
    }

    #[test]
    fn alerts_render_a_labeled_colored_bar() {
        let text = render("> [!NOTE]\n> useful context\n", 40);
        let rows: Vec<String> = text.lines.iter().map(|l| l.to_string()).collect();
        assert_eq!(rows[0], "| Note");
        assert_eq!(rows[1], "| useful context");
        let label = text.lines[0]
            .spans
            .iter()
            .find(|s| s.content.contains("Note"))
            .unwrap();
        assert_eq!(label.style.fg, Some(Theme::dark().link), "Note is blue");
        assert!(label.style.add_modifier.contains(Modifier::BOLD));
        // Alert bodies stay upright, unlike quotes.
        let body = text.lines[1]
            .spans
            .iter()
            .find(|s| s.content.contains("useful"))
            .unwrap();
        assert!(!body.style.add_modifier.contains(Modifier::ITALIC));
    }

    #[test]
    fn plain_quotes_stay_italic_and_unlabeled() {
        let text = render("> wisdom\n", 40);
        assert_eq!(text.lines[0].to_string(), "| wisdom");
    }

    #[test]
    fn table_renders_rounded_borders_and_alignment() {
        let text = render("| name | n |\n|:-----|--:|\n| ada | 3 |\n| bo | 14 |\n", 40);
        let rows: Vec<String> = text.lines.iter().map(|l| l.to_string()).collect();
        assert_eq!(
            rows,
            vec![
                "\u{256d}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{252c}\u{2500}\u{2500}\u{2500}\u{2500}\u{256e}",
                "\u{2502} name \u{2502}  n \u{2502}",
                "\u{251c}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{253c}\u{2500}\u{2500}\u{2500}\u{2500}\u{2524}",
                "\u{2502} ada  \u{2502}  3 \u{2502}",
                "\u{2502} bo   \u{2502} 14 \u{2502}",
                "\u{2570}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2534}\u{2500}\u{2500}\u{2500}\u{2500}\u{256f}",
            ]
        );
    }

    #[test]
    fn tables_sit_on_the_code_panel_background() {
        let text = render("| h |\n|---|\n| b |\n", 40);
        let code_bg = Theme::dark().code_background;
        assert!(
            text.lines.iter().all(|l| l.style.bg == Some(code_bg)),
            "every table row carries the panel background"
        );
    }

    #[test]
    fn table_header_cells_are_bold() {
        let text = render("| head |\n|------|\n| body |\n", 40);
        let header_row = &text.lines[1];
        let head_span = header_row
            .spans
            .iter()
            .find(|s| s.content.contains("head"))
            .unwrap();
        assert!(head_span.style.add_modifier.contains(Modifier::BOLD));
        let body_row = &text.lines[3];
        let body_span = body_row
            .spans
            .iter()
            .find(|s| s.content.contains("body"))
            .unwrap();
        assert!(!body_span.style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn wide_tables_wrap_cells_to_fit() {
        let text = render(
            "| a | words |\n|---|-------|\n| x | one two three four five |\n",
            22,
        );
        let rows: Vec<String> = text.lines.iter().map(|l| l.to_string()).collect();
        // Every line fits, and the long cell wrapped onto extra lines
        // while its short neighbor padded with blanks.
        assert!(rows.iter().all(|r| r.chars().count() <= 22), "{rows:?}");
        assert!(rows.len() > 5, "wrapped body rows: {rows:?}");
        let x_row = rows.iter().position(|r| r.contains('x')).unwrap();
        assert!(rows[x_row].contains("one"));
        assert!(rows[x_row + 1].contains('\u{2502}'), "continuation row");
    }

    #[test]
    fn tables_render_inside_columns() {
        let rendered = render_cols(&["left", "| h |\n|---|\n| b |\n"], 23);
        let all: String = rendered
            .text
            .lines
            .iter()
            .map(|l| l.to_string() + "\n")
            .collect();
        assert!(all.contains('\u{256d}'), "table border in column 2:\n{all}");
        assert!(all.contains("left"));
        // The table's rows are highlightable like any content.
        assert!(!rendered.columns[1].non_blank.is_empty());
    }

    #[test]
    fn degenerate_width_tables_clip_but_never_overflow() {
        let text = render("| aaaa | bbbb | cccc |\n|---|---|---|\n| 1 | 2 | 3 |\n", 10);
        assert!(text.lines.iter().all(|l| l.width() <= 10));
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
