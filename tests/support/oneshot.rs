use gateway::{Result, dependencies as deps, model::Profile, oneshot, store};
use std::{
    fs,
    path::Path,
    process::Command,
    thread,
    time::{Duration, Instant},
};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::System::{Console::*, Threading::*};
fn wait(mut action: impl FnMut() -> bool) -> Result<()> {
    let until = Instant::now() + Duration::from_secs(20);
    while Instant::now() < until {
        if action() {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(50));
    }
    Err("等待一次性命令验收超时".into())
}
fn type_line(pid: u32, line: &str) -> Result<()> {
    use std::os::windows::io::AsRawHandle;
    unsafe {
        let saved = [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE]
            .map(|id| (id, GetStdHandle(id)));
        FreeConsole();
        let result = (|| {
            if AttachConsole(pid) == 0 {
                return Err(gateway::native::error("附加验收终端"));
            }
            let input = fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open("CONIN$")
                .map_err(|e| e.to_string())?;
            let handle = input.as_raw_handle();
            let mut input = vec![];
            for code in line.encode_utf16() {
                for down in [1, 0] {
                    input.push(INPUT_RECORD {
                        EventType: KEY_EVENT as u16,
                        Event: INPUT_RECORD_0 {
                            KeyEvent: KEY_EVENT_RECORD {
                                bKeyDown: down,
                                wRepeatCount: 1,
                                wVirtualKeyCode: if code == 13 { 13 } else { 0 },
                                uChar: KEY_EVENT_RECORD_0 { UnicodeChar: code },
                                ..Default::default()
                            },
                        },
                    });
                }
            }
            let mut written = 0;
            if WriteConsoleInputW(handle, input.as_ptr(), input.len() as u32, &mut written) == 0 {
                return Err(gateway::native::error("输入验收终端"));
            }
            Ok(())
        })();
        FreeConsole();
        AttachConsole(ATTACH_PARENT_PROCESS);
        for (id, handle) in saved {
            SetStdHandle(id, handle);
        }
        result
    }
}
fn alive(pid: u32) -> bool {
    unsafe {
        let handle = gateway::native::Handle(OpenProcess(PROCESS_SYNCHRONIZE, 0, pid));
        !handle.0.is_null() && WaitForSingleObject(handle.0, 0) == WAIT_TIMEOUT
    }
}
pub fn verify(root: &Path, gui: &Path, dependencies: &Path) -> Result<()> {
    use std::os::windows::process::CommandExt;
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(".build")
        .canonicalize()
        .map_err(|e| e.to_string())?;
    if !root
        .canonicalize()
        .map_err(|e| e.to_string())?
        .starts_with(workspace)
    {
        return Err("只允许隔离的 .build 目录".into());
    }
    gateway::storage::set_dependency_directory(root, dependencies)?;
    let mut p = Profile {
        managed: true,
        ..Default::default()
    };
    p.dependency.package = "controlled-one-shot".into();
    p.dependency.version = "1.0.0".into();
    p.dependency.installed = gateway::model::new_id();
    p.dependency.args = vec!["keep-server-argument".into()];
    let dir = deps::installed_path(root, &p)?;
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let script = r#"const fs=require('node:fs');const path=require('node:path');
const leaf=require('node:child_process').spawn(process.execPath,['-e','setInterval(()=>{},1000)'],{stdio:'ignore'});
const base=process.env.SG_ONESHOT_OUTPUT;
fs.writeFileSync(path.join(base,'started.json'),JSON.stringify({pid:process.pid,leaf:leaf.pid,args:process.argv.slice(2),cwd:process.cwd(),secret:process.env.TEST_SECRET,cache:process.env.npm_config_cache}));
console.log('Controlled setup: enter yes');
require('node:readline').createInterface({input:process.stdin,output:process.stdout}).once('line',line=>{
fs.writeFileSync(path.join(base,'result.json'),JSON.stringify({input:line}));process.exit(0);});"#;
    fs::write(dir.join("entry.js"), script).map_err(|e| e.to_string())?;
    let receipt = deps::Receipt {
        spec: p.dependency.clone(),
        resolved_version: "1.0.0".into(),
        executable: "entry.js".into(),
        args: vec![],
    };
    fs::write(
        dir.join("installed.json"),
        serde_json::to_vec(&receipt).unwrap(),
    )
    .map_err(|e| e.to_string())?;
    p.env = vec![
        gateway::model::Pair {
            key: "SG_ONESHOT_OUTPUT".into(),
            value: root.to_string_lossy().into_owned(),
        },
        gateway::model::Pair {
            key: "TEST_SECRET".into(),
            value: "console-secret".into(),
        },
    ];
    let args = vec![
        "setup".into(),
        "中文 空格".into(),
        "& echo %PATH% $(whoami)".into(),
        "".into(),
        "quoted\"value\\".into(),
    ];
    let invocation = oneshot::prepare(root, &p, &Default::default(), args.clone())?;
    let request = root.join("invocation.dat");
    let protected = store::protect(&serde_json::to_vec(&invocation).unwrap(), true)?;
    if String::from_utf8_lossy(&protected).contains("console-secret") {
        return Err("未加密终端请求".into());
    }
    fs::write(&request, protected).map_err(|e| e.to_string())?;
    let mut terminal = Command::new(gui)
        .arg("--one-shot")
        .arg(&request)
        .env("PATH", "")
        .creation_flags(CREATE_NEW_CONSOLE)
        .spawn()
        .map_err(|e| e.to_string())?;
    let result = (|| {
        wait(|| root.join("started.json").is_file())?;
        let started: serde_json::Value =
            serde_json::from_slice(&fs::read(root.join("started.json")).unwrap()).unwrap();
        if started["args"] != serde_json::json!(args) || started["secret"] != "console-secret" {
            return Err("参数或子进程环境不一致".into());
        }
        if !Path::new(started["cache"].as_str().unwrap_or("")).starts_with(dependencies) {
            return Err("终端缓存目录不一致".into());
        }
        if request.exists() {
            return Err("加密请求未清理".into());
        }
        println!(
            "PASS empty-PATH console starts committed package with exact Unicode/special/empty arguments"
        );
        println!(
            "PASS console uses child-only managed cache and secret environment; encrypted request consumed"
        );
        type_line(terminal.id(), "yes\r")?;
        wait(|| root.join("result.json").is_file())?;
        let response: serde_json::Value =
            serde_json::from_slice(&fs::read(root.join("result.json")).unwrap()).unwrap();
        if response["input"] != "yes" {
            return Err("终端未接受交互输入".into());
        }
        let leaf = started["leaf"].as_u64().unwrap() as u32;
        wait(|| !alive(leaf))?;
        println!("PASS interactive setup reads terminal response");
        println!("PASS completion cleans descendant process via Job Object");
        thread::sleep(Duration::from_millis(250));
        type_line(terminal.id(), "\r")?;
        wait(|| terminal.try_wait().ok().flatten().is_some())?;
        println!("PASS completed terminal remains visible until Enter then exits");
        fs::remove_file(root.join("started.json")).map_err(|e| e.to_string())?;
        fs::write(
            &request,
            store::protect(&serde_json::to_vec(&invocation).unwrap(), true)?,
        )
        .map_err(|e| e.to_string())?;
        terminal = Command::new(gui)
            .arg("--one-shot")
            .arg(&request)
            .env("PATH", "")
            .creation_flags(CREATE_NEW_CONSOLE)
            .spawn()
            .map_err(|e| e.to_string())?;
        wait(|| root.join("started.json").is_file())?;
        let started: serde_json::Value =
            serde_json::from_slice(&fs::read(root.join("started.json")).unwrap()).unwrap();
        terminal.kill().map_err(|e| e.to_string())?;
        let _ = terminal.wait();
        wait(|| {
            !alive(started["pid"].as_u64().unwrap() as u32)
                && !alive(started["leaf"].as_u64().unwrap() as u32)
        })?;
        println!("PASS abnormal terminal exit cleans command and descendant process tree");
        Ok(())
    })();
    if result.is_err() {
        let _ = terminal.kill();
        let _ = terminal.wait();
    }
    deps::remove_service(root, &p.id, &[])?;
    result
}
