//! Native language switching against two controlled running MCP services.
use super::ai_checks::http;
use super::*;
use gateway::i18n::Language;
pub(super) fn prepare(store: &Store) -> Result<http::Mock> {
    let server = http::Mock::new(|request| {
        if request.line.contains("chat/completions") {
            let request: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
            let content: serde_json::Value =
                serde_json::from_str(request["messages"][1]["content"].as_str().unwrap()).unwrap();
            assert_eq!(content["response_language"], "en");
            let mut proposal: serde_json::Value =
                serde_json::from_str(include_str!("../../../tests/fixtures/ai-proposal.json"))
                    .unwrap();
            proposal["explanation"] = "English fixture explanation".into();
            return (
                200,
                serde_json::to_vec(
                    &serde_json::json!({"choices":[{"message":{"content":proposal.to_string()}}]}),
                )
                .unwrap(),
            );
        }
        (200, br#"{"versions":{"1.2.3":{}}}"#.to_vec())
    });
    let mut settings = store.load()?;
    // Preserve an existing preference without creating a real login task.
    // Saving an unrelated language setting must also work without elevation.
    settings.admin_login_startup = true;
    settings.ai = gateway::ai::AiSettings {
        base_url: format!("{}/v1", server.url),
        api_key: "local-language-test-key".into(),
        model: "mock".into(),
    };
    settings.dependency_network.npm_registry = server.url.clone();
    store.save(&settings)?;
    Ok(server)
}
struct TestChild(std::process::Child);
impl std::ops::Deref for TestChild {
    type Target = std::process::Child;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl std::ops::DerefMut for TestChild {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}
impl Drop for TestChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
unsafe fn value(h: HWND) -> String {
    let len = SendMessageW(h, WM_GETTEXTLENGTH, 0, 0).max(0) as usize;
    let mut buf = vec![0u16; len + 1];
    let n = SendMessageW(h, WM_GETTEXT, buf.len(), buf.as_mut_ptr() as isize).max(0) as usize;
    String::from_utf16_lossy(&buf[..n])
}
unsafe fn click(h: HWND, id: usize) {
    PostMessageW(h, WM_COMMAND, id, 0);
}
unsafe fn main_window(pid: u32) -> Result<HWND> {
    let mut window = null_mut();
    wait(|| {
        let mut search = WindowSearch {
            pid,
            window: null_mut(),
        };
        EnumWindows(Some(find_main), (&mut search as *mut WindowSearch) as isize);
        window = search.window;
        !window.is_null()
    })?;
    SendMessageW(
        window,
        RegisterWindowMessageW(wide("SuperGatewayGUI.Restore.v1").as_ptr()),
        0,
        0,
    );
    Ok(window)
}
unsafe fn choose(h: HWND, pid: u32, index: usize, title: &str) -> Result<()> {
    click(h, 16);
    let settings = ready_dialog(pid, h, title)?;
    check(
        SendMessageW(GetDlgItem(settings, 505), BM_GETCHECK, 0, 0) == BST_CHECKED as isize
            && (gateway::startup::is_elevated() || IsWindowEnabled(GetDlgItem(settings, 505)) == 0),
        "administrator startup remains checked and protected while changing language",
    )?;
    let combo = GetDlgItem(settings, 518);
    check(
        !combo.is_null() && SendMessageW(combo, CB_GETCOUNT, 0, 0) == 3,
        "language settings offer System, Chinese and English",
    )?;
    SendMessageW(combo, CB_SETCURSEL, index, 0);
    click(settings, 1);
    wait(|| IsWindow(settings) == 0)?;
    Ok(())
}
pub(super) unsafe fn run(store: &Store, pid: u32, exe: &std::path::Path) -> Result<()> {
    let h = main_window(pid)?;
    wait(|| IsWindowEnabled(GetDlgItem(h, 30)) != 0)?;
    let profiles = store.load()?.services;
    wait(|| {
        profiles
            .iter()
            .all(|p| gateway::net::profile_health(p).is_ok())
    })?;
    let panel = GetDlgItem(h, 22);
    let program_before = value(GetDlgItem(panel, 102));
    let name_before = value(GetDlgItem(panel, 100));
    let pids_before = profiles
        .iter()
        .map(gateway::net::test)
        .collect::<Result<Vec<_>>>()?;
    check(
        !pids_before.is_empty(),
        "language fixture has two usable MCP services",
    )?;
    choose(h, pid, 2, "应用设置")?;
    wait(|| value(GetDlgItem(h, 16)) == "Settings")?;
    check(
        store.load()?.language == Language::English
            && store.load()?.services == profiles
            && store.load()?.admin_login_startup,
        "language persists without changing services or existing administrator startup",
    )?;
    check(
        value(GetDlgItem(panel, 102)) == program_before
            && value(GetDlgItem(panel, 100)) == name_before,
        "language switch preserves Chinese service names and program paths",
    )?;
    check(
        value(GetDlgItem(h, 30)) == "Save profile"
            && value(GetDlgItem(panel, 160)) == "Edit arguments…",
        "main toolbar and form labels switch to English immediately",
    )?;
    check(
        profiles.iter().all(|p| gateway::net::test(p).is_ok()),
        "switching language keeps both MCP services connected",
    )?;
    click(h, 18);
    let help = ready_dialog(pid, h, "SuperGateway · Offline Help")?;
    let body = value(GetDlgItem(help, 500));
    check(
        body.contains("SuperGateway User Guide") && body.contains("Windows display language"),
        "English offline help explains language and startup behavior",
    )?;
    SendMessageW(GetDlgItem(help, 504), LB_SETCURSEL, 7, 0);
    SendMessageW(help, WM_COMMAND, 504 | ((LBN_SELCHANGE as usize) << 16), 0);
    check(
        value(GetDlgItem(help, 500)).contains("Confirm and send"),
        "English AI help has a separate translated chapter",
    )?;
    click(help, 1);
    wait(|| IsWindow(help) == 0)?;
    click(h, 161);
    let wizard = ready_dialog(pid, h, "AI-assisted MCP Setup Wizard")?;
    check(
        value(GetDlgItem(wizard, 819)) == "Preview and generate"
            && value(GetDlgItem(wizard, 806)) == "Save AI settings",
        "AI wizard buttons and title are localized",
    )?;
    SendMessageW(
        GetDlgItem(wizard, 816),
        WM_SETTEXT,
        0,
        wide("Controlled documentation only.").as_ptr() as isize,
    );
    click(wizard, 819);
    let preview = ready_dialog(pid, wizard, "Request preview · API Key excluded")?;
    click(preview, 1);
    wait(|| IsWindowVisible(GetDlgItem(wizard, 821)) != 0)?;
    let explanation = value(GetDlgItem(wizard, 821));
    check(
        explanation.contains("English fixture explanation")
            && explanation.contains("Service arguments:")
            && explanation.contains("Prerequisites:"),
        "English generation requests English descriptions and displays translated analysis headings",
    )?;
    click(wizard, 822);
    wait(|| IsWindowVisible(GetDlgItem(wizard, 833)) != 0)?;
    click(wizard, 833);
    let confirm = ready_dialog(pid, wizard, "Confirm draft changes")?;
    let warning = value(GetDlgItem(confirm, 65535));
    check(
        warning.contains("Fill an unsaved draft only")
            && warning.contains("Registry confirmed version 1.2.3")
            && warning.contains("Some required variables are still missing"),
        "English draft confirmation translates system warnings and package status",
    )?;
    click(confirm, IDNO as usize);
    wait(|| IsWindow(confirm) == 0)?;
    click(wizard, 2);
    wait(|| IsWindow(wizard) == 0)?;
    choose(h, pid, 1, "Application settings")?;
    wait(|| value(GetDlgItem(h, 16)) == "设置")?;
    check(
        store.load()?.language == Language::Chinese
            && profiles.iter().all(|p| gateway::net::test(p).is_ok()),
        "switching back to Chinese preserves running MCP services",
    )?;
    choose(h, pid, 2, "应用设置")?;
    wait(|| value(GetDlgItem(h, 16)) == "Settings")?;
    choose(h, pid, 0, "Application settings")?;
    wait(|| store.load().is_ok_and(|s| s.language == Language::System))?;
    let system_english =
        Language::System.english_for(windows_sys::Win32::Globalization::GetUserDefaultUILanguage());
    wait(|| value(GetDlgItem(h, 16)) == if system_english { "Settings" } else { "设置" })?;
    check(
        true,
        "System language setting follows Windows display language",
    )?;
    choose(
        h,
        pid,
        2,
        if system_english {
            "Application settings"
        } else {
            "应用设置"
        },
    )?;
    wait(|| value(GetDlgItem(h, 16)) == "Settings")?;
    click(h, 17);
    let prompt = ready_dialog(pid, h, "SuperGateway")?;
    check(
        value(GetDlgItem(prompt, 65535)).contains("Stop these MCP services and exit the GUI?"),
        "running-service exit confirmation is English",
    )?;
    click(prompt, IDYES as usize);
    wait(|| IsWindow(h) == 0)?;
    let mut child = TestChild(
        Command::new(exe)
            .args(["--data-dir", store.root.to_str().ok_or("invalid root")?])
            .env("PATH", "")
            .spawn()
            .map_err(|e| e.to_string())?,
    );
    let restarted = main_window(child.id())?;
    check(
        value(GetDlgItem(restarted, 16)) == "Settings",
        "actual GUI restart restores English from encrypted settings",
    )?;
    wait(|| IsWindowEnabled(GetDlgItem(restarted, 30)) != 0)?;
    click(restarted, 15);
    wait(|| {
        profiles
            .iter()
            .all(|p| gateway::net::profile_health(p).is_err())
    })?;
    wait(|| IsWindowEnabled(GetDlgItem(restarted, 30)) != 0)?;
    click(restarted, 17);
    wait(|| IsWindow(restarted) == 0)?;
    child.wait().map_err(|e| e.to_string())?;
    check(
        true,
        "restarted English GUI exits and releases managed services",
    )?;
    Ok(())
}
