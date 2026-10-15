//! App-owned runtimes and isolated MCP installations. Never alters the parent's environment.
use crate::{
    Result,
    model::{Pair, Profile},
    native::{Child, wide},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    ptr::{null, null_mut},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};
use windows_sys::Win32::Networking::WinHttp::*;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "PascalCase", default)]
pub struct ServiceDependency {
    pub kind: String,
    pub package: String,
    pub version: String,
    pub entry: String,
    #[serde(
        deserialize_with = "crate::arguments::deserialize",
        serialize_with = "crate::arguments::serialize"
    )]
    pub args: Vec<String>,
    pub node: String,
    pub uv: String,
    pub python: String,
    pub installed: String,
}
impl Default for ServiceDependency {
    fn default() -> Self {
        Self {
            kind: "npm".into(),
            package: String::new(),
            version: String::new(),
            entry: String::new(),
            args: vec![],
            node: "24.21.0".into(),
            uv: "0.13.0".into(),
            python: "3.13.16-20261009".into(),
            installed: String::new(),
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "PascalCase", default)]
pub struct Network {
    pub proxy: String,
    pub username: String,
    pub password: String,
    pub npm_registry: String,
    pub python_index: String,
}
impl Default for Network {
    fn default() -> Self {
        Self {
            proxy: String::new(),
            username: String::new(),
            password: String::new(),
            npm_registry: "https://registry.npmjs.org".into(),
            python_index: "https://pypi.org/simple".into(),
        }
    }
}
impl Network {
    pub fn validate(&self) -> Result<()> {
        for url in [&self.npm_registry, &self.python_index] {
            let parsed = crate::net::Url::parse(url)?;
            if parsed.path.contains(['?', '#']) {
                return Err("包源不支持查询参数或 URL 认证；请填写公开的包源地址".into());
            }
        }
        if !self.proxy.is_empty() {
            let url = crate::net::Url::parse(&self.proxy)?;
            if url.secure || url.path != "/" {
                return Err(
                    "代理须为 http://主机:端口，不含路径；认证请使用独立用户名和密码字段".into(),
                );
            }
        }
        if [
            &self.username,
            &self.password,
            &self.proxy,
            &self.npm_registry,
            &self.python_index,
        ]
        .iter()
        .any(|v| v.contains(['\r', '\n', '\0']))
        {
            return Err("网络设置不能包含换行或 NUL".into());
        }
        Ok(())
    }
    fn proxy_url(&self) -> String {
        if self.username.is_empty() {
            return self.proxy.clone();
        }
        fn escape(v: &str) -> String {
            v.bytes()
                .map(|b| {
                    if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                        (b as char).to_string()
                    } else {
                        format!("%{b:02X}")
                    }
                })
                .collect()
        }
        self.proxy.replacen(
            "http://",
            &format!(
                "http://{}:{}@",
                escape(&self.username),
                escape(&self.password)
            ),
            1,
        )
    }
    fn redact(&self, text: &str) -> String {
        let mut result = text.to_string();
        for secret in [&self.password, &self.proxy_url()] {
            if !secret.is_empty() {
                result = result.replace(secret, "[已隐藏]");
            }
        }
        result
    }
}
pub struct Runtime {
    pub kind: &'static str,
    pub version: &'static str,
    pub url: &'static str,
    pub sha256: &'static str,
    pub archive: &'static str,
    pub prefix: &'static str,
    pub executable: &'static str,
}
pub const CATALOG: &[Runtime] = &[
    Runtime {
        kind: "node",
        version: "24.21.0",
        url: "https://nodejs.org/dist/v24.21.0/node-v24.21.0-win-x64.zip",
        sha256: "158f7685b44de51f6c0df1d153526cbcd3e1bc739a8dfc607721cef75de9e541",
        archive: "zip",
        prefix: "node-v24.21.0-win-x64",
        executable: "node.exe",
    },
    Runtime {
        kind: "uv",
        version: "0.13.0",
        url: "https://github.com/astral-sh/uv/releases/download/0.13.0/uv-x86_64-pc-windows-msvc.zip",
        sha256: "088962f9e7b7bd9ea740c04c650b2a21c8928c345bd99ac24350dc924dba656c",
        archive: "zip",
        prefix: "",
        executable: "uv.exe",
    },
    Runtime {
        kind: "python",
        version: "3.13.16-20261009",
        url: "https://github.com/astral-sh/python-build-standalone/releases/download/20261009/cpython-3.13.16%2B20261009-x86_64-pc-windows-msvc-install_only_stripped.tar.gz",
        sha256: "3a7adfb69e3ff02a41ed38ac637808532b438ab3c8619af46ca5dbec5c1f5190",
        archive: "tar.gz",
        prefix: "python",
        executable: "python.exe",
    },
];
pub fn runtime(root: &Path, kind: &str, version: &str) -> Result<PathBuf> {
    if !CATALOG
        .iter()
        .any(|r| r.kind == kind && r.version == version)
    {
        return Err(format!("运行环境版本不在固定清单中：{kind} {version}"));
    }
    Ok(crate::storage::dependency_directory(root)?
        .join("runtimes")
        .join(kind)
        .join(version))
}
fn safe_component(s: &str) -> Result<()> {
    if s.is_empty()
        || s.len() > 128
        || !s
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err("依赖目录标识无效".into());
    }
    Ok(())
}
fn service_root(root: &Path, id: &str) -> Result<PathBuf> {
    safe_component(id)?;
    Ok(crate::storage::dependency_directory(root)?
        .join("services")
        .join(id))
}
pub fn installed_path(root: &Path, p: &Profile) -> Result<PathBuf> {
    safe_component(&p.dependency.installed)?;
    Ok(service_root(root, &p.id)?.join(&p.dependency.installed))
}
pub fn validate_spec(d: &ServiceDependency) -> Result<()> {
    if !["npm", "python"].contains(&d.kind.as_str()) {
        return Err("依赖类型须为 npm 或 Python".into());
    }
    let name = &d.package;
    let allowed = |s: &str| {
        !s.is_empty()
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
    };
    let valid = if d.kind == "npm" && name.starts_with('@') {
        name[1..]
            .split_once('/')
            .is_some_and(|(a, b)| allowed(a) && allowed(b))
    } else {
        allowed(name)
    };
    if !valid || name.starts_with(['-', '.']) {
        return Err("请填写包名称，不支持 URL、本地路径或安装选项".into());
    }
    if !d.version.is_empty()
        && (d.version.starts_with('-')
            || !d
                .version
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._+-".contains(&b)))
    {
        return Err("版本须为确切版本号；留空在安装时解析当前版本".into());
    }
    if !d.version.is_empty() {
        let base = d.version.split(['-', '+']).next().unwrap();
        if !d.version.as_bytes()[0].is_ascii_digit()
            || d.kind == "npm"
                && (base.split('.').count() != 3
                    || base
                        .split('.')
                        .any(|v| v.is_empty() || !v.bytes().all(|b| b.is_ascii_digit())))
        {
            return Err(
                "请填写确切版本（npm 例如 1.2.3）；不支持 latest / 范围，留空选择当前版本".into(),
            );
        }
    }
    if !d.entry.is_empty() && (!allowed(&d.entry) || d.entry.starts_with(['-', '.'])) {
        return Err("启动入口须为包提供的命令名，不含路径或 shell 表达式".into());
    }
    Ok(())
}
pub fn infer(p: &Profile) -> Option<ServiceDependency> {
    if p.source != "local" || !p.stdio.is_empty() || Path::new(&p.command).is_absolute() {
        return None;
    }
    let command = p.command.to_ascii_lowercase();
    let kind = match command.as_str() {
        "npx" | "npx.cmd" => "npm",
        "uvx" | "uvx.exe" => "python",
        _ => return None,
    };
    let mut args = p.args.iter();
    let mut package = args.next()?;
    if kind == "npm" && ["-y", "--yes"].contains(&package.as_str()) {
        package = args.next()?;
    }
    if package.starts_with('-') {
        return None;
    }
    let (name, version) = if kind == "npm" {
        package
            .rsplit_once('@')
            .filter(|(a, _)| !a.is_empty())
            .unwrap_or((package, ""))
    } else {
        package.split_once("==").unwrap_or((package, ""))
    };
    let d = ServiceDependency {
        kind: kind.into(),
        package: name.into(),
        version: version.into(),
        args: args.cloned().collect(),
        ..Default::default()
    };
    validate_spec(&d).ok()?;
    Some(d)
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Receipt {
    pub spec: ServiceDependency,
    pub resolved_version: String,
    pub executable: String,
    pub args: Vec<String>,
}
pub fn receipt(root: &Path, p: &Profile) -> Result<Receipt> {
    let dir = installed_path(root, p)
        .map_err(|_| "依赖尚未安装，请点击配置包与安装依赖 → 安装".to_string())?;
    let r: Receipt = serde_json::from_slice(
        &fs::read(dir.join("installed.json"))
            .map_err(|_| "依赖尚未安装或目录已移动，请前往安装".to_string())?,
    )
    .map_err(|e| e.to_string())?;
    if Path::new(&r.executable).is_absolute()
        || Path::new(&r.executable)
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        return Err("依赖安装记录包含不安全的入口路径，请重新安装".into());
    }
    // Changing the requested specification cannot silently keep launching stale packages.
    if r.spec.kind != p.dependency.kind
        || r.spec.package != p.dependency.package
        || r.spec.version != p.dependency.version
        || r.spec.entry != p.dependency.entry
        || r.spec.node != p.dependency.node
        || r.spec.uv != p.dependency.uv
        || r.spec.python != p.dependency.python
    {
        return Err("依赖配置已变更，请先安装，再应用并重启".into());
    }
    Ok(r)
}
/// Read-only availability check for the UI; never prepares directories or installs anything.
pub fn check_installation(root: &Path, p: &Profile) -> Result<()> {
    let r = receipt(root, p)?;
    let kinds = if p.dependency.kind == "npm" {
        vec![("node", p.dependency.node.as_str())]
    } else {
        vec![
            ("uv", p.dependency.uv.as_str()),
            ("python", p.dependency.python.as_str()),
        ]
    };
    for (kind, version) in kinds {
        let dir = runtime(root, kind, version)?;
        let entry = CATALOG
            .iter()
            .find(|r| r.kind == kind && r.version == version)
            .ok_or("运行环境版本不在固定清单中，请重新选择")?;
        if !dir.join(entry.executable).is_file() {
            return Err(format!(
                "缺少托管 {kind} {version}，请前往运行环境与依赖安装"
            ));
        }
    }
    if !installed_path(root, p)?.join(r.executable).is_file() {
        return Err("托管启动入口缺失，请重新安装依赖".into());
    }
    Ok(())
}
pub fn prepared(root: &Path, p: &Profile) -> Result<Profile> {
    if !p.managed || p.source != "local" {
        let mut result = p.clone();
        crate::arguments::normalize(&mut result.args);
        return Ok(result);
    }
    let mut result = p.clone();
    let mut env = environment(root, &p.dependency, &Network::default(), false)?;
    if p.stdio.is_empty() {
        let r = receipt(root, p)?;
        let dir = installed_path(root, p)?;
        result.command = if p.dependency.kind == "npm" {
            runtime(root, "node", &p.dependency.node)?.join("node.exe")
        } else {
            dir.join(&r.executable)
        }
        .to_string_lossy()
        .into_owned();
        result.args = if p.dependency.kind == "npm" {
            vec![dir.join(&r.executable).to_string_lossy().into_owned()]
        } else {
            r.args
        };
        result.args.extend(p.dependency.args.clone());
        crate::arguments::normalize(&mut result.args);
        if !Path::new(&result.command).is_file() {
            return Err("托管启动入口缺失，请重新安装依赖".into());
        }
        if p.dependency.kind == "npm" && !Path::new(&result.args[0]).is_file() {
            return Err("npm 启动入口缺失，请重新安装依赖".into());
        }
        if p.dependency.kind == "python" {
            env.insert(
                "VIRTUAL_ENV".into(),
                dir.join("venv").to_string_lossy().into_owned(),
            );
            env.insert(
                "UV_PYTHON".into(),
                dir.join("venv/Scripts/python.exe")
                    .to_string_lossy()
                    .into_owned(),
            );
            env.insert(
                "PATH".into(),
                format!("{};{}", dir.join("venv/Scripts").display(), env["PATH"]),
            );
            let cfg = fs::read_to_string(dir.join("venv/pyvenv.cfg"))
                .map_err(|_| "Python 虚拟环境配置缺失，请重新安装依赖")?;
            let home = cfg.lines().find_map(|line| line.strip_prefix("home = "));
            if home.map(PathBuf::from).and_then(|p| p.canonicalize().ok())
                != runtime(root, "python", &p.dependency.python)?
                    .canonicalize()
                    .ok()
            {
                return Err(
                    "Python 虚拟环境包含原始绝对路径；数据目录已移动，请重新安装此服务依赖".into(),
                );
            }
        }
    }
    // Reject ambiguous service overrides of variables required for isolation.
    for (key, value) in env {
        if p.env.iter().any(|e| e.key.eq_ignore_ascii_case(&key)) {
            return Err(format!(
                "托管模式保留环境变量 {key}；请删除服务中的同名变量或切回原有模式"
            ));
        }
        result.env.push(Pair {
            key,
            value,
            ..Default::default()
        });
    }
    result.managed = false;
    result.managed_runtime = true;
    Ok(result)
}
pub fn operational_variable(key: &str) -> bool {
    [
        "PATH",
        "npm_config_cache",
        "npm_config_prefix",
        "npm_config_userconfig",
        "npm_config_globalconfig",
        "UV_CACHE_DIR",
        "UV_PYTHON_DOWNLOADS",
        "UV_PYTHON_PREFERENCE",
        "UV_NO_CONFIG",
        "PYTHONNOUSERSITE",
        "UV_PYTHON",
        "PYTHONPATH",
        "PYTHONHOME",
        "VIRTUAL_ENV",
        "ALL_PROXY",
        "NO_PROXY",
        "npm_config_node_options",
        "NODE_OPTIONS",
        "NODE_PATH",
        "npm_config_offline",
        "UV_OFFLINE",
        "UV_TOOL_DIR",
        "UV_TOOL_BIN_DIR",
        "UV_PYTHON_INSTALL_DIR",
        "TEMP",
        "TMP",
    ]
    .iter()
    .any(|name| key.eq_ignore_ascii_case(name))
}
pub fn environment(
    root: &Path,
    d: &ServiceDependency,
    n: &Network,
    install: bool,
) -> Result<BTreeMap<String, String>> {
    environment_inner(root, d, n, install, true)
}
pub fn preview_environment(root: &Path, d: &ServiceDependency) -> Result<BTreeMap<String, String>> {
    environment_inner(root, d, &Network::default(), false, false)
}
fn environment_inner(
    root: &Path,
    d: &ServiceDependency,
    n: &Network,
    install: bool,
    prepare: bool,
) -> Result<BTreeMap<String, String>> {
    let dep = crate::storage::dependency_directory(root)?;
    let mut paths = vec![];
    let kinds = if d.kind == "npm" {
        vec![("node", d.node.as_str())]
    } else {
        vec![("uv", d.uv.as_str()), ("python", d.python.as_str())]
    };
    for (kind, version) in kinds {
        let dir = runtime(root, kind, version)?;
        let r = CATALOG
            .iter()
            .find(|r| r.kind == kind && r.version == version)
            .unwrap();
        if prepare && !dir.join(r.executable).is_file() {
            return Err(format!(
                "缺少托管 {kind} {version}，请前往运行环境与依赖安装"
            ));
        }
        paths.push(dir);
    }
    // System directories supply cmd and Windows tools; no system Node/Python fallback.
    let system =
        PathBuf::from(std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into()));
    paths.push(system.join("System32"));
    paths.push(system.clone());
    paths.push(system.join("System32/WindowsPowerShell/v1.0"));
    let mut env = BTreeMap::new();
    if prepare {
        fs::create_dir_all(dep.join("scratch/temp"))
            .map_err(|e| format!("创建托管临时目录失败：{e}"))?;
    }
    for key in ["TEMP", "TMP"] {
        env.insert(
            key.into(),
            dep.join("scratch/temp").to_string_lossy().into(),
        );
    }
    env.insert(
        "UV_TOOL_DIR".into(),
        dep.join("scratch/uv-tools").to_string_lossy().into(),
    );
    env.insert(
        "UV_TOOL_BIN_DIR".into(),
        dep.join("scratch/uv-bin").to_string_lossy().into(),
    );
    env.insert(
        "UV_PYTHON_INSTALL_DIR".into(),
        dep.join("runtimes/python").to_string_lossy().into(),
    );
    if d.kind == "python" {
        env.insert(
            "UV_PYTHON".into(),
            runtime(root, "python", &d.python)?
                .join("python.exe")
                .to_string_lossy()
                .into(),
        );
    }
    env.insert(
        "PATH".into(),
        paths
            .iter()
            .map(|p| p.to_string_lossy())
            .collect::<Vec<_>>()
            .join(";"),
    );
    env.insert(
        "npm_config_cache".into(),
        dep.join("cache/npm").to_string_lossy().into(),
    );
    env.insert(
        "npm_config_prefix".into(),
        dep.join("scratch/npm-global").to_string_lossy().into(),
    );
    env.insert(
        "npm_config_userconfig".into(),
        dep.join("empty.npmrc").to_string_lossy().into(),
    );
    env.insert(
        "npm_config_globalconfig".into(),
        dep.join("empty-global.npmrc").to_string_lossy().into(),
    );
    env.insert(
        "UV_CACHE_DIR".into(),
        dep.join("cache/uv").to_string_lossy().into(),
    );
    env.insert("UV_PYTHON_DOWNLOADS".into(), "never".into());
    env.insert("UV_PYTHON_PREFERENCE".into(), "only-managed".into());
    env.insert("UV_NO_CONFIG".into(), "1".into());
    env.insert("PYTHONNOUSERSITE".into(), "1".into());
    for key in [
        "PYTHONPATH",
        "PYTHONHOME",
        "VIRTUAL_ENV",
        "ALL_PROXY",
        "NO_PROXY",
        "npm_config_node_options",
        "NODE_OPTIONS",
        "NODE_PATH",
    ] {
        env.insert(key.into(), String::new());
    }
    if !install {
        env.insert("npm_config_offline".into(), "true".into());
        env.insert("UV_OFFLINE".into(), "1".into());
    } else {
        env.insert("npm_config_registry".into(), n.npm_registry.clone());
        env.insert("UV_DEFAULT_INDEX".into(), n.python_index.clone());
        if !n.proxy.is_empty() {
            let proxy = n.proxy_url();
            for key in [
                "HTTP_PROXY",
                "HTTPS_PROXY",
                "npm_config_proxy",
                "npm_config_https_proxy",
            ] {
                env.insert(key.into(), proxy.clone());
            }
        } else {
            for key in [
                "HTTP_PROXY",
                "HTTPS_PROXY",
                "ALL_PROXY",
                "npm_config_proxy",
                "npm_config_https_proxy",
            ] {
                env.insert(key.into(), String::new());
            }
        }
    }
    Ok(env)
}
pub struct Task {
    pub cancel: AtomicBool,
    busy: Mutex<()>,
}
impl Default for Task {
    fn default() -> Self {
        Self {
            cancel: AtomicBool::new(false),
            busy: Mutex::new(()),
        }
    }
}
impl Task {
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }
    pub fn cancel_and_wait(&self) {
        self.cancel();
        let _guard = self.busy.lock().unwrap();
    }
    pub fn check(&self) -> Result<()> {
        if self.cancel.load(Ordering::SeqCst) {
            Err("操作已取消".into())
        } else {
            Ok(())
        }
    }
    pub fn execute<T>(&self, f: impl FnOnce() -> Result<T>) -> Result<T> {
        let _guard = self.busy.try_lock().map_err(|_| "另一个依赖操作正在进行")?;
        let result = if self.cancel.load(Ordering::SeqCst) {
            Err("操作已取消".into())
        } else {
            f()
        };
        self.cancel.store(false, Ordering::SeqCst);
        result
    }
    pub fn prepare(&self) -> Result<()> {
        let _guard = self.busy.try_lock().map_err(|_| "另一个依赖操作正在进行")?;
        self.cancel.store(false, Ordering::SeqCst);
        Ok(())
    }
}
struct Internet(usize);
impl Internet {
    fn raw(&self) -> *mut std::ffi::c_void {
        self.0 as _
    }
    fn new(h: *mut std::ffi::c_void) -> Result<Self> {
        if h.is_null() {
            Err(crate::native::error("下载网络请求"))
        } else {
            Ok(Self(h as usize))
        }
    }
}
impl Drop for Internet {
    fn drop(&mut self) {
        unsafe {
            WinHttpCloseHandle(self.raw());
        }
    }
}
// Bounded streaming download. A request watchdog closes it on cancellation, including blocked reads.
pub fn download(
    url: &str,
    to: &Path,
    network: &Network,
    task: &Task,
    progress: &impl Fn(String),
) -> Result<()> {
    network.validate()?;
    task.check()?;
    let proxy = if network.proxy.is_empty() {
        String::new()
    } else {
        let u = crate::net::Url::parse(&network.proxy)?;
        if u.host.contains(':') {
            format!("[{}]:{}", u.host, u.port)
        } else {
            format!("{}:{}", u.host, u.port)
        }
    };
    let proxy_wide = wide(&proxy);
    let session = Internet::new(unsafe {
        WinHttpOpen(
            wide("SuperGatewayGUI dependencies/2").as_ptr(),
            if proxy.is_empty() {
                WINHTTP_ACCESS_TYPE_NO_PROXY
            } else {
                WINHTTP_ACCESS_TYPE_NAMED_PROXY
            },
            if proxy.is_empty() {
                null()
            } else {
                proxy_wide.as_ptr()
            },
            null(),
            0,
        )
    })?;
    unsafe {
        WinHttpSetTimeouts(session.raw(), 15000, 15000, 15000, 15000);
    }
    let u = crate::net::Url::parse(url)?;
    if !u.secure {
        return Err("运行环境下载只允许 HTTPS".into());
    }
    let connection =
        Internet::new(unsafe { WinHttpConnect(session.raw(), wide(&u.host).as_ptr(), u.port, 0) })?;
    let request = Internet::new(unsafe {
        WinHttpOpenRequest(
            connection.raw(),
            wide("GET").as_ptr(),
            wide(&u.path).as_ptr(),
            null(),
            null(),
            null(),
            WINHTTP_FLAG_SECURE,
        )
    })?;
    // Block HTTPS -> HTTP redirects, while allowing release CDN redirects.
    let policy = WINHTTP_OPTION_REDIRECT_POLICY_DISALLOW_HTTPS_TO_HTTP;
    unsafe {
        WinHttpSetOption(
            request.raw(),
            WINHTTP_OPTION_REDIRECT_POLICY,
            (&policy as *const u32).cast(),
            4,
        );
    }
    if !network.username.is_empty()
        && unsafe {
            WinHttpSetCredentials(
                request.raw(),
                WINHTTP_AUTH_TARGET_PROXY,
                WINHTTP_AUTH_SCHEME_BASIC,
                wide(&network.username).as_ptr(),
                wide(&network.password).as_ptr(),
                null_mut(),
            )
        } == 0
    {
        return Err(crate::native::error("设置代理认证"));
    }
    let done = Arc::new(AtomicBool::new(false));
    let closed = Arc::new(AtomicBool::new(false));
    let raw = request.0;
    let result = thread::scope(|scope| {
        let done2 = done.clone();
        let closed2 = closed.clone();
        scope.spawn(move || {
            while !done2.load(Ordering::SeqCst) {
                if task.cancel.load(Ordering::SeqCst) {
                    if !closed2.swap(true, Ordering::SeqCst) {
                        unsafe {
                            WinHttpCloseHandle(raw as _);
                        }
                    }
                    break;
                }
                thread::sleep(Duration::from_millis(50));
            }
        });
        let result = (|| {
            if unsafe { WinHttpSendRequest(request.raw(), null(), 0, null(), 0, 0, 0) } == 0
                || unsafe { WinHttpReceiveResponse(request.raw(), null_mut()) } == 0
            {
                task.check()?;
                return Err(crate::native::error("下载失败，请检查代理或网络"));
            }
            let mut status = 0u32;
            let mut len = 4;
            if unsafe {
                WinHttpQueryHeaders(
                    request.raw(),
                    WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
                    null(),
                    (&mut status as *mut u32).cast(),
                    &mut len,
                    null_mut(),
                )
            } == 0
            {
                return Err(crate::native::error("读取下载状态"));
            }
            if status != 200 {
                return Err(format!(
                    "下载返回 HTTP {status}{}",
                    if status == 407 {
                        "，请检查代理认证"
                    } else {
                        ""
                    }
                ));
            }
            let mut output = fs::File::create(to).map_err(|e| e.to_string())?;
            let mut buffer = [0; 65536];
            let mut count = 0u64;
            let mut next = 0;
            loop {
                task.check()?;
                let mut read = 0;
                if unsafe {
                    WinHttpReadData(
                        request.raw(),
                        buffer.as_mut_ptr().cast(),
                        buffer.len() as u32,
                        &mut read,
                    )
                } == 0
                {
                    task.check()?;
                    return Err(crate::native::error("下载中断"));
                }
                if read == 0 {
                    break;
                }
                count += read as u64;
                if count > 512 * 1024 * 1024 {
                    return Err("下载超过 512 MiB 上限".into());
                }
                output
                    .write_all(&buffer[..read as usize])
                    .map_err(|e| format!("写入下载文件失败，请检查磁盘空间：{e}"))?;
                if count >= next {
                    progress(format!("已下载 {:.1} MiB", count as f64 / 1048576.));
                    next = count + 1024 * 1024;
                }
            }
            output.sync_all().map_err(|e| e.to_string())?;
            Ok(())
        })();
        done.store(true, Ordering::SeqCst);
        result
    });
    if closed.load(Ordering::SeqCst) {
        std::mem::forget(request);
    }
    if result.is_err() {
        let _ = fs::remove_file(to);
    }
    result
}
fn hash_file(path: &Path) -> Result<String> {
    let mut input = fs::File::open(path).map_err(|e| e.to_string())?;
    let mut hash = Sha256::new();
    let mut buffer = [0; 65536];
    loop {
        let n = input.read(&mut buffer).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
    }
    Ok(format!("{:x}", hash.finalize()))
}
pub fn verify_archive(path: &Path, expected: &str) -> Result<()> {
    if hash_file(path)?.eq_ignore_ascii_case(expected) {
        Ok(())
    } else {
        Err("运行环境 SHA256 校验失败，未解压或安装".into())
    }
}
fn checked_archive_path(path: &Path) -> Result<()> {
    use std::path::Component;
    if path
        .components()
        .any(|c| !matches!(c, Component::Normal(_)))
        || path.to_string_lossy().contains([':', '\\'])
    {
        return Err("压缩包包含不安全的路径".into());
    }
    Ok(())
}
pub fn extract(archive: &Path, format: &str, target: &Path, task: &Task) -> Result<()> {
    fs::create_dir_all(target).map_err(|e| e.to_string())?;
    let mut total = 0u64;
    if format == "zip" {
        let mut archive = zip::ZipArchive::new(fs::File::open(archive).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        for i in 0..archive.len() {
            task.check()?;
            let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
            let path = entry.enclosed_name().ok_or("压缩包路径越界")?;
            checked_archive_path(&path)?;
            if entry.is_symlink() {
                return Err("运行环境 ZIP 不允许符号链接".into());
            }
            total += entry.size();
            if total > 2 * 1024 * 1024 * 1024 {
                return Err("解压超过 2 GiB 上限".into());
            }
            let dest = target.join(path);
            if entry.is_dir() {
                fs::create_dir_all(dest).map_err(|e| e.to_string())?;
            } else {
                fs::create_dir_all(dest.parent().unwrap()).map_err(|e| e.to_string())?;
                std::io::copy(
                    &mut entry,
                    &mut fs::File::create(dest).map_err(|e| e.to_string())?,
                )
                .map_err(|e| e.to_string())?;
            }
        }
    } else {
        let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(
            fs::File::open(archive).map_err(|e| e.to_string())?,
        ));
        for entry in archive.entries().map_err(|e| e.to_string())? {
            task.check()?;
            let mut entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path().map_err(|e| e.to_string())?.into_owned();
            checked_archive_path(&path)?;
            if !entry.header().entry_type().is_file() && !entry.header().entry_type().is_dir() {
                return Err("运行环境 TAR 不允许链接或特殊文件".into());
            }
            total += entry.size();
            if total > 2 * 1024 * 1024 * 1024 {
                return Err("解压超过 2 GiB 上限".into());
            }
            // Preserve bytes, not Unix read-only directory attributes on Windows.
            let dest = target.join(&path);
            if entry.header().entry_type().is_dir() {
                fs::create_dir_all(&dest)
                    .map_err(|e| format!("解压目录 {}：{e}", path.display()))?;
            } else {
                fs::create_dir_all(dest.parent().unwrap()).map_err(|e| e.to_string())?;
                std::io::copy(
                    &mut entry,
                    &mut fs::File::create(&dest)
                        .map_err(|e| format!("解压文件 {}：{e}", path.display()))?,
                )
                .map_err(|e| e.to_string())?;
            }
        }
    }
    Ok(())
}
struct Cleanup(PathBuf);
impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
pub fn install_runtime(
    root: &Path,
    r: &Runtime,
    n: &Network,
    task: &Task,
    progress: &impl Fn(String),
) -> Result<()> {
    let dest = runtime(root, r.kind, r.version)?;
    if dest.join(r.executable).is_file() {
        progress("此运行环境已经安装".into());
        return Ok(());
    }
    let staging = crate::storage::dependency_directory(root)?
        .join("staging")
        .join(crate::model::new_id());
    fs::create_dir_all(&staging).map_err(|e| e.to_string())?;
    let _cleanup = Cleanup(staging.clone());
    let archive = staging.join("download");
    progress(format!(
        "下载 {} {}（官方源，SHA256 校验）",
        r.kind, r.version
    ));
    download(r.url, &archive, n, task, progress)?;
    verify_archive(&archive, r.sha256)?;
    progress("校验通过，正在解压…".into());
    extract(&archive, r.archive, &staging.join("extracted"), task)?;
    task.check()?;
    let extracted = staging.join("extracted").join(r.prefix);
    if !extracted.join(r.executable).is_file() {
        return Err("压缩包内缺少预期程序".into());
    }
    fs::write(
        extracted.join("runtime.json"),
        serde_json::to_vec_pretty(
            &json!({"Kind":r.kind,"Version":r.version,"Source":r.url,"Sha256":r.sha256}),
        )
        .unwrap(),
    )
    .map_err(|e| e.to_string())?;
    fs::create_dir_all(dest.parent().unwrap()).map_err(|e| e.to_string())?;
    let licenses = extracted.join("SUPERGATEWAY-LICENSES");
    fs::create_dir_all(&licenses).map_err(|e| e.to_string())?;
    let notices: &[(&str, &str)] = match r.kind {
        "uv" => &[
            (
                "LICENSE-MIT.txt",
                include_str!("../assets/dependencies-licenses/uv-MIT.txt"),
            ),
            (
                "LICENSE-APACHE.txt",
                include_str!("../assets/dependencies-licenses/uv-APACHE.txt"),
            ),
        ],
        "python" => &[
            (
                "PYTHON-LICENSE.txt",
                include_str!("../assets/dependencies-licenses/python.txt"),
            ),
            (
                "STANDALONE-LICENSE.txt",
                include_str!("../assets/dependencies-licenses/python-build-standalone.txt"),
            ),
        ],
        _ => &[],
    };
    for (name, text) in notices {
        fs::write(licenses.join(name), text).map_err(|e| e.to_string())?;
    }
    fs::rename(extracted, dest).map_err(|e| e.to_string())?;
    progress("运行环境安装完成".into());
    Ok(())
}
fn run(
    exe: &Path,
    args: &[String],
    env: &BTreeMap<String, String>,
    cwd: &Path,
    n: &Network,
    task: &Task,
    progress: &impl Fn(String),
) -> Result<String> {
    task.check()?;
    let mut child = Child::spawn(exe, args, env, cwd)?;
    child.input.take();
    let (tx, rx) = std::sync::mpsc::channel();
    let mut readers = vec![];
    for pipe in [child.output.take().unwrap(), child.errors.take().unwrap()] {
        let tx = tx.clone();
        readers.push(thread::spawn(move || {
            use std::io::BufRead;
            let mut reader = std::io::BufReader::new(pipe);
            loop {
                let mut line = vec![];
                match reader.by_ref().take(16385).read_until(b'\n', &mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        let _ = tx.send(String::from_utf8_lossy(&line).into_owned());
                    }
                }
            }
        }));
    }
    drop(tx);
    let mut output = String::new();
    let mut cancelled = false;
    let deadline = std::time::Instant::now() + Duration::from_secs(900);
    while !child.exited() {
        if task.check().is_err() || std::time::Instant::now() >= deadline {
            child.terminate();
            cancelled = true;
            break;
        }
        for line in rx.try_iter() {
            let text = n.redact(&line);
            progress(text.trim().to_owned());
            output.push_str(&text);
            if output.len() > 1024 * 1024 {
                output = output
                    .chars()
                    .rev()
                    .take(524288)
                    .collect::<String>()
                    .chars()
                    .rev()
                    .collect();
            }
        }
        thread::sleep(Duration::from_millis(50));
    }
    // Successful installers must not leave helpers holding the output pipes open.
    let exit_code = child.exit_code();
    child.terminate();
    for reader in readers {
        let _ = reader.join();
    }
    for line in rx.try_iter() {
        let text = n.redact(&line);
        progress(text.trim().to_owned());
        output.push_str(&text);
    }
    if cancelled {
        return Err("安装已取消，原有依赖保持可用".into());
    }
    if exit_code != 0 {
        return Err(format!("依赖安装失败（退出码 {}），请查看日志", exit_code));
    }
    Ok(output)
}
pub fn install_service(
    root: &Path,
    p: &Profile,
    n: &Network,
    task: &Task,
    progress: &impl Fn(String),
) -> Result<ServiceDependency> {
    validate_spec(&p.dependency)?;
    n.validate()?;
    let d = &p.dependency;
    let env = environment(root, d, n, true)?;
    let base = service_root(root, &p.id)?;
    fs::create_dir_all(&base).map_err(|e| e.to_string())?;
    let id = crate::model::new_id();
    let dir = base.join(&id);
    fs::create_dir(&dir).map_err(|e| e.to_string())?;
    // Python venv launchers embed their absolute location: build at its final unique path.
    // installed.json is the commit marker; errors remove only this unpublished version.
    let mut published = false;
    let result = (|| {
        let (resolved, executable, args) = if d.kind == "npm" {
            let node = runtime(root, "node", &d.node)?;
            fs::write(
                dir.join("package.json"),
                b"{\"private\":true,\"name\":\"supergateway-managed-mcp\",\"version\":\"1.0.0\"}",
            )
            .map_err(|e| e.to_string())?;
            let spec = if d.version.is_empty() {
                d.package.clone()
            } else {
                format!("{}@{}", d.package, d.version)
            };
            run(
                &node.join("node.exe"),
                &[
                    node.join("node_modules/npm/bin/npm-cli.js")
                        .to_string_lossy()
                        .into(),
                    "install".into(),
                    "--save-exact".into(),
                    "--no-audit".into(),
                    "--no-fund".into(),
                    "--".into(),
                    spec,
                ],
                &env,
                &dir,
                n,
                task,
                progress,
            )?;
            let package_dir = dir.join("node_modules").join(&d.package);
            let info: Value = serde_json::from_slice(
                &fs::read(package_dir.join("package.json")).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
            let version = info["version"]
                .as_str()
                .ok_or("npm 包缺少版本")?
                .to_string();
            let bins = &info["bin"];
            let entry = if let Some(path) = bins.as_str() {
                if !d.entry.is_empty() && d.entry != d.package.rsplit('/').next().unwrap() {
                    return Err("指定入口不是包提供的命令".into());
                }
                path
            } else {
                let obj = bins
                    .as_object()
                    .ok_or("npm 包没有可执行入口，请选择 MCP 包")?;
                if d.entry.is_empty() {
                    if obj.len() != 1 {
                        return Err(format!(
                            "包提供多个启动入口，请填写：{}",
                            obj.keys().cloned().collect::<Vec<_>>().join("、")
                        ));
                    }
                    obj.values().next().unwrap().as_str().ok_or("入口无效")?
                } else {
                    obj.get(&d.entry)
                        .and_then(Value::as_str)
                        .ok_or("指定入口不存在")?
                }
            };
            let entry_path = package_dir.join(entry);
            let canonical = entry_path.canonicalize().map_err(|e| e.to_string())?;
            if !canonical.starts_with(dir.canonicalize().map_err(|e| e.to_string())?) {
                return Err("npm 入口越出服务目录".into());
            }
            (
                version,
                entry_path
                    .strip_prefix(&dir)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
                vec![],
            )
        } else {
            let uv = runtime(root, "uv", &d.uv)?.join("uv.exe");
            let python = runtime(root, "python", &d.python)?.join("python.exe");
            run(
                &uv,
                &[
                    "venv".into(),
                    "--python".into(),
                    python.to_string_lossy().into(),
                    dir.join("venv").to_string_lossy().into(),
                ],
                &env,
                &dir,
                n,
                task,
                progress,
            )?;
            let spec = if d.version.is_empty() {
                d.package.clone()
            } else {
                format!("{}=={}", d.package, d.version)
            };
            fs::write(dir.join("requirements.in"), format!("{spec}\n"))
                .map_err(|e| e.to_string())?;
            run(
                &uv,
                &[
                    "pip".into(),
                    "compile".into(),
                    "--python".into(),
                    python.to_string_lossy().into(),
                    "--generate-hashes".into(),
                    "--output-file".into(),
                    "requirements.lock".into(),
                    "requirements.in".into(),
                ],
                &env,
                &dir,
                n,
                task,
                progress,
            )?;
            let interpreter = dir.join("venv/Scripts/python.exe");
            run(
                &uv,
                &[
                    "pip".into(),
                    "sync".into(),
                    "--python".into(),
                    interpreter.to_string_lossy().into(),
                    "--require-hashes".into(),
                    "requirements.lock".into(),
                ],
                &env,
                &dir,
                n,
                task,
                progress,
            )?;
            let script = "import importlib.metadata as m,json,sys;d=m.distribution(sys.argv[1]);print(json.dumps({'version':d.version,'entries':[e.name for e in d.entry_points if e.group=='console_scripts']}))";
            let info = run(
                &interpreter,
                &["-c".into(), script.into(), d.package.clone()],
                &env,
                &dir,
                n,
                task,
                &|_| {},
            )?;
            let info: Value = serde_json::from_str(info.trim()).map_err(|e| e.to_string())?;
            let entries = info["entries"].as_array().ok_or("Python 包入口无效")?;
            let entry = if d.entry.is_empty() {
                if entries.len() != 1 {
                    return Err(format!(
                        "请填写 Python 启动入口：{}",
                        entries
                            .iter()
                            .filter_map(Value::as_str)
                            .collect::<Vec<_>>()
                            .join("、")
                    ));
                }
                entries[0].as_str().ok_or("入口无效")?
            } else {
                if !entries.iter().any(|e| e.as_str() == Some(&d.entry)) {
                    return Err("指定 Python 启动入口不存在".into());
                }
                &d.entry
            };
            let exe = format!("venv/Scripts/{entry}.exe");
            if !dir.join(&exe).is_file() {
                return Err("Python 启动入口文件不存在".into());
            }
            (
                info["version"].as_str().ok_or("Python 包缺少版本")?.into(),
                exe,
                vec![],
            )
        };
        task.check()?;
        let mut spec = d.clone();
        spec.installed = id;
        spec.version = resolved.clone();
        let receipt = Receipt {
            spec: spec.clone(),
            resolved_version: resolved,
            executable,
            args,
        };
        crate::store::atomic_write(
            &dir.join("installed.json"),
            &serde_json::to_vec_pretty(&receipt).unwrap(),
        )?;
        published = true;
        progress(format!(
            "安装成功，固定版本 {}；保存服务后应用并重启",
            receipt.resolved_version
        ));
        Ok(spec)
    })();
    if !published {
        let _ = fs::remove_dir_all(dir);
    }
    result
}
pub fn size(path: &Path) -> u64 {
    let Ok(items) = fs::read_dir(path) else {
        return 0;
    };
    items
        .flatten()
        .map(|i| match i.file_type() {
            Ok(t) if t.is_dir() => size(&i.path()),
            Ok(t) if t.is_file() => i.metadata().map(|m| m.len()).unwrap_or(0),
            _ => 0,
        })
        .sum()
}
pub fn report(root: &Path, profiles: &[Profile], active: &[Profile]) -> String {
    let directory = match crate::storage::dependency_directory(root) {
        Ok(path) => path,
        Err(error) => return error,
    };
    let mut text = format!(
        "管理目录：{}\r\n不会修改用户 / 系统 PATH；导入和启动不自动安装。\r\n\r\n",
        directory.display()
    );
    for r in CATALOG {
        let dir = match runtime(root, r.kind, r.version) {
            Ok(path) => path,
            Err(error) => return error,
        };
        let refs = profiles
            .iter()
            .chain(active)
            .filter(|p| {
                p.managed
                    && match r.kind {
                        "node" => p.dependency.kind == "npm" && p.dependency.node == r.version,
                        "uv" => p.dependency.kind == "python" && p.dependency.uv == r.version,
                        _ => p.dependency.kind == "python" && p.dependency.python == r.version,
                    }
            })
            .map(|p| p.name.clone())
            .collect::<std::collections::BTreeSet<_>>();
        text += &format!(
            "{} {}：{}，{:.1} MiB\r\n  {}\r\n  引用：{}\r\n",
            r.kind,
            r.version,
            if dir.join(r.executable).is_file() {
                "已安装"
            } else {
                "未安装"
            },
            size(&dir) as f64 / 1048576.,
            dir.display(),
            refs.into_iter().collect::<Vec<_>>().join("、")
        );
    }
    text += &format!(
        "\r\n共享缓存：{:.1} MiB\r\n服务依赖：{:.1} MiB\r\n",
        size(&directory.join("cache")) as f64 / 1048576.,
        size(&directory.join("services")) as f64 / 1048576.
    );
    for p in profiles.iter().filter(|p| p.managed) {
        text += &format!(
            "{}：{} {}，{}\r\n",
            p.name,
            p.dependency.kind,
            p.dependency.package,
            receipt(root, p)
                .map(|r| format!("已安装 {}", r.resolved_version))
                .unwrap_or_else(|_| "待安装 / 待应用".into())
        );
    }
    text
}
pub fn remove_runtime(
    root: &Path,
    r: &Runtime,
    profiles: &[Profile],
    active: &[Profile],
) -> Result<()> {
    if profiles.iter().chain(active).any(|p| {
        p.managed
            && match r.kind {
                "node" => p.dependency.kind == "npm" && p.dependency.node == r.version,
                "uv" => p.dependency.kind == "python" && p.dependency.uv == r.version,
                _ => p.dependency.kind == "python" && p.dependency.python == r.version,
            }
    }) {
        return Err("此版本仍被服务配置或运行中服务引用，请先切换运行环境并停止服务".into());
    }
    let path = runtime(root, r.kind, r.version)?;
    if path.exists() {
        fs::remove_dir_all(path).map_err(|e| e.to_string())?;
    }
    Ok(())
}
pub fn remove_service(root: &Path, id: &str, active: &[Profile]) -> Result<()> {
    if active.iter().any(|p| p.id == id) {
        return Err("服务运行中，不能删除依赖".into());
    }
    let path = service_root(root, id)?;
    if path.exists() {
        fs::remove_dir_all(path).map_err(|e| e.to_string())?;
    }
    Ok(())
}
pub fn clear_cache(root: &Path, active: &[Profile]) -> Result<()> {
    if !active.is_empty() {
        return Err("请停止服务后清理共享缓存，避免影响正在使用 npx / uvx 的完整命令".into());
    }
    let path = crate::storage::dependency_directory(root)?.join("cache");
    if path.exists() {
        fs::remove_dir_all(path).map_err(|e| e.to_string())?;
    }
    Ok(())
}
