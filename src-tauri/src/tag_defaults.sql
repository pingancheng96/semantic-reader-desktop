CREATE TABLE tag_defaults (
  id INTEGER PRIMARY KEY CHECK(id=1),
  pristine INTEGER NOT NULL CHECK(pristine IN (0,1))
);
PRAGMA user_version=7;
