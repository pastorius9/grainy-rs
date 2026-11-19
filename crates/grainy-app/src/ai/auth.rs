//! Read-only sign-in status checks and the programs' own, visible sign-in (port of luma/ai_auth.py). Only fixed
//! state and kind values come back from a check; credential files are never opened (Gemini's is only looked for);
//! sign-in runs in a console window the person owns.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;

use super::programs::{py_dumps, Profile};
use super::system::{Console, Launch, Os, System};

/// Port of `AuthStatus`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthStatus {
    /// `signed_in`, `signed_out`, `timeout` or `unknown`.
    pub state: String,
    /// `Claude`, `ChatGPT`, `Google`, `API key`, `Cloud` or empty.
    pub kind: String,
}

impl Default for AuthStatus {
    fn default() -> Self {
        Self::new("unknown", "")
    }
}

impl AuthStatus {
    /// A status.
    pub fn new(state: &str, kind: &str) -> Self {
        Self { state: state.into(), kind: kind.into() }
    }
}

fn regex(pattern: &'static str, slot: &'static std::sync::OnceLock<regex_lite::Regex>) -> &'static regex_lite::Regex {
    slot.get_or_init(|| regex_lite::Regex::new(pattern).expect("valid pattern"))
}

/// Port of `parse_status(key, code, stdout, stderr)`: allow-listed labels only, never account data.
pub fn parse_status(key: &str, code: i32, stdout: &str, stderr: &str) -> AuthStatus {
    match key {
        "claude" => {
            let Ok(Value::Object(data)) = serde_json::from_str::<Value>(stdout) else { return AuthStatus::default() };
            if data.get("loggedIn") == Some(&Value::Bool(false)) && (code == 0 || code == 1) {
                return AuthStatus::new("signed_out", "");
            }
            if data.get("loggedIn") == Some(&Value::Bool(true)) && code == 0 {
                let method = data.get("authMethod").map(|v| match v {
                    Value::String(s) => s.clone(),
                    other => grainy_mcp::pyjson::py_str(other),
                });
                let kind = match method.as_deref().unwrap_or("None") {
                    "claude.ai" | "oauth_token" => "Claude",
                    "api_key" | "api_key_helper" => "API key",
                    "third_party" => "Cloud",
                    _ => "",
                };
                return AuthStatus::new("signed_in", kind);
            }
        }
        "codex" => {
            static OUT: std::sync::OnceLock<regex_lite::Regex> = std::sync::OnceLock::new();
            static GPT: std::sync::OnceLock<regex_lite::Regex> = std::sync::OnceLock::new();
            static KEY: std::sync::OnceLock<regex_lite::Regex> = std::sync::OnceLock::new();
            // Codex reports its login status on stderr.
            let output = format!("{stdout}\n{stderr}");
            if regex(r"(?im)^not logged in\s*$", &OUT).is_match(&output) && (code == 0 || code == 1) {
                return AuthStatus::new("signed_out", "");
            }
            if code == 0 {
                if regex(r"(?im)^logged in using chatgpt\s*$", &GPT).is_match(&output) {
                    return AuthStatus::new("signed_in", "ChatGPT");
                }
                if regex(r"(?im)^logged in using an? api key\b", &KEY).is_match(&output) {
                    return AuthStatus::new("signed_in", "API key");
                }
            }
        }
        _ => {}
    }
    AuthStatus::default()
}

