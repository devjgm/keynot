//! Parses one slide's markdown into a small block-level AST.
//!
//! The AST is intentionally minimal: just what the terminal renderer can
//! draw. HTML comments become speaker notes, `<u>`/`</u>` toggles underline
//! (markdown has no native underline), and `<br>` forces a line break.
//! Any other raw HTML is ignored.

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

/// One parsed slide: one or more side-by-side columns of blocks.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Slide {
    /// The slide's columns (from `|||` separators); single-column slides
    /// have exactly one entry.
    pub columns: Vec<Vec<Block>>,
    /// Speaker notes collected from HTML comments across all columns, in
    /// document order.
    pub notes: Vec<String>,
}

/// A block-level element on a slide.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    Heading {
        level: u8,
        content: Vec<InlineSpan>,
    },
    Paragraph(Vec<InlineSpan>),
    List(ListBlock),
    CodeBlock {
        language: Option<String>,
        code: String,
    },
    BlockQuote {
        /// `Some` for GFM alerts (`> [!NOTE]` and friends), which render
        /// with a labeled, colored bar instead of the plain quote style.
        kind: Option<AlertKind>,
        blocks: Vec<Block>,
    },
    /// An image that was alone in its paragraph; `source` is the URL or
    /// path exactly as written. Images mixed into text stay inline.
    Image {
        source: String,
        alt: String,
    },
    Table(TableBlock),
    /// `Term` / `: definition` pairs (the definition-list extension).
    DefinitionList(Vec<DefinitionItem>),
    /// Footnote definitions, synthesized at the end of a column that
    /// referenced any (`[^1]` markers point here).
    Footnotes(Vec<Footnote>),
    Rule,
}

/// One term and its (possibly several) definitions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefinitionItem {
    pub term: Vec<InlineSpan>,
    pub definitions: Vec<Vec<Block>>,
}

/// One footnote: its number (assigned in reference order) and body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Footnote {
    pub number: usize,
    pub blocks: Vec<Block>,
}

/// The GFM alert flavors (`> [!NOTE]` etc.).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlertKind {
    Note,
    Tip,
    Important,
    Warning,
    Caution,
}

impl AlertKind {
    /// The label shown on the alert's first line.
    pub fn label(self) -> &'static str {
        match self {
            AlertKind::Note => "Note",
            AlertKind::Tip => "Tip",
            AlertKind::Important => "Important",
            AlertKind::Warning => "Warning",
            AlertKind::Caution => "Caution",
        }
    }
}

/// A GFM table. Cells are inline-only (GFM allows no block content in
/// them); the parser normalizes ragged rows to the header's width.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableBlock {
    /// Per-column alignment, from the `:---:` header markers.
    pub alignments: Vec<TableAlign>,
    pub header: Vec<Vec<InlineSpan>>,
    pub rows: Vec<Vec<Vec<InlineSpan>>>,
}

/// Column alignment inside a table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TableAlign {
    #[default]
    Left,
    Center,
    Right,
}

/// An ordered or unordered list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListBlock {
    pub ordered: bool,
    /// First item number for ordered lists (`1` for unordered).
    pub start: u64,
    pub items: Vec<ListItem>,
}

/// One list item: its content is a sequence of blocks so items can hold
/// paragraphs, nested lists, code blocks, and so on.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ListItem {
    pub blocks: Vec<Block>,
    /// `Some(checked)` when this is a task list item (`- [x]` / `- [ ]`).
    pub task: Option<bool>,
}

/// A run of inline text with uniform styling.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct InlineSpan {
    pub text: String,
    pub style: InlineStyle,
    /// Destination URL when this span is inside a link.
    pub link: Option<String>,
    /// Image URL/path for an inline image; `text` holds the alt text.
    pub image: Option<String>,
}

/// Inline style flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct InlineStyle {
    /// A footnote reference marker like `[1]`, drawn in the accent.
    pub footnote_ref: bool,
    pub bold: bool,
    pub italic: bool,
    pub strikethrough: bool,
    pub underline: bool,
    pub code: bool,
}

impl InlineSpan {
    pub fn plain(text: impl Into<String>) -> Self {
        InlineSpan {
            text: text.into(),
            ..Default::default()
        }
    }
}

impl Slide {
    /// Parse one single-column slide. Markdown parsing cannot fail;
    /// malformed input just renders as literal text.
    pub fn parse(source: &str) -> Self {
        Slide::parse_columns(std::slice::from_ref(&source.to_string()))
    }

