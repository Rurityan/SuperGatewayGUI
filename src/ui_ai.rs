//! Native, advisory-only AI wizard; all network work stays off the UI thread.
use super::*;
use gateway::ai::{self, AiSettings, PackageCheck, Proposal, Selection};
use gateway::dependencies::{Network, Task};

enum Update {
    Documents(Result<String>),
    Test(Result<String>),
    Proposal(Box<Result<(Proposal, PackageCheck)>>),
}
pub(super) struct Dialog {
    root: PathBuf,
    profile: Option<Profile>,
    output: Option<Profile>,
    network: Network,
    stage: usize,
    context: serde_json::Value,
    proposal: Option<Proposal>,
    check: PackageCheck,
    variables: Vec<Pair>,
    rx: Option<mpsc::Receiver<Update>>,
    worker: Option<thread::JoinHandle<()>>,
    task: Arc<Task>,
    closing: bool,
    font: HFONT,
}
impl Drop for Dialog {
    fn drop(&mut self) {
        self.task.cancel();
        if let Some(w) = self.worker.take() {
            let _ = w.join();
        }
        if !self.font.is_null() {
            unsafe {
                DeleteObject(self.font);
            }
        }
    }
}
unsafe fn text(m: &Modal, id: i32) -> String {
    let h = GetDlgItem(m.hwnd, id);
    let mut v = vec![0u16; GetWindowTextLengthW(h) as usize + 1];
    let n = GetWindowTextW(h, v.as_mut_ptr(), v.len() as i32);
    String::from_utf16_lossy(&v[..n.max(0) as usize])
}
unsafe fn set(m: &Modal, id: i32, value: &str) {
    let value = if id == 816 || id == 818 || (801..=809).contains(&id) {
        std::borrow::Cow::Borrowed(value)
    } else {
        i18n::text(value)
    };
    SetWindowTextW(
        GetDlgItem(m.hwnd, id),
        wide(value.replace('\n', "\r\n").replace("\r\r\n", "\r\n")).as_ptr(),
    );
}
unsafe fn config(m: &Modal) -> Result<AiSettings> {
    let c = AiSettings {
        base_url: text(m, 801).trim().into(),
        api_key: text(m, 802),
        model: text(m, 803).trim().into(),
    };
    c.endpoint()?;
    Ok(c)
}
fn stage_of(id: i32) -> Option<usize> {
    match id {
        801..=819 => Some(0),
        820..=829 => Some(1),
        830..=839 => Some(2),
        _ => None,
    }
}
unsafe fn layout(m: &Modal) {
    let d = m.ai.as_ref().unwrap();
    let dpi = GetDpiForWindow(m.hwnd) as i32;
    let mut r: RECT = zeroed();
    GetClientRect(m.hwnd, &mut r);
    let w = r.right * 96 / dpi;
    let h = r.bottom * 96 / dpi;
    let place = |id, x, y, width, height| {
        SetWindowPos(
            GetDlgItem(m.hwnd, id),
            null_mut(),
            x * dpi / 96,
            y * dpi / 96,
            width * dpi / 96,
            height * dpi / 96,
            SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOREDRAW,
        );
    };
    for (id, x) in [(790, 18), (791, 200), (792, 382)] {
        place(id, x, 12, 172, 30);
        EnableWindow(
            GetDlgItem(m.hwnd, id),
            (d.worker.is_none() && (id == 790 || d.proposal.is_some())) as i32,
        );
    }
    place(800, 18, h - 82, w - 36, 28);
    place(2, w - 128, h - 44, 108, 30);
    place(793, 18, h - 44, 110, 30);
    for id in (801..=819).chain(820..=829).chain(830..=839) {
        if let Some(stage) = stage_of(id) {
            ShowWindow(
                GetDlgItem(m.hwnd, id),
                if stage == d.stage { SW_SHOWNA } else { SW_HIDE },
            );
        }
    }
    for (id, x, y, width, height) in [
        (811, 18, 54, 86, 24),
        (801, 108, 50, w - 392, 28),
        (812, w - 270, 54, 55, 24),
        (803, w - 212, 50, 194, 28),
        (813, 18, 90, 86, 24),
        (802, 108, 86, w - 392, 28),
        (804, w - 270, 86, 110, 28),
        (805, w - 150, 86, 132, 28),
        (806, 18, 122, 140, 28),
        (807, 170, 122, w - 188, 24),
        (814, 18, 164, 86, 24),
        (808, 108, 160, w - 470, 28),
        (809, w - 350, 160, 160, 28),
        (810, w - 180, 160, 162, 28),
        (815, 18, 198, w - 36, 24),
        (816, 18, 228, w - 36, h - 452),
        (817, 18, h - 212, 160, 24),
        (818, 18, h - 184, w - 36, 54),
        (819, 18, h - 120, 180, 30),
        (820, 18, 54, w - 36, 38),
        (821, 18, 98, w - 36, h - 260),
        (822, 18, h - 152, 170, 30),
        (823, 202, h - 152, 230, 30),
        (824, 18, h - 110, w - 36, 24),
        (830, 18, 54, w - 36, 54),
        (831, 18, 112, w - 36, h - 294),
        (832, 18, h - 172, 200, 30),
        (833, 232, h - 172, 210, 30),
        (834, 18, h - 130, w - 36, 40),
    ] {
        place(id, x, y, width, height);
    }
    for (i, width) in [210, 270, 560].iter().enumerate() {
        SendMessageW(
            GetDlgItem(m.hwnd, 831),
            LVM_SETCOLUMNWIDTH,
            i,
            (width * dpi / 96) as isize,
        );
    }
    let busy = d.worker.is_some();
    for id in (801..=806).chain([808, 809, 810, 816, 818, 819, 822, 823, 831, 832, 833]) {
        EnableWindow(GetDlgItem(m.hwnd, id), (!busy) as i32);
    }
    EnableWindow(GetDlgItem(m.hwnd, 793), busy as i32);
    EnableWindow(
        GetDlgItem(m.hwnd, 833),
        (!busy
            && d.profile.is_some()
            && d.proposal.as_ref().is_some_and(|p| p.kind != "unsupported")
            && d.check != PackageCheck::Missing) as i32,
    );
    RedrawWindow(
        m.hwnd,
        null(),
        null_mut(),
        RDW_INVALIDATE | RDW_ERASE | RDW_ALLCHILDREN | RDW_UPDATENOW,
    );
}
unsafe fn start(m: &mut Modal, action: i32) -> Result<()> {
    let c = if action == 810 {
        AiSettings::default()
    } else {
        config(m)?
    };
    let d = m.ai.as_ref().unwrap();
    if d.worker.is_some() {
        return Err("操作正在进行".into());
    }
    let n = d.network.clone();
    let task = d.task.clone();
    task.prepare()?;
    let source = text(m, 808);
    let reference = text(m, 809);
    let requirements = text(m, 818);
    let documents = text(m, 816);
    let messages = if action == 819 {
        Some(ai::messages(&d.context, &documents, &requirements)?)
    } else {
        None
    };
    if let Some(messages) = &messages {
        let value = serde_json::to_string_pretty(messages).map_err(|_| "无法预览请求")?;
        if !preview(
            m.hwnd,
            m.font,
            "发送内容预览 · 不含 API Key",
            &value,
            Some("确认发送"),
        ) {
            return Ok(());
        }
    }
    let (tx, rx) = mpsc::channel();
    let worker = thread::spawn(move || {
        let update = if action == 810 {
            Update::Documents(
                ai::documents(&source, &reference, &n, &task).and_then(|docs| {
                    let content = docs
                        .iter()
                        .map(|d| format!("来源：{}\n{}", d.url, d.text))
                        .collect::<Vec<_>>()
                        .join("\n\n");
                    if content.len() > ai::DOCUMENT_LIMIT {
                        Err("资料合计超过 256 KiB，请粘贴精简内容".into())
                    } else {
                        Ok(content)
                    }
                }),
            )
        } else if action == 805 {
            Update::Test(
                ai::completion(
                    &c,
                    &n,
                    &task,
                    serde_json::json!([{"role":"user","content":"Reply OK."}]),
                )
                .map(|_| "连接成功，模型返回有效文本；此测试会产生一次 API 请求。".into()),
            )
        } else {
            Update::Proposal(Box::new((|| {
                let text = ai::completion(&c, &n, &task, messages.unwrap())?;
                let p = Proposal::parse(&text)?;
                let check = ai::check_package(&p, &n, &task)?;
                Ok((p, check))
            })()))
        };
        let _ = tx.send(update);
    });
    let d = m.ai.as_mut().unwrap();
    d.rx = Some(rx);
    d.worker = Some(worker);
    set(m, 800, "处理中… 可取消，不会执行安装或命令");
    layout(m);
    Ok(())
}
unsafe fn list_text(list: HWND, index: usize, column: i32, value: &str) {
    let mut s = wide(value);
    let item = LVITEMW {
        iSubItem: column,
        pszText: s.as_mut_ptr(),
        ..zeroed()
    };
    SendMessageW(list, LVM_SETITEMTEXTW, index, &item as *const _ as isize);
}
unsafe fn review(m: &mut Modal) {
    let d = m.ai.as_mut().unwrap();
    let Some(p) = &d.proposal else {
        return;
    };
    let Some(old) = &d.profile else {
        return;
    };
    let list = GetDlgItem(m.hwnd, 831);
    SendMessageW(list, LVM_DELETEALLITEMS, 0, 0);
    d.variables = p
        .environment
        .iter()
        .map(|v| {
            old.env
                .iter()
                .find(|e| e.key.eq_ignore_ascii_case(&v.name))
                .cloned()
                .unwrap_or(Pair {
                    key: v.name.clone(),
                    value: String::new(),
                    hidden: v.secret,
                })
        })
        .collect();
    let mut rows = vec![
        (
            "启动方式 / 外部程序".to_string(),
            if old.managed {
                i18n::text("应用管理").into_owned()
            } else {
                old.command.clone()
            },
            if p.kind == "external" {
                p.command.clone()
            } else {
                i18n::text(&format!("应用管理 {}（保留完整命令）", p.kind)).into_owned()
            },
        ),
        (
            "包类型 / 包名 / 版本 / 入口".into(),
            format!(
                "{} / {} / {} / {}",
                old.dependency.kind,
                old.dependency.package,
                old.dependency.version,
                old.dependency.entry
            ),
            format!("{} / {} / {} / {}", p.kind, p.package, p.version, p.entry),
        ),
        (
            "常驻启动参数".into(),
            argument_ui::summary(if old.managed {
                &old.dependency.args
            } else {
                &old.args
            }),
            argument_ui::summary(
                &p.arguments
                    .iter()
                    .map(|a| a.value.clone())
                    .collect::<Vec<_>>(),
            ),
        ),
    ];
    for (v, pair) in p.environment.iter().zip(&d.variables) {
        rows.push((
            format!(
                "环境 {}{}",
                v.name,
                if v.required { "（必填）" } else { "" }
            ),
            if old.env.iter().any(|e| e.key.eq_ignore_ascii_case(&v.name)) {
                i18n::text("已有值（不显示）").into_owned()
            } else {
                i18n::text("未设置").into_owned()
            },
            if pair.value.is_empty() {
                i18n::text(&format!("待填写：{}", v.description)).into_owned()
            } else {
                i18n::text("沿用已有值（不显示）").into_owned()
            },
        ));
    }
    for (i, (field, old, new)) in rows.iter().enumerate() {
        let mut s = localized_wide(field);
        let item = LVITEMW {
            mask: LVIF_TEXT,
            iItem: i as i32,
            pszText: s.as_mut_ptr(),
            ..zeroed()
        };
        SendMessageW(list, LVM_INSERTITEMW, 0, &item as *const _ as isize);
        let checked = i < 3;
        let state = LVITEMW {
            stateMask: LVIS_STATEIMAGEMASK,
            state: if checked { 2 << 12 } else { 1 << 12 },
            ..zeroed()
        };
        SendMessageW(list, LVM_SETITEMSTATE, i, &state as *const _ as isize);
        list_text(list, i, 1, old);
        list_text(list, i, 2, new);
    }
    let label = d.check.label();
    set(
        m,
        830,
        &format!("{label}\r\n勾选字段后填入草稿；环境变量需选中行并填写实际值。"),
    );
}
fn explanation(p: &Proposal, check: &PackageCheck) -> String {
    let mut s = i18n::text(&format!(
        "{}\r\n\r\n{}\r\n\r\n安装：{} / {} / {}\r\n入口：{}{}\r\n\r\n常驻参数：\r\n",
        i18n::text(&check.label()),
        p.explanation,
        p.kind,
        p.package,
        if p.version.is_empty() {
            "安装时解析并固定"
        } else {
            &p.version
        },
        p.command,
        p.entry
    ))
    .into_owned();
    for a in &p.arguments {
        s += &format!("{:?}：{}\r\n", a.value, a.description);
    }
    s += &i18n::text("\r\n环境变量（值只在本机填写）：\r\n");
    for e in &p.environment {
        s += &format!(
            "{} [{}]：{}\r\n",
            e.name,
            i18n::text(if e.required { "必填" } else { "可选" }),
            e.description
        );
    }
    s += &i18n::text("\r\n运行条件：\r\n");
    for r in &p.prerequisites {
        s += &format!("• {r}\r\n");
    }
    s += &i18n::text("\r\n独立一次性步骤（不作为常驻参数；安装后手动运行）：\r\n");
    for (i, step) in p.setup.iter().enumerate() {
        s += &i18n::text(&format!(
            "{}. {}\r\n参数数组：{}\r\n",
            i + 1,
            step.description,
            serde_json::to_string(&step.args).unwrap_or_default()
        ));
    }
    s += &i18n::text("\r\n不确定项：\r\n");
    for r in &p.uncertainties {
        s += &format!("• {r}\r\n");
    }
    s += &i18n::text("\r\n资料来源（模型声明，请核对）：\r\n");
    for r in &p.sources {
        s += &format!("• {r}\r\n");
    }
    s
}
pub(super) unsafe fn procedure(m: &mut Modal, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    let result = (|| -> Result<()> {
        match msg {
            WM_TIMER => {
                let updates =
                    m.ai.as_ref()
                        .unwrap()
                        .rx
                        .as_ref()
                        .map(|rx| rx.try_iter().collect::<Vec<_>>())
                        .unwrap_or_default();
                if updates.is_empty() {
                    return Ok(());
                }
                for update in updates {
                    let d = m.ai.as_mut().unwrap();
                    if let Some(worker) = d.worker.take() {
                        let _ = worker.join();
                    }
                    d.rx = None;
                    if d.closing {
                        KillTimer(m.hwnd, 78);
                        DestroyWindow(m.hwnd);
                        return Ok(());
                    }
                    match update {
                        Update::Documents(r) => {
                            set(m, 816, &r?);
                            set(m, 800, "资料已读取，可编辑后生成方案；不会安装。");
                        }
                        Update::Test(r) => {
                            set(m, 800, &r?);
                        }
                        Update::Proposal(r) => {
                            let (p, check) = (*r)?;
                            set(m, 821, &explanation(&p, &check));
                            let d = m.ai.as_mut().unwrap();
                            d.proposal = Some(p);
                            d.check = check;
                            d.stage = 1;
                            review(m);
                            set(
                                m,
                                800,
                                "方案已生成，请核对来源与参数；可返回资料页补充要求后重新生成。",
                            );
                        }
                    }
                }
                layout(m);
            }
            WM_CLOSE => close(m),
            WM_SIZE => layout(m),
            WM_GETMINMAXINFO => {
                let dpi = GetDpiForWindow(m.hwnd) as i32;
                let i = &mut *(l as *mut MINMAXINFO);
                i.ptMinTrackSize.x = 860 * dpi / 96;
                i.ptMinTrackSize.y = 680 * dpi / 96;
            }
            WM_DPICHANGED => {
                let r = &*(l as *const RECT);
                SetWindowPos(
                    m.hwnd,
                    null_mut(),
                    r.left,
                    r.top,
                    r.right - r.left,
                    r.bottom - r.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
                if let Ok(icons) = WindowIcons::load((w & 0xffff) as u32) {
                    icons.apply(m.hwnd);
                    m.icons = Some(icons);
                }
                let dpi = GetDpiForWindow(m.hwnd) as i32;
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
                    0,
                    0,
                    wide("Microsoft YaHei UI").as_ptr(),
                );
                if !font.is_null() {
                    let d = m.ai.as_mut().unwrap();
                    let old = d.font;
                    d.font = font;
                    m.font = font;
                    let mut c = GetWindow(m.hwnd, GW_CHILD);
                    while !c.is_null() {
                        SendMessageW(c, WM_SETFONT, font as usize, 1);
                        c = GetWindow(c, GW_HWNDNEXT);
                    }
                    if !old.is_null() {
                        DeleteObject(old);
                    }
                }
                layout(m);
            }
            WM_COMMAND => {
                let id = (w & 0xffff) as i32;
                if (w >> 16) != 0 && id != 804 {
                    return Ok(());
                }
                if m.ai.as_ref().unwrap().worker.is_some() && ![2, 793].contains(&id) {
                    return Ok(());
                }
                match id {
                    790..=792 => {
                        let stage = (id - 790) as usize;
                        if stage == 0 || m.ai.as_ref().unwrap().proposal.is_some() {
                            m.ai.as_mut().unwrap().stage = stage;
                            layout(m);
                        }
                    }
                    804 => {
                        let show = SendMessageW(GetDlgItem(m.hwnd, 804), BM_GETCHECK, 0, 0)
                            == BST_CHECKED as isize;
                        SendMessageW(
                            GetDlgItem(m.hwnd, 802),
                            EM_SETPASSWORDCHAR,
                            if show { 0 } else { '●' as usize },
                            0,
                        );
                        InvalidateRect(GetDlgItem(m.hwnd, 802), null(), 1);
                    }
                    805 | 810 | 819 => start(m, id)?,
                    806 => {
                        let c = config(m)?;
                        let store = Store {
                            root: m.ai.as_ref().unwrap().root.clone(),
                        };
                        let mut settings = store.load()?;
                        settings.ai = c;
                        store.save(&settings)?;
                        set(m, 800, "AI 设置已加密保存；服务配置尚未保存。");
                    }
                    822 => {
                        m.ai.as_mut().unwrap().stage = 2;
                        layout(m);
                    }
                    823 => {
                        let value = serde_json::to_string_pretty(
                            &m.ai.as_ref().unwrap().proposal.as_ref().unwrap().setup,
                        )
                        .map_err(|_| "步骤无效")?;
                        preview(m.hwnd, m.font, "一次性步骤 · 安装后手动运行", &value, None);
                    }
                    832 => {
                        let index = SendMessageW(
                            GetDlgItem(m.hwnd, 831),
                            LVM_GETNEXTITEM,
                            usize::MAX,
                            LVNI_SELECTED as isize,
                        );
                        if index < 3 {
                            return Err("请先选中一条环境变量行".into());
                        }
                        let i = index as usize - 3;
                        let pair =
                            m.ai.as_ref()
                                .unwrap()
                                .variables
                                .get(i)
                                .cloned()
                                .ok_or("请选择环境变量")?;
                        if let Some(pair) = pair_dialog(m.hwnd, m.font, pair, false, true) {
                            let d = m.ai.as_mut().unwrap();
                            let expected = &d.proposal.as_ref().unwrap().environment[i].name;
                            if &pair.key != expected {
                                return Err("请保留方案中的变量名称".into());
                            }
                            if ai::placeholder(&pair.value) {
                                return Err("请填写实际值，不能使用占位内容".into());
                            }
                            d.variables[i] = pair;
                            list_text(
                                GetDlgItem(m.hwnd, 831),
                                index as usize,
                                2,
                                "已在本机填写（不发送给 AI）",
                            );
                            let item = LVITEMW {
                                stateMask: LVIS_STATEIMAGEMASK,
                                state: 2 << 12,
                                ..zeroed()
                            };
                            SendMessageW(
                                GetDlgItem(m.hwnd, 831),
                                LVM_SETITEMSTATE,
                                index as usize,
                                &item as *const _ as isize,
                            );
                        }
                    }
                    833 => {
                        let checked = |i| {
                            SendMessageW(
                                GetDlgItem(m.hwnd, 831),
                                LVM_GETITEMSTATE,
                                i,
                                LVIS_STATEIMAGEMASK as isize,
                            ) == 2 << 12
                        };
                        let d = m.ai.as_ref().unwrap();
                        let p = d.proposal.as_ref().ok_or("请先生成方案")?;
                        if d.check == PackageCheck::Missing {
                            return Err("包不存在，请修正方案".into());
                        }
                        let vars = d
                            .variables
                            .iter()
                            .enumerate()
                            .filter(|(i, _)| checked(i + 3))
                            .map(|(_, v)| v.clone())
                            .collect::<Vec<_>>();
                        let selection = Selection {
                            launch: checked(0),
                            package: checked(1),
                            arguments: checked(2),
                        };
                        if !selection.launch
                            && !selection.package
                            && !selection.arguments
                            && vars.is_empty()
                        {
                            return Err("请至少勾选一个字段".into());
                        }
                        let new = ai::apply(
                            d.profile.as_ref().ok_or("请从本地服务进入以填入配置")?,
                            p,
                            selection,
                            &vars,
                        )?;
                        let warning = format!(
                            "只填入未保存草稿，不安装、不执行、不启动。\r\n{}\r\n{}\r\n{}",
                            i18n::text(&d.check.label()),
                            if !new.stdio.is_empty() {
                                i18n::text("注意：原完整命令仍优先，需手动清空后才采用托管配置。")
                            } else {
                                i18n::text("")
                            },
                            if p.environment.iter().any(|e| e.required
                                && !new.env.iter().any(|v| v.key.eq_ignore_ascii_case(&e.name)
                                    && !ai::placeholder(&v.value)))
                            {
                                i18n::text("部分必填环境变量尚未填写，请保存后在环境页补齐。")
                            } else {
                                i18n::text("")
                            }
                        );
                        if MessageBoxW(
                            m.hwnd,
                            localized_wide(warning).as_ptr(),
                            localized_wide("确认填入草稿").as_ptr(),
                            MB_YESNO | MB_ICONINFORMATION,
                        ) == IDYES
                        {
                            m.ai.as_mut().unwrap().output = Some(new);
                            m.result = Some(true);
                            KillTimer(m.hwnd, 78);
                            DestroyWindow(m.hwnd);
                        }
                    }
                    793 => {
                        m.ai.as_ref().unwrap().task.cancel();
                        set(m, 800, "正在取消… 输入会保留");
                    }
                    2 => close(m),
                    _ => {}
                }
            }
            _ => {}
        }
        Ok(())
    })();
    if let Err(error) = result {
        set(m, 800, &error);
        layout(m);
        MessageBoxW(
            m.hwnd,
            localized_wide(error).as_ptr(),
            localized_wide("AI 安装向导").as_ptr(),
            MB_ICONWARNING,
        );
    }
    0
}
unsafe fn close(m: &mut Modal) {
    let d = m.ai.as_mut().unwrap();
    if d.worker.is_some() {
        d.closing = true;
        d.task.cancel();
        set(m, 800, "正在取消请求并关闭…");
    } else {
        KillTimer(m.hwnd, 78);
        m.result = Some(false);
        DestroyWindow(m.hwnd);
    }
}
pub(super) unsafe fn show(
    owner: HWND,
    font: HFONT,
    root: PathBuf,
    network: Network,
    profile: Option<Profile>,
) -> Result<Option<Profile>> {
    let settings = Store { root: root.clone() }.load()?.ai;
    let context = ai::environment_summary(&root, profile.as_ref());
    let mut m = modal_window(
        owner,
        font,
        "AI 辅助 MCP 安装向导",
        1000,
        800,
        false,
        false,
        false,
    );
    for (id, label, x) in [
        (790, "1. 资料与 API", 18),
        (791, "2. 分析结果", 200),
        (792, "3. 审核填写", 382),
    ] {
        modal_control(&m, id, "BUTTON", label, WS_TABSTOP, x, 12, 172, 30);
    }
    for (id, label) in [
        (811, "base_url"),
        (812, "model"),
        (813, "api_key"),
        (814, "仓库 URL"),
        (
            815,
            "公开 GitHub 仓库；可填分支，或直接粘贴资料。资料最多 256 KiB。",
        ),
        (817, "补充要求（可选）"),
        (807, "AI 设置仅保存应用配置；连接测试会发送一次计费请求。"),
        (820, "请核对包源验证结果、参数说明、所需变量及一次性步骤。"),
        (
            824,
            "修改要求：返回资料页编辑后重新生成。不会携带本机填写的变量值。",
        ),
        (830, "请先生成方案"),
        (
            834,
            "完整命令保持原样且优先；不确定的安装方案需要人工核实。",
        ),
    ] {
        modal_control(&m, id, "STATIC", label, 0, 18, 50, 200, 24);
    }
    for (id, value, password) in [
        (801, settings.base_url, false),
        (802, settings.api_key, true),
        (803, settings.model, false),
        (808, String::new(), false),
        (809, String::new(), false),
    ] {
        modal_control(
            &m,
            id,
            "EDIT",
            &value,
            WS_TABSTOP | ES_AUTOHSCROLL as u32 | if password { ES_PASSWORD as u32 } else { 0 },
            18,
            50,
            300,
            28,
        );
    }
    SendMessageW(
        GetDlgItem(m.hwnd, 809),
        EM_SETCUEBANNER,
        0,
        localized_wide("分支（可选）").as_ptr() as isize,
    );
    modal_control(
        &m,
        804,
        "BUTTON",
        "显示密钥",
        WS_TABSTOP | BS_AUTOCHECKBOX as u32,
        18,
        50,
        110,
        28,
    );
    for (id, label) in [
        (805, "测试连接"),
        (806, "保存 AI 设置"),
        (810, "读取仓库资料"),
        (819, "预览并生成方案"),
        (822, "下一步：审核填写"),
        (823, "查看 / 复制一次性步骤"),
        (832, "填写选中的环境变量"),
        (833, "填入勾选项到草稿"),
        (793, "取消请求"),
        (2, "关闭"),
    ] {
        modal_control(&m, id, "BUTTON", label, WS_TABSTOP, 18, 50, 180, 30);
    }
    for (id, readonly) in [(816, false), (818, false), (821, true)] {
        let h = modal_control(
            &m,
            id,
            "EDIT",
            "",
            WS_TABSTOP
                | WS_VSCROLL
                | ES_MULTILINE as u32
                | ES_AUTOVSCROLL as u32
                | if readonly { ES_READONLY as u32 } else { 0 },
            18,
            50,
            800,
            300,
        );
        SendMessageW(
            h,
            EM_SETLIMITTEXT,
            if id == 818 { 16384 } else { 1024 * 1024 },
            0,
        );
    }
    modal_control(
        &m,
        800,
        "STATIC",
        "填入配置后仍需手动保存、安装和启动。AI 不具备命令执行权限。",
        0,
        18,
        650,
        800,
        28,
    );
    let list = modal_control(
        &m,
        831,
        "SysListView32",
        "",
        WS_TABSTOP | LVS_REPORT | LVS_SHOWSELALWAYS,
        18,
        110,
        900,
        350,
    );
    SendMessageW(
        list,
        LVM_SETEXTENDEDLISTVIEWSTYLE,
        0,
        (LVS_EX_CHECKBOXES | LVS_EX_FULLROWSELECT | LVS_EX_LABELTIP | LVS_EX_DOUBLEBUFFER) as isize,
    );
    let dpi = GetDpiForWindow(m.hwnd) as i32;
    for (i, (label, width)) in [("勾选字段", 210), ("原值", 270), ("建议值 / 说明", 560)]
        .iter()
        .enumerate()
    {
        let mut s = localized_wide(label);
        let col = LVCOLUMNW {
            mask: LVCF_TEXT | LVCF_WIDTH,
            pszText: s.as_mut_ptr(),
            cx: width * dpi / 96,
            ..zeroed()
        };
        SendMessageW(list, LVM_INSERTCOLUMNW, i, &col as *const _ as isize);
    }
    m.ai = Some(Box::new(Dialog {
        root,
        profile,
        output: None,
        network,
        stage: 0,
        context,
        proposal: None,
        check: PackageCheck::NotApplicable,
        variables: vec![],
        rx: None,
        worker: None,
        task: Arc::new(Task::default()),
        closing: false,
        font: null_mut(),
    }));
    SetWindowLongPtrW(
        m.hwnd,
        GWL_STYLE,
        GetWindowLongPtrW(m.hwnd, GWL_STYLE) | (WS_THICKFRAME | WS_CLIPCHILDREN) as isize,
    );
    layout(&m);
    SetTimer(m.hwnd, 78, 100, None);
    SetFocus(GetDlgItem(m.hwnd, 808));
    modal_loop(owner, m.hwnd);
    Ok(m.ai.as_mut().unwrap().output.take())
}
