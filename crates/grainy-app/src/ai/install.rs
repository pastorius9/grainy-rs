//! The programs' official installers, run only after the person confirmed the command, in a console they own
//! (port of luma/ai_install.py). The commands are the vendors' fixed instructions, never the person's input.

use std::collections::BTreeMap;
use std::path::Path;

use super::auth::{shell_quote, temporary_folder, LaunchError};
use super::discover;
use super::system::{Console, Launch, Os, System};
use crate::text::tr;

/// Port of `Installer`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Installer {
    /// The program (through `tr`).
    pub name: &'static str,
    /// Who provides it.
    pub vendor: &'static str,
    /// The command that is run.
    pub command: &'static str,
    /// The official instructions.
    pub source: &'static str,
    /// `powershell` or `cmd` (Windows).
    pub shell: &'static str,
    /// npm installs need Node.js first.
    pub needs_node: bool,
}

/// `CLAUDE_SOURCE`.
pub const CLAUDE_SOURCE: &str = "https://code.claude.com/docs/en/setup";
/// `CODEX_SOURCE`.
pub const CODEX_SOURCE: &str = "https://github.com/openai/codex#installing-and-running-codex-cli";
/// `GEMINI_SOURCE`.
pub const GEMINI_SOURCE: &str = "https://github.com/google-gemini/gemini-cli#installation";
/// `NODE_URL`.
pub const NODE_URL: &str = "https://nodejs.org/en/download";

const fn installer(name: &'static str, vendor: &'static str, command: &'static str, source: &'static str) -> Installer {
    Installer { name, vendor, command, source, shell: "powershell", needs_node: false }
}

const GEMINI: Installer = Installer {
    name: "Gemini CLI",
    vendor: "Google (npm)",
    command: "npm install -g @google/gemini-cli",
    source: GEMINI_SOURCE,
    shell: "cmd",
    needs_node: true,
};

/// Port of `INSTALLERS`: (program, system, installer).
pub static INSTALLERS: [(&str, Os, Installer); 6] = [
    (
        "claude",
        Os::Windows,
        installer("Claude Code", "Anthropic", "irm https://claude.ai/install.ps1 | iex", CLAUDE_SOURCE),
    ),
    (
        "claude",
        Os::MacOs,
        installer("Claude Code", "Anthropic", "curl -fsSL https://claude.ai/install.sh | bash", CLAUDE_SOURCE),
    ),
    (
        "codex",
        Os::Windows,
        installer(
            "Codex",
            "OpenAI",
            "powershell -ExecutionPolicy ByPass -c \"irm https://chatgpt.com/codex/install.ps1 | iex\"",
            CODEX_SOURCE,
        ),
    ),
    (
        "codex",
        Os::MacOs,
        installer("Codex", "OpenAI", "curl -fsSL https://chatgpt.com/codex/install.sh | sh", CODEX_SOURCE),
    ),
    ("gemini", Os::Windows, GEMINI),
    ("gemini", Os::MacOs, GEMINI),
];

/// Port of `NODE_WINDOWS`.
pub const NODE_WINDOWS: Installer = Installer {
    name: "Node.js LTS",
    vendor: "OpenJS Foundation (winget)",
    command: "winget install OpenJS.NodeJS.LTS",
    source: NODE_URL,
    shell: "cmd",
    needs_node: false,
};

/// Port of `ps_quote`.
pub fn ps_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

/// The PowerShell command line of a Windows install (`launch_install`'s `script`): the installer in a child shell,
/// then a pause that an `exit` inside it cannot skip.
pub fn windows_script(installer: &Installer) -> String {
    let pause = tr("결과를 확인한 뒤 아무 키나 누르면 이 설치 실행을 마칩니다.");
    let child = if installer.shell == "powershell" {
        // Windows PowerShell can strip embedded quotes when forwarding arguments: the fixed command is encoded.
        let utf16: Vec<u8> = installer.command.encode_utf16().flat_map(u16::to_le_bytes).collect();
        format!("& powershell.exe -NoProfile -EncodedCommand {}", grainy_mcp::b64::encode(&utf16))
    } else {
        format!("& cmd.exe /d /c {}", ps_quote(installer.command))
    };
    format!(
        "try {{ {child} }} finally {{ Write-Host {}; $null = $Host.UI.RawUI.ReadKey('NoEcho,IncludeKeyDown') }}",
        ps_quote(&format!("\n{pause}"))
    )
}

/// Port of `launch_install(installer, folder, platform)`.
pub fn launch_install(
    installer: &Installer,
    folder: &Path,
    os: Os,
    sys: &dyn System,
) -> Result<Box<dyn Console>, LaunchError> {
    if os == Os::Other {
        return Err(LaunchError::Os("No supported installation terminal".into()));
    }
    let root = folder.join("ai-install");
    std::fs::create_dir_all(&root)?;
    let env = discover::install_environment(sys, os);
    if os == Os::Windows {
        let argv = ["powershell.exe", "-NoProfile", "-Command"].map(str::to_owned).to_vec();
        let argv = [argv, vec![windows_script(installer)]].concat();
        return Ok(sys.console(&Launch { argv, cwd: root, env: Some(env) }, None)?);
    }
    let path = env.iter().find(|(k, _)| k == "PATH").map(|(_, v)| v.clone()).unwrap_or_default();
    let directory = temporary_folder(&root, "install-")?;
    let script = directory.join("install.command");
    let done = directory.join("closed");
    let pause = tr("결과를 확인한 뒤 아무 키나 누르면 이 설치 실행을 마칩니다.");
    let text = format!(
        "#!/bin/bash\nset -o pipefail\ntrap {} EXIT\ntrap 'exit 0' HUP INT TERM\nexport PATH={}\ncd {} || exit 1\n(\n{}\n)\nresult=$?\nprintf '\\n%s\\n' {}\nread -r -n 1\nexit \"$result\"\n",
        shell_quote(&format!("touch {}", shell_quote(&done.to_string_lossy()))),
        shell_quote(&path),
        shell_quote(&root.to_string_lossy()),
        installer.command,
        shell_quote(&pause),
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

/// Port of `InstallController`: at most one install at a time (never two global npm installs).
#[derive(Default)]
pub struct InstallController {
    /// The open install windows by program.
    pub sessions: BTreeMap<String, Box<dyn Console>>,
}

impl InstallController {
    /// Port of `start(key, installer, folder)`: false when an install is open already.
    pub fn start(
        &mut self,
        key: &str,
        installer: &Installer,
        folder: &Path,
        sys: &dyn System,
    ) -> Result<bool, LaunchError> {
        if !self.sessions.is_empty() {
            return Ok(false);
        }
        let session = launch_install(installer, folder, sys.os(), sys)?;
        self.sessions.insert(key.into(), session);
        Ok(true)
    }

    /// Port of `poll`: the programs whose install window closed.
    pub fn poll(&mut self) -> Vec<String> {
        let keys: Vec<String> = self.sessions.keys().cloned().collect();
        let mut closed = Vec::new();
        for key in keys {
            if self.sessions.get_mut(&key).is_some_and(|s| s.finished()) {
                if let Some(mut session) = self.sessions.remove(&key) {
                    session.cleanup();
                }
                closed.push(key);
            }
        }
        closed
    }
}