/// Port of `gemini_status(home, environ)`: an API key in the environment, or Gemini's cached Google sign-in file
/// present (only its existence is looked at). A hint, not a claim that it is still valid.
pub fn gemini_status(sys: &dyn System, home: Option<&Path>) -> AuthStatus {
    let set = |name: &str| sys.var(name).is_some_and(|v| !v.is_empty());
    if set("GEMINI_API_KEY") || set("GOOGLE_API_KEY") {
        return AuthStatus::new("signed_in", "API key");
    }
    let root = sys
        .var("GEMINI_CLI_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| home.map(Path::to_path_buf))
        .unwrap_or_else(|| sys.home());
    if root.join(".gemini").join("oauth_creds.json").is_file() {
        return AuthStatus::new("signed_in", "Google");
    }
    AuthStatus::default()
}

const AUTH_PATTERN: &str = r"(?i)\b401\b|\bunauthenticated\b|authentication[_ ]?(?:failed|required|error)|failed to authenticate|not (?:logged|signed) in|(?:log|sign)[ -]?in (?:is )?required|oauth access token is invalid|invalid[_ ]grant|invalid[_ ](?:api[_ ]key|credentials)|api[_ ]key[_ ](?:invalid|not[_ ]valid)|api key (?:is )?not valid|(?:access|refresh|oauth) token.{0,60}(?:expired|invalid|revoked|refresh)|(?:missing|no|could not load).{0,40}(?:credentials|api[_ ]key)|please (?:log|sign) in|please set an auth method|failed to (?:refresh|sign in)|authentication (?:is )?required|no authentication is configured|re-authenticate";

/// Port of `authentication_failed(value)` for text (stderr, a stream line).
pub fn authentication_failed(text: &str) -> bool {
    static PATTERN: std::sync::OnceLock<regex_lite::Regex> = std::sync::OnceLock::new();
    regex(AUTH_PATTERN, &PATTERN).is_match(text)
}

/// Port of `authentication_failed(value)` for a decoded value: dicts and lists as `json.dumps(ensure_ascii=False)`,
/// anything else as `str()`.
pub fn authentication_failed_value(value: &Value) -> bool {
    let text = match value {
        Value::Object(_) | Value::Array(_) => py_dumps(value, None, false),
        Value::String(s) => s.clone(),
        other => grainy_mcp::pyjson::py_str(other),
    };
    authentication_failed(&text)
}

/// Port of `check_status(profile, executable, timeout=5)`: runs the program's status command (in the home folder,
/// output kept in memory) and reduces its answer to a state.
pub fn check_status(
    profile: &Profile,
    executable: Option<&[String]>,
    sys: &dyn System,
    timeout: Duration,
) -> AuthStatus {
    let Some(executable) = executable.filter(|e| !e.is_empty()) else { return AuthStatus::default() };
    let Some(args) = profile.status_args else {
        return if profile.key == "gemini" { gemini_status(sys, None) } else { AuthStatus::default() };
    };
    let mut argv = executable.to_vec();
    argv.extend(args.iter().map(|a| (*a).to_owned()));
    let Ok(mut process) = sys.piped(&Launch { argv, cwd: sys.home(), env: None }) else { return AuthStatus::default() };
    let stop = process.stopper();
    let read = |reader: Option<Box<dyn std::io::Read + Send>>| {
        std::thread::spawn(move || {
            let mut out = Vec::new();
            if let Some(mut reader) = reader {
                let _ = reader.read_to_end(&mut out);
            }
            out
        })
    };
    let out = read(process.stdout().map(|r| Box::new(r) as Box<dyn std::io::Read + Send>));
    let err = read(process.stderr());
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let _ = tx.send(process.wait());
    });
    let code = match rx.recv_timeout(timeout) {
        Ok(Ok(code)) => code,
        Ok(Err(_)) => {
            stop();
            return AuthStatus::default();
        }
        Err(_) => {
            stop();
            let _ = rx.recv_timeout(Duration::from_secs(2));
            return AuthStatus::new("timeout", "");
        }
    };
    stop();
    let text =
        |h: std::thread::JoinHandle<Vec<u8>>| String::from_utf8_lossy(&h.join().unwrap_or_default()).into_owned();
    parse_status(profile.key, code, &text(out), &text(err))
}

/// Python's `shlex.quote`.
pub fn shell_quote(text: &str) -> String {
    if text.is_empty() {
        return "''".into();
    }
    if text.chars().all(|c| c.is_ascii_alphanumeric() || "_@%+=:,./-".contains(c)) {
        return text.into();
    }
    format!("'{}'", text.replace('\'', "'\"'\"'"))
}

/// A folder `prefix<random>` in `root` (`tempfile.TemporaryDirectory(prefix=…, dir=root)`).
pub(crate) fn temporary_folder(root: &Path, prefix: &str) -> std::io::Result<PathBuf> {
    for attempt in 0..100u32 {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.subsec_nanos());
        let folder = root.join(format!("{prefix}{:08x}", nanos ^ std::process::id().rotate_left(attempt)));
        match std::fs::create_dir(&folder) {
            Ok(()) => return Ok(folder),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::other("no temporary folder"))
}

/// Why a console could not be opened (`ValueError` / `OSError`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchError {
    /// `ValueError`.
    Value(String),
    /// `OSError`.
    Os(String),
}

impl std::fmt::Display for LaunchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LaunchError::Value(t) => write!(f, "ValueError: {t}"),
            LaunchError::Os(t) => write!(f, "OSError: {t}"),
        }
    }
}

impl From<std::io::Error> for LaunchError {
    fn from(e: std::io::Error) -> Self {
        LaunchError::Os(e.to_string())
    }
}

