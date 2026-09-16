// daemon/hr_source.h — 心率数据源抽象 + 线程安全汇聚点
#pragma once

#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#include <windows.h>

#include <memory>
#include <mutex>
#include <string>

#include "../common/hr_shared.h"

// 数据源（BLE 或 demo）把最新值推到这里；主循环只从这里读。
// 所有方法线程安全。
class HrSink {
public:
    void SetBpm(int bpm) {
        std::lock_guard<std::mutex> lk(m_mtx);
        m_bpm    = bpm;
        m_lastMs = GetTickCount64();
        m_status = HRS_OK;
    }

    // 连接状态变化（扫描中/已连接/已断开）。
    void SetStatus(DWORD st) {
        std::lock_guard<std::mutex> lk(m_mtx);
        m_status = st;
    }

    void SetDevice(const std::wstring& name) {
        std::lock_guard<std::mutex> lk(m_mtx);
        m_device = name;
    }

    struct Snapshot {
        int          bpm    = -1;
        ULONGLONG    lastMs = 0;      // 最近一次收到通知的时刻，0 = 从未收到
        DWORD        status = HRS_CONNECTING;
        std::wstring device;
    };

    Snapshot Get() {
        std::lock_guard<std::mutex> lk(m_mtx);
        return Snapshot{ m_bpm, m_lastMs, m_status, m_device };
    }

private:
    std::mutex   m_mtx;
    int          m_bpm    = -1;
    ULONGLONG    m_lastMs = 0;
    DWORD        m_status = HRS_CONNECTING;
    std::wstring m_device;
};

// 数据源统一接口：Start 起后台线程，Stop 请求退出并 join。
class HrSource {
public:
    virtual ~HrSource() = default;
    virtual bool Start(std::string& err) = 0;
    virtual void Stop() = 0;
};

// demo.cpp
std::unique_ptr<HrSource> MakeDemoSource(HrSink& sink);

// ble.cpp
struct BleConfig {
    bool               haveAddress   = false;   // 给了地址就跳过扫描直连
    unsigned long long address       = 0;       // BLE MAC（小端 48 位）
    std::wstring       nameHint;                // 日志里用的名字
    int                scan_timeout_ms = 20000; // 每轮扫描最长时长
    int                backoff_min_sec = 1;     // 重连退避下限
    int                backoff_max_sec = 30;    // 重连退避上限
    // 多久没收到心率通知就判定连接失效（display.timeout_ms）。
    // 必须和 daemon 判超时、插件兜底用的是同一个数，否则界面显示的和底层行为不一致。
    int                timeout_ms      = 15000;
};
std::unique_ptr<HrSource> MakeBleSource(HrSink& sink, const BleConfig& cfg);

// 扫描-only 模式（供 hr-config.exe 枚举设备用）：
// 扫 seconds 秒，把每个设备写成一行 "<MAC>\t<名字>"（UTF-8）到 outPath，
// 返回发现的设备数；-1 表示扫描起不来。
int BleScanToFile(int seconds, const std::wstring& outPath);
