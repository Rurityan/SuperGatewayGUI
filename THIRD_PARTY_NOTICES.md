# 第三方组件

- Supergateway：MIT，固定提交 `292048a5a35c77984873bc741ffd196663c5a241`，引擎版本 `4.2.0-rc.1`。来源：https://github.com/supercorp-ai/supergateway 。此版本支持配置文件、远程转接、多协议及认证。
- HTTPS / WSS 使用内嵌 Rust TLS 实现：rustls、tokio-rustls、tokio、rcgen、ring、time 及锁定的传递依赖，具体版本与许可证见 Cargo.lock 和便携包 licenses；无额外 HTTPS 代理 EXE 或运行时。
- 引擎通过上游 `scripts/build-binary.mjs` 构建 Node Single Executable Application。Supergateway、Node 与嵌入 npm 运行库许可在 `engine/LICENSE` 和 `engine/THIRD_PARTY_LICENSES.txt`。
- 引擎构建使用 `@modelcontextprotocol/client 2.3.1`、`@modelcontextprotocol/sdk 1.32.1`、`proxy-addr 2.0.8`。依赖版本与 integrity 固定在 `scripts/engine-package-lock.json`；引擎清单记录锁文件 SHA256。
- Rust GUI 使用 serde、serde_json、jsonc-parser、sha2、windows-sys 及其锁定的传递依赖。版本与许可声明列于便携包 `licenses/RUST-DEPENDENCIES.txt`，各包原始许可文件位于对应子目录。依赖源码来自 https://crates.io ，固定版本见 Cargo.lock。
- GUI 采用 MSVC 静态 CRT 链接；只使用 Windows 系统 API，没有随包交付 .NET、WPF、Windows Forms、浏览器或 WebView 运行时。
- 便携包不预装托管运行环境。用户点击下载时使用 Node.js 24.21.0 LTS（原包保留 LICENSE）、uv 0.13.0（MIT / Apache-2.0）、CPython 3.13.16 的 python-build-standalone 20261009 便携发行（Python 许可及发行工具 MPL-2.0 许可）。固定来源和 SHA256 嵌入程序；uv / Python 许可副本位于源码 assets/dependencies-licenses 并在安装时写入运行环境 SUPERGATEWAY-LICENSES，发行包内其他许可文件保留。发行工具源码：https://github.com/astral-sh/python-build-standalone/tree/20261009 。
- 运行环境解压使用 zip、flate2、tar 及 Cargo.lock 固定的传递依赖，其原始许可列入便携包 licenses。第三方 MCP 包的许可随其独立 npm / Python 安装目录保留，用户自行选择和安装这些包。
- Windows 图标资源编译使用构建依赖 embed-resource 及其锁定的传递依赖，许可一并列入 `licenses`。这些依赖不随 GUI 引入运行时库。
- 本应用“网关与连接节点”图标通过 Image MCP 的 image_gen 工具生成，生成提示词及原始输出保存在源码包 `assets` 目录，图标资产按本项目许可分发。

本项目为基于 Supergateway 的独立 GUI，不代表上游官方产品。
