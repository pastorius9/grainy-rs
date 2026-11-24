//! Finding the AI programs after installation, also when Grainy was started with an older `PATH`. Port of
//! luma/ai_paths.py (`tool_folders`, `find_tool`, `install_environment`), the discovery half of ai_programs.py
//! (`find_program`, `codex_program`) and auth.py `find_codex`. Everything is read through a [`System`].

use std::path::{Path, PathBuf};

use super::system::{Os, System};

/// `shutil._WIN_DEFAULT_PATHEXT`.
const DEFAULT_PATHEXT: &str = ".COM;.EXE;.BAT;.CMD;.VBS;.JS;.WS;.MSC";

fn var_nonempty(sys: &dyn System, name: &str) -> Option<String> {
    sys.var(name).filter(|v| !v.is_empty())
}

fn join(base: &Path, parts: &[&str]) -> PathBuf {
    let mut path = base.to_path_buf();
    for part in parts {
        path.push(part);
    }
    path
}

/// Port of `tool_folders(platform)`: where installers put the programs.
pub fn tool_folders(sys: &dyn System, os: Os) -> Vec<PathBuf> {
    let home = sys.home();
    let mut folders = vec![join(&home, &[".local", "bin"])];
    if os == Os::Windows {
        let local = var_nonempty(sys, "LOCALAPPDATA").map_or_else(|| join(&home, &["AppData", "Local"]), PathBuf::from);
        let roaming = var_nonempty(sys, "APPDATA").map_or_else(|| join(&home, &["AppData", "Roaming"]), PathBuf::from);
        let programs = var_nonempty(sys, "ProgramFiles").unwrap_or_else(|| "C:/Program Files".to_owned());
        folders.extend([
            join(&local, &["Programs", "OpenAI", "Codex", "bin"]),
            roaming.join("npm"),
            PathBuf::from(programs).join("nodejs"),
            join(&local, &["Programs", "nodejs"]),
            join(&local, &["Microsoft", "WindowsApps"]),
        ]);
    } else {
        folders.extend([
            PathBuf::from("/opt/homebrew/bin"),
            PathBuf::from("/usr/local/bin"),
            join(&home, &[".npm-global", "bin"]),
        ]);
    }
    if let Some(dir) = var_nonempty(sys, "CODEX_INSTALL_DIR") {
        folders.push(PathBuf::from(dir));
    }
    folders
}

fn executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).map(|m| m.permissions().mode() & 0o111 != 0).unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        path.exists()
    }
}

fn access(path: &Path, os: Os) -> bool {
    path.exists() && !path.is_dir() && (os == Os::Windows || executable(path))
}

/// Port of `shutil.which(cmd)` with the default mode.
pub fn which(sys: &dyn System, cmd: &str) -> Option<String> {
    let os = sys.os();
    let separator = if os == Os::Windows { ';' } else { ':' };
    let given = Path::new(cmd);
    let (dirs, file): (Vec<String>, String) = match (given.parent(), given.file_name()) {
        (Some(parent), Some(name)) if !parent.as_os_str().is_empty() => {
            (vec![parent.to_string_lossy().into_owned()], name.to_string_lossy().into_owned())
        }
        _ => {
            let path = sys.var("PATH")?;
            if path.is_empty() {
                return None;
            }
            (
                path.split(separator)
                    .filter(|dir| os != Os::Windows || Path::new(dir).is_absolute())
                    .map(str::to_owned)
                    .collect(),
                cmd.to_owned(),
            )
        }
    };
    let mut files = vec![file.clone()];
    if os == Os::Windows {
        let source = var_nonempty(sys, "PATHEXT").unwrap_or_else(|| DEFAULT_PATHEXT.to_owned());
        let exts: Vec<String> =
            source.split(';').filter(|e| !e.is_empty()).map(|e| e.trim_end_matches('.').to_owned()).collect();
        files = exts.iter().map(|e| format!("{file}{e}")).collect();
        let upper = file.to_uppercase();
        if exts.iter().any(|e| upper.ends_with(&e.to_uppercase())) {
            files.insert(0, file);
        }
    }
    let mut seen = Vec::new();
    for dir in dirs {
        let norm = if os == Os::Windows { dir.to_lowercase().replace('/', "\\") } else { dir.clone() };
        if seen.contains(&norm) {
            continue;
        }
        seen.push(norm);
        for name in &files {
            let joined = py_join(&dir, name, os);
            let real = if dir == "." { sys.current_dir().join(name) } else { PathBuf::from(&joined) };
            if access(&real, os) {
                return Some(joined);
            }
        }
    }
    None
}

