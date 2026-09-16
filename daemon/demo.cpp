// daemon/demo.cpp — 无手表也能跑通全链路的模拟心率源（60~180，1Hz）
#include "hr_source.h"

#include <cmath>
#include <memory>
#include <random>
#include <thread>

#include "log.h"

namespace {

class DemoSource final : public HrSource {
public:
    explicit DemoSource(HrSink& sink) : m_sink(sink) {}
    ~DemoSource() override { Stop(); }

    bool Start(std::string& err) override {
        (void)err;
        m_sink.SetDevice(L"DEMO 模拟心率");
        try {
            m_thread = std::thread([this] { Run(); });
        } catch (const std::exception& e) {
            err = e.what();
            return false;
        }
        LogInfo("demo: 模拟心率源已启动（60~180 bpm，1Hz）");
        return true;
    }

    void Stop() override {
        m_stop = true;
        if (m_thread.joinable()) m_thread.join();
    }

private:
    void Run() {
        std::mt19937 rng{ std::random_device{}() };
        std::normal_distribution<double> noise(0.0, 1.8);

        constexpr double kPi = 3.14159265358979323846;
        double t = 0.0;      // 秒
        double v = 78.0;     // 当前心率

        while (!m_stop) {
            // 90 秒一个周期的缓慢起伏（模拟有氧爬升/恢复）+ 小幅噪声，
            // 这样任务栏和 OSD 曲线都看得出在动。
            const double target = 108.0 + 37.0 * std::sin(t * 2.0 * kPi / 90.0);
            v += (target - v) * 0.25 + noise(rng);
            if (v < 60.0)  v = 60.0;
            if (v > 180.0) v = 180.0;

            m_sink.SetBpm((int)std::lround(v));
            t += 1.0;

            // 拆成 10×100ms，Stop() 能及时返回
            for (int i = 0; i < 10 && !m_stop; ++i) Sleep(100);
        }
        LogInfo("demo: 模拟心率源已停止");
    }

    HrSink&           m_sink;
    std::atomic<bool> m_stop{ false };
    std::thread       m_thread;
};

} // namespace

std::unique_ptr<HrSource> MakeDemoSource(HrSink& sink) {
    return std::make_unique<DemoSource>(sink);
}
