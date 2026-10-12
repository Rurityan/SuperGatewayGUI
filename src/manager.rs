use crate::{Result, config, model::Profile, native::Child};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap},
    fs,
    io::{BufRead, BufReader, Read},
    path::PathBuf,
    sync::{Arc, Mutex, mpsc},
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Debug)]
pub enum Event {
    Log(String, String),
    State(String, String, String),
    Tools(String, Vec<crate::net::Tool>),
    Error(String),
    SettingsSaved(crate::model::Settings),
    StorageChanged(PathBuf),
    Certificates(String),
    Done,
}
struct Running {
    child: Child,
    profile: Profile,
    config: PathBuf,
    proxy: Option<crate::tls::Proxy>,
}
pub struct Manager {
    pub engine: PathBuf,
    root: PathBuf,
    processes: Mutex<HashMap<String, Running>>,
    last_tls_refresh: Mutex<Instant>,
    pub tx: mpsc::SyncSender<Event>,
    pub dependencies: Arc<crate::dependencies::Task>,
    one_shots: Mutex<Vec<(std::process::Child, Profile)>>,
}
impl Manager {
    pub fn new(root: PathBuf, engine: PathBuf, tx: mpsc::SyncSender<Event>) -> Arc<Self> {
        Arc::new(Self {
            engine,
            root,
            processes: Mutex::new(HashMap::new()),
            last_tls_refresh: Mutex::new(Instant::now()),
            tx,
            dependencies: Arc::new(crate::dependencies::Task::default()),
            one_shots: Mutex::new(vec![]),
        })
    }
    pub fn verify(&self) -> Result<()> {
        let manifest: serde_json::Value = serde_json::from_slice(
            &fs::read(self.engine.with_file_name("manifest.json"))
                .map_err(|e| format!("缺少包内网关清单：{e}"))?,
        )
        .map_err(|e| e.to_string())?;
        let mut file = fs::File::open(&self.engine).map_err(|e| format!("缺少网关引擎：{e}"))?;
        let mut hash = Sha256::new();
        let mut buf = [0; 65536];
        loop {
            let n = file.read(&mut buf).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            hash.update(&buf[..n]);
        }
        if format!("{:x}", hash.finalize())
            .eq_ignore_ascii_case(manifest["sha256"].as_str().unwrap_or(""))
        {
            Ok(())
        } else {
            Err("网关校验失败，请重新解压便携包".into())
        }
    }
    pub fn is_running(&self, id: &str) -> bool {
        self.processes.lock().unwrap().contains_key(id)
    }
    pub fn active(&self, p: &Profile) -> Profile {
        self.processes
            .lock()
            .unwrap()
            .get(&p.id)
            .map(|r| r.profile.clone())
            .unwrap_or(p.clone())
    }
    pub fn pid(&self, id: &str) -> Option<u32> {
        self.processes.lock().unwrap().get(id).map(|r| r.child.id)
    }
    pub fn state(&self, p: &Profile, status: &str, detail: &str) {
        let _ = self
            .tx
            .send(Event::State(p.id.clone(), status.into(), detail.into()));
    }
    pub fn start(&self, p: &Profile) -> Result<()> {
        let mut processes = self.processes.lock().unwrap();
        if processes.contains_key(&p.id) {
            return Ok(());
        }
        let result = (|| {
            config::validate(p, false)?;
            let prepared =
                crate::environment::prepare(&crate::dependencies::prepared(&self.root, p)?)?;
            config::validate(&prepared, true)?;
            self.verify()?;
            self.state(p, "启动中", "正在验证配置");
            let runtime = self.root.join("runtime");
            fs::create_dir_all(&runtime).map_err(|e| e.to_string())?;
            let file = runtime.join(format!("{}.json", p.id));
            let mut backend = prepared;
            if p.tls {
                crate::tls::ensure_ca(&self.root)?;
                backend.tls = false;
                backend.listen_address = Some("127.0.0.1".into());
                backend.lan = false;
                backend.port = std::net::TcpListener::bind(("127.0.0.1", 0))
                    .map_err(|e| e.to_string())?
                    .local_addr()
                    .map_err(|e| e.to_string())?
                    .port();
                if backend.port == p.port {
                    return Err("HTTPS 与内部端口冲突，请重新启动服务".into());
                }
            }
            let (content, mut secrets) = config::build(&backend, true)?;
            let mut child_environment = crate::environment::effective(&backend)?;
            child_environment.extend(secrets.clone());
            // Inherited credentials are known to the app even when not explicitly injected.
            secrets.extend(
                child_environment
                    .iter()
                    .filter(|(key, value)| {
                        crate::environment::sensitive_name(key) && !value.is_empty()
                    })
                    .map(|(key, value)| (key.clone(), value.clone())),
            );
            fs::write(&file, content).map_err(|e| e.to_string())?;
            let cwd = if p.cwd.is_empty() {
                std::env::current_exe()
                    .map_err(|e| e.to_string())?
                    .parent()
                    .unwrap()
                    .into()
            } else {
                std::path::absolute(config::expand(&p.cwd)?).map_err(|e| e.to_string())?
            };
            let args = vec!["--config".into(), file.to_string_lossy().into_owned()];
            let mut check_args = args.clone();
            check_args.push("--checkConfig".into());
            let check_result = (|| -> Result<()> {
                let mut check =
                    Child::spawn_exact(&self.engine, &check_args, &child_environment, &cwd)?;
                let out = drain(check.output.take().unwrap());
                let err = drain(check.errors.take().unwrap());
                check.input.take();
                let completed = check.wait(12000);
                if !completed {
                    check.terminate();
                }
                let output = out.join().unwrap_or_default() + &err.join().unwrap_or_default();
                if !completed {
                    return Err("网关配置验证超时".into());
                }
                if check.exit_code() != 0 {
                    self.log(p, &output, &secrets);
                    return Err("上游配置验证失败，请查看日志".into());
                }
                Ok(())
            })();
            if let Err(e) = check_result {
                let _ = fs::remove_file(&file);
                return Err(e);
            }
            let mut child = match Child::spawn_exact(&self.engine, &args, &child_environment, &cwd)
            {
                Ok(c) => c,
                Err(e) => {
                    let _ = fs::remove_file(&file);
                    return Err(e);
                }
            };
            for pipe in [child.output.take().unwrap(), child.errors.take().unwrap()] {
                let tx = self.tx.clone();
                let snapshot = p.clone();
                let secrets = secrets.clone();
                thread::spawn(move || {
                    let mut reader = BufReader::new(pipe);
                    loop {
                        let mut line = Vec::new();
                        match reader.by_ref().take(32769).read_until(b'\n', &mut line) {
                            Ok(0) | Err(_) => break,
                            Ok(_) => {
                                let text = String::from_utf8_lossy(&line);
                                let _ = tx.try_send(Event::Log(
                                    snapshot.id.clone(),
                                    redact(&snapshot, &text, &secrets),
                                ));
                            }
                        }
                    }
                });
            }
            let deadline = Instant::now() + Duration::from_secs(10);
            let mut ready = false;
            while Instant::now() < deadline && !child.exited() {
                if crate::net::profile_health(&backend).is_ok() {
                    ready = true;
                    break;
                }
                thread::sleep(Duration::from_millis(100));
            }
            if !ready {
                child.stop();
                let _ = fs::remove_file(&file);
                return Err("网关未开始监听，请查看日志".into());
            }
            let proxy = if p.tls {
                match crate::tls::Proxy::start(p, backend.port, &self.root) {
                    Ok(proxy) => Some(proxy),
                    Err(error) => {
                        child.stop();
                        let _ = fs::remove_file(&file);
                        return Err(error);
                    }
                }
            } else {
                None
            };
            processes.insert(
                p.id.clone(),
                Running {
                    child,
                    profile: p.clone(),
                    config: file,
                    proxy,
                },
            );
            self.state(
                p,
                "网关运行",
                if p.tls {
                    "HTTPS 已运行；请在证书管理中信任本机 CA，再测试连接"
                } else {
                    "点击测试连接验证 MCP 服务"
                },
            );
            Ok(())
        })();
        if let Err(e) = &result {
            self.state(p, "失败", e);
        }
        result
    }
    fn log(&self, p: &Profile, line: &str, secrets: &BTreeMap<String, String>) {
        let _ = self
            .tx
            .try_send(Event::Log(p.id.clone(), redact(p, line, secrets)));
    }
    pub fn stop(&self, p: &Profile) {
        let mut processes = self.processes.lock().unwrap();
        if let Some(mut r) = processes.remove(&p.id) {
            drop(r.proxy.take());
            r.child.stop();
            let _ = fs::remove_file(r.config);
        }
        self.state(p, "已停止", "");
    }
    pub fn stop_all(&self) {
        let profiles = self
            .processes
            .lock()
            .unwrap()
            .values()
            .map(|r| r.profile.clone())
            .collect::<Vec<_>>();
        for p in profiles {
            self.stop(&p);
        }
    }
    pub fn active_profiles(&self) -> Vec<Profile> {
        let mut profiles: Vec<_> = self
            .processes
            .lock()
            .unwrap()
            .values()
            .map(|r| r.profile.clone())
            .collect();
        let mut terminals = self.one_shots.lock().unwrap();
        terminals.retain_mut(|(child, _)| child.try_wait().ok().flatten().is_none());
        profiles.extend(terminals.iter().map(|(_, profile)| profile.clone()));
        profiles
    }
    pub fn launch_one_shot(
        &self,
        root: &std::path::Path,
        invocation: &crate::oneshot::Invocation,
        profile: &Profile,
    ) -> Result<u32> {
        self.dependencies.execute(|| {
            let child = crate::oneshot::launch(root, invocation)?;
            let pid = child.id();
            let mut profile = profile.clone();
            profile.managed = true;
            self.one_shots.lock().unwrap().push((child, profile));
            Ok(pid)
        })
    }
    pub fn poll(self: &Arc<Self>) {
        if let Ok(mut last) = self.last_tls_refresh.try_lock()
            && last.elapsed() >= Duration::from_secs(60)
        {
            *last = Instant::now();
            let manager = self.clone();
            thread::spawn(move || {
                if let Err(error) = manager.refresh_tls(false) {
                    let _ = manager.tx.send(Event::Error(error));
                }
            });
        }
        if let Ok(mut processes) = self.processes.try_lock() {
            let exited = processes
                .iter()
                .filter(|(_, r)| {
                    r.child.exited() || r.proxy.as_ref().is_some_and(|proxy| proxy.exited())
                })
                .map(|(id, _)| id.clone())
                .collect::<Vec<_>>();
            for id in exited {
                if let Some(mut r) = processes.remove(&id) {
                    let detail = if r.child.exited() {
                        format!("网关异常退出，代码 {}", r.child.exit_code())
                    } else {
                        "HTTPS 入口异常退出；已停止本服务，不回退明文 HTTP".into()
                    };
                    drop(r.proxy.take());
                    r.child.terminate();
                    self.state(&r.profile, "失败", &detail);
                    let _ = fs::remove_file(r.config);
                }
            }
        }
    }
    pub fn any_tls_running(&self) -> bool {
        self.processes
            .lock()
            .unwrap()
            .values()
            .any(|r| r.proxy.is_some())
    }
    pub fn certificate_action(&self, action: i32, destination: Option<PathBuf>) -> Result<()> {
        // Serialize CA changes with service starts and IP refresh.
        let processes = self.processes.lock().unwrap();
        match action {
            507 => crate::tls::prepare(&self.root)?,
            508 => crate::tls::export_ca(&self.root, &destination.ok_or("请选择导出文件")?)?,
            509 => crate::tls::set_trust(&self.root, true)?,
            512 => crate::tls::set_trust(&self.root, false)?,
            513 => {
                if processes.values().any(|r| r.proxy.is_some()) {
                    return Err("请先停止所有 HTTPS 服务，再重建 CA".into());
                }
                crate::tls::rotate(&self.root)?;
                crate::tls::prepare(&self.root)?;
            }
            _ => return Err("证书操作无效".into()),
        }
        let _ = self
            .tx
            .send(Event::Certificates(crate::tls::info(&self.root)));
        Ok(())
    }
    pub fn refresh_tls(&self, force: bool) -> Result<()> {
        let mut processes = self.processes.lock().unwrap();
        let ids = processes
            .values()
            .filter_map(|r| r.proxy.as_ref().map(|_| r.profile.id.clone()))
            .collect::<Vec<_>>();
        for id in ids {
            let r = processes.get_mut(&id).unwrap();
            match r.proxy.as_mut().unwrap().refresh(&self.root) {
                Ok(updated) => {
                    if updated || force {
                        self.state(
                            &r.profile,
                            "网关运行",
                            "共享 IP 证书已检查/更新；MCP 进程与现有连接保持运行",
                        );
                    }
                }
                Err(error) => {
                    let mut r = processes.remove(&id).unwrap();
                    drop(r.proxy.take());
                    r.child.terminate();
                    let _ = fs::remove_file(r.config);
                    self.state(&r.profile, "失败", &error);
                    return Err(error);
                }
            }
        }
        Ok(())
    }
}
impl Drop for Manager {
    fn drop(&mut self) {
        self.stop_all();
    }
}
fn drain(file: fs::File) -> thread::JoinHandle<String> {
    thread::spawn(move || {
        let mut s = String::new();
        let _ = file.take(1024 * 1024).read_to_string(&mut s);
        s
    })
}
pub fn redact(p: &Profile, line: &str, secrets: &BTreeMap<String, String>) -> String {
    let mut values = p.secrets();
    values.extend(secrets.values().filter(|v| !v.is_empty()).cloned());
    values.sort_by_key(|s| std::cmp::Reverse(s.len()));
    let mut line = line.trim_end().to_string();
    for secret in values {
        line = line.replace(&secret, "***");
    }
    // URL query values can contain additional credentials unknown to the form.
    let mut result = String::new();
    for word in line.split_inclusive(char::is_whitespace) {
        if (word.contains("http://") || word.contains("https://")) && word.contains('?') {
            result.push_str(word.split('?').next().unwrap());
            result.push_str("?*** ");
        } else {
            result.push_str(word);
        }
    }
    if result.chars().count() > 8192 {
        result = result.chars().take(8192).collect::<String>() + " …（日志已截断）";
    }
    result
}
