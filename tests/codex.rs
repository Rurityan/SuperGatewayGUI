use gateway::{config, model::Profile};
use std::path::Path;

fn exported(profile: &Profile) -> toml::Value {
    toml::from_str(
        &config::codex_export(profile, Path::new(r"D:\网关 带空格\supergateway.exe")).unwrap(),
    )
    .unwrap()
}
fn entry(document: &toml::Value) -> &toml::Value {
    document["mcp_servers"]
        .as_table()
        .unwrap()
        .values()
        .next()
        .unwrap()
}
fn profile() -> Profile {
    Profile {
        name: "中文 服务\".\\特殊".into(),
        listen_address: Some("127.0.0.1".into()),
        ..Default::default()
    }
}
#[test]
fn http_auth_and_name_are_valid_toml() {
    let p = Profile {
        api_key: "中文 key\"\\value".into(),
        bearer: "upstream-token-must-not-export".into(),
        ..profile()
    };
    let document = exported(&p);
    assert_eq!(
        entry(&document)["url"].as_str(),
        Some("http://127.0.0.1:8000/mcp")
    );
    assert_eq!(
        entry(&document)["http_headers"]["X-API-Key"].as_str(),
        Some(p.api_key.as_str())
    );
    let name = document["mcp_servers"]
        .as_table()
        .unwrap()
        .keys()
        .next()
        .unwrap();
    assert!(
        name.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    );
    assert!(
        !config::codex_export(&p, Path::new("engine.exe"))
            .unwrap()
            .contains(&p.bearer)
    );
}
#[test]
fn optional_auth_uses_extra_key_and_omits_empty_headers() {
    assert!(entry(&exported(&profile())).get("http_headers").is_none());
    let p = Profile {
        api_keys: vec![String::new(), "extra-key".into()],
        ..profile()
    };
    assert_eq!(
        entry(&exported(&p))["http_headers"]["X-API-Key"].as_str(),
        Some("extra-key")
    );
}
#[test]
fn sse_export_requires_http_without_creating_stdio_bridge() {
    let p = Profile {
        output: "sse".into(),
        ..profile()
    };
    let error = config::codex_export(&p, Path::new("engine.exe")).unwrap_err();
    assert!(error.contains("Streamable HTTP"));
}
#[test]
fn websocket_has_actionable_error() {
    let p = Profile {
        output: "ws".into(),
        ..profile()
    };
    let error = config::codex_export(&p, Path::new("engine.exe")).unwrap_err();
    assert!(error.contains("Streamable HTTP") && error.contains("保存"));
}
