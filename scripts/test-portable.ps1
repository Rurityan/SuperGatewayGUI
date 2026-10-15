param([string]$Package = 'artifacts/SuperGatewayGUI-win-x64')
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
Push-Location $root
try {
    $stage = "$root/.build/中文 便携 验收 $([guid]::NewGuid().ToString('N'))"
    New-Item -ItemType Directory -Force "$stage/app" | Out-Null
    Copy-Item "$Package/*" "$stage/app" -Recurse -Force
    $bin = "$root/target/x86_64-pc-windows-msvc/release"
    Copy-Item "$bin/mock-mcp.exe" "$stage/app/mock-mcp.exe"
    & "$bin/desktop-checks.exe" prepare-portable "$stage/data" 1 "$stage/app/mock-mcp.exe"
    if ($LASTEXITCODE -ne 0) { throw 'Portable settings preparation failed' }
    Add-Type @'
using System;using System.Text;using System.Runtime.InteropServices;
public static class PortableWindow {
 public delegate bool Callback(IntPtr h,IntPtr l);
 [DllImport("user32.dll")] static extern bool EnumWindows(Callback c,IntPtr l);
 [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h,out uint pid);
 [DllImport("user32.dll",CharSet=CharSet.Unicode)] static extern int GetClassName(IntPtr h,StringBuilder b,int n);
 [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h,uint msg,UIntPtr w,IntPtr l);
 public static IntPtr Find(int target) {IntPtr result=IntPtr.Zero;EnumWindows((h,l)=>{uint pid;GetWindowThreadProcessId(h,out pid);var b=new StringBuilder(64);GetClassName(h,b,64);if(pid==target&&b.ToString()=="SGRustMain"){result=h;return false;}return true;},IntPtr.Zero);return result;}
}
'@
    foreach ($attempt in @(1,2)) {
        Remove-Item "$stage/data/connected", "$stage/data/release" -ErrorAction SilentlyContinue
        $start = [Diagnostics.ProcessStartInfo]::new("$stage/app/SuperGatewayGUI.exe")
        $start.UseShellExecute = $false
        $start.ArgumentList.Add('--background');$start.ArgumentList.Add('--data-dir');$start.ArgumentList.Add("$stage/data")
        $start.Environment['PATH']='';$start.Environment['DOTNET_ROOT']="$stage/no-dotnet"
        $gui=[Diagnostics.Process]::Start($start)
        $holder=$null
        try {
            $holder=Start-Process -FilePath "$bin/desktop-checks.exe" -ArgumentList ('hold "'+$stage+'/data"') -PassThru -WindowStyle Hidden -RedirectStandardError "$stage/data/probe-error.txt"
            $deadline=[DateTime]::UtcNow.AddSeconds(35)
            while (!(Test-Path "$stage/data/connected") -and [DateTime]::UtcNow -lt $deadline -and !$holder.HasExited -and !$gui.HasExited) {Start-Sleep -Milliseconds 100}
            if (!(Test-Path "$stage/data/connected")) {throw "Portable MCP failed: $(Get-Content "$stage/data/probe-error.txt" -Raw)"}
            $all=Get-CimInstance Win32_Process
            $engines=@($all|Where-Object {$_.ParentProcessId -eq $gui.Id -and $_.Name -eq 'supergateway.exe'})
            $mcp=@($all|Where-Object {$_.ParentProcessId -in $engines.ProcessId -and $_.Name -eq 'mock-mcp.exe'})
            if ($engines.Count -ne 1 -or $mcp.Count -ne 1) {throw 'Unexpected portable process tree'}
            'done'|Set-Content "$stage/data/release"
            if (!$holder.WaitForExit(5000) -or $holder.ExitCode -ne 0) {throw 'Probe did not close session'}
            $h=[PortableWindow]::Find($gui.Id)
            if ($h -eq [IntPtr]::Zero) {throw 'Portable main window not found'}
            $null=[PortableWindow]::PostMessage($h,0x111,[UIntPtr]17,[IntPtr]::Zero)
            if (!$gui.WaitForExit(20000) -or $gui.ExitCode -ne 0) {throw 'Portable GUI normal exit failed'}
            foreach ($id in @($engines.ProcessId)+@($mcp.ProcessId)) {if (Get-Process -Id $id -ErrorAction SilentlyContinue) {throw "Portable child remains: $id"}}
            Write-Host "PASS Chinese/spaced portable path, empty PATH, initialized MCP, normal tree cleanup, launch $attempt"
        } finally {
            if ($holder -and !$holder.HasExited) {Stop-Process -Id $holder.Id -Force}
            if (!$gui.HasExited) {Stop-Process -Id $gui.Id -Force}
        }
    }
} finally {Pop-Location}
