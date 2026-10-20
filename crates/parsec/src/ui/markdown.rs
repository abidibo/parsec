//! A little Markdown for the Quick Look pane: turned into Pango markup so
//! a plain label can show it. Headings, emphasis, code, lists, quotes and
//! links come through; images and raw HTML are dropped (in READMEs they
//! are mostly badges and logos), and tables, which a label can't draw,
//! become one line per row.

use gtk::glib;
use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};

const CODE: &str = r##"<span font_family="monospace" bgcolor="#80808033">"##;
const BLOCK_CODE: &str = r#"<span font_family="monospace" size="small">"#;
const DIM: &str = r#"<span fgalpha="60%">"#;

/// `md` as Pango markup, ready for `gtk::Label::set_markup`.
pub fn to_markup(md: &str) -> String {
    let options =
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let mut w = Writer::default();
    for event in Parser::new_ext(md, options) {
        w.event(event);
    }
    w.out.truncate(w.out.trim_end().len());
    w.out
}

#[derive(Default)]
struct Writer {
    out: String,
    /// One entry per open list: the next number, or `None` for bullets.
    lists: Vec<Option<u64>>,
    /// A bullet or quote was just opened: its first block goes right here.
    fresh: bool,
    code_block: bool,
    /// Inside an image: its alt text is not shown.
    images: usize,
    /// Where each open link's `<a>` starts and ends, or `None` for one
    /// shown as text.
    links: Vec<Option<(usize, usize)>>,
    /// Table cells of the current row, and the markup outside the cell.
    cells: Vec<String>,
    outside_cell: String,
    head_row: bool,
}

