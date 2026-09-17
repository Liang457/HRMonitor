# HRMonitor

把华为手表的"心率广播"（标准 BLE Heart Rate Profile，服务 `0x180D`）读进电脑，
再送到两个显示端：MSI Afterburner 的游戏内 OSD，和 TrafficMonitor 的任务栏。

```
                                     ┌─▶ HeartRate.dll (x86，Afterburner OSD)
手表（心率广播） ──BLE──▶ hr-daemon.exe ──┼─▶ hr_plugin.dll (x64，任务栏)
                 连接/重连/扫描      │    └─▶ hr-manager.exe (托盘 + 配置面板)
                                     └─ 写 Local\HuaWeiHR_SM（64 字节共享内存）
                                        配置：hr-daemon.ini（hr-manager 读写）
```

daemon 只负责采集（蓝牙全在它这边：连接/重连/扫描）和写共享内存，读端各自去读、
互不依赖：Afterburner 没开不影响任务栏，TrafficMonitor 没开也不影响 OSD，
hr-manager 没开采集照跑。

`hr-manager.exe` 是管理器：托盘常驻，配置面板（WebView2 渲染的 HTML 单页）按需
弹出、关闭即整体销毁（内存回落）；带 CLI 子命令给脚本用。开机自启、选表扫描、
重启采集、第三方部署入口都在它身上。

第三方依赖方面，BLE 走 Windows SDK 自带的 C++/WinRT（链接 `windowsapp.lib`），
两个插件只依赖 `kernel32`；hr-manager 的 GUI 栈（wry + tao + tray-icon）是全仓库
唯一的第三方 crate，其余 Rust 代码全部手写 WinAPI 声明。

第三方组件的授权情况见 [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)。

## 功能

- **心率采集**：按 `0x180D` 服务扫描附近的广播设备，连上后订阅 `0x2A37` 通知；
  断开或超时会自动重连，退避从 1 秒翻倍到 30 秒
- **两个显示端**：Afterburner 里是一条名为 `Heart rate` 的原生监控数据源
  （bpm），TrafficMonitor 里是任务栏上的一项 `HR 128`
- **管理器 hr-manager**：托盘常驻 + HTML 配置面板——实时心率大字显示、
  点一下扫描选表（自动停/启 daemon）、改配置、开关机自启、调部署脚本；
  同一个 exe 带完整 CLI 子命令（给脚本/CI）
- **模拟源**：`demo=1` 时用心率随机游走代替手表，构建完不用有表也能验通整条链路
- **设备区分**：数据源按 MAC 直连或自动挑选，切换手表只改一行配置

## 目录结构

```
HRMonitor\
├─ README.md
├─ LICENSE                     本仓库自己的代码：MIT
├─ THIRD_PARTY_NOTICES.md      第三方组件授权说明
├─ common\
│  ├─ hr_shared.h              共享内存协议（daemon 与两个插件共用的唯一定义）
│  └─ hr_config.h/.cpp         INI 读写 + 配置结构 + 路径/MAC 工具
├─ daemon\                     hr-daemon.exe（C++，x64）：采集 + 写共享内存
├─ ab-plugin\                  HeartRate.dll（C++，x86）：Afterburner 监控数据源
├─ tm-plugin\                  hr_plugin.dll（C++，x64）：TrafficMonitor 插件
├─ tools\
│  ├─ hr-manager\              hr-manager.exe（Rust + wry）：托盘/配置面板/CLI
│  ├─ mahm-probe\              读 Afterburner 共享内存的小脚本，排查用
│  └─ osd_test\                osd_test.exe（C++）：D3D11 清屏窗口，验证 OSD 用
├─ scripts\                    TrafficMonitor / Afterburner 部署脚本 + 旧自启清理
└─ build\                      构建产物（不入库）
```

## 构建

