//! The interactive presentation player: event loop, input, and drawing.

mod config;
mod images;

use crate::markdown::{Block as MdBlock, HighlightStyle, Presentation, Transition};
use crate::render::{Highlighter, RenderContext, RenderedSlide, render_slide};
use crate::theme::Theme;
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
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tachyonfx::Effect;
use tachyonfx::EffectRenderer;

/// Options for `keynot play`.
#[derive(Debug, Clone, Copy, Default)]
pub struct PlayOptions {
    /// 1-based slide to start on (0 or 1 both mean the first slide).
    pub start_slide: usize,
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
    Ok(LoadedPresentation {
        presentation,
        theme,
    })
}

/// Play a presentation in the terminal.
pub fn play(path: &Path, options: PlayOptions) -> Result<()> {
    let highlighter = Highlighter::new();
    let loaded = load(path, &highlighter)?;
    let mut terminal = ratatui::init();
    // Probe the terminal for its graphics protocol and font size. This
    // should run after terminal init; ratatui-image manages raw mode for
    // its own stdio query round-trip.
    let picker = Picker::from_query_stdio().ok();
    let mut app = App::new(path.to_path_buf(), loaded, highlighter, picker, options);
    let result = app.run(&mut terminal);
    // ratatui::restore() does not unhide the cursor that draw() hides.
    let _ = terminal.show_cursor();
    ratatui::restore();
    result
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Slides,
    Outline,
}

