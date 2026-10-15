use std::{path::PathBuf, process::Command};

#[test]
fn core_gateway_protocols_and_lifecycle() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    assert!(
        root.join("engine/supergateway.exe").is_file(),
        "Run scripts/build-engine.ps1 before protocol tests"
    );
    let status = Command::new(env!("CARGO_BIN_EXE_gateway-checks"))
        .arg(root.join("engine/supergateway.exe"))
        .arg(env!("CARGO_BIN_EXE_mock-mcp"))
        .current_dir(root)
        .status()
        .expect("Launch Rust core checks");
    assert!(status.success(), "Core integration checks failed");
}

#[test]
fn native_desktop_controls_and_tray() {
    let data = std::env::temp_dir().join(format!(
        "SGGUI 中文 UI {} {}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let status = Command::new(env!("CARGO_BIN_EXE_desktop-checks"))
        .arg("ui")
        .arg(data)
        .arg(env!("CARGO_BIN_EXE_SuperGatewayGUI"))
        .status()
        .expect("Launch Rust desktop checks");
    assert!(status.success(), "Native UI checks failed");
    let data = std::env::temp_dir().join(format!("SGGUI AI 中文 UI {}", gateway::model::new_id()));
    let status = Command::new(env!("CARGO_BIN_EXE_desktop-checks"))
        .arg("ai-ui")
        .arg(data)
        .arg(env!("CARGO_BIN_EXE_SuperGatewayGUI"))
        .status()
        .expect("Launch AI wizard UI checks");
    assert!(status.success(), "AI wizard UI checks failed");
}
