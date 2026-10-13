#![allow(unsafe_op_in_unsafe_fn)]
use gateway::{
    Result,
    model::{Profile, Settings},
    native::wide,
    net::Session,
    store::Store,
};
use std::{
    path::PathBuf,
    process::Command,
    ptr::{null, null_mut},
    thread,
    time::{Duration, Instant},
};
use windows_sys::Win32::UI::Shell::{NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW, Shell_NotifyIconW};
use windows_sys::Win32::UI::{
    Controls::*, HiDpi::*, Input::KeyboardAndMouse::IsWindowEnabled, Shell::ExtractIconExW,
    WindowsAndMessaging::*,
};
use windows_sys::Win32::{
    Foundation::{FreeLibrary, HWND, LPARAM},
    Graphics::Gdi::*,
    System::LibraryLoader::{
        LOAD_LIBRARY_AS_DATAFILE, LOAD_LIBRARY_AS_IMAGE_RESOURCE, LoadLibraryExW,
    },
    System::{DataExchange::*, Memory::*},
};
unsafe fn icon_size(icon: HICON) -> Option<(i32, i32)> {
    let mut info = ICONINFO::default();
    if icon.is_null() || GetIconInfo(icon, &mut info) == 0 {
        return None;
    }
    let mut bitmap = BITMAP::default();
    let got = GetObjectW(
        info.hbmColor,
        std::mem::size_of::<BITMAP>() as i32,
        (&mut bitmap as *mut BITMAP).cast(),
    );
    if !info.hbmColor.is_null() {
        DeleteObject(info.hbmColor);
    }
    if !info.hbmMask.is_null() {
        DeleteObject(info.hbmMask);
    }
    if got == 0 {
        None
    } else {
        Some((bitmap.bmWidth, bitmap.bmHeight))
    }
}
// Read the displayed client pixels, rather than WM_PRINT (which repaints the
// controls and would hide stale pixels left by incremental layout).
unsafe fn client_pixels(hwnd: HWND) -> Result<Vec<u8>> {
    let mut rect = std::mem::zeroed();
    GetClientRect(hwnd, &mut rect);
    let dc = GetDC(hwnd);
    let memory = CreateCompatibleDC(dc);
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: rect.right,
            biHeight: -rect.bottom,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            ..std::mem::zeroed()
        },
        ..std::mem::zeroed()
    };
    let mut bits = null_mut();
    let bitmap = CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
    if bitmap.is_null() {
        DeleteDC(memory);
        ReleaseDC(hwnd, dc);
        return Err("创建重绘测试位图失败".into());
    }
    let old = SelectObject(memory, bitmap);
    let copied = BitBlt(memory, 0, 0, rect.right, rect.bottom, dc, 0, 0, SRCCOPY);
    GdiFlush();
    let pixels =
        std::slice::from_raw_parts(bits.cast::<u8>(), (rect.right * rect.bottom * 4) as usize)
            .to_vec();
    SelectObject(memory, old);
    DeleteObject(bitmap);
    DeleteDC(memory);
    ReleaseDC(hwnd, dc);
    if copied == 0 {
        Err("读取窗口像素失败".into())
    } else {
        Ok(pixels)
    }
}
unsafe fn repaint_is_clean(hwnd: HWND) -> Result<bool> {
    thread::sleep(Duration::from_millis(150));
    let before = client_pixels(hwnd)?;
    RedrawWindow(
        hwnd,
        null(),
        null_mut(),
        RDW_INVALIDATE | RDW_ERASE | RDW_ALLCHILDREN | RDW_UPDATENOW,
    );
    thread::sleep(Duration::from_millis(150));
    let after = client_pixels(hwnd)?;
    // Themed edit borders can still fade by a few RGB levels after moving.
    // Ignore that tiny color variation and the unused DIB alpha byte, while
    // detecting stale text, missing controls and copied background pixels.
    let count = before
        .chunks_exact(4)
        .zip(after.chunks_exact(4))
        .filter(|(a, b)| (0..3).any(|i| a[i].abs_diff(b[i]) > 4))
        .count();
    if count > 0 {
        let mut rect = std::mem::zeroed();
        GetClientRect(hwnd, &mut rect);
        for (name, pixels) in [("before", &before), ("after", &after)] {
            let mut ppm = format!("P6\n{} {}\n255\n", rect.right, rect.bottom).into_bytes();
            for pixel in pixels.chunks_exact(4) {
                ppm.extend_from_slice(&[pixel[2], pixel[1], pixel[0]]);
            }
            std::fs::write(format!("artifacts/repaint-{name}.ppm"), ppm)
                .map_err(|e| e.to_string())?;
        }
        eprintln!(
            "repaint changed {count} pixels ({}x{})",
            rect.right, rect.bottom
        );
    }
    Ok(count == 0)
}
struct WindowSearch {
    pid: u32,
    window: HWND,
}
struct DesktopContext {
    dpi: DPI_AWARENESS_CONTEXT,
    cursor: windows_sys::Win32::Foundation::POINT,
}
impl Drop for DesktopContext {
    fn drop(&mut self) {
        unsafe {
            SetCursorPos(self.cursor.x, self.cursor.y);
            SetThreadDpiAwarenessContext(self.dpi);
        }
    }
}
unsafe extern "system" fn find_main(hwnd: HWND, l: LPARAM) -> i32 {
    let search = &mut *(l as *mut WindowSearch);
    let mut pid = 0;
    GetWindowThreadProcessId(hwnd, &mut pid);
    let mut class = [0u16; 64];
    let n = GetClassNameW(hwnd, class.as_mut_ptr(), 64);
    if pid == search.pid && String::from_utf16_lossy(&class[..n as usize]) == "SGRustMain" {
        search.window = hwnd;
        return 0;
    }
    1
}
fn main() {
    if let Err(e) = run() {
        eprintln!("FAIL {e}");
        std::process::exit(1);
    }
}
fn wait(mut predicate: impl FnMut() -> bool) -> Result<()> {
    let until = Instant::now() + Duration::from_secs(15);
    while Instant::now() < until {
        if predicate() {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(50));
    }
    Err("等待界面/会话超时".into())
}
fn check(ok: bool, name: &str) -> Result<()> {
    if !ok {
        return Err(name.into());
    }
    println!("PASS {name}");
    Ok(())
}
unsafe fn clipboard_text(owner: HWND) -> Result<String> {
    if OpenClipboard(owner) == 0 {
        return Err("无法打开测试剪贴板".into());
    }
    let handle = GetClipboardData(13);
    let memory = GlobalLock(handle);
    let text = if memory.is_null() {
        Err("剪贴板没有 Unicode 文本".into())
    } else {
        let ptr = memory.cast::<u16>();
        let max = GlobalSize(handle) / 2;
        let mut len = 0;
        while len < max && *ptr.add(len) != 0 {
            len += 1;
        }
        let value = String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len));
        GlobalUnlock(handle);
        Ok(value)
    };
    CloseClipboard();
    text
}
#[path = "../../tests/support/oneshot.rs"]
mod oneshot_support;
#[path = "../../tests/support/tls.rs"]
mod tls_support;
fn run() -> Result<()> {
    let args = std::env::args().collect::<Vec<_>>();
    let mode = args.get(1).ok_or("mode")?;
    let root = PathBuf::from(args.get(2).ok_or("root")?);
    let store = Store::new(root)?;
    if mode == "oneshot-console" {
        return oneshot_support::verify(
            &store.root,
            std::path::Path::new(&args[3]),
            std::path::Path::new(&args[4]),
        );
    }
    if mode == "oneshot-python-console" {
        return oneshot_support::verify_python_console(
            &store.root,
            std::path::Path::new(&args[3]),
            std::path::Path::new(&args[4]),
        );
    }
    if mode == "oneshot-cli-output" {
        return oneshot_support::verify_cli_output(
            &store.root,
            std::path::Path::new(&args[3]),
            std::path::Path::new(&args[4]),
            &args[5],
            &args[6],
        );
    }
    if mode == "untrust-test-ca" {
        tls_support::untrust(&store.root)?;
        return Ok(());
    }
    if mode == "prepare" || mode == "prepare-tls" {
        let count: usize = args[3].parse().unwrap();
        let fixture = std::path::absolute(&args[4]).unwrap();
        let services = (0..count)
            .map(|i| Profile {
                name: format!("性能测试 {}", i + 1),
                command: fixture.to_string_lossy().into(),
                output: "ws".into(),
                route: "/ws".into(),
                port: 18000 + i as u16,
                tls: mode == "prepare-tls",
                listen_address: Some("127.0.0.1".into()),
                auto_start: true,
                ..Default::default()
            })
            .collect();
        store.save(&Settings {
            services,
            ..Default::default()
        })?;
        if mode == "prepare-tls" && count > 0 {
            gateway::tls::prepare(&store.root)?;
            tls_support::trust(&store.root)?;
        }
        return Ok(());
    }
    if mode == "hold" {
        let mut sessions = vec![];
        for p in store.load()?.services {
            wait(|| gateway::net::profile_health(&p).is_ok())?;
            let mut session = Session::connect(&p, Duration::from_secs(900))?;
            session.tools()?;
            sessions.push(session);
        }
        std::fs::write(store.root.join("connected"), "ready").unwrap();
        let until = Instant::now() + Duration::from_secs(600);
        while !store.root.join("release").exists() && Instant::now() < until {
            thread::sleep(Duration::from_millis(100));
        }
        return Ok(());
    }
    if mode == "compat" {
        let original = std::fs::read(store.root.join("settings.dat")).unwrap();
        let settings = store.load()?;
        check(
            settings.services[0].name == "旧版中文"
                && settings.services[0].bearer == "旧令牌"
                && settings.services[0].env[0].value == "旧秘密"
                && settings.services[0].listen_host() == "127.0.0.1",
            "C# SettingsStore DPAPI/PascalCase compatibility",
        )?;
        store.save(&settings)?;
        check(
            std::fs::read(store.root.join("settings.dat.pre-rust.bak")).unwrap() == original,
            "old encrypted settings backed up verbatim",
        )?;
        std::fs::write(store.root.join("settings.dat"), "broken").unwrap();
        check(
            store.load().is_err()
                && std::fs::read(store.root.join("settings.dat")).unwrap() == b"broken",
            "corrupt settings rejected without overwrite",
        )?;
        return Ok(());
    }
    if mode == "dependency-ui" {
        if !store
            .root
            .canonicalize()
            .map_err(|e| e.to_string())?
            .starts_with(
                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join(".build")
                    .canonicalize()
                    .map_err(|e| e.to_string())?,
            )
        {
            return Err("依赖 UI 验收只允许工作区 .build 下的隔离目录".into());
        }
        let mut child = Command::new(&args[3])
            .args(["--data-dir", store.root.to_str().unwrap()])
            .env("PATH", "")
            .spawn()
            .map_err(|e| e.to_string())?;
        let result = unsafe { dependency_acceptance(&store, child.id()) };
        if result.is_err() {
            let _ = child.kill();
        }
        let _ = child.wait();
        return result;
    }
    if mode != "ui" {
        return Err("unknown mode".into());
    }
    unsafe {
        let mut large = null_mut();
        let mut small = null_mut();
        let count = ExtractIconExW(wide(&args[3]).as_ptr(), 0, &mut large, &mut small, 1);
        let valid = count > 0 && !large.is_null() && !small.is_null();
        if !large.is_null() {
            DestroyIcon(large);
        }
        if !small.is_null() {
            DestroyIcon(small);
        }
        check(valid, "EXE contains extractable application icon")?;
        let module = LoadLibraryExW(
            wide(&args[3]).as_ptr(),
            null_mut(),
            LOAD_LIBRARY_AS_DATAFILE | LOAD_LIBRARY_AS_IMAGE_RESOURCE,
        );
        if module.is_null() {
            return Err("无法加载 EXE 图标资源".into());
        }
        let scaled = [96, 120, 144, 168, 192, 240].into_iter().all(|dpi| {
            [SM_CXICON, SM_CXSMICON].into_iter().all(|metric| {
                let size = GetSystemMetricsForDpi(metric, dpi);
                let icon = LoadImageW(
                    module,
                    101 as *const u16,
                    IMAGE_ICON,
                    size,
                    size,
                    LR_DEFAULTCOLOR,
                );
                let valid = icon_size(icon) == Some((size, size));
                if !icon.is_null() {
                    DestroyIcon(icon);
                }
                valid
            })
        });
        FreeLibrary(module);
        check(
            scaled,
            "embedded icon loads at 100/125/150/175/200/250 percent DPI",
        )?;
    }
    let mut child = Command::new(&args[3])
        .args(["--data-dir", store.root.to_str().unwrap()])
        .env("PATH", "")
        .env("DOTNET_ROOT", "X:\\not-installed")
        .arg("--background")
        .spawn()
        .map_err(|e| e.to_string())?;
    let result = unsafe { ui(&store, child.id()) };
    if result.is_err() {
        let _ = child.kill();
    }
    let until = Instant::now() + Duration::from_secs(15);
    while Instant::now() < until {
        if child.try_wait().unwrap().is_some() {
            return result;
        }
        thread::sleep(Duration::from_millis(50));
    }
    let _ = child.kill();
    Err("GUI 退出超时".into())
}