需要 MSVC（VS 2019/2022/2026 的 Build Tools 都行，勾上 C++ 生成工具，**并且要包含
x86 目标**），以及 Rust 工具链（只有 hr-manager 需要，[rustup.rs](https://rustup.rs)
默认选项即可）。

```cmd
cmd /c daemon\build.cmd           :: -> build\hr-daemon.exe   (C++, x64)
cmd /c ab-plugin\build.cmd        :: -> build\HeartRate.dll   (C++, x86, 需要 x86 工具集)
cmd /c tm-plugin\build.cmd        :: -> build\hr_plugin.dll   (C++, x64)
cmd /c tools\osd_test\build.cmd   :: -> build\osd_test.exe    (C++, x64)
cmd /c tools\hr-manager\build.cmd :: -> build\hr-manager.exe  (Rust + WebView2 GUI)
```

hr-manager 首次构建需要联网拉依赖（wry/tao/tray-icon，Cargo.lock 已锁版本），
之后可离线重建；仓库里其余部分构建不依赖网络。面板用 WebView2 运行时渲染
（Win10/11 系统自带，Evergreen），无需额外安装。

各 `.cmd` 都用 `vswhere` 自动找 MSVC，不绑死 VS 版本或安装路径。C++ 产物都是
`/MT` 静态 CRT，目标机器不用装 VC++ 运行库。

**发行版由 [GitHub Actions](.github/workflows/release.yml) 自动构建**：每次 push 到
main 都会构建全部产物（zip 在对应运行页面的 Artifacts 里下载）；打 `v*` 标签推送时
还会自动创建 GitHub Release 并附上完整发行包。

**`ab-plugin` 必须是 x86**：`MSIAfterburner.exe` 本身是 32 位进程（PE machine
`0x14C`），加载不了 x64 DLL。daemon 是 x64 也没关系，因为两边只通过共享内存交换
数据，按字节布局，和位数无关。`ab-plugin\build.cmd` 会自己用 `vcvarsall.bat x86`
初始化，并在构建末尾用 `dumpbin` 自查"是不是 x86"和"三个导出名有没有被修饰"。

**三个程序要一起重新构建**：共享内存的记录里带 `version` 字段，读者会校验它。
只换 daemon 不换插件（或反过来）会导致读不到数据。

`tm-plugin\PluginInterface.h` 取自 TrafficMonitor V1.86，用的是"反996许可证"，
发布前请看 [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)。

## 配置

配置全在 `hr-daemon.ini`，和 `hr-daemon.exe` 同目录，UTF-8、带注释，可以直接手改。
改配置的日常入口是 **hr-manager 的面板**（托盘图标 → 打开面板）：实时心率大字、
扫描选表、全部配置项、启停 daemon、开机自启、部署脚本按钮都在一页里。

不想动 GUI 也可以用同一个 exe 的 CLI 子命令（GUI 是主交互，CLI 给脚本兜底）：

```cmd
build\hr-manager.exe show            :: 列出当前配置
build\hr-manager.exe get demo        :: 读一项
build\hr-manager.exe set demo 1      :: 改一项（只写文件，重启后生效）
build\hr-manager.exe reset [-y]      :: 恢复默认
build\hr-manager.exe status          :: daemon 状态 + 共享内存实时值
build\hr-manager.exe start / stop    :: 启动 / 停止 daemon
build\hr-manager.exe restart         :: 重启 daemon 让改动生效
build\hr-manager.exe scan [秒数]     :: 扫描附近的心率广播设备
build\hr-manager.exe path            :: 显示各文件位置
build\hr-manager.exe help            :: 全部命令
build\hr-manager.exe                 :: 不带参数 = 打开面板（--minimized 只出托盘）
```

保存时会先把原文件备份成 `hr-daemon.ini.bak`，再写临时文件、原子替换，写到一半不会
留下半份配置。如果 `hr-daemon.ini` 存在但读不出来（被编辑器锁着、存成了别的编码），
hr-manager 会直接报错退出，**不会**退回默认值——否则你下一次保存就会用默认值把真实
配置覆盖掉。

**扫描选表**（面板"选择手表"卡片，或 CLI `hr-manager scan`）：手表需停在**"心率广播"
页面并保持亮屏**。手表**被 daemon 连着的时候通常就不再广播了**，所以扫描会自动先停掉
daemon、等 2 秒让手表恢复广播，扫完选定后自动保存并重启 daemon；不选就点"放弃"恢复
采集。扫描边扫边刷新列表（10 秒），赶时间的设备可能漏掉，扫不到就再扫一次。扫描是借
`hr-daemon.exe --scan` 做的（daemon 是唯一的蓝牙入口），hr-manager 自己不碰蓝牙。

### hr-daemon.ini

| 键 | 默认 | 说明 |
|---|---|---|
| `demo` | 否 | 换成模拟心率源（60~180 随机游走），不需要手表 |
| `address` | 空 | 直连的手表 MAC；**留空则不连接**——daemon 不会自己扫一台连上（多设备环境会连错表），必须用 hr-manager 选表或手填明确指定 |
| `scan_timeout_ms` | 20000 | `--scan` 不带秒数参数时的默认扫描时长（毫秒） |
| `backoff_min_sec` / `backoff_max_sec` | 1 / 30 | 重连退避的上下限 |
| `timeout_ms` | 15000 | 多久没数据就显示 `--`（毫秒） |
| `refresh_ms` | 1000 | 采样/推送周期（毫秒） |
| `max_kb` | 4096 | 单个日志文件的上限（KB），写满就轮转 |
| `debug` | 否 | `是` = 连每条心率都写进日志（排查用，平时别开） |
| `tm_dir` | 空 | TrafficMonitor 安装目录（面板的部署按钮会传给脚本） |

`timeout_ms` 是同一个数被三处使用：daemon 判超时、BLE 层判定连接失效（超过这么久
没收到通知就断开重连）、插件在 daemon 消失时的兜底。

**日志不会无限长**：每次启动开新的一份 `hr-daemon.log`，上一份顶成
`hr-daemon.1.log`、再上一份顶成 `.2.log`，更老的删掉，磁盘上最多留 3 份（也就是
最近三次运行）。单份超过 `max_kb` 时会在运行中就地轮转，所以总量最多 3 × `max_kb`。

**心率值默认不写进日志**：手表心率几乎每秒都在变，全写下来一天能涨好几 MB。平时
日志里只有状态变化（连上/断开/超时）和错误。想看每条心率就把 `debug` 打开
（`hr-manager set debug 1` + 重启，或直接 `hr-daemon.exe --demo --debug` 跑前台），
这些行带 `[DEBG]` 标记，方便事后过滤。

### hr_plugin.ini

放在插件 DLL 旁边，也就是 `<TrafficMonitor>\plugins\`：

```ini
[plugin]
label=HR          ; 数值前面显示的标签
name=心率          ; 在 TrafficMonitor"显示设置"里的名字
sample=188        ; 示例值，决定显示区宽度（用最宽的情况）
timeout_ms=15000  ; daemon 挂掉时插件自己的兜底超时
```

改完要重启 TrafficMonitor 才生效，文件不存在就用上面这些默认值。

## 不用手表的快速验证

```cmd
:: 1. 换成模拟心率源并重启 daemon
build\hr-manager.exe set demo 1
build\hr-manager.exe restart

:: 2. 看数据到底出去没有（不需要 GUI，也不需要 RTSS）
pwsh -File tools\mahm-probe\mahm-probe.ps1 -Filter Heart
```

第 2 步会打印 Afterburner 共享内存里那条 `Heart rate` 的名字、单位、当前值和标志位：

```
signature=0x4D41484D  version=0x00020000  headerSize=32  entries=156  entrySize=1324

[155] Heart rate
      units='BPM'  format='%.0f'  value=137  flags=OSD/-/-  gpu=0x0  srcId=0xFF
```

`value=` 跟着模拟心率一直在变，就说明整条链路是通的。前提是 Afterburner 正在运行
且插件已启用。daemon 停了 15 秒之后这一项会变成 `UNAVAILABLE (FLT_MAX)`，这是
MAHM 约定的"当前没有数据"。

## 手表设置

1. 手表和手机"运动健康"App 配对好。
2. 在运动健康里开启**心率广播**（不同机型路径略有差异，一般在设备 → 健康监测/心率
   里；有些机型是锻炼界面里的"广播心率"）。
3. 手表停在**心率广播页面并保持亮屏**，离开该页面广播就停了。
4. 选定手表并起 daemon。双击 `build\hr-manager.exe` 打开面板，点**开始扫描**选出你的
   手表（会自动停启 daemon、保存并重启）；或者命令行一把梭（MAC 可先用 `scan` 确认，
   见下）：

```cmd
build\hr-manager.exe set demo 0
build\hr-manager.exe set address AA:BB:CC:DD:EE:FF
build\hr-manager.exe restart
```

日志里出现 `模式: 直连配置里的地址 ...` → `BLE: 已连接 ...，已订阅心率通知` 就成了。
**地址留空 daemon 不会连接**（它不会自己扫一台连上，避免连错设备），任务栏和 OSD 会
一直显示 `--`，日志里提示"未配置手表地址"。

想先确认广播在线，可以用 `build\hr-manager.exe scan 10`：扫 10 秒，边扫边把发现的
设备打到屏幕上。华为手表在广播里常常不带名字，列表里那台会先显示"（还没拿到名字）"，
认 MAC 就行。

## TrafficMonitor（任务栏）

去 GitHub Releases 下载 `TrafficMonitor_V1.86_x64_Lite.zip`，解压到比如
`D:\Program Files\TrafficMonitor`，然后：

```powershell
Copy-Item build\hr_plugin.dll "D:\Program Files\TrafficMonitor\plugins\hr_plugin.dll"
powershell -ExecutionPolicy Bypass -File scripts\configure-trafficmonitor.ps1
```

脚本会关掉 TrafficMonitor、把 `hr` 加进 `plugin_display_item`、写上
`show_task_bar_wnd=true`，再把它启动起来。它**不传 `-TmDir` 时**会依次尝试：
`hr-daemon.ini` 里的 `integration.tm_dir` → 几个常见安装位置，都不行就报错让你用
`-TmDir` 指定。改 `config.ini` 之前会先备份成 `config.ini.bak`，并按原编码原子写回。

插件文件名必须是 `.dll`，不能是 `.tmd`：V1.86 只扫描 `plugins\*.dll`
（`PluginManager.cpp:41`），源码里根本没有 `.tmd` 这个字符串，那是更老版本插件系统
的扩展名。放 `.tmd` 进去"插件管理"里会是空的，且没有任何报错。另外插件默认就是启用
的，`config.ini` 里的 `plugin_disabled` 是黑名单（默认空），不需要去"插件管理"里点
启用。

任务栏窗口没出现的话，`show_task_bar_wnd` 写进 `config.ini` 不一定生效
（TrafficMonitor 运行时会用自己的内存值覆盖回去）。可靠做法是右键**通知区图标** →
**显示任务栏窗口**，开一次之后就会被记住。任务栏上没有 `HR` 这一项，则右键任务栏上
的 TrafficMonitor 区域 → **显示设置** → 勾上"心率"，顺序可以拖动。

## MSI Afterburner（游戏内 OSD）

**Afterburner 的游戏内 OSD 是 RTSS 画的**，Afterburner 自己不会画覆盖层。本项目不直接
碰 RTSS，而是把自己做成 Afterburner 的一条原生监控数据源，于是心率就和 GPU 温度、
CPU 占用、帧率并列在同一条 OSD 里，颜色、字号、量程、显示位置、是否进 OSD/托盘/LCD
全都在 Afterburner 一个界面里配。RTSS 仍然作为 Afterburner 的渲染器存在，但本项目与
它零耦合：不写它的共享内存、不引用它的头文件、不关心它在不在跑。

部署：

```powershell
cmd /c ab-plugin\build.cmd
powershell -ExecutionPolicy Bypass -File scripts\deploy-afterburner-plugin.ps1
```

脚本做四件事：把 `HeartRate.dll` 拷进 `<Afterburner>\Plugins\Monitoring\`、写一份插件
说明到 `<Afterburner>\Help\Plugins\Monitoring\HeartRate`、在
`Profiles\MSIAfterburner.cfg` 的 `[Monitoring]` 段里写 `HeartRate.dll=1`、确认根配置
`EnablePlugins=1`。全程先关 Afterburner 再改文件（它退出时会把内存里的配置写回
profile），只插入/删除那一行，profile 其余字节原样不动，并留一份 `.bak`。
卸载加 `-Uninstall`。

脚本**不**去改 `Sources=` / `[Source ...]` 这些段，那是 Afterburner 自己的数据结构，
硬改容易被它覆盖。启用插件后在界面里点两下：

1. **设置 → 监控** → 在"硬件监控图表列表"里找到并勾上 **Heart rate**。勾上之后
   Afterburner 的监控窗口就会实时画这条曲线（这一步不需要 RTSS，也不需要有 3D 程序
   在跑，所以数据链路可以脱离 OSD 单独验证）。
2. 选中 `Heart rate`，点右边的**设置**按钮 → 勾上 **Show in On-Screen Display**。

想改 OSD 上的样子，都在同一个属性窗口里：

| 想改什么 | 在哪里改 |
|---|---|
| 把显示名缩短成 `HR` | `Override graph name` |
| 曲线上下限（默认 40~180） | `Min limit` / `Max limit` |
| 颜色 | `Graph color`（OSD 上的分组配色由 Afterburner 的 OSD 布局决定） |
| 数值格式 / 单位 | `Override graph name` 旁边的相关项；单位默认 `BPM` |
| 进不进 OSD / 托盘 / 键盘 LCD | `Show in On-Screen Display` / `Show in Tray` / `Show in LCD` |
| OSD 整体位置、字号、配色 | **设置 → On-Screen Display** 里的布局编辑器 |

RTSS 只在自己 hook 到的 3D 程序上画 OSD，所以要确认 OSD 真的画出来了，需要一个
"画布"程序：跑 `build\osd_test.exe`，窗口左上角应该出现心率那一行（数值跟着手表或
模拟源变）。这一步才需要 RTSS 在跑；它不跑也不影响上面第 1 步的曲线和任务栏。

脚本按"目录能不能写"判断要不要管理员，所以 Afterburner 目录本来就允许写入的机器上
不需要提权。但 Afterburner 一般以管理员运行，而它启动时会加载该目录下所有 DLL，
别把这个目录的写权限开放给不受信任的账户。

## 开机自启

面板里勾上**"开机自启"**即可（或托盘右键菜单里开关）。原理是往注册表
`HKCU\Software\Microsoft\Windows\CurrentVersion\Run` 写一个值
（`HuaWeiHRManager` → `"…\hr-manager.exe" --minimized`）：

- 不需要管理员，不需要计划任务；
- 登录后 manager 只出托盘不弹面板，并按 ini 拉起/看护 daemon；
- daemon 与插件天然同在一个登录会话，共享内存 `Local\` 命名空间一定对；
- manager 退出不连带杀 daemon，采集不中断。

旧版本用计划任务（`HuaweiHRDaemon`）自启，已废弃；面板检测到残留会提示，
清理跑 `powershell -ExecutionPolicy Bypass -File scripts\uninstall-task.ps1`。

daemon 现在**地址留空就不连接**：想让开机后真正连上表，先用 hr-manager 选表保存
（把 `source.address` 写进 ini）即可，自启实例读的就是同一份 ini。

## hr-daemon 命令行参数

```
hr-daemon.exe                        读 hr-daemon.ini，按配置采集
hr-daemon.exe --demo                 强制用模拟心率源（覆盖配置）
hr-daemon.exe --address AA:BB:CC:DD:EE:FF
                                     强制直连指定手表（覆盖配置）
hr-daemon.exe --scan [秒数]
                                     只扫描附近的心率广播设备，边扫边往 stdout
                                     按行输出 JSON（device/done），扫完退出
hr-daemon.exe --debug                连每条心率都写进日志
hr-daemon.exe --quiet                不附加控制台，日志只写文件（hr-manager 拉起时用）
hr-daemon.exe --help
```

单实例靠互斥体 `Local\HuaWeiHR_daemon`，重复启动会记一行日志后退出。正常退出
（`taskkill` 不带 `/F`、Ctrl+C、注销、关机）会关掉共享内存映射；正在扫描时也能立刻
收手，不会卡在扫描循环里。

`hr-manager` 拉起 / restart 出来的 daemon 都带 `--quiet`：不附加任何控制台，日志只进
文件，面板关开、manager 退出都不会连带杀掉它。从 cmd 手动跑则保持原行为：附加父
控制台、日志同屏，可 Ctrl+C 退出。

## 排查

先看日志 `build\hr-daemon.log`（写完即刷盘，可以边跑边看）。这是**最近一次**运行的
日志，`hr-daemon.1.log` / `.2.log` 是再往前两次的（只留 3 份）。日志里没有逐条的心率
值，那是 `[DEBG]`，默认不写。

| 现象 | 排查 |
|---|---|
| OSD 和任务栏都是 `--`，日志"未配置手表地址，不连接" | 还没选表：hr-manager 面板扫描选表（或 `set address <MAC>` + `restart`） |
| 日志反复 "BLE: N 秒后重试" / "找不到设备 …" | 手表没停在心率广播页面 / 没亮屏 / 太远 / 被手机连走。用 `hr-daemon.exe --scan 10` 确认广播在线 |
| 日志 "无法启动扫描 …（蓝牙适配器关了？）" | 电脑蓝牙关了，或适配器被禁用 |
| 日志 "未找到心率服务 0x180D" | 连上了但没有心率服务，手表那边没真正开始广播 |
| 日志反复 "已 N 秒没有收到心率通知，判定连接失效" | 广播断了（常见于手表息屏）。daemon 会自动重连，退避 1→2→4…→30 秒 |
| Afterburner 插件列表里没有 `HeartRate` | ① DLL 没拷进去；② profile 的 `[Monitoring]` 里没有 `HeartRate.dll=1`；③ 根配置 `EnablePlugins=0`。重跑部署脚本 |
| 插件在列表里，但曲线列表里找不到 `Heart rate` | 插件被禁用了，或者数据源没在"硬件监控图表列表"里勾上 |
| 曲线一直在动，但 OSD 上不显示 | 该项属性里 `Show in On-Screen Display` 没勾；或前台不是 3D 程序（用 `build\osd_test.exe` 当画布）；或 RTSS 没在跑 |
| 值一直是 `--`，但任务栏正常 | daemon 和 Afterburner 不在同一个会话（见"开机自启"）。用 `tools\mahm-probe\mahm-probe.ps1` 看 MAHM 里那条到底是 `FLT_MAX` 还是有值，能立刻区分"插件没数据"和"OSD 没配" |
| 任务栏没有 `HR` 项 | 见 "TrafficMonitor" 一节 |
| `hr-manager` 报"既不是 UTF-8 也不是 UTF-16" | 配置文件被存成了别的编码，用记事本另存为 UTF-8，或删掉它让 hr-manager 重建 |
| 中文乱码 | 只在自编译时可能发生：源码是无 BOM UTF-8，各 `build.cmd` 必须带 `/utf-8` |

Afterburner 通常以管理员身份运行（实测普通权限连 `taskkill` 都关不掉它）。这不影响
共享内存：实测 daemon 普通权限、Afterburner 管理员权限，数据照样通。

## 当前限制

- **真机（华为手表）验收进行中**：2026-09-17 实测扫描 → 连接 → 订阅 → 读到设备名
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

本仓库自己的代码是 MIT，见 [LICENSE](LICENSE)。

有一个第三方组件不在这条许可下，发布前请读一遍
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)：

| 文件 | 来源 | 许可 |
|---|---|---|
| `tm-plugin/PluginInterface.h` | TrafficMonitor (zhongyang219) | "Anti 996" License v1.0，分发时要原样带上版权声明和许可证全文。不想带上就删掉 `tm-plugin/`，只用 Afterburner 那条链路，它不受影响 |

早先版本在 `third_party/rtss/` 里带的两份 RTSS SDK 头文件没有任何许可证声明，
现在 daemon 不再碰 RTSS，那两份文件已经删除。
