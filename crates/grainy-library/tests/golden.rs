//! Golden scenarios of `tools/golden/gen_library.py`: the generator ran the oracle on temporary folders; this test
//! rebuilds the same starting tree in a temporary folder, runs the Rust port step by step and compares return
//! values, exception types and messages, the resulting file tree (sizes and SHA-256) and the catalogue rows.
//! Tolerance is 0 except where a scenario says otherwise (`tolerance`).

use grainy_catalog::folders::Platform;
use grainy_catalog::rusqlite::{params_from_iter, types::Value as Sql, Connection};
use grainy_catalog::Catalog;
use grainy_library::file_access::{access, present, unreadable_folders};
use grainy_library::folder_locations::{self, directory_identity, Identity};
use grainy_library::folder_recovery::{self, RecoveryPlan, SmallRgb, Thumbnailer};
use grainy_library::folder_sync::{self, FolderSync, ScanJob, SyncHost};
use grainy_library::import_copy::{self, DuplicateKey};
use grainy_library::pathx;
use grainy_library::{catalog_maintenance, library, quick_export, workflow_data, xmp, Cancel, Error};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

fn golden_dir() -> PathBuf {
    grainy_testkit::root().join("golden").join("library")
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn platform() -> Platform {
    Platform::native()
}

/// Numbers are equal when both are integers and equal, or within `tolerance`; maps ignore key order.
fn json_eq(a: &Value, b: &Value, tolerance: f64, worst: &mut f64) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => {
            if (x.is_i64() || x.is_u64()) && (y.is_i64() || y.is_u64()) {
                return x.as_i64() == y.as_i64() && x.as_u64() == y.as_u64();
            }
            let (x, y) = (x.as_f64().unwrap_or(f64::NAN), y.as_f64().unwrap_or(f64::NAN));
            let d = (x - y).abs();
            if d.is_nan() {
                return x.is_nan() && y.is_nan();
            }
            *worst = worst.max(d);
            d <= tolerance
        }
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(p, q)| json_eq(p, q, tolerance, worst))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| json_eq(v, w, tolerance, worst)))
        }
        _ => a == b,
    }
}

fn first_diff(a: &Value, b: &Value, path: &str) -> Option<String> {
    let clip = |v: &Value| -> String { v.to_string().chars().take(220).collect() };
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            for (k, v) in x {
                match y.get(k) {
                    Some(w) => {
                        if let Some(d) = first_diff(v, w, &format!("{path}.{k}")) {
                            return Some(d);
                        }
                    }
                    None => return Some(format!("{path}.{k} only in rust: {}", clip(v))),
                }
            }
            y.keys().find(|k| !x.contains_key(*k)).map(|k| format!("{path}.{k} only in oracle"))
        }
        (Value::Array(x), Value::Array(y)) => {
            for (i, (p, q)) in x.iter().zip(y).enumerate() {
                if let Some(d) = first_diff(p, q, &format!("{path}[{i}]")) {
                    return Some(d);
                }
            }
            (x.len() != y.len()).then(|| format!("{path} length {} vs {}", x.len(), y.len()))
        }
        _ => {
            if a == b {
                None
            } else {
                Some(format!("{path}: rust {} / oracle {}", clip(a), clip(b)))
            }
        }
    }
}

struct Env {
    _base: tempfile::TempDir,
    base: PathBuf,
    root: String,
    catdir: PathBuf,
    catalog: Option<Catalog>,
    spec: Value,
    identities: HashMap<String, HashMap<String, Identity>>,
    sync: Option<FolderSync>,
    host: SyncHost,
    job: Option<ScanJob>,
    job_files: Vec<String>,
    mono: Arc<AtomicU64>,
    epoch: f64,
    started: usize,
    messages: Vec<String>,
    plans: Vec<RecoveryPlan>,
    seeds: Option<ndarray::Array4<u8>>,
}

type Res = Result<Value, Error>;

fn os_err(e: std::io::Error) -> Error {
    Error::from(e)
}

impl Env {
    fn new(spec: Value) -> Env {
        let tmp = tempfile::Builder::new().prefix("grainy-lib-").tempdir().expect("temp dir");
        let base = PathBuf::from(pathx::resolve(platform(), &pathx::text(tmp.path())));
        let root = base.join("files");
        std::fs::create_dir(&root).expect("root");
        let catdir = base.join("catalog");
        Env {
            root: pathx::text(&root),
            _base: tmp,
            base,
            catdir,
            catalog: None,
            spec,
            identities: HashMap::new(),
            sync: None,
            host: SyncHost::default(),
            job: None,
            job_files: Vec::new(),
            mono: Arc::new(AtomicU64::new(1000.0f64.to_bits())),
            epoch: 1_800_000_000.0,
            started: 0,
            messages: Vec::new(),
            plans: Vec::new(),
            seeds: None,
        }
    }

    fn cat(&self) -> &Catalog {
        self.catalog.as_ref().expect("catalog")
    }

    fn p(&self, value: &str) -> String {
        if let Some(rest) = value.strip_prefix("$KEY:") {
            return platform().path_key(&self.p(rest));
        }
        if value == "$ANCHOR" {
            return pathx::split(platform(), &self.root).0;
        }
        if let Some(rest) = value.strip_prefix("$ROOT") {
            return format!("{}{}", self.root, rest.replace('/', std::path::MAIN_SEPARATOR_STR));
        }
        value.to_owned()
    }

    fn deep_in(&self, v: &Value) -> Value {
        match v {
            Value::String(s) => Value::String(self.p(s)),
            Value::Array(a) => Value::Array(a.iter().map(|x| self.deep_in(x)).collect()),
            Value::Object(m) => Value::Object(m.iter().map(|(k, x)| (self.p(k), self.deep_in(x))).collect()),
            other => other.clone(),
        }
    }

    fn out_str(&self, s: &str) -> String {
        let doubled = self.root.replace('\\', "\\\\");
        let low = self.root.to_lowercase();
        s.replace(&doubled, "$ROOT")
            .replace(&self.root, "$ROOT")
            .replace(&low.replace('\\', "\\\\"), "$root")
            .replace(&low, "$root")
    }