    /// Parse a slide from its column sources (one per `|||` section).
    pub fn parse_columns(columns: &[String]) -> Self {
        let mut slide = Slide::default();
        for source in columns {
            let options = Options::ENABLE_STRIKETHROUGH
                | Options::ENABLE_TASKLISTS
                | Options::ENABLE_TABLES
                | Options::ENABLE_HEADING_ATTRIBUTES
                | Options::ENABLE_GFM
                | Options::ENABLE_DEFINITION_LIST
                | Options::ENABLE_FOOTNOTES;
            let parser = Parser::new_ext(source, options);
            let mut builder = SlideBuilder::default();
            for event in parser {
                builder.event(event);
            }
            let column = builder.finish();
            slide.columns.push(column.blocks);
            slide.notes.extend(column.notes);
        }
        if slide.columns.is_empty() {
            slide.columns.push(Vec::new());
        }
        slide
    }

    /// All inline text of the first heading (or `None`), joined; columns
    /// are searched in order. Used as the slide's outline label and as
    /// the presentation title fallback.
    pub fn title_text(&self) -> Option<String> {
        self.columns.iter().flatten().find_map(|b| match b {
            Block::Heading { content, .. } if !content.is_empty() => {
                Some(content.iter().map(|s| s.text.as_str()).collect())
            }
            _ => None,
        })
    }
}

/// One parsed `|||` column: its blocks and the notes found in it.
struct ParsedColumn {
    blocks: Vec<Block>,
    notes: Vec<String>,
}

/// Open container blocks while walking parser events.
#[derive(Debug)]
enum Container {
    Quote(Option<AlertKind>, Vec<Block>),
    DefinitionList(Vec<DefinitionItem>),
    Definition(Vec<Block>),
    Footnote {
        number: usize,
        blocks: Vec<Block>,
    },
    Table {
        table: TableBlock,
        /// Cells collect into the header until the head section ends.
        in_head: bool,
    },
    List(ListBlock),
    Item {
        blocks: Vec<Block>,
        task: Option<bool>,
    },
}

#[derive(Default)]
struct SlideBuilder {
    blocks: Vec<Block>,
    containers: Vec<Container>,
    inline: Vec<InlineSpan>,
    style: InlineStyle,
    links: Vec<String>,
    notes: Vec<String>,
    /// `Some` while inside a code block: (language, accumulated code).
    code: Option<(Option<String>, String)>,
    /// `Some` while inside an HTML block: accumulated raw HTML.
    html: Option<String>,
    /// `Some((start_index, url))` while inside an image.
    image: Option<(usize, String)>,
    /// Footnote numbers by name, assigned in order of first sighting.
    footnote_numbers: std::collections::HashMap<String, usize>,
    footnotes: Vec<Footnote>,
}

