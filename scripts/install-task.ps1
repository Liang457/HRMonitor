# scripts/install-task.ps1
# 让 hr-daemon 在登录后自动启动。
#
# 默认不安装 —— 想装的时候手动跑一次：
#     powershell -ExecutionPolicy Bypass -File scripts\install-task.ps1
#
# 两种方式：
#   Task     计划任务（更好：可配重启策略、任务计划程序里能看状态）
#            —— 在根目录注册任务需要管理员权限。
#   Startup  启动文件夹里放一个快捷方式（**不需要管理员**，登录时同样会启动）
#
# -Mode Auto（默认）会先试计划任务，权限不够就自动退到启动文件夹，并明确告诉你。
#
# 参数：
#   -Mode Auto|Task|Startup
#   -Demo             启动参数带 --demo（模拟心率）
#   -Address MAC      启动参数带 --address，例如 AA:BB:CC:DD:EE:FF
#   -Force            计划任务已存在时先删掉再建（失败会自动恢复原来的任务）
#
# 注意：这里给的计划任务带的是**启动参数**，而 daemon 的规则是"命令行参数优先于
# hr-daemon.ini"。所以用 -Address/-Demo 装出来的自启实例不会受 ini 里
# source.address/source.demo 的影响，而 `hr-config restart` 是不带参数启动的，
# 两者行为会不一样。想只靠 ini 控制，就别传这两个开关。
#
# 卸载：powershell -ExecutionPolicy Bypass -File scripts\uninstall-task.ps1

[CmdletBinding()]
param(
    [ValidateSet('Auto', 'Task', 'Startup')]
    [string]$Mode = 'Auto',
    [switch]$Demo,
    # 空串放行，非空就必须是一个完整的 MAC；挡掉 'x --demo' 这类能改写参数的值
    [ValidatePattern('^$|^([0-9A-Fa-f]{2}[:-]){5}[0-9A-Fa-f]{2}$')]
    [string]$Address,
    [switch]$Force
)

$ErrorActionPreference = 'Stop'

$TaskName = 'HuaweiHRDaemon'
$Root     = Split-Path -Parent $PSScriptRoot          # 仓库根目录
$Exe      = Join-Path $Root 'build\hr-daemon.exe'
$LinkPath = Join-Path ([Environment]::GetFolderPath('Startup')) 'HuaweiHRDaemon.lnk'

if (-not (Test-Path -LiteralPath $Exe)) {
    throw "找不到 $Exe —— 先构建：cmd /c daemon\build.cmd"
}

# 登录自启时网络驱动器通常还没连上，放在网络路径上的仓库会静默起不来。
if ($Root.StartsWith('\\')) {
    throw "仓库在 UNC 路径（$Root）上：登录自启时网络还没就绪，daemon 起不来。请把仓库放到本地磁盘。"
}

# 组装启动参数。MAC 已经过 ValidatePattern，只可能是十六进制和分隔符，
# 不会带空格或引号，所以这里不需要再做转义。
$argList = @()
if ($Demo) { $argList += '--demo' }
if ($Address) {
    $hex = ($Address -replace '[:-]', '').ToUpperInvariant()
    $mac = (0..5 | ForEach-Object { $hex.Substring($_ * 2, 2) }) -join ':'
    $argList += @('--address', $mac)
}
$argString = ($argList -join ' ')

# 当前用户的完整身份（DOMAIN\user）。只用 $env:USERNAME 在域里含义不明确。
$CurrentUser = [Security.Principal.WindowsIdentity]::GetCurrent().Name

function Install-StartupShortcut {
    $shell = New-Object -ComObject WScript.Shell
    $lnk = $shell.CreateShortcut($LinkPath)
    $lnk.TargetPath       = $Exe
    $lnk.Arguments        = $argString
    $lnk.WorkingDirectory = Split-Path -Parent $Exe
    $lnk.Description      = 'Huawei watch heart rate -> TrafficMonitor + Afterburner OSD'
    $lnk.Save()
    Write-Host "[startup] 已创建快捷方式: $LinkPath"
    Write-Host "[startup] 目标: $Exe $argString"
    Write-Host '[startup] 下次登录时自动启动。现在就试：直接双击那个快捷方式。'
}

