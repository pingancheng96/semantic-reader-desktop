use crate::{
    sqlite::Result,
    web_layout::{self, Layout},
};
use dom_query::Document;
use encoding_rs::{Encoding, UTF_8, WINDOWS_1252};
use reqwest::{header, redirect::Policy, Client, Url};
use std::{net::IpAddr, time::Duration};

const MAX_HTML: usize = 5_000_000;
const MAX_TEXT: usize = 2_000_000;

pub struct Page {
    pub url: String,
    pub title: String,
    pub content: String,
    pub layout: Option<Layout>,
    pub images: Vec<Vec<Url>>,
}

pub fn parse_url(value: &str) -> Result<Url> {
    let mut url = Url::parse(value.trim()).map_err(|_| "请填写完整的 HTTP 或 HTTPS 网页地址")?;
    if value.len() > 4096
        || !matches!(url.scheme(), "http" | "https")
        || url.host().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err("仅支持无登录凭据的公开 HTTP 或 HTTPS 网页地址".into());
    }
    if url.as_str().len() > 4096 {
        return Err("网页地址过长".into());
    }
    url.set_fragment(None);
    Ok(url)
}

fn public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => match ip.octets() {
            [0 | 10 | 127 | 224..=255, ..]
            | [100, 64..=127, ..]
            | [169, 254, ..]
            | [172, 16..=31, ..]
            | [192, 168, ..]
            | [192, 0, 0 | 2, _]
            | [192, 88, 99, _]
            | [198, 18 | 19, ..]
            | [198, 51, 100, _]
            | [203, 0, 113, _] => false,
            _ => true,
        },
        IpAddr::V6(ip) => match ip.segments() {
            [0x2000..=0x3fff, ..] => !matches!(
                ip.segments(),
                [0x2001, 0..=0x1ff, ..] | [0x2001, 0xdb8, ..] | [0x2002, ..] | [0x3fff, ..]
            ),
            _ => false,
        },
    }
}

pub async fn fetch(url: Url) -> Result<Page> {
    let mut page = load_page(url).await?;
    fill_images(&mut page).await;
    if let Some(layout) = &mut page.layout {
        layout.cap_images();
    }
    Ok(page)
}

pub async fn load_page(url: Url) -> Result<Page> {
    tokio::time::timeout(Duration::from_secs(25), fetch_page(url))
        .await
        .map_err(|_| "网页导入超时")?
}

pub async fn fill_images(page: &mut Page) {
    let Some(layout) = &mut page.layout else {
        return;
    };
    let mut downloads = tokio::task::JoinSet::new();
    let concurrency = std::sync::Arc::new(tokio::sync::Semaphore::new(4));
    let referer = Url::parse(&page.url).ok();
    layout.image_errors.resize(layout.images.len(), None);
    for (index, urls) in page.images.drain(..).enumerate() {
        if layout.images[index].is_some() {
            continue;
        }
        let referer = referer.clone();
        let concurrency = concurrency.clone();
        downloads.spawn(async move {
            let _permit = concurrency.acquire().await.expect("semaphore stays open");
            let data =
                tokio::time::timeout(Duration::from_secs(40), fetch_image(urls, referer.as_ref()))
                    .await
                    .unwrap_or_else(|_| Err("尝试所有图片地址时超时".into()));
            (index, data)
        });
    }
    while let Some(result) = downloads.join_next().await {
        if let Ok((index, data)) = result {
            match data {
                Ok(image) => {
                    layout.images[index] = Some(image);
                    layout.image_errors[index] = None;
                }
                Err(message) => {
                    layout.image_errors[index] = Some(message);
                }
            }
        }
    }
}

