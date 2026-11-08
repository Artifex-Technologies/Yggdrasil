//! Markdown to a small tree of blocks the reader draws (headings, paragraphs, lists, tables, code, quotes), from pulldown-cmark's
//! events. GitHub's extras are on: tables, task lists, strikethrough, footnotes, alerts (`> [!NOTE]`) and front matter. Raw HTML is
//! not run: an HTML block shows its text, its pictures (`<img src>`) and its headings, and everything else of it is dropped.

use pulldown_cmark::{Alignment, BlockQuoteKind, CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Style {
    pub bold: bool,
    pub italic: bool,
    pub strike: bool,
    pub code: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Inline {
    Text { text: String, style: Style, link: Option<String> },
    Image { url: String, alt: String, link: Option<String> },
    Break,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Alert {
    Note,
    Tip,
    Important,
    Warning,
    Caution,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    None,
    Left,
    Center,
    Right,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    /// A task list item: Some(done).
    pub task: Option<bool>,
    pub blocks: Vec<Block>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Block {
    Heading { level: u8, text: Vec<Inline>, slug: String },
    Para(Vec<Inline>),
    /// A paragraph centred (from HTML's `align="center"`, the usual README logo).
    Center(Vec<Inline>),
    Code { lang: String, text: String },
    Quote { alert: Option<Alert>, blocks: Vec<Block> },
    List { start: Option<u64>, items: Vec<Item> },
    Table { align: Vec<Align>, head: Vec<Vec<Inline>>, rows: Vec<Vec<Vec<Inline>>> },
    Footnote { label: String, blocks: Vec<Block> },
    Rule,
}

enum Container {
    Root,
    Quote(Option<Alert>),
    Item(Option<bool>),
    Footnote(String),
}

#[derive(Clone, Copy)]
enum InlineKind {
    Para,
    Heading(u8),
    Cell,
}

enum Frame {
    Blocks { kind: Container, blocks: Vec<Block> },
    List { start: Option<u64>, items: Vec<Item> },
    Inline { kind: InlineKind, inl: Vec<Inline>, implicit: bool },
    Table { align: Vec<Align>, head: Vec<Vec<Inline>>, rows: Vec<Vec<Vec<Inline>>>, row: Vec<Vec<Inline>> },
    Code { lang: String, text: String },
    Html(String),
}

struct Builder {
    stack: Vec<Frame>,
    bold: u32,
    italic: u32,
    strike: u32,
    links: Vec<String>,
    image: Option<(String, String)>,
    slugs: HashMap<String, usize>,
}

/// The blocks of a Markdown text.
pub fn parse(text: &str) -> Vec<Block> {
    let opts = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_GFM
        | Options::ENABLE_YAML_STYLE_METADATA_BLOCKS;
    let mut b = Builder { stack: vec![Frame::Blocks { kind: Container::Root, blocks: vec![] }], bold: 0, italic: 0, strike: 0, links: vec![], image: None, slugs: HashMap::new() };
    for ev in Parser::new_ext(text, opts) {
        b.event(ev);
    }
    b.finish()
}

impl Builder {
    fn event(&mut self, ev: Event) {
        match ev {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(t) => match self.stack.last_mut() {
                Some(Frame::Code { text, .. }) | Some(Frame::Html(text)) => text.push_str(&t),
                _ => self.text(&t, false),
            },
            Event::Code(t) | Event::InlineMath(t) | Event::DisplayMath(t) => self.text(&t, true),
            Event::Html(t) => match self.stack.last_mut() {
                Some(Frame::Html(text)) => text.push_str(&t),
                _ => self.inline_html(&t),
            },
            Event::InlineHtml(t) => self.inline_html(&t),
            Event::FootnoteReference(l) => self.text(&format!("[{l}]"), false),
            Event::SoftBreak => self.text(" ", false),
            Event::HardBreak => self.inlines().push(Inline::Break),
            Event::Rule => {
                self.close_implicit();
                self.push_block(Block::Rule);
            }
            Event::TaskListMarker(done) => {
                for f in self.stack.iter_mut().rev() {
                    if let Frame::Blocks { kind: Container::Item(task), .. } = f {
                        *task = Some(done);
                        break;
                    }
                }
            }
        }
    }

    fn start(&mut self, tag: Tag) {
        match tag {
            Tag::Paragraph => {
                self.close_implicit();
                self.stack.push(Frame::Inline { kind: InlineKind::Para, inl: vec![], implicit: false });
            }
            Tag::Heading { level, .. } => {
                self.close_implicit();
                self.stack.push(Frame::Inline { kind: InlineKind::Heading(level as u8), inl: vec![], implicit: false });
            }
            Tag::BlockQuote(kind) => {
                self.close_implicit();
                let alert = kind.map(|k| match k {
                    BlockQuoteKind::Note => Alert::Note,
                    BlockQuoteKind::Tip => Alert::Tip,
                    BlockQuoteKind::Important => Alert::Important,
                    BlockQuoteKind::Warning => Alert::Warning,
                    BlockQuoteKind::Caution => Alert::Caution,
                });
                self.stack.push(Frame::Blocks { kind: Container::Quote(alert), blocks: vec![] });
            }
            Tag::CodeBlock(kind) => {
                self.close_implicit();
                let lang = match kind {
                    CodeBlockKind::Fenced(info) => info.split(|c: char| c.is_whitespace() || c == ',').next().unwrap_or("").to_string(),
                    CodeBlockKind::Indented => String::new(),
                };
                self.stack.push(Frame::Code { lang, text: String::new() });
            }
            Tag::MetadataBlock(_) => {
                self.close_implicit();
                self.stack.push(Frame::Code { lang: "front matter".into(), text: String::new() });
            }
            Tag::HtmlBlock => {
                self.close_implicit();
                self.stack.push(Frame::Html(String::new()));
            }
            Tag::List(start) => {
                self.close_implicit();
                self.stack.push(Frame::List { start, items: vec![] });
            }
            Tag::Item => {
                self.close_implicit();
                self.stack.push(Frame::Blocks { kind: Container::Item(None), blocks: vec![] });
            }
            Tag::FootnoteDefinition(label) => {
                self.close_implicit();
                self.stack.push(Frame::Blocks { kind: Container::Footnote(label.to_string()), blocks: vec![] });
            }
            Tag::Table(al) => {
                self.close_implicit();
                let align = al
                    .iter()
                    .map(|a| match a {
                        Alignment::None => Align::None,
                        Alignment::Left => Align::Left,
                        Alignment::Center => Align::Center,
                        Alignment::Right => Align::Right,
                    })
                    .collect();
                self.stack.push(Frame::Table { align, head: vec![], rows: vec![], row: vec![] });
            }
            Tag::TableCell => self.stack.push(Frame::Inline { kind: InlineKind::Cell, inl: vec![], implicit: false }),
            Tag::Emphasis => self.italic += 1,
            Tag::Strong => self.bold += 1,
            Tag::Strikethrough => self.strike += 1,
            Tag::Link { dest_url, .. } => self.links.push(dest_url.to_string()),
            Tag::Image { dest_url, .. } => self.image = Some((dest_url.to_string(), String::new())),
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => {
                if let Some(Frame::Inline { inl, .. }) = self.pop_inline() {
                    if !inl.is_empty() {
                        self.push_block(Block::Para(inl));
                    }
                }
            }
            TagEnd::Heading(_) => {
                if let Some(Frame::Inline { kind: InlineKind::Heading(level), inl, .. }) = self.pop_inline() {
                    let slug = self.unique_slug(&plain(&inl));
                    self.push_block(Block::Heading { level, text: inl, slug });
                }
            }
            TagEnd::BlockQuote(_) => {
                self.close_implicit();
                if let Some(Frame::Blocks { kind: Container::Quote(alert), blocks }) = self.stack.pop() {
                    self.push_block(Block::Quote { alert, blocks });
                }
            }
            TagEnd::CodeBlock | TagEnd::MetadataBlock(_) => {
                if let Some(Frame::Code { lang, text }) = self.stack.pop() {
                    self.push_block(Block::Code { lang, text: text.trim_end_matches('\n').to_string() });
                }
            }
            TagEnd::HtmlBlock => {
                if let Some(Frame::Html(text)) = self.stack.pop() {
                    for b in html_block(&text, &mut |t| self.unique_slug(t)) {
                        self.push_block(b);
                    }
                }
            }
            TagEnd::Item => {
                self.close_implicit();
                if let Some(Frame::Blocks { kind: Container::Item(task), blocks }) = self.stack.pop() {
                    if let Some(Frame::List { items, .. }) = self.stack.last_mut() {
                        items.push(Item { task, blocks });
                    }
                }
            }
            TagEnd::List(_) => {
                self.close_implicit();
                if let Some(Frame::List { start, items }) = self.stack.pop() {
                    self.push_block(Block::List { start, items });
                }
            }
            TagEnd::FootnoteDefinition => {
                self.close_implicit();
                if let Some(Frame::Blocks { kind: Container::Footnote(label), blocks }) = self.stack.pop() {
                    self.push_block(Block::Footnote { label, blocks });
                }
            }
            TagEnd::TableCell => {
                if let Some(Frame::Inline { inl, .. }) = self.pop_inline() {
                    if let Some(Frame::Table { row, .. }) = self.stack.last_mut() {
                        row.push(inl);
                    }
                }
            }
            TagEnd::TableHead => {
                if let Some(Frame::Table { head, row, .. }) = self.stack.last_mut() {
                    *head = std::mem::take(row);
                }
            }
            TagEnd::TableRow => {
                if let Some(Frame::Table { rows, row, .. }) = self.stack.last_mut() {
                    rows.push(std::mem::take(row));
                }
            }
            TagEnd::Table => {
                if let Some(Frame::Table { align, head, rows, .. }) = self.stack.pop() {
                    self.push_block(Block::Table { align, head, rows });
                }
            }
            TagEnd::Emphasis => self.italic = self.italic.saturating_sub(1),
            TagEnd::Strong => self.bold = self.bold.saturating_sub(1),
            TagEnd::Strikethrough => self.strike = self.strike.saturating_sub(1),
            TagEnd::Link => {
                self.links.pop();
            }
            TagEnd::Image => {
                if let Some((url, alt)) = self.image.take() {
                    let link = self.links.last().cloned();
                    self.inlines().push(Inline::Image { url, alt, link });
                }
            }
            _ => {}
        }
    }

    /// The inline frame on top (a paragraph opened by itself when text comes where none is, as in a tight list's items).
    fn inlines(&mut self) -> &mut Vec<Inline> {
        if !matches!(self.stack.last(), Some(Frame::Inline { .. })) {
            self.stack.push(Frame::Inline { kind: InlineKind::Para, inl: vec![], implicit: true });
        }
        match self.stack.last_mut() {
            Some(Frame::Inline { inl, .. }) => inl,
            _ => unreachable!(),
        }
    }

    fn pop_inline(&mut self) -> Option<Frame> {
        match self.stack.last() {
            Some(Frame::Inline { .. }) => self.stack.pop(),
            _ => None,
        }
    }

    fn close_implicit(&mut self) {
        if matches!(self.stack.last(), Some(Frame::Inline { implicit: true, .. })) {
            if let Some(Frame::Inline { inl, .. }) = self.stack.pop() {
                if !inl.is_empty() {
                    self.push_block(Block::Para(inl));
                }
            }
        }
    }

    fn push_block(&mut self, b: Block) {
        for f in self.stack.iter_mut().rev() {
            if let Frame::Blocks { blocks, .. } = f {
                blocks.push(b);
                return;
            }
        }
    }

    fn text(&mut self, t: &str, code: bool) {
        if let Some((_, alt)) = &mut self.image {
            alt.push_str(t);
            return;
        }
        let style = Style { bold: self.bold > 0, italic: self.italic > 0, strike: self.strike > 0, code };
        let link = self.links.last().cloned();
        let v = self.inlines();
        if let Some(Inline::Text { text, style: s, link: l }) = v.last_mut() {
            if *s == style && *l == link {
                text.push_str(t);
                return;
            }
        }
        v.push(Inline::Text { text: t.to_string(), style, link });
    }

    /// Inline HTML: a line break, a picture; any other tag is dropped (its text, which comes as text, stays).
    fn inline_html(&mut self, t: &str) {
        let lower = t.trim().to_ascii_lowercase();
        if lower.starts_with("<br") {
            self.inlines().push(Inline::Break);
        } else if lower.starts_with("<img") {
            if let Some(url) = attr(t, "src") {
                let alt = attr(t, "alt").unwrap_or_default();
                let link = self.links.last().cloned();
                self.inlines().push(Inline::Image { url, alt, link });
            }
        }
    }

    fn unique_slug(&mut self, text: &str) -> String {
        let base = slug(text);
        let n = self.slugs.entry(base.clone()).or_insert(0);
        let s = if *n == 0 { base.clone() } else { format!("{base}-{n}") };
        *n += 1;
        s
    }

    fn finish(mut self) -> Vec<Block> {
        self.close_implicit();
        // anything left open (a broken table) is closed as far as it goes
        while self.stack.len() > 1 {
            self.close_implicit();
            match self.stack.pop() {
                Some(Frame::Inline { inl, .. }) if !inl.is_empty() => self.push_block(Block::Para(inl)),
                Some(Frame::Blocks { blocks, .. }) => {
                    for b in blocks {
                        self.push_block(b);
                    }
                }
                Some(Frame::Code { lang, text }) => self.push_block(Block::Code { lang, text }),
                _ => {}
            }
        }
        match self.stack.pop() {
            Some(Frame::Blocks { blocks, .. }) => blocks,
            _ => vec![],
        }
    }
}

/// The text of some inlines, without their styles (a heading's words, a picture's description).
pub fn plain(inl: &[Inline]) -> String {
    let mut s = String::new();
    for i in inl {
        match i {
            Inline::Text { text, .. } => s.push_str(text),
            Inline::Image { alt, .. } => s.push_str(alt),
            Inline::Break => s.push(' '),
        }
    }
    s
}

/// A heading's anchor the way GitHub makes it: lower case, spaces to dashes, punctuation dropped (`#my-heading`).
pub fn slug(text: &str) -> String {
    let mut out = String::new();
    for c in text.trim().chars() {
        if c.is_alphanumeric() || c == '-' || c == '_' {
            out.extend(c.to_lowercase());
        } else if c == ' ' {
            out.push('-');
        }
    }
    out
}

/// One attribute of an HTML tag (`src="a.png"`, `src='a.png'` or `src=a.png`).
pub fn attr(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let mut from = 0;
    while let Some(i) = lower[from..].find(name) {
        let at = from + i;
        from = at + name.len();
        let before_ok = at == 0 || !lower.as_bytes()[at - 1].is_ascii_alphanumeric();
        let rest = lower[from..].trim_start();
        if !before_ok || !rest.starts_with('=') {
            continue;
        }
        let vstart = tag.len() - rest.len() + 1;
        let v = tag[vstart..].trim_start();
        let value = match v.chars().next() {
            Some(q @ ('"' | '\'')) => v[1..].split(q).next().unwrap_or(""),
            _ => v.split(|c: char| c.is_whitespace() || c == '>' || c == '/').next().unwrap_or(""),
        };
        return Some(value.to_string());
    }
    None
}

/// An HTML block as blocks: its headings (`<h1>`..`<h6>`), its pictures (centred when it says `align="center"`), and its text.
/// Comments and every other tag are dropped; nothing in it runs.
fn html_block(html: &str, slug_of: &mut dyn FnMut(&str) -> String) -> Vec<Block> {
    let mut out = vec![];
    let mut cleaned = String::new();
    let mut rest = html;
    while let Some(i) = rest.find("<!--") {
        cleaned.push_str(&rest[..i]);
        rest = rest[i..].find("-->").map(|j| &rest[i + j + 3..]).unwrap_or("");
    }
    cleaned.push_str(rest);
    let centered = cleaned.to_ascii_lowercase().contains("align=\"center\"") || cleaned.to_ascii_lowercase().contains("align='center'");
    let mut images = vec![];
    let mut text = String::new();
    let mut heading: Option<u8> = None;
    let mut s = cleaned.as_str();
    let flush = |text: &mut String, heading: Option<u8>, out: &mut Vec<Block>, slug_of: &mut dyn FnMut(&str) -> String| {
        let t = text.split_whitespace().collect::<Vec<_>>().join(" ");
        text.clear();
        if t.is_empty() {
            return;
        }
        let inl = vec![Inline::Text { text: t.clone(), style: Style::default(), link: None }];
        out.push(match heading {
            Some(level) => Block::Heading { level, text: inl, slug: slug_of(&t) },
            None if centered => Block::Center(inl),
            None => Block::Para(inl),
        });
    };
    while let Some(lt) = s.find('<') {
        text.push_str(&decode_entities(&s[..lt]));
        let Some(gt) = s[lt..].find('>') else { break };
        let tag = &s[lt..lt + gt + 1];
        let name: String = tag[1..].trim_start_matches('/').chars().take_while(|c| c.is_ascii_alphanumeric()).collect::<String>().to_ascii_lowercase();
        let closing = tag[1..].starts_with('/');
        if !closing && (name == "script" || name == "style") {
            // their contents are code, not text: skipped whole
            let end = format!("</{name}");
            let after = &s[lt + gt + 1..];
            s = after.to_ascii_lowercase().find(&end).and_then(|i| after[i..].find('>').map(|j| &after[i + j + 1..])).unwrap_or("");
            continue;
        }
        if name == "img" {
            if let Some(url) = attr(tag, "src") {
                images.push(Inline::Image { url, alt: attr(tag, "alt").unwrap_or_default(), link: None });
                images.push(Inline::Text { text: " ".into(), style: Style::default(), link: None });
            }
        } else if name == "br" || name == "p" || name == "div" {
            flush(&mut text, heading, &mut out, slug_of);
        } else if name.len() == 2 && name.starts_with('h') && name.as_bytes()[1].is_ascii_digit() {
            flush(&mut text, heading, &mut out, slug_of);
            heading = if closing { None } else { Some(name.as_bytes()[1] - b'0') };
        }
        s = &s[lt + gt + 1..];
    }
    text.push_str(&decode_entities(s));
    if !images.is_empty() {
        images.pop();
        out.insert(0, if centered { Block::Center(images) } else { Block::Para(images) });
    }
    flush(&mut text, heading, &mut out, slug_of);
    out
}

fn decode_entities(s: &str) -> String {
    s.replace("&nbsp;", " ").replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&#39;", "'").replace("&amp;", "&")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(s: &str) -> Inline {
        Inline::Text { text: s.into(), style: Style::default(), link: None }
    }

    #[test]
    fn headings_paragraphs_and_anchors() {
        let b = parse("# Hello, World!\n\nSome *soft* text\nand more.\n\n## Hello, World!\n");
        assert_eq!(b.len(), 3);
        assert!(matches!(&b[0], Block::Heading { level: 1, slug, .. } if slug == "hello-world"));
        assert!(matches!(&b[2], Block::Heading { level: 2, slug, .. } if slug == "hello-world-1"), "a second one gets a number");
        let Block::Para(p) = &b[1] else { panic!() };
        assert_eq!(p[0], t("Some "));
        assert_eq!(p[1], Inline::Text { text: "soft".into(), style: Style { italic: true, ..Default::default() }, link: None });
        assert_eq!(p[2], t(" text and more."), "a soft line break is a space, and plain text is joined");
    }

    #[test]
    fn styles_links_and_code() {
        let b = parse("**bold [link](https://a.b)** and `code` ~~gone~~");
        let Block::Para(p) = &b[0] else { panic!() };
        assert_eq!(p[0], Inline::Text { text: "bold ".into(), style: Style { bold: true, ..Default::default() }, link: None });
        assert_eq!(p[1], Inline::Text { text: "link".into(), style: Style { bold: true, ..Default::default() }, link: Some("https://a.b".into()) });
        assert!(p.contains(&Inline::Text { text: "code".into(), style: Style { code: true, ..Default::default() }, link: None }));
        assert!(p.contains(&Inline::Text { text: "gone".into(), style: Style { strike: true, ..Default::default() }, link: None }));
    }

    #[test]
    fn tight_and_task_lists() {
        let b = parse("- [x] done\n- [ ] open\n  - nested\n\n3. three\n4. four\n");
        let Block::List { start: None, items } = &b[0] else { panic!("{b:?}") };
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].task, Some(true));
        assert_eq!(items[0].blocks, vec![Block::Para(vec![t("done")])]);
        assert_eq!(items[1].task, Some(false));
        assert!(matches!(&items[1].blocks[1], Block::List { items, .. } if items[0].blocks == vec![Block::Para(vec![t("nested")])]));
        assert!(matches!(&b[1], Block::List { start: Some(3), items } if items.len() == 2));
    }

    #[test]
    fn tables_code_quotes_and_rules() {
        let b = parse("| a | b |\n|:--|--:|\n| 1 | **2** |\n\n```rust title\nfn x() {}\n```\n\n> [!WARNING]\n> careful\n\n---\n");
        let Block::Table { align, head, rows } = &b[0] else { panic!("{b:?}") };
        assert_eq!(align, &vec![Align::Left, Align::Right]);
        assert_eq!(head, &vec![vec![t("a")], vec![t("b")]]);
        assert_eq!(rows.len(), 1);
        assert_eq!(b[1], Block::Code { lang: "rust".into(), text: "fn x() {}".into() });
        assert_eq!(b[2], Block::Quote { alert: Some(Alert::Warning), blocks: vec![Block::Para(vec![t("careful")])] });
        assert_eq!(b[3], Block::Rule);
    }

    #[test]
    fn pictures_and_front_matter() {
        let b = parse("---\ntitle: x\n---\n\n![A logo](img/logo.png)\n");
        assert_eq!(b[0], Block::Code { lang: "front matter".into(), text: "title: x".into() });
        assert_eq!(b[1], Block::Para(vec![Inline::Image { url: "img/logo.png".into(), alt: "A logo".into(), link: None }]));
    }

    #[test]
    fn html_blocks_show_pictures_and_text_only() {
        let b = parse("<p align=\"center\">\n  <img src=\"assets/logo.png\" alt='Logo' width=\"200\">\n</p>\n\n<h1 align=\"center\">My App</h1>\n\n<!-- hidden --><div>Some &amp; text<script>x()</script></div>\n");
        assert_eq!(b[0], Block::Center(vec![Inline::Image { url: "assets/logo.png".into(), alt: "Logo".into(), link: None }]));
        assert!(matches!(&b[1], Block::Heading { level: 1, slug, .. } if slug == "my-app"));
        let Block::Para(p) = &b[2] else { panic!("{b:?}") };
        assert_eq!(plain(p), "Some & text", "tags dropped, scripts skipped whole, comments gone");
    }

    #[test]
    fn attributes() {
        assert_eq!(attr("<img src=\"a b.png\">", "src").as_deref(), Some("a b.png"));
        assert_eq!(attr("<IMG SRC='x.svg' alt=y>", "src").as_deref(), Some("x.svg"));
        assert_eq!(attr("<img datasrc=\"no\" src=yes.png/>", "src").as_deref(), Some("yes.png"));
        assert_eq!(attr("<img alt=\"x\">", "src"), None);
    }

    #[test]
    fn slugs_like_github() {
        assert_eq!(slug("What's new in 2.0?"), "whats-new-in-20");
        assert_eq!(slug("  Über  Café "), "über--café");
        assert_eq!(slug("snake_case and-dash"), "snake_case-and-dash");
    }
}
