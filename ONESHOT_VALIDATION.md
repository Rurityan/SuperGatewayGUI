# 一次性 MCP 安装向导验收（2026-10-12）

本地服务“管理此服务依赖”新增“一次性命令 / 安装向导”入口，支持在终端手动执行已安装包的 setup、install-addon、登录或其他命令参数。

## 操作示例

`uvx mcp-for-blender setup` 的 setup 是第三方交互初始化向导，不是常驻 MCP 服务。上游文档和 PyPI 2.1.9 wheel 的 entry_points 确认此包提供 `mcp-for-blender` 入口，setup 会选择配置其他 MCP 客户端及安装 / 启用 Blender 插件。来源：https://pypi.org/project/mcp-for-blender/ 、https://github.com/ahujasid/blender-mcp 。

1. 选择一个本地服务，进入“管理此服务依赖”，类型选择 Python 包，名称填 `mcp-for-blender`，入口留空自动识别。
2. 手动安装 uv、Python 和服务包。服务包仍按现有逻辑安装并固定实际版本；未安装时一次性入口报错，不自动下载。
3. 点击“一次性命令 / 安装向导”，临时参数填一行 `setup`。不填写 uvx 和包名；多参数每行一项。
4. 预览实际的已安装入口、参数和工作目录，确认后在新终端按上游提示操作。命令结束显示退出码，按 Enter 关闭；随时关闭终端结束该命令及其后代进程。
5. MCP 服务正常启动时参数不填 setup。一次性参数不保存为服务启动参数；只安装 Blender 插件时可选择临时参数 `install-addon`。

向导可能下载插件、修改 Blender 或其他客户端配置，选择由用户在终端完成。本应用不替第三方改写其设置行为；向导生成的客户端配置可能仍使用 uvx 自行管理包。客户端希望通过 GUI 网关连接时，应使用本应用的客户端配置导出。

## 实现范围

- 直接调用当前提交安装的 npm / Python 包入口，参数经 Windows 程序参数引用传入，无 shell 拆解或临时 uvx / npx 下载。原始完整命令和常驻启动参数保持原值。
- 预览后用户明确确认才打开终端。使用当前依赖管理界面的代理 / 包源和服务环境变量，只对子进程注入，不写系统或用户 PATH。初始化程序允许联网；Python 仍指定已有托管解释器、禁止隐式下载。
- 终端是同一内置 EXE 的单次运行模式，不引入 PowerShell 下载脚本、外部终端运行库或额外 EXE。原有启动时冻结的 pwsh / PowerShell / cmd 选择保持不变，结构化入口不需要解释器。
- 环境和入口通过临时 DPAPI 加密请求交给终端，不将凭据放在终端启动命令行；读取后清理请求文件。预览中对已知服务密钥脱敏。
- 终端绑定自己的 console 输入输出，可读取交互输入。命令进程挂起创建，先加入 Job Object 再恢复；终端退出后清理进程树。
- 终端独立于 GUI 主进程，GUI 退出不替用户中断向导。当前 GUI 会记录未关闭终端的包 / 环境引用以阻止删除及缓存清理；完成后请关闭终端。
- 不提供任意完整 shell 表达式执行，也不自动将导入的 setup 命令运行一次。已安装的第三方程序自身可能执行其他子命令或更新客户端配置，这属于用户确认的向导行为。

## 已完成检查

- **3 项专项 Rust 测试**：临时参数准确（中文、空格、空参数、引号和 shell 特殊字符）且原服务不变；未安装 / 规格修改 / 远程服务拒绝执行；代理和环境仅注入子进程，工作目录检查及父进程 PATH 保持原值。
- **6 项真实交互终端检查**：空 PATH 启动固定 Node 包入口；准确参数、隔离缓存、环境值及加密请求清理；控制台键盘输入被测试 setup 接收；完成后 Job Object 清理后代；命令完成终端等 Enter 再退出；异常终止终端后命令与后代进程清理。
- 实际已安装 npm / Python MCP 的 GUI 检查 **13 项通过**，包含一次性入口启用、实际入口与 setup 临时参数预览、取消不改变服务参数，以及原有安装确认 / 更新 / 待应用 / 单服务重启检查。
- 格式、全目标 Clippy 与 Release 回归覆盖原有核心协议、TLS、配置恢复、托盘、存放路径和依赖管理。原始记录见源码包 verification 中 oneshot-*.txt。

终端测试只运行工作区隔离目录中的可控 Node 测试程序，不执行会改动用户个人客户端配置或 Blender 安装的真实 setup 向导。没有安装或更改用户个人 Blender 插件；**真实 Blender 全流程人工验收未完成**。干净 Windows 10 / 11 验收及本版内存 / 冷启动测量亦未完成，历史测量仅为基线。

复现（需先在隔离数据目录显式安装托管 Node）：

```powershell
./scripts/test.ps1
./target/x86_64-pc-windows-msvc/release/desktop-checks.exe oneshot-console `
  '.build/一次性终端 隔离验收' `
  './artifacts/SuperGatewayGUI-win-x64/SuperGatewayGUI.exe' `
  'D:/绝对路径/隔离测试依赖目录'
```

第三个路径是依赖根目录（其下已有 runtimes/node），不是配置目录。测试生成可控包并在完成后删除它，不下载包或改动实际客户端。

最终便携 EXE 再次通过 6 项交互终端检查与 13 项真实依赖 GUI 检查。GUI 为 **2,246,656 字节（2.14 MiB）**，SHA256 为 `f12e31ea994de66dd880f8b60c9ddc42705947e5ed2a8aa83bd2a596c50cb413`。ZIP 体积 / SHA256 见 artifacts/dependency-delivery.json 和 ZIP.sha256。此前 DEPENDENCIES_VALIDATION.md / STORAGE_VALIDATION.md 所列 EXE 校验值保留为对应功能交付基线。
