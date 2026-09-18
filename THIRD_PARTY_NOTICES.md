# 第三方组件说明

本仓库自己的代码是 MIT，见 `LICENSE`。下面这几个第三方组件不在 MIT 之下，发布和分发
时要留意。

## `tm-plugin/PluginInterface.h` —— TrafficMonitor 插件接口

- **来源**：https://github.com/zhongyang219/TrafficMonitor，路径 `include/PluginInterface.h`（取自 V1.86 tag）
- **版权**：Copyright (C) by Zhong Yang 2021，文件头保留原样未改动
- **许可**："Anti 996" License Version 1.0 (Draft)，全文见 `third_party/trafficmonitor-LICENSE.txt`

该许可基于 MIT，另加了两条劳工权益条款。它对使用者施加的条件是：

1. 每个再分发或衍生副本上都必须原样保留版权声明和本许可证，不得自行修改。
2. 使用者必须遵守其所在地（或注册地/经营地，以较严格者为准）与劳动就业相关的法律、
   法规、规则和标准。
3. 不得以任何方式诱导、暗示或强迫员工或独立承包人放弃上述法律赋予的权利，也不得限制
   其向版权持有人或监管机构举报违反本许可证的行为。

这个头文件在本仓库里被分发，所以本仓库要一直带着 `third_party/trafficmonitor-LICENSE.txt`
（已包含），`PluginInterface.h` 的文件头版权声明不能删。严格的解读下，第 2/3 条会跟到
包含它的分发件上。

想完全避开这份许可证就删掉 `tm-plugin/`（连任务栏显示一起不要），游戏内 OSD 那条链路
（`ab-plugin/` + Afterburner）不受影响。界面本身是纯虚类声明、靠 vtable 顺序对齐的 ABI，
自己重写一份声明在法律上是灰色地带，本仓库没有走这条路。

## Afterburner 插件接口（`ab-plugin/MonitoringSourceDesc.h`）—— 转写，不是分发

- **来源**：MSI Afterburner 安装目录里的公开 SDK，`<Afterburner>\SDK\Include\MSIAfterburnerMonitoringSourceDesc.h`（结构体定义）和同目录的 `MAHMSharedMemory.h`（数据源 ID 常量）。官方 ReadMe 把这份 SDK 称为 open source SDK，随安装包公开发布，里面带 7 个开源插件样例（CPU / GPU / Ping / SMART / PerfCounter / AIDA64 / HwInfo），用意就是让第三方写插件。
- **版权**：MSI / Alexey Nicolaychuk（Unwind）
- **本仓库怎么处理的**：`ab-plugin/MonitoringSourceDesc.h` 是按字段转写的声明，不是 MSI 文件的副本，仓库里不含任何 MSI 的文件，不存在再分发第三方文件的问题。

转写而不是直接引用本机 SDK 头，有两个原因：原文件是 CP1251 编码（注释里的 `°C` 是单字节
`0xB0`），纳入构建会跟项目统一的 `/utf-8` 冲突；结构体本身是纯 C 的 POD，宿主只按指针
传入，布局一致就是 ABI 一致。转写内容里加了 `static_assert(sizeof(...) == 1580)` 把大小
钉死，端到端验证（真机 Afterburner 读到的名字/单位/数值都对）也确认了字段位置。用得到的
那条数据源 ID 常量（`MONITORING_SOURCE_ID_PLUGIN_MISC`）同样是转写的，没把整份 CP1251 的
`MAHMSharedMemory.h` 拉进来。

## 已移除：RTSS（RivaTuner Statistics Server）SDK 头文件

早先版本在 `third_party/rtss/` 里带过两份 RTSS SDK 头文件（`RTSSSharedMemory.h`、
`RTSSHooksTypes.h`），因为那时 daemon 直接往 `RTSSSharedMemoryV2` 里写 OSD 文本。那两份
文件没有任何许可证声明（打开只有一句功能描述），再分发处于没有明确授权的状态；现在
daemon 不再碰 RTSS、也不再需要它们，整个目录已删除。

Afterburner 的游戏内 OSD 本身就是 RTSS 渲染的（Afterburner 自己不会画覆盖层，它把 OSD
文本交给 RTSS，由 RTSS 去 hook 3D 程序），所以 RTSS 仍然作为渲染器装在机器上，但本项目
与它不再有耦合：不写它的共享内存、不引用它的头文件。

## 其他依赖

本仓库的代码只用系统自带的东西：

| 组件 | 依赖 |
|---|---|
| `hr-daemon.exe` | Windows SDK 的 C++/WinRT 头 + `windowsapp.lib`（BLE）；只写中立共享内存 |
| `HeartRate.dll`（Afterburner 插件） | 仅 `kernel32`（`/MT` 静态 CRT）。实测 `dumpbin /dependents` 只有 KERNEL32.dll |
| `hr_plugin.dll`（TrafficMonitor 插件） | 仅 `kernel32`（`/MT` 静态 CRT） |
| `hr-manager.exe` | GUI 栈用第三方 crate：`wry`（WebView2 绑定）+ `tao` + `tray-icon` + `serde`/`serde_json`，均 MIT/Apache-2.0 双许可，版本由 `tools/hr-manager/Cargo.lock` 锁定；其余部分（配置/进程/注册表/共享内存）只用 Rust 标准库 + 手写 `extern "system"` 声明 |
| `osd_test.exe` | 系统自带的 `d3d11` / `dxgi`（D3D11 画布，用来验证 OSD） |
