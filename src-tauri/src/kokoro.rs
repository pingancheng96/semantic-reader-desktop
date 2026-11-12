use sha2::{Digest, Sha256};
use std::{
    io::{Read, Seek, SeekFrom},
    path::Path,
};
use tauri::ipc::Channel;
use tokio::io::AsyncWriteExt;

type Result<T> = std::result::Result<T, String>;
const SOURCE: &str = "https://huggingface.co/onnx-community/Kokoro-82M-v1.0-ONNX/resolve/1939ad2a8e416c0acfeecc08a694d14ef25f2231";
pub const MODEL_SIZE: u64 = 325_532_232;
const ASSETS: [(&str, &str, u64, &str); 3] = [
    (
        "model.onnx",
        "onnx/model.onnx",
        MODEL_SIZE,
        "8fbea51ea711f2af382e88c833d9e288c6dc82ce5e98421ea61c058ce21a34cb",
    ),
    (
        "af_heart.bin",
        "voices/af_heart.bin",
        522_240,
        "d583ccff3cdca2f7fae535cb998ac07e9fcb90f09737b9a41fa2734ec44a8f0b",
    ),
    (
        "bf_emma.bin",
        "voices/bf_emma.bin",
        522_240,
        "669fe0647f9dd04fcab92f1439a40eeb4c8b4ab1f82e4996fe3d918ce4a63b73",
    ),
];

#[derive(Clone, Copy, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Variant {
    Fp32,
    Int8,
}

impl Variant {
    pub fn directory(self) -> &'static str {
        match self {
            Self::Fp32 => "kokoro-v1-fp32",
            Self::Int8 => "kokoro-v1-int8",
        }
    }

    fn assets(self) -> [(&'static str, &'static str, u64, &'static str); 3] {
        let mut assets = ASSETS;
        if matches!(self, Self::Int8) {
            assets[0] = (
                "model.onnx",
                "onnx/model_quantized.onnx",
                92_361_116,
                "fbae9257e1e05ffc727e951ef9b9c98418e6d79f1c9b6b13bd59f5c9028a1478",
            );
        }
        assets
    }
}

#[derive(Clone, serde::Serialize)]
pub struct Progress {
    bytes: u64,
    total: u64,
}

pub fn installed(root: &Path, variant: Variant) -> bool {
    variant.assets().iter().all(|(name, _, size, _)| {
        root.join(name)
            .metadata()
            .is_ok_and(|meta| meta.is_file() && meta.len() == *size)
    })
}

pub async fn install(root: &Path, variant: Variant, progress: Channel<Progress>) -> Result<()> {
    if installed(root, variant) {
        return Ok(());
    }
    let parent = root.parent().ok_or("找不到离线发音目录")?;
    tokio::fs::create_dir_all(parent)
        .await
        .map_err(|e| e.to_string())?;
    let stage = parent.join(format!(".kokoro-{}", uuid::Uuid::new_v4()));
    tokio::fs::create_dir(&stage)
        .await
        .map_err(|e| e.to_string())?;
    let result = async {
        let client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(20))
            .read_timeout(std::time::Duration::from_secs(45))
            .build()
            .map_err(|e| e.to_string())?;
        let assets = variant.assets();
        let total = assets.iter().map(|asset| asset.2).sum();
        let mut downloaded = 0;
        let mut reported = 0;
        for (name, path, size, expected) in assets {
            let mut response = client
                .get(format!("{SOURCE}/{path}"))
                .send()
                .await
                .and_then(reqwest::Response::error_for_status)
                .map_err(|e| format!("Kokoro 下载失败（{e}）"))?;
            let mut output = tokio::fs::File::create(stage.join(name))
                .await
                .map_err(|e| e.to_string())?;
            let mut digest = Sha256::new();
            let mut length = 0;
            while let Some(chunk) = response.chunk().await.map_err(|e| e.to_string())? {
                length += chunk.len() as u64;
                if length > size {
                    return Err("Kokoro 文件超过预期大小".into());
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
                return Err("Kokoro 文件校验失败，请重新下载".into());
            }
        }
        if installed(root, variant) {
            return Ok(());
        }
        if root.exists() {
            tokio::fs::remove_dir_all(root)
                .await
                .map_err(|e| e.to_string())?;
        }
        match tokio::fs::rename(&stage, root).await {
            Ok(()) => {}
            Err(_) if installed(root, variant) => {}
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
    variant: Variant,
    asset: usize,
    offset: u64,
    length: usize,
) -> Result<Vec<u8>> {
    let assets = variant.assets();
    let (name, _, size, _) = assets.get(asset).ok_or("未知 Kokoro 文件")?;
    if length == 0
        || length > 8 * 1024 * 1024
        || offset
            .checked_add(length as u64)
            .is_none_or(|end| end > *size)
    {
        return Err("Kokoro 读取范围无效".into());
    }
    let mut file =
        std::fs::File::open(root.join(name)).map_err(|_| "请先下载 Kokoro 离线发音模型")?;
    file.seek(SeekFrom::Start(offset))
        .map_err(|e| e.to_string())?;
    let mut data = vec![0; length];
    file.read_exact(&mut data).map_err(|e| e.to_string())?;
    Ok(data)
}
