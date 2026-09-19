# scripts/pack.ps1
#
# 发行打包脚本：把 build\ 下的构建产物组装成规范布局的发行目录再压成 zip。
# .github/workflows/release.yml 的"组装发行包"步骤就是调这个脚本；
# 本地打一个发行包也是它：
#
#     powershell -ExecutionPolicy Bypass -File scripts\pack.ps1
#
# 布局（发行目录的根只放 exe 和 README.md，其余各归子目录）：
#   HRMonitor-<版本>-win64\
#     hr-daemon.exe  hr-manager.exe  README.md
#     config\hr-daemon.ini            示例配置
#     docs\LICENSE、THIRD_PARTY_NOTICES.md
#     plugins\HeartRate.dll、hr_plugin.dll
#     scripts\                        部署/迁移脚本原样带上
#
# 版本一致性：Cargo.toml 的 [package] version 与 common\version.h 的
# HR_VERSION_STRING 是跨语言双副本（common\version.h 头部注释有约定），
# 两处不一致直接报错；HEAD 恰好打着 v* 标签时还要求标签号一致
# （尽力而为：浅克隆/无 git 拿不到标签就跳过）。
#
# 参数：
#   -Name <名字>      产物目录/zip 名（默认 HRMonitor-<版本>-win64；CI 的 dev
#                     构建传 HRMonitor-dev-<短SHA>-win64）
#   -OutputDir <目录> 输出位置（默认仓库根 dist\）
#
# 注意：本地 build\config\hr-daemon.ini 已存在时会原样带进发行包 —— 那可能是
# 你自己的测试配置（比如里面有手表地址），不想带上就先删掉它。

[CmdletBinding()]
param(
    [string]$Name = '',
    [string]$OutputDir = ''
)

$ErrorActionPreference = 'Stop'
$Root = Split-Path -Parent $PSScriptRoot

# --- 版本一致性：发行包里的每个组件对"自己是什么版本"必须回答一致 -----------
$cargoText = [System.IO.File]::ReadAllText((Join-Path $Root 'tools\hr-manager\Cargo.toml'), [System.Text.Encoding]::UTF8)
if ($cargoText -notmatch '(?m)^\s*version\s*=\s*"([^"]+)"') {
    throw 'Cargo.toml 里找不到 [package] version'
}
$cargoVer = $Matches[1]

$verHText = [System.IO.File]::ReadAllText((Join-Path $Root 'common\version.h'), [System.Text.Encoding]::UTF8)
if ($verHText -notmatch '#define HR_VERSION_STRING\s+"([^"]+)"') {
    throw 'common\version.h 里找不到 HR_VERSION_STRING'
}
$verH = $Matches[1]

if ($cargoVer -ne $verH) {
    throw "版本不一致：Cargo.toml=$cargoVer，common\version.h=$verH —— 发版必须两边同步改（common\version.h 头部注释有说明）"
}
$Version = $cargoVer

# --- HEAD 上的 v* 标签（拿不到就跳过，属尽力而为） --------------------------
$tag = $null
try {
    $t = & git -C $Root describe --tags --exact-match 2>$null
    if ($LASTEXITCODE -eq 0 -and $t) { $tag = ("$t").Trim() }
} catch { }
if (-not $tag) {
    try {
        $t = & git -C $Root tag --points-at HEAD 2>$null | Select-Object -First 1
        if ($LASTEXITCODE -eq 0 -and $t) { $tag = ("$t").Trim() }
    } catch { }
}
if ($tag -and $tag.StartsWith('v')) {
    $tagVer = $tag.Substring(1)
    if ($tagVer -ne $Version) {
        throw "版本不一致：标签 $tag，但产物版本是 $Version —— 先把 Cargo.toml 和 common\version.h 改到一致，再打标签"
    }
}

