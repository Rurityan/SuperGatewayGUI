# Generate the measured portion from raw samples. Run after measure/package.
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
$measurement = Get-Content "$root/artifacts/measurements.json" -Raw | ConvertFrom-Json
function MiB($value) { '{0:F2}' -f ($value / 1MB) }
$tlsRows = @()
$tlsDetails = 'TLS 专项资源测量尚未完成。'
if (Test-Path "$root/artifacts/tls-measurements.json") {
    $tlsMeasurement = Get-Content "$root/artifacts/tls-measurements.json" -Raw | ConvertFrom-Json
    foreach ($run in $tlsMeasurement.Runs) {
        foreach ($summary in $run.Summary | Where-Object Processes -gt 0) {
            $roleLabel = if ($summary.Role -eq 'Other') { "Other（$($run.OtherProcessNames -join ', ')）" } else { $summary.Role }
            $tlsRows += "| $($run.Services) | $roleLabel | $($summary.Processes) | $(MiB $summary.WorkingSetMedian) / $(MiB $summary.WorkingSetMax) | $(MiB $summary.PrivateMedian) / $(MiB $summary.PrivateMax) |"
        }
    }
    $tlsDetails = "本阶段实测 EXE SHA256：$($tlsMeasurement.GuiSha256)。原始记录 artifacts/tls-measurements.json；与图标前迁移基线分开，不能作为旧 WPF 的同期对照。"
}
$guiRows = @(); $engineRows = @(); $startupRows = @()
foreach ($run in $measurement.Runs) {
    $versionLabel = if ($run.Version -eq 'Rust') {'Rust（图标前基线）'} else {$run.Version}
    $startupRows += "| $versionLabel | $($run.Services) | $([Math]::Round($run.StartupMs)) |"
    foreach ($summary in $run.Summary) {
        $row = "| $versionLabel | $($run.Services) | $($summary.Role) | $($summary.Processes) | $(MiB $summary.WorkingSetMedian) / $(MiB $summary.WorkingSetMax) | $(MiB $summary.PrivateMedian) / $(MiB $summary.PrivateMax) |"
        if ($summary.Role -eq 'GUI') { $guiRows += $row }
        elseif ($summary.Role -in @('Gateway','MCP') -and $run.Services -gt 0) { $engineRows += $row }
    }
}
$old = "$root/legacy/dotnet/artifacts/SuperGatewayGUI-win-x64"
$new = "$root/artifacts/SuperGatewayGUI-win-x64"
$oldBytes = (Get-ChildItem $old -Recurse -File | Measure-Object Length -Sum).Sum
$newBytes = (Get-ChildItem $new -Recurse -File | Measure-Object Length -Sum).Sum
$oldZip = (Get-Item "$root/legacy/dotnet/artifacts/SuperGatewayGUI-win-x64.zip").Length
$newZip = (Get-Item "$root/artifacts/SuperGatewayGUI-win-x64.zip").Length
$text = @'
# Rust Win32 迁移验证

验证日期：2026-10-11。测试机为 Windows 11 家庭中文版 10.0.26300、AMD Ryzen 7 8845HS，Rust 1.96.0 MSVC x64。使用隔离的 `--data-dir`，未读写日常账户的应用配置。

## 功能检查

