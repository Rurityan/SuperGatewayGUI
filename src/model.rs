use serde::{Deserialize, Serialize};
use windows_sys::{Win32::System::Com::CoCreateGuid, core::GUID};

#[derive(Clone, Default, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "PascalCase", default)]
pub struct Pair {
    pub key: String,
    pub value: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "PascalCase", default)]
pub struct ClientVariable {
    pub key: String,
    pub value: String,
    pub enabled: bool,
}
impl Default for ClientVariable {
    fn default() -> Self {
        Self {
            key: String::new(),
            value: String::new(),
            enabled: true,
        }
    }
}
#[derive(Clone, Default, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "PascalCase", default)]
pub struct ClientOptions {
    // Retained only to read and preserve previous settings; network exports ignore it.
    pub environment: Vec<ClientVariable>,
    pub startup_timeout_sec: Option<u32>,
    pub tool_timeout_sec: Option<u32>,
    pub stdio: bool, // Deprecated: the GUI only exports connections to its gateway.
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "PascalCase", default)]
pub struct Profile {
    pub id: String,
    pub name: String,
    pub source: String,
    pub command: String,
    pub args: Vec<String>,
    pub stdio: String,
    pub cwd: String,
    pub env: Vec<Pair>,
    pub inherit_environment: bool,
    pub additional_paths: Vec<String>,
    #[serde(skip)]
    pub environment_prepared: bool,
    pub url: String,
    pub headers: Vec<Pair>,
    pub bearer: String,
    pub output: String,
    pub port: u16,
    pub tls: bool,
    pub lan: bool,
    // Missing in old settings: retain the old Lan value on deserialization.
    #[serde(default)]
    pub listen_address: Option<String>,
    pub route: String,
    pub message_path: String,
    pub api_key: String,
    pub api_keys: Vec<String>,
    pub cors: String,
    pub stateful: bool,
    pub session_timeout: u64,
    pub log_level: String,
    pub enabled: bool,
    pub auto_start: bool,
    pub managed: bool,
    #[serde(skip)]
    pub managed_runtime: bool,
    pub dependency: crate::dependencies::ServiceDependency,
    pub client: ClientOptions,
}
impl Default for Profile {
    fn default() -> Self {
        Self {
            id: new_id(),
            name: "新服务".into(),
            source: "local".into(),
            command: String::new(),
            args: vec![],
            stdio: String::new(),
            cwd: String::new(),
            env: vec![],
            inherit_environment: true,
            additional_paths: vec![],
            environment_prepared: false,
            url: String::new(),
            headers: vec![],
            bearer: String::new(),
            output: "streamableHttp".into(),
            port: 8000,
            tls: false,
            lan: false,
            listen_address: Some("0.0.0.0".into()),
            route: "/mcp".into(),
            message_path: "/message".into(),
            api_key: String::new(),
            api_keys: vec![],
            cors: String::new(),
            stateful: false,
            session_timeout: 60000,
            log_level: "info".into(),
            enabled: true,
            auto_start: false,
            managed: false,
            managed_runtime: false,
            dependency: Default::default(),
            client: Default::default(),
        }
    }
}
impl Profile {
    pub fn gateway_eq(&self, other: &Self) -> bool {
        let mut a = self.clone();
        let mut b = other.clone();
        a.client = Default::default();
        b.client = Default::default();
        a == b
    }
    pub fn listen_host(&self) -> &str {
        self.listen_address
            .as_deref()
            .unwrap_or(if self.lan { "0.0.0.0" } else { "127.0.0.1" })
    }
    pub fn connection_host(&self, probe: bool) -> String {
        match self.listen_host() {
            "0.0.0.0" if probe => "127.0.0.1".into(),
            "0.0.0.0" => crate::network::preferred_address(),
            host => host.into(),
        }
    }
    pub fn address(&self, probe: bool) -> String {
        let scheme = match (self.output.as_str(), self.tls) {
            ("ws", true) => "wss",
            ("ws", false) => "ws",
            (_, true) => "https",
            _ => "http",
        };
        let host = self.connection_host(probe);
        format!("{scheme}://{host}:{}{}", self.port, self.route)
    }
    pub fn secrets(&self) -> Vec<String> {
        self.env
            .iter()
            .chain(&self.headers)
            .map(|p| p.value.clone())
            .chain(self.api_keys.clone())
            .chain([self.bearer.clone(), self.api_key.clone()])
            .chain(self.client.environment.iter().map(|v| v.value.clone()))
            .filter(|s| !s.is_empty())
            .collect()
    }
}
#[derive(Clone, Default, Debug, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct Settings {
    pub dependency_network: crate::dependencies::Network,
    pub login_startup: bool,
    pub admin_login_startup: bool,
    pub services: Vec<Profile>,
}
pub fn new_id() -> String {
    let mut guid = GUID::default();
    unsafe {
        CoCreateGuid(&mut guid);
    }
    format!(
        "{:08x}{:04x}{:04x}{}",
        guid.data1,
        guid.data2,
        guid.data3,
        guid.data4
            .iter()
            .map(|v| format!("{v:02x}"))
            .collect::<String>()
    )
}
