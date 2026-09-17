// src/daemon_ctl.rs —— hr-daemon 的探活与生命周期控制。
//
// 策略是早期纯 std 版本验证过的，原样沿用：
//   探活 = OpenMutexW（互斥体是 daemon 整个生命周期都握着的，比窗口准）；
//   停止 = FindWindowW + WM_CLOSE（优雅退出，它会收 BLE 线程、关共享内存映射），
//          然后等互斥体消失——必须等进程真退出，只等窗口消失的话新实例会被
//          单实例互斥体挡回去，落得"一个都没在跑"（第四轮踩过的坑）；
//   启动 = CreateProcessW 拉起 + 轮询互斥体确认它没被挡回去。
// 这里不做任何打印/弹窗：CLI 和 GUI 各自决定怎么呈现。
use crate::config;
use crate::names;
use crate::proc;
use crate::win;
use std::path::PathBuf;

pub fn daemon_path() -> PathBuf {
    config::exe_dir().join(names::DAEMON_EXE)
}

/// daemon 的进程还在吗？它整个生命周期都握着单实例互斥体，能打开就说明还在。
pub fn alive() -> bool {
    let name = win::wide(names::DAEMON_MUTEX);
    let h = unsafe { win::OpenMutexW(win::SYNCHRONIZE, 0, name.as_ptr()) };
    if h.is_null() {
        return false;
    }
    unsafe { win::CloseHandle(h) };
    true
}

pub fn hwnd() -> win::Hwnd {
    let cls = win::wide(names::DAEMON_WNDCLASS);
    unsafe { win::FindWindowW(cls.as_ptr(), std::ptr::null()) }
}

/// 在不在跑。窗口没了但进程还在（正在收尾）也算在跑。
pub fn running() -> bool {
    !hwnd().is_null() || alive()
}

/// 等它彻底退出：窗口和互斥体都没了才算走干净。
pub fn wait_gone(timeout_ms: u32) -> bool {
    let mut waited = 0u32;
    while waited < timeout_ms && running() {
        unsafe { win::Sleep(200) };
        waited += 200;
    }
    !running()
}

/// 等新起的 daemon 注册上互斥体（= 真的跑起来了，没被单实例挡回去）。
pub fn wait_up(timeout_ms: u32) -> bool {
    let mut waited = 0u32;
    while waited < timeout_ms {
        if alive() {
            return true;
        }
        unsafe { win::Sleep(200) };
        waited += 200;
    }
    false
}

/// 让正在跑的 daemon 走正常退出路径（WM_CLOSE），然后等它**真的退出**。
/// 没在跑就什么都不做。超时返回 Err（daemon 可能卡在一次 BLE 连接里，收尾要十几秒）。
pub fn stop() -> Result<(), String> {
    let hwnd = hwnd();
    if hwnd.is_null() && !alive() {
        return Ok(());
    }
    if !hwnd.is_null() {
        unsafe { win::PostMessageW(hwnd, win::WM_CLOSE, 0, 0) };
    }
    if wait_gone(names::DAEMON_STOP_TIMEOUT_MS) {
        Ok(())
    } else {
        Err(format!(
            "旧的 hr-daemon 没在 {} 秒内退出，请手动结束它再试（可能正连着手表收尾）",
            names::DAEMON_STOP_TIMEOUT_MS / 1000
        ))
    }
}

/// 拉起一份 daemon（--quiet，常驻后台），并确认它注册上了互斥体。
pub fn start() -> Result<(), String> {
    let exe = daemon_path();
    if !exe.exists() {
        return Err(format!(
            "找不到 {}，先构建：cmd /c daemon\\build.cmd",
            exe.display()
        ));
    }
    proc::spawn_detached(&exe, &["--quiet"])?;
    if !wait_up(names::DAEMON_START_TIMEOUT_MS) {
        return Err(format!(
            "{} 起来了但没注册上（多半是还有个旧的没退干净，或者启动失败）——看同目录的 hr-daemon.log",
            exe.display()
        ));
    }
    Ok(())
}

/// 先让正在跑的 daemon 退出，再启动一份新的。没在跑就直接启动。
pub fn restart() -> Result<String, String> {
    let was_running = running();
    stop()?;
    start()?;
    Ok(if was_running {
        "已重启 hr-daemon".to_string()
    } else {
        "已启动 hr-daemon".to_string()
    })
}
