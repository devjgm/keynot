//! Project automation, run as `cargo xtask <command>` (the justfile
//! recipes wrap the common invocations).

use clap::{Parser, Subcommand};
use image::codecs::gif::GifDecoder;
use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use image::{
    AnimationDecoder, DynamicImage, ExtendedColorType, ImageEncoder, Rgba, RgbaImage, imageops,
};
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use std::io::{Read, Write};
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

const COLS: u16 = 100;
const ROWS: u16 = 30;
const TOUR: &str = "examples/tour.keynot";
const KEYNOT: &str = "target/debug/keynot";
const OUT_DIR: &str = "assets/screenshots";
/// The real image pasted over half-block cells (the tour's Ferris).
const IMAGE_URL: &str = "https://rustacean.net/assets/rustacean-flat-happy.png";
const THEME_BG: Rgba<u8> = Rgba([30, 30, 30, 255]);

/// Project automation for keynot.
#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    task: Task,
}

#[derive(Subcommand)]
enum Task {
    /// Regenerate the README screenshots in assets/screenshots/.
    ///
    /// Plays examples/tour.keynot inside a pty (answering the terminal
    /// probes keynot sends), captures each slide's final screen,
    /// renders it to PNG via asciinema's `agg`, and -- on slides that
    /// draw a picture -- replaces the chunky half-block cells with the
    /// real image, aspect-fit into the same region. Needs `agg` on
    /// PATH and a debug build of keynot.
    #[command(verbatim_doc_comment)]
    Screenshots {
        /// The 1-based tour slide numbers to capture
        #[arg(required = true)]
        slides: Vec<u16>,
    },
}

fn main() {
    match Cli::parse().task {
        Task::Screenshots { slides } => screenshots(&slides),
    }
}

fn screenshots(slides: &[u16]) {
    assert!(
        Path::new(KEYNOT).exists(),
        "{KEYNOT} not found; run `cargo build` first (or use `just screenshots`)"
    );
    fs_err::create_dir_all(OUT_DIR).unwrap();
    let real = fetch_image(IMAGE_URL);
    for &n in slides {
        let png = format!("{OUT_DIR}/slide-{n}.png");
        let raw = capture_slide(n);
        let mut shot = render_screen(&raw);
        let composited = composite_real_image(&mut shot, &real);
        save_png(&shot, &png);
        println!(
            "{png}{}",
            if composited {
                "  (real image composited)"
            } else {
                ""
            }
        );
    }
}

/// Run keynot on slide `n` in a pty, answering its terminal probes, and
/// return the raw bytes it writes.
fn capture_slide(n: u16) -> Vec<u8> {
    let pty = native_pty_system()
        .openpty(PtySize {
            rows: ROWS,
            cols: COLS,
            pixel_width: COLS * 10,
            pixel_height: ROWS * 20,
        })
        .unwrap();
    let mut cmd = CommandBuilder::new(fs_err::canonicalize(KEYNOT).unwrap());
    cmd.args([
        "play",
        "--images",
        "halfblocks",
        "--start-slide",
        &n.to_string(),
        TOUR,
    ]);
    cmd.cwd(std::env::current_dir().unwrap());
    cmd.env("TERM", "xterm-256color");
    let mut child = pty.slave.spawn_command(cmd).unwrap();
    drop(pty.slave);

    let mut reader = pty.master.try_clone_reader().unwrap();
    let mut writer = pty.master.take_writer().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = [0u8; 65536];
        while let Ok(n) = reader.read(&mut buf) {
            if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                break;
            }
        }
    });

    let mut out = Vec::new();
    let mut probes: Vec<(&[u8], &[u8])> = vec![
        (b"\x1b[c", b"\x1b[?62c"),       // device attributes
        (b"\x1b[16t", b"\x1b[6;20;10t"), // cell size 10x20 px
        (b"\x1b[5n", b"\x1b[0n"),        // device status
    ];
    let deadline = Instant::now() + Duration::from_millis(2500);
    while Instant::now() < deadline {
        if let Ok(chunk) = rx.recv_timeout(Duration::from_millis(100)) {
            out.extend_from_slice(&chunk);
            probes.retain(|(probe, reply)| {
                if contains(&out, probe) {
                    let _ = writer.write_all(reply);
                    let _ = writer.flush();
                    false
                } else {
                    true
                }
            });
        }
    }
    let _ = child.kill();
    out
}