impl Writer {
    fn event(&mut self, event: Event) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) => self.text(&text),
            Event::Code(code) => {
                if self.images == 0 {
                    self.out.push_str(CODE);
                    self.text(&code);
                    self.out.push_str("</span>");
                }
            }
            Event::SoftBreak => self.text(" "),
            Event::HardBreak => self.out.push('\n'),
            Event::Rule => {
                self.breaks(2);
                self.out
                    .push_str(r#"<span fgalpha="40%">──────────</span>"#);
            }
            Event::TaskListMarker(done) => self.text(if done { "☑ " } else { "☐ " }),
            // Raw HTML, footnote marks, maths: nothing a label can show well.
            _ => {}
        }
    }

    fn start(&mut self, tag: Tag) {
        match tag {
            Tag::Paragraph => self.breaks(2),
            Tag::Heading { level, .. } => {
                self.breaks(2);
                self.out.push_str(match level {
                    HeadingLevel::H1 => r#"<span size="x-large" weight="bold">"#,
                    HeadingLevel::H2 => r#"<span size="large" weight="bold">"#,
                    _ => r#"<span weight="bold">"#,
                });
            }
            Tag::BlockQuote(_) => {
                self.breaks(2);
                self.out.push_str(DIM);
                self.out.push_str("<i>");
                self.fresh = true;
            }
            Tag::CodeBlock(_) => {
                self.breaks(2);
                self.out.push_str(BLOCK_CODE);
                self.code_block = true;
            }
            Tag::List(first) => {
                self.breaks(if self.lists.is_empty() { 2 } else { 1 });
                self.lists.push(first);
            }
            Tag::Item => {
                self.breaks(1);
                let depth = self.lists.len().saturating_sub(1);
                self.out.push_str(&"    ".repeat(depth));
                let marker = match self.lists.last_mut() {
                    Some(Some(n)) => {
                        *n += 1;
                        format!("{}. ", *n - 1)
                    }
                    _ => "• ".to_string(),
                };
                self.out.push_str(&marker);
                self.fresh = true;
            }
            Tag::Emphasis => self.out.push_str("<i>"),
            Tag::Strong => self.out.push_str("<b>"),
            Tag::Strikethrough => self.out.push_str("<s>"),
            Tag::Link { dest_url, .. } => {
                // Relative links point into the repository; nothing to open.
                if dest_url.contains("://") || dest_url.starts_with("mailto:") {
                    let at = self.out.len();
                    self.out.push_str(&format!(
                        r#"<a href="{}">"#,
                        glib::markup_escape_text(&dest_url)
                    ));
                    self.links.push(Some((at, self.out.len())));
                } else {
                    self.links.push(None);
                }
            }
            Tag::Image { .. } => self.images += 1,
            Tag::Table(_) => self.breaks(2),
            Tag::TableHead => {
                self.head_row = true;
                self.cells.clear();
            }
            Tag::TableRow => self.cells.clear(),
            Tag::TableCell => self.outside_cell = std::mem::take(&mut self.out),
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Heading(_) => self.out.push_str("</span>"),
            TagEnd::BlockQuote(_) => self.out.push_str("</i></span>"),
            TagEnd::CodeBlock => {
                self.code_block = false;
                self.out.truncate(self.out.trim_end_matches('\n').len());
                self.out.push_str("</span>");
            }
            TagEnd::List(_) => {
                self.lists.pop();
            }
            TagEnd::Item => self.fresh = false,
            TagEnd::Emphasis => self.out.push_str("</i>"),
            TagEnd::Strong => self.out.push_str("</b>"),
            TagEnd::Strikethrough => self.out.push_str("</s>"),
            TagEnd::Link => {
                if let Some(Some((at, inner))) = self.links.pop() {
                    if self.out.len() == inner {
                        // Nothing shown inside (a linked badge): no empty link.
                        self.out.truncate(at);
                    } else {
                        self.out.push_str("</a>");
                    }
                }
            }
            TagEnd::Image => self.images -= 1,
            TagEnd::TableCell => {
                let cell = std::mem::replace(&mut self.out, std::mem::take(&mut self.outside_cell));
                self.cells.push(cell.trim().to_string());
            }
            TagEnd::TableHead | TagEnd::TableRow => self.row(),
            _ => {}
        }
    }

    fn text(&mut self, text: &str) {
        if self.images > 0 {
            return;
        }
        // A dropped image or HTML can leave a line starting with a space.
        let line_start = self.out.is_empty() || self.out.ends_with('\n');
        let text = if line_start && !self.code_block {
            text.trim_start()
        } else {
            text
        };
        self.out.push_str(&glib::markup_escape_text(text));
        self.fresh = false;
    }

    /// A table row on one line: the header dimmed, a row's first cell bold.
    fn row(&mut self) {
        let cells: Vec<String> = std::mem::take(&mut self.cells)
            .into_iter()
            .filter(|c| !c.is_empty())
            .collect();
        if cells.is_empty() {
            return;
        }
        self.breaks(1);
        if std::mem::take(&mut self.head_row) {
            self.out.push_str(DIM);
            self.out.push_str(&cells.join(" · "));
            self.out.push_str("</span>");
        } else {
            self.out.push_str(&format!("<b>{}</b>", cells[0]));
            for cell in &cells[1..] {
                self.out.push_str(" · ");
                self.out.push_str(cell);
            }
        }
    }

    /// End what came before with `n` line breaks (none at the very top,
    /// or right after a bullet or a quote's opening).
    fn breaks(&mut self, n: usize) {
        if std::mem::take(&mut self.fresh) || self.out.is_empty() {
            return;
        }
        let have = self.out.len() - self.out.trim_end_matches('\n').len();
        for _ in have..n {
            self.out.push('\n');
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Well-formed for a label: `<a>` is GTK's own, Pango never sees it.
    fn valid(markup: &str) -> bool {
        let mut plain = String::new();
        let mut rest = markup;
        while let Some(at) = rest.find("<a href=") {
            plain.push_str(&rest[..at]);
            rest = &rest[at + rest[at..].find('>').unwrap() + 1..];
        }
        plain.push_str(rest);
        gtk::pango::parse_markup(&plain.replace("</a>", ""), '\0').is_ok()
    }

    #[test]
    fn real_documents_give_valid_markup() {
        let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
        for doc in ["README.md", "DESIGN.md"] {
            let md = std::fs::read_to_string(format!("{root}/{doc}")).unwrap();
            assert!(valid(&to_markup(&md)), "{doc}");
        }
    }

    #[test]
    fn blocks_and_inlines() {
        let m = to_markup("# Title\n\nSome **bold**, *it* and `code`.\n\n## Next\n");
        assert!(m.starts_with(r#"<span size="x-large" weight="bold">Title</span>"#));
        assert!(m.contains("<b>bold</b>, <i>it</i> and "));
        assert!(m.contains(&format!("{CODE}code</span>")));
        assert!(m.ends_with(r#"<span size="large" weight="bold">Next</span>"#));
        assert!(valid(&m));
    }

    #[test]
    fn text_is_escaped() {
        let m = to_markup("a < b && `x<y>`\n\n[go](https://x.dev/?a=1&b=2)");
        assert!(m.contains("a &lt; b &amp;&amp; "));
        assert!(m.contains("x&lt;y&gt;"));
        assert!(m.contains(r#"<a href="https://x.dev/?a=1&amp;b=2">go</a>"#));
        assert!(valid(&m));
    }

    #[test]
    fn lists_nest_and_count() {
        let m = to_markup("Intro\n\n- one\n- two\n  - deep\n\n3. three\n4. four\n");
        assert_eq!(m, "Intro\n\n• one\n• two\n    • deep\n\n3. three\n4. four");
    }

    #[test]
    fn loose_list_items_keep_their_text_beside_the_bullet() {
        let m = to_markup("- one\n\n- two\n");
        assert_eq!(m, "• one\n• two");
    }

    #[test]
    fn images_and_html_are_dropped_and_badge_links_vanish() {
        let m = to_markup(
            "<p align=\"center\"><img src=\"logo.png\"></p>\n\n\
             [![CI](https://x/badge.svg)](https://x/actions) Hello ![pic](a.png)\n",
        );
        assert_eq!(m, "Hello");
        assert!(valid(&m));
    }

    #[test]
    fn relative_links_are_plain_text() {
        assert_eq!(to_markup("See [the spec](docs/spec.md)."), "See the spec.");
    }

    #[test]
    fn unclosed_code_block_still_closes() {
        let m = to_markup("Run:\n\n```sh\ncargo build\ncargo test\n");
        assert_eq!(
            m,
            format!("Run:\n\n{BLOCK_CODE}cargo build\ncargo test</span>")
        );
        assert!(valid(&m));
    }

    #[test]
    fn tables_become_one_line_per_row() {
        let m = to_markup("| I want to… | Read | Needs |\n|---|---|---|\n| build | `BUILD.md` | Java |\n| | | |\n");
        assert_eq!(
            m,
            format!("{DIM}I want to… · Read · Needs</span>\n<b>build</b> · {CODE}BUILD.md</span> · Java")
        );
        assert!(valid(&m));
    }

    #[test]
    fn quotes_rules_and_tasks() {
        let m = to_markup("> careful\n>\n> really\n\n---\n\n- [x] done\n- [ ] todo\n");
        assert!(m.starts_with(&format!("{DIM}<i>careful\n\nreally</i></span>")));
        assert!(m.ends_with("• ☑ done\n• ☐ todo"));
        assert!(valid(&m));
    }
}
