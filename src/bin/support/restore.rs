//! Reproduce a login-style minimized/hidden window before its first tray show.
use super::*;

unsafe fn size(hwnd: HWND) -> (i32, i32) {
    let mut rect = std::mem::zeroed();
    GetClientRect(hwnd, &mut rect);
    (rect.right, rect.bottom)
}

pub(super) unsafe fn run(hwnd: HWND) -> Result<()> {
    let panel = GetDlgItem(hwnd, 22);
    let log = GetDlgItem(hwnd, 40);
    let expected = (size(hwnd), size(panel), size(log));
    check(
        expected.1.0 > 300 && expected.1.1 > 150,
        "hidden startup has a complete initial form layout",
    )?;
    let activate = RegisterWindowMessageW(wide("SuperGatewayGUI.Restore.v1").as_ptr());
    for tray in [true, false] {
        ShowWindow(hwnd, SW_SHOWMINNOACTIVE);
        wait(|| IsIconic(hwnd) != 0)?;
        PostMessageW(hwnd, WM_CLOSE, 0, 0);
        wait(|| IsWindowVisible(hwnd) == 0)?;
        if tray {
            PostMessageW(hwnd, WM_APP + 1, 0, WM_LBUTTONDBLCLK as isize);
        } else {
            SendMessageW(hwnd, activate, 0, 0);
        }
        wait(|| IsWindowVisible(hwnd) != 0 && IsIconic(hwnd) == 0)?;
        let result = wait(|| (size(hwnd), size(panel), size(log)) == expected);
        if result.is_err() {
            return Err(format!(
                "restored layout mismatch (tray={tray}): expected {expected:?}, actual {:?}",
                (size(hwnd), size(panel), size(log))
            ));
        }
        check(
            true,
            if tray {
                "first tray restore repairs minimized hidden form and full log area without maximizing"
            } else {
                "registered activation restores full layout from minimized hidden state"
            },
        )?;
        PostMessageW(hwnd, WM_CLOSE, 0, 0);
        wait(|| IsWindowVisible(hwnd) == 0)?;
    }
    ShowWindow(hwnd, SW_SHOWMAXIMIZED);
    wait(|| IsZoomed(hwnd) != 0)?;
    let maximized = (size(hwnd), size(panel), size(log));
    PostMessageW(hwnd, WM_CLOSE, 0, 0);
    wait(|| IsWindowVisible(hwnd) == 0)?;
    PostMessageW(hwnd, WM_APP + 1, 0, WM_LBUTTONDBLCLK as isize);
    wait(|| IsWindowVisible(hwnd) != 0)?;
    check(
        IsZoomed(hwnd) != 0 && (size(hwnd), size(panel), size(log)) == maximized,
        "tray restore preserves maximized placement and complete layout",
    )?;
    ShowWindow(hwnd, SW_RESTORE);
    wait(|| IsZoomed(hwnd) == 0 && (size(hwnd), size(panel), size(log)) == expected)?;
    PostMessageW(hwnd, WM_CLOSE, 0, 0);
    wait(|| IsWindowVisible(hwnd) == 0)?;
    Ok(())
}
