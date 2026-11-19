PRAGMA journal_mode=WAL;
            CREATE TABLE IF NOT EXISTS photos (
                id INTEGER PRIMARY KEY, path TEXT UNIQUE NOT NULL, name TEXT NOT NULL,
                rating INTEGER NOT NULL DEFAULT 0, flag INTEGER NOT NULL DEFAULT 0,
                keywords TEXT NOT NULL DEFAULT '', edits TEXT NOT NULL, metadata TEXT NOT NULL DEFAULT '{}',
                imported TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS history (
                id INTEGER PRIMARY KEY, photo_id INTEGER NOT NULL,
                label TEXT NOT NULL, edits TEXT NOT NULL, created TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS presets (name TEXT PRIMARY KEY, edits TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS folder_roots (path TEXT PRIMARY KEY COLLATE NOCASE);
            CREATE TABLE IF NOT EXISTS preferences (key TEXT PRIMARY KEY, value TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS collections (id INTEGER PRIMARY KEY,name TEXT NOT NULL,parent_id INTEGER,rules TEXT);
            CREATE TABLE IF NOT EXISTS collection_members (collection_id INTEGER,photo_id INTEGER,PRIMARY KEY(collection_id,photo_id));
            CREATE TABLE IF NOT EXISTS snapshots (id INTEGER PRIMARY KEY,photo_id INTEGER,name TEXT,edits TEXT,created TEXT);