- 50 项 Rust 核心检查通过。迁移原 42 项检查，并增加备份、Windows batch/shell 启动、SSE/WS 请求截止时间、工具分页等检查。实现位于 `src/bin/checks.rs`；测试 MCP 是自包含 Rust exe。
- 本地 stdio → Streamable HTTP / SSE / WebSocket，以及远程 HTTP / SSE → 三种输出均完成初始化和工具查询。覆盖输出认证拒绝、上游远程认证失败、HTTP 会话、无工具能力、分页、远程服务的 stdio 客户端导出。
- 覆盖 JSONC 导入、重名/端口调整、未知字段拒绝、DPAPI 保存恢复、备份、缺失程序、端口占用、MCP 崩溃、连接超时、日志脱敏、空环境变量、中文/空格/双引号/空参数/尾部反斜线、相对工作目录、单服务隔离及进程树清理。
- 用模拟 `npx.cmd` 验证 Windows batch 启动，并验证粘贴完整 shell 命令；不依赖实际安装 npm 包或访问外部包仓库。
- 迁移时的 12 项原生 UI 冒烟检查通过：表单启用与保存、协议默认路径、密钥编辑框与隐藏、密钥保存、未保存提示与取消、高级页切换、关闭到托盘、托盘恢复及退出。图标更新后扩展至 16 项，包括 EXE 图标提取、6 种 DPI 的资源加载、主窗口大小图标尺寸和配置对话框图标；在空 PATH、无效 DOTNET_ROOT 条件下验证。
- 3 项旧配置兼容检查通过：调用归档的 C# SettingsStore 生成真实 DPAPI 文件，Rust 读取中文名称、认证和秘密值，备份原始密文，损坏配置拒绝且不覆盖。
- 强制终止管理进程后，Job Object 清理网关及 MCP；单独停止服务不会终止其他服务。正常停止先关闭 stdin，8 秒未退出则终止 Job Object。
- 本机 175% DPI 下检查原生窗口布局；中文/空格配置、程序和工作目录均使用宽字符 API。实时日志限制 1000 行/128 KiB，后台日志队列 2048 项，拥塞时丢弃日志而非无限增长。
- `cargo fmt --check`、`cargo clippy --all-targets --locked -- -D warnings` 通过。迁移版本有 50 项核心检查与 12 项 UI 检查；图标版本扩展 UI 检查至 16 项，重绘修复扩展至 28 项；本次 IP 选择更新重新运行两项 Cargo 集成测试，核心检查扩展为 61 项，UI 另增加地址默认值、逐网卡列表与刷新保持选择检查。

中文带空格的便携程序目录在空 PATH 下完成网关/MCP 初始化、正常退出与进程树清理，并重新打开恢复配置后重复验证；见 `artifacts/rust-portable-service-checks.txt`。

复现：`scripts/test.ps1`、`scripts/test-portable.ps1`；可选旧配置生成检查：`scripts/test-legacy-compat.ps1`（仅开发机需要 .NET SDK）。测试输出在 `artifacts/rust-core-checks.txt`、`rust-ui-checks.txt`、`rust-portable-ui-checks.txt`、`rust-legacy-compat-checks.txt`。

## 应用图标更新

通过生图工具生成无文字的网关与连接节点图标。原始 PNG、完整提示词、透明 1024×1024 主图、浅色/深色预览和转换脚本均随源码交付，见 `assets/icon-prompt.md` 与 `scripts/build-icon.ps1`。ICO 包含 16、20、24、32、40、48、64、128、256 像素；预览检查了小尺寸辨识度与透明边缘。

图标作为资源 101 内置于 GUI EXE，便携包不依赖外置图片。主窗口及配置对话框通过 WM_SETICON 设置大小图标，任务栏使用主窗口图标，托盘使用小图标；DPI 变化后重新加载尺寸。自动检查验证 EXE 可提取图标、100/125/150/175/200/250% DPI 资源尺寸、主窗口与对话框图标，以及托盘恢复和退出。未执行跨显示器 DPI 切换的人工验收。

初次图标更新的 GUI 为 661,504 字节，比图标前 629,760 字节增加 31,744 字节；当前版本体积见下表。图标更新后的便携版在中文和带空格路径、空 PATH 下完成两次 MCP 初始化与正常退出；图标更新输出为 `artifacts/icon-portable-ui-checks.txt`，两次服务检查亦已通过。

## 窗口缩放与滚动重绘修复

在图标版上复现滚轮滚动残影：读取屏幕上的面板像素，与强制完整重绘后的像素比较，旧版检查失败。原因是逐个 MoveWindow 立即绘制并复制旧位置像素，最后仅使面板背景失效，未统一重绘子控件。

修复通过带 SWP_NOREDRAW / SWP_NOCOPYBITS 的 SetWindowPos 调整位置及显示状态，控件使用 WS_CLIPSIBLINGS，最后对整个主窗口及子控件统一请求擦除和重绘。未改变服务配置与网关引擎。

重绘修复时界面验收为 28 项：原 16 项全部通过，新增 4 种窗口尺寸下的缩放、分页滚动和滚轮滚动共 12 项屏幕像素检查。面板包括密码框、说明文字、多行输入框和协议相关控件；检查忽略系统主题动画每 RGB 通道不超过 4/255 的微小变化与未使用的 DIB alpha 字节，检测实际残影。见 `artifacts/repaint-ui-checks.txt`。保持原内存与启动结果为迁移基线，没有新增占用测量。

## 网卡 IPv4 地址选择