    fn out(&self, v: Value) -> Value {
        match v {
            Value::String(s) => Value::String(self.out_str(&s)),
            Value::Array(a) => Value::Array(a.into_iter().map(|x| self.out(x)).collect()),
            Value::Object(m) => Value::Object(m.into_iter().map(|(k, x)| (self.out_str(&k), self.out(x))).collect()),
            other => other,
        }
    }

    fn sql_arg(&self, v: &Value) -> Sql {
        match v {
            Value::Null => Sql::Null,
            Value::Bool(b) => Sql::Integer(i64::from(*b)),
            Value::Number(n) => n.as_i64().map(Sql::Integer).unwrap_or_else(|| Sql::Real(n.as_f64().unwrap_or(0.0))),
            Value::String(s) => Sql::Text(self.p(s)),
            Value::Object(m) => {
                let h = m["blob_hex"].as_str().unwrap_or("");
                Sql::Blob((0..h.len() / 2).map(|i| u8::from_str_radix(&h[2 * i..2 * i + 2], 16).unwrap_or(0)).collect())
            }
            Value::Array(_) => panic!("array SQL argument"),
        }
    }

    fn exec(&self, catalog: &Catalog, sql: &str, args: &[Value]) {
        let params: Vec<Sql> = args.iter().map(|a| self.sql_arg(a)).collect();
        catalog.connection().execute(sql, params_from_iter(params)).unwrap_or_else(|e| panic!("{sql}: {e}"));
    }

    fn build(&mut self) {
        let setup = self.spec.get("setup").cloned().unwrap_or(json!({}));
        if let Some(files) = setup.get("files").and_then(Value::as_object) {
            for (rel, item) in files {
                let path = Path::new(&self.root).join(rel);
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                let data: Vec<u8> = if let Some(t) = item.get("text").and_then(Value::as_str) {
                    t.as_bytes().to_vec()
                } else if let Some(t) = item.get("text16").and_then(Value::as_str) {
                    let mut d = vec![0xff, 0xfe];
                    for u in t.encode_utf16() {
                        d.extend(u.to_le_bytes());
                    }
                    d
                } else if let Some(h) = item.get("bytes_hex").and_then(Value::as_str) {
                    (0..h.len() / 2).map(|i| u8::from_str_radix(&h[2 * i..2 * i + 2], 16).unwrap()).collect()
                } else {
                    let f = item["fill"].as_array().unwrap();
                    vec![f[0].as_u64().unwrap() as u8; f[1].as_u64().unwrap() as usize]
                };
                std::fs::write(&path, data).unwrap();
            }
        }
        for rel in setup.get("dirs").and_then(Value::as_array).into_iter().flatten() {
            std::fs::create_dir_all(Path::new(&self.root).join(rel.as_str().unwrap())).unwrap();
        }
        for (rel, ns) in setup.get("mtimes").and_then(Value::as_object).into_iter().flatten() {
            set_mtime(&Path::new(&self.root).join(rel), ns.as_u64().unwrap());
        }
        let catalog = Catalog::new(&self.catdir).expect("catalog");
        self.populate(&catalog, &setup);
        self.catalog = Some(catalog);
        if let Some(prefs) = setup.get("prefs").and_then(Value::as_object) {
            for (k, v) in prefs {
                self.cat().save_preference(k, &self.deep_in(v)).unwrap();
            }
        }
        for (name, other) in setup.get("other").and_then(Value::as_object).into_iter().flatten() {
            let directory = self.base.join(format!("other_{name}"));
            let c = Catalog::new(&directory).unwrap();
            self.populate(&c, other);
            for (k, v) in other.get("prefs").and_then(Value::as_object).into_iter().flatten() {
                c.save_preference(k, &self.deep_in(v)).unwrap();
            }
            c.backup(self.base.join("backups_in").join(format!("{name}.sqlite"))).unwrap();
        }
    }

    fn populate(&self, catalog: &Catalog, setup: &Value) {
        for photo in setup.get("photos").and_then(Value::as_array).into_iter().flatten() {
            if let Some(id) = photo.get("virtual_of").and_then(Value::as_i64) {
                catalog.virtual_copy(id).unwrap();
            } else {
                let settings = photo.get("settings").map(|s| grainy_core::normalized(s.as_object().unwrap()));
                catalog
                    .add(&self.p(photo["path"].as_str().unwrap()), settings.as_ref(), photo.get("metadata"), false)
                    .unwrap();
            }
        }
        for item in setup.get("db").and_then(Value::as_array).into_iter().flatten() {
            let args: Vec<Value> = item[1].as_array().cloned().unwrap_or_default();
            self.exec(catalog, item[0].as_str().unwrap(), &args);
        }
    }

