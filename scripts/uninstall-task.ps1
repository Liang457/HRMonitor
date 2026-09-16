# scripts/uninstall-task.ps1
# 撤销 install-task.ps1 做的一切（计划任务 + 启动文件夹快捷方式），
# 不动 hr-daemon.exe 本身，也不会去杀正在跑的 daemon。
#
#     powershell -ExecutionPolicy Bypass -File scripts\uninstall-task.ps1

[CmdletBinding()]
param(
    # 删快捷方式前不检查它指向哪里（默认只删指向本仓库 hr-daemon.exe 的那个）
    [switch]$Force
)

$ErrorActionPreference = 'Stop'

$TaskName = 'HuaweiHRDaemon'
$LinkPath = Join-Path ([Environment]::GetFolderPath('Startup')) 'HuaweiHRDaemon.lnk'
$Exe      = Join-Path (Split-Path -Parent $PSScriptRoot) 'build\hr-daemon.exe'

$did = $false

# --- 计划任务
$task = Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
if ($task) {
    Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false
    Write-Host "[task] 已删除计划任务 '$TaskName'"
    $did = $true
} else {
    Write-Host "[task] 没有名为 '$TaskName' 的计划任务。"
}

# --- 启动文件夹快捷方式
# 只按文件名删会误删同名的、指向别处的快捷方式，所以先看一眼它的目标。
if (Test-Path -LiteralPath $LinkPath) {
    $target = $null
    try {
        $target = (New-Object -ComObject WScript.Shell).CreateShortcut($LinkPath).TargetPath
    } catch { }

    if (-not $Force -and $target -and ($target -ine $Exe)) {
        Write-Warning "$LinkPath 指向 $target，不是本仓库的 $Exe —— 没动它。确实要删就加 -Force。"
    } else {
        Remove-Item -LiteralPath $LinkPath -Force
        Write-Host "[startup] 已删除快捷方式: $LinkPath"
        $did = $true
    }
} else {
    Write-Host '[startup] 启动文件夹里没有快捷方式。'
}

if (-not $did) { Write-Host '没有需要清理的东西。' }
Write-Host ''
Write-Host '（正在运行的 hr-daemon 没被动。要停：taskkill /IM hr-daemon.exe）'
