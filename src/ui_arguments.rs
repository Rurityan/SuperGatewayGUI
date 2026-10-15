//! Native argument rows. Values remain a Vec, including empty arguments.
use super::*;
const SS_ETCHEDHORZ: u32 = 0x10;

unsafe extern "system" fn edit_proc(
    hwnd: HWND,
    msg: u32,
    w: WPARAM,
    l: LPARAM,
    id: usize,
    _: usize,
) -> LRESULT {
    if msg == WM_PASTE {
        if let Ok(value) = clipboard_read(hwnd) {
            SendMessageW(hwnd, EM_REPLACESEL, 1, wide(value.trim()).as_ptr() as isize);
            return 0;
        }
    } else if msg == WM_NCDESTROY {
        RemoveWindowSubclass(hwnd, Some(edit_proc), id);
    }
    DefSubclassProc(hwnd, msg, w, l)
}

pub(super) struct Editor {
    rows: Vec<Vec<HWND>>,
    first: usize,
    font: HFONT,
}
impl Drop for Editor {
    fn drop(&mut self) {
        if !self.font.is_null() {
            unsafe {
                DeleteObject(self.font);
            }
        }
    }
}

pub(super) fn summary(values: &[String]) -> String {
    if values.is_empty() {
        return "未设置参数".into();
    }
    values
        .iter()
        .enumerate()
        .map(|(i, value)| {
            format!(
                "{}. {}",
                i + 1,
                if value.is_empty() {
                    "（空参数）"
                } else {
                    value
                }
            )
        })
        .collect::<Vec<_>>()
        .join("\r\n")
}

unsafe fn values(modal: &Modal) -> Vec<String> {
    modal
        .arguments
        .as_ref()
        .unwrap()
        .rows
        .iter()
        .map(|row| {
            let control = row[1];
            let mut text = vec![0u16; GetWindowTextLengthW(control) as usize + 1];
            let len = GetWindowTextW(control, text.as_mut_ptr(), text.len() as i32);
            String::from_utf16_lossy(&text[..len as usize])
        })
        .collect()
}

unsafe fn rebuild(modal: &mut Modal, args: &[String]) {
    for row in &modal.arguments.as_ref().unwrap().rows {
        for control in row {
            DestroyWindow(*control);
        }
    }
    modal.arguments.as_mut().unwrap().rows.clear();
    for (i, value) in args.iter().enumerate() {
        let base = 7000 + i as i32 * 8;
        let row = vec![
            modal_control(
                modal,
                base,
                "STATIC",
                &format!("{}", i + 1),
                0,
                0,
                0,
                40,
                24,
            ),
            modal_control(
                modal,
                base + 1,
                "EDIT",
                value,
                ES_AUTOHSCROLL as u32 | WS_TABSTOP,
                0,
                0,
                200,
                30,
            ),
            modal_control(modal, base + 2, "BUTTON", "上移", WS_TABSTOP, 0, 0, 56, 30),
            modal_control(modal, base + 3, "BUTTON", "下移", WS_TABSTOP, 0, 0, 56, 30),
            modal_control(modal, base + 4, "BUTTON", "删除", WS_TABSTOP, 0, 0, 56, 30),
            modal_control(modal, base + 5, "STATIC", "", SS_ETCHEDHORZ, 0, 0, 200, 2),
        ];
        EnableWindow(row[2], (i > 0) as i32);
        EnableWindow(row[3], (i + 1 < args.len()) as i32);
        // The edit control's default 32767-character limit must not truncate a saved value.
        SendMessageW(row[1], EM_SETLIMITTEXT, 0x7ffffffe, 0);
        SetWindowSubclass(row[1], Some(edit_proc), 1, 0);
        modal.arguments.as_mut().unwrap().rows.push(row);
    }
    layout(modal);
}