    fn tree(&self) -> Value {
        let mut files = Map::new();
        let mut dirs = Vec::new();
        fn walk(root: &Path, dir: &Path, files: &mut Map<String, Value>, dirs: &mut Vec<Value>) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let entry = entry.unwrap();
                let path = entry.path();
                let rel = path.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
                if path.is_dir() {
                    dirs.push(Value::String(rel));
                    walk(root, &path, files, dirs);
                } else {
                    let data = std::fs::read(&path).unwrap();
                    files.insert(rel, json!({"size": data.len(), "sha256": hex(&Sha256::digest(&data))}));
                }
            }
        }
        walk(Path::new(&self.root), Path::new(&self.root), &mut files, &mut dirs);
        json!({"files": files, "dirs": dirs})
    }

    fn database(&self) -> Value {
        let Some(c) = &self.catalog else { return json!({}) };
        let photos: Vec<Value> = c.rows("SELECT id,path,name,virtual_source FROM photos ORDER BY id", []).unwrap();
        let mut roots: Vec<String> = c
            .rows("SELECT path FROM folder_roots", [])
            .unwrap()
            .iter()
            .map(|r| r["path"].as_str().unwrap().to_owned())
            .collect();
        roots.sort();
        let mut prefs = Map::new();
        for r in c.rows("SELECT key,value FROM preferences ORDER BY key", []).unwrap() {
            prefs.insert(r["key"].as_str().unwrap().to_owned(), r["value"].clone());
        }
        let mut result = Map::new();
        result.insert("photos".into(), self.out(Value::Array(photos)));
        result.insert("folder_roots".into(), self.out(json!(roots)));
        result.insert("preferences".into(), self.out(Value::Object(prefs)));
        for table in self.spec.get("dump_tables").and_then(Value::as_array).into_iter().flatten() {
            let table = table.as_str().unwrap();
            result.insert(table.into(), self.out(Value::Array(self.dump_table(c.connection(), table))));
        }
        let backups = self.catdir.join("backups");
        if backups.exists() {
            let mut names: Vec<String> = std::fs::read_dir(&backups)
                .unwrap()
                .map(|e| strip_stamp(&e.unwrap().file_name().to_string_lossy()))
                .collect();
            names.sort();
            result.insert("backup_names".into(), json!(names));
        }
        Value::Object(result)
    }

    fn dump_table(&self, db: &Connection, table: &str) -> Vec<Value> {
        let columns: Vec<String> = {
            let mut stmt = db.prepare(&format!("PRAGMA table_info({table})")).unwrap();
            stmt.query_map([], |r| r.get::<_, String>(1)).unwrap().map(|r| r.unwrap()).collect()
        };
        if columns.is_empty() {
            return Vec::new();
        }
        let mut stmt = db.prepare(&format!("SELECT {} FROM {table} ORDER BY 1", columns.join(","))).unwrap();
        let rows = stmt
            .query_map([], |r| {
                let mut row = Vec::new();
                for i in 0..columns.len() {
                    row.push(match r.get::<_, Sql>(i)? {
                        Sql::Null => Value::Null,
                        Sql::Integer(n) => json!(n),
                        Sql::Real(f) => json!(f),
                        Sql::Text(t) => json!(t),
                        Sql::Blob(b) => json!(hex(&b)),
                    });
                }
                Ok(Value::Array(row))
            })
            .unwrap();
        rows.map(|r| r.unwrap()).collect()
    }

    fn snapshot_tables(&self) -> Value {
        let db = Connection::open(self.catdir.join("catalog.sqlite")).unwrap();
        let mut out = Map::new();
        for table in ["photos", "history", "presets", "snapshots", "preferences", "profile_assets"] {
            let columns: Vec<String> = {
                let mut stmt = db.prepare(&format!("PRAGMA table_info({table})")).unwrap();
                stmt.query_map([], |r| r.get::<_, String>(1)).unwrap().map(|r| r.unwrap()).collect()
            };
            if columns.is_empty() {
                continue;
            }
            let mut rows = self.dump_table(&db, table);
            if table == "photos" {
                let skip = columns.iter().position(|c| c == "imported");
                for row in &mut rows {
                    if let (Some(i), Value::Array(cells)) = (skip, row) {
                        cells.remove(i);
                    }
                }
            }
            out.insert(table.into(), self.out(Value::Array(rows)));
        }
        Value::Object(out)
    }

    fn run(&mut self) -> (Vec<Value>, Value, Value) {
        self.build();
        let calls = self.spec["calls"].as_array().cloned().unwrap();
        let mut results = Vec::new();
        for step in calls {
            let args = step.get("args").cloned().unwrap_or(json!({}));
            let fname = step["fn"].as_str().unwrap().to_owned();
            let value = self.call(&fname, &args);
            results.push(match value {
                Ok(v) => json!({"ok": self.out(v)}),
                Err(e) => json!({"err": {"type": e.python_type(), "message": e.to_string()}}),
            });
        }
        (results, self.tree(), self.database())
    }
}

fn strip_stamp(name: &str) -> String {
    let b: Vec<char> = name.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < b.len() {
        if i + 15 <= b.len()
            && b[i..i + 8].iter().all(char::is_ascii_digit)
            && b[i + 8] == '-'
            && b[i + 9..i + 15].iter().all(char::is_ascii_digit)
        {
            out.push_str("<stamp>");
            i += 15;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    // before-optimize-<stamp>-<8 hex>.sqlite: the id is random
    if out.starts_with("before-optimize-") && out.ends_with(".sqlite") && out.len() > 15 {
        let stem = &out[..out.len() - ".sqlite".len()];
        if let Some((head, id)) = stem.rsplit_once('-') {
            if id.len() == 8 && id.chars().all(|c| c.is_ascii_hexdigit()) {
                return format!("{head}-<id>.sqlite");
            }
        }
    }
    out
}

fn set_mtime(path: &Path, ns: u64) {
    let time = std::time::UNIX_EPOCH + std::time::Duration::from_nanos(ns);
    let file = std::fs::OpenOptions::new().write(true).open(path).unwrap();
    file.set_times(std::fs::FileTimes::new().set_modified(time).set_accessed(time)).unwrap();
}

fn s(args: &Value, key: &str) -> String {
    args[key].as_str().unwrap_or_else(|| panic!("missing {key}")).to_owned()
}

struct Seeds<'a>(&'a ndarray::Array4<u8>);

impl Seeds<'_> {
    fn image(&self, seed: usize, w: u32, h: u32) -> SmallRgb {
        SmallRgb { pixels: self.0.index_axis(ndarray::Axis(0), seed).to_owned(), width: w, height: h }
    }
    fn image_of_payload(&self, payload: &[u8]) -> Option<SmallRgb> {
        let d: Value = serde_json::from_slice(payload).ok()?;
        Some(self.image(d["seed"].as_u64()? as usize, d["w"].as_u64()? as u32, d["h"].as_u64()? as u32))
    }
}

impl Thumbnailer for Seeds<'_> {
    fn evidence_image(&self, jpeg: &[u8]) -> Option<SmallRgb> {
        self.image_of_payload(jpeg)
    }
    fn candidate_image(
        &self,
        path: &Path,
        _settings: &grainy_core::Settings,
        _rendered: bool,
        _color_space: &str,
    ) -> grainy_library::Result<SmallRgb> {
        let data = std::fs::read(path).map_err(os_err)?;
        self.image_of_payload(&data).ok_or_else(|| Error::value("not an image"))
    }
}

fn dup_key_value(k: &DuplicateKey) -> Value {
    match k {
        DuplicateKey::Dated(n, d) => json!([n, d]),
        DuplicateKey::Sized(n, bits) => json!([n, f64::from_bits(*bits)]),
    }
}

