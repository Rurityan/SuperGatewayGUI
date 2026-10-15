use super::*;
use gateway::dependencies::{self as deps, Network};
enum Update {
    Log(String),
    Complete(Result<Option<gateway::dependencies::ServiceDependency>>),
}
pub(super) struct Dialog {
    root: PathBuf,
    manager: Arc<Manager>,
    profiles: Vec<Profile>,
    network: Network,
    profile: Option<Profile>,
    arguments: Vec<String>,
    rx: Option<mpsc::Receiver<Update>>,
    worker: Option<thread::JoinHandle<()>>,
    logs: VecDeque<String>,
    saved: bool,
    layout: Vec<(HWND, i32, RECT)>,
    design_width: i32,
    scroll: i32,
    font: HFONT,
}
impl Drop for Dialog {
    fn drop(&mut self) {
        if !self.font.is_null() {
            unsafe {
                DeleteObject(self.font);
            }
        }
    }
}
unsafe fn layout(modal: &Modal) {
    let d = modal.dependency.as_ref().unwrap();
    let dpi = GetDpiForWindow(modal.hwnd) as i32;
    let mut client: RECT = zeroed();
    GetClientRect(modal.hwnd, &mut client);
    let width = client.right * 96 / dpi;
    let height = client.bottom * 96 / dpi;
    let max = (714 - (height - 58)).max(0);
    let scroll = d.scroll.min(max);
    let info = SCROLLINFO {
        cbSize: size_of::<SCROLLINFO>() as u32,
        fMask: SIF_RANGE | SIF_PAGE | SIF_POS,
        nMin: 0,
        nMax: 713,
        nPage: (height - 58).max(1) as u32,
        nPos: scroll,
        ..zeroed()
    };
    SetScrollInfo(modal.hwnd, SB_VERT, &info, 1);
    for (control, id, r) in &d.layout {
        let x = r.left * width / d.design_width;
        let y = if *id == 1 || *id == 2 {
            height - 44
        } else {
            r.top - scroll
        };
        SetWindowPos(
            *control,
            null_mut(),
            x * dpi / 96,
            y * dpi / 96,
            (r.right - r.left) * width / d.design_width * dpi / 96,
            (r.bottom - r.top) * dpi / 96,
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
unsafe fn scroll_to_focus(modal: &mut Modal, id: i32) {
    let dpi = GetDpiForWindow(modal.hwnd) as i32;
    let mut client: RECT = zeroed();
    GetClientRect(modal.hwnd, &mut client);
    let visible = client.bottom * 96 / dpi - 58;
    let d = modal.dependency.as_mut().unwrap();
    if let Some((_, _, r)) = d
        .layout
        .iter()
        .find(|(_, key, _)| *key == id && id != 1 && id != 2)
    {
        if r.top < d.scroll {
            d.scroll = r.top.max(0);
        } else if r.bottom > d.scroll + visible {
            d.scroll = (r.bottom - visible).max(0);
        }
    }
    layout(modal);
}
unsafe fn text(hwnd: HWND, id: i32) -> String {
    let control = GetDlgItem(hwnd, id);
    let mut value = vec![0u16; GetWindowTextLengthW(control) as usize + 1];
    let copied = GetWindowTextW(control, value.as_mut_ptr(), value.len() as i32);
    String::from_utf16_lossy(&value[..copied.max(0) as usize])
}
unsafe fn set(hwnd: HWND, id: i32, value: &str) {
    SetWindowTextW(GetDlgItem(hwnd, id), wide(value).as_ptr());
}
unsafe fn network(hwnd: HWND) -> Result<Network> {
    let n = Network {
        proxy: text(hwnd, 601).trim().into(),
        username: text(hwnd, 602),
        password: text(hwnd, 603),
        npm_registry: text(hwnd, 604).trim().into(),
        python_index: text(hwnd, 605).trim().into(),
    };
    n.validate()?;
    Ok(n)
}
unsafe fn spec(modal: &Modal, mut p: Profile) -> Result<Profile> {
    let hwnd = modal.hwnd;
    p.dependency.kind = if SendMessageW(GetDlgItem(hwnd, 620), CB_GETCURSEL, 0, 0) == 1 {
        "python"
    } else {
        "npm"
    }
    .into();
    p.dependency.package = text(hwnd, 621).trim().into();
    p.dependency.version = text(hwnd, 622).trim().into();
    p.dependency.entry = text(hwnd, 623).trim().into();
    p.dependency.args = modal.dependency.as_ref().unwrap().arguments.clone();
    p.managed = SendMessageW(GetDlgItem(hwnd, 625), BM_GETCHECK, 0, 0) == BST_CHECKED as isize;
    if p.managed && p.stdio.is_empty() {
        deps::validate_spec(&p.dependency).map_err(|error| {
            format!(
                "{error}（类型：{}，包名：{:?}）",
                p.dependency.kind, p.dependency.package
            )
        })?;
    }
    Ok(p)
}
unsafe fn busy(modal: &Modal, busy: bool) {
    for id in [
        601, 602, 603, 604, 605, 610, 611, 612, 613, 614, 620, 621, 622, 623, 624, 625, 626, 627,
        628, 631, 632, 633, 1,
    ] {
        EnableWindow(GetDlgItem(modal.hwnd, id), (!busy) as i32);
    }
    EnableWindow(GetDlgItem(modal.hwnd, 629), busy as i32);
}
unsafe fn refresh(modal: &Modal) {
    let d = modal.dependency.as_ref().unwrap();
    let profiles = if let Some(p) = &d.profile {
        let mut profiles = d.profiles.clone();
        profiles.retain(|v| v.id != p.id);
        profiles.push(p.clone());
        profiles
    } else {
        d.profiles.clone()
    };
    set(
        modal.hwnd,
        600,
        &deps::report(&d.root, &profiles, &d.manager.active_profiles()),
    );
}
unsafe fn launch(modal: &mut Modal, action: i32) -> Result<()> {
    let hwnd = modal.hwnd;
    let n = network(hwnd)?;
    let profile = modal
        .dependency
        .as_ref()
        .unwrap()
        .profile
        .clone()
        .map(|p| spec(modal, p))
        .transpose()?;
    let d = modal.dependency.as_mut().unwrap();
    if d.worker.is_some() {
        return Err("另一个操作正在进行".into());
    }
    d.manager.dependencies.prepare()?;
    d.profile = profile;
    let runtime_index = SendMessageW(GetDlgItem(hwnd, 610), CB_GETCURSEL, 0, 0).max(0) as usize;
    let r = deps::CATALOG.get(runtime_index).ok_or("请选择运行环境")?;
    let prompt = match action {
        611 => format!(
            "安装 {} {}？\r\n来源：{}\r\n目录：{}\r\n将校验固定 SHA256。",
            r.kind,
            r.version,
            r.url,
            deps::runtime(&d.root, r.kind, r.version)?.display()
        ),
        626 => {
            let p = d.profile.as_ref().ok_or("请从本地服务的依赖按钮进入")?;
            deps::validate_spec(&p.dependency)?;
            format!(
                "安装 {} 包 {} {}？\r\n启动入口：{}\r\n服务目录：{}\r\n包安装可能执行第三方代码。完成后需保存服务并应用重启。",
                p.dependency.kind,
                p.dependency.package,
                if p.dependency.version.is_empty() {
                    "（解析当前版本并固定）"
                } else {
                    &p.dependency.version
                },
                if p.dependency.entry.is_empty() {
                    "自动选择唯一入口"
                } else {
                    &p.dependency.entry
                },
                gateway::storage::dependency_directory(&d.root)?
                    .join("services")
                    .join(&p.id)
                    .display()
            )
        }
        612 => "删除所选运行环境？被配置或运行中服务引用时会拒绝。".into(),
        613 => "清理共享下载缓存？已安装的包目录会保留。".into(),
        628 => "删除此服务的所有依赖版本？运行中会拒绝，之后需要重新安装。".into(),
        _ => return Err("未知操作".into()),
    };
    if MessageBoxW(
        hwnd,
        wide(prompt).as_ptr(),
        wide("确认依赖操作").as_ptr(),
        MB_YESNO | MB_ICONWARNING,
    ) != IDYES
    {
        return Ok(());
    }
    d.network = n.clone();
    let root = d.root.clone();
    let manager = d.manager.clone();
    let p = d.profile.clone();
    let mut profiles = d.profiles.clone();
    if let Some(p) = &p {
        profiles.retain(|v| v.id != p.id);
        profiles.push(p.clone());
    }
    let (tx, rx) = mpsc::channel();
    d.rx = Some(rx);
    d.worker = Some(thread::spawn(move || {
        let progress = |line: String| {
            let _ = tx.send(Update::Log(line));
        };
        let result = manager.dependencies.execute(|| match action {
            611 => {
                deps::install_runtime(&root, r, &n, &manager.dependencies, &progress).map(|_| None)
            }
            612 => {
                deps::remove_runtime(&root, r, &profiles, &manager.active_profiles()).map(|_| None)
            }
            613 => deps::clear_cache(&root, &manager.active_profiles()).map(|_| None),
            626 => deps::install_service(
                &root,
                p.as_ref().unwrap(),
                &n,
                &manager.dependencies,
                &progress,
            )
            .map(Some),
            628 => deps::remove_service(&root, &p.as_ref().unwrap().id, &manager.active_profiles())
                .map(|_| None),
            _ => unreachable!(),
        });
        let _ = tx.send(Update::Complete(result));
    }));
    busy(modal, true);
    Ok(())
}
unsafe fn one_shot(modal: &Modal) -> Result<()> {
    let d = modal.dependency.as_ref().unwrap();
    let p = spec(modal, d.profile.clone().ok_or("请从服务的依赖管理进入")?)?;
    let n = network(modal.hwnd)?;
    // Check installation before opening an argument editor; no automatic install.
    gateway::oneshot::prepare(&d.root, &p, &n, vec![])?;
    let Some(args) = one_shot_arguments(modal.hwnd, d.font) else {
        return Ok(());
    };
    let invocation = gateway::oneshot::prepare(&d.root, &p, &n, args)?;
    let command = std::iter::once(native::quote(&invocation.executable.to_string_lossy()))
        .chain(invocation.args.iter().map(|arg| native::quote(arg)))
        .collect::<Vec<_>>()
        .join(" ");
    let mut description = format!(
        "程序与参数：\r\n{command}\r\n\r\n工作目录：{}\r\n\r\n使用已安装的固定版本，不临时下载 MCP 包。\r\n按当前网络设置运行；向导可能联网、修改其他客户端配置或安装插件。\r\n不会修改系统 PATH；第三方程序的行为由你在终端中确认。\r\n终端与 GUI 独立，关闭终端会清理它管理的命令进程树。\r\n服务启动参数保持不变。",
        invocation.directory.display()
    );
    for secret in p
        .secrets()
        .into_iter()
        .chain([n.password.clone(), n.username.clone()])
        .filter(|s| !s.is_empty())
    {
        description = description.replace(&secret, "[隐藏]");
    }
    if preview(
        modal.hwnd,
        d.font,
        "一次性命令预览",
        &description,
        Some("确认并打开终端"),
    ) {
        let pid = d.manager.launch_one_shot(&d.root, &invocation, &p)?;
        MessageBoxW(modal.hwnd, wide(format!("已打开交互终端（PID {pid}）。请在终端中完成向导。\r\n完成后关闭终端；正常启动 MCP 时不要将 setup 填入服务启动参数。")).as_ptr(),
            wide("一次性命令已启动").as_ptr(), MB_OK);
    }
    Ok(())
}
pub(super) unsafe fn procedure(modal: &mut Modal, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    match msg {
        WM_SIZE => {
            layout(modal);
            0
        }
        WM_GETMINMAXINFO => {
            let dpi = GetDpiForWindow(modal.hwnd) as i32;
            let info = &mut *(l as *mut MINMAXINFO);
            info.ptMinTrackSize.x = 720 * dpi / 96;
            info.ptMinTrackSize.y = 440 * dpi / 96;
            0
        }
        WM_VSCROLL | WM_MOUSEWHEEL => {
            let mut info = SCROLLINFO {
                cbSize: size_of::<SCROLLINFO>() as u32,
                fMask: SIF_ALL,
                ..zeroed()
            };
            GetScrollInfo(modal.hwnd, SB_VERT, &mut info);
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
            modal.dependency.as_mut().unwrap().scroll =
                next.clamp(0, (info.nMax - info.nPage as i32 + 1).max(0));
            layout(modal);
            0
        }
        WM_TIMER => {
            let updates = modal
                .dependency
                .as_ref()
                .unwrap()
                .rx
                .as_ref()
                .map(|rx| rx.try_iter().collect::<Vec<_>>())
                .unwrap_or_default();
            for update in updates {
                match update {
                    Update::Log(line) => {
                        let d = modal.dependency.as_mut().unwrap();
                        d.logs.push_back(line);
                        while d.logs.len() > 300
                            || d.logs.iter().map(String::len).sum::<usize>() > 65536
                        {
                            d.logs.pop_front();
                        }
                        set(
                            modal.hwnd,
                            630,
                            &d.logs.iter().cloned().collect::<Vec<_>>().join("\r\n"),
                        );
                        SendMessageW(GetDlgItem(modal.hwnd, 630), EM_SETSEL, usize::MAX, -1);
                        SendMessageW(GetDlgItem(modal.hwnd, 630), EM_SCROLLCARET, 0, 0);
                    }
                    Update::Complete(result) => {
                        let d = modal.dependency.as_mut().unwrap();
                        if let Some(worker) = d.worker.take() {
                            let _ = worker.join();
                        }
                        d.rx = None;
                        match result {
                            Ok(Some(spec)) => {
                                set(modal.hwnd, 622, &spec.version);
                                if let Some(p) = &mut d.profile {
                                    p.dependency = spec;
                                    p.managed = true;
                                    SendMessageW(
                                        GetDlgItem(modal.hwnd, 625),
                                        BM_SETCHECK,
                                        BST_CHECKED as usize,
                                        0,
                                    );
                                }
                            }
                            Ok(None) => {}
                            Err(error) => {
                                MessageBoxW(
                                    modal.hwnd,
                                    wide(error).as_ptr(),
                                    wide("依赖操作结果").as_ptr(),
                                    MB_ICONWARNING,
                                );
                            }
                        }
                        busy(modal, false);
                        if modal.dependency.as_ref().unwrap().profile.is_none() {
                            for id in (620..=628).chain([631, 632, 633]) {
                                EnableWindow(GetDlgItem(modal.hwnd, id), 0);
                            }
                        }
                        refresh(modal);
                    }
                }
            }
            0
        }
        WM_COMMAND => {
            let id = (w & 0xffff) as i32;
            if (w >> 16) as u32 == EN_SETFOCUS || (w >> 16) as u32 == CBN_SETFOCUS {
                scroll_to_focus(modal, id);
            }
            let result = (|| -> Result<()> {
                match id {
                    633 => {
                        let d = modal.dependency.as_ref().unwrap();
                        if d.worker.is_some() {
                            return Ok(());
                        }
                        let mut p = d.profile.clone().ok_or("请从本地服务进入")?;
                        // Unlike installation, AI must also work with an incomplete package form.
                        p.managed = SendMessageW(GetDlgItem(modal.hwnd, 625), BM_GETCHECK, 0, 0)
                            == BST_CHECKED as isize;
                        p.dependency.kind =
                            if SendMessageW(GetDlgItem(modal.hwnd, 620), CB_GETCURSEL, 0, 0) == 1 {
                                "python"
                            } else {
                                "npm"
                            }
                            .into();
                        p.dependency.package = text(modal.hwnd, 621).trim().into();
                        p.dependency.version = text(modal.hwnd, 622).trim().into();
                        p.dependency.entry = text(modal.hwnd, 623).trim().into();
                        p.dependency.args = d.arguments.clone();
                        let result = ai_ui::show(
                            modal.hwnd,
                            modal.font,
                            d.root.clone(),
                            network(modal.hwnd)?,
                            Some(p),
                        )?;
                        if let Some(p) = result {
                            SendMessageW(
                                GetDlgItem(modal.hwnd, 620),
                                CB_SETCURSEL,
                                usize::from(p.dependency.kind == "python"),
                                0,
                            );
                            SendMessageW(
                                GetDlgItem(modal.hwnd, 625),
                                BM_SETCHECK,
                                if p.managed {
                                    BST_CHECKED
                                } else {
                                    BST_UNCHECKED
                                } as usize,
                                0,
                            );
                            set(modal.hwnd, 621, &p.dependency.package);
                            set(modal.hwnd, 622, &p.dependency.version);
                            set(modal.hwnd, 623, &p.dependency.entry);
                            set(modal.hwnd, 624, &argument_ui::summary(&p.dependency.args));
                            let d = modal.dependency.as_mut().unwrap();
                            d.arguments = p.dependency.args.clone();
                            d.profile = Some(p);
                        }
                    }
                    611 | 612 | 613 | 626 | 628 => launch(modal, id)?,
                    631 => one_shot(modal)?,
                    632 => {
                        let d = modal.dependency.as_ref().unwrap();
                        if d.profile.is_none() || d.worker.is_some() {
                            return Ok(());
                        }
                        if let Some(args) = argument_ui::show(
                            modal.hwnd,
                            d.font,
                            "编辑托管包启动参数",
                            &d.arguments,
                            false,
                        ) {
                            set(modal.hwnd, 624, &argument_ui::summary(&args));
                            modal.dependency.as_mut().unwrap().arguments = args;
                        }
                    }
                    629 => modal
                        .dependency
                        .as_ref()
                        .unwrap()
                        .manager
                        .dependencies
                        .cancel(),
                    627 => {
                        let d = modal.dependency.as_mut().unwrap();
                        if let Some(p) = &d.profile {
                            if let Some(spec) = deps::infer(p) {
                                let preview = format!(
                                    "识别为 {}：{} {}\r\n将保留原始命令，仅填写依赖表单；不会安装。\r\n其余参数会填入服务启动参数。setup / install-addon 等一次性步骤请改用“一次性命令”入口，勿作为常驻服务参数。",
                                    spec.kind, spec.package, spec.version
                                );
                                if MessageBoxW(
                                    modal.hwnd,
                                    wide(preview).as_ptr(),
                                    wide("转换预览").as_ptr(),
                                    MB_YESNO | MB_ICONQUESTION,
                                ) == IDYES
                                {
                                    SendMessageW(
                                        GetDlgItem(modal.hwnd, 620),
                                        CB_SETCURSEL,
                                        usize::from(spec.kind == "python"),
                                        0,
                                    );
                                    set(modal.hwnd, 621, &spec.package);
                                    set(modal.hwnd, 622, &spec.version);
                                    set(modal.hwnd, 623, &spec.entry);
                                    set(modal.hwnd, 624, &argument_ui::summary(&spec.args));
                                    d.arguments = spec.args.clone();
                                }
                            } else {
                                return Err("无法安全识别此命令，请手动填写包、版本和启动入口；原始命令不会执行。".into());
                            }
                        }
                    }
                    614 | 1 => {
                        let n = network(modal.hwnd)?;
                        let profile = modal
                            .dependency
                            .as_ref()
                            .unwrap()
                            .profile
                            .clone()
                            .map(|p| spec(modal, p))
                            .transpose()?;
                        let d = modal.dependency.as_mut().unwrap();
                        d.profile = profile;
                        d.network = n;
                        d.saved = true;
                        if id == 1 {
                            modal.result = Some(true);
                            KillTimer(modal.hwnd, 77);
                            DestroyWindow(modal.hwnd);
                        } else {
                            MessageBoxW(
                                modal.hwnd,
                                wide(
                                    "网络设置已确认，将在关闭窗口后保存。安装任务使用当前表单值。",
                                )
                                .as_ptr(),
                                wide("网络设置").as_ptr(),
                                MB_OK,
                            );
                        }
                    }
                    2 => {
                        if modal.dependency.as_ref().unwrap().worker.is_some() {
                            modal
                                .dependency
                                .as_ref()
                                .unwrap()
                                .manager
                                .dependencies
                                .cancel();
                            return Err("正在取消操作，请等待完成后关闭窗口".into());
                        }
                        modal.result = Some(false);
                        KillTimer(modal.hwnd, 77);
                        DestroyWindow(modal.hwnd);
                    }
                    606 => {
                        let show = SendMessageW(GetDlgItem(modal.hwnd, 606), BM_GETCHECK, 0, 0)
                            == BST_CHECKED as isize;
                        SendMessageW(
                            GetDlgItem(modal.hwnd, 603),
                            EM_SETPASSWORDCHAR,
                            if show { 0 } else { '●' as usize },
                            0,
                        );
                        InvalidateRect(GetDlgItem(modal.hwnd, 603), null(), 1);
                    }
                    _ => {}
                }
                Ok(())
            })();
            if let Err(error) = result {
                MessageBoxW(
                    modal.hwnd,
                    wide(error).as_ptr(),
                    wide("运行环境与依赖").as_ptr(),
                    MB_ICONWARNING,
                );
            }
            0
        }
        WM_CLOSE => {
            SendMessageW(modal.hwnd, WM_COMMAND, 2, 0);
            0
        }
        WM_DPICHANGED => {
            if let Ok(icons) = WindowIcons::load((w & 0xffff) as u32) {
                icons.apply(modal.hwnd);
                modal.icons = Some(icons);
            }
            let r = &*(l as *const RECT);
            SetWindowPos(
                modal.hwnd,
                null_mut(),
                r.left,
                r.top,
                r.right - r.left,
                r.bottom - r.top,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
            let dpi = GetDpiForWindow(modal.hwnd) as i32;
            let font = CreateFontW(
                -16 * dpi / 96,
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
            if !font.is_null() {
                let d = modal.dependency.as_mut().unwrap();
                for (control, _, _) in &d.layout {
                    SendMessageW(*control, WM_SETFONT, font as usize, 0);
                }
                if !d.font.is_null() {
                    DeleteObject(d.font);
                }
                d.font = font;
            }
            layout(modal);
            0
        }
        _ => DefWindowProcW(modal.hwnd, msg, w, l),
    }
}
pub(super) unsafe fn show(
    owner: HWND,
    font: HFONT,
    root: PathBuf,
    manager: Arc<Manager>,
    profiles: Vec<Profile>,
    network: Network,
    profile: Option<Profile>,
) -> (Option<Network>, Option<Profile>) {
    let mut modal = modal_window(
        owner,
        font,
        "运行环境与 MCP 依赖 · 不修改系统环境",
        980,
        800,
        false,
        false,
        false,
    );
    modal_control(
        &modal,
        600,
        "EDIT",
        "",
        ES_READONLY as u32 | ES_MULTILINE as u32 | WS_VSCROLL | WS_TABSTOP,
        18,
        18,
        455,
        274,
    );
    for (id, label, value, y, password) in [
        (
            601,
            "HTTP 代理（留空直连）",
            network.proxy.as_str(),
            18,
            false,
        ),
        (602, "代理用户名", network.username.as_str(), 65, false),
        (603, "代理密码", network.password.as_str(), 112, true),
        (604, "npm 包源", network.npm_registry.as_str(), 159, false),
        (
            605,
            "Python 包源",
            network.python_index.as_str(),
            206,
            false,
        ),
    ] {
        modal_control(&modal, id + 100, "STATIC", label, 0, 490, y, 220, 20);
        modal_control(
            &modal,
            id,
            "EDIT",
            value,
            WS_TABSTOP | ES_AUTOHSCROLL as u32 | if password { ES_PASSWORD as u32 } else { 0 },
            490,
            y + 22,
            445,
            24,
        );
    }
    modal_control(
        &modal,
        606,
        "BUTTON",
        "显示密码",
        WS_TABSTOP | BS_AUTOCHECKBOX as u32,
        490,
        254,
        110,
        26,
    );
    modal_control(
        &modal,
        614,
        "BUTTON",
        "确认网络设置",
        WS_TABSTOP,
        730,
        254,
        205,
        30,
    );
    let runtime = modal_control(
        &modal,
        610,
        "COMBOBOX",
        "",
        CBS_DROPDOWNLIST as u32 | WS_TABSTOP | WS_VSCROLL,
        18,
        304,
        455,
        180,
    );
    for r in deps::CATALOG {
        SendMessageW(
            runtime,
            CB_ADDSTRING,
            0,
            wide(format!("{} {} · Windows x64", r.kind, r.version)).as_ptr() as isize,
        );
    }
    SendMessageW(runtime, CB_SETCURSEL, 0, 0);
    for (id, label, x, width) in [
        (611, "安装所选环境", 18, 160),
        (612, "删除所选环境", 190, 160),
        (613, "清理缓存", 362, 110),
    ] {
        modal_control(&modal, id, "BUTTON", label, WS_TABSTOP, x, 346, width, 30);
    }
    modal_control(
        &modal,
        607,
        "STATIC",
        "可先勾选应用管理、填写包名并保存，无需安装。\r\n安装时：npm → Node.js；Python → uv + Python。\r\nsetup 请在安装后通过一次性命令运行。",
        0,
        490,
        300,
        445,
        66,
    );
    let combo = modal_control(
        &modal,
        620,
        "COMBOBOX",
        "",
        CBS_DROPDOWNLIST as u32 | WS_TABSTOP,
        18,
        414,
        160,
        140,
    );
    for label in ["npm 包", "Python 包"] {
        SendMessageW(combo, CB_ADDSTRING, 0, wide(label).as_ptr() as isize);
    }
    let d = profile
        .as_ref()
        .map(|p| p.dependency.clone())
        .unwrap_or_default();
    SendMessageW(combo, CB_SETCURSEL, usize::from(d.kind == "python"), 0);
    for (id, label, value, x, width) in [
        (621, "包名称", d.package.as_str(), 190, 290),
        (622, "版本（空=当前）", d.version.as_str(), 490, 165),
        (623, "入口（空=唯一入口）", d.entry.as_str(), 670, 265),
    ] {
        modal_control(&modal, id + 100, "STATIC", label, 0, x, 389, width, 20);
        modal_control(
            &modal,
            id,
            "EDIT",
            value,
            WS_TABSTOP | ES_AUTOHSCROLL as u32,
            x,
            414,
            width,
            26,
        );
    }
    modal_control(
        &modal,
        608,
        "STATIC",
        profile
            .as_ref()
            .map(|p| p.name.as_str())
            .unwrap_or("服务依赖请从本地服务的“管理此服务依赖”进入。"),
        0,
        18,
        383,
        455,
        23,
    );
    let managed = modal_control(
        &modal,
        625,
        "BUTTON",
        "使用应用管理的运行环境",
        WS_TABSTOP | BS_AUTOCHECKBOX as u32,
        18,
        454,
        270,
        28,
    );
    modal_control(
        &modal,
        633,
        "BUTTON",
        "AI 安装向导…",
        WS_TABSTOP,
        300,
        454,
        180,
        30,
    );
    SendMessageW(
        managed,
        BM_SETCHECK,
        if profile.as_ref().is_some_and(|p| p.managed) {
            BST_CHECKED
        } else {
            BST_UNCHECKED
        } as usize,
        0,
    );
    modal_control(
        &modal,
        624,
        "EDIT",
        &argument_ui::summary(&d.args),
        WS_TABSTOP | ES_MULTILINE as u32 | ES_READONLY as u32 | WS_VSCROLL,
        490,
        454,
        445,
        86,
    );
    modal_control(
        &modal,
        632,
        "BUTTON",
        "编辑启动参数…",
        WS_TABSTOP,
        720,
        544,
        215,
        30,
    );
    modal_control(
        &modal,
        631,
        "BUTTON",
        "一次性命令 / 安装向导…",
        WS_TABSTOP,
        490,
        544,
        220,
        30,
    );
    for (id, label, x, width) in [
        (626, "安装 / 更新服务依赖", 18, 210),
        (627, "从原始命令识别", 240, 220),
        (628, "删除服务依赖", 18, 210),
        (629, "取消当前操作", 240, 220),
    ] {
        modal_control(
            &modal,
            id,
            "BUTTON",
            label,
            WS_TABSTOP,
            x,
            if id < 628 { 493 } else { 535 },
            width,
            32,
        );
    }
    modal_control(
        &modal,
        630,
        "EDIT",
        "安装日志（密钥隐藏，限制容量）",
        WS_TABSTOP | ES_READONLY as u32 | ES_MULTILINE as u32 | WS_VSCROLL,
        18,
        580,
        917,
        126,
    );
    modal_control(
        &modal,
        1,
        "BUTTON",
        "保存并关闭",
        WS_TABSTOP | BS_DEFPUSHBUTTON as u32,
        674,
        718,
        125,
        30,
    );
    modal_control(&modal, 2, "BUTTON", "关闭", WS_TABSTOP, 812, 718, 123, 30);
    modal.dependency = Some(Box::new(Dialog {
        root,
        manager,
        profiles,
        network,
        profile,
        arguments: d.args.clone(),
        rx: None,
        worker: None,
        logs: VecDeque::new(),
        saved: false,
        layout: vec![],
        design_width: 1,
        scroll: 0,
        font: null_mut(),
    }));
    let dpi = GetDpiForWindow(modal.hwnd) as i32;
    let mut client: RECT = zeroed();
    GetClientRect(modal.hwnd, &mut client);
    modal.dependency.as_mut().unwrap().design_width = client.right * 96 / dpi;
    let mut control = GetWindow(modal.hwnd, GW_CHILD);
    while !control.is_null() {
        let mut r: RECT = zeroed();
        GetWindowRect(control, &mut r);
        let mut top = POINT {
            x: r.left,
            y: r.top,
        };
        ScreenToClient(modal.hwnd, &mut top);
        r = RECT {
            left: top.x * 96 / dpi,
            top: top.y * 96 / dpi,
            right: (top.x + r.right - r.left) * 96 / dpi,
            bottom: (top.y + r.bottom - r.top) * 96 / dpi,
        };
        modal
            .dependency
            .as_mut()
            .unwrap()
            .layout
            .push((control, GetDlgCtrlID(control), r));
        SetWindowLongPtrW(
            control,
            GWL_STYLE,
            GetWindowLongPtrW(control, GWL_STYLE) | WS_CLIPSIBLINGS as isize,
        );
        control = GetWindow(control, GW_HWNDNEXT);
    }
    SetWindowLongPtrW(
        modal.hwnd,
        GWL_STYLE,
        GetWindowLongPtrW(modal.hwnd, GWL_STYLE)
            | (WS_THICKFRAME | WS_VSCROLL | WS_CLIPCHILDREN) as isize,
    );
    let monitor = MonitorFromWindow(owner, MONITOR_DEFAULTTONEAREST);
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..zeroed()
    };
    GetMonitorInfoW(monitor, &mut info);
    let mut r: RECT = zeroed();
    GetWindowRect(modal.hwnd, &mut r);
    let width = (r.right - r.left).min(info.rcWork.right - info.rcWork.left);
    let height = (r.bottom - r.top).min(info.rcWork.bottom - info.rcWork.top);
    let x = r.left.clamp(
        info.rcWork.left,
        (info.rcWork.right - width).max(info.rcWork.left),
    );
    let y = r.top.clamp(
        info.rcWork.top,
        (info.rcWork.bottom - height).max(info.rcWork.top),
    );
    SetWindowPos(
        modal.hwnd,
        null_mut(),
        x,
        y,
        width,
        height,
        SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
    );
    layout(&modal);
    EnableWindow(GetDlgItem(modal.hwnd, 629), 0);
    if modal.dependency.as_ref().unwrap().profile.is_none() {
        for id in (620..=628).chain([631, 632, 633]) {
            EnableWindow(GetDlgItem(modal.hwnd, id), 0);
        }
    }
    refresh(&modal);
    SetTimer(modal.hwnd, 77, 100, None);
    modal_loop(owner, modal.hwnd);
    let d = modal.dependency.take().unwrap();
    (
        if d.saved {
            Some(d.network.clone())
        } else {
            None
        },
        if modal.result == Some(true) {
            d.profile.clone()
        } else {
            None
        },
    )
}
