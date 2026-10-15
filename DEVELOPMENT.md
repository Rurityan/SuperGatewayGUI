# 开发说明

2026-10-15 AI 安装向导：`src/ai.rs` 实现带代理 / 取消 / 总期限 / 响应容量限制的 WinHTTP 请求、固定 GitHub 提交的资料读取、受限 Proposal 类型、包源核实和纯草稿映射；该模块不创建进程。`src/ui_ai.rs` 提供资料 / 结果 / 差异勾选三阶段原生窗口，UI 线程通过定时器接收工作线程结果，空闲不重绘。设置中可保存 Ai.BaseUrl / ApiKey / Model，复用 settings.dat DPAPI，旧文件缺失字段默认空配置；AI 设置不进入服务交换。服务草稿只在确认采用后更新，完整命令保留，setup 不写常驻参数；变量实际值只保留本机，不加入请求。模拟 API / 仓库 / 包源测试见 `tests/ai.rs`、库内 AI 测试及 `desktop-checks ai-ui`。首版无真实供应商、私有仓库或自主执行功能。

便携启动验收使用 `desktop-checks prepare-portable` 从空闲端口生成受控服务，避免与用户现有 18000 服务冲突；原 `prepare` / `prepare-tls` 仍用于历史性能测量，不改变其端口规则。

2026-10-15 参数编辑：`src/ui_arguments.rs` 提供原生 Win32 逐项编辑器，每个参数独立 EDIT，行间使用 SS_ETCHEDHORZ，支持添加 / 删除 / 排序、离散行滚动、缩放与 DPI 字体刷新。主服务和托管包表单只读摘要不参与解析，直接保存 Vec<String>，避免空参数和引号被重新拆解；取消返回 None。一次性向导复用编辑器，确认只进入原有预览流程，不写服务启动参数。配置格式不变。

环境变量隐藏偏好：Pair 增加 `Hidden`（缺失默认 false），经 settings.dat DPAPI 与完整服务配置交换保存。环境变量编辑框保存该项隐藏选择，主服务保存后持久化；列表总开关仅额外遮盖，不能覆盖单项 Hidden。`gateway_eq` 忽略隐藏元数据，保存偏好不标记服务待重启。环境合并、认证、日志及客户端导出仍使用实际 Value，隐藏不代表交换文件加密。

2026-10-14 界面更新：新环境变量值默认显示，主列表提供额外隐藏，编辑框按每项保存“隐藏内容”；请求头与密钥仍默认隐藏。验证记录见 VALIDATION 的环境变量显示更新及持久化章节。

同步日期：2026-10-13。本文件描述当前 Rust Win32 实现，Cargo 版本 `2.0.0`。产品使用说明见 [README](README.md)。历史 .NET 实现仅在开发机 `legacy/dotnet` 本地归档，整个 `legacy` 不提交到 GitHub，也不参与 Cargo 构建。克隆仓库不包含旧版，常规构建和打包不要求该目录存在。

## 开发环境

开发及自动 UI 验收使用 Windows x64，Rust MSVC 工具链、Visual Studio C++ Build Tools 和 Windows SDK。Rust 使用 edition 2024；已有验收工具链为 Rust 1.96.0。PowerShell 脚本建议在 PowerShell 7 下运行。资源编译需要 SDK 的 `rc.exe`，重建网关还需要 Git、Node.js 24、npm 和 `signtool.exe`。

`.cargo/config.toml` 固定 `x86_64-pc-windows-msvc` 并启用静态 CRT。Release 使用体积优化、LTO、单代码生成单元和符号剥离。提交 `Cargo.lock`，构建及测试使用 `--locked`；应用本身不需要 Rust、Node、Python、.NET 或 GUI 运行时。

## 首次准备与构建

### 界面语言

`src/i18n.rs` 管理可选 `Settings.Language`（`system` / `zh-CN` / `en`）、Windows `GetUserDefaultUILanguage` 选择及文本匹配。旧配置默认 system，中文显示语言映射为简体中文，其余使用英文。`assets/locales/en.json` 是中文原文与英文的配对目录，通过 include_str 内置，不引入 GUI 或翻译运行时。新增翻译需保留格式占位符数量和顺序；名称、路径、程序、参数、变量与密钥不是翻译源，不可对输入值做全局替换。

主窗口仅重新标注应用控件、下拉项和标签页，保留 EDIT 内容、服务状态和进程；后台 SettingsSaved 事件后应用语言。保存纯语言变更不更新登录任务；即使配置中已开启管理员登录启动，普通权限仍能保存语言。提权校验只针对管理员启动选项的实际变更，未变更启动设置时，保存失败也不调用启动任务回退。`src/help.rs` 按语言提供同样八章帮助；AI 请求只增加 response_language，不发送其他隐私数据，现有模型输出保留原文。Win32 系统按钮和第三方错误遵循各自语言。

