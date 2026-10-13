#![allow(unsafe_op_in_unsafe_fn)]
// Win32 helpers keep explicit control geometry and styles together.
#![allow(clippy::too_many_arguments)]
use gateway::{
    Result, config,
    manager::{Event, Manager},
    model::{Pair, Profile, Settings},
    native::{self, wide},
    store::{Store, default_root},
};
use std::{
    collections::{HashMap, VecDeque},
    fs,
    mem::{size_of, zeroed},
    path::PathBuf,
    ptr::{null, null_mut},
    sync::{Arc, mpsc},
    thread,
};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::Gdi::*,
    System::{
        Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize},
        DataExchange::*,
        LibraryLoader::GetModuleHandleW,
        Memory::*,
        Threading::CreateMutexW,
    },
    UI::{Controls::*, HiDpi::*, Input::KeyboardAndMouse::*, Shell::*, WindowsAndMessaging::*},
};
#[path = "ui_dependencies.rs"]
mod dependency_ui;
const CF_UNICODETEXT: u32 = 13;
const ICON_RESOURCE: *const u16 = 101 as *const u16;

struct WindowIcons {
    large: HICON,
    small: HICON,
}
impl WindowIcons {
    unsafe fn load(dpi: u32) -> Result<Self> {
        let instance = GetModuleHandleW(null());
        let mut icons = Self {
            large: null_mut(),
            small: null_mut(),
        };
        icons.large = LoadImageW(
            instance,
            ICON_RESOURCE,
            IMAGE_ICON,
            GetSystemMetricsForDpi(SM_CXICON, dpi),
            GetSystemMetricsForDpi(SM_CYICON, dpi),
            LR_DEFAULTCOLOR,
        );
        icons.small = LoadImageW(
            instance,
            ICON_RESOURCE,
            IMAGE_ICON,
            GetSystemMetricsForDpi(SM_CXSMICON, dpi),
            GetSystemMetricsForDpi(SM_CYSMICON, dpi),
            LR_DEFAULTCOLOR,
        );
        if icons.large.is_null() || icons.small.is_null() {
            return Err(native::error("加载应用图标"));
        }
        Ok(icons)
    }
    unsafe fn apply(&self, hwnd: HWND) {
        SendMessageW(hwnd, WM_SETICON, ICON_BIG as usize, self.large as isize);
        SendMessageW(hwnd, WM_SETICON, ICON_SMALL as usize, self.small as isize);
    }
}
impl Drop for WindowIcons {
    fn drop(&mut self) {
        unsafe {
            if !self.large.is_null() {
                DestroyIcon(self.large);
            }
            if !self.small.is_null() {
                DestroyIcon(self.small);
            }
        }
    }
}

