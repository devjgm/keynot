//! End-to-end tests of the `keynot` CLI (everything except the interactive
//! player, which needs a real terminal).

use snapbox::cmd::Command;
use snapbox::str;

fn keynot() -> Command {
    Command::cargo_bin("keynot")
}

#[test]
fn new_creates_a_playable_skeleton() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("demo-talk.keynot");

    keynot()
        .arg("new")
        .arg(&file)
        .assert()
        .success()
        .stdout_eq(str![[r#"
Created [..]demo-talk.keynot
Play it with: keynot play [..]demo-talk.keynot

"#]]);

    // The title is derived from the file stem, dashes become spaces.
    let content = fs_err::read_to_string(&file).unwrap();
    assert!(content.contains("title: demo talk"));

    keynot()
        .arg("check")
        .arg(&file)
        .assert()
        .success()
        .stdout_eq(str![[r#"
[..]demo-talk.keynot: OK
  title:  demo talk
  author: Your Name
  theme:  dark
  slides: 6
  notes:  1
  tallest: slide 2, 14 lines (at 80 columns)

"#]]);
}

#[test]
fn new_refuses_to_overwrite() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("talk.keynot");
    fs_err::write(&file, "precious").unwrap();

    keynot()
        .arg("new")
        .arg(&file)
        .assert()
        .failure()
        .stderr_eq(str![[r#"
Error: [..]talk.keynot already exists (use --force to overwrite)
...
"#]]);
    assert_eq!(fs_err::read_to_string(&file).unwrap(), "precious");

    keynot()
        .args(["new", "--force"])
        .arg(&file)
        .assert()
        .success();
    assert_ne!(fs_err::read_to_string(&file).unwrap(), "precious");
}

#[test]
fn check_reports_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("t.keynot");
    fs_err::write(
        &file,
        "---\ntitle: My Talk\nauthor: Alice\n---\n# One\n\n<!-- note -->\n---\n# Two\n",
    )
    .unwrap();

    keynot()
        .arg("check")
        .arg(&file)
        .assert()
        .success()
        .stdout_eq(str![[r#"
[..]t.keynot: OK
  title:  My Talk
  author: Alice
  theme:  dark
  slides: 2
  notes:  1
  tallest: slide 1, 2 lines (at 80 columns)

"#]]);
}

#[test]
fn check_validates_the_shipped_example() {
    keynot()
        .arg("check")
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/examples/tour.keynot"))
        .assert()
        .success();
}

#[test]
fn keynot_log_writes_a_trace_file() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("t.keynot");
    fs_err::write(&file, "# One\n---\n# Two\n").unwrap();

    keynot()
        .current_dir(dir.path())
        .env("KEYNOT_LOG", "debug")
        .arg("check")
        .arg(&file)
        .assert()
        .success();

    let log = fs_err::read_to_string(dir.path().join("keynot.log")).unwrap();
    assert!(log.contains("keynot starting"), "log:\n{log}");
    assert!(log.contains("loaded presentation"), "log:\n{log}");
    assert!(log.contains("slides=2"), "log:\n{log}");
}

#[test]
fn no_log_file_without_the_env_var() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("t.keynot");
    fs_err::write(&file, "# One\n").unwrap();

    keynot()
        .current_dir(dir.path())
        .arg("check")
        .arg(&file)
        .assert()
        .success();
    assert!(!dir.path().join("keynot.log").exists());
}

#[test]
fn check_fails_on_bad_frontmatter() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("bad.keynot");
    fs_err::write(&file, "---\ntitle: [unclosed\n---\n# S\n").unwrap();

    keynot()
        .arg("check")
        .arg(&file)
        .assert()
        .failure()
        .stderr_eq(str![[r#"
Error: cannot parse [..]bad.keynot

Caused by:
   0: invalid frontmatter: [..]
...
"#]]);
}

#[test]
fn check_fails_on_unterminated_frontmatter() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("bad.keynot");
    fs_err::write(&file, "---\ntitle: T\n# never closed\n").unwrap();

    keynot()
        .arg("check")
        .arg(&file)
        .assert()
        .failure()
        .stderr_eq(str![[r#"
Error: cannot parse [..]bad.keynot

Caused by:
    unterminated frontmatter: the leading `---` needs a closing `---`
...
"#]]);
}

#[test]
fn check_fails_on_misspelled_frontmatter_key() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("t.keynot");
    fs_err::write(&file, "---\ntitle: T\ntranstion: fade\n---\n# S\n").unwrap();

    keynot()
        .arg("check")
        .arg(&file)
        .assert()
        .failure()
        .stderr_eq(str![[r#"
Error: unknown frontmatter key `transtion` (line 3) (valid keys: title, author, date, theme, colors, code_theme, code_style, transition, highlight, footer; colors: background, text, heading, accent, link, blockquote, code_background, code_border)
...
"#]]);
}

#[test]
fn check_reports_every_misspelled_key_at_once() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("t.keynot");
    fs_err::write(
        &file,
        "---\nhilight: dim\ntranstion: fade\ncolors:\n  backgroud: red\n---\n# S\n",
    )
    .unwrap();

    keynot()
        .arg("check")
        .arg(&file)
        .assert()
        .failure()
        .stderr_eq(str![[r#"
Error: unknown frontmatter keys `hilight` (line 2), `transtion` (line 3), `colors.backgroud` (line 5) (valid keys: [..])
...
"#]]);
}

#[test]
fn check_fails_on_unknown_theme() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("t.keynot");
    fs_err::write(&file, "---\ntheme: neon\n---\n# S\n").unwrap();

    keynot()
        .arg("check")
        .arg(&file)
        .assert()
        .failure()
        .stderr_eq(str![[r#"
Error: unknown theme `neon` (available: dark, light)
...
"#]]);
}

#[test]
fn check_fails_on_unknown_code_theme() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("t.keynot");
    fs_err::write(&file, "---\ncode_theme: nope\n---\n# S\n").unwrap();

    keynot()
        .arg("check")
        .arg(&file)
        .assert()
        .failure()
        .stderr_eq(str![[r#"
Error: unknown code_theme `nope` (available: [..])
...
"#]]);
}

#[test]
fn check_fails_on_unknown_highlight_style() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("t.keynot");
    fs_err::write(&file, "---\nhighlight: sparkles\n---\n# S\n").unwrap();

    keynot()
        .arg("check")
        .arg(&file)
        .assert()
        .failure()
        .stderr_eq(str![[r#"
Error: cannot parse [..]t.keynot

Caused by:
   0: invalid frontmatter: highlight: unknown variant `sparkles`, expected `bar` or `dim` at line 1 column 12
...
"#]]);
}

#[test]
fn check_fails_on_unknown_transition() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("t.keynot");
    fs_err::write(&file, "---\ntransition: spiral\n---\n# S\n").unwrap();

    keynot()
        .arg("check")
        .arg(&file)
        .assert()
        .failure()
        .stderr_eq(str![[r#"
Error: cannot parse [..]t.keynot

Caused by:
   0: invalid frontmatter: transition: unknown variant `spiral`, expected one of `coalesce`, `slide`, `fade`, `sweep`, `none` at line 1 column 13
...
"#]]);
}

#[test]
fn check_fails_on_empty_file() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("empty.keynot");
    fs_err::write(&file, "").unwrap();

    keynot()
        .arg("check")
        .arg(&file)
        .assert()
        .failure()
        .stderr_eq(str![[r#"
Error: cannot parse [..]empty.keynot

Caused by:
    the file contains no slides
...
"#]]);
}

#[test]
fn play_fails_cleanly_on_missing_file() {
    keynot()
        .args(["play", "/no/such/file.keynot"])
        .assert()
        .failure()
        .stderr_eq(str![[r#"
Error: cannot read /no/such/file.keynot

Caused by:
    [..]
...
"#]]);
}

#[test]
fn play_rejects_non_numeric_start_slide() {
    keynot()
        .args(["play", "--start-slide", "abc", "x.keynot"])
        .assert()
        .code(2);
}

#[test]
fn help_lists_subcommands() {
    keynot()
        .arg("--help")
        .assert()
        .success()
        .stdout_eq(str![[r#"
Terminal slide presentations from markdown

    keynot new my-talk.keynot  # Then edit the markdown in the file
    keynot play my-talk.keynot

Usage: keynot[EXE] <COMMAND>

Commands:
  play   Play a presentation
  new    Create a new skeleton presentation
  check  Validate a presentation and print a summary
  help   Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help (see a summary with '-h')

  -V, --version
          Print version

"#]]);
}
