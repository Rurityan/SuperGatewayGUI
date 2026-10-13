use std::process::Command;

#[test]
fn selected_configuration_exchange_through_native_gui() {
    let data = std::env::temp_dir().join(format!(
        "SGGUI 中文 配置交换 {} {}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let status = Command::new(env!("CARGO_BIN_EXE_desktop-checks"))
        .arg("exchange-ui")
        .arg(&data)
        .arg(env!("CARGO_BIN_EXE_SuperGatewayGUI"))
        .status()
        .expect("Launch controlled configuration exchange UI checks");
    assert!(status.success(), "Configuration exchange UI checks failed");
    std::fs::remove_dir_all(data).unwrap();
}
