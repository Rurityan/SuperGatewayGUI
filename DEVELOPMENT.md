# 开发说明

同步日期：2026-10-12。本文件描述当前 Rust Win32 实现，Cargo 版本 `2.0.0`。产品使用说明见 [README](README.md)。历史 .NET 实现仅在开发机 `legacy/dotnet` 本地归档，整个 `legacy` 不提交到 GitHub，也不参与 Cargo 构建。克隆仓库不包含旧版，常规构建和打包不要求该目录存在。

## 开发环境

开发及自动 UI 验收使用 Windows x64，Rust MSVC 工具链、Visual Studio C++ Build Tools 和 Windows SDK。Rust 使用 edition 2024；已有验收工具链为 Rust 1.96.0。PowerShell 脚本建议在 PowerShell 7 下运行。资源编译需要 SDK 的 `rc.exe`，重建网关还需要 Git、Node.js 24、npm 和 `signtool.exe`。

`.cargo/config.toml` 固定 `x86_64-pc-windows-msvc` 并启用静态 CRT。Release 使用体积优化、LTO、单代码生成单元和符号剥离。提交 `Cargo.lock`，构建及测试使用 `--locked`；应用本身不需要 Rust、Node、Python、.NET 或 GUI 运行时。

## 首次准备与构建

从源码 ZIP 解压后，进入包含 `Cargo.toml` 的目录；从 GitHub 克隆后同样在仓库根目录执行：

```powershell
New-Item -ItemType Directory -Force artifacts | Out-Null
./scripts/build-engine.ps1
cargo build --release --locked
```

网关脚本固定上游提交 `292048a5a35c77984873bc741ffd196663c5a241`，使用 `scripts/engine-package.json` 和 `engine-package-lock.json`，构建独立 Windows EXE 并检查所需参数。当前引擎版本为 `4.2.0-rc.1`。首次准备会访问 GitHub 和 npm；它是开发步骤，用户机器不会运行 `npx supergateway`。

也可以将可信的本项目便携发布包中整个 `engine` 目录复制到仓库根目录，再执行网关脚本验证缓存。需包含 `supergateway.exe`、`manifest.json`、`LICENSE` 和 `THIRD_PARTY_LICENSES.txt`。只编译 GUI 不需要 Node；运行网关、协议测试和便携打包需要这些引擎文件。强制重建使用 `./scripts/build-engine.ps1 -Rebuild`。

`engine/manifest.json` 记录实际引擎、上游提交及依赖锁文件校验值。不同开发环境重建的 EXE 不承诺字节相同；不要套用旧 EXE 的 SHA256。发布脚本和启动检查均使用对应清单验证。

构建输出为 `target/x86_64-pc-windows-msvc/release`。开发运行时需指定包内引擎路径，例如先执行打包脚本，再运行 `artifacts/SuperGatewayGUI-win-x64/SuperGatewayGUI.exe`；测试程序自行使用仓库中的引擎和隔离配置目录。

## 模块职责

| 文件 | 职责 |
| --- | --- |
| `src/main.rs`、`ui.rs`、`ui_dependencies.rs`、`help.rs` | 启动入口、原生控件、依赖管理界面和内嵌中文帮助 |
| `src/model.rs`、`store.rs`、`storage.rs` | PascalCase 配置、DPAPI、原子保存、备份和配置 / 依赖路径 |
| `src/config.rs`、`shell.rs`、`environment.rs` | 配置校验与生成、启动解释器、每服务环境合并和 PATH |
| `src/manager.rs`、`native.rs` | 独立服务状态、工作线程、Win32 进程 / 管道 / Job Object |
| `src/dependencies.rs`、`oneshot.rs`、`stdio.rs` | 固定环境与包安装、一次性终端向导、加密 stdio 导出启动器 |
| `src/net.rs`、`network.rs`、`tls.rs`、`startup.rs` | MCP 协议诊断、网卡发现、内嵌 TLS、普通 / 管理员登录启动 |
| `src/bin`、`tests` | 自包含 MCP 模拟程序、核心 / UI 检查和专项集成测试 |
| `assets`、`build.rs`、`app.manifest` | 图标源图及提示词、许可、Windows 资源和 DPI 清单 |

GUI 通过工作线程执行进程与网络操作，用消息和事件更新界面。每个服务独立网关和端口。GUI 创建进程时先挂起，加入 Job Object 再恢复；停止先关闭 stdin，8 秒后仍未退出则清理进程树。stdio 客户端启动器有独立生命周期；一次性终端可以在 GUI 退出后继续由用户操作。

## 修改时需要保持的行为

服务保存和导入只校验、写配置，不下载或执行命令；有效托管包配置在未安装、程序路径为空时可以保存。缺少环境在启动时提示安装，不静默回退系统解释器。完整命令非空仍优先，解释器在应用启动时按 pwsh 7、Windows PowerShell、cmd 顺序确定，不因命令报错切换。

