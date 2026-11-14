use crate::sqlite::Result;
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::Duration;

const REPOSITORY: &str = "pingancheng96/semantic-reader-releases";

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    body: Option<String>,
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}

fn version_parts(tag: &str) -> Option<[u64; 3]> {
    let mut parts = tag.trim_start_matches('v').split('.');
    let version = [
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
    ];
    parts.next().is_none().then_some(version)
}

pub async fn check() -> Result<Value> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(8))
        .user_agent("Semantic-Reader")
        .build()
        .map_err(|_| "无法初始化更新检查")?;
    let response = client
        .get(format!("https://api.github.com/repos/{REPOSITORY}/releases/latest"))
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|_| "无法连接 GitHub，请稍后重试。")?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Err("GitHub 发布信息不可公开读取。可在浏览器登录 GitHub 查看。".into());
    }
    let release: Release = response.error_for_status()
        .map_err(|_| "GitHub 暂时无法返回发布信息")?
        .json().await.map_err(|_| "无法读取发布信息")?;
    let current = version_parts(env!("CARGO_PKG_VERSION")).ok_or("当前版本号无效")?;
    let latest = version_parts(&release.tag_name).ok_or("发布版本号无效")?;
    let platform = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "macos-arm64.zip",
        ("macos", "x86_64") => "macos-x64.zip",
        ("windows", "x86_64") => "windows-x64-setup.exe",
        _ => "",
    };
    let asset = release.assets.into_iter().find(|asset| {
        !platform.is_empty() && asset.name.to_ascii_lowercase().ends_with(platform)
            && asset.browser_download_url.starts_with(&format!("https://github.com/{REPOSITORY}/releases/download/"))
    });
    Ok(json!({
        "available": latest > current,
        "version": release.tag_name,
        "notes": release.body.unwrap_or_default(),
        "download": asset.map(|asset| asset.browser_download_url),
    }))
}

pub fn open_link(value: &str) -> Result<Value> {
    let url = reqwest::Url::parse(value).map_err(|_| "下载链接无效")?;
    let allowed = match url.host_str() {
        Some("github.com") => url.path().starts_with(&format!("/{REPOSITORY}/releases")),
        _ => false,
    };
    if !allowed || url.scheme() != "https" || !url.username().is_empty()
        || url.password().is_some() || url.port().is_some()
    {
        return Err("GitHub 发布链接无效。".into());
    }
    let opener = match std::env::consts::OS {
        "macos" => "/usr/bin/open",
        "windows" => "explorer.exe",
        _ => "xdg-open",
    };
    std::process::Command::new(opener).arg(url.as_str()).spawn()
        .map_err(|_| "无法打开浏览器")?;
    Ok(Value::Null)
}

pub fn info() -> Value {
    json!({
        "current": env!("CARGO_PKG_VERSION"),
        "releases": format!("https://github.com/{REPOSITORY}/releases"),
    })
}
