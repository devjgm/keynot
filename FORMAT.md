# The keynot file format

A keynot presentation is a single markdown file, conventionally named with
a `.keynot` extension. The format is a small superset of CommonMark,
inspired by [marp](https://marp.app): frontmatter on top, slides separated
by `---`, comments for speaker notes.

A minimal complete example:

```markdown
---
title: Why Terminals Are Enough
author: Ada
theme: dark
---

# Why Terminals Are Enough

An introduction

<!-- Open with the anecdote about the 1978 VT100. -->

---

## Agenda

- History
- **The good parts**
- Questions
```

Run it with `keynot play talk.keynot`. Create this skeleton with
`keynot new talk.keynot`, and validate a file with `keynot check talk.keynot`.

## File structure

A file has two parts, both optional in principle (but you need at least one
non-empty slide):

1. **Frontmatter**: a YAML block delimited by `---` lines, which must start
   on the very first line of the file.
2. **Slides**: markdown, separated by lines containing only `---`, each
   optionally split into side-by-side columns by lines containing only
   `|||` (see [Columns](#columns)).

### Frontmatter

The frontmatter configures the whole presentation. It must begin on line 1;
a `---` anywhere else is a slide separator. The block is closed by `---`
(or YAML's `...`). All keys are optional, and unknown keys are ignored so
files keep working across keynot versions.

```yaml
---
title: My Talk            # shown in the footer; defaults to the first heading
author: Ada Lovelace      # shown in the footer
date: 2026-07-07          # free-form text, shown in the footer
theme: dark               # base theme: dark (default) or light
transition: coalesce      # coalesce (default), slide, fade, sweep, or none
highlight: bar            # speaker line highlight: bar (default) or dim
code_theme: base16-eighties.dark   # syntect theme for code blocks
footer: true              # set false to hide the footer entirely
colors:                   # per-element overrides, applied on top of the theme
  background: '#1e1e1e'
  text: '#d4d4d4'
  heading: '#569cd6'
  accent: '#dcdcaa'
  link: '#3794ff'
  blockquote: '#6a9955'
  code_background: '#252526'
---
```

Notes:

- An empty frontmatter (`---` immediately followed by `---`) is valid.
- A `---` on line 1 that is never closed is an error ("unterminated
  frontmatter"). If your first slide needs to start with a horizontal
  rule, add an empty frontmatter first.

#### Theming

`theme` picks the base palette. Two are built in:

| theme   | look                                                            |
|---------|-----------------------------------------------------------------|
| `dark`  | VS Code Dark+: charcoal background, blue headings, yellow accents (the default) |
| `light` | near-white background, dark text                                |

`default` is accepted as an alias for `dark`.

Every color can then be overridden individually under `colors:`. Color
values accept:

- hex strings: `'#ff8800'` (quote them; `#` starts a YAML comment)
- ANSI color names: `red`, `lightcyan`, `gray`, ...
- 256-color palette indexes: `'42'`

What each color controls:

| key               | controls                                        |
|-------------------|-------------------------------------------------|
| `background`      | the whole screen                                |
| `text`            | body text                                       |
| `heading`         | `#` and `##` headings                           |
| `accent`          | bullets, inline code, rules, UI highlights      |
| `link`            | link text                                       |
| `blockquote`      | the `|` bar in front of quotes                  |
| `code_background` | background of code blocks and inline code       |

`code_theme` selects the syntax-highlighting palette for fenced code
blocks. `Dark+` (the dark default) ships with keynot; any theme bundled
with [syntect](https://github.com/trishume/syntect) also works: `base16-eighties.dark`, `base16-ocean.dark`, `base16-mocha.dark`,
`base16-ocean.light`, `InspiredGitHub` (light default),
`Solarized (dark)`, and `Solarized (light)`. A typo here is caught by
`keynot check`, which lists the valid names.

#### Transitions

`transition` sets the effect when changing slides:

| value      | effect                                                      |
|------------|-------------------------------------------------------------|
| `coalesce` | characters dissolve into place (the default)                |
| `slide`    | push: the old slide slides out, the new one slides in, in the direction of navigation |
| `fade`     | fade in from the background color                           |
| `sweep`    | wipe across in the direction of navigation                  |
| `none`     | instant switch                                              |

## Slides

Slides are separated by a line containing only `---` (surrounding
whitespace is allowed). The separator must be exactly three dashes;
`----` is not a separator.

Two things to know about `---`:

- Inside a fenced code block, `---` is code, not a separator. Both
  backtick and tilde fences are tracked, and a longer fence (four or more
  backticks) can wrap a shorter one, exactly as in CommonMark.
- Because `---` always splits slides, markdown's "setext" heading style
  (a title underlined with dashes) is not available. Use `#` headings.

Slides containing only whitespace are dropped, so a trailing `---` at the
end of the file does not create an empty slide.

### Columns

A line containing only `|||` splits the current slide into side-by-side
columns -- a keynot extension that most markdown slide formats can only
approximate with layout hacks:

````markdown
## Comparison

The left column.

- Anything can go in a column: lists,
  quotes, code, even images.

|||

The right column.

```rust
fn main() {
    println!("code on the right");
}
```
````

How columns behave:

- `|||` follows exactly the same rules as `---`: it must be alone on its
  line (surrounding whitespace allowed), and it never splits inside a
  fenced code block.
- Every `|||` adds one more column; two separators make three columns.
- Columns share the slide width equally, separated by a small gutter,
  and are laid out top-aligned; the tallest column determines the
  slide's height for vertical centering.
- Each column is its own little slide: text wraps to the column width,
  code blocks clip to it, and images center within their column.
- A column containing only whitespace is dropped (and a slide whose
  columns are all blank is dropped entirely).
- There is no shared full-width region: a heading belongs to whichever
  column it is written in. Put it in the first column for a
  title-on-the-left look.
- The speaker line highlight (up/down) operates on whole rows, spanning
  all columns.

## Comments and speaker notes

HTML comments never render. Use them for speaker notes or to stash
material:

```markdown
## Results

Revenue tripled.

<!--
Pause here.
Mention that the Q3 numbers are preliminary.
-->
```

Comments work inline too (`before <!-- hidden --> after`) and may span
multiple lines. `keynot check` reports how many notes a file contains.

## Supported markdown

### Headings

```markdown
# Title          -> bold, heading color, underlined with a rule
## Section       -> bold, heading color
### Subsection   -> bold, text color (levels 3-6 look the same)
```

A `#` heading works well as the single element of a title slide; every
slide is centered vertically.

### Inline styles

| write                        | get                          |
|------------------------------|------------------------------|
| `**bold**`                   | bold                         |
| `*italic*` or `_italic_`     | italic                       |
| `~~strikethrough~~`          | strikethrough                |
| `<u>underline</u>`           | underline (HTML tag; markdown has none) |
| `` `inline code` ``          | accent color on code background |
| `[text](https://url)`        | underlined text in link color, followed by the URL in dim parentheses |
| `line one<br>line two`       | forced line break            |

Styles nest: `***bold italic***`, ``**bold with `code`**``, and so on.
Long lines wrap to the slide width at word boundaries.

The URL is shown after link text because terminals can only open URLs
they can see (most terminals make visible URLs clickable, e.g. with
cmd-click). Autolinks like `<https://url>` show just the URL once.

### Lists

```markdown
- unordered items use `-`, `*`, or `+`
- nesting works
  - indent by two spaces
  1. ordered lists too
  2. numbering continues automatically
- [x] task list items render their checkbox as the marker
```

Ordered lists respect their starting number (`4.` starts at 4). Wrapped
list items get a hanging indent under their marker. Task items (`- [x]` /
`- [ ]`) drop the bullet and show the checkbox itself as the marker:
checked boxes in bold accent, unchecked ones dimmed.

### Code blocks

Fenced code blocks are syntax highlighted. Put the language after the
opening fence:

````markdown
```rust
fn main() {
    println!("hello");
}
```
````

The language token can be a name (`rust`, `python`) or a file extension
(`rs`, `py`); anything syntect recognizes works. Unknown languages render
as plain text. Code is never re-wrapped: lines wider than the slide are
clipped, so format your snippets for the room.

### Blockquotes

```markdown
> Quoted text renders italic behind a colored bar.
> > Quotes can nest.
```

### Horizontal rules

`***` or `___` draws a rule across the slide. (`---` cannot be used for
this; it separates slides.)

### Images

An image alone in its paragraph renders as a real picture:

```markdown
![diagram of the pipeline](figures/pipeline.png)

![Ferris](https://rustacean.net/assets/rustacean-flat-happy.png)
```

Relative paths resolve against the `.keynot` file's directory. `http://`
and `https://` URLs are fetched (with a 10 second timeout) when the
presentation starts, so a slow network can delay startup but never a
slide change; a failed fetch renders the placeholder. If your talk must
survive without wifi, download the image and use a local path. keynot
probes the terminal at startup and picks the best protocol it supports:
kitty graphics (kitty, WezTerm, Ghostty, ...), iTerm2 inline images,
sixel, or colored half-block cells as a universal fallback. Images are
scaled down (never up) to fit the slide, keeping aspect ratio, and are
centered horizontally.

Images mixed into a line of text, or nested inside lists and quotes,
render as an italic `[image: alt text]` placeholder instead. The same
placeholder appears when the file cannot be read.

### Not supported (yet)

Tables, footnotes, and math render as plain text or are dropped. Raw HTML
other than `<u>`, `<br>`, and comments is ignored (its text content still
renders).

## Playing a presentation

```
keynot play talk.keynot                     # start at slide 1
keynot play --start-slide 7 talk.keynot     # resume where you left off
keynot play --images halfblocks talk.keynot # textual images (recordings)
```

`--images halfblocks` draws pictures as colored half-block cells instead
of the terminal's native graphics protocol. Native graphics look better
live, but only half-blocks survive asciinema recordings and GIF renders;
`--images off` shows placeholders instead.

Keys during the show (press `?` anytime for this list):

| key                          | action                    |
|------------------------------|---------------------------|
| right, space, `l`, `n`, page down | next slide           |
| left, backspace, `h`, `p`, page up | previous slide      |
| down / up, `j` / `k`         | highlight the next / previous line |
| esc                          | clear the line highlight  |
| `g` / `G`, home / end        | first / last slide        |
| `o`                          | toggle the outline overview |
| enter, space (in outline)    | jump to selected slide    |
| `0`-`9` (in outline)         | type a slide number to select it |
| esc (in outline)             | back without jumping (clears a typed number first) |
| `!`                          | open an interactive shell; exiting it resumes the show |
| `r`                          | reload the file from disk |
| `?`                          | toggle help               |
| `q`, ctrl-c                  | quit                      |

The `!` key suspends the presentation and starts your shell in the
normal terminal screen -- handy for live demos. On macOS and Linux this
is `$SHELL` (falling back to `/bin/sh`); on Windows it is `%COMSPEC%`
(usually cmd.exe). When the shell exits, the show resumes exactly where
it was.

The line highlight is for speaking: press down to spotlight the first
line of the slide and keep pressing to walk through the lines you are
discussing. Blank lines are skipped. Esc (or changing slides) turns it
off. The `highlight:` frontmatter key picks the look:

| value | effect                                                        |
|-------|---------------------------------------------------------------|
| `bar` | an accent-colored bar behind the line (the default)           |
| `dim` | the line keeps full brightness while everything else dims     |

The outline lists every slide by its first heading (or first line of
text). Typing a number selects that slide live as you type -- for slide
12, type `1` then `2` then enter; arrows or esc clear the pending
number. `r` re-reads the file in place, so you can edit in another
window and refresh without restarting; parse errors show in the footer
and keep the current deck.
