//! Style-preserving word wrap for ratatui spans.
//!
//! ratatui's `Paragraph` can wrap, but we pre-wrap so that vertical
//! centering, hanging indents for list items, and per-line prefixes (quote
//! bars) all know the real line count up front.

use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthChar;

/// Wrap styled spans to `width` columns. A `"\n"` inside span text forces a
/// line break. Words longer than the width are hard-split. Returns at least
/// one (possibly empty) line.
pub fn wrap_spans(spans: Vec<Span<'static>>, width: usize) -> Vec<Line<'static>> {
    let width = width.max(1);
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut current: Vec<Span<'static>> = Vec::new();
    let mut current_width = 0usize;

    let mut flush = |current: &mut Vec<Span<'static>>, current_width: &mut usize| {
        trim_trailing_space(current);
        lines.push(Line::from(std::mem::take(current)));
        *current_width = 0;
    };

    for span in spans {
        for token in tokenize(&span.content) {
            match token {
                Token::Newline => flush(&mut current, &mut current_width),
                Token::Space(text) => {
                    // Never start a wrapped line with whitespace.
                    if current_width == 0 {
                        continue;
                    }
                    let w = display_width(text);
                    if current_width + w <= width {
                        push_span(&mut current, Span::styled(text.to_string(), span.style));
                        current_width += w;
                    }
                    // A space that does not fit is simply dropped; the
                    // upcoming word will wrap.
                }
                Token::Word(text) => {
                    let w = display_width(text);
                    if current_width + w > width && current_width > 0 {
                        flush(&mut current, &mut current_width);
                    }
                    if w <= width && current_width + w <= width {
                        push_span(&mut current, Span::styled(text.to_string(), span.style));
                        current_width += w;
                        continue;
                    }
                    // Word longer than the whole line: hard-split by chars.
                    let mut chunk = String::new();
                    let mut chunk_width = 0;
                    for ch in text.chars() {
                        let cw = ch.width().unwrap_or(0);
                        if current_width + chunk_width + cw > width && !chunk.is_empty() {
                            push_span(&mut current, Span::styled(chunk.clone(), span.style));
                            chunk.clear();
                            chunk_width = 0;
                            flush(&mut current, &mut current_width);
                        }
                        chunk.push(ch);
                        chunk_width += cw;
                    }
                    if !chunk.is_empty() {
                        push_span(&mut current, Span::styled(chunk, span.style));
                        current_width += chunk_width;
                    }
                }
            }
        }
    }
    flush(&mut current, &mut current_width);

    // The final flush always adds one line; drop it only if it is empty and
    // there are earlier lines (i.e. trailing newline artifacts).
    if lines.len() > 1 && lines.last().is_some_and(|l| l.width() == 0) {
        lines.pop();
    }
    lines
}

/// Split spans at display column `at`: everything left of it, and
/// everything at or right of it. A wide character straddling the
/// boundary goes right, with a pad space keeping the left side's width.
pub fn split_spans_at(
    spans: Vec<Span<'static>>,
    at: usize,
) -> (Vec<Span<'static>>, Vec<Span<'static>>) {
    let mut left = Vec::new();
    let mut right = Vec::new();
    let mut width = 0usize;
    for span in spans {
        let span_width = display_width(&span.content);
        if width + span_width <= at && right.is_empty() {
            width += span_width;
            left.push(span);
        } else if width >= at || !right.is_empty() {
            right.push(span);
        } else {
            let mut head = String::new();
            let mut tail = String::new();
            for ch in span.content.chars() {
                let ch_width = ch.width().unwrap_or(0);
                if width + ch_width <= at && tail.is_empty() {
                    head.push(ch);
                    width += ch_width;
                } else {
                    tail.push(ch);
                }
            }
            if width < at {
                head.push_str(&" ".repeat(at - width));
                width = at;
            }
            if !head.is_empty() {
                left.push(Span::styled(head, span.style));
            }
            if !tail.is_empty() {
                right.push(Span::styled(tail, span.style));
            }
        }
    }
    (left, right)
}

/// Total display width of styled spans.
pub fn spans_width(spans: &[Span]) -> usize {
    spans.iter().map(|s| display_width(&s.content)).sum()
}

fn display_width(text: &str) -> usize {
    text.chars().map(|c| c.width().unwrap_or(0)).sum()
}

fn push_span(current: &mut Vec<Span<'static>>, span: Span<'static>) {
    if let Some(last) = current.last_mut()
        && last.style == span.style
    {
        last.content.to_mut().push_str(&span.content);
        return;
    }
    current.push(span);
}

fn trim_trailing_space(current: &mut Vec<Span<'static>>) {
    while let Some(last) = current.last_mut() {
        let trimmed = last.content.trim_end();
        if trimmed.len() == last.content.len() {
            break;
        }
        if trimmed.is_empty() {
            current.pop();
        } else {
            last.content = trimmed.to_string().into();
            break;
        }
    }
}