unsafe fn layout(modal: &mut Modal) {
    let dpi = GetDpiForWindow(modal.hwnd) as i32;
    let mut rect: RECT = zeroed();
    GetClientRect(modal.hwnd, &mut rect);
    let width = rect.right * 96 / dpi;
    let height = rect.bottom * 96 / dpi;
    let visible = ((height - 150) / 62).max(1) as usize;
    let editor = modal.arguments.as_mut().unwrap();
    editor.first = editor.first.min(editor.rows.len().saturating_sub(visible));
    let info = SCROLLINFO {
        cbSize: size_of::<SCROLLINFO>() as u32,
        fMask: SIF_RANGE | SIF_PAGE | SIF_POS,
        nMin: 0,
        nMax: editor.rows.len().saturating_sub(1) as i32,
        nPage: visible as u32,
        nPos: editor.first as i32,
        ..zeroed()
    };
    SetScrollInfo(modal.hwnd, SB_VERT, &info, 1);
    let place = |control, x, y, w, h| {
        SetWindowPos(
            control,
            null_mut(),
            x * dpi / 96,
            y * dpi / 96,
            w * dpi / 96,
            h * dpi / 96,
            SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOREDRAW,
        );
    };
    for (i, row) in editor.rows.iter().enumerate() {
        let show = i >= editor.first && i < editor.first + visible;
        for control in row {
            ShowWindow(*control, if show { SW_SHOWNA } else { SW_HIDE });
        }
        if !show {
            continue;
        }
        let y = 94 + (i - editor.first) as i32 * 62;
        place(row[0], 20, y + 5, 36, 24);
        place(row[1], 60, y, width - 280, 30);
        for (j, x) in [(2, width - 210), (3, width - 146), (4, width - 82)] {
            place(row[j], x, y, 56, 30);
        }
        place(row[5], 20, y + 46, width - 42, 2);
    }
    place(GetDlgItem(modal.hwnd, 580), 20, 50, 126, 30);
    place(GetDlgItem(modal.hwnd, 581), 158, 54, width - 178, 24);
    place(GetDlgItem(modal.hwnd, 582), 20, 12, width - 40, 32);
    place(GetDlgItem(modal.hwnd, 1), width - 246, height - 44, 106, 32);
    place(GetDlgItem(modal.hwnd, 2), width - 126, height - 44, 106, 32);
    RedrawWindow(
        modal.hwnd,
        null(),
        null_mut(),
        RDW_INVALIDATE | RDW_ERASE | RDW_ALLCHILDREN,
    );
}

pub(super) unsafe fn procedure(modal: &mut Modal, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    match msg {
        WM_COMMAND => {
            let id = (w & 0xffff) as i32;
            if id == 1 {
                modal.values = values(modal);
                let changed = gateway::arguments::normalize(&mut modal.values);
                if changed != 0 {
                    MessageBoxW(
                        modal.hwnd,
                        wide(format!("已自动清理 {changed} 项参数首尾的空格、换行等空白。\r\n参数内部的空格及空参数保持不变。")).as_ptr(),
                        wide("参数已整理").as_ptr(),
                        MB_OK | MB_ICONINFORMATION,
                    );
                }
                modal.result = Some(true);
                DestroyWindow(modal.hwnd);
            } else if id == 2 {
                modal.result = Some(false);
                DestroyWindow(modal.hwnd);
            } else if id == 580 {
                let mut args = values(modal);
                if args.len() >= 6000 {
                    MessageBoxW(
                        modal.hwnd,
                        wide("参数数量已达到 6000 项上限").as_ptr(),
                        wide("参数").as_ptr(),
                        MB_OK,
                    );
                    return 0;
                }
                args.push(String::new());
                modal.arguments.as_mut().unwrap().first = args.len() - 1;
                rebuild(modal, &args);
                SetFocus(modal.arguments.as_ref().unwrap().rows.last().unwrap()[1]);
            } else if id >= 7000 && (w >> 16) == 0 {
                let index = ((id - 7000) / 8) as usize;
                let operation = (id - 7000) % 8;
                let mut args = values(modal);
                if index < args.len() {
                    let target = match operation {
                        2 if index > 0 => {
                            args.swap(index, index - 1);
                            index - 1
                        }
                        3 if index + 1 < args.len() => {
                            args.swap(index, index + 1);
                            index + 1
                        }
                        4 => {
                            args.remove(index);
                            index.min(args.len().saturating_sub(1))
                        }
                        _ => return 0,
                    };
                    modal.arguments.as_mut().unwrap().first = target.saturating_sub(1);
                    rebuild(modal, &args);
                    if let Some(row) = modal.arguments.as_ref().unwrap().rows.get(target) {
                        SetFocus(row[1]);
                    } else {
                        SetFocus(GetDlgItem(modal.hwnd, 580));
                    }
                }
            }
            0
        }
        WM_CLOSE => {
            modal.result = Some(false);
            DestroyWindow(modal.hwnd);
            0
        }
        WM_SIZE => {
            layout(modal);
            0
        }
        WM_GETMINMAXINFO => {
            let dpi = GetDpiForWindow(modal.hwnd) as i32;
            let info = &mut *(l as *mut MINMAXINFO);
            info.ptMinTrackSize.x = 650 * dpi / 96;
            info.ptMinTrackSize.y = 350 * dpi / 96;
            0
        }
        WM_DPICHANGED => {
            let rect = &*(l as *const RECT);
            SetWindowPos(
                modal.hwnd,
                null_mut(),
                rect.left,
                rect.top,
                rect.right - rect.left,
                rect.bottom - rect.top,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
            if let Ok(icons) = WindowIcons::load((w & 0xffff) as u32) {
                icons.apply(modal.hwnd);
                modal.icons = Some(icons);
            }
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
                let mut control = GetWindow(modal.hwnd, GW_CHILD);
                while !control.is_null() {
                    SendMessageW(control, WM_SETFONT, font as usize, 0);
                    control = GetWindow(control, GW_HWNDNEXT);
                }
                let editor = modal.arguments.as_mut().unwrap();
                if !editor.font.is_null() {
                    DeleteObject(editor.font);
                }
                editor.font = font;
                modal.font = font;
            }
            layout(modal);
            0
        }
        WM_VSCROLL | WM_MOUSEWHEEL => {
            let mut info: SCROLLINFO = zeroed();
            info.cbSize = size_of::<SCROLLINFO>() as u32;
            info.fMask = SIF_ALL;
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
                SB_LINEUP => info.nPos - 1,
                SB_LINEDOWN => info.nPos + 1,
                SB_PAGEUP => info.nPos - info.nPage as i32,
                SB_PAGEDOWN => info.nPos + info.nPage as i32,
                SB_THUMBTRACK => info.nTrackPos,
                SB_TOP => 0,
                SB_BOTTOM => info.nMax,
                _ => info.nPos,
            };
            modal.arguments.as_mut().unwrap().first = next.max(0) as usize;
            layout(modal);
            0
        }
        _ => DefWindowProcW(modal.hwnd, msg, w, l),
    }
}

