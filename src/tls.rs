//! Small in-process TLS tunnel. One shared CA and server certificate for all services.
use crate::{
    Result,
    model::Profile,
    native::{self, wide},
};
use rcgen::{
    BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair,
    KeyUsagePurpose,
};
use rustls::{
    ServerConfig,
    pki_types::{CertificateDer, PrivatePkcs8KeyDer},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    mem::zeroed,
    path::{Path, PathBuf},
    ptr::{null, null_mut},
    sync::{
        Arc, OnceLock, RwLock,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use time::OffsetDateTime;
use windows_sys::Win32::{
    Foundation::*,
    Security::{Authorization::*, Cryptography::*, *},
};

pub fn ca_path(root: &Path) -> PathBuf {
    root.join("tls/root.crt")
}
fn failure(e: impl std::fmt::Display) -> String {
    format!("TLS：{e}")
}
#[derive(Serialize, Deserialize)]
struct StoredCertificate {
    certificate: String,
    key: String,
    subjects: Vec<String>,
    expires: i64,
}
fn read_private(path: &Path) -> Result<StoredCertificate> {
    let bytes = crate::store::protect(&fs::read(path).map_err(failure)?, false)?;
    serde_json::from_slice(&bytes).map_err(failure)
}
fn write_private(path: &Path, value: &StoredCertificate) -> Result<()> {
    let bytes = crate::store::protect(&serde_json::to_vec(value).map_err(failure)?, true)?;
    crate::store::atomic_write(path, &bytes)
}
pub fn subjects() -> Result<Vec<String>> {
    let mut values = vec!["127.0.0.1".into()];
    values.extend(
        crate::network::local_addresses()?
            .into_iter()
            .map(|a| a.ip.to_string()),
    );
    values.sort();
    values.dedup();
    Ok(values)
}
pub fn ensure_ca(root: &Path) -> Result<()> {
    protect_directory(&root.join("tls"))?;
    let path = root.join("tls/ca.dat");
    if path.exists() {
        let ca = read_private(&path)?;
        if ca.expires <= OffsetDateTime::now_utc().unix_timestamp() + 86400 {
            return Err(
                "本地 CA 已到期或即将到期，请停止 HTTPS 服务后重建 CA 并重新导入公开证书".into(),
            );
        }
        let key = KeyPair::from_pem(&ca.key).map_err(failure)?;
        Issuer::from_ca_cert_pem(&ca.certificate, key).map_err(failure)?;
        // Public file is always regenerated from the encrypted authoritative record.
        if fs::read(ca_path(root)).ok().as_deref() != Some(ca.certificate.as_bytes()) {
            crate::store::atomic_write(&ca_path(root), ca.certificate.as_bytes())?;
        }
        return Ok(());
    }
    if ca_path(root).exists() {
        return Err("CA 私钥记录缺失，不会覆盖现有 CA。请从备份恢复，或在证书管理中重建。".into());
    }
    let now = OffsetDateTime::now_utc();
    let key = KeyPair::generate().map_err(failure)?;
    let mut params = CertificateParams::default();
    params.not_before = now - time::Duration::days(1);
    params.not_after = now + time::Duration::days(365 * 30);
    params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    params.distinguished_name.push(
        DnType::CommonName,
        format!("SuperGateway GUI Local CA {}", crate::model::new_id()),
    );
    let cert = params.self_signed(&key).map_err(failure)?;
    let stored = StoredCertificate {
        certificate: cert.pem(),
        key: key.serialize_pem(),
        subjects: vec![],
        expires: params.not_after.unix_timestamp(),
    };
    write_private(&path, &stored)?;
    crate::store::atomic_write(&ca_path(root), stored.certificate.as_bytes())
}
fn server_config(root: &Path) -> Result<(Arc<ServerConfig>, String)> {
    ensure_ca(root)?;
    let subjects = subjects()?;
    let path = root.join("tls/server.dat");
    let now = OffsetDateTime::now_utc();
    let current = if path.exists() {
        Some(read_private(&path)?)
    } else {
        None
    };
    let cert = if let Some(cert) = current.filter(|cert| {
        cert.subjects == subjects && cert.expires > now.unix_timestamp() + 30 * 86400
    }) {
        cert
    } else {
        let ca = read_private(&root.join("tls/ca.dat"))?;
        let issuer = Issuer::from_ca_cert_pem(
            &ca.certificate,
            KeyPair::from_pem(&ca.key).map_err(failure)?,
        )
        .map_err(failure)?;
        let key = KeyPair::generate().map_err(failure)?;
        let mut params = CertificateParams::new(subjects.clone()).map_err(failure)?;
        params.not_before = now - time::Duration::days(1);
        params.not_after = (now + time::Duration::days(365 * 20)).min(
            OffsetDateTime::from_unix_timestamp(ca.expires).map_err(failure)?
                - time::Duration::days(1),
        );
        if params.not_after <= now + time::Duration::days(30) {
            return Err("CA 即将到期，无法续签有效服务证书，请重建 CA".into());
        }
        params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        params
            .distinguished_name
            .push(DnType::CommonName, "SuperGateway GUI shared IP certificate");
        let cert = params.signed_by(&key, &issuer).map_err(failure)?;
        let stored = StoredCertificate {
            certificate: cert.pem(),
            key: key.serialize_pem(),
            subjects,
            expires: params.not_after.unix_timestamp(),
        };
        write_private(&path, &stored)?;
        crate::store::atomic_write(&root.join("tls/server.crt"), stored.certificate.as_bytes())?;
        stored
    };
    let key = KeyPair::from_pem(&cert.key).map_err(failure)?;
    let parsed = Certificate::load_pem(&cert.certificate)?;
    let fingerprint = format!("{:x}", Sha256::digest(parsed.bytes()));
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(failure)?
        .with_no_client_auth()
        .with_single_cert(
            vec![CertificateDer::from(parsed.bytes().to_vec())],
            PrivatePkcs8KeyDer::from(key.serialize_der()).into(),
        )
        .map_err(failure)?;
    // No ALPN h2: tunnel forwards HTTP/1.1, SSE and WebSocket transparently.
    Ok((Arc::new(config), fingerprint))
}
fn runtime() -> Result<&'static tokio::runtime::Runtime> {
    static RUNTIME: OnceLock<std::result::Result<tokio::runtime::Runtime, String>> =
        OnceLock::new();
    RUNTIME
        .get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .max_blocking_threads(1)
                .enable_all()
                .build()
                .map_err(failure)
        })
        .as_ref()
        .map_err(Clone::clone)
}
pub struct Proxy {
    config: Arc<RwLock<Arc<ServerConfig>>>,
    pub fingerprint: String,
    pub backend_port: u16,
    alive: Arc<AtomicBool>,
    shutdown: tokio::sync::watch::Sender<bool>,
    finished: std::sync::mpsc::Receiver<()>,
}
impl Proxy {
    pub fn start(p: &Profile, backend_port: u16, root: &Path) -> Result<Self> {
        let (config, fingerprint) = server_config(root)?;
        let config = Arc::new(RwLock::new(config));
        let listener = std::net::TcpListener::bind((p.listen_host(), p.port)).map_err(failure)?;
        listener.set_nonblocking(true).map_err(failure)?;
        let runtime = runtime()?;
        let listener = {
            let _guard = runtime.enter();
            tokio::net::TcpListener::from_std(listener).map_err(failure)?
        };
        let alive = Arc::new(AtomicBool::new(true));
        let (shutdown, mut stop) = tokio::sync::watch::channel(false);
        let (finished_tx, finished) = std::sync::mpsc::channel();
        let task_alive = alive.clone();
        let task_config = config.clone();
        runtime.spawn(async move {
            struct Finished(Arc<AtomicBool>,std::sync::mpsc::Sender<()>);
            impl Drop for Finished {fn drop(&mut self){self.0.store(false,Ordering::Release);let _=self.1.send(());}}
            let _finished=Finished(task_alive,finished_tx);
            let mut clients=tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    _=stop.changed()=>break,
                    Some(_)=clients.join_next(),if !clients.is_empty()=>{},
                    accepted=listener.accept()=>{
                        let Ok((stream,_))=accepted else{break;};
                        // Bound handshake and connection memory; no request bodies/logged keys.
                        if clients.len()>=64 {drop(stream);continue;}
                        let config=task_config.read().unwrap().clone();
                        clients.spawn(async move {
                            let acceptor=tokio_rustls::TlsAcceptor::from(config);
                            let Ok(Ok(mut tls))=tokio::time::timeout(Duration::from_secs(10),acceptor.accept(stream)).await else{return;};
                            let Ok(Ok(mut backend))=tokio::time::timeout(Duration::from_secs(5),tokio::net::TcpStream::connect(("127.0.0.1",backend_port))).await else{return;};
                            let _=tokio::io::copy_bidirectional(&mut tls,&mut backend).await;
                        });
                    }
                }
            }
            clients.abort_all();while clients.join_next().await.is_some(){}
            drop(listener);
        });
        Ok(Self {
            config,
            fingerprint,
            backend_port,
            alive,
            shutdown,
            finished,
        })
    }
    pub fn exited(&self) -> bool {
        !self.alive.load(Ordering::Acquire)
    }
    pub fn refresh(&mut self, root: &Path) -> Result<bool> {
        let (config, fingerprint) = server_config(root)?;
        if fingerprint == self.fingerprint {
            return Ok(false);
        }
        *self.config.write().unwrap() = config;
        self.fingerprint = fingerprint;
        Ok(true)
    }
}
impl Drop for Proxy {
    fn drop(&mut self) {
        let _ = self.shutdown.send(true);
        let _ = self.finished.recv_timeout(Duration::from_secs(3));
    }
}
pub fn protect_directory(path: &Path) -> Result<()> {
    fs::create_dir_all(path).map_err(|e| e.to_string())?;
    let sddl = wide(format!(
        "D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;{})",
        crate::startup::user_sid()?
    ));
    unsafe {
        let mut descriptor = null_mut();
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            null_mut(),
        ) == 0
        {
            return Err(native::error("保护证书目录"));
        }
        let mut present = 0;
        let mut defaulted = 0;
        let mut acl = null_mut();
        let ok = GetSecurityDescriptorDacl(descriptor, &mut present, &mut acl, &mut defaulted);
        let status = if ok != 0 {
            SetNamedSecurityInfoW(
                wide(path).as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                null_mut(),
                null_mut(),
                acl,
                null(),
            )
        } else {
            GetLastError()
        };
        LocalFree(descriptor);
        if status != 0 {
            return Err(format!(
                "无法限制证书目录权限（错误 {status}）；HTTPS 未启动"
            ));
        }
    }
    Ok(())
}

