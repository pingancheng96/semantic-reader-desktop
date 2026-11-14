use std::io::{Read, Seek};

use crate::web_layout::{self, Layout, Node};
use dom_query::Document;
use url::Url;
use zip::ZipArchive;

pub const TEXT_LIMIT: usize = 10_000_000;
const ENTRY_LIMIT: usize = 8_000_000;

fn read_entry<R: Read + Seek>(zip: &mut ZipArchive<R>, path: &str) -> Result<String, String> {
    let entry = zip
        .by_name(path)
        .map_err(|_| format!("EPUB 缺少文件：{path}"))?;
    let mut bytes = Vec::new();
    entry
        .take(ENTRY_LIMIT as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "无法解压 EPUB 内容")?;
    if bytes.len() > ENTRY_LIMIT {
        return Err("EPUB 单个解压文件超过 8 MB".into());
    }
    String::from_utf8(bytes).map_err(|_| "EPUB 章节必须使用 UTF-8 编码".into())
}

fn shift_nodes(nodes: &mut [Node], offset: usize) {
    for node in nodes {
        match node {
            Node::Text { start, end } => {
                *start += offset;
                *end += offset;
            }
            Node::Element { children, .. } => shift_nodes(children, offset),
            Node::Image { .. } => {}
        }
    }
}

pub fn read<R: Read + Seek>(file: R) -> Result<(String, Layout), String> {
    let mut zip = ZipArchive::new(file).map_err(|_| "无法读取 EPUB 文件")?;
    let container = read_entry(&mut zip, "META-INF/container.xml")?;
    let container = roxmltree::Document::parse(&container).map_err(|_| "EPUB 目录格式无效")?;
    let package = container
        .descendants()
        .find(|node| node.has_tag_name("rootfile"))
        .and_then(|node| node.attribute("full-path"))
        .ok_or("EPUB 缺少书籍清单")?;
    let opf = read_entry(&mut zip, package)?;
    let opf = roxmltree::Document::parse(&opf).map_err(|_| "EPUB 书籍清单格式无效")?;
    let mut base = Url::parse("https://epub.invalid/").unwrap();
    base.set_path(&format!("/{package}"));
    let manifest = opf
        .descendants()
        .find(|node| node.has_tag_name("manifest"))
        .ok_or("EPUB 缺少章节清单")?;
    let chapters: std::collections::HashMap<_, _> = manifest
        .children()
        .filter(|node| node.has_tag_name("item"))
        .filter_map(|node| Some((node.attribute("id")?, node.attribute("href")?)))
        .collect();
    let spine = opf
        .descendants()
        .find(|node| node.has_tag_name("spine"))
        .ok_or("EPUB 缺少阅读顺序")?;
    let mut content = String::new();
    let mut offset = 0;
    let mut nodes = Vec::new();
    for node in spine.children().filter(|node| node.has_tag_name("itemref")) {
        let href = node
            .attribute("idref")
            .and_then(|id| chapters.get(id))
            .ok_or("EPUB 章节引用无效")?;
        let url = base.join(href).map_err(|_| "EPUB 章节路径无效")?;
        if url.origin() != base.origin() {
            return Err("EPUB 章节必须位于电子书内".into());
        }
        let path = percent_encoding::percent_decode_str(url.path().trim_start_matches('/'))
            .decode_utf8()
            .map_err(|_| "EPUB 章节路径编码无效")?;
        let html = read_entry(&mut zip, &path)?;
        let chapter = Document::from(html.as_str());
        chapter
            .select("script, style, template, noscript, img, svg")
            .remove();
        let body = chapter.select("body");
        let Some(root) = body.nodes().first() else {
            nodes.push(Node::Element {
                tag: "section".into(),
                children: Vec::new(),
            });
            continue;
        };
        let (text, mut layout, _) = web_layout::extract(root, &url);
        if text.trim().is_empty() {
            nodes.push(Node::Element {
                tag: "section".into(),
                children: Vec::new(),
            });
            continue;
        }
        let mut children = Vec::new();
        if body.select("h1, h2, h3, h4, h5, h6").nodes().is_empty() {
            let title = format!("第 {} 章", nodes.len() + 1);
            let end = offset + title.encode_utf16().count();
            children.push(Node::Element {
                tag: "h2".into(),
                children: vec![Node::Text { start: offset, end }],
            });
            content.push_str(&title);
            content.push_str("\n\n");
            offset = end + 2;
        }
        shift_nodes(&mut layout.nodes, offset);
        children.extend(layout.nodes);
        nodes.push(Node::Element {
            tag: "section".into(),
            children,
        });
        content.push_str(&text);
        offset += text.encode_utf16().count();
        if !content.ends_with("\n\n") {
            content.push_str("\n\n");
            offset += 2;
        }
        if content.len() > TEXT_LIMIT {
            return Err("EPUB 正文超过 10 MB".into());
        }
    }
    if content.is_empty() {
        return Err("EPUB 没有可导入的正文。受保护或仅含图片的电子书无法导入。".into());
    }
    Ok((
        content,
        Layout {
            nodes,
            images: Vec::new(),
            image_errors: Vec::new(),
        },
    ))
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Write};
    use zip::{write::SimpleFileOptions, ZipWriter};

    #[test]
    fn imports_spine_order_and_ignores_active_markup() {
        let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
        for (name, content) in [
            ("META-INF/container.xml", r#"<container><rootfiles><rootfile full-path="Book/contents.opf"/></rootfiles></container>"#),
            ("Book/contents.opf", r#"<package><manifest><item id="later" href="b.xhtml"/><item id="first" href="a%20chapter.xhtml"/></manifest><spine><itemref idref="first"/><itemref idref="later"/></spine></package>"#),
            ("Book/b.xhtml", "<html><body><p>Second chapter</p></body></html>"),
            ("Book/a chapter.xhtml", "<html><body><script>ignore me</script><h1>First chapter</h1><p>Hello world.</p></body></html>"),
        ] {
            zip.start_file(name, SimpleFileOptions::default()).unwrap();
            zip.write_all(content.as_bytes()).unwrap();
        }
        let mut file = zip.finish().unwrap();
        file.set_position(0);
        let (text, layout) = super::read(file).unwrap();
        assert_eq!(layout.nodes.len(), 2);
        assert!(text.starts_with("First chapter"));
        assert!(text.trim_end().ends_with("第 2 章\n\nSecond chapter"));
        assert!(text.contains("Hello world."));
        assert!(!text.contains("ignore me"));
    }
}
