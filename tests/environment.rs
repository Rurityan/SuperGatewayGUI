use gateway::{
    config, dependencies as deps, environment as env,
    manager::{Event, Manager},
    model::{Pair, Profile, Settings},
    native::Child,
    net,
    store::Store,
};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::mpsc,
    thread,
};
fn root() -> PathBuf {
    let root = std::env::temp_dir().join(format!("SG 环境 隔离 {}", gateway::model::new_id()));
    fs::create_dir_all(&root).unwrap();
    root
}
#[test]
fn defaults_validation_preview_and_encrypted_restoration() {
    let old: Profile = serde_json::from_str(r#"{"Name":"旧配置","Command":"npx"}"#).unwrap();
    assert!(old.inherit_environment && old.additional_paths.is_empty());
    let root = root();
    let p = Profile {
        command: "uvx".into(),
        inherit_environment: false,
        additional_paths: vec![root.to_string_lossy().into_owned()],
        env: vec![Pair {
            key: "SECRET".into(),
            value: "environment-preview-secret".into(),
        }],
        ..Default::default()
    };
    let store = Store::new(root.clone()).unwrap();
    store
        .save(&Settings {
            services: vec![p.clone()],
            ..Default::default()
        })
        .unwrap();
    assert_eq!(store.load().unwrap().services[0], p);
    let encrypted = fs::read(root.join("settings.dat")).unwrap();
    assert!(!String::from_utf8_lossy(&encrypted).contains("environment-preview-secret"));
    let text = env::preview(&root, &p).unwrap();
    assert!(
        text.contains("服务注入")
            && text.contains("Windows 基础环境")
            && !text.contains("environment-preview-secret")
    );
    assert!(!root.join("dependencies").exists());
    let mut p = p;
    p.additional_paths = vec!["relative/directory".into()];
    assert!(config::validate(&p, false).is_err());
    p.additional_paths = vec![r"D:\one;D:\two".into()];
    assert!(config::validate(&p, false).is_err());
    p.additional_paths.clear();
    p.managed = true;
    p.dependency.package = "mcp-for-blender".into();
    p.env.push(Pair {
        key: "path".into(),
        value: "override".into(),
    });
    assert!(
        config::validate(&p, false)
            .unwrap_err()
            .contains("附加程序目录")
    );
    p.env.retain(|e| !e.key.eq_ignore_ascii_case("PATH"));
    let text = env::preview(&root, &p).unwrap();
    assert!(text.contains("尚不能启动"));
    assert!(!root.join("dependencies").exists());
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn service_environment_protocols() {
    if std::env::var("SG_ENVIRONMENT_TEST_CHILD").is_err() {
        let status = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "service_environment_protocols", "--nocapture"])
            .env("SG_ENVIRONMENT_TEST_CHILD", "1")
            .env("SG_PARENT_ONLY", "parent-snapshot-secret")
            .status()
            .unwrap();
        assert!(status.success());
        return;
    }
    env::initialize();
    let before = std::env::vars_os().collect::<BTreeMap<_, _>>();
    let root = root();
    let blender = root.join("Blender 中文 目录");
    fs::create_dir_all(&blender).unwrap();
    fs::copy(env!("CARGO_BIN_EXE_mock-mcp"), blender.join("blender.exe")).unwrap();
    let p = Profile {
        name: "隔离服务".into(),
        command: env!("CARGO_BIN_EXE_mock-mcp").into(),
        args: vec!["--env-report".into()],
        inherit_environment: false,
        additional_paths: vec![blender.to_string_lossy().into_owned()],
        listen_address: Some("127.0.0.1".into()),
        port: config::free_port(&[]).unwrap(),
        env: vec![
            Pair {
                key: "sg_service_marker".into(),
                value: "one".into(),
            },
            Pair {
                key: "SG_EMPTY".into(),
                value: String::new(),
            },
            Pair {
                key: "sg_parent_only".into(),
                value: "service-overrides-parent".into(),
            },
        ],
        ..Default::default()
    };
    let effective = env::effective(&p).unwrap();
    assert_eq!(effective["SG_PARENT_ONLY"], "service-overrides-parent");
    assert_eq!(effective["SG_EMPTY"], "");
    assert!(!effective.contains_key("SG_ENVIRONMENT_TEST_CHILD"));
    assert!(effective["PATH"].starts_with(&blender.to_string_lossy().to_string()));
    assert_eq!(
        effective,
        env::effective(&env::prepare(&p).unwrap()).unwrap()
    );
    let mut managed = p.clone();
    managed.managed_runtime = true;
    managed.env.push(Pair {
        key: "PATH".into(),
        value: format!(
            "{};{}",
            root.join("managed-node").display(),
            env::windows_paths().join(";")
        ),
    });
    let path = env::path(&managed).unwrap();
    assert!(path.find("managed-node").unwrap() < path.find("Blender 中文").unwrap());
    assert!(path.find("Blender 中文").unwrap() < path.find("System32").unwrap());
    let mut explicit = p.clone();
    explicit.env.push(Pair {
        key: "Path".into(),
        value: r"D:\显式目录".into(),
    });
    assert_eq!(
        env::path(&explicit).unwrap(),
        format!("{};D:\\显式目录", blender.display())
    );
    let engine = Path::new(env!("CARGO_MANIFEST_DIR")).join("engine/supergateway.exe");
    let (tx, rx) = mpsc::sync_channel::<Event>(4096);
    let manager = Manager::new(root.clone(), engine.clone(), tx);
    let receiver = thread::spawn(move || while rx.recv().is_ok() {});
    let mut second = p.clone();
    second.id = gateway::model::new_id();
    second.name = "继承服务".into();
    second.port = config::free_port(std::slice::from_ref(&p)).unwrap();
    second.inherit_environment = true;
    second.env = vec![Pair {
        key: "SG_SERVICE_MARKER".into(),
        value: "two".into(),
    }];
    manager.start(&p).unwrap();
    manager.start(&second).unwrap();
    let second_pid = manager.pid(&second.id).unwrap();
    let report = |p: &Profile| -> Value {
        serde_json::from_str(&net::test(p).unwrap()[0].description).unwrap()
    };
    let a = report(&p);
    let b = report(&second);
    assert_eq!(a["marker"], "one");
    assert_eq!(a["empty"], "");
    assert_eq!(a["parent"], "service-overrides-parent");
    assert_eq!(b["marker"], "two");
    assert_eq!(b["parent"], "parent-snapshot-secret");
    assert!(b["empty"].is_null());
    assert!(
        a["blender"]
            .as_str()
            .unwrap()
            .contains("controlled fixture")
    );
    println!(
        "PASS two live MCP services isolate variables, preserve empty values and discover custom Blender directory"
    );
    for output in ["sse", "ws"] {
        manager.stop(&p);
        let mut changed = p.clone();
        changed.output = output.into();
        changed.route = format!("/{output}");
        manager.start(&changed).unwrap();
        assert_eq!(report(&changed)["marker"], "one");
        assert_eq!(manager.pid(&second.id), Some(second_pid));
        assert_eq!(report(&second)["marker"], "two");
    }
    println!(
        "PASS HTTP/SSE/WebSocket share environment policy and single-service restart preserves other service"
    );
    manager.stop(&p);
    let mut shell = p.clone();
    shell.stdio = format!("& {} --env-report", gateway::native::quote(&shell.command));
    if gateway::shell::detect().unwrap().kind == gateway::shell::Kind::Cmd {
        shell.stdio = format!("{} --env-report", gateway::native::quote(&shell.command));
    }
    manager.start(&shell).unwrap();
    assert_eq!(report(&shell)["marker"], "one");
    manager.stop(&shell);
    println!("PASS full command uses the same effective environment");
    let missing = p.clone();
    let mut no_runtime = missing;
    no_runtime.managed = true;
    no_runtime.dependency.package = "mcp-for-blender".into();
    no_runtime.dependency.kind = "python".into();
    assert!(deps::prepared(&root, &no_runtime).is_err());
    let child = Child::spawn_exact(
        Path::new(env!("CARGO_BIN_EXE_mock-mcp")),
        &["--version".into()],
        &effective,
        &root,
    )
    .unwrap();
    assert!(child.wait(2000));
    drop(child);
    manager.stop_all();
    drop(manager);
    receiver.join().unwrap();
    assert_eq!(std::env::vars_os().collect::<BTreeMap<_, _>>(), before);
    println!("PASS parent environment unchanged; missing managed runtime cannot fall back");
    fs::remove_dir_all(root).unwrap();
}
