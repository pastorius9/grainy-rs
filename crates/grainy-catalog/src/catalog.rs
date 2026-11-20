use crate::{
    folders::Platform, integer, invalid, json, object, profiles, row_value, schema, settings, sql_value, string, Result,
};
use grainy_core::Settings;
use rusqlite::{params, Connection, OpenFlags, OptionalExtension, Params};
use serde_json::{json, Map, Value};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

/// Injectable clock, preview token source and path rules for `Catalog` writers.
#[derive(Clone)]
pub struct Sources {
    pub(crate) clock: Arc<dyn Fn() -> String + Send + Sync>,
    pub(crate) token: Option<Arc<dyn Fn() -> String + Send + Sync>>,
    pub(crate) platform: Platform,
}
impl Default for Sources {
    fn default() -> Self {
        Self {
            clock: Arc::new(|| chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Micros, false)),
            token: None,
            platform: Platform::native(),
        }
    }
}
impl Sources {
    /// Deterministic sources for oracle parity tests; production uses SQLite randomblob tokens.
    pub fn new(
        clock: impl Fn() -> String + Send + Sync + 'static,
        token: impl Fn() -> String + Send + Sync + 'static,
    ) -> Self {
        Self { clock: Arc::new(clock), token: Some(Arc::new(token)), platform: Platform::native() }
    }
    /// The path rules of another system instead of this one's ([`Platform::native`]): parity tests replay a
    /// session the oracle ran on Windows (drive letters, backslashes) on every system.
    pub fn on(mut self, platform: Platform) -> Self {
        self.platform = platform;
        self
    }
}

