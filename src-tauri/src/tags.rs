use crate::sqlite::{Db, Result};
use serde_json::{json, Value as V};
use uuid::Uuid;

pub fn normalize(name: &str) -> String {
    name.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

pub fn name(value: &V) -> Result<String> {
    let raw = value.as_str().ok_or("标签名称无效")?;
    let cleaned = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if cleaned.is_empty() || cleaned.chars().count() > 48 || raw.chars().any(char::is_control) {
        return Err("标签名称须为 1–48 个字符".into());
    }
    Ok(cleaned)
}

fn owner(db: &Db, normalized: &str) -> Result<Option<V>> {
    Ok(db
        .query(
            "SELECT tag_id FROM tag_names WHERE normalized=?1",
            &[json!(normalized)],
        )?
        .into_iter()
        .next()
        .map(|row| row["tag_id"].clone()))
}

fn resolve(db: &Db, value: &V) -> Result<V> {
    let name = name(value)?;
    let normalized = normalize(&name);
    if let Some(id) = owner(db, &normalized)? {
        return Ok(id);
    }
    mark_defaults_modified(db)?;
    let id = json!(Uuid::new_v4().to_string());
    db.write("INSERT INTO tags VALUES(?1,?2)", &[id.clone(), json!(name)])?;
    db.write(
        "INSERT INTO tag_names VALUES(?1,?2,?3)",
        &[json!(normalized), json!(name), id.clone()],
    )?;
    Ok(id)
}

pub fn group_tags(db: &Db, id: &V) -> Result<Vec<V>> {
    db.query(
        "SELECT t.id,t.name FROM group_tags gt JOIN tags t ON t.id=gt.tag_id
        WHERE gt.group_id=?1 ORDER BY t.name COLLATE NOCASE,t.id",
        std::slice::from_ref(id),
    )
}

pub fn save_group(db: &Db, id: &V, names: &V) -> Result<()> {
    db.query(
        "DELETE FROM group_tags WHERE group_id=?1",
        std::slice::from_ref(id),
    )?;
    add_group(db, id, names)
}

pub fn add_group(db: &Db, id: &V, names: &V) -> Result<()> {
    let names = names
        .as_array()
        .filter(|n| n.len() <= 32)
        .ok_or("一次最多可保存 32 个标签")?;
    for value in names {
        let tag = resolve(db, value)?;
        db.query(
            "INSERT OR IGNORE INTO group_tags VALUES(?1,?2)",
            &[id.clone(), tag],
        )?;
    }
    let count = db.one(
        "SELECT count(*) AS count FROM group_tags WHERE group_id=?1",
        std::slice::from_ref(id),
    )?;
    if count["count"].as_u64().unwrap_or(0) > 32 {
        return Err("一个语义组最多可保存 32 个标签".into());
    }
    Ok(())
}

pub fn catalog(db: &Db) -> Result<Vec<V>> {
    let mut tags = db.query(
        "SELECT t.*,s.scope,s.source_label,s.source_url, (SELECT count(*) FROM group_tags gt WHERE gt.tag_id=t.id) AS count
        FROM tags t LEFT JOIN tag_scopes s ON s.tag_id=t.id ORDER BY t.name COLLATE NOCASE,t.id",
        &[],
    )?;
    let rows = db.query(
        "SELECT n.tag_id,n.name FROM tag_names n JOIN tags t ON t.id=n.tag_id
        WHERE n.name<>t.name ORDER BY n.name",
        &[],
    )?;
    let mut aliases = std::collections::HashMap::<String, Vec<V>>::new();
    for row in rows {
        aliases
            .entry(row["tag_id"].as_str().unwrap().into())
            .or_default()
            .push(row["name"].clone());
    }
    for tag in &mut tags {
        tag["aliases"] = json!(aliases
            .remove(tag["id"].as_str().unwrap())
            .unwrap_or_default());
    }
    Ok(tags)
}

pub fn install_unesco(db: &Db) -> Result<()> {
    let data = flate2::read::GzDecoder::new(&include_bytes!("unesco-tags.json.gz")[..]);
    let rows: Vec<[String; 3]> =
        serde_json::from_reader(data).map_err(|_| "内置 UNESCO 词表无效")?;
    let aliases: Vec<[String; 4]> =
        serde_json::from_str(include_str!("tag-starter.json")).map_err(|_| "试用词表格式无效")?;
    let translated: std::collections::HashMap<_, _> = aliases
        .iter()
        .filter(|row| row[3].contains("unesco.org"))
        .map(|row| (row[2].as_str(), (row[0].as_str(), row[1].as_str())))
        .collect();
    for [label, scope, source] in rows {
        let translated = translated.get(label.as_str());
        let normalized = normalize(&label);
        let tag = if let Some(existing) = owner(db, &normalized)? {
            existing
        } else if let Some((name, _)) = translated {
            resolve(db, &json!(name))?
        } else {
            resolve(db, &json!(label))?
        };
        db.query(
            "INSERT OR IGNORE INTO tag_names VALUES(?1,?2,?3)",
            &[json!(normalized), json!(label), tag.clone()],
        )?;
        let scope = translated.map(|(_, scope)| *scope).unwrap_or(&scope);
        db.query(
            "INSERT OR IGNORE INTO tag_scopes VALUES(?1,?2,?3,?4)",
            &[tag, json!(scope), json!(label), json!(source)],
        )?;
    }
    Ok(())
}

pub fn mark_defaults_modified(db: &Db) -> Result<()> {
    db.query(
        "UPDATE tag_defaults SET pristine=0 WHERE id=1 AND pristine=1",
        &[],
    )?;
    Ok(())
}

pub fn install_starter(db: &Db) -> Result<V> {
    let rows: Vec<[String; 4]> =
        serde_json::from_str(include_str!("tag-starter.json")).map_err(|_| "试用词表格式无效")?;
    let mut added = 0;
    for [label, scope, source_label, source_url] in rows {
        let tag = resolve(db, &json!(label))?;
        let existing = db.query(
            "SELECT 1 AS present FROM tag_scopes WHERE tag_id=?1",
            std::slice::from_ref(&tag),
        )?;
        if existing.is_empty() {
            db.write(
                "INSERT INTO tag_scopes VALUES(?1,?2,?3,?4)",
                &[tag, json!(scope), json!(source_label), json!(source_url)],
            )?;
            added += 1;
        }
    }
    Ok(json!({"added":added}))
}

pub fn save_scope(db: &Db, existing: Option<V>, label: &V, scope: &str) -> Result<()> {
    let tag = if let Some(id) = existing {
        db.one("SELECT id FROM tags WHERE id=?1", std::slice::from_ref(&id))?;
        id
    } else {
        if scope.is_empty() {
            return Err("请填写标签的使用范围".into());
        }
        resolve(db, label)?
    };
    if scope.is_empty() {
        db.query("DELETE FROM tag_scopes WHERE tag_id=?1", &[tag])?;
    } else {
        db.write(
            "INSERT INTO tag_scopes VALUES(?1,?2,'','')
            ON CONFLICT(tag_id) DO UPDATE SET scope=excluded.scope",
            &[tag, json!(scope)],
        )?;
    }
    Ok(())
}

pub fn rename(db: &Db, id: &V, value: &V) -> Result<()> {
    let name = name(value)?;
    let normalized = normalize(&name);
    if owner(db, &normalized)?.is_some_and(|owner| owner != *id) {
        return Err("此名称已属于另一个标签。可以使用「合并标签」。".into());
    }
    db.write(
        "UPDATE tags SET name=?2 WHERE id=?1",
        &[id.clone(), json!(name)],
    )?;
    db.write(
        "INSERT INTO tag_names VALUES(?1,?2,?3)
        ON CONFLICT(normalized) DO UPDATE SET name=excluded.name",
        &[json!(normalized), json!(name), id.clone()],
    )
}

pub fn merge(db: &Db, source: &V, target: &V) -> Result<()> {
    if source == target {
        return Err("请选择不同的标签".into());
    }
    for id in [source, target] {
        db.one("SELECT id FROM tags WHERE id=?1", std::slice::from_ref(id))?;
    }
    db.query(
        "INSERT OR IGNORE INTO group_tags SELECT group_id,?2 FROM group_tags WHERE tag_id=?1",
        &[source.clone(), target.clone()],
    )?;
    db.query(
        "UPDATE tag_names SET tag_id=?2 WHERE tag_id=?1",
        &[source.clone(), target.clone()],
    )?;
    db.query("INSERT OR IGNORE INTO tag_scopes SELECT ?2,scope,source_label,source_url FROM tag_scopes WHERE tag_id=?1",
        &[source.clone(),target.clone()])?;
    db.write("DELETE FROM tags WHERE id=?1", std::slice::from_ref(source))
}