function Install-ScheduledTask {
    $existing = Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
    if ($existing -and -not $Force) {
        throw "计划任务 '$TaskName' 已存在。要覆盖请加 -Force，或先跑 uninstall-task.ps1。"
    }

    # 先把旧任务导出成 XML：注册失败时还能恢复回去。以前是"先删再建"，
    # 建失败就落得一个自启都没有，且没法回滚。
    $backupXml = $null
    if ($existing) {
        $backupXml = Export-ScheduledTask -TaskName $TaskName
        Write-Host "[task] 删除已存在的任务 '$TaskName'"
        Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false
    }

    $action = New-ScheduledTaskAction -Execute $Exe -Argument $argString -WorkingDirectory (Split-Path -Parent $Exe)
    $trigger = New-ScheduledTaskTrigger -AtLogOn -User $CurrentUser
    $settings = New-ScheduledTaskSettingsSet `
        -AllowStartIfOnBatteries `
        -DontStopIfGoingOnBatteries `
        -StartWhenAvailable `
        -ExecutionTimeLimit ([TimeSpan]::Zero) `
        -MultipleInstances IgnoreNew
    $principal = New-ScheduledTaskPrincipal -UserId $CurrentUser -LogonType Interactive -RunLevel Limited

    try {
        Register-ScheduledTask -TaskName $TaskName `
            -Action $action -Trigger $trigger -Settings $settings -Principal $principal `
            -Description '华为手表心率广播 → TrafficMonitor + MSI Afterburner OSD' | Out-Null
    } catch {
        if ($backupXml) {
            Write-Warning "注册失败：$($_.Exception.Message)"
            Write-Host '[task] 正在把原来的任务恢复回去 ...'
            try {
                Register-ScheduledTask -Xml $backupXml -TaskName $TaskName | Out-Null
                Write-Host '[task] 已恢复原任务。'
            } catch {
                Write-Warning "恢复也失败了：$($_.Exception.Message)"
                $dump = Join-Path $env:TEMP 'HuaweiHRDaemon.old.xml'
                Set-Content -LiteralPath $dump -Value $backupXml -Encoding UTF8
                Write-Warning "原任务定义已存到 $dump，可以手工恢复。"
            }
        }
        throw
    }

    Write-Host "[task] 已注册 '$TaskName'"
    Write-Host "       程序: $Exe"
    Write-Host "       参数: $(if ($argString) { $argString } else { '(无，读 hr-daemon.ini)' })"
    Write-Host "       用户: $CurrentUser"
    Write-Host ''
    Write-Host "立即试跑:  Start-ScheduledTask -TaskName $TaskName"
    Write-Host "查看状态:  Get-ScheduledTask -TaskName $TaskName | Get-ScheduledTaskInfo"
}

function Test-Admin {
    ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
    ).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}

# 启动文件夹里如果已经有一个同名但指向别处的快捷方式，先确认一下再覆盖
if ((Test-Path -LiteralPath $LinkPath) -and -not $Force) {
    $existingTarget = $null
    try {
        $existingTarget = (New-Object -ComObject WScript.Shell).CreateShortcut($LinkPath).TargetPath
    } catch { }
    if ($existingTarget -and ($existingTarget -ine $Exe)) {
        throw "$LinkPath 已存在，但它指向 $existingTarget（不是 $Exe）。要覆盖请加 -Force。"
    }
}

switch ($Mode) {
    'Startup' {
        Install-StartupShortcut
    }
    'Task' {
        Install-ScheduledTask
    }
    'Auto' {
        if (Test-Admin) {
            Install-ScheduledTask
        } else {
            try {
                Install-ScheduledTask
            } catch {
                # 注册根目录下的计划任务需要管理员；退到启动文件夹，同样能登录自启。
                Write-Warning "计划任务注册失败（需要管理员权限）：$($_.Exception.Message)"
                Write-Host '[auto] 自动退到"启动文件夹"方式（不需要管理员）...'
                Install-StartupShortcut
            }
        }
    }
}

Write-Host ''
Write-Host '卸载: powershell -ExecutionPolicy Bypass -File scripts\uninstall-task.ps1'
