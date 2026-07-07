use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};

use keynot::app::{self, PlayOptions};
use keynot::render::Highlighter;
use keynot::template;

#[derive(Parser)]
#[command(
    name = "keynot",
    version,
    about = "Terminal slide presentations from markdown"
)]
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
    },
    /// Create a new skeleton presentation
    New {
        /// Path of the file to create (e.g. talk.keynot)
        file: PathBuf,
        /// Overwrite the file if it already exists
        #[arg(long)]
        force: bool,
    },
    /// Validate a presentation and print a summary
    Check {
        /// The .keynot file to validate
        file: PathBuf,
    },
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Play { file, start_slide } => app::play(&file, PlayOptions { start_slide }),
        Command::New { file, force } => new(file, force),
        Command::Check { file } => check(file),
    }
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
        .with_context(|| format!("cannot write {}", file.display()))?;
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
