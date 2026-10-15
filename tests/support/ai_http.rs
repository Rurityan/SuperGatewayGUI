//! Loopback-only mock. No external requests or installations.
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};
#[derive(Clone, Debug)]
pub struct Request {
    pub line: String,
    pub headers: String,
    pub body: Vec<u8>,
}
pub struct Mock {
    pub url: String,
    pub requests: Arc<Mutex<Vec<Request>>>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Mock {
    pub fn new(handler: impl Fn(&Request) -> (u16, Vec<u8>) + Send + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let requests = Arc::new(Mutex::new(vec![]));
        let end = stop.clone();
        let captured = requests.clone();
        let worker = thread::spawn(move || {
            while !end.load(Ordering::SeqCst) {
                let Ok((mut socket, _)) = listener.accept() else {
                    thread::sleep(Duration::from_millis(5));
                    continue;
                };
                // Winsock accept inherits the nonblocking listener mode.
                socket.set_nonblocking(false).unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let mut raw = vec![];
                let mut buf = [0u8; 4096];
                let mut header_end = None;
                let mut length = 0;
                while let Ok(n) = socket.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    raw.extend_from_slice(&buf[..n]);
                    if header_end.is_none()
                        && let Some(i) = raw.windows(4).position(|v| v == b"\r\n\r\n")
                    {
                        header_end = Some(i + 4);
                        let h = String::from_utf8_lossy(&raw[..i]);
                        length = h
                            .lines()
                            .filter_map(|l| l.split_once(':'))
                            .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
                            .and_then(|(_, v)| v.trim().parse::<usize>().ok())
                            .unwrap_or(0);
                    }
                    if header_end.is_some_and(|i| raw.len() >= i + length) {
                        break;
                    }
                }
                let Some(i) = header_end else {
                    continue;
                };
                let headers = String::from_utf8_lossy(&raw[..i]).to_string();
                let request = Request {
                    line: headers.lines().next().unwrap_or("").into(),
                    headers,
                    body: raw[i..].to_vec(),
                };
                captured.lock().unwrap().push(request.clone());
                let (status, body) = handler(&request);
                let header = format!(
                    "HTTP/1.1 {status} Mock\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = socket.write_all(header.as_bytes());
                let _ = socket.write_all(&body);
            }
        });
        Self {
            url: format!("http://{address}"),
            requests,
            stop,
            worker: Some(worker),
        }
    }
}
impl Drop for Mock {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(self.url.trim_start_matches("http://"));
        if let Some(w) = self.worker.take() {
            let _ = w.join();
        }
    }
}
