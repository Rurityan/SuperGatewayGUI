# Optional developer-only compatibility test. Requires .NET SDK to generate an
# authentic legacy settings.dat; the Rust build and portable application do not.
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
$fixture = "$root/.build/legacy-compat"
New-Item -ItemType Directory -Force $fixture | Out-Null
@'
<Project Sdk="Microsoft.NET.Sdk"><PropertyGroup><OutputType>Exe</OutputType><TargetFramework>net10.0</TargetFramework><ImplicitUsings>enable</ImplicitUsings><Nullable>enable</Nullable></PropertyGroup><ItemGroup><Compile Include="../../legacy/dotnet/src/SuperGatewayGUI/Core/Models.cs" Link="Models.cs"/><Compile Include="../../legacy/dotnet/src/SuperGatewayGUI/Core/SettingsStore.cs" Link="SettingsStore.cs"/></ItemGroup></Project>
'@ | Set-Content "$fixture/Compat.csproj"
@'
using SuperGatewayGUI.Core;
new SettingsStore(args[0]).Save(new AppSettings { Services = [new ServiceProfile { Name = "旧版中文", Source = "http", Url = "https://example.com/mcp", Bearer = "旧令牌", Env = [new Pair { Key = "TOKEN", Value = "旧秘密" }] }] });
'@ | Set-Content "$fixture/Program.cs" -Encoding utf8
$data = "$fixture/data-$([guid]::NewGuid().ToString('N'))"
dotnet run --project "$fixture/Compat.csproj" -- $data
if ($LASTEXITCODE -ne 0) { throw 'Legacy fixture generation failed' }
& "$root/target/x86_64-pc-windows-msvc/release/desktop-checks.exe" compat $data
if ($LASTEXITCODE -ne 0) { throw 'Legacy compatibility failed' }