/// Save with maximum compression; these are committed to the repo.
fn save_png(shot: &RgbaImage, path: &str) {
    let file = fs_err::File::create(path).unwrap();
    PngEncoder::new_with_quality(
        std::io::BufWriter::new(file),
        CompressionType::Best,
        FilterType::Adaptive,
    )
    .write_image(
        shot.as_raw(),
        shot.width(),
        shot.height(),
        ExtendedColorType::Rgba8,
    )
    .unwrap();
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// Raw pty bytes -> single-event asciinema cast -> agg GIF -> the last
/// (settled) frame as an image.
fn render_screen(raw: &[u8]) -> RgbaImage {
    let dir = std::env::temp_dir();
    // PID-unique names so concurrent invocations cannot collide.
    let cast = dir.join(format!("keynot-xtask-{}.cast", std::process::id()));
    let gif = dir.join(format!("keynot-xtask-{}.gif", std::process::id()));
    let header = serde_json::json!({"version": 2, "width": COLS, "height": ROWS});
    let event = serde_json::json!([0.1, "o", String::from_utf8_lossy(raw)]);
    fs_err::write(&cast, format!("{header}\n{event}\n")).unwrap();

    let status = Command::new("agg")
        .args(["--idle-time-limit", "1", "--font-size", "16"])
        .arg(&cast)
        .arg(&gif)
        .output()
        .expect("`agg` must be installed (https://github.com/asciinema/agg)");
    assert!(status.status.success(), "agg failed: {status:?}");

    let decoder =
        GifDecoder::new(std::io::BufReader::new(fs_err::File::open(&gif).unwrap())).unwrap();
    let frames = decoder.into_frames().collect_frames().unwrap();
    let last = frames.last().expect("gif has frames").buffer().clone();
    let _ = fs_err::remove_file(&cast);
    let _ = fs_err::remove_file(&gif);
    last
}

fn fetch_image(url: &str) -> DynamicImage {
    let bytes = ureq::get(url)
        .call()
        .expect("fetching the real image")
        .body_mut()
        .read_to_vec()
        .unwrap();
    image::load_from_memory(&bytes).unwrap()
}

/// Replace a half-block-rendered picture with `real`, if one exists.
///
/// This is a heuristic, not knowledge: nothing here asks keynot where
/// (or whether) a slide drew an image. Every screenshot is scanned for
/// pixels only half-block image cells can produce, and when some are
/// found, `real` is pasted over their bounding box. Baked-in
/// assumptions, which hold for today's tour:
///
/// - The tour shows exactly one picture, Ferris ([`IMAGE_URL`]); if a
///   second image slide is ever added, Ferris would be pasted over it.
/// - The picture sits in the right half of the screen (the scan skips
///   the left half to avoid false positives in text).
/// - Its cells are recognizable by color: Ferris's orange hues, plus
///   the pure black the half-block renderer paints for a PNG's
///   transparent pixels. True black occurs nowhere else -- the theme
///   background is (30,30,30) and text is lighter. An opaque,
///   non-orange image would go undetected and stay as half-blocks.
fn composite_real_image(shot: &mut RgbaImage, real: &DynamicImage) -> bool {
    let (w, h) = shot.dimensions();
    let is_image_cell = |p: &Rgba<u8>| {
        let [r, g, b, _] = p.0;
        (r < 15 && g < 15 && b < 15) || (r > 150 && g < 120 && b < 90 && r - g > 80)
    };

    let (mut min_x, mut min_y, mut max_x, mut max_y) = (u32::MAX, u32::MAX, 0, 0);
    for y in 0..h {
        for x in w / 2..w {
            if is_image_cell(shot.get_pixel(x, y)) {
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
            }
        }
    }
    if max_x == 0 {
        return false;
    }

    let margin = 4;
    for y in min_y.saturating_sub(margin)..(max_y + margin + 1).min(h) {
        for x in min_x.saturating_sub(margin)..(max_x + margin + 1).min(w) {
            shot.put_pixel(x, y, THEME_BG);
        }
    }

    let (bw, bh) = (max_x - min_x + 1, max_y - min_y + 1);
    let scale = f64::min(
        bw as f64 / real.width() as f64,
        bh as f64 / real.height() as f64,
    );
    let (fw, fh) = (
        (real.width() as f64 * scale) as u32,
        (real.height() as f64 * scale) as u32,
    );
    let fitted = imageops::resize(real, fw, fh, imageops::FilterType::Lanczos3);
    imageops::overlay(
        shot,
        &fitted,
        (min_x + (bw - fw) / 2) as i64,
        (min_y + (bh - fh) / 2) as i64,
    );
    true
}
