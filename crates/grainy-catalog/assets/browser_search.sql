CREATE VIRTUAL TABLE browser_text USING fts5(
    name_key, keywords_key, tokenize='trigram case_sensitive 1'
);
INSERT INTO browser_text(rowid,name_key,keywords_key) SELECT id,name_key,keywords_key FROM browser_index;
CREATE INDEX browser_text_nul ON browser_index(id) WHERE instr(name_key,char(0)) OR instr(keywords_key,char(0));
CREATE INDEX browser_folder_counts ON browser_index(folder_key,folder);
CREATE TABLE browser_folder_totals(folder_key TEXT PRIMARY KEY, folder TEXT NOT NULL, count INTEGER NOT NULL) WITHOUT ROWID;
INSERT INTO browser_folder_totals SELECT folder_key,MIN(folder),COUNT(*) FROM browser_index GROUP BY folder_key;

CREATE TRIGGER browser_text_added AFTER INSERT ON browser_index BEGIN
    DELETE FROM browser_folder_totals;
    INSERT OR REPLACE INTO browser_text(rowid,name_key,keywords_key) VALUES(NEW.id,NEW.name_key,NEW.keywords_key);
END;
CREATE TRIGGER browser_text_changed AFTER UPDATE OF name_key,keywords_key ON browser_index BEGIN
    INSERT OR REPLACE INTO browser_text(rowid,name_key,keywords_key) VALUES(NEW.id,NEW.name_key,NEW.keywords_key);
END;
CREATE TRIGGER browser_text_removed AFTER DELETE ON browser_index BEGIN
    DELETE FROM browser_folder_totals;
    DELETE FROM browser_text WHERE rowid=OLD.id;
END;

CREATE TRIGGER browser_folder_changed AFTER UPDATE OF folder_key,folder ON browser_index BEGIN
    DELETE FROM browser_folder_totals;
END;
