CREATE TABLE IF NOT EXISTS preview_state(photo_id INTEGER PRIMARY KEY,token TEXT NOT NULL);
        CREATE TRIGGER IF NOT EXISTS preview_added AFTER INSERT ON photos BEGIN
            INSERT OR REPLACE INTO preview_state VALUES(NEW.id,lower(hex(randomblob(16)))); END;
        CREATE TRIGGER IF NOT EXISTS preview_changed AFTER UPDATE OF edits,path ON photos
            WHEN OLD.edits != NEW.edits OR OLD.path != NEW.path BEGIN
            INSERT OR REPLACE INTO preview_state VALUES(NEW.id,lower(hex(randomblob(16)))); END;
        CREATE TRIGGER IF NOT EXISTS preview_removed AFTER DELETE ON photos BEGIN
            DELETE FROM preview_state WHERE photo_id=OLD.id; END;
        INSERT OR IGNORE INTO preview_state SELECT id,lower(hex(randomblob(16))) FROM photos;
