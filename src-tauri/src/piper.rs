use sha2::{Digest, Sha256};
use std::{
    io::{Read, Seek, SeekFrom},
    path::Path,
};
use tauri::ipc::Channel;
use tokio::io::AsyncWriteExt;

type Result<T> = std::result::Result<T, String>;
const SOURCE: &str = "https://huggingface.co/rhasspy/piper-voices/resolve/e9c8a39444359388adaa94255becb37a5a2b8e51/da/da_DK/talesyntese/medium";
pub const DIRECTORY: &str = "piper-da-talesyntese-v1";
pub const ASSETS: [(&str, &str, u64, &str); 3] = [
    ("model.onnx","da_DK-talesyntese-medium.onnx",63201294,"b9271efd25f7b8494bbd28d48dd675c8c119daa284f3ee488008935f515f1241"),
    ("config.json","da_DK-talesyntese-medium.onnx.json",4878,"89fe13bd251406cc0088570d103ea7ac35823211d8466faf913268ab8506f41b"),
    ("MODEL_CARD","MODEL_CARD",308,"b5b7a51d4d815a307594619a519eb389b2e17839b30c0eb69a39d3ade5b1ed65"),
];

#[derive(Clone, serde::Serialize)]
pub struct Progress {
    bytes: u64,
    total: u64,
}

pub fn installed(root: &Path) -> bool {
    ASSETS.iter().all(|(name, _, size, _)| {
        root.join(name)
            .metadata()
            .is_ok_and(|meta| meta.is_file() && meta.len() == *size)
    })
}

pub async fn install(root: &Path, progress: Channel<Progress>) -> Result<()> {
    if installed(root) {
        return Ok(());
    }
    let parent = root.parent().ok_or("找不到离线发音目录")?;
    tokio::fs::create_dir_all(parent)
        .await
        .map_err(|e| e.to_string())?;
    let stage = parent.join(format!(".piper-{}", uuid::Uuid::new_v4()));
    tokio::fs::create_dir(&stage)
        .await
        .map_err(|e| e.to_string())?;
    let result = async {
        let client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(20))
            .read_timeout(std::time::Duration::from_secs(45))
            .build()
            .map_err(|e| e.to_string())?;
        let assets = ASSETS;
        let total = assets.iter().map(|asset| asset.2).sum();
        let mut downloaded = 0;
        let mut reported = 0;
        for (name, path, size, expected) in assets {
            let mut response = client
                .get(format!("{SOURCE}/{path}"))
                .send()
                .await
                .and_then(reqwest::Response::error_for_status)
                .map_err(|e| format!("Piper 下载失败（{e}）"))?;
            let mut output = tokio::fs::File::create(stage.join(name))
                .await
                .map_err(|e| e.to_string())?;
            let mut digest = Sha256::new();
            let mut length = 0;
            while let Some(chunk) = response.chunk().await.map_err(|e| e.to_string())? {
                length += chunk.len() as u64;
                if length > size {
                    return Err("Piper 文件超过预期大小".into());
                }
                digest.update(&chunk);
                output.write_all(&chunk).await.map_err(|e| e.to_string())?;
                downloaded += chunk.len() as u64;
                if downloaded - reported >= 512 * 1024 {
                    let _ = progress.send(Progress {
                        bytes: downloaded,
                        total,
                    });
                    reported = downloaded;
                }
            }
            output.flush().await.map_err(|e| e.to_string())?;
            if length != size || format!("{:x}", digest.finalize()) != expected {
                return Err("Piper 文件校验失败，请重新下载".into());
            }
        }
        if installed(root) {
            return Ok(());
        }
        if root.exists() {
            tokio::fs::remove_dir_all(root)
                .await
                .map_err(|e| e.to_string())?;
        }
        match tokio::fs::rename(&stage, root).await {
            Ok(()) => {}
            Err(_) if installed(root) => {}
            Err(error) => return Err(error.to_string()),
        }
        let _ = progress.send(Progress {
            bytes: total,
            total,
        });
        Ok(())
    }
    .await;
    let _ = tokio::fs::remove_dir_all(&stage).await;
    result
}

pub fn read(
    root: &Path,
    asset: usize,
    offset: u64,
    length: usize,
) -> Result<Vec<u8>> {
    let assets = ASSETS;
    let (name, _, size, _) = assets.get(asset).ok_or("未知 Piper 文件")?;
    if length == 0
        || length > 8 * 1024 * 1024
        || offset
            .checked_add(length as u64)
            .is_none_or(|end| end > *size)
    {
        return Err("Piper 读取范围无效".into());
    }
    let mut file =
        std::fs::File::open(root.join(name)).map_err(|_| "请先下载 Piper 离线发音模型")?;
    file.seek(SeekFrom::Start(offset))
        .map_err(|e| e.to_string())?;
    let mut data = vec![0; length];
    file.read_exact(&mut data).map_err(|e| e.to_string())?;
    Ok(data)
}
