# HRMonitor

把 BLE 心率设备的"心率广播"（标准 BLE Heart Rate Profile，服务 `0x180D`）读进电脑，再送到
两个显示端：MSI Afterburner 的游戏内 OSD，和 TrafficMonitor 的任务栏。

```
BLE 心率设备（心率广播） ──BLE──▶ hr-daemon.exe ──写──▶ Local\BleHR_SM（64 字节共享内存）
                          连接/重连/扫描               │
                                          ┌────────────┼────────────┐
                                          ▼            ▼            ▼
                                 HeartRate.dll  hr_plugin.dll  hr-manager.exe
                                  Afterburner OSD / 任务栏 / 托盘 + 配置面板
```

daemon 只负责采集和写共享内存，蓝牙全在它这边；三个读端各自去读，互不依赖。

`hr-manager.exe` 是管理器：托盘常驻，配置面板（WebView2 渲染的 HTML 单页）按需弹出、
关闭即整体销毁；同一个 exe 带 CLI 子命令，开机自启、选表扫描、重启采集、部署脚本都在
它身上。

BLE 走 Windows SDK 自带的 C++/WinRT（链接 `windowsapp.lib`），两个插件只依赖 `kernel32`；
hr-manager 的 GUI 栈（wry + tao + tray-icon）和构建期嵌版本信息的 winresource 是仅有的
第三方 crate（后者只在 build.rs 用）。第三方组件的授权情况见
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)。

## 功能

- **心率采集**：按 `0x180D` 扫描广播设备，订阅 `0x2A37` 通知，断开或超时自动重连
  （退避从 1 秒翻倍到 30 秒）
- **两个显示端**：Afterburner 里是一条名为 `Heart rate` 的原生监控数据源（bpm），
  TrafficMonitor 里是任务栏上的一项 `HR 128`
- **管理器 hr-manager**：托盘常驻 + 配置面板，实时心率、扫描选表、改配置、开机自启、
  调部署脚本都在一页里
- **模拟源**：`demo=1` 时用心率随机游走代替手表，没有表也能验通整条链路
- **设备区分**：按 MAC 直连，换手表只改 `address` 一行
- **版本一致**：四个 exe/DLL 的文件属性带版本号（资源管理器 → 属性 → 详细信息），
  `hr-daemon --version`、`hr-manager version`、面板底部和托盘提示也都能看到；版本号只在
  `common\version.h` 和 hr-manager 的 `Cargo.toml` 两处维护，打包脚本强制校验一致

## 目录结构

```
HRMonitor\
├─ README.md
├─ LICENSE                     本仓库自己的代码：MIT
├─ THIRD_PARTY_NOTICES.md      第三方组件授权说明
├─ common\
│  ├─ hr_shared.h              共享内存协议（daemon 与两个插件共用的唯一定义）
│  ├─ hr_config.h/.cpp         INI 读写 + 配置结构 + 路径/MAC 工具
│  ├─ version.h                产品版本号（C++ 侧唯一定义，与 Cargo.toml 双副本同步）
│  └─ version.rc.in            各 exe/DLL 共用的版本资源模板（各组件目录里有自己的壳）
├─ daemon\                     hr-daemon.exe（C++，x64）：采集 + 写共享内存
├─ ab-plugin\                  HeartRate.dll（C++，x86）：Afterburner 监控数据源
├─ tm-plugin\                  hr_plugin.dll（C++，x64）：TrafficMonitor 插件
├─ tools\
│  ├─ hr-manager\              hr-manager.exe（Rust + wry）：托盘/配置面板/CLI
│  ├─ mahm-probe\              读 Afterburner 共享内存的小脚本，排查用
│  └─ osd_test\                osd_test.exe（C++）：D3D11 清屏窗口，验证 OSD 用
├─ scripts\                    部署 / 发行打包 / 迁移 / 旧自启清理脚本
└─ build\                      构建产物（不入库）：exe 在根，DLL 在 plugins\，示例 ini 在 config\
```

## 快速开始

### 构建