新服务默认监听 0.0.0.0。基础页提供所有网卡、仅本机 127.0.0.1、已连接网卡的 IPv4 地址，并显示网卡名称；“刷新 IP”保留当前选择，已保存但当前未检测到的 IP 仍显示，启动时提示监听失败。网卡信息使用 Windows GetAdaptersAddresses，不通过主机名解析或外部联网获取。只列出已连接且地址状态为 Preferred 的网卡，优先带网关、较低接口 metric 的地址。

展示及复制的连接地址使用 IP。监听 0.0.0.0 时选择检测到的网卡 IP，没有网卡时回退 127.0.0.1；具体 IP 的健康检查及 MCP 初始化也使用该 IP，所有网卡监听的本机检查仍使用 127.0.0.1。端口检查按实际监听 IP 执行。

配置增加 PascalCase ListenAddress 字段，旧文件缺失该字段时按原 Lan 字段保留监听范围；旧配置的首次读取不覆盖文件。导入支持具体 IPv4，并将 localhost 转为 127.0.0.1，其他主机名拒绝。

61 项核心检查通过，包括原 50 项、默认监听范围、旧 Lan 兼容、IP 连接地址、具体 IP 导入和主机名拒绝，以及真实网卡 IP 的 HTTP/SSE/WebSocket 健康检查、初始化、工具查询及端口占用。当前测试机检测到 2 个网卡地址，因此 UI 共 32 项通过；网卡数量不同会相应增减逐地址检查数。真实 C# DPAPI 兼容检查确认旧配置仍仅监听本机。见 `artifacts/ip-checks.txt`、`artifacts/ip-ui-checks.txt`；便携包在空 PATH、中文与带空格路径下完成两次启动、配置恢复、MCP 验证和退出清理。

## 启动时选择命令解释器

GUI 启动前检测一次，优先 pwsh 7（通过 Windows 版本资源确认主版本 7），然后 Windows PowerShell，最后 cmd.exe；结果和绝对路径通过 OnceLock 固定到进程结束。可从 PATH 及常见安装/系统目录发现解释器。缺少所有解释器不会阻止独立 exe 或远程 MCP 管理，但完整命令会显示可操作的错误。基础页显示本次已选解释器。

仅“完整启动命令”使用此选择；普通程序与参数、Windows batch 的兼容启动方式保留。PowerShell 使用 NoLogo / NoProfile / NonInteractive / EncodedCommand，脚本以 UTF-16LE Base64 传递，输入输出设置 UTF-8；引号包住的程序路径开头自动补调用运算符 &。cmd 回退通过上游原生 shell 模式处理引号，并在网关环境固定 COMSPEC。命令执行错误不会更换解释器或重复启动，安装/修改运行环境后需重新启动 GUI。

70 项核心检查通过，新增三种解释器在中文路径下的中文参数、MCP 输入输出、stderr 日志与工具说明验证，解释器优先级、缺失时回退以及 PATH 修改后选择保持不变；完整命令中以 npx / npm 开头时映射到 .cmd 启动器，避免 .ps1 执行策略限制，并用可控 npx.cmd 验证初始化。回退检查在独立子进程中模拟不同搜索环境，不修改实际安装或用户配置。UI 在本机两张网卡条件下为 33 项，包括已选解释器提示与已有重绘检查；编码单元检查通过。输出见 `artifacts/shell-checks.txt`。便携版继续在中文/空格路径、空 PATH 下完成两次服务初始化与退出清理。网关引擎和用户配置格式未改动，旧内存与启动测量仍仅作为迁移基线。

## Codex TOML 剪贴板导出

新增“复制 Codex TOML”，采用已保存配置，运行中的服务采用 Manager 中实际生效的快照。HTTP 输出生成 Codex 的 url/http_headers；SSE 输出用包内网关转换成 stdio，生成 command/args 和绝对路径；WebSocket 显示切换协议的提示。密钥为网关端的 X-API-Key，支持从额外密钥中选择一个；不会混用上游认证。有密钥时沿用导出前的明文确认交互。

按 OpenAI 官方文档 https://developers.openai.com/codex/mcp/ 核对字段，2026-10-11 通过用户指定的本地代理成功读取；本机 Codex CLI 的 mcp get --json 也识别 HTTP url/http_headers 形状。TOML 表键使用可供 Codex 使用的 ASCII 名称及稳定服务标识，中文名称在注释中保留。

