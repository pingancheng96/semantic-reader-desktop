use crate::sqlite::Result;
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::Duration;

const REPOSITORY: &str = "pingancheng96/semantic-reader-desktop";
const LEGACY_REPOSITORY: &str = "pingancheng96/semantic-reader-releases";

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

fn release_url_allowed(value:&str,repository:&str,download:bool) -> bool {
    let Ok(url)=reqwest::Url::parse(value) else {return false;};
    let prefix=if download {format!("/{repository}/releases/download/")} else {format!("/{repository}/releases")};
    url.scheme()=="https" && url.host_str()==Some("github.com") && url.username().is_empty()
        && url.password().is_none() && url.port().is_none() && url.query().is_none() && url.fragment().is_none()
        && if download {url.path().starts_with(&prefix)} else {url.path()==prefix || url.path().starts_with(&(prefix+"/"))}
}

fn matches_platform(name:&str,os:&str,arch:&str) -> bool {
    let name=name.to_ascii_lowercase();
    match (os,arch) {
        ("macos","aarch64")=>name.ends_with("macos-arm64.zip") || name.ends_with("macos-aarch64.zip"),
        ("macos","x86_64")=>name.ends_with("macos-x64.zip") || name.ends_with("macos-x86_64.zip"),
        ("windows","x86_64")=>name.ends_with("windows-x64-setup.exe") || name.ends_with("_x64-setup.exe") || name.ends_with("-x64-setup.exe"),
        _=>false,
    }
}

pub async fn check() -> Result<Value> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(8))
        .user_agent("Semantic-Reader")
        .build()
        .map_err(|_| "无法初始化更新检查")?;
    let mut found=None;
    let mut failure="无法连接 GitHub，请稍后重试。";
    for repository in [REPOSITORY,LEGACY_REPOSITORY] {
        let response=match client.get(format!("https://api.github.com/repos/{repository}/releases/latest"))
            .header("Accept","application/vnd.github+json").send().await {
                Ok(response)=>response,Err(_)=>continue,
            };
        if !response.status().is_success() {failure="GitHub 暂时无法返回发布信息";continue;}
        match response.json::<Release>().await {
            Ok(release)=>{found=Some((release,repository));break;},
            Err(_)=>failure="无法读取发布信息",
        }
    }
    let (release,repository)=found.ok_or(failure)?;
    let current = version_parts(env!("CARGO_PKG_VERSION")).ok_or("当前版本号无效")?;
    let latest = version_parts(&release.tag_name).ok_or("发布版本号无效")?;
    let asset = release.assets.into_iter().find(|asset| {
        matches_platform(&asset.name,std::env::consts::OS,std::env::consts::ARCH)
            && release_url_allowed(&asset.browser_download_url,repository,true)
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
    if ![REPOSITORY,LEGACY_REPOSITORY].iter().any(|repository|release_url_allowed(value,repository,false)) {
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn accepts_current_and_legacy_installer_names_without_cross_platform_matches() {
        assert!(matches_platform("Semantic.Reader_0.20.27_x64-setup.exe","windows","x86_64"));
        assert!(matches_platform("Semantic-Reader-0.20.27-Windows-x64-setup.exe","windows","x86_64"));
        assert!(matches_platform("Semantic-Reader-macOS-arm64.zip","macos","aarch64"));
        assert!(matches_platform("Semantic-Reader-macOS-x86_64.zip","macos","x86_64"));
        assert!(!matches_platform("Semantic-Reader-Windows-build-source.zip","windows","x86_64"));
        assert!(!matches_platform("Semantic-Reader-macOS-arm64.zip","macos","x86_64"));
        assert!(!matches_platform("Semantic.Reader_0.20.27_arm64-setup.exe","windows","x86_64"));
    }
    #[test]
    fn release_links_are_limited_to_the_two_exact_repositories() {
        for repo in [REPOSITORY,LEGACY_REPOSITORY] {
            assert!(release_url_allowed(&format!("https://github.com/{repo}/releases/download/v0.20.27/app.exe"),repo,true));
            assert!(release_url_allowed(&format!("https://github.com/{repo}/releases"),repo,false));
            assert!(!release_url_allowed(&format!("https://github.com/{repo}/releases-other"),repo,false));
            assert!(!release_url_allowed(&format!("https://evil.example/{repo}/releases"),repo,false));
            assert!(!release_url_allowed(&format!("https://user@github.com/{repo}/releases"),repo,false));
        }
    }
}