struct Certificate(*const CERT_CONTEXT);
impl Drop for Certificate {
    fn drop(&mut self) {
        unsafe {
            CertFreeCertificateContext(self.0);
        }
    }
}
impl Certificate {
    fn load(path: &Path) -> Result<Self> {
        Self::load_pem(&fs::read_to_string(path).map_err(|e| format!("尚未生成证书：{e}"))?)
    }
    fn load_pem(pem: &str) -> Result<Self> {
        let text = wide(pem);
        unsafe {
            let mut size = 0;
            if CryptStringToBinaryW(
                text.as_ptr(),
                0,
                CRYPT_STRING_BASE64HEADER,
                null_mut(),
                &mut size,
                null_mut(),
                null_mut(),
            ) == 0
            {
                return Err(native::error("解析证书"));
            }
            let mut bytes = vec![0u8; size as usize];
            if CryptStringToBinaryW(
                text.as_ptr(),
                0,
                CRYPT_STRING_BASE64HEADER,
                bytes.as_mut_ptr(),
                &mut size,
                null_mut(),
                null_mut(),
            ) == 0
            {
                return Err(native::error("解析证书"));
            }
            let ctx = CertCreateCertificateContext(
                X509_ASN_ENCODING | PKCS_7_ASN_ENCODING,
                bytes.as_ptr(),
                size,
            );
            if ctx.is_null() {
                return Err(native::error("读取证书"));
            }
            Ok(Self(ctx))
        }
    }
    fn bytes(&self) -> &[u8] {
        unsafe {
            std::slice::from_raw_parts((*self.0).pbCertEncoded, (*self.0).cbCertEncoded as usize)
        }
    }
    fn expired(&self) -> bool {
        unsafe { CertVerifyTimeValidity(null(), (*self.0).pCertInfo) != 0 }
    }
    fn dates(&self) -> String {
        unsafe {
            let info = &*(*self.0).pCertInfo;
            crate::i18n::format_text!("{} 至 {}", date(info.NotBefore), date(info.NotAfter))
        }
    }
}
fn date(ft: FILETIME) -> String {
    let ticks = ((ft.dwHighDateTime as u64) << 32) | ft.dwLowDateTime as u64;
    let ticks = ticks + 8 * 3600 * 10_000_000;
    let local = FILETIME {
        dwLowDateTime: ticks as u32,
        dwHighDateTime: (ticks >> 32) as u32,
    };
    let mut time = unsafe { zeroed() };
    unsafe {
        windows_sys::Win32::System::Time::FileTimeToSystemTime(&local, &mut time);
    }
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}（UTC+8）",
        time.wYear, time.wMonth, time.wDay, time.wHour, time.wMinute
    )
}
struct RootStore(HCERTSTORE);
impl Drop for RootStore {
    fn drop(&mut self) {
        unsafe {
            CertCloseStore(self.0, 0);
        }
    }
}
fn root_store() -> Result<RootStore> {
    unsafe {
        let store = CertOpenStore(
            CERT_STORE_PROV_SYSTEM_W,
            X509_ASN_ENCODING,
            0,
            CERT_SYSTEM_STORE_CURRENT_USER,
            wide("ROOT").as_ptr().cast(),
        );
        if store.is_null() {
            return Err(native::error("打开当前用户证书库"));
        }
        Ok(RootStore(store))
    }
}
pub fn set_trust(root: &Path, trusted: bool) -> Result<()> {
    let cert = Certificate::load(&ca_path(root))?;
    let store = root_store()?;
    unsafe {
        if trusted {
            if cert.expired() {
                return Err("本地 CA 已过期，不能信任".into());
            }
            if CertAddCertificateContextToStore(
                store.0,
                cert.0,
                CERT_STORE_ADD_USE_EXISTING,
                null_mut(),
            ) == 0
            {
                return Err(native::error("信任本地 CA"));
            }
        } else {
            let found = CertFindCertificateInStore(
                store.0,
                X509_ASN_ENCODING | PKCS_7_ASN_ENCODING,
                0,
                CERT_FIND_EXISTING,
                cert.0.cast(),
                null(),
            );
            if !found.is_null() && CertDeleteCertificateFromStore(found) == 0 {
                return Err(native::error("取消信任本地 CA"));
            }
        }
    }
    Ok(())
}
pub fn export_ca(root: &Path, destination: &Path) -> Result<()> {
    let cert = Certificate::load(&ca_path(root))?;
    // DER .cer is readily imported by Windows; never copy an arbitrary key file.
    if destination
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("pem"))
    {
        fs::write(destination, fs::read(ca_path(root)).map_err(failure)?).map_err(failure)
    } else {
        fs::write(destination, cert.bytes()).map_err(failure)
    }
}
pub fn prepare(root: &Path) -> Result<()> {
    server_config(root).map(|_| ())
}
pub fn info(root: &Path) -> String {
    let intro = crate::i18n::text(
        "HTTPS 证书管理\r\n\r\n所有服务共用一张证书，包含本机网卡 IPv4，服务证书默认 20 年、CA 30 年。启动时检查，运行中每分钟检查，剩余不足 30 天或新增网卡 IP 时自动补签。新连接使用新证书，现有 MCP 连接不重启。无需公网或额外进程。\r\n\r\n客户端必须信任公开 CA，不会跳过验证。Windows 导入公开 .cer 到当前用户的受信任根证书库。Node/独立证书库使用公开 .pem，配置 NODE_EXTRA_CA_CERTS。\r\n\r\n",
    );
    match Certificate::load(&ca_path(root)) {
        Ok(cert) => {
            let fingerprint = crate::i18n::format_text!("{:x}", Sha256::digest(cert.bytes()));
            let trusted = root_store()
                .map(|store| unsafe {
                    let found = CertFindCertificateInStore(
                        store.0,
                        X509_ASN_ENCODING | PKCS_7_ASN_ENCODING,
                        0,
                        CERT_FIND_EXISTING,
                        cert.0.cast(),
                        null(),
                    );
                    let exists = !found.is_null();
                    if exists {
                        CertFreeCertificateContext(found);
                    }
                    exists
                })
                .unwrap_or(false);
            let server = Certificate::load(&root.join("tls/server.crt"))
                .map(|cert| crate::i18n::format_text!("共享服务证书有效期：{}", cert.dates()))
                .unwrap_or_else(|_| crate::i18n::text("共享服务证书尚未生成").into_owned());
            let sans = subjects().unwrap_or_default().join(", ");
            crate::i18n::format_text!(
                "{intro}CA SHA256：{fingerprint}\r\n有效期：{}\r\n状态：{}；本机当前用户{}信任\r\n公开 CA：{}\r\n\r\n私钥只保存在当前用户和 SYSTEM 可访问的证书目录，不随便携包分发，不会导出。新网卡 IP 会在下次启动/刷新入口时签发证书。CA 更换后，所有客户端都需重新导入。",
                cert.dates(),
                crate::i18n::text(if cert.expired() {
                    "已失效"
                } else {
                    "有效"
                }),
                crate::i18n::text(if trusted { "已" } else { "尚未" }),
                ca_path(root).display()
            ) + &crate::i18n::format_text!(
                "\r\n{server}\r\n当前网卡 IP：{sans}\r\nCA/服务私钥以 DPAPI 加密保存；公开证书不含私钥。"
            )
        }
        Err(error) => {
            crate::i18n::format_text!(
                "{intro}{error}\r\n\r\n请点击“生成/检查”，或先启用并启动一个 HTTPS 服务。"
            )
        }
    }
}
pub fn rotate(root: &Path) -> Result<()> {
    if ca_path(root).exists() {
        set_trust(root, false)?;
    }
    let old = root.join("tls");
    if old.exists() {
        fs::rename(
            &old,
            root.join(format!("tls-backup-{}", crate::model::new_id())),
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn root() -> PathBuf {
        std::env::temp_dir().join(format!("SG TLS 中文 {}", crate::model::new_id()))
    }
    #[test]
    fn shared_certificate_is_cached_long_lived_and_private_keys_are_encrypted() {
        let root = root();
        let (_, first) = server_config(&root).unwrap();
        let (_, second) = server_config(&root).unwrap();
        assert_eq!(first, second);
        let ca = read_private(&root.join("tls/ca.dat")).unwrap();
        let cert = read_private(&root.join("tls/server.dat")).unwrap();
        let now = OffsetDateTime::now_utc().unix_timestamp();
        assert!(ca.expires - now > 29 * 365 * 86400);
        assert!(cert.expires - now > 19 * 365 * 86400);
        assert!(cert.subjects.contains(&"127.0.0.1".into()));
        for name in ["ca.dat", "server.dat"] {
            let bytes = fs::read(root.join("tls").join(name)).unwrap();
            assert!(!String::from_utf8_lossy(&bytes).contains("PRIVATE KEY"));
        }
        let root_cert = fs::read(ca_path(&root)).unwrap();
        let export = root.join("public.cer");
        export_ca(&root, &export).unwrap();
        assert_eq!(
            fs::read(export).unwrap(),
            Certificate::load(&ca_path(&root)).unwrap().bytes()
        );
        let export = root.join("public.pem");
        export_ca(&root, &export).unwrap();
        assert_eq!(fs::read(export).unwrap(), root_cert);
    }
    #[test]
    fn nearing_expiry_and_changed_ip_trigger_reissue_under_same_ca() {
        let root = root();
        let (_, first) = server_config(&root).unwrap();
        let ca = fs::read(ca_path(&root)).unwrap();
        let path = root.join("tls/server.dat");
        let mut old = read_private(&path).unwrap();
        old.expires = OffsetDateTime::now_utc().unix_timestamp() + 86400;
        write_private(&path, &old).unwrap();
        let (_, second) = server_config(&root).unwrap();
        assert_ne!(first, second);
        let mut old = read_private(&path).unwrap();
        old.subjects.push("192.0.2.123".into());
        write_private(&path, &old).unwrap();
        let (_, third) = server_config(&root).unwrap();
        assert_ne!(second, third);
        assert_eq!(ca, fs::read(ca_path(&root)).unwrap());
    }
    #[test]
    fn missing_or_corrupt_private_record_does_not_silently_replace_identity() {
        let root = root();
        ensure_ca(&root).unwrap();
        let public = fs::read(ca_path(&root)).unwrap();
        fs::write(root.join("tls/ca.dat"), b"broken").unwrap();
        assert!(ensure_ca(&root).is_err());
        assert_eq!(public, fs::read(ca_path(&root)).unwrap());
        fs::remove_file(root.join("tls/ca.dat")).unwrap();
        assert!(ensure_ca(&root).is_err());
        assert_eq!(public, fs::read(ca_path(&root)).unwrap());
    }
}
