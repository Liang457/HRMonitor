// daemon/ble.cpp — C++/WinRT 采集华为手表"心率广播"
//
// 标准 BLE Heart Rate Profile：
//   服务    0000180d-0000-1000-8000-00805f9b34fb
//   测量特征 00002a37-0000-1000-8000-00805f9b34fb （notify）
//   flags bit0 = 0 → bpm 为 uint8；=1 → bpm 为 uint16 小端
//
// 只依赖 Windows SDK 自带的 cppwinrt 头，链接 windowsapp.lib，无第三方库。
#include "hr_source.h"

#include "../common/hr_config.h"

#include <winrt/base.h>
#include <winrt/Windows.Foundation.h>
#include <winrt/Windows.Foundation.Collections.h>
#include <winrt/Windows.Devices.Bluetooth.h>
#include <winrt/Windows.Devices.Bluetooth.Advertisement.h>
#include <winrt/Windows.Devices.Bluetooth.GenericAttributeProfile.h>
#include <winrt/Windows.Storage.Streams.h>

#include <atomic>
#include <chrono>
#include <condition_variable>
#include <cstdio>
#include <functional>
#include <map>
#include <memory>
#include <mutex>
#include <string>
#include <thread>

#include "log.h"

namespace {

namespace WBluetooth = winrt::Windows::Devices::Bluetooth;
namespace WAdv       = winrt::Windows::Devices::Bluetooth::Advertisement;
namespace WGatt      = winrt::Windows::Devices::Bluetooth::GenericAttributeProfile;
namespace WStreams   = winrt::Windows::Storage::Streams;

std::string Narrow(const winrt::hstring& h) { return ToUtf8(std::wstring(h.c_str())); }

std::string MacToString(unsigned long long a) { return ToUtf8(HrFormatMac(a)); }

// WinRT 异步操作的等待上限。蓝牙栈真卡住时 .get() 是不回来的，而 Stop() 要
// join 采集线程，于是 daemon 就关不掉了（互斥体一直被占，下次启动被当成"已经在跑"）。
// 所以一律带 deadline：超时就 Cancel，当这次尝试失败，交给重连退避重来。
constexpr DWORD kConnectTimeoutMs = 10000;   // 连接、服务与特征发现
constexpr DWORD kGattTimeoutMs    = 5000;    // 订阅 / 退订
// 扫到设备之后再多等一会儿，把广播里的名字收全
constexpr int   kNameGraceMs      = 1500;

// 发起 create()（一个 WinRT 异步操作）并最多等 timeoutMs 毫秒。
// 不用 Completed 回调而是轮询 Status()：回调可能在别的线程池线程上派发，
// 生命周期不好管，轮询没这个问题，代价只是连接路径上每秒几十次属性读。
// quiet=true 时不记警告（收尾阶段的失败是常态，不值得刷日志）。
template <typename TCreate, typename T>
bool AwaitOp(TCreate&& create, DWORD timeoutMs, T& out, const char* what, bool quiet = false) {
    using TOp = decltype(create());

    TOp op{ nullptr };
    try {
        op = create();
    } catch (const winrt::hresult_error& e) {
        if (!quiet) LogWarn("BLE: %s 发起失败 0x%08X %s",
                            what, (unsigned)e.code().value, Narrow(e.message()).c_str());
        return false;
    } catch (const std::exception& e) {
        if (!quiet) LogWarn("BLE: %s 发起失败 %s", what, e.what());
        return false;
    }
    if (!op) {
        if (!quiet) LogWarn("BLE: %s 拿到的是空操作", what);
        return false;
    }

    const ULONGLONG deadline = GetTickCount64() + timeoutMs;
    while (op.Status() == winrt::Windows::Foundation::AsyncStatus::Started) {
        if (GetTickCount64() >= deadline) {
            if (!quiet) LogWarn("BLE: %s 超过 %lu 毫秒没完成，放弃本次尝试", what, timeoutMs);
            try { op.Cancel(); } catch (...) {}
            return false;
        }
        Sleep(20);
    }

    if (op.Status() != winrt::Windows::Foundation::AsyncStatus::Completed) {
        if (!quiet) {
            // ErrorCode() 返回的是裸 hresult（没有 message()）；只有确实是失败码
            // 才能拿它构造 hresult_error —— 那个构造函数里有 WINRT_ASSERT(code < 0)。
            const winrt::hresult hr = op.ErrorCode();
            if (hr.value < 0) {
                const winrt::hresult_error err{ hr };
                LogWarn("BLE: %s 失败 0x%08X %s",
                        what, (unsigned)hr.value, Narrow(err.message()).c_str());
            } else {
                LogWarn("BLE: %s 被取消（0x%08X）", what, (unsigned)hr.value);
            }
        }
        return false;
    }
    out = op.GetResults();
    return true;
}

// 扫描：按服务 UUID 0x180D 过滤。Windows 蓝牙栈在广播层做过滤，
// 与 bleak 在 Windows 上的做法一致（bleak 内部用的就是同一组 WinRT API）。
//
// 收集匹配设备（同一个地址可能广播多次，保留第一次见到的名字）。
// stop 非空时置位即可中断等待（daemon 退出要能立刻收手）。
// stopOnFirst=true 时扫到一台就走（常驻采集只要一台，不用白等满整轮）。
// emit 非空时，每发现一台新设备、以及某台的后到的名字补全，都会回调一次——
// --scan 的流式输出靠它边扫边吐。
// 返回 false 表示扫描器起不来（蓝牙关了等）。
using ScanEmit = std::function<void(unsigned long long addr, const std::wstring& name)>;
bool ScanCollect(int timeoutMs, std::map<unsigned long long, std::wstring>& found,
                 const std::atomic<bool>* stop, bool stopOnFirst,
                 const ScanEmit& emit = {}) {
    WAdv::BluetoothLEAdvertisementWatcher watcher;
    watcher.ScanningMode(WAdv::BluetoothLEScanningMode::Active);
    watcher.AdvertisementFilter().Advertisement().ServiceUuids().Append(
        WGatt::GattServiceUuids::HeartRate());

    std::mutex              m;
    std::condition_variable cv;

    auto recvTok = watcher.Received(
        [&](const WAdv::BluetoothLEAdvertisementWatcher&,
            const WAdv::BluetoothLEAdvertisementReceivedEventArgs& args) {
            const unsigned long long a = args.BluetoothAddress();
            std::wstring n(args.Advertisement().LocalName().c_str());
            bool report = false;
            {
                std::lock_guard<std::mutex> lk(m);
                auto it = found.find(a);
                if (it == found.end()) {
                    found.emplace(a, n);
                    report = true;       // 新设备
                } else if (it->second.empty() && !n.empty()) {
                    it->second = n;      // 后一条广播补上了名字
                    report = true;
                }
                if (report) cv.notify_all();   // 让等待方尽快重新判断（可能要提前收手）
            }
            // 回调放在锁外：它可能往管道里写阻塞数据，别把等 cv 的扫描主循环也卡住
            if (report && emit) emit(a, n);
        });

    auto stopTok = watcher.Stopped(
        [&](const WAdv::BluetoothLEAdvertisementWatcher&,
            const WAdv::BluetoothLEAdvertisementWatcherStoppedEventArgs& args) {
            LogWarn("BLE: 扫描器被系统停止，原因=%d", (int)args.Error());
            std::lock_guard<std::mutex> lk(m);
            cv.notify_all();
        });

    try {
        watcher.Start();
    } catch (const winrt::hresult_error& e) {
        LogError("BLE: 无法启动扫描 0x%08X %s（蓝牙适配器关了？）",
                 (unsigned)e.code().value, Narrow(e.message()).c_str());
        watcher.Received(recvTok);
        watcher.Stopped(stopTok);
        return false;
    }

    LogInfo("BLE: 开始扫描心率广播设备（服务 0x180D，最长 %d 秒）...", timeoutMs / 1000);

    {
        std::unique_lock<std::mutex> lk(m);
        const auto deadline = std::chrono::steady_clock::now() + std::chrono::milliseconds(timeoutMs);
        std::chrono::steady_clock::time_point firstHit{};
        while (std::chrono::steady_clock::now() < deadline) {
            if (stop && stop->load()) break;
            if (stopOnFirst && !found.empty()) {
                if (firstHit == std::chrono::steady_clock::time_point{})
                    firstHit = std::chrono::steady_clock::now();
                // 名字拿到就走；拿不到也只再多等一小会儿（连接后还能从
                // device.Name() 补上），不为一个名字白等满整轮。
                if (!found.begin()->second.empty()) break;
                if (std::chrono::steady_clock::now() - firstHit >=
                    std::chrono::milliseconds(kNameGraceMs)) break;
            }
            cv.wait_for(lk, std::chrono::milliseconds(100));
        }
    }

    try { watcher.Stop(); } catch (...) {}
    watcher.Received(recvTok);
    watcher.Stopped(stopTok);
    return true;
}

// 返回 true 表示"确实连上过"（用于决定重连退避）。
bool ConnectAndStream(HrSink& sink, const std::atomic<bool>& stop,
                      unsigned long long address, const std::wstring& nameHint,
                      int timeoutMs) {
    LogInfo("BLE: 正在连接 %s ...", MacToString(address).c_str());

    WBluetooth::BluetoothLEDevice device{ nullptr };
    if (!AwaitOp([&] { return WBluetooth::BluetoothLEDevice::FromBluetoothAddressAsync(address); },
                 kConnectTimeoutMs, device, "FromBluetoothAddressAsync"))
        return false;
    if (!device) {
        LogWarn("BLE: 找不到设备 %s（不在范围内，或地址已失效）", MacToString(address).c_str());
        return false;
    }

    // 事件回调可能在本次调用返回之后才被派发完，所以状态放堆上、由 lambda 按值
    // 持有 shared_ptr。按引用捕获栈上局部量会变成 use-after-free。
    struct StreamState {
        std::atomic<unsigned long long> lastNotify{ GetTickCount64() };
        std::atomic<int>                notifies{ 0 };
        std::atomic<bool>               disconnected{ false };
    };
    auto state = std::make_shared<StreamState>();

    auto connTok = device.ConnectionStatusChanged([state](const auto& d, const auto&) {
        if (d.ConnectionStatus() == WBluetooth::BluetoothConnectionStatus::Disconnected)
            state->disconnected = true;
    });

    // 收尾统一走这个 lambda，避免每条失败路径漏掉事件注销。
    auto fail = [&](const char* what) {
        LogWarn("BLE: %s", what);
        device.ConnectionStatusChanged(connTok);
        try { device.Close(); } catch (...) {}
        return false;
    };

    // ---- 心率服务：先用缓存，空了再用 Uncached 强制重新发现
    WGatt::GattDeviceServicesResult svcRes{ nullptr };
    bool svcOk = AwaitOp(
        [&] {
            return device.GetGattServicesForUuidAsync(WGatt::GattServiceUuids::HeartRate());
        },
        kConnectTimeoutMs, svcRes, "获取心率服务");

    if (svcOk && (svcRes.Status() != WGatt::GattCommunicationStatus::Success ||
                  svcRes.Services().Size() == 0)) {
        LogInfo("BLE: 缓存里没有心率服务，改为强制发现（Uncached）");
        svcOk = AwaitOp(
            [&] {
                return device.GetGattServicesForUuidAsync(
                    WGatt::GattServiceUuids::HeartRate(),
                    WBluetooth::BluetoothCacheMode::Uncached);
            },
            kConnectTimeoutMs, svcRes, "强制发现心率服务");
    }
    if (!svcOk) return fail("心率服务查询失败");

    if (svcRes.Status() != WGatt::GattCommunicationStatus::Success ||
        svcRes.Services().Size() == 0) {
        char buf[128];
        _snprintf_s(buf, sizeof(buf), _TRUNCATE,
                    "未找到心率服务 0x180D（status=%d, count=%u）——手表可能没开\"心率广播\"",
                    (int)svcRes.Status(), (unsigned)svcRes.Services().Size());
        return fail(buf);
    }

    auto svc = svcRes.Services().GetAt(0);

    // ---- 心率测量特征
    WGatt::GattCharacteristicsResult chrRes{ nullptr };
    bool chrOk = AwaitOp(
        [&] {
            return svc.GetCharacteristicsForUuidAsync(
                WGatt::GattCharacteristicUuids::HeartRateMeasurement());
        },
        kConnectTimeoutMs, chrRes, "获取测量特征");

    if (chrOk && (chrRes.Status() != WGatt::GattCommunicationStatus::Success ||
                  chrRes.Characteristics().Size() == 0)) {
        LogInfo("BLE: 缓存里没有测量特征，改为强制发现（Uncached）");
        chrOk = AwaitOp(
            [&] {
                return svc.GetCharacteristicsForUuidAsync(
                    WGatt::GattCharacteristicUuids::HeartRateMeasurement(),
                    WBluetooth::BluetoothCacheMode::Uncached);
            },
            kConnectTimeoutMs, chrRes, "强制发现测量特征");
    }
    if (!chrOk) return fail("心率测量特征查询失败");

    if (chrRes.Status() != WGatt::GattCommunicationStatus::Success ||
        chrRes.Characteristics().Size() == 0)
        return fail("未找到心率测量特征 0x2A37");

    auto chr = chrRes.Characteristics().GetAt(0);

    // ---- 订阅 notify
    WGatt::GattCommunicationStatus subSt = WGatt::GattCommunicationStatus::Unreachable;
    if (!AwaitOp(
            [&] {
                return chr.WriteClientCharacteristicConfigurationDescriptorAsync(
                    WGatt::GattClientCharacteristicConfigurationDescriptorValue::Notify);
            },
            kGattTimeoutMs, subSt, "订阅 notify") ||
        subSt != WGatt::GattCommunicationStatus::Success) {
        char buf[96];
        _snprintf_s(buf, sizeof(buf), _TRUNCATE, "订阅 notify 失败（status=%d）", (int)subSt);
        return fail(buf);
    }

    // ---- 数据回调
    auto valTok = chr.ValueChanged([state, &sink](const WGatt::GattCharacteristic&,
                                                  const WGatt::GattValueChangedEventArgs& args) {
        try {
            WStreams::DataReader reader =
                WStreams::DataReader::FromBuffer(args.CharacteristicValue());
            reader.ByteOrder(WStreams::ByteOrder::LittleEndian);
            const uint8_t flags = reader.ReadByte();
            const int bpm = (flags & 0x01) ? (int)reader.ReadUInt16()
                                           : (int)reader.ReadByte();
            if (bpm > 0 && bpm < 300) {
                state->lastNotify = GetTickCount64();
                state->notifies.fetch_add(1);
                sink.SetBpm(bpm);
            }
        } catch (...) {
            // 单条通知畸形不影响后续
        }
    });

    std::wstring devName = device.Name().c_str();
    if (devName.empty()) devName = nameHint;
    if (!devName.empty()) sink.SetDevice(devName);
    sink.SetStatus(HRS_OK);
    LogInfo("BLE: 已连接 %s，已订阅心率通知",
            devName.empty() ? "(未知设备)" : ToUtf8(devName).c_str());

    // ---- 等到断开 / 数据超时 / 收到退出信号
    // 超时值用配置里的 display.timeout_ms，和 daemon 自己判超时、插件兜底用的
    // 是同一个数；以前这里写死 15 秒，用户把超时调大也照样 15 秒就断链重连。
    while (!stop.load()) {
        Sleep(100);                          // 小步走，退出时不用等太久
        if (state->disconnected) { LogWarn("BLE: 连接状态变为已断开"); break; }
        if (GetTickCount64() - state->lastNotify.load() > (ULONGLONG)timeoutMs) {
            LogWarn("BLE: 已 %d 秒没有收到心率通知，判定连接失效", timeoutMs / 1000);
            break;
        }
    }

    // ---- 清理。退订失败是常态（连接已经没了），所以静默处理。
    chr.ValueChanged(valTok);
    device.ConnectionStatusChanged(connTok);
    WGatt::GattCommunicationStatus ignored = WGatt::GattCommunicationStatus::Unreachable;
    AwaitOp(
        [&] {
            return chr.WriteClientCharacteristicConfigurationDescriptorAsync(
                WGatt::GattClientCharacteristicConfigurationDescriptorValue::None);
        },
        kGattTimeoutMs, ignored, "退订 notify", /*quiet=*/true);
    try { device.Close(); } catch (...) {}

    LogInfo("BLE: 本次连接结束，共收到 %d 条心率通知", state->notifies.load());
    return true;
}

class BleSource final : public HrSource {
public:
    BleSource(HrSink& sink, const BleConfig& cfg) : m_sink(sink), m_cfg(cfg) {}
    ~BleSource() override { Stop(); }

