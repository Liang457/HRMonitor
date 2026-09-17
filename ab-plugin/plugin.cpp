// ab-plugin/plugin.cpp
// 把 daemon 采到的心率暴露成 MSI Afterburner 的一条硬件监控数据源。
//
// 宿主 MSIAfterburner.exe 的插件契约很小：它按名字 GetProcAddress 找下面三个导出，
// 每个硬件轮询周期（[Settings] HwPollPeriod，默认 1000ms）调一次 GetSourceData()：
//
//   DWORD GetSourcesNum();
//   BOOL  GetSourceDesc(DWORD dwIndex, LPMONITORING_SOURCE_DESC pDesc);
//   FLOAT GetSourceData(DWORD dwIndex);
//
// 可选的 SetupSource(DWORD, HWND) / Uninit() 本插件不实现：配置全部交给 Afterburner
// 自己的"每数据源"覆盖项（名字/分组/量程/颜色/是否进 OSD、托盘、LCD），这正是本次
// 改造的目的——OSD 只留 Afterburner 一个配置入口。不实现 SetupSource 也就不需要 MFC
// （官方 7 个样例都是 MFC 扩展 DLL，因为它们的 SetupSource 要弹对话框），
// 所以本 DLL 是纯 Win32 + /MT 静态 CRT，零依赖。
//
// 必须编成 32 位：MSIAfterburner.exe 是 x86 PE（machine 0x14C），装不了 x64 DLL。
// 数据来自 daemon 写的中立共享内存 Local\HuaWeiHR_SM，与位数无关（按字节布局）。
#include "../common/hr_shared.h"
#include "MonitoringSourceDesc.h"

#include <cfloat>
#include <cstring>
#include <atomic>

namespace {

// 只暴露一条数据源，下标恒为 0。
constexpr DWORD kSourcesNum = 1;

// 曲线默认量程，沿用改造前 OSD 内嵌曲线的 40~180 bpm。
constexpr FLOAT kMinLimit = 40.0f;
constexpr FLOAT kMaxLimit = 180.0f;

// 共享内存读者。打开一次长期持有即可：daemon 退出后映射对象依然存在（我们握着句柄），
// daemon 重启时 CreateFileMappingW 拿到的是同一个对象，所以不需要重开或重试逻辑。
// 宿主实践中是单线程轮询（见官方 ReadMe 里 GetTimestamp 的描述），但打开路径
// 仍用原子状态守一下：真有多线程调进来，也不会并发双开泄漏句柄。
HrSharedReader g_reader;
constexpr ULONGLONG kOpenRetryMs = 1000;    // 打不开时最多 1 秒试一次（HwPollPeriod 可调到很低）

FLOAT ReadHeartRate()
{
    if (!g_reader.IsOpen()) {
        const ULONGLONG now = GetTickCount64();
        static std::atomic<ULONGLONG> s_lastTry{ 0 };
        ULONGLONG last = s_lastTry.load(std::memory_order_relaxed);
        if (now - last < kOpenRetryMs) return FLT_MAX;
        if (!s_lastTry.compare_exchange_strong(last, now, std::memory_order_relaxed))
            return FLT_MAX;                 // 另一个线程正在试
        if (!g_reader.Open())
            return FLT_MAX;
    }

    HrSharedData d;
    if (!g_reader.Read(d))
        return FLT_MAX;

    // daemon 会把自己判定的超时写进 status，所以以它为准；
    // HrEffectiveBpm 里的 tick_ms 超时检查只是 daemon 非正常退出时的兜底
    // （bpm > 300 的毛刺也在这里被挡掉）。
    const LONG bpm = HrEffectiveBpm(d, GetTickCount64());

    // FLT_MAX 是 MAHM 共享内存里"该数据源当前不可用"的约定值。
    return (bpm < 0) ? FLT_MAX : (FLOAT)bpm;
}

}  // namespace

extern "C" {

// 插件里有多少条数据源。
__declspec(dllexport) DWORD GetSourcesNum()
{
    return kSourcesNum;
}

// 填第 dwIndex 条数据源的描述。宿主只保证填好了 dwVersion。
__declspec(dllexport) BOOL GetSourceDesc(DWORD dwIndex, LPMONITORING_SOURCE_DESC pDesc)
{
    if (dwIndex >= kSourcesNum || !pDesc)
        return FALSE;
    // 官方头文件注释："由宿主填好，>= 0x00010000 才会用这个结构体"。宿主用更老/更小
    // 的结构调进来时，下面的 memset 会写穿宿主的堆——不做这个检查是越界写。
    if (pDesc->dwVersion < 0x00010000u)
        return FALSE;

    // 其余字段可能是上一个数据源留下的内容，先整体清零再填。
    // 但绝不能把 dwVersion 一起清掉——官方头文件原文：
    // "Don't change this field when filling the descriptor!"
    const DWORD dwVersion = pDesc->dwVersion;
    memset(pDesc, 0, sizeof(*pDesc));
    pDesc->dwVersion = dwVersion;

    strcpy_s(pDesc->szName, sizeof(pDesc->szName), "Heart rate");
    strcpy_s(pDesc->szUnits, sizeof(pDesc->szUnits), "BPM");
    strcpy_s(pDesc->szGroup, sizeof(pDesc->szGroup), "Heart rate");
    // szFormat 留空 = 用宿主默认的 %.0f（心率本来就是整数，不用自己定格式）。
    // szNameTemplate / szGroupTemplate 留空 = 单实例，名字里不带序号。

    pDesc->dwID = MONITORING_SOURCE_ID_PLUGIN_MISC;   // 插件自定数据源
    pDesc->dwInstance = dwIndex;
    pDesc->fltMinLimit = kMinLimit;
    pDesc->fltMaxLimit = kMaxLimit;

    return TRUE;
}

// 宿主每个轮询周期调一次，取当前值。
__declspec(dllexport) FLOAT GetSourceData(DWORD dwIndex)
{
    if (dwIndex >= kSourcesNum)
        return FLT_MAX;

    return ReadHeartRate();
}

}  // extern "C"
