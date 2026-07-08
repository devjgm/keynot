//! The interactive presentation player: event loop, input, and drawing.

mod config;
mod images;

use crate::markdown::GradientDirection;
use crate::markdown::{Block as MdBlock, HighlightStyle, Presentation, Slide, Transition};
use crate::render::{
    ColumnSpan, Highlighter, RenderContext, RenderedSlide, render_slide, split_spans_at,
};
use crate::theme::{Background, Theme};
use config::TransitionEffects;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use eyre::{Result, WrapErr, bail};
use images::Images;
use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::symbols::border;
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Clear, Paragraph};
use ratatui_image::StatefulImage;
use ratatui_image::picker::Picker;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tachyonfx::{Effect, EffectRenderer};
use unicode_width::UnicodeWidthStr;

/// Options for `keynot play`.
#[derive(Debug, Clone, Copy, Default)]
pub struct PlayOptions {
    /// 1-based slide to start on (0 or 1 both mean the first slide).
    pub start_slide: usize,
    /// How to draw images.
    pub images: ImageMode,
}

/// How the player draws images, from `play --images`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum ImageMode {
    /// The best protocol the terminal supports.
    #[default]
    Auto,
    /// Textual half-block cells only. Unlike kitty/iTerm2/sixel
    /// graphics, these survive asciinema recordings and GIF renders.
    Halfblocks,
    /// Placeholders instead of pictures.
    Off,
}

/// Everything `load` validates and resolves for a presentation file.
pub struct LoadedPresentation {
    pub presentation: Presentation,
    pub theme: Theme,
}

/// Load and validate a presentation file: parse (which also validates
/// transition and highlight values), resolve the theme, and check the
/// code theme name. Used by `play` and `check`.
pub fn load(path: &Path, highlighter: &Highlighter) -> Result<LoadedPresentation> {
    let source =
        fs_err::read_to_string(path).wrap_err_with(|| format!("cannot read {}", path.display()))?;
    let presentation = Presentation::parse(&source)
        .wrap_err_with(|| format!("cannot parse {}", path.display()))?;
    let theme = Theme::from_metadata(&presentation.metadata)?;
    if !highlighter.has_theme(&theme.code_theme) {
        bail!(
            "unknown code_theme `{}` (available: {})",
            theme.code_theme,
            highlighter.available_themes().join(", ")
        );
    }
    tracing::info!(
        path = %path.display(),
        slides = presentation.slides.len(),
        theme = presentation.metadata.theme.as_deref().unwrap_or("dark"),
        transition = ?presentation.metadata.transition,
        code_theme = theme.code_theme,
        "loaded presentation"
    );
    Ok(LoadedPresentation {
        presentation,
        theme,
    })
}

/// Play a presentation in the terminal.
pub fn play(path: &Path, options: PlayOptions) -> Result<()> {
    let highlighter = Highlighter::new();
    let loaded = load(path, &highlighter)?;

    // Fetch and decode images before entering the TUI, so a slow network
    // delays startup at the shell prompt instead of freezing a blank
    // alternate screen.
    let base = path.parent().map(Path::to_path_buf).unwrap_or_default();
    let decoded = images::decode_all(&loaded.presentation.slides, &base);

    let mut terminal = ratatui::init();
    // Probe the terminal for its graphics protocol and font size. This
    // should run after terminal init; ratatui-image manages raw mode for
    // its own stdio query round-trip.
    let picker = match options.images {
        ImageMode::Off => None,
        ImageMode::Auto => probe_graphics(),
        ImageMode::Halfblocks => probe_graphics().map(|mut picker| {
            picker.set_protocol_type(ratatui_image::picker::ProtocolType::Halfblocks);
            picker
        }),
    };
    let mut app = App::new(
        path.to_path_buf(),
        loaded,
        highlighter,
        picker,
        decoded,
        options,
    );
    let result = app.run(&mut terminal);
    // ratatui::restore() does not unhide the cursor that draw() hides.
    let _ = terminal.show_cursor();
    ratatui::restore();
    result
}