const WM_TRAY: u32 = WM_APP + 1;
const WM_SCROLL_FORM: u32 = WM_APP + 2;
const WM_EXIT_READY: u32 = WM_APP + 3;
const LIST: i32 = 20;
const TAB: i32 = 21;
const PANEL: i32 = 22;
const NAME: i32 = 100;
const SOURCE: i32 = 101;
const COMMAND: i32 = 102;
const ARGS: i32 = 103;
const CWD: i32 = 104;
const STDIO: i32 = 105;
const URL: i32 = 106;
const BEARER: i32 = 107;
const OUTPUT: i32 = 108;
const PORT: i32 = 109;
const LAN: i32 = 110;
const ENABLED: i32 = 111;
const AUTO: i32 = 112;
const ROUTE: i32 = 120;
const MESSAGE: i32 = 121;
const KEY: i32 = 122;
const CORS: i32 = 123;
const STATEFUL: i32 = 124;
const TIMEOUT: i32 = 125;
const LEVEL: i32 = 126;
const ENV: i32 = 130;
const HEADERS: i32 = 134;
const KEYS: i32 = 138;
const TLS: i32 = 142;
const MANAGED: i32 = 150;
const INHERIT_ENV: i32 = 152;
const PATHS: i32 = 153;
const CLIENT_STARTUP: i32 = 171;
const CLIENT_TOOL: i32 = 172;
struct Field {
    window: HWND,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    page: i32,
    source: i32,
}
struct App {
    hwnd: HWND,
    panel: HWND,
    font: HFONT,
    heading: HFONT,
    dpi: u32,
    icons: Option<WindowIcons>,
    controls: HashMap<i32, HWND>,
    fields: Vec<Field>,
    store: Store,
    settings: Settings,
    selected: Option<usize>,
    draft: Profile,
    saved: Profile,
    page: i32,
    scroll: i32,
    listen_addresses: Vec<String>,
    manager: Arc<Manager>,
    rx: mpsc::Receiver<Event>,
    states: HashMap<String, (String, String, bool)>,
    logs: VecDeque<String>,
    busy: bool,
    loading: bool,
    exiting: bool,
    activate_message: u32,
    taskbar_message: u32,
    instance_property: Vec<u16>,
    tray_ready: bool,
}
pub fn run() -> Result<()> {
    // Freeze the interpreter choice before creating the UI or starting services.
    // Missing shells must not prevent managing remote or standalone MCP services.
    let _ = gateway::shell::detect();
    let args = std::env::args().collect::<Vec<_>>();
    if let Some(pid) = args.windows(2).find(|v| v[0] == "--wait-for-pid") {
        gateway::startup::wait_for_previous(pid[1].parse().map_err(|_| "重启进程标识无效")?)?;
    }
    let explicit = args
        .windows(2)
        .find(|v| v[0] == "--data-dir")
        .map(|v| PathBuf::from(&v[1]));
    let root = gateway::storage::configuration_directory(&default_root(), explicit.as_deref())?;
    gateway::storage::dependency_directory(&root)?;
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    root.to_string_lossy()
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_uppercase()
        .hash(&mut hasher);
    gateway::startup::user_sid()?.hash(&mut hasher);
    let identity = format!("SuperGatewayGUI-Rust-{:x}", hasher.finish());
    let instance_property = wide(&identity);
    let activate_message =
        unsafe { RegisterWindowMessageW(wide("SuperGatewayGUI.Restore.v1").as_ptr()) };
    let taskbar_message = unsafe { RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()) };
    if activate_message == 0 || taskbar_message == 0 {
        return Err(native::error("注册窗口恢复消息"));
    }
    // Restore before opening the mutex: a normal token can be denied access
    // to a mutex created by the same user's elevated instance.
    if unsafe {
        activate_existing(
            &instance_property,
            activate_message,
            !args.iter().any(|v| v == "--background"),
        )?
    } {
        return Ok(());
    }
    let name = format!("Local\\{identity}");
    let mutex = unsafe { CreateMutexW(null(), 0, wide(name.replace(['/', ':'], "_")).as_ptr()) };
    if mutex.is_null() {
        return Err(native::error("创建应用互斥锁"));
    }
    let _mutex = native::Handle(mutex);
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        for _ in 0..40 {
            if unsafe {
                activate_existing(
                    &instance_property,
                    activate_message,
                    !args.iter().any(|v| v == "--background"),
                )?
            } {
                return Ok(());
            }
            thread::sleep(std::time::Duration::from_millis(50));
        }
        return Err("已有实例正在启动或无法响应，请稍后重新打开程序。不要重复启动服务。".into());
    }
    let store = Store::new(root.clone())?;
    let settings = store.load()?;
    let (tx, rx) = mpsc::sync_channel(2048);
    let engine = std::env::current_exe()
        .map_err(|e| e.to_string())?
        .parent()
        .unwrap()
        .join("engine/supergateway.exe");
    let manager = Manager::new(root, engine, tx);
    let instance = unsafe { GetModuleHandleW(null()) };
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        CoInitializeEx(null(), COINIT_APARTMENTTHREADED as u32);
        let init = INITCOMMONCONTROLSEX {
            dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_TAB_CLASSES | ICC_STANDARD_CLASSES,
        };
        InitCommonControlsEx(&init);
        let classes: [(&str, WNDPROC); 3] = [
            ("SGRustMain", Some(main_proc)),
            ("SGRustPanel", Some(panel_proc)),
            ("SGRustModal", Some(modal_proc)),
        ];
        for (name, proc) in classes {
            let class_name = wide(name);
            let class = WNDCLASSW {
                lpfnWndProc: proc,
                hInstance: instance,
                hCursor: LoadCursorW(null_mut(), IDC_ARROW),
                hIcon: LoadIconW(instance, ICON_RESOURCE),
                hbrBackground: GetSysColorBrush(COLOR_BTNFACE),
                lpszClassName: class_name.as_ptr(),
                ..zeroed()
            };
            if RegisterClassW(&class) == 0 {
                return Err(native::error("注册原生窗口"));
            }
        }
        let mut app = Box::new(App {
            hwnd: null_mut(),
            panel: null_mut(),
            font: null_mut(),
            heading: null_mut(),
            dpi: 96,
            icons: None,
            controls: HashMap::new(),
            fields: vec![],
            store,
            settings,
            selected: None,
            draft: Profile::default(),
            saved: Profile::default(),
            page: 0,
            scroll: 0,
            listen_addresses: vec![],
            manager,
            rx,
            states: HashMap::new(),
            logs: VecDeque::new(),
            busy: false,
            loading: false,
            exiting: false,
            activate_message,
            taskbar_message,
            instance_property,
            tray_ready: false,
        });
        let hwnd = CreateWindowExW(
            0,
            wide("SGRustMain").as_ptr(),
            wide("SuperGateway · Rust Win32 MCP 网关管理").as_ptr(),
            WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            1160 * GetDpiForSystem() as i32 / 96,
            860 * GetDpiForSystem() as i32 / 96,
            null_mut(),
            null_mut(),
            instance,
            (&mut *app as *mut App).cast(),
        );
        if hwnd.is_null() {
            return Err(native::error("创建主窗口"));
        }
        ShowWindow(
            hwnd,
            if args.iter().any(|v| v == "--background") && app.tray_ready {
                SW_HIDE
            } else {
                SW_SHOW
            },
        );
        UpdateWindow(hwnd);
        let mut message: MSG = zeroed();
        while GetMessageW(&mut message, null_mut(), 0, 0) > 0 {
            if IsDialogMessageW(hwnd, &message) == 0 {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        app.manager.stop_all();
        app.tray(NIM_DELETE);
        DeleteObject(app.font);
        DeleteObject(app.heading);
        CoUninitialize();
    }
    Ok(())
}
unsafe fn activate_existing(property: &[u16], message: u32, restore: bool) -> Result<bool> {
    let mut window = null_mut();
    loop {
        window = FindWindowExW(null_mut(), window, wide("SGRustMain").as_ptr(), null());
        if window.is_null() {
            return Ok(false);
        }
        if !GetPropW(window, property.as_ptr()).is_null() {
            if restore {
                let mut pid = 0;
                GetWindowThreadProcessId(window, &mut pid);
                AllowSetForegroundWindow(pid);
                let mut result = 0;
                if SendMessageTimeoutW(
                    window,
                    message,
                    0,
                    0,
                    SMTO_ABORTIFHUNG | SMTO_BLOCK,
                    2000,
                    &mut result,
                ) == 0
                {
                    return Err("已有后台实例无法响应窗口恢复，请稍后重试。".into());
                }
            }
            return Ok(true);
        }
    }
}
unsafe extern "system" fn main_proc(hwnd: HWND, message: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    if message == WM_NCCREATE {
        let create = &*(l as *const CREATESTRUCTW);
        let app = &mut *(create.lpCreateParams as *mut App);
        app.hwnd = hwnd;
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
        if SetPropW(hwnd, app.instance_property.as_ptr(), 1usize as HANDLE) == 0
            || ChangeWindowMessageFilterEx(hwnd, app.activate_message, MSGFLT_ALLOW, null_mut())
                == 0
            || ChangeWindowMessageFilterEx(hwnd, app.taskbar_message, MSGFLT_ALLOW, null_mut()) == 0
            || ChangeWindowMessageFilterEx(hwnd, WM_TRAY, MSGFLT_ALLOW, null_mut()) == 0
        {
            return 0;
        }
        return DefWindowProcW(hwnd, message, w, l);
    }
    let pointer = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut App;
    if !pointer.is_null() && message == (*pointer).activate_message {
        // No configuration or process mutation is exposed across integrity levels.
        let enabled = GetLastActivePopup(hwnd);
        ShowWindow(hwnd, SW_SHOW);
        ShowWindow(hwnd, SW_RESTORE);
        SetForegroundWindow(if IsWindowEnabled(hwnd) == 0 && !enabled.is_null() {
            enabled
        } else {
            hwnd
        });
        return 1;
    }
    let taskbar_recreated = !pointer.is_null() && message == (*pointer).taskbar_message;
    // Native APIs and modal dialogs dispatch messages synchronously. Handle
    // these messages without borrowing App, and suppress nested App dispatch.
    match message {
        WM_GETMINMAXINFO => {
            let dpi = GetDpiForWindow(hwnd).max(96) as i32;
            let info = &mut *(l as *mut MINMAXINFO);
            info.ptMinTrackSize.x = 960 * dpi / 96;
            info.ptMinTrackSize.y = 680 * dpi / 96;
            return 0;
        }
        WM_CLOSE => {
            let pointer = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut App;
            if !pointer.is_null() {
                (*pointer).tray_ready = (*pointer).tray(NIM_MODIFY) || (*pointer).tray(NIM_ADD);
                if (*pointer).tray_ready {
                    ShowWindow(hwnd, SW_HIDE);
                }
            }
            return 0;
        }
        WM_QUERYENDSESSION => return 1,
        WM_ENDSESSION => {
            if w != 0 {
                DestroyWindow(hwnd);
            }
            return 0;
        }
        WM_DESTROY => {
            KillTimer(hwnd, 1);
            PostQuitMessage(0);
            return 0;
        }
        WM_CREATE | WM_SIZE | WM_DPICHANGED | WM_COMMAND | WM_NOTIFY | WM_SCROLL_FORM
        | WM_TIMER | WM_TRAY | WM_EXIT_READY => {}
        _ if taskbar_recreated => {}
        _ => return DefWindowProcW(hwnd, message, w, l),
    }
    thread_local! {static DISPATCHING:std::cell::Cell<bool>=const {std::cell::Cell::new(false)};}
    if DISPATCHING.with(|busy| busy.replace(true)) {
        return 0;
    }
    struct DispatchGuard;
    impl Drop for DispatchGuard {
        fn drop(&mut self) {
            DISPATCHING.with(|busy| busy.set(false));
        }
    }
    let _guard = DispatchGuard;
    let data = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut App;
    if data.is_null() {
        return DefWindowProcW(hwnd, message, w, l);
    }
    let app = &mut *data;
    if taskbar_recreated {
        app.tray_ready = app.tray(NIM_ADD);
        if !app.tray_ready {
            app.restore();
        }
        return 0;
    }
    match message {
        WM_CREATE => {
            app.dpi = GetDpiForWindow(hwnd);
            app.update_icons();
            app.create();
            app.layout();
            app.tray_ready = app.tray(NIM_ADD);
            SetTimer(hwnd, 1, 100, None);
            app.refresh_list();
            if !app.settings.services.is_empty() {
                app.select(0);
            }
            let auto = app
                .settings
                .services
                .iter()
                .filter(|p| p.enabled && p.auto_start)
                .cloned()
                .collect::<Vec<_>>();
            if !auto.is_empty() {
                app.start_many(auto);
            }

            0
        }
        WM_SIZE => {
            app.layout();
            0
        }
        WM_GETMINMAXINFO => {
            let info = &mut *(l as *mut MINMAXINFO);
            info.ptMinTrackSize.x = app.scale(960);
            info.ptMinTrackSize.y = app.scale(680);
            0
        }
        WM_DPICHANGED => {
            app.dpi = (w & 0xffff) as u32;
            app.update_icons();
            app.tray(NIM_MODIFY);
            app.fonts();
            let r = &*(l as *const RECT);
            SetWindowPos(
                hwnd,
                null_mut(),
                r.left,
                r.top,
                r.right - r.left,
                r.bottom - r.top,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
            app.layout();
            0
        }
        WM_COMMAND => {
            let id = (w & 0xffff) as i32;
            let code = (w >> 16) as u32;
            if let Err(error) = app.command(id, code) {
                app.message(&error, MB_ICONWARNING);
            }
            0
        }
        WM_NOTIFY => {
            let header = &*(l as *const NMHDR);
            if header.idFrom == TAB as usize && header.code == TCN_SELCHANGE {
                app.page = SendMessageW(app.ctrl(TAB), TCM_GETCURSEL, 0, 0) as i32;
                app.scroll = 0;
                app.layout();
            }
            0
        }
        WM_SCROLL_FORM => {
            app.scroll = w as i32;
            app.layout();
            0
        }
        WM_TIMER => {
            app.events();
            let manager = app.manager.clone();
            manager.poll();
            0
        }
        WM_TRAY => {
            match l as u32 {
                WM_LBUTTONDBLCLK => app.restore(),
                WM_RBUTTONUP => {
                    let menu = CreatePopupMenu();
                    AppendMenuW(menu, MF_STRING, 200, wide("打开主窗口").as_ptr());
                    AppendMenuW(menu, MF_STRING, 201, wide("停止全部服务").as_ptr());
                    AppendMenuW(menu, MF_STRING, 202, wide("彻底退出").as_ptr());
                    let mut point: POINT = zeroed();
                    GetCursorPos(&mut point);
                    SetForegroundWindow(hwnd);
                    let selected = TrackPopupMenu(
                        menu,
                        TPM_RETURNCMD | TPM_RIGHTBUTTON,
                        point.x,
                        point.y,
                        0,
                        hwnd,
                        null(),
                    );
                    DestroyMenu(menu);
                    match selected {
                        200 => app.restore(),
                        201 => app.stop_many(app.settings.services.clone()),
                        202 => app.exit(),
                        _ => {}
                    }
                }
                _ => {}
            }
            0
        }
        WM_CLOSE => {
            ShowWindow(hwnd, SW_HIDE);
            0
        }
        WM_QUERYENDSESSION => 1,
        WM_ENDSESSION => {
            if w != 0 {
                DestroyWindow(hwnd);
            }
            0
        }
        WM_EXIT_READY => {
            DestroyWindow(hwnd);
            0
        }
        WM_DESTROY => {
            app.tray(NIM_DELETE);
            KillTimer(hwnd, 1);
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, message, w, l),
    }
}
unsafe extern "system" fn panel_proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    if msg == WM_COMMAND || msg == WM_NOTIFY {
        return SendMessageW(GetParent(hwnd), msg, w, l);
    }
    if msg == WM_VSCROLL || msg == WM_MOUSEWHEEL {
        let mut info = SCROLLINFO {
            cbSize: size_of::<SCROLLINFO>() as u32,
            fMask: SIF_ALL,
            ..zeroed()
        };
        GetScrollInfo(hwnd, SB_VERT, &mut info);
        let action = if msg == WM_MOUSEWHEEL {
            if (w >> 16) as i16 > 0 {
                SB_LINEUP
            } else {
                SB_LINEDOWN
            }
        } else {
            (w & 0xffff) as i32
        };
        let next = match action {
            SB_LINEUP => info.nPos - 32,
            SB_LINEDOWN => info.nPos + 32,
            SB_PAGEUP => info.nPos - info.nPage as i32,
            SB_PAGEDOWN => info.nPos + info.nPage as i32,
            SB_THUMBTRACK => info.nTrackPos,
            _ => info.nPos,
        };
        let next = next.clamp(0, (info.nMax - info.nPage as i32 + 1).max(0));
        SendMessageW(GetParent(hwnd), WM_SCROLL_FORM, next as usize, 0);
        return 0;
    }
    DefWindowProcW(hwnd, msg, w, l)
}
unsafe extern "system" fn combo_wheel_proc(
    hwnd: HWND,
    msg: u32,
    w: WPARAM,
    l: LPARAM,
    id: usize,
    data: usize,
) -> LRESULT {
    if msg == WM_MOUSEWHEEL || msg == WM_MOUSEHWHEEL {
        let combo = data as HWND;
        // Leave mouse clicks and keyboard selection intact. An open popup
        // consumes wheel input; a closed combo scrolls its containing form.
        if msg == WM_MOUSEWHEEL && SendMessageW(combo, CB_GETDROPPEDSTATE, 0, 0) == 0 {
            SendMessageW(GetParent(combo), msg, w, l);
        }
        return 0;
    }
    if msg == WM_NCDESTROY {
        RemoveWindowSubclass(hwnd, Some(combo_wheel_proc), id);
    }
    DefSubclassProc(hwnd, msg, w, l)
}
unsafe fn disable_combo_wheel(combo: HWND) {
    SetWindowSubclass(combo, Some(combo_wheel_proc), 1, combo as usize);
    let mut info = COMBOBOXINFO {
        cbSize: size_of::<COMBOBOXINFO>() as u32,
        ..zeroed()
    };
    if GetComboBoxInfo(combo, &mut info) != 0 && !info.hwndList.is_null() {
        SetWindowSubclass(info.hwndList, Some(combo_wheel_proc), 1, combo as usize);
    }
}
impl App {
    unsafe fn update_icons(&mut self) {
        match WindowIcons::load(self.dpi) {
            Ok(icons) => {
                icons.apply(self.hwnd);
                self.icons = Some(icons);
            }
            Err(error) => {
                self.message(&error, MB_ICONWARNING);
            }
        }
    }
    fn scale(&self, v: i32) -> i32 {
        v * self.dpi as i32 / 96
    }
    fn ctrl(&self, id: i32) -> HWND {
        *self.controls.get(&id).unwrap_or(&null_mut())
    }
    unsafe fn add(&mut self, parent: HWND, id: i32, class: &str, text: &str, style: u32) -> HWND {
        let window = CreateWindowExW(
            if class == "EDIT" || class == "LISTBOX" {
                WS_EX_CLIENTEDGE
            } else {
                0
            },
            wide(class).as_ptr(),
            wide(text).as_ptr(),
            WS_CHILD | WS_VISIBLE | WS_CLIPSIBLINGS | style,
            0,
            0,
            1,
            1,
            parent,
            id as usize as HMENU,
            GetModuleHandleW(null()),
            null(),
        );
        SendMessageW(window, WM_SETFONT, self.font as usize, 1);
        if class == "COMBOBOX" {
            disable_combo_wheel(window);
        }
        self.controls.insert(id, window);
        window
    }
    unsafe fn field(
        &mut self,
        id: i32,
        class: &str,
        text: &str,
        style: u32,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        page: i32,
        source: i32,
    ) {
        let window = self.add(self.panel, id, class, text, style);
        self.fields.push(Field {
            window,
            x,
            y,
            width,
            height,
            page,
            source,
        });
    }
    unsafe fn label_edit(
        &mut self,
        id: i32,
        label: &str,
        y: i32,
        height: i32,
        page: i32,
        source: i32,
        password: bool,
    ) {
        self.field(
            id + 1000,
            "STATIC",
            label,
            0,
            12,
            y + 6,
            126,
            24,
            page,
            source,
        );
        let style = if height > 32 {
            ES_MULTILINE as u32 | ES_AUTOVSCROLL as u32 | WS_VSCROLL
        } else {
            ES_AUTOHSCROLL as u32
        } | if password { ES_PASSWORD as u32 } else { 0 };
        self.field(
            id,
            "EDIT",
            "",
            style | WS_TABSTOP,
            144,
            y,
            -14,
            height,
            page,
            source,
        );
    }
    unsafe fn combo(&mut self, id: i32, label: &str, items: &[&str], y: i32, page: i32) {
        self.field(id + 1000, "STATIC", label, 0, 12, y + 6, 126, 24, page, 0);
        self.field(
            id,
            "COMBOBOX",
            "",
            CBS_DROPDOWNLIST as u32 | WS_TABSTOP | WS_VSCROLL,
            144,
            y,
            -14,
            230,
            page,
            0,
        );
        for item in items {
            SendMessageW(self.ctrl(id), CB_ADDSTRING, 0, wide(item).as_ptr() as isize);
        }
    }
    unsafe fn fonts(&mut self) {
        let old = self.font;
        let oldh = self.heading;
        self.font = CreateFontW(
            -self.scale(16),
            0,
            0,
            0,
            FW_NORMAL as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET as u32,
            0,
            0,
            CLEARTYPE_QUALITY as u32,
            0,
            wide("Microsoft YaHei UI").as_ptr(),
        );
        self.heading = CreateFontW(
            -self.scale(23),
            0,
            0,
            0,
            FW_BOLD as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET as u32,
            0,
            0,
            CLEARTYPE_QUALITY as u32,
            0,
            wide("Microsoft YaHei UI").as_ptr(),
        );
        for h in self.controls.values() {
            SendMessageW(*h, WM_SETFONT, self.font as usize, 1);
        }
        if !self.ctrl(2).is_null() {
            SendMessageW(self.ctrl(2), WM_SETFONT, self.heading as usize, 1);
        }
        if !old.is_null() {
            DeleteObject(old);
        }
        if !oldh.is_null() {
            DeleteObject(oldh);
        }
    }
    unsafe fn create(&mut self) {
        self.fonts();
        self.add(self.hwnd, 2, "STATIC", "SuperGateway", 0);
        SendMessageW(self.ctrl(2), WM_SETFONT, self.heading as usize, 1);
        self.add(self.hwnd, 3, "STATIC", "Rust 原生 Win32 · MCP 网关管理", 0);
        for (id, label) in [
            (10, "＋ 添加"),
            (11, "删除"),
            (12, "导入文件"),
            (13, "粘贴导入"),
            (14, "全部启动"),
            (15, "全部停止"),
            (16, "设置"),
            (17, "退出"),
            (18, "帮助"),
        ] {
            self.add(
                self.hwnd,
                id,
                "BUTTON",
                label,
                WS_TABSTOP | BS_PUSHBUTTON as u32,
            );
        }
        self.add(
            self.hwnd,
            LIST,
            "LISTBOX",
            "",
            LBS_NOTIFY as u32 | WS_VSCROLL | WS_TABSTOP,
        );
        self.add(self.hwnd, 4, "STATIC", "添加你的第一个 MCP 服务", 0);
        self.add(self.hwnd, 5, "STATIC", "支持本地程序、远程 HTTP 和 SSE", 0);
        self.add(self.hwnd, TAB, "SysTabControl32", "", WS_TABSTOP);
        for (i, text) in [
            "基础配置",
            "高级设置",
            "环境 / 请求头 / 密钥",
            "客户端配置 / 导出",
        ]
        .iter()
        .enumerate()
        {
            let name = wide(text);
            let item = TCITEMW {
                mask: TCIF_TEXT,
                pszText: name.as_ptr() as *mut u16,
                ..zeroed()
            };
            SendMessageW(
                self.ctrl(TAB),
                TCM_INSERTITEMW,
                i,
                (&item as *const TCITEMW) as isize,
            );
        }
        self.panel = self.add(
            self.hwnd,
            PANEL,
            "SGRustPanel",
            "",
            WS_VSCROLL | WS_CLIPCHILDREN,
        );
        SetWindowLongPtrW(self.panel, GWL_EXSTYLE, WS_EX_CONTROLPARENT as isize);
        self.label_edit(NAME, "服务名称", 8, 30, 0, 0, false);
        self.combo(
            SOURCE,
            "服务来源",
            &[
                "本地 MCP 程序（npx / uvx / exe）",
                "远程 Streamable HTTP（/mcp）",
                "远程 SSE（/sse）",
            ],
            48,
            0,
        );
        self.label_edit(COMMAND, "程序名或路径", 88, 30, 0, 1, false);
        self.field(113, "BUTTON", "浏览…", WS_TABSTOP, -92, 88, 78, 30, 0, 1);
        self.label_edit(ARGS, "参数（逐行）", 128, 60, 0, 1, false);
        self.label_edit(CWD, "工作目录", 198, 30, 0, 1, false);
        self.field(114, "BUTTON", "目录…", WS_TABSTOP, -92, 198, 78, 30, 0, 1);
        self.label_edit(STDIO, "完整启动命令", 238, 56, 0, 1, false);
        let shell_hint = format!(
            "填写后忽略上方程序和参数；启动时选定：{}。",
            gateway::shell::detect()
                .map(|s| s.kind.name())
                .unwrap_or("未检测到解释器")
        );
        self.field(115, "STATIC", &shell_hint, 0, 144, 298, -14, 24, 0, 1);
        let before = self.fields.len();
        self.combo(
            MANAGED,
            "启动方式",
            &["已有程序 / 完整命令", "应用管理的 npm / Python 包"],
            494,
            0,
        );
        for field in &mut self.fields[before..] {
            field.source = 1;
        }
        self.field(
            145,
            "BUTTON",
            "配置包与安装依赖…",
            WS_TABSTOP,
            144,
            538,
            260,
            32,
            0,
            1,
        );
        self.field(
            151,
            "STATIC",
            "未安装 MCP？选择应用管理，填写包名即可保存，无需程序路径或先运行 setup。",
            0,
            144,
            578,
            -14,
            90,
            0,
            1,
        );
        self.label_edit(URL, "远程 MCP URL", 88, 30, 0, 2, false);
        self.label_edit(BEARER, "Bearer Token", 128, 30, 0, 2, true);
        self.combo(
            OUTPUT,
            "输出协议",
            &["Streamable HTTP（推荐）", "SSE（旧客户端）", "WebSocket"],
            330,
            0,
        );
        self.label_edit(PORT, "监听端口", 370, 30, 0, 0, false);
        self.combo(LAN, "监听地址", &[], 410, 0);
        self.field(117, "BUTTON", "刷新 IP", WS_TABSTOP, -92, 410, 78, 30, 0, 0);
        self.field(
            ENABLED,
            "BUTTON",
            "包含在全部启动中",
            BS_AUTOCHECKBOX as u32 | WS_TABSTOP,
            144,
            454,
            210,
            27,
            0,
            0,
        );
        self.field(
            AUTO,
            "BUTTON",
            "应用启动时自动运行",
            BS_AUTOCHECKBOX as u32 | WS_TABSTOP,
            360,
            454,
            240,
            27,
            0,
            0,
        );
        self.label_edit(ROUTE, "连接路径", 8, 30, 1, 0, false);
        self.label_edit(MESSAGE, "SSE 消息路径", 48, 30, 1, 3, false);
        self.label_edit(KEY, "网关访问密钥", 88, 30, 1, 0, true);
        self.field(
            127,
            "STATIC",
            "客户端发送 Bearer Token 或 X-API-Key；额外密钥在第三页管理。",
            0,
            144,
            126,
            -14,
            36,
            1,
            0,
        );
        self.label_edit(CORS, "CORS 来源", 170, 65, 1, 0, false);
        self.field(
            128,
            "STATIC",
            "留空关闭，* 允许所有来源，或每行一个来源。",
            0,
            144,
            242,
            -14,
            24,
            1,
            0,
        );
        self.field(
            STATEFUL,
            "BUTTON",
            "有状态会话 · 保留同一客户端的 MCP 进程",
            BS_AUTOCHECKBOX as u32 | WS_TABSTOP,
            144,
            280,
            -14,
            30,
            1,
            4,
        );
        self.label_edit(TIMEOUT, "会话超时(ms)", 318, 30, 1, 4, false);
        self.combo(LEVEL, "日志级别", &["普通", "调试", "关闭"], 366, 1);
        self.field(
            TLS,
            "BUTTON",
            "启用 HTTPS / WSS · 自动签发及续期 IP 证书",
            BS_AUTOCHECKBOX as u32 | WS_TABSTOP,
            144,
            414,
            -14,
            30,
            1,
            0,
        );
        self.field(
            143,
            "STATIC",
            "启用后仅 TLS 入口对外；证书管理与信任操作位于“设置”。",
            0,
            144,
            452,
            -14,
            42,
            1,
            0,
        );
        self.field(
            INHERIT_ENV,
            "BUTTON",
            "继承系统环境变量（GUI 启动时的环境）",
            BS_AUTOCHECKBOX as u32 | WS_TABSTOP,
            12,
            8,
            -14,
            28,
            2,
            1,
        );
        self.field(
            157,
            "BUTTON",
            "预览服务环境…",
            WS_TABSTOP,
            12,
            42,
            180,
            30,
            2,
            1,
        );
        self.field(
            158,
            "STATIC",
            "关闭后保留必要 Windows 环境；额外变量仅注入此服务，不修改系统设置。",
            0,
            12,
            77,
            -14,
            24,
            2,
            1,
        );
        for (base, title, y) in [
            (ENV, "环境变量 · 值默认隐藏", 108),
            (HEADERS, "请求头 / 响应头 · 值默认隐藏", 270),
            (KEYS, "额外访问密钥 · 用于多客户端或密钥轮换", 432),
        ] {
            self.field(base + 1000, "STATIC", title, 0, 12, y, -14, 25, 2, 0);
            self.field(
                base,
                "LISTBOX",
                "",
                LBS_NOTIFY as u32 | WS_VSCROLL | WS_TABSTOP,
                12,
                y + 30,
                -14,
                85,
                2,
                0,
            );
            for (offset, label) in [(1, "添加"), (2, "编辑"), (3, "删除")] {
                self.field(
                    base + offset,
                    "BUTTON",
                    label,
                    WS_TABSTOP,
                    12 + (offset - 1) * 92,
                    y + 122,
                    82,
                    30,
                    2,
                    0,
                );
            }
        }
        self.field(
            1153,
            "STATIC",
            "附加程序目录 · 例如 Blender 所在文件夹",
            0,
            12,
            594,
            -14,
            25,
            2,
            1,
        );
        self.field(
            PATHS,
            "LISTBOX",
            "",
            LBS_NOTIFY as u32 | WS_VSCROLL | WS_TABSTOP,
            12,
            624,
            -14,
            100,
            2,
            1,
        );
        self.field(
            154,
            "BUTTON",
            "添加目录…",
            WS_TABSTOP,
            12,
            732,
            120,
            30,
            2,
            1,
        );
        self.field(
            155,
            "BUTTON",
            "删除目录",
            WS_TABSTOP,
            144,
            732,
            120,
            30,
            2,
            1,
        );
        self.field(156, "STATIC", "托管 PATH：虚拟环境 → 托管工具 → 附加目录 → Windows → 继承 PATH。列表顺序为查找顺序；移动目录后重新导出。", 0, 12, 774, -14, 65, 2, 1);
        self.field(
            170,
            "STATIC",
            "仅生成客户端连接配置 · 请先启动 GUI 网关",
            0,
            12,
            14,
            -14,
            35,
            3,
            0,
        );
        self.label_edit(CLIENT_STARTUP, "启动超时（秒）", 65, 30, 3, 0, false);
        self.label_edit(CLIENT_TOOL, "工具超时（秒）", 111, 30, 3, 0, false);
        self.field(
            173,
            "STATIC",
            "超时留空使用客户端默认值，范围 1–86400 秒；仅 Codex TOML 导出。不是网关会话空闲超时。",
            0,
            12,
            153,
            -14,
            45,
            3,
            0,
        );
        self.field(175, "STATIC", "此页仅影响导出的客户端配置，不改变网关或 MCP 服务。HTTP 请求不创建本地 MCP 子进程，不导出环境变量。服务变量在“环境 / 请求头 / 密钥”页设置。JSON 不包含 Codex 超时字段。", 0, 12, 209, -14, 95, 3, 0);
        for (i, (id, name)) in [(38, "复制 Codex TOML"), (39, "复制客户端 JSON")]
            .iter()
            .enumerate()
        {
            self.field(
                *id,
                "BUTTON",
                name,
                WS_TABSTOP,
                12 + (i % 2) as i32 * 230,
                325 + (i / 2) as i32 * 43,
                218,
                32,
                3,
                0,
            );
        }
        self.field(176, "STATIC", "客户端参数只用于导出。认证取当前网关访问密钥；不复制上游认证或 MCP 环境。JSON 支持 HTTP / SSE；Codex TOML 仅支持 Streamable HTTP，不生成 stdio 桥接。", 0, 12, 375, -14, 75, 3, 0);
        self.field(
            36,
            "BUTTON",
            "复制网关配置",
            WS_TABSTOP,
            144,
            510,
            218,
            32,
            1,
            0,
        );
        for (id, text) in [
            (30, "保存配置"),
            (31, "启动"),
            (32, "停止"),
            (33, "应用并重启"),
            (34, "测试连接"),
            (35, "复制连接地址"),
        ] {
            self.add(self.hwnd, id, "BUTTON", text, WS_TABSTOP);
        }
        self.add(
            self.hwnd,
            40,
            "EDIT",
            "",
            ES_READONLY as u32
                | ES_MULTILINE as u32
                | ES_AUTOVSCROLL as u32
                | WS_VSCROLL
                | WS_HSCROLL,
        );
        SendMessageW(self.ctrl(40), EM_SETLIMITTEXT, 256 * 1024, 0);
        self.add(self.hwnd, 41, "BUTTON", "清空日志", WS_TABSTOP);
        for id in [TAB, PANEL, 30, 31, 32, 33, 34, 35, 36, 38, 39] {
            EnableWindow(self.ctrl(id), 0);
        }
    }
    unsafe fn move_control(&self, id: i32, x: i32, y: i32, w: i32, h: i32) {
        SetWindowPos(
            self.ctrl(id),
            null_mut(),
            self.scale(x),
            self.scale(y),
            self.scale(w.max(1)),
            self.scale(h.max(1)),
            SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOREDRAW | SWP_NOCOPYBITS,
        );
    }
    unsafe fn layout(&mut self) {
        if self.ctrl(LIST).is_null() {
            return;
        }
        let mut rect: RECT = zeroed();
        GetClientRect(self.hwnd, &mut rect);
        let width = rect.right * 96 / self.dpi as i32;
        let height = rect.bottom * 96 / self.dpi as i32;
        self.move_control(2, 20, 14, 250, 32);
        self.move_control(3, 20, 51, 260, 26);
        for (i, id) in [12, 13, 14, 15, 16, 18, 17].iter().enumerate() {
            self.move_control(*id, width - 634 + i as i32 * 88, 24, 82, 32);
        }
        self.move_control(10, 20, 90, 104, 32);
        self.move_control(11, 134, 90, 104, 32);
        self.move_control(LIST, 20, 132, 230, (height - 316).max(60));
        self.move_control(4, 274, 86, width - 294, 24);
        self.move_control(5, 274, 114, width - 294, 28);
        self.move_control(TAB, 274, 150, width - 294, 30);
        let action_y = height - 232;
        let panel_height = (action_y - 190).max(100);
        self.move_control(PANEL, 274, 188, width - 294, panel_height);
        for (i, id) in [30, 31, 32, 33, 34].iter().enumerate() {
            self.move_control(*id, 274 + i as i32 * 113, action_y, 106, 32);
        }
        let export_width = (width - 294) / 4;
        for (i, id) in [35].iter().enumerate() {
            self.move_control(
                *id,
                274 + i as i32 * export_width,
                action_y + 40,
                export_width - 8,
                32,
            );
        }
        self.move_control(40, 20, height - 145, width - 40, 125);
        self.move_control(41, 20, height - 185, 108, 30);
        let panel_width = width - 314;
        let source = self.combo_index(SOURCE);
        let original = self.combo_index(MANAGED) != 1 || !self.text(STDIO).trim().is_empty();
        self.dependency_summary();
        for id in [COMMAND, ARGS, 113] {
            EnableWindow(self.ctrl(id), original as i32);
        }
        let max = if self.page == 3 {
            690
        } else if self.page == 2 {
            850
        } else if self.page == 0 && source != 0 {
            360
        } else if self.page == 0 {
            680
        } else {
            510
        };
        self.scroll = self.scroll.clamp(0, (max - panel_height).max(0));
        let info = SCROLLINFO {
            cbSize: size_of::<SCROLLINFO>() as u32,
            fMask: SIF_RANGE | SIF_PAGE | SIF_POS,
            nMin: 0,
            nMax: max - 1,
            nPage: panel_height as u32,
            nPos: self.scroll,
            ..zeroed()
        };
        SetScrollInfo(self.panel, SB_VERT, &info, 1);
        let output = self.combo_index(OUTPUT);
        for f in &self.fields {
            let show = f.page == self.page
                && match f.source {
                    1 => source == 0,
                    2 => source != 0,
                    3 => output == 1,
                    4 => output == 0,
                    _ => true,
                };
            let x = if f.x < 0 { panel_width + f.x } else { f.x };
            let mut width = if f.width < 0 {
                panel_width - x + f.width
            } else {
                f.width
            };
            if [self.ctrl(COMMAND), self.ctrl(CWD), self.ctrl(LAN)].contains(&f.window) {
                width -= 88;
            }
            if f.window == self.ctrl(PORT) {
                width = 132;
            }
            SetWindowPos(
                f.window,
                null_mut(),
                self.scale(x),
                self.scale(
                    f.y - self.scroll
                        - if f.page == 0 && source != 0 && f.y >= 330 {
                            140
                        } else {
                            0
                        },
                ),
                self.scale(width.max(10)),
                self.scale(f.height),
                SWP_NOZORDER
                    | SWP_NOACTIVATE
                    | SWP_NOREDRAW
                    | SWP_NOCOPYBITS
                    | if show { SWP_SHOWWINDOW } else { SWP_HIDEWINDOW },
            );
        }
        // Moving one child must not copy pixels from overlapping siblings at
        // their old positions. Paint the final layout, including every child,
        // only after all geometry and visibility changes are complete.
        RedrawWindow(
            self.hwnd,
            null(),
            null_mut(),
            RDW_INVALIDATE | RDW_ERASE | RDW_ALLCHILDREN,
        );
    }
    unsafe fn text(&self, id: i32) -> String {
        let h = self.ctrl(id);
        let mut text = vec![0u16; GetWindowTextLengthW(h) as usize + 1];
        let count = GetWindowTextW(h, text.as_mut_ptr(), text.len() as i32);
        String::from_utf16_lossy(&text[..count as usize])
    }
    unsafe fn set(&self, id: i32, text: &str) {
        SetWindowTextW(self.ctrl(id), wide(text).as_ptr());
    }
    unsafe fn combo_index(&self, id: i32) -> i32 {
        SendMessageW(self.ctrl(id), CB_GETCURSEL, 0, 0) as i32
    }
    unsafe fn combo_set(&self, id: i32, index: i32) {
        SendMessageW(self.ctrl(id), CB_SETCURSEL, index as usize, 0);
    }
    unsafe fn refresh_addresses(&mut self, selected: &str) {
        let mut choices = vec![
            (
                "0.0.0.0".to_owned(),
                "0.0.0.0 · 所有网卡（默认）".to_owned(),
            ),
            ("127.0.0.1".to_owned(), "127.0.0.1 · 仅本机访问".to_owned()),
        ];
        match gateway::network::local_addresses() {
            Ok(addresses) => choices.extend(
                addresses
                    .into_iter()
                    .map(|a| (a.ip.to_string(), format!("{} · {}", a.ip, a.adapter))),
            ),
            Err(error) => {
                self.message(&error, MB_ICONWARNING);
            }
        }
        if !choices.iter().any(|(ip, _)| ip == selected) {
            choices.push((
                selected.into(),
                format!("{selected} · 已保存，当前网卡未检测到"),
            ));
        }
        SendMessageW(self.ctrl(LAN), CB_RESETCONTENT, 0, 0);
        for (_, label) in &choices {
            SendMessageW(
                self.ctrl(LAN),
                CB_ADDSTRING,
                0,
                wide(label).as_ptr() as isize,
            );
        }
        self.listen_addresses = choices.into_iter().map(|(ip, _)| ip).collect();
        self.combo_set(
            LAN,
            self.listen_addresses
                .iter()
                .position(|ip| ip == selected)
                .unwrap_or(0) as i32,
        );
    }
    unsafe fn check(&self, id: i32) -> bool {
        SendMessageW(self.ctrl(id), BM_GETCHECK, 0, 0) == BST_CHECKED as isize
    }
    unsafe fn check_set(&self, id: i32, checked: bool) {
        SendMessageW(
            self.ctrl(id),
            BM_SETCHECK,
            if checked { BST_CHECKED } else { BST_UNCHECKED } as usize,
            0,
        );
    }
    unsafe fn message(&self, text: &str, flags: u32) -> i32 {
        MessageBoxW(
            self.hwnd,
            wide(text).as_ptr(),
            wide("SuperGateway").as_ptr(),
            flags,
        )
    }
    unsafe fn refresh_list(&self) {
        SendMessageW(self.ctrl(LIST), LB_RESETCONTENT, 0, 0);
        for p in &self.settings.services {
            let mut status = self
                .states
                .get(&p.id)
                .map(|v| format!("{}{}", v.0, if v.2 { " · 待应用" } else { "" }))
                .unwrap_or("已停止".into());
            if status == "已停止" && self.needs_install(p) {
                status = "待安装".into();
            }
            SendMessageW(
                self.ctrl(LIST),
                LB_ADDSTRING,
                0,
                wide(format!("{} · {} · :{}", p.name, status, p.port)).as_ptr() as isize,
            );
        }
        if let Some(i) = self.selected {
            SendMessageW(self.ctrl(LIST), LB_SETCURSEL, i, 0);
        }
    }
    unsafe fn select(&mut self, index: usize) {
        self.loading = true;
        self.selected = Some(index);
        self.draft = self.settings.services[index].clone();
        self.saved = self.draft.clone();
        let p = self.draft.clone();
        self.set(NAME, &p.name);
        self.combo_set(
            SOURCE,
            match p.source.as_str() {
                "http" => 1,
                "sse" => 2,
                _ => 0,
            },
        );
        self.set(COMMAND, &p.command);
        self.set(ARGS, &p.args.join("\r\n"));
        self.set(CWD, &p.cwd);
        self.set(STDIO, &p.stdio);
        self.check_set(INHERIT_ENV, p.inherit_environment);
        self.combo_set(MANAGED, if p.managed { 1 } else { 0 });
        self.set(URL, &p.url);
        self.set(BEARER, &p.bearer);
        self.combo_set(
            OUTPUT,
            match p.output.as_str() {
                "sse" => 1,
                "ws" => 2,
                _ => 0,
            },
        );
        self.set(PORT, &p.port.to_string());
        self.refresh_addresses(p.listen_host());
        self.check_set(ENABLED, p.enabled);
        self.check_set(AUTO, p.auto_start);
        self.set(ROUTE, &p.route);
        self.set(MESSAGE, &p.message_path);
        self.set(KEY, &p.api_key);
        self.set(CORS, &p.cors.replace('\n', "\r\n"));
        self.check_set(STATEFUL, p.stateful);
        self.check_set(TLS, p.tls);
        self.set(TIMEOUT, &p.session_timeout.to_string());
        self.set(
            CLIENT_STARTUP,
            &p.client
                .startup_timeout_sec
                .map(|v| v.to_string())
                .unwrap_or_default(),
        );
        self.set(
            CLIENT_TOOL,
            &p.client
                .tool_timeout_sec
                .map(|v| v.to_string())
                .unwrap_or_default(),
        );
        self.combo_set(
            LEVEL,
            match p.log_level.as_str() {
                "debug" => 1,
                "none" => 2,
                _ => 0,
            },
        );
        self.pair_lists();
        SendMessageW(self.ctrl(LIST), LB_SETCURSEL, index, 0);
        for id in [TAB, PANEL, 30, 31, 32, 33, 34, 35, 36, 38, 39] {
            EnableWindow(
                self.ctrl(id),
                if self.busy && [30, 31, 32, 33, 34].contains(&id) {
                    0
                } else {
                    1
                },
            );
        }
        self.loading = false;
        self.update_status();
        self.layout();
    }
    unsafe fn read(&self) -> Result<Profile> {
        if self.selected.is_none() {
            return Err("请先添加或选择服务".into());
        }
        let mut p = self.draft.clone();
        let seconds = |id| -> Result<Option<u32>> {
            let value = self.text(id);
            if value.trim().is_empty() {
                Ok(None)
            } else {
                value
                    .trim()
                    .parse()
                    .map(Some)
                    .map_err(|_| "客户端超时须为整数秒，或留空".into())
            }
        };
        p.client.stdio = false; // Legacy field; GUI exports only network connections.
        p.client.startup_timeout_sec = seconds(CLIENT_STARTUP)?;
        p.client.tool_timeout_sec = seconds(CLIENT_TOOL)?;
        p.name = self.text(NAME).trim().into();
        p.source = match self.combo_index(SOURCE) {
            1 => "http",
            2 => "sse",
            _ => "local",
        }
        .into();
        p.command = self.text(COMMAND).trim().trim_matches('"').into();
        let args = self.text(ARGS).replace('\r', "");
        p.args = if args.is_empty() {
            vec![]
        } else {
            args.split('\n').map(str::to_owned).collect()
        };
        p.cwd = self.text(CWD).trim().into();
        p.stdio = self.text(STDIO).trim().into();
        p.managed = self.combo_index(MANAGED) == 1;
        p.inherit_environment = self.check(INHERIT_ENV);
        p.url = self.text(URL).trim().into();
        p.bearer = self.text(BEARER);
        p.output = match self.combo_index(OUTPUT) {
            1 => "sse",
            2 => "ws",
            _ => "streamableHttp",
        }
        .into();
        p.port = self
            .text(PORT)
            .parse::<u16>()
            .map_err(|_| "端口须为 1–65535")?;
        let host = self
            .listen_addresses
            .get(self.combo_index(LAN) as usize)
            .ok_or("请选择监听地址")?;
        if host != p.listen_host() {
            p.listen_address = Some(host.clone());
            p.lan = host != "127.0.0.1";
        }
        p.enabled = self.check(ENABLED);
        p.auto_start = self.check(AUTO);
        p.route = self.text(ROUTE).trim().into();
        p.message_path = self.text(MESSAGE).trim().into();
        p.api_key = self.text(KEY);
        p.cors = self.text(CORS).replace('\r', "").trim().into();
        p.stateful = self.check(STATEFUL);
        p.tls = self.check(TLS);
        p.session_timeout = self.text(TIMEOUT).parse().map_err(|_| "会话超时须为整数")?;
        p.log_level = match self.combo_index(LEVEL) {
            1 => "debug",
            2 => "none",
            _ => "info",
        }
        .into();
        Ok(p)
    }
    unsafe fn persist(&self) -> Result<()> {
        self.store.save(&self.settings)
    }
    unsafe fn dependencies_dialog(&mut self, service: bool) -> Result<()> {
        let profile = if service {
            let p = self.read()?;
            if p.source != "local" {
                return Err("只有本地 MCP 服务需要托管依赖".into());
            }
            Some(p)
        } else {
            None
        };
        let (network, profile) = dependency_ui::show(
            self.hwnd,
            self.font,
            self.store.root.clone(),
            self.manager.clone(),
            self.settings.services.clone(),
            self.settings.dependency_network.clone(),
            profile,
        );
        if let Some(network) = network {
            let previous = self.settings.dependency_network.clone();
            self.settings.dependency_network = network;
            if let Err(error) = self.persist() {
                self.settings.dependency_network = previous;
                return Err(error);
            }
        }
        if let Some(profile) = profile {
            self.draft.dependency = profile.dependency;
            self.draft.managed = profile.managed;
            self.combo_set(MANAGED, if profile.managed { 1 } else { 0 });
            self.update_status();
            self.layout();
        }
        Ok(())
    }
    unsafe fn storage_dialog(&mut self) -> Result<()> {
        if !self.unsaved()? {
            return Ok(());
        }
        let dependency = gateway::storage::dependency_directory(&self.store.root)?;
        let explicit = std::env::args().any(|v| v == "--data-dir");
        let Some((configuration, dependencies)) = directories_dialog(
            self.hwnd,
            self.font,
            &self.store.root,
            &dependency,
            explicit,
        ) else {
            return Ok(());
        };
        if configuration == self.store.root && dependencies == dependency {
            return Ok(());
        }
        if self.settings.admin_login_startup && !gateway::startup::is_elevated() {
            return Err(
                "已启用管理员登录启动，请先以管理员身份重启，再更改存放路径，以便同步计划任务。"
                    .into(),
            );
        }
        if self.message("保存目录并重启应用？\r\n现有配置与证书复制到新配置目录，原文件保留；新配置目录必须为空。\r\n依赖不迁移、不删除、不自动安装。网关将停止，仅自动运行的服务会在重启后恢复。", MB_YESNO | MB_ICONQUESTION) != IDYES {
            return Ok(());
        }
        self.persist()?;
        let old = self.store.root.clone();
        let settings = self.settings.clone();
        self.worker(move |manager| {
            manager.dependencies.execute(|| {
                manager.stop_all();
                let root = gateway::storage::prepare(&old, &configuration, &dependencies)?;
                let startup = settings.login_startup || settings.admin_login_startup;
                let result = (|| {
                    if startup {
                        gateway::startup::apply(
                            settings.admin_login_startup,
                            settings.login_startup,
                            settings.admin_login_startup,
                            &root,
                            explicit,
                        )?;
                    }
                    if !explicit {
                        gateway::storage::remember_configuration(&default_root(), &root)?;
                    }
                    Ok(())
                })();
                if let Err(error) = result {
                    let _ = gateway::storage::set_dependency_directory(&old, &dependency);
                    if startup {
                        let _ = gateway::startup::apply(
                            settings.admin_login_startup,
                            settings.login_startup,
                            settings.admin_login_startup,
                            &old,
                            explicit,
                        );
                    }
                    return Err(error);
                }
                let _ = manager.tx.send(Event::StorageChanged(root));
                Ok(())
            })
        });
        Ok(())
    }
    unsafe fn save(&mut self) -> Result<Profile> {
        let p = self.read()?;
        config::validate(&p, false)?;
        if self
            .settings
            .services
            .iter()
            .any(|v| v.id != p.id && (v.name == p.name || v.port == p.port))
        {
            return Err("名称或端口与另一个服务重复".into());
        }
        let old = self.settings.clone();
        self.settings.services[self.selected.unwrap()] = p.clone();
        if let Err(e) = self.persist() {
            self.settings = old;
            return Err(e);
        }
        if let Some(state) = self.states.get_mut(&p.id)
            && ["网关运行", "MCP 验证成功", "MCP 验证失败"].contains(&state.0.as_str())
        {
            state.2 = !p.gateway_eq(&self.saved) || state.2;
        }
        self.draft = p.clone();
        self.saved = p.clone();
        self.refresh_list();
        self.update_status();
        Ok(p)
    }
    unsafe fn unsaved(&mut self) -> Result<bool> {
        if self.selected.is_none() {
            return Ok(true);
        }
        if self.read().map(|p| p == self.saved).unwrap_or(false) {
            return Ok(true);
        }
        match self.message(
            "当前配置尚未保存。是否先保存？",
            MB_YESNOCANCEL | MB_ICONQUESTION,
        ) {
            IDYES => {
                self.save()?;
                Ok(true)
            }
            IDNO => Ok(true),
            _ => Ok(false),
        }
    }
    unsafe fn update_status(&self) {
        if let Some(i) = self.selected {
            let p = &self.settings.services[i];
            let (mut state, mut detail, pending) =
                self.states
                    .get(&p.id)
                    .cloned()
                    .unwrap_or(("已停止".into(), String::new(), false));
            if self.needs_install(p) && state == "已停止" {
                state = "待安装".into();
                detail = "点击“配置包与安装依赖”安装；setup 在安装后通过一次性命令运行。".into();
            }
            self.set(
                4,
                &format!(
                    "{} · {}{}",
                    p.name,
                    state,
                    if pending { " · 配置待应用" } else { "" }
                ),
            );
            self.set(5, &format!("{}   {}", p.address(false), detail));
        }
    }
    fn needs_install(&self, p: &Profile) -> bool {
        p.source == "local"
            && p.managed
            && p.stdio.is_empty()
            && !p.dependency.package.is_empty()
            && !self.states.get(&p.id).is_some_and(|s| {
                ["启动中", "网关运行", "MCP 验证成功", "MCP 验证失败"].contains(&s.0.as_str())
            })
            && gateway::dependencies::check_installation(&self.store.root, p).is_err()
    }
    unsafe fn dependency_summary(&self) {
        let text = if self.combo_index(MANAGED) != 1 {
            "未安装 MCP？选择“应用管理的 npm / Python 包”，点击“配置包与安装依赖”，填写包名即可保存；无需程序路径或先运行 setup。".into()
        } else {
            let d = &self.draft.dependency;
            let mut summary = format!(
                "{}：{} · 入口：{} · 启动参数：{} 项\r\n{}",
                if d.kind == "python" {
                    "Python 包"
                } else {
                    "npm 包"
                },
                if d.package.is_empty() {
                    "尚未填写包名"
                } else {
                    &d.package
                },
                if d.entry.is_empty() {
                    "安装时自动识别"
                } else {
                    &d.entry
                },
                d.args.len(),
                if self.text(STDIO).trim().is_empty() {
                    "当前使用包入口；填写有效包名后即可保存，原始程序与参数保留。"
                } else {
                    "当前优先执行完整启动命令，包入口与参数不参与启动；清空完整命令后才使用包入口。"
                }
            );
            summary
                .push_str("\r\n保存不会下载或执行命令；安装完成后，可通过一次性命令运行 setup。");
            summary
        };
        self.set(151, &text);
    }
    unsafe fn pair_lists(&self) {
        SendMessageW(self.ctrl(PATHS), LB_RESETCONTENT, 0, 0);
        for dir in &self.draft.additional_paths {
            SendMessageW(
                self.ctrl(PATHS),
                LB_ADDSTRING,
                0,
                wide(dir).as_ptr() as isize,
            );
        }
        for (id, pairs) in [(ENV, &self.draft.env), (HEADERS, &self.draft.headers)] {
            SendMessageW(self.ctrl(id), LB_RESETCONTENT, 0, 0);
            for p in pairs {
                SendMessageW(
                    self.ctrl(id),
                    LB_ADDSTRING,
                    0,
                    wide(format!("{}  =  ********", p.key)).as_ptr() as isize,
                );
            }
        }
        SendMessageW(self.ctrl(KEYS), LB_RESETCONTENT, 0, 0);
        for i in 0..self.draft.api_keys.len() {
            SendMessageW(
                self.ctrl(KEYS),
                LB_ADDSTRING,
                0,
                wide(format!("密钥 {}  ·  ********", i + 1)).as_ptr() as isize,
            );
        }
    }
    unsafe fn set_busy(&mut self, busy: bool) {
        self.busy = busy;
        for id in [10, 11, 12, 13, 14, 15, 16, 30, 31, 32, 33, 34] {
            EnableWindow(
                self.ctrl(id),
                if !busy && (id < 30 || self.selected.is_some()) {
                    1
                } else {
                    0
                },
            );
        }
    }
    unsafe fn worker(&mut self, action: impl FnOnce(Arc<Manager>) -> Result<()> + Send + 'static) {
        self.set_busy(true);
        let manager = self.manager.clone();
        thread::spawn(move || {
            if let Err(e) = action(manager.clone()) {
                let _ = manager.tx.send(Event::Error(e));
            }
            let _ = manager.tx.send(Event::Done);
        });
    }
    unsafe fn start_many(&mut self, profiles: Vec<Profile>) {
        if self.busy {
            return;
        }
        self.worker(move |manager| {
            let mut errors = vec![];
            for p in profiles {
                if let Err(e) = manager.start(&p) {
                    errors.push(format!("{}：{e}", p.name));
                }
            }
            if errors.is_empty() {
                Ok(())
            } else {
                Err(errors.join("\n"))
            }
        });
    }
    unsafe fn stop_many(&mut self, profiles: Vec<Profile>) {
        if self.busy {
            return;
        }
        self.worker(move |manager| {
            for p in profiles {
                manager.stop(&p);
            }
            Ok(())
        });
    }
    unsafe fn exit(&mut self) {
        if self.exiting {
            return;
        }
        self.exiting = true;
        EnableWindow(self.hwnd, 0);
        let hwnd = self.hwnd as usize;
        let manager = self.manager.clone();
        thread::spawn(move || {
            manager.dependencies.cancel();
            manager.stop_all();
            manager.dependencies.cancel_and_wait();
            PostMessageW(hwnd as HWND, WM_EXIT_READY, 0, 0);
        });
    }
    unsafe fn events(&mut self) {
        let mut logs_changed = false;
        while let Ok(event) = self.rx.try_recv() {
            match event {
                Event::Log(id, line) => {
                    let name = self
                        .settings
                        .services
                        .iter()
                        .find(|p| p.id == id)
                        .map(|p| p.name.as_str())
                        .unwrap_or("网关");
                    self.logs.push_back(format!("[{name}] {line}"));
                    while self.logs.len() > 1000
                        || self.logs.iter().map(String::len).sum::<usize>() > 128 * 1024
                    {
                        self.logs.pop_front();
                    }
                    logs_changed = true;
                }
                Event::State(id, status, detail) => {
                    let pending = self.states.get(&id).map(|s| s.2).unwrap_or(false)
                        && status != "已停止"
                        && status != "启动中";
                    self.states.insert(id, (status, detail, pending));
                    self.refresh_list();
                    self.update_status();
                }
                Event::Tools(id, tools) => {
                    let pending = self.states.get(&id).map(|s| s.2).unwrap_or(false);
                    self.states.insert(
                        id,
                        (
                            "MCP 验证成功".into(),
                            format!("{} 个工具", tools.len()),
                            pending,
                        ),
                    );
                    self.refresh_list();
                    self.update_status();
                    let text = if tools.is_empty() {
                        "连接成功，此 MCP 没有工具".into()
                    } else {
                        tools
                            .iter()
                            .map(|v| format!("{}\r\n{}", v.name, v.description))
                            .collect::<Vec<_>>()
                            .join("\r\n\r\n")
                    };
                    preview(self.hwnd, self.font, "MCP 工具列表", &text, None);
                }
                Event::Error(e) => {
                    if !self.exiting {
                        self.message(&e, MB_ICONWARNING);
                    }
                }
                Event::SettingsSaved(settings) => self.settings = settings,
                Event::StorageChanged(root) => {
                    let result = std::env::current_exe().and_then(|exe| {
                        let mut command = std::process::Command::new(exe);
                        if std::env::args().any(|v| v == "--data-dir") {
                            command.arg("--data-dir").arg(&root);
                        }
                        command
                            .arg("--wait-for-pid")
                            .arg(std::process::id().to_string());
                        command.spawn()
                    });
                    match result {
                        Ok(_) => self.exit(),
                        Err(error) => {
                            self.message(
                                &format!(
                                    "目录设置已保存，自动重启失败：{error}。请退出后重新打开。"
                                ),
                                MB_ICONERROR,
                            );
                        }
                    }
                }
                Event::Certificates(text) => {
                    preview(self.hwnd, self.font, "HTTPS 证书状态", &text, None);
                }
                Event::Done => self.set_busy(false),
            }
        }
        if logs_changed {
            self.set(
                40,
                &self.logs.iter().cloned().collect::<Vec<_>>().join("\r\n"),
            );
            SendMessageW(self.ctrl(40), EM_SETSEL, usize::MAX, -1);
            SendMessageW(self.ctrl(40), EM_SCROLLCARET, 0, 0);
        }
    }
    unsafe fn command(&mut self, id: i32, code: u32) -> Result<()> {
        if self.loading {
            return Ok(());
        }
        if self.busy && ![17, 18, 36, 41, LIST, TAB, SOURCE, OUTPUT].contains(&id) {
            return Ok(());
        }
        match id {
            18 => help_dialog(self.hwnd, self.font),
            10 => {
                if !self.unsaved()? {
                    return Ok(());
                }
                let mut p = Profile {
                    port: config::free_port(&self.settings.services)?,
                    ..Default::default()
                };
                let mut n = 2;
                while self.settings.services.iter().any(|s| s.name == p.name) {
                    p.name = format!("新服务 {n}");
                    n += 1;
                }
                self.settings.services.push(p);
                self.persist()?;
                self.refresh_list();
                self.select(self.settings.services.len() - 1);
            }
            11 => {
                if let Some(i) = self.selected {
                    let p = self.settings.services[i].clone();
                    if self.message(
                        &format!("删除「{}」并停止其网关？", p.name),
                        MB_YESNO | MB_ICONQUESTION,
                    ) == IDYES
                    {
                        let remove_dependencies = self.message("是否同时删除这个服务的全部托管依赖目录？下载缓存和共享运行环境会保留。", MB_YESNO | MB_ICONQUESTION) == IDYES;
                        let root = self.store.root.clone();
                        let manager = self.manager.clone();
                        self.worker(move |_| {
                            manager.stop(&p);
                            if remove_dependencies {
                                manager.dependencies.execute(|| {
                                    gateway::dependencies::remove_service(
                                        &root,
                                        &p.id,
                                        &manager.active_profiles(),
                                    )
                                })?;
                            }
                            Ok(())
                        });
                        self.settings.services.remove(i);
                        self.selected = None;
                        self.persist()?;
                        self.refresh_list();
                        if !self.settings.services.is_empty() {
                            self.select(i.min(self.settings.services.len() - 1));
                        } else {
                            for id in [TAB, PANEL, 30, 31, 32, 33, 34, 35, 36, 38, 39] {
                                EnableWindow(self.ctrl(id), 0);
                            }
                            self.set(4, "添加你的第一个 MCP 服务");
                            self.set(5, "");
                        }
                    }
                }
            }
            12 | 13 => {
                let text = if id == 12 {
                    let Some(path) = native::file_dialog(
                        self.hwnd,
                        false,
                        "MCP 配置|*.json;*.jsonc|所有文件|*.*",
                        "",
                    ) else {
                        return Ok(());
                    };
                    fs::read_to_string(path).map_err(|e| e.to_string())?
                } else {
                    clipboard_read(self.hwnd)?
                };
                let result = config::import(&text, &self.settings.services)?;
                if preview(
                    self.hwnd,
                    self.font,
                    "导入预览 · 不会自动启动",
                    &result.messages.join("\r\n"),
                    if result.profiles.is_empty() {
                        None
                    } else {
                        Some("保存有效服务")
                    },
                ) {
                    if !self.unsaved()? {
                        return Ok(());
                    }
                    let first = self.settings.services.len();
                    self.settings.services.extend(result.profiles);
                    self.persist()?;
                    self.refresh_list();
                    if self.settings.services.len() > first {
                        self.select(first);
                    }
                }
            }
            14 => self.start_many(
                self.settings
                    .services
                    .iter()
                    .filter(|p| p.enabled)
                    .cloned()
                    .collect(),
            ),
            15 => self.stop_many(self.settings.services.clone()),
            16 => match settings_dialog(
                self.hwnd,
                self.font,
                &self.settings,
                &gateway::tls::info(&self.store.root),
            ) {
                Some(SettingsAction::Certificate(action)) => {
                    if action == 515 {
                        self.dependencies_dialog(false)?;
                        return Ok(());
                    }
                    if action == 516 {
                        self.storage_dialog()?;
                        return Ok(());
                    }
                    if action == 509 && self.message("将此本地 CA 加入当前用户的受信任根证书库？该 CA 可为本机/局域网 IP 签发证书。请确认是自己的 CA。", MB_YESNO | MB_ICONWARNING) != IDYES { return Ok(()); }
                    if action == 513 && self.message("重建 CA 会取消本机对旧 CA 的信任，并备份旧证书目录。所有客户端需重新导入新 CA。确定继续？", MB_YESNO | MB_ICONWARNING) != IDYES { return Ok(()); }
                    let destination = if action == 508 {
                        let Some(path) = native::file_dialog(
                            self.hwnd,
                            true,
                            "Windows CA 证书|*.cer|Node CA 证书|*.pem|所有文件|*.*",
                            "SuperGatewayGUI-CA.cer",
                        ) else {
                            return Ok(());
                        };
                        Some(path)
                    } else {
                        None
                    };
                    self.worker(move |manager| manager.certificate_action(action, destination));
                }
                Some(SettingsAction::Restart) => {
                    if self.unsaved()? {
                        gateway::startup::restart_elevated(
                            self.hwnd,
                            &self.store.root,
                            std::env::args().any(|v| v == "--data-dir"),
                        )?;
                        self.exit();
                    }
                }
                Some(SettingsAction::Save(normal, admin)) => {
                    let old = self.settings.clone();
                    if old.admin_login_startup && admin && !gateway::startup::is_elevated() {
                        return Ok(());
                    }
                    let mut settings = old.clone();
                    settings.login_startup = normal;
                    settings.admin_login_startup = admin;
                    let root = self.store.root.clone();
                    let custom = std::env::args().any(|v| v == "--data-dir");
                    self.worker(move |manager| {
                        if old.admin_login_startup != admin && !gateway::startup::is_elevated() {
                            return Err("修改管理员登录启动必须先以管理员身份运行".into());
                        }
                        gateway::startup::apply(
                            old.admin_login_startup,
                            normal,
                            admin,
                            &root,
                            custom,
                        )?;
                        if let Err(error) = Store::new(root.clone())?.save(&settings) {
                            let rollback = gateway::startup::apply(
                                admin,
                                old.login_startup,
                                old.admin_login_startup,
                                &root,
                                custom,
                            );
                            return Err(format!(
                                "保存配置失败：{error}；启动设置回退：{}",
                                rollback.err().unwrap_or_else(|| "成功".into())
                            ));
                        }
                        let _ = manager.tx.send(Event::SettingsSaved(settings));
                        Ok(())
                    });
                }
                None => {}
            },
            17 => self.exit(),
            145 => self.dependencies_dialog(true)?,
            154 => {
                if let Some(path) = native::folder_dialog_title(
                    self.hwnd,
                    "选择额外程序目录（例如 Blender 所在文件夹）",
                ) {
                    let path = path.to_string_lossy().into_owned();
                    if !self
                        .draft
                        .additional_paths
                        .iter()
                        .any(|p| p.eq_ignore_ascii_case(&path))
                    {
                        self.draft.additional_paths.push(path);
                        self.pair_lists();
                    }
                }
            }
            155 => {
                let index = SendMessageW(self.ctrl(PATHS), LB_GETCURSEL, 0, 0);
                if index >= 0 {
                    self.draft.additional_paths.remove(index as usize);
                    self.pair_lists();
                }
            }
            157 => {
                let p = self.read()?;
                let text = gateway::environment::preview(&self.store.root, &p)?;
                preview(self.hwnd, self.font, "服务环境预览 · 值隐藏", &text, None);
            }
            LIST if code == LBN_SELCHANGE => {
                let index = SendMessageW(self.ctrl(LIST), LB_GETCURSEL, 0, 0);
                if index >= 0 && Some(index as usize) != self.selected {
                    if self.unsaved()? {
                        self.select(index as usize);
                    } else if let Some(i) = self.selected {
                        SendMessageW(self.ctrl(LIST), LB_SETCURSEL, i, 0);
                    }
                }
            }
            SOURCE if code == CBN_SELCHANGE => self.layout(),
            MANAGED if code == CBN_SELCHANGE => self.layout(),
            STDIO if code == EN_CHANGE => self.layout(),
            OUTPUT if code == CBN_SELCHANGE => {
                self.set(
                    ROUTE,
                    match self.combo_index(OUTPUT) {
                        1 => "/sse",
                        2 => "/message",
                        _ => "/mcp",
                    },
                );
                self.layout();
            }
            113 => {
                if let Some(path) = native::file_dialog(
                    self.hwnd,
                    false,
                    "可执行程序|*.exe;*.cmd;*.bat|所有文件|*.*",
                    "",
                ) {
                    self.set(COMMAND, &path.to_string_lossy());
                }
            }
            114 => {
                if let Some(path) = native::folder_dialog(self.hwnd) {
                    self.set(CWD, &path.to_string_lossy());
                }
            }
            30 => match self.save() {
                Ok(p) => {
                    let pending = self.states.get(&p.id).is_some_and(|state| state.2);
                    let detail = if pending {
                        "\n运行中的服务有待应用修改，请点击“应用并重启”使其生效。"
                    } else if self.needs_install(&p) {
                        "\n依赖尚未安装，可通过“配置包与安装依赖”安装。保存不会下载或运行服务。"
                    } else {
                        ""
                    };
                    self.message(
                        &format!("配置保存成功。{detail}"),
                        MB_OK | MB_ICONINFORMATION,
                    );
                }
                Err(error) => {
                    self.message(&format!("配置保存失败：\n{error}"), MB_OK | MB_ICONERROR);
                }
            },
            31 | 33 => {
                let p = self.save()?;
                self.worker(move |manager| {
                    if id == 33 {
                        manager.stop(&p);
                    } else if manager.is_running(&p.id) {
                        return Err("服务已运行，请使用“应用并重启”使配置生效".into());
                    }
                    manager.start(&p)
                });
            }
            32 => {
                if let Some(i) = self.selected {
                    self.stop_many(vec![self.settings.services[i].clone()]);
                }
            }
            34 => {
                let p = self
                    .settings
                    .services
                    .get(self.selected.ok_or("请先选择服务")?)
                    .unwrap()
                    .clone();
                self.worker(move |manager| {
                    if !manager.is_running(&p.id) {
                        return Err("请先启动服务".into());
                    }
                    match gateway::net::test(&manager.active(&p)) {
                        Ok(tools) => {
                            let _ = manager.tx.send(Event::Tools(p.id.clone(), tools));
                            Ok(())
                        }
                        Err(e) => {
                            manager.state(&p, "MCP 验证失败", &e);
                            Err(e)
                        }
                    }
                });
            }
            35 => {
                let p = self
                    .settings
                    .services
                    .get(self.selected.ok_or("请先选择服务")?)
                    .unwrap();
                let active = self.manager.active(p);
                clipboard_write(self.hwnd, &active.address(false))?;
            }
            117 => {
                let host = self
                    .listen_addresses
                    .get(self.combo_index(LAN) as usize)
                    .cloned()
                    .unwrap_or_else(|| "0.0.0.0".into());
                self.refresh_addresses(&host);
                self.worker(|manager| manager.refresh_tls(false));
            }
            38 | 39 => {
                let draft = self.read()?;
                gateway::client::validate(&draft.client)?;
                let p = self
                    .settings
                    .services
                    .get(self.selected.ok_or("请先选择服务")?)
                    .ok_or("服务不存在")?;
                let mut active = self.manager.active(p);
                active.client = draft.client.clone();
                let ca = gateway::tls::ca_path(&self.store.root);
                if (!active.api_key.is_empty() || active.api_keys.iter().any(|key| !key.is_empty()))
                    && self.message(
                        "客户端配置包含认证信息，请保存到可信位置。继续？",
                        MB_YESNO | MB_ICONWARNING,
                    ) != IDYES
                {
                    return Ok(());
                }
                let content = if id == 38 {
                    config::codex_export_with_ca(&active, &self.manager.engine, Some(&ca))?
                } else {
                    gateway::client::network_json(&active, &self.manager.engine, Some(&ca))?
                };
                if id == 39 && (draft.client.startup_timeout_sec.is_some() || draft.client.tool_timeout_sec.is_some())
                    && self.message("通用 JSON 不包含 Codex 专属启动 / 工具超时字段；由目标客户端自行设置。继续复制？", MB_YESNO | MB_ICONINFORMATION) != IDYES { return Ok(()); }
                clipboard_write(self.hwnd, &content)?;
            }
            36 => {
                let p = self.read()?;
                if !p.inherit_environment {
                    return Err(
                        "网关 JSON 本身不能限制环境继承。请由 GUI 启动服务以保留环境设置。".into(),
                    );
                }
                let content = config::build(
                    &gateway::environment::prepare(&gateway::dependencies::prepared(
                        &self.store.root,
                        &p,
                    )?)?,
                    false,
                )?
                .0;
                if !p.secrets().is_empty()
                    && self.message(
                        "导出的网关配置包含明文环境变量或认证信息，请保存到可信位置。继续？",
                        MB_YESNO | MB_ICONWARNING,
                    ) != IDYES
                {
                    return Ok(());
                }
                clipboard_write(self.hwnd, &content)?;
            }
            131..=133 | 135..=137 | 139..=141 => {
                let base = if id < 134 {
                    ENV
                } else if id < 138 {
                    HEADERS
                } else {
                    KEYS
                };
                let operation = id - base;
                let index = SendMessageW(self.ctrl(base), LB_GETCURSEL, 0, 0);
                let pairs = if base == ENV {
                    self.draft.env.clone()
                } else if base == HEADERS {
                    self.draft.headers.clone()
                } else {
                    self.draft
                        .api_keys
                        .iter()
                        .map(|v| Pair {
                            key: "密钥".into(),
                            value: v.clone(),
                        })
                        .collect()
                };
                let mut pairs = pairs;
                if operation == 3 {
                    if index >= 0 {
                        pairs.remove(index as usize);
                    }
                } else {
                    if operation == 2 && index < 0 {
                        return Err("请先选择要编辑的行".into());
                    }
                    let initial = if operation == 1 {
                        Pair::default()
                    } else {
                        pairs[index as usize].clone()
                    };
                    if let Some(pair) = pair_dialog(self.hwnd, self.font, initial, base == KEYS) {
                        if operation == 1 {
                            pairs.push(pair);
                        } else {
                            pairs[index as usize] = pair;
                        }
                    }
                }
                if base == ENV {
                    self.draft.env = pairs;
                } else if base == HEADERS {
                    self.draft.headers = pairs;
                } else {
                    self.draft.api_keys = pairs.into_iter().map(|p| p.value).collect();
                }
                self.pair_lists();
            }
            41 => {
                self.logs.clear();
                self.set(40, "");
            }
            _ => {}
        }
        Ok(())
    }
    unsafe fn tray(&self, operation: u32) -> bool {
        let mut data: NOTIFYICONDATAW = zeroed();
        data.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
        data.hWnd = self.hwnd;
        data.uID = 1;
        data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
        data.uCallbackMessage = WM_TRAY;
        data.hIcon = self
            .icons
            .as_ref()
            .map(|icons| icons.small)
            .unwrap_or_else(|| LoadIconW(GetModuleHandleW(null()), ICON_RESOURCE));
        let tip = wide("SuperGateway Rust · 双击打开");
        data.szTip[..tip.len()].copy_from_slice(&tip);
        Shell_NotifyIconW(operation, &data) != 0
    }
    unsafe fn restore(&self) {
        ShowWindow(self.hwnd, SW_SHOW);
        ShowWindow(self.hwnd, SW_RESTORE);
        SetForegroundWindow(self.hwnd);
    }
}

