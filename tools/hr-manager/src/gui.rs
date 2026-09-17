// src/gui.rs —— GUI 主循环：托盘 + 按需面板 + 各后台线程。
//
// 线程模型：
//   主线程    tao 事件循环。唯一能碰窗口/WebView 的地方；所有回推都经
//             UserEvent::Eval 在这里执行 evaluate_script。
//   状态轮询  1 秒一次读共享内存，面板开着才推 window.__hr.status(...)。
//   单实例    第二个实例 SetEvent 通知后退出；监视线程收到事件 → 打开面板。
//   慢操作    重启 daemon / 扫描 / 部署脚本各自起临时线程，结果经 proxy 回主线程。
// 托盘菜单事件也统一转成 UserEvent（经 proxy），所有处理集中在一处。
use crate::autostart;
use crate::daemon_ctl as ctl;
use crate::names;
use crate::panel;
use crate::shared_mem;
use crate::state::{Core, UserEvent};
use crate::win;
use serde_json::json;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tao::event::{Event, StartCause, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy, EventLoopWindowTarget};

pub fn run(minimized: bool) -> i32 {
    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let proxy: EventLoopProxy<UserEvent> = event_loop.create_proxy();

    // ---- 单实例。已有实例在跑：把"打开面板"的信号发过去，本次退出。
    // 互斥体句柄故意握到进程退出——它就是"我还在跑"的凭证。
    let self_mutex = unsafe {
        win::CreateMutexW(std::ptr::null_mut(), 1, win::wide(names::MANAGER_MUTEX).as_ptr())
    };
    if self_mutex.is_null() {
        // 创建失败 = 没有单实例保护，两个实例会互相干扰，宁可不起
        eprintln!("创建单实例互斥体失败（错误码 {}）", unsafe { win::GetLastError() });
        return 1;
    }
    if unsafe { win::GetLastError() } == win::ERROR_ALREADY_EXISTS {
        unsafe { win::CloseHandle(self_mutex) };
        // 第一实例可能还在启动早期（事件还没建好），短重试几次
        let mut ev = std::ptr::null_mut();
        for _ in 0..5 {
            ev = unsafe {
                win::OpenEventW(
                    win::EVENT_MODIFY_STATE | win::SYNCHRONIZE,
                    0,
                    win::wide(names::MANAGER_OPEN_EVENT).as_ptr(),
                )
            };
            if !ev.is_null() {
                break;
            }
            unsafe { win::Sleep(200) };
        }
        if !ev.is_null() {
            unsafe { win::SetEvent(ev) };
            unsafe { win::CloseHandle(ev) };
        }
        return 0;
    }

    let core = Core::new(proxy.clone());

    // 上次在扫描中死掉的话，daemon 还停着：看到恢复标记就把它拉回来。
    crate::scan_job::restore_pending();

    // ---- 托盘。起不来就退化成"纯面板"模式：面板一关即退出。
    let tray = match crate::tray::Tray::build() {
        Ok(t) => Some(t),
        Err(e) => {
            eprintln!("{}（继续运行，但托盘不可用）", e);
            None
        }
    };

    // ---- 后台线程
    spawn_status_thread(core.clone());
    spawn_open_event_thread(core.clone());
    spawn_old_task_probe(core.clone());

    // 面板统一在事件循环里创建（窗口目标只在循环闭包里拿得到）。
    // 启动时不是 --minimized 就开面板；托盘不可用时无论如何都开，
    // 否则这个进程没有任何交互入口。
    let open_on_start = !minimized || tray.is_none();
    let mut panel: Option<panel::Panel> = None;

    let mut next_tick = Instant::now();
    event_loop.run(move |event, target, control_flow| {
        match event {
            // 循环一转起来就把启动面板开掉（窗口目标只在闭包里拿得到）
            Event::NewEvents(StartCause::Init) => {
                if open_on_start {
                    open_or_focus(&core, target, &mut panel);
                }
            }

            // 周期心跳：轮询托盘菜单事件 + 维持 200ms 醒一次的节奏
            Event::NewEvents(StartCause::ResumeTimeReached { .. }) => {
                if let Some(t) = &tray {
                    for ue in t.poll() {
                        let _ = proxy.send_event(ue);
                    }
                }
                next_tick = Instant::now() + Duration::from_millis(200);
            }

            // 所有用户事件（托盘菜单 / 单实例信号 / 各线程回推）集中在这里处理
            Event::UserEvent(ue) => match ue {
                UserEvent::Eval(js) => {
                    if let Some(p) = &panel {
                        p.eval(&js);
                    }
                }
                UserEvent::OpenPanel => open_or_focus(&core, target, &mut panel),
                UserEvent::RestartDaemon => {
                    let core2 = core.clone();
                    std::thread::spawn(move || match ctl::restart() {
                        Ok(m) => core2.toast(&m),
                        Err(e) => core2.toast(&e),
                    });
                }
                UserEvent::ToggleAutostart => {
                    let enabled = autostart::get().is_some();
                    let r = if enabled { autostart::disable() } else { autostart::enable() };
                    match r {
                        Ok(()) => {
                            if let Some(t) = &tray {
                                t.set_autostart_checked(!enabled);
                            }
                            core.toast(if enabled {
                                "已关闭开机自启"
                            } else {
                                "已开启开机自启（登录后只出托盘）"
                            });
                        }
                        Err(e) => core.toast(&e),
                    }
                }
                UserEvent::Quit => {
                    *control_flow = ControlFlow::Exit;
                }
            },

            Event::WindowEvent { window_id, event, .. } => {
                if let Some(p) = &panel {
                    if window_id == p.window.id() {
                        match event {
                            WindowEvent::CloseRequested => {
                                close_panel(&core, &mut panel);
                                // 托盘没有而面板关了 = 交互全没了，直接退
                                if tray.is_none() {
                                    *control_flow = ControlFlow::Exit;
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }

            Event::LoopDestroyed => {
                close_panel(&core, &mut panel);
            }

            _ => {}
        }
        if *control_flow != ControlFlow::Exit {
            *control_flow = ControlFlow::WaitUntil(next_tick);
        }
    });

    // event_loop.run 正常不返回（进程在 Exit 后由 tao 结束）
}

/// 面板打开（或已开就聚焦）
fn open_or_focus(
    core: &Arc<Core>,
    target: &EventLoopWindowTarget<UserEvent>,
    panel: &mut Option<panel::Panel>,
) {
    match panel {
        Some(p) => p.focus(),
        None => match panel::open(core, target) {
            Ok(p) => *panel = Some(p),
            Err(e) => core.toast(&e),
        },
    }
}

/// 关面板 = 整体销毁（内存回落）。
/// 扫描善后在这里收口：
///   * 扫描还在跑 → 置取消标志 + 强杀子进程，扫描线程的取消路径会把 daemon 拉回来；
///   * 扫描已停在"等选表"（粘滞标志）→ 直接把 daemon 拉回来。
/// 两条路都不走的话，daemon 会一直停着没人管（旧版的窟窿）。
fn close_panel(core: &Arc<Core>, panel: &mut Option<panel::Panel>) {
    if let Some(p) = panel.take() {
        core.set_panel_open(false);
        drop(p); // 字段序：webview 先析构 → WebView2 渲染进程退出
    }
    if core.scan_running.swap(false, Ordering::SeqCst) {
        core.scan_stop.store(true, Ordering::SeqCst);
        let h = core.scan_cancel.lock().unwrap().take();
        if let Some(h) = h {
            h.terminate();
        }
        // 扫描线程的取消分支看到标志会恢复 daemon；它失败时置回的粘滞标志
        // 由下面的消费点兜底。
    }
    if core.scan_needs_restore.swap(false, Ordering::SeqCst) {
        let core2 = core.clone();
        std::thread::spawn(move || {
            if ctl::running() {
                crate::scan_job::clear_marker();
                return;
            }
            if let Err(e) = ctl::start() {
                // 进程可能正在退出，这条 toast 未必有机会显示；标志放回去，
                // 标记文件还在，下次启动 manager 会再试
                core2.scan_needs_restore.store(true, Ordering::SeqCst);
                core2.toast(&format!("hr-daemon 没能自动拉起：{}", e));
            } else {
                crate::scan_job::clear_marker();
            }
        });
    }
}

/// 占位辅助已删除：面板统一在事件循环闭包内创建，见 open_on_start / open_or_focus。

// ---------------------------------------------------------------- 后台线程

fn spawn_status_thread(core: Arc<Core>) {
    std::thread::spawn(move || {
        let mut reader = shared_mem::Reader::new();
        loop {
            unsafe { win::Sleep(1000) };
            if !core.panel_open.load(Ordering::SeqCst) {
                continue;
            }
            let daemon = ctl::running();
            let now = shared_mem::now_ms();
            let timeout = core.timeout_ms.load(Ordering::SeqCst) as u64;
            // daemon 没跑就是没跑：映射对象因为读端握着句柄而活着，daemon 退出后
            // read() 仍会返回最后一条旧快照——拿它当"在跑"的依据，面板就永远
            // 显示运行中、启动按钮永远灰着（踩过的坑）。所以先问互斥体再看数据。
            let payload = if !daemon {
                json!({
                    "daemon": false,
                    "bpm": -1,
                    "status": "off",
                    "device": "",
                    "ageSec": 0,
                })
            } else {
                match reader.read() {
                    Some(s) => {
                        let bpm = s.effective_bpm(now, timeout);
                        let status = if bpm >= 0 {
                            "ok"
                        } else {
                            match s.status {
                                shared_mem::HRS_CONNECTING => "connecting",
                                shared_mem::HRS_TIMEOUT => "timeout",
                                _ => "nodata",
                            }
                        };
                        json!({
                            "daemon": true,
                            "bpm": bpm,
                            "status": status,
                            "device": s.device,
                            "ageSec": now.saturating_sub(s.tick_ms) / 1000,
                        })
                    }
                    None => json!({
                        "daemon": true,
                        "bpm": -1,
                        "status": "connecting",
                        "device": "",
                        "ageSec": 0,
                    }),
                }
            };
            core.eval(format!("window.__hr.status({});", payload));
        }
    });
}

fn spawn_open_event_thread(core: Arc<Core>) {
    std::thread::spawn(move || {
        let ev = unsafe {
            win::CreateEventW(std::ptr::null_mut(), 0, 0, win::wide(names::MANAGER_OPEN_EVENT).as_ptr())
        };
        if ev.is_null() {
            return;
        }
        loop {
            let r = unsafe { win::WaitForSingleObject(ev, win::INFINITE) };
            if r == win::WAIT_OBJECT_0 {
                let _ = core.proxy.send_event(UserEvent::OpenPanel);
            } else {
                // WAIT_FAILED 之类的意外返回：睡一觉再试，别忙等空转
                unsafe { win::Sleep(1000) };
            }
        }
    });
}

fn spawn_old_task_probe(core: Arc<Core>) {
    std::thread::spawn(move || {
        let exists = autostart::old_task_exists();
        core.old_task.store(exists, Ordering::SeqCst);
    });
}
