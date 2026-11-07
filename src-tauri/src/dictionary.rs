use crate::sqlite::{Db, Result};
use flate2::read::MultiGzDecoder;
use serde_json::{json, Value as V};
use std::{
    fs::File,
    io::{BufRead, BufReader, Read},
    path::Path,
};
use uuid::Uuid;

const SCHEMA: &str = "
CREATE TABLE entries (
  normalized TEXT NOT NULL, word TEXT NOT NULL, pos TEXT NOT NULL,
  region TEXT NOT NULL, ipa TEXT NOT NULL, labels TEXT NOT NULL,
  PRIMARY KEY(normalized,word,pos,region,ipa,labels)
) WITHOUT ROWID;
CREATE TABLE metadata (filename TEXT NOT NULL, words INTEGER NOT NULL);
";

#[derive(serde::Serialize, Clone)]
pub struct Progress {
    phase: &'static str,
    bytes: u64,
    total: Option<u64>,
}

const DOWNLOAD: &str = "https://kaikki.org/dictionary/English/kaikki.org-dictionary-English.jsonl";
const MAX_DOWNLOAD: u64 = 8_000_000_000;

pub struct Downloaded(std::path::PathBuf);
impl Downloaded {
    pub fn path(&self) -> &Path {
        &self.0
    }
}
impl Drop for Downloaded {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(self.0.parent().unwrap());
    }
}

pub async fn download(dir: &Path, progress: &tauri::ipc::Channel<Progress>) -> Result<Downloaded> {
    use std::time::{Duration, Instant};
    use tokio::io::AsyncWriteExt;
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(20))
        .read_timeout(Duration::from_secs(60))
        .redirect(reqwest::redirect::Policy::limited(3))
        .build()
        .map_err(|_| "无法初始化词典下载")?;
    let mut response = client
        .get(DOWNLOAD)
        .send()
        .await
        .map_err(|_| "无法连接 Kaikki 下载词典")?;
    if !response.status().is_success() {
        return Err(format!(
            "词典下载失败，Kaikki 返回 HTTP {}",
            response.status().as_u16()
        ));
    }
    let total = response.content_length();
    if total.is_some_and(|size| size > MAX_DOWNLOAD) {
        return Err("词典文件超过下载上限".into());
    }
    let directory = dir.join(format!(".wiktionary-download-{}", Uuid::new_v4()));
    tokio::fs::create_dir(&directory)
        .await
        .map_err(|_| "无法创建词典下载目录")?;
    let file = Downloaded(directory.join("kaikki.org-dictionary-English.jsonl"));
    let mut output = tokio::fs::File::create(file.path())
        .await
        .map_err(|_| "无法创建词典下载文件")?;
    let mut bytes = 0;
    let mut last = Instant::now();
    let _ = progress.send(Progress {
        phase: "download",
        bytes,
        total,
    });
    while let Some(chunk) = response.chunk().await.map_err(|_| "词典下载中断，请重试")? {
        bytes += chunk.len() as u64;
        if bytes > MAX_DOWNLOAD {
            return Err("词典文件超过下载上限".into());
        }
        output
            .write_all(&chunk)
            .await
            .map_err(|_| "无法写入词典下载文件，请检查剩余磁盘空间")?;
        if last.elapsed() >= Duration::from_millis(300) {
            let _ = progress.send(Progress {
                phase: "download",
                bytes,
                total,
            });
            last = Instant::now();
        }
    }
    output.flush().await.map_err(|_| "无法保存下载文件")?;
    if total.is_some_and(|total| total != bytes) || bytes == 0 {
        return Err("下载文件不完整，请重试".into());
    }
    Ok(file)
}

struct CountedReader(File, std::rc::Rc<std::cell::Cell<u64>>);
impl Read for CountedReader {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let size = self.0.read(buffer)?;
        self.1.set(self.1.get() + size as u64);
        Ok(size)
    }
}

pub fn has_pronunciation(result: &V) -> bool {
    result["entries"].as_array().is_some_and(|rows| {
        rows.iter()
            .any(|row| row["ipa"].as_str().is_some_and(|ipa| !ipa.is_empty()))
    })
}

pub fn resolve_fallback(
    surface: &str,
    lemma: &str,
    original: &V,
    headword: &V,
    suggested: &V,
) -> V {
    let has_entries = |value: &V| {
        value["entries"]
            .as_array()
            .is_some_and(|rows| !rows.is_empty())
    };
    let found = !lemma.eq_ignore_ascii_case(surface) && has_entries(headword);
    let use_ipa = !has_pronunciation(original) && !has_pronunciation(headword);
    let use_pos = !has_entries(original) && !has_entries(headword);
    let fallback = json!({
        "word":lemma,
        "ipa_uk":if use_ipa {suggested["ipa_uk"].clone()} else {V::Null},
        "ipa_us":if use_ipa {suggested["ipa_us"].clone()} else {V::Null},
        "part_of_speech":if use_pos {suggested["part_of_speech"].clone()} else {V::Null}
    });
    let available = ["ipa_uk", "ipa_us", "part_of_speech"]
        .iter()
        .any(|key| fallback[key].is_string());
    json!({"lemma":found.then_some(lemma),"dictionary":found.then_some(headword),"fallback":available.then_some(fallback)})
}