需要 MSVC（VS 2019/2022/2026 的 Build Tools 都行，要包含 x86 目标）和 Rust 工具链
（只有 hr-manager 需要，[rustup.rs](https://rustup.rs) 默认选项即可）。

```cmd
cmd /c daemon\build.cmd           :: -> build\hr-daemon.exe            (C++, x64)
cmd /c ab-plugin\build.cmd        :: -> build\plugins\HeartRate.dll    (C++, x86, 需要 x86 工具集)
cmd /c tm-plugin\build.cmd        :: -> build\plugins\hr_plugin.dll    (C++, x64)
cmd /c tools\osd_test\build.cmd   :: -> build\osd_test.exe             (C++, x64)
cmd /c tools\hr-manager\build.cmd :: -> build\hr-manager.exe           (Rust + WebView2 GUI)
```

hr-manager 首次构建要联网拉依赖（Cargo.lock 已锁版本），之后可离线重建，其余部分不
依赖网络；面板用系统自带的 WebView2 渲染。各 `.cmd` 用 `vswhere` 找 MSVC，不绑死 VS
版本；C++ 产物是 `/MT` 静态 CRT，目标机器不用装 VC++ 运行库。

`ab-plugin` 必须是 x86：`MSIAfterburner.exe` 是 32 位进程（PE `0x14C`），加载不了 x64
DLL。daemon 是 x64 不影响，两边只通过共享内存交换数据。`ab-plugin\build.cmd` 自己用
`vcvarsall.bat x86` 初始化，末尾用 `dumpbin` 自查架构和三个导出名有没有被修饰。

构建时各组件把版本资源（rc.exe / winresource）链进 PE：版本号来自 `common\version.h`，
它与 `tools/hr-manager/Cargo.toml` 的 `[package] version` 是跨语言双副本（同 `hr_names.h`
的同步约定），改版本要两边一起改——打包时 `scripts\pack.ps1` 会校验，不一致直接报错。
构建目录布局与发行目录一致：exe 和 PDB 在 `build\` 根，两个 DLL 在 `build\plugins\`，
`hr-manager reset -y` 生成的示例配置在 `build\config\`。

改过共享内存布局后，写端和所有读端要一起重建（daemon + 两个插件 + hr-manager）：记录里
带 `version` 字段，读端会校验，只换一半读不到数据（这是协议版本 `HRSM_VERSION`，与产品
版本号是两回事）。

发行版由 [GitHub Actions](.github/workflows/release.yml) 构建：push 到 main 出
Artifacts，推 `v*` 标签建 Release 并附完整发行包。组装/校验/压缩都收在
`scripts\pack.ps1`，本地打一个同样的包：
`powershell -ExecutionPolicy Bypass -File scripts\pack.ps1`（产物在 `dist\`）。
发行目录的根只放 exe 和 README.md，其余各归子目录：`config\`（示例配置）、`docs\`
（LICENSE、THIRD_PARTY_NOTICES.md）、`plugins\`（两个 DLL）、`scripts\`。运行后生成
的日志和 WebView2 数据分别落在 `log\`、`webview2\`，根目录不会多出别的东西。
`tm-plugin\PluginInterface.h` 取自
TrafficMonitor V1.86，是"反996许可证"，发布前看一眼
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)。

### 不用手表跑通链路

```cmd
:: 1. 换成模拟心率源并重启 daemon
build\hr-manager.exe set demo 1
build\hr-manager.exe restart

:: 2. 看数据到底出去没有（不需要 GUI，也不需要 RTSS）
pwsh -File tools\mahm-probe\mahm-probe.ps1 -Filter Heart
```

第 2 步会打印共享内存里那条 `Heart rate` 的名字、单位、当前值和标志位：

```
signature=0x4D41484D  version=0x00020000  headerSize=32  entries=156  entrySize=1324

[155] Heart rate
      units='BPM'  format='%.0f'  value=137  flags=OSD/-/-  gpu=0x0  srcId=0xFF
```

`value=` 跟着模拟心率一直在变，整条链路就是通的（Afterburner 要在运行、插件要启用）。
daemon 停 15 秒后这一项会变成 `UNAVAILABLE (FLT_MAX)`，是 MAHM 约定的"当前没有数据"。

## 配置

配置全在 `config\hr-daemon.ini`（`hr-daemon.exe` 同级的 `config\` 子目录），UTF-8、带注
释，可以直接手改；日常入口是 hr-manager 面板（托盘图标 → 打开面板）。不想动 GUI 就用
同一个 exe 的 CLI：

```cmd
build\hr-manager.exe show            :: 列出当前配置
build\hr-manager.exe get demo        :: 读一项
build\hr-manager.exe set demo 1      :: 改一项（只写文件，重启后生效）
build\hr-manager.exe reset [-y]      :: 恢复默认
build\hr-manager.exe status          :: daemon 状态 + 共享内存实时值
build\hr-manager.exe start / stop    :: 启动 / 停止 daemon
build\hr-manager.exe restart         :: 重启 daemon 让改动生效
build\hr-manager.exe scan [秒数]     :: 扫描附近的心率广播设备（默认 5 秒）
build\hr-manager.exe path            :: 显示各文件位置
build\hr-manager.exe version         :: 显示版本号
build\hr-manager.exe help            :: 全部命令
build\hr-manager.exe                 :: 不带参数 = 打开面板（--minimized 只出托盘）
```

保存时先把原文件备份成 `hr-daemon.ini.bak`，再写临时文件原子替换，不会留下半份配置。
ini 存在但读不出来（被编辑器锁着、存成了别的编码）时 hr-manager 直接报错退出，不退回
默认值。

### hr-daemon.ini

| 段 | 键 | 默认 | 说明 |
|---|---|---|---|
| `[source]` | `demo` | 否 | 换成模拟心率源（60~180 随机游走），不需要手表 |
| `[source]` | `address` | 空 | 直连的手表 MAC。留空则不连接，daemon 不会自己扫一台连上（多设备环境会连错表），选表靠 hr-manager |
| `[source]` | `scan_timeout_ms` | 20000 | `--scan` 不带秒数参数时的默认扫描时长（毫秒） |
| `[source]` | `backoff_min_sec` / `backoff_max_sec` | 1 / 30 | 重连退避的上下限 |
| `[display]` | `timeout_ms` | 15000 | 多久没数据就显示 `--`（毫秒） |
| `[display]` | `refresh_ms` | 1000 | 采样/推送周期（毫秒） |
| `[log]` | `max_kb` | 4096 | 单个日志文件的上限（KB），写满就轮转 |
| `[log]` | `debug` | 否 | `是` = 连每条心率都写进日志（排查用，平时别开） |
| `[integration]` | `tm_dir` | 空 | TrafficMonitor 安装目录（面板的部署按钮会传给脚本） |

`timeout_ms` 一个数用在三处：daemon 判显示超时、BLE 层判连接失效（这么久没收到通知就断开
重连）、daemon 不在时插件的兜底。

日志每次启动开新的 `hr-daemon.log`，上一份顶成 `hr-daemon.1.log`、再上一份顶成 `.2.log`，
磁盘上最多留 3 份，单份超过 `max_kb` 运行中就地轮转。心率值默认不写日志（几乎每秒都在变，
全写下来一天能涨好几 MB），日志里只有状态变化和错误；要看每条心率就开 `debug`
（`hr-manager set debug 1` + 重启，或前台跑 `hr-daemon.exe --debug`），这些行带 `[DEBG]` 标记。

### hr_plugin.ini

放在插件 DLL 旁边，也就是 `<TrafficMonitor>\plugins\`：

```ini
[plugin]
label=HR          ; 数值前面显示的标签
name=心率          ; 在 TrafficMonitor"显示设置"里的名字
sample=128        ; 示例值，决定显示区宽度（用最宽的情况）
timeout_ms=15000  ; daemon 挂掉时插件自己的兜底超时
```

改完要重启 TrafficMonitor 才生效，文件不存在就用上面这些默认值。

## 手表设置

1. 手表和手机"运动健康"App 配对好。
2. 在运动健康里开启"心率广播"（不同机型路径略有差异，一般在设备 → 健康监测/心率里）。
3. 手表停在"心率广播"页面并保持亮屏，离开该页面广播就停了。
4. 打开 `build\hr-manager.exe` 面板点"开始扫描"选出手表：扫描会先停掉 daemon 等 2 秒
   （手表被连着时不广播），设备随发现随出现，选完自动保存并重启 daemon，不选就点"放弃"
   恢复采集。也可以命令行直接指定（MAC 可先用 `scan` 确认）：

```cmd
build\hr-manager.exe set demo 0
build\hr-manager.exe set address AA:BB:CC:DD:EE:FF
build\hr-manager.exe restart
```

日志里出现 `模式: 直连配置里的地址 ...` → `BLE: 已连接 ...，已订阅心率通知` 就成了。
地址留空 daemon 不会连接，任务栏和 OSD 一直显示 `--`，日志提示"未配置手表地址"。

想确认广播在线，用 `build\hr-manager.exe scan 10`。很多设备在广播里常常不带名字，列表里
那台会先显示"（还没拿到名字）"，认 MAC 就行。

## 显示端

### TrafficMonitor（任务栏）

去 GitHub Releases 下载 `TrafficMonitor_V1.86_x64_Lite.zip`，解压到比如
`D:\Program Files\TrafficMonitor`：

```powershell
powershell -ExecutionPolicy Bypass -File scripts\configure-trafficmonitor.ps1
```

脚本会关掉 TrafficMonitor，把插件 `hr_plugin.dll` 替换进 `plugins\`（旧文件留成
`.bak`，内容相同则跳过），把 `hr` 加进 `plugin_display_item`、写上
`show_task_bar_wnd=true`，再启动它。插件 DLL 依次从 `<仓库>\build\plugins\`、
`<仓库>\plugins\`、`<仓库>\` 找，都不在就只改配置并警告（也可用 `-PluginDll` 指定）；
`-TmDir` 缺省时依次读 `hr-daemon.ini` 的 `integration.tm_dir`（`config\` 里的和旧位置
的都认）和常见安装位置，改 `config.ini` 前先备份成 `config.ini.bak` 并按原编码原子写回。

插件文件名必须是 `.dll`，不能是 `.tmd`：V1.86 只扫 `plugins\*.dll`
（`PluginManager.cpp:41`），放 `.tmd` 进去"插件管理"里会是空的，且没有任何报错。插件默认
即启用，`plugin_disabled` 是黑名单（默认空），不用去点启用。

任务栏窗口没出现就右键通知区图标 → 显示任务栏窗口，开一次会被记住（`show_task_bar_wnd`
写进 `config.ini` 不一定生效，运行时会被内存值覆盖）。任务栏上没有 `HR` 项，右键任务栏
上的 TrafficMonitor 区域 → 显示设置 → 勾上"心率"。

### MSI Afterburner（游戏内 OSD）

Afterburner 的游戏内 OSD 是 RTSS 画的。本项目不碰 RTSS，而是做成 Afterburner 的一条
原生监控数据源，心率就和 GPU 温度、帧率并列在同一条 OSD 里，颜色、量程、位置都在
Afterburner 一个界面里配。

```powershell
cmd /c ab-plugin\build.cmd
powershell -ExecutionPolicy Bypass -File scripts\deploy-afterburner-plugin.ps1
```

脚本自己找 `HeartRate.dll`（依次 `<仓库>\build\plugins\`、`<仓库>\plugins\`、`<仓库>\`，
或用 `-DllPath` 指定）装进 `<Afterburner>\Plugins\Monitoring\`（旧文件留成 `.bak`）、写
插件说明到 `Help\Plugins\Monitoring\HeartRate`、在 `Profiles\MSIAfterburner.cfg` 的
`[Monitoring]` 段写 `HeartRate.dll=1`、确认根配置 `EnablePlugins=1`。
`-AbDir` 缺省时按常见安装位置自动探测。全程先关 Afterburner 再改文件
（它退出时会把内存里的配置写回 profile），只动自己那一行并留 `.bak`，卸载加 `-Uninstall`。
它不去改 `Sources=` / `[Source ...]` 段，那是 Afterburner 自己的数据结构。

启用插件后在界面里点两下：

1. 设置 → 监控 → 在"硬件监控图表列表"里勾上 `Heart rate`。这步不需要 RTSS，也不需要有
   3D 程序在跑，勾上后监控窗口就会实时画曲线，数据链路可以脱离 OSD 单独验证。
2. 选中 `Heart rate`，点右边的设置按钮 → 勾上 `Show in On-Screen Display`。

想改 OSD 上的样子都在同一个属性窗口里：

| 想改什么 | 在哪里改 |
|---|---|
| 把显示名缩短成 `HR` | `Override graph name` |
| 曲线上下限（默认 40~180） | `Min limit` / `Max limit` |
| 颜色 | `Graph color`（OSD 上的分组配色由 Afterburner 的 OSD 布局决定） |
| 数值格式 / 单位 | `Override graph name` 旁边的相关项；单位默认 `BPM` |
| 进不进 OSD / 托盘 / 键盘 LCD | `Show in On-Screen Display` / `Show in Tray` / `Show in LCD` |
| OSD 整体位置、字号、配色 | 设置 → On-Screen Display 里的布局编辑器 |

RTSS 只在自己 hook 到的 3D 程序上画 OSD。要确认 OSD 真画出来了，跑 `build\osd_test.exe`
当画布，窗口左上角应该出现心率那一行；这一步才需要 RTSS 在跑，它不跑不影响曲线和任务栏。

脚本按"目录能不能写"判断要不要管理员。Afterburner 一般以管理员运行，而它启动时会加载
该目录下所有 DLL，别把这个目录的写权限开放给不受信任的账户。

## 开机自启

面板里勾上"开机自启"即可（或托盘右键菜单里开关），原理是往注册表
`HKCU\Software\Microsoft\Windows\CurrentVersion\Run` 写 `BleHRManager` →
`"…\hr-manager.exe" --minimized`。

走 Run 键而不是计划任务：不需要管理员，登录后只出托盘不弹面板并按 ini 拉起 daemon，
daemon 与插件天然同在一个登录会话（`Local\` 命名空间一定对），manager 退出也不连带杀
daemon。旧版本的计划任务（`HuaweiHRDaemon`）已废弃，面板检测到残留会提示，清理跑
`powershell -ExecutionPolicy Bypass -File scripts\uninstall-task.ps1`。想让开机后真正连上
表，先用 hr-manager 选表保存（写 `source.address`），自启实例读的是同一份 ini。

## 从 1.1.2 升级

1.1.3 把全部进程间标识符的前缀从 `HuaWeiHR` 改成了 `BleHR`，包括共享内存
`Local\BleHR_SM`、单实例互斥体、窗口类名、注册表自启值名 `BleHRManager` 和日志目录
`%LOCALAPPDATA%\BleHR`。之后的版本又把文件布局收进了子目录：配置在 `config\`、日志在
`log\`、插件在 `plugins\`，根目录只放 exe 和 README.md。新旧版本的组件互相认不出来，
不能混跑，升级时要把 hr-daemon.exe、hr-manager.exe 和两个插件 DLL 全部换成新版文件，
然后跑一次
`powershell -ExecutionPolicy Bypass -File scripts\migrate-1.1.3.ps1`。

迁移脚本会停掉正在运行的 daemon 和 manager，把注册表自启值改名为 `BleHRManager`，删除
旧版计划任务 `HuaweiHRDaemon` 残留，把日志目录改名为 `BleHR`，并把根目录的
`hr-daemon.ini` 移进 `config\`（原位置留 `.bak`）。**这一步必须跑**：新版程序只认
`config\hr-daemon.ini`，不跑脚本配置会回到默认值（手表地址就丢了）。脚本可以重复执行，
随发行包的 `scripts\` 目录一起附带；全新安装不需要跑它。

## hr-daemon 命令行参数

```
hr-daemon.exe [--demo] [--address AA:BB:CC:DD:EE:FF] [--scan [秒数]] [--debug] [--quiet] [--version] [--help]
```

| 参数 | 作用 |
|---|---|
| `--demo` | 强制用模拟心率源（覆盖配置） |
| `--address` | 强制直连指定手表（覆盖配置） |
| `--scan [秒数]` | 只扫描附近的广播设备，边扫边往 stdout 按行吐 JSON（`device`/`done`），扫完退出 |
| `--debug` | 连每条心率都写进日志 |
| `--quiet` | 不附加控制台，日志只写文件（hr-manager 拉起时用） |
| `--version` | 显示版本号后退出 |

单实例靠互斥体 `Local\BleHR_daemon`，重复启动记一行日志后退出。正常退出（`taskkill`
不带 `/F`、Ctrl+C、注销、关机）会关掉共享内存映射，正在扫描时也能立刻收手。

hr-manager 拉起 / restart 出来的 daemon 都带 `--quiet`，不附加控制台、日志只进文件，
面板关开、manager 退出都不会连带杀掉它；从 cmd 手动跑则附加父控制台、日志同屏，可 Ctrl+C。

## 排查

先看日志 `log\hr-daemon.log`（在 exe 同级的 `log\` 子目录，写完即刷盘，可以边跑边看），
`.1.log` / `.2.log` 是再往前两次的，只留 3 份，里面没有逐条心率值。

| 现象 | 排查 |
|---|---|
| OSD 和任务栏都是 `--`，日志"未配置手表地址，不连接" | 还没选表：面板扫描选表，或 `set address <MAC>` + `restart` |
| 日志反复 "BLE: N 秒后重试" / "找不到设备 …" | 手表没停在心率广播页面 / 没亮屏 / 太远 / 被手机连走。用 `hr-daemon.exe --scan 10` 确认广播在线 |
| 日志 "无法启动扫描 …（蓝牙适配器关了？）" | 电脑蓝牙关了，或适配器被禁用 |
| 日志 "未找到心率服务 0x180D" | 连上了但没有心率服务，手表那边没真正开始广播 |
| 日志反复 "已 N 秒没有收到心率通知，判定连接失效" | 广播断了（常见于手表息屏）。daemon 会自动重连，退避 1→2→4…→30 秒 |
| Afterburner 插件列表里没有 `HeartRate` | DLL 没拷进去 / profile 的 `[Monitoring]` 里没有 `HeartRate.dll=1` / 根配置 `EnablePlugins=0`。重跑部署脚本 |
| 插件在列表里，但曲线列表里找不到 `Heart rate` | 插件被禁用了，或者数据源没在"硬件监控图表列表"里勾上 |
| 曲线一直在动，但 OSD 上不显示 | 该项属性里 `Show in On-Screen Display` 没勾；或前台不是 3D 程序（用 `build\osd_test.exe` 当画布）；或 RTSS 没在跑 |
| 值一直是 `--`，但任务栏正常 | daemon 和 Afterburner 不在同一个会话。用 `tools\mahm-probe\mahm-probe.ps1` 看 MAHM 里那条到底是 `FLT_MAX` 还是有值，能立刻区分"插件没数据"和"OSD 没配" |
| 任务栏没有 `HR` 项 | 见"显示端 → TrafficMonitor" |
| 扫描中途把 hr-manager 关掉/杀掉，之后采集一直是停的 | 重开一次 hr-manager（托盘/面板即可）：它看到 `config\` 里的 `scan-pending` 标记会自动把 hr-daemon 拉回来（标记在扫描停 daemon 前写下、daemon 确认回来后删除） |
| TrafficMonitor 悬浮提示显示"版本不匹配" | 新 daemon 配了旧 hr_plugin.dll：共享内存 version 校验拦住了，两边要一起更新 |
| `hr-manager` 报"既不是 UTF-8 也不是 UTF-16" | 配置文件被存成了别的编码，用记事本另存为 UTF-8，或删掉它让 hr-manager 重建 |
| 中文乱码 | 只在自编译时可能发生：源码是无 BOM UTF-8，各 `build.cmd` 必须带 `/utf-8` |

Afterburner 通常以管理员身份运行（实测普通权限连 `taskkill` 都关不掉它）。这不影响共享
内存：实测 daemon 普通权限、Afterburner 管理员权限，数据照样通。

## 当前限制

- **真机验收进行中**（开发期实测设备为华为手表）：2026-09-17 实测扫描 → 连接 → 订阅 → 读到设备名
  （HUAWEI WATCH HR-05F）已通；心率值进共享内存、息屏/超时断开后的长时间重连还没
  长测。协议按标准 BLE HR Profile 实现（flags bit0 决定 bpm 是 uint8 还是 uint16
  小端），"直连不存在地址会优雅失败并退避"已验证。
- 只暴露一条数据源（`Heart rate`，bpm）。电量（`0x180F`）、RR 间期、连接状态、
  PMDP 数据源、桌面常驻 overlay：明确不做，留二期。
- 数据超过 `timeout_ms` 没更新即视为超时，两处都显示 `--`；插件对 Afterburner 报的是
  MAHM 约定的 `FLT_MAX`（"当前无数据"），界面上显示成什么由 Afterburner 决定。
- 曲线长度不是本项目的参数：Afterburner 自己按 `MonitoringDataBufferSize`（默认 3600
  个采样点）存历史，量程在数据源属性里改。

## 许可证

本仓库自己的代码是 MIT，见 [LICENSE](LICENSE)。有一个第三方组件不在这条许可下，发布前
读一遍 [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)：

| 文件 | 来源 | 许可 |
|---|---|---|
| `tm-plugin/PluginInterface.h` | TrafficMonitor (zhongyang219) | "Anti 996" License v1.0，分发时要原样带上版权声明和许可证全文。不想带上就删掉 `tm-plugin/`，只用 Afterburner 那条链路，它不受影响 |
