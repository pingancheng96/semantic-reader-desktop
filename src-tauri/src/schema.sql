CREATE TABLE documents (
  id TEXT PRIMARY KEY, title TEXT NOT NULL, format TEXT NOT NULL CHECK(format IN ('md','txt')),
  content TEXT NOT NULL, created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP, source_url TEXT, web_layout TEXT
);
CREATE TABLE semantic_groups (id TEXT PRIMARY KEY, name TEXT NOT NULL, description TEXT NOT NULL);
CREATE TABLE group_vocabulary (
  group_id TEXT REFERENCES semantic_groups(id) ON DELETE CASCADE,
  expression TEXT NOT NULL, nuance TEXT NOT NULL DEFAULT '',
  PRIMARY KEY(group_id, expression)
);
CREATE TABLE expressions (id TEXT PRIMARY KEY, lemma TEXT NOT NULL, normalized TEXT NOT NULL UNIQUE);
CREATE TABLE senses (
  id TEXT PRIMARY KEY, expression_id TEXT NOT NULL REFERENCES expressions(id),
  group_id TEXT NOT NULL REFERENCES semantic_groups(id), meaning TEXT NOT NULL,
  UNIQUE(group_id, expression_id, meaning)
);
CREATE TABLE occurrences (
  id TEXT PRIMARY KEY, document_id TEXT NOT NULL REFERENCES documents(id),
  sense_id TEXT NOT NULL REFERENCES senses(id), start INTEGER NOT NULL, end INTEGER NOT NULL,
  surface TEXT NOT NULL, context TEXT NOT NULL, learning TEXT, CHECK(start >= 0 AND end > start),
  UNIQUE(document_id, start, end)
);
CREATE INDEX senses_group ON senses(group_id);
CREATE INDEX senses_expression ON senses(expression_id);
CREATE INDEX occurrences_sense ON occurrences(sense_id);
CREATE INDEX occurrences_surface ON occurrences(surface COLLATE NOCASE);
CREATE INDEX occurrences_context ON occurrences(surface, context);
CREATE TABLE operation_history (id INTEGER PRIMARY KEY, payload TEXT NOT NULL);
CREATE VIRTUAL TABLE group_search USING fts5(group_id UNINDEXED, body, tokenize='porter unicode61');
PRAGMA user_version=4;
