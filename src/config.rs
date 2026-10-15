use crate::{
    Result,
    model::{Pair, Profile},
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashSet},
    net::TcpListener,
    path::{Path, PathBuf},
};

pub fn free_port(profiles: &[Profile]) -> Result<u16> {
    (8000..=65535)
        .find(|p| profiles.iter().all(|s| s.port != *p) && port_available(*p, true))
        .ok_or("没有可用端口".into())
}
pub fn port_available(port: u16, lan: bool) -> bool {
    TcpListener::bind((if lan { "0.0.0.0" } else { "127.0.0.1" }, port)).is_ok()
}
pub fn resolve(command: &str) -> Result<PathBuf> {
    resolve_with_path(command, &std::env::var("PATH").unwrap_or_default())
}
pub fn resolve_with_path(command: &str, search: &str) -> Result<PathBuf> {
    let p = Path::new(command);
    if p.is_absolute() && p.is_file() {
        return Ok(p.into());
    }
    if !p.is_absolute() {
        let extensions = if p.extension().is_some() {
            vec![""]
        } else {
            vec![".exe", ".cmd", ".bat", ""]
        };
        for dir in search.split(';').filter(|s| !s.is_empty()) {
            for ext in &extensions {
                let candidate = Path::new(dir.trim_matches('"')).join(format!("{command}{ext}"));
                if candidate.is_file() {
                    return Ok(candidate);
                }
            }
        }
    }
    Err(format!(
        "找不到程序 {command}。npx 需要 Node.js，uvx 需要 uv；请安装后重开应用，或选择完整路径。"
    ))
}
pub fn expand(value: &str) -> Result<String> {
    let mut result = String::new();
    let mut chars = value.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '$' {
            result.push(c);
            continue;
        }
        match chars.peek() {
            Some('$') => {
                chars.next();
                result.push('$');
            }
            Some('{') => {
                chars.next();
                let mut body = String::new();
                let mut closed = false;
                for c in chars.by_ref() {
                    if c == '}' {
                        closed = true;
                        break;
                    }
                    body.push(c);
                }
                if !closed {
                    return Err("环境变量占位符缺少 }，字面量 $ 请写 $$".into());
                }
                let body = body.strip_prefix("env:").unwrap_or(&body);
                let (name, fallback) = body
                    .split_once(":-")
                    .map(|(a, b)| (a, Some(b)))
                    .unwrap_or((body, None));
                if name.is_empty()
                    || name.chars().enumerate().any(|(i, c)| {
                        !(c == '_' || c.is_ascii_alphabetic() || i > 0 && c.is_ascii_digit())
                    })
                {
                    return Err("环境变量名称无效".into());
                }
                match std::env::var(name)
                    .ok()
                    .filter(|s| !s.is_empty())
                    .or_else(|| fallback.map(str::to_owned))
                {
                    Some(s) => result.push_str(&s),
                    None => return Err(format!("环境变量未设置：{name}")),
                }
            }
            _ => result.push('$'),
        }
    }
    Ok(result)
}
pub fn validate(p: &Profile, launch: bool) -> Result<()> {
    crate::client::validate(&p.client)?;
    if p.name.trim().is_empty() {
        return Err("请输入服务名称".into());
    }
    if p.port == 0 {
        return Err("端口必须为 1–65535".into());
    }
    if !["streamableHttp", "sse", "ws"].contains(&p.output.as_str()) {
        return Err("输出协议无效".into());
    }
    for path in [&p.route, &p.message_path] {
        if !path.starts_with('/')
            || path.contains("//")
            || path.contains(['?', '#'])
            || path.chars().any(char::is_whitespace)
        {
            return Err("路径必须以 / 开头，不能有空格、查询参数或 //".into());
        }
    }
    if p.route == "/_gateway_health" || p.output == "sse" && p.route == p.message_path {
        return Err("连接路径与健康检查或消息路径冲突".into());
    }
    if !(1..=2592000000).contains(&p.session_timeout) {
        return Err("会话超时须为正数，且不超过 30 天".into());
    }
    if !["info", "debug", "none"].contains(&p.log_level.as_str()) {
        return Err("日志级别无效".into());
    }
    for pairs in [&p.env, &p.headers] {
        let mut keys = HashSet::new();
        for pair in pairs {
            if pair.key.trim().is_empty()
                || !keys.insert(pair.key.to_lowercase())
                || pair.key.contains('\0')
                || pair.value.contains('\0')
            {
                return Err("键值不能有空名称、重复名称或 NUL 字符".into());
            }
        }
    }
    if p.env.iter().any(|s| s.key.contains('=')) {
        return Err("环境变量名称不能包含 =".into());
    }
    for dir in &p.additional_paths {
        if dir.trim().is_empty()
            || dir.contains(['\0', ';', '\r', '\n', '"'])
            || !Path::new(&expand(dir)?).is_absolute()
        {
            return Err(
                "附加程序目录须为绝对目录路径，不含分号、引号或换行；请用目录选择按钮添加".into(),
            );
        }
        if launch && !Path::new(&expand(dir)?).is_dir() {
            return Err(format!("附加程序目录不存在：{dir}"));
        }
    }
    if p.managed
        && p.env
            .iter()
            .any(|e| crate::dependencies::operational_variable(&e.key))
    {
        return Err("托管模式不允许覆盖 PATH、缓存和解释器等保留变量；请使用“附加程序目录”添加 Blender 等外部程序路径。".into());
    }
    if p.headers.iter().any(|s| {
        s.value.contains(['\r', '\n'])
            || s.key
                .chars()
                .any(|c| !c.is_ascii_alphanumeric() && !"!#$%&'*+.^_`|~-".contains(c))
    }) {
        return Err("请求头名称或内容无效".into());
    }
    if p.source == "local" {
        if p.managed && p.stdio.is_empty() {
            crate::dependencies::validate_spec(&p.dependency)?;
        }
        if p.command.is_empty() && p.stdio.is_empty() && !p.managed {
            return Err("请填写程序名 / 完整启动命令；尚未安装 MCP 时，可选择“应用管理的 npm / Python 包”，点击“配置包与安装依赖”，填写包名后保存，无需先安装或运行 setup。".into());
        }
        if !p.cwd.is_empty() && !Path::new(&expand(&p.cwd)?).is_dir() {
            return Err("工作目录不存在".into());
        }
        if launch {
            let search = crate::environment::path(p)?;
            if p.stdio.is_empty() {
                resolve_with_path(&expand(&p.command)?, &search)?;
            } else {
                crate::shell::detect()?;
                crate::shell::check_dependency_with_path(&p.stdio, &search)?;
            }
        }
    } else if ["http", "sse"].contains(&p.source.as_str()) {
        crate::net::Url::parse(&expand(&p.url)?)?;
    } else {
        return Err("服务来源无效".into());
    }
    if !crate::network::valid_listen_address(p.listen_host()) {
        return Err("监听地址须为 0.0.0.0、127.0.0.1 或本机网卡的 IPv4 地址".into());
    }
    if launch {
        TcpListener::bind((p.listen_host(), p.port)).map_err(|e| {
            format!(
                "无法监听 {}:{}：{e}。请检查端口占用或刷新网卡地址。",
                p.listen_host(),
                p.port
            )
        })?;
    }
    Ok(())
}
pub fn build(p: &Profile, runtime: bool) -> Result<(String, BTreeMap<String, String>)> {
    if p.tls {
        return Err("上游网关 JSON 无法包含 HTTPS 入口。请由 GUI 管理此服务，并复制 HTTPS 地址或 Codex TOML。".into());
    }
    let mut secrets = BTreeMap::new();
    let mut cmd_path = None;
    let mut secret = |s: &str| -> Result<String> {
        if !runtime {
            return Ok(s.into());
        }
        let s = expand(s)?;
        if s.is_empty() {
            return Ok(s);
        }
        let key = format!("SGGUI_{}", secrets.len());
        secrets.insert(key.clone(), s);
        Ok(format!("${{{key}}}"))
    };
    let mut entry = json!({"outputTransport":p.output,"path":"/"});
    if p.source == "local" {
        if !p.stdio.is_empty() {
            let shell = crate::shell::detect()?;
            if shell.kind == crate::shell::Kind::Cmd {
                cmd_path = Some(shell.path.to_string_lossy().replace('/', "\\"));
            }
            crate::shell::configure(&mut entry, &shell, &p.stdio);
        } else {
            entry["command"] = if runtime {
                let search = crate::environment::path(p)?;
                json!(resolve_with_path(&expand(&p.command)?, &search)?)
            } else {
                json!(if ["npx", "npm"].contains(&p.command.as_str()) {
                    format!("{}.cmd", p.command)
                } else {
                    p.command.clone()
                })
            };
            entry["args"] = json!(p.args);
            let command = entry["command"].as_str().unwrap();
            if Path::new(command).extension().is_some_and(|ext| {
                ext.eq_ignore_ascii_case("cmd") || ext.eq_ignore_ascii_case("bat")
            }) {
                // Node's shell:false cannot launch Windows batch files.
                // Let the engine's documented stdio shell mode invoke cmd.
                let line = std::iter::once(crate::native::quote(command))
                    .chain(p.args.iter().map(|arg| crate::native::quote(arg)))
                    .collect::<Vec<_>>()
                    .join(" ");
                entry.as_object_mut().unwrap().remove("command");
                entry.as_object_mut().unwrap().remove("args");
                entry["stdio"] = json!(line);
            }
        }
        if !p.env.is_empty() {
            let env = p
                .env
                .iter()
                .map(|v| {
                    Ok((
                        v.key.clone(),
                        if p.managed_runtime && crate::dependencies::operational_variable(&v.key)
                            || p.environment_prepared && v.key.eq_ignore_ascii_case("PATH")
                        {
                            json!(v.value)
                        } else {
                            json!(secret(&v.value)?)
                        },
                    ))
                })
                .collect::<Result<serde_json::Map<String, Value>>>()?;
            entry["env"] = Value::Object(env);
        }
        if !p.cwd.is_empty() {
            entry["cwd"] = json!(if runtime {
                std::path::absolute(expand(&p.cwd)?).map_err(|e| e.to_string())?
            } else {
                PathBuf::from(&p.cwd)
            });
        }
    } else {
        entry["url"] = json!(secret(&p.url)?);
        entry["type"] = json!(p.source);
    }
    if !p.headers.is_empty() {
        entry["headers"] = Value::Object(
            p.headers
                .iter()
                .map(|v| Ok((v.key.clone(), json!(secret(&v.value)?))))
                .collect::<Result<_>>()?,
        );
    }
    if !p.bearer.is_empty() {
        entry["oauth2Bearer"] = json!(secret(&p.bearer)?);
    }
    if p.output == "streamableHttp" {
        entry["streamableHttpPath"] = json!(p.route);
        entry["stateful"] = json!(p.stateful);
        if p.stateful {
            entry["sessionTimeout"] = json!(p.session_timeout);
        }
    } else {
        entry["ssePath"] = json!(p.route);
        entry["messagePath"] = json!(if p.output == "ws" {
            &p.route
        } else {
            &p.message_path
        });
    }
    let mut keys = p.api_keys.clone();
    if !p.api_key.is_empty() {
        keys.push(p.api_key.clone());
    }
    keys.sort();
    keys.dedup();
    if !keys.is_empty() {
        entry["apiKey"] = json!(keys.iter().map(|s| secret(s)).collect::<Result<Vec<_>>>()?);
    }
    if !p.cors.is_empty() {
        entry["cors"] = if p.cors.trim() == "*" {
            json!(true)
        } else {
            json!(
                p.cors
                    .lines()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
            )
        };
    }
    if runtime && let Some(cmd) = cmd_path {
        secrets.insert("COMSPEC".into(), cmd);
    }
    Ok((serde_json::to_string_pretty(&json!({"port":p.port,"host":p.listen_host(),"logLevel":p.log_level,"logFormat":"json","healthEndpoint":["/_gateway_health"],"mcpServers":{"service":entry}})).map_err(|e| e.to_string())?, secrets))
}
pub fn codex_export(p: &Profile, engine: &Path) -> Result<String> {
    codex_export_with_ca(p, engine, None)
}
pub fn codex_export_with_ca(p: &Profile, _engine: &Path, _ca: Option<&Path>) -> Result<String> {
    crate::client::validate(&p.client)?;
    let quoted = |value: &str| serde_json::to_string(value).unwrap();
    let name = crate::client::table_name(p);
    let key = if p.api_key.is_empty() {
        p.api_keys
            .iter()
            .find(|key| !key.is_empty())
            .map(String::as_str)
            .unwrap_or("")
    } else {
        &p.api_key
    };
    let key = expand(key)?;
    let mut text = format!(
        "# SuperGateway 服务：{}\r\n# 粘贴到 Codex config.toml；先启动 GUI 中对应服务。\r\n[mcp_servers.{}]\r\n",
        quoted(&p.name),
        quoted(&name)
    );
    match p.output.as_str() {
        "streamableHttp" => {
            text.push_str(&format!("url = {}\r\n", quoted(&p.address(false))));
            if !key.is_empty() { text.push_str(&format!("http_headers = {{ \"X-API-Key\" = {} }}\r\n", quoted(&key))); }
        }
        "sse" => return Err("此客户端导出仅支持 Streamable HTTP；请切换输出协议，保存并在运行中应用重启。不生成 stdio 桥接配置。".into()),
        "ws" => return Err("Codex 的 MCP 配置不支持直接连接 WebSocket。请选择 Streamable HTTP 输出，保存配置；运行中请应用并重启。".into()),
        _ => return Err("无法导出此输出协议的 Codex 配置".into()),
    }
    crate::client::append_codex_options(&mut text, p)?;
    Ok(text)
}
pub struct Import {
    pub profiles: Vec<Profile>,
    pub messages: Vec<String>,
}
pub fn import(text: &str, existing: &[Profile]) -> Result<Import> {
    let root: Value = jsonc_parser::parse_to_serde_value(
        text,
        &jsonc_parser::ParseOptions {
            allow_comments: true,
            allow_trailing_commas: true,
            allow_loose_object_property_names: false,
            allow_missing_commas: false,
            allow_single_quoted_strings: false,
            allow_hexadecimal_numbers: false,
            allow_unary_plus_numbers: false,
            allow_bare_decimal_point_numbers: false,
            allow_non_finite_numbers: false,
            allow_extended_string_escapes: false,
        },
    )
    .map_err(|e| e.to_string())?;
    let servers = root
        .get("mcpServers")
        .and_then(Value::as_object)
        .ok_or("配置必须包含 mcpServers 对象")?;
    let supported: HashSet<&str> = "command,args,stdio,cwd,env,url,type,transportType,headers,oauth2Bearer,outputTransport,port,host,path,ssePath,messagePath,streamableHttpPath,stateful,sessionTimeout,apiKey,cors,logLevel,disabled,enabled".split(',').collect();
    let ignored: HashSet<&str> = "autoApprove,alwaysAllow,timeout,trust,alwaysLoad,disabledTools,includeTools,excludeTools,description,globalShortcut".split(',').collect();
    for key in root.as_object().ok_or("配置必须为对象")?.keys() {
        if !["mcpServers", "$schema"].contains(&key.as_str()) && !supported.contains(key.as_str()) {
            return Err(format!("暂不支持网关字段 {key}"));
        }
    }
    let mut all = existing.to_vec();
    let mut profiles = vec![];
    let mut messages = vec![];
    for (name, raw) in servers {
        let result = (|| -> Result<Profile> {
            let obj = raw.as_object().ok_or("服务必须为对象")?;
            for key in obj.keys() {
                if ignored.contains(key.as_str()) {
                    messages.push(format!("{name}：客户端字段 {key} 已忽略"));
                } else if !supported.contains(key.as_str()) {
                    return Err(format!("暂不支持字段 {key}，未导入此服务"));
                }
            }
            let get = |k: &str| raw.get(k).or_else(|| root.get(k));
            let text = |k: &str, fallback: &str| -> Result<String> {
                match get(k) {
                    None => Ok(fallback.into()),
                    Some(v) => v
                        .as_str()
                        .map(str::to_owned)
                        .ok_or(format!("{k} 必须为字符串")),
                }
            };
            let boolean = |k: &str, fallback| -> Result<bool> {
                match get(k) {
                    None => Ok(fallback),
                    Some(v) => v.as_bool().ok_or(format!("{k} 必须为布尔值")),
                }
            };
            let pairs = |k: &str| -> Result<Vec<Pair>> {
                match get(k) {
                    None => Ok(vec![]),
                    Some(v) => v
                        .as_object()
                        .ok_or(format!("{k} 必须为对象"))?
                        .iter()
                        .map(|(k, v)| {
                            Ok(Pair {
                                key: k.clone(),
                                value: v.as_str().ok_or("键值内容必须为字符串")?.into(),
                                ..Default::default()
                            })
                        })
                        .collect(),
                }
            };
            let mut p = Profile {
                name: name.clone(),
                command: text("command", "")?,
                stdio: text("stdio", "")?,
                cwd: text("cwd", "")?,
                url: text("url", "")?,
                env: pairs("env")?,
                headers: pairs("headers")?,
                bearer: text("oauth2Bearer", "")?,
                output: text("outputTransport", "streamableHttp")?,
                port: match get("port") {
                    None => free_port(&all)?,
                    Some(v) => v
                        .as_u64()
                        .and_then(|v| u16::try_from(v).ok())
                        .filter(|v| *v > 0)
                        .ok_or("端口须为 1–65535")?,
                },
                stateful: boolean("stateful", false)?,
                session_timeout: match get("sessionTimeout") {
                    None => 60000,
                    Some(v) => v.as_u64().ok_or("超时须为整数")?,
                },
                log_level: text("logLevel", "info")?,
                enabled: !boolean("disabled", false)? && boolean("enabled", true)?,
                ..Default::default()
            };
            let host = text("host", "0.0.0.0")?;
            let host = match host.as_str() {
                "localhost" => "127.0.0.1",
                "::" => "0.0.0.0",
                value => value,
            };
            if !crate::network::valid_listen_address(host) {
                return Err("监听地址须为 IPv4 地址，不支持主机名".into());
            }
            p.listen_address = Some(host.into());
            p.lan = host != "127.0.0.1";
            if (!p.command.is_empty() || !p.stdio.is_empty()) && !p.url.is_empty()
                || !p.command.is_empty() && !p.stdio.is_empty()
            {
                return Err("服务来源冲突".into());
            }
            p.source = if p.url.is_empty() {
                "local"
            } else {
                match text("type", &text("transportType", "http")?)?.as_str() {
                    "sse" => "sse",
                    "http" | "streamableHttp" | "streamable-http" | "streamable_http" => "http",
                    _ => return Err("远程协议无效".into()),
                }
            }
            .into();
            if p.output == "stdio" {
                p.output = "streamableHttp".into();
                messages.push(format!("{name}：stdio 输出转为 GUI 管理的 HTTP 网络输出"));
            }
            if let Some(v) = get("args") {
                p.args = strings(v)?;
                crate::arguments::normalize(&mut p.args);
            }
            if let Some(v) = get("apiKey") {
                p.api_keys = if let Some(s) = v.as_str() {
                    vec![s.into()]
                } else {
                    strings(v)?
                };
            }
            if let Some(v) = get("cors") {
                p.cors = match v.as_bool() {
                    Some(true) => "*".into(),
                    Some(false) => String::new(),
                    None => strings(v)?.join("\n"),
                };
            }
            let prefix = text("path", "")?;
            let prefix = prefix.trim_end_matches('/');
            let (key, default) = match p.output.as_str() {
                "sse" => ("ssePath", "/sse"),
                "ws" => ("messagePath", "/message"),
                _ => ("streamableHttpPath", "/mcp"),
            };
            p.route = format!("{prefix}{}", text(key, default)?);
            p.message_path = format!("{prefix}{}", text("messagePath", "/message")?);
            if all.iter().any(|s| s.port == p.port) {
                p.port = free_port(&all)?;
                messages.push(format!("{name}：端口重复，改用 {}", p.port));
            }
            let mut n = 2;
            while all.iter().any(|s| s.name == p.name) {
                p.name = format!("{name} ({n})");
                n += 1;
            }
            if p.name != *name {
                messages.push(format!("{name}：重名，保存为 {}", p.name));
            }
            validate(&p, false)?;
            Ok(p)
        })();
        match result {
            Ok(p) => {
                if let Some(d) = crate::dependencies::infer(&p) {
                    messages.push(format!(
                        "{}：可转换为托管 {} 依赖 {}；仅识别，不安装。",
                        p.name, d.kind, d.package
                    ));
                }
                messages.push(format!(
                    "✓ {}：{} → {}，端口 {}",
                    p.name, p.source, p.output, p.port
                ));
                all.push(p.clone());
                profiles.push(p);
            }
            Err(e) => messages.push(format!("✗ {name}：{e}")),
        }
    }
    Ok(Import { profiles, messages })
}
fn strings(v: &Value) -> Result<Vec<String>> {
    v.as_array()
        .ok_or("需要字符串数组")?
        .iter()
        .map(|v| v.as_str().map(str::to_owned).ok_or("需要字符串数组".into()))
        .collect()
}
