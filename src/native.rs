use crate::Result;
use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    fs::File,
    mem::{size_of, zeroed},
    os::windows::{ffi::OsStrExt, io::FromRawHandle},
    path::Path,
    ptr::{null, null_mut},
};
use windows_sys::Win32::{
    Foundation::*,
    Security::SECURITY_ATTRIBUTES,
    System::{JobObjects::*, Pipes::CreatePipe, Threading::*},
};

pub fn wide(s: impl AsRef<OsStr>) -> Vec<u16> {
    s.as_ref().encode_wide().chain(Some(0)).collect()
}
pub fn error(context: &str) -> String {
    format!("{context}：{}", std::io::Error::last_os_error())
}
pub struct Handle(pub HANDLE);
unsafe impl Send for Handle {}
unsafe impl Sync for Handle {}
impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}
fn pipe() -> Result<(Handle, Handle)> {
    let mut read = null_mut();
    let mut write = null_mut();
    let attrs = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: null_mut(),
        bInheritHandle: 1,
    };
    if unsafe { CreatePipe(&mut read, &mut write, &attrs, 0) } == 0 {
        return Err(error("创建进程管道"));
    }
    Ok((Handle(read), Handle(write)))
}
fn file(mut handle: Handle) -> File {
    let raw = handle.0;
    handle.0 = null_mut();
    unsafe { File::from_raw_handle(raw) }
}
pub fn quote(s: &str) -> String {
    let mut output = String::from("\"");
    let mut slashes = 0;
    for c in s.chars() {
        if c == '\\' {
            slashes += 1;
        } else {
            output.extend(std::iter::repeat_n(
                '\\',
                if c == '"' { slashes * 2 + 1 } else { slashes },
            ));
            slashes = 0;
            output.push(c);
        }
    }
    output.extend(std::iter::repeat_n('\\', slashes * 2));
    output.push('"');
    output
}
pub struct Child {
    pub id: u32,
    process: Handle,
    job: Handle,
    pub input: Option<File>,
    pub output: Option<File>,
    pub errors: Option<File>,
}
impl Child {
    /// Interactive child shares the caller's console, with a suspended Job assignment.
    pub fn spawn_console(
        exe: &Path,
        args: &[String],
        env: &BTreeMap<String, String>,
        cwd: &Path,
    ) -> Result<Self> {
        unsafe {
            let job = Handle(CreateJobObjectW(null(), null()));
            if job.0.is_null() {
                return Err(error("创建终端 Job Object"));
            }
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = zeroed();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if SetInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            ) == 0
            {
                return Err(error("设置终端 Job Object"));
            }
            let mut startup: STARTUPINFOW = zeroed();
            startup.cb = size_of::<STARTUPINFOW>() as u32;
            let mut process: PROCESS_INFORMATION = zeroed();
            let mut command = wide(
                std::iter::once(quote(&exe.to_string_lossy()))
                    .chain(args.iter().map(|s| quote(s)))
                    .collect::<Vec<_>>()
                    .join(" "),
            );
            let mut values: BTreeMap<OsString, OsString> = BTreeMap::new();
            for (key, value) in env {
                values.insert(key.to_uppercase().into(), value.into());
            }
            let mut block = vec![];
            for (key, value) in values {
                block.extend(key.encode_wide());
                block.push('=' as u16);
                block.extend(value.encode_wide());
                block.push(0);
            }
            block.push(0);
            if CreateProcessW(
                wide(exe).as_ptr(),
                command.as_mut_ptr(),
                null(),
                null(),
                0,
                CREATE_SUSPENDED | CREATE_UNICODE_ENVIRONMENT,
                block.as_ptr().cast(),
                wide(cwd).as_ptr(),
                &startup,
                &mut process,
            ) == 0
            {
                return Err(error("启动交互命令"));
            }
            let process_handle = Handle(process.hProcess);
            let thread = Handle(process.hThread);
            if AssignProcessToJobObject(job.0, process_handle.0) == 0 {
                TerminateProcess(process_handle.0, 1);
                return Err(error("将交互命令加入 Job Object"));
            }
            if ResumeThread(thread.0) == u32::MAX {
                TerminateJobObject(job.0, 1);
                return Err(error("恢复交互命令"));
            }
            Ok(Self {
                id: process.dwProcessId,
                process: process_handle,
                job,
                input: None,
                output: None,
                errors: None,
            })
        }
    }
    pub fn spawn(
        exe: &Path,
        args: &[String],
        env: &BTreeMap<String, String>,
        cwd: &Path,
    ) -> Result<Self> {
        Self::spawn_environment(exe, args, env, cwd, true)
    }
    pub fn spawn_exact(
        exe: &Path,
        args: &[String],
        env: &BTreeMap<String, String>,
        cwd: &Path,
    ) -> Result<Self> {
        Self::spawn_environment(exe, args, env, cwd, false)
    }
    fn spawn_environment(
        exe: &Path,
        args: &[String],
        env: &BTreeMap<String, String>,
        cwd: &Path,
        inherit: bool,
    ) -> Result<Self> {
        unsafe {
            let (inputr, inputw) = pipe()?;
            let (outputr, outputw) = pipe()?;
            let (errorr, errorw) = pipe()?;
            for handle in [&inputw, &outputr, &errorr] {
                if SetHandleInformation(handle.0, HANDLE_FLAG_INHERIT, 0) == 0 {
                    return Err(error("设置管道继承"));
                }
            }
            let job = Handle(CreateJobObjectW(null(), null()));
            if job.0.is_null() {
                return Err(error("创建 Job Object"));
            }
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = zeroed();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if SetInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            ) == 0
            {
                return Err(error("设置进程树退出清理"));
            }
            let mut startup: STARTUPINFOW = zeroed();
            startup.cb = size_of::<STARTUPINFOW>() as u32;
            startup.dwFlags = STARTF_USESTDHANDLES;
            startup.hStdInput = inputr.0;
            startup.hStdOutput = outputw.0;
            startup.hStdError = errorw.0;
            let mut process: PROCESS_INFORMATION = zeroed();
            let mut cmd = wide(
                std::iter::once(quote(&exe.to_string_lossy()))
                    .chain(args.iter().map(|s| quote(s)))
                    .collect::<Vec<_>>()
                    .join(" "),
            );
            let mut environment: BTreeMap<OsString, OsString> = std::env::vars_os()
                .filter(|_| inherit)
                .map(|(k, v)| (OsString::from(k.to_string_lossy().to_uppercase()), v))
                .collect();
            for (k, v) in env {
                environment.insert(k.to_uppercase().into(), v.into());
            }
            let mut block = vec![];
            for (k, v) in environment {
                block.extend(k.encode_wide());
                block.push('=' as u16);
                block.extend(v.encode_wide());
                block.push(0);
            }
            block.push(0);
            if CreateProcessW(
                wide(exe.as_os_str()).as_ptr(),
                cmd.as_mut_ptr(),
                null(),
                null(),
                1,
                CREATE_NO_WINDOW | CREATE_SUSPENDED | CREATE_UNICODE_ENVIRONMENT,
                block.as_ptr().cast(),
                wide(cwd.as_os_str()).as_ptr(),
                &startup,
                &mut process,
            ) == 0
            {
                return Err(error("启动网关"));
            }
            let process_handle = Handle(process.hProcess);
            let thread = Handle(process.hThread);
            if AssignProcessToJobObject(job.0, process_handle.0) == 0 {
                TerminateProcess(process_handle.0, 1);
                return Err(error("将挂起网关加入 Job Object"));
            }
            if ResumeThread(thread.0) == u32::MAX {
                TerminateJobObject(job.0, 1);
                return Err(error("恢复网关线程"));
            }
            drop(inputr);
            drop(outputw);
            drop(errorw);
            Ok(Self {
                id: process.dwProcessId,
                process: process_handle,
                job,
                input: Some(file(inputw)),
                output: Some(file(outputr)),
                errors: Some(file(errorr)),
            })
        }
    }
    pub fn exited(&self) -> bool {
        unsafe { WaitForSingleObject(self.process.0, 0) == WAIT_OBJECT_0 }
    }
    pub fn exit_code(&self) -> u32 {
        let mut code = 0;
        unsafe {
            GetExitCodeProcess(self.process.0, &mut code);
        }
        code
    }
    pub fn wait(&self, milliseconds: u32) -> bool {
        unsafe { WaitForSingleObject(self.process.0, milliseconds) == WAIT_OBJECT_0 }
    }
    pub fn terminate(&self) {
        unsafe {
            TerminateJobObject(self.job.0, 1);
            WaitForSingleObject(self.process.0, 3000);
        }
    }
    pub fn stop(&mut self) {
        self.input.take();
        if !self.wait(8000) {
            self.terminate();
        }
    }
}
impl Drop for Child {
    fn drop(&mut self) {
        self.input.take();
    }
}

