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
fn sse_exports_stdio_bridge_with_exact_windows_path() {
    let p = Profile {
        output: "sse".into(),
        route: "/sse".into(),
        api_key: "quoted\"key\\中文$${LITERAL_TOKEN}".into(),
        ..profile()
    };
    let document = exported(&p);
    let entry = entry(&document);
    assert_eq!(
        entry["command"].as_str(),
        Some(r"D:\网关 带空格\supergateway.exe")
    );
    let args = entry["args"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        args,
        vec![
            "--sse",
            "http://127.0.0.1:8000/sse",
            "--outputTransport",
            "stdio",
            "--header",
            "X-API-Key: quoted\"key\\中文${LITERAL_TOKEN}"
        ]
    );
    assert!(entry.get("url").is_none());
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

#[test]
fn exported_authenticated_sse_bridge_initializes_and_lists_tools() {
    use gateway::{manager::Manager, native::Child};
    use serde_json::{Value, json};
    use std::{
        collections::BTreeMap,
        io::{BufRead, BufReader, Write},
        sync::mpsc,
        thread,
        time::Duration,
    };
    let root =
        std::env::temp_dir().join(format!("Codex TOML 中文 测试 {}", gateway::model::new_id()));
    std::fs::create_dir_all(&root).unwrap();
    let engine = Path::new(env!("CARGO_MANIFEST_DIR")).join("engine/supergateway.exe");
    let (tx, _rx) = mpsc::sync_channel(2048);
    let manager = Manager::new(root.clone(), engine.clone(), tx);
    let p = Profile {
        command: env!("CARGO_BIN_EXE_mock-mcp").into(),
        output: "sse".into(),
        route: "/sse".into(),
        port: config::free_port(&[]).unwrap(),
        api_key: "codex-sse-test-key".into(),
        ..profile()
    };
    manager.start(&p).unwrap();
    let document: toml::Value =
        toml::from_str(&config::codex_export(&p, &engine).unwrap()).unwrap();
    let exported = entry(&document);
    let args = exported["args"]
        .as_array()
        .unwrap()
        .iter()
        .map(|arg| arg.as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    let mut bridge = Child::spawn(
        Path::new(exported["command"].as_str().unwrap()),
        &args,
        &BTreeMap::new(),
        &root,
    )
    .unwrap();
    let errors = bridge.errors.take().unwrap();
    let error_reader = thread::spawn(move || for _ in BufReader::new(errors).lines() {});
    let output = bridge.output.take().unwrap();
    let (messages, received) = mpsc::channel();
    let output_reader = thread::spawn(move || {
        for line in BufReader::new(output).lines().map_while(Result::ok) {
            let _ = messages.send(line);
        }
    });
    let mut input = bridge.input.take().unwrap();
    writeln!(input, "{}", json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"codex-export-check","version":"1"}}})).unwrap();
    input.flush().unwrap();
    let initialized: Value =
        serde_json::from_str(&received.recv_timeout(Duration::from_secs(15)).unwrap()).unwrap();
    assert!(initialized["result"]["serverInfo"].is_object());
    writeln!(
        input,
        "{}",
        json!({"jsonrpc":"2.0","method":"notifications/initialized"})
    )
    .unwrap();
    writeln!(
        input,
        "{}",
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}})
    )
    .unwrap();
    input.flush().unwrap();
    let tools: Value =
        serde_json::from_str(&received.recv_timeout(Duration::from_secs(15)).unwrap()).unwrap();
    assert_eq!(tools["result"]["tools"][0]["description"], "中文测试工具");
    drop(input);
    bridge.stop();
    manager.stop_all();
    output_reader.join().unwrap();
    error_reader.join().unwrap();
}