pub(super) unsafe fn show(
    owner: HWND,
    font: HFONT,
    title: &str,
    args: &[String],
    one_shot: bool,
) -> Option<Vec<String>> {
    let mut modal = modal_window(owner, font, title, 800, 550, false, false, false);
    modal.arguments = Some(Box::new(Editor {
        rows: vec![],
        first: 0,
        font: null_mut(),
    }));
    modal_control(
        &modal,
        582,
        "STATIC",
        if one_shot {
            "仅填写临时参数，不填 uvx / npx 或包名；下一步预览，不写入服务参数。"
        } else {
            "每项一个输入框，首尾空白自动清理；内部空格、引号及空参数保留。"
        },
        0,
        20,
        12,
        740,
        32,
    );
    modal_control(
        &modal,
        580,
        "BUTTON",
        "＋ 添加参数",
        WS_TABSTOP,
        20,
        50,
        126,
        30,
    );
    modal_control(
        &modal,
        581,
        "STATIC",
        "自动清理首尾空白；内部空格保留。空参数不需要时请删除该行。",
        0,
        158,
        54,
        600,
        24,
    );
    modal_control(
        &modal,
        1,
        "BUTTON",
        if one_shot {
            "预览命令"
        } else {
            "确认参数"
        },
        WS_TABSTOP | BS_DEFPUSHBUTTON as u32,
        540,
        456,
        106,
        32,
    );
    modal_control(&modal, 2, "BUTTON", "取消", WS_TABSTOP, 658, 456, 106, 32);
    SetWindowLongPtrW(
        modal.hwnd,
        GWL_STYLE,
        GetWindowLongPtrW(modal.hwnd, GWL_STYLE)
            | (WS_THICKFRAME | WS_VSCROLL | WS_CLIPCHILDREN) as isize,
    );
    rebuild(&mut modal, args);
    SetFocus(GetDlgItem(modal.hwnd, 580));
    modal_loop(owner, modal.hwnd);
    (modal.result == Some(true)).then_some(modal.values.clone())
}