/// `os.path.join(dir, name)`.
fn py_join(dir: &str, name: &str, os: Os) -> String {
    if dir.is_empty() {
        return name.to_owned();
    }
    let separators: &[char] = if os == Os::Windows { &['\\', '/'] } else { &['/'] };
    if dir.ends_with(separators) || (os == Os::Windows && dir.len() == 2 && dir.ends_with(':')) {
        format!("{dir}{name}")
    } else {
        format!("{dir}{}{name}", if os == Os::Windows { '\\' } else { '/' })
    }
}

/// Port of `find_tool(name)`.
pub fn find_tool(sys: &dyn System, name: &str) -> Option<String> {
    if let Some(found) = which(sys, name) {
        return Some(found);
    }
    let os = sys.os();
    let suffixes: &[&str] =
        if os == Os::Windows && Path::new(name).extension().is_none() { &[".exe", ".cmd", ".bat", ""] } else { &[""] };
    for folder in tool_folders(sys, os) {
        for suffix in suffixes {
            let path = folder.join(format!("{name}{suffix}"));
            if path.is_file() && (os == Os::Windows || executable(&path)) {
                return Some(path.to_string_lossy().into_owned());
            }
        }
    }
    None
}

/// Port of `install_environment(platform)`: this environment with the tool folders added to `PATH`.
pub fn install_environment(sys: &dyn System, os: Os) -> Vec<(String, String)> {
    let mut env = sys.vars();
    let separator = if os == Os::Windows { ";" } else { ":" };
    let mut paths = vec![sys.var("PATH").unwrap_or_default()];
    paths.extend(tool_folders(sys, os).iter().map(|p| p.to_string_lossy().into_owned()));
    let value = paths.into_iter().filter(|p| !p.is_empty()).collect::<Vec<_>>().join(separator);
    match env.iter_mut().find(|(k, _)| if os == Os::Windows { k.eq_ignore_ascii_case("PATH") } else { k == "PATH" }) {
        Some(slot) => slot.1 = value,
        None => env.push(("PATH".into(), value)),
    }
    env
}

/// Port of `ai_install.node_ready`.
pub fn node_ready(sys: &dyn System) -> bool {
    find_tool(sys, "node").is_some() && find_tool(sys, "npm").is_some()
}

/// `Path.resolve()`: the real path without the `\\?\` prefix; the path as it is when it does not exist.
pub fn resolve(path: &Path) -> PathBuf {
    match std::fs::canonicalize(path) {
        Ok(real) => {
            let text = real.to_string_lossy().into_owned();
            PathBuf::from(text.strip_prefix(r"\\?\").unwrap_or(&text))
        }
        Err(_) => std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf()),
    }
}

fn suffix_lower(path: &Path) -> String {
    path.extension().map(|e| format!(".{}", e.to_string_lossy().to_lowercase())).unwrap_or_default()
}

/// Port of `ai_programs.find_program(name)` (`claude` or `gemini`): the program to start, as an argument list
/// (npm's JavaScript entry with node, never a `.cmd` file interpolated by a shell).
pub fn find_program(sys: &dyn System, name: &str) -> Option<Vec<String>> {
    let os = sys.os();
    let mut found = find_tool(sys, name);
    if found.is_none() && name == "claude" {
        let candidate = join(&sys.home(), &[".local", "bin", if os == Os::Windows { "claude.exe" } else { "claude" }]);
        if candidate.is_file() {
            found = Some(candidate.to_string_lossy().into_owned());
        }
    }
    let found = found?;
    let path = PathBuf::from(&found);
    if os != Os::Windows && name == "gemini" {
        let entry = resolve(&path);
        let node = find_tool(sys, "node");
        return match node {
            Some(node) if entry.extension().is_some_and(|e| e == "js") => {
                Some(vec![node, entry.to_string_lossy().into_owned()])
            }
            _ => None,
        };
    }
    if os == Os::Windows && [".cmd", ".ps1", ".bat", ""].contains(&suffix_lower(&path).as_str()) {
        let (package, entry): (&[&str], &[&str]) = match name {
            "gemini" => (&["@google", "gemini-cli"], &["dist", "index.js"]),
            "claude" => (&["@anthropic-ai", "claude-code"], &["cli.js"]),
            _ => return None,
        };
        let mut script = path.parent().map(Path::to_path_buf).unwrap_or_default().join("node_modules");
        for part in package.iter().chain(entry) {
            script.push(part);
        }
        let node = find_tool(sys, "node");
        return match node {
            Some(node) if script.is_file() => Some(vec![node, script.to_string_lossy().into_owned()]),
            _ => None,
        };
    }
    Some(vec![found])
}

/// Entries of `folder` whose names match `pattern` (`*` and a literal prefix only, as the oracle's patterns use),
/// sorted as `sorted(Path.glob(...))` sorts them.
fn matching(folder: &Path, pattern: &str, os: Os) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(folder) else { return Vec::new() };
    let fold = |s: &str| if os == Os::Windows { s.to_lowercase() } else { s.to_owned() };
    let prefix = fold(pattern.trim_end_matches('*'));
    let mut out: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .filter(|e| fold(&e.file_name().to_string_lossy()).starts_with(&prefix))
        .map(|e| e.path())
        .collect();
    out.sort_by_key(|p| fold(&p.to_string_lossy()));
    out
}