impl SlideBuilder {
    fn event(&mut self, event: Event) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) => match (&mut self.code, &mut self.html) {
                (Some((_, buf)), _) => buf.push_str(&text),
                (None, Some(buf)) => buf.push_str(&text),
                (None, None) => self.push_text(&text),
            },
            Event::Code(text) => {
                let mut style = self.style;
                style.code = true;
                self.push_span(&text, style);
            }
            Event::SoftBreak => self.push_text(" "),
            Event::HardBreak => self.push_text("\n"),
            Event::Rule => {
                self.flush_inline();
                self.push_block(Block::Rule);
            }
            Event::TaskListMarker(checked) => {
                if let Some(Container::Item { task, .. }) = self.containers.last_mut() {
                    *task = Some(checked);
                }
            }
            Event::FootnoteReference(name) => {
                let number = self.footnote_number(&name);
                let mut style = self.style;
                style.footnote_ref = true;
                self.push_span(&format!("[{number}]"), style);
            }
            Event::InlineHtml(html) => self.inline_html(&html),
            Event::Html(html) => {
                if let Some(buf) = &mut self.html {
                    buf.push_str(&html);
                } else {
                    self.notes.extend(comment_texts(&html));
                }
            }
            // Footnotes, math, etc. are not supported; drop them quietly.
            _ => {}
        }
    }

    fn start(&mut self, tag: Tag) {
        match tag {
            Tag::Paragraph | Tag::Heading { .. } => self.flush_inline(),
            Tag::BlockQuote(kind) => {
                self.flush_inline();
                let kind = kind.map(|k| match k {
                    pulldown_cmark::BlockQuoteKind::Note => AlertKind::Note,
                    pulldown_cmark::BlockQuoteKind::Tip => AlertKind::Tip,
                    pulldown_cmark::BlockQuoteKind::Important => AlertKind::Important,
                    pulldown_cmark::BlockQuoteKind::Warning => AlertKind::Warning,
                    pulldown_cmark::BlockQuoteKind::Caution => AlertKind::Caution,
                });
                self.containers.push(Container::Quote(kind, Vec::new()));
            }
            Tag::List(start) => {
                // A nested list may start while the parent (tight) item's
                // text is still pending; flush it into the item first.
                self.flush_inline();
                self.containers.push(Container::List(ListBlock {
                    ordered: start.is_some(),
                    start: start.unwrap_or(1),
                    items: Vec::new(),
                }));
            }
            Tag::Item => self.containers.push(Container::Item {
                blocks: Vec::new(),
                task: None,
            }),
            Tag::Table(alignments) => {
                self.flush_inline();
                self.containers.push(Container::Table {
                    table: TableBlock {
                        alignments: alignments
                            .into_iter()
                            .map(|a| match a {
                                pulldown_cmark::Alignment::Center => TableAlign::Center,
                                pulldown_cmark::Alignment::Right => TableAlign::Right,
                                _ => TableAlign::Left,
                            })
                            .collect(),
                        header: Vec::new(),
                        rows: Vec::new(),
                    },
                    in_head: true,
                });
            }
            Tag::TableRow => {
                if let Some(Container::Table {
                    table,
                    in_head: false,
                }) = self.containers.last_mut()
                {
                    table.rows.push(Vec::new());
                }
            }
            Tag::TableHead | Tag::TableCell => {}
            Tag::CodeBlock(kind) => {
                self.flush_inline();
                let language = match kind {
                    CodeBlockKind::Fenced(info) => {
                        let lang = info.split_whitespace().next().unwrap_or("");
                        (!lang.is_empty()).then(|| lang.to_string())
                    }
                    CodeBlockKind::Indented => None,
                };
                self.code = Some((language, String::new()));
            }
            Tag::DefinitionList => {
                self.flush_inline();
                self.containers.push(Container::DefinitionList(Vec::new()));
            }
            Tag::DefinitionListTitle => self.flush_inline(),
            Tag::DefinitionListDefinition => {
                self.containers.push(Container::Definition(Vec::new()));
            }
            Tag::FootnoteDefinition(name) => {
                self.flush_inline();
                let number = self.footnote_number(&name);
                self.containers.push(Container::Footnote {
                    number,
                    blocks: Vec::new(),
                });
            }
            Tag::HtmlBlock => self.html = Some(String::new()),
            Tag::Emphasis => self.style.italic = true,
            Tag::Strong => self.style.bold = true,
            Tag::Strikethrough => self.style.strikethrough = true,
            Tag::Link { dest_url, .. } => self.links.push(dest_url.to_string()),
            Tag::Image { dest_url, .. } => {
                self.image = Some((self.inline.len(), dest_url.to_string()));
            }
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => {
                let spans = self.take_inline();
                // An image alone in its paragraph becomes a block so the
                // player can draw the actual picture.
                if let [only] = spans.as_slice()
                    && let Some(source) = &only.image
                {
                    self.push_block(Block::Image {
                        source: source.clone(),
                        alt: only.text.clone(),
                    });
                } else if !spans.is_empty() {
                    self.push_block(Block::Paragraph(spans));
                }
            }
            TagEnd::Heading(level) => {
                let content = self.take_inline();
                self.push_block(Block::Heading {
                    level: heading_level(level),
                    content,
                });
            }
            TagEnd::BlockQuote(_) => {
                self.flush_inline();
                if let Some(Container::Quote(kind, blocks)) = self.containers.pop() {
                    self.push_block(Block::BlockQuote { kind, blocks });
                }
            }
            TagEnd::List(_) => {
                if let Some(Container::List(list)) = self.containers.pop() {
                    self.push_block(Block::List(list));
                }
            }
            TagEnd::Item => {
                self.flush_inline();
                if let Some(Container::Item { blocks, task }) = self.containers.pop()
                    && let Some(Container::List(list)) = self.containers.last_mut()
                {
                    list.items.push(ListItem { blocks, task });
                }
            }
            TagEnd::DefinitionList => {
                if let Some(Container::DefinitionList(items)) = self.containers.pop() {
                    self.push_block(Block::DefinitionList(items));
                }
            }
            TagEnd::DefinitionListTitle => {
                let term = self.take_inline();
                if let Some(Container::DefinitionList(items)) = self.containers.last_mut() {
                    items.push(DefinitionItem {
                        term,
                        definitions: Vec::new(),
                    });
                }
            }
            TagEnd::DefinitionListDefinition => {
                self.flush_inline();
                if let Some(Container::Definition(blocks)) = self.containers.pop()
                    && let Some(Container::DefinitionList(items)) = self.containers.last_mut()
                    && let Some(item) = items.last_mut()
                {
                    item.definitions.push(blocks);
                }
            }
            TagEnd::FootnoteDefinition => {
                self.flush_inline();
                if let Some(Container::Footnote { number, blocks }) = self.containers.pop() {
                    self.footnotes.push(Footnote { number, blocks });
                }
            }
            TagEnd::Table => {
                if let Some(Container::Table { table, .. }) = self.containers.pop() {
                    self.push_block(Block::Table(table));
                }
            }
            TagEnd::TableHead => {
                if let Some(Container::Table { in_head, .. }) = self.containers.last_mut() {
                    *in_head = false;
                }
            }
            TagEnd::TableRow => {}
            TagEnd::TableCell => {
                let cell = self.take_inline();
                if let Some(Container::Table { table, in_head }) = self.containers.last_mut() {
                    if *in_head {
                        table.header.push(cell);
                    } else if let Some(row) = table.rows.last_mut() {
                        row.push(cell);
                    }
                }
            }
            TagEnd::CodeBlock => {
                if let Some((language, code)) = self.code.take() {
                    self.push_block(Block::CodeBlock { language, code });
                }
            }
            TagEnd::HtmlBlock => {
                if let Some(html) = self.html.take() {
                    self.notes.extend(comment_texts(&html));
                }
            }
            TagEnd::Emphasis => self.style.italic = false,
            TagEnd::Strong => self.style.bold = false,
            TagEnd::Strikethrough => self.style.strikethrough = false,
            TagEnd::Link => {
                self.links.pop();
            }
            TagEnd::Image => {
                if let Some((start, url)) = self.image.take() {
                    let alt: String = self.inline.drain(start..).map(|s| s.text).collect();
                    let label = if alt.trim().is_empty() {
                        url.clone()
                    } else {
                        alt
                    };
                    // Pushed directly (not via push_span) so it never
                    // merges with neighboring text spans.
                    self.inline.push(InlineSpan {
                        text: label,
                        style: self.style,
                        link: self.links.last().cloned(),
                        image: Some(url),
                    });
                }
            }
            _ => {}
        }
    }

    fn inline_html(&mut self, html: &str) {
        match html.trim().to_ascii_lowercase().as_str() {
            "<u>" => self.style.underline = true,
            "</u>" => self.style.underline = false,
            "<br>" | "<br/>" | "<br />" => self.push_text("\n"),
            _ => self.notes.extend(comment_texts(html)),
        }
    }

    fn push_text(&mut self, text: &str) {
        self.push_span(text, self.style);
    }

    fn push_span(&mut self, text: &str, style: InlineStyle) {
        if text.is_empty() {
            return;
        }
        // Tabs would render as zero-width cells; expand them here so all
        // downstream width math is correct.
        let text: std::borrow::Cow<str> = if text.contains('\t') {
            text.replace('\t', "    ").into()
        } else {
            text.into()
        };
        let link = self.links.last().cloned();
        // Merge adjacent spans with identical styling to keep the AST
        // small. Never merge across an image boundary, and never while
        // collecting alt text (it is drained by span index at image end).
        if self.image.is_none()
            && let Some(last) = self.inline.last_mut()
            && last.style == style
            && last.link == link
            && last.image.is_none()
        {
            last.text.push_str(&text);
            return;
        }
        self.inline.push(InlineSpan {
            text: text.into_owned(),
            style,
            link,
            image: None,
        });
    }

    fn take_inline(&mut self) -> Vec<InlineSpan> {
        std::mem::take(&mut self.inline)
    }

    /// Flush pending inline spans as an implicit paragraph. This handles
    /// tight list items, whose text arrives without paragraph events.
    fn flush_inline(&mut self) {
        let spans = self.take_inline();
        if !spans.is_empty() {
            self.push_block(Block::Paragraph(spans));
        }
    }

    fn push_block(&mut self, block: Block) {
        match self.containers.last_mut() {
            Some(Container::Quote(_, blocks))
            | Some(Container::Item { blocks, .. })
            | Some(Container::Definition(blocks))
            | Some(Container::Footnote { blocks, .. }) => {
                blocks.push(block);
            }
            // A block can never be a direct child of a list, and GFM
            // table cells hold no blocks; defensively treat such a block
            // as a sibling instead of dropping it.
            Some(Container::List(_))
            | Some(Container::Table { .. })
            | Some(Container::DefinitionList(_))
            | None => self.blocks.push(block),
        }
    }

    /// The stable number for footnote `name`, assigned on first sight.
    fn footnote_number(&mut self, name: &str) -> usize {
        let next = self.footnote_numbers.len() + 1;
        *self
            .footnote_numbers
            .entry(name.to_string())
            .or_insert(next)
    }

    fn finish(mut self) -> ParsedColumn {
        self.flush_inline();
        if !self.footnotes.is_empty() {
            self.footnotes.sort_by_key(|f| f.number);
            self.blocks.push(Block::Footnotes(self.footnotes));
        }
        ParsedColumn {
            blocks: self.blocks,
            notes: self.notes,
        }
    }
}