/// Port of `launch_sign_in(profile, executable, folder, platform)`: the program's own sign-in in a console window
/// the person owns (Windows), or in Terminal through a script that marks when it ends (macOS).
pub fn launch_sign_in(
    profile: &Profile,
    executable: Option<&[String]>,
    folder: &Path,
    os: Os,
    sys: &dyn System,
) -> Result<Box<dyn Console>, LaunchError> {
    let Some(executable) = executable.filter(|e| !e.is_empty()) else {
        return Err(LaunchError::Value("Program is not installed".into()));
    };
    let mut argv = executable.to_vec();
    argv.extend(profile.login_args.iter().map(|a| (*a).to_owned()));
    match os {
        // No pipes or inherited redirected handles: the person owns this console.
        Os::Windows => Ok(sys.console(&Launch { argv, cwd: sys.home(), env: None }, None)?),
        Os::Other => Err(LaunchError::Os("No supported sign-in terminal".into())),
        Os::MacOs => {
            let root = folder.join("ai-login");
            std::fs::create_dir_all(&root)?;
            let directory = temporary_folder(&root, "signin-")?;
            let script = directory.join("signin.command");
            let done = directory.join("closed");
            let text = format!(
                "#!/bin/sh\ntrap {} EXIT\ntrap 'exit 0' HUP INT TERM\ncd {} || exit 1\n{}\n",
                shell_quote(&format!("touch {}", shell_quote(&done.to_string_lossy()))),
                shell_quote(&sys.home().to_string_lossy()),
                argv.iter().map(|a| shell_quote(a)).collect::<Vec<_>>().join(" "),
            );
            let result = (|| -> std::io::Result<Box<dyn Console>> {
                std::fs::write(&script, text)?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700))?;
                }
                let open = vec!["open".into(), "-a".into(), "Terminal".into(), script.to_string_lossy().into_owned()];
                sys.console(&Launch { argv: open, cwd: sys.current_dir(), env: None }, Some(done.clone()))
            })();
            if result.is_err() {
                let _ = std::fs::remove_dir_all(&directory);
            }
            Ok(result?)
        }
    }
}

/// What the [`AuthController`] reports to its window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthNews {
    /// `status_ready(key, status)`.
    Status(String, AuthStatus),
    /// `login_closed(key)`.
    LoginClosed(String),
}

/// Port of `AuthController`: status checks on threads (one at a time per program, the last asked for waits) and
/// the open sign-in windows.
pub struct AuthController {
    sys: Arc<dyn System>,
    active: HashSet<String>,
    pending: HashMap<String, (Profile, Option<Vec<String>>)>,
    /// The open sign-in windows by program.
    pub logins: BTreeMap<String, Box<dyn Console>>,
    tx: Sender<(String, AuthStatus)>,
    rx: Receiver<(String, AuthStatus)>,
    /// How long a status command may take.
    pub timeout: Duration,
}

impl AuthController {
    /// A controller using `sys`.
    pub fn new(sys: Arc<dyn System>) -> Self {
        let (tx, rx) = channel();
        Self {
            sys,
            active: HashSet::new(),
            pending: HashMap::new(),
            logins: BTreeMap::new(),
            tx,
            rx,
            timeout: Duration::from_secs(5),
        }
    }

    /// Port of `check(profile, executable)`.
    pub fn check(&mut self, profile: &Profile, executable: Option<Vec<String>>) {
        if self.active.contains(profile.key) {
            self.pending.insert(profile.key.into(), (*profile, executable));
            return;
        }
        self.active.insert(profile.key.into());
        let (sys, tx, profile, timeout) = (self.sys.clone(), self.tx.clone(), *profile, self.timeout);
        std::thread::spawn(move || {
            let status = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                check_status(&profile, executable.as_deref(), sys.as_ref(), timeout)
            }))
            .unwrap_or_default();
            let _ = tx.send((profile.key.into(), status));
        });
    }

    /// Whether a check of `key` runs.
    pub fn checking(&self) -> bool {
        !self.active.is_empty()
    }

    /// Port of `login(profile, executable, folder)`.
    pub fn login(
        &mut self,
        profile: &Profile,
        executable: Option<&[String]>,
        folder: &Path,
    ) -> Result<(), LaunchError> {
        if !self.logins.contains_key(profile.key) {
            let session = launch_sign_in(profile, executable, folder, self.sys.os(), self.sys.as_ref())?;
            self.logins.insert(profile.key.into(), session);
        }
        Ok(())
    }

    /// `_checked` and `poll_logins`: what finished since the last call.
    pub fn poll(&mut self) -> Vec<AuthNews> {
        let mut news = Vec::new();
        while let Ok((key, status)) = self.rx.try_recv() {
            self.active.remove(&key);
            if let Some((profile, executable)) = self.pending.remove(&key) {
                self.check(&profile, executable);
            } else {
                news.push(AuthNews::Status(key, status));
            }
        }
        let keys: Vec<String> = self.logins.keys().cloned().collect();
        for key in keys {
            if self.logins.get_mut(&key).is_some_and(|s| s.finished()) {
                if let Some(mut session) = self.logins.remove(&key) {
                    session.cleanup();
                }
                news.push(AuthNews::LoginClosed(key));
            }
        }
        news
    }
}
