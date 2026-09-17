// hr-manager —— 华为手表心率采集的管理器。
//
// 一个 exe 两种形态：
//   无参数 / --minimized → GUI：托盘常驻 + 按需弹出的 WebView2 配置面板
//   子命令               → CLI：给脚本、CI 和愿意敲命令的人（gui/cli.rs）
//
// release 构建是 GUI 子系统（不弹黑窗）；CLI 路径会先附加父控制台恢复
// 标准句柄再打印，所以从 cmd 里跑子命令输出照常可见。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod autostart;
mod cli;
mod config;
mod daemon_ctl;
mod gui;
mod ipc;
mod names;
mod panel;
mod proc;
mod scan_job;
mod shared_mem;
mod state;
mod tray;
mod win;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    match args.first().map(|s| s.as_str()) {
        None => std::process::exit(gui::run(false)),
        Some("--minimized" | "-m") => std::process::exit(gui::run(true)),
        Some("--gui") => std::process::exit(gui::run(false)),
        // 其余都当子命令：先恢复控制台，再分发
        Some(_) => {
            cli::attach_console();
            std::process::exit(cli::run(&args));
        }
    }
}
