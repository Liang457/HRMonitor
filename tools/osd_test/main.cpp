// tools/osd_test/main.cpp
//
// HR OSD Test - a minimal but *valid* D3D11 application.
//
// RTSS (RivaTuner Statistics Server) only draws its OSD on top of a running
// 3D/Direct3D application that it has hooked.  This tool is nothing more than
// such a "sink": an overlapped HWND plus a D3D11 device/swap chain that
// presents real frames steadily, so the OSD can be verified without launching a
// real game.
//
// Note: MSI Afterburner does not draw an overlay itself -- it hands its OSD
// text to RTSS, which is the component that actually hooks the 3D app. So this
// canvas is still what you need to check the OSD after configuring a data
// source (e.g. "Heart rate") to show up there. hr-daemon does not touch RTSS at
// all anymore; the data reaches the OSD through the HeartRate.dll plugin.
//
// No font library, no shader compiler and no external dependency: the frame is
// an animated clear of the back buffer (visually obvious that the app is alive)
// and the frame counter is reported on stdout instead of being drawn.
// One frame per vsync tick is presented, so the rate tracks the display
// refresh rate (~30-60 FPS); a steady stream of real frames is what the OSD
// renderer needs.
//
// Build: tools\osd_test\build.cmd
// Exit : Esc or close the window.

#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#ifndef _WIN32_WINNT
#define _WIN32_WINNT 0x0601
#endif

#include <windows.h>
#include <d3d11.h>
#include <dxgi.h>

#include <wrl/client.h>   // Microsoft::WRL::ComPtr, ships with the Windows SDK

#include <cmath>
#include <cstdio>

using Microsoft::WRL::ComPtr;

