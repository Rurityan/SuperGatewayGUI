use gateway::{
    arguments, dependencies,
    model::{Pair, Profile, Settings},
    store::{Store, protect},
};

#[test]
fn legacy_dpapi_and_exports_normalize_both_argument_lists_without_rewriting_on_read() {
    let root = std::env::temp_dir().join(format!("参数 清理 {}", gateway::model::new_id()));
    let store = Store::new(root.clone()).unwrap();
    let raw = br#"{"Services":[{"Args":["  --browser\r\n"," msedge ",""],"Dependency":{"Args":["\t --isolated \r\n"]},"Stdio":" keep original command ","Env":[{"Key":"KEEP","Value":"  secret value \r\n"}]}]}"#;
    let encrypted = protect(raw, true).unwrap();
    std::fs::write(root.join("settings.dat"), &encrypted).unwrap();
    let settings = store.load().unwrap();
    let p = &settings.services[0];
    assert_eq!(p.args, ["--browser", "msedge", ""]);
    assert_eq!(p.dependency.args, ["--isolated"]);
    assert_eq!(p.stdio, " keep original command ");
    assert_eq!(p.env[0].value, "  secret value \r\n");
    assert_eq!(std::fs::read(root.join("settings.dat")).unwrap(), encrypted);
    let mut dirty = settings.clone();
    dirty.services[0].args[0] = " \r\n--browser \t".into();
    store.save(&dirty).unwrap();
    assert_eq!(store.load().unwrap().services, settings.services);
    let text = gateway::exchange::export(&dirty.services).unwrap();
    assert!(!text.contains("\\r\\n--browser"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn imported_and_direct_launch_arguments_share_cleanup_and_preserve_other_values() {
    let imported = gateway::config::import(r#"{"mcpServers":{"demo":{"command":"not-installed.exe","args":[" \r\n--isolated \t"," 中文 路径 ",""]}}}"#, &[]).unwrap();
    assert_eq!(imported.profiles[0].args, ["--isolated", "中文 路径", ""]);
    let mut p = Profile {
        args: vec![" \r\n--isolated \t".into(), " 中文 路径 ".into(), "".into()],
        env: vec![Pair {
            key: "KEEP".into(),
            value: "  value  ".into(),
            ..Default::default()
        }],
        ..Default::default()
    };
    let prepared = dependencies::prepared(std::path::Path::new("."), &p).unwrap();
    assert_eq!(prepared.args, imported.profiles[0].args);
    assert_eq!(prepared.env, p.env);
    assert!(p.args[0].starts_with(' '));
    arguments::normalize(&mut p.args);
    let settings = Settings {
        services: vec![p],
        ..Default::default()
    };
    assert!(
        serde_json::to_string(&settings)
            .unwrap()
            .contains("--isolated")
    );
}

#[test]
fn ai_proposal_and_setup_normalize_before_validation_and_draft_apply() {
    let mut value: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/ai-proposal.json")).unwrap();
    value["arguments"][0]["value"] = serde_json::json!(" \r\n--browser \t");
    value["setup"][0]["args"][0] = serde_json::json!(" \r\nsetup ");
    let proposal = gateway::ai::Proposal::parse(&value.to_string()).unwrap();
    assert_eq!(proposal.arguments[0].value, "--browser");
    assert_eq!(proposal.setup[0].args, ["setup"]);
    let original = Profile::default();
    let draft = gateway::ai::apply(
        &original,
        &proposal,
        gateway::ai::Selection {
            launch: true,
            package: true,
            arguments: true,
        },
        &[],
    )
    .unwrap();
    assert_eq!(draft.dependency.args[0], "--browser");
    assert!(original.dependency.args.is_empty());
}
