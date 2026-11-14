use serde::Deserialize;
use serde_json::{json, Value};
use std::{collections::HashSet, sync::OnceLock};

#[derive(Deserialize)]
struct Domain {
    code: String,
    name: String,
    examples: String,
    definition: String,
}

fn domains() -> &'static [Domain] {
    static DATA: OnceLock<Vec<Domain>> = OnceLock::new();
    DATA.get_or_init(|| serde_json::from_str(include_str!("sil-v4.json")).expect("valid SIL catalog"))
}

pub fn valid(code: &str) -> bool {
    domains().iter().any(|domain| domain.code == code)
}

pub fn catalog() -> Value {
    json!(domains().iter().map(|d| json!({"code":d.code,"name":d.name})).collect::<Vec<_>>())
}

pub fn candidates(text: &str) -> Value {
    fn words(text: &str) -> impl Iterator<Item = &str> {
        text.split(|c: char| !c.is_ascii_alphabetic()).filter(|word| {
            word.len() >= 3 && !matches!(*word, "the"|"and"|"with"|"from"|"that"|"this"|"for"|"was"|"were"|"have"|"had"|"but"|"into")
        })
    }
    let text = text.to_ascii_lowercase();
    let query: HashSet<_> = words(&text).collect();
    let mut ranked = domains().iter().filter_map(|domain| {
        let name = domain.name.to_ascii_lowercase();
        let examples = domain.examples.to_ascii_lowercase();
        let score = words(&name).filter(|w| query.contains(w)).count() * 5
            + words(&examples).filter(|w| query.contains(w)).collect::<HashSet<_>>().len()
            + usize::from(name.len() > 3 && text.contains(&name)) * 8;
        (score > 0).then_some((score, domain))
    }).collect::<Vec<_>>();
    ranked.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
    json!(ranked.into_iter().take(8).map(|(_, d)| json!([d.code,d.name])).collect::<Vec<_>>())
}

pub fn validate_suggestion(result: &mut Value, candidates: &Value) {
    let allowed = result["sil_code"].as_str().is_some_and(|code|
        candidates.as_array().into_iter().flatten().any(|item| item[0] == code));
    if !allowed {
        result["sil_code"] = Value::Null;
    }
}


pub fn name(code: &str) -> Option<&'static str> {
    domains().iter().find(|domain| domain.code == code).map(|domain| domain.name.as_str())
}

pub fn children(parent: &str) -> Vec<Value> {
    domains().iter().filter(|d| {
        d.code.rsplit_once('.').map(|(base,_)| base).unwrap_or("") == parent
    }).map(|d| json!([d.code,d.name])).collect()
}

pub fn path(code: &str) -> String {
    let mut prefix = String::new();
    code.split('.').filter_map(|part| {
        if !prefix.is_empty() { prefix.push('.'); }
        prefix.push_str(part);
        name(&prefix)
    }).collect::<Vec<_>>().join(" › ")
}

pub fn review_options(lemma: &str, meaning: &str, parent: &str, limit: usize) -> Vec<Value> {
    let matches = |text: &str| candidates(text).as_array().cloned().unwrap_or_default();
    let mut codes = Vec::new();
    if ["米色", "黄色", "颜色", "色调"].iter().any(|term| meaning.contains(term)) {
        codes.extend(domains().iter().filter(|domain| domain.name == "Color")
            .map(|domain| json!([domain.code,domain.name])));
    }
    for item in matches(lemma).into_iter().take(2) {
        if !codes.contains(&item) { codes.push(item); }
    }
    for item in matches(&format!("{lemma} {meaning}")).into_iter().chain(matches(lemma)) {
        if !codes.contains(&item) { codes.push(item); }
    }
    let mut selected = codes.iter().take(limit.saturating_sub(usize::from(!parent.is_empty())))
        .filter_map(|item|item[0].as_str()).collect::<Vec<_>>();
    if !parent.is_empty() && !selected.contains(&parent) { selected.push(parent); }
    selected.into_iter().filter_map(|code| {
        let d = domains().iter().find(|item|item.code==code)?;
        Some(json!({"code":code,"path":path(code),"definition":d.definition}))
    }).collect()
}

pub fn option(code: &str) -> Option<Value> {
    domains().iter().find(|domain| domain.code==code).map(|domain|
        json!({"code":code,"path":path(code),"definition":domain.definition}))
}

pub fn compact_directory() -> (String, Vec<&'static str>) {
    let mut text = String::new();
    let mut codes = Vec::new();
    for (index, domain) in domains().iter().enumerate() {
        use std::fmt::Write;
        let indent = " ".repeat(domain.code.matches('.').count());
        writeln!(text, "{indent}d{} {}", index+1, domain.name).unwrap();
        codes.push(domain.code.as_str());
    }
    (text,codes)
}
