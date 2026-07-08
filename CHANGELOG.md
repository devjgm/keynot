# Changelog

Notable changes to keynot, newest first. The format follows
[Keep a Changelog](https://keepachangelog.com); versions follow
semantic versioning.

## [Unreleased]

### Added

- GFM tables render with rounded borders, bold headers, and per-column
  alignment; columns size to their content and word-wrap when the
  table is wider than the slide.
- Overflowing slides scroll: on a slide taller than the terminal,
  walking the line highlight (down/up) moves the view to follow the
  bar, with dim markers showing how many lines are hidden past each
  edge. Esc (or changing slides) returns to the top.
- `keynot check` reports the tallest slide and its height at a
  reference 80 columns; run in a terminal, it also says whether every
  slide fits that terminal's real size.

## [0.3.0] - 2026-07-08

### Added

- Background gradients: `colors.background` accepts
  `{ gradient: ['#hex', ...], direction: vertical | horizontal | radial }`
  in addition to a solid color. The dark theme's default background is
  now a slate gradient (`#2d2d30` down to `#181818`).
- Code blocks render as small terminal windows: a rounded border with
  traffic-light dots, the language name in the bottom edge, and a panel
  darker than any background. `code_style: plain` restores the bare
  panel; the frame color is themeable as `colors.code_border`.
- The speaker line highlight is column-aware: down/up walk lines within
  a column, left/right move between columns, and moving past the
  slide's edge changes slides (so single-column decks behave exactly as
  before).
- `keynot check` errors on unknown frontmatter keys, listing each with
  its line number and the valid keys; `keynot play` ignores them so a
  deck written for a newer keynot still opens on an older one.
- Error reports are colorized on a terminal (respecting `NO_COLOR`);
  piped output stays plain.
- FORMAT.md documents every frontmatter default, enforced by tests that
  fail when the docs drift from the code.

### Changed

- The dark theme's code panel darkened to `#141414` so it stands out
  against the gradient background.

### Fixed

- Reloading with `r` no longer freezes the show while URL images
  re-fetch; decodes happen on a worker thread and the network is never
  touched from the draw path.
- A highlight begun during a slide transition no longer lands on an
  arbitrary line of the incoming slide.

## [0.2.0] - 2026-07-08

### Added

- Multi-column slides: `|||` on its own line splits a slide into
  side-by-side columns (fence-aware, like `---`).
- `play --images auto|halfblocks|off` to control how pictures draw;
  half-blocks survive asciinema recordings.
- `KEYNOT_LOG` writes tracing diagnostics (graphics probe, image
  fetches, render timings) to `keynot.log`.

### Changed

- The default slide transition is `coalesce` (was the `slide` push).
- The tour slimmed to five slides that showcase columns throughout.

### Fixed

- Images in a column draw inside their column instead of centering
  across the whole slide.
- kitty-protocol images no longer vanish when a transition effect
  overwrites the cell carrying the image transmission.

## [0.1.0] - 2026-07-08

Initial release: markdown slides with YAML frontmatter, marp-style
`---` separators, and speaker-note comments; VS Code Dark+ and light
themes with per-color overrides; syntax-highlighted code; images via
kitty/iTerm2/sixel/half-blocks, from files or URLs; slide transitions;
a speaker line highlight; an outline view with number jumping; a shell
escape; and live reload.

[Unreleased]: https://github.com/devjgm/keynot/compare/v0.3.0...HEAD
[0.3.0]: https://github.com/devjgm/keynot/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/devjgm/keynot/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/devjgm/keynot/releases/tag/v0.1.0
