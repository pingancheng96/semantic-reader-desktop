use crate::{
    sqlite::{Db, Result},
    store,
};
use serde_json::{json, Value};
use std::{
    fs::{self, File},
    io::{BufReader, BufWriter, Write},
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;

fn snapshot_metadata(id: &str) -> Option<Value> {
    let (timestamp, suffix) = id.split_once('-')?;
    let timestamp = timestamp.parse::<u64>().ok()?;
    let (kind, key) = suffix.split_once('-')?;
    if !matches!(kind, "manual" | "safety") || Uuid::parse_str(key).is_err() {
        return None;
    }
    Some(json!({"id":id, "timestamp":timestamp, "safety":kind=="safety"}))
}

fn list(directory: &Path) -> Result<Value> {
    let mut entries = Vec::new();
    for entry in fs::read_dir(directory).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        if !entry.file_type().map_err(|e| e.to_string())?.is_file() {
            continue;
        }
        if let Some(metadata) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.strip_suffix(".json"))
            .and_then(snapshot_metadata)
        {
            entries.push(metadata);
        }
    }
    entries.sort_by_key(|entry| std::cmp::Reverse(entry["timestamp"].as_u64()));
    Ok(json!(entries))
}

fn save(directory: &Path, kind: &str, data: &Value) -> Result<()> {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_millis();
    let id = format!("{timestamp}-{kind}-{}", Uuid::new_v4());
    let temporary = directory.join(format!("{id}.tmp"));
    let destination = directory.join(format!("{id}.json"));
    let result = (|| {
        let file = File::create(&temporary).map_err(|e| e.to_string())?;
        let mut writer = BufWriter::new(file);
        serde_json::to_writer(&mut writer, data).map_err(|e| e.to_string())?;
        writer.flush().map_err(|e| e.to_string())?;
        writer.get_ref().sync_all().map_err(|e| e.to_string())?;
        fs::rename(&temporary, destination).map_err(|e| e.to_string())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

pub fn run(db: &Db, database: &Path, op: &str, payload: &Value) -> Result<Value> {
    let directory = database.with_file_name("snapshots");
    fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
    match op {
        "snapshot_list" => list(&directory),
        "snapshot_create" => {
            let data = db.transaction(|| store::backup_data(db))?;
            save(&directory, "manual", &data)?;
            list(&directory)
        }
        "snapshot_preview" | "snapshot_restore" | "snapshot_delete" => {
            let id = payload["id"]
                .as_str()
                .filter(|id| snapshot_metadata(id).is_some())
                .ok_or("快照编号无效")?;
            let path = directory.join(format!("{id}.json"));
            if !fs::symlink_metadata(&path)
                .map_err(|e| e.to_string())?
                .is_file()
            {
                return Err("找不到本地备份文件".into());
            }
            if op == "snapshot_delete" {
                fs::remove_file(&path).map_err(|e| format!("无法删除备份：{e}"))?;
                return list(&directory);
            }
            let file = File::open(path).map_err(|e| e.to_string())?;
            let mut data: Value =
                serde_json::from_reader(BufReader::new(file)).map_err(|e| e.to_string())?;
            if op == "snapshot_preview" {
                if let Some(fields) = data.as_object_mut() {
                    fields.remove("pdf_files");
                }
                if let Some(documents) = data["documents"].as_array_mut() {
                    for document in documents {
                        if let Some(fields) = document.as_object_mut() {
                            fields.remove("web_layout");
                        }
                    }
                }
                if let Some(pages) = data["pdf_pages"].as_array_mut() {
                    for page in pages {
                        if let Some(fields) = page.as_object_mut() {
                            fields.remove("layer");
                        }
                    }
                }
                return Ok(data);
            }
            db.transaction(|| {
                let previous = store::backup_data(db)?;
                store::replace_backup(db, &data)?;
                save(&directory, "safety", &previous)
            })?;
            let _ = store::prune_pdf_files(db);
            Ok(Value::Null)
        }
        _ => Err("未知快照操作".into()),
    }
}
