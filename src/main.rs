use clap::builder::styling::{Color, RgbColor, Style, Styles};
use clap::{Parser, Subcommand};
use eyre::{Result, WrapErr, bail};
use keynot::app::{self, ImageMode, PlayOptions};
use keynot::render::Highlighter;
use keynot::template;
use std::path::PathBuf;

/// Help colors matching the default dark theme (VS Code Dark+): blue
/// headings, yellow command/flag names (see `Theme::dark`).
const HELP_HEADING: Color = Color::Rgb(RgbColor(0x56, 0x9c, 0xd6));
const HELP_ACCENT: Color = Color::Rgb(RgbColor(0xdc, 0xdc, 0xaa));
const HELP_STYLES: Styles = Styles::styled()
    .header(Style::new().bold().fg_color(Some(HELP_HEADING)))
    .usage(Style::new().bold().fg_color(Some(HELP_HEADING)))
    .literal(Style::new().fg_color(Some(HELP_ACCENT)))
    .placeholder(Style::new().dimmed());

/// Terminal slide presentations from markdown
///
///     keynot new my-talk.keynot  # Then edit the markdown in the file
///     keynot play my-talk.keynot
///
#[derive(Parser)]
#[command(name = "keynot", version, verbatim_doc_comment, styles = HELP_STYLES)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Play a presentation
    Play {
        /// The .keynot file to present
        file: PathBuf,
        /// Start at slide N (1-based)
        #[arg(long, value_name = "N", default_value_t = 1)]
        start_slide: usize,
        /// How to draw images: the terminal's best protocol, textual half-blocks, or not at all
        #[arg(long, value_enum, default_value_t)]
        images: ImageMode,
    },
    /// Create a new skeleton presentation
    New {
        /// Path of the file to create (e.g. talk.keynot)
        file: PathBuf,
        /// Overwrite the file if it already exists
        #[arg(short, long)]
        force: bool,
    },
    /// Validate a presentation and print a summary
    Check {
        /// The .keynot file to validate
        file: PathBuf,
    },
}

fn main() -> Result<()> {
    init_tracing()?;
    match Cli::parse().command {
        Command::Play {
            file,
            start_slide,
            images,
        } => app::play(
            &file,
            PlayOptions {
                start_slide,
                images,
            },
        ),
        Command::New { file, force } => new(file, force),
        Command::Check { file } => check(file),
    }
}

/// When `KEYNOT_LOG` is set (to an `EnvFilter` spec such as `debug` or
/// `keynot=trace`), write tracing output to `keynot.log` in the current
/// directory. A file, never stdio: the player owns the terminal.
fn init_tracing() -> Result<()> {
    let Ok(filter) = std::env::var("KEYNOT_LOG") else {
        return Ok(());
    };
    if filter.is_empty() {
        return Ok(());
    }
    let log = fs_err::File::create("keynot.log")
        .wrap_err("cannot create keynot.log for KEYNOT_LOG output")?;
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new(filter))
        .with_writer(std::sync::Mutex::new(log))
        .with_ansi(false)
        .init();
    tracing::info!(version = env!("CARGO_PKG_VERSION"), "keynot starting");
    Ok(())
}

fn new(file: PathBuf, force: bool) -> Result<()> {
    if file.exists() && !force {
        bail!(
            "{} already exists (use --force to overwrite)",
            file.display()
        );
    }
    let title = file
        .file_stem()
        .map(|s| s.to_string_lossy().replace(['-', '_'], " "))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "My Presentation".to_string());
    fs_err::write(&file, template::skeleton(&title))
        .wrap_err_with(|| format!("cannot write {}", file.display()))?;
    println!("Created {}", file.display());
    println!("Play it with: keynot play {}", file.display());
    Ok(())
}

fn check(file: PathBuf) -> Result<()> {
    let highlighter = Highlighter::new();
    let presentation = app::load(&file, &highlighter)?.presentation;
    println!("{}: OK", file.display());
    if let Some(title) = presentation.title() {
        println!("  title:  {title}");
    }
    if let Some(author) = &presentation.metadata.author {
        println!("  author: {author}");
    }
    println!(
        "  theme:  {}",
        presentation.metadata.theme.as_deref().unwrap_or("dark")
    );
    println!("  slides: {}", presentation.slides.len());
    let notes: usize = presentation.slides.iter().map(|s| s.notes.len()).sum();
    if notes > 0 {
        println!("  notes:  {notes}");
    }
    Ok(())
}
