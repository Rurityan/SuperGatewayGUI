use gateway::{
    dependencies as deps,
    model::{Pair, Profile},
    oneshot,
};
use std::{fs, path::PathBuf};
fn fixture(kind: &str) -> (PathBuf, Profile) {
    let root = std::env::temp_dir().join(format!("SG 一次性 命令 {}", gateway::model::new_id()));
    fs::create_dir_all(&root).unwrap();
    let mut p = Profile {
        managed: true,
        stdio: "uvx preserved original".into(),
        ..Default::default()
    };
    p.dependency.kind = kind.into();
    p.dependency.package = "fixture-command".into();
    p.dependency.version = "1.0.0".into();
    p.dependency.installed = gateway::model::new_id();
    p.dependency.args = vec!["server-argument".into()];
    for r in deps::CATALOG {
        let dir = deps::runtime(&root, r.kind, r.version).unwrap();
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(r.executable), b"fake for prepare only").unwrap();
    }
    let installed = deps::installed_path(&root, &p).unwrap();
    let executable = if kind == "npm" {
        "entry.js"
    } else {
        "venv/Scripts/fixture-command.exe"
    };
    fs::create_dir_all(installed.join("venv/Scripts")).unwrap();
    fs::write(installed.join(executable), b"fixture").unwrap();
    fs::write(
        installed.join("venv/pyvenv.cfg"),
        format!(
            "home = {}\n",
            deps::runtime(&root, "python", &p.dependency.python)
                .unwrap()
                .display()
        ),
    )
    .unwrap();
    let receipt = deps::Receipt {
        spec: p.dependency.clone(),
        resolved_version: "1.0.0".into(),
        executable: executable.into(),
        args: vec![],
    };
    fs::write(
        installed.join("installed.json"),
        serde_json::to_vec(&receipt).unwrap(),
    )
    .unwrap();
    (root, p)
}
#[test]
fn one_off_args_are_exact_and_do_not_change_service_or_run_shell() {
    for kind in ["npm", "python"] {
        let (root, p) = fixture(kind);
        let path = std::env::var_os("PATH");
        let args = vec![
            "setup".into(),
            "中文 空格".into(),
            "&echo %PATH% $(whoami)".into(),
            "".into(),
            "a\"b\\".into(),
        ];
        let invocation = oneshot::prepare(&root, &p, &Default::default(), args.clone()).unwrap();
        assert_eq!(&invocation.args[invocation.args.len() - args.len()..], args);
        assert!(invocation.executable.is_absolute());
        assert!(
            !invocation
                .args
                .iter()
                .any(|s| s == "uvx" || s == "-Command" || s == "/c")
        );
        assert_eq!(p.dependency.args, ["server-argument"]);
        assert_eq!(p.stdio, "uvx preserved original");
        assert_eq!(invocation.environment["UV_PYTHON_DOWNLOADS"], "never");
        assert_eq!(invocation.environment["UV_OFFLINE"], "0");
        assert_eq!(std::env::var_os("PATH"), path);
        if kind == "python" {
            assert!(invocation.environment["VIRTUAL_ENV"].ends_with("venv"));
            assert!(invocation.environment["UV_PYTHON"].ends_with("Scripts/python.exe"));
        }
        fs::remove_dir_all(root).unwrap();
    }
}
#[test]
fn missing_changed_or_remote_packages_never_download_or_fall_back() {
    let (root, mut p) = fixture("npm");
    p.dependency.version = "2.0.0".into();
    assert!(oneshot::prepare(&root, &p, &Default::default(), vec![]).is_err());
    p.dependency.version = "1.0.0".into();
    p.source = "streamableHttp".into();
    assert!(oneshot::prepare(&root, &p, &Default::default(), vec![]).is_err());
    p.source = "local".into();
    fs::remove_dir_all(deps::installed_path(&root, &p).unwrap()).unwrap();
    assert!(oneshot::prepare(&root, &p, &Default::default(), vec!["setup".into()]).is_err());
    assert!(!root.join("dependencies/staging").exists());
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn one_off_network_and_service_environment_are_child_only() {
    let (root, mut p) = fixture("python");
    p.env = vec![Pair {
        key: "CUSTOM_VALUE".into(),
        value: "test-secret".into(),
        ..Default::default()
    }];
    p.inherit_environment = false;
    let extra = root.join("Blender 中文 目录");
    fs::create_dir_all(&extra).unwrap();
    p.additional_paths = vec![extra.to_string_lossy().into_owned()];
    let network = deps::Network {
        proxy: "http://127.0.0.1:10808".into(),
        username: "u".into(),
        password: "p@ss".into(),
        ..Default::default()
    };
    let invocation = oneshot::prepare(&root, &p, &network, vec![]).unwrap();
    assert_eq!(invocation.environment["CUSTOM_VALUE"], "test-secret");
    assert!(!invocation.environment.contains_key("CARGO"));
    let path = &invocation.environment["PATH"];
    assert!(
        path.find("venv\\Scripts")
            .or_else(|| path.find("venv/Scripts"))
            .unwrap()
            < path.find("Blender 中文").unwrap()
    );
    assert!(path.find("Blender 中文").unwrap() < path.find("System32").unwrap());
    assert!(invocation.environment["HTTPS_PROXY"].contains("p%40ss"));
    assert_eq!(invocation.directory, root);
    p.cwd = root.join("missing").to_string_lossy().into_owned();
    assert!(oneshot::prepare(&root, &p, &network, vec![]).is_err());
    fs::remove_dir_all(root).unwrap();
}
