//! Controlled npm registry: real npm installs, no external packages or lifecycle scripts.
use gateway::{
    dependencies::{self as deps, Network, ServiceDependency, Task},
    manager::Manager,
    model::Profile,
};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    io::{Read, Write},
    net::TcpListener,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};
fn base64(bytes: &[u8]) -> String {
    let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for c in bytes.chunks(3) {
        let a = c[0];
        let b = c.get(1).copied().unwrap_or(0);
        let d = c.get(2).copied().unwrap_or(0);
        out.push(alphabet[(a >> 2) as usize] as char);
        out.push(alphabet[((a & 3) << 4 | b >> 4) as usize] as char);
        out.push(if c.len() > 1 {
            alphabet[((b & 15) << 2 | d >> 6) as usize] as char
        } else {
            '='
        });
        out.push(if c.len() > 2 {
            alphabet[(d & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}
fn archive(version: &str) -> Vec<u8> {
    let mut tar = tar::Builder::new(flate2::write::GzEncoder::new(
        vec![],
        flate2::Compression::fast(),
    ));
    let metadata=serde_json::to_vec(&json!({"name":"sgui-controlled-mcp","version":version,"bin":{"sgui-controlled-mcp":"bin.js"}})).unwrap();
    let script = format!(
        r#"#!/usr/bin/env node
const rl=require('node:readline').createInterface({{input:process.stdin}});
rl.on('line',line=>{{const q=JSON.parse(line);if(q.id===undefined)return;let result={{}};
if(q.method==='initialize')result={{protocolVersion:'2024-11-05',capabilities:{{tools:{{}}}},serverInfo:{{name:'controlled-mcp',version:'{version}'}}}};
if(q.method==='tools/list')result={{tools:[{{name:'version_{}',description:'Controlled version {version}',inputSchema:{{type:'object',properties:{{}}}}}}]}};
process.stdout.write(JSON.stringify({{jsonrpc:'2.0',id:q.id,result}})+'\n');}});
"#,
        version.replace('.', "_")
    );
    for (name, bytes) in [
        ("package/package.json", metadata.as_slice()),
        ("package/bin.js", script.as_bytes()),
    ] {
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        tar.append_data(&mut header, name, bytes).unwrap();
    }
    tar.into_inner().unwrap().finish().unwrap()
}
pub fn verify_versions(root: &Path, task: &Task, manager: &Arc<Manager>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    listener.set_nonblocking(true).unwrap();
    let one = archive("1.0.0");
    let two = archive("2.0.0");
    let mut versions = serde_json::Map::new();
    for (version, data) in [("1.0.0", &one), ("2.0.0", &two)] {
        versions.insert(version.into(),json!({"name":"sgui-controlled-mcp","version":version,"bin":{"sgui-controlled-mcp":"bin.js"},"dist":{"tarball":format!("{base}/{version}.tgz"),"integrity":format!("sha256-{}",base64(&Sha256::digest(data)))}}));
    }
    let metadata = serde_json::to_vec(
        &json!({"name":"sgui-controlled-mcp","dist-tags":{"latest":"2.0.0"},"versions":versions}),
    )
    .unwrap();
    let routes = HashMap::from([
        ("/sgui-controlled-mcp".to_string(), metadata),
        ("/1.0.0.tgz".into(), one),
        ("/2.0.0.tgz".into(), two),
    ]);
    let stop = Arc::new(AtomicBool::new(false));
    let stop2 = stop.clone();
    let server = thread::spawn(move || {
        while !stop2.load(Ordering::SeqCst) {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    stream
                        .set_read_timeout(Some(Duration::from_secs(3)))
                        .unwrap();
                    let mut request = vec![];
                    let mut buffer = [0; 2048];
                    while !request.windows(4).any(|v| v == b"\r\n\r\n") && request.len() < 16384 {
                        match stream.read(&mut buffer) {
                            Ok(0) | Err(_) => break,
                            Ok(n) => request.extend_from_slice(&buffer[..n]),
                        }
                    }
                    let line = String::from_utf8_lossy(&request);
                    let path = line
                        .split_whitespace()
                        .nth(1)
                        .unwrap_or("")
                        .split('?')
                        .next()
                        .unwrap_or("");
                    let (status, body) = if let Some(body) = routes.get(path) {
                        ("200 OK", body.as_slice())
                    } else {
                        ("404 Not Found", b"{}".as_slice())
                    };
                    let header = format!(
                        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nContent-Type: application/octet-stream\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(header.as_bytes());
                    let _ = stream.write_all(body);
                }
                Err(_) => thread::sleep(Duration::from_millis(20)),
            }
        }
    });
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let network = Network {
            npm_registry: base,
            ..Default::default()
        };
        let mut fixtures = vec![];
        for version in ["1.0.0", "2.0.0"] {
            let mut p = Profile {
                name: format!("控制包 {version}"),
                managed: true,
                port: gateway::config::free_port(
                    &manager
                        .active_profiles()
                        .into_iter()
                        .chain(fixtures.iter().cloned())
                        .collect::<Vec<_>>(),
                )
                .unwrap(),
                listen_address: Some("127.0.0.1".into()),
                dependency: ServiceDependency {
                    package: "sgui-controlled-mcp".into(),
                    version: version.into(),
                    entry: "sgui-controlled-mcp".into(),
                    ..Default::default()
                },
                ..Default::default()
            };
            p.dependency = task
                .execute(|| {
                    deps::install_service(root, &p, &network, task, &|line| {
                        println!("FIXTURE: {line}")
                    })
                })
                .unwrap();
            manager.start(&p).unwrap();
            assert_eq!(
                gateway::net::test(&p).unwrap()[0].name,
                format!("version_{}", version.replace('.', "_"))
            );
            fixtures.push(p);
        }
        let first_pid = manager.pid(&fixtures[0].id);
        let second_pid = manager.pid(&fixtures[1].id);
        let mut updated = fixtures[0].clone();
        updated.dependency.version = "2.0.0".into();
        updated.dependency = task
            .execute(|| deps::install_service(root, &updated, &network, task, &|_| {}))
            .unwrap();
        assert_eq!(manager.pid(&fixtures[0].id), first_pid);
        assert_eq!(
            gateway::net::test(&fixtures[0]).unwrap()[0].name,
            "version_1_0_0"
        );
        manager.stop(&fixtures[0]);
        manager.start(&updated).unwrap();
        assert_eq!(
            gateway::net::test(&updated).unwrap()[0].name,
            "version_2_0_0"
        );
        assert_eq!(manager.pid(&fixtures[1].id), second_pid);
        assert_eq!(
            gateway::net::test(&fixtures[1]).unwrap()[0].name,
            "version_2_0_0"
        );
        manager.stop(&updated);
        manager.stop(&fixtures[1]);
        for p in fixtures {
            deps::remove_service(root, &p.id, &manager.active_profiles()).unwrap();
        }
        println!(
            "VERIFIED two isolated npm versions 1.0.0 / 2.0.0 and upgrade with unchanged second gateway PID"
        );
    }));
    stop.store(true, Ordering::SeqCst);
    server.join().unwrap();
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}
