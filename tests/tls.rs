use gateway::{
    config,
    manager::Manager,
    model::Profile,
    native::{Child, wide},
    net, store, tls,
};
use serde_json::{Value, json};
#[path = "support/tls.rs"]
mod tls_support;
use std::{
    collections::BTreeMap,
    fs,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    sync::mpsc,
    thread,
    time::Duration,
};

struct TemporaryTrust(PathBuf);
impl Drop for TemporaryTrust {
    fn drop(&mut self) {
        let _ = tls_support::untrust(&self.0);
    }
}
fn fingerprint(port: u16, root: &Path) -> Vec<u8> {
    let pem = fs::read_to_string(tls::ca_path(root)).unwrap();
    // Use the generated test CA explicitly; never disable verification.
    let mut der_len = 0;
    let text = wide(pem);
    unsafe {
        windows_sys::Win32::Security::Cryptography::CryptStringToBinaryW(
            text.as_ptr(),
            0,
            windows_sys::Win32::Security::Cryptography::CRYPT_STRING_BASE64HEADER,
            std::ptr::null_mut(),
            &mut der_len,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        );
    }
    let mut der = vec![0; der_len as usize];
    unsafe {
        assert_ne!(
            windows_sys::Win32::Security::Cryptography::CryptStringToBinaryW(
                text.as_ptr(),
                0,
                windows_sys::Win32::Security::Cryptography::CRYPT_STRING_BASE64HEADER,
                der.as_mut_ptr(),
                &mut der_len,
                std::ptr::null_mut(),
                std::ptr::null_mut()
            ),
            0
        );
    }
    let mut roots = rustls::RootCertStore::empty();
    roots
        .add(rustls::pki_types::CertificateDer::from(der))
        .unwrap();
    let config = rustls::ClientConfig::builder_with_provider(std::sync::Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_root_certificates(roots)
    .with_no_client_auth();
    let mut conn = rustls::ClientConnection::new(
        std::sync::Arc::new(config),
        rustls::pki_types::ServerName::IpAddress(
            "127.0.0.1".parse::<std::net::IpAddr>().unwrap().into(),
        ),
    )
    .unwrap();
    let mut socket = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    while conn.is_handshaking() {
        conn.complete_io(&mut socket).unwrap();
    }
    conn.peer_certificates().unwrap()[0].to_vec()
}
fn stdio_sse(p: &Profile, engine: &Path, root: &Path) {
    let document: toml::Value = toml::from_str(
        &config::codex_export_with_ca(p, engine, Some(&tls::ca_path(root))).unwrap(),
    )
    .unwrap();
    let entry = document["mcp_servers"]
        .as_table()
        .unwrap()
        .values()
        .next()
        .unwrap();
    assert!(entry["args"][1].as_str().unwrap().starts_with("https://"));
    let args = entry["args"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    let env = entry["env"]
        .as_table()
        .unwrap()
        .iter()
        .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_owned()))
        .collect::<BTreeMap<_, _>>();
    let mut child = Child::spawn(engine, &args, &env, root).unwrap();
    let errors = child.errors.take().unwrap();
    let error_reader = thread::spawn(move || for _ in BufReader::new(errors).lines() {});
    let output = child.output.take().unwrap();
    let (tx, rx) = mpsc::channel();
    let reader = thread::spawn(move || {
        for line in BufReader::new(output).lines().map_while(Result::ok) {
            let _ = tx.send(line);
        }
    });
    let mut input = child.input.take().unwrap();
    for message in [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"tls-check","version":"1"}}}),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}),
    ] {
        writeln!(input, "{message}").unwrap();
        input.flush().unwrap();
        if message.get("id").is_some() {
            let response: Value =
                serde_json::from_str(&rx.recv_timeout(Duration::from_secs(15)).unwrap()).unwrap();
            assert!(response.get("error").is_none(), "{response}");
        }
    }
    drop(input);
    child.stop();
    reader.join().unwrap();
    error_reader.join().unwrap();
}

