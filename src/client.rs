//! Client export preferences never alter a GUI-managed gateway's environment.
use crate::{
    Result,
    model::{ClientOptions, Profile},
};
use std::{collections::HashSet, path::Path};

pub fn validate(c: &ClientOptions) -> Result<()> {
    for value in [c.startup_timeout_sec, c.tool_timeout_sec]
        .into_iter()
        .flatten()
    {
        if !(1..=86400).contains(&value) {
            return Err("客户端超时须为 1–86400 秒，留空使用客户端默认值".into());
        }
    }
    let mut names = HashSet::new();
    for item in &c.environment {
        if item.key.trim().is_empty()
            || item.key.contains(['=', '\0', '\r', '\n'])
            || item.value.contains('\0')
            || !names.insert(item.key.to_uppercase())
        {
            return Err("客户端变量名称不能为空、重复或包含 = / 换行 / NUL；值不能包含 NUL".into());
        }
    }
    Ok(())
}
pub fn table_name(p: &Profile) -> String {
    let slug: String = p
        .name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
        .take(40)
        .collect();
    let suffix: String =
        p.id.chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .take(12)
            .collect();
    format!(
        "sg_{}_{}",
        if slug.is_empty() { "service" } else { &slug },
        suffix
    )
}
pub fn append_codex_options(text: &mut String, p: &Profile) -> Result<()> {
    validate(&p.client)?;
    for (key, value) in [
        ("startup_timeout_sec", p.client.startup_timeout_sec),
        ("tool_timeout_sec", p.client.tool_timeout_sec),
    ] {
        if let Some(value) = value {
            text.push_str(&format!("{key} = {value}\r\n"));
        }
    }
    Ok(())
}
pub fn network_json(p: &Profile, engine: &Path, ca: Option<&Path>) -> Result<String> {
    // Export only the GUI gateway address and authentication.
    validate(&p.client)?;
    if !["streamableHttp", "sse"].contains(&p.output.as_str()) {
        return Err("客户端 JSON 仅支持 HTTP / SSE 网络连接，请切换输出协议并应用重启。".into());
    }
    let _ = (engine, ca);
    let key = if p.api_key.is_empty() {
        p.api_keys
            .iter()
            .find(|s| !s.is_empty())
            .map(String::as_str)
            .unwrap_or("")
    } else {
        &p.api_key
    };
    let mut entry = serde_json::json!({"type":if p.output == "sse" { "sse" } else { "http" },"url":p.address(false)});
    if !key.is_empty() {
        entry["headers"] = serde_json::json!({"X-API-Key":crate::config::expand(key)?});
    }
    Ok(serde_json::to_string_pretty(&serde_json::json!({"mcpServers":{&p.name:entry}})).unwrap())
}