/// Probe the terminal for graphics support, logging the outcome (the
/// probe result is invisible on screen and a frequent support question).
fn probe_graphics() -> Option<Picker> {
    match Picker::from_query_stdio() {
        Ok(picker) => {
            tracing::info!(
                protocol = ?picker.protocol_type(),
                font_size = ?picker.font_size(),
                "graphics probe"
            );
            Some(picker)
        }
        Err(err) => {
            tracing::warn!(%err, "graphics probe failed; images will not draw");
            None
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Slides,
    Outline,
}

/// One slide rendered for a specific size, so idle and animation frames
/// skip re-rendering (and especially re-highlighting) unchanged slides.
/// Cache hits draw `paragraph` by reference, allocation-free.
struct RenderCache {
    index: usize,
    width: u16,
    height: u16,
    rendered: RenderedSlide,
    paragraph: Paragraph<'static>,
}

/// Style the line at row `target`, within `column`'s horizontal extent
/// only, per the `highlight:` metadata key: an accent bar behind it, or
/// dimming everything else. Single-column slides pass a span covering
/// the full width.
fn style_highlight(
    theme: &Theme,
    style: HighlightStyle,
    text: &mut Text<'static>,
    column: &ColumnSpan,
    target: usize,
) {
    match style {
        HighlightStyle::Dim => {
            for (i, line) in text.lines.iter_mut().enumerate() {
                if i != target {
                    line.style = line.style.add_modifier(Modifier::DIM);
                    continue;
                }
                // On the target row, the other columns' segments dim too.
                let spans = std::mem::take(&mut line.spans);
                let (before, rest) = split_spans_at(spans, column.x);
                let (mid, after) = split_spans_at(rest, column.width);
                let dim = |mut s: Span<'static>| {
                    s.style = s.style.add_modifier(Modifier::DIM);
                    s
                };
                let mut joined: Vec<Span> = before.into_iter().map(dim).collect();
                joined.extend(mid);
                joined.extend(after.into_iter().map(dim));
                line.spans = joined;
            }
        }
        HighlightStyle::Bar => {
            // Repaint the column's segment of the row onto an accent bar
            // exactly the column's width. The bar owns the colors (fg
            // becomes the background color, since arbitrary foregrounds
            // like syntax colors are unreadable on the accent);
            // bold/italic and such survive.
            let bar = Style::default()
                .fg(theme.background.base())
                .bg(theme.accent);
            let line = &mut text.lines[target];
            let base = line.style;
            let spans = std::mem::take(&mut line.spans);
            let (before, rest) = split_spans_at(spans, column.x);
            let (mid, after) = split_spans_at(rest, column.width);

            let mut joined = before;
            let lead: usize = joined.iter().map(|s| s.content.width()).sum();
            if lead < column.x {
                // The row may end before this column starts; bridge it.
                joined.push(Span::raw(" ".repeat(column.x - lead)));
            }
            let mut bar_width = 0;
            for mut span in mid {
                bar_width += span.content.width();
                span.style = base.patch(span.style).patch(bar);
                joined.push(span);
            }
            if bar_width < column.width {
                joined.push(Span::styled(" ".repeat(column.width - bar_width), bar));
            }
            joined.extend(after);
            line.spans = joined;
        }
    }
}

struct App {
    path: PathBuf,
    presentation: Presentation,
    theme: Theme,
    highlighter: Highlighter,
    images: Images,
    render_cache: Option<RenderCache>,
    current: usize,
    mode: Mode,
    /// Outline cursor (slide index).
    selected: usize,
    /// Slide number being typed in the outline (1-based, as displayed).
    outline_input: Option<usize>,
    help: bool,
    /// Speaker's line highlight: (column, index into that column's
    /// non-blank rendered lines), styled per the `highlight:` key.
    highlight: Option<(usize, usize)>,
    /// Fresh image decodes from a reload, arriving from a worker thread
    /// (network fetches must never block the draw loop).
    pending_images: Option<std::sync::mpsc::Receiver<HashMap<String, images::Decoded>>>,
    /// Set by the `!` key: drop to an interactive shell on the next tick.
    shell_requested: bool,
    /// Effect running on the incoming (current) slide.
    effect: Option<Effect>,
    /// Exit animation still playing on the slide being navigated away
    /// from: its index and the effect.
    outgoing: Option<(usize, Effect)>,
    /// Enter effect queued to start once `outgoing` finishes.
    pending_enter: Option<Effect>,
    error: Option<String>,
    last_frame: Instant,
}

impl App {
    fn new(
        path: PathBuf,
        loaded: LoadedPresentation,
        highlighter: Highlighter,
        picker: Option<Picker>,
        decoded: HashMap<String, images::Decoded>,
        options: PlayOptions,
    ) -> Self {
        // Upheld by Presentation::parse (NoSlides); asserted here because
        // a Presentation can also be constructed directly.
        assert!(
            !loaded.presentation.slides.is_empty(),
            "a presentation must have at least one slide"
        );
        let last = loaded.presentation.slides.len() - 1;
        let current = options.start_slide.saturating_sub(1).min(last);
        let base = path.parent().map(Path::to_path_buf).unwrap_or_default();
        let mut images = Images::new(picker, base);
        images.adopt(decoded);
        let error = images.take_errors().into_iter().next();
        App {
            path,
            presentation: loaded.presentation,
            theme: loaded.theme,
            highlighter,
            images,
            render_cache: None,
            current,
            mode: Mode::Slides,
            selected: current,
            outline_input: None,
            help: false,
            highlight: None,
            pending_images: None,
            shell_requested: false,
            effect: None,
            outgoing: None,
            pending_enter: None,
            error,
            last_frame: Instant::now(),
        }
    }

    fn run(&mut self, terminal: &mut ratatui::DefaultTerminal) -> Result<()> {
        loop {
            self.adopt_pending_images();
            let elapsed = self.last_frame.elapsed();
            self.last_frame = Instant::now();
            terminal.draw(|frame| self.draw(frame, elapsed))?;
            let timeout = if self.effect.is_some() || self.outgoing.is_some() {
                Duration::from_millis(16)
            } else if self.pending_images.is_some() {
                Duration::from_millis(100)
            } else {
                Duration::from_millis(500)
            };
            if event::poll(timeout)?
                && let Event::Key(key) = event::read()?
            {
                if key.kind == KeyEventKind::Release {
                    continue;
                }
                if self.handle_key(key) {
                    return Ok(());
                }
                if self.shell_requested {
                    self.shell_requested = false;
                    if let Err(err) = self.run_shell(terminal) {
                        self.error = Some(format!("shell failed: {err:#}"));
                    }
                }
            }
        }
    }

    /// Suspend the presentation and hand the terminal to an interactive
    /// shell; when it exits, take the terminal back and redraw where we
    /// left off.
    fn run_shell(&mut self, terminal: &mut ratatui::DefaultTerminal) -> Result<()> {
        // Drawing hides the cursor each frame, and cursor visibility is
        // global terminal state that ratatui::restore() does not touch;
        // show it again or the shell gets an invisible cursor.
        let _ = terminal.show_cursor();
        ratatui::restore();
        println!(
            "keynot: paused at slide {}/{}; exit the shell to resume",
            self.current + 1,
            self.presentation.slides.len()
        );
        let shell = default_shell();
        tracing::info!(shell, "suspending for interactive shell");
        let status = std::process::Command::new(&shell).status();

        // Re-enter the TUI before propagating any error, so a missing
        // shell does not leave the terminal in cooked mode.
        *terminal = ratatui::init();
        terminal.clear()?;
        self.last_frame = Instant::now();
        let status = status.wrap_err_with(|| format!("cannot run {shell}"))?;
        tracing::info!(%status, "shell exited; resuming");
        Ok(())
    }

    // --- input ---

    /// Handle a key press; returns true to quit.
    fn handle_key(&mut self, key: KeyEvent) -> bool {
        tracing::trace!(?key, mode = ?self.mode, "key");
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return true;
        }
        if self.help {
            self.help = false;
            return false;
        }
        match key.code {
            KeyCode::Char('q') => return true,
            KeyCode::Char('?') => self.help = true,
            KeyCode::Char('r') => self.reload(),
            KeyCode::Char('!') => self.shell_requested = true,
            _ => match self.mode {
                Mode::Slides => self.slides_key(key.code),
                Mode::Outline => return self.outline_key(key.code),
            },
        }
        false
    }

    fn slides_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Right | KeyCode::Char('l') => self.highlight_sideways(1),
            KeyCode::Left | KeyCode::Char('h') => self.highlight_sideways(-1),
            KeyCode::PageDown | KeyCode::Enter | KeyCode::Char(' ') | KeyCode::Char('n') => {
                self.goto(self.current + 1)
            }
            KeyCode::PageUp | KeyCode::Backspace | KeyCode::Char('p') => {
                self.goto(self.current.saturating_sub(1))
            }
            KeyCode::Down | KeyCode::Char('j') => self.highlight_move(1),
            KeyCode::Up | KeyCode::Char('k') => self.highlight_move(-1),
            KeyCode::Home | KeyCode::Char('g') => self.goto(0),
            KeyCode::End | KeyCode::Char('G') => self.goto(usize::MAX),
            KeyCode::Esc => self.highlight = None,
            KeyCode::Char('o') => {
                self.selected = self.current;
                self.mode = Mode::Outline;
            }
            _ => {}
        }
    }

    /// The current slide's column geometry, straight from the render
    /// cache (empty before the first draw), so the highlight keys and
    /// the drawn frame can never disagree.
    fn columns(&self) -> &[ColumnSpan] {
        self.render_cache
            .as_ref()
            .map(|c| c.rendered.columns.as_slice())
            .unwrap_or(&[])
    }

    /// Move the line highlight down (`+1`) or up (`-1`) within its
    /// column. Starting fresh, down highlights the first line of the
    /// first (highlightable) column, up its last line.
    fn highlight_move(&mut self, delta: isize) {
        match self.highlight {
            Some((col, pos)) => {
                let Some(span) = self.columns().get(col) else {
                    return;
                };
                let count = span.non_blank.len();
                if count > 0 {
                    self.highlight = Some((col, pos.saturating_add_signed(delta).min(count - 1)));
                }
            }
            None => {
                let Some(col) = self.next_highlight_column(-1, 1) else {
                    return;
                };
                let last = self.columns()[col].non_blank.len() - 1;
                self.highlight = Some((col, if delta < 0 { last } else { 0 }));
            }
        }
    }

    /// Right/left: while highlighting, move the highlight to the next
    /// or previous column with highlightable lines; walking past the
    /// slide's edge changes slides (which drops the highlight). With no
    /// highlight, plain slide navigation -- so single-column slides
    /// behave as if columns did not exist.
    fn highlight_sideways(&mut self, dir: isize) {
        if let Some((col, _)) = self.highlight
            && let Some(next) = self.next_highlight_column(col as isize, dir)
        {
            self.highlight = Some((next, 0));
            return;
        }
        if dir > 0 {
            self.goto(self.current + 1);
        } else {
            self.goto(self.current.saturating_sub(1));
        }
    }

    /// The nearest column in direction `dir` from `from` (exclusive)
    /// with at least one highlightable line.
    fn next_highlight_column(&self, from: isize, dir: isize) -> Option<usize> {
        let columns = self.columns();
        let mut col = from + dir;
        while col >= 0 && (col as usize) < columns.len() {
            if !columns[col as usize].non_blank.is_empty() {
                return Some(col as usize);
            }
            col += dir;
        }
        None
    }

    /// Adopt a reload's freshly decoded images once the worker thread
    /// delivers them, and re-render so their placements take effect.
    fn adopt_pending_images(&mut self) {
        if let Some(rx) = &self.pending_images
            && let Ok(decoded) = rx.try_recv()
        {
            self.pending_images = None;
            self.images.adopt(decoded);
            self.error = self.images.take_errors().into_iter().next();
            self.render_cache = None;
        }
    }

    /// Keys in outline mode; returns true to quit.
    fn outline_key(&mut self, code: KeyCode) -> bool {
        let last = self.presentation.slides.len() - 1;
        match code {
            // Typing a slide number moves the selection as each digit
            // arrives; enter (below) jumps to it. Multi-digit numbers
            // just keep extending until a non-digit key.
            KeyCode::Char(c) if c.is_ascii_digit() => {
                let digit = (c as u8 - b'0') as usize;
                let n = self
                    .outline_input
                    .unwrap_or(0)
                    .saturating_mul(10)
                    .saturating_add(digit);
                if n > 0 {
                    self.outline_input = Some(n);
                    self.selected = n.min(last + 1) - 1;
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.outline_input = None;
                self.selected = (self.selected + 1).min(last);
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.outline_input = None;
                self.selected = self.selected.saturating_sub(1);
            }
            KeyCode::Home | KeyCode::Char('g') => {
                self.outline_input = None;
                self.selected = 0;
            }
            KeyCode::End | KeyCode::Char('G') => {
                self.outline_input = None;
                self.selected = last;
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                self.outline_input = None;
                self.mode = Mode::Slides;
                self.goto(self.selected);
            }
            // Esc clears a pending number first; a second one leaves.
            KeyCode::Esc if self.outline_input.is_some() => self.outline_input = None,
            KeyCode::Esc | KeyCode::Char('o') => {
                self.outline_input = None;
                self.mode = Mode::Slides;
            }
            _ => {}
        }
        false
    }

    fn transition(&self) -> Transition {
        self.presentation.metadata.transition
    }

    fn goto(&mut self, index: usize) {
        let last = self.presentation.slides.len() - 1;
        let index = index.min(last);
        if index != self.current {
            tracing::debug!(from = self.current, to = index, "goto");
            let forward = index > self.current;
            let old = self.current;
            self.current = index;
            self.highlight = None;
            self.effect = None;
            // Navigating again mid-animation skips the exit phase so rapid
            // keypresses stay snappy.
            match self.transition().exit(&self.theme, forward) {
                Some(exit) if self.outgoing.is_none() => {
                    self.outgoing = Some((old, exit));
                    self.pending_enter = self.transition().enter(&self.theme, forward);
                }
                _ => {
                    self.outgoing = None;
                    self.pending_enter = None;
                    self.effect = self.transition().enter(&self.theme, forward);
                }
            }
        }
    }

    /// Re-read the file from disk, keeping the current position. Parse
    /// errors keep the old presentation and show up in the footer.
    fn reload(&mut self) {
        match load(&self.path, &self.highlighter) {
            Ok(loaded) => {
                tracing::info!(slides = loaded.presentation.slides.len(), "reloaded");
                self.current = self.current.min(loaded.presentation.slides.len() - 1);
                self.selected = self.selected.min(loaded.presentation.slides.len() - 1);
                self.presentation = loaded.presentation;
                self.theme = loaded.theme;
                // Slide indices may have shifted; drop any running animation
                // and line highlight, and re-read images and rendered
                // slides from scratch.
                self.effect = None;
                self.outgoing = None;
                self.pending_enter = None;
                self.highlight = None;
                self.images.clear();
                // Local images re-read on demand (fast); URL images are
                // fetched by a worker so a slow network never freezes
                // the show, and adopted when the decode lands.
                let slides = self.presentation.slides.clone();
                let base = self
                    .path
                    .parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_default();
                let (tx, rx) = std::sync::mpsc::channel();
                std::thread::spawn(move || {
                    let _ = tx.send(images::decode_all(&slides, &base));
                });
                self.pending_images = Some(rx);
                self.render_cache = None;
            }
            Err(err) => {
                tracing::warn!(err = %format!("{err:#}"), "reload failed");
                self.error = Some(format!("reload failed: {err:#}"));
            }
        }
    }

    // --- drawing ---

    fn draw(&mut self, frame: &mut Frame, elapsed: Duration) {
        let area = frame.area();
        self.draw_background(frame, area);
        if area.height < 3 || area.width < 10 {
            return;
        }

        let footer = Rect::new(area.x, area.y + area.height - 1, area.width, 1);
        let pad_x = (area.width / 12).clamp(2, 12);
        let pad_y = (area.height / 12).clamp(1, 3);
        let content = Rect::new(
            area.x + pad_x,
            area.y + pad_y,
            area.width - 2 * pad_x,
            area.height - 2 * pad_y - 1,
        );

        match self.mode {
            Mode::Slides => self.draw_slide(frame, content, elapsed),
            Mode::Outline => self.draw_outline(frame, content),
        }
        self.draw_footer(frame, footer);
        if self.help {
            self.draw_help(frame, area);
        }
    }

    /// Fill the render cache for the slide at `index`, sized for
    /// `content`, unless it already holds exactly that.
    fn ensure_rendered(&mut self, index: usize, content: Rect) {
        if self.render_cache.as_ref().is_some_and(|c| {
            c.index == index && c.width == content.width && c.height == content.height
        }) {
            return;
        }
        let started = Instant::now();
        self.images.preload(&self.presentation.slides[index]);
        let images = &self.images;
        let max = (content.width, content.height.saturating_sub(2).max(1));
        let sizer = move |source: &str| images.fitted(source, max);
        let ctx = RenderContext {
            theme: &self.theme,
            highlighter: &self.highlighter,
            image_sizer: Some(&sizer),
        };
        let slide = &self.presentation.slides[index];
        let rendered = render_slide(slide, &ctx, content.width as usize);

        let paragraph = Paragraph::new(rendered.text.clone());
        tracing::debug!(
            index,
            width = content.width,
            height = content.height,
            elapsed = ?started.elapsed(),
            "rendered slide (cache miss)"
        );
        self.render_cache = Some(RenderCache {
            index,
            width: content.width,
            height: content.height,
            rendered,
            paragraph,
        });
    }

    /// Fill `area` with the theme background: one block for a solid
    /// color; per-row, per-column, or per-cell colors for a gradient.
    fn draw_background(&self, frame: &mut Frame, area: Rect) {
        let background = &self.theme.background;
        if let Background::Solid(color) = background {
            frame.render_widget(Block::default().style(Style::default().bg(*color)), area);
            return;
        }
        let buffer = frame.buffer_mut();
        match background.direction() {
            GradientDirection::Vertical => {
                for y in area.top()..area.bottom() {
                    let t = f64::from(y - area.top()) / f64::from(area.height.max(2) - 1);
                    let style = Style::default().bg(background.color_at(t));
                    for x in area.left()..area.right() {
                        buffer[(x, y)].set_style(style);
                    }
                }
            }
            GradientDirection::Horizontal => {
                for x in area.left()..area.right() {
                    let t = f64::from(x - area.left()) / f64::from(area.width.max(2) - 1);
                    let style = Style::default().bg(background.color_at(t));
                    for y in area.top()..area.bottom() {
                        buffer[(x, y)].set_style(style);
                    }
                }
            }
            GradientDirection::Radial => {
                // Normalized elliptical distance from the center: 0 at
                // the middle, 1 at the corners, so the last stop lands
                // exactly at the screen's edge regardless of aspect.
                let (cx, cy) = (
                    f64::from(area.left()) + f64::from(area.width) / 2.0,
                    f64::from(area.top()) + f64::from(area.height) / 2.0,
                );
                let (rx, ry) = (
                    f64::from(area.width.max(2)) / 2.0,
                    f64::from(area.height.max(2)) / 2.0,
                );
                for y in area.top()..area.bottom() {
                    for x in area.left()..area.right() {
                        let dx = (f64::from(x) + 0.5 - cx) / rx;
                        let dy = (f64::from(y) + 0.5 - cy) / ry;
                        let t = (dx * dx + dy * dy).sqrt() / std::f64::consts::SQRT_2;
                        buffer[(x, y)].set_style(Style::default().bg(background.color_at(t)));
                    }
                }
            }
        }
    }

    fn draw_slide(&mut self, frame: &mut Frame, content: Rect, elapsed: Duration) {
        // While an exit animation runs, keep drawing the old slide.
        let index = self.outgoing.as_ref().map_or(self.current, |(i, _)| *i);
        self.ensure_rendered(index, content);
        let cache = self.render_cache.as_ref().expect("cache was just filled");

        // The highlight cursor indexes a column's non-blank lines;
        // geometry can change on resize or reload, so re-validate.
        self.highlight = match self.highlight {
            Some((col, pos)) => match cache.rendered.columns.get(col) {
                Some(span) if !span.non_blank.is_empty() => {
                    Some((col, pos.min(span.non_blank.len() - 1)))
                }
                _ => None,
            },
            None => None,
        };

        let height = (cache.rendered.text.height() as u16).min(content.height);
        let y = content.y + (content.height - height) / 2;
        let slide_area = Rect::new(content.x, y, content.width, height);

        if let Some((col, pos)) = self.highlight {
            // Highlighting styles a copy so the cached text stays pristine.
            let span = &cache.rendered.columns[col];
            let mut text = cache.rendered.text.clone();
            style_highlight(
                &self.theme,
                self.presentation.metadata.highlight,
                &mut text,
                span,
                span.non_blank[pos],
            );
            frame.render_widget(Paragraph::new(text), slide_area);
        } else {
            frame.render_widget(&cache.paragraph, slide_area);
        }

        if let Some((_, effect)) = &mut self.outgoing {
            frame.render_effect(effect, slide_area, elapsed.into());
            if effect.done() {
                self.outgoing = None;
                self.effect = self.pending_enter.take();
                // A highlight set during the exit animation indexed the
                // outgoing slide's geometry; drop it rather than letting
                // it land somewhere arbitrary on the incoming slide.
                self.highlight = None;
            }
        } else if let Some(effect) = &mut self.effect {
            frame.render_effect(effect, slide_area, elapsed.into());
            if effect.done() {
                self.effect = None;
            }
        }

        // Images are drawn after transition effects, never before: the
        // kitty/iTerm2 protocols pack their escape payload (including a
        // transmit-once sequence) into single buffer cells, and an effect
        // rewriting such a cell would silently swallow the image.
        for placement in &cache.rendered.images {
            let Some(protocol) = self.images.protocol_mut(&placement.source) else {
                continue;
            };
            let width = placement
                .width
                .min(slide_area.width.saturating_sub(placement.x));
            let x = slide_area.x + placement.x;
            let y = slide_area.y + placement.line as u16;
            let height = placement.height.min(slide_area.bottom().saturating_sub(y));
            if height == 0 || y >= slide_area.bottom() {
                continue;
            }
            frame.render_stateful_widget(
                StatefulImage::default(),
                Rect::new(x, y, width, height),
                protocol,
            );
        }
    }

    fn draw_outline(&self, frame: &mut Frame, content: Rect) {
        let total = self.presentation.slides.len();
        let mut lines = vec![
            Line::styled(
                format!("Outline ({total} slides)"),
                Style::default()
                    .fg(self.theme.heading)
                    .add_modifier(Modifier::BOLD),
            ),
            Line::raw(""),
        ];

        let visible = (content.height as usize).saturating_sub(lines.len()).max(1);
        let offset = self.selected.saturating_sub(visible.saturating_sub(1));
        for (i, slide) in self
            .presentation
            .slides
            .iter()
            .enumerate()
            .skip(offset)
            .take(visible)
        {
            let label = slide
                .title_text()
                .or_else(|| first_text(slide))
                .unwrap_or_else(|| "(untitled)".to_string());
            let marker = if i == self.current { "*" } else { " " };
            let row = format!("{marker} {:>3}  {label}", i + 1);
            let style = if i == self.selected {
                Style::default()
                    .fg(self.theme.background.base())
                    .bg(self.theme.accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(self.theme.text)
            };
            lines.push(Line::styled(row, style));
        }
        frame.render_widget(Paragraph::new(Text::from(lines)), content);
    }

    fn draw_footer(&self, frame: &mut Frame, footer: Rect) {
        if self.presentation.metadata.footer == Some(false) {
            return;
        }
        let dim = Style::default()
            .fg(self.theme.text)
            .add_modifier(Modifier::DIM);

        let left = if let Some(error) = &self.error {
            Line::styled(
                error.clone(),
                Style::default().fg(ratatui::style::Color::Red),
            )
        } else {
            let meta = &self.presentation.metadata;
            let mut parts: Vec<String> = Vec::new();
            if let Some(t) = self.presentation.title() {
                parts.push(t);
            }
            if let Some(a) = &meta.author {
                parts.push(a.clone());
            }
            if let Some(d) = &meta.date {
                parts.push(d.clone());
            }
            Line::styled(parts.join("  -  "), dim)
        };

        let right = match (self.mode, self.outline_input) {
            (Mode::Slides, _) => format!(
                "? help  {}/{}",
                self.current + 1,
                self.presentation.slides.len()
            ),
            (Mode::Outline, Some(n)) => format!("go to {n}  enter jump  esc clear"),
            (Mode::Outline, None) => "enter jump  esc back  ? help".to_string(),
        };

        frame.render_widget(Paragraph::new(left), footer);
        frame.render_widget(
            Paragraph::new(Line::styled(right, dim)).alignment(Alignment::Right),
            footer,
        );
    }

    fn draw_help(&self, frame: &mut Frame, area: Rect) {
        let rows: &[(&str, &str)] = &[
            ("right, space, l, n", "next slide"),
            ("left, bksp, h, p", "previous slide"),
            ("down / up, j / k", "highlight line"),
            ("right / left (highlighting)", "next / previous column"),
            ("esc", "clear highlight"),
            ("g / G", "first / last slide"),
            ("o", "outline"),
            ("enter (outline)", "jump to slide"),
            ("0-9 (outline)", "go to number"),
            ("!", "shell; exit resumes"),
            ("r", "reload file"),
            ("?", "help"),
            ("q, ctrl-c", "quit"),
        ];
        let key_width = rows.iter().map(|(k, _)| k.len()).max().unwrap_or(0);
        let desc_width = rows.iter().map(|(_, v)| v.len()).max().unwrap_or(0);
        let mut lines: Vec<Line> = rows
            .iter()
            .map(|(k, v)| {
                Line::from(vec![
                    Span::styled(
                        format!("  {k:key_width$}  "),
                        Style::default().fg(self.theme.accent),
                    ),
                    Span::styled((*v).to_string(), Style::default().fg(self.theme.text)),
                ])
            })
            .collect();
        lines.push(Line::raw(""));

        // Size the box to its content (keys + descriptions + padding and
        // borders), clamped to the terminal.
        let width = ((key_width + desc_width + 6) as u16).min(area.width.saturating_sub(2));
        let height = (lines.len() as u16 + 2).min(area.height.saturating_sub(2));
        let popup = Rect::new(
            area.x + (area.width - width) / 2,
            area.y + (area.height - height) / 2,
            width,
            height,
        );
        let block = Block::bordered()
            .border_set(ASCII_BORDER)
            .border_style(Style::default().fg(self.theme.accent))
            .title(" keynot help ")
            .title_alignment(Alignment::Center)
            .style(Style::default().bg(self.theme.code_background));
        frame.render_widget(Clear, popup);
        frame.render_widget(Paragraph::new(Text::from(lines)).block(block), popup);
    }
}

/// The user's interactive shell: `%COMSPEC%` (usually cmd.exe) on
/// Windows, `$SHELL` with a `/bin/sh` fallback elsewhere.
fn default_shell() -> String {
    #[cfg(windows)]
    {
        std::env::var("COMSPEC")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "cmd.exe".to_string())
    }
    #[cfg(not(windows))]
    {
        std::env::var("SHELL")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "/bin/sh".to_string())
    }
}