namespace {

constexpr UINT    kWidth       = 1280;
constexpr UINT    kHeight      = 720;
constexpr wchar_t kClassName[] = L"HR_OSD_Test_D3D11_Class";
constexpr wchar_t kTitle[]     = L"HR OSD Test - D3D11";

// A year of frames at 60 FPS; keeps the HUD readable.
constexpr DWORD kReportIntervalMs = 2000;

// ---------------------------------------------------------------- globals
// The window procedure needs to reach the swap chain for WM_SIZE, so the
// objects live at file scope.  WndProc only touches them once g_ready is set.
HWND        g_hwnd   = nullptr;
bool        g_ready  = false;   // D3D objects are usable
bool        g_quit   = false;   // set by WndProc to end the render loop

ComPtr<ID3D11Device>           g_device;
ComPtr<ID3D11DeviceContext>    g_context;
ComPtr<IDXGISwapChain>         g_swapChain;
ComPtr<ID3D11RenderTargetView> g_rtv;

// ---------------------------------------------------------------- helpers

const char* FeatureLevelName(D3D_FEATURE_LEVEL level) {
    switch (level) {
    case D3D_FEATURE_LEVEL_11_1: return "11_1";
    case D3D_FEATURE_LEVEL_11_0: return "11_0";
    case D3D_FEATURE_LEVEL_10_1: return "10_1";
    case D3D_FEATURE_LEVEL_10_0: return "10_0";
    case D3D_FEATURE_LEVEL_9_3:  return "9_3";
    case D3D_FEATURE_LEVEL_9_2:  return "9_2";
    case D3D_FEATURE_LEVEL_9_1:  return "9_1";
    default:                     return "unknown";
    }
}

// (Re)create the render target view from the swap chain back buffer.
// Called at startup and after every ResizeBuffers.
bool CreateRenderTarget() {
    g_rtv.Reset();
    ComPtr<ID3D11Texture2D> backBuffer;
    if (FAILED(g_swapChain->GetBuffer(0, IID_PPV_ARGS(&backBuffer)))) return false;
    return SUCCEEDED(g_device->CreateRenderTargetView(backBuffer.Get(), nullptr, &g_rtv));
}

// D3D11CreateDeviceAndSwapChain with the requested settings.
// Tries hardware at 11_0 (falling back to 10_1/10_0 through the feature level
// array), then WARP if no hardware device is usable.
bool CreateDeviceAndSwapChain(HWND hwnd, D3D_DRIVER_TYPE& outDriver, D3D_FEATURE_LEVEL& outLevel) {
    DXGI_SWAP_CHAIN_DESC sd{};
    sd.BufferDesc.Width                   = kWidth;
    sd.BufferDesc.Height                  = kHeight;
    sd.BufferDesc.RefreshRate.Numerator   = 60;
    sd.BufferDesc.RefreshRate.Denominator = 1;
    sd.BufferDesc.Format                  = DXGI_FORMAT_R8G8B8A8_UNORM;
    sd.BufferDesc.ScanlineOrdering        = DXGI_MODE_SCANLINE_ORDER_UNSPECIFIED;
    sd.BufferDesc.Scaling                 = DXGI_MODE_SCALING_UNSPECIFIED;
    sd.SampleDesc.Count                   = 1;
    sd.SampleDesc.Quality                 = 0;
    sd.BufferUsage                        = DXGI_USAGE_RENDER_TARGET_OUTPUT;
    sd.BufferCount                        = 2;
    sd.OutputWindow                       = hwnd;
    sd.Windowed                           = TRUE;
    sd.SwapEffect                         = DXGI_SWAP_EFFECT_DISCARD;
    sd.Flags                              = 0;

    const D3D_FEATURE_LEVEL wanted[] = {
        D3D_FEATURE_LEVEL_11_0,
        D3D_FEATURE_LEVEL_10_1,
        D3D_FEATURE_LEVEL_10_0,
    };
    const UINT flags = D3D11_CREATE_DEVICE_BGRA_SUPPORT;

    const D3D_DRIVER_TYPE drivers[] = { D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP };
    for (D3D_DRIVER_TYPE driver : drivers) {
        g_device.Reset();
        g_context.Reset();
        g_swapChain.Reset();

        D3D_FEATURE_LEVEL got = D3D_FEATURE_LEVEL_10_0;
        const HRESULT hr = D3D11CreateDeviceAndSwapChain(
            nullptr, driver, nullptr, flags,
            wanted, ARRAYSIZE(wanted), D3D11_SDK_VERSION,
            &sd, &g_swapChain, &g_device, &got, &g_context);

        if (SUCCEEDED(hr)) {
            outDriver = driver;
            outLevel  = got;
            return true;
        }
        std::printf("D3D11CreateDeviceAndSwapChain failed for driver type %d: hr=0x%08lX\n",
                    static_cast<int>(driver), static_cast<unsigned long>(hr));
    }
    return false;
}

// Smoothly varying dark blue/teal so it is obvious the app is rendering.
void AnimatedClearColor(double seconds, float& r, float& g, float& b) {
    const double t     = seconds * 0.35;               // slow, clearly visible motion
    const double phase = t * 6.283185307179586;        // one full cycle per ~2.9 s

    r = 0.02f + 0.05f * static_cast<float>(0.5 + 0.5 * std::sin(phase));
    g = 0.08f + 0.32f * static_cast<float>(0.5 + 0.5 * std::sin(phase * 1.5 + 1.0));
    b = 0.14f + 0.45f * static_cast<float>(0.5 + 0.5 * std::sin(phase * 0.75 + 2.0));
}

// ---------------------------------------------------------------- window

LRESULT CALLBACK WndProc(HWND hwnd, UINT msg, WPARAM wParam, LPARAM lParam) {
    switch (msg) {
    case WM_CLOSE:
    case WM_DESTROY:
        g_quit = true;
        PostQuitMessage(0);
        return 0;

    case WM_KEYDOWN:
        if (wParam == VK_ESCAPE) {
            g_quit = true;
            PostQuitMessage(0);
            return 0;
        }
        break;

    case WM_SIZE: {
        // The back buffer is fixed size by default; keep it in sync with the
        // client area so a resize/maximize does not stretch or fault.
        if (!g_ready || !g_swapChain) break;
        if (wParam == SIZE_MINIMIZED) break;
        const UINT w = LOWORD(lParam);
        const UINT h = HIWORD(lParam);
        if (w == 0 || h == 0) break;

        g_context->OMSetRenderTargets(0, nullptr, nullptr);
        g_rtv.Reset();
        if (SUCCEEDED(g_swapChain->ResizeBuffers(0, w, h, DXGI_FORMAT_UNKNOWN, 0)) &&
            !CreateRenderTarget()) {
            std::printf("failed to recreate the render target view after resize\n");
            g_ready = false;   // stop presenting rather than clearing a null view
            return 0;
        }
        return 0;
    }

    default:
        break;
    }
    return DefWindowProcW(hwnd, msg, wParam, lParam);
}

bool CreateWindowAndClass(HWND& outHwnd) {
    WNDCLASSEXW wc{};
    wc.cbSize        = sizeof(wc);
    wc.style         = CS_HREDRAW | CS_VREDRAW;
    wc.lpfnWndProc   = WndProc;
    wc.hInstance     = GetModuleHandleW(nullptr);
    wc.hCursor       = LoadCursorW(nullptr, IDC_ARROW);
    wc.hbrBackground = nullptr;              // D3D paints every frame
    wc.lpszClassName = kClassName;
    if (!RegisterClassExW(&wc)) {
        std::printf("RegisterClassExW failed: %lu\n", GetLastError());
        return false;
    }

    // Fixed client area of exactly 1280x720.
    RECT rc = { 0, 0, static_cast<LONG>(kWidth), static_cast<LONG>(kHeight) };
    AdjustWindowRectEx(&rc, WS_OVERLAPPEDWINDOW, FALSE, 0);

    outHwnd = CreateWindowExW(
        0, kClassName, kTitle, WS_OVERLAPPEDWINDOW,
        CW_USEDEFAULT, CW_USEDEFAULT,
        rc.right - rc.left, rc.bottom - rc.top,
        nullptr, nullptr, wc.hInstance, nullptr);
    if (!outHwnd) {
        std::printf("CreateWindowExW failed: %lu\n", GetLastError());
        return false;
    }

    ShowWindow(outHwnd, SW_SHOW);
    UpdateWindow(outHwnd);
    return true;
}

} // namespace