    bool Start(std::string& err) override {
        try {
            m_thread = std::thread([this] { Run(); });
        } catch (const std::exception& e) {
            err = e.what();
            return false;
        }
        return true;
    }

    void Stop() override {
        m_stop = true;
        if (m_thread.joinable()) m_thread.join();
    }

private:
    void Run() {
        try {
            // 后台线程用 MTA，避免 STA 消息泵缺失导致 .get() 卡死
            winrt::init_apartment(winrt::apartment_type::multi_threaded);
        } catch (const winrt::hresult_error& e) {
            LogError("BLE: init_apartment 失败 0x%08X %s —— 蓝牙栈不可用",
                     (unsigned)e.code().value, Narrow(e.message()).c_str());
            m_sink.SetStatus(HRS_CONNECTING);
            return;
        }

        int backoffSec = (m_cfg.backoff_min_sec < 1) ? 1 : m_cfg.backoff_min_sec;
        const int streamTimeoutMs = (m_cfg.timeout_ms < 1000) ? 1000 : m_cfg.timeout_ms;

        // main 只在配了地址时才建 BLE 源（未配置 = 不连接，免得多设备环境连错表）。
        // 这里留一道一行版的防御：万一哪天被建出来了，也别自作主张去扫描乱连。
        if (!m_cfg.haveAddress) {
            LogError("BLE: 未配置手表地址却建了 BLE 源（不该发生），采集线程退出");
            return;
        }

        while (!m_stop) {
            bool live = false;
            try {
                LogInfo("BLE: 使用配置里的地址直连");
                live = ConnectAndStream(m_sink, m_stop, m_cfg.address, m_cfg.nameHint,
                                        streamTimeoutMs);
            } catch (const winrt::hresult_error& e) {
                LogWarn("BLE: 未处理异常 0x%08X %s",
                        (unsigned)e.code().value, Narrow(e.message()).c_str());
            } catch (const std::exception& e) {
                LogWarn("BLE: 未处理异常 %s", e.what());
            } catch (...) {
                LogWarn("BLE: 未处理异常（未知类型）");
            }

            if (m_stop) break;

            m_sink.SetStatus(HRS_CONNECTING);

            if (live) backoffSec = m_cfg.backoff_min_sec;   // 连上过 → 快速重连
            if (backoffSec < 1) backoffSec = 1;             // 兜住非法配置，别退化成忙等
            LogInfo("BLE: %d 秒后重试", backoffSec);
            for (int i = 0; i < backoffSec * 10 && !m_stop; ++i) Sleep(100);

            if (!live && backoffSec < m_cfg.backoff_max_sec) {
                backoffSec *= 2;
                if (backoffSec > m_cfg.backoff_max_sec) backoffSec = m_cfg.backoff_max_sec;
            }
        }

        LogInfo("BLE: 采集线程退出");
        // 线程即将结束，不调用 uninit_apartment —— MTA 下带着未释放对象
        // 反初始化有崩溃风险，进程退出时由系统回收。
    }