语言验证：`cargo test --release --locked --test language` 检查 DPAPI / 配置恢复 / 服务交换隔离；i18n 库测试覆盖系统语种映射、旧配置默认、目录重复 / 占位符和动态用户值保留。`desktop-checks language-ui <隔离配置目录> <GUI.exe> <mock-mcp.exe>` 验证两个真实测试 MCP 的即时切换、系统选择、中文路径、英文帮助 / AI 入口及重启恢复，已接入原生 UI 回归。不得对用户现有 GUI 或服务运行该隔离流程。

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
| `src/main.rs`、`ui.rs`、`ui_dependencies.rs`、`help.rs` | 启动入口、原生控件、依赖管理界面和内嵌中英文帮助 |
| `src/model.rs`、`store.rs`、`storage.rs` | PascalCase 配置、DPAPI、原子保存、备份和配置 / 依赖路径 |
| `src/config.rs`、`shell.rs`、`environment.rs` | 配置校验与生成、启动解释器、每服务环境合并和 PATH |
| `src/client.rs` | 客户端导出选项校验、稳定 Codex 表名、超时映射与网络 JSON 导出 |
| `src/exchange.rs` | Version 1 完整 GUI 服务配置交换、字段检查、独立错误预览、重名 / 端口处理和新 ID |
| `src/logs.rs` | 按服务 ID 保留日志、筛选和按范围清空，共享容量限制 |
| `src/manager.rs`、`native.rs` | 独立服务状态、工作线程、Win32 进程 / 管道 / Job Object |
| `src/dependencies.rs`、`oneshot.rs` | 固定环境与包安装、一次性终端向导 |
| `src/net.rs`、`network.rs`、`tls.rs`、`startup.rs` | MCP 协议诊断、网卡发现、内嵌 TLS、普通 / 管理员登录启动 |
| `src/bin`、`tests` | 自包含 MCP 模拟程序、核心 / UI 检查和专项集成测试 |
| `assets`、`build.rs`、`app.manifest` | 图标源图及提示词、许可、Windows 资源和 DPI 清单 |

GUI 通过工作线程执行进程与网络操作，用消息和事件更新界面。每个服务独立网关和端口。GUI 创建进程时先挂起，加入 Job Object 再恢复；停止先关闭 stdin，8 秒后仍未退出则清理进程树。一次性终端可以在 GUI 退出后继续由用户操作。

完整服务交换文件为 `{ "Format": "SuperGatewayGUI", "Version": 1, "Services": [...] }`，Services 使用 Profile 的 PascalCase 持久化字段，跳过运行期字段。独立版本校验及嵌套字段检查避免新配置被静默丢失；不改变 settings.dat 的兼容读取。导入逐项校验、分配新 ID，清空依赖 Installed 引用、保留固定包版本及启动偏好；端口 / 名称冲突在勾选前预览。勾选结果一次持久化，写入失败恢复内存中的原配置；不调用安装或启动。原有 mcpServers 导入共用勾选界面。

导出文件为可迁移明文 JSON，与本机 DPAPI 存储不同，UI 对已知敏感值额外确认；预览不展示值，字段类型错误不回显原始值。服务文件不包含全局配置、依赖文件或 TLS 私钥。专项检查：`desktop-checks.exe exchange-ui <隔离绝对数据目录> <GUI绝对路径>`，自动验证原生勾选、中文路径文件保存、密钥确认、取消、恢复及无执行副作用；命令需要 Windows 交互桌面，仅使用受控配置。

GUI 实例由当前用户 SID 和规范化配置目录共同识别，显式 / 默认指定同一配置目录使用同一实例。再次前台启动通过注册的窗口消息恢复已有实例，后台启动不弹窗；寻找窗口先于打开互斥锁，以兼容普通权限打开同用户的管理员实例。UIPI 仅放行窗口恢复、托盘回调及 `TaskbarCreated` 消息，恢复请求不接受配置或进程参数。资源管理器重新创建任务栏后重新注册图标，图标不可用则显示窗口；登录任务仍使用交互式令牌，不能运行在非交互桌面。

一次性终端打开自己的 CONIN$ / CONOUT$ 后，创建交互子进程时复制并显式传递可继承的三项标准句柄。仅让子进程共享控制台不足以支持 uv 的 Python 包入口；否则启动器再启动解释器时可能丢失输出。受控 Python 终端检查必须读取实际终端缓冲区验证 stdout、stderr 和输入提示，而不能只验证程序写出的文件。

