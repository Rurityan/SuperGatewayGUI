use crate::{Result, config};
use std::{
    path::{Path, PathBuf},
    sync::OnceLock,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Pwsh,
    PowerShell,
    Cmd,
}
impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Pwsh => "pwsh 7",
            Self::PowerShell => "Windows PowerShell",
            Self::Cmd => "cmd.exe",
        }
    }
}
#[derive(Debug, Clone)]
pub struct Shell {
    pub kind: Kind,
    pub path: PathBuf,
}

fn find(name: &str, extra: Vec<PathBuf>, accept: impl Fn(&Path) -> bool) -> Option<PathBuf> {
    config::resolve(name)
        .ok()
        .into_iter()
        .chain(extra)
        .find(|p| p.is_file() && accept(p))
}
fn is_pwsh7(path: &Path) -> bool {
    use windows_sys::Win32::Storage::FileSystem::*;
    unsafe {
        let path = crate::native::wide(path);
        let size = GetFileVersionInfoSizeW(path.as_ptr(), std::ptr::null_mut());
        if size == 0 || size > 1024 * 1024 {
            return false;
        }
        let mut buffer = vec![0u64; (size as usize).div_ceil(8)];
        if GetFileVersionInfoW(path.as_ptr(), 0, size, buffer.as_mut_ptr().cast()) == 0 {
            return false;
        }
        let mut value = std::ptr::null_mut();
        let mut length = 0;
        if VerQueryValueW(
            buffer.as_ptr().cast(),
            crate::native::wide("\\").as_ptr(),
            &mut value,
            &mut length,
        ) == 0
            || value.is_null()
            || (length as usize) < std::mem::size_of::<VS_FIXEDFILEINFO>()
        {
            return false;
        }
        let version = &*value.cast::<VS_FIXEDFILEINFO>();
        version.dwSignature == 0xfeef04bd && version.dwFileVersionMS >> 16 == 7
    }
}
pub fn detect() -> Result<Shell> {
    static SELECTED: OnceLock<Result<Shell>> = OnceLock::new();
    SELECTED.get_or_init(discover).clone()
}
fn discover() -> Result<Shell> {
    let pwsh = ["ProgramW6432", "ProgramFiles", "LOCALAPPDATA"]
        .into_iter()
        .filter_map(|key| std::env::var_os(key).map(PathBuf::from))
        .map(|root| root.join("PowerShell/7/pwsh.exe"))
        .collect();
    if let Some(path) = find("pwsh.exe", pwsh, is_pwsh7) {
        return Ok(Shell {
            kind: Kind::Pwsh,
            path,
        });
    }
    let root = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .unwrap_or_else(|| "C:/Windows".into());
    if let Some(path) = find(
        "powershell.exe",
        vec![root.join("System32/WindowsPowerShell/v1.0/powershell.exe")],
        |_| true,
    ) {
        return Ok(Shell {
            kind: Kind::PowerShell,
            path,
        });
    }
    if let Some(path) = find("cmd.exe", vec![root.join("System32/cmd.exe")], |_| true) {
        return Ok(Shell {
            kind: Kind::Cmd,
            path,
        });
    }
    Err("找不到命令解释器：请安装 PowerShell 7，或检查 Windows PowerShell / cmd.exe。".into())
}
pub(crate) fn powershell_command(shell: &Shell, line: &str) -> (PathBuf, Vec<String>) {
    // A quoted executable path needs PowerShell's invocation operator.
    let line = line.trim();
    // npm installs both .ps1 and .cmd launchers. Prefer the native batch
    // launcher so a normal npx command does not depend on script policy.
    let first = line.split_whitespace().next().unwrap_or("");
    let normalized = if ["npx", "npm"].contains(&first.to_lowercase().as_str()) {
        format!("{}.cmd{}", first, &line[first.len()..])
    } else {
        line.into()
    };
    let line = normalized.as_str();
    let invocation = if line.starts_with(['\'', '"']) {
        format!("& {line}")
    } else {
        line.into()
    };
    let script = format!(
        "$ErrorActionPreference='Stop'; $ProgressPreference='SilentlyContinue'; [Console]::InputEncoding=[Text.UTF8Encoding]::new($false); [Console]::OutputEncoding=[Text.UTF8Encoding]::new($false); $OutputEncoding=[Text.UTF8Encoding]::new($false); $global:LASTEXITCODE=0; {invocation}\nexit $LASTEXITCODE"
    );
    let bytes = script
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect::<Vec<_>>();
    (
        shell.path.clone(),
        vec![
            "-NoLogo".into(),
            "-NoProfile".into(),
            "-NonInteractive".into(),
            "-EncodedCommand".into(),
            base64(&bytes),
        ],
    )
}
pub fn configure(entry: &mut serde_json::Value, shell: &Shell, line: &str) {
    let object = entry.as_object_mut().expect("MCP entry object");
    object.remove("stdio");
    object.remove("command");
    object.remove("args");
    if shell.kind == Kind::Cmd {
        // Node's shell mode handles cmd's special /c quoting. The gateway
        // environment pins COMSPEC to the interpreter selected at startup.
        object.insert("stdio".into(), serde_json::json!(line));
    } else {
        let (command, args) = powershell_command(shell, line);
        object.insert("command".into(), serde_json::json!(command));
        object.insert("args".into(), serde_json::json!(args));
    }
}
fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut result = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let a = chunk[0] as usize;
        let b = chunk.get(1).copied().unwrap_or(0) as usize;
        let c = chunk.get(2).copied().unwrap_or(0) as usize;
        result.push(TABLE[a >> 2] as char);
        result.push(TABLE[((a & 3) << 4) | (b >> 4)] as char);
        result.push(if chunk.len() > 1 {
            TABLE[((b & 15) << 2) | (c >> 6)] as char
        } else {
            '='
        });
        result.push(if chunk.len() > 2 {
            TABLE[c & 63] as char
        } else {
            '='
        });
    }
    result
}

pub fn check_dependency(line: &str) -> Result<()> {
    check_dependency_with_path(line, &std::env::var("PATH").unwrap_or_default())
}
pub fn check_dependency_with_path(line: &str, search: &str) -> Result<()> {
    let line = line.trim().strip_prefix('&').unwrap_or(line.trim()).trim();
    let first = if let Some(quote) = line.chars().next().filter(|c| *c == '\'' || *c == '"') {
        line[1..].split(quote).next().unwrap_or("")
    } else {
        line.split_whitespace().next().unwrap_or("")
    };
    // General shell expressions cannot be reliably validated by tokenizing.
    // Detect common missing MCP runtimes and explicit program paths only.
    if [
        "npx", "npx.cmd", "npm", "npm.cmd", "uvx", "uv", "python", "python3", "node",
    ]
    .contains(&first.to_lowercase().as_str())
        || Path::new(first).is_absolute()
    {
        config::resolve_with_path(first, search)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn encoded_command_base64() {
        for (input, expected) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64(input.as_bytes()), expected);
        }
    }
}
