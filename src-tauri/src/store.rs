use base64::{engine::general_purpose::STANDARD, Engine};

use crate::{
    sqlite::{Db, Result},
    tags,
};
use serde_json::{json, Value as V};
use uuid::Uuid;

pub fn init(db: &Db) -> Result<()> {
    init_schema(db, true)
}

fn init_schema(db: &Db, seed: bool) -> Result<()> {
    db.transaction(|| {
        let version = db.one("PRAGMA user_version", &[])?["user_version"].as_i64();
        match version {
            Some(0) => db.batch(include_str!("schema.sql"))?,
            Some(1) => db.batch("ALTER TABLE documents ADD COLUMN source_url TEXT; ALTER TABLE occurrences ADD COLUMN learning TEXT;")?,
            Some(2) => db.batch("ALTER TABLE occurrences ADD COLUMN learning TEXT;")?,
            Some(3) => (),
            Some(4) => (),
            Some(5) => (),
            Some(6) => (),
            Some(7) => { migrate_context_meaning(db)?; return migrate_pdf(db); },
            Some(8) => return migrate_pdf(db),
            Some(9) => return migrate_pdf_storage(db),
            Some(10) => return Ok(()),
            _ => return Err("数据库版本不兼容".into()),
        }
        if matches!(version, Some(1..=3)) {
            db.batch("ALTER TABLE documents ADD COLUMN web_layout TEXT;")?;
        }
        if !matches!(version,Some(5|6)) { db.batch(include_str!("tags.sql"))?; }
        if version!=Some(6) { db.batch(include_str!("tag_scopes.sql"))?; }
        db.batch(include_str!("tag_defaults.sql"))?;
        if seed {
            let empty = db.one("SELECT NOT EXISTS(SELECT 1 FROM tags) AS empty",&[])?["empty"].clone();
            tags::install_unesco(db)?;
            db.write("INSERT INTO tag_defaults VALUES(1,?1)",&[empty])?;
        }
        migrate_context_meaning(db)?;
        migrate_pdf(db)
    })?;
    db.transaction(|| {
        if !db.query("PRAGMA table_info(semantic_groups)", &[])?.iter().any(|row| row["name"] == "sil_code") {
            db.batch("ALTER TABLE semantic_groups ADD COLUMN sil_code TEXT;")?;
        }
        Ok(())
    })?;
    db.batch("CREATE TABLE IF NOT EXISTS sense_sil (
        sense_id TEXT NOT NULL REFERENCES senses(id) ON DELETE CASCADE,
        code TEXT NOT NULL, PRIMARY KEY(sense_id,code)
    );
    CREATE TABLE IF NOT EXISTS sil_diagnostics (
        sense_id TEXT PRIMARY KEY REFERENCES senses(id) ON DELETE CASCADE,
        payload TEXT NOT NULL
    );
    CREATE VIEW IF NOT EXISTS sense_sil_memberships AS
        SELECT sense_id,code FROM sense_sil WHERE code<>''
        UNION
        SELECT s.id,g.sil_code FROM senses s JOIN semantic_groups g ON g.id=s.group_id
        WHERE g.sil_code IS NOT NULL AND NOT EXISTS(SELECT 1 FROM sense_sil m WHERE m.sense_id=s.id AND m.code='');
    CREATE TABLE IF NOT EXISTS epub_files (
        document_id TEXT PRIMARY KEY REFERENCES documents(id) ON DELETE CASCADE,
        data TEXT NOT NULL
    );")
}

fn migrate_context_meaning(db: &Db) -> Result<()> {
    db.batch("ALTER TABLE occurrences ADD COLUMN context_meaning TEXT;
        UPDATE occurrences SET context_meaning=(SELECT meaning FROM senses WHERE senses.id=occurrences.sense_id);
        PRAGMA user_version=8;")
}

fn migrate_pdf(db: &Db) -> Result<()> {
    db.batch("CREATE TABLE pdf_files (
        document_id TEXT PRIMARY KEY REFERENCES documents(id) ON DELETE CASCADE, data TEXT NOT NULL
    );
    CREATE TABLE pdf_pages (
        document_id TEXT NOT NULL REFERENCES pdf_files(document_id) ON DELETE CASCADE,
        page INTEGER NOT NULL CHECK(page>0), offset INTEGER NOT NULL CHECK(offset>=0),
        text TEXT NOT NULL, layer TEXT NOT NULL, PRIMARY KEY(document_id,page)
    );
    PRAGMA user_version=9;")?;
    migrate_pdf_storage(db)
}

fn migrate_pdf_storage(db: &Db) -> Result<()> {
    db.batch("ALTER TABLE pdf_files ADD COLUMN storage TEXT;
        CREATE INDEX pdf_pages_offset ON pdf_pages(document_id,offset);
        PRAGMA user_version=10;")
}

pub const PDF_MAX_BYTES: u64 = 100_000_000;

fn pdf_directory(db: &Db) -> std::path::PathBuf {
    db.path().with_file_name("pdf-originals")
}

fn pdf_path(db: &Db, storage: &str) -> Result<std::path::PathBuf> {
    Uuid::parse_str(storage).map_err(|_| "PDF 文件编号无效")?;
    Ok(pdf_directory(db).join(format!("{storage}.pdf")))
}

fn write_pdf(db: &Db, mut source: impl std::io::Read) -> Result<(String, std::path::PathBuf)> {
    use std::io::{Read, Write};
    let mut prefix = Vec::new();
    source.by_ref().take(1024).read_to_end(&mut prefix).map_err(|e| e.to_string())?;
    if !prefix.windows(5).any(|part| part == b"%PDF-") {
        return Err("PDF 文件格式无效".into());
    }
    std::fs::create_dir_all(pdf_directory(db)).map_err(|e| e.to_string())?;
    let storage = Uuid::new_v4().to_string();
    let path = pdf_path(db, &storage)?;
    let result = (|| {
        let mut file = std::fs::OpenOptions::new().write(true).create_new(true)
            .open(&path).map_err(|e| e.to_string())?;
        file.write_all(&prefix).map_err(|e| e.to_string())?;
        let remainder = std::io::copy(&mut source.take(PDF_MAX_BYTES + 1 - prefix.len() as u64), &mut file)
            .map_err(|e| e.to_string())?;
        if remainder + prefix.len() as u64 > PDF_MAX_BYTES {
            return Err("PDF 文件最大 100 MB".into());
        }
        file.sync_all().map_err(|e| e.to_string())
    })();
    if let Err(error) = result {
        let _ = std::fs::remove_file(&path);
        return Err(error);
    }
    Ok((storage,path))
}

pub fn import_pdf(db: &Db, title: &str, source: impl std::io::Read) -> Result<V> {
    let mut created = None;
    let result = db.transaction(|| {
        let (storage,path) = write_pdf(db,source)?;
        created = Some(path);
        let key = uuid();
        db.write("INSERT INTO documents(id,title,format,content) VALUES(?1,?2,'txt','')",
            &[key.clone(),json!(title)])?;
        db.write("INSERT INTO pdf_files(document_id,data,storage) VALUES(?1,'',?2)",
            &[key.clone(),json!(storage)])?;
        Ok(key)
    });
    if result.is_err() {
        if let Some(path) = created {
            let _ = std::fs::remove_file(path);
        }
    }
    result
}

pub fn pdf_prepare(db: &Db, p: &V) -> Result<V> {
    use base64::{engine::general_purpose::STANDARD, read::DecoderReader};
    let key = id(p, "id")?;
    let mut created = None;
    let result = db.transaction(|| {
        let args = std::slice::from_ref(&key);
        let file = db.one("SELECT storage FROM pdf_files WHERE document_id=?1", args)?;
        if file["storage"].is_null() {
            let legacy = db.one("SELECT data FROM pdf_files WHERE document_id=?1", args)?;
            let source = legacy["data"].as_str().ok_or("PDF 文件数据无效")?;
            let (storage,path) = write_pdf(db,DecoderReader::new(source.as_bytes(), &STANDARD))?;
            created = Some(path);
            db.write("UPDATE pdf_files SET data='',storage=?1 WHERE document_id=?2",
                &[json!(storage),key.clone()])?;
        }
        run(db,"pdf_load",p)
    });
    if result.is_err() {
        if let Some(path) = created {
            let _ = std::fs::remove_file(path);
        }
    }
    result
}

pub fn pdf_range(db: &Db, p: &V, begin: u64, end: u64) -> Result<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};
    let key = id(p, "id")?;
    let file = db.one("SELECT storage FROM pdf_files WHERE document_id=?1", &[key])?;
    let path = pdf_path(db,file["storage"].as_str().ok_or("PDF 尚未准备就绪")?)?;
    let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    if end <= begin || end - begin > PDF_MAX_BYTES || end > file.metadata().map_err(|e| e.to_string())?.len() {
        return Err("PDF 读取范围无效".into());
    }
    let mut bytes = vec![0; (end-begin) as usize];
    file.seek(SeekFrom::Start(begin)).and_then(|_| file.read_exact(&mut bytes)).map_err(|e| e.to_string())?;
    Ok(bytes)
}

pub fn prune_pdf_files(db: &Db) -> Result<()> {
    db.transaction(|| {
        let directory = pdf_directory(db);
        if !directory.is_dir() {
            return Ok(());
        }
        let files = db.query("SELECT storage FROM pdf_files WHERE storage IS NOT NULL", &[])?;
        let referenced: std::collections::HashSet<_> = files.iter().filter_map(|file| file["storage"].as_str()).collect();
        for entry in std::fs::read_dir(directory).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("pdf") {
                continue;
            }
            if let Some(key) = path.file_stem().and_then(|stem| stem.to_str()) {
                if Uuid::parse_str(key).is_ok() && !referenced.contains(key) {
                    std::fs::remove_file(path).map_err(|e| e.to_string())?;
                }
            }
        }
        Ok(())
    })
}

fn save_pdf_page(db: &Db, p: &V) -> Result<V> {
    let key = id(p, "id")?;
    let page = p["page"].as_u64().filter(|n| (1..=100_000).contains(n)).ok_or("PDF 页码无效")?;
    let args = &[key.clone(),json!(page)];
    if let Some(row) = db.query("SELECT page,offset,text,layer FROM pdf_pages WHERE document_id=?1 AND page=?2", args)?.pop() {
        return Ok(row);
    }
    let text = p["text"].as_str().filter(|text| text.len() <= 2_000_000 && !text.contains(['\0','\r','\n']))
        .ok_or("PDF 页面文字无效或过长")?;
    let layer = p["layer"].as_object().ok_or("PDF 文字层无效")?;
    let layer = json!(layer).to_string();
    if layer.len() > 12_000_000 {
        return Err("PDF 页面文字层过大".into());
    }
    db.one("SELECT 1 AS present FROM pdf_files WHERE document_id=?1", &[key.clone()])?;
    let previous = db.query("SELECT page,offset,text FROM pdf_pages WHERE document_id=?1 ORDER BY page DESC LIMIT 1",
        &[key.clone()])?.pop();
    let last = previous.as_ref().and_then(|row| row["page"].as_u64()).unwrap_or(0);
    if last != page - 1 {
        return Err("PDF 页面索引顺序无效".into());
    }
    let offset = previous.map_or(0, |row| row["offset"].as_u64().unwrap()
        + row["text"].as_str().unwrap().encode_utf16().count() as u64 + 2);
    db.write("INSERT INTO pdf_pages VALUES(?1,?2,?3,?4,?5)",
        &[key,json!(page),json!(offset),json!(text),json!(layer)])?;
    Ok(json!({"page":page,"offset":offset,"text":text,"layer":layer}))
}

fn uuid() -> V {
    json!(Uuid::new_v4().to_string())
}

pub fn field<'a>(p: &'a V, key: &str, max: usize, required: bool) -> Result<&'a str> {
    let value = p[key].as_str().unwrap_or_default().trim();
    if (required && value.is_empty()) || value.chars().count() > max || value.contains('\0') {
        return Err(format!("{key} 不能为空或超出长度限制（{max} 字符）"));
    }
    Ok(value)
}

fn id(p: &V, key: &str) -> Result<V> {
    let value = field(p, key, 36, true)?;
    Uuid::parse_str(value).map_err(|_| format!("{key} 无效"))?;
    Ok(json!(value))
}

fn ids(p: &V, key: &str) -> Result<Vec<V>> {
    let list = p[key]
        .as_array()
        .filter(|v| !v.is_empty() && v.len() <= 500)
        .ok_or("请选择 1–500 条记录")?;
    list.iter().map(|v| id(&json!({"id":v}), "id")).collect()
}

const MARKS: &str = "SELECT o.*, d.title, e.lemma, s.meaning, s.group_id, g.name AS group_name
    FROM occurrences o JOIN documents d ON d.id=o.document_id
    JOIN senses s ON s.id=o.sense_id JOIN expressions e ON e.id=s.expression_id
    JOIN semantic_groups g ON g.id=s.group_id";
const SENSES: &str = "SELECT s.*, e.lemma FROM senses s JOIN expressions e ON e.id=s.expression_id";

