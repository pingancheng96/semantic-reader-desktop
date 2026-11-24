#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod ai;
mod languages;
mod epub;
mod kokoro;
mod piper;
mod mdict;
mod pronunciation;
mod snapshots;
mod pdf_geometry;
mod sqlite;
mod sil;
mod store;
mod tags;
mod updates;
mod web_import;
mod web_layout;

use serde_json::Value;
use sqlite::{Db, Result};
use std::path::PathBuf;
use tauri::{Manager, State};

struct Database(PathBuf);

#[tauri::command]
async fn request(state: State<'_, Database>, op: String, payload: Value) -> Result<Value> {
    let language = payload["language"].as_str().unwrap_or("en");
    languages::validate(language)?;
    let shared = state.0.clone();
    if op=="workspace_open" {
        let root=shared.clone(); let code=language.to_owned();
        return tauri::async_runtime::spawn_blocking(move || {
            languages::prepare(&root,&code)?;
            Ok(serde_json::json!({"language":code}))
        }).await.map_err(|_|"无法打开学习空间")?;
    }
    let path = languages::path(&shared, language)?;
    match op.as_str() {
        "sil_catalog" => Ok(sil::catalog()),
        "update_info" => Ok(updates::info()),
        "update_check" => updates::check().await,
        "update_open_link" => updates::open_link(payload["url"].as_str().unwrap_or_default()),
        "piper_status" => Ok(serde_json::json!({"installed":piper::installed(&shared.with_file_name(piper::DIRECTORY)),"size":piper::ASSETS[0].2})),
        "kokoro_status" => {
            let installed = |variant: kokoro::Variant| {
                kokoro::installed(&shared.with_file_name(variant.directory()), variant)
            };
            Ok(serde_json::json!({"fp32":installed(kokoro::Variant::Fp32),"int8":installed(kokoro::Variant::Int8)}))
        }
        "pronunciation_list" | "pronunciation_install" | "pronunciation_remove"
        | "pronunciation_lookup" | "pronunciation_audio" => {
            tauri::async_runtime::spawn_blocking(move || {
                let root = shared.with_file_name("pronunciation-dictionaries");
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
            let key = ai::request_key(&shared, &payload)?;
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
                Ok::<_, String>((input, ai::request_key(&shared, &params)?))
            })
            .await
            .map_err(|_| "读取同组词项失败")??;
            ai::compare_group_senses(&payload, key, input).await
        }
        "sil_count" | "sil_review" => {
            let review = op == "sil_review";
            let params = payload.clone();
            let parent = payload["parent"].as_str().ok_or("缺少当前分类")?;
            if !parent.is_empty() && !sil::valid(parent) { return Err("分类编号无效".into()); }
            let (batch,key) = tauri::async_runtime::spawn_blocking(move || {
                let db = Db::open(&path)?;
                let parent = params["parent"].as_str().unwrap();
                if !review { return Ok::<_, String>((store::sil_count(&db,parent)?,None)); }
                let batch = db.snapshot(|| store::sil_batch(&db,params["cursor"].as_str().unwrap_or_default(),parent))?;
                Ok((batch,ai::request_key(&shared,&params)?))
            }).await.map_err(|_| "无法读取当前分类")??;
            if review { ai::review_sil(&payload,key,batch,parent).await } else { Ok(batch) }
        }
        "sil_audit" => {
            let params = payload.clone();
            let (rows,key) = tauri::async_runtime::spawn_blocking(move || {
                let code = params["code"].as_str().filter(|code| sil::valid(code)).ok_or("分类编号无效")?;
                let db = Db::open(&path)?;
                let rows = db.snapshot(|| store::sil_audit_rows(&db, code, params["descendants"] == true))?;
                Ok::<_, String>((rows, ai::request_key(&shared, &params)?))
            }).await.map_err(|_| "无法读取当前分类")??;
            ai::audit_sil(&payload, key, rows).await
        }
        "sense_gloss_suggest" => {
            let params=payload.clone();
            let (input,key)=tauri::async_runtime::spawn_blocking(move || {
                let db=Db::open(&path)?;
                Ok::<_,String>((db.snapshot(||store::sense_gloss_input(&db,&params))?,ai::request_key(&shared,&params)?))
            }).await.map_err(|_|"无法读取义项释义")??;
            ai::suggest_sense_gloss(&payload,key,input).await
        }
        "organize_glosses" => {
            let params = payload.clone();
            let (input, key) = tauri::async_runtime::spawn_blocking(move || {
                let db = Db::open(&path)?;
                let input = db.snapshot(|| store::gloss_batch(&db, &params))?;
                Ok::<_, String>((input, ai::request_key(&shared, &params)?))
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
                Ok::<_, String>((input, ai::request_key(&shared, &params)?))
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
                let key = ai::request_key(&shared, &params)?;
                Ok::<_, String>((local, key))
            })
            .await
            .map_err(|_| "读取候选失败")??;
            let previous = local["groups"].clone();
            let context = local["context"].clone();
            let saved = !local["saved_sense"].is_null();
            let initial_ids:Vec<Value>=local["notes"].as_array().into_iter().flatten().map(|note|note["id"].clone()).collect();
            let mut response = ai::suggest(payload.clone(), local, key.clone()).await?;
            if !saved {
                let mut params=payload.clone();
                params["lookup_lemma"]=response["suggestion"]["lemma"].clone();
                let lookup_path=path.clone();
                let lookup=tauri::async_runtime::spawn_blocking(move || {
                    let db=Db::open(&lookup_path)?;
                    db.snapshot(||store::candidates(&db,&params))
                }).await;
                match lookup {
                    Ok(Ok(candidates))=>{
                        let notes=candidates["notes"].as_array().cloned().unwrap_or_default();
                        let new_candidates=notes.iter().any(|note|!initial_ids.contains(&note["id"]));
                        if response["suggestion"]["sense_id"].is_null() && new_candidates {
                            match ai::match_lemma_sense(&payload,key.clone(),&response["suggestion"],&notes,&context).await {
                                Ok((matched,usage))=>{
                                    response["usage"]=ai::combined_usage(&response["usage"],&usage);
                                    if let Some(note)=matched {ai::apply_sense_match(&mut response["suggestion"],&note);}
                                }
                                Err(_)=>{
                                    response["lemma_check_failed"]=Value::Bool(true);
                                    response["usage"]=Value::Null;
                                }
                            }
                        }
                        response["candidates"]=candidates;
                    }
                    _=>response["lemma_check_failed"]=Value::Bool(true),
                }
            }
            if payload["ungrouped_capture"] != true && !saved && response["suggestion"]["group_id"].is_null() {
                let name = response["suggestion"]["group_name"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned();
                let terms = response["suggestion"]["group_search_terms"].clone();
                let code = response["suggestion"]["sil_code"].clone();
                let lookup = tauri::async_runtime::spawn_blocking(move || {
                    let db = Db::open(&path)?;
                    db.snapshot(|| {
                        let exact = store::group_with_name(&db, &name)?;
                        let groups = if exact.is_some() {
                            Vec::new()
                        } else {
                            store::sil_followup_groups(&db, &terms, &previous, &code)?
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
                            key.clone(),
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
            ai::lemma_decision(&payload,key,&mut response,&context).await;
            Ok(response)
        }
        "key_status" => {
            tauri::async_runtime::spawn_blocking(move || ai::key_status(&shared, &payload))
                .await
                .map_err(|_| "无法检查密钥状态")?
        }
        "key_save" | "key_remove" => tauri::async_runtime::spawn_blocking(move || {
            ai::key_action(&shared, &payload, op == "key_remove")
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
async fn piper_install(state: State<'_, Database>, language: String, progress: tauri::ipc::Channel<piper::Progress>) -> Result<()> {
    if language!="da" { return Err("当前 Piper 模型仅适用于丹麦语".into()); }
    piper::install(&state.0.with_file_name(piper::DIRECTORY),progress).await
}

#[tauri::command]
async fn piper_read(state: State<'_, Database>, language: String, asset: usize, offset: u64, length: usize) -> Result<tauri::ipc::Response> {
    if language!="da" { return Err("当前 Piper 模型仅适用于丹麦语".into()); }
    let root=state.0.with_file_name(piper::DIRECTORY);
    let data=tauri::async_runtime::spawn_blocking(move ||piper::read(&root,asset,offset,length)).await.map_err(|_|"无法读取 Piper 模型")??;
    Ok(tauri::ipc::Response::new(data))
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
async fn browser_open(app: tauri::AppHandle, url: String) -> Result<()> {
    let url = web_import::parse_url(&url)?;
    if let Some(window) = app.get_webview_window("web-import") {
        window.navigate(url).map_err(|e|e.to_string())?;
        window.set_focus().map_err(|e|e.to_string())?;
    } else {
        tauri::WebviewWindowBuilder::new(&app, "web-import", tauri::WebviewUrl::External(url))
            .title("加载文章后，回到 Semantic Reader 点击「导入已打开的页面」")
            .inner_size(1100.0,800.0)
            .on_navigation(|url|matches!(url.scheme(),"http"|"https"))
            .build().map_err(|e|e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
async fn browser_import(app: tauri::AppHandle, state: State<'_, Database>, language: Option<String>) -> Result<Value> {
    let window = app.get_webview_window("web-import").ok_or("请先打开浏览窗口")?;
    let (send, receive) = tokio::sync::oneshot::channel();
    let send = std::sync::Mutex::new(Some(send));
    window.eval_with_callback(
        "({url:location.href,html:document.documentElement.outerHTML})",
        move |value| { if let Ok(mut sender) = send.lock() { if let Some(sender) = sender.take() { let _ = sender.send(value); } } },
    ).map_err(|e|e.to_string())?;
    let raw = tokio::time::timeout(std::time::Duration::from_secs(20),receive).await
        .map_err(|_|"读取浏览页面超时")?.map_err(|_|"读取浏览页面失败")?;
    let captured: Value = serde_json::from_str(&raw).map_err(|_|"浏览页面返回内容无效")?;
    let mut page = web_import::browser_page(captured["url"].as_str().ok_or("无法读取网页地址")?,
        captured["html"].as_str().ok_or("无法读取网页正文")?)?;
    web_import::fill_images(&mut page).await;
    let path = languages::path(&state.0, language.as_deref().unwrap_or("en"))?;
    tauri::async_runtime::spawn_blocking(move || store::import_web(&Db::open(&path)?,page))
        .await.map_err(|_|"保存网页失败")?
}

#[tauri::command]
async fn import_file(app: tauri::AppHandle, state: State<'_, Database>, language: Option<String>) -> Result<Value> {
    use tauri_plugin_dialog::DialogExt;
    let database = languages::path(&state.0, language.as_deref().unwrap_or("en"))?;
    tauri::async_runtime::spawn_blocking(move || {
        let Some(file) = app.dialog().file()
            .add_filter("Markdown, TXT, PDF, EPUB", &["md", "markdown", "txt", "pdf", "epub"])
            .blocking_pick_file() else { return Ok(Value::Null); };
        let path = file.into_path().map_err(|_| "请选择本地文件")?;
        import_local_path(&database, &path)
    }).await.map_err(|_| "导入文件失败")?
}

#[tauri::command]
async fn import_dropped_files(paths: Vec<PathBuf>, state: State<'_, Database>, language: Option<String>) -> Result<Value> {
    let database = languages::path(&state.0, language.as_deref().unwrap_or("en"))?;
    tauri::async_runtime::spawn_blocking(move || {
        let mut imported = 0;
        let mut errors = Vec::new();
        for path in paths {
            match import_local_path(&database, &path) {
                Ok(_) => imported += 1,
                Err(error) => errors.push(format!("{}：{error}", path.file_name().unwrap_or_default().to_string_lossy())),
            }
        }
        Ok(serde_json::json!({"imported":imported,"errors":errors}))
    }).await.map_err(|_| "导入文件失败")?
}

fn import_local_path(database: &std::path::Path, path: &std::path::Path) -> Result<Value> {
    use std::io::Read;
    let title = path.file_name().and_then(|name| name.to_str()).ok_or("文件名无效")?;
    if title.chars().count() > 255 {
        return Err("文件名太长".into());
    }
    let extension = path.extension().and_then(|value| value.to_str()).unwrap_or_default().to_ascii_lowercase();
    if !path.is_file() { return Err("请选择本地文件".into()); }
    let mut file = std::fs::File::open(&path).map_err(|e| e.to_string())?;
    let db = Db::open(&database)?;
    let id = match extension.as_str() {
        "epub" => {
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
            let (content, layout) = epub::read(std::io::Cursor::new(&bytes))?;
            store::import_rich_text(&db, title, &content, &layout, &bytes)?
        }
        "pdf" => {
            store::import_pdf(&db, title, file)?
        }
        "md" | "markdown" | "txt" => {
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
            let content = std::str::from_utf8(&bytes).map_err(|_| "请选择 UTF-8 文本文件")?;
            store::import(&db, title, if extension == "txt" { "txt" } else { "md" }, content)?
        }
        _ => return Err("请选择 Markdown、TXT、PDF 或 EPUB 文件".into()),
    };
    Ok(serde_json::json!({"id":id,"title":title}))
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
        languages::restore_all(&database, &backup)?;
        Ok(true)
    }).await.map_err(|_| "恢复备份文件失败")?
}

#[tauri::command]
async fn epub_entry(state: State<'_, Database>, id: String, name: String, language: Option<String>) -> Result<tauri::ipc::Response> {
    use std::io::Read;
    let path = languages::path(&state.0, language.as_deref().unwrap_or("en"))?;
    let data = tauri::async_runtime::spawn_blocking(move || {
        let db = Db::open(&path)?;
        let key = serde_json::json!(id);
        let path = store::epub_path(&db,&key)?;
        let file = std::fs::File::open(path).map_err(|e|e.to_string())?;
        let mut zip = zip::ZipArchive::new(file).map_err(|e|e.to_string())?;
        let mut entry = zip.by_name(&name).map_err(|e|e.to_string())?;
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).map_err(|e|e.to_string())?;
        Ok::<_,String>(bytes)
    }).await.map_err(|_| "无法读取 EPUB 资源")??;
    Ok(tauri::ipc::Response::new(data))
}

#[tauri::command]
async fn pdf_range(state: State<'_, Database>, id: String, begin: u64, end: u64, language: Option<String>) -> Result<tauri::ipc::Response> {
    let path = languages::path(&state.0, language.as_deref().unwrap_or("en"))?;
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
    language: Option<String>,
) -> Result<bool> {
    use tauri_plugin_dialog::DialogExt;
    let shared = state.0.clone();
    let path = languages::path(&state.0, language.as_deref().unwrap_or("en"))?;
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
            let backup = languages::backup_all(&shared)?;
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

#[tauri::command]
fn epub_keyboard(app: tauri::AppHandle, enabled: bool) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        use tauri::menu::MenuItemKind;
        if let Some(menu) = app.menu() {
            if let Some(MenuItemKind::Submenu(reader)) = menu.get("epub-navigation") {
                for id in ["epub-previous", "epub-next"] {
                    if let Some(MenuItemKind::MenuItem(item)) = reader.get(id) {
                        item.set_enabled(enabled).map_err(|error| error.to_string())?;
                    }
                }
            }
        }
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (app, enabled);
    Ok(())
}

#[cfg(target_os = "macos")]
fn reader_menu(app: &tauri::AppHandle) -> tauri::Result<tauri::menu::Menu<tauri::Wry>> {
    use tauri::menu::{Menu, MenuItemKind, MenuItem, Submenu};
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
    let reader = Submenu::with_id(app, "epub-navigation", "EPUB", true)?;
    reader.append(&MenuItem::with_id(app, "epub-previous", "Previous Page", false, Some("ArrowLeft"))?)?;
    reader.append(&MenuItem::with_id(app, "epub-next", "Next Page", false, Some("ArrowRight"))?)?;
    menu.append(&reader)?;
    Ok(menu)
}

fn main() {
    let builder = tauri::Builder::default();
    #[cfg(target_os = "macos")]
    let builder = builder.menu(reader_menu).on_menu_event(|app, event| {
        let message = match event.id().as_ref() {
            "find-in-article" => "semantic-reader-find",
            "epub-previous" => "semantic-reader-epub-previous",
            "epub-next" => "semantic-reader-epub-next",
            _ => return,
        };
        if let Some(window) = app.get_webview_window("main") {
            let _ = window.eval(format!("window.dispatchEvent(new Event('{message}'))"));
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
            browser_open,
            browser_import,
            import_dropped_files,
            epub_keyboard,
            restore_backup_file,
            pdf_range,
            epub_entry,
            pronunciation_folder,
            piper_install,
            piper_read,
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
        let directory=std::env::temp_dir().join(format!("semantic-reader-ipc-{}",Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        let path=directory.join("library.sqlite3");
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
        call("workspace_open",json!({"language":"fr"}));
        let french=call("import",json!({"language":"fr","title":"Français","format":"txt","content":"pain"}));
        assert_eq!(call("list",json!({"language":"fr"}))["documents"].as_array().unwrap().len(),1);
        assert_eq!(call("list",json!({"language":"en"}))["documents"].as_array().unwrap().len(),10);
        assert_eq!(call("document",json!({"language":"fr","id":french}))["document"]["language"],"fr");
        // Exercise the complete AI -> lemma lookup -> context match -> UI candidate response path.
        let root=call("import",json!({"language":"fr","title":"Root","format":"txt","content":"manger"}));
        call("annotate",json!({"language":"fr","document_id":root,"start":0,"end":6,"lemma":"manger","meaning":"吃","ungrouped_capture":true}));
        let root_note=call("candidates",json!({"language":"fr","document_id":root,"start":0,"end":6}))["notes"][0].clone();
        let target=call("import",json!({"language":"fr","title":"Passage","format":"txt","content":"mangeaient"}));
        let listener=std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint=format!("http://{}/chat/completions",listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let expected=root_note["id"].clone();
        let server=std::thread::spawn(move || {
            use std::io::{Read,Write};
            let base=json!({"lemma":"manger","meaning":"吃东西","context_meaning":"当时正在吃饭","sense_id":null,"group_id":null,"confidence":0.9,
                "learning":{"part_of_speech":"动词","usage_note":"过去时","example":"Ils mangeaient.","related":[]}});
            for index in 0..8 {
                let deadline=std::time::Instant::now()+std::time::Duration::from_secs(15);
                let mut stream=loop {
                    match listener.accept() {Ok((stream,_))=>break stream,Err(error) if error.kind()==std::io::ErrorKind::WouldBlock=>{
                        assert!(std::time::Instant::now()<deadline,"missing AI request {index}");
                        std::thread::sleep(std::time::Duration::from_millis(10));
                    },Err(error)=>panic!("{error}")}
                };
                stream.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
                let mut headers=Vec::new();let mut byte=[0];
                while !headers.ends_with(b"\r\n\r\n") {stream.read_exact(&mut byte).unwrap();headers.push(byte[0]);assert!(headers.len()<16000);}
                let headers=String::from_utf8(headers).unwrap();
                let length=headers.lines().find_map(|line|line.split_once(':').filter(|(key,_)|key.eq_ignore_ascii_case("content-length")).map(|(_,value)|value.trim().parse::<usize>().unwrap())).unwrap();
                let mut body=vec![0;length];stream.read_exact(&mut body).unwrap();
                let input:Value=serde_json::from_slice(&body).unwrap();
                let content:Value=serde_json::from_str(input["messages"][1]["content"].as_str().unwrap()).unwrap();
                if index==0 || index==6 || index==7 {assert!(content["existing_senses"].as_array().unwrap().is_empty());}
                else if index!=5 {assert_eq!(content["existing_senses"][0]["id"],expected);}
                let mut answer=base.clone();
                if index==1 {answer=json!({"sense_id":expected});}
                if index==2 || index==3 || index==4 {answer["sense_id"]=expected.clone();}
                if index==5 {assert_eq!(content["user_lemma"],"mangeaient");answer=json!({"reason":"原文是过去时变形，词典原形为 manger。"});}
                if index==6 || index==7 {assert_eq!(content["force_lemma"],true);assert_eq!(content["user_lemma"],"mangeaient");}
                if index==7 {answer["lemma"]=json!("mangeaient");}
                if index==3 {assert_eq!(content["user_lemma"],"manger");}
                let payload=json!({"choices":[{"message":{"content":answer.to_string()}}],"usage":{"total_tokens":10}}).to_string();
                write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",payload.len(),payload).unwrap();
            }
        });
        let response=call("suggest",json!({"language":"fr","document_id":target,"start":0,"end":10,"surface":"mangeaient","ungrouped_capture":true,"endpoint":endpoint,"model":"mock"}));
        assert_eq!(response["suggestion"]["sense_id"],root_note["id"]);
        assert_eq!(response["candidates"]["notes"][0]["id"],root_note["id"]);
        assert_eq!(response["suggestion"]["meaning"],"吃");assert_eq!(response["usage"]["total_tokens"],20);
        call("annotate",json!({"language":"fr","document_id":target,"start":0,"end":10,"sense_id":root_note["id"]}));
        let next=call("import",json!({"language":"fr","title":"Later passage","format":"txt","content":"mangeaient"}));
        let response=call("suggest",json!({"language":"fr","document_id":next,"start":0,"end":10,"surface":"mangeaient","ungrouped_capture":true,"endpoint":endpoint,"model":"mock"}));
        assert_eq!(response["suggestion"]["sense_id"],root_note["id"]);assert_eq!(response["usage"]["total_tokens"],10);
        let hint_doc=call("import",json!({"language":"fr","title":"Manual headword","format":"txt","content":"mangèrent"}));
        let response=call("suggest",json!({"language":"fr","document_id":hint_doc,"start":0,"end":9,"surface":"mangèrent","user_lemma":"manger","ungrouped_capture":true,"endpoint":endpoint,"model":"mock"}));
        assert_eq!(response["suggestion"]["sense_id"],root_note["id"]);
        assert_eq!(response["usage"]["total_tokens"],10);
        let decision_doc=call("import",json!({"language":"fr","title":"Headword decision","format":"txt","content":"mangeaient"}));
        let response=call("suggest",json!({"language":"fr","document_id":decision_doc,"start":0,"end":10,"surface":"mangeaient","user_lemma":"mangeaient","ungrouped_capture":true,"endpoint":endpoint,"model":"mock"}));
        assert_eq!(response["lemma_decision"]["accepted"],false);assert_eq!(response["lemma_decision"]["reason"],"原文是过去时变形，词典原形为 manger。");assert_eq!(response["usage"]["total_tokens"],20);
        let response=call("suggest",json!({"language":"fr","document_id":decision_doc,"start":0,"end":10,"surface":"mangeaient","user_lemma":"mangeaient","force_lemma":true,"ungrouped_capture":true,"endpoint":endpoint,"model":"mock"}));
        assert_eq!(response["suggestion"]["lemma"],"mangeaient");assert_eq!(response["lemma_decision"]["forced"],true);assert_eq!(response["lemma_decision"]["accepted"],true);assert_eq!(response["usage"]["total_tokens"],20);
        server.join().unwrap();
        let _=std::fs::remove_dir_all(path.with_file_name("languages"));
        drop(window);
        drop(app);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