impl Env {
    fn sync_state(&self) -> Value {
        let fs = self.sync.as_ref().expect("sync");
        let mut skipped = Map::new();
        let mut keys: Vec<_> = fs.skipped().iter().collect();
        keys.sort_by(|a, b| a.0.cmp(b.0));
        for (k, v) in keys {
            skipped.insert(k.clone(), json!([v.0, v.1]));
        }
        json!({
            "running": fs.running, "revision": fs.revision(), "auto": fs.auto().iter().collect::<Vec<_>>(),
            "skipped": skipped, "excluded": fs.excluded().iter().collect::<Vec<_>>(),
            "queue": self.host.queue, "pending": self.host.pending_paths.iter().collect::<Vec<_>>(),
            "errors": self.host.errors, "messages": self.messages, "started": self.started, "last": fs.last(),
            "excluded_pref": self.cat().preference("sync_excluded", Value::Null).unwrap(),
        })
    }

    fn call(&mut self, fname: &str, a: &Value) -> Res {
        let pl = platform();
        match fname {
            // ---- harness steps -------------------------------------------------
            "fs_write" => {
                let path = self.p(&s(a, "path"));
                std::fs::create_dir_all(pathx::parent(pl, &path)).map_err(os_err)?;
                std::fs::write(path, s(a, "text")).map_err(os_err)?;
                Ok(Value::Null)
            }
            "fs_rename" => {
                std::fs::rename(self.p(&s(a, "from")), self.p(&s(a, "to"))).map_err(os_err)?;
                Ok(Value::Null)
            }
            "fs_mkdir" => {
                std::fs::create_dir_all(self.p(&s(a, "path"))).map_err(os_err)?;
                Ok(Value::Null)
            }
            "fs_rmdir" => {
                std::fs::remove_dir_all(self.p(&s(a, "path"))).map_err(os_err)?;
                Ok(Value::Null)
            }
            "fs_utime" => {
                set_mtime(Path::new(&self.p(&s(a, "path"))), a["ns"].as_u64().unwrap());
                Ok(Value::Null)
            }
            "fs_snapshot_identities" => {
                let mut map = HashMap::new();
                for path in a["paths"].as_array().unwrap() {
                    let path = self.p(path.as_str().unwrap());
                    if let Some(id) = directory_identity(Path::new(&path)) {
                        map.insert(pl.path_key(&path), id);
                    }
                }
                self.identities.insert(s(a, "name"), map);
                Ok(Value::Null)
            }
            "fs_sql" => {
                let args: Vec<Value> = a.get("args").and_then(Value::as_array).cloned().unwrap_or_default();
                self.exec(self.cat(), &s(a, "sql"), &args);
                Ok(Value::Null)
            }
            "pref" => {
                self.cat().save_preference(&s(a, "key"), &self.deep_in(&a["value"]))?;
                Ok(Value::Null)
            }
            // ---- library.py ----------------------------------------------------
            "file_digest" => Ok(json!(library::digest(Path::new(&self.p(&s(a, "path"))))?)),
            "duplicate_groups" => {
                let groups = library::duplicate_groups(pl, &self.cat().photos()?)?;
                Ok(json!(groups
                    .iter()
                    .map(|g| g.iter().map(|r| r["id"].clone()).collect::<Vec<_>>())
                    .collect::<Vec<_>>()))
            }
            "move_files" => {
                let mapping: Vec<(String, String)> = a["mapping"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|m| (self.p(m[0].as_str().unwrap()), self.p(m[1].as_str().unwrap())))
                    .collect();
                Ok(json!(library::move_files(self.cat(), &mapping)?))
            }
            "relink" => {
                library::relink(self.cat(), a["id"].as_i64().unwrap(), &self.p(&s(a, "path")))?;
                Ok(Value::Null)
            }
            "move_folder" => {
                library::move_folder(self.cat(), &self.p(&s(a, "source")), &self.p(&s(a, "destination")))?;
                Ok(Value::Null)
            }
            "relink_folder" => {
                Ok(json!(library::relink_folder(self.cat(), &self.p(&s(a, "source")), &self.p(&s(a, "destination")))?))
            }
            "restore_backup" => {
                let path = match a.get("name").and_then(Value::as_str) {
                    Some(n) => self.base.join("backups_in").join(n),
                    None => PathBuf::from(self.p(&s(a, "path"))),
                };
                library::restore_backup(self.cat(), &path)?;
                Ok(Value::Null)
            }
            "write_sidecar" => {
                xmp::write_sidecar(&a["record"], Path::new(&self.p(&s(a, "path"))))?;
                Ok(Value::Null)
            }
            "read_sidecar" => {
                let r = xmp::read_sidecar(Path::new(&self.p(&s(a, "path"))))?;
                let mut m = Map::new();
                if let Some(v) = r.rating {
                    m.insert("rating".into(), json!(v));
                }
                if let Some(v) = r.label {
                    m.insert("label".into(), json!(v));
                }
                if let Some(v) = r.settings {
                    m.insert("settings".into(), Value::Object(v.0));
                }
                if let Some(v) = r.user_metadata {
                    m.insert("user_metadata".into(), v);
                }
                if let Some(v) = r.keywords {
                    m.insert("keywords".into(), json!(v));
                }
                Ok(Value::Object(m))
            }
            "read_file" => Ok(json!(hex(&std::fs::read(self.p(&s(a, "path"))).map_err(os_err)?))),
            // ---- file_access.py ------------------------------------------------
            "access" => Ok(json!(access(
                Path::new(&self.p(&s(a, "path"))),
                a.get("folder").and_then(Value::as_bool).unwrap_or(false)
            )
            .as_str())),
            "present" => Ok(json!(present(Path::new(&self.p(&s(a, "path")))))),
            "unreadable_folders" => {
                let roots: Vec<String> =
                    a["roots"].as_array().unwrap().iter().map(|r| self.p(r.as_str().unwrap())).collect();
                Ok(json!(unreadable_folders(&roots)))
            }
            // ---- folder_locations.py --------------------------------------------
            "directory_identity_shape" => {
                Ok(directory_identity(Path::new(&self.p(&s(a, "path")))).map_or(Value::Null, |_| json!(3)))
            }
            "rebase" => Ok(json!(folder_locations::rebase(
                pl,
                &self.p(&s(a, "path")),
                &self.p(&s(a, "source")),
                &self.p(&s(a, "destination"))
            )?)),
            "scan_folders" => {
                let cancel = Cancel::new();
                if a.get("cancel").and_then(Value::as_bool).unwrap_or(false) {
                    cancel.set();
                }
                let identities = a
                    .get("identities")
                    .and_then(Value::as_str)
                    .and_then(|n| self.identities.get(n))
                    .cloned()
                    .unwrap_or_default();
                let list = |k: &str| -> Vec<String> {
                    a[k].as_array().unwrap().iter().map(|x| self.p(x.as_str().unwrap())).collect()
                };
                let expanded: HashSet<String> = list("expanded").iter().map(|x| pl.path_key(x)).collect();
                let expire = a.get("clock").and_then(Value::as_str) == Some("expire");
                let calls = Cell::new(0u32);
                let clock = || {
                    calls.set(calls.get() + 1);
                    if calls.get() == 1 || !expire {
                        0.0
                    } else {
                        100.0
                    }
                };
                let r = folder_locations::scan_folders_with(
                    pl,
                    &list("paths"),
                    &list("roots"),
                    &expanded,
                    &identities,
                    &cancel,
                    &clock,
                );
                let under = pl.path_key(&self.root);
                let mut idk: Vec<&String> =
                    r.identities.iter().map(|(k, _)| k).filter(|k| k.starts_with(&under)).collect();
                idk.sort();
                let mut missing: Vec<String> = r.missing.iter().filter(|k| k.starts_with(&under)).cloned().collect();
                missing.sort();
                let mut moves: Vec<Value> = r.moves.iter().map(|(x, y)| json!([x, y])).collect();
                moves.sort_by_key(|v| v.to_string());
                let mut scanned = r.scanned.clone();
                scanned.sort();
                Ok(
                    json!({"identities": idk, "missing": missing, "moves": moves, "children": r.children, "scanned": scanned, "incomplete": r.incomplete}),
                )
            }
            "relink_plan" => Ok(plan_json(&folder_locations::relink_plan(
                self.cat(),
                &self.p(&s(a, "source")),
                &self.p(&s(a, "destination")),
            )?)),
            "apply_relinks" => {
                let mut plans = Vec::new();
                for pair in a["pairs"].as_array().unwrap() {
                    plans.push(folder_locations::relink_plan(
                        self.cat(),
                        &self.p(pair[0].as_str().unwrap()),
                        &self.p(pair[1].as_str().unwrap()),
                    )?);
                }
                Ok(json!(folder_locations::apply_relinks(self.cat(), &plans)?))
            }
            // ---- folder_sync.py -------------------------------------------------
            "new_files" => {
                let known: HashSet<String> = a
                    .get("known")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .map(|x| pl.path_key(&self.p(x.as_str().unwrap())))
                    .collect();
                let mut skipped = HashMap::new();
                for x in a.get("skipped").and_then(Value::as_array).into_iter().flatten() {
                    let path = self.p(x.as_str().unwrap());
                    let meta = std::fs::metadata(&path).map_err(os_err)?;
                    let d = meta.modified().unwrap().duration_since(std::time::UNIX_EPOCH).unwrap();
                    let mtime = d.as_secs() as f64 + f64::from(d.subsec_nanos()) * 1e-9;
                    skipped.insert(pl.path_key(&pathx::resolve(pl, &path)), (mtime, meta.len()));
                }
                let cancel = Cancel::new();
                if a.get("cancel").and_then(Value::as_bool).unwrap_or(false) {
                    cancel.set();
                }
                let roots: Vec<String> =
                    a["roots"].as_array().unwrap().iter().map(|x| self.p(x.as_str().unwrap())).collect();
                let removed: Vec<String> = a
                    .get("removed")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .map(|x| self.p(x.as_str().unwrap()))
                    .collect();
                let settle = a.get("settle").and_then(Value::as_f64).unwrap_or(5.0);
                Ok(json!(folder_sync::new_files(
                    pl,
                    &roots,
                    &known,
                    &skipped,
                    Some(&cancel),
                    settle,
                    a["now"].as_f64().unwrap(),
                    &removed
                )))
            }
            "fs_init" => {
                folder_sync::clear_outputs();
                self.host = SyncHost::default();
                self.mono.store(a.get("mono").and_then(Value::as_f64).unwrap_or(1000.0).to_bits(), Ordering::SeqCst);
                self.epoch = a.get("epoch").and_then(Value::as_f64).unwrap_or(1_800_000_000.0);
                let clock = Arc::clone(&self.mono);
                self.sync = Some(FolderSync::new(self.cat(), move || f64::from_bits(clock.load(Ordering::SeqCst)))?);
                Ok(self.sync_state())
            }
            "fs_host" => {
                for (k, v) in a.as_object().unwrap() {
                    match k.as_str() {
                        "import_busy" => self.host.import_busy = v.as_bool().unwrap(),
                        "export_running" => self.host.export_running = v.as_bool().unwrap(),
                        "import_cancelled" => self.host.import_cancelled = v.as_bool().unwrap(),
                        "import_scans" => self.host.import_scans = v.as_u64().unwrap() as usize,
                        other => panic!("host field {other}"),
                    }
                }
                Ok(Value::Null)
            }
            "fs_time" => {
                if let Some(m) = a.get("mono").and_then(Value::as_f64) {
                    self.mono.store(m.to_bits(), Ordering::SeqCst);
                }
                if let Some(e) = a.get("epoch").and_then(Value::as_f64) {
                    self.epoch = e;
                }
                Ok(Value::Null)
            }
            "fs_scan" => {
                let mut sync = self.sync.take().unwrap();
                let job = sync.scan(self.cat(), &self.host)?;
                self.sync = Some(sync);
                let spawned = usize::from(job.is_some());
                let started = job.is_some();
                if job.is_some() {
                    self.job = job;
                }
                Ok(json!({"started": started, "spawned": spawned, "state": self.sync_state()}))
            }
            "fs_run_job" => {
                let files = self.job.as_ref().expect("job").run_at(self.epoch);
                self.job_files = files.clone();
                Ok(json!(files))
            }
            "fs_deliver" => {
                let files: Vec<String> = match a.get("files").and_then(Value::as_array) {
                    Some(list) => list.iter().map(|x| self.p(x.as_str().unwrap())).collect(),
                    None => self.job_files.clone(),
                };
                let revision = self.job.as_ref().map(|j| j.revision);
                let mut sync = self.sync.take().unwrap();
                let found = sync.found(&files, revision, self.catalog.as_ref().unwrap(), &mut self.host)?;
                self.sync = Some(sync);
                if let Some(m) = found.message {
                    self.messages.push(m);
                }
                if found.start_import {
                    self.started += 1;
                }
                Ok(self.sync_state())
            }
            "fs_failed" => {
                self.sync.as_mut().unwrap().failed();
                Ok(self.sync_state())
            }
            "fs_activated" => {
                let mut sync = self.sync.take().unwrap();
                let job = sync.activated(self.cat(), &self.host, a["active"].as_bool().unwrap())?;
                self.sync = Some(sync);
                let spawned = usize::from(job.is_some());
                if job.is_some() {
                    self.job = job;
                }
                Ok(json!({"spawned": spawned, "state": self.sync_state()}))
            }
            "fs_set_enabled" => {
                let mut sync = self.sync.take().unwrap();
                let job = sync.set_enabled(self.cat(), &self.host, a["value"].as_bool().unwrap())?;
                let enabled = sync.enabled(self.cat())?;
                self.sync = Some(sync);
                let spawned = usize::from(job.is_some());
                if job.is_some() {
                    self.job = job;
                }
                Ok(json!({"spawned": spawned, "enabled": enabled, "state": self.sync_state()}))
            }
            "fs_import_failed" => {
                let path = self.p(&s(a, "path"));
                let r = self.sync.as_mut().unwrap().import_failed(&path);
                Ok(json!({"result": r, "state": self.sync_state()}))
            }
            "fs_imported" => {
                let mut sync = self.sync.take().unwrap();
                sync.imported(
                    self.cat(),
                    &self.p(&s(a, "path")),
                    a.get("resolved").and_then(Value::as_bool).unwrap_or(false),
                )?;
                self.sync = Some(sync);
                Ok(self.sync_state())
            }
            "fs_exclude" => {
                let paths: Vec<String> =
                    a["paths"].as_array().unwrap().iter().map(|x| self.p(x.as_str().unwrap())).collect();
                let mut sync = self.sync.take().unwrap();
                sync.exclude(self.cat(), &paths)?;
                self.sync = Some(sync);
                Ok(self.sync_state())
            }
            "fs_folder_removed" => {
                self.sync.as_mut().unwrap().folder_removed();
                Ok(self.sync_state())
            }
            "fs_release" => {
                self.sync.as_mut().unwrap().release(&mut self.host);
                Ok(self.sync_state())
            }
            "fs_note_output" => {
                folder_sync::note_output(pl, &self.p(&s(a, "path")));
                Ok(Value::Null)
            }
            "fs_keep_outputs" => {
                let mut sync = self.sync.take().unwrap();
                sync.keep_outputs(self.cat())?;
                self.sync = Some(sync);
                Ok(self.sync_state())
            }
            "fs_shutdown" => {
                let mut sync = self.sync.take().unwrap();
                sync.shutdown(self.cat())?;
                let cancelled = sync.cancel.is_set();
                self.sync = Some(sync);
                Ok(json!({"cancelled": cancelled, "state": self.sync_state()}))
            }
            // ---- import_copy.py ---------------------------------------------------
            "capture_time" => {
                let meta = a.get("metadata").and_then(Value::as_object);
                let t = import_copy::capture_time(&self.p(&s(a, "path")), meta)?;
                Ok(json!(t.format("%Y-%m-%d %H:%M:%S").to_string()))
            }
            "check_template" => {
                import_copy::check_template(&s(a, "template"))?;
                Ok(Value::Null)
            }
            "duplicate_key" => {
                let k = import_copy::duplicate_key(pl, &self.p(&s(a, "path")), a["metadata"].as_object().unwrap())?;
                Ok(dup_key_value(&k))
            }
            "plan" => {
                let table = a.get("meta").cloned().unwrap_or(json!({}));
                let known: HashSet<DuplicateKey> = a
                    .get("known")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .map(|k| match &k[1] {
                        Value::String(d) => DuplicateKey::Dated(k[0].as_str().unwrap().to_owned(), d.clone()),
                        other => {
                            DuplicateKey::Sized(k[0].as_str().unwrap().to_owned(), other.as_f64().unwrap().to_bits())
                        }
                    })
                    .collect();
                let sources: Vec<String> =
                    a["sources"].as_array().unwrap().iter().map(|x| self.p(x.as_str().unwrap())).collect();
                let mut read = |p: &str| -> grainy_library::Result<Map<String, Value>> {
                    Ok(table.get(pathx::name(pl, p)).and_then(Value::as_object).cloned().unwrap_or_default())
                };
                let (planned, skipped) = import_copy::plan(
                    pl,
                    &sources,
                    a.get("subfolders").and_then(Value::as_str).unwrap_or("year/date"),
                    a.get("template").and_then(Value::as_str).unwrap_or("{stem}"),
                    &known,
                    &mut read,
                )?;
                Ok(
                    json!({"planned": planned.iter().map(|(x, r)| json!([x, r.replace('\\', "/")])).collect::<Vec<_>>(), "skipped": skipped}),
                )
            }
            "free" => Ok(json!(import_copy::free_name(pl, &self.p(&s(a, "path"))))),
            "execute" => {
                let planned: Vec<(String, String)> = a["planned"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|m| (self.p(m[0].as_str().unwrap()), m[1].as_str().unwrap().to_owned()))
                    .collect();
                let cancel = Cancel::new();
                let seen = RefCell::new(Vec::new());
                let after = a.get("cancel_after").and_then(Value::as_u64);
                let mut progress = |n: usize| {
                    seen.borrow_mut().push(n);
                    if after == Some(n as u64) {
                        cancel.set();
                    }
                };
                let backup = a.get("backup").and_then(Value::as_str).map(|b| self.p(b));
                let r = import_copy::execute(
                    pl,
                    &planned,
                    &self.p(&s(a, "destination")),
                    backup.as_deref(),
                    Some(&cancel),
                    Some(&mut progress),
                )?;
                Ok(json!({"copied": r.copied, "errors": r.errors, "progress": seen.into_inner()}))
            }
            "known_keys" => {
                let keys = import_copy::known_keys(self.cat())?;
                let mut list: Vec<Value> = import_copy::sorted_keys(&keys).iter().map(dup_key_value).collect();
                list.sort_by_key(|v| v.to_string());
                Ok(json!(list))
            }
            // ---- quick_export.py ---------------------------------------------------
            "preset" => Ok(Value::Object(quick_export::preset(&s(a, "kind"))?)),
            "previous" => {
                let hdr = a.get("hdr").and_then(Value::as_bool).unwrap_or(true);
                Ok(quick_export::previous(self.cat(), hdr)?.map_or(Value::Null, Value::Object))
            }
            "copy_original" => {
                let record = self.deep_in(&a["record"]);
                let (w, h) = quick_export::copy_original(&record, Path::new(&self.p(&s(a, "destination"))))?;
                Ok(json!([w, h]))
            }
            "export_concurrency" => {
                let records = a["records"].as_array().unwrap().clone();
                Ok(json!(quick_export::export_concurrency(
                    &records,
                    a["cpus"].as_u64().unwrap() as usize,
                    a["memory"].as_u64()
                )?))
            }
            // ---- catalog_maintenance.py ----------------------------------------------
            "optimize" => {
                let cancel = Cancel::new();
                let stages = RefCell::new(Vec::new());
                let at = a.get("cancel_at_stage").and_then(Value::as_str).map(str::to_owned);
                let mut progress = |p: catalog_maintenance::Progress| {
                    stages.borrow_mut().push(json!([p.stage, p.examined, p.total]));
                    if at.as_deref() == Some(p.stage.as_str()) {
                        cancel.set();
                    }
                };
                if a.get("cancel_before").and_then(Value::as_bool).unwrap_or(false) {
                    cancel.set();
                }
                let before = self.snapshot_tables();
                let r = catalog_maintenance::optimize_with(
                    &self.catdir,
                    Some(&cancel),
                    &mut progress,
                    a.get("batch_size").and_then(Value::as_u64).unwrap_or(16) as usize,
                )?;
                let after = self.snapshot_tables();
                let warnings: Vec<String> = r
                    .warnings
                    .iter()
                    .map(|w| {
                        let (owner, message) = w.split_once(": ").unwrap_or((w, ""));
                        if message.chars().next().is_some_and(|c| c >= '\u{ac00}') {
                            format!("{owner}|{message}")
                        } else {
                            owner.to_owned()
                        }
                    })
                    .collect();
                Ok(json!({
                    "converted": r.converted, "examined": r.examined, "total": r.total, "removed_assets": r.removed_assets,
                    "profiles": r.profiles, "cancelled": r.cancelled, "vacuumed": r.vacuumed, "warnings": warnings,
                    "warning_count": r.warning_count, "has_backup": r.backup.as_ref().is_some_and(|b| Path::new(b).exists()),
                    "integrity": r.integrity, "stages": stages.into_inner(), "checkpoint_len": r.checkpoint.len(),
                    "tables_equal": before == after, "tables": after,
                }))
            }
            // ---- workflow_data.py ---------------------------------------------------
            "capture_exposure" => Ok(json!(workflow_data::capture_exposure(&a["info"])?)),
            "match_exposure" => Ok(Value::Object(workflow_data::match_exposure(&a["reference"], &a["target"])?.0)),
            // ---- folder_recovery.py ---------------------------------------------------
            "missing_directory" => Ok(json!(folder_recovery::missing_directory(Path::new(&self.p(&s(a, "path")))))),
            "write_previews" => {
                let thumbs = self.catdir.join("thumbnails");
                std::fs::create_dir_all(&thumbs).map_err(os_err)?;
                for item in a["items"].as_array().unwrap() {
                    let id = item["id"].as_i64().unwrap();
                    let row = self.cat().rows("SELECT path FROM photos WHERE id=?", [id])?;
                    let path = row[0]["path"].as_str().unwrap().to_owned();
                    let token: Option<String> = self
                        .cat()
                        .connection()
                        .query_row("SELECT token FROM preview_state WHERE photo_id=?", [id], |r| r.get(0))
                        .ok();
                    let payload = format!(
                        "{{\"seed\": {}, \"w\": {}, \"h\": {}}}",
                        item["seed"],
                        item.get("w").and_then(Value::as_u64).unwrap_or(96),
                        item.get("h").and_then(Value::as_u64).unwrap_or(64)
                    );
                    std::fs::write(thumbs.join(format!("{id}.jpg")), &payload).map_err(os_err)?;
                    if !item.get("legacy").and_then(Value::as_bool).unwrap_or(false) {
                        let stamp = json!({"version": grainy_catalog::preview::VERSION, "path": path, "token": token,
                                           "sha256": hex(&Sha256::digest(payload.as_bytes())), "color_space": "sRGB"});
                        std::fs::write(thumbs.join(format!("{id}.json")), stamp.to_string()).map_err(os_err)?;
                    }
                }
                Ok(Value::Null)
            }
            "recover" => {
                if self.seeds.is_none() {
                    self.seeds = Some(grainy_testkit::Case::open("library", "_seeds").array("seeds"));
                }
                let cancel = Cancel::new();
                if a.get("cancel").and_then(Value::as_bool).unwrap_or(false) {
                    cancel.set();
                }
                let sources: Option<Vec<String>> = a
                    .get("sources")
                    .and_then(Value::as_array)
                    .map(|l| l.iter().map(|x| self.p(x.as_str().unwrap())).collect());
                let thumbs = Seeds(self.seeds.as_ref().unwrap());
                let timeout = a.get("timeout").and_then(Value::as_f64).unwrap_or(120.0);
                let started = std::time::Instant::now();
                let clock = move || started.elapsed().as_secs_f64();
                let r = folder_recovery::recover_folders_with(
                    &self.catdir,
                    Some(&cancel),
                    sources.as_deref(),
                    timeout,
                    &thumbs,
                    &clock,
                )?;
                let mut plans = Vec::new();
                for p in &r.plans {
                    let parent = pathx::parent(pl, &p.plan.destination);
                    let parent_ns = std::fs::metadata(&parent)
                        .unwrap()
                        .modified()
                        .unwrap()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_nanos();
                    plans.push(json!({
                        "source": p.plan.source, "destination": p.plan.destination,
                        "updates": p.plan.updates.iter().map(|(x, i)| json!([x, i])).collect::<Vec<_>>(),
                        "found": p.plan.found, "missing": p.plan.missing, "physical": p.physical, "proof": p.proof,
                        "anchors": p.anchors, "collision": p.collision, "expected_paths": p.expected_paths,
                        "guards": p.guards.iter().map(|(x, g)| json!([x, {"size": g.size, "mtime_ns": g.mtime_ns as u64}])).collect::<Vec<_>>(),
                        "identity_ok": p.identity == directory_identity(Path::new(&p.plan.destination)),
                        "parent_mtime_ok": p.parent_mtime_ns == parent_ns,
                    }));
                }
                let unresolved: Vec<Value> =
                    r.unresolved.iter().map(|u| json!({"source": u.source, "reason": u.reason})).collect();
                self.plans = r.plans.clone();
                Ok(
                    json!({"plans": plans, "unresolved": unresolved, "total": r.total, "incomplete": r.incomplete, "cancelled": r.cancelled}),
                )
            }
            "validate" => Ok(json!(folder_recovery::validate_recovery(self.cat(), &self.plans)?)),
            other => panic!("unknown call {other}"),
        }
    }
}

