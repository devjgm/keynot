//! Syntax highlighting for code blocks, backed by syntect.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use syntect::easy::HighlightLines;
use syntect::highlighting::{FontStyle, ThemeSet};
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;

/// Loaded syntax and theme sets. Construct once and share; loading the
/// default syntax set is relatively expensive.
pub struct Highlighter {
    syntaxes: SyntaxSet,
    themes: ThemeSet,
}

impl Highlighter {
    pub fn new() -> Self {
        let started = std::time::Instant::now();
        let mut themes = ThemeSet::load_defaults();
        // Vendored under assets/ (a plain file, embedded at compile
        // time): our approximation of VS Code's Dark+ token colors.
        let dark_plus = include_bytes!("../../assets/dark-plus.tmTheme");
        let theme = ThemeSet::load_from_reader(&mut std::io::Cursor::new(dark_plus.as_slice()))
            .expect("embedded Dark+ theme must parse");
        themes.themes.insert("Dark+".to_string(), theme);
        let syntaxes = SyntaxSet::load_defaults_newlines();
        tracing::debug!(
            elapsed = ?started.elapsed(),
            themes = themes.themes.len(),
            "loaded syntax and theme sets"
        );
        Highlighter { syntaxes, themes }
    }

    /// Names of the available code themes, sorted.
    pub fn available_themes(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.themes.themes.keys().map(String::as_str).collect();
        names.sort_unstable();
        names
    }

    pub fn has_theme(&self, name: &str) -> bool {
        self.themes.themes.contains_key(name)
    }

    /// Highlight `code` as `language`, returning one ratatui line per code
    /// line. Unknown languages or themes fall back to plain text. Tabs are
    /// expanded to spaces: terminal cells have no tab stops, and the
    /// wrapping/clipping math counts a tab as zero columns.
    pub fn highlight(
        &self,
        code: &str,
        language: Option<&str>,
        theme_name: &str,
    ) -> Vec<Line<'static>> {
        let code = &code.replace('\t', "    ");
        let syntax = language
            .and_then(|lang| self.syntaxes.find_syntax_by_token(lang))
            .unwrap_or_else(|| self.syntaxes.find_syntax_plain_text());
        let Some(theme) = self.themes.themes.get(theme_name) else {
            return plain_lines(code);
        };

        let mut highlighter = HighlightLines::new(syntax, theme);
        let mut lines = Vec::new();
        for line in LinesWithEndings::from(code) {
            let Ok(ranges) = highlighter.highlight_line(line, &self.syntaxes) else {
                lines.push(Line::raw(line.trim_end_matches('\n').to_string()));
                continue;
            };
            let spans: Vec<Span<'static>> = ranges
                .into_iter()
                .filter_map(|(style, text)| {
                    let text = text.trim_end_matches('\n');
                    (!text.is_empty()).then(|| Span::styled(text.to_string(), convert_style(style)))
                })
                .collect();
            lines.push(Line::from(spans));
        }
        if lines.is_empty() {
            lines.push(Line::raw(""));
        }
        lines
    }
}

impl Default for Highlighter {
    fn default() -> Self {
        Self::new()
    }
}

fn plain_lines(code: &str) -> Vec<Line<'static>> {
    code.trim_end_matches('\n')
        .split('\n')
        .map(|l| Line::raw(l.to_string()))
        .collect()
}

fn convert_style(style: syntect::highlighting::Style) -> Style {
    let fg = style.foreground;
    let mut out = Style::default().fg(Color::Rgb(fg.r, fg.g, fg.b));
    if style.font_style.contains(FontStyle::BOLD) {
        out = out.add_modifier(Modifier::BOLD);
    }
    if style.font_style.contains(FontStyle::ITALIC) {
        out = out.add_modifier(Modifier::ITALIC);
    }
    if style.font_style.contains(FontStyle::UNDERLINE) {
        out = out.add_modifier(Modifier::UNDERLINED);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn highlighter() -> Highlighter {
        Highlighter::new()
    }

    #[test]
    fn highlights_rust_with_colors() {
        let h = highlighter();
        let lines = h.highlight("fn main() {}\n", Some("rust"), "base16-eighties.dark");
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].to_string(), "fn main() {}");
        // At least one span should carry a foreground color.
        assert!(lines[0].spans.iter().any(|s| s.style.fg.is_some()));
    }

    #[test]
    fn one_line_per_code_line() {
        let h = highlighter();
        let lines = h.highlight("a = 1\nb = 2\n", Some("python"), "base16-eighties.dark");
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].to_string(), "a = 1");
        assert_eq!(lines[1].to_string(), "b = 2");
    }

    #[test]
    fn unknown_language_falls_back_to_plain() {
        let h = highlighter();
        let lines = h.highlight("whatever\n", Some("nosuchlang"), "base16-eighties.dark");
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].to_string(), "whatever");
    }

    #[test]
    fn no_language_is_plain() {
        let h = highlighter();
        let lines = h.highlight("plain text\n", None, "base16-eighties.dark");
        assert_eq!(lines[0].to_string(), "plain text");
    }

    #[test]
    fn unknown_theme_falls_back_to_plain() {
        let h = highlighter();
        let lines = h.highlight("fn x() {}\n", Some("rust"), "no-such-theme");
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].to_string(), "fn x() {}");
    }

    #[test]
    fn default_themes_are_available() {
        let h = highlighter();
        assert!(h.has_theme("base16-eighties.dark"));
        assert!(h.has_theme("Dark+"), "embedded theme is loaded");
        assert!(h.has_theme("InspiredGitHub"));
        assert!(!h.available_themes().is_empty());
    }

    #[test]
    fn empty_code_yields_one_empty_line() {
        let h = highlighter();
        let lines = h.highlight("", Some("rust"), "base16-eighties.dark");
        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn tabs_expand_to_spaces() {
        let h = highlighter();
        let lines = h.highlight("\tindented\n", None, "base16-eighties.dark");
        assert_eq!(lines[0].to_string(), "    indented");
        // Same for the unknown-theme plain path.
        let lines = h.highlight("\tindented\n", None, "no-such-theme");
        assert_eq!(lines[0].to_string(), "    indented");
    }

    #[test]
    fn dark_plus_token_colors_are_applied() {
        // Guards the hand-written tmTheme: a scope typo in that XML would
        // silently render everything in the plain foreground color.
        let h = highlighter();
        let lines = h.highlight("fn main() { let s = \"hi\"; }\n", Some("rust"), "Dark+");
        let colors: Vec<(String, Option<Color>)> = lines[0]
            .spans
            .iter()
            .map(|s| (s.content.to_string(), s.style.fg))
            .collect();
        let color_of = |text: &str| {
            colors
                .iter()
                .find(|(t, _)| t.contains(text))
                .and_then(|(_, c)| *c)
        };
        assert_eq!(
            color_of("fn"),
            Some(Color::Rgb(0x56, 0x9C, 0xD6)),
            "keywords are Dark+ blue; spans: {colors:?}"
        );
        assert_eq!(
            color_of("hi"),
            Some(Color::Rgb(0xCE, 0x91, 0x78)),
            "strings are Dark+ sienna; spans: {colors:?}"
        );
    }

    #[test]
    fn language_aliases_work() {
        let h = highlighter();
        // "rs" is the file-extension token for Rust.
        let lines = h.highlight("let x = 1;\n", Some("rs"), "base16-eighties.dark");
        assert!(lines[0].spans.iter().any(|s| s.style.fg.is_some()));
    }
}