async fn fetch_image(urls: Vec<Url>, referer: Option<&Url>) -> Result<String> {
    let mut failure = "未找到可下载的图片地址".to_owned();
    for url in urls {
        let attempt = tokio::time::timeout(
            Duration::from_secs(13),
            download(
                url,
                "image/avif,image/webp,image/png,image/jpeg,image/gif,image/*",
                16_000_000,
                referer,
            ),
        )
        .await;
        match attempt {
            Ok(Ok((_, _, bytes))) => {
                let result = if bytes.len() > 1_000_000 {
                    tauri::async_runtime::spawn_blocking(move || prepare_image(&bytes))
                        .await
                        .unwrap_or_else(|_| Err("图片缩放失败".into()))
                } else {
                    prepare_image(&bytes)
                };
                match result {
                    Ok(data) => return Ok(data),
                    Err(error) => failure = error,
                }
            }
            Ok(Err(error)) => failure = error,
            Err(_) => failure = "图片下载超时".into(),
        }
    }
    Err(failure)
}

fn prepare_image(bytes: &[u8]) -> Result<String> {
    if web_layout::image_mime(bytes).is_none() {
        return Err("服务器返回的内容不是支持的图片格式".into());
    }
    #[cfg(target_os = "macos")]
    if bytes.len() > 1_000_000 && !bytes.starts_with(b"GIF") {
        if let Some(smaller) = resize_image(bytes) {
            return Ok(smaller);
        }
    }
    if bytes.len() > web_layout::IMAGE_BYTES {
        return Err("图片过大，缩小失败".into());
    }
    web_layout::image_uri(bytes).ok_or("图片格式不受支持".into())
}

