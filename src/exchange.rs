//! Transferable service configuration, separate from DPAPI application storage.
use crate::{
    Result, config,
    model::{ClientVariable, Pair, Profile, new_id},
};
use serde_json::{Value, json};

pub fn export(profiles: &[Profile]) -> Result<String> {
    if profiles.is_empty() {
        return Err("请至少勾选一个服务".into());
    }
    serde_json::to_string_pretty(&json!({
        "Format": "SuperGatewayGUI", "Version": 1, "Services": profiles
    }))
    .map_err(|e| e.to_string())
}

fn known_fields(value: &Value, template: &Value, path: &str) -> Result<()> {
    if let (Some(obj), Some(fields)) = (value.as_object(), template.as_object()) {
        for (key, v) in obj {
            let t = fields
                .get(key)
                .ok_or_else(|| format!("不支持字段 {path}.{key}"))?;
            known_fields(v, t, &format!("{path}.{key}"))?;
        }
    }
    Ok(())
}

pub fn import(text: &str, existing: &[Profile]) -> Result<config::Import> {
    let root: Value =
        jsonc_parser::parse_to_serde_value(text, &Default::default()).map_err(|e| e.to_string())?;
    if root.get("Format").is_none() && root.get("Services").is_none() {
        return config::import(text, existing);
    }
    if root.get("Format").and_then(Value::as_str) != Some("SuperGatewayGUI")
        || root.get("Version").and_then(Value::as_u64) != Some(1)
    {
        return Err("不是受支持的 SuperGatewayGUI 配置文件（需要 Version 1）".into());
    }
    known_fields(
        &root,
        &json!({"Format":"", "Version":1, "Services":[]}),
        "配置",
    )?;
    let rows = root
        .get("Services")
        .and_then(Value::as_array)
        .ok_or("Services 必须为数组")?;
    let mut all = existing.to_vec();
    let mut profiles = Vec::new();
    let mut messages = Vec::new();
    let template = serde_json::to_value(Profile::default()).map_err(|e| e.to_string())?;
    for (index, row) in rows.iter().enumerate() {
        let label = row.get("Name").and_then(Value::as_str).unwrap_or("未命名");
        let result = (|| -> Result<Profile> {
            if !row.is_object() {
                return Err("服务必须为对象".into());
            }
            known_fields(row, &template, "服务")?;
            for field in ["Env", "Headers"] {
                if let Some(items) = row.get(field).and_then(Value::as_array) {
                    for item in items {
                        known_fields(item, &serde_json::to_value(Pair::default()).unwrap(), field)?;
                    }
                }
            }
            if let Some(items) = row.pointer("/Client/Environment").and_then(Value::as_array) {
                for item in items {
                    known_fields(
                        item,
                        &serde_json::to_value(ClientVariable::default()).unwrap(),
                        "Client.Environment",
                    )?;
                }
            }
            let mut p: Profile = serde_json::from_value(row.clone())
                .map_err(|_| "服务字段类型无效：请检查字符串、数组、数值和布尔类型".to_string())?;
            config::validate(&p, false)?;
            p.id = new_id();
            p.dependency.installed.clear();
            let original = p.name.clone();
            let mut suffix = 2;
            while all.iter().any(|other| other.name == p.name) {
                p.name = format!("{original} ({suffix})");
                suffix += 1;
            }
            if p.name != original {
                messages.push(format!("{original}：重名，保存为 {}", p.name));
            }
            if all.iter().any(|other| other.port == p.port) || !config::port_available(p.port, true)
            {
                let old = p.port;
                p.port = config::free_port(&all)?;
                messages.push(format!("{}：端口 {old} 冲突，改为 {}", p.name, p.port));
            }
            if p.managed {
                messages.push(format!(
                    "{}：保留包规格，需在本机安装依赖；不复制安装目录",
                    p.name
                ));
            }
            if p.tls {
                messages.push(format!("{}：使用本机证书，客户端需信任本机 CA", p.name));
            }
            Ok(p)
        })();
        match result {
            Ok(p) => {
                all.push(p.clone());
                profiles.push(p);
            }
            Err(e) => messages.push(format!("第 {} 项 {label}：无法导入：{e}", index + 1)),
        }
    }
    messages.insert(
        0,
        format!(
            "识别 {} 项，可选择 {} 项。仅新增配置，不覆盖、不启动、不安装。",
            rows.len(),
            profiles.len()
        ),
    );
    Ok(config::Import { profiles, messages })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sample() -> Profile {
        Profile {
            name: "中文 MCP".into(),
            command: "not-installed.exe".into(),
            args: vec!["带 空格".into()],
            env: vec![Pair {
                key: "SECRET".into(),
                value: "secret".into(),
            }],
            headers: vec![Pair {
                key: "Authorization".into(),
                value: "Bearer secret".into(),
            }],
            api_key: "key".into(),
            api_keys: vec!["extra".into()],
            bearer: "token".into(),
            ..Profile::default()
        }
    }
    #[test]
    fn complete_round_trip() {
        let mut p = sample();
        p.inherit_environment = false;
        p.additional_paths = vec![r"D:\中文 路径".into()];
        p.client.tool_timeout_sec = Some(300);
        p.client.startup_timeout_sec = Some(30);
        p.tls = true;
        p.stateful = true;
        p.session_timeout = 12345;
        let mut imported = import(&export(&[p.clone()]).unwrap(), &[])
            .unwrap()
            .profiles
            .remove(0);
        assert_ne!(p.id, imported.id);
        imported.id = p.id.clone();
        p.port = imported.port;
        assert_eq!(p, imported);
    }
    #[test]
    fn duplicates_do_not_replace() {
        let p = sample();
        let result = import(
            &export(&[p.clone(), p.clone()]).unwrap(),
            std::slice::from_ref(&p),
        )
        .unwrap();
        assert_eq!(result.profiles.len(), 2);
        assert_eq!(result.profiles[0].name, "中文 MCP (2)");
        assert_eq!(result.profiles[1].name, "中文 MCP (3)");
        assert_ne!(result.profiles[0].port, result.profiles[1].port);
        assert_ne!(result.profiles[0].id, p.id);
    }
    #[test]
    fn errors_and_versions() {
        let mut value: Value =
            serde_json::from_str(&export(&[sample(), sample()]).unwrap()).unwrap();
        value["Services"][0]["Unknown"] = json!(true);
        let result = import(&value.to_string(), &[]).unwrap();
        assert_eq!(result.profiles.len(), 1);
        assert!(result.messages.join(" ").contains("Unknown"));
        value["Version"] = json!(2);
        assert!(import(&value.to_string(), &[]).is_err());
        assert!(export(&[]).is_err());
    }
    #[test]
    fn managed_spec_without_installation() {
        let mut p = sample();
        p.command.clear();
        p.managed = true;
        p.dependency.kind = "python".into();
        p.dependency.package = "mcp-for-blender".into();
        p.dependency.installed = "old-install".into();
        let result = import(&export(&[p]).unwrap(), &[]).unwrap();
        assert_eq!(result.profiles.len(), 1);
        assert!(result.profiles[0].dependency.installed.is_empty());
        assert_eq!(result.profiles[0].dependency.package, "mcp-for-blender");
    }
    #[test]
    fn generic_import_preserved() {
        assert_eq!(
            import(r#"{"mcpServers":{"test":{"command":"missing.exe"}}}"#, &[])
                .unwrap()
                .profiles
                .len(),
            1
        );
    }
    #[test]
    fn jsonc_and_sensitive_type_errors() {
        let mut value: Value = serde_json::from_str(&export(&[sample()]).unwrap()).unwrap();
        assert_eq!(
            import(&format!("// backup\n{value}"), &[])
                .unwrap()
                .profiles
                .len(),
            1
        );
        value["Services"][0]["Env"] = json!("must-not-appear-in-preview");
        let result = import(&value.to_string(), &[]).unwrap();
        assert!(result.profiles.is_empty());
        assert!(!result.messages.join(" ").contains("must-not-appear"));
    }
}