    HrSink&           m_sink;
    BleConfig         m_cfg;
    std::atomic<bool> m_stop{ false };
    std::thread       m_thread;
};

} // namespace

std::unique_ptr<HrSource> MakeBleSource(HrSink& sink, const BleConfig& cfg) {
    return std::make_unique<BleSource>(sink, cfg);
}

// ---------------------------------------------------------------- --scan 流式输出

namespace {

// --scan 的输出句柄，拿到一次就一直用。
// 被 hr-manager 拉起时，stdout 是继承来的管道（STARTF_USESTDHANDLES），
// GetStdHandle 直接拿得到；从 cmd 手动跑时是 GUI 子系统没有标准句柄，
// 就附加到父控制台后打开 CONOUT$。两种情况都按字节写 UTF-8。
HANDLE ScanOutHandle() {
    static HANDLE g_out   = INVALID_HANDLE_VALUE;
    static bool   g_tried = false;
    if (g_tried) return g_out;
    g_tried = true;

    HANDLE h = GetStdHandle(STD_OUTPUT_HANDLE);
    if (h && h != INVALID_HANDLE_VALUE) {
        g_out = h;
        return g_out;
    }
    if (GetConsoleWindow() == nullptr)
        AttachConsole(ATTACH_PARENT_PROCESS);   // 没有父控制台就算了，输出丢弃
    if (GetConsoleWindow() != nullptr) {
        SetConsoleOutputCP(CP_UTF8);
        g_out = CreateFileW(L"CONOUT$", GENERIC_WRITE, FILE_SHARE_WRITE,
                            nullptr, OPEN_EXISTING, 0, nullptr);
    }
    return g_out;
}

bool ScanWriteLine(const std::string& utf8) {
    HANDLE h = ScanOutHandle();
    if (h == INVALID_HANDLE_VALUE) return false;
    DWORD written = 0;
    // 写失败（读者没了/管道断了）不当作错误：扫描照常跑完，退出码只反映扫描本身
    WriteFile(h, utf8.data(), (DWORD)utf8.size(), &written, nullptr);
    return true;
}

// 设备名里可能有引号/反斜杠/控制字符（名字来自广播数据，别信它乖）。
// UTF-8 字节本身原样通过。
std::string JsonEscape(const std::string& utf8) {
    std::string out;
    out.reserve(utf8.size() + 8);
    for (unsigned char c : utf8) {
        switch (c) {
        case '"':  out += "\\\""; break;
        case '\\': out += "\\\\"; break;
        case '\b': out += "\\b";  break;
        case '\f': out += "\\f";  break;
        case '\n': out += "\\n";  break;
        case '\r': out += "\\r";  break;
        case '\t': out += "\\t";  break;
        default:
            if (c < 0x20) {
                char buf[8];
                snprintf(buf, sizeof(buf), "\\u%04X", c);
                out += buf;
            } else {
                out += (char)c;
            }
        }
    }
    return out;
}

void ScanEmitJson(const char* type, unsigned long long addr, const std::wstring& name) {
    std::string line = "{\"type\":\"";
    line += type;
    line += "\",\"mac\":\"";
    line += ToUtf8(HrFormatMac(addr));
    line += "\",\"name\":\"";
    line += JsonEscape(ToUtf8(name));
    line += "\"}\r\n";
    ScanWriteLine(line);
}

} // namespace

