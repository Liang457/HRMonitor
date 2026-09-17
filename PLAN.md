# 华为手表心率 → Afterburner OSD + TrafficMonitor 任务栏：架构与实施计划

> 状态：**已实现并验收通过**（2026-09-16）。本文档自包含，新会话可直接照此理解项目。
> 定稿日期：2026-09-16；实现完成日期：2026-09-16；OSD 迁移到 Afterburner 插件：2026-09-16。
>
> **实现期间对本文档的两处修正**（详见文末 §10）：
> 1. TrafficMonitor 插件产物是 **`hr_plugin.dll`**，不是 `.tmd` —— V1.86 只扫描
>    `plugins\*.dll`，源码里根本没有 `.tmd` 这个字符串。
> 2. `show_task_bar_wnd` 直接改 config.ini **不一定生效**，可靠做法是右键通知区图标
>    →"显示任务栏窗口"。
>
> **第三轮改动（§13）已经取代本文档原有的 OSD 方案**：daemon 不再直写 RTSS 共享内存，
> 改成给 MSI Afterburner 写一个监控插件（`ab-plugin/HeartRate.dll`）。
> 阅读时以下小节请以 §13 为准：§0 目标第 2 条、§3 架构图、§4.2（RTSS slot 协议，已废弃）、
> §7.2、§12.1 与 §12.4 里涉及 OSD 配置和 RTSS 授权的部分、§12.6。

## 0. 目标

把华为手表的"心率广播"（BLE）采集回电脑，同时显示到：

1. **TrafficMonitor**（任务栏，插件形式显示数值）
2. **MSI Afterburner**（游戏内 OSD：一条 `Heart rate` 监控数据源 / 曲线）

> 第 2 条原本是"直写 RTSS 共享内存"，见 §13 已改为 Afterburner 插件。

## 1. 已核实的环境事实（2026-09-16 在本机逐项验证）

| 事项 | 结论 |
|---|---|
| 华为心率广播协议 | 标准 BLE Heart Rate Profile：服务 `0000180d-…`，测量特征 `00002a37-…`（notify）。flags 位0=0 为 uint8 bpm，=1 为 uint16 小端。参考实现是一对 bleak 脚本（扫描 / 连接订阅），**不在本仓库内**，仅作协议参考；真机地址形如 `AA:BB:CC:DD:EE:FF` |
| TrafficMonitor | 从 GitHub zhongyang219/TrafficMonitor 的 Releases 下载 `TrafficMonitor_V1.86_x64_Lite.zip`（Lite 1.86 **支持插件**，官方自 1.86 起只发 Lite 版）；插件是导出 `TMPluginGetInstance()` 的 C++ **DLL**，放进 exe 同级 `plugins\` 子目录即自动加载（**扩展名必须是 `.dll`，不是 `.tmd`**，见 §10.1）；接口头文件取自该仓库 V1.86 tag 的 `include\PluginInterface.h` |
| RTSS | 已装 `D:\Program Files\RivaTuner Statistics Server`（RTSS.exe 32 位 PE32，2025-09 构建，共享内存 v2.2x 全特性可用）。**无需写 RTSS 插件**：第三方程序直写命名共享内存 `RTSSSharedMemoryV2` 的 OSD slot 即可上屏；官方插件 SDK 不公开发放，此路线完全绕开 |
| RTSS SDK | `D:\Program Files\RivaTuner Statistics Server\SDK\`：`Include\RTSSSharedMemory.h`（v2.21 完整结构）+ `Samples\SharedMemory\RTSSSharedMemorySample\RTSSSharedMemorySampleDlg.cpp`（998 行，slot 协议/格式标签/内嵌曲线全部可照抄，关键段：258–282 取版本、284–375 嵌曲线、377–456 更新文本、458+ 释放、630–851 文本拼装） |
| 编译工具链 | VS18 BuildTools：`C:\Program Files (x86)\Microsoft Visual Studio\18\BuildTools\VC\Tools\MSVC\14.51.36231\`；Windows SDK `10.0.26100.0`（含 C++/WinRT 头 `Include\10.0.26100.0\cppwinrt\`），链接 `windowsapp.lib` 即可用 WinRT BLE，零第三方依赖 |
| Rust | ~/.cargo 全套存在（本次不使用，全部 C++） |

## 2. 已定案决策（用户拍板）

- 全部 C++（daemon 用 C++/WinRT；TrafficMonitor 插件本就必须 MSVC C++）。
- RTSS OSD 显示：`HR 128 bpm` 文本（红色）+ 60 秒历史曲线（RTSS 内嵌 graph 对象）；**不用 emoji**。
- 断连/数据超时（>15 秒无更新）：显示 `HR --`、曲线清零（任务栏同样 `--`）。
- daemon **先不**开机自启，但保留 `install-task.ps1`，想装时一键安装。
- daemon 提供 `--demo` 模拟心率源，无表也能全链路联调。

## 3. 架构

```
华为手表（运动健康 App 开"心率广播"，≈1Hz GATT 通知）
        │ BLE 0x180D / 0x2A37
        ▼
hr-daemon.exe（C++/WinRT 常驻，/SUBSYSTEM:WINDOWS 静默，写日志）
  ├─ 扫描(0x180D 过滤)→连接→订阅→断线指数退避重连(1s→30s 封顶)
  ├─ 单实例互斥体 Local\HuaWeiHR_daemon
  ├─ --demo 模式：60~180 随机游走，1Hz
  │
  └─写─► HuaWeiHR_SM（64B 命名共享内存）
            │
            ├─读─► ab-plugin\HeartRate.dll（C++ x86 /MT，Afterburner 监控插件）
            │        GetSourceData() 每个轮询周期读一次
            │        ▼
            │      MSI Afterburner 监控曲线 / OSD / 托盘 / 键盘 LCD
            │      （Afterburner 的游戏内 OSD 由 RTSS 渲染，但本项目不直接碰 RTSS）
            │
            └─读─► tm-plugin\hr_plugin.dll（C++ x64 /MT，TrafficMonitor 插件）
                     DataRequired() 读 SM
                     ▼
                   TrafficMonitor，任务栏显示 "HR 128" / "--"