/// Port of `luma/catalog.py Catalog`. Connections are `Send`, not shared between threads.
pub struct Catalog {
    pub(crate) db: Connection,
    pub(crate) directory: PathBuf,
    pub(crate) platform: Platform,
    pub(crate) sources: Sources,
    reader: bool,
}
impl Catalog {
    /// Port of `Catalog.__init__`, including WAL, migrations and historical folder registration.
    pub fn new(directory: impl AsRef<Path>) -> Result<Self> {
        Self::with_sources(directory, Sources::default())
    }
    /// Port of `Catalog.__init__` with injectable clock/id sources for deterministic writes.
    pub fn with_sources(directory: impl AsRef<Path>, sources: Sources) -> Result<Self> {
        let directory = std::path::absolute(directory)?;
        std::fs::create_dir_all(directory.join("thumbnails"))?;
        let db = Connection::open(directory.join("catalog.sqlite"))?;
        db.busy_timeout(Duration::from_secs(5))?;
        let c = Self { db, directory, platform: sources.platform, sources, reader: false };
        schema::migrate(&c)?;
        Ok(c)
    }
    /// Port of `Catalog.open_reader`; no migration, directory creation, or write connection.
    /// `immutable=1` avoids creating WAL sidecars for closed catalogs; live WAL uses SQLite read-only mode.
    pub fn open_reader(directory: impl AsRef<Path>) -> Result<Self> {
        let directory = std::path::absolute(directory)?;
        let path = directory.join("catalog.sqlite");
        let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let db = if directory.join("catalog.sqlite-wal").exists() {
            Connection::open_with_flags(&path, flags)?
        } else {
            let name =
                crate::path_text(&path).replace('\\', "/").replace('%', "%25").replace('?', "%3f").replace('#', "%23");
            let uri = format!("file:{}{}?immutable=1", if name.starts_with("//") { "//" } else { "" }, name);
            Connection::open_with_flags(uri, flags)?
        };
        db.busy_timeout(Duration::from_secs(2))?;
        db.execute_batch("BEGIN")?;
        Ok(Self { db, directory, platform: Platform::native(), sources: Sources::default(), reader: true })
    }
    /// The oracle's `catalog.db`: the SQLite connection, for the raw statements `photo_service`, `library` and
    /// the other catalogue-level modules run. Writers must go through [`Catalog::transaction`].
    pub fn connection(&self) -> &Connection {
        &self.db
    }
    /// The catalogue folder (`catalog.directory`).
    pub fn directory(&self) -> &Path {
        &self.directory
    }
    /// Path rules of this machine (`luma/folders.py`).
    pub fn platform(&self) -> Platform {
        self.platform
    }
    /// True for a connection from [`Catalog::open_reader`].
    pub fn is_reader(&self) -> bool {
        self.reader
    }
    /// All rows of a query as JSON objects.
    pub fn rows<P: Params>(&self, sql: &str, params: P) -> Result<Vec<Value>> {
        let mut statement = self.db.prepare(sql)?;
        let rows = statement.query_map(params, row_value)?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }
    /// `with catalog.db:` — commit when `f` succeeds, roll back when it fails.
    pub fn transaction<T>(&self, f: impl FnOnce() -> Result<T>) -> Result<T> {
        self.transaction_mode("BEGIN", f)
    }
    /// Port of command_batch's BEGIN IMMEDIATE with rollback on failure.
    pub fn immediate<T>(&self, f: impl FnOnce() -> Result<T>) -> Result<T> {
        self.transaction_mode("BEGIN IMMEDIATE", f)
    }
    fn transaction_mode<T>(&self, begin: &str, f: impl FnOnce() -> Result<T>) -> Result<T> {
        if self.reader {
            return Err(invalid("catalog is read-only"));
        }
        crate::transaction::run(&self.db, begin == "BEGIN IMMEDIATE", f)
    }
    /// The injected clock: `datetime.now(timezone.utc).isoformat()`.
    pub fn now(&self) -> String {
        (self.sources.clock)()
    }
    fn retoken(&self, id: i64) -> Result<()> {
        if let Some(f) = &self.sources.token {
            self.db.execute("UPDATE preview_state SET token=? WHERE photo_id=?", params![f(), id])?;
        }
        Ok(())
    }
    pub(crate) fn count_photos(&self) -> Result<i64> {
        Ok(self.db.query_row("SELECT COUNT(*) FROM photos", [], |r| r.get(0))?)
    }
    fn path(&self, path: &str, resolved: bool) -> Result<String> {
        let p = if resolved { path.into() } else { crate::path_text(&crate::resolved_path(Path::new(path))?) };
        Ok(self.platform.stored_path(&self.platform.normalized(&p, !resolved)))
    }
    /// Port of `Catalog.add`; duplicate originals use SQLite NOCASE, virtual copies remain distinct.
    pub fn add(
        &self,
        path: &str,
        settings_value: Option<&Settings>,
        metadata: Option<&Value>,
        resolved: bool,
    ) -> Result<i64> {
        let path = self.path(path, resolved)?;
        if let Some(id) = self
            .db
            .query_row("SELECT id FROM photos WHERE path=? COLLATE NOCASE AND virtual_source IS NULL", [&path], |r| {
                r.get(0)
            })
            .optional()?
        {
            return Ok(id);
        }
        let values = settings(&settings_value.map(|s| Value::Object(s.0.clone())).unwrap_or(json!({})))?;
        self.transaction(|| {
            let folder = self.platform.parent(&path);
            if !self.folder_roots()?.iter().any(|r| self.platform.contains_folder(r, &folder)) {
                self.db.execute("INSERT OR IGNORE INTO folder_roots VALUES(?)", [folder])?;
            }
            let payload = profiles::dumps(&self.db, &values.0.into())?;
            self.db.execute(
                "INSERT INTO photos(path,name,edits,metadata,imported) VALUES(?,?,?,?,?)",
                params![
                    path,
                    self.platform.name(&path),
                    payload,
                    json::dumps(metadata.unwrap_or(&json!({})), true, false, false),
                    self.now()
                ],
            )?;
            let id = self.db.last_insert_rowid();
            self.retoken(id)?;
            Ok(id)
        })
    }
    /// Port of `Catalog.photos`; raw JSON columns are retained exactly.
    pub fn photos(&self) -> Result<Vec<Value>> {
        self.rows("SELECT * FROM photos ORDER BY id", [])
    }
    /// Port of `Catalog.paths`.
    pub fn paths(&self) -> Result<Vec<String>> {
        self.rows("SELECT DISTINCT path FROM photos", [])?.iter().map(|r| Ok(string(&r["path"])?.into())).collect()
    }
    /// Port of `Catalog.photo`; settings are expanded and normalized, other columns remain raw.
    pub fn photo(&self, id: i64) -> Result<Option<Value>> {
        let Some(mut row) = self.rows("SELECT * FROM photos WHERE id=?", [id])?.pop() else { return Ok(None) };
        row["settings"] = settings(&profiles::loads(&self.db, string(&row["edits"])?)?)?.0.into();
        row["info"] = serde_json::from_str(string(&row["metadata"])?)?;
        row["user_metadata"] = serde_json::from_str(row["extras"].as_str().filter(|s| !s.is_empty()).unwrap_or("{}"))?;
        Ok(Some(row))
    }
    /// Port of `Catalog.update_many`; unsupported field names are rejected before SQL preparation.
    pub fn update_many(&self, ids: &[i64], field: &str, value: &Value) -> Result<()> {
        if !["rating", "flag", "label", "stack_id"].contains(&field) {
            return Err(invalid("unsupported bulk field"));
        }
        let v = sql_value(value)?;
        self.transaction(|| {
            for id in ids {
                self.db.execute(&format!("UPDATE photos SET {field}=? WHERE id=?"), params![v, id])?;
            }
            Ok(())
        })
    }
    /// Port of `Catalog.update`; unknown fields are ignored.
    pub fn update(&self, id: i64, fields: &Map<String, Value>) -> Result<()> {
        let allowed = ["rating", "flag", "keywords", "metadata", "label", "stack_id", "extras", "name"];
        let fields = fields.iter().filter(|(k, _)| allowed.contains(&k.as_str())).collect::<Vec<_>>();
        if fields.is_empty() {
            return Ok(());
        }
        let mut values = fields.iter().map(|(_, v)| sql_value(v)).collect::<Result<Vec<_>>>()?;
        values.push(id.into());
        self.transaction(|| {
            self.db.execute(
                &format!(
                    "UPDATE photos SET {} WHERE id=?",
                    fields.iter().map(|(k, _)| format!("{k}=?")).collect::<Vec<_>>().join(",")
                ),
                rusqlite::params_from_iter(values),
            )?;
            Ok(())
        })
    }
    /// Port of `Catalog.edit`; exact no-op edits do not add history, timestamps or preview tokens.
    pub fn edit(&self, id: i64, value: &Settings, label: &str) -> Result<()> {
        let values = settings(&Value::Object(value.0.clone()))?;
        let Some(p) = self.photo(id)? else { return Ok(()) };
        if json::equivalent(&p["settings"], &Value::Object(values.0.clone())) {
            return Ok(());
        }
        self.transaction_mode("BEGIN IMMEDIATE", || {
            let payload = profiles::dumps(&self.db, &values.0.into())?;
            self.db.execute_batch("SAVEPOINT previous_profile")?;
            let old: Value = serde_json::from_str(string(&p["edits"])?)?;
            let previous = match profiles::pack(&self.db, &old, true) {
                Ok(v) => json::dumps(&v, true, false, false),
                Err(crate::Error::Invalid(_) | crate::Error::Json(_)) => {
                    self.db.execute_batch("ROLLBACK TO previous_profile")?;
                    string(&p["edits"])?.into()
                }
                Err(e) => return Err(e),
            };
            self.db.execute_batch("RELEASE previous_profile")?;
            self.db.execute(
                "INSERT INTO history(photo_id,label,edits,created) VALUES(?,?,?,?)",
                params![id, label, previous, self.now()],
            )?;
            let mut meta = p["user_metadata"].clone();
            object(&meta)?;
            meta["_last_edit_time"] = self.now().into();
            self.db.execute(
                "UPDATE photos SET edits=?,extras=? WHERE id=?",
                params![payload, json::dumps(&meta, false, false, false), id],
            )?;
            self.retoken(id)
        })
    }
    /// Port of `Catalog.set_settings`; updates settings without adding a history row.
    pub fn set_settings(&self, id: i64, value: &Settings) -> Result<()> {
        let Some(p) = self.photo(id)? else { return Ok(()) };
        let values = settings(&Value::Object(value.0.clone()))?;
        if json::equivalent(&p["settings"], &Value::Object(values.0.clone())) {
            return Ok(());
        }
        self.transaction(|| {
            let mut meta = p["user_metadata"].clone();
            object(&meta)?;
            meta["_last_edit_time"] = self.now().into();
            self.db.execute(
                "UPDATE photos SET edits=?,extras=? WHERE id=?",
                params![profiles::dumps(&self.db, &values.0.into())?, json::dumps(&meta, false, false, false), id],
            )?;
            self.retoken(id)
        })
    }
    fn settings_rows(&self, sql: &str, id: i64) -> Result<Vec<Value>> {
        let mut rows = self.rows(sql, [id])?;
        for r in &mut rows {
            r["settings"] = profiles::loads(&self.db, string(&r["edits"])?)?;
        }
        Ok(rows)
    }
    /// Port of `Catalog.histories`; returns at most 80 entries, newest first, without normalization.
    pub fn histories(&self, id: i64) -> Result<Vec<Value>> {
        self.settings_rows("SELECT * FROM history WHERE photo_id=? ORDER BY id DESC LIMIT 80", id)
    }
    /// Port of `Catalog.save_preset`; photo-specific geometry is reset to defaults.
    pub fn save_preset(&self, name: &str, value: &Settings) -> Result<()> {
        let mut v = settings(&Value::Object(value.0.clone()))?;
        let defaults = grainy_core::defaults();
        for key in ["crop", "rotation", "straighten", "flip"] {
            v.set(key, defaults.value(key).clone());
        }
        self.transaction(|| {
            self.db.execute(
                "INSERT OR REPLACE INTO presets VALUES(?,?)",
                params![name, profiles::dumps(&self.db, &v.0.into())?],
            )?;
            Ok(())
        })
    }
    /// Port of `Catalog.save_adobe_preset` storage boundary. `translate` is the Adobe importer adapter:
    /// it must validate the preserved source and return `settings` and nonempty `mapped`, as the oracle does.
    pub fn save_adobe_preset(
        &self,
        name: &str,
        preset: &Value,
        translate: impl FnOnce(&Value) -> Result<Value>,
    ) -> Result<()> {
        let record = preset.get("_adobe_preset").ok_or_else(|| invalid("missing Adobe preset source"))?;
        let checked = translate(record)?;
        if checked["mapped"].as_array().is_none_or(Vec::is_empty) {
            return Err(invalid("no convertible preset adjustments"));
        }
        let mut value = object(&checked["settings"])?.clone();
        value.insert("_adobe_preset".into(), record.clone());
        self.transaction(|| {
            self.db.execute(
                "INSERT OR REPLACE INTO presets VALUES(?,?)",
                params![name, profiles::dumps(&self.db, &value.into())?],
            )?;
            Ok(())
        })
    }
    /// Port of `Catalog.presets`.
    pub fn presets(&self) -> Result<Value> {
        let mut result = Map::new();
        for r in self.rows("SELECT * FROM presets ORDER BY name", [])? {
            result.insert(string(&r["name"])?.into(), profiles::loads(&self.db, string(&r["edits"])?)?);
        }
        Ok(result.into())
    }
    /// Port of `Catalog.delete_preset`.
    pub fn delete_preset(&self, name: &str) -> Result<()> {
        self.transaction(|| {
            self.db.execute("DELETE FROM presets WHERE name=?", [name])?;
            Ok(())
        })
    }
    /// Port of `Catalog.folder_roots`.
    pub fn folder_roots(&self) -> Result<Vec<String>> {
        self.rows("SELECT path FROM folder_roots ORDER BY path COLLATE NOCASE", [])?
            .iter()
            .map(|r| Ok(string(&r["path"])?.into()))
            .collect()
    }
    /// Port of `Catalog.register_folder`, including exact removed-folder clearing.
    pub fn register_folder(&self, path: &str, resolved: bool) -> Result<String> {
        let path = self.path(path, resolved)?;
        self.transaction(|| {
            self.db.execute("INSERT OR IGNORE INTO folder_roots VALUES(?)", [&path])?;
            Ok(())
        })?;
        let removed = self.removed_folders()?;
        let kept = removed
            .iter()
            .filter(|p| self.platform.path_key(p) != self.platform.path_key(&path))
            .cloned()
            .collect::<Vec<_>>();
        if kept.len() != removed.len() {
            self.save_preference("removed_folders", &json!(kept))?;
        }
        Ok(path)
    }
    /// Port of `Catalog.removed_folders`.
    pub fn removed_folders(&self) -> Result<Vec<String>> {
        let v = self.preference("removed_folders", json!([]))?;
        if v.is_null() {
            return Ok(vec![]);
        }
        v.as_array()
            .ok_or_else(|| invalid("removed_folders must be an array"))?
            .iter()
            .map(|v| Ok(string(v)?.into()))
            .collect()
    }
    /// Port of `Catalog.folder_photo_ids`.
    pub fn folder_photo_ids(&self, folder: &str) -> Result<Vec<i64>> {
        self.rows("SELECT id,path FROM photos", [])?
            .iter()
            .filter(|p| self.platform.in_folder(p["path"].as_str().unwrap_or(""), folder, true))
            .map(|p| integer(&p["id"]))
            .collect()
    }
    /// Port of `Catalog.remove_folder`; source files and cached preview files are untouched.
    pub fn remove_folder(&self, folder: &str) -> Result<Vec<i64>> {
        let folder = self.path(folder, true)?;
        let ids = self.folder_photo_ids(&folder)?;
        let mut removed = self
            .removed_folders()?
            .into_iter()
            .filter(|p| !self.platform.contains_folder(&folder, p))
            .collect::<Vec<_>>();
        removed.push(folder.clone());
        let watches = self.preference("watch_folders", json!([]))?;
        let watches = watches
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|v| !self.platform.contains_folder(&folder, v.as_str().unwrap_or("")))
            .collect::<Vec<_>>();
        self.transaction(|| {
            self.delete_photos(&ids)?;
            for root in self.folder_roots()? {
                if self.platform.contains_folder(&folder, &root) {
                    self.db.execute("DELETE FROM folder_roots WHERE path=?", [root])?;
                }
            }
            for (k, v) in [("removed_folders", json!(removed)), ("watch_folders", json!(watches))] {
                self.db.execute(
                    "INSERT OR REPLACE INTO preferences VALUES(?,?)",
                    params![k, json::dumps(&v, false, false, false)],
                )?;
            }
            Ok(())
        })?;
        Ok(ids)
    }
    /// Port of `Catalog.restore_folder`.
    pub fn restore_folder(&self, folder: &str) -> Result<()> {
        let removed = self.removed_folders()?;
        let kept = removed.iter().filter(|p| !self.platform.contains_folder(folder, p)).cloned().collect::<Vec<_>>();
        if kept.len() != removed.len() {
            self.save_preference("removed_folders", &json!(kept))?;
        }
        Ok(())
    }
    /// Port of `Catalog.preference`; expands profile references even in unknown preferences.
    pub fn preference(&self, key: &str, default: Value) -> Result<Value> {
        let text: Option<String> =
            self.db.query_row("SELECT value FROM preferences WHERE key=?", [key], |r| r.get(0)).optional()?;
        match text {
            Some(s) => profiles::loads(&self.db, &s),
            None => Ok(default),
        }
    }
    /// Port of `Catalog.save_preference`; undo and RAW defaults payloads pack profile assets.
    pub fn save_preference(&self, key: &str, value: &Value) -> Result<()> {
        self.transaction(|| {
            let payload = if key.starts_with("undo:") || key == "raw_defaults_v1" {
                profiles::dumps(&self.db, value)?
            } else {
                json::dumps(value, true, false, false)
            };
            self.db.execute("INSERT OR REPLACE INTO preferences VALUES(?,?)", params![key, payload])?;
            Ok(())
        })
    }
    /// Port of `Catalog.close`; pending read transactions end when the connection closes.
    pub fn close(self) -> Result<()> {
        self.db.close().map_err(|(_, e)| e.into())
    }
    /// Port of library.restore_backup's SQLite page copy and migrations.
    /// The caller validates the source snapshot and creates its safety backup first.
    pub fn restore_from(&self, source: &Connection) -> Result<()> {
        if self.reader {
            return Err(invalid("catalog is read-only"));
        }
        let mut target = Connection::open(self.directory.join("catalog.sqlite"))?;
        target.busy_timeout(Duration::from_secs(5))?;
        {
            let backup = rusqlite::backup::Backup::new(source, &mut target)?;
            backup.run_to_completion(1_000_000, Duration::from_millis(0), None)?;
        }
        drop(target);
        Self::new(&self.directory)?;
        Ok(())
    }
    /// Port of `Catalog.backup`; uses SQLite's online backup API, including committed WAL pages.
    pub fn backup(&self, destination: impl AsRef<Path>) -> Result<PathBuf> {
        let dest = std::path::absolute(destination)?;
        let source = std::path::absolute(self.directory.join("catalog.sqlite"))?;
        if self.platform.path_key(&dest.to_string_lossy()) == self.platform.path_key(&source.to_string_lossy())
            || (dest.exists() && std::fs::canonicalize(&dest)? == std::fs::canonicalize(&source)?)
        {
            return Err(invalid("cannot back up over catalog source"));
        }
        if let Some(p) = dest.parent() {
            std::fs::create_dir_all(p)?;
        }
        let mut output = Connection::open(&dest)?;
        let backup = rusqlite::backup::Backup::new(&self.db, &mut output)?;
        backup.run_to_completion(100, Duration::from_millis(10), None)?;
        drop(backup);
        Ok(dest)
    }
    /// Port of `Catalog.virtual_copy`; copy-of-copy refers to the original and never copies stack membership.
    pub fn virtual_copy(&self, id: i64) -> Result<i64> {
        let p = self.photo(id)?.ok_or_else(|| invalid("photo not found"))?;
        self.transaction(||{let payload=json::dumps(&profiles::pack(&self.db,&serde_json::from_str(string(&p["edits"])?)?,true)?,true,false,false);
            self.db.execute("INSERT INTO photos(path,name,rating,flag,keywords,edits,metadata,imported,label,virtual_source,extras) VALUES(?,?,?,?,?,?,?,?,?,?,?)",params![string(&p["path"])?,format!("{} · 사본",string(&p["name"])?),integer(&p["rating"])?,integer(&p["flag"])?,string(&p["keywords"])?,payload,string(&p["metadata"])?,self.now(),string(&p["label"])?,p["virtual_source"].as_i64().filter(|v|*v!=0).unwrap_or(id),string(&p["extras"]) ?])?;
            let id=self.db.last_insert_rowid();self.retoken(id)?;Ok(id)})
    }
    /// Port of `Catalog.snapshot`; snapshots keep the stored settings without renormalizing.
    pub fn snapshot(&self, id: i64, name: &str) -> Result<()> {
        let p = self.photo(id)?.ok_or_else(|| invalid("photo not found"))?;
        self.transaction(|| {
            let payload = json::dumps(
                &profiles::pack(&self.db, &serde_json::from_str(string(&p["edits"])?)?, true)?,
                true,
                false,
                false,
            );
            self.db.execute(
                "INSERT INTO snapshots(photo_id,name,edits,created) VALUES(?,?,?,?)",
                params![id, name, payload, self.now()],
            )?;
            Ok(())
        })
    }
    /// Port of `Catalog.snapshots`.
    pub fn snapshots(&self, id: i64) -> Result<Vec<Value>> {
        self.settings_rows("SELECT * FROM snapshots WHERE photo_id=? ORDER BY id DESC", id)
    }
    /// Port of `Catalog.add_collection`; smart rules are stored verbatim JSON, including Adobe descriptors.
    pub fn add_collection(&self, name: &str, rules: Option<&Value>, parent_id: Option<i64>) -> Result<i64> {
        self.transaction(|| {
            self.db.execute(
                "INSERT INTO collections(name,parent_id,rules) VALUES(?,?,?)",
                params![name, parent_id, rules.map(|v| json::dumps(v, true, false, false))],
            )?;
            Ok(self.db.last_insert_rowid())
        })
    }
    /// Port of `Catalog.collections`.
    pub fn collections(&self) -> Result<Vec<Value>> {
        self.rows("SELECT * FROM collections ORDER BY name", [])
    }
    /// Port of `Catalog.collection_add`; duplicate members are ignored.
    pub fn collection_add(&self, collection_id: i64, ids: &[i64]) -> Result<()> {
        self.transaction(|| {
            for id in ids {
                self.db.execute("INSERT OR IGNORE INTO collection_members VALUES(?,?)", params![collection_id, id])?;
            }
            Ok(())
        })
    }
    /// Port of `Catalog.collection_ids`; smart collections are evaluated against current metadata.
    pub fn collection_ids(&self, collection_id: i64) -> Result<BTreeSet<i64>> {
        let Some(row) = self.rows("SELECT * FROM collections WHERE id=?", [collection_id])?.pop() else {
            return Ok(BTreeSet::new());
        };
        if let Some(rules) = row["rules"].as_str() {
            let rules = serde_json::from_str(rules)?;
            let mut ids = BTreeSet::new();
            for p in self.photos()? {
                if crate::query::matches(&p, &rules, chrono::Local::now().naive_local(), self.platform)? {
                    ids.insert(integer(&p["id"])?);
                }
            }
            Ok(ids)
        } else {
            self.rows("SELECT photo_id FROM collection_members WHERE collection_id=?", [collection_id])?
                .iter()
                .map(|r| integer(&r["photo_id"]))
                .collect()
        }
    }
    /// Port of `Catalog.remove_collection`; child collections survive with a NULL parent.
    pub fn remove_collection(&self, id: i64) -> Result<()> {
        self.transaction(|| {
            self.db.execute("UPDATE collections SET parent_id=NULL WHERE parent_id=?", [id])?;
            self.db.execute("DELETE FROM collection_members WHERE collection_id=?", [id])?;
            self.db.execute("DELETE FROM collections WHERE id=?", [id])?;
            Ok(())
        })
    }
    fn delete_photos(&self, ids: &[i64]) -> Result<()> {
        for part in ids.chunks(500) {
            let marks = vec!["?"; part.len()].join(",");
            for table in ["history", "snapshots", "collection_members"] {
                self.db.execute(
                    &format!("DELETE FROM {table} WHERE photo_id IN ({marks})"),
                    rusqlite::params_from_iter(part),
                )?;
            }
            self.db.execute(
                &format!("DELETE FROM preferences WHERE key IN ({marks})"),
                rusqlite::params_from_iter(part.iter().map(|id| format!("undo:{id}"))),
            )?;
            self.db.execute(&format!("DELETE FROM photos WHERE id IN ({marks})"), rusqlite::params_from_iter(part))?;
        }
        Ok(())
    }
    /// Port of `Catalog.remove_photos`; references in saved batches intentionally remain unchanged.
    pub fn remove_photos(&self, ids: &[i64]) -> Result<()> {
        self.transaction(|| self.delete_photos(ids))
    }
    /// Port of `Catalog.stack`; minimum selected id is the stack identifier.
    pub fn stack(&self, ids: &[i64]) -> Result<()> {
        if let Some(id) = ids.iter().min() {
            self.update_many(ids, "stack_id", &json!(id))
        } else {
            Ok(())
        }
    }
    /// Port of `Catalog.set_user_metadata`; merges extras while preserving hidden workflow keys.
    pub fn set_user_metadata(&self, id: i64, values: &Map<String, Value>) -> Result<()> {
        let p = self.photo(id)?.ok_or_else(|| invalid("photo not found"))?;
        let mut current = object(&p["user_metadata"])?.clone();
        current.extend(values.clone());
        let mut fields = Map::new();
        fields.insert("extras".into(), json::dumps(&current.into(), false, false, false).into());
        self.update(id, &fields)
    }
}
