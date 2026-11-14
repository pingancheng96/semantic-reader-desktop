use crate::sqlite::Result;
use dom_query::NodeRef;
use reqwest::Url;
use serde::{Deserialize, Serialize};

pub const MAX_IMAGES: usize = 24;
pub const IMAGE_BYTES: usize = 4_000_000;
pub const MAX_LAYOUT_BYTES: usize = 34_000_000;
pub const MAX_IMAGE_DATA: usize = 30_000_000;

#[derive(Serialize, Deserialize)]
pub struct Layout {
    pub nodes: Vec<Node>,
    pub images: Vec<Option<String>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub image_errors: Vec<Option<String>>,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Node {
    Text { start: usize, end: usize },
    Element { tag: String, children: Vec<Node> },
    Image { index: usize, alt: String },
}

pub fn extract(root: &NodeRef<'_>, base: &Url) -> (String, Layout, Vec<Vec<Url>>) {
    let mut writer = Writer {
        content: String::new(),
        offset: 0,
        sources: Vec::new(),
        budget: 20_000,
    };
    let nodes = root
        .children_it(false)
        .flat_map(|node| writer.render(&node, base, 0))
        .collect();
    let images = (0..writer.sources.len()).map(|_| None).collect();
    (
        writer.content,
        Layout {
            nodes,
            images,
            image_errors: Vec::new(),
        },
        writer.sources,
    )
}

struct Writer {
    content: String,
    offset: usize,
    sources: Vec<Vec<Url>>,
    budget: usize,
}

impl Writer {
    fn separate(&mut self) {
        if !self.content.is_empty() && !self.content.ends_with("\n\n") {
            self.content.push_str("\n\n");
            self.offset += 2;
        }
    }

    fn render(&mut self, node: &NodeRef<'_>, base: &Url, depth: usize) -> Vec<Node> {
        if depth > 36 || self.budget == 0 {
            return Vec::new();
        }
        self.budget -= 1;
        if node.is_text() {
            let raw = node.text();
            let start = self.offset;
            for ch in raw.chars() {
                if ch.is_whitespace() {
                    if self.content.ends_with(|last: char| !last.is_whitespace()) {
                        self.content.push(' ');
                        self.offset += 1;
                    }
                } else {
                    self.content.push(ch);
                    self.offset += ch.len_utf16();
                }
            }
            return if start < self.offset {
                vec![Node::Text {
                    start,
                    end: self.offset,
                }]
            } else {
                Vec::new()
            };
        }
        let Some(mut tag) = node.node_name() else {
            return Vec::new();
        };
        if matches!(tag.as_ref(), "p" | "div" | "span")
            && node.attr("class").is_some_and(|classes| {
                classes.split_whitespace().any(|class| {
                    matches!(
                        class,
                        "caption"
                            | "image-caption"
                            | "photo-caption"
                            | "wp-caption-text"
                            | "figcaption"
                    )
                })
            })
        {
            tag = "figcaption".into();
        }
        match tag.as_ref() {
            "img" => {
                let choices = image_sources(node, base);
                if !choices.is_empty() {
                    let index = if let Some(index) =
                        self.sources.iter().position(|urls| urls == &choices)
                    {
                        index
                    } else if self.sources.len() < MAX_IMAGES {
                        self.sources.push(choices);
                        self.sources.len() - 1
                    } else {
                        return Vec::new();
                    };
                    return vec![Node::Image {
                        index,
                        alt: node.attr_or("alt", "").chars().take(240).collect(),
                    }];
                }
                return Vec::new();
            }
            "br" | "hr" => {
                self.separate();
                return vec![Node::Element {
                    tag: tag.to_string(),
                    children: Vec::new(),
                }];
            }
            _ => (),
        }
        let block = matches!(
            tag.as_ref(),
            "div"
                | "section"
                | "article"
                | "header"
                | "p"
                | "h1"
                | "h2"
                | "h3"
                | "h4"
                | "h5"
                | "h6"
                | "ul"
                | "ol"
                | "li"
                | "blockquote"
                | "figure"
                | "figcaption"
                | "table"
                | "thead"
                | "tbody"
                | "tfoot"
                | "tr"
                | "th"
                | "td"
                | "pre"
                | "dl"
                | "dt"
                | "dd"
        );
        let inline = matches!(
            tag.as_ref(),
            "strong" | "b" | "em" | "i" | "code" | "s" | "sub" | "sup" | "mark"
        );
        if block {
            self.separate();
        }
        let children: Vec<_> = node
            .children_it(false)
            .flat_map(|child| self.render(&child, base, depth + 1))
            .collect();
        if block {
            self.separate();
        }
        if children.is_empty() {
            return children;
        }
        if block || inline {
            vec![Node::Element {
                tag: tag.to_string(),
                children,
            }]
        } else {
            children
        }
    }
}

fn image_sources(node: &NodeRef<'_>, base: &Url) -> Vec<Url> {
    let mut values = Vec::new();
    for name in [
        "data-src",
        "data-original",
        "data-lazy-src",
        "data-original-src",
        "data-actualsrc",
    ] {
        if let Some(value) = node.attr(name) {
            values.push(value.to_string());
        }
    }
    let mut sets = vec![node.clone()];
    if let Some(parent) = node
        .parent()
        .filter(|n| n.node_name().as_deref() == Some("picture"))
    {
        sets.extend(
            parent
                .children_it(false)
                .filter(|n| n.node_name().as_deref() == Some("source"))
                .take(4),
        );
    }
    for source in sets {
        for name in ["data-srcset", "data-lazy-srcset", "srcset"] {
            if let Some(set) = source.attr(name) {
                let mut choices: Vec<_> = set
                    .split(',')
                    .filter_map(|entry| {
                        let mut parts = entry.split_whitespace();
                        let address = parts.next()?;
                        let width = parts
                            .next()
                            .and_then(|d| d.strip_suffix('w'))
                            .and_then(|d| d.parse::<u32>().ok())
                            .unwrap_or(800);
                        Some((width.abs_diff(800), address))
                    })
                    .collect();
                choices.sort_by_key(|choice| choice.0);
                values.extend(choices.into_iter().take(3).map(|(_, url)| url.to_owned()));
            }
        }
    }
    if let Some(value) = node.attr("src") {
        values.push(value.to_string());
    }
    let mut urls = Vec::new();
    for value in values {
        if value.trim().is_empty() {
            continue;
        }
        let Ok(joined) = base.join(&value) else {
            continue;
        };
        if let Ok(url) = crate::web_import::parse_url(joined.as_str()) {
            if !urls.contains(&url) {
                urls.push(url);
            }
        }
    }
    if urls.len() > 8 {
        let last = urls.pop().unwrap();
        urls.truncate(7);
        urls.push(last);
    }
    urls
}

pub fn image_mime(bytes: &[u8]) -> Option<&'static str> {
    let mime = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        "png"
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        "jpeg"
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        "gif"
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        "webp"
    } else if bytes.get(4..8) == Some(b"ftyp")
        && bytes
            .get(8..12)
            .is_some_and(|brand| matches!(brand, b"avif" | b"avis"))
    {
        "avif"
    } else {
        return None;
    };
    Some(mime)
}