int main() {
    // Note: no setvbuf() here. setvbuf(stdout, nullptr, _IOLBF, 0) trips the
    // UCRT invalid-parameter handler and fast-fails the process (0xC0000409)
    // before a single line is printed. Explicit flushes are used instead.
    std::printf("HR OSD Test - D3D11 sink (starting)\n");
    std::fflush(stdout);

    // Must run before any window exists: AdjustWindowRectEx below computes the
    // outer size from the desired client area, and on a high-DPI display that
    // is only correct once the process is DPI aware.
    SetProcessDPIAware();   // best effort; no per-monitor handling needed here

    HWND hwnd = nullptr;
    if (!CreateWindowAndClass(hwnd)) return 1;
    g_hwnd = hwnd;
    std::printf("window created (client area %ux%u, title \"HR OSD Test - D3D11\")\n",
                static_cast<unsigned>(kWidth), static_cast<unsigned>(kHeight));
    std::fflush(stdout);

    D3D_DRIVER_TYPE driver = D3D_DRIVER_TYPE_HARDWARE;
    D3D_FEATURE_LEVEL level = D3D_FEATURE_LEVEL_10_0;
    if (!CreateDeviceAndSwapChain(hwnd, driver, level)) {
        std::printf("could not create a D3D11 device at all (hardware and WARP both failed)\n");
        DestroyWindow(hwnd);
        return 1;
    }
    if (!CreateRenderTarget()) {
        std::printf("failed to create the render target view\n");
        DestroyWindow(hwnd);
        return 1;
    }

    D3D11_VIEWPORT vp{};
    vp.Width    = static_cast<float>(kWidth);
    vp.Height   = static_cast<float>(kHeight);
    vp.MinDepth = 0.0f;
    vp.MaxDepth = 1.0f;
    g_context->RSSetViewports(1, &vp);

    g_ready = true;

    std::printf("D3D11 device ready\n");
    std::printf("window size   : %ux%u (client area, title \"HR OSD Test - D3D11\")\n",
                static_cast<unsigned>(kWidth), static_cast<unsigned>(kHeight));
    std::printf("feature level : %s\n", FeatureLevelName(level));
    std::printf("driver type   : %s\n",
                driver == D3D_DRIVER_TYPE_WARP ? "WARP (software)" : "HARDWARE");
    std::printf("swap effect   : DXGI_SWAP_EFFECT_DISCARD, BufferCount=2, R8G8B8A8_UNORM\n");
    std::printf("Presenting one frame per vsync (Present(1,0)); RTSS should hook this process.\n");
    std::printf("Press Esc or close the window to exit.\n");
    std::fflush(stdout);

    unsigned long long frames      = 0;
    DWORD              lastReport  = GetTickCount();
    const DWORD        startTicks  = lastReport;
    HRESULT            lastPresent = S_OK;

    LARGE_INTEGER freq{}, start{}, now{};
    // QPC can in principle fail; freq.QuadPart == 0 would make the animation and
    // the FPS report garbage (inf/nan). Fall back to the millisecond tick count.
    const bool haveQpc = QueryPerformanceFrequency(&freq) != FALSE && freq.QuadPart != 0;
    if (haveQpc) QueryPerformanceCounter(&start);

    auto elapsedSeconds = [&]() -> double {
        if (!haveQpc) return static_cast<double>(GetTickCount() - startTicks) / 1000.0;
        QueryPerformanceCounter(&now);
        return static_cast<double>(now.QuadPart - start.QuadPart) /
               static_cast<double>(freq.QuadPart);
    };

    MSG msg{};
    while (!g_quit) {
        while (PeekMessageW(&msg, nullptr, 0, 0, PM_REMOVE)) {
            if (msg.message == WM_QUIT) { g_quit = true; break; }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        if (g_quit) break;
        if (!g_ready) break;   // a resize left us without a render target

        const double seconds = elapsedSeconds();

        float r = 0.0f, g = 0.0f, b = 0.0f;
        AnimatedClearColor(seconds, r, g, b);
        const float color[4] = { r, g, b, 1.0f };
        if (g_rtv) g_context->ClearRenderTargetView(g_rtv.Get(), color);

        // Present with vsync: drives the loop at the display refresh rate and,
        // more importantly, produces the steady stream of real frames RTSS needs.
        lastPresent = g_swapChain->Present(1, 0);
        ++frames;

        if (lastPresent == DXGI_ERROR_DEVICE_REMOVED || lastPresent == DXGI_ERROR_DEVICE_RESET) {
            std::printf("device removed/reset during Present (hr=0x%08lX), exiting\n",
                        static_cast<unsigned long>(lastPresent));
            break;
        }
        if (FAILED(lastPresent)) {
            std::printf("Present failed (hr=0x%08lX), exiting\n",
                        static_cast<unsigned long>(lastPresent));
            break;
        }

        // Minimised or fully occluded, Present(1,0) returns immediately instead of
        // waiting for vsync -- and DXGI_STATUS_OCCLUDED (0x087A0001) is a success
        // code, so the FAILED() check above does not catch it. Without this the
        // loop spins at 100% CPU. Nothing is visible anyway, so just idle a bit.
        if (lastPresent == DXGI_STATUS_OCCLUDED || IsIconic(g_hwnd)) {
            Sleep(50);
        }

        const DWORD nowTicks = GetTickCount();
        if (nowTicks - lastReport >= kReportIntervalMs) {
            const double elapsed = static_cast<double>(nowTicks - startTicks) / 1000.0;
            std::printf("[%6.1fs] frame %llu  ~%.1f FPS  clear=(%.2f, %.2f, %.2f)\n",
                        elapsed, frames, static_cast<double>(frames) / elapsed, r, g, b);
            std::fflush(stdout);
            lastReport = nowTicks;
        }
    }

    // ------------------------------------------------------------ shutdown
    g_ready = false;
    if (g_context) g_context->ClearState();
    g_rtv.Reset();
    if (g_swapChain) g_swapChain->SetFullscreenState(FALSE, nullptr);
    g_swapChain.Reset();
    g_context.Reset();
    g_device.Reset();

    if (IsWindow(hwnd)) DestroyWindow(hwnd);
    UnregisterClassW(kClassName, GetModuleHandleW(nullptr));

    std::printf("exit: presented %llu frames, last Present hr=0x%08lX\n",
                frames, static_cast<unsigned long>(lastPresent));
    std::fflush(stdout);
    return 0;
}
