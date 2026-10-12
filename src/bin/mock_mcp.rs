use serde_json::{Value, json};
use std::io::{self, BufRead, Write};
fn main() {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.iter().any(|s| s == "--version") {
        println!("Blender 4.5.0 (controlled fixture)");
        return;
    }
    if let Ok(file) = std::env::var("RECORD_FILE") {
        use std::fs::OpenOptions;
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(file)
            .unwrap();
        writeln!(
            f,
            "{} {}",
            std::process::id(),
            serde_json::to_string(&args).unwrap()
        )
        .unwrap();
    }
    if let Ok(secret) = std::env::var("SECRET") {
        eprintln!("fixture-secret: {secret}");
    }
    let mut stdout = io::stdout().lock();
    for line in io::stdin()
        .lock()
        .lines()
        .map_while(std::result::Result::ok)
    {
        let Ok(m) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let Some(id) = m.get("id") else {
            continue;
        };
        if args.iter().any(|s| s == "--hang") {
            std::thread::sleep(std::time::Duration::from_secs(60));
            continue;
        }
        if args.iter().any(|s| s == "--crash") {
            std::process::exit(7);
        }
        let result = match m["method"].as_str().unwrap_or("") {
            "initialize" => {
                json!({"protocolVersion":"2024-11-05","capabilities":if args.iter().any(|s|s=="--no-tools"){json!({})}else{json!({"tools":{}})},"serverInfo":{"name":"gui-fixture","version":"1"}})
            }
            "tools/list" => {
                if args.iter().any(|s| s == "--env-report") {
                    let description = json!({
                        "marker":std::env::var("SG_SERVICE_MARKER").ok(),
                        "empty":std::env::var("SG_EMPTY").ok(),
                        "parent":std::env::var("SG_PARENT_ONLY").ok(),
                        "client":std::env::var("SG_CLIENT_ONLY").ok(),
                        "path":std::env::var("PATH").ok(),
                        "blender":gateway::config::resolve("blender").ok().and_then(|exe| std::process::Command::new(exe).arg("--version").output().ok()).map(|out|String::from_utf8_lossy(&out.stdout).into_owned())
                    });
                    json!({"tools":[{"name":"environment","description":description.to_string(),"inputSchema":{"type":"object"}}]})
                } else if args.iter().any(|s| s == "--pages") {
                    if m["params"]["cursor"] == "second" {
                        json!({"tools":[{"name":"second","inputSchema":{"type":"object"}}]})
                    } else {
                        json!({"tools":[{"name":"echo","inputSchema":{"type":"object"}}],"nextCursor":"second"})
                    }
                } else {
                    json!({"tools":[{"name":"echo","description":"中文测试工具","inputSchema":{"type":"object"}}]})
                }
            }
            _ => json!({}),
        };
        writeln!(
            stdout,
            "{}",
            json!({"jsonrpc":"2.0","id":id,"result":result})
        )
        .unwrap();
        stdout.flush().unwrap();
    }
}