fn normalized(word: &str) -> String {
    word.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

pub fn status(path: &Path) -> Result<V> {
    if !path.is_file() {
        return Ok(V::Null);
    }
    Db::open(path)?.one("SELECT filename,words FROM metadata", &[])
}

pub fn lookup(path: &Path, word: &str) -> Result<V> {
    if !path.is_file() {
        return Ok(json!({"installed":false,"entries":[]}));
    }
    let rows = Db::open(path)?.query(
        "SELECT word,pos,region,ipa,labels FROM entries WHERE normalized=?1
         ORDER BY (word<>?2),word,pos,region,ipa,labels LIMIT 160",
        &[json!(normalized(word)), json!(word)],
    )?;
    let exact_spelling = rows.iter().any(|entry| entry["word"] == word);
    let rows: Vec<_> = rows
        .into_iter()
        .filter(|entry| !exact_spelling || entry["word"] == word)
        .collect();
    Ok(json!({"installed":true,"entries":rows}))
}

pub fn install(path: &Path, input: &Path, mut progress: impl FnMut(Progress)) -> Result<V> {
    let file = File::open(input).map_err(|_| "无法读取 Wiktionary 数据文件")?;
    let total = file.metadata().ok().map(|m| m.len());
    let bytes = std::rc::Rc::new(std::cell::Cell::new(0));
    let mut reader = BufReader::new(CountedReader(file, bytes.clone()));
    progress(Progress {
        phase: "index",
        bytes: 0,
        total,
    });
    let gzip = reader
        .fill_buf()
        .map_err(|_| "无法读取 Wiktionary 文件")?
        .starts_with(&[0x1f, 0x8b]);
    let reader: Box<dyn BufRead> = if gzip {
        Box::new(BufReader::new(MultiGzDecoder::new(reader)))
    } else {
        Box::new(reader)
    };
    let filename = input
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("Wiktionary JSONL");
    let temporary = path.with_file_name(format!(".wiktionary-{}.sqlite3", Uuid::new_v4()));
    let result = (|| {
        let db = Db::open(&temporary)?;
        db.batch(SCHEMA)?;
        db.transaction(|| {
            ingest(&db, reader, filename, || {
                progress(Progress {
                    phase: "index",
                    bytes: bytes.get(),
                    total,
                });
            })
        })?;
        let metadata = db.one("SELECT filename,words FROM metadata", &[])?;
        drop(db);
        std::fs::rename(&temporary, path).map_err(|_| "无法替换本地词典")?;
        progress(Progress {
            phase: "index",
            bytes: bytes.get(),
            total,
        });
        Ok(metadata)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    result
}

fn ingest(db: &Db, mut reader: impl BufRead, filename: &str, mut tick: impl FnMut()) -> Result<()> {
    let mut line = Vec::new();
    let mut number = 0usize;
    loop {
        line.clear();
        if reader
            .by_ref()
            .take(32_000_001)
            .read_until(b'\n', &mut line)
            .map_err(|_| "词典数据读取失败，文件可能不完整")?
            == 0
        {
            break;
        }
        number += 1;
        if number % 2_000 == 0 {
            tick();
        }
        if line.len() > 32_000_000 {
            return Err(format!("词典数据第 {number} 行过长"));
        }
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let entry: V =
            serde_json::from_slice(&line).map_err(|_| format!("词典数据第 {number} 行无效"))?;
        if entry["lang_code"] != "en" {
            continue;
        }
        let Some(word) = entry["word"].as_str().filter(|s| valid(s, 240)) else {
            continue;
        };
        let Some(pos) = entry["pos"].as_str().filter(|s| valid(s, 40)) else {
            continue;
        };
        let key = normalized(word);
        db.query(
            "INSERT OR IGNORE INTO entries VALUES(?1,?2,?3,'','','')",
            &[json!(key), json!(word), json!(pos)],
        )?;
        for sound in entry["sounds"].as_array().into_iter().flatten() {
            let Some(ipa) = sound["ipa"].as_str().filter(|s| {
                valid(s, 160)
                    && ((s.starts_with('/') && s.ends_with('/'))
                        || (s.starts_with('[') && s.ends_with(']')))
            }) else {
                continue;
            };
            // A sound restricted to a different surface form does not belong to this headword.
            if sound["form"]
                .as_str()
                .is_some_and(|form| normalized(form) != key)
            {
                continue;
            }
            let labels: Vec<_> = sound["tags"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(V::as_str)
                .filter(|tag| valid(tag, 64))
                .take(12)
                .collect();
            let uk = labels.iter().any(|label| {
                matches!(
                    *label,
                    "UK" | "Received-Pronunciation" | "British" | "England" | "Southern-England"
                )
            });
            let us = labels
                .iter()
                .any(|label| matches!(*label, "US" | "General-American" | "American"));
            let label = labels.join(" · ");
            for (region, include) in [("UK", uk), ("US", us), ("other", !uk && !us)] {
                if include {
                    db.query(
                        "INSERT OR IGNORE INTO entries VALUES(?1,?2,?3,?4,?5,?6)",
                        &[
                            json!(key),
                            json!(word),
                            json!(pos),
                            json!(region),
                            json!(ipa),
                            json!(label),
                        ],
                    )?;
                }
            }
        }
    }
    let count = db.one(
        "SELECT count(DISTINCT normalized) AS words FROM entries",
        &[],
    )?["words"]
        .clone();
    if count == 0 {
        return Err("文件没有可导入的英语 Wiktextract 词条".into());
    }
    db.write(
        "INSERT INTO metadata VALUES(?1,?2)",
        &[json!(filename), count],
    )
}

fn valid(value: &str, max: usize) -> bool {
    !value.trim().is_empty() && value.chars().count() <= max && !value.chars().any(char::is_control)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn dictionary_headword_takes_priority_and_ai_fills_only_missing_data() {
        let empty = json!({"entries":[]});
        let sound = json!({"entries":[{"ipa":"/dɑːns/","pos":"verb"}]});
        let pos = json!({"entries":[{"ipa":"","pos":"verb"}]});
        let ai = json!({"ipa_uk":"/dɑːns/","ipa_us":"/dæns/","part_of_speech":"动词"});
        let hit = resolve_fallback("danced", "dance", &empty, &sound, &ai);
        assert_eq!(hit["lemma"], "dance");
        assert_eq!(hit["dictionary"]["entries"][0]["ipa"], "/dɑːns/");
        assert!(hit["fallback"].is_null());
        let partial = resolve_fallback("danced", "dance", &empty, &pos, &ai);
        assert_eq!(partial["fallback"]["ipa_us"], "/dæns/");
        assert!(partial["fallback"]["part_of_speech"].is_null());
        let miss = resolve_fallback("danced", "dance", &empty, &empty, &ai);
        assert!(miss["lemma"].is_null());
        assert_eq!(miss["fallback"]["part_of_speech"], "动词");
    }

    #[test]
    fn imports_gzip_preserves_pos_and_accent_variants_and_keeps_previous_on_error() {
        let dir = std::env::temp_dir().join(format!("semantic-dictionary-{}", Uuid::new_v4()));
        std::fs::create_dir(&dir).unwrap();
        let input = dir.join("sample.jsonl.gz");
        let mut gzip = flate2::write::GzEncoder::new(
            File::create(&input).unwrap(),
            flate2::Compression::fast(),
        );
        writeln!(
            gzip,
            "{}",
            json!({"lang_code":"en","word":"record","pos":"noun","sounds":[
                {"ipa":"/ˈɹɛkɔːd/","tags":["Received-Pronunciation"]},
                {"ipa":"/ˈɹɛkɚd/","tags":["General-American"]},
                {"ipa":"/ˈɹɛkɔɹd/","tags":["US"]},
                {"ipa":"/bad/","tags":["UK"],"form":"records"}
            ]})
        )
        .unwrap();
        writeln!(gzip,"{}",json!({"lang_code":"en","word":"record","pos":"verb","sounds":[{"ipa":"/ɹɪˈkɔːd/","tags":["UK"]}]})).unwrap();
        writeln!(
            gzip,
            "{}",
            json!({"lang_code":"fr","word":"record","pos":"noun"})
        )
        .unwrap();
        gzip.finish().unwrap();
        let path = dir.join("dictionary.sqlite3");
        assert_eq!(status(&path).unwrap(), V::Null);
        let mut events = Vec::new();
        assert_eq!(
            install(&path, &input, |event| events.push(event)).unwrap()["words"],
            1
        );
        assert_eq!(events[0].bytes, 0);
        assert_eq!(
            events.last().unwrap().total,
            Some(events.last().unwrap().bytes)
        );
        let result = lookup(&path, "record").unwrap();
        let entries = result["entries"].as_array().unwrap();
        assert_eq!(
            entries.iter().filter(|row| row["region"] == "US").count(),
            2
        );
        assert_eq!(
            entries.iter().filter(|row| row["region"] == "UK").count(),
            2
        );
        assert!(entries.iter().any(|row| row["pos"] == "verb"));
        std::fs::write(&input, "invalid").unwrap();
        assert!(install(&path, &input, |_| {}).is_err());
        assert_eq!(lookup(&path, "record").unwrap(), result);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