int BleScanStream(int seconds) {
    try {
        winrt::init_apartment(winrt::apartment_type::multi_threaded);
    } catch (const winrt::hresult_error& e) {
        LogError("BLE: init_apartment 失败 0x%08X —— 蓝牙栈不可用", (unsigned)e.code().value);
        return -1;
    }

    if (seconds < 1)    seconds = 1;
    if (seconds > 120)  seconds = 120;

    std::map<unsigned long long, std::wstring> found;
    // 一次性扫描：要列全，所以不提前收手。emit 边扫边把新设备/新名字推给读者，
    // hr-manager 的面板因此能实时刷新列表。
    const ScanEmit emit = [](unsigned long long a, const std::wstring& n) {
        // 消费端按 mac 去重、后到的名字覆盖前面的，所以统一用 device 类型即可
        ScanEmitJson("device", a, n);
    };
    if (!ScanCollect(seconds * 1000, found, nullptr, /*stopOnFirst=*/false, emit)) return -1;

    {
        // 收尾行借 mac 字段带设备数出去（消费端按 type 分流）
        char buf[64];
        snprintf(buf, sizeof(buf), "{\"type\":\"done\",\"count\":%d}\r\n", (int)found.size());
        ScanWriteLine(buf);
    }
    LogInfo("BLE: 扫描结束，找到 %zu 台设备", found.size());
    return (int)found.size();
}
