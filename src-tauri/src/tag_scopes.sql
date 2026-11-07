CREATE TABLE tag_scopes (
  tag_id TEXT PRIMARY KEY NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
  scope TEXT NOT NULL CHECK(length(trim(scope)) BETWEEN 1 AND 500),
  source_label TEXT NOT NULL,
  source_url TEXT NOT NULL
);
PRAGMA user_version=6;
