//! Splits raw `.keynot` source into frontmatter and per-slide chunks.
//!
//! The splitter works on lines, before any markdown parsing:
//!
//! - If the very first line is `---`, everything up to the next `---` (or
//!   `...`, as in YAML) is frontmatter.
//! - After that, any line that is exactly `---` (ignoring surrounding
//!   whitespace) separates slides, and `|||` separates columns within a
//!   slide -- unless either appears inside a fenced code block (backtick
//!   or tilde fences, per CommonMark).
//! - Slides and columns that contain only whitespace are dropped.

use super::ParseError;

/// One slide's raw markdown source, one string per `|||` column.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawSlide {
    /// The slide's columns, without the `---` or `|||` separators. Always
    /// at least one entry.
    pub columns: Vec<String>,
    /// 1-based file line where each column's text begins (parallel to
    /// `columns`), so parsed blocks can be traced back to source lines.
    pub column_lines: Vec<usize>,
    /// 1-based line number in the original file where this slide starts.
    pub line: usize,
}

/// The result of splitting a source file.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SplitResult {
    /// The YAML text between the frontmatter fences, if present.
    pub frontmatter: Option<String>,
    pub slides: Vec<RawSlide>,
}

/// Split source text into frontmatter and raw slides.
pub fn split(source: &str) -> Result<SplitResult, ParseError> {
    let lines: Vec<&str> = source.lines().collect();
    let mut result = SplitResult::default();
    let mut idx = 0;

    // Frontmatter: only recognized on the very first line, like marp.
    if lines.first().map(|l| l.trim_end()) == Some("---") {
        let close = lines[1..]
            .iter()
            .position(|l| matches!(l.trim_end(), "---" | "..."))
            .ok_or(ParseError::UnterminatedFrontmatter)?;
        result.frontmatter = Some(lines[1..=close].join("\n"));
        idx = close + 2;
    }

    let mut fence: Option<Fence> = None;
    let mut columns: Vec<(String, usize)> = Vec::new();
    let mut current = Vec::new();
    let mut start_line = idx + 1;
    let mut column_line = start_line;

    for (offset, line) in lines[idx..].iter().enumerate() {
        let file_line = idx + offset + 1;
        match &fence {
            Some(open) if open.closed_by(line) => fence = None,
            Some(_) => {}
            None => {
                if let Some(open) = Fence::opened_by(line) {
                    fence = Some(open);
                } else if line.trim() == "---" {
                    push_column(&mut columns, &mut current, column_line);
                    push_slide(&mut result.slides, &mut columns, start_line);
                    start_line = file_line + 1;
                    column_line = start_line;
                    continue;
                } else if line.trim() == "|||" {
                    push_column(&mut columns, &mut current, column_line);
                    column_line = file_line + 1;
                    continue;
                }
            }
        }
        current.push(*line);
    }
    push_column(&mut columns, &mut current, column_line);
    push_slide(&mut result.slides, &mut columns, start_line);

    Ok(result)
}

/// Finish the current column, dropping it when blank. `line` is the
/// 1-based file line of the column's first line.
fn push_column(columns: &mut Vec<(String, usize)>, current: &mut Vec<&str>, line: usize) {
    let content = current.join("\n");
    current.clear();
    if !content.trim().is_empty() {
        columns.push((content, line));
    }
}

/// Finish the current slide, dropping it when it has no columns.
fn push_slide(slides: &mut Vec<RawSlide>, columns: &mut Vec<(String, usize)>, start_line: usize) {
    let columns = std::mem::take(columns);
    if !columns.is_empty() {
        let (columns, column_lines) = columns.into_iter().unzip();
        slides.push(RawSlide {
            columns,
            column_lines,
            line: start_line,
        });
    }
}

/// An open fenced code block: the fence character and its length.
/// Shared with the slide parser, which needs fence awareness for its
/// pre-parse emoji pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Fence {
    ch: char,
    len: usize,
}

impl Fence {
    /// Does this line open a fenced code block?
    pub(super) fn opened_by(line: &str) -> Option<Fence> {
        // A fence may be indented by at most three spaces (a tab or deeper
        // indentation makes it indented code, which we do not protect).
        let trimmed = line.trim_start_matches(' ');
        if line.len() - trimmed.len() > 3 {
            return None;
        }
        let ch = trimmed.chars().next()?;
        if ch != '`' && ch != '~' {
            return None;
        }
        let len = trimmed.chars().take_while(|&c| c == ch).count();
        if len < 3 {
            return None;
        }
        let info = &trimmed[len..];
        // Backtick fences may not contain backticks in the info string.
        if ch == '`' && info.contains('`') {
            return None;
        }
        Some(Fence { ch, len })
    }

