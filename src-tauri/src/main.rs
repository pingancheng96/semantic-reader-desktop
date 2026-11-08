#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod ai;
mod dictionary;
mod sqlite;
mod store;
mod tags;
mod web_import;
mod web_layout;

use serde_json::Value;
use sqlite::{Db, Result};
use std::path::PathBuf;
use tauri::{Manager, State};

struct Database(PathBuf);

#[tauri::command]
async fn request(state: State<'_, Database>, op: String, payload: Value) -> Result<Value> {
    let path = state.0.clone();
    match op.as_str() {
        "dictionary_lookup" | "dictionary_status" => {
            tauri::async_runtime::spawn_blocking(move || {
                let dictionary = path.with_file_name("wiktionary.sqlite3");
                if op == "dictionary_status" {
                    return dictionary::status(&dictionary);
                }
                dictionary::lookup(&dictionary, store::field(&payload, "word", 240, true)?)
            })
            .await
            .map_err(|_| "无法读取本地词典")?
        }
        "dictionary_resolve" => {
            let params = payload.clone();
            let source_path = path.clone();
            let (surface, context, local, key) = tauri::async_runtime::spawn_blocking(move || {
                let db = Db::open(&source_path)?;
                let (_, _, _, surface, context) = store::selection(&db, &params)?;
                let local = dictionary::lookup(
                    &source_path.with_file_name("wiktionary.sqlite3"),
                    &surface,
                )?;
                let needs_ai = !dictionary::has_pronunciation(&local);
                let key = if needs_ai {
                    ai::request_key(&source_path, &params)?
                } else {
                    None
                };
                Ok::<_, String>((surface, context, local, key))
            })
            .await
            .map_err(|_| "无法查找词典原形")??;
            if dictionary::has_pronunciation(&local) {
                return Ok(serde_json::json!({"lemma":null}));
            }
            let proposed = ai::find_lemma(&payload, key, &surface, &context).await?;
            let lemma = proposed["lemma"].as_str().unwrap().trim().to_owned();
            tauri::async_runtime::spawn_blocking(move || {
                let result = if lemma.eq_ignore_ascii_case(&surface) {
                    local.clone()
                } else {
                    dictionary::lookup(&path.with_file_name("wiktionary.sqlite3"), &lemma)?
                };
                Ok(dictionary::resolve_fallback(
                    &surface, &lemma, &local, &result, &proposed,
                ))
            })
            .await
            .map_err(|_| "无法查询词典原形")?
        }
        "import_web" => {
            let url = web_import::parse_url(store::field(&payload, "url", 4096, true)?)?;
            let page = web_import::fetch(url).await?;
            tauri::async_runtime::spawn_blocking(move || {
                let db = Db::open(&path)?;
                store::import_web(&db, page)
            })
            .await
            .map_err(|_| "保存网页失败")?
        }
        "repair_images" => {
            let params = payload.clone();
            let read_path = path.clone();
            let snapshot = tauri::async_runtime::spawn_blocking(move || {
                store::image_repair_snapshot(&Db::open(&read_path)?, &params)
            })
            .await
            .map_err(|_| "无法读取网页来源")??;
            let source = snapshot["source_url"].as_str().unwrap().to_owned();
            let mut page = web_import::load_page(web_import::parse_url(&source)?).await?;
            let mut page = tauri::async_runtime::spawn_blocking(move || {
                store::seed_saved_images(&mut page, &snapshot)?;
                Ok::<_, String>(page)
            })
            .await
            .map_err(|_| "无法匹配原有图片")??;
            web_import::fill_images(&mut page).await;
            tauri::async_runtime::spawn_blocking(move || {
                store::repair_images(&Db::open(&path)?, &payload, &source, page)
            })
            .await
            .map_err(|_| "无法保存补图结果")?
        }
        "suggest" => {
            let params = payload.clone();
            let read_path = path.clone();
            let (local, key) = tauri::async_runtime::spawn_blocking(move || {
                let db = Db::open(&read_path)?;
                let local = db.snapshot(|| store::candidates(&db, &params))?;
                let key = ai::request_key(&read_path, &params)?;
                Ok::<_, String>((local, key))
            })
            .await
            .map_err(|_| "读取候选失败")??;
            let mut response = ai::suggest(payload, local, key).await?;
            let suggestion = &mut response["suggestion"];
            if suggestion["group_id"].is_null() {
                if let Some(name) = suggestion["group_name"].as_str() {
                    let name = name.to_owned();
                    let existing = tauri::async_runtime::spawn_blocking(move || {
                        store::group_with_name(&Db::open(&path)?, &name)
                    })
                    .await
                    .map_err(|_| "检查同名语义组失败")??;
                    if let Some(id) = existing {
                        suggestion["group_id"] = id;
                    }
                }
            }
            Ok(response)
        }
        "key_status" => {
            tauri::async_runtime::spawn_blocking(move || ai::key_status(&path, &payload))
                .await
                .map_err(|_| "无法检查密钥状态")?
        }
        "key_save" | "key_remove" => tauri::async_runtime::spawn_blocking(move || {
            ai::key_action(&path, &payload, op == "key_remove")
        })
        .await
        .map_err(|_| "密钥操作失败")?,
        _ => tauri::async_runtime::spawn_blocking(move || {
            let db = Db::open(&path)?;
            if op == "import" {
                let title = store::field(&payload, "title", 255, true)?;
                let format = store::field(&payload, "format", 3, true)?;
                return store::import(
                    &db,
                    title,
                    format,
                    payload["content"].as_str().ok_or("文件内容无效")?,
                );
            }
            if matches!(op.as_str(), "list" | "document" | "group" | "candidates") {
                db.snapshot(|| store::run(&db, &op, &payload))
            } else {
                store::run(&db, &op, &payload)
            }
        })
        .await
        .map_err(|_| "数据库操作失败")?,
    }
}

