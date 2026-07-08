//! End-to-end tests of the player against a fake terminal.
//!
//! Each test runs the real `keynot` binary inside a pty and scripts the
//! terminal side of the conversation: answering the probes keynot sends
//! (device attributes, cell size, and optionally the kitty graphics
//! query) exactly as a real terminal would. That exercises the layer no
//! in-process test can reach -- the escape-sequence stream a terminal
//! actually receives: alternate-screen handling, graphics protocol
//! negotiation, and where image cells really land on screen.
#![cfg(unix)]

use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The fake terminal's geometry: 100x30 cells of 10x20 px.
const COLS: u16 = 100;
const ROWS: u16 = 30;

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// keynot running inside a scripted pty.
struct FakeTerminal {
    output: Arc<Mutex<Vec<u8>>>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    // Keeps the pty open for the lifetime of the test.
    _master: Box<dyn portable_pty::MasterPty + Send>,
}

impl FakeTerminal {
    /// Spawn `keynot <args>` in `dir`. When `kitty` is true the fake
    /// terminal claims kitty graphics support, as Ghostty or kitty would;
    /// otherwise it only reports a cell size (enough for halfblocks).
    fn spawn(dir: &Path, args: &[&str], kitty: bool) -> Self {
        let pty = native_pty_system()
            .openpty(PtySize {
                rows: ROWS,
                cols: COLS,
                pixel_width: COLS * 10,
                pixel_height: ROWS * 20,
            })
            .unwrap();
        let mut cmd = CommandBuilder::new(env!("CARGO_BIN_EXE_keynot"));
        cmd.args(args);
        cmd.cwd(dir);
        cmd.env("TERM", "xterm-256color");
        let child = pty.slave.spawn_command(cmd).unwrap();
        drop(pty.slave);

        let mut reader = pty.master.try_clone_reader().unwrap();
        let writer: Arc<Mutex<Box<dyn Write + Send>>> =
            Arc::new(Mutex::new(pty.master.take_writer().unwrap()));
        let output = Arc::new(Mutex::new(Vec::new()));

        // The terminal side: accumulate everything keynot writes and
        // answer each probe once.
        let reply_output = Arc::clone(&output);
        let reply_writer = Arc::clone(&writer);
        std::thread::spawn(move || {
            let mut probes: Vec<(&[u8], &[u8])> = vec![
                (b"\x1b[c", b"\x1b[?62c"),       // primary device attributes
                (b"\x1b[16t", b"\x1b[6;20;10t"), // cell size: 20px by 10px
                (b"\x1b[5n", b"\x1b[0n"),        // device status report
            ];
            if kitty {
                probes.insert(0, (b"\x1b_Gi=31", b"\x1b_Gi=31;OK\x1b\\"));
            }
            let mut buf = [0u8; 65536];
            loop {
                let n = match reader.read(&mut buf) {
                    Ok(0) | Err(_) => return,
                    Ok(n) => n,
                };
                let out = {
                    let mut out = reply_output.lock().unwrap();
                    out.extend_from_slice(&buf[..n]);
                    out.clone()
                };
                probes.retain(|(probe, reply)| {
                    if contains(&out, probe) {
                        let mut w = reply_writer.lock().unwrap();
                        let _ = w.write_all(reply);
                        let _ = w.flush();
                        false
                    } else {
                        true
                    }
                });
            }
        });

        FakeTerminal {
            output,
            writer,
            child,
            _master: pty.master,
        }
    }

    fn output(&self) -> Vec<u8> {
        self.output.lock().unwrap().clone()
    }

    /// Wait until keynot has written `needle`, panicking after 30s (CI
    /// runners can be slow to cold-start the binary).
    fn wait_for(&self, needle: &[u8], what: &str) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            if contains(&self.output(), needle) {
                return;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        panic!(
            "timed out waiting for {what}; got {} bytes:\n{}",
            self.output().len(),
            String::from_utf8_lossy(&self.output())
        );
    }

    /// Type `bytes` into keynot's stdin.
    fn send(&self, bytes: &[u8]) {
        let mut w = self.writer.lock().unwrap();
        let _ = w.write_all(bytes);
        let _ = w.flush();
    }

    /// Send `q`, wait for a clean exit, and return everything written.
    fn quit(mut self) -> Vec<u8> {
        {
            let mut w = self.writer.lock().unwrap();
            let _ = w.write_all(b"q");
            let _ = w.flush();
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match self.child.try_wait() {
                Ok(Some(status)) => {
                    assert!(status.success(), "keynot exited with {status:?}");
                    break;
                }
                _ if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(25)),
                _ => {
                    let _ = self.child.kill();
                    panic!(
                        "keynot did not exit after q:\n{}",
                        String::from_utf8_lossy(&self.output())
                    );
                }
            }
        }
        // Give the reader thread a beat to drain the final bytes.
        std::thread::sleep(Duration::from_millis(100));
        self.output()
    }
}

/// A deck with text in column 1 and a local (network-free) red image in
/// column 2: 300x200 px, so 30x10 cells at the fake terminal's font.
fn write_deck(dir: &Path) {
    let red = image::RgbImage::from_pixel(300, 200, image::Rgb([255, 0, 0]));
    red.save(dir.join("red.png")).unwrap();
    fs_err::write(dir.join("deck.keynot"), "left text\n|||\n![p](red.png)\n").unwrap();
}

/// The pure-red SGR the all-red test image produces (halfblock cells
/// where both pixels are red carry red as both foreground and
/// background; nothing else in the theme is pure red).
const RED: &str = "38;2;255;0;0";

