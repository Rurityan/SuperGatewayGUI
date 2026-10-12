//! Explicit, interactive one-off commands use committed packages, never uvx downloads.
use crate::{
    Result,
    dependencies::{self as deps, Network},
    model::Profile,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Serialize, Deserialize)]
pub struct Invocation {
    pub executable: PathBuf,
    pub args: Vec<String>,
    pub environment: BTreeMap<String, String>,
    pub directory: PathBuf,
}
pub fn prepare(
    root: &Path,
    profile: &Profile,
    network: &Network,
    args: Vec<String>,
) -> Result<Invocation> {
    network.validate()?;
    if profile.source != "local" {
        return Err("一次性命令仅支持已安装的本地服务包".into());
    }
    if args.iter().any(|s| s.contains('\0')) {
        return Err("参数不能包含 NUL".into());
    }
    let mut p = profile.clone();
    p.managed = true;
    p.stdio.clear(); // Always choose the committed structured package, not preserved shell text.
    p.dependency.args = args;
    let mut prepared = deps::prepared(root, &p)?;
    let mut environment = prepared
        .env
        .iter()
        .map(|pair| (pair.key.clone(), pair.value.clone()))
        .collect::<BTreeMap<_, _>>();
    for (key, value) in deps::environment(root, &p.dependency, network, true)? {
        environment.retain(|existing, _| !existing.eq_ignore_ascii_case(&key));
        environment.insert(key, value);
    }
    // Setup may deliberately download an addon; managed Python must remain installed-only.
    environment.insert("UV_OFFLINE".into(), "0".into());
    environment.insert("npm_config_offline".into(), "false".into());
    if p.dependency.kind == "python" {
        let venv = deps::installed_path(root, &p)?.join("venv");
        environment.insert("VIRTUAL_ENV".into(), venv.to_string_lossy().into_owned());
        environment.insert(
            "UV_PYTHON".into(),
            venv.join("Scripts/python.exe")
                .to_string_lossy()
                .into_owned(),
        );
        environment.insert(
            "PATH".into(),
            format!("{};{}", venv.join("Scripts").display(), environment["PATH"]),
        );
    }
    let directory = if profile.cwd.is_empty() {
        root.to_path_buf()
    } else {
        PathBuf::from(crate::config::expand(&profile.cwd)?)
    };
    if !directory.is_dir() {
        return Err("一次性命令工作目录不存在，请修正服务工作目录".into());
    }
    prepared.env = environment
        .into_iter()
        .map(|(key, value)| crate::model::Pair { key, value })
        .collect();
    let environment = crate::environment::effective(&prepared)?;
    Ok(Invocation {
        executable: prepared.command.into(),
        args: prepared.args,
        environment,
        directory,
    })
}
pub fn launch(root: &Path, invocation: &Invocation) -> Result<std::process::Child> {
    use std::os::windows::process::CommandExt;
    let directory = root.join("one-shot");
    fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
    let file = directory.join(format!("{}.dat", crate::model::new_id()));
    crate::store::atomic_write(
        &file,
        &crate::store::protect(
            &serde_json::to_vec(invocation).map_err(|e| e.to_string())?,
            true,
        )?,
    )?;
    let result = std::process::Command::new(std::env::current_exe().map_err(|e| e.to_string())?)
        .arg("--one-shot")
        .arg(&file)
        .creation_flags(windows_sys::Win32::System::Threading::CREATE_NEW_CONSOLE)
        .spawn()
        .map_err(|e| format!("打开一次性命令终端失败：{e}"));
    if result.is_err() {
        let _ = fs::remove_file(file);
    }
    result
}
pub fn run(file: &Path) -> Result<()> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::System::Console::*;
    unsafe {
        if GetConsoleWindow().is_null() && AllocConsole() == 0 {
            return Err(crate::native::error("打开交互终端"));
        }
        SetConsoleTitleW(
            crate::native::wide("SuperGateway · 一次性命令（关闭终端结束进程树）").as_ptr(),
        );
    }
    // A GUI launcher may have inherited absent or redirected standard handles.
    // Bind this runner explicitly to its own console before the first I/O.
    let input = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("CONIN$")
        .map_err(|e| e.to_string())?;
    let output = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("CONOUT$")
        .map_err(|e| e.to_string())?;
    unsafe {
        SetStdHandle(STD_INPUT_HANDLE, input.as_raw_handle());
        SetStdHandle(STD_OUTPUT_HANDLE, output.as_raw_handle());
        SetStdHandle(STD_ERROR_HANDLE, output.as_raw_handle());
    }
    let result = (|| {
        let bytes = fs::read(file).map_err(|e| e.to_string())?;
        fs::remove_file(file).map_err(|e| e.to_string())?;
        let invocation: Invocation = serde_json::from_slice(&crate::store::protect(&bytes, false)?)
            .map_err(|e| e.to_string())?;
        println!(
            "一次性命令：{}\n工作目录：{}\n按程序提示操作；本操作独立于 MCP 服务启停。\n",
            invocation.executable.display(),
            invocation.directory.display()
        );
        let child = crate::native::Child::spawn_console(
            &invocation.executable,
            &invocation.args,
            &invocation.environment,
            &invocation.directory,
        )?;
        while !child.wait(250) {}
        let code = child.exit_code();
        println!("\n命令结束，退出码：{code}");
        Ok(())
    })();
    if let Err(error) = &result {
        println!("\n执行失败：{error}");
    }
    println!("按 Enter 关闭此终端。");
    let _ = std::io::stdin().read_line(&mut String::new());
    result
}