if (-not $Name) {
    $Name = if ($tag) { "HRMonitor-$tag-win64" } else { "HRMonitor-$Version-win64" }
}
if (-not $OutputDir) { $OutputDir = Join-Path $Root 'dist' }

# --- 产物就位检查 -----------------------------------------------------------
$artifacts = @(
    'build\hr-daemon.exe',
    'build\hr-manager.exe',
    'build\plugins\HeartRate.dll',
    'build\plugins\hr_plugin.dll'
)
foreach ($a in $artifacts) {
    if (-not (Test-Path -LiteralPath (Join-Path $Root $a))) {
        throw "找不到 $a —— 先跑全部 build.cmd：daemon / ab-plugin / tm-plugin / tools\hr-manager"
    }
}

# --- 示例配置：CI 的 build\ 是全新的，直接 reset 生成；本地已有的配置原样带上
$ini = Join-Path $Root 'build\config\hr-daemon.ini'
if (-not (Test-Path -LiteralPath $ini)) {
    Write-Host '[pack] build\config\hr-daemon.ini 不存在，用 hr-manager reset -y 生成默认配置 ...'
    & (Join-Path $Root 'build\hr-manager.exe') reset -y
    if ($LASTEXITCODE -ne 0) { throw 'hr-manager reset -y 失败' }
    if (-not (Test-Path -LiteralPath $ini)) { throw "reset 之后仍找不到 $ini" }
} else {
    Write-Host "[pack] 使用现有的 build\config\hr-daemon.ini（注意：那是本机 build\ 里的配置）"
}

# --- 组装 + 压缩 ------------------------------------------------------------
$stage = Join-Path $OutputDir $Name
$zip   = Join-Path $OutputDir "$Name.zip"
if (Test-Path -LiteralPath $stage) { Remove-Item -LiteralPath $stage -Recurse -Force }
if (Test-Path -LiteralPath $zip)   { Remove-Item -LiteralPath $zip -Force }

New-Item -ItemType Directory -Path (Join-Path $stage 'config')  -Force | Out-Null
New-Item -ItemType Directory -Path (Join-Path $stage 'docs')    -Force | Out-Null
New-Item -ItemType Directory -Path (Join-Path $stage 'plugins') -Force | Out-Null

Copy-Item -LiteralPath (Join-Path $Root 'build\hr-daemon.exe')  -Destination $stage
Copy-Item -LiteralPath (Join-Path $Root 'build\hr-manager.exe') -Destination $stage
Copy-Item -LiteralPath (Join-Path $Root 'README.md')            -Destination $stage
Copy-Item -LiteralPath $ini -Destination (Join-Path $stage 'config\hr-daemon.ini')
Copy-Item -LiteralPath (Join-Path $Root 'build\plugins\HeartRate.dll') -Destination (Join-Path $stage 'plugins\HeartRate.dll')
Copy-Item -LiteralPath (Join-Path $Root 'build\plugins\hr_plugin.dll') -Destination (Join-Path $stage 'plugins\hr_plugin.dll')
Copy-Item -LiteralPath (Join-Path $Root 'LICENSE')                -Destination (Join-Path $stage 'docs\LICENSE')
Copy-Item -LiteralPath (Join-Path $Root 'THIRD_PARTY_NOTICES.md') -Destination (Join-Path $stage 'docs\THIRD_PARTY_NOTICES.md')
Copy-Item -LiteralPath (Join-Path $Root 'scripts') -Destination (Join-Path $stage 'scripts') -Recurse

Compress-Archive -Path $stage -DestinationPath $zip -Force

Write-Host ''
Write-Host "[pack] $zip"
Write-Host "[pack] 版本：$Version"
Get-ChildItem -LiteralPath $stage -Recurse | ForEach-Object {
    $rel = $_.FullName.Substring($stage.Length + 1)
    if ($_.PSIsContainer) { Write-Host "  $rel\" }
    else { Write-Host ("  {0}  ({1} 字节)" -f $rel, $_.Length) }
}
