// src/panel.rs —— 配置面板：tao 窗口 + wry(WebView2)，装嵌入的单页 HTML。
//
// 内存模型（设计要求）：面板**按需创建、关闭即整体销毁**——WebView2 的渲染
// 进程（msedgewebview2.exe）随 drop 全部退出，内存立刻回落；托盘进程本身
// 只有几 MB。再点托盘"打开面板"时重建。
//
// 字段顺序即 drop 顺序：webview 先于 window 释放，先停掉页面再拆窗口。
use crate::ipc;
use crate::state::Core;
use std::sync::Arc;

pub struct Panel {
    webview: wry::WebView,
    pub window: tao::window::Window,
}

/// 打开面板。同一时刻只有一份（调用方保证：已开时只 set_focus）。
/// 只能在事件循环闭包里调（要窗口目标）。
pub fn open(
    core: &Arc<Core>,
    target: &tao::event_loop::EventLoopWindowTarget<crate::state::UserEvent>,
) -> Result<Panel, String> {
    let window = tao::window::WindowBuilder::new()
        .with_title("华为手表心率")
        .with_inner_size(tao::dpi::LogicalSize::new(500.0f64, 760.0f64))
        .with_min_inner_size(tao::dpi::LogicalSize::new(420.0f64, 560.0f64))
        .build(target)
        .map_err(|e| format!("创建窗口失败：{}", e))?;

    let core2 = core.clone();
    let webview = wry::WebViewBuilder::new()
        .with_html(include_str!("assets/index.html"))
        .with_ipc_handler(move |req: wry::http::Request<String>| {
            let body = req.body().clone();
            ipc::dispatch(&core2, &body);
        })
        .build(&window)
        .map_err(|e| format!("创建 WebView 失败：{}", e))?;

    core.set_panel_open(true);
    Ok(Panel { webview, window })
}

impl Panel {
    pub fn focus(&self) {
        self.window.set_focus();
    }

    pub fn eval(&self, js: &str) {
        let _ = self.webview.evaluate_script(js);
    }
}

// 面板随窗口变化的大小由 wry 内部处理（WebView2 子类化了窗口过程），无需手动 resize。

// Drop 交给字段自然析构（webview → window）；panel_open 标志由 gui.rs 的
// close_panel() 在 drop 之前清，保证状态轮询线程立刻停推数据。