pub fn image_uri(bytes: &[u8]) -> Option<String> {
    use base64::{engine::general_purpose::STANDARD, Engine};
    let mime = image_mime(bytes)?;
    Some(format!(
        "data:image/{mime};base64,{}",
        STANDARD.encode(bytes)
    ))
}

impl Layout {
    fn image_sequence(&self) -> Vec<(usize, &str)> {
        fn walk<'a>(nodes: &'a [Node], images: &mut Vec<(usize, &'a str)>) {
            for node in nodes {
                match node {
                    Node::Text { .. } => {}
                    Node::Element { children, .. } => walk(children, images),
                    Node::Image { index, alt } => images.push((*index, alt)),
                }
            }
        }
        let mut images = Vec::new();
        walk(&self.nodes, &mut images);
        images
    }

    pub fn check_image_sequence(&self, other: &Layout) -> Result<()> {
        if self.images.len() != other.images.len() || self.image_sequence() != other.image_sequence() {
            return Err("原网页的图片列表已变化，无法匹配原来的图片位置。".into());
        }
        Ok(())
    }

    pub fn fill_missing_images(&mut self, downloaded: Layout) -> Result<()> {
        self.check_image_sequence(&downloaded)?;
        let mut available = MAX_IMAGE_DATA.saturating_sub(self.images.iter().flatten().map(String::len).sum());
        self.image_errors.resize(self.images.len(), None);
        for (index, (slot, image)) in self.images.iter_mut().zip(downloaded.images).enumerate() {
            if slot.is_some() {
                continue;
            }
            if let Some(image) = image {
                if image.len() <= available {
                    available -= image.len();
                    *slot = Some(image);
                    self.image_errors[index] = None;
                } else {
                    self.image_errors[index] = Some("超过本篇图片存储上限".into());
                }
            } else {
                self.image_errors[index] = downloaded.image_errors.get(index).cloned().flatten();
            }
        }
        Ok(())
    }

    pub fn cap_images(&mut self) {
        let mut available = MAX_IMAGE_DATA;
        self.image_errors.resize(self.images.len(), None);
        for (index, image) in self.images.iter_mut().enumerate() {
            if let Some(data) = image {
                if data.len() > available {
                    *image = None;
                    self.image_errors[index] = Some("超过本篇图片存储上限".into());
                } else {
                    available -= data.len();
                }
            }
        }
    }
}

pub fn validate(value: &str) -> Result<()> {
    if value.len() > MAX_LAYOUT_BYTES {
        return Err("网页版式数据过大".into());
    }
    let layout: Layout = serde_json::from_str(value).map_err(|_| "网页版式数据无效")?;
    if layout.image_errors.len() > layout.images.len()
        || layout
            .image_errors
            .iter()
            .flatten()
            .any(|msg| msg.chars().count() > 180)
    {
        return Err("图片诊断记录无效".into());
    }
    if layout.images.len() > MAX_IMAGES {
        return Err("网页图片过多".into());
    }
    use base64::{engine::general_purpose::STANDARD, Engine};
    for image in layout.images.iter().flatten() {
        let (_, data) = image.split_once(',').ok_or("网页图片数据无效")?;
        let bytes = STANDARD.decode(data).map_err(|_| "网页图片编码无效")?;
        if bytes.len() > IMAGE_BYTES || image_uri(&bytes).as_deref() != Some(image.as_str()) {
            return Err("网页图片格式无效".into());
        }
    }
    Ok(())
}