    /// Does this line close this fence?
    pub(super) fn closed_by(&self, line: &str) -> bool {
        let trimmed = line.trim();
        !trimmed.is_empty() && trimmed.chars().all(|c| c == self.ch) && trimmed.len() >= self.len
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Single-column content of each slide (most tests use no `|||`).
    fn slide_contents(src: &str) -> Vec<String> {
        split(src)
            .unwrap()
            .slides
            .into_iter()
            .map(|s| s.columns.join("\n<col>\n"))
            .collect()
    }

    /// The columns of each slide.
    fn slide_columns(src: &str) -> Vec<Vec<String>> {
        split(src)
            .unwrap()
            .slides
            .into_iter()
            .map(|s| s.columns)
            .collect()
    }

    #[test]
    fn single_slide_no_frontmatter() {
        let r = split("# Hello\n\nworld\n").unwrap();
        assert_eq!(r.frontmatter, None);
        assert_eq!(r.slides.len(), 1);
        assert_eq!(r.slides[0].columns, vec!["# Hello\n\nworld"]);
        assert_eq!(r.slides[0].line, 1);
    }

    #[test]
    fn splits_on_dashes() {
        let slides = slide_contents("# A\n---\n# B\n---\n# C\n");
        assert_eq!(slides, vec!["# A", "# B", "# C"]);
    }

    #[test]
    fn separator_allows_surrounding_whitespace() {
        let slides = slide_contents("# A\n  ---  \n# B\n");
        assert_eq!(slides, vec!["# A", "# B"]);
    }

    #[test]
    fn four_dashes_is_not_a_separator() {
        let slides = slide_contents("# A\n----\n# B\n");
        assert_eq!(slides.len(), 1);
    }

    #[test]
    fn dashes_with_trailing_text_are_not_a_separator() {
        let slides = slide_contents("# A\n--- extra\n# B\n");
        assert_eq!(slides.len(), 1);
    }

    #[test]
    fn extracts_frontmatter() {
        let r = split("---\ntitle: Hi\nauthor: Alice\n---\n# Slide\n").unwrap();
        assert_eq!(r.frontmatter.as_deref(), Some("title: Hi\nauthor: Alice"));
        assert_eq!(r.slides.len(), 1);
        assert_eq!(r.slides[0].columns, vec!["# Slide"]);
        assert_eq!(r.slides[0].line, 5);
    }

    #[test]
    fn frontmatter_can_close_with_yaml_dots() {
        let r = split("---\ntitle: Hi\n...\n# Slide\n").unwrap();
        assert_eq!(r.frontmatter.as_deref(), Some("title: Hi"));
        assert_eq!(r.slides.len(), 1);
    }

    #[test]
    fn empty_frontmatter() {
        let r = split("---\n---\n# Slide\n").unwrap();
        assert_eq!(r.frontmatter.as_deref(), Some(""));
        assert_eq!(r.slides.len(), 1);
    }

    #[test]
    fn unterminated_frontmatter_errors() {
        assert!(matches!(
            split("---\ntitle: Hi\n# Slide\n"),
            Err(ParseError::UnterminatedFrontmatter)
        ));
    }

    #[test]
    fn dashes_inside_backtick_fence_do_not_split() {
        let src = "# A\n```yaml\n---\nkey: value\n---\n```\n---\n# B\n";
        let slides = slide_contents(src);
        assert_eq!(slides.len(), 2);
        assert!(slides[0].contains("key: value"));
        assert_eq!(slides[1], "# B");
    }

    #[test]
    fn dashes_inside_tilde_fence_do_not_split() {
        let src = "~~~\n---\n~~~\n---\n# B\n";
        let slides = slide_contents(src);
        assert_eq!(slides.len(), 2);
    }

    #[test]
    fn longer_fence_needs_longer_close() {
        // The inner ``` does not close a ```` fence.
        let src = "````\n```\n---\n```\n````\n---\n# B\n";
        let slides = slide_contents(src);
        assert_eq!(slides.len(), 2);
        assert!(slides[0].contains("---"));
    }

    #[test]
    fn backtick_info_string_with_backtick_is_not_a_fence() {
        // ``` `x` ``` is inline code, not a fence opener, so the --- splits.
        let src = "``` `x` ```\n---\n# B\n";
        let slides = slide_contents(src);
        assert_eq!(slides.len(), 2);
    }

    #[test]
    fn fence_with_language_info() {
        let src = "```rust\nlet x = 1;\n---\n```\n";
        let slides = slide_contents(src);
        assert_eq!(slides.len(), 1);
    }

    #[test]
    fn unclosed_fence_swallows_rest_of_file() {
        let src = "```\n---\n# still code\n";
        let slides = slide_contents(src);
        assert_eq!(slides.len(), 1);
    }

    #[test]
    fn drops_blank_slides() {
        let slides = slide_contents("# A\n---\n\n   \n---\n# B\n---\n");
        assert_eq!(slides, vec!["# A", "# B"]);
    }

    #[test]
    fn pipes_split_columns_within_a_slide() {
        let cols = slide_columns("left\n|||\nright\n---\n# B\n");
        assert_eq!(cols, vec![vec!["left", "right"], vec!["# B"]]);
    }

    #[test]
    fn column_separator_allows_surrounding_whitespace() {
        let cols = slide_columns("a\n  |||  \nb\n");
        assert_eq!(cols, vec![vec!["a", "b"]]);
    }

    #[test]
    fn four_pipes_is_not_a_column_separator() {
        let cols = slide_columns("a\n||||\nb\n");
        assert_eq!(cols, vec![vec!["a\n||||\nb"]]);
    }

    #[test]
    fn pipes_with_trailing_text_are_not_a_separator() {
        let cols = slide_columns("a\n||| extra\nb\n");
        assert_eq!(cols.len(), 1);
        assert_eq!(cols[0].len(), 1);
    }

    #[test]
    fn pipes_inside_code_fences_do_not_split() {
        let cols = slide_columns("```\n|||\n```\n");
        assert_eq!(cols, vec![vec!["```\n|||\n```"]]);
    }

    #[test]
    fn blank_columns_are_dropped() {
        let cols = slide_columns("a\n|||\n   \n|||\nb\n");
        assert_eq!(cols, vec![vec!["a", "b"]]);
    }

    #[test]
    fn all_blank_columns_drop_the_slide() {
        let cols = slide_columns("# A\n---\n|||\n   \n|||\n---\n# B\n");
        assert_eq!(cols, vec![vec!["# A"], vec!["# B"]]);
    }

    #[test]
    fn columns_do_not_affect_slide_line_numbers() {
        let r = split("# A\n|||\nright\n---\n# B\n").unwrap();
        assert_eq!(r.slides[0].line, 1);
        assert_eq!(r.slides[1].line, 5);
    }

    #[test]
    fn three_columns() {
        let cols = slide_columns("a\n|||\nb\n|||\nc\n");
        assert_eq!(cols, vec![vec!["a", "b", "c"]]);
    }

    #[test]
    fn columns_record_their_own_file_lines() {
        let r = split("# A\n\nbody\n|||\nright col\n---\n# B\n").unwrap();
        assert_eq!(r.slides[0].column_lines, vec![1, 5]);
        assert_eq!(r.slides[1].column_lines, vec![7]);
    }

    #[test]
    fn handles_crlf_line_endings() {
        let r = split("---\r\ntitle: Hi\r\n---\r\n# A\r\n---\r\n# B\r\n").unwrap();
        assert_eq!(r.frontmatter.as_deref(), Some("title: Hi"));
        assert_eq!(r.slides.len(), 2);
    }

    #[test]
    fn records_slide_line_numbers() {
        let r = split("---\ntitle: T\n---\n# A\n---\n# B\n").unwrap();
        assert_eq!(r.slides[0].line, 4);
        assert_eq!(r.slides[1].line, 6);
    }

    #[test]
    fn empty_input() {
        let r = split("").unwrap();
        assert_eq!(r.frontmatter, None);
        assert!(r.slides.is_empty());
    }

    #[test]
    fn tab_indented_fence_is_not_a_fence() {
        // A tab makes the line indented code, not a fence, so the ---
        // still separates slides (same as other indented code).
        let slides = slide_contents("\t```\n---\n\t```\n");
        assert_eq!(slides.len(), 2);
    }

    #[test]
    fn indented_code_block_dashes_still_split() {
        // We only protect fenced blocks; a bare --- at column 0 splits even
        // if the author meant it as indented code. Documented behavior.
        let slides = slide_contents("# A\n\n    code\n---\n# B\n");
        assert_eq!(slides.len(), 2);
    }
}