fn reindex(db: &Db, group: &V) -> Result<()> {
    db.query(
        "DELETE FROM group_search WHERE group_id=?1",
        &[group.clone()],
    )?;
    db.query("INSERT INTO group_search(group_id,body)
        SELECT g.id, g.name || ' ' || g.description || ' ' ||
        coalesce((SELECT group_concat(expression || ' ' || nuance,' ') FROM group_vocabulary WHERE group_id=g.id),'') || ' ' ||
        coalesce((SELECT group_concat(e.lemma || ' ' || s.meaning,' ') FROM senses s JOIN expressions e ON e.id=s.expression_id WHERE s.group_id=g.id),'')
        FROM semantic_groups g WHERE g.id=?1", &[group.clone()])?;
    Ok(())
}

fn match_query(q: &str) -> String {
    q.split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .take(12)
        .map(|s| format!("\"{s}\"*"))
        .collect::<Vec<_>>()
        .join(" OR ")
}

fn groups(db: &Db, q: &str, limit: Option<i64>) -> Result<Vec<V>> {
    let query = match_query(q);
    if query.is_empty() {
        return db.query(
            "SELECT * FROM semantic_groups ORDER BY name COLLATE NOCASE LIMIT ?1",
            &[json!(limit.unwrap_or(-1))],
        );
    }
    if let Some(limit) = limit {
        return db.query(
            "SELECT g.* FROM group_search f JOIN semantic_groups g ON g.id=f.group_id
            WHERE group_search MATCH ?1 ORDER BY bm25(group_search), g.name LIMIT ?2",
            &[json!(query), json!(limit)],
        );
    }
    db.query("WITH matches AS MATERIALIZED (SELECT group_id,bm25(group_search) AS score FROM group_search WHERE group_search MATCH ?1)
        SELECT g.* FROM group_search f JOIN semantic_groups g ON g.id=f.group_id
        LEFT JOIN matches m ON m.group_id=g.id
        WHERE m.score IS NOT NULL OR instr(lower(f.body),lower(?2))>0
        ORDER BY m.score IS NULL,m.score,g.name",
        &[json!(query),json!(q)])
}

fn group_senses(db: &Db, key: &V) -> Result<Vec<V>> {
    db.query(
        &format!("{SENSES} WHERE s.group_id=?1 ORDER BY e.lemma,s.meaning,s.id"),
        std::slice::from_ref(key),
    )
}

fn group(db: &Db, key: &V) -> Result<V> {
    db.one("SELECT * FROM semantic_groups WHERE id=?1", &[key.clone()])
}

fn vocabulary(db: &Db, key: &V) -> Result<Vec<V>> {
    db.query(
        "SELECT expression, nuance FROM group_vocabulary WHERE group_id=?1 ORDER BY expression",
        &[key.clone()],
    )
}

fn put_group(db: &Db, p: &V) -> Result<V> {
    let name = field(p, "name", 120, true)?;
    let description = field(p, "description", 2000, false)?;
    let vocab = field(p, "vocabulary", 6000, false)?;
    let key = if p["id"].is_null() {
        let key = uuid();
        db.write(
            "INSERT INTO semantic_groups(id,name,description) VALUES(?1,?2,?3)",
            &[key.clone(), json!(name), json!(description)],
        )?;
        key
    } else {
        let key = id(p, "id")?;
        db.write(
            "UPDATE semantic_groups SET name=?2,description=?3 WHERE id=?1",
            &[key.clone(), json!(name), json!(description)],
        )?;
        db.query(
            "DELETE FROM group_vocabulary WHERE group_id=?1",
            &[key.clone()],
        )?;
        key
    };
    if let Some(code) = p.get("sil_code") {
        let code = checked_sil_code(code)?;
        db.query("UPDATE semantic_groups SET sil_code=?2 WHERE id=?1", &[key.clone(), code])?;
    }
    for line in vocab.lines().map(str::trim).filter(|line| !line.is_empty()) {
        let (expression, nuance) = line.split_once('|').unwrap_or((line, ""));
        if expression.trim().is_empty() {
            return Err("核心表达不能为空".into());
        }
        db.query(
            "INSERT INTO group_vocabulary VALUES(?1,?2,?3)
            ON CONFLICT(group_id,expression) DO UPDATE SET nuance=excluded.nuance",
            &[key.clone(), json!(expression.trim()), json!(nuance.trim())],
        )?;
    }
    if let Some(values) = p.get("tags") {
        tags::save_group(db, &key, values)?;
    }
    reindex(db, &key)?;
    Ok(key)
}

fn new_group(db: &Db, p: &V) -> Result<V> {
    if !p["id"].is_null() {
        return Err("新组不能带已有 ID".into());
    }
    put_group(db, p)
}

pub fn import(db: &Db, title: &str, format: &str, content: &str) -> Result<V> {
    insert_document(db, title, format, content, None, V::Null)
}

pub fn import_rich_text(db: &Db, title: &str, content: &str, layout: &crate::web_layout::Layout, original: &[u8]) -> Result<V> {
    let layout = serde_json::to_string(layout).map_err(|_| "无法保存 EPUB 版式")?;
    if layout.len() > crate::web_layout::MAX_LAYOUT_BYTES { return Err("EPUB 版式数据过大".into()); }
    db.transaction(|| {
        let id = insert_document(db, title, "txt", content, None, json!(layout))?;
        db.write("INSERT INTO epub_files VALUES(?1,?2)", &[id.clone(), json!(STANDARD.encode(original))])?;
        Ok(id)
    })
}

pub fn import_web(db: &Db, page: crate::web_import::Page) -> Result<V> {
    let layout = page
        .layout
        .map(|layout| {
            let value = serde_json::to_string(&layout).map_err(|_| "无法保存网页版式")?;
            if value.len() > crate::web_layout::MAX_LAYOUT_BYTES {
                return Err("网页版式数据过大");
            }
            Ok(json!(value))
        })
        .transpose()?
        .unwrap_or(V::Null);
    insert_document(
        db,
        &page.title,
        "txt",
        &page.content,
        Some(&page.url),
        layout,
    )
}

pub fn image_repair_snapshot(db: &Db, p: &V) -> Result<V> {
    let row = db.one(
        "SELECT source_url,web_layout FROM documents WHERE id=?1",
        &[id(p, "id")?],
    )?;
    if !row["source_url"].is_string() {
        return Err("这篇文档没有网页来源".into());
    }
    Ok(row)
}

pub fn seed_saved_images(page: &mut crate::web_import::Page, snapshot: &V) -> Result<()> {
    let layout = page.layout.as_mut().ok_or("该地址没有可读取的网页版式")?;
    let previous = saved_image_layout(snapshot)?;
    layout.check_image_sequence(&previous)?;
    layout.images = previous.images;
    Ok(())
}

fn saved_image_layout(row: &V) -> Result<crate::web_layout::Layout> {
    let raw = row["web_layout"].as_str().ok_or("这篇文档没有保存网页版式")?;
    serde_json::from_str(raw).map_err(|_| "旧网页版式数据无效".into())
}

pub fn repair_images(db: &Db, p: &V, source: &str, page: crate::web_import::Page) -> Result<V> {
    let key = id(p, "id")?;
    let downloaded = page.layout.ok_or("该地址没有可读取的网页版式")?;
    db.transaction(|| {
        let args = std::slice::from_ref(&key);
        let old = db.one("SELECT source_url,web_layout FROM documents WHERE id=?1", args)?;
        if old["source_url"] != source {
            return Err("文档的网页来源已变化，请重新尝试。".into());
        }
        let mut layout = saved_image_layout(&old)?;
        layout.fill_missing_images(downloaded)?;
        let saved = layout.images.iter().filter(|image| image.is_some()).count();
        let missing = layout.images.len() - saved;
        let data = serde_json::to_string(&layout).map_err(|_| "无法保存补图结果")?;
        if data.len() > crate::web_layout::MAX_LAYOUT_BYTES {
            return Err("补图结果过大".into());
        }
        db.write(
            "UPDATE documents SET web_layout=?1 WHERE id=?2",
            &[json!(data), key.clone()],
        )?;
        Ok(json!({"saved":saved,"missing":missing}))
    })
}

fn insert_document(
    db: &Db,
    title: &str,
    format: &str,
    content: &str,
    source: Option<&str>,
    layout: V,
) -> Result<V> {
    if title.trim().is_empty()
        || title.chars().count() > 255
        || !matches!(format, "md" | "txt")
        || content.trim().is_empty()
        || content.len() > if layout.is_null() || source.is_some() { 2_000_000 } else { crate::epub::TEXT_LIMIT }
        || content.contains('\0')
    {
        return Err("请选择非空 UTF-8 Markdown/TXT 文件（最大 2 MB）".into());
    }
    let key = uuid();
    db.write(
        "INSERT INTO documents(id,title,format,content,source_url,web_layout) VALUES(?1,?2,?3,?4,?5,?6)",
        &[
            key.clone(),
            json!(title),
            json!(format),
            json!(content),
            json!(source),
            layout,
        ],
    )?;
    Ok(key)
}

fn byte_offset(text: &str, utf16: usize) -> Option<usize> {
    let mut units = 0;
    for (byte, ch) in text.char_indices() {
        if units == utf16 {
            return Some(byte);
        }
        if units > utf16 {
            return None;
        }
        units += ch.len_utf16();
    }
    (units == utf16).then_some(text.len())
}

pub fn selection(db: &Db, p: &V) -> Result<(V, V, V, String, String)> {
    let doc = id(p, "document_id")?;
    let start = p["start"]
        .as_u64()
        .filter(|n| *n <= 100_000_000_000)
        .ok_or("起始位置无效")?;
    let end = p["end"]
        .as_u64()
        .filter(|n| *n > start && *n - start <= 240)
        .ok_or("请选择 1–240 个字符")? as usize;
    let indexed = db.query("SELECT text AS content,offset FROM pdf_pages WHERE document_id=?1 AND offset<=?2
        ORDER BY offset DESC LIMIT 1", &[doc.clone(),json!(start)])?.pop();
    let row = match indexed {
        Some(row) => row,
        None => db.one("SELECT content,0 AS offset FROM documents WHERE id=?1", &[doc.clone()])?,
    };
    let offset = row["offset"].as_u64().unwrap_or(0) as usize;
    let content = row["content"].as_str().unwrap();
    let a = byte_offset(content, start as usize - offset).ok_or("起始位置无效")?;
    let b = byte_offset(content, end - offset).ok_or("结束位置无效")?;
    let surface = &content[a..b];
    if surface.trim() != surface || surface.is_empty() || surface.contains(['\n', '\r']) {
        return Err("请选择同一行中的词或短语".into());
    }
    Ok((doc, json!(start), json!(end), surface.into(), passage_context(content, a, b)))
}

fn passage_context(content: &str, a: usize, b: usize) -> String {
    let surface = &content[a..b];
    let prefix: String = content[..a]
        .chars()
        .rev()
        .take_while(|c| !matches!(c, '\r' | '\n'))
        .take(240)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let suffix: String = content[b..]
        .chars()
        .take_while(|c| !matches!(c, '\r' | '\n'))
        .take(240)
        .collect();
    format!("{prefix}⟦{surface}⟧{suffix}")
}

fn edit_document(db: &Db, p: &V) -> Result<V> {
    let key = id(p, "id")?;
    let args = std::slice::from_ref(&key);
    if !db.query("SELECT 1 FROM pdf_files WHERE document_id=?1", args)?.is_empty() {
        return Err("原版式 PDF 不支持编辑正文".into());
    }
    let old = db.one("SELECT content,web_layout,source_url FROM documents WHERE id=?1", args)?;
    if old["content"] != p["original"] {
        return Err("文章已更新，请重新打开后编辑".into());
    }
    let limit = if old["web_layout"].is_string() && old["source_url"].is_null() { crate::epub::TEXT_LIMIT } else { 2_000_000 };
    let content = p["content"].as_str()
        .filter(|s| !s.trim().is_empty() && s.len() <= limit && !s.contains('\0'))
        .ok_or_else(|| format!("正文不能为空，且最大为 {} MB", limit / 1_000_000))?;
    if old["content"] == content {
        return Ok(json!(0));
    }
    let layout = if let Some(raw) = old["web_layout"].as_str() {
        let mut layout: crate::web_layout::Layout =
            serde_json::from_str(raw).map_err(|_| "网页版式数据无效")?;
        layout.nodes = serde_json::from_value(p["web_nodes"].clone())
            .map_err(|_| "编辑后的网页版式无效")?;
        let serialized = serde_json::to_string(&layout).map_err(|_| "无法保存网页版式")?;
        if serialized.len() > crate::web_layout::MAX_LAYOUT_BYTES {
            return Err("网页版式数据过大".into());
        }
        json!(serialized)
    } else {
        V::Null
    };
    let marks = db.query("SELECT id,surface FROM occurrences WHERE document_id=?1 ORDER BY start,end", args)?;
    let positions = p["positions"].as_array().ok_or("标注位置无效")?;
    if positions.len() != marks.len() {
        return Err("文章标注已更新，请重新打开后编辑".into());
    }

    // Vacate the old ranges before moving marks to avoid collisions with their unique index.
    db.query("UPDATE occurrences SET start=start+?2,end=end+?2 WHERE document_id=?1", &[key.clone(), json!(limit + 1)])?;
    let mut previous_end = 0;
    let mut previous_byte = 0;
    let mut removed = 0;
    for (mark, position) in marks.iter().zip(positions) {
        if mark["id"] != position["id"] {
            return Err("文章标注已更新，请重新打开后编辑".into());
        }
        let mark_id = &mark["id"];
        if position["start"].is_null() {
            db.write("DELETE FROM occurrences WHERE id=?1", std::slice::from_ref(mark_id))?;
            removed += 1;
            continue;
        }
        let surface = mark["surface"].as_str().unwrap();
        let start = position["start"].as_u64().filter(|n| *n <= limit as u64)
            .ok_or("标注位置无效")? as usize;
        let end = start + surface.encode_utf16().count();
        let gap = start.checked_sub(previous_end).ok_or("标注顺序无效")?;
        let a = previous_byte + byte_offset(&content[previous_byte..], gap).ok_or("标注起点无效")?;
        if !content[a..].starts_with(surface) {
            return Err("标注与编辑后的正文不一致".into());
        }
        let b = a + surface.len();
        previous_end = end;
        previous_byte = b;
        db.write("UPDATE occurrences SET start=?1,end=?2,context=?3 WHERE id=?4",
            &[json!(start), json!(end), json!(passage_context(content, a, b)), mark_id.clone()])?;
    }
    db.write("UPDATE documents SET content=?1,web_layout=?2 WHERE id=?3", &[json!(content), layout, key])?;
    Ok(json!(removed))
}

fn normalize(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

pub fn group_with_name(db: &Db, name: &str) -> Result<Option<V>> {
    let normalized = normalize(name);
    Ok(db
        .query("SELECT id,name FROM semantic_groups ORDER BY rowid", &[])?
        .into_iter()
        .find(|group| normalize(group["name"].as_str().unwrap_or_default()) == normalized)
        .map(|group| group["id"].clone()))
}

pub fn comparison_input(db: &Db, p: &V) -> Result<V> {
    let id = id(p, "id")?;
    let current = group(db, &id)?;
    let exclude = p["exclude"].as_str();
    let senses: Vec<_> = group_senses(db, &id)?.into_iter()
        .filter(|sense| sense["id"].as_str() != exclude)
        .take(12)
        .map(|sense| json!({"id":sense["id"],"lemma":sense["lemma"],
            "meaning":sense["meaning"].as_str().unwrap_or_default().chars().take(240).collect::<String>()}))
        .collect();
    Ok(
        json!({"group":current["name"],"lemma":field(p,"lemma",240,true)?,
        "meaning":field(p,"meaning",1200,true)?,"senses":senses}),
    )
}

pub fn organization_input(db: &Db, p: &V) -> Result<V> {
    let id = id(p, "id")?;
    let current = group(db, &id)?;
    let words = vocabulary(db, &id)?;
    let terms = words
        .iter()
        .take(4)
        .map(|row| row["expression"].clone())
        .collect::<Vec<_>>();
    let terms = std::iter::once(current["name"].clone())
        .chain(terms)
        .collect::<Vec<_>>();
    let matches = followup_groups(db, &json!(terms), &json!([{"id":id}]))?;
    let senses = group_senses(db, &id)?;
    let allow_split = (2..=60).contains(&senses.len());
    let senses: Vec<_> = senses.into_iter().take(60).map(|sense| json!({
        "id":sense["id"],"lemma":sense["lemma"],
        "meaning":sense["meaning"].as_str().unwrap_or_default().chars().take(240).collect::<String>()
    })).collect();
    Ok(json!({"current":current,"senses":senses,"allow_split":allow_split,"candidates":matches}))
}

pub fn sil_batch(db: &Db, cursor: &str, parent: &str) -> Result<V> {
    let mut rows = db.query(
        "SELECT s.id,e.lemma,s.meaning,g.sil_code AS current_code,(SELECT substr(o.context,1,400) FROM occurrences o
         WHERE o.sense_id=s.id ORDER BY o.rowid LIMIT 1) AS context
         FROM senses s JOIN expressions e ON e.id=s.expression_id
         JOIN semantic_groups g ON g.id=s.group_id WHERE s.id>?1 AND (EXISTS(SELECT 1 FROM sense_sil_memberships m WHERE m.sense_id=s.id AND m.code=?2) OR (?2='' AND NOT EXISTS(SELECT 1 FROM sense_sil_memberships m WHERE m.sense_id=s.id))) ORDER BY s.id LIMIT 101",
        &[json!(cursor),json!(parent)],
    )?;
    let next = if rows.len()>100 {
        rows.pop();
        rows.last().unwrap()["id"].clone()
    } else { V::Null };
    Ok(json!({"rows":rows,"next":next}))
}

pub fn sil_audit_rows(db: &Db, code: &str, descendants: bool) -> Result<Vec<V>> {
    let mut rows = db.query("SELECT s.id,e.lemma,s.meaning FROM senses s
        JOIN expressions e ON e.id=s.expression_id WHERE EXISTS(
          SELECT 1 FROM sense_sil_memberships m WHERE m.sense_id=s.id
          AND (m.code=?1 OR (?2=1 AND m.code LIKE ?1 || '.%')))
        ORDER BY s.id", &[json!(code),json!(i32::from(descendants))])?;
    for row in &mut rows {
        row["categories"] = json!(db.query("SELECT code FROM sense_sil_memberships WHERE sense_id=?1 ORDER BY code",
            &[row["id"].clone()])?.into_iter().map(|membership| membership["code"].clone()).collect::<Vec<_>>());
    }
    Ok(rows)
}

pub fn sil_count(db: &Db, parent: &str) -> Result<V> {
    Ok(db.one("SELECT count(*) AS n FROM senses s JOIN semantic_groups g ON g.id=s.group_id
        WHERE EXISTS(SELECT 1 FROM sense_sil_memberships m WHERE m.sense_id=s.id AND m.code=?1) OR (?1='' AND NOT EXISTS(SELECT 1 FROM sense_sil_memberships m WHERE m.sense_id=s.id))", &[json!(parent)])?["n"].clone())
}

fn apply_sil(db: &Db, p: &V, manual: bool) -> Result<V> {
    let rows = p["rows"].as_array().filter(|rows| rows.len() <= 10000).ok_or("义项清单无效")?;
    let mut touched = std::collections::HashSet::new();
    let mut destination = V::Null;
    for row in rows {
        let code = row["code"].as_str().ok_or("缺少 SIL 编号")?;
        let returning = code=="unclassified";
        let name = if returning { "待重新归类" } else { crate::sil::name(code).ok_or("SIL 编号无效")? };
        let parent = p["parent"].as_str().ok_or("缺少当前分类")?;
        if row["current_code"].as_str().unwrap_or_default()!=parent
            || (!manual && row["manual"] != true && row["ranked_move"] != true && !(returning && !parent.is_empty() || crate::sil::children(parent).iter().any(|child|child[0]==code))) {
            return Err("义项只能移到当前分类的直属子类".into());
        }
        let sense = db.one("SELECT * FROM senses WHERE id=?1", &[row["id"].clone()])?;
        let current = db.one("SELECT sil_code FROM semantic_groups WHERE id=?1", &[sense["group_id"].clone()])?;
        if sense["meaning"] != row["meaning"] || current["sil_code"] != row["current_code"] { return Err("义项已修改，请重新复核".into()); }
        let source = sense["group_id"].clone();
        let existing = if returning {
            db.query("SELECT id FROM semantic_groups WHERE sil_code IS NULL AND name=?1 AND description=?2 ORDER BY id LIMIT 1",
                &[json!(name),json!("从 SIL 分类退回的义项")])?
        } else {
            db.query("SELECT id FROM semantic_groups WHERE sil_code=?1 ORDER BY id LIMIT 1", &[json!(code)])?
        };
        let target = if let Some(group) = existing.first() {
            let target = group["id"].clone();
            db.query("UPDATE semantic_groups SET name=?2 WHERE id=?1", &[target.clone(),json!(name)])?;
            target
        } else if returning {
            new_group(db,&json!({"name":name,"description":"从 SIL 分类退回的义项"}))?
        } else {
            new_group(db,&json!({"name":name,"sil_code":code}))?
        };
        destination = target.clone();
        if source != target {
            let duplicate = db.query("SELECT id FROM senses WHERE group_id=?1 AND expression_id=?2 AND meaning=?3",
                &[target.clone(),sense["expression_id"].clone(),sense["meaning"].clone()])?;
            if let Some(existing) = duplicate.first() {
                db.query("UPDATE occurrences SET sense_id=?2 WHERE sense_id=?1", &[sense["id"].clone(),existing["id"].clone()])?;
                db.write("DELETE FROM senses WHERE id=?1", &[sense["id"].clone()])?;
            } else {
                db.write("UPDATE senses SET group_id=?2 WHERE id=?1", &[sense["id"].clone(),target.clone()])?;
            }
            db.query("INSERT OR IGNORE INTO group_tags SELECT ?2,tag_id FROM group_tags WHERE group_id=?1",
                &[source.clone(),target.clone()])?;
            db.query("INSERT OR IGNORE INTO group_vocabulary SELECT ?2,expression,nuance FROM group_vocabulary WHERE group_id=?1",
                &[source.clone(),target.clone()])?;
        }
        touched.insert(source.as_str().unwrap_or_default().to_owned());
        touched.insert(target.as_str().unwrap_or_default().to_owned());
    }
    for group in touched {
        let key = json!(group);
        if db.one("SELECT count(*) AS n FROM senses WHERE group_id=?1", &[key.clone()])?["n"] == 0 {
            db.query("DELETE FROM group_search WHERE group_id=?1", &[key.clone()])?;
            db.write("DELETE FROM semantic_groups WHERE id=?1", &[key])?;
        } else {
            reindex(db,&key)?;
        }
    }
    db.query("DELETE FROM operation_history", &[])?;
    Ok(if manual { destination } else { json!(rows.len()) })
}

fn checked_sil_code(value: &V) -> Result<V> {
    match value {
        V::Null => Ok(V::Null),
        V::String(code) if code.is_empty() => Ok(V::Null),
        V::String(code) if crate::sil::valid(code) => Ok(value.clone()),
        _ => Err("SIL 分类编号无效".into()),
    }
}

pub fn sil_followup_groups(db: &Db, terms: &V, previous: &V, code: &V) -> Result<Vec<V>> {
    let mut groups = followup_groups(db, terms, previous)?;
    if let Some(code) = code.as_str() {
        let nearby = db.query(
            "SELECT * FROM semantic_groups WHERE sil_code=?1 OR sil_code LIKE ?1 || '.%'
             OR ?1 LIKE sil_code || '.%' ORDER BY sil_code=?1 DESC,name LIMIT 12",
            &[json!(code)],
        )?;
        let mut added = Vec::new();
        for mut group in nearby {
            if previous.as_array().into_iter().flatten().chain(groups.iter()).any(|seen| seen["id"]==group["id"]) {
                continue;
            }
            group["vocabulary"] = json!(vocabulary(db, &group["id"])?);
            added.push(group);
            if added.len()==2 { break; }
        }
        added.extend(groups);
        groups = added;
    }
    Ok(groups)
}

pub fn followup_groups(db: &Db, terms: &V, previous: &V) -> Result<Vec<V>> {
    let query = terms
        .as_array()
        .into_iter()
        .flatten()
        .take(5)
        .filter_map(V::as_str)
        .filter(|term| term.len() <= 80 && term.chars().any(char::is_alphanumeric))
        .collect::<Vec<_>>()
        .join(" ");
    if query.is_empty() {
        return Ok(Vec::new());
    }
    let previous = previous.as_array().map(Vec::as_slice).unwrap_or_default();
    let mut matches = Vec::new();
    for mut group in groups(db, &query, Some(5 + previous.len() as i64))? {
        if previous.iter().any(|seen| seen["id"] == group["id"]) {
            continue;
        }
        group["vocabulary"] = json!(vocabulary(db, &group["id"])?);
        matches.push(group);
        if matches.len() == 5 {
            break;
        }
    }
    Ok(matches)
}

// These are possible inflections. The context still determines which sense applies.
fn inflection_keys(word: &str) -> Vec<String> {
    let normalized = normalize(word);
    let mut keys = vec![normalized.clone()];
    let (first, tail) = normalized.split_once(' ').unwrap_or((&normalized, ""));
    if !first.bytes().all(|c| c.is_ascii_lowercase()) {
        return keys;
    }
    let mut add = |base: &str| {
        let key = if tail.is_empty() {
            base.to_owned()
        } else {
            format!("{base} {tail}")
        };
        if !keys.contains(&key) {
            keys.push(key);
        }
    };
    for (base, forms) in [
        ("be", "am is are was were been being"),
        ("go", "goes went gone"),
        ("have", "has had"),
        ("do", "does did done"),
        ("take", "took taken"),
        ("get", "got gotten"),
        ("run", "ran"),
        ("write", "wrote written"),
        ("see", "saw seen"),
        ("come", "came"),
        ("bring", "brought"),
        ("think", "thought"),
        ("buy", "bought"),
        ("make", "made"),
        ("find", "found"),
        ("give", "gave given"),
        ("child", "children"),
        ("person", "people"),
        ("man", "men"),
        ("woman", "women"),
        ("mouse", "mice"),
        ("foot", "feet"),
        ("tooth", "teeth"),
    ] {
        if forms.split_whitespace().any(|form| form == first) {
            add(base);
        }
    }
    for (suffix, replacement) in [("ies", "y"), ("ied", "y"), ("ves", "f"), ("ves", "fe")] {
        if let Some(stem) = first.strip_suffix(suffix).filter(|stem| stem.len() >= 2) {
            add(&format!("{stem}{replacement}"));
        }
    }
    if let Some(stem) = first.strip_suffix("es").filter(|stem| {
        ["s", "x", "z", "ch", "sh", "o"]
            .iter()
            .any(|end| stem.ends_with(end))
    }) {
        add(stem);
    }
    if !first.ends_with("ss") {
        if let Some(stem) = first.strip_suffix('s').filter(|stem| stem.len() >= 3) {
            add(stem);
        }
    }
    for suffix in ["ed", "ing"] {
        if let Some(stem) = first.strip_suffix(suffix).filter(|stem| stem.len() >= 2) {
            add(stem);
            add(&format!("{stem}e"));
            let bytes = stem.as_bytes();
            let last = bytes[bytes.len() - 1];
            if last == bytes[bytes.len() - 2] && !b"aeiou".contains(&last) {
                add(&stem[..stem.len() - 1]);
            }
        }
    }
    keys
}

fn matching_inflections(db: &Db, word: &str) -> Result<Vec<V>> {
    let keys = inflection_keys(word);
    Ok(db
        .query("SELECT normalized FROM expressions", &[])?
        .into_iter()
        .filter_map(|row| {
            let normalized = row["normalized"].as_str()?;
            inflection_keys(normalized)
                .iter()
                .any(|key| keys.contains(key))
                .then(|| row["normalized"].clone())
        })
        .collect())
}

pub fn candidates(db: &Db, p: &V) -> Result<V> {
    let (doc, start, end, surface, context) = selection(db, p)?;
    let saved_sense = db.query(
        "SELECT e.lemma,s.meaning FROM occurrences o JOIN senses s ON s.id=o.sense_id
         JOIN expressions e ON e.id=s.expression_id WHERE o.document_id=?1 AND o.start=?2 AND o.end=?3",
        &[doc,start,end],
    )?.into_iter().next();
    let forms = matching_inflections(db, &surface)?;
    let exact = db.query(
        &format!(
            "{SENSES} WHERE e.normalized IN (SELECT value FROM json_each(?1)) OR e.id IN (
            SELECT prior.expression_id FROM occurrences o JOIN senses prior ON prior.id=o.sense_id
            WHERE o.surface=?2 COLLATE NOCASE
        ) ORDER BY (e.normalized=?3) DESC,s.meaning,s.id LIMIT 30"
        ),
        &[json!(V::Array(forms).to_string()), json!(surface), json!(normalize(&surface))],
    )?;
    let reused = db.query("SELECT DISTINCT s.id, s.group_id, s.meaning, e.lemma
        FROM occurrences o JOIN senses s ON s.id=o.sense_id JOIN expressions e ON e.id=s.expression_id
        WHERE o.surface=?1 AND o.context=?2 ORDER BY s.id LIMIT 5", &[json!(surface),json!(context)])?;
    let mut notes = Vec::new();
    for sense in reused.iter().chain(exact.iter()) {
        if notes.iter().any(|note: &V| note["id"] == sense["id"]) {
            continue;
        }
        let mut note = sense.clone();
        note["group_name"] = group(db, &sense["group_id"])?["name"].clone();
        let recent = db.query(
            "SELECT learning FROM occurrences WHERE sense_id=?1 AND learning IS NOT NULL
            ORDER BY (surface=?2 AND context=?3) DESC, rowid DESC LIMIT 1",
            &[sense["id"].clone(), json!(surface), json!(context)],
        )?;
        note["learning"] = recent
            .first()
            .map(|row| row["learning"].clone())
            .unwrap_or(V::Null);
        notes.push(note);
    }
    let mut result = Vec::new();
    for sense in reused.iter().chain(exact.iter()) {
        if !result.iter().any(|g: &V| g["id"] == sense["group_id"]) {
            result.push(group(db, &sense["group_id"])?);
        }
        if result.len() == 5 {
            break;
        }
    }
    let retrieved = if result.len() < 5 {
        groups(db, &surface, Some(5))?
    } else {
        Vec::new()
    };
    for candidate in retrieved {
        if result.len() == 5 {
            break;
        }
        if !result.iter().any(|g| g["id"] == candidate["id"]) {
            result.push(candidate);
        }
    }
    for g in &mut result {
        g["vocabulary"] = json!(vocabulary(db, &g["id"])?);
    }
    Ok(
        json!({"surface":surface,"context":context,"exact":exact,"reused":reused,"groups":result,"saved_sense":saved_sense,"notes":notes}),
    )
}

fn upsert_sense(db: &Db, expression: &V, group_id: &V, meaning: &V) -> Result<V> {
    db.query("INSERT INTO senses VALUES(?1,?2,?3,?4) ON CONFLICT(group_id,expression_id,meaning) DO NOTHING",
        &[uuid(),expression.clone(),group_id.clone(),meaning.clone()])?;
    Ok(db.one(
        "SELECT id FROM senses WHERE expression_id=?1 AND group_id=?2 AND meaning=?3",
        &[expression.clone(), group_id.clone(), meaning.clone()],
    )?["id"]
        .clone())
}

fn annotate(db: &Db, p: &V) -> Result<V> {
    let (doc, start, end, surface, context) = selection(db, p)?;
    let conflict = db.query(
        "SELECT id FROM occurrences WHERE document_id=?1 AND start<?3 AND end>?2 LIMIT 1",
        &[doc.clone(), start.clone(), end.clone()],
    )?;
    if !conflict.is_empty() {
        return Err("选区与已有标注重叠，请在语义库中重新分类".into());
    }
    let sense = if !p["sense_id"].is_null() {
        id(p, "sense_id")?
    } else {
        let lemma = field(p, "lemma", 240, true)?;
        let meaning = field(p, "meaning", 1200, true)?;
        let group = if p["ungrouped_capture"] == true {
            let existing = db.query("SELECT id FROM semantic_groups WHERE name='未归类' AND sil_code IS NULL ORDER BY id LIMIT 1", &[])?;
            match existing.first() {
                Some(group) => group["id"].clone(),
                None => new_group(db,&json!({"name":"未归类"}))?,
            }
        } else if p["group_id"].is_null() {
            if !p["new_group"]["id"].is_null() {
                return Err("新组不能带已有 ID".into());
            }
            let name = field(&p["new_group"], "name", 120, true)?;
            match group_with_name(db, &name)? {
                Some(existing) => existing,
                None => new_group(db, &p["new_group"])?,
            }
        } else {
            id(p, "group_id")?
        };
        let normalized = json!(normalize(lemma));
        let forms = matching_inflections(db, lemma)?;
        let existing = db.query(
            &format!(
                "{SENSES} WHERE s.group_id=?1 AND s.meaning=?2
                AND e.normalized IN (SELECT value FROM json_each(?3))
                ORDER BY (e.normalized=?4) DESC, s.id LIMIT 2"
            ),
            &[
                group.clone(),
                json!(meaning),
                json!(V::Array(forms).to_string()),
                normalized.clone(),
            ],
        )?;
        let sense = match existing.as_slice() {
            [sense] => sense["id"].clone(),
            [sense, _]
                if normalize(sense["lemma"].as_str().unwrap_or_default())
                    == normalized.as_str().unwrap_or_default() =>
            {
                sense["id"].clone()
            }
            _ => {
                db.query(
                    "INSERT INTO expressions VALUES(?1,?2,?3) ON CONFLICT(normalized) DO NOTHING",
                    &[uuid(), json!(lemma), normalized.clone()],
                )?;
                let expression = db.one(
                    "SELECT id FROM expressions WHERE normalized=?1",
                    &[normalized],
                )?;
                upsert_sense(db, &expression["id"], &group, &json!(meaning))?
            }
        };
        reindex(db, &group)?;
        sense
    };
    let key = uuid();
    let learning = learning_value(&p["learning"])?;
    let context_meaning = field(p, "context_meaning", 1200, false)?;
    db.write(
        "INSERT INTO occurrences(id,document_id,sense_id,start,end,surface,context,learning,context_meaning) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
        &[
            key.clone(),
            doc,
            sense,
            start,
            end,
            json!(surface),
            json!(context),
            learning,
            json!((!context_meaning.is_empty()).then_some(context_meaning)),
        ],
    )?;
    Ok(key)
}

pub fn gloss_batch(db: &Db, p: &V) -> Result<V> {
    let cursor = field(p, "cursor", 40, false)?;
    let mut rows = db.query(
        "WITH work AS (
            SELECT 'a:'||s.id AS key,'sense' AS kind,s.id,e.lemma,s.meaning AS previous,
                s.meaning AS general,s.id AS sense_id,
                (SELECT context FROM occurrences WHERE sense_id=s.id ORDER BY id LIMIT 1) AS context
            FROM senses s JOIN expressions e ON e.id=s.expression_id
            UNION ALL
            SELECT 'b:'||o.id,'mark',o.id,e.lemma,o.context_meaning,s.meaning,s.id,o.context
            FROM occurrences o JOIN senses s ON s.id=o.sense_id JOIN expressions e ON e.id=s.expression_id
            WHERE o.context_meaning IS NULL OR o.context_meaning=s.meaning
         ) SELECT * FROM work WHERE key>?1 ORDER BY key LIMIT 13",
        &[json!(cursor)],
    )?;
    let next = if rows.len() > 12 {
        rows.pop();
        rows[11]["key"].clone()
    } else {
        V::Null
    };
    Ok(json!({"rows":rows,"next":next}))
}

fn apply_glosses(db: &Db, p: &V) -> Result<V> {
    let changes = p["changes"].as_array().filter(|rows| !rows.is_empty()).ok_or("请选择要写入的释义")?;
    let mut seen = std::collections::HashSet::new();
    let mut applied = 0;
    let mut conflicts = 0;
    let mut groups = std::collections::HashSet::new();
    // Write contextual glosses first so their original general sense can still be checked.
    for kind in ["mark", "sense"] {
        for row in changes.iter().filter(|row| row["kind"] == kind) {
            let key = id(row, "id")?;
            if !seen.insert((kind, key.clone())) {
                return Err("释义列表包含重复项目".into());
            }
            let meaning = json!(field(row, "meaning", 1200, true)?);
            let args = std::slice::from_ref(&key);
            if kind == "mark" {
                let old = db.one("SELECT o.context_meaning,o.sense_id,o.context,s.meaning FROM occurrences o
                    JOIN senses s ON s.id=o.sense_id WHERE o.id=?1", args)?;
                if old["context_meaning"] != row["previous"] || old["sense_id"] != row["sense_id"]
                    || old["meaning"] != row["general"] || old["context"] != row["context"] {
                    return Err("原文标注已变更，请重新生成预览".into());
                }
                db.write("UPDATE occurrences SET context_meaning=?1 WHERE id=?2", &[meaning,key])?;
            } else {
                let old = db.one("SELECT meaning,group_id,expression_id FROM senses WHERE id=?1", args)?;
                if old["meaning"] != row["previous"] {
                    return Err("通用义项已变更，请重新生成预览".into());
                }
                if !db.query("SELECT id FROM senses WHERE group_id=?1 AND expression_id=?2 AND meaning=?3 AND id<>?4",
                    &[old["group_id"].clone(),old["expression_id"].clone(),meaning.clone(),key.clone()])?.is_empty() {
                    conflicts += 1;
                    continue;
                }
                db.query("UPDATE occurrences SET context_meaning=?1 WHERE sense_id=?2 AND context_meaning IS NULL",
                    &[old["meaning"].clone(),key.clone()])?;
                db.write("UPDATE senses SET meaning=?1 WHERE id=?2", &[meaning,key])?;
                groups.insert(old["group_id"].clone());
            }
            applied += 1;
        }
    }
    if applied + conflicts != changes.len() {
        return Err("释义类型无效".into());
    }
    for group in groups {
        reindex(db, &group)?;
    }
    if applied > 0 {
        db.batch("DELETE FROM operation_history")?;
    }
    Ok(json!({"applied":applied,"conflicts":conflicts}))
}

fn merge_senses(db: &Db, p: &V) -> Result<V> {
    let source = id(p, "source")?;
    let target = id(p, "target")?;
    let meaning = field(p, "meaning", 1200, true)?;
    if source == target {
        return Err("请选择两个不同的义项".into());
    }
    let sql = "SELECT * FROM senses WHERE id=?1";
    let from = db.one(sql, std::slice::from_ref(&source))?;
    let to = db.one(sql, std::slice::from_ref(&target))?;
    if from["expression_id"] != to["expression_id"] {
        return Err("只能合并同一词形的义项".into());
    }
    for sense in [&from, &to] {
        db.query("UPDATE occurrences SET context_meaning=coalesce(context_meaning,?1) WHERE sense_id=?2",
            &[sense["meaning"].clone(),sense["id"].clone()])?;
    }
    db.query("UPDATE occurrences SET sense_id=?1 WHERE sense_id=?2", &[target.clone(), source.clone()])?;
    db.write("DELETE FROM senses WHERE id=?1", &[source])?;
    db.write("UPDATE senses SET meaning=?1 WHERE id=?2", &[json!(meaning),target])?;
    reindex(db, &from["group_id"])?;
    reindex(db, &to["group_id"])?;
    db.batch("DELETE FROM operation_history")?;
    Ok(to["group_id"].clone())
}

fn regroup(db: &Db, op: &str, p: &V) -> Result<V> {
    let source = id(p, "source")?;
    group(db, &source)?;
    let target = if op == "split" {
        new_group(db, &p["new_group"])?
    } else {
        id(p, "target")?
    };
    group(db, &target)?;
    if source == target && op != "reclassify" {
        return Err("请选择不同的目标组".into());
    }
    let mut undo = json!({"source":source,"target":target,"operation":op,
        "source_group":group(db,&source)?,"source_vocab":vocabulary(db,&source)?,
        "target_vocab":vocabulary(db,&target)?,"senses":[],"occurrences":[],"created_senses":[]});
    if op == "reclassify" {
        let old = id(p, "sense_id")?;
        let meaning = json!(field(p, "meaning", 1200, true)?);
        let row = db.one(
            "SELECT * FROM senses WHERE id=?1 AND group_id=?2",
            &[old.clone(), source.clone()],
        )?;
        let existed = db.query(
            "SELECT id FROM senses WHERE expression_id=?1 AND group_id=?2 AND meaning=?3",
            &[
                row["expression_id"].clone(),
                target.clone(),
                meaning.clone(),
            ],
        )?;
        let next = upsert_sense(db, &row["expression_id"], &target, &meaning)?;
        if next == old {
            return Err("请选择不同的义项或填写新的解释".into());
        }
        if existed.is_empty() {
            undo["created_senses"] = json!([next]);
        }
        for occurrence in ids(p, "occurrence_ids")? {
            db.write(
                "UPDATE occurrences SET sense_id=?1 WHERE id=?2 AND sense_id=?3",
                &[next.clone(), occurrence.clone(), old.clone()],
            )?;
            undo["occurrences"]
                .as_array_mut()
                .unwrap()
                .push(json!({"id":occurrence,"sense_id":old}));
        }
    } else {
        let selected = if op == "merge" {
            db.query("SELECT id FROM senses WHERE group_id=?1", &[source.clone()])?
                .into_iter()
                .map(|v| v["id"].clone())
                .collect()
        } else {
            ids(p, "sense_ids")?
        };
        for sense in selected {
            let row = db.one(
                "SELECT * FROM senses WHERE id=?1 AND group_id=?2",
                &[sense.clone(), source.clone()],
            )?;
            let duplicates = db.query(
                "SELECT id FROM senses WHERE group_id=?1 AND expression_id=?2 AND meaning=?3",
                &[
                    target.clone(),
                    row["expression_id"].clone(),
                    row["meaning"].clone(),
                ],
            )?;
            if let Some(duplicate) = duplicates.first() {
                let marks = db.query(
                    "SELECT id,sense_id FROM occurrences WHERE sense_id=?1",
                    &[sense.clone()],
                )?;
                undo["occurrences"].as_array_mut().unwrap().extend(marks);
                db.query(
                    "UPDATE occurrences SET sense_id=?1 WHERE sense_id=?2",
                    &[duplicate["id"].clone(), sense.clone()],
                )?;
                db.write("DELETE FROM senses WHERE id=?1", &[sense])?;
            } else {
                db.write(
                    "UPDATE senses SET group_id=?1 WHERE id=?2",
                    &[target.clone(), sense],
                )?;
            }
            undo["senses"].as_array_mut().unwrap().push(row);
        }
        if op == "merge" {
            db.query("INSERT OR IGNORE INTO group_vocabulary SELECT ?1,expression,nuance FROM group_vocabulary WHERE group_id=?2",
                &[target.clone(),source.clone()])?;
            undo["source_tags"] = json!(tags::group_tags(db, &source)?);
            undo["target_tags"] = json!(tags::group_tags(db, &target)?);
            db.query("INSERT OR IGNORE INTO group_tags SELECT ?1,tag_id FROM group_tags WHERE group_id=?2",
                &[target.clone(),source.clone()])?;
            db.write("DELETE FROM semantic_groups WHERE id=?1", &[source.clone()])?;
        }
    }
    if op == "split" && field(&p["new_group"], "vocabulary", 6000, false)?.is_empty() {
        db.query(
            "INSERT INTO group_vocabulary(group_id,expression,nuance)
             SELECT ?1,e.lemma,coalesce(
               (SELECT nullif(v.nuance,'') FROM group_vocabulary v
                WHERE v.group_id=?2 AND v.expression=e.lemma COLLATE NOCASE
                ORDER BY (v.expression=e.lemma) DESC,v.expression LIMIT 1),min(s.meaning))
             FROM senses s JOIN expressions e ON e.id=s.expression_id
             WHERE s.group_id=?1 GROUP BY e.id,e.lemma",
            &[target.clone(), source.clone()],
        )?;
    }
    reindex(db, &source)?;
    reindex(db, &target)?;
    db.write(
        "INSERT INTO operation_history(payload) VALUES(?1)",
        &[json!(undo.to_string())],
    )?;
    Ok(target)
}

fn undo(db: &Db) -> Result<V> {
    let entry = db.one(
        "SELECT * FROM operation_history ORDER BY id DESC LIMIT 1",
        &[],
    )?;
    let u: V =
        serde_json::from_str(entry["payload"].as_str().unwrap()).map_err(|e| e.to_string())?;
    let source = &u["source"];
    let target = &u["target"];
    if u["operation"] == "merge" {
        db.write(
            "INSERT INTO semantic_groups(id,name,description,sil_code) VALUES(?1,?2,?3,?4)",
            &[
                source.clone(),
                u["source_group"]["name"].clone(),
                u["source_group"]["description"].clone(),
                u["source_group"]["sil_code"].clone(),
            ],
        )?;
        if let Some(target_tags) = u["target_tags"].as_array() {
            db.query(
                "DELETE FROM group_tags WHERE group_id=?1",
                std::slice::from_ref(target),
            )?;
            for (id, values) in [
                (target, target_tags),
                (source, u["source_tags"].as_array().unwrap()),
            ] {
                for tag in values {
                    db.write(
                        "INSERT INTO group_tags VALUES(?1,?2)",
                        &[id.clone(), tag["id"].clone()],
                    )?;
                }
            }
        }
        db.query(
            "DELETE FROM group_vocabulary WHERE group_id=?1",
            &[target.clone()],
        )?;
        for (key, values) in [(source, &u["source_vocab"]), (target, &u["target_vocab"])] {
            for row in values.as_array().unwrap() {
                db.write(
                    "INSERT INTO group_vocabulary VALUES(?1,?2,?3)",
                    &[
                        key.clone(),
                        row["expression"].clone(),
                        row["nuance"].clone(),
                    ],
                )?;
            }
        }
    }
    for s in u["senses"].as_array().unwrap() {
        db.write("INSERT INTO senses VALUES(?1,?2,?3,?4) ON CONFLICT(id) DO UPDATE SET group_id=excluded.group_id",
            &[s["id"].clone(),s["expression_id"].clone(),source.clone(),s["meaning"].clone()])?;
    }
    for o in u["occurrences"].as_array().unwrap() {
        // A document may have been deleted after this relationship change.
        db.query(
            "UPDATE occurrences SET sense_id=?1 WHERE id=?2",
            &[o["sense_id"].clone(), o["id"].clone()],
        )?;
    }
    for s in u["created_senses"].as_array().unwrap() {
        db.write("DELETE FROM senses WHERE id=?1", &[s.clone()])?;
    }
    if u["operation"] == "split" {
        db.write("DELETE FROM semantic_groups WHERE id=?1", &[target.clone()])?;
    }
    reindex(db, source)?;
    reindex(db, target)?;
    db.write(
        "DELETE FROM operation_history WHERE id=?1",
        &[entry["id"].clone()],
    )?;
    Ok(source.clone())
}

fn learning_value(value: &V) -> Result<V> {
    if value.is_null() {
        return Ok(V::Null);
    }
    crate::ai::validate_learning(value)?;
    let serialized = value.to_string();
    if serialized.len() > 12_000 {
        return Err("学习提示过长".into());
    }
    Ok(json!(serialized))
}

const BACKUP_TABLES: &[(&str, &[&str])] = &[
    (
        "documents",
        &[
            "id",
            "title",
            "format",
            "content",
            "created_at",
            "source_url",
            "web_layout",
        ],
    ),
    ("epub_files", &["document_id", "data"]),
    ("pdf_files", &["document_id", "data"]),
    ("pdf_pages", &["document_id", "page", "offset", "text", "layer"]),
    ("semantic_groups", &["id", "name", "description", "sil_code"]),
    ("tags", &["id", "name"]),
    ("tag_names", &["normalized", "name", "tag_id"]),
    (
        "tag_scopes",
        &["tag_id", "scope", "source_label", "source_url"],
    ),
    ("group_tags", &["group_id", "tag_id"]),
    ("group_vocabulary", &["group_id", "expression", "nuance"]),
    ("expressions", &["id", "lemma", "normalized"]),
    ("senses", &["id", "expression_id", "group_id", "meaning"]),
    ("sense_sil", &["sense_id", "code"]),
    (
        "occurrences",
        &[
            "id",
            "document_id",
            "sense_id",
            "start",
            "end",
            "surface",
            "context",
            "learning",
            "context_meaning",
        ],
    ),
];

pub fn backup_data(db: &Db) -> Result<V> {
    let mut data = json!({"version":9});
    for &(table, _) in BACKUP_TABLES {
        data[table] = json!(db.query(&format!("SELECT * FROM {table}"), &[])?);
    }
    use base64::{engine::general_purpose::STANDARD, Engine};
    for file in data["pdf_files"].as_array_mut().unwrap() {
        if let Some(key) = file["storage"].as_str() {
            let original = std::fs::read(pdf_path(db,key)?).map_err(|e| e.to_string())?;
            file["data"] = json!(STANDARD.encode(original));
        }
        file.as_object_mut().unwrap().remove("storage");
    }
    Ok(data)
}

// The caller holds a write transaction so a validation or disk error rolls back the replacement.
pub fn replace_backup(db: &Db, data: &V) -> Result<V> {
    for &(table, _) in BACKUP_TABLES.iter().rev() {
        db.batch(&format!("DELETE FROM {table}"))?;
    }
    db.batch("DELETE FROM group_search; DELETE FROM operation_history;")?;
    restore(db, data)
}

fn restore(db: &Db, p: &V) -> Result<V> {
    if !matches!(p["version"].as_u64(), Some(1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 | 9)) {
        return Err("JSON 导出版本不兼容".into());
    }
    let defaults = db.query("SELECT pristine FROM tag_defaults WHERE id=1", &[])?;
    if defaults.first().is_some_and(|row| row["pristine"] == 1) {
        if db.one("SELECT EXISTS(SELECT 1 FROM documents) OR EXISTS(SELECT 1 FROM semantic_groups) AS occupied",&[])?["occupied"] == 0 {
            db.query("DELETE FROM tags",&[])?;
        }
    }
    for &(table, columns) in BACKUP_TABLES {
        if db.one(&format!("SELECT count(*) AS count FROM {table}"), &[])?["count"] != 0 {
            return Err("请在空词汇库中恢复 JSON，以避免覆盖现有数据".into());
        }
        if p["version"].as_u64().unwrap_or(0) < 5
            && matches!(table, "tags" | "tag_names" | "group_tags")
        {
            continue;
        }
        if matches!(table, "pdf_files" | "pdf_pages") && p["version"].as_u64().unwrap_or(0) < 9 {
            continue;
        }
        if table == "tag_scopes" && p["version"].as_u64().unwrap_or(0) < 6 {
            continue;
        }
        if matches!(table,"epub_files"|"sense_sil") && p[table].is_null() { continue; }
        let rows = p[table]
            .as_array()
            .ok_or_else(|| format!("导出缺少 {table}"))?;
        let placeholders = vec!["?"; columns.len()].join(",");
        let sql = format!(
            "INSERT INTO {table}({}) VALUES({placeholders})",
            columns.join(",")
        );
        for row in rows {
            let values = columns
                .iter()
                .map(|&col| {
                    if matches!(col, "start" | "end" | "page" | "offset") {
                        return row[col]
                            .as_i64()
                            .map(|n| json!(n))
                            .ok_or_else(|| format!("{col} 无效"));
                    }
                    if col == "id" || col.ends_with("_id") {
                        return id(row, col);
                    }
                    if col == "context_meaning" {
                        if p["version"].as_u64().unwrap_or(0) < 8 && row[col].is_null() {
                            let sense = db.one("SELECT meaning FROM senses WHERE id=?1", &[id(row,"sense_id")?])?;
                            return Ok(sense["meaning"].clone());
                        }
                        return match &row[col] {
                            V::Null => Ok(V::Null),
                            V::String(_) => Ok(json!(field(row,col,1200,true)?)),
                            _ => Err("本句释义格式无效".into()),
                        };
                    }
                    if col == "sil_code" { return checked_sil_code(&row[col]); }
                    if col == "learning" {
                        return match &row[col] {
                            V::Null => Ok(V::Null),
                            V::String(value) => learning_value(
                                &serde_json::from_str(value).map_err(|_| "学习提示格式无效")?,
                            ),
                            _ => Err("学习提示格式无效".into()),
                        };
                    }
                    if col == "web_layout" {
                        return match &row[col] {
                            V::Null => Ok(V::Null),
                            V::String(value) => {
                                crate::web_layout::validate(value)?;
                                Ok(json!(value))
                            }
                            _ => Err("网页版式数据无效".into()),
                        };
                    }
                    if col == "source_url" {
                        return match &row[col] {
                            V::Null => Ok(V::Null),
                            V::String(value) => {
                                crate::web_import::parse_url(value).map(|url| json!(url.as_str()))
                            }
                            _ => Err("导出中的网页地址无效".into()),
                        };
                    }
                    if table == "epub_files" && col == "data" {
                        return row[col].as_str().filter(|text| text.len() <= 40_000_000)
                            .filter(|text| STANDARD.decode(text).is_ok())
                            .map(|text| json!(text)).ok_or_else(|| "EPUB 备份文件无效".into());
                    }
                    if matches!(table, "pdf_files" | "pdf_pages") {
                        let limit = match col { "data" => 133_333_336, "layer" => 12_000_000, _ => 2_000_000 };
                        return row[col].as_str().filter(|value| value.len() <= limit && !value.contains('\0'))
                            .map(|value| json!(value)).ok_or_else(|| format!("PDF 备份字段 {col} 无效"));
                    }
                    if col == "content" {
                        let text = row[col]
                            .as_str()
                            .filter(|s| {
                                s.len() <= crate::epub::TEXT_LIMIT && !s.contains('\0') && (!s.trim().is_empty()
                                    || p["pdf_files"].as_array().is_some_and(|rows|
                                        rows.iter().any(|pdf| pdf["document_id"] == row["id"])))
                            })
                            .ok_or("导出中的文档内容无效")?;
                        return Ok(json!(text));
                    }
                    let text = row[col]
                        .as_str()
                        .filter(|s| s.len() <= 24_000 && !s.contains('\0'))
                        .ok_or_else(|| format!("{col} 无效"))?;
                    Ok(json!(text))
                })
                .collect::<Result<Vec<_>>>()?;
            if table == "occurrences" {
                let (doc, start, end, surface, context) = selection(db, row)?;
                if row["surface"] != surface || row["context"] != context ||
                    !db.query("SELECT id FROM occurrences WHERE document_id=?1 AND start<?3 AND end>?2 LIMIT 1",
                        &[doc,start,end])?.is_empty() {
                    return Err("导出的标注与原文不一致或相互重叠".into());
                }
            }
            if matches!(table, "tags" | "tag_names") {
                let name = tags::name(&row["name"])?;
                if table == "tag_names" && row["normalized"] != tags::normalize(&name) {
                    return Err("备份的标签别名无效".into());
                }
            }
            db.write(&sql, &values)?;
        }
    }
    for tag in db.query("SELECT id,name FROM tags", &[])? {
        let normalized = tags::normalize(tag["name"].as_str().unwrap());
        db.one(
            "SELECT 1 AS valid FROM tag_names WHERE normalized=?1 AND tag_id=?2",
            &[json!(normalized), tag["id"].clone()],
        )
        .map_err(|_| "备份缺少标签的正式名称")?;
    }
    for row in db.query("SELECT id FROM semantic_groups", &[])? {
        reindex(db, &row["id"])?;
    }
    if !defaults.is_empty() && p["version"].as_u64().unwrap_or(0) < 7 {
        tags::install_unesco(db)?;
    }
    tags::mark_defaults_modified(db)?;
    Ok(V::Null)
}

fn delete_sense(db: &Db, key: &V) -> Result<()> {
    let args = std::slice::from_ref(key);
    let sense = db.one(&format!("{SENSES} WHERE s.id=?1"), args)?;
    let group = &sense["group_id"];
    db.query("DELETE FROM occurrences WHERE sense_id=?1", args)?;
    db.write("DELETE FROM senses WHERE id=?1", args)?;
    // The vocabulary list is separate. Remove this lemma only when the group
    // no longer has a sense that uses it.
    db.query("DELETE FROM group_vocabulary WHERE group_id=?1 AND expression=?2 COLLATE NOCASE
        AND NOT EXISTS(SELECT 1 FROM senses WHERE group_id=?1 AND expression_id=?3)",
        &[group.clone(), sense["lemma"].clone(), sense["expression_id"].clone()])?;
    reindex(db, group)?;
    db.batch("DELETE FROM operation_history")?;
    Ok(())
}

pub fn run(db: &Db, op: &str, p: &V) -> Result<V> {
    match op {
        "list" => {
            let q = field(p, "q", 200, false)?;
            let docs = db.query("SELECT id,title,CASE WHEN EXISTS(SELECT 1 FROM pdf_files WHERE document_id=documents.id) THEN 'pdf' WHEN EXISTS(SELECT 1 FROM epub_files WHERE document_id=documents.id) THEN 'epub' ELSE format END AS format,source_url FROM documents WHERE instr(lower(title),lower(?1))>0 OR instr(lower(coalesce(source_url,'')),lower(?1))>0 ORDER BY created_at DESC, rowid DESC",&[json!(q)])?;
            let undo = db.one(
                "SELECT EXISTS(SELECT 1 FROM operation_history) AS available",
                &[],
            )?;
            let counts = db.query("WITH paths(sense_id,code) AS (
                SELECT sense_id,code FROM sense_sil_memberships UNION
                SELECT id,'unclassified' FROM senses s WHERE NOT EXISTS(SELECT 1 FROM sense_sil_memberships m WHERE m.sense_id=s.id)
                ) SELECT code,count(*) AS count,group_concat(sense_id) AS ids FROM paths GROUP BY code", &[])?;
            Ok(json!({"sil_counts":counts,"documents":docs,"groups":groups(db,q,None)?,"undo":undo["available"],"tags":tags::catalog(db)?,"tag_links":db.query("SELECT * FROM group_tags",&[])?}))
        }
        "document" => {
            let key = id(p, "id")?;
            let doc = db.one("SELECT documents.*,EXISTS(SELECT 1 FROM pdf_files WHERE document_id=id) AS pdf,EXISTS(SELECT 1 FROM epub_files WHERE document_id=id) AS epub FROM documents WHERE id=?1", &[key.clone()])?;
            let marks = db.query(
                &format!("{MARKS} WHERE o.document_id=?1 ORDER BY o.start"),
                &[key],
            )?;
            Ok(json!({"document":doc,"marks":marks}))
        }
        "web_cover" => {
            use crate::web_layout::Node;
            fn first_image<'a>(nodes: &[Node], images: &'a [Option<String>]) -> Option<&'a String> {
                nodes.iter().find_map(|node| match node {
                    Node::Image { index, .. } => images.get(*index).and_then(Option::as_ref),
                    Node::Element { children, .. } => first_image(children, images),
                    Node::Text { .. } => None,
                })
            }
            let row = db.one(
                "SELECT source_url,web_layout FROM documents WHERE id=?1",
                &[id(p, "id")?],
            )?;
            if !row["source_url"].is_string() || !row["web_layout"].is_string() {
                return Ok(V::Null);
            }
            let layout = saved_image_layout(&row)?;
            Ok(json!(first_image(&layout.nodes, &layout.images)))
        }
        "pdf_load" => {
            let key = id(p, "id")?;
            let args = std::slice::from_ref(&key);
            let file = db.one("SELECT storage FROM pdf_files WHERE document_id=?1", args)?;
            let storage = file["storage"].as_str().ok_or("PDF 尚未准备就绪")?;
            let length = std::fs::metadata(pdf_path(db,storage)?).map_err(|e| e.to_string())?.len();
            let pages = db.query("SELECT page,offset,text FROM pdf_pages WHERE document_id=?1 ORDER BY page", args)?;
            Ok(json!({"length":length,"pages":pages}))
        }
        "pdf_native_boxes" => {
            let key = id(p, "id")?;
            let page = p["page"].as_u64().filter(|n| (1..=100_000).contains(n)).ok_or("PDF 页码无效")?;
            let file = db.one("SELECT storage FROM pdf_files WHERE document_id=?1", &[key])?;
            let path = pdf_path(db,file["storage"].as_str().ok_or("PDF 尚未准备就绪")?)?;
            crate::pdf_geometry::word_boxes(&path,page as usize)
        }
        "pdf_page_layer" => {
            let key = id(p, "id")?;
            let page = p["page"].as_u64().filter(|n| (1..=100_000).contains(n)).ok_or("PDF 页码无效")?;
            Ok(db.one("SELECT layer FROM pdf_pages WHERE document_id=?1 AND page=?2",
                &[key,json!(page)])?["layer"].clone())
        }
        "pdf_index_page" => db.transaction(|| save_pdf_page(db, p)),
        "sil_related_senses" => {
            let key = id(p, "id")?;
            let mut categories = db.query(
                "SELECT code FROM sense_sil_memberships WHERE sense_id=?1 ORDER BY code",
                &[key.clone()],
            )?;
            for category in &mut categories {
                category["senses"] = json!(db.query(
                    "SELECT s.id,e.lemma,s.meaning FROM sense_sil_memberships m
                     JOIN senses s ON s.id=m.sense_id JOIN expressions e ON e.id=s.expression_id
                     WHERE m.code=?1 AND s.id<>?2 ORDER BY e.lemma COLLATE NOCASE,s.id",
                    &[category["code"].clone(),key.clone()],
                )?);
            }
            Ok(json!(categories))
        }
        "sil_senses" => {
            let code = field(p,"code",32,false)?;
            let query = field(p,"q",200,false)?;
            let mut senses = db.query(
                "SELECT s.id,s.group_id,e.lemma,s.meaning,o.context FROM senses s
                 JOIN expressions e ON e.id=s.expression_id
                 LEFT JOIN occurrences o ON o.rowid=(SELECT min(m.rowid) FROM occurrences m WHERE m.sense_id=s.id)
                 WHERE (?1='' OR EXISTS(SELECT 1 FROM sense_sil_memberships m WHERE m.sense_id=s.id AND
                    (m.code=?1 OR (?2=1 AND m.code LIKE ?1 || '.%')))
                 OR (?1='unclassified' AND NOT EXISTS(SELECT 1 FROM sense_sil_memberships m WHERE m.sense_id=s.id)))
                 AND (?3='' OR instr(lower(e.lemma),lower(?3))>0 OR instr(s.meaning,?3)>0)
                 ORDER BY e.lemma COLLATE NOCASE,s.id",
                &[json!(code),json!(i32::from(p["descendants"]==true)),json!(query)],
            )?;
            for sense in &mut senses {
                sense["links"] = json!(db.query(
                    &format!("{MARKS} WHERE s.id=?1 ORDER BY d.title,o.start"),
                    &[sense["id"].clone()],
                )?);
                let codes = db.query("SELECT code FROM sense_sil_memberships WHERE sense_id=?1 ORDER BY code", &[sense["id"].clone()])?;
                sense["categories"] = json!(codes.into_iter().map(|row|row["code"].clone()).collect::<Vec<_>>());
                if let Some(record) = db.query("SELECT payload FROM sil_diagnostics WHERE sense_id=?1", &[sense["id"].clone()])?.first() {
                    sense["sil_diagnostic"] = serde_json::from_str(record["payload"].as_str().unwrap_or_default()).unwrap_or(V::Null);
                }
            }
            Ok(json!(senses))
        }
        "group_senses" => {
            let key = id(p, "id")?;
            Ok(json!(group_senses(db, &key)?))
        },
        "group" => {
            let key = id(p, "id")?;
            let g = group(db, &key)?;
            let senses = group_senses(db, &key)?;
            let vocab = vocabulary(db, &key)?;
            let group_tags = tags::group_tags(db,&key)?;
            let marks = db.query(
                &format!("{MARKS} WHERE s.group_id=?1 ORDER BY d.title,o.start"),
                &[key],
            )?;
            Ok(json!({"group":g,"senses":senses,"vocabulary":vocab,"marks":marks,"tags":group_tags}))
        }
        "candidates" => candidates(db, p),
        "tag_install_starter" => db.transaction(|| {
            let result = tags::install_starter(db)?;
            if result["added"] != 0 { tags::mark_defaults_modified(db)?; }
            Ok(result)
        }),
        "tag_scope_save" => db.transaction(|| {
            tags::mark_defaults_modified(db)?;
            let existing = if p["id"].is_null(){None}else{Some(id(p,"id")?)};
            tags::save_scope(db,existing,&p["name"],field(p,"scope",500,false)?)?;
            Ok(V::Null)
        }),
        "tag_rename" => db.transaction(|| {
            tags::mark_defaults_modified(db)?;
            tags::rename(db,&id(p,"id")?,&p["name"])?;
            Ok(V::Null)
        }),
        "tag_merge" => db.transaction(|| {
            tags::mark_defaults_modified(db)?;
            tags::merge(db,&id(p,"source")?,&id(p,"target")?)?;
            db.batch("DELETE FROM operation_history")?;
            Ok(V::Null)
        }),
        "group_delete" | "groups_delete" => db.transaction(|| {
            let keys = if op=="group_delete" {vec![id(p,"id")?]} else {ids(p,"ids")?};
            let mut seen = std::collections::HashSet::new();
            for key in keys.iter().filter(|key| seen.insert(key.as_str().unwrap())) {
            let args = std::slice::from_ref(key);
            db.query("DELETE FROM occurrences WHERE sense_id IN (SELECT id FROM senses WHERE group_id=?1)", args)?;
            db.query("DELETE FROM senses WHERE group_id=?1", args)?;
            db.write("DELETE FROM semantic_groups WHERE id=?1", args)?;
            reindex(db, key)?;
            }
            // Older relationship changes may reference removed groups, senses or occurrences.
            db.batch("DELETE FROM operation_history")?;
            Ok(V::Null)
        }),
        "epub_original" => {
            Ok(db.one("SELECT data FROM epub_files WHERE document_id=?1", &[id(p, "id")?])?["data"].clone())
        }
        "document_rename" => {
            let key = id(p, "id")?;
            let title = field(p, "title", 255, true)?;
            if title.chars().any(char::is_control) { return Err("标题只能为单行文字".into()); }
            db.write("UPDATE documents SET title=?1 WHERE id=?2", &[json!(title), key])?;
            Ok(json!(title))
        }
        "document_edit" => db.transaction(|| edit_document(db, p)),
        "sense_merge_options" => Ok(json!(db.query(
            "SELECT s.id,s.meaning,g.name AS group_name,
                (SELECT count(*) FROM occurrences WHERE sense_id=s.id) AS links
             FROM senses s JOIN semantic_groups g ON g.id=s.group_id
             WHERE s.expression_id=(SELECT expression_id FROM senses WHERE id=?1) AND s.id<>?1
             ORDER BY g.name,s.meaning,s.id",
            &[id(p,"id")?],
        )?)),
        "gloss_apply" => db.transaction(|| apply_glosses(db, p)),
        "sense_merge" => db.transaction(|| merge_senses(db, p)),
        "mark_delete" => db.transaction(|| {
            let mark = id(p, "id")?;
            let document = id(p, "document_id")?;
            let args = &[mark, document];
            let occurrence = db.one("SELECT sense_id FROM occurrences WHERE id=?1 AND document_id=?2", args)?;
            db.write("DELETE FROM occurrences WHERE id=?1 AND document_id=?2", args)?;
            let sense = &occurrence["sense_id"];
            let last = db.query("SELECT 1 FROM occurrences WHERE sense_id=?1 LIMIT 1", std::slice::from_ref(sense))?.is_empty();
            if last {
                delete_sense(db, sense)?;
            }
            Ok(json!(last))
        }),
        "sense_delete" => db.transaction(|| {
            delete_sense(db, &id(p, "id")?)?;
            Ok(V::Null)
        }),
        "document_delete" => db.transaction(|| {
            let key = id(p, "id")?;
            db.query(
                "DELETE FROM occurrences WHERE document_id=?1",
                std::slice::from_ref(&key),
            )?;
            db.write("DELETE FROM documents WHERE id=?1", &[key])?;
            Ok(V::Null)
        }),
        "backup" => db.transaction(|| backup_data(db)),
        "learning_save" => {
            let mark = id(p, "id")?;
            let learning = learning_value(&p["learning"])?;
            db.write(
                "UPDATE occurrences SET learning=?1 WHERE id=?2",
                &[learning, mark],
            )?;
            Ok(V::Null)
        }
        "restore" => db.transaction(|| restore(db, p)),
        "group_save" | "annotate" => db.transaction(|| {
            let value = if op == "group_save" {
                put_group(db, p)?
            } else {
                annotate(db, p)?
            };
            // Undo is valid only while subsequent changes are also undoable.
            db.batch("DELETE FROM operation_history")?;
            Ok(value)
        }),
        "sil_audit_apply" => db.transaction(|| {
            let rows = p["rows"].as_array().ok_or("无效审查结果")?;
            let code = field(p,"code",32,true)?;
            if !crate::sil::valid(&code) { return Err("分类编号无效".into()); }
            for row in rows {
                let key = id(row,"id")?;
                let sense = db.one("SELECT meaning FROM senses WHERE id=?1", &[key.clone()])?;
                let existing = db.query("SELECT code FROM sense_sil_memberships WHERE sense_id=?1 ORDER BY code", &[key.clone()])?
                    .into_iter().map(|entry|entry["code"].clone()).collect::<Vec<_>>();
                if sense["meaning"] != row["meaning"] || row["categories"].as_array() != Some(&existing) {
                    return Err("义项或分类已变化，请重新审查".into());
                }
                db.query("INSERT OR IGNORE INTO sense_sil SELECT sense_id,code FROM sense_sil_memberships WHERE sense_id=?1", &[key.clone()])?;
                db.query("INSERT OR IGNORE INTO sense_sil VALUES(?1,'')", &[key.clone()])?;
                for (action,sql) in [("add","INSERT OR IGNORE INTO sense_sil VALUES(?1,?2)"),
                    ("remove","DELETE FROM sense_sil WHERE sense_id=?1 AND code=?2")] {
                    for item in row[action].as_array().ok_or("无效分类变更")? {
                        let target = item.as_str().filter(|target|crate::sil::valid(target)).ok_or("分类编号无效")?;
                        if action=="remove" && target!=code && !(p["descendants"]==true &&
                            target.strip_prefix(&code).is_some_and(|suffix|suffix.starts_with('.'))) {
                            return Err("不能移除审查范围外的归属".into());
                        }
                        db.query(sql,&[key.clone(),item.clone()])?;
                    }
                }
            }
            Ok(json!(rows.len()))
        }),
        "sil_memberships" => db.transaction(|| {
            let rows = p["rows"].as_array().filter(|rows|rows.len()<=10000).ok_or("义项清单无效")?;
            for row in rows {
                let key = id(row,"id")?;
                let sense = db.one("SELECT meaning FROM senses WHERE id=?1", &[key.clone()])?;
                if sense["meaning"]!=row["meaning"] { return Err("义项已修改，请刷新".into()); }
                let codes = row["codes"].as_array().ok_or("分类清单无效")?;
                db.query("INSERT OR IGNORE INTO sense_sil SELECT sense_id,code FROM sense_sil_memberships WHERE sense_id=?1", &[key.clone()])?;
                db.query("INSERT OR IGNORE INTO sense_sil VALUES(?1,'')", &[key.clone()])?;
                for code in codes {
                    if !code.as_str().is_some_and(crate::sil::valid) { return Err("SIL 分类编号无效".into()); }
                    let sql = if p["remove"]==true { "DELETE FROM sense_sil WHERE sense_id=?1 AND code=?2" }
                        else { "INSERT OR IGNORE INTO sense_sil VALUES(?1,?2)" };
                    db.query(sql,&[key.clone(),code.clone()])?;
                }
                if row["classification_trace"].is_object() {
                    let stored = db.query("SELECT code FROM sense_sil_memberships WHERE sense_id=?1 ORDER BY code", &[key.clone()])?;
                    let record = json!({"model_output":row["classification_trace"],"lemma":row["lemma"],
                        "meaning":row["meaning"],"selected_codes":codes,"stored_categories":stored});
                    db.query("INSERT OR REPLACE INTO sil_diagnostics VALUES(?1,?2)", &[key,json!(record.to_string())])?;
                }
            }
            Ok(json!(rows.len()))
        }),
        "sil_reset" => db.transaction(|| {
            let count = db.one("SELECT count(*) AS n FROM semantic_groups WHERE sil_code IS NOT NULL", &[])?["n"].clone();
            db.query("UPDATE semantic_groups SET sil_code=NULL WHERE sil_code IS NOT NULL", &[])?;
            db.query("DELETE FROM sense_sil", &[])?;
            Ok(count)
        }),
        "sil_apply" => db.transaction(|| apply_sil(db,p,false)),
        "sil_move_sense" => db.transaction(|| {
            let row = &p["sense"];
            apply_sil(db,&json!({"parent":row["current_code"].as_str().unwrap_or_default(),"rows":[row]}),true)
        }),
        "merge" | "split" | "move" | "reclassify" => db.transaction(|| regroup(db, op, p)),
        "undo" => db.transaction(|| undo(db)),
        _ => Err("未知操作".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn memory() -> Db {
        let db = Db::open(Path::new(":memory:")).unwrap();
        init_schema(&db, false).unwrap();
        db
    }

    fn create(db: &Db, name: &str) -> V {
        run(
            db,
            "group_save",
            &json!({"name":name,"vocabulary":"plummet | sharp decrease"}),
        )
        .unwrap()
    }

    fn anchor(content: &str, document_id: &V, at: usize) -> V {
        let start = content[..at].encode_utf16().count();
        json!({"document_id":document_id,"start":start,"end":start+7})
    }

    fn annotate_at(db: &Db, mut data: V, group: &V, meaning: &str) -> V {
        data["group_id"] = group.clone();
        data["lemma"] = json!("plunge");
        data["meaning"] = json!(meaning);
        run(db, "annotate", &data).unwrap()
    }

    #[test]
    fn inflected_selection_finds_and_reuses_an_existing_sense() {
        let db = memory();
        let group = create(&db, "Attack");
        let saved_doc = import(&db, "First.txt", "txt", "They assailed us.").unwrap();
        run(&db, "annotate", &json!({
            "document_id":saved_doc,"start":5,"end":13,"group_id":group,
            "lemma":"assailed","meaning":"猛烈攻击"
        })).unwrap();

        let doc = import(&db, "Second.txt", "txt", "They assail the plan.").unwrap();
        let mut selection = json!({"document_id":doc,"start":5,"end":11});
        assert_eq!(candidates(&db, &selection).unwrap()["notes"][0]["lemma"], "assailed");
        selection["group_id"] = group.clone();
        selection["lemma"] = json!("assail");
        selection["meaning"] = json!("猛烈攻击");
        selection["context_meaning"] = json!("猛烈批评这项计划");
        run(&db, "annotate", &selection).unwrap();
        assert_eq!(run(&db, "group_senses", &json!({"id":group})).unwrap().as_array().unwrap().len(), 1);

        let doc = import(&db, "Third.txt", "txt", "Doubts assailing her.").unwrap();
        selection["document_id"] = doc;
        selection["start"] = json!(7);
        selection["end"] = json!(16);
        selection["meaning"] = json!("困扰");
        run(&db, "annotate", &selection).unwrap();
        assert_eq!(run(&db, "group_senses", &json!({"id":group})).unwrap().as_array().unwrap().len(), 2);
    }

    #[test]
    fn global_gloss_cleanup_updates_single_senses_and_keeps_explicit_glosses() {
        let db = memory();
        let text = "Prices plunged. They plunged.";
        let doc = import(&db, "Article.txt", "txt", text).unwrap();
        let group = create(&db, "Decline");
        let first = annotate_at(&db, anchor(text, &doc, 7), &group, "价格暴跌");
        let mut second_anchor = anchor(text, &doc, 21);
        second_anchor["context_meaning"] = json!("他们急速下落");
        let second = annotate_at(&db, second_anchor, &group, "价格暴跌");
        let batch = gloss_batch(&db, &V::Null).unwrap();
        let rows = batch["rows"].as_array().unwrap();
        assert_eq!(rows.len(), 2);
        let mut sense = rows[0].clone();
        let mut mark = rows[1].clone();
        assert_eq!(sense["kind"], "sense");
        assert_eq!(mark["id"], first);
        sense["meaning"] = json!("突然急剧下降");
        mark["meaning"] = json!("价格大幅下跌");
        let mut stale = sense.clone();
        stale["previous"] = json!("失效");
        assert!(run(&db, "gloss_apply", &json!({"changes":[mark,stale]})).is_err());
        let before = run(&db, "document", &json!({"id":doc})).unwrap();
        assert!(before["marks"][0]["context_meaning"].is_null());
        assert_eq!(run(&db, "gloss_apply", &json!({"changes":[sense,mark]})).unwrap()["applied"], 2);
        let after = run(&db, "document", &json!({"id":doc})).unwrap();
        assert_eq!(after["marks"][0]["context_meaning"], "价格大幅下跌");
        assert_eq!(after["marks"][1]["context_meaning"], "他们急速下落");
        assert_eq!(after["marks"][1]["id"], second);
        assert_eq!(after["marks"][0]["meaning"], "突然急剧下降");
        assert_eq!(after["marks"][0]["sense_id"], after["marks"][1]["sense_id"]);
    }

    #[test]
    fn merging_same_word_senses_keeps_occurrence_glosses_and_notes() {
        let db = memory();
        let text = "Prices plunged. They plunged.";
        let doc = import(&db, "Article.txt", "txt", text).unwrap();
        let first_group = create(&db, "Financial change");
        let second_group = create(&db, "Rapid movement");
        let first = annotate_at(&db, anchor(text, &doc, 7), &first_group, "价格暴跌");
        let mut second_anchor = anchor(text, &doc, 21);
        second_anchor["context_meaning"] = json!("他们猛地向下跳");
        second_anchor["learning"] = json!({"usage_note":"旧笔记","example":"They plunged.","related":[]});
        let second = annotate_at(&db, second_anchor, &second_group, "突然下降");
        let source = group_senses(&db, &first_group).unwrap()[0]["id"].clone();
        let target = group_senses(&db, &second_group).unwrap()[0]["id"].clone();
        let options = run(&db, "sense_merge_options", &json!({"id":source})).unwrap();
        assert_eq!(options[0]["id"], target);
        assert_eq!(options[0]["links"], 1);
        assert_eq!(run(&db, "sense_merge", &json!({
            "source":source,"target":target,"meaning":"迅速向下移动"
        })).unwrap(), second_group);
        let result = run(&db, "document", &json!({"id":doc})).unwrap();
        let marks = result["marks"].as_array().unwrap();
        assert_eq!(marks[0]["id"], first);
        assert_eq!(marks[1]["id"], second);
        assert_eq!(marks[0]["context_meaning"], "价格暴跌");
        assert_eq!(marks[1]["context_meaning"], "他们猛地向下跳");
        assert!(marks[1]["learning"].as_str().unwrap().contains("旧笔记"));
        assert!(marks.iter().all(|mark| mark["meaning"] == "迅速向下移动" && mark["sense_id"] == target));
        assert!(group_senses(&db, &first_group).unwrap().is_empty());
        let data = run(&db, "backup", &V::Null).unwrap();
        let restored = memory();
        run(&restored, "restore", &data).unwrap();
        assert_eq!(run(&restored, "document", &json!({"id":doc})).unwrap(), result);
    }

    #[test]
    fn pdf_backup_keeps_source_page_offsets_and_annotations() {
        let directory = std::env::temp_dir().join(format!("semantic-pdf-{}",Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        let db = Db::open(&directory.join("library.sqlite3")).unwrap();
        init_schema(&db,false).unwrap();
        let bytes = b"%PDF-1.7";
        let doc = import_pdf(&db, "Source.pdf", &bytes[..]).unwrap();
        let group = create(&db, "Movement");
        let index = |page, text| run(&db, "pdf_index_page",
            &json!({"id":doc,"page":page,"text":text,"layer":{"words":[],"offsets":[]}}));
        assert!(index(2, "They plunged.").is_err());
        index(1, "📚 A heading.").unwrap();
        let page = index(2, "They plunged.").unwrap();
        assert_eq!(page["offset"], 15);
        assert_eq!(index(2, "They plunged.").unwrap(), page);
        let content = "📚 A heading.\n\nThey plunged.\n\n";
        let mark = annotate_at(&db, anchor(content, &doc, content.find("plunged").unwrap()), &group, "下坠");
        let saved = run(&db, "pdf_load", &json!({"id":doc})).unwrap();
        let backup = run(&db, "backup", &V::Null).unwrap();
        let restored_dir = directory.join("restored");
        std::fs::create_dir_all(&restored_dir).unwrap();
        let restored = Db::open(&restored_dir.join("library.sqlite3")).unwrap();
        init_schema(&restored,false).unwrap();
        run(&restored, "restore", &backup).unwrap();
        assert_eq!(pdf_prepare(&restored, &json!({"id":doc})).unwrap(), saved);
        assert_eq!(pdf_range(&restored,&json!({"id":doc}),0,bytes.len() as u64).unwrap(), bytes);
        let restored_doc = run(&restored, "document", &json!({"id":doc})).unwrap();
        assert_eq!(restored_doc["document"]["pdf"], 1);
        assert_eq!(restored_doc["marks"][0]["id"], mark);
        run(&restored, "document_delete", &json!({"id":doc})).unwrap();
        assert!(run(&restored, "pdf_load", &json!({"id":doc})).is_err());
        use std::io::Read;
        let large_pdf = (&bytes[..]).chain(std::io::repeat(0).take(26_000_000));
        let large = import_pdf(&db,"Large.pdf",large_pdf).unwrap();
        assert_eq!(pdf_range(&db,&json!({"id":large}),26_000_000,26_000_004).unwrap(), vec![0;4]);
        for page in 1..=2 {
            run(&db,"pdf_index_page",&json!({"id":large,"page":page,"text":"x".repeat(1_100_000),
                "layer":{"words":[],"offsets":[]}})).unwrap();
        }
        let last = run(&db,"pdf_index_page",&json!({"id":large,"page":3,"text":"plunged",
            "layer":{"words":[],"offsets":[]}})).unwrap();
        let start = last["offset"].as_u64().unwrap();
        assert!(start > 2_000_000);
        assert_eq!(selection(&db,&json!({"document_id":large,"start":start,"end":start+7})).unwrap().3,"plunged");
        drop(restored);
        drop(db);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn local_snapshots_replace_library_with_links_and_keep_a_recovery_point() {
        use crate::snapshots;
        let db = memory();
        let directory = std::env::temp_dir().join(format!("semantic-reader-snapshots-{}", Uuid::new_v4()));
        let path = directory.join("library.sqlite3");
        let text = "They plunged.";
        let doc = import(&db, "First.txt", "txt", text).unwrap();
        let group = create(&db, "Movement");
        let mark = annotate_at(&db, anchor(text, &doc, 5), &group, "跳入");
        let saved = snapshots::run(&db, &path, "snapshot_create", &V::Null).unwrap();
        let id = &saved[0]["id"];
        let preview = snapshots::run(&db, &path, "snapshot_preview", &json!({"id":id})).unwrap();
        assert_eq!(preview["documents"][0]["content"], text);
        assert_eq!(preview["occurrences"][0]["id"], mark);
        assert!(preview["documents"][0].get("web_layout").is_none());
        run(&db, "document_delete", &json!({"id":doc})).unwrap();
        let replacement = import(&db, "Second.txt", "txt", "Later document").unwrap();

        snapshots::run(&db, &path, "snapshot_restore", &json!({"id":id})).unwrap();
        let restored = run(&db, "document", &json!({"id":doc})).unwrap();
        assert_eq!(restored["document"]["content"], text);
        assert_eq!(restored["marks"][0]["id"], mark);
        assert_eq!(run(&db, "group", &json!({"id":group})).unwrap()["marks"][0]["document_id"], doc);
        assert!(run(&db, "document", &json!({"id":replacement})).is_err());

        let entries = snapshots::run(&db, &path, "snapshot_list", &V::Null).unwrap();
        let safety = entries.as_array().unwrap().iter().find(|item| item["safety"] == true).unwrap();
        let invalid = "1-manual-00000000-0000-0000-0000-000000000000";
        std::fs::write(directory.join("snapshots").join(format!("{invalid}.json")), r#"{"version":7}"#).unwrap();
        assert!(snapshots::run(&db, &path, "snapshot_restore", &json!({"id":invalid})).is_err());
        assert_eq!(run(&db, "document", &json!({"id":doc})).unwrap(), restored);

        snapshots::run(&db, &path, "snapshot_restore", &json!({"id":safety["id"]})).unwrap();
        assert!(run(&db, "document", &json!({"id":replacement})).is_ok());
        assert!(run(&db, "document", &json!({"id":doc})).is_err());
        let remaining = snapshots::run(&db, &path, "snapshot_delete", &json!({"id":id})).unwrap();
        assert!(remaining.as_array().unwrap().iter().all(|item| item["id"] != *id));
        assert!(snapshots::run(&db, &path, "snapshot_preview", &json!({"id":id})).is_err());
        assert!(run(&db, "document", &json!({"id":replacement})).is_ok());
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn semantic_terms_find_groups_missed_by_the_surface_and_exclude_previous_candidates() {
        let db = memory();
        let siege = create(&db, "Military siege and capture");
        let pressure = create(&db, "Siege and Hostile Pressure");
        assert!(groups(&db, "beleaguered", Some(5)).unwrap().is_empty());
        let found = followup_groups(
            &db,
            &json!(["under siege", "military"]),
            &json!([{"id":pressure}]),
        )
        .unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0]["id"], siege);
        assert!(followup_groups(&db, &json!(["!"]), &V::Null)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn reused_sense_links_another_document_without_creating_another_sense() {
        let db = memory();
        let group = create(&db, "Writing systems");
        let content = "They studied cuneiform.";
        let first = import(&db, "First.txt", "txt", content).unwrap();
        run(
            &db,
            "annotate",
            &json!({"document_id":first,"start":13,"end":22,
            "lemma":"cuneiform","meaning":"楔形文字","group_id":group}),
        )
        .unwrap();
        let saved = group_senses(&db, &group).unwrap();
        let second = import(&db, "Second.txt", "txt", "They decoded cuneiform.").unwrap();
        run(
            &db,
            "annotate",
            &json!({"document_id":second,"start":13,"end":22,
            "sense_id":saved[0]["id"]}),
        )
        .unwrap();
        assert_eq!(group_senses(&db, &group).unwrap(), saved);
        assert_eq!(
            db.query("SELECT sense_id FROM occurrences", &[]).unwrap(),
            vec![json!({"sense_id":saved[0]["id"]}); 2]
        );
    }

    #[test]
    fn annotation_reuses_group_with_same_normalized_name() {
        let db = memory();
        let group = create(&db, "Ancient Writing Systems");
        let doc = import(&db, "Note.txt", "txt", "They plunged.").unwrap();
        let mut note = anchor("They plunged.", &doc, 5);
        note["lemma"] = json!("plunge");
        note["meaning"] = json!("投入");
        note["new_group"] = json!({"name":" ancient  WRITING systems "});
        run(&db, "annotate", &note).unwrap();
        assert_eq!(
            db.one("SELECT group_id FROM senses", &[]).unwrap()["group_id"],
            group
        );
        assert_eq!(groups(&db, "", None).unwrap().len(), 1);
    }

    #[test]
    fn migrates_existing_library_and_preserves_web_source_in_backup() {
        let db = Db::open(Path::new(":memory:")).unwrap();
        let v1_schema = include_str!("schema.sql")
            .replace(", source_url TEXT, web_layout TEXT", "")
            .replace(" learning TEXT,", "")
            .replace("user_version=4", "user_version=1");
        db.batch(&v1_schema).unwrap();
        let old_id = uuid();
        db.write(
            "INSERT INTO documents(id,title,format,content) VALUES(?1,'Old.txt','txt','Original')",
            &[old_id.clone()],
        )
        .unwrap();
        init(&db).unwrap();
        assert_eq!(
            run(&db, "document", &json!({"id":old_id})).unwrap()["document"]["content"],
            "Original"
        );
        let mut old_backup = run(&db, "backup", &V::Null).unwrap();
        old_backup["version"] = json!(1);
        old_backup["documents"][0]
            .as_object_mut()
            .unwrap()
            .remove("source_url");
        run(&memory(), "restore", &old_backup).unwrap();

        let source = "https://example.org/news";
        let web_id = import_web(
            &db,
            crate::web_import::Page {
                title: "News".into(),
                content: "Prices plunged.".into(),
                url: source.into(),
                layout: None,
                images: Vec::new(),
            },
        )
        .unwrap();
        let anchor = json!({"document_id":web_id,"start":7,"end":14});
        let target = create(&db, "Decline");
        let mark = annotate_at(&db, anchor, &target, "下降");
        let learning = json!({"usage_note":"常用于价格急跌","example":"Prices plunged.",
            "etymology":"源自古法语。","mnemonic":"联想物体突然落入水中。",
            "related":[{"expression":"plummet","kind":"synonym","note":"同样强调急跌"}]});
        run(
            &db,
            "learning_save",
            &json!({"id":mark,"learning":learning}),
        )
        .unwrap();
        assert_eq!(
            run(&db, "list", &json!({"q":"example.org"})).unwrap()["documents"][0]["id"],
            web_id
        );

        let other = import(&db, "Later.txt", "txt", "They plunged again.").unwrap();
        let lookup = candidates(&db, &json!({"document_id":other,"start":5,"end":12})).unwrap();
        let saved = run(&db, "group_senses", &json!({"id":target})).unwrap();
        assert_eq!(saved.as_array().unwrap().len(), 1);
        assert_eq!(saved[0]["lemma"], "plunge");
        assert_eq!(lookup["notes"][0]["meaning"], "下降");
        assert_eq!(lookup["notes"][0]["group_name"], "Decline");
        assert_eq!(
            serde_json::from_str::<V>(lookup["notes"][0]["learning"].as_str().unwrap()).unwrap(),
            learning
        );

        let restored = memory();
        let backup = run(&db, "backup", &V::Null).unwrap();
        run(&restored, "restore", &backup).unwrap();
        let result = run(&restored, "document", &json!({"id":web_id})).unwrap();
        assert_eq!(result["document"]["source_url"], source);
        assert_eq!(result["marks"][0]["surface"], "plunged");
        assert_eq!(
            serde_json::from_str::<V>(result["marks"][0]["learning"].as_str().unwrap()).unwrap(),
            learning
        );
    }

    #[test]
    fn reopen_polysemy_move_split_merge_dedup_undo_and_restore() {
        let path =
            std::env::temp_dir().join(format!("semantic-reader-test-{}.sqlite3", Uuid::new_v4()));
        let db = Db::open(&path).unwrap();
        init(&db).unwrap();
        let text = "# 📉 Markets\r\nProfits plunged.\nThe swimmer plunged.\nPrices plunged.";
        let doc = import(&db, "Examples.md", "md", text).unwrap();
        let fall = create(&db, "Sharp decline");
        let water = create(&db, "Enter water");
        let duplicate = create(&db, "Financial shift");
        let anchors: Vec<_> = text
            .match_indices("plunged")
            .map(|(at, _)| anchor(text, &doc, at))
            .collect();
        let first = annotate_at(&db, anchors[0].clone(), &fall, "急剧下降");
        annotate_at(&db, anchors[1].clone(), &water, "突然跳入水中");
        annotate_at(&db, anchors[2].clone(), &duplicate, "急剧下降");
        assert!(run(&db, "annotate", &anchors[0])
            .unwrap_err()
            .contains("重叠"));
        assert_eq!(
            run(&db, "list", &json!({"q":"plummet"})).unwrap()["groups"]
                .as_array()
                .unwrap()
                .len(),
            3
        );
        assert_eq!(
            run(&db, "list", &json!({"q":"跳入"})).unwrap()["groups"][0]["id"],
            water
        );
        assert_eq!(
            candidates(&db, &anchors[0]).unwrap()["reused"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        drop(db);

        let db = Db::open(&path).unwrap();
        init(&db).unwrap();
        let read = || run(&db, "document", &json!({"id":doc})).unwrap();
        let before = read();
        assert_eq!(before["document"]["content"], text);
        assert_eq!(before["marks"][1]["group_id"], water);
        let sense = &before["marks"][0]["sense_id"];
        let split = run(
            &db,
            "split",
            &json!({"source":fall,"new_group":{"name":"Price decreases"},"sense_ids":[sense]}),
        )
        .unwrap();
        assert_eq!(read()["marks"][0]["group_id"], split);
        assert_eq!(
            vocabulary(&db, &split).unwrap(),
            vec![json!({"expression":"plunge","nuance":"急剧下降"})]
        );
        run(&db, "undo", &V::Null).unwrap();
        assert_eq!(read(), before);
        let merge = json!({"source":fall,"target":duplicate});
        run(&db, "merge", &merge).unwrap();
        assert_eq!(
            read()["marks"][0]["sense_id"],
            read()["marks"][2]["sense_id"]
        );
        assert!(group(&db, &fall).is_err());
        run(&db, "undo", &V::Null).unwrap();
        assert_eq!(read(), before);
        run(
            &db,
            "move",
            &json!({"source":fall,"target":water,"sense_ids":[sense]}),
        )
        .unwrap();
        assert_eq!(read()["marks"][0]["group_id"], water);
        run(&db, "undo", &V::Null).unwrap();
        assert_eq!(read(), before);

        run(
            &db,
            "reclassify",
            &json!({"source":fall,"target":duplicate,"sense_id":sense,
            "occurrence_ids":[first],"meaning":"短时间内急跌"}),
        )
        .unwrap();
        assert_eq!(read()["marks"][0]["group_id"], duplicate);
        assert_eq!(read()["marks"][2], before["marks"][2]);
        run(&db, "undo", &V::Null).unwrap();
        assert_eq!(read(), before);

        let backup = run(&db, "backup", &V::Null).unwrap();
        let restored = memory();
        let mut broken = backup.clone();
        broken["occurrences"][0]["surface"] = json!("broken");
        assert!(run(&restored, "restore", &broken).is_err());
        run(&restored, "restore", &backup).unwrap();
        assert!(run(&restored, "restore", &backup).is_err());
        assert_eq!(
            run(&restored, "document", &json!({"id":doc})).unwrap(),
            before
        );
        assert!(db
            .query("PRAGMA foreign_key_check", &[])
            .unwrap()
            .is_empty());
        drop(db);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn full_default_catalog_installs_once_and_empty_library_can_restore_old_backup() {
        let db = Db::open(Path::new(":memory:")).unwrap();
        init(&db).unwrap();
        assert_eq!(
            db.one("SELECT count(*) AS n FROM tag_scopes", &[]).unwrap()["n"],
            4500
        );
        init(&db).unwrap();
        let old = memory();
        let id = run(
            &old,
            "group_save",
            &json!({"name":"Writing","tags":["古代史"]}),
        )
        .unwrap();
        let mut backup = run(&old, "backup", &V::Null).unwrap();
        backup["version"] = json!(6);
        run(&db, "restore", &backup).unwrap();
        assert_eq!(tags::group_tags(&db, &id).unwrap()[0]["name"], "古代史");
        assert_eq!(
            db.one("SELECT count(*) AS n FROM tag_scopes", &[]).unwrap()["n"],
            4500
        );
        assert!(run(&db, "restore", &backup).is_err());
    }

    #[test]
    fn controlled_trial_is_idempotent_and_preserved_in_backup() {
        let db = memory();
        run(
            &db,
            "group_save",
            &json!({"name":"Writing","tags":["Existing"]}),
        )
        .unwrap();
        assert_eq!(
            run(&db, "tag_install_starter", &V::Null).unwrap()["added"],
            9
        );
        assert_eq!(
            run(&db, "tag_install_starter", &V::Null).unwrap()["added"],
            0
        );
        let backup = run(&db, "backup", &V::Null).unwrap();
        let restored = memory();
        run(&restored, "restore", &backup).unwrap();
        assert_eq!(
            tags::catalog(&restored).unwrap(),
            tags::catalog(&db).unwrap()
        );
    }

    #[test]
    fn tag_aliases_reuse_merge_and_restore() {
        let db = memory();
        let first = run(
            &db,
            "group_save",
            &json!({"name":"Writing","tags":["古代史","Writing"]}),
        )
        .unwrap();
        let second = run(
            &db,
            "group_save",
            &json!({"name":"Materials","tags":["古代历史"," writing "]}),
        )
        .unwrap();
        let catalog = tags::catalog(&db).unwrap();
        assert_eq!(catalog.len(), 3);
        let find = |name: &str| catalog.iter().find(|t| t["name"] == name).unwrap()["id"].clone();
        let target = find("古代史");
        let source = find("古代历史");
        let writing = find("Writing");
        run(&db, "tag_rename", &json!({"id":writing,"name":"书写"})).unwrap();
        assert!(run(&db, "tag_rename", &json!({"id":writing,"name":"古代历史"})).is_err());
        run(&db, "tag_merge", &json!({"source":source,"target":target})).unwrap();
        assert_eq!(tags::group_tags(&db, &first).unwrap().len(), 2);
        assert_eq!(tags::group_tags(&db, &second).unwrap().len(), 2);
        let third = run(
            &db,
            "group_save",
            &json!({"name":"Inscriptions","tags":["Writing","古代历史"]}),
        )
        .unwrap();
        assert_eq!(tags::catalog(&db).unwrap().len(), 2);
        assert_eq!(tags::group_tags(&db, &third).unwrap().len(), 2);
        let backup = run(&db, "backup", &V::Null).unwrap();
        let other = memory();
        run(&other, "restore", &backup).unwrap();
        assert_eq!(tags::catalog(&other).unwrap(), tags::catalog(&db).unwrap());
        assert_eq!(
            tags::group_tags(&other, &third).unwrap(),
            tags::group_tags(&db, &third).unwrap()
        );
    }

    #[test]
    fn merging_and_undoing_groups_keeps_tag_memberships() {
        let db = memory();
        let a = run(&db, "group_save", &json!({"name":"A","tags":["A tag"]})).unwrap();
        let b = run(&db, "group_save", &json!({"name":"B","tags":["B tag"]})).unwrap();
        run(&db, "merge", &json!({"source":a,"target":b})).unwrap();
        assert_eq!(tags::group_tags(&db, &b).unwrap().len(), 2);
        run(&db, "undo", &V::Null).unwrap();
        assert_eq!(tags::group_tags(&db, &a).unwrap()[0]["name"], "A tag");
        assert_eq!(tags::group_tags(&db, &b).unwrap().len(), 1);
        assert_eq!(tags::group_tags(&db, &b).unwrap()[0]["name"], "B tag");
    }

    #[test]
    fn repair_images_keeps_saved_images_and_edited_text_and_annotations() {
        use crate::web_layout::{Layout, Node};
        let db = memory();
        let content = "They plunged.";
        let url = "https://example.org/news";
        let page = |content: &str, images| crate::web_import::Page {
            url: url.into(),
            title: "News".into(),
            content: content.into(),
            images: Vec::new(),
            layout: Some(Layout {
                images,
                image_errors: Vec::new(),
                nodes: vec![
                    Node::Image {
                        index: 0,
                        alt: "First".into(),
                    },
                    Node::Text { start: 0, end: 13 },
                    Node::Image {
                        index: 1,
                        alt: "Second".into(),
                    },
                ],
            }),
        };
        let png = crate::web_layout::image_uri(b"\x89PNG\r\n\x1a\n").unwrap();
        let doc = import_web(&db, page(content, vec![Some(png.clone()), None])).unwrap();
        let group = create(&db, "Movement");
        let mark = annotate_at(&db, anchor(content, &doc, 5), &group, "突然跳入");
        let params = json!({"id":doc});
        let snapshot = image_repair_snapshot(&db, &params).unwrap();
        let mut pending = page(content, vec![None, None]);
        seed_saved_images(&mut pending, &snapshot).unwrap();
        assert!(pending.layout.as_ref().unwrap().images[0].is_some());
        assert!(pending.layout.as_ref().unwrap().images[1].is_none());

        let mut changed_page = page("Different website text", vec![None, Some(png.clone())]);
        let layout = changed_page.layout.as_mut().unwrap();
        layout.nodes[1] = Node::Text { start: 0, end: 22 };
        seed_saved_images(&mut changed_page, &snapshot).unwrap();
        changed_page.layout.as_mut().unwrap().images[1] = Some(png);
        assert_eq!(
            repair_images(&db, &params, url, changed_page).unwrap(),
            json!({"saved":2,"missing":0})
        );
        let after = image_repair_snapshot(&db, &params).unwrap();
        let before_layout: V = serde_json::from_str(snapshot["web_layout"].as_str().unwrap()).unwrap();
        let after_layout: V = serde_json::from_str(after["web_layout"].as_str().unwrap()).unwrap();
        assert_eq!(after_layout["nodes"], before_layout["nodes"]);
        assert_eq!(after_layout["images"][0], before_layout["images"][0]);
        let read = run(&db, "document", &params).unwrap();
        assert_eq!(read["document"]["content"], content);
        assert_eq!(read["marks"][0]["id"], mark);
    }

    #[test]
    fn editing_web_body_keeps_images_and_source() {
        use crate::web_layout::{Layout, Node};
        let db = memory();
        let png = crate::web_layout::image_uri(b"\x89PNG\r\n\x1a\n").unwrap();
        let url = "https://example.org/news";
        let doc = import_web(&db, crate::web_import::Page {
            url: url.into(),
            title: "News".into(),
            content: "Headline".into(),
            images: Vec::new(),
            layout: Some(Layout {
                nodes: vec![Node::Text { start: 0, end: 8 }, Node::Image { index: 0, alt: "Photo".into() }],
                images: vec![Some(png.clone())],
                image_errors: Vec::new(),
            }),
        }).unwrap();
        run(&db, "document_edit", &json!({
            "id":doc, "original":"Headline", "content":"New headline", "positions":[],
            "web_nodes":[{"type":"text","start":0,"end":12},{"type":"image","index":0,"alt":"Photo"}]
        })).unwrap();
        let result = run(&db, "document", &json!({"id":doc})).unwrap();
        let layout: V = serde_json::from_str(result["document"]["web_layout"].as_str().unwrap()).unwrap();
        assert_eq!(result["document"]["content"], "New headline");
        assert_eq!(result["document"]["source_url"], url);
        assert_eq!(layout["nodes"][0]["end"], 12);
        assert_eq!(layout["images"][0], png);
    }

    #[test]
    fn bulk_group_delete_is_atomic_and_deduplicates_ids() {
        let db = memory();
        let a = create(&db, "A");
        let b = create(&db, "B");
        let kept = create(&db, "Kept");
        assert!(run(&db, "groups_delete", &json!({"ids":[a,uuid()]})).is_err());
        assert!(group(&db, &a).is_ok());
        run(&db, "groups_delete", &json!({"ids":[a,b,a]})).unwrap();
        assert!(group(&db, &a).is_err());
        assert!(group(&db, &b).is_err());
        assert!(group(&db, &kept).is_ok());
        assert!(db
            .query("PRAGMA foreign_key_check", &[])
            .unwrap()
            .is_empty());
    }

    #[test]
    fn delete_group_removes_its_marks_and_search_result_but_keeps_other_senses_and_text() {
        let db = memory();
        let text = "Prices plunged. They plunged.";
        let doc = import(&db, "Article.txt", "txt", text).unwrap();
        let removed = create(&db, "Falling prices");
        let kept = create(&db, "Physical movement");
        annotate_at(&db, anchor(text, &doc, 7), &removed, "下跌");
        let other = annotate_at(&db, anchor(text, &doc, 21), &kept, "跳入");
        let target = create(&db, "Temporary");
        run(&db, "merge", &json!({"source":removed,"target":target})).unwrap();
        run(&db, "group_delete", &json!({"id":target})).unwrap();
        let result = run(&db, "document", &json!({"id":doc})).unwrap();
        assert_eq!(result["document"]["content"], text);
        assert_eq!(result["marks"].as_array().unwrap().len(), 1);
        assert_eq!(result["marks"][0]["id"], other);
        assert!(run(&db, "group", &json!({"id":target})).is_err());
        assert!(groups(&db, "Temporary", None).unwrap().is_empty());
        assert!(run(&db, "undo", &V::Null).is_err());
        assert!(db
            .query("PRAGMA foreign_key_check", &[])
            .unwrap()
            .is_empty());
    }

    #[test]
    fn editing_body_moves_unchanged_marks_and_keeps_removed_marks_senses() {
        let db = memory();
        let original = "Prices plunged. They plunged.";
        let doc = import(&db, "Article.txt", "txt", original).unwrap();
        let group = create(&db, "Decline");
        let first = annotate_at(&db, anchor(original, &doc, 7), &group, "下降");
        let second = annotate_at(&db, anchor(original, &doc, 21), &group, "下落");
        let content = "🙂 Prices plunged. They rose.";
        let mut edit = json!({"id":doc,"original":original,"content":content,
            "positions":[{"id":first,"start":8},{"id":second,"start":null}]});
        assert!(run(&db, "document_edit", &edit).is_err());
        let before = run(&db, "document", &json!({"id":doc})).unwrap();
        assert_eq!(before["document"]["content"], original);
        assert_eq!(before["marks"].as_array().unwrap().len(), 2);
        edit["positions"][0]["start"] = json!(10);
        assert_eq!(run(&db, "document_edit", &edit).unwrap(), 1);
        assert!(run(&db, "document_edit", &edit).is_err());
        let after = run(&db, "document", &json!({"id":doc})).unwrap();
        assert_eq!(after["document"]["content"], content);
        assert_eq!(after["marks"].as_array().unwrap().len(), 1);
        assert_eq!(after["marks"][0]["id"], first);
        assert_eq!(after["marks"][0]["context"], "🙂 Prices ⟦plunged⟧. They rose.");
        assert_eq!(group_senses(&db, &group).unwrap().len(), 2);
    }

    #[test]
    fn deleting_one_mark_preserves_its_sense_and_other_links() {
        let db = memory();
        let text = "Prices plunged. They plunged.";
        let first = import(&db, "First.txt", "txt", text).unwrap();
        let second = import(&db, "Second.txt", "txt", text).unwrap();
        let group = create(&db, "Decline");
        let removed = annotate_at(&db, anchor(text, &first, 7), &group, "下降");
        let kept = annotate_at(&db, anchor(text, &second, 7), &group, "下降");
        run(&db, "mark_delete", &json!({"id":removed,"document_id":first})).unwrap();
        let detail = run(&db, "group", &json!({"id":group})).unwrap();
        assert_eq!(detail["senses"].as_array().unwrap().len(), 1);
        assert_eq!(detail["marks"].as_array().unwrap().len(), 1);
        assert_eq!(detail["marks"][0]["id"], kept);
        assert_eq!(run(&db, "mark_delete", &json!({"id":kept,"document_id":second})).unwrap(), true);
        assert!(group_senses(&db, &group).unwrap().is_empty());
        assert_eq!(run(&db, "document", &json!({"id":first})).unwrap()["document"]["content"], text);
    }

    #[test]
    fn deleting_one_sense_removes_its_links_across_articles_and_preserves_other_senses() {
        let db = memory();
        let text = "Prices plunged. They plunged.";
        let first = import(&db, "First.txt", "txt", text).unwrap();
        let second = import(&db, "Second.txt", "txt", text).unwrap();
        let group = create(&db, "Decline");
        db.write("INSERT INTO group_vocabulary VALUES(?1,'plunge','')", &[group.clone()]).unwrap();
        annotate_at(&db, anchor(text, &first, 7), &group, "Financial loss");
        annotate_at(&db, anchor(text, &second, 7), &group, "Financial loss");
        let other = annotate_at(&db, anchor(text, &first, 21), &group, "Jumped");
        let sense = run(&db, "document", &json!({"id":second})).unwrap()["marks"][0]["sense_id"].clone();
        run(&db, "sense_delete", &json!({"id":sense})).unwrap();
        let remaining = run(&db, "group", &json!({"id":group})).unwrap();
        assert_eq!(remaining["senses"].as_array().unwrap().len(), 1);
        assert_eq!(remaining["marks"].as_array().unwrap().len(), 1);
        assert_eq!(remaining["marks"][0]["id"], other);
        assert!(groups(&db, "Financial", None).unwrap().is_empty());
        let after = run(&db, "document", &json!({"id":second})).unwrap();
        assert_eq!(after["document"]["content"], text);
        assert_eq!(after["marks"], json!([]));
        assert!(remaining["vocabulary"].as_array().unwrap().iter().any(|row| row["expression"] == "plunge"));
        run(&db, "sense_delete", &json!({"id":remaining["senses"][0]["id"]})).unwrap();
        let empty = run(&db, "group", &json!({"id":group})).unwrap();
        assert!(empty["vocabulary"].as_array().unwrap().iter().all(|row| row["expression"] != "plunge"));
        assert_eq!(empty["senses"], json!([]));
    }

    #[test]
    fn delete_article_keeps_vocabulary_other_articles_and_relationship_undo() {
        let db = memory();
        let text = "They plunged.";
        let first = import(&db, "First.txt", "txt", text).unwrap();
        let second = import(&db, "Second.txt", "txt", text).unwrap();
        let source = create(&db, "Decline");
        let target = create(&db, "Movement");
        let first_mark = annotate_at(&db, anchor(text, &first, 5), &source, "下降");
        annotate_at(&db, anchor(text, &second, 5), &source, "下降");
        let before = run(&db, "document", &json!({"id":first})).unwrap();
        run(
            &db,
            "reclassify",
            &json!({"source":source,"target":target,
            "sense_id":before["marks"][0]["sense_id"],"occurrence_ids":[first_mark],
            "meaning":"突然向下移动"}),
        )
        .unwrap();
        run(&db, "document_delete", &json!({"id":first})).unwrap();
        assert!(run(&db, "document", &json!({"id":first})).is_err());
        assert_eq!(
            run(&db, "group", &json!({"id":target})).unwrap()["senses"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            run(&db, "group", &json!({"id":source})).unwrap()["marks"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert!(run(&db, "document_delete", &json!({"id":first})).is_err());
        run(&db, "undo", &V::Null).unwrap();
        assert_eq!(
            run(&db, "document", &json!({"id":second})).unwrap()["marks"][0]["meaning"],
            "下降"
        );
        assert!(db
            .query("PRAGMA foreign_key_check", &[])
            .unwrap()
            .is_empty());
    }

    #[test]
    fn failed_split_rolls_back_and_surrogate_middle_is_rejected() {
        let db = memory();
        let text = "🦊 plunged";
        let doc = import(&db, "Story.txt", "txt", text).unwrap();
        assert!(selection(&db, &json!({"document_id":doc,"start":1,"end":4})).is_err());
        let source = create(&db, "Decline");
        let mark = anchor(text, &doc, 5);
        annotate_at(&db, mark, &source, "下降");
        let err = run(
            &db,
            "split",
            &json!({"source":source,"new_group":{"name":"Temporary group"},"sense_ids":[uuid()]}),
        );
        assert!(err.is_err());
        assert_eq!(groups(&db, "", None).unwrap().len(), 1);
        assert!(db
            .query("SELECT * FROM operation_history", &[])
            .unwrap()
            .is_empty());
    }
}
