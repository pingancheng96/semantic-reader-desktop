use crate::mdict::{resource_key, Archive};
use base64::{engine::general_purpose::STANDARD, Engine};
use dom_query::Document;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

type Result<T> = std::result::Result<T, String>;

#[derive(Serialize, Deserialize)]
struct Library {
    id: String,
    name: String,
    files: Vec<String>,
    size: u64,
}

struct Indexed {
    key: PathBuf,
    words: Archive,
    resources: Vec<Archive>,
}
static INDEX: OnceLock<Mutex<Option<Indexed>>> = OnceLock::new();

fn valid_id(id: &str) -> Result<&str> {
    uuid::Uuid::parse_str(id)
        .ok()
        .filter(|value| value.to_string() == id)
        .map(|_| id)
        .ok_or_else(|| "词典 ID 无效".into())
}

fn read_library(path: &Path) -> Result<Library> {
    let data = fs::read(path.join("library.json")).map_err(|e| e.to_string())?;
    let library: Library = serde_json::from_slice(&data).map_err(|e| e.to_string())?;
    if library.files.is_empty()
        || library
            .files
            .iter()
            .any(|name| Path::new(name).file_name().and_then(|n| n.to_str()) != Some(name))
    {
        return Err("词典目录无效".into());
    }
    Ok(library)
}

fn discover(folder: &Path) -> Result<(String, Vec<String>, u64)> {
    let mut files: Vec<_> = fs::read_dir(folder)
        .map_err(|e| e.to_string())?
        .filter_map(|entry| {
            let entry = entry.ok()?;
            if !entry.file_type().ok()?.is_file() {
                return None;
            }
            entry.file_name().into_string().ok()
        })
        .collect();
    files.sort();
    let mut dictionaries = files
        .iter()
        .filter(|name| name.to_ascii_lowercase().ends_with(".mdx"));
    let mdx = dictionaries.next().ok_or("文件夹里没有 MDX 词典")?;
    if dictionaries.next().is_some() {
        return Err("请选择只包含一套 MDX 词典的文件夹".into());
    }
    let name = &mdx[..mdx.len() - 4];
    let prefix = format!("{}.", name.to_lowercase());
    let mut selected = vec![mdx.clone()];
    selected.extend(
        files
            .iter()
            .filter(|file| {
                let lower = file.to_lowercase();
                let Some(tail) = lower.strip_prefix(&prefix) else {
                    return false;
                };
                tail == "mdd"
                    || tail.strip_suffix(".mdd").is_some_and(|part| {
                        !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit())
                    })
            })
            .cloned(),
    );
    let mut size = 0u64;
    for file in &selected {
        size = size
            .checked_add(
                fs::metadata(folder.join(file))
                    .map_err(|e| e.to_string())?
                    .len(),
            )
            .ok_or("词典文件过大")?;
    }
    let display_name = folder
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(name);
    Ok((display_name.to_owned(), selected, size))
}

pub fn preview(folder: &Path) -> Result<Value> {
    let (name, files, size) = discover(folder)?;
    Ok(json!({"folder":folder,"name":name,"files":files,"size":size}))
}