新增 5 项导出检查通过：真实 TOML 解析、中文名称与密钥转义、不混入上游 Token、额外密钥及无密钥处理、Windows 中文/空格路径、WebSocket 提示，以及实际执行带认证 SSE 的 stdio 配置完成 MCP 初始化和工具查询。上述断言归于 5 个 Cargo 测试，见 tests/codex.rs 与 artifacts/codex-toml-checks.txt。toml 是测试依赖，GUI 无新增运行时依赖。UI 在本机两张网卡下扩展为 34 项，新增实际按钮操作和 Unicode 剪贴板内容检查，见 artifacts/codex-ui-checks.txt；原重绘/托盘检查通过。原 70 项核心检查为上一阶段基线，本次仅新增导出代码与按钮。空 PATH 便携启动及退出清理再次验证；未重新测量内存或启动时间。

## 内嵌中文帮助

主窗口增加“帮助”，使用原生章节列表与只读、多行滚动文本，七个章节覆盖基础/高级参数、解释器选择、网卡监听、认证与密钥、启停/托盘、JSON 导入、Codex TOML 示例和故障恢复。内容通过 Rust 编译直接嵌入 EXE，不需要网络或运行时文档文件；不执行示例。帮助窗口沿用内置图标，支持调整大小及键盘导航。

本阶段 cargo fmt 与全目标 Clippy（警告视为错误）通过；UI 冒烟在独立数据目录、空 PATH 下执行，包含帮助打开、章节切换与示例文本、只读状态、图标、缩放布局、正文滚动、关闭恢复主窗口，以及原有表单重绘/托盘检查。输出见 artifacts/help-ui-checks.txt。不修改用户配置目录与登录启动项。未重新测量内存和启动时间，以下数据继续作为图标前迁移基线。

## 管理员重启与管理员登录启动

设置新增“以管理员身份重启”，通过 ShellExecuteEx 的 runas 请求 UAC 授权；取消授权保留当前应用。成功授权后旧应用正常停止受管理网关并退出，新进程等待旧进程退出后获取互斥锁，沿用配置目录。未保存服务修改沿用现有保存/放弃/取消提示。

“开机时以管理员身份启动（登录后）”仅在当前进程的 TokenElevation 表示已提权时可变更，普通权限下禁用；后端再次验证权限。使用当前用户 SID 限定的 InteractiveToken / HighestAvailable 登录计划任务，不保存密码，不以 SYSTEM 运行，与普通 HKCU Run 登录启动互斥。任务创建/删除与配置保存在线程中执行，失败显示错误，保存失败尝试回退系统启动设置。路径和参数使用独立 XML 字段与转义，支持中文/空格和指定数据目录；新增 PascalCase AdminLoginStartup 默认为 false，兼容旧配置。

本阶段格式检查、Clippy 全目标警告检查与完整 Cargo Release 测试通过（包含 70 项网关核心检查、Codex 协议导出与原生 UI 检查）。新增 3 项启动单元检查覆盖任务权限/账户范围、XML 特殊字符、路径参数、旧配置默认值，以及无权限启用/关闭的拒绝。最终便携版 UI 检查覆盖设置窗口、提权状态对应的控件启用状态、默认关闭和取消恢复；本机两张网卡时共 47 项，输出见 artifacts/admin-ui-checks.txt。

自动测试不改变用户实际登录启动项，不弹出真实 UAC 授权；实际 UAC 同意/取消、管理员计划任务注册/删除及重新登录后提权运行尚未人工验收。原内存和启动时间仍是迁移基线，未重新测量。

## 内嵌 TLS 与共享长有效期证书

采用内嵌 rustls / tokio-rustls 字节流 TLS 转发，无 Caddy 或其他 HTTPS 代理进程。每服务保留独立端口，启用 HTTPS / WSS 时外部端口仅接受 TLS，网关改用 127.0.0.1 的内部独立端口。HTTP/1.1、SSE、WebSocket 透明转发，不提供 HTTP/2/HTTP/3、ACME 或复杂路由。

所有服务共用一张含回环及本机活动网卡 IPv4 的证书，默认约 20 年，CA 约 30 年。首次生成后缓存复用，启动及运行中每分钟检查；剩余不足 30 天或 IP 集合变化时补签，新连接热更新，已有 MCP 会话不重启。CA 身份不会静默更换，CA 即将到期时要求重建并重新分发信任。私钥与证书记录以 DPAPI 加密保存，目录 ACL 仅当前用户及 SYSTEM；公开 CA 可导出 DER .cer 或 PEM .pem，不导出私钥。只有用户点击信任并确认后才写入当前用户根证书库，Windows 可能另显示原生证书安全确认；从不忽略证书验证。

