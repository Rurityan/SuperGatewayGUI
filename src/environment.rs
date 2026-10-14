//! Service-local environment policy. Never writes the parent environment or registry.
use crate::{
    Result,
    model::{Pair, Profile},
};
use std::{collections::BTreeMap, path::PathBuf, sync::OnceLock};

static SNAPSHOT: OnceLock<BTreeMap<String, String>> = OnceLock::new();
pub fn snapshot() -> &'static BTreeMap<String, String> {
    SNAPSHOT.get_or_init(|| {
        std::env::vars_os()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().to_uppercase(),
                    v.to_string_lossy().into_owned(),
                )
            })
            .collect()
    })
}
pub fn initialize() {
    let _ = snapshot();
}
pub fn sensitive_name(key: &str) -> bool {
    let key = key.to_ascii_uppercase();
    [
        "TOKEN",
        "PASSWORD",
        "SECRET",
        "API_KEY",
        "APIKEY",
        "AUTHORIZATION",
        "CREDENTIAL",
    ]
    .iter()
    .any(|word| key.contains(word))
}
const ESSENTIAL: &[&str] = &[
    "SYSTEMROOT",
    "WINDIR",
    "SYSTEMDRIVE",
    "COMSPEC",
    "TEMP",
    "TMP",
    "USERPROFILE",
    "APPDATA",
    "LOCALAPPDATA",
    "PROGRAMDATA",
    "PROGRAMFILES",
    "PROGRAMFILES(X86)",
    "PROGRAMW6432",
    "PATHEXT",
    "USERNAME",
    "USERDOMAIN",
    "OS",
];
pub fn windows_paths() -> Vec<String> {
    let root = snapshot()
        .get("SYSTEMROOT")
        .map(String::as_str)
        .unwrap_or("C:\\Windows");
    ["System32", "", "System32/WindowsPowerShell/v1.0"]
        .iter()
        .map(|p| PathBuf::from(root).join(p).to_string_lossy().into_owned())
        .collect()
}
fn directory_key(value: &str) -> String {
    value
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_uppercase()
}
fn append(paths: &mut Vec<String>, value: &str) {
    for part in value
        .split(';')
        .map(|s| s.trim().trim_matches('"'))
        .filter(|s| !s.is_empty())
    {
        if !paths
            .iter()
            .any(|p| directory_key(p) == directory_key(part))
        {
            paths.push(part.replace('/', "\\"));
        }
    }
}
pub fn path(p: &Profile) -> Result<String> {
    let explicit = p.env.iter().find(|e| e.key.eq_ignore_ascii_case("PATH"));
    if p.environment_prepared {
        return Ok(explicit.map(|e| e.value.clone()).unwrap_or_default());
    }
    let mut paths = vec![];
    let managed = p.managed || p.managed_runtime;
    let windows = windows_paths();
    if managed {
        // A prepared profile carries managed tool paths followed by Windows paths.
        if let Some(e) = explicit {
            for item in e.value.split(';') {
                if !windows
                    .iter()
                    .any(|w| directory_key(w) == directory_key(item))
                {
                    append(&mut paths, item);
                }
            }
        }
    }
    for dir in &p.additional_paths {
        append(&mut paths, &crate::config::expand(dir)?);
    }
    if managed {
        for dir in &windows {
            append(&mut paths, dir);
        }
        if p.inherit_environment {
            append(
                &mut paths,
                snapshot().get("PATH").map(String::as_str).unwrap_or(""),
            );
        }
    } else if let Some(e) = explicit {
        append(&mut paths, &crate::config::expand(&e.value)?);
    } else if p.inherit_environment {
        append(
            &mut paths,
            snapshot().get("PATH").map(String::as_str).unwrap_or(""),
        );
    } else {
        for dir in &windows {
            append(&mut paths, dir);
        }
    }
    Ok(paths.join(";"))
}
pub fn effective(p: &Profile) -> Result<BTreeMap<String, String>> {
    let mut names = std::collections::HashSet::new();
    for pair in &p.env {
        if pair.key.trim().is_empty()
            || pair.key.contains(['=', '\0'])
            || pair.value.contains('\0')
            || !names.insert(pair.key.to_uppercase())
        {
            return Err("环境变量名称不能为空、包含 = / NUL 或重复（名称不区分大小写）".into());
        }
    }
    let mut values = snapshot()
        .iter()
        .filter(|(k, _)| p.inherit_environment || ESSENTIAL.contains(&k.as_str()))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect::<BTreeMap<_, _>>();
    for e in &p.env {
        values.insert(
            e.key.to_uppercase(),
            if p.managed_runtime && crate::dependencies::operational_variable(&e.key)
                || p.environment_prepared && e.key.eq_ignore_ascii_case("PATH")
            {
                e.value.clone()
            } else {
                crate::config::expand(&e.value)?
            },
        );
    }
    values.insert("PATH".into(), path(p)?);
    Ok(values)
}
pub fn prepare(p: &Profile) -> Result<Profile> {
    let mut result = p.clone();
    let value = path(p)?;
    result.env.retain(|e| !e.key.eq_ignore_ascii_case("PATH"));
    result.env.push(Pair {
        key: "PATH".into(),
        value,
        ..Default::default()
    });
    result.environment_prepared = true;
    Ok(result)
}
pub fn preview(root: &std::path::Path, p: &Profile) -> Result<String> {
    let mut prepared = p.clone();
    let mut note = String::new();
    if p.managed {
        if let Err(error) = crate::dependencies::check_installation(root, p) {
            note = format!("\r\n尚不能启动：{error}\r\n");
        }
        // Compute the same operational variables without creating directories.
        let mut operational = crate::dependencies::preview_environment(root, &p.dependency)?;
        if p.dependency.kind == "python"
            && p.stdio.is_empty()
            && let Ok(dir) = crate::dependencies::installed_path(root, p)
        {
            operational.insert(
                "VIRTUAL_ENV".into(),
                dir.join("venv").to_string_lossy().into_owned(),
            );
            operational.insert(
                "UV_PYTHON".into(),
                dir.join("venv/Scripts/python.exe")
                    .to_string_lossy()
                    .into_owned(),
            );
            operational.insert(
                "PATH".into(),
                format!(
                    "{};{}",
                    dir.join("venv/Scripts").display(),
                    operational["PATH"]
                ),
            );
        }
        for (key, value) in operational {
            if p.env.iter().any(|e| e.key.eq_ignore_ascii_case(&key)) {
                return Err(format!(
                    "托管模式保留变量 {key}，请删除覆盖项；PATH 请使用附加程序目录。"
                ));
            }
            prepared.env.push(Pair {
                key,
                value,
                ..Default::default()
            });
        }
        prepared.managed_runtime = true;
    }
    let values = effective(&prepared)?;
    let mut text = format!(
        "继承 GUI 启动环境：{}\r\n实际启动方式：{}\r\n{}\r\n最终 PATH（由上到下）：\r\n{}\r\n\r\n变量来源（值隐藏）：\r\n",
        p.inherit_environment,
        if !p.stdio.is_empty() {
            "完整启动命令优先"
        } else if p.managed {
            "固定托管包入口"
        } else {
            "已有程序与参数"
        },
        note,
        values["PATH"].split(';').collect::<Vec<_>>().join("\r\n")
    );
    for (key, value) in values {
        let source = if key == "PATH" {
            "合并后的目录"
        } else if p.managed && crate::dependencies::operational_variable(&key) {
            "应用管理"
        } else if p.env.iter().any(|e| e.key.eq_ignore_ascii_case(&key)) {
            "服务注入"
        } else if !p.inherit_environment {
            "Windows 基础环境"
        } else {
            "GUI 启动环境"
        };
        text += &format!(
            "{key} · {source} · {}\r\n",
            if value.is_empty() {
                "空值"
            } else {
                "[隐藏]"
            }
        );
    }
    Ok(text)
}
