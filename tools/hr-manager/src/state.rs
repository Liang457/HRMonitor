// src/state.rs —— GUI 各线程共享的核心状态。
//
// 主线程（事件循环）拥有面板和托盘；扫描线程、状态轮询线程、daemon 操作线程
// 通过 Core 通信：结果一律 proxy.send_event(UserEvent::Eval(js)) 送回主线程，
// 由主线程 evaluate_script 注入面板（WebView 只能在主线程碰）。
use crate::scan_job::CancelSlot;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use tao::event_loop::EventLoopProxy;

pub enum UserEvent {
    /// 面板打开时执行一段 JS（payload 已序列化成 JSON）
    Eval(String),
    /// 托盘/第二个实例请求打开面板
    OpenPanel,
    /// 托盘菜单"重启采集"
    RestartDaemon,
    /// 托盘菜单"开机自启"勾选切换
    ToggleAutostart,
    /// 托盘菜单"退出"
    Quit,
}

pub struct Core {
    pub proxy: EventLoopProxy<UserEvent>,
    /// 面板当前开没开（状态轮询线程据此决定要不要推数据）
    pub panel_open: AtomicBool,
    /// display.timeout_ms 的缓存（get_config 时刷新），状态轮询算 effective bpm 用
    pub timeout_ms: AtomicU32,
    /// 扫描线程正在跑
    pub scan_running: AtomicBool,
    /// 扫描取消标志（配合 cancel_slot 的 kill 才叫得醒阻塞在读管道上的扫描线程）
    pub scan_stop: AtomicBool,
    pub scan_cancel: CancelSlot,
    /// 扫描正常结束后 daemon 保持停止（等用户选表）。此时若用户不选而离开
    /// （放弃/关面板），要把 daemon 拉回来。
    pub scan_needs_restore: AtomicBool,
    /// 旧版计划任务 HuaweiHRDaemon 是否存在（启动时后台查一次，schtasks 可能慢）
    pub old_task: AtomicBool,
}

impl Core {
    pub fn new(proxy: EventLoopProxy<UserEvent>) -> Arc<Core> {
        Arc::new(Core {
            proxy,
            panel_open: AtomicBool::new(false),
            timeout_ms: AtomicU32::new(15_000),
            scan_running: AtomicBool::new(false),
            scan_stop: AtomicBool::new(false),
            scan_cancel: CancelSlot::new(None),
            scan_needs_restore: AtomicBool::new(false),
            old_task: AtomicBool::new(false),
        })
    }

    pub fn eval(&self, js: String) {
        let _ = self.proxy.send_event(UserEvent::Eval(js));
    }

    /// 回应一次 IPC 调用（id 配对；前端用 Promise resolve）
    pub fn reply(&self, id: u64, payload: serde_json::Value) {
        let msg = serde_json::json!({ "id": id, "payload": payload });
        self.eval(format!("window.__hr.recv({});", msg));
    }

    pub fn toast(&self, msg: &str) {
        let s = serde_json::json!(msg);
        self.eval(format!("window.__hr.toast({});", s));
    }

    pub fn set_panel_open(&self, open: bool) {
        self.panel_open.store(open, Ordering::SeqCst);
    }
}
