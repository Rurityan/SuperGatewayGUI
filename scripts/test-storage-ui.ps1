param([string]$Executable = '', [string]$DataDirectory = '')
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
if (!$Executable) { $Executable = Join-Path $root 'target/x86_64-pc-windows-msvc/release/SuperGatewayGUI.exe' }
if (!$DataDirectory) { $DataDirectory = Join-Path $root ('.build/目录 UI 验收 ' + [guid]::NewGuid().ToString('N')) }
$Executable = [IO.Path]::GetFullPath($Executable)
$DataDirectory = [IO.Path]::GetFullPath($DataDirectory)
if (!$DataDirectory.StartsWith([IO.Path]::GetFullPath((Join-Path $root '.build')) + '\', [StringComparison]::OrdinalIgnoreCase)) { throw 'Only isolated workspace .build directories are allowed' }
if (Test-Path $DataDirectory) { throw 'Use a fresh test directory' }
Add-Type @'
using System;
using System.Text;
using System.Runtime.InteropServices;
public static class StorageUi {
 public delegate bool EnumProc(IntPtr h, IntPtr p);
 [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc cb, IntPtr p);
 [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
 [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
 [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
 [DllImport("user32.dll")] public static extern bool IsWindow(IntPtr h);
 [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr h, int id);
 [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr h, uint m, IntPtr w, IntPtr l);
 [DllImport("user32.dll")] public static extern IntPtr SendMessageW(IntPtr h, uint m, IntPtr w, IntPtr l);
 [DllImport("user32.dll", CharSet=CharSet.Unicode, EntryPoint="SendMessageW")] public static extern IntPtr Text(IntPtr h, uint m, IntPtr w, string s);
 [DllImport("user32.dll", CharSet=CharSet.Unicode, EntryPoint="SendMessageW")] static extern IntPtr Read(IntPtr h, uint m, IntPtr w, StringBuilder s);
 public static string Value(IntPtr h) { var s=new StringBuilder(32768); Read(h,13,(IntPtr)s.Capacity,s);return s.ToString(); }
 public static string ListItem(IntPtr h, int index) { var s=new StringBuilder(32768); Read(h,393,(IntPtr)index,s);return s.ToString(); }
 public static IntPtr Find(uint process, string title) { IntPtr result=IntPtr.Zero; EnumWindows((h,p)=>{uint id;GetWindowThreadProcessId(h,out id);if(id!=process)return true;var s=new StringBuilder(1024);GetWindowTextW(h,s,s.Capacity);if(s.ToString()==title && IsWindowVisible(h)){result=h;return false;}return true;},IntPtr.Zero);return result; }
}
'@
function Wait-Value([scriptblock]$Action) {
    $deadline = [DateTime]::UtcNow.AddSeconds(30)
    do { $value = & $Action; if ($value) { return $value }; Start-Sleep -Milliseconds 80 } while ([DateTime]::UtcNow -lt $deadline)
    throw 'Timed out waiting for isolated UI'
}
function Window([int]$ProcessId, [string]$Title) { Wait-Value { $h = [StorageUi]::Find($ProcessId,$Title); if ($h -ne [IntPtr]::Zero) { $h } } }
function Action([IntPtr]$Window, [int]$Id) { [void][StorageUi]::PostMessageW($Window,273,[IntPtr]$Id,[IntPtr]::Zero) }
function Check([bool]$Ok, [string]$Message) { if (!$Ok) { throw $Message }; "PASS $Message" }
function Launch([string]$Configuration) {
    $info = [Diagnostics.ProcessStartInfo]::new($Executable)
    $info.UseShellExecute = $false
    $info.ArgumentList.Add('--data-dir'); $info.ArgumentList.Add($Configuration)
    $info.Environment['PATH'] = ''
    [Diagnostics.Process]::Start($info)
}
$old = Join-Path $DataDirectory '原配置'
$new = Join-Path $DataDirectory '新配置 空格'
$packages = Join-Path $DataDirectory '独立环境与依赖 空格'
$tracked = @()
try {
    $process = Launch $old
    $tracked += $process.Id
    $main = Window $process.Id 'SuperGateway · Rust Win32 MCP 网关管理'
    Action $main 10
    Start-Sleep -Milliseconds 150
    $panel = [StorageUi]::GetDlgItem($main,22)
    [void][StorageUi]::Text([StorageUi]::GetDlgItem($panel,100),12,[IntPtr]::Zero,'目录切换服务')
    [void][StorageUi]::SendMessageW([StorageUi]::GetDlgItem($panel,101),334,[IntPtr]1,[IntPtr]::Zero)
    [void][StorageUi]::PostMessageW($main,273,[IntPtr](101 -bor (1 -shl 16)),[IntPtr]::Zero)
    [void][StorageUi]::Text([StorageUi]::GetDlgItem($panel,106),12,[IntPtr]::Zero,'https://example.test/mcp')
    Action $main 30
    [void](Wait-Value { Test-Path "$old/settings.dat" })
    $originalHash = (Get-FileHash "$old/settings.dat").Hash
    Action $main 16
    $settings = Window $process.Id '应用设置'
    Check ([StorageUi]::GetDlgItem($settings,516) -ne [IntPtr]::Zero) 'settings exposes independent storage locations'
    Action $settings 516
    $paths = Window $process.Id '配置与依赖存放路径'
    Check ([StorageUi]::Value([StorageUi]::GetDlgItem($paths,550)) -eq $old) 'path editor shows current configuration directory'
    Check ([StorageUi]::Value([StorageUi]::GetDlgItem($paths,551)) -eq "$old\dependencies") 'path editor shows default dependency directory'
    [void][StorageUi]::Text([StorageUi]::GetDlgItem($paths,550),12,[IntPtr]::Zero,'relative')
    Action $paths 1
    $warning = Window $process.Id '检查目录'
    Action $warning 1
    Check ([StorageUi]::IsWindow($paths)) 'relative path rejected without closing editor'
    Action $paths 2
    [void](Wait-Value { ![StorageUi]::IsWindow($paths) })
    Check (!(Test-Path "$old/storage.json") -and (Get-FileHash "$old/settings.dat").Hash -eq $originalHash) 'cancel preserves configuration and storage settings'
    Action $main 16
    $settings = Window $process.Id '应用设置'
    Action $settings 516
    $paths = Window $process.Id '配置与依赖存放路径'
    [void][StorageUi]::Text([StorageUi]::GetDlgItem($paths,550),12,[IntPtr]::Zero,$new)
    [void][StorageUi]::Text([StorageUi]::GetDlgItem($paths,551),12,[IntPtr]::Zero,$packages)
    Action $paths 1
    $confirmation = Window $process.Id 'SuperGateway'
    Action $confirmation 6
    $replacement = Wait-Value { Get-CimInstance Win32_Process -Filter "ParentProcessId=$($process.Id) AND Name='SuperGatewayGUI.exe'" | Select-Object -First 1 }
    $tracked += [int]$replacement.ProcessId
    $reopened = Window $replacement.ProcessId 'SuperGateway · Rust Win32 MCP 网关管理'
    [void](Wait-Value { $process.Refresh(); $process.HasExited })
    Check (Test-Path "$new/settings.dat") 'save restarts GUI with new configuration directory'
    Check ([StorageUi]::ListItem([StorageUi]::GetDlgItem($reopened,20),0).Contains('目录切换服务')) 'existing encrypted service configuration survives restart'
    Check (Test-Path "$old/settings.dat") 'original encrypted configuration retained'
    Action $reopened 16
    $settings = Window $replacement.ProcessId '应用设置'
    Action $settings 516
    $paths = Window $replacement.ProcessId '配置与依赖存放路径'
    Check ([StorageUi]::Value([StorageUi]::GetDlgItem($paths,550)).EndsWith($new)) 'configuration selection restores after restart'
    Check ([StorageUi]::Value([StorageUi]::GetDlgItem($paths,551)).EndsWith($packages)) 'independent dependency selection restores after restart'
    Check ((Get-ChildItem -LiteralPath $packages -Force).Count -eq 0) 'directory change does not install or migrate dependencies'
    Action $paths 2
    [void](Wait-Value { ![StorageUi]::IsWindow($paths) })
    Action $reopened 17
    [void](Wait-Value { !(Get-Process -Id $replacement.ProcessId -ErrorAction SilentlyContinue) })
    'PASS restarted GUI exits normally'
} finally {
    foreach ($processId in $tracked) {
        $remaining = Get-Process -Id $processId -ErrorAction SilentlyContinue
        if ($remaining -and $remaining.Path -eq $Executable) { Stop-Process -Id $processId -Force }
    }
}