```

两个显示端互相独立：TrafficMonitor 没开不影响 OSD，反之亦然。
daemon 不再写 `RTSSSharedMemoryV2`，也不再需要 RTSS SDK 头文件（见 §13）。

## 4. 协议细节（实现时的权威依据）

### 4.1 中立共享内存 `HuaWeiHR_SM`

```cpp
#pragma pack(push, 4)
struct HrSharedData {          // 64 字节定长
    DWORD     magic;           // 'HRSM' = 0x4D535248
    DWORD     version;         // 1
    LONG      bpm;             // -1 = 无效
    ULONGLONG tick_ms;         // GetTickCount64() 采样时刻
    DWORD     status;          // 0=无数据 1=正常 2=连接中 3=超时
    DWORD     battery;         // 保留（未来读 0x180F）
    char      device_name[40]; // 手表名
};
#pragma pack(pop)
```
daemon：`CreateFileMappingW(INVALID_HANDLE_VALUE, …, PAGE_READWRITE, 0, 64, L"Local\\HuaWeiHR_SM")`，变化时 + 每秒心跳各写一次。
插件：`OpenFileMappingW(FILE_MAP_READ, …)` 读；tick_ms 距今 >15s 按"超时"显示。

### 4.2 RTSS OSD slot 写入（照抄官方示例协议）—— **已废弃，见 §13**

1. `OpenFileMappingA(FILE_MAP_ALL_ACCESS, FALSE, "RTSSSharedMemoryV2")` + `MapViewOfFile(…, 0,0,0)`；校验 `dwSignature=='RTSS' && dwVersion>=0x00020000`。
2. 选 slot：`for dwEntry=1 .. dwOSDArrSize-1`，entry 地址 = `(LPBYTE)pMem + dwOSDArrOffset + dwEntry*dwOSDEntrySize`；两遍扫描——先找 `szOSDOwner=="HuaweiHR"`，再找 owner 为空并写入 `HuaweiHR`。
3. 文本写入 `szOSDEx`（v2.7+，4096 字节）。格式标签示例：
   ```
   <C0=FF6060><C1=FFFFFF>HR <A0>128<A><C1><S1> bpm<S><C>\n<C0><OBJ=00000000><C>
   ```
   （`<Cn=color>` 定义色变量、`<Cn>…<C>` 应用/恢复、`<A0=-5>` 对齐变量、`<S1=50>` 字号变量、`\n` 换行；标签需 v2.11+）
4. 内嵌曲线（v2.12+）：把 `RTSS_EMBEDDED_OBJECT_GRAPH` 写进 `pEntry->buffer` 偏移 0 处：
   `header.dwSignature='GR00'`，`dwSize=sizeof(RTSS_EMBEDDED_OBJECT_GRAPH)+count*4`，`dwWidth=-32, dwHeight=-2`（负数=字符单位），`dwMargin=1`，`fltMin=40, fltMax=180`，`dwFlags=RTSS_EMBEDDED_OBJECT_GRAPH_FLAG_FILLED`，`fltData[]`=64 点环形缓冲（采样 1Hz ≈ 最近 60 秒）。文本中用 `<OBJ=%08X>`（8 位十六进制 buffer 偏移）引用。
5. 提交：v2.14+ 先 `if (!_interlockedbittestandset(&pMem->dwBusy,0)) { 写文本; pMem->dwBusy=0; }`（busy 时本轮跳过），然后 `pMem->dwOSDFrame++` 触发全局刷新。
6. 释放/退出：清空 `szOSDEx`、`szOSDOwner` 置空、`dwOSDFrame++`。
7. 超时：bpm 无效时文本换 `HR --`，曲线数据清零。注意 daemon/64 位进程写 32 位 RTSS 的共享内存没有位数问题（共享内存按字节布局）。

### 4.3 TrafficMonitor 插件（hr_plugin.dll）

- `extern "C" __declspec(dllexport) ITMPlugin* TMPluginGetInstance();` 返回静态对象。
- `ITMPlugin`：实现 `GetItem(int)`（0 号返回唯一 IPluginItem，1 号返回 nullptr）、`DataRequired()`（此处读 `HuaWeiHR_SM`，缓存到成员）、`GetInfo(PluginInfoIndex)`（名称/描述/作者）、可选 `ShowOptionsDialog` 不做。
- `IPluginItem`：`GetItemId()=L"hr"`、`GetItemName()=L"心率"`、`GetItemLableText()=L"HR"`、`GetItemValueText()` 只做格式化（返回 `L"128"` 或 `L"--"`，**不得**在这里取数据）、`GetItemValueSampleText()=L"128"`、`IsCustomDraw()=false`。
- 构建：cl /LD /O2 /MT /EHsc /std:c++20 /utf-8 /DUNICODE /D_UNICODE plugin.cpp → `hr_plugin.dll` 放 TrafficMonitor 同级 `plugins\`（**必须 .dll 扩展名**，见 §10）。
- `PluginInterface.h` 获取：`https://github.com/zhongyang219/TrafficMonitor/tree/V1.86.1/include`（实现时以实际最新 1.86.x tag 为准），开发指南 wiki：《插件开发指南》。

### 4.4 BLE（C++/WinRT，SDK 26100 头文件）

`BluetoothLEAdvertisementWatcher`（`AdvertisementFilter.ServiceUuids.Append(GattServiceUuids::HeartRate())`）扫描 → `BluetoothLEDevice::FromBluetoothAddressAsync` → `GetGattServicesForUuidAsync(HeartRate)` → `GetCharacteristicsForUuidAsync(HeartRateMeasurement)` → `WriteClientDescriptorAsync(ClientCharacteristicConfigurationDescriptorValue::Notify)` → `ValueChanged` 事件按 flags 解析 bpm。断开（`ConnectionStatusChanged`/异常）→ 退避重扫重连。线程 `RoInitialize`。链接 `windowsapp.lib`。可加 `--address AA:BB:CC:DD:EE:FF` 参数跳过扫描直连。

## 5. 目录结构与产物

```
<仓库根目录>\
├─ PLAN.md                  # 本文档
├─ common\hr_shared.h       # 4.1 结构体 + 打开/读写辅助（daemon 与插件共用）
├─ daemon\
│  ├─ main.cpp  ble.cpp  rtss_osd.cpp  demo.cpp  log.cpp
│  └─ build.cmd             # cl.exe (Hostx64\x64) /std:c++20 /O2 /MT，链 windowsapp.lib
├─ tm-plugin\
│  ├─ plugin.cpp  PluginInterface.h
│  └─ build.cmd             # → hr_plugin.dll
├─ tools\osd_test\          # ~200 行 D3D11 清屏窗口，供 RTSS 挂钩验证 OSD（免开真游戏）
├─ scripts\
│  ├─ run-daemon.cmd        # 手动启动
│  ├─ install-task.ps1      # 计划任务自启（保留，默认不执行）
│  └─ uninstall-task.ps1
└─ README.md                # 实现完成后写：使用说明+手表开广播指引+排查
```

产物：`hr-daemon.exe`（`--demo`/`--address`）、`hr_plugin.dll`、`osd_test.exe`。

## 6. 分步实施清单

1. `common\hr_shared.h`
2. daemon：日志 → demo 源 → RTSS slot 写入器（先用 demo 数据）→ `HuaWeiHR_SM` 写入 → BLE 采集/重连（最后接真源）→ 单实例互斥 → 命令行参数
3. osd_test D3D11 窗口
4. TrafficMonitor 插件（下载 PluginInterface.h → 编 DLL → hr_plugin.dll）
5. 部署 TrafficMonitor：解压 zip → `D:\Program Files\TrafficMonitor`，拷 `hr_plugin.dll` 进 `plugins\`（默认即启用）
6. 联调（见 §7）
7. README

## 7. 测试计划（无表可做 §7.1–7.2）

**7.1 demo 全链路（现在就能做）**
- `hr-daemon.exe --demo` → `osd_test.exe`：窗口内出现红色 `HR xxx bpm` + 滚动曲线
- 开 TrafficMonitor：任务栏显示模拟心率
- `taskkill` daemon：两处 15 秒内变 `--`；重启 daemon 恢复

**7.2 RTSS 协议冒烟**（**已废弃，见 §13**）：无 RTSS 时 daemon 不崩、日志提示重试；RTSS 运行时用 osd_test 验证；确认 Afterburner 的 OSD 与我们的 slot 并存不冲突

**7.3 真机验收（回家有表后）**
1. 手表/运动健康开启"心率广播"并保持
2. 先跑 `else\tmp\11\2.py` 复核广播在线（可选）
3. `hr-daemon.exe`（无参数）→ 日志确认连接 → OSD/任务栏数值与手表一致
4. 手表离开范围 15s：显示 `--`；回来自动恢复（验证重连）
5. 重启机器：手动起 daemon 一切正常

## 8. 风险与对策

- 华为广播息屏/超时断开行为未真机验证 → 指数退避重连兜底，README 写排查步骤
- RTSS 未运行 → 打不开映射每 5s 重试，其余功能不受影响
- daemon 崩溃会在 OSD 残留最后一帧（共享内存注入的共性限制）→ 正常退出路径清理；异常残留重启 daemon/RTSS 即清
- TrafficMonitor 首次要手动启用插件一次
- 任务栏/OSD 都不依赖 emoji（字体兼容性差），纯文本 + 曲线

## 9. 明确不做（留作二期）

电量显示（0x180F）、RR 间期、PMDP 数据源、桌面常驻 overlay（DesktopOverlayHost）、Rust 重写。

## 10. 实现期间对计划的修正（2026-09-16 实测）

### 10.1 插件产物必须是 `hr_plugin.dll`，不是 `.tmd`
原计划 §1/§4.3/§5 认为 TrafficMonitor 插件是"导出 `TMPluginGetInstance()` 的 C++ DLL，
改名 `.tmd`"。前一半对，后一半错：

- `TrafficMonitor/PluginManager.cpp:41` 只扫 `plugins\*.dll`：
  `CCommon::GetFiles((plugin_dir + L"\*.dll").c_str(), plugin_files);`
- 全仓库 grep `tmd` **零命中**（V1.86 源码里不存在这个扩展名）。
- 官方 README 原文："插件dll必须放在'TrafficMonitor.exe'同级目录的'plugins'目录下。"

实测：放 `hr_plugin.tmd` 时"插件管理"里列表**全空**；改名 `.dll` 后立即出现，
且 TrafficMonitor 进程的模块列表里能看到 `plugins\hr_plugin.dll`。

### 10.2 插件不需要"启用"，只需要登记显示项目
- 插件默认就是启用的：`config.ini` 里的 `plugin_disabled` 是**黑名单**（default 空），
  只有被禁用的才登记在册。
- 但"显示哪个项目"要在 `[task_bar] plugin_display_item` 里登记（逗号分隔的项目 id 集合），
  值为 `hr`（即 `IPluginItem::GetItemId()`）。这一步可以直接改 config.ini。

### 10.3 `show_task_bar_wnd` 改 config.ini 不一定生效
`src` 里的流程是启动后 1 秒判断 `m_show_task_bar_wnd` 再 `OpenTaskBarWnd()`。
实测直接写 config.ini 有时不生效（任务栏窗口不出现），而右键通知区图标 →
"显示任务栏窗口"一次之后就正常且被记住。所以 `scripts/configure-trafficmonitor.ps1`
仍然会写这个键，但 README 里明确写了"没出现就用菜单开一次"。

顺带记录一个排查结论：本机（Windows 11 build 26200）上 TrafficMonitor 的
Win11 任务栏判定是

```cpp
m_is_windows11_taskbar = (::FindWindowExW(hTaskbar, 0,
    L"Windows.UI.Composition.DesktopWindowContentBridge", NULL) != NULL);