enum Token<'a> {
    Word(&'a str),
    Space(&'a str),
    Newline,
}

/// Split text into words, runs of spaces, and explicit newlines.
fn tokenize(text: &str) -> Vec<Token<'_>> {
    let mut tokens = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix('\n') {
            tokens.push(Token::Newline);
            rest = after;
            continue;
        }
        let is_space = rest.starts_with(' ');
        let end = rest
            .find(|c: char| (c == ' ') != is_space || c == '\n')
            .unwrap_or(rest.len());
        let (chunk, after) = rest.split_at(end);
        tokens.push(if is_space {
            Token::Space(chunk)
        } else {
            Token::Word(chunk)
        });
        rest = after;
    }
    tokens
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_at_zero_puts_everything_right() {
        let (l, r) = split_spans_at(vec![Span::raw("abc")], 0);
        assert!(l.is_empty());
        assert_eq!(r[0].content, "abc");
    }

    #[test]
    fn split_at_exact_span_boundary() {
        let spans = vec![Span::raw("ab"), Span::raw("cd")];
        let (l, r) = split_spans_at(spans, 2);
        assert_eq!(l.len(), 1);
        assert_eq!(l[0].content, "ab");
        assert_eq!(r[0].content, "cd");
    }

    #[test]
    fn split_mid_span_preserves_style() {
        use ratatui::style::Style;
        let spans = vec![Span::styled("abcd", Style::new().bold())];
        let (l, r) = split_spans_at(spans, 3);
        assert_eq!(l[0].content, "abc");
        assert_eq!(r[0].content, "d");
        assert_eq!(l[0].style, r[0].style, "style survives the split");
    }

    #[test]
    fn split_beyond_the_end_leaves_right_empty() {
        let (l, r) = split_spans_at(vec![Span::raw("ab")], 10);
        assert_eq!(l[0].content, "ab");
        assert!(r.is_empty());
    }

    #[test]
    fn wide_char_straddling_the_boundary_goes_right_with_a_pad() {
        // "a" (1 cell) + CJK (2 cells), split at 2: the wide char cannot
        // be halved, so it moves right and a space keeps the left width.
        let (l, r) = split_spans_at(vec![Span::raw("a\u{4e16}b")], 2);
        let left: String = l.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(left, "a ");
        let right: String = r.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(right, "\u{4e16}b");
    }
    use ratatui::style::{Color, Style};

    fn plain(text: &str) -> Span<'static> {
        Span::raw(text.to_string())
    }

    fn line_strings(lines: &[Line]) -> Vec<String> {
        lines.iter().map(|l| l.to_string()).collect()
    }

    #[test]
    fn short_text_stays_on_one_line() {
        let lines = wrap_spans(vec![plain("hello world")], 20);
        assert_eq!(line_strings(&lines), vec!["hello world"]);
    }

    #[test]
    fn wraps_at_word_boundaries() {
        let lines = wrap_spans(vec![plain("the quick brown fox jumps")], 10);
        assert_eq!(
            line_strings(&lines),
            vec!["the quick", "brown fox", "jumps"]
        );
    }

    #[test]
    fn exact_width_fits() {
        let lines = wrap_spans(vec![plain("12345")], 5);
        assert_eq!(line_strings(&lines), vec!["12345"]);
    }

    #[test]
    fn long_word_is_hard_split() {
        let lines = wrap_spans(vec![plain("abcdefghij")], 4);
        assert_eq!(line_strings(&lines), vec!["abcd", "efgh", "ij"]);
    }

    #[test]
    fn newline_forces_break() {
        let lines = wrap_spans(vec![plain("one\ntwo")], 20);
        assert_eq!(line_strings(&lines), vec!["one", "two"]);
    }

    #[test]
    fn styles_survive_wrapping() {
        let bold = Style::default().fg(Color::Red);
        let spans = vec![
            plain("plain "),
            Span::styled("styled words here".to_string(), bold),
        ];
        let lines = wrap_spans(spans, 12);
        assert_eq!(line_strings(&lines), vec!["plain styled", "words here"]);
        // The second line's spans keep the red style.
        assert!(lines[1].spans.iter().all(|s| s.style == bold));
    }

    #[test]
    fn wrapped_lines_do_not_start_with_space() {
        let lines = wrap_spans(vec![plain("aaaa bbbb cccc")], 5);
        for l in line_strings(&lines) {
            assert!(!l.starts_with(' '), "line starts with space: {l:?}");
        }
    }

    #[test]
    fn trailing_spaces_are_trimmed_at_breaks() {
        let lines = wrap_spans(vec![plain("word   next")], 6);
        assert_eq!(line_strings(&lines), vec!["word", "next"]);
    }

    #[test]
    fn empty_input_gives_one_empty_line() {
        let lines = wrap_spans(vec![], 10);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].width(), 0);
    }

    #[test]
    fn wide_chars_counted_by_display_width() {
        // CJK chars are two columns wide.
        let lines = wrap_spans(vec![plain("\u{4f60}\u{597d}\u{4e16}\u{754c}")], 4);
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn emoji_are_two_columns_wide() {
        // Three grinning faces at width 4: two fit per line.
        let lines = wrap_spans(vec![plain("\u{1f600}\u{1f600}\u{1f600}")], 4);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].to_string(), "\u{1f600}\u{1f600}");
    }

    #[test]
    fn consecutive_newlines_produce_empty_lines() {
        let lines = wrap_spans(vec![plain("a\n\nb")], 10);
        assert_eq!(line_strings(&lines), vec!["a", "", "b"]);
    }

    #[test]
    fn zero_width_is_clamped() {
        let lines = wrap_spans(vec![plain("ab")], 0);
        assert_eq!(line_strings(&lines), vec!["a", "b"]);
    }

    #[test]
    fn adjacent_same_style_spans_merge_in_output() {
        let spans = vec![plain("aa "), plain("bb")];
        let lines = wrap_spans(spans, 10);
        assert_eq!(lines[0].spans.len(), 1);
    }

    #[test]
    fn spans_width_counts_all_spans() {
        assert_eq!(spans_width(&[plain("ab"), plain("cde")]), 5);
    }
}
