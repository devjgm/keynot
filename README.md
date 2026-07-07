# keynot

```text
________________
\    keynot    /
 \____________/
      |  |
      |  |
   ___|__|___
```

Terminal slide presentations from a single markdown file. Like Keynote,
but it's not, and it lives in your terminal, like you.

![keynot demo](https://raw.githubusercontent.com/devjgm/keynot/main/assets/demo.gif)

- One `.keynot` file per presentation: YAML frontmatter for metadata and
  theming, slides in plain markdown separated by `---`, HTML comments as
  speaker notes (a format in the spirit of [marp](https://marp.app))
- Renders headings, emphasis, lists, blockquotes, links, and
  syntax-highlighted code blocks with [ratatui](https://ratatui.rs)
- Real images in the terminal via
  [ratatui-image](https://github.com/ratatui/ratatui-image): kitty,
  iTerm2, and sixel graphics, with a half-block fallback everywhere
  else; local files or URLs
- Slide transitions via [tachyonfx](https://github.com/ratatui/tachyonfx)
- Outline overview for jumping around, in-show help, live reload

## Install

```sh
cargo install --locked keynot
```

## Quick start

```sh
keynot new talk.keynot     # write a skeleton presentation
keynot play talk.keynot    # present it
keynot check talk.keynot   # validate and summarize a file
```

Useful `play` flags:

```sh
keynot play --start-slide 7 talk.keynot   # resume at slide 7
```

During the show: left/right arrows or space to change slides, up/down to
highlight the line you are talking about (an accent bar by default; set
`highlight: dim` to dim everything else instead), `o` for the outline,
`!` to drop into a shell for a live demo (exit to resume), `r` to reload
the file after editing it, `?` for the full key list, `q` to quit.

## The format in 20 seconds

````markdown
---
title: My Talk
author: Ada
theme: dark
colors:
  accent: '#dcdcaa'
---

# My Talk

The title slide

<!-- speaker notes go in comments -->

---

## Second slide

- Markdown bullets, **bold**, *italic*, `code`, [links](https://example.com)

```rust
fn main() { println!("highlighted"); }
```
````

See [FORMAT.md](FORMAT.md) for the complete reference: all frontmatter
keys, theming and color values, transitions, supported markdown, and the
exact splitting rules.

## Transitions

Set `transition:` in the frontmatter to control how slides change. All
available values:

| value      | effect                                                      |
|------------|-------------------------------------------------------------|
| `slide`    | push: the old slide slides out, the new one slides in, in the direction of navigation (the default) |
| `coalesce` | characters dissolve into place                              |
| `fade`     | fade in from the background color                           |
| `sweep`    | wipe across in the direction of navigation                  |
| `none`     | instant switch                                              |

## Development

```sh
cargo test      # parser, renderer, and CLI tests
cargo clippy --all-targets
```

The crate is organized as a library plus a thin binary: `markdown/` parses
`.keynot` files into a small slide AST, `render/` turns the AST into
styled terminal text, `src/app/` is the interactive player, and `main.rs`
the CLI.

Formatting uses a nightly-only rustfmt option (`group_imports`), so
format with `cargo +nightly fmt`; CI checks it that way.