fn index(dir: &Path, files: &[String]) -> Result<Indexed> {
    let words = Archive::open(&dir.join(&files[0]), false)
        .map_err(|error| format!("{} · {error}", files[0]))?;
    let resources = files[1..]
        .iter()
        .map(|name| {
            Archive::open(&dir.join(name), true).map_err(|error| format!("{name} · {error}"))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(Indexed {
        key: dir.to_owned(),
        words,
        resources,
    })
}

pub fn install(root: &Path, folder: &Path) -> Result<Value> {
    let (name, files, size) = discover(folder)?;
    fs::create_dir_all(root).map_err(|e| e.to_string())?;
    let id = uuid::Uuid::new_v4().to_string();
    let stage = root.join(format!(".{id}"));
    fs::create_dir(&stage).map_err(|e| e.to_string())?;
    let result = (|| {
        let mut copied = 0u64;
        for file in &files {
            copied += fs::copy(folder.join(file), stage.join(file))
                .map_err(|e| format!("复制词典失败，可能没有足够的磁盘空间（{e}）"))?;
        }
        if copied != size {
            return Err("导入时源文件大小发生变化，请重试".into());
        }
        let library = Library {
            id: id.clone(),
            name,
            files,
            size,
        };
        index(&stage, &library.files)?;
        fs::write(
            stage.join("library.json"),
            serde_json::to_vec(&library).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        fs::rename(&stage, root.join(&id)).map_err(|e| e.to_string())?;
        Ok(json!(library))
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(stage);
    }
    result
}

pub fn list(root: &Path) -> Result<Value> {
    if !root.exists() {
        return Ok(json!([]));
    }
    let mut libraries = Vec::new();
    for entry in fs::read_dir(root).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry.file_name();
        if name.to_str().is_some_and(|id| valid_id(id).is_ok()) {
            libraries.push(read_library(&entry.path())?);
        }
    }
    libraries.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
    Ok(json!(libraries))
}

pub fn remove(root: &Path, id: &str) -> Result<Value> {
    let dir = root.join(valid_id(id)?);
    let mut cache = INDEX
        .get_or_init(Default::default)
        .lock()
        .map_err(|_| "无法读取发音词典")?;
    read_library(&dir)?;
    let removed = root.join(format!(".removed-{id}"));
    fs::rename(&dir, &removed).map_err(|e| e.to_string())?;
    if cache.as_ref().is_some_and(|value| value.key == dir) {
        *cache = None;
    }
    fs::remove_dir_all(removed).map_err(|e| e.to_string())?;
    Ok(Value::Null)
}

fn with_index(
    root: &Path,
    id: &str,
    action: impl FnOnce(&Indexed) -> Result<Value>,
) -> Result<Value> {
    let dir = root.join(valid_id(id)?);
    let mut cache = INDEX
        .get_or_init(Default::default)
        .lock()
        .map_err(|_| "无法读取发音词典")?;
    if cache.as_ref().is_none_or(|value| value.key != dir) {
        *cache = Some(index(&dir, &read_library(&dir)?.files)?);
    }
    action(cache.as_ref().unwrap())
}

#[derive(Serialize, PartialEq)]
struct Sound {
    word: String,
    pos: String,
    region: &'static str,
    ipa: String,
    audio: Option<String>,
}

fn mp3(href: &str) -> Option<String> {
    let path = href.strip_prefix("sound://")?;
    (path.to_ascii_lowercase().ends_with(".mp3") && path.len() <= 1024).then(|| resource_key(path))
}

fn parse_entry(html: &str, headword: &str) -> Vec<Sound> {
    let doc = Document::from(html);
    let mut sounds = Vec::new();
    for row in doc.select(".phons_br, .phons_n_am").iter() {
        let word = row.attr_or("wd", headword).trim().to_lowercase();
        let pos = row
            .nodes()
            .first()
            .and_then(|node| {
                let mut parent = node.parent();
                while let Some(ancestor) = parent {
                    if let Some(pos) = ancestor
                        .children()
                        .into_iter()
                        .find(|child| child.has_class("pos"))
                    {
                        return Some(pos.text().trim().to_owned());
                    }
                    parent = ancestor.parent();
                }
                None
            })
            .unwrap_or_default();
        let ipa = row.select(".phon").first().text().trim().to_owned();
        let audio = row
            .select("a[href]")
            .attr("href")
            .and_then(|href| mp3(&href));
        if !ipa.is_empty() || audio.is_some() {
            let region = if row.has_class("phons_br") {
                "UK"
            } else {
                "US"
            };
            sounds.push(Sound {
                word,
                pos,
                region,
                ipa,
                audio,
            });
        }
    }
    if sounds.is_empty() {
        for row in doc.select(".hpron_word").iter() {
            let Some(node) = row.nodes().first() else {
                continue;
            };
            let Some(parent) = node.parent() else {
                continue;
            };
            let siblings = parent.children();
            let Some(i) = siblings.iter().position(|child| child.id == node.id) else {
                continue;
            };
            let ipa = row.text().trim().to_owned();
            if ipa.is_empty() {
                continue;
            }
            let british = i > 0 && siblings[i - 1].has_class("hpron_label_b");
            let region = if british { "UK" } else { "US" };
            let pos = siblings
                .iter()
                .find(|child| child.has_class("fl"))
                .map(|child| child.text().trim().to_owned())
                .unwrap_or_default();
            let audio = siblings.iter().find_map(|child| {
                let pron = child.attr("data-pron")?;
                let lang = child.attr("data-lang")?;
                if pron.as_ref() != ipa.trim_matches('/')
                    || lang.as_ref() != if british { "en_gb" } else { "en_us" }
                {
                    return None;
                }
                mp3(&child.attr("href")?)
            });
            sounds.push(Sound {
                word: headword.to_owned(),
                pos,
                region,
                ipa,
                audio,
            });
        }
    }
    sounds
}

fn entry(index: &Indexed, query: &str) -> Result<Vec<Sound>> {
    let mut key = query.to_owned();
    let mut seen = Vec::new();
    for _ in 0..12 {
        if seen.contains(&key) {
            break;
        }
        seen.push(key.clone());
        let Some(raw) = index.words.read(&key)? else {
            break;
        };
        let html = std::str::from_utf8(&raw)
            .map_err(|_| "词条编码无效")?
            .trim_end_matches('\0');
        if let Some(link) = html.strip_prefix("@@@LINK=") {
            key = link.trim().to_lowercase();
            continue;
        }
        let mut sounds = parse_entry(html, &key);
        let word = if sounds.iter().any(|sound| sound.word == query) {
            query
        } else {
            &key
        };
        sounds.retain(|sound| sound.word == word);
        let mut unique = Vec::new();
        for mut sound in sounds {
            sound.audio = sound
                .audio
                .filter(|path| index.resources.iter().any(|archive| archive.contains(path)));
            if let Some(existing) = unique.iter_mut().find(|existing: &&mut Sound| {
                existing.region == sound.region
                    && existing.ipa == sound.ipa
                    && existing.word == sound.word
                    && (existing.pos == sound.pos
                        || existing.pos.is_empty()
                        || sound.pos.is_empty())
            }) {
                if existing.audio.is_none() {
                    existing.audio = sound.audio;
                }
                if existing.pos.is_empty() {
                    existing.pos = sound.pos;
                }
            } else {
                unique.push(sound);
            }
        }
        return Ok(unique);
    }
    Ok(Vec::new())
}

pub fn lookup(root: &Path, id: &str, word: &str, base: &str) -> Result<Value> {
    with_index(root, id, |index| {
        let word = word.trim().to_lowercase();
        let base = base.trim().to_lowercase();
        let mut sounds = entry(index, &word)?;
        if sounds.is_empty() && !base.is_empty() && word != base {
            sounds = entry(index, &base)?;
        }
        Ok(json!(sounds))
    })
}

pub fn audio(root: &Path, id: &str, key: &str) -> Result<Value> {
    with_index(root, id, |index| {
        if !key.ends_with(".mp3") {
            return Err("录音格式不受支持".into());
        }
        for resource in &index.resources {
            if let Some(data) = resource.read(key)? {
                if data.len() > 4 * 1024 * 1024 {
                    return Err("录音文件过大".into());
                }
                return Ok(json!(STANDARD.encode(data)));
            }
        }
        Err("找不到词典录音".into())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_accent_wordform_and_pos_from_supported_dictionary_layouts() {
        let oxford = r#"<div><span class="pos">verb</span><span class="phonetics">
        <div class="phons_br" wd="assailed"><a href="sound://assailed__gb_1.mp3"></a><span class="phon">/əˈseɪld/</span></div>
        <div class="phons_n_am" wd="assailed"><a href="sound://assailed__us_1.mp3"></a><span class="phon">/əˈseɪld/</span></div>
        </span></div>"#;
        let rows = parse_entry(oxford, "assail");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].pos, "verb");
        assert_eq!(rows[0].word, "assailed");
        assert_eq!(rows[0].region, "UK");
        assert_eq!(rows[1].audio.as_deref(), Some("\\assailed__us_1.mp3"));

        let webster = r#"<div><span class="hpron_word">/təˈmeɪtoʊ/</span>
        <span class="hpron_label_b">Brit</span><span class="hpron_word">/təˈmɑːtəʊ/</span>
        <a data-pron="təˈmɑːtəʊ" data-lang="en_us" href="sound://tomato.mp3"></a>
        <span class="fl">noun</span></div>"#;
        let rows = parse_entry(webster, "tomato");
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|row| row.audio.is_none()));
    }

    #[test]
    #[ignore = "requires the user's local Oxford and Merriam-Webster files"]
    fn installs_and_reads_local_dictionary_samples() {
        let root = std::env::temp_dir().join(format!("semantic-dict-{}", uuid::Uuid::new_v4()));
        let result = (|| -> Result<()> {
            for variable in ["SEMANTIC_OXFORD_SAMPLE", "SEMANTIC_WEBSTER_SAMPLE"] {
                let source = std::env::var(variable).unwrap();
                let preview = preview(Path::new(&source))?;
                let library = install(&root, Path::new(&source))?;
                let id = library["id"].as_str().unwrap();
                assert_eq!(library["files"], preview["files"]);
                for file in library["files"].as_array().unwrap() {
                    let name = file.as_str().unwrap();
                    assert_eq!(
                        fs::metadata(root.join(id).join(name)).unwrap().len(),
                        fs::metadata(Path::new(&source).join(name)).unwrap().len()
                    );
                }
                let rows = lookup(&root, id, "assailed", "")?;
                let rows = rows.as_array().unwrap();
                let us = rows
                    .iter()
                    .find(|row| row["region"] == "US" && row["audio"].is_string())
                    .unwrap();
                let mp3 = audio(&root, id, us["audio"].as_str().unwrap())?;
                assert!(mp3.as_str().unwrap().len() > 100);
                if variable == "SEMANTIC_OXFORD_SAMPLE" {
                    assert_eq!(library["files"].as_array().unwrap().len(), 6);
                    assert_eq!(us["word"], "assailed");
                    assert!(rows
                        .iter()
                        .any(|row| row["region"] == "UK" && row["audio"].is_string()));
                    let record = lookup(&root, id, "record", "")?;
                    let record = record.as_array().unwrap();
                    for pos in ["noun", "verb"] {
                        for region in ["UK", "US"] {
                            assert!(
                                record.iter().any(|row| row["pos"] == pos
                                    && row["region"] == region
                                    && row["audio"].is_string()),
                                "{record:?}"
                            );
                        }
                    }
                } else {
                    assert_eq!(us["word"], "assail");
                }
                remove(&root, id)?;
                assert!(!root.join(id).exists());
            }
            Ok(())
        })();
        let _ = fs::remove_dir_all(root);
        result.unwrap();
    }
}
