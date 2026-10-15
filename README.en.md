# SuperGateway GUI

A portable Windows x64 application built with Rust and native Win32 controls. Manage independent MCP gateways through Chinese or English forms, without .NET, Electron or a WebView. The bundled, pinned supergateway engine exposes local stdio MCP programs or remote HTTP / SSE servers over Streamable HTTP, SSE or WebSocket.

## Start

Extract the complete portable ZIP and run `SuperGatewayGUI.exe`. The GUI and engine need no system Node.js or Python. Third-party MCP programs may need dependencies; install them explicitly in the application or use an existing executable.

1. Add a service and choose its source.
2. Enter a program and individual arguments, or select a managed npm / Python package. Saving a valid package specification does not require installation.
3. Configure the listen IP, independent port and access key; save and start.
4. Test connection to initialize MCP and list tools. Copy the address or client configuration into your MCP client.

`0.0.0.0` listens on all adapters. Select `127.0.0.1` for local access or an adapter IP to restrict binding. Use HTTPS / WSS on a LAN and configure clients to trust the exported public CA. Gateway credentials and upstream credentials are separate.

## Language

Open **Settings → Language**, choose **System**, **Simplified Chinese** or **English**, and save. The change is immediate and does not restart running gateways or MCP services. Changing language does not require elevation, even when administrator login startup is enabled. System uses the Windows display language: Chinese selects Simplified Chinese; other languages select English.

The optional global `Language` setting is stored in DPAPI-encrypted `settings.dat`. It persists across launches and is excluded from service exports. User names, command lines, paths, arguments, variables and keys remain unchanged. Original MCP logs, operating-system diagnostics and existing model replies retain their language. Eight offline help chapters are available in both languages.

## Managed dependencies

Settings → Runtimes and dependencies manages private Node.js (npm / npx), uv (uvx) and Python. Each service has isolated packages; runtimes and download caches are shared. Runtime downloads use pinned versions with SHA256 verification. Package installation may execute third-party code and requires explicit confirmation.

The GUI never changes the user or system PATH. Per-service environment inheritance, variables and additional program folders affect child processes only. Managed tools and Python virtual environments take priority. Missing dependencies never trigger automatic installation or silent system-runtime fallback.

A separate one-shot command entry runs setup wizards in an interactive terminal after review. Keep `setup` separate from normal service arguments. For example, `mcp-for-blender` is a Python package; configure its normal package entry, install dependencies, then run `setup` as a temporary argument if required.

The HTTP proxy defaults to direct. Configure an optional proxy such as `http://127.0.0.1:10808` and npm / Python registries in dependency settings.

## AI setup wizard

Save `base_url`, `api_key` and `model` in Settings → AI setup wizard. Requests use `POST {base_url}/chat/completions`; include `/v1` only if the provider requires it. Testing sends a potentially billable request. Credentials are encrypted and excluded from service exports.

Enter a public GitHub repository or paste documentation. Preview the outgoing documents and environment summary, then **Confirm and send**. Successful generation opens Analysis. Review package checks and explanations before selectively applying a draft. English UI requests English explanations. AI never runs commands, installs packages, saves service changes or starts services. Saving, installation, setup and startup remain independent manual steps.

## Storage, clients and exit

Settings supports separate configuration and dependency folders. `--data-dir "D:\My configuration"` overrides the configuration directory. The default is `%LOCALAPPDATA%\SuperGatewayGUI`. Portable packages do not include your configuration or downloaded runtimes.

Client exports support network JSON and Codex HTTP TOML. Client timeout fields only affect exports. GUI profile import / export allows selecting services and includes environment, headers and keys; exchange files contain plaintext secrets, so store them securely. No independent stdio output launcher is provided.

Closing the main window hides it to the tray. Double-click the tray icon or reopen the same application and configuration directory to restore it. Exit asks before stopping running MCP gateways; No keeps them running. Confirmation closes stdin, then cleans the managed process tree after the timeout. Login startup is optional. Administrator login startup can only be changed while elevated.

## Build and validation

Use Rust MSVC x64 and Windows SDK / Visual C++ build tools. From PowerShell:

```powershell
./scripts/build-engine.ps1
./scripts/test.ps1
./scripts/package.ps1
```

Cargo.lock is fixed; release builds use static CRT, size optimization and LTO. Source archives include assets, translations, scripts, licenses and verification logs. See [DEVELOPMENT.md](DEVELOPMENT.md), [VALIDATION.md](VALIDATION.md) and [README.md](README.md) for detailed Chinese documentation and remaining acceptance limits. Clean Windows VM and performance results must be reported only when measured.

MIT license; bundled third-party licenses are provided in the portable package.