/// `sorted(package.glob(pattern))` for the oracle's `NATIVE_WINDOWS` / `NATIVE_POSIX` patterns, in their order.
fn native_candidates(package: &Path, os: Os) -> Vec<PathBuf> {
    let (platform_prefix, program) =
        if os == Os::Windows { ("codex-win32-*", "codex.exe") } else { ("codex-darwin-*", "codex") };
    let mut out = Vec::new();
    for place in [true, false] {
        for folder in ["bin", "codex"] {
            let bases: Vec<PathBuf> = if place {
                matching(&join(package, &["node_modules", "@openai"]), platform_prefix, os)
            } else {
                vec![package.to_path_buf()]
            };
            let mut found = Vec::new();
            for base in bases {
                for target in matching(&base.join("vendor"), "*", os) {
                    found.push(join(&target, &[folder, program]));
                }
            }
            found.retain(|p| p.exists());
            found.sort_by_key(|p| {
                if os == Os::Windows {
                    p.to_string_lossy().to_lowercase()
                } else {
                    p.to_string_lossy().into_owned()
                }
            });
            out.extend(found);
        }
    }
    out
}

/// Port of auth.py `find_codex()`: the native Codex program (never npm's Node launcher).
pub fn find_codex(sys: &dyn System) -> Option<String> {
    let os = sys.os();
    if os != Os::Windows {
        return find_codex_posix(sys);
    }
    if let Some(native) = which(sys, "codex.exe") {
        return Some(native);
    }
    let appdata = sys.var("APPDATA").map_or_else(|| sys.home(), PathBuf::from);
    let mut roots = vec![appdata.join("npm")];
    for name in ["codex", "codex.cmd", "codex.ps1"] {
        if let Some(found) = which(sys, name) {
            roots.push(Path::new(&found).parent().map(Path::to_path_buf).unwrap_or_default());
        }
    }
    let mut seen: Vec<PathBuf> = Vec::new();
    for root in roots {
        if seen.contains(&root) {
            continue;
        }
        seen.push(root.clone());
        let package = join(&root, &["node_modules", "@openai", "codex"]);
        if let Some(path) = native_candidates(&package, os).into_iter().find(|p| p.is_file()) {
            return Some(resolve(&path).to_string_lossy().into_owned());
        }
    }
    for folder in tool_folders(sys, Os::Windows) {
        let native = folder.join("codex.exe");
        if native.is_file() {
            return Some(resolve(&native).to_string_lossy().into_owned());
        }
    }
    None
}

/// Port of `_find_codex_posix`.
fn find_codex_posix(sys: &dyn System) -> Option<String> {
    let usable = |p: &Path| p.is_file() && executable(p) && !matches!(suffix_lower(p).as_str(), ".js" | ".mjs");
    let mut places: Vec<String> = which(sys, "codex").into_iter().collect();
    places.extend(tool_folders(sys, Os::MacOs).iter().map(|f| f.join("codex").to_string_lossy().into_owned()));
    let mut seen = Vec::new();
    for place in places {
        if seen.contains(&place) {
            continue;
        }
        seen.push(place.clone());
        let command = PathBuf::from(&place);
        if !command.exists() {
            continue;
        }
        let real = resolve(&command);
        for folder in real.ancestors().skip(1) {
            let named = |p: &Path, n: &str| p.file_name().is_some_and(|f| f == n);
            if named(folder, "codex") && folder.parent().is_some_and(|p| named(p, "@openai")) {
                if let Some(native) = native_candidates(folder, Os::MacOs).into_iter().find(|p| usable(p)) {
                    return Some(resolve(&native).to_string_lossy().into_owned());
                }
            }
        }
        let script = std::fs::read(&real).map(|b| b.starts_with(b"#!")).unwrap_or(false);
        if usable(&real) && !script {
            return Some(real.to_string_lossy().into_owned());
        }
    }
    None
}

/// Port of `codex_program()`.
pub fn codex_program(sys: &dyn System) -> Option<Vec<String>> {
    find_codex(sys).map(|p| vec![p])
}