GUI 启动环境快照只读。变量名不区分大小写，服务变量覆盖继承值，空值保持；不写用户 / 系统环境变量。托管 PATH 顺序为服务虚拟环境、托管工具、服务附加目录、Windows 必要目录、继承 PATH。关闭继承省略最后一项；原有程序模式仍允许显式 PATH 覆盖，附加目录排在前面。正常服务、完整命令和一次性向导采用同一环境规则，安装任务有单独受控环境。

保留旧配置默认值、PascalCase、DPAPI 和读取失败不覆盖机制。配置目录保存 `settings.dat`、证书、备份与 `exports`；依赖目录可独立设置，路径记录在 `storage.json`。移动配置复制到空目录并保留原件，依赖不迁移。加密记录绑定当前 Windows 用户；stdio 导出保存入口和环境快照，移动目录或改变配置后重新导出。

界面下拉框通过公共创建函数安装 Win32 subclass。收起时转发垂直滚轮到所在页面；展开时下拉框和弹出列表忽略滚轮，点击及键盘仍可选择。新下拉框应使用这些创建函数，避免重新引入滚动误改。

运行中配置修改只标记待应用，由用户重启该服务生效。日志限制容量并脱敏。TLS 在应用内实现，服务共用证书及 TLS 工作线程，无 Caddy 进程；更换根 CA 需要明确操作和客户端重新信任。

## 测试

```powershell
./scripts/test.ps1
./scripts/test-environment.ps1
```

`test.ps1` 执行格式检查、全目标 Clippy 和 Release Cargo 测试。需要已准备好的引擎，以及已登录且可交互的桌面；原生 UI 检查包括窗口、目录选择器、剪贴板、托盘与屏幕重绘，不能把无桌面的 CI 成功当作 UI 验收。测试使用隔离目录和受控 MCP，不应指定个人配置。

环境专项比较用户和系统环境变量前后值，不输出其内容。常规测试跳过明确标记的联网依赖安装。需要实际下载与包安装时，手动执行：

```powershell
./scripts/test-dependencies.ps1 -Proxy 'http://127.0.0.1:10808'
```

代理可省略。该测试会下载运行环境并安装实际测试包，使用 `.build` 中的隔离目录，不运行真实 Blender setup。旧 DPAPI 兼容测试 `test-legacy-compat.ps1` 需要开发机 .NET SDK；测量脚本 `measure.ps1` 需要归档的旧版发布包，均不属于产品运行依赖。

`write-validation.ps1` 是历史测量报告生成器，会重写 `VALIDATION.md`。不要把它作为常规发布步骤；需要重做测量时先保留现有报告，再合并新结果及当前功能验收。

最新验证摘要见 [VALIDATION](VALIDATION.md) 顶部。70 项核心检查来自上一版完整回归；滚轮修复后 76 项 UI 检查通过。UI 计数包含逐网卡检查，会随测试机器网卡数量变化。当前未完成干净 Windows 10/11 虚拟机验收、真实 Blender setup、多显示器人工 DPI 验收或新的内存 / 冷启动测量。

## 打包与 GitHub 发布

```powershell
./scripts/package.ps1
./scripts/test-environment.ps1 -Gui './artifacts/SuperGatewayGUI-win-x64/SuperGatewayGUI.exe'
./scripts/test-portable.ps1
```

按顺序运行；测试中的 EXE 在运行期间不能被 Windows 构建覆盖。打包脚本验证引擎、Release 构建、收集 Rust 和网关许可，生成便携 ZIP、源码 ZIP 和两份 SHA256。源码打包包含图标原始资产、开发文档、脚本和可用的验证日志；纯文档更新可单独执行 `scripts/package-source.ps1`。

GitHub 源码提交包含 `src`、`tests`、`scripts`、`assets`、`.cargo`、Cargo 清单及锁文件、Windows 资源清单、文档和许可。`.build`、`target`、`artifacts`、生成的引擎文件及整个 `legacy` 由 `.gitignore` 排除。旧兼容 / 对照测量脚本需要开发机另行保留旧版；源码 ZIP 打包仅在本地归档存在时附带旧源码。不要上传个人 `settings.dat`、DPAPI 导出记录、TLS 私钥或已安装 MCP 环境；这些是运行数据。

Release 附件使用 `SuperGatewayGUI-win-x64.zip`、`SuperGatewayGUI-source.zip` 及各自 `.sha256`；源码不包含网关 EXE，构建方式已在上文说明。保留便携包中的许可。发布说明可从 [CHANGELOG](CHANGELOG.md) 整理；按实际准备发布的版本设置标签，本文不创建标签或上传仓库。

验证原始记录位于本地 `artifacts` 或源码发布包 `verification`，两者并非必须提交到 Git 的源码文件。发布时以对应 ZIP 的 SHA256 为准，重打包文档也会改变 ZIP 校验值。当前网关的远程 HTTP → stdio 在 stdin EOF 后可能出现 libuv 断言；保持真实退出码和错误，详见 [环境验收限制](ENVIRONMENT_VALIDATION.md#限制)。