Codex HTTP 使用 HTTPS URL；SSE stdio 配置附带 NODE_EXTRA_CA_CERTS 指向公开 CA，移动目录后重新导出。上游网关 JSON 无法表达内嵌 TLS，HTTPS 服务明确阻止该导出以免丢失加密；远程来源 stdio 导出仍连接其上游地址。GUI EXE 从管理员功能版 775,680 字节增加到约 1.8 MiB，实际最终尺寸见后表。TLS 共用一个运行线程，每服务最多 64 连接，握手限时 10 秒；TLS 入口停止时清理该服务所有转发连接，GUI 异常退出后入口随进程消失，网关沿用 Job Object 清理。

完整 Cargo Release 回归通过：70 项核心检查、5 项 Codex 测试、3 项 TLS 单元测试（共享缓存/长有效期/加密与公开导出，接近到期及 IP 变化补签，损坏/缺失私钥不覆盖 CA），以及真实 HTTPS、加密 SSE、WSS 的协议认证、未信任拒绝、禁止明文端口、共享证书、SSE stdio 导出与在线补签保持 WSS 会话/网关 PID 的集成检查。TLS 检查只对独立临时目录的测试 CA 执行短暂当前用户信任，自动确认严格匹配测试进程和随机 CA 名称的 Windows 提示，结束时删除该测试 CA；不改变其他证书或实际用户配置。检查输出 artifacts/tls-checks.txt 与 artifacts/tls-protocol-checks.txt。最终 GUI 空 PATH UI 在本机两张网卡时共 51 项，包括 HTTPS 保存/地址复制及证书管理控件，输出 artifacts/tls-ui-checks.txt。

### 当前 TLS 版本资源测量

__TLS_DETAILS__

同机 0、1、5 个服务，GUI 收起托盘；1/5 场景保持经过初始化与工具查询的 WSS 会话。稳定 30 秒后每秒采样，共 30 次，按角色求和，分别记录工作集与私有提交中位数/最大值。测试 CA 仅测量期间临时信任，结束移除。其他代理进程数为零，不计入外部测量/会话保持程序。0 服务场景没有激活 TLS 运行线程。这里记录保持会话时的内存，不代表高并发吞吐或 CPU 压测。

Other 单独记录由网关启动控制台 MCP 时产生的 Windows conhost.exe，不属于 TLS 代理；进程角色检查见 artifacts/tls-process-roles.txt。源码包 verification 目录附本阶段检查输出和原始 TLS 采样。

| 服务数 | 角色 | 进程数 | 工作集 中位数 / 最大（MiB） | 私有提交 中位数 / 最大（MiB） |
|---|---|---:|---:|---:|
__TLS_ROWS__

未实际验收其他设备/手机对长期私有证书的兼容性、真实网卡动态插拔、跨账户 DPAPI 使用、超过 20 年的真实运行或高并发负载。续签路径通过缩短加密记录中的期限与改变 IP 集合模拟，不等待证书真实经过 20 年；真实 HTTP/SSE/WSS 验证使用最终签发的长有效期证书。原干净系统验收及真实管理员启动验收仍保留为未完成。

## 占用对照方法

旧 WPF 和 Rust 在同一机器、相同配置下分别运行 0、1、5 个独立服务。两个 GUI 均收起到托盘。服务使用同一个 Rust MCP fixture，保持 WebSocket MCP 会话，完成初始化和工具查询。每个场景先稳定 30 秒，再每隔约 1 秒采样，共 30 次。没有把测量程序计入应用占用。

WorkingSet64 是工作集，PrivateMemorySize64 是私有提交内存；二者含义不同，不应相加。多进程角色按每个采样点先求和，再求中位数和最大值。网关/MCP 行为该角色的总和，并列出进程数。单位统一为 MiB（1024² 字节）。

脚本为 `scripts/measure.ps1`，原始数据为 `artifacts/measurements.json`，保留每次采样、进程数、方法及被测 GUI SHA256。以下内存和启动结果是添加图标前的 Rust 迁移基线；对应 EXE 保留于 `artifacts/baselines/rust-before-icon`。本次图标更新只重新实测体积和验证界面，不将旧内存或启动结果视作图标版本的新测量。

### GUI 内存

| 版本 | 服务数 | 角色 | 进程数 | 工作集 中位数 / 最大 | 私有提交 中位数 / 最大 |
|---|---:|---|---:|---:|---:|
__GUI_ROWS__

### 网关与第三方 MCP 内存