/// First bit of plain text on a slide, for outline labels.
fn first_text(slide: &Slide) -> Option<String> {
    slide.columns.iter().flatten().find_map(|b| match b {
        MdBlock::Paragraph(spans) if !spans.is_empty() => {
            Some(spans.iter().map(|s| s.text.as_str()).collect::<String>())
        }
        _ => None,
    })
}

/// Plain ASCII borders, keeping output portable.
const ASCII_BORDER: border::Set = border::Set {
    top_left: "+",
    top_right: "+",
    bottom_left: "+",
    bottom_right: "+",
    vertical_left: "|",
    vertical_right: "|",
    horizontal_top: "-",
    horizontal_bottom: "-",
};

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn test_app(src: &str) -> App {
        App::new(
            PathBuf::from("/nonexistent.keynot"),
            LoadedPresentation {
                presentation: Presentation::parse(src).unwrap(),
                theme: Theme::dark(),
            },
            Highlighter::new(),
            None,
            HashMap::new(),
            PlayOptions::default(),
        )
    }

    /// Every cell of the test terminal as one string, rows separated by
    /// newlines, for substring assertions.
    fn buffer_text(terminal: &Terminal<TestBackend>) -> String {
        let buffer = terminal.backend().buffer();
        let area = *buffer.area();
        let mut out = String::new();
        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                out.push_str(buffer[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    fn draw(app: &mut App, elapsed: Duration) -> Terminal<TestBackend> {
        let mut terminal = Terminal::new(TestBackend::new(60, 20)).unwrap();
        terminal.draw(|frame| app.draw(frame, elapsed)).unwrap();
        terminal
    }

    fn press(app: &mut App, code: KeyCode) {
        app.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
    }

    #[test]
    fn left_right_arrows_change_slides() {
        let mut app = test_app("# A\n---\n# B\n---\n# C\n");
        press(&mut app, KeyCode::Right);
        assert_eq!(app.current, 1);
        press(&mut app, KeyCode::Right);
        assert_eq!(app.current, 2);
        press(&mut app, KeyCode::Left);
        assert_eq!(app.current, 1);
    }

    #[test]
    fn down_arrow_highlights_lines_not_slides() {
        let mut app = test_app("a\n\nb\n\nc\n---\n# B\n");
        draw(&mut app, Duration::ZERO);
        press(&mut app, KeyCode::Down);
        assert_eq!(app.current, 0, "down must not change slides");
        assert_eq!(app.highlight, Some((0, 0)));
        press(&mut app, KeyCode::Down);
        assert_eq!(app.highlight, Some((0, 1)));
        press(&mut app, KeyCode::Up);
        assert_eq!(app.highlight, Some((0, 0)));
        press(&mut app, KeyCode::Up);
        assert_eq!(app.highlight, Some((0, 0)), "clamps at the top");
    }

    #[test]
    fn up_from_no_highlight_starts_at_last_line() {
        let mut app = test_app("a\n\nb\n\nc\n\nd\n");
        draw(&mut app, Duration::ZERO);
        press(&mut app, KeyCode::Up);
        assert_eq!(app.highlight, Some((0, 3)));
    }

    #[test]
    fn highlight_clamps_at_bottom() {
        let mut app = test_app("a\n\nb\n");
        draw(&mut app, Duration::ZERO);
        for _ in 0..5 {
            press(&mut app, KeyCode::Down);
        }
        assert_eq!(app.highlight, Some((0, 1)));
    }

    #[test]
    fn no_highlight_before_the_first_draw() {
        // Until a draw fills the render cache there is no geometry to
        // highlight against; the key is a no-op.
        let mut app = test_app("# A\n");
        press(&mut app, KeyCode::Down);
        assert_eq!(app.highlight, None);
    }

    #[test]
    fn esc_clears_highlight_and_never_opens_outline() {
        let mut app = test_app("a\n\nb\n");
        draw(&mut app, Duration::ZERO);
        press(&mut app, KeyCode::Down);
        assert_eq!(app.highlight, Some((0, 0)));
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.highlight, None);
        assert_eq!(app.mode, Mode::Slides);
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.mode, Mode::Slides, "esc must not open the outline");
    }

    #[test]
    fn only_o_toggles_the_outline() {
        let mut app = test_app("# A\n");
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.mode, Mode::Slides, "tab must not open the outline");
        press(&mut app, KeyCode::Char('o'));
        assert_eq!(app.mode, Mode::Outline);
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.mode, Mode::Outline, "tab must not close it either");
        press(&mut app, KeyCode::Char('o'));
        assert_eq!(app.mode, Mode::Slides);
    }

    #[test]
    fn bang_requests_a_shell() {
        let mut app = test_app("# A\n");
        assert!(!app.shell_requested);
        press(&mut app, KeyCode::Char('!'));
        assert!(app.shell_requested);
        // Works from the outline too.
        app.shell_requested = false;
        app.mode = Mode::Outline;
        press(&mut app, KeyCode::Char('!'));
        assert!(app.shell_requested);
    }

    #[test]
    fn changing_slides_clears_highlight() {
        let mut app = test_app("aa\n\nbb\n---\n# B\n");
        draw(&mut app, Duration::ZERO);
        press(&mut app, KeyCode::Down);
        assert_eq!(app.highlight, Some((0, 0)));
        press(&mut app, KeyCode::Right);
        assert_eq!(app.highlight, None);
    }

    #[test]
    fn vim_keys_follow_arrow_semantics() {
        let mut app = test_app("# A\n---\n# B\n");
        draw(&mut app, Duration::ZERO);
        press(&mut app, KeyCode::Char('j'));
        assert_eq!(app.current, 0);
        assert_eq!(app.highlight, Some((0, 0)));
        press(&mut app, KeyCode::Char('l'));
        assert_eq!(app.current, 1);
    }

    #[test]
    fn dim_style_dims_all_but_the_highlighted_line() {
        use ratatui::backend::TestBackend;

        // Two one-line paragraphs with distinct letters.
        let mut app = test_app("xxx\n\nzzz\n");
        app.presentation.metadata.highlight = HighlightStyle::Dim;
        app.highlight = Some((0, 0));
        let mut terminal = ratatui::Terminal::new(TestBackend::new(60, 20)).unwrap();
        terminal
            .draw(|frame| app.draw(frame, Duration::ZERO))
            .unwrap();

        assert_eq!(app.columns()[0].non_blank.len(), 2);
        assert_eq!(app.highlight, Some((0, 0)));
        let buffer = terminal.backend().buffer();
        let mut saw = (false, false);
        for cell in buffer.content() {
            match cell.symbol() {
                "x" => {
                    saw.0 = true;
                    assert!(
                        !cell.modifier.contains(Modifier::DIM),
                        "highlighted line must not dim"
                    );
                }
                "z" => {
                    saw.1 = true;
                    assert!(
                        cell.modifier.contains(Modifier::DIM),
                        "other lines must dim"
                    );
                }
                _ => {}
            }
        }
        assert!(saw.0 && saw.1, "both lines should be on screen");
    }

    #[test]
    fn bar_style_paints_a_full_width_accent_bar() {
        use ratatui::backend::TestBackend;

        let mut app = test_app("xxx\n\nzzz\n");
        app.highlight = Some((0, 0));
        let mut terminal = ratatui::Terminal::new(TestBackend::new(60, 20)).unwrap();
        terminal
            .draw(|frame| app.draw(frame, Duration::ZERO))
            .unwrap();

        let accent = app.theme.accent;
        let background = app.theme.background.base();
        let buffer = terminal.backend().buffer();

        // Find the highlighted row via an 'x' cell.
        let area = *buffer.area();
        let mut bar_row = None;
        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                let cell = &buffer[(x, y)];
                match cell.symbol() {
                    "x" => {
                        bar_row = Some(y);
                        assert_eq!(cell.bg, accent, "bar behind the text");
                        assert_eq!(cell.fg, background, "text flips to bg color");
                    }
                    "z" => {
                        assert_ne!(cell.bg, accent, "other lines keep their bg");
                        assert!(!cell.modifier.contains(Modifier::DIM), "bar never dims");
                    }
                    _ => {}
                }
            }
        }
        let bar_row = bar_row.expect("highlighted line on screen");

        // The bar extends past the text across the content area. Content
        // starts at pad_x = clamp(60/12, 2, 12) = 5 and is 50 wide.
        let bar_cells = (5u16..55)
            .filter(|&x| buffer[(x, bar_row)].bg == accent)
            .count();
        assert_eq!(bar_cells, 50, "bar spans the full content width");
    }

    #[test]
    fn draw_counts_highlightable_lines() {
        use ratatui::backend::TestBackend;

        let mut app = test_app("# Title\n\none\n\ntwo\n");
        let mut terminal = ratatui::Terminal::new(TestBackend::new(60, 20)).unwrap();
        terminal
            .draw(|frame| app.draw(frame, Duration::ZERO))
            .unwrap();
        // Title, its underline rule, and two paragraphs; blank lines skip.
        assert_eq!(app.columns()[0].non_blank.len(), 4);
        assert_eq!(app.highlight, None, "drawing alone must not highlight");
    }

    #[test]
    fn help_modal_text_is_not_clipped() {
        let mut app = test_app("# A\n");
        app.help = true;
        let terminal = draw(&mut app, Duration::ZERO);
        let screen = buffer_text(&terminal);
        // Longest key label and longest description must appear whole.
        assert!(screen.contains("right, space, l, n"), "screen:\n{screen}");
        assert!(screen.contains("shell; exit resumes"), "screen:\n{screen}");
        assert!(screen.contains("first / last slide"), "screen:\n{screen}");
    }

    #[test]
    fn footer_shows_title_author_and_counter() {
        let mut app = test_app("---\ntitle: My Talk\nauthor: Alice\n---\n# A\n---\n# B\n");
        let terminal = draw(&mut app, Duration::ZERO);
        let screen = buffer_text(&terminal);
        assert!(screen.contains("My Talk  -  Alice"), "screen:\n{screen}");
        assert!(screen.contains("? help  1/2"), "screen:\n{screen}");
    }

    #[test]
    fn footer_hidden_when_disabled() {
        let mut app = test_app("---\nfooter: false\n---\n# A\n---\n# B\n");
        let terminal = draw(&mut app, Duration::ZERO);
        let screen = buffer_text(&terminal);
        assert!(!screen.contains("1/2"), "screen:\n{screen}");
    }

    #[test]
    fn footer_shows_errors_over_metadata() {
        let mut app = test_app("---\ntitle: T\n---\n# A\n");
        app.error = Some("reload failed: boom".to_string());
        let terminal = draw(&mut app, Duration::ZERO);
        let screen = buffer_text(&terminal);
        assert!(screen.contains("reload failed: boom"), "screen:\n{screen}");
    }

    #[test]
    fn outline_lists_slides_and_marks_the_current_one() {
        let mut app = test_app("# Alpha\n---\n# Beta\n---\nno heading here\n");
        app.mode = Mode::Outline;
        let terminal = draw(&mut app, Duration::ZERO);
        let screen = buffer_text(&terminal);
        assert!(screen.contains("Outline (3 slides)"), "screen:\n{screen}");
        assert!(screen.contains("*   1  Alpha"), "screen:\n{screen}");
        assert!(screen.contains("    2  Beta"), "screen:\n{screen}");
        // Headingless slides fall back to their first text.
        assert!(screen.contains("3  no heading here"), "screen:\n{screen}");

        // The selected row is highlighted in accent.
        let buffer = terminal.backend().buffer();
        let accent_cells = buffer
            .content()
            .iter()
            .filter(|c| c.bg == app.theme.accent)
            .count();
        assert!(accent_cells > 0, "selected outline row must be highlighted");
    }

    #[test]
    fn outline_labels_untitled_slides() {
        let mut app = test_app("```\ncode only\n```\n");
        app.mode = Mode::Outline;
        let terminal = draw(&mut app, Duration::ZERO);
        assert!(buffer_text(&terminal).contains("(untitled)"));
    }

    #[test]
    fn slide_transition_promotes_enter_after_exit() {
        // The slide transition has an exit phase.
        let mut app = test_app("---\ntransition: slide\n---\n# A\n---\n# B\n");
        press(&mut app, KeyCode::Right);
        assert!(app.outgoing.is_some(), "exit effect starts on goto");
        assert!(app.pending_enter.is_some());
        assert!(app.effect.is_none());

        // One long frame finishes the exit and promotes the enter effect.
        draw(&mut app, Duration::from_millis(500));
        assert!(app.outgoing.is_none(), "exit finished");
        assert!(app.effect.is_some(), "enter effect promoted");
        assert!(app.pending_enter.is_none());

        // Another long frame finishes the enter effect.
        draw(&mut app, Duration::from_millis(500));
        assert!(app.effect.is_none(), "enter finished");
    }

    #[test]
    fn navigating_mid_animation_skips_the_exit_phase() {
        let mut app = test_app("---\ntransition: slide\n---\n# A\n---\n# B\n---\n# C\n");
        press(&mut app, KeyCode::Right);
        assert!(app.outgoing.is_some());
        press(&mut app, KeyCode::Right);
        assert_eq!(app.current, 2);
        assert!(app.outgoing.is_none(), "second goto cancels the exit");
        assert!(app.effect.is_some(), "and enters directly");
    }

    /// A ten-slide deck for outline number-typing tests.
    fn ten_slides() -> String {
        (1..=10)
            .map(|i| format!("# Slide {i}\n"))
            .collect::<Vec<_>>()
            .join("---\n")
    }

    #[test]
    fn typing_a_number_in_the_outline_selects_that_slide() {
        let mut app = test_app(&ten_slides());
        app.mode = Mode::Outline;
        press(&mut app, KeyCode::Char('7'));
        assert_eq!(app.selected, 6);
        assert_eq!(app.outline_input, Some(7));
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.current, 6);
        assert_eq!(app.mode, Mode::Slides);
        assert_eq!(app.outline_input, None);
    }

    #[test]
    fn multi_digit_numbers_extend_the_selection() {
        let mut app = test_app(&ten_slides());
        app.mode = Mode::Outline;
        press(&mut app, KeyCode::Char('1'));
        assert_eq!(app.selected, 0, "first digit selects slide 1");
        press(&mut app, KeyCode::Char('0'));
        assert_eq!(app.selected, 9, "second digit extends to slide 10");
        assert_eq!(app.outline_input, Some(10));
    }

    #[test]
    fn out_of_range_numbers_clamp_to_the_last_slide() {
        let mut app = test_app(&ten_slides());
        app.mode = Mode::Outline;
        for c in ['9', '9'] {
            press(&mut app, KeyCode::Char(c));
        }
        assert_eq!(app.selected, 9);
        assert_eq!(app.outline_input, Some(99));
    }

    #[test]
    fn leading_zero_is_ignored() {
        let mut app = test_app(&ten_slides());
        app.mode = Mode::Outline;
        press(&mut app, KeyCode::Char('0'));
        assert_eq!(app.outline_input, None);
        press(&mut app, KeyCode::Char('3'));
        assert_eq!(app.selected, 2, "0 then 3 selects slide 3, not 03");
    }

    #[test]
    fn arrows_and_esc_clear_the_pending_number() {
        let mut app = test_app(&ten_slides());
        app.mode = Mode::Outline;
        press(&mut app, KeyCode::Char('5'));
        press(&mut app, KeyCode::Down);
        assert_eq!(app.outline_input, None);
        assert_eq!(app.selected, 5, "down moves from the typed selection");

        press(&mut app, KeyCode::Char('5'));
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.outline_input, None);
        assert_eq!(app.mode, Mode::Outline, "first esc only clears the number");
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.mode, Mode::Slides, "second esc leaves the outline");
    }

    #[test]
    fn pending_number_shows_in_the_footer() {
        let mut app = test_app(&ten_slides());
        app.mode = Mode::Outline;
        press(&mut app, KeyCode::Char('4'));
        let terminal = draw(&mut app, Duration::ZERO);
        assert!(buffer_text(&terminal).contains("go to 4"));
    }

    #[test]
    fn start_slide_clamps_to_bounds() {
        let loaded = |src: &str| LoadedPresentation {
            presentation: Presentation::parse(src).unwrap(),
            theme: Theme::dark(),
        };
        for (start, expected) in [(0, 0), (1, 0), (2, 1), (99, 1)] {
            let app = App::new(
                PathBuf::from("/x.keynot"),
                loaded("# A\n---\n# B\n"),
                Highlighter::new(),
                None,
                HashMap::new(),
                PlayOptions {
                    start_slide: start,
                    images: ImageMode::Auto,
                },
            );
            assert_eq!(app.current, expected, "start_slide: {start}");
        }
    }

    /// An app with real (halfblock) image support: a fixed-font picker
    /// instead of a terminal probe, and `source` pre-decoded to a plain
    /// 60x40 px image (6x2 cells at the 10x20 test font).
    fn test_app_with_image(src: &str, source: &str) -> App {
        // halfblocks() uses a fixed 10x20 font, matching the cell math
        // the tests below rely on.
        let picker = ratatui_image::picker::Picker::halfblocks();
        let mut decoded = HashMap::new();
        let red = image::RgbImage::from_pixel(60, 40, image::Rgb([255, 0, 0]));
        decoded.insert(source.to_string(), Ok(image::DynamicImage::ImageRgb8(red)));
        App::new(
            PathBuf::from("/nonexistent.keynot"),
            LoadedPresentation {
                presentation: Presentation::parse(src).unwrap(),
                theme: Theme::dark(),
            },
            Highlighter::new(),
            Some(picker),
            decoded,
            PlayOptions::default(),
        )
    }

    /// Columns of cells the (all-red) test image was drawn into: cells
    /// whose foreground or background is pure red. Nothing else in the
    /// dark theme uses that color.
    fn image_columns(terminal: &Terminal<TestBackend>) -> Vec<u16> {
        let red = ratatui::style::Color::Rgb(255, 0, 0);
        let buffer = terminal.backend().buffer();
        let area = *buffer.area();
        let mut cols = Vec::new();
        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                let cell = &buffer[(x, y)];
                if (cell.fg == red || cell.bg == red) && !cols.contains(&x) {
                    cols.push(x);
                }
            }
        }
        cols.sort_unstable();
        cols
    }

    fn cell_bg(terminal: &Terminal<TestBackend>, x: u16, y: u16) -> ratatui::style::Color {
        terminal.backend().buffer()[(x, y)].bg
    }

    /// Like [`test_app`], but resolving the theme from the deck's own
    /// frontmatter (needed by anything testing `colors:` overrides).
    fn test_app_themed(src: &str) -> App {
        let presentation = Presentation::parse(src).unwrap();
        let theme = Theme::from_metadata(&presentation.metadata).unwrap();
        App::new(
            PathBuf::from("/nonexistent.keynot"),
            LoadedPresentation {
                presentation,
                theme,
            },
            Highlighter::new(),
            None,
            HashMap::new(),
            PlayOptions::default(),
        )
    }

    #[test]
    fn vertical_gradient_paints_rows_top_to_bottom() {
        let mut app = test_app_themed(
            "---\ncolors:\n  background:\n    gradient: ['#000000', '#ffffff']\n---\n# A\n",
        );
        let terminal = draw(&mut app, Duration::ZERO);
        let area = *terminal.backend().buffer().area();
        assert_eq!(
            cell_bg(&terminal, 0, 0),
            ratatui::style::Color::Rgb(0, 0, 0)
        );
        assert_eq!(
            cell_bg(&terminal, 0, area.height - 1),
            ratatui::style::Color::Rgb(255, 255, 255)
        );
        // A middle row sits between the stops, and rows are uniform.
        let mid = cell_bg(&terminal, 0, area.height / 2);
        assert!(matches!(mid, ratatui::style::Color::Rgb(v, _, _) if v > 60 && v < 200));
        assert_eq!(mid, cell_bg(&terminal, area.width - 1, area.height / 2));
    }

    #[test]
    fn horizontal_gradient_paints_columns_left_to_right() {
        let deck = "---\ncolors:\n  background:\n    gradient: ['#000000', '#ffffff']\n    direction: horizontal\n---\n# A\n";
        let mut app = test_app_themed(deck);
        let terminal = draw(&mut app, Duration::ZERO);
        let area = *terminal.backend().buffer().area();
        assert_eq!(
            cell_bg(&terminal, 0, 0),
            ratatui::style::Color::Rgb(0, 0, 0)
        );
        assert_eq!(
            cell_bg(&terminal, area.width - 1, 0),
            ratatui::style::Color::Rgb(255, 255, 255)
        );
    }

    #[test]
    fn radial_gradient_is_darkest_at_the_center() {
        let deck = "---\ncolors:\n  background:\n    gradient: ['#000000', '#ffffff']\n    direction: radial\n---\n# A\n";
        let mut app = test_app_themed(deck);
        let terminal = draw(&mut app, Duration::ZERO);
        let area = *terminal.backend().buffer().area();
        let center = cell_bg(&terminal, area.width / 2, area.height / 2);
        let corner = cell_bg(&terminal, 0, 0);
        let (ratatui::style::Color::Rgb(c, _, _), ratatui::style::Color::Rgb(k, _, _)) =
            (center, corner)
        else {
            panic!("gradient cells must be RGB: {center:?} {corner:?}");
        };
        assert!(c < 40, "center is near the first stop: {c}");
        assert!(k > 215, "corners are near the last stop: {k}");
    }

    #[test]
    fn gradient_shows_through_slide_text_rows() {
        // Text cells keep the gradient bg (spans set no background).
        let mut app = test_app_themed(
            "---\ncolors:\n  background:\n    gradient: ['#000000', '#ffffff']\n---\nhello\n",
        );
        let terminal = draw(&mut app, Duration::ZERO);
        let screen = buffer_text(&terminal);
        let row = screen.lines().position(|l| l.contains("hello")).unwrap() as u16;
        let x = screen.lines().nth(row as usize).unwrap().find('h').unwrap() as u16;
        let bg = cell_bg(&terminal, x, row);
        assert!(
            matches!(bg, ratatui::style::Color::Rgb(..)),
            "text cell keeps the gradient bg: {bg:?}"
        );
    }

    #[test]
    fn reload_decodes_images_on_a_worker_and_adopts_them() {
        let dir = tempfile::tempdir().unwrap();
        let red = image::RgbImage::from_pixel(60, 40, image::Rgb([255, 0, 0]));
        red.save(dir.path().join("x.png")).unwrap();
        let deck = dir.path().join("t.keynot");
        fs_err::write(&deck, "![p](x.png)\n").unwrap();

        let highlighter = Highlighter::new();
        let loaded = load(&deck, &highlighter).unwrap();
        let picker = ratatui_image::picker::Picker::halfblocks();
        let mut app = App::new(
            deck,
            loaded,
            highlighter,
            Some(picker),
            HashMap::new(),
            PlayOptions::default(),
        );

        app.reload();
        assert!(app.pending_images.is_some(), "worker spawned");
        // The worker reads a local file; bounded wait for its delivery.
        let deadline = Instant::now() + Duration::from_secs(5);
        while app.pending_images.is_some() && Instant::now() < deadline {
            app.adopt_pending_images();
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(app.pending_images.is_none(), "decode adopted");
        assert_eq!(app.error, None);
        // The adopted image draws.
        let terminal = draw(&mut app, Duration::ZERO);
        assert!(!image_columns(&terminal).is_empty());
    }

    #[test]
    fn image_draws_centered_in_a_single_column_slide() {
        let mut app = test_app_with_image("![pic](x.png)\n", "x.png");
        let terminal = draw(&mut app, Duration::ZERO);
        let cols = image_columns(&terminal);
        assert_eq!(cols.len(), 6, "6-cell-wide image");
        let mid = (cols[0] + cols[5]) / 2;
        let screen_mid = terminal.backend().buffer().area().width / 2;
        assert!(
            mid.abs_diff(screen_mid) <= 2,
            "roughly centered: {cols:?} vs screen middle {screen_mid}"
        );
    }

    #[test]
    fn image_draws_inside_its_column_not_slide_centered() {
        // Text left, image right: the image must be drawn within the
        // second column, never re-centered across the whole slide (which
        // would paint it over the left column's text).
        let mut app = test_app_with_image("LEFTTEXT\n|||\n![pic](x.png)\n", "x.png");
        let terminal = draw(&mut app, Duration::ZERO);
        let cols = image_columns(&terminal);
        assert!(!cols.is_empty(), "image cells drawn");
        let screen = buffer_text(&terminal);
        let row = screen.lines().find(|l| l.contains("LEFTTEXT")).unwrap();
        let text_end = row.find("LEFTTEXT").unwrap() + "LEFTTEXT".len();
        assert!(
            cols[0] as usize > text_end,
            "image starts right of the left column text: cols {cols:?}, text ends {text_end}"
        );
        // And past the second column's start (content is ~72 wide on the
        // 80-col test terminal: columns ~34 wide, column 2 starts ~38).
        assert!(cols[0] >= 38, "in the second column: {cols:?}");
        // The left column's text is not painted over.
        assert!(screen.contains("LEFTTEXT"));
    }

    #[test]
    fn two_column_slides_draw_both_columns() {
        let mut app = test_app("LEFTTEXT\n|||\nRIGHTTEXT\n");
        let terminal = draw(&mut app, Duration::ZERO);
        let screen = buffer_text(&terminal);
        let row = screen
            .lines()
            .find(|l| l.contains("LEFTTEXT"))
            .expect("left column on screen");
        assert!(row.contains("RIGHTTEXT"), "columns share a row: {row:?}");
    }

    #[test]
    fn three_column_slides_draw_all_columns() {
        let mut app = test_app("AAA\n|||\nBBB\n|||\nCCC\n");
        let terminal = draw(&mut app, Duration::ZERO);
        let row = buffer_text(&terminal)
            .lines()
            .find(|l| l.contains("AAA"))
            .expect("first column on screen")
            .to_string();
        assert!(row.contains("BBB"), "second column shares the row: {row:?}");
        assert!(row.contains("CCC"), "third column shares the row: {row:?}");
        let (a, b, c) = (
            row.find("AAA").unwrap(),
            row.find("BBB").unwrap(),
            row.find("CCC").unwrap(),
        );
        assert!(a < b && b < c, "columns in order: {row:?}");
    }

    #[test]
    fn outline_labels_find_headingless_column_text() {
        let mut app = test_app("first col\n|||\n## Col Title\n---\n# B\n");
        app.mode = Mode::Outline;
        let terminal = draw(&mut app, Duration::ZERO);
        // The heading in the second column labels the slide.
        assert!(buffer_text(&terminal).contains("Col Title"));
    }

    /// Letter cells whose background is the accent bar.
    fn letters_on_bar(terminal: &Terminal<TestBackend>, accent: ratatui::style::Color) -> String {
        let buffer = terminal.backend().buffer();
        let mut on_bar = String::new();
        for cell in buffer.content() {
            if cell.symbol().chars().all(|c| c.is_ascii_lowercase())
                && cell.symbol() != " "
                && cell.bg == accent
            {
                on_bar.push_str(cell.symbol());
            }
        }
        on_bar
    }

    #[test]
    fn highlight_bar_covers_only_its_column() {
        let mut app = test_app("aaa\n|||\nbbb\n");
        app.highlight = Some((0, 0));
        let terminal = draw(&mut app, Duration::ZERO);
        assert_eq!(
            letters_on_bar(&terminal, app.theme.accent),
            "aaa",
            "only the first column rides the bar"
        );
    }

    #[test]
    fn highlight_bar_follows_the_column() {
        let mut app = test_app("aaa\n|||\nbbb\n");
        app.highlight = Some((1, 0));
        let terminal = draw(&mut app, Duration::ZERO);
        assert_eq!(letters_on_bar(&terminal, app.theme.accent), "bbb");
    }

    #[test]
    fn right_moves_the_highlight_to_the_next_column() {
        let mut app = test_app("aaa\n|||\nbbb\n---\n# Two\n");
        draw(&mut app, Duration::ZERO);
        press(&mut app, KeyCode::Down);
        assert_eq!(app.highlight, Some((0, 0)));
        press(&mut app, KeyCode::Right);
        assert_eq!(app.highlight, Some((1, 0)), "into column 2, first line");
        assert_eq!(app.current, 0, "still the same slide");
    }

    #[test]
    fn right_past_the_last_column_changes_slides() {
        let mut app = test_app("aaa\n|||\nbbb\n---\n# Two\n");
        draw(&mut app, Duration::ZERO);
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Right);
        press(&mut app, KeyCode::Right);
        assert_eq!(app.current, 1, "walked off the right edge");
        assert_eq!(app.highlight, None, "highlight dropped on slide change");
    }

    #[test]
    fn left_past_the_first_column_changes_slides() {
        let mut app = test_app("# One\n---\naaa\n|||\nbbb\n");
        draw(&mut app, Duration::ZERO);
        press(&mut app, KeyCode::Right); // to the columns slide
        draw(&mut app, Duration::ZERO);
        press(&mut app, KeyCode::Down);
        assert_eq!(app.highlight, Some((0, 0)));
        press(&mut app, KeyCode::Left);
        assert_eq!(app.current, 0, "walked off the left edge");
        assert_eq!(app.highlight, None);
    }

    #[test]
    fn moving_between_columns_lands_on_the_first_line() {
        let mut app = test_app("a1\n\na2\n\na3\n|||\nb1\n\nb2\n");
        draw(&mut app, Duration::ZERO);
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Down);
        assert_eq!(app.highlight, Some((0, 2)), "third line of column 1");
        press(&mut app, KeyCode::Right);
        assert_eq!(app.highlight, Some((1, 0)), "first line of column 2");
    }

    #[test]
    fn image_only_columns_are_skipped_sideways() {
        // Column 2 is a picture with no highlightable lines; right from
        // column 1 walks straight past it off the slide.
        let mut app = test_app_with_image("aaa\n|||\n![p](x.png)\n---\n# Two\n", "x.png");
        draw(&mut app, Duration::ZERO);
        press(&mut app, KeyCode::Down);
        assert_eq!(app.highlight, Some((0, 0)));
        press(&mut app, KeyCode::Right);
        assert_eq!(app.current, 1, "the empty column does not trap the cursor");
    }

    #[test]
    fn single_column_right_still_changes_slides_while_highlighting() {
        let mut app = test_app("# One\n\ntext\n---\n# Two\n");
        draw(&mut app, Duration::ZERO);
        press(&mut app, KeyCode::Down);
        assert!(app.highlight.is_some());
        press(&mut app, KeyCode::Right);
        assert_eq!(app.current, 1, "exactly the pre-columns behavior");
        assert_eq!(app.highlight, None);
    }

    #[test]
    fn dim_highlight_dims_the_other_columns_segment() {
        let mut app = test_app("---\nhighlight: dim\n---\naaa\n|||\nbbb\n");
        app.highlight = Some((0, 0));
        let terminal = draw(&mut app, Duration::ZERO);
        let buffer = terminal.backend().buffer();
        let (mut a_dim, mut b_dim) = (false, false);
        for cell in buffer.content() {
            if cell.symbol() == "a" {
                a_dim |= cell.modifier.contains(Modifier::DIM);
            }
            if cell.symbol() == "b" {
                b_dim |= cell.modifier.contains(Modifier::DIM);
            }
        }
        assert!(!a_dim, "the highlighted column keeps full brightness");
        assert!(b_dim, "the other column dims, even on the same row");
    }

    #[test]
    fn render_cache_serves_stale_content_until_invalidated() {
        let mut app = test_app("first slide\n---\nsecond slide\n");
        let terminal = draw(&mut app, Duration::ZERO);
        assert!(buffer_text(&terminal).contains("first slide"));
        let cached = app.render_cache.as_ref().expect("cache filled by draw");
        assert_eq!((cached.index, cached.width, cached.height), (0, 50, 17));

        // Mutating the slide without invalidation keeps serving the cache.
        app.presentation.slides[0] = crate::markdown::Slide::parse("CHANGED");
        let terminal = draw(&mut app, Duration::ZERO);
        assert!(
            buffer_text(&terminal).contains("first slide"),
            "same slide and size must be a cache hit"
        );

        // A different terminal size misses and re-renders.
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal
            .draw(|frame| app.draw(frame, Duration::ZERO))
            .unwrap();
        assert!(buffer_text(&terminal).contains("CHANGED"));

        // Navigating changes the cached index (two frames: the first
        // draws the outgoing slide's exit animation to completion).
        press(&mut app, KeyCode::Right);
        for _ in 0..2 {
            terminal
                .draw(|frame| app.draw(frame, Duration::from_secs(1)))
                .unwrap();
        }
        assert_eq!(app.render_cache.as_ref().unwrap().index, 1);
    }

    #[test]
    fn reload_clears_the_render_cache() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.keynot");
        fs_err::write(&path, "# A\n").unwrap();
        let highlighter = Highlighter::new();
        let loaded = load(&path, &highlighter).unwrap();
        let mut app = App::new(
            path.clone(),
            loaded,
            highlighter,
            None,
            HashMap::new(),
            PlayOptions::default(),
        );
        draw(&mut app, Duration::ZERO);
        assert!(app.render_cache.is_some());
        app.reload();
        assert!(app.render_cache.is_none(), "reload must invalidate");
    }

    #[test]
    fn reload_picks_up_changes_and_clamps_position() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.keynot");
        fs_err::write(&path, "# A\n---\n# B\n---\n# C\n").unwrap();
        let highlighter = Highlighter::new();
        let loaded = load(&path, &highlighter).unwrap();
        let mut app = App::new(
            path.clone(),
            loaded,
            highlighter,
            None,
            HashMap::new(),
            PlayOptions {
                start_slide: 3,
                images: ImageMode::Auto,
            },
        );
        assert_eq!(app.current, 2);

        fs_err::write(&path, "# Only\n").unwrap();
        app.reload();
        assert!(app.error.is_none());
        assert_eq!(app.presentation.slides.len(), 1);
        assert_eq!(app.current, 0, "position clamps to the shorter deck");
    }

    #[test]
    fn reload_failure_keeps_the_old_deck() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.keynot");
        fs_err::write(&path, "# A\n---\n# B\n").unwrap();
        let highlighter = Highlighter::new();
        let loaded = load(&path, &highlighter).unwrap();
        let mut app = App::new(
            path.clone(),
            loaded,
            highlighter,
            None,
            HashMap::new(),
            PlayOptions::default(),
        );

        fs_err::write(&path, "---\ntitle: [unclosed\n---\n# X\n").unwrap();
        app.reload();
        let error = app.error.as_deref().expect("reload error is surfaced");
        assert!(error.starts_with("reload failed"), "got: {error}");
        assert_eq!(app.presentation.slides.len(), 2, "old deck is kept");
    }
    #[test]
    fn highlight_bar_works_after_navigation() {
        let src =
            fs_err::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/examples/tour.keynot"))
                .unwrap();
        let mut app = test_app(&src);
        // Navigate to slide 2 and let the transition finish.
        press(&mut app, KeyCode::Right);
        draw(&mut app, Duration::from_millis(1000));
        draw(&mut app, Duration::from_millis(1000));
        assert!(app.effect.is_none() && app.outgoing.is_none());
        assert!(!app.columns().is_empty(), "geometry after transition");
        press(&mut app, KeyCode::Down);
        assert_eq!(app.highlight, Some((0, 0)));
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal
            .draw(|frame| app.draw(frame, Duration::ZERO))
            .unwrap();
        let buffer = terminal.backend().buffer();
        let accent = app.theme.accent;
        let hits = buffer.content().iter().filter(|c| c.bg == accent).count();
        assert!(hits > 0, "no accent bar cells found");
    }
}
