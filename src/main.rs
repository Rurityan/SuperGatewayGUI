#![windows_subsystem = "windows"]
mod help;
mod ui;
fn main() {
    // Retired exports must fail instead of accidentally opening another GUI.
    if std::env::args_os().any(|arg| arg == "--stdio-launch") {
        std::process::exit(1);
    }
    gateway::environment::initialize();
    if let Some(file) = std::env::args_os()
        .collect::<Vec<_>>()
        .windows(2)
        .find(|v| v[0] == "--one-shot")
    {
        let _ = gateway::oneshot::run(std::path::Path::new(&file[1]));
        return;
    }
    if let Err(error) = ui::run() {
        unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::MessageBoxW(
                std::ptr::null_mut(),
                gateway::native::wide(error).as_ptr(),
                gateway::native::wide("SuperGateway · 启动失败").as_ptr(),
                0x10,
            );
        }
    }
}