fn plan_json(p: &folder_locations::Plan) -> Value {
    json!({"source": p.source, "destination": p.destination, "updates": p.updates.iter().map(|(x, i)| json!([x, i])).collect::<Vec<_>>(),
           "found": p.found, "missing": p.missing})
}

fn scenario_names() -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(golden_dir())
        .expect("golden/library is missing: run tools/golden/gen_library.py")
        .filter_map(|e| e.ok())
        .filter(|e| e.path().join("case.json").exists())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

fn run_scenario(name: &str) -> Result<f64, String> {
    let text = std::fs::read_to_string(golden_dir().join(name).join("case.json")).map_err(|e| e.to_string())?;
    let case: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    // The oracle ran the scenarios on Windows; on macOS it writes the same paths below the root with slashes.
    let case = grainy_testkit::native_paths(&case, &["$ROOT", "$root"]);
    let Some(spec) = case.get("spec") else { return Ok(0.0) };
    let tolerance = spec.get("tolerance").and_then(Value::as_f64).unwrap_or(0.0);
    let mut env = Env::new(spec.clone());
    let (results, tree, db) = env.run();
    // Expected values carry the placeholders as they are; the actual ones were mapped to them already.
    let mut worst = 0.0;
    let mut problems = Vec::new();
    let expected = case["results"].as_array().unwrap();
    for (i, (got, want)) in results.iter().zip(expected).enumerate() {
        let step = &spec["calls"][i];
        let label = format!(
            "step {i} {} {}",
            step["fn"],
            step.get("args").map(|a| a.to_string().chars().take(100).collect::<String>()).unwrap_or_default()
        );
        match (got.get("err"), want.get("err")) {
            (Some(g), Some(w)) => {
                if g["type"] != w["type"] {
                    problems.push(format!("{label}: error type {} != {}", g["type"], w["type"]));
                } else if !w["message"].is_null() && g["message"] != w["message"] {
                    problems.push(format!("{label}: error message {} != {}", g["message"], w["message"]));
                }
            }
            (None, None) => {
                if !json_eq(&got["ok"], &want["ok"], tolerance, &mut worst) {
                    problems
                        .push(format!("{label}:\n   {}", first_diff(&got["ok"], &want["ok"], "").unwrap_or_default()));
                }
            }
            (g, w) => problems.push(format!("{label}: rust err {g:?} / oracle err {w:?}; rust {}", got)),
        }
    }
    if results.len() != expected.len() {
        problems.push("number of steps differs".into());
    }
    let sorted_dirs = |v: &Value| -> Vec<String> {
        let mut d: Vec<String> =
            v["dirs"].as_array().unwrap().iter().map(|x| x.as_str().unwrap().to_lowercase()).collect();
        d.sort();
        d
    };
    if tree["files"] != case["tree"]["files"] {
        let diff = first_diff(&tree["files"], &case["tree"]["files"], "").unwrap_or_default();
        problems.push(format!("file tree differs:\n   {diff}"));
    }
    if sorted_dirs(&tree) != sorted_dirs(&case["tree"]) {
        problems.push(format!("folders differ: {:?} vs {:?}", sorted_dirs(&tree), sorted_dirs(&case["tree"])));
    }
    if !json_eq(&db, &case["db"], 0.0, &mut worst) {
        problems.push(format!("catalogue differs:\n   {}", first_diff(&db, &case["db"], "").unwrap_or_default()));
    }
    if problems.is_empty() {
        Ok(worst)
    } else {
        Err(problems.join("\n"))
    }
}

#[test]
fn library_scenarios() {
    // Local times (a file's modification time when a photo has no capture date) as on the machine the goldens
    // were written on (UTC+9), whatever the zone of this one.
    grainy_core::local_time::fix_offset(9 * 3600);
    let mut failures = Vec::new();
    let mut worst = 0.0f64;
    let names = scenario_names();
    for name in &names {
        // A scenario that panics is one failure among the others (so one run shows every differing scenario).
        match std::panic::catch_unwind(|| run_scenario(name)) {
            Ok(Ok(w)) => worst = worst.max(w),
            Ok(Err(e)) => failures.push(format!("== {name}\n{e}")),
            Err(panic) => {
                let message = panic
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| panic.downcast_ref::<&str>().map(|s| s.to_string()))
                    .unwrap_or_default();
                failures.push(format!("== {name}\npanicked: {message}"));
            }
        }
    }
    eprintln!("{} scenarios, largest numeric difference {worst:e}", names.len());
    assert!(failures.is_empty(), "{} scenario(s) differ:\n{}", failures.len(), failures.join("\n"));
}
