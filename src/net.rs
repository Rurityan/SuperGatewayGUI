use crate::{
    Result,
    model::Profile,
    native::{error, wide},
};
use serde_json::{Value, json};
use std::{
    collections::HashSet,
    ffi::c_void,
    mem::size_of,
    ptr::{null, null_mut},
    sync::{
        Arc,
        atomic::{AtomicPtr, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};
use windows_sys::Win32::Networking::WinHttp::*;

#[derive(Clone, Debug)]
pub struct Tool {
    pub name: String,
    pub description: String,
}
#[derive(Clone, Debug)]
pub struct Url {
    pub host: String,
    pub port: u16,
    pub path: String,
    pub secure: bool,
}
impl Url {
    pub fn parse(url: &str) -> Result<Self> {
        let normalized = if let Some(v) = url.strip_prefix("ws://") {
            format!("http://{v}")
        } else if let Some(v) = url.strip_prefix("wss://") {
            format!("https://{v}")
        } else {
            url.to_string()
        };
        let text = wide(&normalized);
        let mut parts = URL_COMPONENTS {
            dwStructSize: size_of::<URL_COMPONENTS>() as u32,
            dwHostNameLength: u32::MAX,
            dwUrlPathLength: u32::MAX,
            dwExtraInfoLength: u32::MAX,
            dwUserNameLength: u32::MAX,
            dwPasswordLength: u32::MAX,
            ..Default::default()
        };
        if unsafe { WinHttpCrackUrl(text.as_ptr(), (text.len() - 1) as u32, 0, &mut parts) } == 0
            || parts.dwHostNameLength == 0
            || ![WINHTTP_INTERNET_SCHEME_HTTP, WINHTTP_INTERNET_SCHEME_HTTPS]
                .contains(&parts.nScheme)
            || parts.dwUserNameLength > 0
            || parts.dwPasswordLength > 0
        {
            return Err("URL 必须是 HTTP/HTTPS 地址；认证请使用请求头或 Bearer Token".into());
        }
        unsafe fn component(p: *const u16, n: u32) -> String {
            if n == 0 {
                String::new()
            } else {
                String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(p, n as usize) })
            }
        }
        let host = unsafe { component(parts.lpszHostName, parts.dwHostNameLength) };
        let mut path = unsafe { component(parts.lpszUrlPath, parts.dwUrlPathLength) };
        if path.is_empty() {
            path = "/".into();
        }
        path += &unsafe { component(parts.lpszExtraInfo, parts.dwExtraInfoLength) };
        Ok(Self {
            host,
            port: parts.nPort,
            path,
            secure: parts.nScheme == WINHTTP_INTERNET_SCHEME_HTTPS,
        })
    }
    fn origin(&self) -> String {
        format!(
            "{}://{}:{}",
            if self.secure { "https" } else { "http" },
            self.host,
            self.port
        )
    }
}
struct Internet(AtomicPtr<c_void>);
unsafe impl Send for Internet {}
unsafe impl Sync for Internet {}
impl Internet {
    fn new(raw: *mut c_void) -> Result<Arc<Self>> {
        if raw.is_null() {
            Err(error("WinHTTP"))
        } else {
            Ok(Arc::new(Self(AtomicPtr::new(raw))))
        }
    }
    fn raw(&self) -> *mut c_void {
        self.0.load(Ordering::Acquire)
    }
    fn close(&self) {
        let raw = self.0.swap(null_mut(), Ordering::AcqRel);
        if !raw.is_null() {
            unsafe {
                WinHttpCloseHandle(raw);
            }
        }
    }
}
impl Drop for Internet {
    fn drop(&mut self) {
        self.close();
    }
}
fn remaining(deadline: Instant) -> Result<i32> {
    let remaining = deadline
        .saturating_duration_since(Instant::now())
        .as_millis();
    if remaining == 0 {
        Err("连接测试超时，请检查网络、认证或服务日志".into())
    } else {
        Ok(remaining.min(i32::MAX as u128) as i32)
    }
}
fn timeout(h: &Internet, deadline: Instant) -> Result<()> {
    let ms = remaining(deadline)?;
    if unsafe { WinHttpSetTimeouts(h.raw(), ms, ms, ms, ms) } == 0 {
        return Err(error("设置网络超时"));
    }
    Ok(())
}
struct Http {
    session: Arc<Internet>,
}
struct Deadline {
    cancel: mpsc::Sender<()>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Deadline {
    fn new(handle: Arc<Internet>, deadline: Instant) -> Self {
        let (cancel, rx) = mpsc::channel();
        let duration = deadline.saturating_duration_since(Instant::now());
        Self {
            cancel,
            worker: Some(thread::spawn(move || {
                if rx.recv_timeout(duration).is_err() {
                    handle.close();
                }
            })),
        }
    }
}
impl Drop for Deadline {
    fn drop(&mut self) {
        let _ = self.cancel.send(());
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
struct Response {
    _deadline: Deadline,
    request: Arc<Internet>,
    _connection: Arc<Internet>,
    status: u32,
}
impl Http {
    fn new() -> Result<Self> {
        Ok(Self {
            session: Internet::new(unsafe {
                WinHttpOpen(
                    wide("SuperGatewayGUI/2.0").as_ptr(),
                    WINHTTP_ACCESS_TYPE_NO_PROXY,
                    null(),
                    null(),
                    0,
                )
            })?,
        })
    }
    fn request(
        &self,
        method: &str,
        url: &Url,
        headers: &[(String, String)],
        body: &[u8],
        deadline: Instant,
        upgrade: bool,
    ) -> Result<Response> {
        timeout(&self.session, deadline)?;
        let connection = Internet::new(unsafe {
            WinHttpConnect(self.session.raw(), wide(&url.host).as_ptr(), url.port, 0)
        })?;
        let request = Internet::new(unsafe {
            WinHttpOpenRequest(
                connection.raw(),
                wide(method).as_ptr(),
                wide(&url.path).as_ptr(),
                null(),
                null(),
                null(),
                if url.secure { WINHTTP_FLAG_SECURE } else { 0 },
            )
        })?;
        timeout(&request, deadline)?;
        let guard = Deadline::new(request.clone(), deadline);
        let policy = WINHTTP_OPTION_REDIRECT_POLICY_NEVER;
        unsafe {
            WinHttpSetOption(
                request.raw(),
                WINHTTP_OPTION_REDIRECT_POLICY,
                (&policy as *const u32).cast(),
                4,
            );
        }
        if upgrade
            && unsafe {
                WinHttpSetOption(
                    request.raw(),
                    WINHTTP_OPTION_UPGRADE_TO_WEB_SOCKET,
                    null(),
                    0,
                )
            } == 0
        {
            return Err(error("准备 WebSocket"));
        }
        let text = headers
            .iter()
            .map(|(k, v)| format!("{k}: {v}\r\n"))
            .collect::<String>();
        let encoded = wide(&text);
        if unsafe {
            WinHttpSendRequest(
                request.raw(),
                encoded.as_ptr(),
                (encoded.len() - 1) as u32,
                if body.is_empty() {
                    null()
                } else {
                    body.as_ptr().cast()
                },
                body.len() as u32,
                body.len() as u32,
                0,
            )
        } == 0
        {
            return Err(request_error("发送 MCP 请求", url.secure));
        }
        timeout(&request, deadline)?;
        if unsafe { WinHttpReceiveResponse(request.raw(), null_mut()) } == 0 {
            return Err(request_error("接收 MCP 响应", url.secure));
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
            return Err(error("读取 HTTP 状态"));
        }
        Ok(Response {
            _deadline: guard,
            request,
            _connection: connection,
            status,
        })
    }
}
fn request_error(context: &str, secure: bool) -> String {
    if secure
        && unsafe { windows_sys::Win32::Foundation::GetLastError() } == ERROR_WINHTTP_SECURE_FAILURE
    {
        "TLS 证书验证失败：GUI 网关请先在设置中信任本机 CA；远程地址请检查客户端信任、证书有效期及 IP 是否匹配。不会忽略验证或回退 HTTP。".into()
    } else {
        error(context)
    }
}
impl Response {
    fn header(&self, name: &str) -> Option<String> {
        let mut text = vec![0u16; 8192];
        let mut bytes = (text.len() * 2) as u32;
        if unsafe {
            WinHttpQueryHeaders(
                self.request.raw(),
                WINHTTP_QUERY_CUSTOM,
                wide(name).as_ptr(),
                text.as_mut_ptr().cast(),
                &mut bytes,
                null_mut(),
            )
        } == 0
        {
            None
        } else {
            Some(
                String::from_utf16_lossy(&text[..(bytes as usize / 2).min(text.len())])
                    .trim_end_matches('\0')
                    .into(),
            )
        }
    }
    fn success(&self) -> Result<()> {
        if (200..300).contains(&self.status) {
            Ok(())
        } else {
            Err(format!("网关返回 HTTP {}，请检查认证与日志", self.status))
        }
    }
    fn read(&self, deadline: Instant) -> Result<Vec<u8>> {
        let mut result = Vec::new();
        loop {
            let chunk = read_chunk(&self.request, deadline)?;
            if chunk.is_empty() {
                return Ok(result);
            }
            result.extend(chunk);
            if result.len() > 4 * 1024 * 1024 {
                return Err("MCP 响应超过 4 MiB".into());
            }
        }
    }
}
fn read_chunk(request: &Internet, deadline: Instant) -> Result<Vec<u8>> {
    timeout(request, deadline)?;
    let mut available = 0;
    if unsafe { WinHttpQueryDataAvailable(request.raw(), &mut available) } == 0 {
        return Err(error("读取可用 MCP 数据"));
    }
    if available == 0 {
        return Ok(vec![]);
    }
    let mut buffer = vec![0u8; available.min(8192) as usize];
    let mut got = 0;
    if unsafe {
        WinHttpReadData(
            request.raw(),
            buffer.as_mut_ptr().cast(),
            buffer.len() as u32,
            &mut got,
        )
    } == 0
    {
        return Err(error("读取 MCP 响应"));
    }
    buffer.truncate(got as usize);
    Ok(buffer)
}
struct Sse {
    bytes: Vec<u8>,
    data: String,
    kind: String,
}
impl Sse {
    fn new() -> Self {
        Self {
            bytes: vec![],
            data: String::new(),
            kind: "message".into(),
        }
    }
    fn feed(&mut self, bytes: &[u8]) -> Result<Vec<(String, String)>> {
        self.bytes.extend(bytes);
        let mut events = vec![];
        while let Some(end) = self.bytes.iter().position(|v| *v == b'\n') {
            let line =
                String::from_utf8(self.bytes.drain(..=end).collect()).map_err(|e| e.to_string())?;
            let line = line.trim_end_matches(['\r', '\n']);
            if line.is_empty() {
                if !self.data.is_empty() {
                    events.push((
                        std::mem::replace(&mut self.kind, "message".into()),
                        self.data.trim_end_matches('\n').into(),
                    ));
                    self.data.clear();
                }
            } else if let Some(v) = line.strip_prefix("event:") {
                self.kind = v.trim().into();
            } else if let Some(v) = line.strip_prefix("data:") {
                self.data.push_str(v.strip_prefix(' ').unwrap_or(v));
                self.data.push('\n');
            }
        }
        if self.bytes.len() + self.data.len() > 4 * 1024 * 1024 {
            return Err("SSE 消息超过 4 MiB".into());
        }
        Ok(events)
    }
}
pub fn health(port: u16) -> Result<()> {
    health_at("127.0.0.1", port)
}
pub fn profile_health(profile: &Profile) -> Result<()> {
    let url = format!(
        "{}://{}:{}/_gateway_health",
        if profile.tls { "https" } else { "http" },
        profile.connection_host(true),
        profile.port
    );
    Http::new()?
        .request(
            "GET",
            &Url::parse(&url)?,
            &[],
            &[],
            Instant::now() + Duration::from_millis(500),
            false,
        )?
        .success()
}
fn health_at(host: &str, port: u16) -> Result<()> {
    let response = Http::new()?.request(
        "GET",
        &Url::parse(&format!("http://{host}:{port}/_gateway_health"))?,
        &[],
        &[],
        Instant::now() + Duration::from_millis(500),
        false,
    )?;
    response.success()
}
pub struct Session {
    profile: Profile,
    http: Http,
    url: Url,
    post: Url,
    headers: Vec<(String, String)>,
    session_id: Option<String>,
    protocol: String,
    id: u64,
    deadline: Instant,
    watchdog: Option<(mpsc::Sender<()>, thread::JoinHandle<()>)>,
    socket: Option<Arc<Internet>>,
    socket_connection: Option<Arc<Internet>>,
    stream: Option<Response>,
    reader: Option<thread::JoinHandle<()>>,
    rx: Option<mpsc::Receiver<Result<(String, String)>>>,
}
impl Session {
    pub fn connect(p: &Profile, timeout_duration: Duration) -> Result<Self> {
        let url = Url::parse(&p.address(true))?;
        let mut headers = vec![
            (
                "Accept".into(),
                "application/json, text/event-stream".into(),
            ),
            ("Content-Type".into(), "application/json".into()),
        ];
        let key = if !p.api_key.is_empty() {
            Some(p.api_key.as_str())
        } else {
            p.api_keys.first().map(String::as_str)
        };
        if let Some(key) = key {
            headers.push((
                "Authorization".into(),
                format!("Bearer {}", crate::config::expand(key)?),
            ));
        }
        let mut wire = Self {
            profile: p.clone(),
            http: Http::new()?,
            post: url.clone(),
            url,
            headers,
            session_id: None,
            protocol: "2024-11-05".into(),
            id: 0,
            deadline: Instant::now() + timeout_duration,
            watchdog: None,
            socket: None,
            socket_connection: None,
            stream: None,
            reader: None,
            rx: None,
        };
        if p.output == "ws" {
            let response =
                wire.http
                    .request("GET", &wire.url, &wire.headers, &[], wire.deadline, true)?;
            if response.status != 101 {
                return Err(format!("WebSocket 返回 HTTP {}", response.status));
            }
            wire.socket = Some(Internet::new(unsafe {
                WinHttpWebSocketCompleteUpgrade(response.request.raw(), 0)
            })?);
            wire.socket_connection = Some(response._connection.clone());
            let socket = wire.socket.as_ref().unwrap().clone();
            let (cancel, rx) = mpsc::channel();
            let duration = wire.deadline.saturating_duration_since(Instant::now());
            wire.watchdog = Some((
                cancel,
                thread::spawn(move || {
                    if rx.recv_timeout(duration).is_err() {
                        socket.close();
                    }
                }),
            ));
        } else if p.output == "sse" {
            let response = wire.http.request(
                "GET",
                &wire.url,
                &[("Accept".into(), "text/event-stream".into())]
                    .into_iter()
                    .chain(
                        wire.headers
                            .iter()
                            .filter(|(k, _)| k == "Authorization")
                            .cloned(),
                    )
                    .collect::<Vec<_>>(),
                &[],
                wire.deadline,
                false,
            )?;
            response.success()?;
            let request = response.request.clone();
            let deadline = wire.deadline;
            let (tx, rx) = mpsc::channel();
            wire.reader = Some(thread::spawn(move || {
                let result = (|| -> Result<()> {
                    let mut sse = Sse::new();
                    loop {
                        let bytes = read_chunk(&request, deadline)?;
                        if bytes.is_empty() {
                            return Err("SSE 连接已关闭".into());
                        }
                        for event in sse.feed(&bytes)? {
                            if tx.send(Ok(event)).is_err() {
                                return Ok(());
                            }
                        }
                    }
                })();
                if let Err(e) = result {
                    let _ = tx.send(Err(e));
                }
            }));
            wire.rx = Some(rx);
            wire.stream = Some(response);
            loop {
                let (kind, data) = wire.receive_sse()?;
                if kind == "endpoint" {
                    let endpoint = if data.starts_with('/') {
                        format!("{}{}", wire.url.origin(), data)
                    } else {
                        data
                    };
                    let post = Url::parse(&endpoint)?;
                    if post.origin() != wire.url.origin() {
                        return Err("SSE 消息地址必须属于同一网关".into());
                    }
                    wire.post = post;
                    break;
                }
            }
        }
        Ok(wire)
    }
    fn receive_sse(&self) -> Result<(String, String)> {
        self.rx
            .as_ref()
            .unwrap()
            .recv_timeout(Duration::from_millis(remaining(self.deadline)? as u64))
            .map_err(|_| "SSE 响应超时或连接已断开".to_string())?
    }
    fn send(&mut self, payload: Value, reply: bool) -> Result<Option<Value>> {
        let bytes = serde_json::to_vec(&payload).unwrap();
        if let Some(socket) = &self.socket {
            remaining(self.deadline)?;
            let code = unsafe {
                WinHttpWebSocketSend(
                    socket.raw(),
                    WINHTTP_WEB_SOCKET_UTF8_MESSAGE_BUFFER_TYPE,
                    bytes.as_ptr().cast(),
                    bytes.len() as u32,
                )
            };
            if code != 0 {
                return Err(format!("WebSocket 发送失败：{code}"));
            }
            return Ok(None);
        }
        let mut headers = self.headers.clone();
        headers.push(("MCP-Protocol-Version".into(), self.protocol.clone()));
        if let Some(id) = &self.session_id {
            headers.push(("Mcp-Session-Id".into(), id.clone()));
        }
        let response =
            self.http
                .request("POST", &self.post, &headers, &bytes, self.deadline, false)?;
        response.success()?;
        if let Some(id) = response.header("Mcp-Session-Id") {
            self.session_id = Some(id);
        }
        if !reply || self.profile.output == "sse" {
            return Ok(None);
        }
        if response
            .header("Content-Type")
            .unwrap_or_default()
            .starts_with("text/event-stream")
        {
            let mut sse = Sse::new();
            loop {
                let chunk = read_chunk(&response.request, self.deadline)?;
                if chunk.is_empty() {
                    return Err("HTTP 事件流未返回请求结果".into());
                }
                for (_, data) in sse.feed(&chunk)? {
                    let value: Value = serde_json::from_str(&data).map_err(|e| e.to_string())?;
                    if value.get("id") == Some(&json!(self.id)) {
                        return Ok(Some(value));
                    }
                }
            }
        } else {
            Ok(Some(
                serde_json::from_slice(&response.read(self.deadline)?)
                    .map_err(|e| e.to_string())?,
            ))
        }
    }
    fn receive(&self) -> Result<Value> {
        if let Some(socket) = &self.socket {
            let mut data = vec![];
            loop {
                remaining(self.deadline)?;
                let mut bytes = vec![0u8; 8192];
                let mut count = 0;
                let mut kind = WINHTTP_WEB_SOCKET_UTF8_MESSAGE_BUFFER_TYPE;
                let code = unsafe {
                    WinHttpWebSocketReceive(
                        socket.raw(),
                        bytes.as_mut_ptr().cast(),
                        bytes.len() as u32,
                        &mut count,
                        &mut kind,
                    )
                };
                if code != 0 {
                    return Err(format!("WebSocket 接收失败：{code}"));
                }
                if kind == WINHTTP_WEB_SOCKET_CLOSE_BUFFER_TYPE {
                    return Err("WebSocket 已关闭".into());
                }
                data.extend(&bytes[..count as usize]);
                if data.len() > 4 * 1024 * 1024 {
                    return Err("MCP 消息过大".into());
                }
                if [
                    WINHTTP_WEB_SOCKET_UTF8_MESSAGE_BUFFER_TYPE,
                    WINHTTP_WEB_SOCKET_BINARY_MESSAGE_BUFFER_TYPE,
                ]
                .contains(&kind)
                {
                    return serde_json::from_slice(&data).map_err(|e| e.to_string());
                }
            }
        }
        loop {
            let (kind, data) = self.receive_sse()?;
            if kind == "message" {
                return serde_json::from_str(&data).map_err(|e| e.to_string());
            }
        }
    }
    pub fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        self.id += 1;
        let id = self.id;
        let direct = self.send(
            json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}),
            true,
        )?;
        let mut direct = direct;
        loop {
            let response = match direct.take() {
                Some(v) => v,
                None => self.receive()?,
            };
            if response.get("method").is_some() && response.get("id").is_some() {
                self.send(json!({"jsonrpc":"2.0","id":response["id"],"error":{"code":-32601,"message":"Probe client does not support server requests"}}),false)?;
                continue;
            }
            if response.get("id") != Some(&json!(id)) {
                continue;
            }
            if let Some(error) = response.get("error") {
                return Err(format!("MCP 错误：{error}"));
            }
            return response
                .get("result")
                .cloned()
                .ok_or("MCP 响应缺少 result".into());
        }
    }
    pub fn initialize(&mut self) -> Result<Value> {
        let value=self.call("initialize",json!({"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"SuperGatewayGUI","version":"2.0.0"}}))?;
        self.protocol = value["protocolVersion"]
            .as_str()
            .ok_or("MCP 初始化缺少协议版本")?
            .into();
        self.send(
            json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
            false,
        )?;
        Ok(value)
    }
    pub fn tools(&mut self) -> Result<Vec<Tool>> {
        let init = self.initialize()?;
        if init["capabilities"].get("tools").is_none() {
            return Ok(vec![]);
        }
        let mut tools = vec![];
        let mut cursor = None;
        let mut seen = HashSet::new();
        for _ in 0..100 {
            let value = self.call(
                "tools/list",
                cursor
                    .as_ref()
                    .map(|c| json!({"cursor":c}))
                    .unwrap_or(json!({})),
            )?;
            for tool in value["tools"].as_array().ok_or("工具列表无效")? {
                tools.push(Tool {
                    name: tool["name"].as_str().ok_or("工具缺少名称")?.into(),
                    description: tool["description"].as_str().unwrap_or("").into(),
                });
            }
            cursor = value["nextCursor"].as_str().map(str::to_owned);
            if let Some(c) = &cursor {
                if !seen.insert(c.clone()) {
                    return Err("工具分页游标重复".into());
                }
            } else {
                return Ok(tools);
            }
        }
        Err("工具列表超过 100 页".into())
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        if let Some(id) = &self.session_id {
            let mut headers = self.headers.clone();
            headers.push(("Mcp-Session-Id".into(), id.clone()));
            headers.push(("MCP-Protocol-Version".into(), self.protocol.clone()));
            let _ = self.http.request(
                "DELETE",
                &self.post,
                &headers,
                &[],
                Instant::now() + Duration::from_secs(2),
                false,
            );
        }
        if let Some(socket) = &self.socket {
            socket.close();
        }
        if let Some(stream) = &self.stream {
            stream.request.close();
        }
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
        if let Some((cancel, watchdog)) = self.watchdog.take() {
            let _ = cancel.send(());
            let _ = watchdog.join();
        }
    }
}
pub fn test(p: &Profile) -> Result<Vec<Tool>> {
    Session::connect(p, Duration::from_secs(25))?.tools()
}
