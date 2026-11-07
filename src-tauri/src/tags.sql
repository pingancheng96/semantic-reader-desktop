CREATE TABLE tags (
  id TEXT PRIMARY KEY NOT NULL,
  name TEXT NOT NULL
);
CREATE TABLE tag_names (
  normalized TEXT PRIMARY KEY NOT NULL,
  name TEXT NOT NULL,
  tag_id TEXT NOT NULL REFERENCES tags(id) ON DELETE CASCADE
);
CREATE INDEX tag_names_tag ON tag_names(tag_id);
CREATE TABLE group_tags (
  group_id TEXT NOT NULL REFERENCES semantic_groups(id) ON DELETE CASCADE,
  tag_id TEXT NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
  PRIMARY KEY(group_id,tag_id)
);
CREATE INDEX group_tags_tag ON group_tags(tag_id,group_id);
PRAGMA user_version=5;
