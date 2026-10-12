use crate::{Result, model::Settings};
use std::{
    fs,
    path::{Path, PathBuf},
    ptr,
};
use windows_sys::Win32::{
    Foundation::LocalFree,
    Security::Cryptography::{CRYPT_INTEGER_BLOB, CryptProtectData, CryptUnprotectData},
};

pub fn default_root() -> PathBuf {
    PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap_or_default()).join("SuperGatewayGUI")
}
pub struct Store {
    pub root: PathBuf,
}
impl Store {
    pub fn new(root: PathBuf) -> Result<Self> {
        fs::create_dir_all(&root).map_err(|e| e.to_string())?;
        Ok(Self { root })
    }
    pub fn load(&self) -> Result<Settings> {
        let path = self.root.join("settings.dat");
        if !path.exists() {
            return Ok(Settings::default());
        }
        let bytes = protect(&fs::read(&path).map_err(|e| e.to_string())?, false)?;
        serde_json::from_slice(&bytes).map_err(|e| format!("无法读取原配置，原文件未更改：{e}"))
    }
    pub fn save(&self, settings: &Settings) -> Result<()> {
        let file = self.root.join("settings.dat");
        let backup = self.root.join("settings.dat.pre-rust.bak");
        if file.exists() && !backup.exists() {
            fs::copy(&file, backup).map_err(|e| e.to_string())?;
        }
        let data = protect(
            &serde_json::to_vec_pretty(settings).map_err(|e| e.to_string())?,
            true,
        )?;
        let temp = self.root.join("settings.dat.tmp");
        fs::write(&temp, data).map_err(|e| e.to_string())?;
        atomic_replace(&temp, &file)
    }
}
pub fn atomic_replace(from: &Path, to: &Path) -> Result<()> {
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };
    if unsafe {
        MoveFileExW(
            crate::native::wide(from.as_os_str()).as_ptr(),
            crate::native::wide(to.as_os_str()).as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        return Err(crate::native::error("保存文件"));
    }
    Ok(())
}
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let temporary = path.with_extension(format!("{}.tmp", crate::model::new_id()));
    std::fs::write(&temporary, bytes).map_err(|e| e.to_string())?;
    let result = atomic_replace(&temporary, path);
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}
pub fn protect(bytes: &[u8], encrypt: bool) -> Result<Vec<u8>> {
    let input = CRYPT_INTEGER_BLOB {
        cbData: bytes.len() as u32,
        pbData: bytes.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    let ok = unsafe {
        if encrypt {
            CryptProtectData(
                &input,
                ptr::null(),
                ptr::null(),
                ptr::null(),
                ptr::null(),
                1,
                &mut output,
            )
        } else {
            CryptUnprotectData(
                &input,
                ptr::null_mut(),
                ptr::null(),
                ptr::null(),
                ptr::null(),
                1,
                &mut output,
            )
        }
    };
    if ok == 0 {
        return Err(crate::native::error("DPAPI：请使用原 Windows 账户读取配置"));
    }
    let result =
        unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() };
    unsafe {
        LocalFree(output.pbData.cast());
    }
    Ok(result)
}