/// 1-based screen columns holding image cells, recovered from the raw
/// stream: track cursor-position sequences, count visible characters
/// (SGR styling is zero-width), record columns drawn while the current
/// style is the test image's red.
fn image_cell_columns(out: &[u8]) -> Vec<u16> {
    let text = String::from_utf8_lossy(out);
    let mut cols = Vec::new();
    let mut chars = text.chars().peekable();
    let mut position: Option<u16> = None;
    let mut advance = 0u16;
    let mut red = false;
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() != Some(&'[') {
                position = None;
                continue;
            }
            chars.next();
            let mut params = String::new();
            let mut terminator = ' ';
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() {
                    terminator = c;
                    break;
                }
                params.push(c);
            }
            match terminator {
                // Cursor position: row;col
                'H' => {
                    position = params.split_once(';').and_then(|(_, col)| col.parse().ok());
                    advance = 0;
                }
                // Styling changes no cell, but note the color.
                'm' => red = params.contains(RED),
                _ => position = None,
            }
        } else if let Some(col) = position {
            if red {
                cols.push(col + advance);
            }
            advance += 1;
        }
    }
    cols.sort_unstable();
    cols.dedup();
    cols
}

/// The kitty command parameters (the part before `;`) of every graphics
/// escape sequence in the stream.
fn kitty_commands(out: &[u8]) -> Vec<String> {
    let mut commands = Vec::new();
    let mut rest = out;
    while let Some(start) = rest.windows(3).position(|w| w == b"\x1b_G").map(|i| i + 3) {
        let body = &rest[start..];
        let end = body
            .windows(2)
            .position(|w| w == b"\x1b\\")
            .unwrap_or(body.len());
        let params = body[..end].split(|&b| b == b';').next().unwrap_or(&[]);
        commands.push(String::from_utf8_lossy(params).into_owned());
        rest = &body[end..];
    }
    commands
}

#[test]
fn halfblock_images_draw_inside_their_column() {
    let dir = tempfile::tempdir().unwrap();
    write_deck(dir.path());
    let term = FakeTerminal::spawn(
        dir.path(),
        &["play", "--images", "halfblocks", "deck.keynot"],
        false,
    );
    term.wait_for(RED.as_bytes(), "halfblock image cells");
    let out = term.quit();

    // The slide text made it to the screen alongside the image.
    assert!(contains(&out, b"left text"));

    // Column 2 of the 100-col terminal starts past the middle; an image
    // drawn slide-centered (the placement regression) would start ~30.
    let cols = image_cell_columns(&out);
    assert!(!cols.is_empty(), "found no image cells");
    let first = *cols.first().unwrap();
    assert!(
        first > 45,
        "image must start in the second column, not at {first} (all: {cols:?})"
    );
}

#[test]
fn kitty_terminals_get_one_virtual_placement() {
    let dir = tempfile::tempdir().unwrap();
    write_deck(dir.path());
    let term = FakeTerminal::spawn(dir.path(), &["play", "deck.keynot"], true);
    term.wait_for(
        "\u{10EEEE}".to_string().as_bytes(),
        "kitty placeholder cells",
    );
    let out = term.quit();

    let commands = kitty_commands(&out);
    let transmits: Vec<&String> = commands.iter().filter(|c| c.contains("a=T")).collect();
    assert_eq!(
        transmits.len(),
        1,
        "the image is transmitted exactly once: {commands:?}"
    );
    assert!(
        transmits[0].contains("U=1"),
        "transmitted as a virtual placement for placeholder cells: {transmits:?}"
    );
    // And no halfblock fallback cells.
    assert!(!contains(&out, RED.as_bytes()));
}

#[test]
fn kitty_transmission_survives_a_transition() {
    // Regression test: the kitty protocol packs its whole payload --
    // including a transmit-once escape -- into single buffer cells. A
    // transition effect (coalesce is the default) rewriting those cells
    // on the arrival frame used to swallow the transmission, leaving a
    // permanently blank image. Images must be drawn after effects.
    let dir = tempfile::tempdir().unwrap();
    write_deck(dir.path());
    fs_err::write(
        dir.path().join("two.keynot"),
        "# One\n---\nleft text\n|||\n![p](red.png)\n",
    )
    .unwrap();
    let term = FakeTerminal::spawn(dir.path(), &["play", "two.keynot"], true);
    term.wait_for(b"One", "the first slide");

    term.send(b"\x1b[C"); // right arrow: coalesce into the image slide
    term.wait_for(
        "\u{10EEEE}".to_string().as_bytes(),
        "kitty placeholder cells",
    );
    // Let the ~220ms transition finish so every animation frame is in
    // the capture, then check the stream.
    std::thread::sleep(Duration::from_millis(500));
    let out = term.quit();

    let commands = kitty_commands(&out);
    let transmits: Vec<&String> = commands.iter().filter(|c| c.contains("a=T")).collect();
    assert_eq!(
        transmits.len(),
        1,
        "the transmission reaches the terminal exactly once: {commands:?}"
    );
    assert!(transmits[0].contains("U=1"), "{transmits:?}");
}

#[test]
fn quitting_restores_the_terminal() {
    let dir = tempfile::tempdir().unwrap();
    write_deck(dir.path());
    let term = FakeTerminal::spawn(dir.path(), &["play", "deck.keynot"], false);
    term.wait_for(b"left text", "the slide");
    let out = term.quit();

    assert!(
        contains(&out, b"\x1b[?1049h"),
        "enters the alternate screen"
    );
    assert!(
        contains(&out, b"\x1b[?1049l"),
        "leaves the alternate screen"
    );
    assert!(contains(&out, b"\x1b[?25h"), "shows the cursor again");
}
