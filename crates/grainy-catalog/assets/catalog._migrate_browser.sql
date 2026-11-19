CREATE TABLE IF NOT EXISTS browser_index (
                id INTEGER PRIMARY KEY,folder TEXT NOT NULL,folder_key TEXT NOT NULL,
                name_key TEXT NOT NULL,keywords_key TEXT NOT NULL,capture_key TEXT NOT NULL);
            CREATE INDEX IF NOT EXISTS browser_folder ON browser_index(folder_key COLLATE NOCASE);
            CREATE INDEX IF NOT EXISTS browser_name ON browser_index(name_key,id);
            CREATE INDEX IF NOT EXISTS browser_capture ON browser_index(capture_key,id);
            CREATE INDEX IF NOT EXISTS photos_rating ON photos(rating,id);
            CREATE INDEX IF NOT EXISTS photos_flag ON photos(flag,id);
            CREATE TABLE IF NOT EXISTS browser_dirty (id INTEGER PRIMARY KEY);
            CREATE TRIGGER IF NOT EXISTS browser_added AFTER INSERT ON photos BEGIN
                INSERT OR IGNORE INTO browser_dirty VALUES(NEW.id); END;
            CREATE TRIGGER IF NOT EXISTS browser_changed AFTER UPDATE OF path,name,keywords,metadata,extras ON photos BEGIN
                INSERT OR IGNORE INTO browser_dirty VALUES(NEW.id); END;
            CREATE TRIGGER IF NOT EXISTS browser_removed AFTER DELETE ON photos BEGIN
                DELETE FROM browser_index WHERE id=OLD.id;
                DELETE FROM browser_dirty WHERE id=OLD.id; END;