## 修改时需要保持的行为

服务保存和导入只校验、写配置，不下载或执行命令；有效托管包配置在未安装、程序路径为空时可以保存。缺少环境在启动时提示安装，不静默回退系统解释器。完整命令非空仍优先，解释器在应用启动时按 pwsh 7、Windows PowerShell、cmd 顺序确定，不因命令报错切换。

GUI 启动环境快照只读。变量名不区分大小写，服务变量覆盖继承值，空值保持；不写用户 / 系统环境变量。托管 PATH 顺序为服务虚拟环境、托管工具、服务附加目录、Windows 必要目录、继承 PATH。关闭继承省略最后一项；原有程序模式仍允许显式 PATH 覆盖，附加目录排在前面。正常服务、完整命令和一次性向导采用同一环境规则，安装任务有单独受控环境。

保留旧配置默认值、PascalCase、DPAPI 和读取失败不覆盖机制。配置目录保存 `settings.dat`、证书、备份与 `exports`；依赖目录可独立设置，路径记录在 `storage.json`。移动配置复制到空目录并保留原件，依赖不迁移。加密记录绑定当前 Windows 用户；旧 exports 记录保留，但不再提供 stdio 启动器和输出配置。

Client 仅用于客户端连接配置，StartupTimeoutSec / ToolTimeoutSec 只导出 Codex TOML。Environment / Stdio 旧字段保留反序列化兼容，不用于网关运行或网络导出；GUI 保存时 Stdio 为 false。客户端参数不参与 gateway_eq，不产生待应用或改变 MCP 环境。已删除 stdio 导出模块、启动入口及 SSE 转 stdio 桥接导出；Codex TOML 仅 HTTP，JSON 支持原生 HTTP / SSE。
显式“保存配置”按钮在持久化成功后显示确认弹窗，并提示待应用或待安装；失败显示具体错误。启动前及未保存确认流程继续复用 `save()`，不额外显示成功弹窗。保存反馈更新后原生 UI 检查为 93 项，核心与客户端测试保留此前回归结果。

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

最新验证摘要见 [VALIDATION](VALIDATION.md) 顶部。之前包含独立 stdio 的检查结果是历史基线；删除该输出后同步删除对应测试，保留本地 stdio 输入、远程网络转接、协议、TLS、环境隔离及配置恢复检查。原生 UI 增加字段标签实际字体尺寸检查。真实管理员计划任务重新登录仍需人工验收。UI 计数包含逐网卡检查，会随测试机器网卡数量变化。当前未完成干净 Windows 10/11 虚拟机验收、真实 Blender setup、多显示器人工 DPI 验收或新的内存 / 冷启动测量。

## 打包与 GitHub 发布

```powershell
./scripts/package.ps1
./scripts/test-environment.ps1 -Gui './artifacts/SuperGatewayGUI-win-x64/SuperGatewayGUI.exe'
./scripts/test-portable.ps1
```

按顺序运行；测试中的 EXE 在运行期间不能被 Windows 构建覆盖。打包脚本验证引擎、Release 构建、收集 Rust 和网关许可，生成便携 ZIP、源码 ZIP 和两份 SHA256。源码打包包含图标原始资产、开发文档、脚本和可用的验证日志；纯文档更新可单独执行 `scripts/package-source.ps1`。

GitHub 源码提交包含 `src`、`tests`、`scripts`、`assets`、`.cargo`、Cargo 清单及锁文件、Windows 资源清单、文档和许可。`.build`、`target`、`artifacts`、生成的引擎文件及整个 `legacy` 由 `.gitignore` 排除。旧兼容 / 对照测量脚本需要开发机另行保留旧版；源码 ZIP 打包仅在本地归档存在时附带旧源码。不要上传个人 `settings.dat`、DPAPI 导出记录、TLS 私钥或已安装 MCP 环境；这些是运行数据。

Release 附件使用 `SuperGatewayGUI-win-x64.zip`、`SuperGatewayGUI-source.zip` 及各自 `.sha256`；源码不包含网关 EXE，构建方式已在上文说明。保留便携包中的许可。发布说明可从 [CHANGELOG](CHANGELOG.md) 整理；按实际准备发布的版本设置标签，本文不创建标签或上传仓库。

验证原始记录位于本地 `artifacts` 或源码发布包 `verification`，两者并非必须提交到 Git 的源码文件。发布时以对应 ZIP 的 SHA256 为准，重打包文档也会改变 ZIP 校验值。上游远程 HTTP → stdio 的历史退出断言记录见 [环境验收限制](ENVIRONMENT_VALIDATION.md#限制)；当前应用不再提供该输出。
