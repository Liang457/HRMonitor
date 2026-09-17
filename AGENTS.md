# AGENTS.md

华为手表（BLE 心率广播，服务 `0x180D`）→ Windows：`hr-daemon.exe` 采集后写入中立共享内存
`Local\HuaWeiHR_SM`（64 字节），两个互不依赖的显示端各自去读——Afterburner 监控数据源插件
（游戏内 OSD，由 RTSS 渲染）和 TrafficMonitor 任务栏插件。仅 Windows。仓库文档与注释全为中文，
新代码的注释/日志/UI 字符串请保持中文。

## 构建与验证

无测试套件，验证 = 构建 + demo 模拟源 + 探针：

```cmd
cmd /c daemon\build.cmd          :: build\hr-daemon.exe   (C++, x64)
cmd /c ab-plugin\build.cmd       :: build\HeartRate.dll   (C++, x86，必须 x86)
cmd /c tm-plugin\build.cmd       :: build\hr_plugin.dll   (C++, x64)
cmd /c tools\osd_test\build.cmd  :: build\osd_test.exe    (C++, x64)
cmd /c tools\hr-config\build.cmd :: build\hr-config.exe   (Rust, cargo --offline)
```

- 各 build.cmd 用 vswhere 自动定位 MSVC（不绑死 VS 版本），产物统一落 `build\`（已 gitignore）。
- C++ 全部 `/MT` 静态 CRT，目标机器不装 VC++ 运行库。
- 不用手表即可验证整条链路：`build\hr-config.exe set demo 1 && build\hr-config.exe restart`，
  然后 `pwsh -File tools\mahm-probe\mahm-probe.ps1 -Filter Heart` 看 Afterburner 共享内存里的值。

## 架构边界（改代码前必读）

- **`common/hr_shared.h` 是共享内存协议的唯一权威定义**：64 字节 `HrSharedData`、
  `#pragma pack(4)`、magic/version。daemon 写，两个插件读。改布局必须升 `HRSM_VERSION`
  并**三个程序一起重建**——读端校验 version，不匹配就直接读不到数据。写端无锁 memcpy
  是已接受的取舍（每 refresh_ms 整条重写自愈），动一致性前先读该文件内注释。
- daemon 只采集、只写共享内存，不碰 RTSS/Afterburner/TrafficMonitor；本项目与 RTSS 零耦合
  （不写它的共享内存、不引用它的头文件）。
- 两个显示端互不依赖，只通过共享内存解耦；插件只依赖 kernel32。
- `common/hr_config.{h,cpp}`：INI 读写 + 配置结构，daemon 和 tm-plugin 共用。
- `daemon/hr_source.h`：`HrSource` 抽象接口；`ble.cpp`（C++/WinRT，链接 windowsapp.lib）和
  `demo.cpp` 是两个实现。
- 依赖策略：BLE 用 Windows SDK 自带 C++/WinRT；hr-config 是零 crate 的纯 std Rust
  （WinAPI 在 `src/win.rs` 手写 extern "system" 声明）。**不要引入第三方依赖。**

## 硬性约束 / 已知坑

- **ab-plugin 必须 x86**：`MSIAfterburner.exe` 是 32 位进程，加载不了 x64 DLL。
  build.cmd 末尾用 dumpbin 自查 x86 + 三个导出名（`GetSourcesNum`/`GetSourceDesc`/
  `GetSourceData`）未修饰。该插件保持 MBCS（故意不定义 UNICODE），因为
  `MONITORING_SOURCE_DESC` 用 `char[]`。
- **所有 C++ 编译必须带 `/utf-8`**：源码是无 BOM UTF-8 且含中文字符串，缺了会按系统
  代码页误读成乱码。
- **`.cmd` 批处理保持 CRLF、注释尽量 ASCII-only**（`.gitattributes` 已强制 eol=crlf）；
  `(...)` 块内 echo 的文本里不能出现裸 `)`，会提前闭合代码块。
- 换行符约定：.cmd/.bat/.ps1 = CRLF；.cpp/.h/.rs/.md/.toml/.ini = LF。
- 计划任务自启必须跑在**当前登录会话**；跑在会话 0 时 `Local\` 命名空间不同，插件读不到
  共享内存（任务栏和 OSD 一直 `--`）。
- daemon 命令行参数（`--demo`/`--address`）优先于 hr-daemon.ini；单实例互斥体
  `Local\HuaWeiHR_daemon`。
- TrafficMonitor V1.86 只扫 `plugins\*.dll`，插件必须是 `.dll` 不是 `.tmd`；
  `config.ini` 的 `plugin_disabled` 是黑名单，插件默认即启用。
- 真机（华为手表）尚未验收：协议按标准 BLE HR Profile 实现，链路用 demo 源验收过。

## 文档

- `README.md`：用法、配置、排查（hr-daemon 命令行、ini 键表、部署脚本）。
- `PLAN.md`：设计决策与实现期间实测修正的权威记录——改架构/协议前先读对应章节
  （尤其 §10/§13 的实测结论）。
- `THIRD_PARTY_NOTICES.md`：`tm-plugin/PluginInterface.h` 取自 TrafficMonitor，Anti-996
  许可，分发约束见此。

## 风格

- C++20；成员变量 `m_` 前缀；中文注释（与现有代码一致）。
- `scripts\` 里的部署脚本改第三方程序配置前的固定模式：先关目标进程、备份 `.bak`、
  原子写回、只动属于自己的那一行——新脚本沿用这个模式。
