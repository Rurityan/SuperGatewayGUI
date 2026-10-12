//! Reusable DPAPI-protected stdio launch records preserve exact service environments.
use crate::{Result, model::Profile, oneshot::Invocation};
use std::{collections::BTreeMap, fs, io, path::Path, thread};

pub fn export(root: &Path, p: &Profile, engine: &Path, gui: &Path) -> Result<String> {
    crate::config::validate(p, false)?;
    let gui = std::path::absolute(gui).map_err(|e| e.to_string())?;
    let gui = gui.as_path();
    let invocation = if p.source == "local" {
        let prepared = crate::environment::prepare(&crate::dependencies::prepared(root, p)?)?;
        // Validate service fields and command without requiring its network port to be free.
        crate::config::validate(&prepared, false)?;
        let environment = crate::environment::effective(&prepared)?;
        let (executable, args) = if prepared.stdio.is_empty() {
            let executable = crate::config::resolve_with_path(
                &crate::config::expand(&prepared.command)?,
                &environment["PATH"],
            )?;
            if executable.extension().is_some_and(|ext| {
                ext.eq_ignore_ascii_case("cmd") || ext.eq_ignore_ascii_case("bat")
            }) {
                let line = std::iter::once(crate::native::quote(&executable.to_string_lossy()))
                    .chain(prepared.args.iter().map(|s| crate::native::quote(s)))
                    .collect::<Vec<_>>()
                    .join(" ");
                let cmd = Path::new(&crate::environment::windows_paths()[0]).join("cmd.exe");
                (
                    cmd,
                    vec!["/d".into(), "/s".into(), "/c".into(), format!("\"{line}\"")],
                )
            } else {
                (executable, prepared.args)
            }
        } else {
            let shell = crate::shell::detect()?;
            if shell.kind == crate::shell::Kind::Cmd {
                (
                    shell.path,
                    vec!["/d".into(), "/s".into(), "/c".into(), prepared.stdio],
                )
            } else {
                crate::shell::powershell_command(&shell, &prepared.stdio)
            }
        };
        Invocation {
            executable,
            args,
            environment,
            directory: if p.cwd.is_empty() {
                gui.parent().ok_or("应用目录无效")?.into()
            } else {
                std::path::absolute(crate::config::expand(&p.cwd)?).map_err(|e| e.to_string())?
            },
        }
    } else {
        let content: serde_json::Value =
            serde_json::from_str(&crate::config::client_export(p, engine)?)
                .map_err(|e| e.to_string())?;
        let entry = &content["mcpServers"][&p.name];
        Invocation {
            executable: entry["command"].as_str().ok_or("导出入口无效")?.into(),
            args: serde_json::from_value(entry["args"].clone()).map_err(|e| e.to_string())?,
            environment: crate::environment::effective(p)?,
            directory: gui.parent().ok_or("应用目录无效")?.into(),
        }
    };
    let directory = root.join("exports");
    fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
    let file = directory.join(format!("{}.dat", crate::model::new_id()));
    crate::store::atomic_write(
        &file,
        &crate::store::protect(
            &serde_json::to_vec(&invocation).map_err(|e| e.to_string())?,
            true,
        )?,
    )?;
    let environment: BTreeMap<String, String> = invocation
        .environment
        .iter()
        .filter(|(k, _)| p.env.iter().any(|e| e.key.eq_ignore_ascii_case(k)) || *k == "PATH")
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    Ok(serde_json::to_string_pretty(&serde_json::json!({"mcpServers":{&p.name:{"command":std::path::absolute(gui).map_err(|e| e.to_string())?, "args":["--stdio-launch",std::path::absolute(file).map_err(|e| e.to_string())?],"env":environment}}})).unwrap())
}
pub fn run(file: &Path) -> Result<u32> {
    let invocation: Invocation = serde_json::from_slice(&crate::store::protect(
        &fs::read(file).map_err(|e| format!("导出记录无法读取，请重新导出：{e}"))?,
        false,
    )?)
    .map_err(|e| e.to_string())?;
    let mut child = crate::native::Child::spawn_exact(
        &invocation.executable,
        &invocation.args,
        &invocation.environment,
        &invocation.directory,
    )?;
    let mut input = child.input.take().unwrap();
    let (closed, input_done) = std::sync::mpsc::channel();
    thread::spawn(move || {
        let _ = io::copy(&mut io::stdin().lock(), &mut input);
        drop(input);
        let _ = closed.send(());
    });
    let mut output = child.output.take().unwrap();
    let out = thread::spawn(move || io::copy(&mut output, &mut io::stdout().lock()));
    let mut errors = child.errors.take().unwrap();
    let err = thread::spawn(move || io::copy(&mut errors, &mut io::stderr().lock()));
    while !child.wait(250) {
        if input_done.try_recv().is_ok() {
            if !child.wait(8000) {
                child.terminate();
            }
            break;
        }
    }
    let code = child.exit_code();
    drop(child); // Kill any descendants before waiting for their inherited pipes.
    out.join()
        .map_err(|_| "stdio 输出线程失败")?
        .map_err(|e| e.to_string())?;
    err.join()
        .map_err(|_| "stdio 错误线程失败")?
        .map_err(|e| e.to_string())?;
    Ok(code)
}