fn heading_level(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

/// The trimmed inner text of every HTML comment in `html`, in order.
/// Empty comments are dropped.
fn comment_texts(html: &str) -> Vec<String> {
    let mut notes = Vec::new();
    let mut rest = html;
    while let Some(start) = rest.find("<!--") {
        let after = &rest[start + 4..];
        let Some(end) = after.find("-->") else { break };
        let note = after[..end].trim();
        if !note.is_empty() {
            notes.push(note.to_string());
        }
        rest = &after[end + 3..];
    }
    notes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_of(spans: &[InlineSpan]) -> String {
        spans.iter().map(|s| s.text.as_str()).collect()
    }

    #[test]
    fn parses_heading_levels() {
        for (src, level) in [("# H", 1), ("## H", 2), ("### H", 3), ("###### H", 6)] {
            let slide = Slide::parse(src);
            assert_eq!(
                slide.columns[0],
                vec![Block::Heading {
                    level,
                    content: vec![InlineSpan::plain("H")]
                }],
                "source: {src}"
            );
        }
    }

    #[test]
    fn parses_paragraph_with_inline_styles() {
        let slide = Slide::parse("plain **bold** *italic* ~~gone~~ `code`");
        let Block::Paragraph(spans) = &slide.columns[0][0] else {
            panic!("expected paragraph");
        };
        assert_eq!(text_of(spans), "plain bold italic gone code");
        assert!(spans.iter().any(|s| s.style.bold && s.text == "bold"));
        assert!(spans.iter().any(|s| s.style.italic && s.text == "italic"));
        assert!(
            spans
                .iter()
                .any(|s| s.style.strikethrough && s.text == "gone")
        );
        assert!(spans.iter().any(|s| s.style.code && s.text == "code"));
    }

    #[test]
    fn nested_emphasis_combines() {
        let slide = Slide::parse("***both***");
        let Block::Paragraph(spans) = &slide.columns[0][0] else {
            panic!()
        };
        assert!(spans[0].style.bold && spans[0].style.italic);
    }

    #[test]
    fn underline_via_html_u_tag() {
        let slide = Slide::parse("a <u>under</u> b");
        let Block::Paragraph(spans) = &slide.columns[0][0] else {
            panic!()
        };
        let under: Vec<_> = spans.iter().filter(|s| s.style.underline).collect();
        assert_eq!(under.len(), 1);
        assert_eq!(under[0].text, "under");
    }

    #[test]
    fn links_carry_url() {
        let slide = Slide::parse("see [the docs](https://example.com) now");
        let Block::Paragraph(spans) = &slide.columns[0][0] else {
            panic!()
        };
        let link = spans.iter().find(|s| s.link.is_some()).unwrap();
        assert_eq!(link.text, "the docs");
        assert_eq!(link.link.as_deref(), Some("https://example.com"));
    }

    #[test]
    fn soft_break_is_a_space() {
        let slide = Slide::parse("one\ntwo");
        let Block::Paragraph(spans) = &slide.columns[0][0] else {
            panic!()
        };
        assert_eq!(text_of(spans), "one two");
    }

    #[test]
    fn hard_break_is_a_newline() {
        let slide = Slide::parse("one  \ntwo");
        let Block::Paragraph(spans) = &slide.columns[0][0] else {
            panic!()
        };
        assert_eq!(text_of(spans), "one\ntwo");
    }

    #[test]
    fn br_tag_is_a_newline() {
        let slide = Slide::parse("one<br>two");
        let Block::Paragraph(spans) = &slide.columns[0][0] else {
            panic!()
        };
        assert_eq!(text_of(spans), "one\ntwo");
    }

    #[test]
    fn parses_unordered_list() {
        let slide = Slide::parse("- one\n- two\n");
        let Block::List(list) = &slide.columns[0][0] else {
            panic!()
        };
        assert!(!list.ordered);
        assert_eq!(list.items.len(), 2);
        let Block::Paragraph(spans) = &list.items[0].blocks[0] else {
            panic!()
        };
        assert_eq!(text_of(spans), "one");
    }

    #[test]
    fn parses_ordered_list_with_start() {
        let slide = Slide::parse("3. three\n4. four\n");
        let Block::List(list) = &slide.columns[0][0] else {
            panic!()
        };
        assert!(list.ordered);
        assert_eq!(list.start, 3);
        assert_eq!(list.items.len(), 2);
    }

    #[test]
    fn parses_nested_list() {
        let slide = Slide::parse("- outer\n  - inner one\n  - inner two\n");
        let Block::List(list) = &slide.columns[0][0] else {
            panic!()
        };
        assert_eq!(list.items.len(), 1);
        let item = &list.items[0];
        // First the item's own text, then the nested list.
        let Block::Paragraph(spans) = &item.blocks[0] else {
            panic!()
        };
        assert_eq!(text_of(spans), "outer");
        let Block::List(inner) = &item.blocks[1] else {
            panic!("expected nested list, got {:?}", item.blocks[1])
        };
        assert_eq!(inner.items.len(), 2);
    }

    #[test]
    fn parses_task_list_markers() {
        let slide = Slide::parse("- [x] done\n- [ ] todo\n- plain\n");
        let Block::List(list) = &slide.columns[0][0] else {
            panic!()
        };
        assert_eq!(list.items[0].task, Some(true));
        assert_eq!(list.items[1].task, Some(false));
        assert_eq!(list.items[2].task, None);
        // The marker is item metadata, not text.
        let Block::Paragraph(spans) = &list.items[0].blocks[0] else {
            panic!()
        };
        assert_eq!(text_of(spans), "done");
    }

    #[test]
    fn parses_fenced_code_block_with_language() {
        let slide = Slide::parse("```rust\nfn main() {}\n```\n");
        assert_eq!(
            slide.columns[0],
            vec![Block::CodeBlock {
                language: Some("rust".to_string()),
                code: "fn main() {}\n".to_string(),
            }]
        );
    }

    #[test]
    fn parses_fenced_code_block_without_language() {
        let slide = Slide::parse("```\nplain\n```\n");
        let Block::CodeBlock { language, .. } = &slide.columns[0][0] else {
            panic!()
        };
        assert_eq!(*language, None);
    }

    #[test]
    fn code_block_preserves_markdown_syntax() {
        let slide = Slide::parse("```md\n# not a heading\n**not bold**\n```\n");
        let Block::CodeBlock { code, .. } = &slide.columns[0][0] else {
            panic!()
        };
        assert_eq!(code, "# not a heading\n**not bold**\n");
    }

    #[test]
    fn parses_blockquote() {
        let slide = Slide::parse("> quoted text\n");
        let Block::BlockQuote { blocks: inner, .. } = &slide.columns[0][0] else {
            panic!()
        };
        let Block::Paragraph(spans) = &inner[0] else {
            panic!()
        };
        assert_eq!(text_of(spans), "quoted text");
    }

    #[test]
    fn parses_nested_blockquote() {
        let slide = Slide::parse("> outer\n>\n> > inner\n");
        let Block::BlockQuote { blocks: outer, .. } = &slide.columns[0][0] else {
            panic!()
        };
        assert!(outer.iter().any(|b| matches!(b, Block::BlockQuote { .. })));
    }

    #[test]
    fn parses_rule_from_stars() {
        // `---` is a slide separator (handled by the splitter), but `***`
        // still reaches the markdown parser as a thematic break.
        let slide = Slide::parse("above\n\n***\n\nbelow\n");
        assert!(slide.columns[0].contains(&Block::Rule));
    }

    #[test]
    fn comments_become_notes_and_do_not_render() {
        let slide = Slide::parse("# Title\n\n<!-- remember to smile -->\n\ntext\n");
        assert_eq!(slide.notes, vec!["remember to smile"]);
        assert!(!slide.columns[0].iter().any(|b| match b {
            Block::Paragraph(spans) => text_of(spans).contains("smile"),
            _ => false,
        }));
    }

    #[test]
    fn inline_comments_become_notes() {
        let slide = Slide::parse("hello <!-- inline note --> world\n");
        assert_eq!(slide.notes, vec!["inline note"]);
        let Block::Paragraph(spans) = &slide.columns[0][0] else {
            panic!()
        };
        assert_eq!(text_of(spans), "hello  world");
    }

    #[test]
    fn multiline_comments_become_notes() {
        let slide = Slide::parse("# T\n\n<!--\nline one\nline two\n-->\n");
        assert_eq!(slide.notes, vec!["line one\nline two"]);
    }

    #[test]
    fn adjacent_comments_become_separate_notes() {
        let slide = Slide::parse("# T\n\n<!-- first --> <!-- second -->\n");
        assert_eq!(slide.notes, vec!["first", "second"]);
    }

    #[test]
    fn empty_comments_are_not_notes() {
        let slide = Slide::parse("# T\n\n<!-- -->\n<!-- real -->\n");
        assert_eq!(slide.notes, vec!["real"]);
    }

    #[test]
    fn tabs_in_text_expand_to_spaces() {
        let slide = Slide::parse("a\tb");
        let Block::Paragraph(spans) = &slide.columns[0][0] else {
            panic!()
        };
        assert_eq!(text_of(spans), "a    b");
    }

    #[test]
    fn other_html_is_ignored() {
        let slide = Slide::parse("a <span>b</span> c\n\n<div>\nblock\n</div>\n");
        let Block::Paragraph(spans) = &slide.columns[0][0] else {
            panic!()
        };
        // Inline tags are dropped; their text content remains.
        assert_eq!(text_of(spans), "a b c");
    }

    #[test]
    fn image_alone_becomes_an_image_block() {
        let slide = Slide::parse("![a chart](chart.png)");
        assert_eq!(
            slide.columns[0],
            vec![Block::Image {
                source: "chart.png".to_string(),
                alt: "a chart".to_string(),
            }]
        );
    }

    #[test]
    fn image_without_alt_uses_url_as_label() {
        let slide = Slide::parse("![](chart.png)");
        let Block::Image { alt, .. } = &slide.columns[0][0] else {
            panic!()
        };
        assert_eq!(alt, "chart.png");
    }

    #[test]
    fn inline_image_stays_a_span() {
        let slide = Slide::parse("see ![the chart](c.png) here");
        let Block::Paragraph(spans) = &slide.columns[0][0] else {
            panic!()
        };
        let img = spans.iter().find(|s| s.image.is_some()).unwrap();
        assert_eq!(img.text, "the chart");
        assert_eq!(img.image.as_deref(), Some("c.png"));
        assert_eq!(text_of(spans), "see the chart here");
    }

    #[test]
    fn slide_title_is_first_heading() {
        let slide = Slide::parse("some text\n\n## Actual Title\n\nmore\n");
        assert_eq!(slide.title_text().as_deref(), Some("Actual Title"));
    }

    #[test]
    fn slide_title_joins_styled_spans() {
        let slide = Slide::parse("# Hello **World**\n");
        assert_eq!(slide.title_text().as_deref(), Some("Hello World"));
    }

    #[test]
    fn slide_title_none_without_heading() {
        assert_eq!(Slide::parse("just text").title_text(), None);
    }

    #[test]
    fn adjacent_same_style_spans_merge() {
        let slide = Slide::parse("one two three");
        let Block::Paragraph(spans) = &slide.columns[0][0] else {
            panic!()
        };
        assert_eq!(spans.len(), 1);
    }

    #[test]
    fn empty_source_is_empty_slide() {
        let slide = Slide::parse("");
        assert!(slide.columns[0].is_empty());
        assert!(slide.notes.is_empty());
    }

    #[test]
    fn parse_columns_builds_one_column_per_source() {
        let slide = Slide::parse_columns(&["left".to_string(), "# Right".to_string()]);
        assert_eq!(slide.columns.len(), 2);
        assert!(matches!(slide.columns[0][0], Block::Paragraph(_)));
        assert!(matches!(slide.columns[1][0], Block::Heading { .. }));
    }

    #[test]
    fn parse_columns_merges_notes_in_order() {
        let slide = Slide::parse_columns(&[
            "a\n\n<!-- first -->".to_string(),
            "b\n\n<!-- second -->".to_string(),
        ]);
        assert_eq!(slide.notes, vec!["first", "second"]);
    }

    #[test]
    fn parse_columns_of_nothing_yields_one_empty_column() {
        let slide = Slide::parse_columns(&[]);
        assert_eq!(slide.columns.len(), 1);
        assert!(slide.columns[0].is_empty());
    }

    #[test]
    fn title_found_in_a_later_column() {
        let slide = Slide::parse_columns(&["just text".to_string(), "## The Title".to_string()]);
        assert_eq!(slide.title_text().as_deref(), Some("The Title"));
    }

    #[test]
    fn parse_is_a_single_column() {
        let slide = Slide::parse("hello");
        assert_eq!(slide.columns.len(), 1);
    }

    #[test]
    fn heading_attributes_are_stripped() {
        let slide = Slide::parse("# Title {#custom-id .cls}\n");
        let Block::Heading { content, .. } = &slide.columns[0][0] else {
            panic!("expected heading");
        };
        assert_eq!(text_of(content), "Title");
    }

    #[test]
    fn gfm_alerts_parse_with_their_kind() {
        let slide = Slide::parse("> [!WARNING]\n> mind the gap\n");
        let Block::BlockQuote { kind, blocks } = &slide.columns[0][0] else {
            panic!("expected quote");
        };
        assert_eq!(*kind, Some(AlertKind::Warning));
        let Block::Paragraph(spans) = &blocks[0] else {
            panic!("expected paragraph body");
        };
        assert_eq!(text_of(spans), "mind the gap");
    }

    #[test]
    fn plain_quotes_have_no_alert_kind() {
        let slide = Slide::parse("> just quoting\n");
        let Block::BlockQuote { kind, .. } = &slide.columns[0][0] else {
            panic!("expected quote");
        };
        assert_eq!(*kind, None);
    }

    #[test]
    fn definition_lists_parse_terms_and_definitions() {
        let slide = Slide::parse("Term\n: first def\n: second def\n");
        let Block::DefinitionList(items) = &slide.columns[0][0] else {
            panic!("expected definition list, got {:?}", slide.columns[0]);
        };
        assert_eq!(items.len(), 1);
        assert_eq!(text_of(&items[0].term), "Term");
        assert_eq!(items[0].definitions.len(), 2);
    }

    #[test]
    fn footnotes_number_in_reference_order() {
        let slide = Slide::parse("See[^b] and[^a].\n\n[^a]: note a\n[^b]: note b\n");
        // References numbered by first sighting: b=1, a=2; the section
        // is appended at the end, sorted by number.
        let Block::Paragraph(spans) = &slide.columns[0][0] else {
            panic!("expected paragraph");
        };
        let markers: Vec<&str> = spans
            .iter()
            .filter(|s| s.style.footnote_ref)
            .map(|s| s.text.as_str())
            .collect();
        assert_eq!(markers, vec!["[1]", "[2]"]);
        let Block::Footnotes(notes) = slide.columns[0].last().unwrap() else {
            panic!("expected footnote section");
        };
        assert_eq!(notes.len(), 2);
        assert_eq!(notes[0].number, 1);
        let Block::Paragraph(body) = &notes[0].blocks[0] else {
            panic!("note body");
        };
        assert_eq!(text_of(body), "note b");
    }

    #[test]
    fn tables_parse_with_alignment_and_cells() {
        let slide = Slide::parse("| name | age |\n|:-----|----:|\n| ada | 36 |\n| bob | 40 |\n");
        let Block::Table(table) = &slide.columns[0][0] else {
            panic!("expected a table, got {:?}", slide.columns[0]);
        };
        assert_eq!(table.alignments, vec![TableAlign::Left, TableAlign::Right]);
        assert_eq!(table.header.len(), 2);
        assert_eq!(table.header[0][0].text, "name");
        assert_eq!(table.rows.len(), 2);
        assert_eq!(table.rows[1][1][0].text, "40");
    }

    #[test]
    fn table_cells_keep_inline_styles() {
        let slide = Slide::parse("| a |\n|---|\n| **bold** and `code` |\n");
        let Block::Table(table) = &slide.columns[0][0] else {
            panic!("expected a table");
        };
        let cell = &table.rows[0][0];
        assert!(cell.iter().any(|s| s.style.bold));
        assert!(cell.iter().any(|s| s.style.code));
    }

    #[test]
    fn ragged_table_rows_are_tolerated() {
        // GFM: short rows pad, long rows truncate (pulldown handles it).
        let slide = Slide::parse("| a | b |\n|---|---|\n| only |\n");
        let Block::Table(table) = &slide.columns[0][0] else {
            panic!("expected a table");
        };
        assert_eq!(table.rows.len(), 1);
        assert!(table.rows[0].len() <= 2);
    }

    #[test]
    fn multiple_blocks_in_order() {
        let slide = Slide::parse("# T\n\npara\n\n- item\n\n```\ncode\n```\n\n> quote\n");
        let kinds: Vec<&str> = slide.columns[0]
            .iter()
            .map(|b| match b {
                Block::Heading { .. } => "heading",
                Block::Paragraph(_) => "para",
                Block::List(_) => "list",
                Block::CodeBlock { .. } => "code",
                Block::BlockQuote { .. } => "quote",
                Block::Image { .. } => "image",
                Block::Rule => "rule",
                Block::Table(_) => "table",
                Block::DefinitionList(_) => "definitions",
                Block::Footnotes(_) => "footnotes",
            })
            .collect();
        assert_eq!(kinds, vec!["heading", "para", "list", "code", "quote"]);
    }
}