#[tauri::command]
async fn dictionary_install(
    app: tauri::AppHandle,
    state: State<'_, Database>,
    progress: tauri::ipc::Channel<dictionary::Progress>,
    download: bool,
) -> Result<Value> {
    use tauri_plugin_dialog::DialogExt;
    let path = state.0.with_file_name("wiktionary.sqlite3");
    if download {
        let input = dictionary::download(path.parent().ok_or("找不到词典目录")?, &progress).await?;
        return tauri::async_runtime::spawn_blocking(move || {
            dictionary::install(&path, input.path(), |value| {
                let _ = progress.send(value);
            })
        })
        .await
        .map_err(|_| "无法安装本地词典")?;
    }
    tauri::async_runtime::spawn_blocking(move || {
        let Some(input) = app
            .dialog()
            .file()
            .add_filter("Wiktextract JSONL", &["jsonl", "gz"])
            .blocking_pick_file()
        else {
            return Ok(Value::Null);
        };
        let input = input.into_path().map_err(|_| "请选择本地词典文件")?;
        dictionary::install(&path, &input, |value| {
            let _ = progress.send(value);
        })
    })
    .await
    .map_err(|_| "无法安装本地词典")?
}

#[tauri::command]
async fn export_file(
    app: tauri::AppHandle,
    state: State<'_, Database>,
    document_id: Option<String>,
) -> Result<bool> {
    use tauri_plugin_dialog::DialogExt;
    let path = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let db = Db::open(&path)?;
        let (name, text) = if let Some(id) = document_id {
            let doc = db.one(
                "SELECT title,content,source_url FROM documents WHERE id=?1",
                &[Value::from(id)],
            )?;
            (
                if doc["source_url"].is_string() {
                    format!(
                        "{}.txt",
                        doc["title"].as_str().unwrap().replace(['/', '\\'], "_")
                    )
                } else {
                    doc["title"].as_str().unwrap().to_owned()
                },
                doc["content"].as_str().unwrap().to_owned(),
            )
        } else {
            let backup = store::run(&db, "backup", &Value::Null)?;
            (
                "semantic-reader.json".into(),
                serde_json::to_string_pretty(&backup).map_err(|e| e.to_string())?,
            )
        };
        let name = std::path::Path::new(&name)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("semantic-reader-export");
        let Some(destination) = app.dialog().file().set_file_name(name).blocking_save_file() else {
            return Ok(false);
        };
        let destination = destination.into_path().map_err(|_| "请选择本地文件位置")?;
        let data_dir = path.parent().and_then(|p| p.canonicalize().ok());
        let destination_dir = destination.parent().and_then(|p| p.canonicalize().ok());
        if data_dir.is_some()
            && data_dir == destination_dir
            && matches!(
                destination.file_name().and_then(|s| s.to_str()),
                Some("library.sqlite3" | "library.sqlite3-wal" | "library.sqlite3-shm")
            )
        {
            return Err("请选择不同于应用数据库的导出文件".into());
        }
        // Write beside the destination so an interrupted export cannot truncate an existing backup.
        let temporary =
            destination.with_file_name(format!(".semantic-reader-{}.tmp", uuid::Uuid::new_v4()));
        let save = std::fs::write(&temporary, text)
            .and_then(|_| std::fs::rename(&temporary, &destination));
        if let Err(e) = save {
            let _ = std::fs::remove_file(temporary);
            return Err(format!("导出失败：{e}"));
        }
        Ok(true)
    })
    .await
    .map_err(|_| "导出失败")?
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let directory = match std::env::var_os("SEMANTIC_READER_DATA_DIR") {
                Some(path) => PathBuf::from(path),
                None => app.path().app_local_data_dir()?,
            };
            std::fs::create_dir_all(&directory)?;
            let path = directory.join("library.sqlite3");
            let db = Db::open(&path).map_err(std::io::Error::other)?;
            db.batch("PRAGMA journal_mode=WAL;")
                .map_err(std::io::Error::other)?;
            store::init(&db).map_err(std::io::Error::other)?;
            app.manage(Database(path));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            request,
            export_file,
            dictionary_install
        ])
        .run(tauri::generate_context!())
        .expect("Semantic Reader 启动失败");
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tauri::{
        ipc::{CallbackFn, InvokeBody},
        test,
        webview::InvokeRequest,
        WebviewWindowBuilder,
    };
    use uuid::Uuid;

    #[test]
    fn ipc_import_and_read_one_hundred_marks() {
        let path =
            std::env::temp_dir().join(format!("semantic-reader-ipc-{}.sqlite3", Uuid::new_v4()));
        store::init(&Db::open(&path).unwrap()).unwrap();
        let app = test::mock_builder()
            .manage(Database(path.clone()))
            .invoke_handler(tauri::generate_handler![request])
            .build(test::mock_context(test::noop_assets()))
            .unwrap();
        let window = WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .unwrap();
        let call = |op: &str, payload: Value| -> Value {
            test::get_ipc_response(
                &window,
                InvokeRequest {
                    cmd: "request".into(),
                    callback: CallbackFn(0),
                    error: CallbackFn(1),
                    url: if cfg!(windows) {
                        "http://tauri.localhost"
                    } else {
                        "tauri://localhost"
                    }
                    .parse()
                    .unwrap(),
                    body: InvokeBody::Json(json!({"op":op,"payload":payload})),
                    headers: Default::default(),
                    invoke_key: test::INVOKE_KEY.into(),
                },
            )
            .unwrap()
            .deserialize()
            .unwrap()
        };

        let groups: Vec<_> = (0..30)
            .map(|i| {
                call(
                    "group_save",
                    json!({"name":format!("Group {i}"),"vocabulary":"drop"}),
                )
            })
            .collect();
        for i in 0..10 {
            let content = "📉 Prices dropped.\r\n".repeat(10);
            let doc = call(
                "import",
                json!({"title":format!("Document {i}.txt"),"format":"txt","content":content}),
            );
            for (j, (at, _)) in content.match_indices("dropped").enumerate() {
                let start = content[..at].encode_utf16().count();
                call(
                    "annotate",
                    json!({"document_id":doc,"start":start,"end":start+7,
                    "lemma":"drop","meaning":format!("下降 {}",j % 3),"group_id":groups[(i*10+j) % groups.len()]}),
                );
            }
            let loaded = call("document", json!({"id":doc}));
            assert_eq!(loaded["marks"].as_array().unwrap().len(), 10);
            assert_eq!(loaded["document"]["content"], content);
        }
        let listed = call("list", json!({"q":"drop"}));
        assert_eq!(listed["groups"].as_array().unwrap().len(), 30);
        assert_eq!(
            call("backup", json!({}))["occurrences"]
                .as_array()
                .unwrap()
                .len(),
            100
        );
        drop(window);
        drop(app);
        std::fs::remove_file(path).unwrap();
    }
}
