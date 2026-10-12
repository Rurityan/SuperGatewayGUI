//! Data locations are file settings; never changes the process or system environment.
use crate::{
    Result,
    store::{Store, atomic_write},
};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
struct Locations {
    configuration_directory: Option<PathBuf>,
    dependency_directory: Option<PathBuf>,
}
fn read(path: &Path) -> Result<Locations> {
    if !path.exists() {
        return Ok(Locations::default());
    }
    serde_json::from_slice(&fs::read(path).map_err(|e| e.to_string())?)
        .map_err(|e| format!("目录设置读取失败，原文件未更改：{e}"))
}
pub fn absolute(path: &Path) -> Result<PathBuf> {
    if !path.is_absolute() {
        return Err("请选择完整的绝对目录路径".into());
    }
    std::path::absolute(path).map_err(|e| format!("目录路径无效：{e}"))
}
pub fn configuration_directory(bootstrap: &Path, explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(path) = explicit {
        return std::path::absolute(path).map_err(|e| e.to_string());
    }
    match read(&bootstrap.join("locations.json"))?.configuration_directory {
        Some(path) => {
            let path = absolute(&path)?;
            if !path.is_dir() {
                return Err(format!(
                    "已选择的配置目录不可用：{}。请恢复目录，或使用 --data-dir 指定已有配置目录。",
                    path.display()
                ));
            }
            Ok(path)
        }
        None => absolute(bootstrap),
    }
}
pub fn dependency_directory(root: &Path) -> Result<PathBuf> {
    absolute(
        &read(&root.join("storage.json"))?
            .dependency_directory
            .unwrap_or_else(|| root.join("dependencies")),
    )
}
pub fn set_dependency_directory(root: &Path, dependencies: &Path) -> Result<()> {
    atomic_write(
        &root.join("storage.json"),
        &serde_json::to_vec_pretty(&Locations {
            dependency_directory: Some(absolute(dependencies)?),
            ..Default::default()
        })
        .map_err(|e| e.to_string())?,
    )
}
fn writable(path: &Path) -> Result<()> {
    fs::create_dir_all(path).map_err(|e| format!("无法创建目录 {}：{e}", path.display()))?;
    let probe = path.join(format!(".sgui-write-{}.tmp", crate::model::new_id()));
    fs::write(&probe, []).map_err(|e| format!("目录不可写 {}：{e}", path.display()))?;
    fs::remove_file(&probe).map_err(|e| e.to_string())
}
fn same(a: &Path, b: &Path) -> bool {
    a.to_string_lossy()
        .eq_ignore_ascii_case(&b.to_string_lossy())
}
fn copy(source: &Path, target: &Path) -> Result<()> {
    let meta = fs::symlink_metadata(source).map_err(|e| e.to_string())?;
    use std::os::windows::fs::MetadataExt;
    if meta.file_attributes() & 0x400 != 0 {
        return Err(format!(
            "配置目录包含链接，不能自动复制：{}",
            source.display()
        ));
    }
    if meta.is_dir() {
        fs::create_dir_all(target).map_err(|e| e.to_string())?;
        for entry in fs::read_dir(source).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            copy(&entry.path(), &target.join(entry.file_name()))?;
        }
    } else {
        fs::copy(source, target).map_err(|e| e.to_string())?;
    }
    Ok(())
}
/// Prepare new locations after the caller has stopped gateways and installation tasks.
/// Config and TLS are copied; original directories and all dependencies stay untouched.
pub fn prepare(old: &Path, configuration: &Path, dependencies: &Path) -> Result<PathBuf> {
    let configuration = absolute(configuration)?;
    let dependencies = absolute(dependencies)?;
    writable(&configuration)?;
    let old = old.canonicalize().map_err(|e| e.to_string())?;
    let configuration = configuration.canonicalize().map_err(|e| e.to_string())?;
    let changing = !same(&old, &configuration);
    if changing {
        if configuration.starts_with(&old) || old.starts_with(&configuration) {
            return Err("新旧配置目录不能互相包含，请选择独立目录".into());
        }
        if fs::read_dir(&configuration)
            .map_err(|e| e.to_string())?
            .next()
            .is_some()
        {
            return Err("新的配置目录必须为空，避免覆盖已有配置；原目录未修改".into());
        }
    }
    writable(&dependencies)?;
    let dependencies = dependencies.canonicalize().map_err(|e| e.to_string())?;
    if configuration.starts_with(&dependencies) {
        return Err("配置目录不能位于依赖目录内，请分别选择目录".into());
    }
    let mut copied = vec![];
    let result = (|| {
        if changing {
            // Copy only application-owned persistent configuration, never runtimes/cache.
            for entry in fs::read_dir(&old).map_err(|e| e.to_string())? {
                let entry = entry.map_err(|e| e.to_string())?;
                let name = entry.file_name().to_string_lossy().into_owned();
                if name == "settings.dat"
                    || name.starts_with("settings.dat.") && !name.ends_with(".tmp")
                    || name == "tls"
                    || name.starts_with("tls-backup-")
                {
                    let target = configuration.join(entry.file_name());
                    copied.push(target.clone());
                    copy(&entry.path(), &target)?;
                }
            }
            Store::new(configuration.clone())?.load()?;
        }
        set_dependency_directory(&configuration, &dependencies)
    })();
    if let Err(error) = result {
        // Every listed target was created in a previously empty, canonical new directory.
        for target in copied {
            if target.is_dir() {
                let _ = fs::remove_dir_all(target);
            } else {
                let _ = fs::remove_file(target);
            }
        }
        return Err(error);
    }
    Ok(configuration)
}
pub fn remember_configuration(bootstrap: &Path, configuration: &Path) -> Result<()> {
    writable(bootstrap)?;
    atomic_write(
        &bootstrap.join("locations.json"),
        &serde_json::to_vec_pretty(&Locations {
            configuration_directory: Some(absolute(configuration)?),
            ..Default::default()
        })
        .map_err(|e| e.to_string())?,
    )
}
