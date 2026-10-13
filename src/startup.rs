//! Per-user interactive elevated startup. Never changes machine-wide startup.
use crate::{
    Result,
    native::{self, Handle, wide},
};
use std::{
    mem::{size_of, zeroed},
    os::windows::process::CommandExt,
    path::Path,
    process::Command,
    ptr::{null, null_mut},
};
use windows_sys::Win32::{
    Foundation::*,
    Security::{Authorization::ConvertSidToStringSidW, *},
    System::{Registry::*, Threading::*},
    UI::{Shell::*, WindowsAndMessaging::SW_SHOWNORMAL},
};

fn token() -> Result<Handle> {
    unsafe {
        let mut handle = null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut handle) == 0 {
            return Err(native::error("读取当前用户令牌"));
        }
        Ok(Handle(handle))
    }
}
pub fn is_elevated() -> bool {
    let Ok(token) = token() else { return false };
    unsafe {
        let mut elevation: TOKEN_ELEVATION = zeroed();
        let mut length = 0;
        GetTokenInformation(
            token.0,
            TokenElevation,
            (&mut elevation as *mut TOKEN_ELEVATION).cast(),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut length,
        ) != 0
            && elevation.TokenIsElevated != 0
    }
}
pub fn user_sid() -> Result<String> {
    let token = token()?;
    unsafe {
        let mut length = 0;
        GetTokenInformation(token.0, TokenUser, null_mut(), 0, &mut length);
        let mut buffer = vec![0usize; (length as usize).div_ceil(size_of::<usize>())];
        if GetTokenInformation(
            token.0,
            TokenUser,
            buffer.as_mut_ptr().cast(),
            length,
            &mut length,
        ) == 0
        {
            return Err(native::error("读取用户 SID"));
        }
        let user = &*(buffer.as_ptr() as *const TOKEN_USER);
        let mut sid = null_mut();
        if ConvertSidToStringSidW(user.User.Sid, &mut sid) == 0 {
            return Err(native::error("转换用户 SID"));
        }
        let mut n = 0;
        while *sid.add(n) != 0 {
            n += 1;
        }
        let value = String::from_utf16_lossy(std::slice::from_raw_parts(sid, n));
        LocalFree(sid.cast());
        Ok(value)
    }
}
pub fn arguments(root: &Path, custom: bool, background: bool) -> String {
    let mut args = String::new();
    if custom {
        args = format!("--data-dir {}", native::quote(&root.to_string_lossy()));
    }
    if background {
        args.push_str(" --background");
    }
    args.trim().into()
}
pub fn restart_elevated(owner: HWND, root: &Path, custom: bool) -> Result<()> {
    let exe = wide(std::env::current_exe().map_err(|e| e.to_string())?);
    let args = wide(format!(
        "{} --wait-for-pid {}",
        arguments(root, custom, false),
        std::process::id()
    ));
    unsafe {
        let mut info: SHELLEXECUTEINFOW = zeroed();
        info.cbSize = size_of::<SHELLEXECUTEINFOW>() as u32;
        info.fMask = SEE_MASK_NOCLOSEPROCESS;
        info.hwnd = owner;
        let verb = wide("runas");
        info.lpVerb = verb.as_ptr();
        info.lpFile = exe.as_ptr();
        info.lpParameters = args.as_ptr();
        info.nShow = SW_SHOWNORMAL;
        if ShellExecuteExW(&mut info) == 0 {
            let code = GetLastError();
            return Err(if code == ERROR_CANCELLED {
                "已取消管理员授权，当前程序继续运行".into()
            } else {
                native::error("以管理员身份重启")
            });
        }
        let _process = Handle(info.hProcess);
    }
    Ok(())
}
pub fn wait_for_previous(pid: u32) -> Result<()> {
    unsafe {
        let handle = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        if handle.is_null() {
            return if GetLastError() == ERROR_INVALID_PARAMETER {
                Ok(())
            } else {
                Err(native::error("等待原应用退出"))
            };
        }
        let handle = Handle(handle);
        if WaitForSingleObject(handle.0, 120_000) != WAIT_OBJECT_0 {
            return Err("原应用未能在 120 秒内退出，管理员重启已停止".into());
        }
    }
    Ok(())
}
fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
pub fn task_xml(exe: &str, args: &str, sid: &str, cwd: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
<Triggers><LogonTrigger><Enabled>true</Enabled><UserId>{sid}</UserId></LogonTrigger></Triggers>
<Principals><Principal id="User"><UserId>{sid}</UserId><LogonType>InteractiveToken</LogonType><RunLevel>HighestAvailable</RunLevel></Principal></Principals>
<Settings><MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy><DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries><StopIfGoingOnBatteries>false</StopIfGoingOnBatteries><ExecutionTimeLimit>PT0S</ExecutionTimeLimit><Enabled>true</Enabled></Settings>
<Actions Context="User"><Exec><Command>{exe}</Command><Arguments>{args}</Arguments><WorkingDirectory>{cwd}</WorkingDirectory></Exec></Actions></Task>"#,
        sid = xml_escape(sid),
        exe = xml_escape(exe),
        args = xml_escape(args),
        cwd = xml_escape(cwd)
    )
}
fn system_command(name: &str) -> Result<Command> {
    // Use the system directory, never a PATH-resolved executable when elevated.
    let mut buffer = [0u16; 32768];
    let n = unsafe {
        windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW(
            buffer.as_mut_ptr(),
            buffer.len() as u32,
        )
    };
    if n == 0 || n as usize >= buffer.len() {
        return Err(native::error("读取系统目录"));
    }
    let mut command = Command::new(
        std::path::PathBuf::from(String::from_utf16_lossy(&buffer[..n as usize])).join(name),
    );
    command.creation_flags(CREATE_NO_WINDOW);
    Ok(command)
}
fn scheduled(enabled: bool, root: &Path, custom: bool) -> Result<()> {
    if !is_elevated() {
        return Err("修改管理员登录启动必须先以管理员身份运行".into());
    }
    let sid = user_sid()?;
    use sha2::{Digest, Sha256};
    let name = format!(
        "SuperGatewayGUI-{}-{:x}",
        sid,
        Sha256::digest(root.to_string_lossy().as_bytes())
    );
    if enabled {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let text = task_xml(
            &exe.to_string_lossy(),
            &arguments(root, custom, true),
            &sid,
            &exe.parent().unwrap().to_string_lossy(),
        );
        let path = root.join(format!("startup-{}.xml", crate::model::new_id()));
        let bytes = std::iter::once(0xfeffu16)
            .chain(text.encode_utf16())
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>();
        std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
        let result = system_command("schtasks.exe")?
            .args(["/Create", "/TN", &name, "/XML"])
            .arg(&path)
            .arg("/F")
            .output();
        let _ = std::fs::remove_file(&path);
        let result = result.map_err(|e| format!("创建管理员启动任务失败：{e}"))?;
        if !result.status.success() {
            return Err("创建管理员启动任务失败，请检查任务计划程序服务和当前管理员权限".into());
        }
    } else {
        let query = system_command("schtasks.exe")?
            .args(["/Query", "/TN", &name])
            .output()
            .map_err(|e| e.to_string())?;
        if !query.status.success() {
            // Distinguish an absent task from an unavailable scheduler.
            if !system_command("schtasks.exe")?
                .arg("/Query")
                .output()
                .map_err(|e| e.to_string())?
                .status
                .success()
            {
                return Err("无法访问任务计划程序，未修改启动配置".into());
            }
            return Ok(());
        }
        if !system_command("schtasks.exe")?
            .args(["/Delete", "/TN", &name, "/F"])
            .output()
            .map_err(|e| e.to_string())?
            .status
            .success()
        {
            return Err("删除管理员启动任务失败".into());
        }
    }
    Ok(())
}
fn normal(enabled: bool, root: &Path, custom: bool) -> Result<()> {
    unsafe {
        let mut raw = null_mut();
        let status = RegCreateKeyExW(
            HKEY_CURRENT_USER,
            wide("Software\\Microsoft\\Windows\\CurrentVersion\\Run").as_ptr(),
            0,
            null(),
            0,
            KEY_SET_VALUE,
            null(),
            &mut raw,
            null_mut(),
        );
        if status != 0 {
            return Err(format!("设置登录启动失败：{status}"));
        }
        let name = wide("SuperGatewayGUI");
        let status = if enabled {
            let value = wide(format!(
                "{} {}",
                native::quote(
                    &std::env::current_exe()
                        .map_err(|e| e.to_string())?
                        .to_string_lossy()
                ),
                arguments(root, custom, true)
            ));
            RegSetValueExW(
                raw,
                name.as_ptr(),
                0,
                REG_SZ,
                value.as_ptr().cast(),
                (value.len() * 2) as u32,
            )
        } else {
            RegDeleteValueW(raw, name.as_ptr())
        };
        RegCloseKey(raw);
        if status == 0 || !enabled && status == ERROR_FILE_NOT_FOUND {
            Ok(())
        } else {
            Err(format!("设置登录启动失败：{status}"))
        }
    }
}
pub fn apply(
    old_admin: bool,
    normal_enabled: bool,
    admin_enabled: bool,
    root: &Path,
    custom: bool,
) -> Result<()> {
    if normal_enabled && admin_enabled {
        return Err("普通登录启动与管理员登录启动不能同时开启".into());
    }
    if old_admin || admin_enabled {
        scheduled(admin_enabled, root, custom)?;
    }
    if let Err(error) = normal(normal_enabled, root, custom) {
        if old_admin || admin_enabled {
            let rollback = scheduled(old_admin, root, custom);
            return Err(format!(
                "{error}；管理员任务回退：{}",
                rollback.err().unwrap_or_else(|| "成功".into())
            ));
        }
        return Err(error);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn elevated_task_is_interactive_user_only_and_escaped() {
        let xml = task_xml(
            r#"D:\中文 & 空格\app.exe"#,
            r#"--data-dir "D:\A&B" --background"#,
            "S-1-5-21-123",
            r"D:\中文 & 空格",
        );
        assert!(xml.contains("<RunLevel>HighestAvailable</RunLevel>"));
        assert!(xml.contains("<LogonType>InteractiveToken</LogonType>"));
        assert_eq!(xml.matches("<UserId>S-1-5-21-123</UserId>").count(), 2);
        assert!(xml.contains("A&amp;B&quot;"));
        assert!(xml.contains("<ExecutionTimeLimit>PT0S</ExecutionTimeLimit>"));
        assert!(!xml.contains("Password"));
    }
    #[test]
    fn preserved_data_directory_and_old_settings_defaults() {
        let args = arguments(Path::new(r"D:\中文 空格"), true, true);
        assert_eq!(args, r#"--data-dir "D:\中文 空格" --background"#);
        let settings: crate::model::Settings =
            serde_json::from_str(r#"{"LoginStartup":true,"Services":[]}"#).unwrap();
        assert!(settings.login_startup);
        assert!(!settings.admin_login_startup);
    }
    #[test]
    fn invalid_or_unprivileged_changes_do_not_touch_startup() {
        assert!(apply(false, true, true, Path::new("X:\\unused"), true).is_err());
        if !is_elevated() {
            assert!(
                apply(false, false, true, Path::new("X:\\unused"), true)
                    .unwrap_err()
                    .contains("管理员")
            );
            assert!(
                apply(true, false, false, Path::new("X:\\unused"), true)
                    .unwrap_err()
                    .contains("管理员")
            );
        }
    }
}