unsafe fn clipboard_write(owner: HWND, text: &str) -> Result<()> {
    if OpenClipboard(owner) == 0 {
        return Err(native::error("打开剪贴板"));
    }
    let text = wide(text);
    let memory = GlobalAlloc(GMEM_MOVEABLE, text.len() * 2);
    if memory.is_null() {
        CloseClipboard();
        return Err(native::error("分配剪贴板"));
    }
    let ptr = GlobalLock(memory);
    if ptr.is_null() {
        GlobalFree(memory);
        CloseClipboard();
        return Err(native::error("写剪贴板"));
    }
    std::ptr::copy_nonoverlapping(text.as_ptr(), ptr.cast::<u16>(), text.len());
    GlobalUnlock(memory);
    EmptyClipboard();
    let result = SetClipboardData(CF_UNICODETEXT, memory);
    if result.is_null() {
        GlobalFree(memory);
    }
    CloseClipboard();
    if result.is_null() {
        Err(native::error("写剪贴板"))
    } else {
        Ok(())
    }
}
unsafe fn clipboard_read(owner: HWND) -> Result<String> {
    if OpenClipboard(owner) == 0 {
        return Err(native::error("打开剪贴板"));
    }
    let handle = GetClipboardData(CF_UNICODETEXT);
    if handle.is_null() {
        CloseClipboard();
        return Err("剪贴板没有文本".into());
    }
    let p = GlobalLock(handle);
    if p.is_null() {
        CloseClipboard();
        return Err(native::error("读取剪贴板"));
    }
    let max = GlobalSize(handle) / 2;
    let units = std::slice::from_raw_parts(p.cast::<u16>(), max);
    let len = units.iter().position(|v| *v == 0).unwrap_or(max);
    let text = String::from_utf16_lossy(&units[..len]);
    GlobalUnlock(handle);
    CloseClipboard();
    Ok(text)
}
struct Modal {
    hwnd: HWND,
    font: HFONT,
    icons: Option<WindowIcons>,
    result: Option<bool>,
    values: Vec<String>,
    pair: bool,
    key: bool,
    setting: bool,
    help: bool,
    dependency: Option<Box<dependency_ui::Dialog>>,
    storage: bool,
    one_shot: bool,
    dispatching: std::cell::Cell<bool>,
}
unsafe extern "system" fn modal_proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    if msg == WM_NCCREATE {
        let create = &*(l as *const CREATESTRUCTW);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
        (*(create.lpCreateParams as *mut Modal)).hwnd = hwnd;
        return DefWindowProcW(hwnd, msg, w, l);
    }
    let data = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Modal;
    if data.is_null() {
        return DefWindowProcW(hwnd, msg, w, l);
    }
    if (*data).dependency.is_some()
        && [
            WM_COMMAND,
            WM_TIMER,
            WM_CLOSE,
            WM_DPICHANGED,
            WM_SIZE,
            WM_GETMINMAXINFO,
            WM_VSCROLL,
            WM_MOUSEWHEEL,
        ]
        .contains(&msg)
    {
        if (*data).dispatching.replace(true) {
            return 0;
        }
        let result = dependency_ui::procedure(&mut *data, msg, w, l);
        (*data).dispatching.set(false);
        return result;
    }
    if ![
        WM_CLOSE,
        WM_COMMAND,
        WM_DPICHANGED,
        WM_SIZE,
        WM_GETMINMAXINFO,
    ]
    .contains(&msg)
    {
        return DefWindowProcW(hwnd, msg, w, l);
    }
    let modal = &mut *data;
    match msg {
        WM_GETMINMAXINFO if modal.help => {
            let dpi = GetDpiForWindow(hwnd) as i32;
            let info = &mut *(l as *mut MINMAXINFO);
            info.ptMinTrackSize.x = 680 * dpi / 96;
            info.ptMinTrackSize.y = 440 * dpi / 96;
            0
        }
        WM_SIZE if modal.help => {
            help_layout(modal);
            0
        }
        WM_DPICHANGED => {
            if let Ok(icons) = WindowIcons::load((w & 0xffff) as u32) {
                icons.apply(hwnd);
                modal.icons = Some(icons);
            }
            if modal.help {
                let rect = &*(l as *const RECT);
                SetWindowPos(
                    hwnd,
                    null_mut(),
                    rect.left,
                    rect.top,
                    rect.right - rect.left,
                    rect.bottom - rect.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
                help_layout(modal);
            }
            0
        }
        WM_CLOSE => {
            modal.result = Some(false);
            DestroyWindow(hwnd);
            0
        }
        WM_COMMAND => {
            match (w & 0xffff) as i32 {
                504 if modal.help && (w >> 16) as u32 == LBN_SELCHANGE => {
                    let index = SendMessageW(GetDlgItem(hwnd, 504), LB_GETCURSEL, 0, 0);
                    if let Some((_, body)) = crate::help::SECTIONS.get(index as usize) {
                        SetWindowTextW(
                            GetDlgItem(hwnd, 500),
                            wide(body.replace('\n', "\r\n")).as_ptr(),
                        );
                        SendMessageW(GetDlgItem(hwnd, 500), EM_SETSEL, 0, 0);
                        SendMessageW(GetDlgItem(hwnd, 500), EM_SCROLLCARET, 0, 0);
                    }
                }
                1 => {
                    if modal.one_shot {
                        let control = GetDlgItem(hwnd, 500);
                        let mut value = vec![0u16; GetWindowTextLengthW(control) as usize + 1];
                        GetWindowTextW(control, value.as_mut_ptr(), value.len() as i32);
                        modal.values = vec![String::from_utf16_lossy(&value[..value.len() - 1])];
                    }
                    if modal.storage {
                        modal.values = [550, 551]
                            .into_iter()
                            .map(|id| {
                                let control = GetDlgItem(hwnd, id);
                                let mut value =
                                    vec![0u16; GetWindowTextLengthW(control) as usize + 1];
                                GetWindowTextW(control, value.as_mut_ptr(), value.len() as i32);
                                String::from_utf16_lossy(&value[..value.len() - 1])
                                    .trim()
                                    .to_string()
                            })
                            .collect();
                        for value in &modal.values {
                            if let Err(error) =
                                gateway::storage::absolute(std::path::Path::new(value))
                            {
                                MessageBoxW(
                                    hwnd,
                                    wide(error).as_ptr(),
                                    wide("检查目录").as_ptr(),
                                    MB_ICONWARNING,
                                );
                                return 0;
                            }
                        }
                    }
                    if modal.pair {
                        let read = |id| {
                            let control = GetDlgItem(hwnd, id);
                            let mut value = vec![0u16; GetWindowTextLengthW(control) as usize + 1];
                            GetWindowTextW(control, value.as_mut_ptr(), value.len() as i32);
                            String::from_utf16_lossy(&value[..value.len() - 1])
                        };
                        modal.values = vec![read(500), read(501)];
                        if !GetDlgItem(hwnd, 505).is_null() {
                            modal.values.push(
                                (SendMessageW(GetDlgItem(hwnd, 505), BM_GETCHECK, 0, 0)
                                    == BST_CHECKED as isize)
                                    .to_string(),
                            );
                        }
                        if modal.values[0].trim().is_empty() && !modal.key
                            || modal.values[1].is_empty() && modal.key
                        {
                            MessageBoxW(
                                hwnd,
                                wide("请输入名称和有效的密钥值").as_ptr(),
                                wide("检查输入").as_ptr(),
                                MB_ICONWARNING,
                            );
                            return 0;
                        }
                    }
                    if modal.setting {
                        modal.values = vec![
                            (SendMessageW(GetDlgItem(hwnd, 502), BM_GETCHECK, 0, 0)
                                == BST_CHECKED as isize)
                                .to_string(),
                        ];
                        modal.values.push(
                            (SendMessageW(GetDlgItem(hwnd, 505), BM_GETCHECK, 0, 0)
                                == BST_CHECKED as isize)
                                .to_string(),
                        );
                    }
                    modal.result = Some(true);
                    DestroyWindow(hwnd);
                }
                2 => {
                    modal.result = Some(false);
                    DestroyWindow(hwnd);
                }
                506 if modal.setting => {
                    modal.values = vec!["restart".into()];
                    modal.result = Some(true);
                    DestroyWindow(hwnd);
                }
                id @ (507..=509 | 512 | 513 | 515 | 516) if modal.setting => {
                    modal.values = vec![id.to_string()];
                    modal.result = Some(true);
                    DestroyWindow(hwnd);
                }
                id @ (552 | 553) if modal.storage => {
                    if let Some(path) = native::folder_dialog(hwnd) {
                        SetWindowTextW(
                            GetDlgItem(hwnd, if id == 552 { 550 } else { 551 }),
                            wide(path).as_ptr(),
                        );
                    }
                }
                id @ (502 | 505) if modal.setting => {
                    if SendMessageW(GetDlgItem(hwnd, id), BM_GETCHECK, 0, 0) == BST_CHECKED as isize
                    {
                        SendMessageW(
                            GetDlgItem(hwnd, if id == 502 { 505 } else { 502 }),
                            BM_SETCHECK,
                            BST_UNCHECKED as usize,
                            0,
                        );
                    }
                }
                503 => {
                    let show = SendMessageW(GetDlgItem(hwnd, 503), BM_GETCHECK, 0, 0)
                        == BST_CHECKED as isize;
                    SendMessageW(
                        GetDlgItem(hwnd, 501),
                        EM_SETPASSWORDCHAR,
                        if show { 0 } else { '●' as usize },
                        0,
                    );
                    InvalidateRect(GetDlgItem(hwnd, 501), null(), 1);
                }
                _ => {}
            }
            0
        }
        _ => DefWindowProcW(hwnd, msg, w, l),
    }
}
unsafe fn modal_window(
    owner: HWND,
    font: HFONT,
    title: &str,
    width: i32,
    height: i32,
    pair: bool,
    key: bool,
    setting: bool,
) -> Box<Modal> {
    let mut modal = Box::new(Modal {
        hwnd: null_mut(),
        font,
        icons: None,
        result: None,
        values: vec![],
        pair,
        key,
        setting,
        help: false,
        dependency: None,
        storage: false,
        one_shot: false,
        dispatching: std::cell::Cell::new(false),
    });
    let dpi = GetDpiForWindow(owner) as i32;
    let mut rect: RECT = zeroed();
    GetWindowRect(owner, &mut rect);
    CreateWindowExW(
        WS_EX_CONTROLPARENT,
        wide("SGRustModal").as_ptr(),
        wide(title).as_ptr(),
        WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU,
        rect.left + 50 * dpi / 96,
        rect.top + 60 * dpi / 96,
        width * dpi / 96,
        height * dpi / 96,
        owner,
        null_mut(),
        GetModuleHandleW(null()),
        (&mut *modal as *mut Modal).cast(),
    );
    if let Ok(icons) = WindowIcons::load(GetDpiForWindow(modal.hwnd)) {
        icons.apply(modal.hwnd);
        modal.icons = Some(icons);
    }
    EnableWindow(owner, 0);
    modal
}
unsafe fn modal_control(
    modal: &Modal,
    id: i32,
    class: &str,
    text: &str,
    style: u32,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
) -> HWND {
    let dpi = GetDpiForWindow(modal.hwnd) as i32;
    let control = CreateWindowExW(
        if class == "EDIT" { WS_EX_CLIENTEDGE } else { 0 },
        wide(class).as_ptr(),
        wide(text).as_ptr(),
        WS_CHILD | WS_VISIBLE | style,
        x * dpi / 96,
        y * dpi / 96,
        w * dpi / 96,
        h * dpi / 96,
        modal.hwnd,
        id as usize as HMENU,
        GetModuleHandleW(null()),
        null(),
    );
    SendMessageW(control, WM_SETFONT, modal.font as usize, 1);
    if class == "COMBOBOX" {
        disable_combo_wheel(control);
    }
    control
}
unsafe fn modal_loop(owner: HWND, hwnd: HWND) {
    ShowWindow(hwnd, SW_SHOW);
    let mut msg: MSG = zeroed();
    while IsWindow(hwnd) != 0 && GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
        if IsDialogMessageW(hwnd, &msg) == 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    EnableWindow(owner, 1);
    SetForegroundWindow(owner);
}
unsafe fn help_layout(modal: &Modal) {
    let dpi = GetDpiForWindow(modal.hwnd) as i32;
    let mut rect: RECT = zeroed();
    GetClientRect(modal.hwnd, &mut rect);
    let width = rect.right * 96 / dpi;
    let height = rect.bottom * 96 / dpi;
    for (id, x, y, w, h) in [
        (504, 16, 16, 170, height - 76),
        (500, 202, 16, width - 218, height - 76),
        (1, width - 116, height - 46, 100, 30),
    ] {
        SetWindowPos(
            GetDlgItem(modal.hwnd, id),
            null_mut(),
            x * dpi / 96,
            y * dpi / 96,
            w * dpi / 96,
            h * dpi / 96,
            SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOREDRAW | SWP_NOCOPYBITS,
        );
    }
    RedrawWindow(
        modal.hwnd,
        null(),
        null_mut(),
        RDW_INVALIDATE | RDW_ERASE | RDW_ALLCHILDREN,
    );
}
unsafe fn help_dialog(owner: HWND, font: HFONT) {
    let mut modal = modal_window(
        owner,
        font,
        "SuperGateway · 离线帮助",
        900,
        640,
        false,
        false,
        false,
    );
    modal.help = true;
    let style = GetWindowLongPtrW(modal.hwnd, GWL_STYLE);
    SetWindowLongPtrW(
        modal.hwnd,
        GWL_STYLE,
        style | WS_THICKFRAME as isize | WS_MAXIMIZEBOX as isize | WS_CLIPCHILDREN as isize,
    );
    SetWindowPos(
        modal.hwnd,
        null_mut(),
        0,
        0,
        0,
        0,
        SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_FRAMECHANGED,
    );
    let list = modal_control(
        &modal,
        504,
        "LISTBOX",
        "",
        LBS_NOTIFY as u32 | WS_VSCROLL | WS_TABSTOP,
        16,
        16,
        170,
        500,
    );
    for (title, _) in crate::help::SECTIONS {
        SendMessageW(list, LB_ADDSTRING, 0, wide(title).as_ptr() as isize);
    }
    modal_control(
        &modal,
        500,
        "EDIT",
        &crate::help::SECTIONS[0].1.replace('\n', "\r\n"),
        ES_READONLY as u32 | ES_MULTILINE as u32 | ES_AUTOVSCROLL as u32 | WS_VSCROLL | WS_TABSTOP,
        202,
        16,
        660,
        500,
    );
    modal_control(
        &modal,
        1,
        "BUTTON",
        "关闭",
        WS_TABSTOP | BS_DEFPUSHBUTTON as u32,
        760,
        548,
        100,
        30,
    );
    SendMessageW(list, LB_SETCURSEL, 0, 0);
    help_layout(&modal);
    modal_loop(owner, modal.hwnd);
}
unsafe fn preview(owner: HWND, font: HFONT, title: &str, text: &str, accept: Option<&str>) -> bool {
    let modal = modal_window(owner, font, title, 780, 540, false, false, false);
    modal_control(
        &modal,
        500,
        "EDIT",
        text,
        ES_READONLY as u32 | ES_MULTILINE as u32 | WS_VSCROLL | WS_HSCROLL | WS_TABSTOP,
        16,
        16,
        730,
        420,
    );
    modal_control(
        &modal,
        1,
        "BUTTON",
        accept.unwrap_or("关闭"),
        WS_TABSTOP | BS_DEFPUSHBUTTON as u32,
        470,
        454,
        150,
        32,
    );
    if accept.is_some() {
        modal_control(&modal, 2, "BUTTON", "取消", WS_TABSTOP, 636, 454, 110, 32);
    }
    modal_loop(owner, modal.hwnd);
    accept.is_some() && modal.result == Some(true)
}
unsafe fn pair_dialog(owner: HWND, font: HFONT, pair: Pair, key: bool) -> Option<Pair> {
    pair_dialog_enabled(owner, font, pair, key, None).map(|v| v.0)
}
unsafe fn pair_dialog_enabled(
    owner: HWND,
    font: HFONT,
    pair: Pair,
    key: bool,
    enabled: Option<bool>,
) -> Option<(Pair, bool)> {
    let modal = modal_window(
        owner,
        font,
        "编辑键值 · 默认隐藏内容",
        560,
        310,
        true,
        key,
        false,
    );
    modal_control(
        &modal,
        510,
        "STATIC",
        if key { "密钥" } else { "名称" },
        0,
        20,
        20,
        95,
        25,
    );
    let name = modal_control(
        &modal,
        500,
        "EDIT",
        if key { "密钥" } else { &pair.key },
        ES_AUTOHSCROLL as u32 | WS_TABSTOP,
        120,
        18,
        390,
        30,
    );
    if key {
        EnableWindow(name, 0);
    }
    modal_control(&modal, 511, "STATIC", "值", 0, 20, 74, 95, 25);
    modal_control(
        &modal,
        501,
        "EDIT",
        &pair.value,
        ES_PASSWORD as u32 | ES_AUTOHSCROLL as u32 | WS_TABSTOP,
        120,
        72,
        390,
        30,
    );
    modal_control(
        &modal,
        503,
        "BUTTON",
        "显示内容",
        BS_AUTOCHECKBOX as u32 | WS_TABSTOP,
        120,
        120,
        150,
        28,
    );
    modal_control(
        &modal,
        1,
        "BUTTON",
        "保存",
        WS_TABSTOP | BS_DEFPUSHBUTTON as u32,
        286,
        198,
        104,
        32,
    );
    modal_control(&modal, 2, "BUTTON", "取消", WS_TABSTOP, 406, 198, 104, 32);
    if let Some(enabled) = enabled {
        let checkbox = modal_control(
            &modal,
            505,
            "BUTTON",
            "启用此变量（写入导出）",
            BS_AUTOCHECKBOX as u32 | WS_TABSTOP,
            120,
            156,
            270,
            28,
        );
        SendMessageW(
            checkbox,
            BM_SETCHECK,
            if enabled { BST_CHECKED } else { BST_UNCHECKED } as usize,
            0,
        );
    }
    SetFocus(if key {
        GetDlgItem(modal.hwnd, 501)
    } else {
        name
    });
    modal_loop(owner, modal.hwnd);
    if modal.result == Some(true) {
        Some((
            Pair {
                key: modal.values[0].trim().into(),
                value: modal.values[1].clone(),
            },
            modal.values.get(2).is_none_or(|v| v == "true"),
        ))
    } else {
        None
    }
}
enum SettingsAction {
    Save(bool, bool),
    Restart,
    Certificate(i32),
}
unsafe fn one_shot_arguments(owner: HWND, font: HFONT) -> Option<Vec<String>> {
    let mut modal = modal_window(owner, font, "一次性命令参数", 700, 390, false, false, false);
    modal.one_shot = true;
    modal_control(
        &modal,
        501,
        "STATIC",
        "填写传给已安装包的参数，每行一项，例如 setup 或 install-addon。\r\n不填写 uvx / npx 和包名；不作为服务启动参数保存。",
        0,
        24,
        16,
        640,
        52,
    );
    modal_control(
        &modal,
        500,
        "EDIT",
        "",
        WS_TABSTOP | ES_MULTILINE as u32 | ES_AUTOVSCROLL as u32 | WS_VSCROLL,
        24,
        80,
        640,
        160,
    );
    modal_control(
        &modal,
        502,
        "STATIC",
        "下一步预览实际命令；确认后打开可交互终端。\r\n向导可能联网、修改客户端配置或安装插件，由你在终端中选择。",
        0,
        24,
        250,
        640,
        52,
    );
    modal_control(
        &modal,
        1,
        "BUTTON",
        "预览命令",
        WS_TABSTOP | BS_DEFPUSHBUTTON as u32,
        440,
        310,
        106,
        32,
    );
    modal_control(&modal, 2, "BUTTON", "取消", WS_TABSTOP, 558, 310, 106, 32);
    modal_loop(owner, modal.hwnd);
    (modal.result == Some(true)).then(|| {
        let text = modal.values[0].replace('\r', "");
        if text.is_empty() {
            vec![]
        } else {
            text.split('\n').map(str::to_owned).collect()
        }
    })
}
unsafe fn directories_dialog(
    owner: HWND,
    font: HFONT,
    configuration: &std::path::Path,
    dependencies: &std::path::Path,
    explicit: bool,
) -> Option<(PathBuf, PathBuf)> {
    let mut modal = modal_window(
        owner,
        font,
        "配置与依赖存放路径",
        740,
        360,
        false,
        false,
        false,
    );
    modal.storage = true;
    for (edit, browse, label, value, y) in [
        (
            550,
            552,
            "用户配置目录（配置、证书与备份）",
            configuration,
            20,
        ),
        (
            551,
            553,
            "运行环境与依赖目录（环境、包与缓存）",
            dependencies,
            100,
        ),
    ] {
        modal_control(&modal, edit + 10, "STATIC", label, 0, 24, y, 674, 24);
        modal_control(
            &modal,
            edit,
            "EDIT",
            &value.to_string_lossy(),
            ES_AUTOHSCROLL as u32 | WS_TABSTOP,
            24,
            y + 28,
            570,
            30,
        );
        modal_control(
            &modal,
            browse,
            "BUTTON",
            "浏览…",
            WS_TABSTOP,
            608,
            y + 28,
            90,
            30,
        );
    }
    modal_control(
        &modal,
        562,
        "STATIC",
        if explicit {
            "依赖不搬运，原目录保留；已有依赖需在新目录重新安装。\r\n当前使用 --data-dir：重启使用新路径，原快捷方式或命令行需自行更新。"
        } else {
            "依赖不搬运，原目录保留；已有依赖需在新目录重新安装。\r\n配置目录选择将保存，之后双击启动继续使用；--data-dir 可显式覆盖。"
        },
        0,
        24,
        186,
        674,
        62,
    );
    modal_control(
        &modal,
        1,
        "BUTTON",
        "保存并重启",
        WS_TABSTOP | BS_DEFPUSHBUTTON as u32,
        458,
        270,
        120,
        32,
    );
    modal_control(&modal, 2, "BUTTON", "取消", WS_TABSTOP, 598, 270, 100, 32);
    modal_loop(owner, modal.hwnd);
    (modal.result == Some(true)).then(|| {
        (
            PathBuf::from(&modal.values[0]),
            PathBuf::from(&modal.values[1]),
        )
    })
}
unsafe fn settings_dialog(
    owner: HWND,
    font: HFONT,
    settings: &Settings,
    certificates: &str,
) -> Option<SettingsAction> {
    let admin = gateway::startup::is_elevated();
    let modal = modal_window(owner, font, "应用设置", 740, 630, false, false, true);
    for (id, label, checked, y) in [
        (502, "登录 Windows 后启动到托盘", settings.login_startup, 20),
        (
            505,
            "开机时以管理员身份启动（登录后）",
            settings.admin_login_startup,
            60,
        ),
    ] {
        let control = modal_control(
            &modal,
            id,
            "BUTTON",
            label,
            BS_AUTOCHECKBOX as u32 | WS_TABSTOP,
            24,
            y,
            560,
            32,
        );
        SendMessageW(
            control,
            BM_SETCHECK,
            if checked { BST_CHECKED } else { BST_UNCHECKED } as usize,
            0,
        );
    }
    EnableWindow(GetDlgItem(modal.hwnd, 505), admin as i32);
    EnableWindow(
        GetDlgItem(modal.hwnd, 502),
        (admin || !settings.admin_login_startup) as i32,
    );
    modal_control(
        &modal,
        510,
        "STATIC",
        if admin {
            "当前已以管理员身份运行，可修改管理员启动选项。\r\n两种登录启动方式互斥；管理员启动使用当前用户的计划任务。"
        } else {
            "当前为普通权限，管理员启动选项仅管理员可以变更。\r\n请先点击下方按钮并同意 Windows UAC 授权。"
        },
        0,
        24,
        104,
        560,
        58,
    );
    let restart = modal_control(
        &modal,
        506,
        "BUTTON",
        "以管理员身份重启",
        WS_TABSTOP,
        24,
        172,
        220,
        34,
    );
    EnableWindow(restart, (!admin) as i32);
    modal_control(
        &modal,
        515,
        "BUTTON",
        "运行环境与依赖…",
        WS_TABSTOP,
        270,
        172,
        220,
        34,
    );
    modal_control(
        &modal,
        516,
        "BUTTON",
        "配置与依赖路径…",
        WS_TABSTOP,
        516,
        172,
        182,
        34,
    );
    modal_control(
        &modal,
        511,
        "STATIC",
        "重启会停止当前网关；自动运行的服务将按原配置重新启动。\r\n各服务自动运行开关决定启动哪些服务。移动目录后重新保存启动设置。",
        0,
        24,
        220,
        560,
        58,
    );
    modal_control(
        &modal,
        1,
        "BUTTON",
        "保存",
        WS_TABSTOP | BS_DEFPUSHBUTTON as u32,
        478,
        550,
        100,
        32,
    );
    modal_control(&modal, 2, "BUTTON", "取消", WS_TABSTOP, 598, 550, 100, 32);
    modal_control(
        &modal,
        514,
        "EDIT",
        certificates,
        ES_READONLY as u32 | ES_MULTILINE as u32 | WS_VSCROLL | WS_TABSTOP,
        24,
        286,
        674,
        190,
    );
    for (index, (id, label)) in [
        (507, "生成 / 检查"),
        (508, "导出 CA"),
        (509, "信任本机 CA"),
        (512, "取消信任"),
        (513, "重建 CA"),
    ]
    .iter()
    .enumerate()
    {
        modal_control(
            &modal,
            *id,
            "BUTTON",
            label,
            WS_TABSTOP,
            24 + index as i32 * 136,
            490,
            128,
            32,
        );
    }
    modal_loop(owner, modal.hwnd);
    if modal.result != Some(true) {
        return None;
    }
    if modal.values.first().is_some_and(|v| v == "restart") {
        return Some(SettingsAction::Restart);
    }
    if modal.values.len() == 1 {
        return modal.values[0]
            .parse()
            .ok()
            .map(SettingsAction::Certificate);
    }
    Some(SettingsAction::Save(
        modal.values[0] == "true",
        modal.values[1] == "true",
    ))
}