| 版本 | 服务数 | 角色 | 进程数 | 工作集 中位数 / 最大 | 私有提交 中位数 / 最大 |
|---|---:|---|---:|---:|---:|
__ENGINE_ROWS__

第三方 MCP 行仅代表本次轻量 Rust 测试服务，真实 MCP 的占用取决于具体服务。网关仍内含 Node SEA，并且每服务独立运行，多个服务时的主要内存成本仍在网关进程。

### 硬盘与启动

| 项目 | 旧 WPF | Rust（当前版本） |
|---|---:|---:|
| GUI EXE | __OLD_EXE__ MiB | __NEW_EXE__ MiB |
| 网关 EXE | 92.09 MiB | 92.09 MiB |
| 解压便携包 | __OLD_DIR__ MiB | __NEW_DIR__ MiB |
| ZIP | __OLD_ZIP__ MiB | __NEW_ZIP__ MiB |

目录/ZIP 取一位小数，EXE 取两位小数。包中说明文档更新会导致细小体积变化；实际文件大小以交付文件为准。

| 版本 | 服务数 | 进程启动到主窗口建立（毫秒） |
|---|---:|---:|
__STARTUP_ROWS__

启动计时从启动新 GUI 进程到可枚举主窗口为止，不含自动服务就绪时间。每个场景记录一次，非 30 次中位数。应用进程此前未运行，但未清空操作系统文件缓存；旧 WPF 单文件运行时的提取缓存可能已经存在。因此这是进程冷启动对照，不能当作重启系统后首次解压运行的启动时间。

## 未完成的环境验收

未实际运行未安装 .NET、Node、Python 的干净 Windows 10/11 虚拟机验收，也未执行系统重启后清空文件缓存的冷启动测量。空 PATH 测试证明独立 GUI/网关/MCP 不需要 PATH 中的这些命令，不能替代干净系统验收。

多显示器 DPI 来回切换、读屏软件、各种真实第三方 MCP、真实远程 TLS/代理环境及不同权限的防火墙/登录启动仍需更广泛人工验收。Windows 登录启动的 UI 和注册表实现已保留，但本次测试不修改用户实际登录启动注册表。

## 交付

根目录为 Rust 源码、Cargo.lock、构建/测试/打包脚本和中文说明；便携包仅包含 Rust GUI、固定网关、文档及必要许可。旧 WPF 源码和发布包保存在 `legacy/dotnet`，不参与新构建。网关 SHA256 与构建提交见 `engine/manifest.json`；ZIP 附带 SHA256 文件。
'@
$text = $text.Replace('__GUI_ROWS__',($guiRows -join "`n")).Replace('__ENGINE_ROWS__',($engineRows -join "`n")).Replace('__STARTUP_ROWS__',($startupRows -join "`n"))
$text = $text.Replace('__TLS_ROWS__',($tlsRows -join "`n")).Replace('__TLS_DETAILS__',$tlsDetails)
$text = $text.Replace('__OLD_EXE__',(MiB (Get-Item "$old/SuperGatewayGUI.exe").Length)).Replace('__NEW_EXE__',(MiB (Get-Item "$new/SuperGatewayGUI.exe").Length))
$text = $text.Replace('__OLD_DIR__',('{0:F1}' -f ($oldBytes/1MB))).Replace('__NEW_DIR__',('{0:F1}' -f ($newBytes/1MB))).Replace('__OLD_ZIP__',('{0:F1}' -f ($oldZip/1MB))).Replace('__NEW_ZIP__',('{0:F1}' -f ($newZip/1MB)))
[IO.File]::WriteAllText("$root/VALIDATION.md",$text,[Text.UTF8Encoding]::new($false))
if (Test-Path "$root/DEPENDENCIES_VALIDATION.md") {
    [IO.File]::AppendAllText("$root/VALIDATION.md", "`n`n" + [IO.File]::ReadAllText("$root/DEPENDENCIES_VALIDATION.md"), [Text.UTF8Encoding]::new($false))
}
if (Test-Path "$root/STORAGE_VALIDATION.md") {
    [IO.File]::AppendAllText("$root/VALIDATION.md", "`n`n" + [IO.File]::ReadAllText("$root/STORAGE_VALIDATION.md"), [Text.UTF8Encoding]::new($false))
}
if (Test-Path "$root/ONESHOT_VALIDATION.md") {
    [IO.File]::AppendAllText("$root/VALIDATION.md", "`n`n" + [IO.File]::ReadAllText("$root/ONESHOT_VALIDATION.md"), [Text.UTF8Encoding]::new($false))
}