#[test]
fn native_https_sse_wss_share_certificate_authenticate_and_renew_without_restart() {
    let root = std::env::temp_dir().join(format!("SG TLS 协议 {}", gateway::model::new_id()));
    fs::create_dir_all(&root).unwrap();
    let engine = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("engine/supergateway.exe");
    let (tx, rx) = mpsc::sync_channel(2048);
    thread::spawn(move || while rx.recv().is_ok() {});
    let manager = Manager::new(root.clone(), engine.clone(), tx);
    let mut profiles = vec![];
    for (output, route) in [("streamableHttp", "/mcp"), ("sse", "/sse"), ("ws", "/ws")] {
        eprintln!("Starting TLS {output}");
        let p = Profile {
            command: env!("CARGO_BIN_EXE_mock-mcp").into(),
            tls: true,
            listen_address: Some("127.0.0.1".into()),
            output: output.into(),
            route: route.into(),
            port: config::free_port(&profiles).unwrap(),
            api_key: "tls-secret-only-over-encrypted-channel".into(),
            ..Default::default()
        };
        manager.start(&p).unwrap();
        profiles.push(p);
    }
    assert!(
        net::test(&profiles[0])
            .unwrap_err()
            .contains("TLS 证书验证失败"),
        "Untrusted CA must fail with an actionable message"
    );
    // Trust only this freshly generated isolated test CA, and remove it on all unwind paths.
    tls_support::trust(&root).unwrap();
    let _trust = TemporaryTrust(root.clone());
    eprintln!("Temporary test CA trusted; checking TLS protocols");
    let first = fingerprint(profiles[0].port, &root);
    for p in &profiles {
        eprintln!("Checking {}", p.output);
        assert_eq!(
            fingerprint(p.port, &root),
            first,
            "All services must share one certificate"
        );
        assert_eq!(net::test(p).unwrap()[0].description, "中文测试工具");
        let mut invalid = p.clone();
        invalid.api_key = "wrong-key".into();
        assert!(net::test(&invalid).is_err());
        let backend: Value =
            serde_json::from_slice(&fs::read(root.join(format!("runtime/{}.json", p.id))).unwrap())
                .unwrap();
        assert_eq!(backend["host"], "127.0.0.1");
        assert_ne!(backend["port"].as_u64().unwrap(), p.port as u64);
        assert!(
            config::build(p, false).is_err(),
            "No plaintext gateway JSON export for TLS service"
        );
        let http = std::net::TcpStream::connect(("127.0.0.1", p.port)).unwrap();
        http.set_write_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        (&http)
            .write_all(b"GET /_gateway_health HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .unwrap();
        http.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let mut response = [0u8; 32];
        use std::io::Read;
        let read = (&http).read(&mut response);
        assert!(
            !read.is_ok_and(|n| response[..n].starts_with(b"HTTP/")),
            "TLS port must not serve plaintext"
        );
    }
    stdio_sse(&profiles[1], &engine, &root);
    let mut held = net::Session::connect(&profiles[2], Duration::from_secs(25)).unwrap();
    held.tools().unwrap();
    let pids = profiles
        .iter()
        .map(|p| manager.pid(&p.id).unwrap())
        .collect::<Vec<_>>();
    let record = root.join("tls/server.dat");
    let mut cert: Value =
        serde_json::from_slice(&store::protect(&fs::read(&record).unwrap(), false).unwrap())
            .unwrap();
    cert["expires"] = json!(time::OffsetDateTime::now_utc().unix_timestamp() + 86400);
    store::atomic_write(
        &record,
        &store::protect(&serde_json::to_vec(&cert).unwrap(), true).unwrap(),
    )
    .unwrap();
    manager.refresh_tls(true).unwrap();
    let renewed = fingerprint(profiles[0].port, &root);
    assert_ne!(renewed, first);
    for (index, p) in profiles.iter().enumerate() {
        assert_eq!(manager.pid(&p.id).unwrap(), pids[index]);
        assert_eq!(fingerprint(p.port, &root), renewed);
    }
    assert_eq!(
        held.tools().unwrap().len(),
        1,
        "Existing WSS session survives reissue"
    );
    manager.stop(&profiles[0]);
    assert_eq!(net::test(&profiles[1]).unwrap().len(), 1);
    assert!(std::net::TcpListener::bind(("127.0.0.1", profiles[0].port)).is_ok());
    manager.stop_all();
    tls_support::untrust(&root).unwrap();
    assert!(net::test(&profiles[1]).is_err());
}
