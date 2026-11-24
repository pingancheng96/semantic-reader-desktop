use crate::{sqlite::{Db, Result}, store};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

pub const CODES: &[&str] = &["en", "fr", "de", "es", "it", "pt", "ja", "ko", "zh", "ru", "ar", "da"];

pub fn validate(code: &str) -> Result<&str> {
    if CODES.contains(&code) { Ok(code) } else { Err("不支持的学习语言".into()) }
}

pub fn name(code: &str) -> &str {
    match code {
        "fr"=>"French", "de"=>"German", "es"=>"Spanish", "it"=>"Italian",
        "pt"=>"Portuguese", "ja"=>"Japanese", "ko"=>"Korean", "zh"=>"Chinese",
        "ru"=>"Russian", "ar"=>"Arabic", "da"=>"Danish", _=>"English",
    }
}

pub fn path(root: &Path, code: &str) -> Result<PathBuf> {
    validate(code)?;
    if code=="en" { return Ok(root.to_owned()); }
    Ok(root.with_file_name("languages").join(code).join("library.sqlite3"))
}

pub fn prepare(root: &Path, code: &str) -> Result<PathBuf> {
    let path = path(root,code)?;
    std::fs::create_dir_all(path.parent().ok_or("数据目录无效")?).map_err(|e|e.to_string())?;
    let db = Db::open(&path)?;
    store::init(&db)?;
    db.write("UPDATE workspace_language SET code=?1 WHERE id=1", &[json!(code)])?;
    Ok(path)
}

pub fn current(db: &Db) -> Result<String> {
    Ok(db.one("SELECT code FROM workspace_language WHERE id=1", &[])?["code"].as_str().unwrap_or("en").to_owned())
}

pub fn backup_all(root: &Path) -> Result<Value> {
    let mut spaces = Vec::new();
    for code in CODES {
        let path = path(root,code)?;
        if !path.is_file() { continue; }
        let db = Db::open(&path)?;
        spaces.push(db.snapshot(||store::backup_data(&db))?);
    }
    Ok(json!({"version":1,"kind":"semantic-reader-languages","spaces":spaces}))
}

pub fn restore_all(root: &Path, data: &Value) -> Result<()> {
    let spaces = if data["kind"]=="semantic-reader-languages" {
        if data["version"]!=1 { return Err("多语言备份版本不兼容".into()); }
        data["spaces"].as_array().ok_or("语言备份无效")?.clone()
    } else { vec![data.clone()] };
    let mut codes = std::collections::HashSet::new();
    // Validate every space in a disposable database before touching existing libraries.
    let stage = std::env::temp_dir().join(format!("semantic-reader-restore-{}",uuid::Uuid::new_v4()));
    let result = (|| {
        for space in &spaces {
            let code = space["language"].as_str().unwrap_or("en");
            validate(code)?;
            if !codes.insert(code.to_owned()) { return Err("备份包含重复的语言空间".into()); }
            let staged = prepare(&stage.join("library.sqlite3"),code)?;
            store::run(&Db::open(&staged)?,"restore",space)?;
            let target = path(root,code)?;
            if target.is_file() {
                let db = Db::open(&target)?;
                if db.one("SELECT EXISTS(SELECT 1 FROM documents) OR EXISTS(SELECT 1 FROM semantic_groups) AS occupied", &[])?["occupied"]==1 {
                    return Err(format!("{} 学习空间已有数据，请先清空该空间或使用本地备份恢复",name(code)));
                }
            }
        }
        for space in &spaces {
            let code = space["language"].as_str().unwrap_or("en");
            let target = prepare(root,code)?;
            store::run(&Db::open(&target)?,"restore",space)?;
        }
        Ok(())
    })();
    let _ = std::fs::remove_dir_all(stage);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn isolates_languages_and_roundtrips_all_spaces() {
        let dir=std::env::temp_dir().join(format!("language-test-{}",uuid::Uuid::new_v4()));
        let root=dir.join("library.sqlite3");
        let en=Db::open(&prepare(&root,"en").unwrap()).unwrap();
        let fr=Db::open(&prepare(&root,"fr").unwrap()).unwrap();
        let doc=store::import(&en,"English","txt","pain").unwrap();
        let french=store::import(&fr,"French","txt","pain").unwrap();
        for (db,id,meaning) in [(&en,doc.clone(),"疼痛"),(&fr,french.clone(),"面包")] {
            store::run(db,"annotate",&json!({"document_id":id,"start":0,"end":4,"lemma":"pain","meaning":meaning,"ungrouped_capture":true})).unwrap();
        }
        let candidates=store::candidates(&fr,&json!({"document_id":french,"start":0,"end":4})).unwrap();
        assert_eq!(candidates["notes"].as_array().unwrap().len(),1);
        assert_eq!(candidates["notes"][0]["meaning"],"面包");
        assert!(store::run(&fr,"document",&json!({"id":doc})).is_err());
        assert_eq!(current(&en).unwrap(),"en");
        assert_eq!(current(&fr).unwrap(),"fr");
        assert!(path(&root,"../en").is_err());
        let backup=backup_all(&root).unwrap();
        assert_eq!(backup["spaces"].as_array().unwrap().len(),2);
        let other=dir.join("restored/library.sqlite3");
        restore_all(&other,&backup).unwrap();
        let db=Db::open(&path(&other,"fr").unwrap()).unwrap();
        assert_eq!(db.one("SELECT title FROM documents",&[]).unwrap()["title"],"French");
        let before=store::backup_data(&fr).unwrap();
        assert!(store::run(&fr,"restore",&store::backup_data(&en).unwrap()).is_err());
        assert_eq!(store::backup_data(&fr).unwrap(),before);
        let _=std::fs::remove_dir_all(dir);
    }
}