struct App {
    path: PathBuf,
    presentation: Presentation,
    theme: Theme,
    highlighter: Highlighter,
    images: Images,
    /// Rendered text and image placements for `(slide index, width,
    /// height)`, so idle and animation frames skip re-rendering (and
    /// especially re-highlighting) unchanged slides.
    render_cache: Option<(usize, u16, u16, RenderedSlide)>,
    current: usize,
    mode: Mode,
    /// Outline cursor (slide index).
    selected: usize,
    /// Slide number being typed in the outline (1-based, as displayed).
    outline_input: Option<usize>,
    help: bool,
    /// Speaker's line highlight: an index into the current slide's
    /// non-blank rendered lines, styled per the `highlight:` metadata key.
    highlight: Option<usize>,
    /// How many lines were highlightable in the last rendered frame;
    /// updated at draw time (it depends on wrapping width).
    highlight_count: usize,
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
        images.preload_all(&loaded.presentation.slides);
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
            highlight_count: 0,
            shell_requested: false,
            effect: None,
            outgoing: None,
            pending_enter: None,
            error: None,
            last_frame: Instant::now(),
        }
    }

    fn run(&mut self, terminal: &mut ratatui::DefaultTerminal) -> Result<()> {
        loop {
            let elapsed = self.last_frame.elapsed();
            self.last_frame = Instant::now();
            terminal.draw(|frame| self.draw(frame, elapsed))?;
            let timeout = if self.effect.is_some() || self.outgoing.is_some() {
                Duration::from_millis(16)
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
        let status = std::process::Command::new(&shell).status();

        // Re-enter the TUI before propagating any error, so a missing
        // shell does not leave the terminal in cooked mode.
        *terminal = ratatui::init();
        terminal.clear()?;
        self.last_frame = Instant::now();
        status.wrap_err_with(|| format!("cannot run {shell}"))?;
        Ok(())
    }

    // --- input ---

    /// Handle a key press; returns true to quit.
    fn handle_key(&mut self, key: KeyEvent) -> bool {
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
            KeyCode::Right
            | KeyCode::PageDown
            | KeyCode::Enter
            | KeyCode::Char(' ')
            | KeyCode::Char('l')
            | KeyCode::Char('n') => self.goto(self.current + 1),
            KeyCode::Left
            | KeyCode::PageUp
            | KeyCode::Backspace
            | KeyCode::Char('h')
            | KeyCode::Char('p') => self.goto(self.current.saturating_sub(1)),
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

    /// Move the line highlight down (`+1`) or up (`-1`). Starting fresh,
    /// down highlights the first line and up the last.
    fn highlight_move(&mut self, delta: isize) {
        if self.highlight_count == 0 {
            return;
        }
        let last = self.highlight_count - 1;
        self.highlight = Some(match (self.highlight, delta) {
            (None, d) if d < 0 => last,
            (None, _) => 0,
            (Some(pos), d) => pos.saturating_add_signed(d).min(last),
        });
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
                self.current = self.current.min(loaded.presentation.slides.len() - 1);
                self.selected = self.selected.min(loaded.presentation.slides.len() - 1);
                self.presentation = loaded.presentation;
                self.theme = loaded.theme;
                self.error = None;
                // Slide indices may have shifted; drop any running animation
                // and line highlight, and re-read images and rendered
                // slides from scratch.
                self.effect = None;
                self.outgoing = None;
                self.pending_enter = None;
                self.highlight = None;
                self.images.clear();
                self.images.preload_all(&self.presentation.slides);
                self.render_cache = None;
            }
            Err(err) => self.error = Some(format!("reload failed: {err:#}")),
        }
    }

    // --- drawing ---

    fn draw(&mut self, frame: &mut Frame, elapsed: Duration) {
        let area = frame.area();
        frame.render_widget(
            Block::default().style(Style::default().bg(self.theme.background)),
            area,
        );
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

    /// Render (or fetch from the cache) the slide at `index`, sized for
    /// `content`. Highlighting is applied by the caller on a copy, so the
    /// cached text stays pristine.
    fn rendered_slide(&mut self, index: usize, content: Rect) -> RenderedSlide {
        if let Some((i, w, h, rendered)) = &self.render_cache
            && *i == index
            && *w == content.width
            && *h == content.height
        {
            return rendered.clone();
        }
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
        self.render_cache = Some((index, content.width, content.height, rendered.clone()));
        rendered
    }

    fn draw_slide(&mut self, frame: &mut Frame, content: Rect, elapsed: Duration) {
        // While an exit animation runs, keep drawing the old slide.
        let index = self.outgoing.as_ref().map_or(self.current, |(i, _)| *i);
        let RenderedSlide {
            mut text,
            images: placements,
        } = self.rendered_slide(index, content);

        // The renderer reserved blank rows for each image; if a renderer
        // change breaks that contract, fail loudly in debug builds
        // instead of drawing pictures over text.
        #[cfg(debug_assertions)]
        for p in &placements {
            for line in text.lines.iter().skip(p.line).take(p.height as usize) {
                debug_assert_eq!(line.width(), 0, "image rows must be blank");
            }
        }

        self.apply_highlight(&mut text, content.width as usize);
        let height = (text.height() as u16).min(content.height);
        let y = content.y + (content.height - height) / 2;
        let slide_area = Rect::new(content.x, y, content.width, height);
        frame.render_widget(Paragraph::new(text), slide_area);

        for placement in placements {
            let Some(protocol) = self.images.protocol_mut(&placement.source) else {
                continue;
            };
            let width = placement.width.min(slide_area.width);
            let x = slide_area.x + (slide_area.width - width) / 2;
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

        if let Some((_, effect)) = &mut self.outgoing {
            frame.render_effect(effect, slide_area, elapsed.into());
            if effect.done() {
                self.outgoing = None;
                self.effect = self.pending_enter.take();
            }
        } else if let Some(effect) = &mut self.effect {
            frame.render_effect(effect, slide_area, elapsed.into());
            if effect.done() {
                self.effect = None;
            }
        }
    }

    /// Mark the highlighted line so the speaker can point the audience at
    /// the line being discussed: either an accent bar behind it or by
    /// dimming everything else, per the `highlight:` metadata key. Blank
    /// lines are not highlightable; the cursor indexes non-blank lines.
    fn apply_highlight(&mut self, text: &mut Text<'static>, width: usize) {
        let non_blank: Vec<usize> = text
            .lines
            .iter()
            .enumerate()
            .filter(|(_, line)| line.width() > 0)
            .map(|(i, _)| i)
            .collect();
        self.highlight_count = non_blank.len();

        let Some(pos) = self.highlight else {
            return;
        };
        if non_blank.is_empty() {
            self.highlight = None;
            return;
        }
        // The line count can shrink on resize; keep the cursor in range.
        let pos = pos.min(non_blank.len() - 1);
        self.highlight = Some(pos);
        let target = non_blank[pos];
        match self.presentation.metadata.highlight {
            HighlightStyle::Dim => {
                for (i, line) in text.lines.iter_mut().enumerate() {
                    if i != target {
                        line.style = line.style.add_modifier(Modifier::DIM);
                    }
                }
            }
            HighlightStyle::Bar => {
                // Repaint the line onto a full-width accent bar. The bar
                // owns the colors (fg becomes the background color, since
                // arbitrary foregrounds like syntax colors are unreadable
                // on the accent); bold/italic and such survive.
                let bar = Style::default()
                    .fg(self.theme.background)
                    .bg(self.theme.accent);
                let line = &mut text.lines[target];
                line.style = line.style.patch(bar);
                for span in &mut line.spans {
                    span.style = span.style.patch(bar);
                }
                let pad = width.saturating_sub(line.width());
                if pad > 0 {
                    line.spans.push(Span::styled(" ".repeat(pad), bar));
                }
            }
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
                    .fg(self.theme.background)
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
fn first_text(slide: &crate::markdown::Slide) -> Option<String> {
    slide.blocks.iter().find_map(|b| match b {
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
        let mut app = test_app("# A\n---\n# B\n");
        app.highlight_count = 3;
        press(&mut app, KeyCode::Down);
        assert_eq!(app.current, 0, "down must not change slides");
        assert_eq!(app.highlight, Some(0));
        press(&mut app, KeyCode::Down);
        assert_eq!(app.highlight, Some(1));
        press(&mut app, KeyCode::Up);
        assert_eq!(app.highlight, Some(0));
        press(&mut app, KeyCode::Up);
        assert_eq!(app.highlight, Some(0), "clamps at the top");
    }

    #[test]
    fn up_from_no_highlight_starts_at_last_line() {
        let mut app = test_app("# A\n");
        app.highlight_count = 4;
        press(&mut app, KeyCode::Up);
        assert_eq!(app.highlight, Some(3));
    }

    #[test]
    fn highlight_clamps_at_bottom() {
        let mut app = test_app("# A\n");
        app.highlight_count = 2;
        for _ in 0..5 {
            press(&mut app, KeyCode::Down);
        }
        assert_eq!(app.highlight, Some(1));
    }

    #[test]
    fn no_highlight_without_lines() {
        let mut app = test_app("# A\n");
        app.highlight_count = 0;
        press(&mut app, KeyCode::Down);
        assert_eq!(app.highlight, None);
    }

    #[test]
    fn esc_clears_highlight_and_never_opens_outline() {
        let mut app = test_app("# A\n");
        app.highlight_count = 2;
        press(&mut app, KeyCode::Down);
        assert_eq!(app.highlight, Some(0));
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
        let mut app = test_app("# A\n---\n# B\n");
        app.highlight_count = 2;
        press(&mut app, KeyCode::Down);
        assert_eq!(app.highlight, Some(0));
        press(&mut app, KeyCode::Right);
        assert_eq!(app.highlight, None);
    }

    #[test]
    fn vim_keys_follow_arrow_semantics() {
        let mut app = test_app("# A\n---\n# B\n");
        app.highlight_count = 3;
        press(&mut app, KeyCode::Char('j'));
        assert_eq!(app.current, 0);
        assert_eq!(app.highlight, Some(0));
        press(&mut app, KeyCode::Char('l'));
        assert_eq!(app.current, 1);
    }

    #[test]
    fn dim_style_dims_all_but_the_highlighted_line() {
        use ratatui::backend::TestBackend;

        // Two one-line paragraphs with distinct letters.
        let mut app = test_app("xxx\n\nzzz\n");
        app.presentation.metadata.highlight = HighlightStyle::Dim;
        app.highlight = Some(0);
        let mut terminal = ratatui::Terminal::new(TestBackend::new(60, 20)).unwrap();
        terminal
            .draw(|frame| app.draw(frame, Duration::ZERO))
            .unwrap();

        assert_eq!(app.highlight_count, 2);
        assert_eq!(app.highlight, Some(0));
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
        app.highlight = Some(0);
        let mut terminal = ratatui::Terminal::new(TestBackend::new(60, 20)).unwrap();
        terminal
            .draw(|frame| app.draw(frame, Duration::ZERO))
            .unwrap();

        let accent = app.theme.accent;
        let background = app.theme.background;
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
        assert_eq!(app.highlight_count, 4);
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
        // Default transition is `slide`, which has an exit phase.
        let mut app = test_app("# A\n---\n# B\n");
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
        let mut app = test_app("# A\n---\n# B\n---\n# C\n");
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
                PlayOptions { start_slide: start },
            );
            assert_eq!(app.current, expected, "start_slide: {start}");
        }
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
            PlayOptions { start_slide: 3 },
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
        assert!(app.highlight_count > 0, "count: {}", app.highlight_count);
        press(&mut app, KeyCode::Down);
        assert_eq!(app.highlight, Some(0));
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
