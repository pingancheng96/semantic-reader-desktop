#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod ai;
mod epub;
mod kokoro;
mod mdict;
mod pronunciation;
mod snapshots;
mod pdf_geometry;
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
        "kokoro_status" => {
            let installed = |variant: kokoro::Variant| {
                kokoro::installed(&path.with_file_name(variant.directory()), variant)
            };
            Ok(serde_json::json!({"fp32":installed(kokoro::Variant::Fp32),"int8":installed(kokoro::Variant::Int8)}))
        }
        "pronunciation_list" | "pronunciation_install" | "pronunciation_remove"
        | "pronunciation_lookup" | "pronunciation_audio" => {
            tauri::async_runtime::spawn_blocking(move || {
                let root = path.with_file_name("pronunciation-dictionaries");
                let field = |key, max| store::field(&payload, key, max, true);
                match op.as_str() {
                    "pronunciation_list" => pronunciation::list(&root),
                    "pronunciation_install" => pronunciation::install(&root, std::path::Path::new(field("folder", 4096)?)),
                    "pronunciation_remove" => pronunciation::remove(&root, field("id", 36)?),
                    "pronunciation_lookup" => pronunciation::lookup(&root, field("id", 36)?, field("word", 240)?, store::field(&payload,"base",240,false)?),
                    _ => pronunciation::audio(&root, field("id", 36)?, field("key", 1024)?),
                }
            }).await.map_err(|_| "无法读取发音词典")?
        }
        "pronunciation_estimate" => {
            let word = store::field(&payload, "word", 240, true)?;
            let context = store::field(&payload, "context", 6000, false)?;
            let key = ai::request_key(&path, &payload)?;
            ai::estimate_pronunciation(&payload, key, word, context).await
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
        "compare_group_senses" => {
            let params = payload.clone();
            let (input, key) = tauri::async_runtime::spawn_blocking(move || {
                let db = Db::open(&path)?;
                let input = db.snapshot(|| store::comparison_input(&db, &params))?;
                Ok::<_, String>((input, ai::request_key(&path, &params)?))
            })
            .await
            .map_err(|_| "读取同组词项失败")??;
            ai::compare_group_senses(&payload, key, input).await
        }
        "organize_glosses" => {
            let params = payload.clone();
            let (input, key) = tauri::async_runtime::spawn_blocking(move || {
                let db = Db::open(&path)?;
                let input = db.snapshot(|| store::gloss_batch(&db, &params))?;
                Ok::<_, String>((input, ai::request_key(&path, &params)?))
            })
            .await
            .map_err(|_| "读取释义候选失败")??;
            ai::organize_glosses(&payload, key, input).await
        }
        "organize_group" => {
            let params = payload.clone();
            let (input, key) = tauri::async_runtime::spawn_blocking(move || {
                let db = Db::open(&path)?;
                let input = db.snapshot(|| store::organization_input(&db, &params))?;
                Ok::<_, String>((input, ai::request_key(&path, &params)?))
            })
            .await
            .map_err(|_| "读取整理候选失败")??;
            ai::organize_group(&payload, key, input).await
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
            let previous = local["groups"].clone();
            let context = local["context"].clone();
            let saved = !local["saved_sense"].is_null();
            let mut response = ai::suggest(payload.clone(), local, key.clone()).await?;
            if !saved && response["suggestion"]["group_id"].is_null() {
                let name = response["suggestion"]["group_name"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned();
                let terms = response["suggestion"]["group_search_terms"].clone();
                let lookup = tauri::async_runtime::spawn_blocking(move || {
                    let db = Db::open(&path)?;
                    db.snapshot(|| {
                        let exact = store::group_with_name(&db, &name)?;
                        let groups = if exact.is_some() {
                            Vec::new()
                        } else {
                            store::followup_groups(&db, &terms, &previous)?
                        };
                        Ok::<_, String>((exact, groups))
                    })
                })
                .await;
                match lookup {
                    Ok(Ok((Some(id), _))) => {
                        response["suggestion"]["group_id"] = id;
                        response["suggestion"]["group_reason"] =
                            serde_json::json!("与已有语义组同名。");
                    }
                    Ok(Ok((None, groups))) if !groups.is_empty() => {
                        match ai::reconsider_group(
                            &payload,
                            key,
                            &response["suggestion"],
                            &context,
                            &groups,
                        )
                        .await
                        {
                            Ok((decision, usage)) => {
                                response["usage"] = ai::combined_usage(&response["usage"], &usage);
                                response["suggestion"]["group_id"] = decision["group_id"].clone();
                                response["suggestion"]["group_reason"] = decision["reason"].clone();
                            }
                            Err(_) => {
                                response["usage"] = Value::Null;
                                response["group_check_failed"] = Value::Bool(true);
                            }
                        }
                    }
                    Ok(Ok(_)) => (),
                    _ => response["group_check_failed"] = Value::Bool(true),
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
            if matches!(
                op.as_str(),
                "snapshot_list" | "snapshot_create" | "snapshot_preview" | "snapshot_restore" | "snapshot_delete"
            ) {
                return snapshots::run(&db, &path, &op, &payload);
            }
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
            if op == "pdf_load" {
                return store::pdf_prepare(&db, &payload);
            }
            if op == "document_delete" {
                let result = store::run(&db, &op, &payload)?;
                let _ = store::prune_pdf_files(&db);
                return Ok(result);
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
async fn kokoro_install(state: State<'_, Database>, variant: kokoro::Variant, progress: tauri::ipc::Channel<kokoro::Progress>) -> Result<()> {
    let root = state.0.with_file_name(variant.directory());
    kokoro::install(&root, variant, progress).await
}

#[tauri::command]
async fn kokoro_read(state: State<'_, Database>, variant: kokoro::Variant, asset: usize, offset: u64, length: usize) -> Result<tauri::ipc::Response> {
    let root = state.0.with_file_name(variant.directory());
    let data = tauri::async_runtime::spawn_blocking(move || kokoro::read(&root, variant, asset, offset, length))
        .await.map_err(|_| "无法读取 Kokoro 模型")??;
    Ok(tauri::ipc::Response::new(data))
}

#[tauri::command]
async fn pronunciation_folder(app: tauri::AppHandle) -> Result<Value> {
    use tauri_plugin_dialog::DialogExt;
    tauri::async_runtime::spawn_blocking(move || {
        let Some(folder) = app.dialog().file().blocking_pick_folder() else {
            return Ok(Value::Null);
        };
        pronunciation::preview(&folder.into_path().map_err(|_| "请选择本地词典文件夹")?)
    }).await.map_err(|_| "无法读取词典文件夹")?
}

#[tauri::command]
async fn import_file(app: tauri::AppHandle, state: State<'_, Database>) -> Result<Value> {
    use std::io::Read;
    use tauri_plugin_dialog::DialogExt;
    let database = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let Some(file) = app.dialog().file()
            .add_filter("Markdown, TXT, PDF, EPUB", &["md", "markdown", "txt", "pdf", "epub"])
            .blocking_pick_file() else { return Ok(Value::Null); };
        let path = file.into_path().map_err(|_| "请选择本地文件")?;
        let title = path.file_name().and_then(|name| name.to_str()).ok_or("文件名无效")?;
        if title.chars().count() > 255 {
            return Err("文件名太长".into());
        }
        let extension = path.extension().and_then(|value| value.to_str()).unwrap_or_default().to_ascii_lowercase();
        let file = std::fs::File::open(&path).map_err(|e| e.to_string())?;
        let db = Db::open(&database)?;
        let id = match extension.as_str() {
            "epub" => {
                let mut bytes = Vec::new();
                file.take(30_000_001).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
                if bytes.len() > 30_000_000 { return Err("EPUB 文件最大 30 MB".into()); }
                let (content, layout) = epub::read(std::io::Cursor::new(&bytes))?;
                store::import_rich_text(&db, title, &content, &layout, &bytes)?
            }
            "pdf" => {
                if file.metadata().map_err(|e| e.to_string())?.len() > store::PDF_MAX_BYTES {
                    return Err("PDF 文件最大 100 MB".into());
                }
                store::import_pdf(&db, title, file)?
            }
            "md" | "markdown" | "txt" => {
                let mut bytes = Vec::new();
                file.take(2_000_001).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
                if bytes.len() > 2_000_000 {
                    return Err("文件最大 2 MB".into());
                }
                let content = std::str::from_utf8(&bytes).map_err(|_| "请选择 UTF-8 文本文件")?;
                store::import(&db, title, if extension == "txt" { "txt" } else { "md" }, content)?
            }
            _ => return Err("请选择 Markdown、TXT、PDF 或 EPUB 文件".into()),
        };
        Ok(serde_json::json!({"id":id,"title":title}))
    }).await.map_err(|_| "导入文件失败")?
}

#[tauri::command]
async fn restore_backup_file(app: tauri::AppHandle, state: State<'_, Database>) -> Result<bool> {
    use tauri_plugin_dialog::DialogExt;
    let database = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let Some(file) = app.dialog().file().add_filter("JSON", &["json"]).blocking_pick_file() else {
            return Ok(false);
        };
        let path = file.into_path().map_err(|_| "请选择本地备份文件")?;
        let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
        let backup = serde_json::from_reader(std::io::BufReader::new(file))
            .map_err(|e| format!("无法读取 JSON 备份：{e}"))?;
        store::run(&Db::open(&database)?, "restore", &backup)?;
        Ok(true)
    }).await.map_err(|_| "恢复备份文件失败")?
}

#[tauri::command]
async fn pdf_range(state: State<'_, Database>, id: String, begin: u64, end: u64) -> Result<tauri::ipc::Response> {
    let path = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        store::pdf_range(&Db::open(&path)?, &serde_json::json!({"id":id}), begin, end)
            .map(tauri::ipc::Response::new)
    }).await.map_err(|_| "读取 PDF 失败")?
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
                &[Value::from(id.clone())],
            )?;
            let pages = db.query("SELECT text FROM pdf_pages WHERE document_id=?1 ORDER BY page", &[Value::from(id)])?;
            let text = if pages.is_empty() { doc["content"].as_str().unwrap().to_owned() } else {
                pages.iter().filter_map(|page| page["text"].as_str()).collect::<Vec<_>>().join("\n\n")
            };
            (
                if doc["source_url"].is_string() {
                    format!(
                        "{}.txt",
                        doc["title"].as_str().unwrap().replace(['/', '\\'], "_")
                    )
                } else {
                    let title = doc["title"].as_str().unwrap();
                    match title.rsplit_once('.') {
                        Some((stem, extension)) if extension.eq_ignore_ascii_case("pdf") => format!("{stem}.txt"),
                        _ => title.to_owned(),
                    }
                },
                text,
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
        let timestamp = db.one(
            "SELECT strftime('%Y-%m-%d_%H-%M-%S', 'now', 'localtime') AS timestamp",
            &[],
        )?;
        let timestamp = timestamp["timestamp"].as_str().ok_or("无法读取导出时间")?;
        let name = match name.rsplit_once('.') {
            Some((stem, extension)) if !stem.is_empty() => format!("{stem}_{timestamp}.{extension}"),
            _ => format!("{name}_{timestamp}"),
        };
        let Some(destination) = app.dialog().file().set_file_name(&name).blocking_save_file() else {
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

#[cfg(target_os = "macos")]
fn reader_menu(app: &tauri::AppHandle) -> tauri::Result<tauri::menu::Menu<tauri::Wry>> {
    use tauri::menu::{Menu, MenuItemKind, MenuItem};
    let menu = Menu::default(app)?;
    for item in menu.items()? {
        if let MenuItemKind::Submenu(submenu) = item {
            if submenu.text()? == "Edit" {
                submenu.append(&MenuItem::with_id(
                    app, "find-in-article", "Find in Article", true, Some("CommandOrControl+F"),
                )?)?;
                break;
            }
        }
    }
    Ok(menu)
}

fn main() {
    let builder = tauri::Builder::default();
    #[cfg(target_os = "macos")]
    let builder = builder.menu(reader_menu).on_menu_event(|app, event| {
        if event.id() == "find-in-article" {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.eval("window.dispatchEvent(new Event('semantic-reader-find'))");
            }
        }
    });
    builder
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
            import_file,
            restore_backup_file,
            pdf_range,
            pronunciation_folder,
            kokoro_install,
            kokoro_read
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
