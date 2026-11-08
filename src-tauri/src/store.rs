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
            Some(7) => return Ok(()),
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
        Ok(())
    })
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
            "INSERT INTO semantic_groups VALUES(?1,?2,?3)",
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
        "SELECT source_url,content,web_layout FROM documents WHERE id=?1",
        &[id(p, "id")?],
    )?;
    if !row["source_url"].is_string() {
        return Err("这篇文档没有网页来源".into());
    }
    Ok(row)
}

pub fn seed_saved_images(page: &mut crate::web_import::Page, snapshot: &V) -> Result<()> {
    if snapshot["content"] != page.content {
        return Err("网页正文与导入时不同，现有文章和标注已保留。".into());
    }
    let layout = page.layout.as_mut().ok_or("该地址没有可读取的网页版式")?;
    if let Some(raw) = snapshot["web_layout"].as_str() {
        let previous = serde_json::from_str(raw).map_err(|_| "旧网页版式数据无效")?;
        layout.keep_saved_images(&previous)?;
    }
    Ok(())
}

pub fn repair_images(db: &Db, p: &V, source: &str, page: crate::web_import::Page) -> Result<V> {
    let key = id(p, "id")?;
    let mut layout = page.layout.ok_or("该地址没有可读取的网页版式")?;
    db.transaction(|| {
        let args = std::slice::from_ref(&key);
        let old = db.one(
            "SELECT source_url,content,web_layout FROM documents WHERE id=?1",
            args,
        )?;
        if old["source_url"] != source || old["content"] != page.content {
            return Err("网页正文与导入时不同，无法安全补图。现有文章和标注已保留。".into());
        }
        if let Some(raw) = old["web_layout"].as_str() {
            let previous = serde_json::from_str(raw).map_err(|_| "旧网页版式数据无效")?;
            layout.keep_saved_images(&previous)?;
        }
        layout.cap_images();
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
        || content.len() > 2_000_000
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
        .filter(|n| *n <= 2_000_000)
        .ok_or("起始位置无效")?;
    let end = p["end"]
        .as_u64()
        .filter(|n| *n > start && *n - start <= 240)
        .ok_or("请选择 1–240 个字符")? as usize;
    let row = db.one("SELECT content FROM documents WHERE id=?1", &[doc.clone()])?;
    let content = row["content"].as_str().unwrap();
    let a = byte_offset(content, start as usize).ok_or("起始位置无效")?;
    let b = byte_offset(content, end).ok_or("结束位置无效")?;
    let surface = &content[a..b];
    if surface.trim() != surface || surface.is_empty() || surface.contains(['\n', '\r']) {
        return Err("请选择同一行中的词或短语".into());
    }
    let prefix: String = content[..a]
        .rsplit(['\r', '\n'])
        .next()
        .unwrap_or("")
        .chars()
        .rev()
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
    Ok((
        doc,
        json!(start),
        json!(end),
        surface.into(),
        format!("{prefix}⟦{surface}⟧{suffix}"),
    ))
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

pub fn candidates(db: &Db, p: &V) -> Result<V> {
    let (doc, start, end, surface, context) = selection(db, p)?;
    let saved_sense = db.query(
        "SELECT e.lemma,s.meaning FROM occurrences o JOIN senses s ON s.id=o.sense_id
         JOIN expressions e ON e.id=s.expression_id WHERE o.document_id=?1 AND o.start=?2 AND o.end=?3",
        &[doc,start,end],
    )?.into_iter().next();
    let exact = db.query(
        &format!(
            "{SENSES} WHERE e.normalized=?1 OR e.id IN (
            SELECT prior.expression_id FROM occurrences o JOIN senses prior ON prior.id=o.sense_id
            WHERE o.surface=?2 COLLATE NOCASE
        ) ORDER BY s.meaning,s.id LIMIT 30"
        ),
        &[json!(normalize(&surface)), json!(surface)],
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
        let group = if p["group_id"].is_null() {
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
        db.query(
            "INSERT INTO expressions VALUES(?1,?2,?3) ON CONFLICT(normalized) DO NOTHING",
            &[uuid(), json!(lemma), normalized.clone()],
        )?;
        let expression = db.one(
            "SELECT id FROM expressions WHERE normalized=?1",
            &[normalized],
        )?;
        let sense = upsert_sense(db, &expression["id"], &group, &json!(meaning))?;
        reindex(db, &group)?;
        sense
    };
    let key = uuid();
    let learning = learning_value(&p["learning"])?;
    db.write(
        "INSERT INTO occurrences(id,document_id,sense_id,start,end,surface,context,learning) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
        &[
            key.clone(),
            doc,
            sense,
            start,
            end,
            json!(surface),
            json!(context),
            learning,
        ],
    )?;
    Ok(key)
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
            "INSERT INTO semantic_groups VALUES(?1,?2,?3)",
            &[
                source.clone(),
                u["source_group"]["name"].clone(),
                u["source_group"]["description"].clone(),
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
    ("semantic_groups", &["id", "name", "description"]),
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
        ],
    ),
];

fn restore(db: &Db, p: &V) -> Result<V> {
    if !matches!(p["version"].as_u64(), Some(1 | 2 | 3 | 4 | 5 | 6 | 7)) {
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
        if table == "tag_scopes" && p["version"].as_u64().unwrap_or(0) < 6 {
            continue;
        }
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
                    if col == "start" || col == "end" {
                        return row[col]
                            .as_i64()
                            .map(|n| json!(n))
                            .ok_or_else(|| format!("{col} 无效"));
                    }
                    if col == "id" || col.ends_with("_id") {
                        return id(row, col);
                    }
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
                    if col == "content" {
                        let text = row[col]
                            .as_str()
                            .filter(|s| {
                                s.len() <= 2_000_000 && !s.trim().is_empty() && !s.contains('\0')
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
    if !defaults.is_empty() && p["version"] != 7 {
        tags::install_unesco(db)?;
    }
    tags::mark_defaults_modified(db)?;
    Ok(V::Null)
}

pub fn run(db: &Db, op: &str, p: &V) -> Result<V> {
    match op {
        "list" => {
            let q = field(p, "q", 200, false)?;
            let docs = db.query("SELECT id,title,format,source_url FROM documents WHERE instr(lower(title),lower(?1))>0 OR instr(lower(coalesce(source_url,'')),lower(?1))>0 ORDER BY created_at DESC, rowid DESC",&[json!(q)])?;
            let undo = db.one(
                "SELECT EXISTS(SELECT 1 FROM operation_history) AS available",
                &[],
            )?;
            Ok(json!({"documents":docs,"groups":groups(db,q,None)?,"undo":undo["available"],"tags":tags::catalog(db)?,"tag_links":db.query("SELECT * FROM group_tags",&[])?}))
        }
        "document" => {
            let key = id(p, "id")?;
            let doc = db.one("SELECT * FROM documents WHERE id=?1", &[key.clone()])?;
            let marks = db.query(
                &format!("{MARKS} WHERE o.document_id=?1 ORDER BY o.start"),
                &[key],
            )?;
            Ok(json!({"document":doc,"marks":marks}))
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
        "document_delete" => db.transaction(|| {
            let key = id(p, "id")?;
            db.query(
                "DELETE FROM occurrences WHERE document_id=?1",
                std::slice::from_ref(&key),
            )?;
            db.write("DELETE FROM documents WHERE id=?1", &[key])?;
            Ok(V::Null)
        }),
        "backup" => db.transaction(|| {
            let mut data = json!({"version":7});
            for &(table, _) in BACKUP_TABLES {
                data[table] = json!(db.query(&format!("SELECT * FROM {table}"), &[])?);
            }
            Ok(data)
        }),
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
    fn repair_images_keeps_saved_images_and_annotations_and_rejects_changed_text() {
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

        assert!(repair_images(
            &db,
            &params,
            url,
            page("Changed", vec![None, Some(png.clone())])
        )
        .is_err());
        assert_eq!(
            repair_images(&db, &params, url, page(content, vec![None, Some(png)])).unwrap(),
            json!({"saved":2,"missing":0})
        );
        let read = run(&db, "document", &params).unwrap();
        assert_eq!(read["document"]["content"], content);
        assert_eq!(read["marks"][0]["id"], mark);
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
