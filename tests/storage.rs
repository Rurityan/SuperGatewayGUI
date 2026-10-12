use gateway::{
    dependencies as deps,
    model::{Profile, Settings},
    storage,
    store::Store,
};
use std::{fs, path::PathBuf};
fn root() -> PathBuf {
    let root = std::env::temp_dir().join(format!("SG 存放路径 {}", gateway::model::new_id()));
    fs::create_dir_all(&root).unwrap();
    root
}
#[test]
fn selection_persists_and_explicit_directory_overrides_without_environment_changes() {
    let root = root();
    let bootstrap = root.join("默认目录");
    let selected = root.join("用户配置 空格");
    fs::create_dir_all(&selected).unwrap();
    let before = std::env::var_os("PATH");
    assert_eq!(
        storage::configuration_directory(&bootstrap, None).unwrap(),
        bootstrap
    );
    storage::remember_configuration(&bootstrap, &selected).unwrap();
    assert_eq!(
        storage::configuration_directory(&bootstrap, None).unwrap(),
        selected
    );
    let explicit = root.join("命令行优先");
    assert_eq!(
        storage::configuration_directory(&bootstrap, Some(&explicit)).unwrap(),
        explicit
    );
    fs::write(bootstrap.join("locations.json"), b"invalid").unwrap();
    assert!(storage::configuration_directory(&bootstrap, None).is_err());
    assert!(storage::configuration_directory(&bootstrap, Some(&explicit)).is_ok());
    assert_eq!(
        fs::read(bootstrap.join("locations.json")).unwrap(),
        b"invalid"
    );
    storage::remember_configuration(&bootstrap, &selected).unwrap();
    fs::remove_dir(&selected).unwrap();
    assert!(storage::configuration_directory(&bootstrap, None).is_err());
    assert_eq!(std::env::var_os("PATH"), before);
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn encrypted_config_backup_and_tls_copy_without_dependency_migration() {
    let root = root();
    let old = root.join("旧配置");
    let new = root.join("新配置 空格");
    let packages = root.join("环境依赖 空格");
    let store = Store::new(old.clone()).unwrap();
    store
        .save(&Settings {
            services: vec![Profile {
                name: "现有服务".into(),
                bearer: "秘密".into(),
                ..Default::default()
            }],
            ..Default::default()
        })
        .unwrap();
    store.save(&store.load().unwrap()).unwrap();
    gateway::tls::prepare(&old).unwrap();
    fs::create_dir_all(old.join("dependencies")).unwrap();
    fs::write(old.join("dependencies/保留.txt"), b"stay").unwrap();
    let original = fs::read(old.join("settings.dat")).unwrap();
    let dest = storage::prepare(&old, &new, &packages).unwrap();
    assert_eq!(
        Store::new(dest.clone()).unwrap().load().unwrap().services[0].bearer,
        "秘密"
    );
    assert_eq!(fs::read(new.join("settings.dat")).unwrap(), original);
    assert_eq!(fs::read(old.join("settings.dat")).unwrap(), original);
    assert_eq!(
        fs::read(old.join("settings.dat.pre-rust.bak")).unwrap(),
        fs::read(new.join("settings.dat.pre-rust.bak")).unwrap()
    );
    for name in ["ca.dat", "server.dat", "root.crt", "server.crt"] {
        assert_eq!(
            fs::read(old.join("tls").join(name)).unwrap(),
            fs::read(new.join("tls").join(name)).unwrap()
        );
    }
    gateway::tls::prepare(&new).unwrap();
    assert_eq!(
        fs::read(old.join("tls/root.crt")).unwrap(),
        fs::read(new.join("tls/root.crt")).unwrap()
    );
    assert_eq!(
        storage::dependency_directory(&new)
            .unwrap()
            .canonicalize()
            .unwrap(),
        packages.canonicalize().unwrap()
    );
    assert_eq!(
        fs::read(old.join("dependencies/保留.txt")).unwrap(),
        b"stay"
    );
    assert!(!new.join("dependencies").exists());
    assert_eq!(fs::read_dir(&packages).unwrap().count(), 0);
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn invalid_or_populated_destinations_preserve_original_and_allow_retry() {
    let root = root();
    let old = root.join("旧配置");
    let new = root.join("目标");
    let packages = root.join("环境");
    let store = Store::new(old.clone()).unwrap();
    store.save(&Settings::default()).unwrap();
    fs::create_dir_all(&new).unwrap();
    fs::write(new.join("不可覆盖.txt"), b"preserved").unwrap();
    assert!(storage::prepare(&old, &new, &packages).is_err());
    assert_eq!(fs::read(new.join("不可覆盖.txt")).unwrap(), b"preserved");
    assert!(storage::prepare(&old, &old.join("nested"), &packages).is_err());
    fs::remove_file(new.join("不可覆盖.txt")).unwrap();
    fs::write(old.join("settings.dat"), b"invalid").unwrap();
    assert!(storage::prepare(&old, &new, &packages).is_err());
    assert!(!new.join("settings.dat").exists());
    assert_eq!(fs::read(old.join("settings.dat")).unwrap(), b"invalid");
    fs::remove_file(old.join("settings.dat")).unwrap();
    store.save(&Settings::default()).unwrap();
    // Dependencies may also be inside the new config folder, using the old default layout.
    storage::prepare(&old, &new, &new.join("dependencies")).unwrap();
    assert!(storage::absolute(std::path::Path::new("relative/path")).is_err());
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn all_managed_operations_use_selected_dependency_directory() {
    let root = root();
    let config = root.join("配置");
    let packages = root.join("包依赖");
    fs::create_dir_all(&config).unwrap();
    fs::create_dir_all(&packages).unwrap();
    storage::set_dependency_directory(&config, &packages).unwrap();
    let runtime = deps::runtime(&config, "node", deps::CATALOG[0].version).unwrap();
    assert_eq!(
        runtime,
        packages
            .join("runtimes/node")
            .join(deps::CATALOG[0].version)
    );
    fs::create_dir_all(&runtime).unwrap();
    fs::write(runtime.join("node.exe"), b"fixture").unwrap();
    let env = deps::environment(&config, &Default::default(), &Default::default(), false).unwrap();
    assert!(env["PATH"].contains("包依赖"));
    assert!(env["npm_config_cache"].contains("包依赖"));
    let p = Profile {
        managed: true,
        ..Default::default()
    };
    assert!(deps::prepared(&config, &p).is_err());
    assert!(deps::report(&config, &[], &[]).contains("包依赖"));
    fs::create_dir_all(packages.join("cache")).unwrap();
    fs::write(packages.join("cache/file"), b"cached").unwrap();
    deps::clear_cache(&config, &[]).unwrap();
    assert!(!packages.join("cache").exists());
    let service = packages.join("services").join(&p.id);
    fs::create_dir_all(&service).unwrap();
    deps::remove_service(&config, &p.id, &[]).unwrap();
    assert!(!service.exists());
    deps::remove_runtime(&config, &deps::CATALOG[0], &[], &[]).unwrap();
    assert!(!runtime.exists());
    assert!(!config.join("dependencies").exists());
    fs::write(config.join("storage.json"), b"bad").unwrap();
    assert!(deps::runtime(&config, "node", deps::CATALOG[0].version).is_err());
    fs::remove_dir_all(root).unwrap();
}
