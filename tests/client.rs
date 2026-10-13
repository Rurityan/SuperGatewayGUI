use gateway::{
    client, config,
    model::{ClientOptions, ClientVariable, Profile},
    store::Store,
};
use std::path::Path;
fn variable(key: &str, value: &str, enabled: bool) -> ClientVariable {
    ClientVariable {
        key: key.into(),
        value: value.into(),
        enabled,
    }
}
fn profile() -> Profile {
    Profile {
        command: "server.exe".into(),
        listen_address: Some("127.0.0.1".into()),
        ..Default::default()
    }
}
#[test]
fn settings_restore_disabled_variables_without_changing_gateway() {
    let old: Profile = serde_json::from_str("{}").unwrap();
    assert_eq!(old.client, ClientOptions::default());
    let mut p = profile();
    let before = config::build(&p, false).unwrap();
    let original = p.clone();
    p.client.environment = vec![
        variable("ON", "$${literal} 中文", true),
        variable("OFF", "private disabled", false),
    ];
    p.client.startup_timeout_sec = Some(15);
    p.client.tool_timeout_sec = Some(300);
    p.client.stdio = true;
    assert!(p.gateway_eq(&original));
    assert_eq!(before.0, config::build(&p, false).unwrap().0);
    assert_eq!(before.1, config::build(&p, false).unwrap().1);
    let dir = std::env::temp_dir().join(format!("客户端配置 中文 {}", gateway::model::new_id()));
    let store = Store::new(dir.clone()).unwrap();
    store
        .save(&gateway::model::Settings {
            services: vec![p.clone()],
            ..Default::default()
        })
        .unwrap();
    assert_eq!(store.load().unwrap().services[0], p);
    assert!(
        !String::from_utf8_lossy(&std::fs::read(dir.join("settings.dat")).unwrap())
            .contains("private disabled")
    );
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn formats_apply_supported_timeouts_environment_and_literal_values() {
    let mut p = profile();
    p.client.startup_timeout_sec = Some(20);
    p.client.tool_timeout_sec = Some(300);
    p.client
        .environment
        .push(variable("DISABLED", "do not export", false));
    let text = config::codex_export(&p, Path::new("engine.exe")).unwrap();
    let document: toml::Value = toml::from_str(&text).unwrap();
    let entry = document["mcp_servers"]
        .as_table()
        .unwrap()
        .values()
        .next()
        .unwrap();
    assert_eq!(entry["startup_timeout_sec"].as_integer(), Some(20));
    assert_eq!(entry["tool_timeout_sec"].as_integer(), Some(300));
    assert!(entry.get("env").is_none() && entry.get("command").is_none());
    p.client
        .environment
        .push(variable("CLIENT_ONLY", "private", true));
    p.client.stdio = true;
    assert!(
        !config::codex_export(&p, Path::new("engine.exe"))
            .unwrap()
            .contains("private")
    );
    assert!(
        !client::network_json(&p, Path::new("engine.exe"), None)
            .unwrap()
            .contains("private")
    );
    p.output = "sse".into();
    assert!(
        config::codex_export(&p, Path::new("engine.exe"))
            .unwrap_err()
            .contains("Streamable HTTP")
    );
    assert!(p.env.is_empty());
    let json: serde_json::Value =
        serde_json::from_str(&client::network_json(&p, Path::new("engine.exe"), None).unwrap())
            .unwrap();
    let entry = &json["mcpServers"][&p.name];
    assert_eq!(entry["type"], "sse");
    assert_eq!(entry["url"], p.address(false));
    assert!(entry.get("command").is_none() && entry.get("env").is_none());
}
#[test]
fn invalid_variables_timeouts_and_managed_overrides_are_rejected() {
    let mut c = ClientOptions {
        tool_timeout_sec: Some(0),
        ..Default::default()
    };
    assert!(client::validate(&c).is_err());
    c.tool_timeout_sec = Some(86401);
    assert!(client::validate(&c).is_err());
    c.tool_timeout_sec = None;
    for key in ["", "A=B", "A\nB"] {
        c.environment = vec![variable(key, "", true)];
        assert!(client::validate(&c).is_err());
    }
    c.environment = vec![variable("Name", "", true), variable("NAME", "", false)];
    assert!(client::validate(&c).is_err());
}
