use gateway::{
    dependencies::{self as deps, Network, ServiceDependency, Task},
    model::{Profile, Settings},
    store::Store,
};
use std::{fs, path::PathBuf, sync::mpsc};
#[path = "support/dependencies.rs"]
mod support;
fn root() -> PathBuf {
    let p = std::env::temp_dir().join(format!("SG 依赖 测试 {}", gateway::model::new_id()));
    fs::create_dir_all(&p).unwrap();
    p
}
#[test]
fn uninstalled_package_can_be_saved_without_a_program_or_setup() {
    let root = root();
    let store = Store::new(root.clone()).unwrap();
    for (kind, package) in [("python", "mcp-for-blender"), ("npm", "@scope/server")] {
        let p = Profile {
            name: "未安装服务".into(),
            managed: true,
            dependency: ServiceDependency {
                kind: kind.into(),
                package: package.into(),
                ..Default::default()
            },
            ..Default::default()
        };
        gateway::config::validate(&p, false).unwrap();
        store
            .save(&Settings {
                services: vec![p.clone()],
                ..Default::default()
            })
            .unwrap();
        assert_eq!(store.load().unwrap().services[0], p);
        assert!(deps::check_installation(&root, &p).is_err());
        assert!(deps::prepared(&root, &p).is_err());
        assert!(
            gateway::oneshot::prepare(&root, &p, &Network::default(), vec!["setup".into()])
                .is_err()
        );
        assert!(!root.join("dependencies").exists());
        let mut invalid = p.clone();
        invalid.dependency.package.clear();
        assert!(gateway::config::validate(&invalid, false).is_err());
        invalid.dependency.package = "https://invalid/package".into();
        assert!(gateway::config::validate(&invalid, false).is_err());
        invalid.managed = false;
        let error = gateway::config::validate(&invalid, false).unwrap_err();
        assert!(error.contains("应用管理") && error.contains("包名"));
        invalid.command = "uvx".into();
        gateway::config::validate(&invalid, false).unwrap();
        invalid.command.clear();
        invalid.stdio = "uvx mcp-for-blender".into();
        gateway::config::validate(&invalid, false).unwrap();
        invalid.managed = true;
        gateway::config::validate(&invalid, false).unwrap();
    }
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn compatibility_and_encrypted_network() {
    let root = root();
    let store = Store::new(root.clone()).unwrap();
    let old: Settings =
        serde_json::from_str(r#"{"Services":[{"Name":"旧服务","Command":"npx"}]}"#).unwrap();
    assert!(!old.services[0].managed);
    assert!(old.services[0].dependency.installed.is_empty());
    let mut settings = old;
    settings.dependency_network = Network {
        proxy: "http://127.0.0.1:10808".into(),
        username: "测试".into(),
        password: "dependency-password-secret".into(),
        ..Default::default()
    };
    store.save(&settings).unwrap();
    assert_eq!(
        store.load().unwrap().dependency_network,
        settings.dependency_network
    );
    assert!(
        !String::from_utf8_lossy(&fs::read(root.join("settings.dat")).unwrap())
            .contains("dependency-password-secret")
    );
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn import_inference_and_no_execution() {
    let p = Profile {
        command: "npx".into(),
        args: vec![
            "-y".into(),
            "@scope/server@1.2.3".into(),
            "中文 空格".into(),
        ],
        ..Default::default()
    };
    let d = deps::infer(&p).unwrap();
    assert_eq!(d.package, "@scope/server");
    assert_eq!(d.version, "1.2.3");
    assert_eq!(d.args, ["中文 空格"]);
    let p = Profile {
        command: "uvx".into(),
        args: vec!["mcp-server-fetch==2025.4.7".into()],
        ..Default::default()
    };
    assert_eq!(deps::infer(&p).unwrap().kind, "python");
    let p = Profile {
        stdio: "npx -y server && echo unsafe".into(),
        ..Default::default()
    };
    assert!(deps::infer(&p).is_none());
    let result = gateway::config::import(
        r#"{"mcpServers":{"a":{"command":"npx","args":["-y","my-server"]}}}"#,
        &[],
    )
    .unwrap();
    assert!(result.messages.iter().any(|s| s.contains("仅识别，不安装")));
    assert!(!result.profiles[0].managed);
}
#[test]
fn validation_and_directory_boundaries() {
    for package in [
        "../evil",
        "--help",
        "https://bad",
        "file:bad",
        "@scope/../../bad",
    ] {
        assert!(
            deps::validate_spec(&ServiceDependency {
                package: package.into(),
                ..Default::default()
            })
            .is_err()
        );
    }
    let root = root();
    let p = Profile {
        managed: true,
        ..Default::default()
    };
    assert!(deps::prepared(&root, &p).unwrap_err().contains("缺少托管"));
    assert!(deps::remove_service(&root, "../outside", &[]).is_err());
    assert!(deps::runtime(&root, "node", "../unknown").is_err());
    let file = root.join("bad");
    fs::write(&file, b"corrupted download").unwrap();
    assert!(deps::verify_archive(&file, deps::CATALOG[0].sha256).is_err());
    let n = Network {
        proxy: "https://proxy.example".into(),
        ..Default::default()
    };
    assert!(n.validate().is_err());
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn child_environment_and_deletion_guards() {
    let before = std::env::var_os("PATH");
    let root = root();
    for r in deps::CATALOG {
        let path = deps::runtime(&root, r.kind, r.version).unwrap();
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join(r.executable), b"test").unwrap();
    }
    let env = deps::environment(
        &root,
        &ServiceDependency::default(),
        &Network::default(),
        false,
    )
    .unwrap();
    assert!(env["PATH"].contains("SG 依赖 测试"));
    assert_eq!(env["UV_PYTHON_DOWNLOADS"], "never");
    assert_eq!(env["npm_config_offline"], "true");
    assert_eq!(std::env::var_os("PATH"), before);
    let p = Profile {
        managed: true,
        ..Default::default()
    };
    assert!(deps::remove_runtime(&root, &deps::CATALOG[0], std::slice::from_ref(&p), &[]).is_err());
    assert!(deps::remove_service(&root, &p.id, std::slice::from_ref(&p)).is_err());
    assert!(deps::clear_cache(&root, std::slice::from_ref(&p)).is_err());
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn cancellation_and_atomic_serialization() {
    let task = Task::default();
    task.cancel();
    assert!(task.check().is_err());
    assert!(task.execute(|| Ok(())).is_err());
    task.execute(|| {
        assert!(task.check().is_ok());
        assert!(task.execute(|| Ok(())).is_err());
        Ok(())
    })
    .unwrap();
}
#[test]
fn proxy_failure_and_download_cancellation_preserve_installations() {
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::Arc,
        time::{Duration, Instant},
    };
    for mode in ["authentication", "interrupted", "cancel"] {
        let root = root();
        fs::write(root.join("original-config"), b"unchanged").unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let proxy = format!("http://{}", listener.local_addr().unwrap());
        let task = Arc::new(Task::default());
        let task2 = task.clone();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut buffer = [0; 4096];
            let _ = socket.read(&mut buffer);
            match mode {
                "authentication" => {
                    let _=socket.write_all(b"HTTP/1.1 407 Proxy Authentication Required\r\nProxy-Authenticate: Basic realm=\"fixture\"\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                }
                "cancel" => {
                    task2.cancel();
                    std::thread::sleep(Duration::from_millis(500));
                }
                _ => {}
            }
        });
        let n = Network {
            proxy,
            ..Default::default()
        };
        let start = Instant::now();
        let result =
            task.execute(|| deps::install_runtime(&root, &deps::CATALOG[0], &n, &task, &|_| {}));
        assert!(result.is_err(), "{mode}");
        server.join().unwrap();
        assert!(start.elapsed() < Duration::from_secs(10));
        if mode == "cancel" {
            assert!(result.unwrap_err().contains("取消"));
        }
        assert_eq!(
            fs::read(root.join("original-config")).unwrap(),
            b"unchanged"
        );
        assert!(
            !deps::runtime(&root, "node", deps::CATALOG[0].version)
                .unwrap()
                .exists()
        );
        assert_eq!(
            fs::read_dir(root.join("dependencies/staging"))
                .unwrap()
                .count(),
            0
        );
        fs::remove_dir_all(root).unwrap();
    }
}
#[test]
fn archive_traversal_and_links() {
    use std::io::Write;
    let root = root();
    let archive = root.join("bad.zip");
    let mut writer = zip::ZipWriter::new(fs::File::create(&archive).unwrap());
    writer
        .start_file("../outside", zip::write::SimpleFileOptions::default())
        .unwrap();
    writer.write_all(b"bad").unwrap();
    writer.finish().unwrap();
    assert!(deps::extract(&archive, "zip", &root.join("extracted"), &Task::default()).is_err());
    fs::remove_dir_all(root).unwrap();
}

#[test]
#[ignore = "requires isolated data produced by official_download_and_actual_mcp"]
fn installed_offline_and_versions() {
    let root = PathBuf::from(
        std::env::var_os("SG_DEPENDENCY_TEST_DATA_DIR").expect("test data directory"),
    );
    let before = std::env::var_os("PATH");
    let mut settings = Store::new(root.clone()).unwrap().load().unwrap();
    let external = root.join(format!("环境外部程序 中文 {}", gateway::model::new_id()));
    fs::create_dir_all(&external).unwrap();
    for name in ["node.exe", "python.exe", "blender.exe"] {
        fs::copy(env!("CARGO_BIN_EXE_mock-mcp"), external.join(name)).unwrap();
    }
    for (index, p) in settings.services.iter_mut().enumerate() {
        p.inherit_environment = false;
        p.additional_paths = vec![external.to_string_lossy().into_owned()];
        p.env.push(gateway::model::Pair {
            key: "SG_SERVICE_MARKER".into(),
            value: format!("environment-service-{index}"),
        });
    }
    let (tx, rx) = mpsc::sync_channel(4096);
    let manager = gateway::manager::Manager::new(
        root.clone(),
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("engine/supergateway.exe"),
        tx,
    );
    std::thread::spawn(move || while rx.recv().is_ok() {});
    for p in &settings.services {
        let prepared = deps::prepared(&root, p).unwrap();
        let effective = gateway::environment::effective(&prepared).unwrap();
        let one_shot = gateway::oneshot::prepare(&root, p, &Network::default(), vec![]).unwrap();
        assert_eq!(one_shot.environment["PATH"], effective["PATH"]);
        assert!(
            effective["PATH"].find("runtimes").unwrap()
                < effective["PATH"].find("环境外部程序").unwrap()
        );
        let blender = gateway::config::resolve_with_path("blender", &effective["PATH"]).unwrap();
        assert_eq!(blender, external.join("blender.exe"));
        assert!(
            String::from_utf8_lossy(
                &std::process::Command::new(blender)
                    .arg("--version")
                    .output()
                    .unwrap()
                    .stdout
            )
            .contains("controlled fixture")
        );
        assert!(std::path::Path::new(&prepared.command).is_absolute());
        let (_, secrets) = gateway::config::build(&prepared, true).unwrap();
        assert!(!secrets.values().any(|s| s == "1"));
        manager.start(p).unwrap();
        let tools = gateway::net::test(p).unwrap();
        assert!(!tools.is_empty());
        assert!(
            !tools.iter().any(|t| t.name == "echo"),
            "shadow Node/Python must not replace managed runtimes"
        );
        let r = deps::receipt(&root, p).unwrap();
        assert_eq!(p.dependency.version, r.resolved_version);
        let mut changed = p.clone();
        changed.dependency.version = "0.0.0-unavailable".into();
        assert!(deps::prepared(&root, &changed).is_err());
        assert!(deps::remove_service(&root, &p.id, &manager.active_profiles()).is_err());
    }
    let second_pid = manager.pid(&settings.services[1].id);
    manager.stop(&settings.services[0]);
    manager.start(&settings.services[0]).unwrap();
    assert_eq!(manager.pid(&settings.services[1].id), second_pid);
    assert!(
        !gateway::net::test(&settings.services[1])
            .unwrap()
            .is_empty()
    );
    manager.stop_all();
    assert_eq!(std::env::var_os("PATH"), before);
    for r in deps::CATALOG {
        let d = ServiceDependency {
            kind: if r.kind == "node" { "npm" } else { "python" }.into(),
            ..Default::default()
        };
        let env = deps::environment(&root, &d, &Network::default(), false).unwrap();
        let mut child = gateway::native::Child::spawn(
            &deps::runtime(&root, r.kind, r.version)
                .unwrap()
                .join(r.executable),
            &["--version".into()],
            &env,
            &root,
        )
        .unwrap();
        child.input.take();
        assert!(child.wait(10000));
        assert_eq!(child.exit_code(), 0);
    }
    // Exercise the bundled npm/npx CLI and uvx with no system command lookup.
    let child_env = deps::environment(
        &root,
        &ServiceDependency::default(),
        &Network::default(),
        false,
    )
    .unwrap();
    let node = deps::runtime(&root, "node", &ServiceDependency::default().node).unwrap();
    let uv = deps::runtime(&root, "uv", &ServiceDependency::default().uv).unwrap();
    for (exe, args) in [
        (
            node.join("node.exe"),
            vec![
                node.join("node_modules/npm/bin/npm-cli.js")
                    .to_string_lossy()
                    .into_owned(),
                "--version".into(),
            ],
        ),
        (
            node.join("node.exe"),
            vec![
                node.join("node_modules/npm/bin/npx-cli.js")
                    .to_string_lossy()
                    .into_owned(),
                "--version".into(),
            ],
        ),
        (uv.join("uvx.exe"), vec!["--version".into()]),
    ] {
        let mut child = gateway::native::Child::spawn(&exe, &args, &child_env, &root).unwrap();
        child.input.take();
        assert!(child.wait(10000));
        assert_eq!(child.exit_code(), 0);
    }
    println!("VERIFIED bundled npm/npx CLI and uvx with empty parent PATH");
    println!(
        "VERIFIED actual managed Node/Python MCP prioritize owned runtimes over shadow tools; custom Blender directory and one-shot PATH agree"
    );
    fs::remove_dir_all(external).unwrap();
    println!(
        "Verified offline cached service starts, exact versions, isolation and owned runtime executables with PATH={:?}",
        before
    );
}

// Explicit network acceptance only; uses an isolated workspace data directory.
#[test]
#[ignore = "downloads fixed runtimes and actual MCP packages; opt in with --ignored"]
fn official_download_and_actual_mcp() {
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let root = std::env::var_os("SG_DEPENDENCY_TEST_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            workspace
                .join(".build")
                .join(format!("托管依赖 验收 {}", gateway::model::new_id()))
        });
    fs::create_dir_all(&root).unwrap();
    if let Some(directory) =
        std::env::var_os("SG_DEPENDENCY_TEST_STORAGE").filter(|s| !s.is_empty())
    {
        gateway::storage::set_dependency_directory(&root, &PathBuf::from(directory)).unwrap();
    }
    println!("DATA_DIR={}", root.display());
    println!(
        "DEPENDENCY_DIR={}",
        gateway::storage::dependency_directory(&root)
            .unwrap()
            .display()
    );
    let task = Task::default();
    let n = Network {
        proxy: std::env::var("SG_DEPENDENCY_TEST_PROXY").unwrap_or_default(),
        ..Default::default()
    };
    let progress = |s: String| println!("{s}");
    for r in deps::CATALOG {
        task.execute(|| deps::install_runtime(&root, r, &n, &task, &progress))
            .unwrap();
    }
    let parent_path = std::env::var_os("PATH");
    let (tx, rx) = mpsc::sync_channel(4096);
    let manager =
        gateway::manager::Manager::new(root.clone(), workspace.join("engine/supergateway.exe"), tx);
    std::thread::spawn(move || {
        while let Ok(event) = rx.recv() {
            if let gateway::manager::Event::Log(_, line) = event {
                println!("MCP: {line}");
            }
        }
    });
    let mut profiles = vec![];
    for (kind, package, entry) in [
        (
            "npm",
            "@modelcontextprotocol/server-filesystem",
            "mcp-server-filesystem",
        ),
        ("python", "mcp-server-fetch", "mcp-server-fetch"),
    ] {
        let mut p = Profile {
            name: format!("{kind} 中文 服务"),
            managed: true,
            port: gateway::config::free_port(&profiles).unwrap(),
            listen_address: Some("127.0.0.1".into()),
            cwd: root.to_string_lossy().into(),
            dependency: ServiceDependency {
                kind: kind.into(),
                package: package.into(),
                entry: entry.into(),
                args: if kind == "npm" {
                    vec![root.to_string_lossy().into()]
                } else {
                    vec![]
                },
                ..Default::default()
            },
            ..Default::default()
        };
        p.dependency = task
            .execute(|| deps::install_service(&root, &p, &n, &task, &progress))
            .unwrap();
        manager.start(&p).unwrap();
        let tools = gateway::net::test(&p).unwrap();
        assert!(!tools.is_empty());
        println!(
            "VERIFIED {kind}: {} tools, version {}",
            tools.len(),
            deps::receipt(&root, &p).unwrap().resolved_version
        );
        profiles.push(p);
    }
    let old = profiles[0].clone();
    support::verify_versions(&root, &task, &manager);
    let old_pid = manager.pid(&old.id).unwrap();
    let mut failed = old.clone();
    failed.dependency.package = "supergateway-package-does-not-exist-acceptance".into();
    assert!(
        task.execute(|| deps::install_service(&root, &failed, &n, &task, &progress))
            .is_err()
    );
    assert!(deps::receipt(&root, &old).is_ok());
    assert_eq!(manager.pid(&old.id), Some(old_pid));
    assert!(!gateway::net::test(&old).unwrap().is_empty());
    assert!(deps::remove_service(&root, &old.id, &manager.active_profiles()).is_err());
    assert!(
        deps::remove_runtime(
            &root,
            &deps::CATALOG[0],
            &profiles,
            &manager.active_profiles()
        )
        .is_err()
    );
    manager.stop_all();
    let store = Store::new(root.clone()).unwrap();
    store
        .save(&Settings {
            services: profiles,
            dependency_network: n,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(std::env::var_os("PATH"), parent_path);
    println!(
        "RUNTIME_BYTES={} CACHE_BYTES={} SERVICE_BYTES={}",
        deps::size(
            &gateway::storage::dependency_directory(&root)
                .unwrap()
                .join("runtimes")
        ),
        deps::size(
            &gateway::storage::dependency_directory(&root)
                .unwrap()
                .join("cache")
        ),
        deps::size(
            &gateway::storage::dependency_directory(&root)
                .unwrap()
                .join("services")
        )
    );
}
