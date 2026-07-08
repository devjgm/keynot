# Using keynot

How to run keynot: the subcommands, the keys during a show, and what
to expect on screen. The `.keynot` file format itself -- frontmatter,
theming, markdown support -- is documented in [FORMAT.md](FORMAT.md).

## The subcommands

```
keynot new talk.keynot      # write a skeleton presentation (the tour)
keynot play talk.keynot     # present it
keynot check talk.keynot    # validate and summarize a file
```

`keynot check` validates the frontmatter strictly (unknown keys are
errors, with line numbers), prints the deck's metadata and slide
count, and reports the tallest slide at a reference 80 columns. Run in
a terminal, it also says whether every slide fits that terminal's real
size. `keynot play` is forgiving where `check` is strict: unknown
frontmatter keys are ignored so a deck written for a newer keynot (or
a plain markdown file with foreign frontmatter) still opens.

### Shell completions

keynot completes itself: subcommands, flags, and flag values come from
the real CLI at runtime, so completions never go stale. Register once
with a one-line file (or rc line) that re-sources the registration
from whatever `keynot` is on your PATH -- it survives reinstalls and
upgrades untouched:

```sh
# fish: no rc change needed -- fish autoloads this file lazily
echo 'COMPLETE=fish keynot | source' > ~/.config/fish/completions/keynot.fish

# bash: also autoloaded, if the bash-completion package is installed
echo 'source <(COMPLETE=bash keynot)' > ~/.local/share/bash-completion/completions/keynot

# zsh: add to ~/.zshrc
source <(COMPLETE=zsh keynot)
```

Elvish and PowerShell work the same way via their `COMPLETE=` names.

For diagnostics, set `KEYNOT_LOG` to a tracing filter (e.g.
`KEYNOT_LOG=debug`) and keynot writes a `keynot.log` in the current
directory.

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
| right / left (while highlighting) | move the highlight between columns; past the slide's edge, change slides |
| esc                          | clear the line highlight  |
| `g` / `G`, home / end        | first / last slide        |
| `o`                          | toggle the outline overview |
| enter, space (in outline)    | jump to selected slide    |
| `0`-`9` (in outline)         | type a slide number to select it |
| esc (in outline)             | back without jumping (clears a typed number first) |
| `!`                          | open an interactive shell; exiting it resumes the show |
| `e`                          | edit the file in `$VISUAL`/`$EDITOR`; exiting reloads and resumes |
| `r`                          | reload the file from disk |
| `?`                          | toggle help               |
| `q`, ctrl-c                  | quit                      |

The `!` key suspends the presentation and starts your shell in the
normal terminal screen -- handy for live demos. On macOS and Linux this
is `$SHELL` (falling back to `/bin/sh`); on Windows it is `%COMSPEC%`
(usually cmd.exe). When the shell exits, the show resumes exactly where
it was.

Spot a typo mid-show? `e` suspends the presentation and opens the deck
in `$VISUAL` or `$EDITOR` (falling back to `vi`, or `notepad` on
Windows), jumping to the line you are looking at -- the highlighted
line's block, or the top of the scrolled view, or the slide's start --
for editors whose line syntax keynot knows: the vi/emacs family
(`+line`), VS Code and Sublime (fused `file:line`, with `--wait` added
so they block until closed), TextMate, and SubEthaEdit. Unrecognized
editors just get the file. When the editor exits, the show reloads from disk and
resumes right where you were: the highlight, and the scroll position
that follows it, survive the reload.

The line highlight is for speaking: press down to spotlight the first
line of the slide and keep pressing to walk through the lines you are
discussing. Blank lines are skipped. On multi-column slides the
highlight lives in one column at a time; left/right move it between
columns, and moving past the outermost column changes slides. Esc (or
changing slides) turns it off. The `highlight:` frontmatter key picks the look:

| value | effect                                                        |
|-------|---------------------------------------------------------------|
| `bar` | an accent-colored bar behind the line (the default)           |
| `dim` | the line keeps full brightness while everything else dims     |

Slides are vertically centered when they fit. A slide taller than the
terminal is clipped at the bottom, with a dim marker in the corner
counting the hidden lines. Walking the line highlight (down/up)
scrolls the view to follow the bar, so an overflowing slide is still
fully presentable; Esc (or changing slides) returns to the top. Images
draw only while fully in view. `keynot check` reports the tallest
slide at a reference 80 columns -- and, when run in a terminal, whether
every slide fits that terminal -- so none of this surprises you at show
time (`---` remains the pagination tool).

The outline lists every slide by its first heading (or first line of
text). Typing a number selects that slide live as you type -- for slide
12, type `1` then `2` then enter; arrows or esc clear the pending
number. `r` re-reads the file in place, so you can edit in another
window and refresh without restarting; parse errors show in the footer
and keep the current deck.