#[cfg(target_os = "macos")]
fn resize_image(bytes: &[u8]) -> Option<String> {
    use std::{
        fs,
        process::{Command, Stdio},
    };
    let dir = std::env::temp_dir().join(format!("semantic-image-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&dir).ok()?;
    let result = (|| {
        let input = dir.join("input");
        let png = web_layout::image_mime(bytes) == Some("png");
        let output = dir.join(if png { "output.png" } else { "output.jpg" });
        fs::write(&input, bytes).ok()?;
        let mut command = Command::new("/usr/bin/sips");
        command.args([
            "-Z",
            "1600",
            "-s",
            "format",
            if png { "png" } else { "jpeg" },
        ]);
        if !png {
            command.args(["-s", "formatOptions", "82"]);
        }
        let status = command
            .arg(input)
            .arg("--out")
            .arg(&output)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .ok()?;
        if !status.success() || fs::metadata(&output).ok()?.len() > web_layout::IMAGE_BYTES as u64 {
            return None;
        }
        web_layout::image_uri(&fs::read(output).ok()?)
    })();
    let _ = fs::remove_dir_all(dir);
    result
}

async fn fetch_page(url: Url) -> Result<Page> {
    let (url, content_type, bytes) = download(
        url,
        "text/html, application/xhtml+xml, text/plain",
        MAX_HTML,
        None,
    )
    .await?;
    let media_type = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if !matches!(
        media_type.as_str(),
        "" | "text/html" | "application/xhtml+xml" | "text/plain"
    ) {
        return Err("这个地址未返回 HTML 网页或纯文本".into());
    }
    tauri::async_runtime::spawn_blocking(move || extract(url, &content_type, &media_type, &bytes))
        .await
        .map_err(|_| "网页解析失败")?
}

async fn download(
    mut url: Url,
    accept: &str,
    limit: usize,
    referer: Option<&Url>,
) -> Result<(Url, String, Vec<u8>)> {
    for _ in 0..=5 {
        let mut builder = Client::builder()
            .no_proxy()
            .redirect(Policy::none())
            .connect_timeout(Duration::from_secs(8))
            .user_agent("SemanticReader/0.4");
        match url.host().unwrap() {
            url::Host::Domain(host) => {
                let addresses: Vec<_> =
                    tokio::net::lookup_host((host, url.port_or_known_default().unwrap()))
                        .await
                        .map_err(|_| "无法解析网页域名")?
                        .collect();
                if addresses.is_empty() || addresses.iter().any(|address| !public(address.ip())) {
                    return Err("网页导入仅允许公开网络地址".into());
                }
                builder = builder.resolve_to_addrs(host, &addresses);
            }
            url::Host::Ipv4(ip) if !public(ip.into()) => {
                return Err("网页导入仅允许公开网络地址".into())
            }
            url::Host::Ipv6(ip) if !public(ip.into()) => {
                return Err("网页导入仅允许公开网络地址".into())
            }
            _ => (),
        }
        let client = builder.build().map_err(|_| "无法初始化网页请求")?;
        let mut request = client.get(url.clone()).header(header::ACCEPT, accept);
        if let Some(page) = referer {
            if url.scheme() == "https" || page.scheme() != "https" {
                let value = if url.origin() == page.origin() {
                    page.to_string()
                } else {
                    format!("{}/", page.origin().ascii_serialization())
                };
                request = request.header(header::REFERER, value);
            }
        }
        let mut response = request
            .send()
            .await
            .map_err(|_| "无法访问网页，请检查地址和网络连接")?;
        if matches!(response.status().as_u16(), 301 | 302 | 303 | 307 | 308) {
            let location = response
                .headers()
                .get(header::LOCATION)
                .and_then(|value| value.to_str().ok())
                .ok_or("网页重定向缺少有效地址")?;
            let next = url.join(location).map_err(|_| "网页重定向地址无效")?;
            url = parse_url(next.as_str())?;
            continue;
        }
        if !response.status().is_success() {
            return Err(format!("网页返回 HTTP {}", response.status().as_u16()));
        }
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_owned();
        if response
            .content_length()
            .is_some_and(|len| len > limit as u64)
        {
            return Err("下载内容超过大小限制".into());
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| "网页下载中断")? {
            if bytes.len() + chunk.len() > limit {
                return Err("下载内容超过大小限制".into());
            }
            bytes.extend_from_slice(&chunk);
        }
        return Ok((url, content_type, bytes));
    }
    Err("网页重定向次数过多".into())
}

fn charset(parameters: &str) -> Option<&'static Encoding> {
    parameters.split(';').find_map(|part| {
        let (key, value) = part.split_once('=')?;
        if !key.trim().eq_ignore_ascii_case("charset") {
            return None;
        }
        Encoding::for_label(value.trim().trim_matches(['"', '\'']).as_bytes())
    })
}

pub fn browser_page(url: &str, html: &str) -> Result<Page> {
    if html.len() > MAX_HTML { return Err("网页内容过大".into()); }
    let doc = Document::from(html);
    let title = doc.select_single("title").text().to_lowercase();
    if title.contains("just a moment") || title.contains("attention required") ||
        !doc.select("#challenge-running, #challenge-stage").nodes().is_empty() {
        return Err("网页仍在进行浏览器验证，请完成后再导入".into());
    }
    extract(parse_url(url)?, "text/html; charset=utf-8", "text/html", html.as_bytes())
}

fn extract(url: Url, content_type: &str, media_type: &str, bytes: &[u8]) -> Result<Page> {
    let encoding = Encoding::for_bom(bytes)
        .map(|(encoding, _)| encoding)
        .or_else(|| charset(content_type))
        .or_else(|| {
            if media_type == "text/plain" {
                return None;
            }
            let prefix = String::from_utf8_lossy(&bytes[..bytes.len().min(4096)]);
            let doc = Document::from(prefix.as_ref());
            let meta = doc.select_single("meta[charset]");
            meta.attr("charset")
                .and_then(|label| Encoding::for_label(label.as_bytes()))
                .or_else(|| {
                    doc.select_single("meta[http-equiv='content-type' i]")
                        .attr("content")
                        .and_then(|value| charset(&value))
                })
        })
        .unwrap_or_else(|| {
            if std::str::from_utf8(bytes).is_ok() {
                UTF_8
            } else {
                WINDOWS_1252
            }
        });
    let (text, _, _) = encoding.decode(bytes);
    let fallback = url.host_str().unwrap_or("网页").to_owned();
    let (title, content, layout, images) = if media_type == "text/plain" {
        (
            fallback.to_owned(),
            text.trim().to_owned(),
            None,
            Vec::new(),
        )
    } else {
        let doc = Document::from(text.as_ref());
        doc.select("script, style, noscript, template, nav, footer, aside, form, iframe, svg, [hidden], [aria-hidden='true'], [role='navigation'], [role='banner'], [role='contentinfo']").remove();
        let title = doc.select_single("head title").text();
        let wikipedia = url.host_str().is_some_and(|host| host=="wikipedia.org" || host.ends_with(".wikipedia.org"));
        if wikipedia {
            doc.select("#p-lang-btn, #p-lang, .mw-portlet-lang, .interlanguage-link, .mw-editsection").remove();
        }
        let selectors = if wikipedia {
            vec!["#mw-content-text .mw-parser-output", "#mw-content-text", "main article", "article", "main", "body"]
        } else {
            vec!["main article", "article", "main, [role='main']", "body"]
        };
        let root = selectors
            .into_iter()
            .find_map(|selector| {
                doc.select(selector)
                    .nodes()
                    .iter()
                    .take(30)
                    .map(|node| (node, node.formatted_text().len()))
                    .filter(|(_, len)| *len > 0)
                    .max_by_key(|(_, len)| *len)
                    .map(|(node, _)| node.clone())
            })
            .ok_or("没有提取到可阅读的正文")?;
        let base = doc
            .select_single("base[href]")
            .attr("href")
            .and_then(|href| url.join(&href).ok())
            .unwrap_or_else(|| url.clone());
        let (content, layout, images) = web_layout::extract(&root, &base);
        (title.to_string(), content, Some(layout), images)
    };
    let title: String = title
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(255)
        .collect();
    if content.trim().is_empty() {
        return Err("没有提取到可阅读的正文。此网页可能依赖 JavaScript 或需要登录。".into());
    }
    if content.len() > MAX_TEXT {
        return Err("提取出的正文超过 2 MB".into());
    }
    Ok(Page {
        url: url.into(),
        title: if title.is_empty() {
            fallback.to_owned()
        } else {
            title
        },
        content,
        layout,
        images,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "macos")]
    #[test]
    fn wikipedia_uses_article_body_without_language_menu() {
        let html = br#"<html><head><title>Christmas</title></head><body><main>
          <div id="p-lang-btn"><ul><li class="interlanguage-link">Language list noise</li></ul></div>
          <div id="mw-content-text"><div class="mw-parser-output"><h2>History</h2><p>Article text.</p></div></div>
          <div>Other interface text</div></main></body></html>"#;
        let page = extract(Url::parse("https://en.wikipedia.org/wiki/Christmas").unwrap(),
            "text/html; charset=utf-8", "text/html",html).unwrap();
        assert!(page.content.contains("Article text."));
        assert!(!page.content.contains("Language list noise"));
        assert!(!page.content.contains("Other interface text"));
    }

    #[test]
    fn macos_can_resize_supported_images_and_reject_invalid_content() {
        let bytes = include_bytes!("../icons/icon.png");
        assert!(resize_image(bytes).is_some_and(|uri| uri.starts_with("data:image/png;")));
        assert!(prepare_image(b"<html>blocked</html>")
            .unwrap_err()
            .contains("图片格式"));
    }

    #[test]
    fn extracts_article_without_navigation_or_scripts() {
        let page = extract(parse_url("https://example.org/story#section").unwrap(), "text/html", "text/html",
            b"<title>One &amp; Two</title><nav>Menu</nav><main><article><h1>Heading</h1><p>Prices <b>plunged</b>.</p><script>secret()</script><p>Second paragraph.</p></article></main><footer>Links</footer>").unwrap();
        assert_eq!(page.title, "One & Two");
        assert_eq!(page.url, "https://example.org/story");
        assert_eq!(
            page.content.trim(),
            "Heading\n\nPrices plunged.\n\nSecond paragraph."
        );
    }

    #[test]
    fn preserves_figure_table_and_utf16_text_anchors() {
        let url = parse_url("https://example.org/articles/one").unwrap();
        let page = extract(url, "text/html", "text/html",
          b"<article><h2>Markets</h2><figure><img src='../chart.png' alt='Chart'><figcaption>Prices</figcaption></figure><p>&#128201; Prices <em>plunged</em>.</p><table><tr><th>Year</th><td>2026</td></tr></table></article>").unwrap();
        assert_eq!(page.images[0][0].as_str(), "https://example.org/chart.png");
        let mut layout = page.layout.unwrap();
        layout.images[0] = web_layout::image_uri(b"\x89PNG\r\n\x1a\n");
        let value = serde_json::to_value(&layout).unwrap();
        assert_eq!(value["nodes"][0]["tag"], "h2");
        assert_eq!(value["nodes"][1]["tag"], "figure");
        assert_eq!(value["nodes"][3]["tag"], "table");
        let token = &value["nodes"][2]["children"][1]["children"][0];
        let units: Vec<_> = page.content.encode_utf16().collect();
        assert_eq!(
            String::from_utf16(
                &units[token["start"].as_u64().unwrap() as usize
                    ..token["end"].as_u64().unwrap() as usize]
            )
            .unwrap(),
            "plunged"
        );
        assert!(web_layout::validate(&value.to_string()).is_ok());
        assert!(web_layout::image_uri(b"<svg></svg>").is_none());
    }

    #[test]
    fn keeps_alternative_picture_urls_and_recognizes_caption_and_avif() {
        let page = extract(parse_url("https://example.org/article").unwrap(), "text/html", "text/html",
            br#"<article><picture><source srcset="/p.avif 800w"><img data-src="/lazy.jpg" srcset="/big.jpg 1600w, /medium.jpg 800w" src="/fallback.jpg"></picture><p class="wp-caption-text">A caption</p></article>"#).unwrap();
        let paths: Vec<_> = page.images[0].iter().map(Url::path).collect();
        assert_eq!(
            paths,
            [
                "/lazy.jpg",
                "/medium.jpg",
                "/big.jpg",
                "/p.avif",
                "/fallback.jpg"
            ]
        );
        assert_eq!(
            serde_json::to_value(page.layout.unwrap()).unwrap()["nodes"][1]["tag"],
            "figcaption"
        );
        assert!(web_layout::image_uri(b"0000ftypavif")
            .unwrap()
            .starts_with("data:image/avif;"));
    }

    #[test]
    fn handles_legacy_charset_and_empty_pages() {
        let url = parse_url("https://example.org").unwrap();
        let page = extract(
            url.clone(),
            "text/plain; charset=windows-1252",
            "text/plain",
            b"caf\xe9",
        )
        .unwrap();
        assert_eq!(page.content, "café");
        assert!(extract(url, "text/html", "text/html", b"<script>app()</script>").is_err());
    }

    #[test]
    fn blocks_non_web_urls_and_non_public_targets() {
        for invalid in [
            "file:///etc/passwd",
            "http://user:pass@example.org/",
            "javascript:alert(1)",
        ] {
            assert!(parse_url(invalid).is_err());
        }
        for address in [
            "127.0.0.1",
            "169.254.169.254",
            "10.1.0.2",
            "100.64.0.1",
            "::1",
            "::ffff:127.0.0.1",
            "fd00::1",
            "2002:7f00:1::1",
        ] {
            assert!(!public(address.parse().unwrap()));
        }
        assert!(public("8.8.8.8".parse().unwrap()));
        assert!(public("2606:4700:4700::1111".parse().unwrap()));
    }
}
