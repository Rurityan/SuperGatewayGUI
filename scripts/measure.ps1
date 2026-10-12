param([string]$Output = 'artifacts/measurements.json', [string[]]$Versions = @('WPF','Rust'), [switch]$TLS)
$ErrorActionPreference = 'Stop'
if ($TLS -and $Output -eq 'artifacts/measurements.json') { $Output = 'artifacts/tls-measurements.json' }
if ($TLS -and ($Versions.Count -ne 1 -or $Versions[0] -ne 'Rust')) { throw 'TLS measurement supports Rust only; use -Versions Rust -TLS' }
$root = Split-Path $PSScriptRoot -Parent
Push-Location $root
try {
    Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class BenchWindow {
 public delegate bool Callback(IntPtr h, IntPtr l);
 [DllImport("user32.dll")] static extern bool EnumWindows(Callback c, IntPtr l);
 [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
 [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern int GetWindowTextLength(IntPtr h);
 [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint msg, UIntPtr w, IntPtr l);
 public static IntPtr Find(int target) { IntPtr found=IntPtr.Zero; EnumWindows((h,l)=>{uint pid;GetWindowThreadProcessId(h,out pid);if(pid==target&&GetWindowTextLength(h)>0){found=h;return false;}return true;},IntPtr.Zero);return found; }
}
'@
    $bin = "$root/target/x86_64-pc-windows-msvc/release"
    $fixture = "$root/.build/性能 测试/fixture/mock-mcp.exe"
    New-Item -ItemType Directory -Force (Split-Path $fixture) | Out-Null
    Copy-Item "$bin/mock-mcp.exe" $fixture -Force
    $runs = @()
    foreach ($version in $Versions) {
        $exe = if ($version -eq 'WPF') { "$root/legacy/dotnet/artifacts/SuperGatewayGUI-win-x64/SuperGatewayGUI.exe" } else { "$root/artifacts/SuperGatewayGUI-win-x64/SuperGatewayGUI.exe" }
        foreach ($count in @(0,1,5)) {
            $data = "$root/.build/性能 测试/$version-$count-$([guid]::NewGuid().ToString('N'))"
            $prepareMode = if ($TLS) { 'prepare-tls' } else { 'prepare' }
            & "$bin/desktop-checks.exe" $prepareMode $data $count $fixture
            if ($LASTEXITCODE -ne 0) { throw 'Prepare failed' }
            $timer = [Diagnostics.Stopwatch]::StartNew()
            $gui = Start-Process -FilePath $exe -ArgumentList ('--background --data-dir "'+$data+'"') -PassThru -WindowStyle Hidden
            $holder = $null
            try {
                $window = [IntPtr]::Zero
                while ($window -eq [IntPtr]::Zero -and $timer.Elapsed.TotalSeconds -lt 20 -and !$gui.HasExited) { $window = [BenchWindow]::Find($gui.Id); Start-Sleep -Milliseconds 20 }
                if ($window -eq [IntPtr]::Zero) { throw 'GUI window did not initialize' }
                $startupMs = $timer.Elapsed.TotalMilliseconds
                if ($count -gt 0) {
                    $holder = Start-Process -FilePath "$bin/desktop-checks.exe" -ArgumentList ('hold "'+$data+'"') -PassThru -WindowStyle Hidden -RedirectStandardError "$data/holder-error.txt"
                    $deadline = [DateTime]::UtcNow.AddSeconds(60)
                    while (!(Test-Path "$data/connected") -and [DateTime]::UtcNow -lt $deadline -and !$holder.HasExited) { Start-Sleep -Milliseconds 100 }
                    if (!(Test-Path "$data/connected")) { throw "MCP sessions failed: $(Get-Content "$data/holder-error.txt" -Raw)" }
                }
                Write-Host "$version / $count services: connected, stabilizing 30 seconds"
                Start-Sleep -Seconds 30
                $all = Get-CimInstance Win32_Process
                $ids = [Collections.Generic.HashSet[int]]::new(); $null=$ids.Add($gui.Id)
                do { $changed=$false;foreach ($process in $all) { if ($ids.Contains([int]$process.ParentProcessId) -and $ids.Add([int]$process.ProcessId)) { $changed=$true } } } while ($changed)
                $roles = @{};foreach ($process in $all | Where-Object { $ids.Contains([int]$_.ProcessId) }) {
                    $roles[[int]$process.ProcessId] = if ($process.ProcessId -eq $gui.Id) {'GUI'} elseif ($process.Name -eq 'supergateway.exe') {'Gateway'} elseif ($process.Name -eq 'mock-mcp.exe') {'MCP'} else {'Other'}
                }
                if (($roles.Values | Where-Object {$_ -eq 'Gateway'}).Count -ne $count -or ($roles.Values | Where-Object {$_ -eq 'MCP'}).Count -ne $count) { throw 'Unexpected process counts' }
                $samples = @()
                for ($sample=0;$sample -lt 30;$sample++) {
                    foreach ($role in @('GUI','Gateway','MCP','Other')) {
                        $members = @($roles.Keys | Where-Object {$roles[$_] -eq $role})
                        [long]$working=0;[long]$private=0
                        foreach ($id in $members) { $process=Get-Process -Id $id; $working += $process.WorkingSet64; $private += $process.PrivateMemorySize64 }
                        $samples += [pscustomobject]@{Index=$sample;Role=$role;Processes=$members.Count;WorkingSet=$working;PrivateBytes=$private}
                    }
                    Start-Sleep -Seconds 1
                }
                $summary = @();foreach ($role in @('GUI','Gateway','MCP','Other')) {
                    $values = @($samples | Where-Object Role -eq $role)
                    $ws = @($values.WorkingSet | Sort-Object);$pb=@($values.PrivateBytes | Sort-Object)
                    $summary += [pscustomobject]@{Role=$role;Processes=$values[0].Processes;WorkingSetMedian=($ws[14]+$ws[15])/2;WorkingSetMax=$ws[-1];PrivateMedian=($pb[14]+$pb[15])/2;PrivateMax=$pb[-1]}
                }
                $otherNames = @($all | Where-Object { $roles[[int]$_.ProcessId] -eq 'Other' } | Select-Object -ExpandProperty Name -Unique)
                $runs += [pscustomobject]@{Version=$version;Services=$count;StartupMs=$startupMs;OtherProcessNames=$otherNames;Summary=$summary;Samples=$samples}
                Write-Host "$version / $count services: sampled"
            } finally {
                if ($holder) { 'done' | Set-Content "$data/release"; $holder.WaitForExit(5000) | Out-Null; if (!$holder.HasExited) { Stop-Process -Id $holder.Id -Force } }
                if (!$gui.HasExited) {
                    if ($version -eq 'Rust') { $null=[BenchWindow]::PostMessage($window,0x111,[UIntPtr]17,[IntPtr]::Zero);$gui.WaitForExit(50000)|Out-Null }
                    if (!$gui.HasExited) { Stop-Process -Id $gui.Id -Force; $gui.WaitForExit(5000)|Out-Null }
                }
                Start-Sleep -Milliseconds 500
                if ($TLS -and $count -gt 0) { & "$bin/desktop-checks.exe" untrust-test-ca $data }
            }
        }
    }
    [pscustomobject]@{Machine=(Get-CimInstance Win32_OperatingSystem).Caption;Timestamp=[DateTimeOffset]::Now.ToString('o');TLS=[bool]$TLS;GuiSha256=(Get-FileHash $exe).Hash;Method='GUI hidden in tray; same Rust MCP; persistent WS/WSS initialized and tools queried; 30 seconds stabilization, 30 samples spaced one second; roles summed; process launch window readiness, OS disk cache not flushed; temporary isolated TLS test CA trusted only during measurement and removed afterward';Runs=$runs} | ConvertTo-Json -Depth 8 | Set-Content $Output -Encoding utf8
} finally { Pop-Location }
