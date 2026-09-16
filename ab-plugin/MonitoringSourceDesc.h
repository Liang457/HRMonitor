// ab-plugin/MonitoringSourceDesc.h
// MONITORING_SOURCE_DESC —— MSI Afterburner 硬件监控插件的"数据源描述"结构。
//
// 出处：MSI Afterburner 安装目录下的公开 SDK
//       <Afterburner>\SDK\Include\MSIAfterburnerMonitoringSourceDesc.h
//       （官方 ReadMe 把这份 SDK 称作 open source SDK，随安装包公开发布）
//
// 这里是**逐字段转写**，不是原文件的副本。原因有两个：
//   1. 原文件是 CP1251 编码（注释里的 °C 是单字节 0xB0），编进来会跟 /utf-8 冲突；
//   2. 转写之后仓库里不含任何第三方文件，省掉一份再分发说明。
//
// 这是个纯 C 的 POD，宿主按指针传进来，只要布局一致就是 ABI 一致的，
// 下面的 static_assert 把大小钉死（6 个 char[MAX_PATH] + 5 个 4 字节字段 = 1580）。
#pragma once

#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#include <windows.h>   // MAX_PATH, DWORD, BOOL, FLOAT

typedef struct MONITORING_SOURCE_DESC {
    DWORD dwVersion;
        // 描述符版本 ((major<<16) + minor)
        // 由宿主填好，>= 0x00010000 才会用这个结构体。
        // 官方头文件原文：Don't change this field when filling the descriptor!
        // —— 填描述符时绝对不要动它。

    char szName[MAX_PATH];     // 数据源名字，如 "GPU temperature"
    char szUnits[MAX_PATH];    // 单位，如 "C"
    char szFormat[MAX_PATH];   // 可选的输出格式；留空表示用默认的 %.0f
    char szGroup[MAX_PATH];    // 分组名，OSD / 罗技键盘 LCD 等按它分组

    DWORD dwID;        // 数据源 ID，取 MONITORING_SOURCE_ID_...
    DWORD dwInstance;  // 从 0 开始的数据源实例下标（多 GPU 时用来区分 GPU1/GPU2）

    FLOAT fltMaxLimit; // 曲线默认上限
    FLOAT fltMinLimit; // 曲线默认下限

    char szNameTemplate[MAX_PATH];   // 多实例名字模板，如 "GPU%d temperature"
    char szGroupTemplate[MAX_PATH];  // 多实例分组模板，如 "GPU%d"
} MONITORING_SOURCE_DESC, *LPMONITORING_SOURCE_DESC;

static_assert(sizeof(MONITORING_SOURCE_DESC) == 1580,
              "MONITORING_SOURCE_DESC 布局与 Afterburner SDK 不符");

// ---------------------------------------------------------------- 数据源 ID

// 取官方 SDK <Afterburner>\SDK\Include\MAHMSharedMemory.h 里的一个值。
// 那份头文件是 CP1251 编码，所以只转写这一条用得到的常量，而不是整份引入。
// 心率在这套 ID 体系里没有专用项，用 MISC（插件自定的杂项数据源）最诚实。
// 插件自定的 ID 落在 0xF0~0xFF，Afterburner 的 OSD 配色规则按 ID 分组，
// 0xFF 已经包含在默认布局的插件分组里（见 [OSDLayout1] 的 GroupColorTag）。
#define MONITORING_SOURCE_ID_PLUGIN_MISC 0x000000FF