struct OwnedDialog {
    pid: u32,
    title: String,
    window: HWND,
}
unsafe extern "system" fn find_dialog(hwnd: HWND, l: LPARAM) -> i32 {
    let data = &mut *(l as *mut OwnedDialog);
    let mut pid = 0;
    GetWindowThreadProcessId(hwnd, &mut pid);
    if pid == data.pid {
        let mut title = [0u16; 256];
        let count = GetWindowTextW(hwnd, title.as_mut_ptr(), title.len() as i32);
        if String::from_utf16_lossy(&title[..count.max(0) as usize]) == data.title {
            data.window = hwnd;
            return 0;
        }
    }
    1
}
unsafe fn owned_dialog(pid: u32, title: &str) -> HWND {
    let mut data = OwnedDialog {
        pid,
        title: title.into(),
        window: null_mut(),
    };
    EnumWindows(Some(find_dialog), (&mut data as *mut OwnedDialog) as isize);
    data.window
}
unsafe fn ready_dialog(pid: u32, owner: HWND, title: &str) -> Result<HWND> {
    let mut window = null_mut();
    wait(|| {
        window = owned_dialog(pid, title);
        !window.is_null() && IsWindowVisible(window) != 0 && IsWindowEnabled(owner) == 0
    })?;
    Ok(window)
}
unsafe fn dependency_acceptance(store: &Store, pid: u32) -> Result<()> {
    let profiles = store.load()?.services;
    if profiles.len() != 2
        || profiles[0].dependency.package != "@modelcontextprotocol/server-filesystem"
        || profiles[1].dependency.package != "mcp-server-fetch"
        || profiles
            .iter()
            .any(|p| !p.managed || p.auto_start || p.source != "local")
    {
        return Err("请先运行隔离的真实依赖网络验收".into());
    }
    let mut h = null_mut();
    wait(|| {
        let mut search = WindowSearch {
            pid,
            window: null_mut(),
        };
        EnumWindows(Some(find_main), (&mut search as *mut WindowSearch) as isize);
        h = search.window;
        !h.is_null()
    })?;
    let action = |id| {
        PostMessageW(h, WM_COMMAND, id, 0);
        thread::sleep(Duration::from_millis(200));
    };
    for (index, p) in profiles.iter().enumerate() {
        SendMessageW(GetDlgItem(h, 20), LB_SETCURSEL, index, 0);
        PostMessageW(h, WM_COMMAND, 20 | ((LBN_SELCHANGE as usize) << 16), 0);
        thread::sleep(Duration::from_millis(100));
        action(31);
        wait(|| gateway::net::profile_health(p).is_ok())?;
        wait(|| IsWindowEnabled(GetDlgItem(h, 31)) != 0)?;
        check(
            !gateway::net::test(p)?.is_empty(),
            &format!(
                "managed {} starts from final GUI with empty PATH",
                p.dependency.kind
            ),
        )?;
    }
    SendMessageW(GetDlgItem(h, 20), LB_SETCURSEL, 0, 0);
    PostMessageW(h, WM_COMMAND, 20 | ((LBN_SELCHANGE as usize) << 16), 0);
    thread::sleep(Duration::from_millis(100));
    action(145);
    let mut dialog = null_mut();
    wait(|| {
        dialog = owned_dialog(pid, "运行环境与 MCP 依赖 · 不修改系统环境");
        !dialog.is_null() && IsWindowVisible(dialog) != 0
    })?;
    check(
        IsWindowEnabled(GetDlgItem(dialog, 631)) != 0,
        "installed service exposes one-shot terminal action",
    )?;
    PostMessageW(dialog, WM_COMMAND, 631, 0);
    let mut arguments = null_mut();
    wait(|| {
        arguments = owned_dialog(pid, "一次性命令参数");
        !arguments.is_null() && IsWindowVisible(arguments) != 0
    })?;
    SendMessageW(
        GetDlgItem(arguments, 500),
        WM_SETTEXT,
        0,
        wide("setup\r\n中文 参数").as_ptr() as isize,
    );
    PostMessageW(arguments, WM_COMMAND, 1, 0);
    let mut one_preview = null_mut();
    wait(|| {
        one_preview = owned_dialog(pid, "一次性命令预览");
        !one_preview.is_null() && IsWindowVisible(one_preview) != 0
    })?;
    let mut preview_text = vec![0u16; 32768];
    GetWindowTextW(
        GetDlgItem(one_preview, 500),
        preview_text.as_mut_ptr(),
        preview_text.len() as i32,
    );
    let preview_text = String::from_utf16_lossy(&preview_text);
    check(
        preview_text.contains("setup")
            && preview_text.contains("中文 参数")
            && preview_text.contains("node.exe"),
        "one-shot preview shows committed executable and separate setup arguments",
    )?;
    PostMessageW(one_preview, WM_COMMAND, 2, 0);
    wait(|| IsWindow(one_preview) == 0)?;
    check(
        store.load()?.services[0].dependency.args == profiles[0].dependency.args,
        "cancelled one-shot leaves service arguments unchanged",
    )?;
    PostMessageW(dialog, WM_COMMAND, 626, 0);
    let mut confirm = null_mut();
    wait(|| {
        confirm = owned_dialog(pid, "确认依赖操作");
        !confirm.is_null()
    })?;
    let mut confirmation_pid = 0;
    GetWindowThreadProcessId(confirm, &mut confirmation_pid);
    check(
        confirmation_pid == pid,
        "installation confirmation belongs to isolated GUI process",
    )?;
    let installations = gateway::storage::dependency_directory(&store.root)?
        .join("services")
        .join(&profiles[0].id);
    let before = std::fs::read_dir(&installations)
        .map_err(|e| e.to_string())?
        .flatten()
        .map(|entry| entry.file_name())
        .collect::<std::collections::HashSet<_>>();
    PostMessageW(confirm, WM_COMMAND, IDYES as usize, 0);
    let until = Instant::now() + Duration::from_secs(180);
    let mut complete = false;
    while Instant::now() < until {
        let error = owned_dialog(pid, "运行环境与依赖");
        if !error.is_null() {
            let mut body = [0u16; 4096];
            let count = GetWindowTextW(
                GetDlgItem(error, 65535),
                body.as_mut_ptr(),
                body.len() as i32,
            );
            return Err(format!(
                "实际 UI 安装失败：{}",
                String::from_utf16_lossy(&body[..count.max(0) as usize])
            ));
        }
        let committed = std::fs::read_dir(&installations).is_ok_and(|entries| {
            entries.flatten().any(|entry| {
                !before.contains(&entry.file_name())
                    && entry.path().join("installed.json").is_file()
            })
        });
        if committed && IsWindowEnabled(GetDlgItem(dialog, 626)) != 0 {
            complete = true;
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
    check(
        complete,
        "GUI completes actual package update without restarting running services",
    )?;
    let mut version = [0u16; 128];
    SendMessageW(
        GetDlgItem(dialog, 622),
        WM_GETTEXT,
        version.len(),
        version.as_mut_ptr() as isize,
    );
    check(
        String::from_utf16_lossy(&version).trim_end_matches('\0') == profiles[0].dependency.version,
        "GUI fills installed exact version into version field",
    )?;
    PostMessageW(dialog, WM_COMMAND, 1, 0);
    wait(|| IsWindow(dialog) == 0)?;
    thread::sleep(Duration::from_millis(300));
    action(30);
    wait(|| {
        store
            .load()
            .is_ok_and(|s| s.services[0].dependency.installed != profiles[0].dependency.installed)
    })?;
    let updated = store.load()?.services[0].clone();
    check(
        gateway::dependencies::receipt(&store.root, &updated).is_ok(),
        "saved GUI installation references committed receipt",
    )?;
    let mut status = [0u16; 256];
    GetWindowTextW(GetDlgItem(h, 4), status.as_mut_ptr(), status.len() as i32);
    check(
        String::from_utf16_lossy(&status).contains("待应用"),
        "running service marks installed update as pending",
    )?;
    check(
        !gateway::net::test(&profiles[0])?.is_empty()
            && !gateway::net::test(&profiles[1])?.is_empty(),
        "both MCP services remain usable before applying update",
    )?;
    action(33);
    wait(|| IsWindowEnabled(GetDlgItem(h, 31)) != 0)?;
    check(
        !gateway::net::test(&updated)?.is_empty() && !gateway::net::test(&profiles[1])?.is_empty(),
        "apply/restart activates managed update and preserves second service",
    )?;
    action(15);
    wait(|| {
        profiles
            .iter()
            .all(|p| gateway::net::profile_health(p).is_err())
    })?;
    action(17);
    wait(|| IsWindow(h) == 0)?;
    check(
        true,
        "actual dependency GUI exits and releases managed service ports",
    )?;
    Ok(())
}
unsafe fn ui(store: &Store, pid: u32) -> Result<()> {
    let mut context = DesktopContext {
        dpi: SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2),
        cursor: std::mem::zeroed(),
    };
    GetCursorPos(&mut context.cursor);
    let mut h = null_mut();
    wait(|| {
        let mut search = WindowSearch {
            pid,
            window: null_mut(),
        };
        EnumWindows(Some(find_main), (&mut search as *mut WindowSearch) as isize);
        h = search.window;
        !h.is_null()
    })?;
    let dpi = GetDpiForWindow(h);
    wait(|| !GetDlgItem(GetDlgItem(h, 22), 100).is_null())?;
    thread::sleep(Duration::from_millis(150));
    check(
        IsWindowVisible(h) == 0,
        "background launch starts hidden with usable tray",
    )?;
    let activate = RegisterWindowMessageW(wide("SuperGatewayGUI.Restore.v1").as_ptr());
    SendMessageW(h, activate, 0, 0);
    wait(|| IsWindowVisible(h) != 0)?;
    check(
        true,
        "registered activation restores actual background launch",
    )?;
    let large = SendMessageW(h, WM_GETICON, ICON_BIG as usize, 0) as HICON;
    let small = SendMessageW(h, WM_GETICON, ICON_SMALL as usize, 0) as HICON;
    check(
        icon_size(large)
            == Some((
                GetSystemMetricsForDpi(SM_CXICON, dpi),
                GetSystemMetricsForDpi(SM_CYICON, dpi),
            ))
            && icon_size(small)
                == Some((
                    GetSystemMetricsForDpi(SM_CXSMICON, dpi),
                    GetSystemMetricsForDpi(SM_CYSMICON, dpi),
                )),
        "main window icons match current DPI",
    )?;
    let panel = GetDlgItem(h, 22);
    let control = |id| GetDlgItem(panel, id);
    let action = |id| {
        PostMessageW(h, WM_COMMAND, id, 0);
        thread::sleep(Duration::from_millis(200));
    };
    action(10);
    check(IsWindowEnabled(control(100)) != 0, "native form enabled")?;
    let mut hint = [0u16; 512];
    SendMessageW(
        control(115),
        WM_GETTEXT,
        hint.len(),
        hint.as_mut_ptr() as isize,
    );
    check(
        String::from_utf16_lossy(&hint).contains("启动时选定："),
        "form displays interpreter selected at startup",
    )?;
    check(
        SendMessageW(control(110), CB_GETCURSEL, 0, 0) == 0
            && SendMessageW(control(110), CB_GETCOUNT, 0, 0) >= 2,
        "listen address defaults to all adapters",
    )?;
    let local_ips = gateway::network::local_addresses()?;
    for (i, address) in local_ips.iter().enumerate() {
        let mut label = [0u16; 2048];
        SendMessageW(
            control(110),
            CB_GETLBTEXT,
            i + 2,
            label.as_mut_ptr() as isize,
        );
        check(
            String::from_utf16_lossy(&label).starts_with(&address.ip.to_string()),
            "IPv4 adapter appears in address selector",
        )?;
    }
    SendMessageW(control(110), CB_SETCURSEL, 1, 0);
    action(117);
    check(
        SendMessageW(control(110), CB_GETCURSEL, 0, 0) == 1,
        "refresh preserves selected listening address",
    )?;
    SendMessageW(
        control(100),
        WM_SETTEXT,
        0,
        wide("中文界面验收").as_ptr() as isize,
    );
    SendMessageW(control(101), CB_SETCURSEL, 1, 0);
    PostMessageW(h, WM_COMMAND, 101 | ((CBN_SELCHANGE as usize) << 16), 0);
    SendMessageW(
        control(106),
        WM_SETTEXT,
        0,
        wide("https://example.com/mcp").as_ptr() as isize,
    );
    action(30);
    check(
        store.load()?.services[0].name == "中文界面验收"
            && store.load()?.services[0].listen_host() == "127.0.0.1",
        "form edits saved with empty PATH",
    )?;
    action(38);
    let copied = clipboard_text(h)?;
    check(
        copied.contains("[mcp_servers.")
            && copied.contains("url = \"http://127.0.0.1:")
            && !copied.contains("https://example.com"),
        "Codex button copies saved gateway HTTP TOML to clipboard",
    )?;
    SendMessageW(control(108), CB_SETCURSEL, 1, 0);
    PostMessageW(h, WM_COMMAND, 108 | ((CBN_SELCHANGE as usize) << 16), 0);
    thread::sleep(Duration::from_millis(150));
    let mut text = [0u16; 32];
    SendMessageW(control(120), WM_GETTEXT, 32, text.as_mut_ptr() as isize);
    check(
        String::from_utf16_lossy(&text).starts_with("/sse"),
        "protocol changes default route",
    )?;
    let protocol = control(108);
    for delta in [120i16, -120i16] {
        SendMessageW(
            protocol,
            WM_MOUSEWHEEL,
            ((delta as u16 as u32) << 16) as usize,
            0,
        );
    }
    check(
        SendMessageW(protocol, CB_GETCURSEL, 0, 0) == 1,
        "closed combo wheel cannot change protocol",
    )?;
    SendMessageW(protocol, CB_SHOWDROPDOWN, 1, 0);
    let mut combo_info: COMBOBOXINFO = std::mem::zeroed();
    combo_info.cbSize = std::mem::size_of::<COMBOBOXINFO>() as u32;
    GetComboBoxInfo(protocol, &mut combo_info);
    SendMessageW(
        combo_info.hwndList,
        WM_MOUSEWHEEL,
        (120u32 << 16) as usize,
        0,
    );
    SendMessageW(
        protocol,
        WM_MOUSEWHEEL,
        ((-120i16 as u16 as u32) << 16) as usize,
        0,
    );
    SendMessageW(protocol, CB_SHOWDROPDOWN, 0, 0);
    check(
        SendMessageW(protocol, CB_GETCURSEL, 0, 0) == 1,
        "open combo and popup list wheel cannot change protocol",
    )?;
    action(30);
    action(131);
    let modal = ready_dialog(pid, h, "编辑键值 · 默认隐藏内容")?;
    check(!modal.is_null(), "secret editor opens")?;
    check(
        icon_size(SendMessageW(modal, WM_GETICON, ICON_SMALL as usize, 0) as HICON).is_some(),
        "configuration dialog uses embedded icon",
    )?;
    SendMessageW(
        GetDlgItem(modal, 500),
        WM_SETTEXT,
        0,
        wide("TOKEN").as_ptr() as isize,
    );
    SendMessageW(
        GetDlgItem(modal, 501),
        WM_SETTEXT,
        0,
        wide("sensitive-value").as_ptr() as isize,
    );
    check(
        SendMessageW(GetDlgItem(modal, 501), EM_GETPASSWORDCHAR, 0, 0) != 0,
        "secret masked by default",
    )?;
    PostMessageW(modal, WM_COMMAND, 1, 0);
    thread::sleep(Duration::from_millis(150));
    action(30);
    check(
        store.load()?.services[0].env[0].value == "sensitive-value",
        "secret list edit persisted",
    )?;
    SendMessageW(
        control(100),
        WM_SETTEXT,
        0,
        wide("未保存").as_ptr() as isize,
    );
    action(10);
    let prompt = ready_dialog(pid, h, "SuperGateway")?;
    check(!prompt.is_null(), "unsaved changes prompt")?;
    PostMessageW(prompt, WM_COMMAND, IDCANCEL as usize, 0);
    thread::sleep(Duration::from_millis(150));
    check(
        store.load()?.services.len() == 1,
        "cancel preserves selection/config",
    )?;
    SendMessageW(
        control(100),
        WM_SETTEXT,
        0,
        wide("中文界面验收").as_ptr() as isize,
    );
    let tab = GetDlgItem(h, 21);
    SendMessageW(tab, WM_KEYDOWN, 0x27, 0);
    SendMessageW(tab, WM_KEYUP, 0x27, 0);
    thread::sleep(Duration::from_millis(100));
    check(
        IsWindowVisible(control(122)) != 0,
        "advanced tab switches native controls",
    )?;
    SetForegroundWindow(h);
    SendMessageW(
        control(122),
        WM_SETTEXT,
        0,
        wide("repaint-secret").as_ptr() as isize,
    );
    let mut original: windows_sys::Win32::Foundation::RECT = std::mem::zeroed();
    GetWindowRect(h, &mut original);
    // A pointer left over a moving combo box starts a themed hover animation;
    // keep it outside the client area while comparing displayed pixels.
    SetCursorPos(original.left + 30, original.top + 12);
    let scale = |value| value * dpi as i32 / 96;
    for (width, height) in [(980, 690), (1180, 860), (1000, 710), (1100, 750)] {
        SetWindowPos(
            h,
            null_mut(),
            0,
            0,
            scale(width),
            scale(height),
            SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
        check(
            repaint_is_clean(panel)?,
            "resize leaves no stale form pixels",
        )?;
        SendMessageW(panel, WM_VSCROLL, SB_PAGEDOWN as usize, 0);
        check(
            repaint_is_clean(panel)?,
            "page scroll leaves no stale form pixels",
        )?;
        SendMessageW(panel, WM_MOUSEWHEEL, (120u32 << 16) as usize, 0);
        check(
            repaint_is_clean(panel)?,
            "wheel scroll leaves no stale form pixels",
        )?;
        SendMessageW(panel, WM_VSCROLL, SB_PAGEUP as usize, 0);
    }
    SetWindowPos(
        h,
        null_mut(),
        0,
        0,
        original.right - original.left,
        original.bottom - original.top,
        SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
    );
    SendMessageW(control(142), BM_SETCHECK, BST_CHECKED as usize, 0);
    action(30);
    check(
        store.load()?.services[0].tls,
        "HTTPS checkbox persists in service settings",
    )?;
    action(35);
    check(
        clipboard_text(h)?.starts_with("https://127.0.0.1:"),
        "copied service address uses HTTPS",
    )?;
    SendMessageW(control(142), BM_SETCHECK, BST_UNCHECKED as usize, 0);
    action(30);
    action(16);
    let settings_window = ready_dialog(pid, h, "应用设置")?;
    check(!settings_window.is_null(), "application settings opens")?;
    check(
        [507, 508, 509, 512, 513]
            .iter()
            .all(|id| !GetDlgItem(settings_window, *id).is_null()),
        "settings offers certificate generation export and trust management",
    )?;
    check(
        GetWindowLongPtrW(GetDlgItem(settings_window, 514), GWL_STYLE) & ES_READONLY as isize != 0,
        "certificate status is a read only native document",
    )?;
    let elevated = gateway::startup::is_elevated();
    check(
        (IsWindowEnabled(GetDlgItem(settings_window, 505)) != 0) == elevated,
        "administrator startup checkbox requires elevated token",
    )?;
    check(
        (IsWindowEnabled(GetDlgItem(settings_window, 506)) != 0) != elevated,
        "administrator restart button matches current privilege",
    )?;
    check(
        SendMessageW(GetDlgItem(settings_window, 505), BM_GETCHECK, 0, 0) == BST_UNCHECKED as isize,
        "administrator startup is off by default",
    )?;
    PostMessageW(settings_window, WM_COMMAND, 2, 0);
    wait(|| IsWindow(settings_window) == 0)?;
    check(
        !store.load()?.admin_login_startup && IsWindowEnabled(h) != 0,
        "cancel settings preserves startup configuration and restores owner",
    )?;
    check(
        SendMessageW(control(150), CB_GETCOUNT, 0, 0) == 2 && !control(145).is_null(),
        "local form offers managed runtime and dependency button",
    )?;
    SendMessageW(control(101), CB_SETCURSEL, 0, 0);
    PostMessageW(h, WM_COMMAND, 101 | ((CBN_SELCHANGE as usize) << 16), 0);
    SendMessageW(control(102), WM_SETTEXT, 0, wide("npx").as_ptr() as isize);
    SendMessageW(
        control(103),
        WM_SETTEXT,
        0,
        wide("-y\r\nfixture-only-package").as_ptr() as isize,
    );
    action(30);
    action(145);
    let deps_window = ready_dialog(pid, h, "运行环境与 MCP 依赖 · 不修改系统环境")?;
    check(
        !deps_window.is_null(),
        "service dependency editor opens with empty PATH",
    )?;
    check(
        SendMessageW(GetDlgItem(deps_window, 610), CB_GETCOUNT, 0, 0) == 3,
        "dependency editor offers three pinned runtimes",
    )?;
    check(
        SendMessageW(GetDlgItem(deps_window, 603), EM_GETPASSWORDCHAR, 0, 0) != 0,
        "dependency proxy password hidden by default",
    )?;
    let mut dependency_rect: windows_sys::Win32::Foundation::RECT = std::mem::zeroed();
    GetWindowRect(deps_window, &mut dependency_rect);
    SetWindowPos(
        deps_window,
        null_mut(),
        0,
        0,
        900,
        620,
        SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
    );
    check(
        repaint_is_clean(deps_window)?,
        "dependency dialog resizes without stale pixels",
    )?;
    SendMessageW(deps_window, WM_VSCROLL, SB_PAGEDOWN as usize, 0);
    check(
        repaint_is_clean(deps_window)?,
        "dependency dialog scrolls without stale pixels",
    )?;
    SetWindowPos(
        deps_window,
        null_mut(),
        0,
        0,
        dependency_rect.right - dependency_rect.left,
        dependency_rect.bottom - dependency_rect.top,
        SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
    );
    SendMessageW(
        GetDlgItem(deps_window, 621),
        WM_SETTEXT,
        0,
        wide("fixture-only-package").as_ptr() as isize,
    );
    SendMessageW(
        GetDlgItem(deps_window, 622),
        WM_SETTEXT,
        0,
        wide("1.0.0").as_ptr() as isize,
    );
    SendMessageW(
        GetDlgItem(deps_window, 625),
        BM_SETCHECK,
        BST_CHECKED as usize,
        0,
    );
    PostMessageW(deps_window, WM_COMMAND, 1, 0);
    thread::sleep(Duration::from_millis(200));
    let popup = GetLastActivePopup(deps_window);
    if popup != deps_window && IsWindowVisible(popup) != 0 {
        let mut title = [0u16; 1024];
        let mut body = [0u16; 2048];
        GetWindowTextW(popup, title.as_mut_ptr(), title.len() as i32);
        GetWindowTextW(
            GetDlgItem(popup, 65535),
            body.as_mut_ptr(),
            body.len() as i32,
        );
        return Err(format!(
            "保存依赖出现提示：{} / {}",
            String::from_utf16_lossy(&title).trim_end_matches('\0'),
            String::from_utf16_lossy(&body).trim_end_matches('\0')
        ));
    }
    if let Err(error) = wait(|| IsWindow(deps_window) == 0) {
        unsafe extern "system" fn dump(hwnd: HWND, l: LPARAM) -> i32 {
            let mut owner = 0;
            GetWindowThreadProcessId(hwnd, &mut owner);
            if owner == l as u32 {
                let mut title = [0u16; 2048];
                GetWindowTextW(hwnd, title.as_mut_ptr(), title.len() as i32);
                eprintln!(
                    "DIALOG: {}",
                    String::from_utf16_lossy(&title).trim_end_matches('\0')
                );
                if String::from_utf16_lossy(&title).contains("运行环境与依赖") {
                    EnumChildWindows(hwnd, Some(body), 0);
                }
            }
            1
        }
        unsafe extern "system" fn body(hwnd: HWND, _: LPARAM) -> i32 {
            let mut text = [0u16; 4096];
            GetWindowTextW(hwnd, text.as_mut_ptr(), text.len() as i32);
            eprintln!(
                "BODY: {}",
                String::from_utf16_lossy(&text).trim_end_matches('\0')
            );
            1
        }
        EnumWindows(Some(dump), pid as isize);
        return Err(error);
    }
    wait(|| SendMessageW(control(150), CB_GETCURSEL, 0, 0) == 1)?;
    thread::sleep(Duration::from_millis(100));
    action(30);
    check(
        store.load()?.services[0].managed
            && store.load()?.services[0].dependency.installed.is_empty(),
        "managed configuration saves without executing or installing",
    )?;
    check(
        IsWindowEnabled(control(102)) == 0 && IsWindowEnabled(control(103)) == 0,
        "managed mode disables preserved original command and arguments",
    )?;
    check(
        !store.root.join("dependencies/runtimes").exists(),
        "saving dependency form does not download runtimes",
    )?;
    SendMessageW(control(102), WM_SETTEXT, 0, wide("").as_ptr() as isize);
    action(145);
    let deps_window = ready_dialog(pid, h, "运行环境与 MCP 依赖 · 不修改系统环境")?;
    SendMessageW(GetDlgItem(deps_window, 620), CB_SETCURSEL, 1, 0);
    SendMessageW(
        GetDlgItem(deps_window, 621),
        WM_SETTEXT,
        0,
        wide("mcp-for-blender").as_ptr() as isize,
    );
    SendMessageW(
        GetDlgItem(deps_window, 622),
        WM_SETTEXT,
        0,
        wide("").as_ptr() as isize,
    );
    PostMessageW(deps_window, WM_COMMAND, 1, 0);
    wait(|| IsWindow(deps_window) == 0)?;
    action(30);
    let saved = store.load()?.services[0].clone();
    check(
        saved.command.is_empty()
            && saved.stdio.is_empty()
            && saved.managed
            && saved.dependency.package == "mcp-for-blender"
            && saved.dependency.args.is_empty(),
        "uninstalled Blender package saves with empty program and no setup arguments",
    )?;
    let mut text = [0u16; 2048];
    GetWindowTextW(GetDlgItem(h, 4), text.as_mut_ptr(), text.len() as i32);
    check(
        String::from_utf16_lossy(&text).contains("待安装"),
        "saved package-only service shows pending installation",
    )?;
    GetWindowTextW(control(151), text.as_mut_ptr(), text.len() as i32);
    let summary = String::from_utf16_lossy(&text);
    check(
        summary.contains("Python 包")
            && summary.contains("mcp-for-blender")
            && summary.contains("入口"),
        "managed summary identifies package type and automatic entry",
    )?;
    SendMessageW(
        control(105),
        WM_SETTEXT,
        0,
        wide("uvx mcp-for-blender").as_ptr() as isize,
    );
    // WM_SETTEXT on a multiline EDIT does not emit EN_CHANGE; simulate user editing.
    PostMessageW(h, WM_COMMAND, 105 | ((EN_CHANGE as usize) << 16), 0);
    thread::sleep(Duration::from_millis(100));
    GetWindowTextW(control(151), text.as_mut_ptr(), text.len() as i32);
    check(
        String::from_utf16_lossy(&text).contains("当前优先执行完整启动命令")
            && IsWindowEnabled(control(102)) != 0,
        "complete command priority is visible in managed mode",
    )?;
    SendMessageW(control(105), WM_SETTEXT, 0, wide("").as_ptr() as isize);
    PostMessageW(h, WM_COMMAND, 105 | ((EN_CHANGE as usize) << 16), 0);
    SendMessageW(tab, WM_KEYDOWN, 0x27, 0);
    SendMessageW(tab, WM_KEYUP, 0x27, 0);
    thread::sleep(Duration::from_millis(100));
    check(
        IsWindowVisible(control(152)) != 0
            && SendMessageW(control(152), BM_GETCHECK, 0, 0) == BST_CHECKED as isize,
        "local environment tab defaults to inheriting GUI startup environment",
    )?;
    check(
        !control(153).is_null()
            && !control(154).is_null()
            && !control(155).is_null()
            && !control(157).is_null(),
        "environment tab offers additional directory list, add/remove and preview",
    )?;
    SendMessageW(control(152), BM_SETCHECK, BST_UNCHECKED as usize, 0);
    let extra = store.root.join("Blender 中文 附加目录");
    std::fs::create_dir_all(&extra).map_err(|e| e.to_string())?;
    action(154);
    let mut folder = null_mut();
    wait(|| {
        folder = GetLastActivePopup(h);
        folder != h && IsWindowVisible(folder) != 0
    })?;
    // Shell messages above WM_USER do not marshal pointers across processes.
    let process = gateway::native::Handle(windows_sys::Win32::System::Threading::OpenProcess(
        windows_sys::Win32::System::Threading::PROCESS_VM_OPERATION
            | windows_sys::Win32::System::Threading::PROCESS_VM_WRITE,
        0,
        pid,
    ));
    let path = wide(&extra);
    let remote = VirtualAllocEx(
        process.0,
        null(),
        path.len() * 2,
        MEM_COMMIT | MEM_RESERVE,
        PAGE_READWRITE,
    );
    if remote.is_null() {
        return Err(gateway::native::error("分配目录选择测试内存"));
    }
    let wrote = windows_sys::Win32::System::Diagnostics::Debug::WriteProcessMemory(
        process.0,
        remote,
        path.as_ptr().cast(),
        path.len() * 2,
        null_mut(),
    );
    if wrote == 0 {
        VirtualFreeEx(process.0, remote, 0, MEM_RELEASE);
        return Err(gateway::native::error("写入目录选择测试路径"));
    }
    SendMessageW(folder, WM_USER + 103, 1, remote as isize);
    VirtualFreeEx(process.0, remote, 0, MEM_RELEASE);
    wait(|| IsWindowEnabled(GetDlgItem(folder, IDOK)) != 0)?;
    PostMessageW(folder, WM_COMMAND, IDOK as usize, 0);
    wait(|| IsWindow(folder) == 0)?;
    action(30);
    check(
        store.load()?.services[0].additional_paths == [extra.to_string_lossy().into_owned()]
            && !store.load()?.services[0].inherit_environment,
        "directory picker and inheritance switch save independently for service",
    )?;
    action(157);
    let preview = ready_dialog(pid, h, "服务环境预览 · 值隐藏")?;
    check(
        !preview.is_null(),
        "service environment preview opens before dependencies are installed",
    )?;
    let body = GetDlgItem(preview, 500);
    let mut content = vec![0u16; GetWindowTextLengthW(body) as usize + 1];
    GetWindowTextW(body, content.as_mut_ptr(), content.len() as i32);
    let content = String::from_utf16_lossy(&content);
    check(
        content.contains("Windows 基础环境")
            && content.contains("最终 PATH")
            && content.contains("Blender 中文 附加目录")
            && !content.contains("sensitive-value"),
        "preview shows ordered directories and variable sources with secret values hidden",
    )?;
    check(
        !store.root.join("dependencies/runtimes").exists(),
        "environment preview never installs runtimes",
    )?;
    PostMessageW(preview, WM_COMMAND, 1, 0);
    wait(|| IsWindow(preview) == 0)?;
    SendMessageW(control(153), LB_SETCURSEL, 0, 0);
    action(155);
    action(30);
    check(
        store.load()?.services[0].additional_paths.is_empty(),
        "additional directory removal persists",
    )?;
    SendMessageW(control(152), BM_SETCHECK, BST_CHECKED as usize, 0);
    SendMessageW(control(102), WM_SETTEXT, 0, wide("npx").as_ptr() as isize);
    SendMessageW(control(150), CB_SETCURSEL, 0, 0);
    PostMessageW(h, WM_COMMAND, 150 | ((CBN_SELCHANGE as usize) << 16), 0);
    action(30);
    check(
        !store.load()?.services[0].managed && !store.load()?.services[0].command.is_empty(),
        "switching back retains original launch command",
    )?;
    action(16);
    let settings_window = ready_dialog(pid, h, "应用设置")?;
    PostMessageW(settings_window, WM_COMMAND, 515, 0);
    wait(|| IsWindow(settings_window) == 0)?;
    let mut dependencies_window = null_mut();
    wait(|| {
        dependencies_window = owned_dialog(pid, "运行环境与 MCP 依赖 · 不修改系统环境");
        !dependencies_window.is_null()
            && IsWindowVisible(dependencies_window) != 0
            && IsWindowEnabled(h) == 0
    })?;
    check(
        IsWindowEnabled(GetDlgItem(dependencies_window, 626)) == 0,
        "global dependency manager requires a service for package installation",
    )?;
    SendMessageW(
        GetDlgItem(dependencies_window, 601),
        WM_SETTEXT,
        0,
        wide("http://127.0.0.1:10808").as_ptr() as isize,
    );
    SendMessageW(
        GetDlgItem(dependencies_window, 603),
        WM_SETTEXT,
        0,
        wide("ui-proxy-test-secret").as_ptr() as isize,
    );
    PostMessageW(dependencies_window, WM_COMMAND, 1, 0);
    wait(|| IsWindow(dependencies_window) == 0)?;
    // Closing the modal precedes the owner's atomic save; await persistence as well.
    wait(|| {
        store
            .load()
            .is_ok_and(|s| s.dependency_network.password == "ui-proxy-test-secret")
    })?;
    check(
        store.load()?.dependency_network.password == "ui-proxy-test-secret"
            && !String::from_utf8_lossy(&std::fs::read(store.root.join("settings.dat")).unwrap())
                .contains("ui-proxy-test-secret"),
        "dependency network settings restore from DPAPI without cleartext credentials",
    )?;
    action(18);
    let help = ready_dialog(pid, h, "SuperGateway · 离线帮助")?;
    check(
        !help.is_null() && IsWindowEnabled(h) == 0,
        "offline help opens without network and disables owner",
    )?;
    let topics = GetDlgItem(help, 504);
    let body = GetDlgItem(help, 500);
    check(
        SendMessageW(topics, LB_GETCOUNT, 0, 0) == 7,
        "help offers seven parameter and example chapters",
    )?;
    check(
        GetWindowLongPtrW(body, GWL_STYLE) & ES_READONLY as isize != 0,
        "help document is read only",
    )?;
    SendMessageW(topics, LB_SETCURSEL, 5, 0);
    SendMessageW(help, WM_COMMAND, 504 | ((LBN_SELCHANGE as usize) << 16), 0);
    let mut content = vec![0u16; SendMessageW(body, WM_GETTEXTLENGTH, 0, 0) as usize + 1];
    SendMessageW(
        body,
        WM_GETTEXT,
        content.len(),
        content.as_mut_ptr() as isize,
    );
    let content = String::from_utf16_lossy(&content);
    check(
        content.contains("[mcp_servers.example]") && content.contains("mcpServers"),
        "chapter selection shows Codex TOML and JSON examples",
    )?;
    check(
        icon_size(SendMessageW(help, WM_GETICON, ICON_SMALL as usize, 0) as HICON).is_some(),
        "help uses embedded application icon",
    )?;
    SetWindowPos(
        help,
        null_mut(),
        0,
        0,
        scale(720),
        scale(480),
        SWP_NOMOVE | SWP_NOZORDER,
    );
    let mut body_rect = std::mem::zeroed();
    let mut client_rect = std::mem::zeroed();
    GetWindowRect(body, &mut body_rect);
    GetClientRect(help, &mut client_rect);
    check(
        body_rect.right - body_rect.left < client_rect.right
            && body_rect.bottom - body_rect.top < client_rect.bottom,
        "resized help keeps body inside client area",
    )?;
    SendMessageW(body, WM_VSCROLL, SB_BOTTOM as usize, 0);
    check(
        SendMessageW(body, EM_GETFIRSTVISIBLELINE, 0, 0) > 0,
        "help text scrolls to examples",
    )?;
    PostMessageW(help, WM_CLOSE, 0, 0);
    wait(|| IsWindow(help) == 0)?;
    check(IsWindowEnabled(h) != 0, "closing help restores owner")?;
    PostMessageW(h, WM_CLOSE, 0, 0);
    thread::sleep(Duration::from_millis(150));
    check(IsWindowVisible(h) == 0, "close minimizes to tray")?;
    let executable = std::env::args().nth(3).ok_or("GUI executable missing")?;
    let run_second = |background: bool| -> Result<()> {
        let mut command = Command::new(&executable);
        command.arg("--data-dir").arg(&store.root).env("PATH", "");
        if background {
            command.arg("--background");
        }
        let mut second = command.spawn().map_err(|e| e.to_string())?;
        let mut status = None;
        let result = wait(|| {
            status = second.try_wait().ok().flatten();
            status.is_some()
        });
        if result.is_err() {
            let _ = second.kill();
            let _ = second.wait();
        }
        result?;
        if !status.unwrap().success() {
            return Err("Duplicate instance activation failed".into());
        }
        Ok(())
    };
    run_second(true)?;
    check(
        IsWindowVisible(h) == 0,
        "duplicate background startup leaves existing GUI hidden",
    )?;
    run_second(false)?;
    wait(|| IsWindowVisible(h) != 0)?;
    check(
        true,
        "reopening executable restores existing hidden GUI without second instance",
    )?;
    let mut tray: NOTIFYICONDATAW = std::mem::zeroed();
    tray.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    tray.hWnd = h;
    tray.uID = 1;
    check(
        Shell_NotifyIconW(NIM_DELETE, &tray) != 0,
        "remove only test instance tray icon",
    )?;
    let taskbar_created = RegisterWindowMessageW(wide("TaskbarCreated").as_ptr());
    SendMessageW(h, taskbar_created, 0, 0);
    check(
        Shell_NotifyIconW(NIM_MODIFY, &tray) != 0,
        "TaskbarCreated re-registers test tray icon",
    )?;
    PostMessageW(h, WM_CLOSE, 0, 0);
    wait(|| IsWindowVisible(h) == 0)?;
    PostMessageW(h, WM_APP + 1, 0, WM_LBUTTONDBLCLK as isize);
    thread::sleep(Duration::from_millis(150));
    check(IsWindowVisible(h) != 0, "tray double click restores window")?;
    action(17);
    wait(|| IsWindow(h) == 0)?;
    check(true, "tray/application exit completes")?;
    Ok(())
}
