//! Port of `luma/folders.py`; lexical catalog paths never scan source directories.
use crate::{invalid, Result};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use unicode_normalization::UnicodeNormalization;

/// Platform rules used by `folders.path_key` and `folders.stored_path`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    /// Windows: backslashes and case-insensitive keys.
    Windows,
    /// macOS: NFC storage and lowercase NFC keys.
    Mac,
    /// Other POSIX systems: case-sensitive keys.
    Posix,
}
impl Platform {
    /// Native platform selection of `folders.MAC` and `os.path`.
    pub fn native() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::Mac
        } else {
            Self::Posix
        }
    }
    pub(crate) fn sep(self) -> &'static str {
        if self == Self::Windows {
            "\\"
        } else {
            "/"
        }
    }
    fn split(self, path: &str) -> (String, Vec<String>) {
        let s = if self == Self::Windows { path.replace('/', "\\") } else { path.into() };
        let sep = self.sep();
        let mut rest = s.as_str();
        let mut anchor = String::new();
        if self == Self::Windows && rest.starts_with("\\\\") {
            let bits = rest[2..].splitn(3, '\\').collect::<Vec<_>>();
            if bits.len() >= 2 {
                anchor = format!("\\\\{}\\{}\\", bits[0], bits[1]);
                rest = bits.get(2).copied().unwrap_or("");
            }
        } else if self == Self::Windows && rest.as_bytes().get(1) == Some(&b':') {
            anchor = rest[..2].into();
            rest = &rest[2..];
            if rest.starts_with(sep) {
                anchor.push_str(sep);
                rest = rest.trim_start_matches(sep);
            }
        } else if rest.starts_with(sep) {
            anchor = if self != Self::Windows && rest.starts_with("//") && !rest.starts_with("///") {
                "//".into()
            } else {
                sep.into()
            };
            rest = rest.trim_start_matches(sep);
        }
        (anchor, rest.split(sep).filter(|p| !p.is_empty() && *p != ".").map(str::to_owned).collect())
    }
    fn join_parts(self, anchor: &str, parts: &[String]) -> String {
        let s = format!("{anchor}{}", parts.join(self.sep()));
        if s.is_empty() {
            ".".into()
        } else {
            s
        }
    }
    pub(crate) fn normalized(self, path: &str, resolve_dots: bool) -> String {
        let (anchor, parts) = self.split(path);
        let mut result = Vec::new();
        for part in parts {
            if resolve_dots && part == ".." {
                if result.last().is_some_and(|x| x != "..") {
                    result.pop();
                } else if !anchor.ends_with(self.sep()) {
                    result.push(part);
                }
            } else {
                result.push(part);
            }
        }
        self.join_parts(&anchor, &result)
    }
    /// Port of `folders.path_key` with an explicit platform for portable catalog tests.
    pub fn path_key(self, path: &str) -> String {
        let p = self.normalized(path, true);
        match self {
            Self::Windows => p.to_lowercase(),
            Self::Mac => p.nfc().collect::<String>().to_lowercase(),
            Self::Posix => p,
        }
    }
    /// Port of `folders.stored_path`; callers pass the equivalent of `str(Path(path))`.
    pub fn stored_path(self, path: &str) -> String {
        if self == Self::Mac {
            path.nfc().collect()
        } else {
            path.into()
        }
    }
    /// Port of luma/folders.py Path.parent.
    pub fn parent(self, path: &str) -> String {
        let (a, mut p) = self.split(path);
        p.pop();
        self.join_parts(&a, &p)
    }
    /// Port of luma/folders.py Path.name.
    pub fn name(self, path: &str) -> String {
        self.split(path).1.last().cloned().unwrap_or_default()
    }
    /// Port of luma/folders.py Path.join.
    pub fn join(self, a: &str, b: &str) -> String {
        self.normalized(&format!("{}{sep}{b}", a.trim_end_matches(['\\', '/']), sep = self.sep()), false)
    }
    fn depth(self, p: &str) -> usize {
        let (a, p) = self.split(p);
        p.len() + usize::from(!a.is_empty())
    }
    /// Port of `folders.in_folder`, including different-drive rejection.
    pub fn in_folder(self, photo_path: &str, folder: &str, recursive: bool) -> bool {
        let p = self.path_key(&self.parent(photo_path));
        let f = self.path_key(folder);
        if !recursive {
            return p == f;
        }
        let (pa, pp) = self.split(&p);
        let (fa, fp) = self.split(&f);
        let same_anchor = if self == Self::Windows { pa == fa } else { pa.is_empty() == fa.is_empty() };
        if !same_anchor {
            return false;
        }
        let common = pp.iter().zip(&fp).take_while(|(a, b)| a == b).map(|(p, _)| p.clone()).collect::<Vec<_>>();
        let anchor = if self != Self::Windows && !pa.is_empty() { "/" } else { &pa };
        // os.path.commonpath returns "" for relative dot and collapses POSIX // to /.
        let result =
            if anchor.is_empty() && common.is_empty() { String::new() } else { self.join_parts(anchor, &common) };
        result == f
    }
    /// Port of `folders.contains_folder`.
    pub fn contains_folder(self, parent: &str, child: &str) -> bool {
        self.in_folder(&self.join(child, "__folder_probe__"), parent, true)
    }
    /// Port of `folders.left_out`; deeper explicit registrations override exclusions.
    pub fn left_out(self, path: &str, registered: &[String], removed: &[String]) -> bool {
        let depth = |list: &[String]| {
            list.iter().filter(|f| self.contains_folder(f, path)).map(|f| self.depth(f)).max().unwrap_or(0)
        };
        depth(removed) > depth(registered)
    }
    /// Port of `folders.below`; original spelling is retained in the relative suffix.
    pub fn below(self, path: &str, folder: &str) -> Result<String> {
        if !self.contains_folder(folder, path) {
            return Err(invalid(format!("{path} is not in {folder}")));
        }
        let parts = self.split(path).1;
        let count = self.split(folder).1.len();
        Ok(self.join_parts("", parts.get(count..).unwrap_or(&[])))
    }
    /// Port of `folders.folder_nodes`; direct and recursive counts include virtual copies.
    pub fn folder_nodes(self, photos: &[Value], registered: &[String]) -> Result<Vec<Value>> {
        let mut roots = BTreeMap::new();
        let mut parents = BTreeMap::new();
        for p in registered {
            roots.insert(self.path_key(p), self.normalized(p, false));
        }
        for p in photos {
            let path = self.parent(crate::string(&p["path"])?);
            parents.insert(self.path_key(&path), path);
        }
        for (k, p) in &parents {
            if !roots.values().any(|r| self.contains_folder(r, p)) {
                roots.insert(k.clone(), p.clone());
            }
        }
        let mut sorted = roots.values().cloned().collect::<Vec<_>>();
        sorted.sort_by_key(|p| (self.depth(p), self.path_key(p)));
        let mut compact: Vec<String> = Vec::new();
        for p in sorted {
            if !compact.iter().any(|r| self.contains_folder(r, &p)) {
                compact.push(p);
            }
        }
        let mut nodes: BTreeMap<String, Value> = BTreeMap::new();
        let add = |nodes: &mut BTreeMap<String, Value>, path: &str, parent: Option<&str>, drive: bool| {
            let key = self.path_key(path);
            nodes.entry(key.clone()).or_insert_with(||json!({"key":key,"path":path,"parent":parent,"drive":drive,"name":if drive{path.trim_end_matches(['\\','/']).to_owned()}else{self.name(path)},"direct":0,"total":0}));
            key
        };
        for root in compact {
            let anchor = self.split(&root).0;
            let anchor = if anchor.is_empty() { ".".into() } else { anchor };
            let dk = add(&mut nodes, &anchor, None, true);
            let rk = if self.path_key(&root) != dk { add(&mut nodes, &root, Some(&dk), false) } else { dk };
            for path in parents.values().chain(roots.values()).filter(|p| self.contains_folder(&root, p)) {
                let relative = self.below(path, &root)?;
                let mut current = root.clone();
                let mut parent = rk.clone();
                for part in self.split(&relative).1 {
                    current = self.join(&current, &part);
                    parent = add(&mut nodes, &current, Some(&parent), false);
                }
            }
        }
        for photo in photos {
            let mut key = Some(self.path_key(&self.parent(crate::string(&photo["path"])?)));
            let count = photo.get("count").and_then(Value::as_i64).unwrap_or(1);
            let mut direct = true;
            while let Some(k) = key {
                let n = nodes.get_mut(&k).ok_or_else(|| invalid("folder tree missing parent"))?;
                if direct {
                    n["direct"] = (n["direct"].as_i64().unwrap_or(0) + count).into();
                    direct = false;
                }
                n["total"] = (n["total"].as_i64().unwrap_or(0) + count).into();
                key = n["parent"].as_str().map(str::to_owned);
            }
        }
        let mut result = nodes.into_values().collect::<Vec<_>>();
        result.sort_by_key(|n| {
            (self.depth(n["path"].as_str().unwrap_or("")), n["key"].as_str().unwrap_or("").to_owned())
        });
        Ok(result)
    }
}