```

`FindWindowEx` 只找**直接**子窗口。本机该 bridge 窗口存在（`EnumChildWindows` 能枚举到）
但不是 `Shell_TrayWnd` 的直接子窗口，所以这个判定可能返回 false 而退到
`CClassicalTaskbarDlg`。实测任务栏窗口最终是能正常显示的，故未深究。

### 10.4 需要 `/utf-8`
`plugin.cpp` 是**无 BOM 的 UTF-8**。不加 `/utf-8` 时 MSVC 按系统 ANSI 代码页
（本机 GBK）解释源码，`L"心率"` 会取决于机器区域设置 —— 本机碰巧编译正确，
换台机器就会变乱码。daemon 和 plugin 的 build.cmd 都显式加了 `/utf-8`。

### 10.5 其他实现细节
- 公共头里的 `HrSharedData.device_name` 是 **36** 字节（不是计划里的 40）：
  `4+4+4+8+4+4+36 = 64`，这样才真的是 64 字节定长；`static_assert` 卡住。
- daemon 用 GUI 子系统（`/SUBSYSTEM:WINDOWS`）+ 隐藏顶层窗口拿消息循环：
  这样 `taskkill`（不带 `/F`）能送 `WM_CLOSE` 进来，走正常退出路径清理 OSD slot。
  从 cmd 启动时用 `AttachConsole(ATTACH_PARENT_PROCESS)` 附加父控制台，日志同屏可见。
- 命令行 `cl` 不会自动链 `user32.lib`，daemon 的 build.cmd 里显式加了。
- BLE 线程用 MTA apartment + 阻塞 `.get()`（不开协程），扫描/连接失败按
  1→2→4…→30 秒指数退避。

## 11. 验收结果（2026-09-16，本机）

| 项 | 结果 |
|---|---|
| daemon 构建 | ✅ `hr-daemon.exe`，仅依赖系统 DLL |
| 插件构建 | ✅ `hr_plugin.dll`，`File Type: DLL`，x64，导出 `TMPluginGetInstance` 无修饰，仅依赖 KERNEL32 |
| RTSS OSD（demo） | ✅ osd_test 窗口内红色 `HR 112 bpm` + 第 2 行 60 秒填充曲线 |
| RTSS 协议 | ✅ 接上 v2.21 共享内存，8 个 OSD slot，内嵌曲线支持检测正确 |
| 共享内存 → 插件 | ✅ daemon 写 111 → 插件 `GetItemValueText()` 返回 `111`，tooltip `HR 111 bpm - DEMO` |
| TrafficMonitor 加载插件 | ✅ 进程模块列表里出现 `D:\Program Files\TrafficMonitor\plugins\hr_plugin.dll` |
| 任务栏显示 | ✅ 任务栏上显示 `HR 130`（数值随 demo 变化） |
| 超时（kill daemon 后 18 秒） | ✅ 插件返回 `--`；OSD 变为 `HR -- bpm`，曲线压平 |
| 正常退出清理 | ✅ `taskkill`（不带 /F）→ 日志"已释放 OSD slot 1" → OSD 立刻消失，无残留帧 |
| BLE 代码路径 | ✅ `--address` 直连不存在地址：优雅失败 + 2→4→8 秒退避，不崩溃 |
| 真机（华为手表） | ⏳ 未验证 —— 需要手表在"心率广播"页面，见 README §4 |

## 12. 第二轮：可配置化 + 开源准备（2026-09-16）

第一轮把功能跑通之后，参数全是编译期写死的（超时、颜色、字号、曲线量程、日志上限……），
开源出去别人没法调。这一轮补齐配置与授权。

### 12.1 配置体系

统一放在 exe 同目录的 INI 里，UTF-8 带 BOM（中文注释在记事本/VS Code 里都正常）：

| 文件 | 位置 | 管什么 |
|---|---|---|
| `hr-daemon.ini` | 与 `hr-daemon.exe` 同目录 | 采集 / 显示 / OSD / 日志 |
| `hr_plugin.ini` | 与 `hr_plugin.dll` 同目录（即 TM 的 `plugins\`） | 插件的显示标签、名称、示例值、兜底超时 |

两边共用 `common/hr_config.h/.cpp` 的读写逻辑：**读端宽容**（缺项用默认值、越界拉回
合法范围并记日志），**写端生成带完整注释的模板**。daemon 永远不信任这个文件。

去掉了这些硬编码（现在都能改）：超时阈值、采样周期、OSD 开关/两种颜色/数值字号/
标签/单位/曲线开关/曲线宽高边距/量程/点数、日志上限、BLE 扫描超时与退避区间。

新增的排查手段：daemon 启动时打一行
`配置: OSD 文本模板 = "<C0=FF6060>...<OBJ=00000000>..."`
（`\n` 转义成 `↵`）。OSD 显示不对时一眼能分出是配置问题还是 RTSS 问题。

### 12.2 配置工具：Rust，控制台，零依赖

用户拍板：**监听端保持 C++，配置工具用 Rust**。

- 产品是 `tools/hr-config/`，纯 std，**一个 crate 都不依赖**（Windows API 用
  `extern "system"` 手写声明，见 `src/win.rs`），所以 `cargo build --offline`
  完全可复现，不需要联网也不需要 MSVC。
- 职责刻意做小：**只改 `hr-daemon.ini`，外加可选的"重启 daemon"**。
  不读共享内存、不扫设备、不动 TrafficMonitor —— 那些分别属于 daemon 和部署脚本。
- 交互式菜单 + 子命令（`show`/`get`/`set`/`reset`/`restart`/`path`）。
  改值时当场做类型与范围校验，不合法就不写文件。
- "重启"是优雅的：先向 daemon 的隐藏窗口发 `WM_CLOSE`（它会释放 RTSS OSD slot），
  等它真的退出（最多 5 秒）再启动新的，避免两个实例抢同一个 OSD slot。
- 中文对齐：终端里 CJK 是双宽，Rust 的 `{:<n}` 按 char 数补空格会参差不齐，
  所以自己算了显示宽度再补。

### 12.3 顺手修的 bug

1. **`%ls` 打不出内容**：`LogInfo("日志文件: %ls", wstr.c_str())` 这类调用在 daemon 里
   稳定输出**空行**（宽字符转换在该 TU 组合下失效），而同样的 `LogV` 在独立测试程序里
   却正常。全部改成 `%s` + `ToUtf8()`，与其它日志统一，也免掉 `%ls`/`%s` 混用的坑。
2. **daemon/build.cmd 拷贝失败仍报 OK**：daemon 正在运行时 `copy` 到 `build\` 会失败，
   脚本却照样打印 `[OK]`。现在检查 `errorlevel` 并给出"先 taskkill"的提示。
3. **`.cmd` 的行尾**：批处理是 LF-only 时 cmd.exe 会把中文注释按错位的字节切开，
   报一堆 `'xxx' is not recognized`。已加 `.gitattributes` 强制 `*.cmd text eol=crlf`。
4. **RTSS 头文件路径**：原来只看 `C:\Program Files\...`，本机 RTSS 装在 D: 就找不到。
   现在按盘符扫，并支持 `%RTSS_SDK%` 覆盖。

### 12.4 授权

本仓库自己的代码用 **MIT**（`LICENSE`，里面有 `<YOUR NAME HERE>` 占位符待填）。
两个第三方组件单独说明，见 `THIRD_PARTY_NOTICES.md`：

1. **`tm-plugin/PluginInterface.h`** —— TrafficMonitor (zhongyang219)，用的是
   **"Anti 996" License v1.0**。该许可证基于 MIT，附加两条劳动权益条款，并且要求
   每个再分发副本**原样保留**版权声明与许可证全文。所以仓库里带了
   `third_party/trafficmonitor-LICENSE.txt`，且该头文件的版权头未做任何改动。
   想彻底摆脱这个条件，干净的做法是删掉 `tm-plugin/`（只用 Afterburner 那条链路，
   它不受这个许可证影响，见 §13.4）—— 自己重写一份接口声明属于 ABI 灰色地带，
   本仓库没走这条路。
2. ~~**`third_party/rtss/*.h`**~~ —— **已在 §13 删除**。那两份 RTSS SDK 头文件
   （RivaTuner/Unwinder）**完全没有许可证声明**，再分发处于没有明确授权的状态；
   daemon 不再直写 RTSS 之后它们就没用了，整个目录已移除，这个顾虑随之消失。

### 12.5 第二轮验收

| 项 | 结果 |
|---|---|
| 四个组件全量重建 | ✅ daemon / tm-plugin / osd_test (MSVC) + hr-config (Rust)，均从零重建通过 |
| `vswhere` 找 MSVC | ✅ 不再绑死 VS 版本/路径 |
| 本机 RTSS SDK 优先 | ✅ 日志 `RTSS SDK: D:\Program Files\RivaTuner Statistics Server\SDK\Include (来自本机 RTSS 安装)` |
| 配置写读跨语言 | ✅ Rust 写的 ini → C++ daemon 读到 `模式: 模拟心率源（配置 source.demo=1）` |
| 配置 → OSD 映射 | ✅ 改 `color_value=FF8000 / value_scale=200 / label=心率` 后模板变为 `<C1=FF8000><S1=200>...心率...` |
| `hr-config` 子命令 | ✅ `show/get/set/reset/restart/path/help`；非法值当场拒绝且不写文件（点数为非 2 的幂、MAC 格式错、量程上限小于下限） |
| `hr-config` 交互菜单 | ✅ 编号改项、回车保持、`s` 保存、`q` 放弃；中文列对齐正常 |
| `hr-config restart` | ✅ 日志确认优雅交接：`正在退出...` → `hr-daemon 已退出` → 新实例 `配置文件: ...` |
| 插件读 `hr_plugin.ini` | ✅ `label=心率 / name=心率监控 / sample=188` 生效（`GetItemLableText` = U+5FC3 U+7387） |
| 任务栏显示 | ✅ 任务栏上实时显示 `HR 140` |
| `--scan` 模式 | ✅ `hr-daemon.exe --scan 8 --out ...` 退出码 0，写出 UTF-8 结果文件（本次 0 台设备——手表不在） |
| 日志 `%ls` 修复 | ✅ 路径等宽字符串正常打印，不再出现空行 |
| 真机（华为手表） | ⏳ 仍未验收（手表不在）。协议与代码路径已按 §11 验证过 |
| RTSS OSD 视觉复验 | ⚠️ 未做 —— 本机 RTSS 在本轮中途退出后再也起不来（详见 §12.6） |

### 12.6 本轮遗留：RTSS 起不来了

第一轮结束时 RTSS 还在运行，本轮中途它退出（共享内存留下 `sig=0x0000DEAD`），
之后用 `start` / `Start-Process` / 桌面自动化三种方式都拉不起来，进程不出现、
事件日志无记录。**这不是本项目的代码问题**（daemon 的日志明确显示它在等 RTSS，
且不依赖 RTSS 的其它功能都正常），大概率是 RTSS 在挂着 hook 时被结束、需要重启
机器才能恢复。

因此"改配置后 OSD 外观真的变了没有"这一条**没有用截图复验**，改用等价且更可靠的
手段验证：daemon 启动时打印的 `OSD 文本模板`（见 §12.1），它和真正写进
`szOSDEx` 的是同一个函数 `RtssOsd::BuildText()` 的产物。机器重启后按 §9 开着
`osd_test.exe` 复看一眼即可。

> **§13 之后的结论**：这个遗留问题已经不重要了。daemon 不再写 RTSS，也就不再关心
> RTSS 起不起得来；OSD 的数据链路改由 Afterburner 插件承担，而**这条链路可以完全
> 脱离 RTSS 验证**（Afterburner 的监控曲线是它自己的 GUI 画的），见 §13.3。

## 13. 第三轮：改用 MSI Afterburner 插件（2026-09-16）

### 13.1 为什么改：Afterburner 的 OSD 本身就是 RTSS

要求是"不再使用 RTSS，改为 MSI Afterburner 的插件，这样可以统一配置 OSD"。
动手前先把一件事核实清楚（本机 Afterburner 4.6.6 实测）：

| 证据 | 说明 |
|---|---|
| `MSIAfterburner.exe` 里的字符串含 `RTSSSharedMemoryV2`、`RTSSInstalled`、`RivaTuner Statistics Server`、`Software\Unwinder\RTSS`、`\RTSS.exe`、`\RTSSHooks.dll` | 它和早先的 daemon 用的是同一个 RTSS 共享内存 |
| 安装目录带 `Redist\RTSSSetup.exe`（18 MB） | Afterburner 把 RTSS 作为自己的渲染器一起分发 |
| OSD 属性页的帮助文本把它叫 `%SERVERPRODUCTNAME%` | Afterburner 只是那个服务端的客户端 |

结论：**Afterburner 自己不会画覆盖层**，游戏内 OSD 是 RTSS 渲染的。所以
"RTSS 完全不装、还要游戏内 OSD"通过 Afterburner 做不到。

能达成、也确实需要的目标是：**本项目侧彻底不再碰 RTSS**，同时把心率做成 Afterburner
的一条原生监控数据源 —— 于是心率与 GPU 温度、CPU 占用、帧率并列在同一条 OSD 里，
颜色/字号/量程/位置/是否进 OSD 全部只在 Afterburner 一个界面里配。

### 13.2 插件契约定案（实测出来的，不是抄文档）

宿主 `MSIAfterburner.exe` 是 **32 位**（PE machine `0x14C`；它建出来的 MAHM 头
`headerSize=32` 也印证了这点），加载不了 x64 DLL，所以插件**必须编成 x86**。
它按名字 `GetProcAddress` 找这几个导出：

| 导出 | 本插件 | 说明 |
|---|---|---|
| `DWORD GetSourcesNum()` | ✅ | 插件里有几条数据源；本插件返回 1 |
| `BOOL GetSourceDesc(DWORD dwIndex, LPMONITORING_SOURCE_DESC pDesc)` | ✅ | 填描述符；**不要动 `dwVersion`**（官方头文件原话 "Don't change this field when filling the descriptor!"） |
| `FLOAT GetSourceData(DWORD dwIndex)` | ✅ | 每个硬件轮询周期（`HwPollPeriod`，默认 1000ms）调一次 |
| `BOOL SetupSource(DWORD, HWND)` | ❌ 不实现 | 配置交给 Afterburner 的每数据源属性；不实现就拿不到 MFC `CWnd`，因此**不需要 MFC** |
| `void Uninit()` | ❌ 不实现 | 没有后台线程，映射句柄随进程结束释放 |

官方 7 个样例是 MFC 扩展 DLL，只因为它们的配置对话框要弹 MFC。本插件实测
`dumpbin /dependents` **只有 `KERNEL32.dll`**。

数据源用 `dwID = MONITORING_SOURCE_ID_PLUGIN_MISC (0x000000FF)`；
`szName="Heart rate"`、`szUnits="BPM"`、`szFormat` 留空（宿主默认 `%.0f`，实测宿主
会把它填回 `%.0f`）、量程 40~180（从原 `graph_min/graph_max` 迁过来）。

`MONITORING_SOURCE_DESC` 是按字段**转写**的一份声明（不是 MSI 文件的副本）——
原文件是 CP1251 编码，纳入构建会跟 `/utf-8` 冲突；结构体是纯 C 的 POD，布局一致即
ABI 一致，用 `static_assert(sizeof(...) == 1580)` 钉死。细节见
`THIRD_PARTY_NOTICES.md` 第 2 节。

### 13.3 数据流与"可脱离 RTSS 验证"

插件读的还是 daemon 那个 64 字节中立共享内存 `Local\HuaWeiHR_SM`（按字节布局，
x64 写 / x86 读没问题）。无效或超时上报 `FLT_MAX`，这是 MAHM 约定的"当前无数据"。

映射**打开一次长期持有**即可：daemon 退出后映射对象依然存在（插件握着句柄），
daemon 重启时 `CreateFileMappingW` 拿到的是同一个对象，所以不需要重开或重试逻辑 ——
这一点已实测确认（§13.5）。

新增的验证手段 `tools/mahm-probe/mahm-probe.ps1`：直接读 Afterburner 的
`MAHMSharedMemory` 并打印数据源列表。**这条链路可以完全脱离 RTSS 验证** ——
Afterburner 的监控曲线是它自己的 GUI 画的，不需要 RTSS、也不需要 3D 程序。
第一轮/第二轮卡住的"RTSS 起不来所以没法复验 OSD"从此不是阻塞项。

### 13.4 顺带解决的四件事

1. **删掉没有许可证的 RTSS 头文件**：`third_party/rtss/`（`RTSSSharedMemory.h`、
   `RTSSHooksTypes.h`）整目录删除，`THIRD_PARTY_NOTICES.md` 第 2 节随之改写。
2. **`hr-daemon.ini` 去掉整个 `[osd]` 段**（13 个外观键：开关/两色/字号/标签/单位/
   曲线开关/宽高边距/量程/点数），配置工具 `hr-config` 同步删掉对应字段与校验
   （含只服务于 OSD 的 `Kind::Color`/`Kind::Pow2` 和 `nearest_pow2`）。
   C++ 侧删了 `NearestPowerOfTwo` —— "点数必须是 2 的幂"纯粹是 RTSS 内嵌对象环形缓冲的
   约束；Afterburner 自己按 `MonitoringDataBufferSize`（默认 3600 点）存历史。
3. **daemon 不再因为 RTSS 不在而刷日志**：`rtss_osd.cpp`（259 行）与其 52 行头文件、
   `main.cpp` 里的 `History` 环形缓冲与 `g_rtss`、`daemon/build.cmd` 里的 RTSS SDK
   扫描块全部移除。`HrParseColor`/`HrFormatColor` 随颜色配置失去用途，一并删掉。
4. **`hr-config` 的输出对齐新配置**：`show` 只剩 9 项；旧键（如 `graph_max`）报
   "未知配置项"；重新生成的 ini 里不再有 `[osd]`。老配置里遗留的 `[osd]` 段会被
   安静忽略，下次保存时自然消失。

保留 `tools/osd_test/`：OSD 仍然需要有个跑着的 3D 窗口才画得出来，它依旧是好用的
验证画布（只更新了文件头注释）。

### 13.5 第三轮验收（2026-09-16，本机）

| 项 | 结果 |
|---|---|
| 插件构建 | ✅ `HeartRate.dll`；`dumpbin /headers` = `14C machine (x86)`；导出恰好 `GetSourcesNum`/`GetSourceDesc`/`GetSourceData` 三个且都无修饰；`dumpbin /dependents` **只有 KERNEL32.dll** |
| 插件被宿主加载 | ✅ Afterburner（x86）正常加载；数据源出现在监控列表 |
| 数据源描述符 | ✅ MAHM 里读到 `szSrcName="Heart rate"`、`szUnits="BPM"`、`format='%.0f'`、`srcId=0xFF`、`gpu=0x0` |
| 数值链路 | ✅ `hr-daemon.exe --demo` 日志报 137 → MAHM 里读回 `value=137`，随模拟心率变化 |
| 超时 | ✅ kill daemon 后 15 秒内仍读到最后一个值（在陈旧窗口内，符合语义）；超过 15 秒变成 `UNAVAILABLE (FLT_MAX)` |
| daemon 重启恢复 | ✅ 重启 daemon、**不重启 Afterburner**，值自动恢复（验证了"映射长期持有、无需重开"的设计） |
| 部署脚本 | ✅ 只往 `Profiles\MSIAfterburner.cfg` 的 `[Monitoring]` 插入一行 `HeartRate.dll=1`；`diff` 确认其余字节完全不变（profile 用 Latin-1 字节级回写，编码/BOM/换行风格都不动） |
| 权限 | ✅ 本机 Afterburner 目录可写，脚本按"实际能不能写"判断、不再硬卡管理员。（Afterburner 进程本身是管理员权限，普通权限连 `taskkill` 都关不掉它，但共享内存照样通 —— 跨完整性级别不影响 `Local\`） |
| 配置迁移 | ✅ `hr-config show` 只剩 9 项；老键报"未知配置项"；重新生成的 ini 不含 `[osd]`；daemon 读新 ini 正常启动并采集 |
| 四组件全量重建 | ✅ daemon / ab-plugin / tm-plugin / osd_test (MSVC) + hr-config (Rust)，删掉 obj 后从零重建通过 |
| OSD 视觉复验 | ⚠️ 未做 —— 需要在 Afterburner 界面里勾选该数据源并勾上 `Show in On-Screen Display`，是用户的一次性界面操作（README §7.3） |
| 真机（华为手表） | ⏳ 仍未验收（手表不在），与 §11/§12.5 一致 |

### 13.6 构建脚本上踩的两个坑

1. **`.cmd` 必须 CRLF**：`.gitattributes` 里写了 `*.cmd text eol=crlf`，但本目录不是
   git 仓库，这条规则不会自动生效。新写的 `ab-plugin/build.cmd` 是 LF-only 时 cmd.exe
   报出一堆莫名其妙的 `'HERE" (' is not recognized` / `then was unexpected at this time.`。
   新建或修改批处理之后要确认行尾是 CRLF。§12.3 第 3 条记的是同一类问题的另一面。
2. **批处理里 echo 的括号要转义**：`if not defined X ( ... echo ...Tools.x86.x64) ... )`
   里那个 `)` 会**提前闭合 if 块**，剩下的 `, then retry.` 被当成命令执行，报
   `then was unexpected at this time.`。块内文本不要出现裸的 `(` / `)`。
   另外 `ab-plugin/build.cmd` 刻意保持**纯 ASCII**：cmd 按字节重读批处理文件，控制台
   代码页和文件编码不一致时非 ASCII 文本会错位（本机从 Git Bash 跑就是这个情形）；
   中文说明写在 `plugin.cpp` 的注释里。

### 13.7 已知取舍

- 界面里的"勾数据源 + 勾 Show in OSD"没有脚本化：`Sources=` 和 `[Source ...]` 是
  Afterburner 自己的数据结构，它退出时会重写 profile，硬改容易被覆盖。部署脚本只做
  能可靠做对的部分（装 DLL、写启用键、写说明文件），剩下两步在 README §7.3 里写清楚了。
- 只暴露一条数据源。连接状态、电量等留二期（与 §9 一致）。
- `scripts/deploy-afterburner-plugin.ps1` 写成 **UTF-8 带 BOM**：PowerShell 5.1 对
  无 BOM 的 .ps1 会按 ANSI 解释，里面的中文就花了。仓库里早先那个
  `configure-trafficmonitor.ps1` 没有 BOM，如果哪天中文输出花了，加个 BOM 即可。

---

## 14. 第四轮：配置项 2 改成"扫描 + 挑设备"（2026-09-16）

原来的第 2 项（`address`）要人手抄 MAC。现在选它会先扫一遍附近的心率广播设备、
列出编号让人挑，`0` 重新扫描，回车保持、`-` 清空、直接敲 MAC 也照样认。

### 14.1 实现：借 `hr-daemon.exe --scan`，一行 C++ 都没改

daemon 早就有 `--scan <秒> [--out 文件]`（`daemon/main.cpp` 的 scan 分支 →
`BleScanToFile`），扫完把每台设备写成一行 `MAC<TAB>名字`（UTF-8 带 BOM）再退出，
用法注释里写的就是"给 hr-config 用"，但一直没人调。这次把它接上了：

- `tools/hr-config/src/scan.rs`（新增）：起 `hr-daemon.exe --scan 5 --out %TEMP%\hr-config-scan-<pid>.txt`、
  等它退出、读文件。**启动前先删同名文件** —— 扫描起不来时 daemon 不写文件（退出码 1），
  留着旧的会被当成本次结果。
- `tools/hr-config/src/proc.rs`（新增）：`spawn` / `spawn_detached` / `run_and_wait`，
  把原来散在 `main.rs` 里的 `CreateProcessW` 收成一处。hr-config 仍然是零 crate、离线可构建。
- `tools/hr-config/src/main.rs`：`edit_address()` + `pick_device()`，只在 `FIELDS` 里
  命中 `address` 时走这条路，其余 8 项的逻辑一行没动。

### 14.2 顺手修掉的 bug：重启会"重启出一个空档"

写这个功能时踩到了 daemon 生命周期的一个真 bug，`restart` 和菜单的 `r` 都有：

`PostMessageW(WM_CLOSE)` 之后，**窗口是 `DestroyWindow` 先没的，进程还在**——BLE 线程
要等当前那次连接尝试收尾才能 join（实测卡了 16 秒：连接 8 秒 + 服务发现 8 秒）。
原来的 `restart_daemon` 只等"窗口消失"就急着起新的，新实例一看单实例互斥体还在
（`Local\HuaWeiHR_daemon`），当场打印"已经有一个 hr-daemon 在运行"退出 —— 结果是
**一个都没在跑**。实测日志：

```
[21:43:19.121] [INFO] 日志文件: ...           <- 新实例起来了
[21:43:19.126] [WARN] 已经有一个 hr-daemon 在运行（互斥体 Local\HuaWeiHR_daemon），本次启动退出
[21:43:25.934] [INFO] hr-daemon 已退出        <- 旧的那个 7 秒后才真正退出
```

修法：判断"还在不在跑"改用**互斥体**（`OpenMutexW`，只打开不创建，不会和 daemon 抢），
`stop_daemon()` 等到窗口和互斥体都没了才算走干净（上限 20 秒，超过 500ms 会打印
"等 hr-daemon 收尾…"）；起完新的再 `wait_daemon_up()` 确认它注册上了，没起来就报错而不是
谎报"已重启"。`offer_restart` 判断"在不在跑"也换成了同一套（窗口没了但进程还在收尾也算在跑）。

### 14.3 验收（2026-09-16，本机）

| 项 | 结果 |
|---|---|
| 编译 | ✅ `cargo build --release --offline`，零依赖不变 |
| 真机扫描 | ✅ 停掉 daemon → 等 2 秒 → 扫到真手表 `AA:BB:CC:DD:EE:FF`（与 daemon 日志里的地址一致） |
| 列设备 / 选编号 | ✅ 借**假 hr-daemon**（临时目录里放一个只写结果文件的同名 exe）验过：2 台设备、`（无名字）` 显示、`← 当前` 标记、选 1 后 ini 写入 `address=AA:BB:CC:DD:EE:01` |
| `0` 重扫 / 越界 / `-` 清空 | ✅ 重扫重打列表；越界提示"列表里没有第 7 台（只有 1~2）"；`-` 落盘 `address=` |
| 手填 MAC | ✅ `aa-bb-cc-dd-ee-05` → 规范成 `AA:BB:CC:DD:EE:05` 并保存 |
| 扫描通道挂了 | ✅ 挪走 hr-daemon.exe → 报"找不到 …"并退回原来的手填提示，不影响改配置 |
| 暂停 daemon → 自动拉起 | ✅ 跑完 `tasklist` 里 daemon 在，日志有新实例的启动行（修 bug 前这里是空的） |
| `restart` 连做三次 | ✅ 每次都是新 PID 且都在跑 |
| 管道 EOF（非交互） | ✅ `printf '2\n' \| hr-config.exe` 不死循环、正常退出 |
| 没扫到设备 | ✅ 打印排查提示（手表要停在广播页/蓝牙要开/被连着时不广播）+ `0` 重扫 |
| 已知限制 | ⚠️ 手表被 daemon 连着时不再广播，得答 y 停掉它才可能扫到；固定 5 秒可能漏设备；扫描子进程的日志会刷到同一个控制台（也算进度提示） |

---

## 15. 第五轮：日志不再刷心率 + 限制大小与份数（2026-09-16）

起因：`心率: 140 bpm` 这类行是按"值变了就写一行"打的，手表的心率几乎每秒都在变，
一天下来就是几 MB。现在：

- **心率值改成 DEBUG 级**，默认不输出。INFO 里只留状态变化（连接中/数据超时/无数据）
  和错误 —— "超时了"这种仍然看得见。
- 打开方式两个：`hr-daemon.ini` 的 `log.debug=1`（`hr-config set debug 1`，菜单第 9 项），
  或前台跑 `hr-daemon.exe --debug`。这些行带 `[DEBG]`，好过滤。
- **限制大小**：单份超过 `log.max_kb`（默认 4096）就在运行中就地轮转 —— 当前这份顶成
  `.1`，另开一份接着写；每份都 ≤ `log.max_kb`。
- **限制份数**：每次 daemon 启动开新的一份，`hr-daemon.log` → `.1` → `.2`，更老的删掉。
  磁盘上永远最多 3 份 = 最近三次运行，总量 ≤ 3 × `log.max_kb`。

### 15.1 几个实现上的取舍

- 旧的"超过上限就删掉重开"只在**启动时**检查，跑久了照样能涨到任意大（真 bug）。现在改成
  运行中轮转，`max_kb` 才真的封得住。
- 日志句柄加了 `FILE_SHARE_DELETE`：Windows 上文件被自己打开着就改不了名，不加这个
  运行中没法轮转。
- `--scan` 是一次性进程，**不轮转、不动历史**（否则 hr-config 每扫一次就把 daemon 的
  历史顶掉一份）。为此 `ParseArgs` 挪到了 `LogInit` 之前（`--scan` 决定要不要轮转），
  它那几条警告先攒进 vector、等日志开了再打，不会丢。
- 轮转挪不动（文件被别的进程占着）时不反复重试，也不把日志写丢：继续往当前那份写。
- 配置项 `log.debug` 两边模板（`common/hr_config.cpp` 与 `tools/hr-config/src/config.rs`）
  逐字节对齐，和 `log.max_kb` 一样。

### 15.2 验收（2026-09-16，本机）

| 项 | 结果 |
|---|---|
| 默认不刷心率 | ✅ `--demo` 跑 8 秒（心率每秒都变），日志里 `心率: N bpm` **0 行**，整份只有 10 行 |
| `--debug` | ✅ 6 行 `[DEBG] 心率: N bpm` |
| `log.debug=1`（走 ini） | ✅ 同样出 `[DEBG]` 行；`hr-config` 菜单/`show` 里有第 9 项 `debug` |
| 启动轮转 + 只留 3 份 | ✅ 连启 5 次 → 磁盘上正好 3 份，分别是最后 3 次运行（对 PID 验证） |
| 运行中轮转 + 大小上限 | ✅ 临时把 `log.cpp` 抽出来单独压：64KB 上限灌 3000 行（~330KB）→ 3 份、每份 ≤ 65536 字节、最新数据在 `hr-daemon.log`、轮转处有说明行 |
| `--scan` 不动历史 | ✅ 扫描后 `.1`/`.2` 的 md5 一字节没变，只往 `hr-daemon.log` 追加，无轮转说明行 |
| 两边 ini 模板 | ✅ C++ `ToIniText` 与 Rust `to_ini_text` 逐字节一致（1178 字节，含新键） |
| daemon 真跑 | ✅ 重建后 `--demo`/正常模式都能起，共享内存照常；日志刷新率从"每秒"降到"有状态变化才写" |

---

## 16. 第六轮：推送前的整轮加固（开源前自查）

开源前的代码复查，按"漏洞 / 鲁棒性 / 效率"三条线通读了一遍 C++、Rust 和 PowerShell。
没有内存破坏类问题（字符串全走 `*_s` 有界版本、下标都有边界检查、`HrSharedData` 有
`static_assert` 锁死 64 字节），问题集中在**关不掉的线程、被静默吞掉的配置错误、
非原子的文件改写**三类。

### 16.1 daemon / 共享内存

- **扫描无法取消（真 bug）**：`ScanCollect` 收不到停止标志，必须跑满 `scan_timeout_ms`
  （上限 600 秒），而 `Stop()` 直接 `join()` —— `WM_CLOSE`（taskkill 不带 `/F`、注销、
  关机）会被堵住最长 10 分钟，进程看起来是卡死的。现在 stop 标志一路传进等待循环，
  常驻路径扫到第一台设备再多收 1.5 秒名字就走。
- **WinRT `.get()` 没有超时**：蓝牙栈卡住时线程永远回不来，`join()` 也就永远不返回，
  单实例互斥体一直被占，之后每次启动都被当成"已经在运行"。改成轮询 `Status()` 的
  带 deadline 等待（连接/服务发现 10 秒、订阅 5 秒），超时就 `Cancel()` 当本次失败。
  用轮询而不是 `Completed` 回调，是为了避开回调生命周期和未观察异常两个坑。
- **重连期间显示"已超时"**：`bpm` 一旦有过值就再也不会回到 -1，于是数据源上报的
  `HRS_CONNECTING` 被 `HRS_TIMEOUT` 覆盖，两个插件在每次重连时都显示"已超时"，
  和代码注释说的相反。
- **BLE 层忽略配置的超时**：`ble.cpp` 写死 `HRSM_TIMEOUT_MS`（15 秒），而 daemon 判断
  超时和插件兜底用的都是 `display.timeout_ms`。用户把超时调到 60 秒，底层照样 15 秒
  就断链重连。现在三处用的是同一个数。
- **配置钳位提示永远打不出来**：`Load()` 内部已经 `Sanitize()` 过，`main.cpp` 又拿
  结果复制一份重新 `Sanitize(&notes)`，此时值早已合法，`notes` 恒为空 —— 用户写错
  `refresh_ms=50` 被静默改掉，一句日志都没有。改成 `Load(path, notes)` 出参。
- **扫描结果文件写入不检查返回值**：磁盘满/介质错误时静默报成功，`--scan` 退出码 0，
  hr-config 于是显示"没扫到设备"。
- **`HrParseMac` 接受尾部垃圾**：只检查转换成功的字段数，`"...:FF junk"`、`"...:FF:99"`
  都算合法，daemon 然后去连第一个 MAC。加 `%n` 要求整串被吃干净。
- **事件回调可能读过栈**：`ValueChanged` 的 lambda 按引用捕获栈上的局部量，而
  `ValueChanged(token)` 只是"请求注销"，不保证已经在别的线程上派发的回调已经返回。
  状态改放堆上、由 lambda 按值持有 `shared_ptr`。
- **日志截断即丢弃**：`_snprintf_s` 配 `_TRUNCATE` 时输出被截断会返回 -1，而代码把
  `n <= 0` 当"没内容"整行丢掉 —— "日志行太长"的后果变成"这一行彻底不见了"。
- 其它：`SetTimer` 失败没检查；设备名超过 35 字节时 `WideCharToMultiByte` 整体失败、
  名字静默变空串（改成先转临时缓冲再按 UTF-8 字符边界截断）；`GetInt` 不看 `errno`
  和范围；`ToIniText` 的定长缓冲改成按 `_scprintf` 实际长度分配，从根上不可能截断；
  `%LOCALAPPDATA%` 取不到时会退回相对路径 `.\HuaWeiHR`。

### 16.2 共享内存协议

`HrSharedReader::Read()` 补上 `version` 校验（以前只查 `magic`），并**修正了那段不成立
的注释**：原注释说"读者不会读到半条记录"，但 `memcpy` 是按地址递增写的，并发读者确实
可能看到新的 `magic` 配旧的 `bpm`，`tick_ms` 是 8 字节而 `pack(4)` 只给 4 字节对齐，
32 位宿主上的 qword 读理论上也会撕裂。

**没有改布局**（没加 seqlock）：最坏结果是读者多显示一次旧值或多一次 `--`，而 daemon
每个 `refresh_ms`（默认 1 秒）就整条重写，下一拍必然自愈；改布局要连带动 `HRSM_VERSION`
和两个插件，风险大于收益。注释里把这一点写清楚了。

### 16.3 hr-config（Rust）

- **读不出来 ≠ 不存在（数据丢失）**：`read_ini` 把所有 IO 错误和非 UTF-8 都塌缩成
  `None`，上层当成"文件不存在，用默认值"，菜单显示默认值，用户改一项保存就用默认值
  覆盖掉真实配置。而记事本另存成 UTF-16（C++ 侧特意兼容了）正好命中这条路径。
  现在区分 `NotFound` 和其它错误、兼容 UTF-16 BOM、解不出来就明确报错退出。
- **写入不是原子的**：改成"拷 `.bak` → 写 `.tmp` → `MoveFileExW(REPLACE_EXISTING)`"。
- **EOF 被当成"是"**：`ask_yes_default_true` 把 EOF 和空行一样当 yes，于是
  `hr-config set demo 1 < NUL` 这种非交互调用会不吭声地停掉再拉起 daemon；而破坏性
  更强的 `reset` 反而更严格（EOF = 否），正好反了。现在 `read_line` 返回三态，EOF
  一律当"不做"。
- **命令行拼接不过引号转义**：加了按 `CommandLineToArgvW` 规则的 `quote_arg()`，
  `--scan ... --out <路径>` 改成传参数数组，不再手拼字符串。
- **扫描结果文件放共享 `%TEMP%` 且名字可预测**：挪到 `%LOCALAPPDATA%\HRMonitor\`
  （每用户私有）并加时间戳后缀。
- 其它：文本项拒绝裸 CR/LF（否则能往 INI 里注入行和段）；`exe_dir()` 加缓存（以前每次
  调用分配并清零 64 KB，`cmd_path` 一处就调三次）、检测截断并按需增长；`offer_restart`
  的失败会传进退出码。

### 16.4 部署脚本（PowerShell）

- **非原子、无备份地改写用户的 TrafficMonitor 配置**：`config.ini` 里装着用户全部的
  窗口布局、项目顺序、逐项颜色，原来用 `WriteAllText` 直接截断重写，中途出事就是一份
  被截断的配置且没有备份；`-Encoding Unicode` 还是无条件假设。
  现在按"哪个编码读出来能找到锚点键"判定编码（实测这份文件是 UTF-8 带 BOM，而更早的
  版本是 UTF-16LE，所以不能只看 BOM 猜一次），改前备份 `.bak`，写 `.tmp` 后
  `File::Replace` 原子替换，替换后重读校验。Afterburner 的 profile 同样处理。
- **修掉一个原有的真 bug**：`plugin_display_item` 解析时 `-split` 在只有一个 id 时返回
  字符串而不是数组，`$ids += $ItemId` 于是变成字符串拼接 —— 往已有的 `hr` 上再加一个
  id 会写成 `hrhrx` 而不是 `hr,hrx`。外层补 `@(...)` 修正。
- **用户数据被当成正则替换模式**：`$` 在 .NET 替换串里是元字符，改成 `MatchEvaluator`
  委托返回字面量。
- **按进程名杀进程**：改成路径可读时按 `Path` 过滤，读不到（提权进程）才退回按名字。
- **先删旧任务再建新任务**：注册失败就什么都不剩。改成先 `Export-ScheduledTask` 存档，
  失败时恢复（恢复也失败就把 XML 存到临时目录并提示）。
- **`-Address` 未校验也未加引号就写进持久化的自启点**：加 `ValidatePattern` 只放行
  完整 MAC，并规范化成 `AA:BB:CC:DD:EE:FF`；计划任务主体改用完整身份
  （`DOMAIN\user`），`-AtLogOn` 带上 `-User`；仓库在 UNC 路径上直接拒绝（登录时网络
  还没就绪）。
- 其它：`osd_test` 被遮挡/最小化时 `Present(1,0)` 立刻返回（`DXGI_STATUS_OCCLUDED` 是
  成功码，原来的 `FAILED` 判断抓不到）导致 100% CPU 空转，现在 `Sleep(50)`；
  `QueryPerformanceFrequency` 失败会让计时变成 inf/nan，加了回退；`SetProcessDPIAware`
  挪到创建窗口之前（高 DPI 下客户区尺寸算错）；`osd_test\build.cmd` 不再写死一条 VS
  路径，改用 `vswhere`。

### 16.5 仓库与文档

- `.gitignore` 补 `.zcode/`（以前没被忽略，两篇代理会话草稿会直接进仓库）和全局
  `*.zip`、`*.bak`、`ab-plugin/obj/`；删掉 `!build/.gitkeep` —— 前面 `build/` 已经把整个
  目录排除了，git 根本不会进去看，那行 negation 永久无效。
- `LICENSE` 的 `<YOUR NAME HERE>` 填成 `Liang457`。
- 文档和帮助文本里作者手表的真实 MAC 全部换成 `AA:BB:CC:DD:EE:FF`（PLAN.md 本来就在用
  这种占位符）。
- README 按 rime-sync 那份的路子重写：压平章节、去掉 `1.1`/`7.4` 这类子节链、去掉
  `>` 提示块和 `见 §N` 交叉引用、把"必须知道的点"这类清单还原成正文，并同步本轮改掉的
  行为（保存留 `.bak`、读不出配置会报错、`timeout_ms` 现在真的传到底层、
  `tm_dir` 现在脚本会读）。

### 16.6 验收（2026-09-16，本机）

| 项 | 结果 |
|---|---|
| 五个目标全部编译 | ✅ daemon / ab-plugin（自查 x86 + 三个未修饰导出）/ tm-plugin / osd_test / hr-config |
| `cargo clippy --release` | ✅ 无警告（顺带清掉了原有 4 条） |
| 四个 `.ps1` 语法 | ✅ `Parser::ParseFile` 全部 parse OK |
| demo 模式跑通 | ✅ 共享内存"已就绪"、`[DEBG] 心率: N bpm` 每秒一条、设备名 `DEMO 模拟心率` |
| 共享内存按字节核对 | ✅ `magic=0x4D535248`、`version=1`、`bpm`/`status` 正确、`device_name` 在偏移 28 起且 NUL 结尾、`tick_ms` 与 `GetTickCount64` 及系统 uptime 对得上（布局和文档一致） |
| **扫描中退出**（第 1 项修复的直接验收） | ✅ 日志停在"开始扫描…最长 20 秒"时发 `WM_CLOSE`，**300 ms** 退出，采集线程 100 ms 收手（旧代码会等到扫满 20 秒）；日志有"正在退出… / 采集线程退出 / 已退出" |
| `hr-config show / set` | ✅ 越界值被拒（`refresh 50` → 退出码 2）；`tm_dir` 注入换行被拒 |
| 原子写 + 备份 | ✅ 保存后出现 `hr-daemon.ini.bak`，四个段齐全，无 `.tmp` 残留 |
| `set ... < NUL`（EOF，第 12 项修复） | ✅ 打印"改动手动重启后生效"，**daemon PID 不变**（旧代码会静默重启） |
| 配置编码 | ✅ 非 UTF-8/UTF-16 的文件明确报错、退出码 1、文件未被改动；UTF-16LE 存法能正常读（与 C++ 侧对齐） |
| `configure-trafficmonitor.ps1`（对副本做，未动真实配置） | ✅ 幂等（连跑两次字节一致）；`-ItemId` 追加后 167 行里只有 1 行变化（`hr` → `hr,hrx,zzz`），行数不变；`.bak` 生成 |
| 单实例 / 正常退出 | ✅ 重复启动记一行日志退出；`taskkill` 不带 `/F` 走正常退出路径并关掉映射 |

### 16.7 已知未修 / 未验证

- **真机手表仍未验收**（和第五轮一样，需要手表停在广播页面）。本轮对 BLE 代码的改动
  （可取消扫描、`.get()` 超时、状态码）只覆盖了"蓝牙适配器可用、扫描起得来、无设备"
  这条路径，没跑过真表。
- `hr_shared.h` 的撕裂读问题按 16.2 的理由保留，只修注释和补 `version` 校验。
- Afterburner 的 `deploy-afterburner-plugin.ps1` 只做了语法检查：真正跑它要关掉正在
  运行的 Afterburner 并改它的 profile 与插件目录，没在验收里执行。
- 日志"每行刷盘"是刻意设计（崩溃/被杀时也能看到最后一行），本轮没改。

## 17. 第七轮：无地址不连接 + 配置工具隔离 daemon 控制台（2026-09-17）

第一次真机联调（华为手表实机，广播/连接/订阅/设备名全部打通，见 17.4）时暴露了两个
问题，加上一条主动的行为收紧：

### 17.1 无地址 = 不连接（防连错设备）

- 旧行为：`source.address` 留空时 daemon 扫描并连**第一台** 0x180D 广播设备。多设备
  环境会连错表；而且一旦连上，手表就停止广播（§14.3），想换表都不好扫。
- 新行为：留空（或 ini 里的地址不是合法 MAC）就**不创建数据源、不碰蓝牙**，共享内存
  持续写 `bpm=-1 + HRS_NODATA`（两个插件显示 `--`，tooltip"无数据"，协议零改动、
  不升 HRSM_VERSION），日志提示"未配置手表地址，不连接"。选表走 hr-config 第 2 项
  （`--scan` 一次性模式原样保留）或手填 MAC。
- `ble.cpp` 运行循环里那段"无地址就扫第一台"保留但已不可达（函数开头拦截待机），
  注释写明是刻意保留——将来要恢复自动扫描，得先解决连错表问题。
- 连带修正：`Run()` 无源时不再调 `g_source->Start()`（旧代码源恒非空，没判空）。
- `scan_timeout_ms` 因此暂时没有生效路径：键和菜单项保留，模板注释与 README 标注
  "预留"。

### 17.2 --quiet：hr-config 拉起的 daemon 不再共享控制台

- 现象一（真机联调实测）：选完表后 daemon 的日志（"BLE: 发现设备…"等）直接刷进
  hr-config 的菜单。这是 §10.5 的设计（GUI 子系统 + AttachConsole 同屏），
  §14.3 列为已知限制。
- 现象二：共享控制台意味着 hr-config 窗口的 Ctrl+C / 关闭会以 CTRL_C_EVENT /
  CTRL_CLOSE_EVENT 连带杀掉 daemon——与"配置操作不让 daemon 停摆"（§14）相反。
- 修法：daemon 新增 `--quiet`（`LogSetQuiet`，`LogInit` 跳过 AttachConsole 和
  CONOUT$），hr-config 的两条拉起路径（edit_address 预览、restart_daemon）都带上。
  从 cmd 手动跑 daemon 不带它，交互行为（同屏日志、Ctrl+C 退出）不变。
- 取舍：配置窗口里看不到 daemon 实时日志了，调试看 `hr-daemon.log` / 开 debug。

### 17.3 预览拉起带上刚选的地址

- 旧行为：edit_address 选完设备后立刻把 daemon 拉起来"预览"，但零参数 + 旧 ini
  （保存要等 s/r），预览实例实际跑在扫描模式——单设备碰巧连对，多设备会连错；日志
  "模式: 扫描心率广播设备"也让人以为刚选的地址没生效。
- 新行为：拉起参数带 `--address <刚选的 MAC>`（命令行优先于 ini 是既有规则），
  提示语说明"临时拉起、保存后才写入 ini"。`spawn_detached` 改成带参数版本。

### 17.4 真机进展（2026-09-17）

真机扫描 → 发现 → 连接 → 订阅心率通知 → 读到设备名（HUAWEI WATCH HR-05F）全部
成功，INFO 日志在订阅成功后静默属预期（心率值走 DEBUG，§15）；判数据是否真在流，
看 15 秒内有没有"已 N 秒没有收到心率通知"WARN，或直接开 debug / mahm-probe。
带本轮新代码的完整真机复测（选表 → 临时直连 → 出值）待手表在位时做。
