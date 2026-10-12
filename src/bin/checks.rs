use gateway::{
    Result, config,
    manager::{Event, Manager},
    model::{Pair, Profile, Settings, new_id},
    native::Child,
    net::{self, Session},
    store::Store,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    io::{BufRead, BufReader, Write},
    path::PathBuf,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};
use windows_sys::Win32::{Foundation::*, System::Threading::*};
fn main() {
    if let Err(error) = run() {
        eprintln!("FAIL {error}");
        std::process::exit(1);
    }
}
fn run() -> Result<()> {
    let args = std::env::args().collect::<Vec<_>>();
    if args.get(1).is_some_and(|arg| arg == "--shell-selection") {
        let selected = gateway::shell::detect()?;
        if selected.kind.name() != args[2] {
            return Err(format!("Unexpected interpreter: {:?}", selected));
        }
        unsafe {
            std::env::set_var("PATH", "");
            std::env::set_var("SystemRoot", "C:/missing-shell-test");
        }
        if gateway::shell::detect()?.path != selected.path {
            return Err("Interpreter choice changed".into());
        }
        return Ok(());
    }
    let engine =
        std::path::absolute(args.get(1).ok_or("需要网关 exe 参数")?).map_err(|e| e.to_string())?;
    let fixture = std::path::absolute(args.get(2).ok_or("需要测试 MCP exe 参数")?)
        .map_err(|e| e.to_string())?;
    let root = if args.get(3).map(|s| s.as_str()) == Some("--victim") {
        PathBuf::from(&args[4])
    } else {
        std::env::temp_dir().join(format!("SGRust 中文 测试 {}", new_id()))
    };
    fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    let (tx, rx) = mpsc::sync_channel(2048);
    let manager = Manager::new(root.clone(), engine.clone(), tx);
    let mut profiles = vec![];
    let mut count = 0;
    let make = |profiles: &mut Vec<Profile>, transport: &str| -> Result<Profile> {
        let p = Profile {
            name: format!("本地 {}", profiles.len()),
            command: fixture.to_string_lossy().into_owned(),
            args: vec![
                "中文 参数".into(),
                "quote\"inside".into(),
                String::new(),
                "ends\\".into(),
            ],
            cwd: root.to_string_lossy().into_owned(),
            port: config::free_port(profiles)?,
            output: transport.into(),
            route: match transport {
                "sse" => "/sse",
                "ws" => "/ws",
                _ => "/mcp",
            }
            .into(),
            api_key: "test-key-abc123".into(),
            ..Default::default()
        };
        profiles.push(p.clone());
        Ok(p)
    };
    if args.get(3).map(|s| s.as_str()) == Some("--victim") {
        let mut p = make(&mut profiles, "ws")?;
        p.env = vec![Pair {
            key: "RECORD_FILE".into(),
            value: root.join("child.txt").to_string_lossy().into_owned(),
        }];
        manager.start(&p)?;
        let mut session = Session::connect(&p, Duration::from_secs(600))?;
        session.initialize()?;
        fs::write(
            root.join("ready.txt"),
            manager.pid(&p.id).unwrap().to_string(),
        )
        .map_err(|e| e.to_string())?;
        loop {
            thread::sleep(Duration::from_secs(10));
        }
    }
    let mut check = |ok: bool, name: &str| -> Result<()> {
        if !ok {
            return Err(name.into());
        }
        count += 1;
        println!("PASS {name}");
        Ok(())
    };
    manager.verify()?;
    check(true, "engine SHA256")?;
    let mut secure = make(&mut profiles, "streamableHttp")?;
    check(
        secure.listen_host() == "0.0.0.0"
            && serde_json::from_str::<Value>(&config::build(&secure, false)?.0)
                .map_err(|e| e.to_string())?["host"]
                == "0.0.0.0",
        "new profiles default to all IPv4 interfaces",
    )?;
    let old_local: Profile = serde_json::from_str(r#"{"Lan":false}"#).map_err(|e| e.to_string())?;
    let old_lan: Profile = serde_json::from_str(r#"{"Lan":true}"#).map_err(|e| e.to_string())?;
    check(
        old_local.listen_host() == "127.0.0.1" && old_lan.listen_host() == "0.0.0.0",
        "legacy Lan settings preserve listening scope",
    )?;
    check(
        secure
            .address(false)
            .split('/')
            .nth(2)
            .unwrap()
            .split(':')
            .next()
            .unwrap()
            .parse::<std::net::Ipv4Addr>()
            .is_ok(),
        "connection address uses IPv4 without hostname or wildcard",
    )?;
    secure.bearer = "secret-token".into();
    secure.env = vec![Pair {
        key: "SECRET".into(),
        value: "secret-env".into(),
    }];
    let store = Store::new(root.join("settings"))?;
    store.save(&Settings {
        services: vec![secure.clone()],
        ..Default::default()
    })?;
    check(
        store.load()?.services[0] == secure,
        "DPAPI round trip and PascalCase schema",
    )?;
    check(
        !String::from_utf8_lossy(&fs::read(store.root.join("settings.dat")).unwrap())
            .contains("secret-token"),
        "no plaintext settings",
    )?;
    store.save(&store.load()?)?;
    check(
        store.root.join("settings.dat.pre-rust.bak").exists(),
        "first-write configuration backup",
    )?;
    let imported = config::import(
        r#"{ // comments
      "mcpServers": { "本地 0": { "command": "npx", "args": ["-y", "package", "D:\\我的资料"], "env": { "TOKEN": "abc" }, }, "remote": { "url": "https://example.com/mcp", "type": "http" }, "bad": { "url": "invalid" } }
    }"#,
        &profiles,
    )?;
    check(
        imported.profiles.len() == 2
            && imported.profiles[0].name == "本地 0 (2)"
            && imported.messages.iter().any(|s| s.starts_with('✗')),
        "JSONC preview, rename, invalid entry",
    )?;
    check(
        imported.profiles[0].port != imported.profiles[1].port,
        "unique imported ports",
    )?;
    check(
        config::import(
            r#"{"mcpServers":{"bad":{"command":"x","unknown":true}}}"#,
            &[],
        )?
        .profiles
        .is_empty(),
        "unknown field rejected",
    )?;
    check(
        config::expand("${UNSET_SG_RUST:-fallback}/$$")? == "fallback/$",
        "environment substitutions and literal dollar",
    )?;
    let mut missing = make(&mut profiles, "streamableHttp")?;
    missing.command = "nonexistent-gui-test-xyz".into();
    check(manager.start(&missing).is_err(), "missing dependency")?;
    let record = root.join("子进程.txt");
    for transport in ["streamableHttp", "sse", "ws"] {
        let mut p = make(&mut profiles, transport)?;
        p.env = vec![Pair {
            key: "RECORD_FILE".into(),
            value: record.to_string_lossy().into_owned(),
        }];
        manager.start(&p)?;
        let tools = net::test(&p)?;
        check(
            tools.len() == 1 && tools[0].name == "echo",
            &format!("local {transport} initialize/tools"),
        )?;
        let mut bad = p.clone();
        bad.api_key = "wrong-key".into();
        check(
            net::test(&bad).is_err(),
            &format!("{transport} auth refusal"),
        )?;
        manager.stop(&p);
        check(!manager.is_running(&p.id), &format!("{transport} stop"))?;
    }
    let mut stateful = make(&mut profiles, "streamableHttp")?;
    stateful.stateful = true;
    manager.start(&stateful)?;
    check(net::test(&stateful)?.len() == 1, "stateful HTTP session")?;
    let second = make(&mut profiles, "streamableHttp")?;
    manager.start(&second)?;
    manager.stop(&stateful);
    check(net::test(&second)?.len() == 1, "service stop isolation")?;
    let mut occupied = make(&mut profiles, "streamableHttp")?;
    occupied.port = second.port;
    check(manager.start(&occupied).is_err(), "port collision")?;
    for source in ["http", "sse"] {
        let upstream = make(
            &mut profiles,
            if source == "http" {
                "streamableHttp"
            } else {
                "sse"
            },
        )?;
        manager.start(&upstream)?;
        for transport in ["streamableHttp", "sse", "ws"] {
            let mut p = make(&mut profiles, transport)?;
            p.source = source.into();
            p.url = upstream.address(true);
            p.bearer = upstream.api_key.clone();
            manager.start(&p)?;
            check(
                net::test(&p)?.len() == 1,
                &format!("remote {source} → {transport}"),
            )?;
            manager.stop(&p);
        }
        let mut bridge = make(&mut profiles, "streamableHttp")?;
        bridge.source = source.into();
        bridge.url = upstream.address(true);
        bridge.bearer = upstream.api_key.clone();
        let mut bad_upstream = bridge.clone();
        bad_upstream.bearer = "invalid-upstream-token".into();
        let rejected = manager
            .start(&bad_upstream)
            .and_then(|_| net::test(&bad_upstream).map(|_| ()));
        check(
            rejected.is_err(),
            &format!("remote {source} upstream authentication refusal"),
        )?;
        manager.stop(&bad_upstream);
        let exported: Value =
            serde_json::from_str(&config::client_export(&bridge, &engine)?).unwrap();
        let entry = &exported["mcpServers"][&bridge.name];
        let args = entry["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect::<Vec<_>>();
        let mut child = Child::spawn(&engine, &args, &BTreeMap::new(), &root)?;
        let mut input = child.input.take().unwrap();
        writeln!(input,"{}",json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test","version":"1"}}})).unwrap();
        input.flush().unwrap();
        let output = child.output.take().unwrap();
        let errors = child.errors.take().unwrap();
        let err = thread::spawn(move || {
            use std::io::Read;
            let mut text = String::new();
            let _ = BufReader::new(errors).read_to_string(&mut text);
            text
        });
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let mut line = String::new();
            let _ = BufReader::new(output).read_line(&mut line);
            let _ = tx.send(line);
        });
        check(
            rx.recv_timeout(Duration::from_secs(15))
                .map_err(|e| e.to_string())?
                .contains("serverInfo"),
            &format!("{source} stdio export initializes"),
        )?;
        drop(input);
        if !child.wait(8000) {
            child.terminate();
        }
        let _ = err.join();
        manager.stop(&upstream);
    }
    let mut crash = make(&mut profiles, "streamableHttp")?;
    crash.args = vec!["--crash".into()];
    manager.start(&crash)?;
    check(net::test(&crash).is_err(), "MCP server crash diagnosis")?;
    manager.stop(&crash);
    let mut hang = make(&mut profiles, "streamableHttp")?;
    hang.args = vec!["--hang".into()];
    manager.start(&hang)?;
    let start = Instant::now();
    let result = Session::connect(&hang, Duration::from_millis(900))?.tools();
    check(
        result.is_err() && start.elapsed() < Duration::from_secs(4),
        "probe deadline and cancellation",
    )?;
    manager.stop(&hang);
    for transport in ["sse", "ws"] {
        let mut hanging = make(&mut profiles, transport)?;
        hanging.args = vec!["--hang".into()];
        manager.start(&hanging)?;
        let started = Instant::now();
        let result = Session::connect(&hanging, Duration::from_millis(900))
            .and_then(|mut session| session.tools());
        check(
            result.is_err() && started.elapsed() < Duration::from_secs(4),
            &format!("{transport} deadline cancels blocking receive"),
        )?;
        manager.stop(&hanging);
    }
    let mut paged = make(&mut profiles, "ws")?;
    paged.args = vec!["--pages".into()];
    manager.start(&paged)?;
    check(
        net::test(&paged)?.len() == 2,
        "tools pagination collects both pages",
    )?;
    manager.stop(&paged);
    let mut no_tools = make(&mut profiles, "streamableHttp")?;
    no_tools.args = vec!["--no-tools".into()];
    manager.start(&no_tools)?;
    check(
        net::test(&no_tools)?.is_empty(),
        "valid MCP without tools capability",
    )?;
    manager.stop(&no_tools);
    let mut masked = make(&mut profiles, "streamableHttp")?;
    masked.env = vec![
        Pair {
            key: "SECRET".into(),
            value: "${UNSET_SG_RUST:-expanded-secret}".into(),
        },
        Pair {
            key: "EMPTY_VALUE".into(),
            value: String::new(),
        },
    ];
    masked.api_key = "${UNSET_SG_RUST:-expanded-secret}".into();
    manager.start(&masked)?;
    check(
        !fs::read_to_string(root.join(format!("runtime/{}.json", masked.id)))
            .unwrap()
            .contains("expanded-secret"),
        "runtime config references secrets; empty env supported",
    )?;
    check(
        net::test(&masked)?.len() == 1,
        "substituted gateway authentication",
    )?;
    manager.stop(&masked);
    thread::sleep(Duration::from_millis(50));
    let log = rx
        .try_iter()
        .filter_map(|e| {
            if let Event::Log(_, l) = e {
                Some(l)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    check(
        log.iter().any(|l| l.contains("fixture-secret: ***"))
            && !log.iter().any(|l| l.contains("expanded-secret")),
        "resolved secrets redacted from logs",
    )?;
    let text = fs::read_to_string(&record).map_err(|e| e.to_string())?;
    let first = text.lines().next().unwrap().split_once(' ').unwrap().1;
    let arguments: Vec<String> = serde_json::from_str(first).unwrap();
    check(
        arguments == vec!["中文 参数", "quote\"inside", "", "ends\\"],
        "Chinese/spaced/quoted/empty arguments and cwd",
    )?;
    thread::sleep(Duration::from_millis(100));
    check(
        text.lines()
            .all(|line| gone(line.split(' ').next().unwrap().parse().unwrap())),
        "child processes cleaned after stop",
    )?;
    let previous = std::env::var_os("PATH");
    unsafe {
        std::env::set_var("PATH", "");
    }
    let mut standalone = make(&mut profiles, "streamableHttp")?;
    standalone.inherit_environment = false;
    manager.start(&standalone)?;
    check(
        net::test(&standalone)?.len() == 1,
        "standalone engine and MCP with empty PATH",
    )?;
    manager.stop(&standalone);
    unsafe {
        if let Some(path) = previous {
            std::env::set_var("PATH", path);
        }
    }
    let abnormal = root.join("abnormal");
    fs::create_dir_all(&abnormal).unwrap();
    let args = vec![
        engine.to_string_lossy().into_owned(),
        fixture.to_string_lossy().into_owned(),
        "--victim".into(),
        abnormal.to_string_lossy().into_owned(),
    ];
    let mut victim = Child::spawn(
        &std::env::current_exe().unwrap(),
        &args,
        &BTreeMap::new(),
        &root,
    )?;
    let deadline = Instant::now() + Duration::from_secs(15);
    while !abnormal.join("ready.txt").exists() && !victim.exited() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(50));
    }
    check(
        abnormal.join("ready.txt").exists(),
        "abnormal exit fixture ready",
    )?;
    let gateway_pid = fs::read_to_string(abnormal.join("ready.txt"))
        .unwrap()
        .parse()
        .unwrap();
    let mcp_pid = fs::read_to_string(abnormal.join("child.txt"))
        .unwrap()
        .split(' ')
        .next()
        .unwrap()
        .parse()
        .unwrap();
    unsafe {
        let h = OpenProcess(PROCESS_TERMINATE, 0, victim.id);
        TerminateProcess(h, 1);
        CloseHandle(h);
    }
    victim.input.take();
    victim.wait(5000);
    thread::sleep(Duration::from_millis(300));
    check(
        gone(gateway_pid) && gone(mcp_pid),
        "Job Object cleans gateway/MCP after host killed",
    )?;
    let mut relative = make(&mut profiles, "streamableHttp")?;
    let cwd = std::env::current_dir().unwrap();
    let relative_dir = cwd.join(".build/rust-relative");
    fs::create_dir_all(&relative_dir).unwrap();
    relative.cwd = ".build/rust-relative".into();
    relative.env = vec![Pair {
        key: "EMPTY".into(),
        value: String::new(),
    }];
    manager.start(&relative)?;
    check(
        net::test(&relative)?.len() == 1,
        "relative cwd normalized once",
    )?;
    let file: Value = serde_json::from_slice(
        &fs::read(root.join(format!("runtime/{}.json", relative.id))).unwrap(),
    )
    .unwrap();
    check(
        PathBuf::from(file["mcpServers"]["service"]["cwd"].as_str().unwrap()).is_absolute(),
        "runtime cwd is absolute",
    )?;
    manager.stop(&relative);
    fs::copy(&fixture, root.join("mock-mcp.exe")).map_err(|e| e.to_string())?;
    fs::write(
        root.join("npx.cmd"),
        "@echo off\r\n\"%~dp0mock-mcp.exe\" %*\r\n",
    )
    .map_err(|e| e.to_string())?;
    let original_path = std::env::var("PATH").unwrap_or_default();
    unsafe {
        std::env::set_var("PATH", format!("{};{}", root.display(), original_path));
    }
    let mut batch = make(&mut profiles, "streamableHttp")?;
    batch.additional_paths = vec![root.to_string_lossy().into_owned()];
    batch.command = "npx.cmd".into();
    batch.args = vec!["中文 空格".into()];
    manager.start(&batch)?;
    check(
        net::test(&batch)?.len() == 1,
        "npx.cmd fixture in Chinese/spaced directory",
    )?;
    manager.stop(&batch);
    batch.command = String::new();
    batch.stdio = format!("\"{}\" \"中文 空格\"", fixture.display());
    manager.start(&batch)?;
    check(
        net::test(&batch)?.len() == 1,
        "preferred shell full command with quoted executable",
    )?;
    manager.stop(&batch);
    let selected_shell = gateway::shell::detect()?;
    batch.stdio = "npx \"中文 参数\"".into();
    manager.start(&batch)?;
    check(
        net::test(&batch)?.len() == 1,
        "PowerShell full npx command uses batch launcher without script policy",
    )?;
    manager.stop(&batch);
    check(
        selected_shell.kind == gateway::shell::Kind::Pwsh,
        "test machine selects installed PowerShell 7 first",
    )?;
    let windows = PathBuf::from(std::env::var_os("SystemRoot").unwrap());
    for kind in [
        gateway::shell::Kind::Pwsh,
        gateway::shell::Kind::PowerShell,
        gateway::shell::Kind::Cmd,
    ] {
        let path = match kind {
            gateway::shell::Kind::Pwsh => selected_shell.path.parent().unwrap().to_path_buf(),
            gateway::shell::Kind::PowerShell => PathBuf::new(),
            gateway::shell::Kind::Cmd => windows.join("System32"),
        };
        let result =
            std::process::Command::new(std::env::current_exe().map_err(|e| e.to_string())?)
                .args(["--shell-selection", kind.name()])
                .env("PATH", path)
                .env("ProgramFiles", root.join("missing-shell"))
                .env("ProgramW6432", root.join("missing-shell"))
                .env("LOCALAPPDATA", root.join("missing-shell"))
                .env(
                    "SystemRoot",
                    if kind == gateway::shell::Kind::Cmd {
                        root.join("missing-shell")
                    } else {
                        windows.clone()
                    },
                )
                .output()
                .map_err(|e| e.to_string())?;
        check(
            result.status.success(),
            &format!(
                "startup discovery/fallback selects {} and freezes choice",
                kind.name()
            ),
        )?;
    }
    let record = root.join("shell 中文 参数.jsonl");
    for shell in [
        selected_shell,
        gateway::shell::Shell {
            kind: gateway::shell::Kind::PowerShell,
            path: windows.join("System32/WindowsPowerShell/v1.0/powershell.exe"),
        },
        gateway::shell::Shell {
            kind: gateway::shell::Kind::Cmd,
            path: windows.join("System32/cmd.exe"),
        },
    ] {
        let mut via_shell = make(&mut profiles, "streamableHttp")?;
        let line = format!("\"{}\" \"中文 参数\"", root.join("mock-mcp.exe").display());
        via_shell.env = vec![
            Pair {
                key: "RECORD_FILE".into(),
                value: record.to_string_lossy().into(),
            },
            Pair {
                key: "SECRET".into(),
                value: "shell-test-secret".into(),
            },
        ];
        let (content, mut secrets) = config::build(&via_shell, true)?;
        if shell.kind == gateway::shell::Kind::Cmd {
            secrets.insert(
                "COMSPEC".into(),
                shell.path.to_string_lossy().replace('/', "\\"),
            );
        }
        let mut runtime: Value = serde_json::from_str(&content).map_err(|e| e.to_string())?;
        gateway::shell::configure(&mut runtime["mcpServers"]["service"], &shell, &line);
        let runtime_path = root.join("shell-runtime.json");
        fs::write(&runtime_path, serde_json::to_string(&runtime).unwrap())
            .map_err(|e| e.to_string())?;
        let mut child = Child::spawn(
            &engine,
            &["--config".into(), runtime_path.to_string_lossy().into()],
            &secrets,
            &root,
        )?;
        let readers = [child.output.take().unwrap(), child.errors.take().unwrap()].map(|pipe| {
            thread::spawn(move || {
                use std::io::Read;
                let mut log = String::new();
                let _ = BufReader::new(pipe).read_to_string(&mut log);
                log
            })
        });
        let deadline = Instant::now() + Duration::from_secs(10);
        while net::profile_health(&via_shell).is_err() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(100));
        }
        let result = net::test(&via_shell);
        child.stop();
        let logs = readers
            .into_iter()
            .map(|reader| reader.join().unwrap_or_default())
            .collect::<Vec<_>>()
            .join("\n");
        let tools = result.map_err(|error| format!("{error}\n{logs}"))?;
        check(
            tools.len() == 1
                && tools[0].description == "中文测试工具"
                && logs.contains("fixture-secret: shell-test-secret")
                && fs::read_to_string(&record)
                    .map_err(|e| e.to_string())?
                    .contains("中文 参数"),
            &format!("{:?}: stdin/stdout Unicode MCP and arguments", shell.kind),
        )?;
        fs::remove_file(&record).map_err(|e| e.to_string())?;
    }
    let frozen = gateway::shell::detect()?;
    unsafe {
        std::env::set_var("PATH", "");
    }
    check(
        gateway::shell::detect()?.path == frozen.path,
        "interpreter selection stays frozen after PATH changes",
    )?;
    unsafe {
        std::env::set_var("PATH", original_path);
    }
    relative.command = "not-installed-on-this-machine".into();
    check(
        config::build(&relative, false)?
            .0
            .contains("not-installed-on-this-machine"),
        "copy config without dependencies",
    )?;
    let ips = gateway::network::local_addresses()?;
    let ip = ips
        .first()
        .ok_or("本机网卡 IPv4 集成测试需要至少一个已连接网卡")?
        .ip
        .to_string();
    let imported_ip = config::import(
        &json!({"mcpServers":{"nic":{"command":"example.exe","host":ip}}}).to_string(),
        &[],
    )?;
    check(
        imported_ip.profiles.len() == 1 && imported_ip.profiles[0].listen_host() == ip,
        "import preserves explicit IPv4 listening address",
    )?;
    check(
        config::import(
            r#"{"mcpServers":{"bad":{"command":"example.exe","host":"MY-HOST"}}}"#,
            &[],
        )?
        .profiles
        .is_empty(),
        "import rejects listening hostname",
    )?;
    for transport in ["streamableHttp", "sse", "ws"] {
        let mut bound = make(&mut profiles, transport)?;
        bound.listen_address = Some(ip.clone());
        manager.start(&bound)?;
        check(
            net::profile_health(&bound).is_ok()
                && net::test(&bound)?.len() == 1
                && bound.address(true).contains(&format!("//{ip}:")),
            &format!("explicit NIC IPv4: {transport} health/init/tools"),
        )?;
        check(
            config::validate(&bound, true).is_err(),
            "occupied port detected on selected NIC",
        )?;
        manager.stop(&bound);
    }
    manager.stop_all();
    println!("Completed {count} checks.");
    Ok(())
}
fn gone(pid: u32) -> bool {
    unsafe {
        let process = OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
            0,
            pid,
        );
        if process.is_null() {
            return true;
        }
        let gone = WaitForSingleObject(process, 0) == WAIT_OBJECT_0;
        CloseHandle(process);
        gone
    }
}