pub fn file_dialog(
    owner: HWND,
    save: bool,
    filter: &str,
    suggested: &str,
) -> Option<std::path::PathBuf> {
    use windows_sys::Win32::UI::Controls::Dialogs::*;
    let mut buffer = vec![0u16; 32768];
    let name = wide(suggested);
    buffer[..name.len()].copy_from_slice(&name);
    let filter = wide(filter.replace('|', "\0") + "\0");
    let mut dialog: OPENFILENAMEW = unsafe { zeroed() };
    dialog.lStructSize = size_of::<OPENFILENAMEW>() as u32;
    dialog.hwndOwner = owner;
    dialog.lpstrFilter = filter.as_ptr();
    dialog.lpstrFile = buffer.as_mut_ptr();
    dialog.nMaxFile = buffer.len() as u32;
    dialog.Flags = OFN_EXPLORER
        | OFN_NOCHANGEDIR
        | OFN_PATHMUSTEXIST
        | if save {
            OFN_OVERWRITEPROMPT
        } else {
            OFN_FILEMUSTEXIST
        };
    let ok = unsafe {
        if save {
            GetSaveFileNameW(&mut dialog)
        } else {
            GetOpenFileNameW(&mut dialog)
        }
    };
    if ok == 0 {
        None
    } else {
        Some(std::path::PathBuf::from(OsString::from(
            String::from_utf16_lossy(&buffer[..buffer.iter().position(|v| *v == 0).unwrap_or(0)]),
        )))
    }
}
pub fn folder_dialog(owner: HWND) -> Option<std::path::PathBuf> {
    folder_dialog_title(owner, "选择 MCP 工作目录")
}
pub fn folder_dialog_title(owner: HWND, description: &str) -> Option<std::path::PathBuf> {
    use windows_sys::Win32::{System::Com::CoTaskMemFree, UI::Shell::*};
    let title = wide(description);
    let mut info: BROWSEINFOW = unsafe { zeroed() };
    info.hwndOwner = owner;
    info.lpszTitle = title.as_ptr();
    info.ulFlags = BIF_RETURNONLYFSDIRS | BIF_NEWDIALOGSTYLE;
    let pidl = unsafe { SHBrowseForFolderW(&info) };
    if pidl.is_null() {
        return None;
    }
    let mut path = vec![0u16; 32768];
    let ok = unsafe { SHGetPathFromIDListW(pidl, path.as_mut_ptr()) };
    unsafe {
        CoTaskMemFree(pidl.cast());
    }
    if ok == 0 {
        None
    } else {
        Some(
            String::from_utf16_lossy(&path[..path.iter().position(|v| *v == 0).unwrap_or(0)])
                .into(),
        )
    }
}
