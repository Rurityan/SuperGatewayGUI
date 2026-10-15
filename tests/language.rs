use gateway::{
    i18n::Language,
    model::{Pair, Profile, Settings},
    store::Store,
};

#[test]
fn language_is_global_encrypted_and_independent_of_services() {
    let root = std::env::temp_dir().join(format!("语言配置 空格 {}", gateway::model::new_id()));
    let store = Store::new(root.clone()).unwrap();
    let profile = Profile {
        name: "启动".into(),
        command: "D:/中文 工具/mcp.exe".into(),
        args: vec!["保存配置".into(), "".into()],
        env: vec![Pair {
            key: "LANGUAGE_VALUE".into(),
            value: "not-a-translation-secret".into(),
            hidden: true,
        }],
        ..Default::default()
    };
    let mut settings = Settings {
        services: vec![profile.clone()],
        ..Default::default()
    };
    assert_eq!(settings.language, Language::System);
    store.save(&settings).unwrap();
    for language in [Language::English, Language::Chinese, Language::System] {
        settings.language = language;
        store.save(&settings).unwrap();
        let loaded = store.load().unwrap();
        assert_eq!(loaded.language, language);
        assert_eq!(loaded.services.as_slice(), std::slice::from_ref(&profile));
        assert!(
            !String::from_utf8_lossy(&std::fs::read(root.join("settings.dat")).unwrap())
                .contains("not-a-translation-secret")
        );
    }
    assert!(root.join("settings.dat.pre-rust.bak").is_file());
    let exported = gateway::exchange::export(&settings.services).unwrap();
    assert!(!exported.contains("\"Language\""));
    assert_eq!(
        serde_json::from_str::<Settings>("{}").unwrap().language,
        Language::System
    );
    std::fs::remove_dir_all(root).unwrap();
}
