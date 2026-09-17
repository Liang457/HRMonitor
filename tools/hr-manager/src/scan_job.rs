// src/scan_job.rs —— 一次完整的"选表扫描"编排。
//
// 物理限制绕不开：手表被 daemon 连着的时候就不再广播，所以扫描前必须先停
// daemon（等 2 秒让手表恢复广播）。谁停的谁负责拉回来：
//   * 用户取消 / 扫描失败   → 立即把 daemon 拉回来（沿用原配置）
//   * 正常扫完              → 保持停止，等用户选表（选定后重启生效）
//                             或放弃（点"放弃"时由调用方拉回）
// 设备列表由调用方按 mac 去重、后到的名字覆盖前面的（广播里名字可能后到）。
//
// 取消路径：调用方（GUI 主线程）把 stop 置位并 kill 子进程（句柄经 cancel_slot
// 交给调用方）。扫描线程正阻塞在读管道上，只置位不 kill 是叫不醒它的。
use crate::daemon_ctl as ctl;
use crate::proc;
use serde_json::Value;
use std::io::BufRead;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

/// 取消用的坑位：扫描期间放着可 kill 的子进程句柄，其他时候是 None
pub type CancelSlot = Mutex<Option<proc::CancelHandle>>;

#[derive(Clone, Debug)]
pub enum ScanEvent {
    /// 正在停 daemon（手表连着时不广播，必须先断开）
    Stopping,
    /// daemon 已停，等手表恢复广播
    Waiting,
    /// 发现设备（或某台的名字补全了，调用方按 mac 覆盖）
    Device { mac: String, name: String },
    /// 扫描自然结束，找到 count 台
    Done { count: usize },
}

pub struct ScanOutcome {
    pub count: usize,
    /// 本次扫描停掉过 daemon
    pub stopped_daemon: bool,
    /// daemon 已经被拉回来了（取消/失败路径）
    pub daemon_restored: bool,
    /// 被用户取消
    pub cancelled: bool,
}

/// 跑一次扫描（阻塞，适合放线程里）。
/// 返回 Err 的情形：daemon 停不掉 / 扫描进程起不来 / 蓝牙适配器不可用。
/// 这三种情形里 daemon 都会被拉回原样（停过的话）。
pub fn run_scan(
    secs: u32,
    stop: &AtomicBool,
    cancel_slot: &CancelSlot,
    mut on_event: impl FnMut(ScanEvent),
) -> Result<ScanOutcome, String> {
    let mut outcome = ScanOutcome {
        count: 0,
        stopped_daemon: false,
        daemon_restored: false,
        cancelled: false,
    };

    // ---- 1. 手表被连着就不广播：先停 daemon
    let was_running = ctl::running();
    if was_running {
        on_event(ScanEvent::Stopping);
        ctl::stop()?; // 停不掉就放弃扫描；daemon 原样在跑
        outcome.stopped_daemon = true;

        if stop.load(Ordering::Relaxed) {
            restore(&mut outcome);
            outcome.cancelled = true;
            return Ok(outcome);
        }
        on_event(ScanEvent::Waiting);
        // 断开之后手表要过一下才重新开始广播
        sleep_cancellable(stop, 2000);
        if stop.load(Ordering::Relaxed) {
            restore(&mut outcome);
            outcome.cancelled = true;
            return Ok(outcome);
        }
    }

    // ---- 2. 起 --scan 子进程，逐行读 JSON
    let exe = ctl::daemon_path();
    if !exe.exists() {
        restore(&mut outcome);
        return Err(format!("找不到 {}，先构建：cmd /c daemon\\build.cmd", exe.display()));
    }
    let args = vec!["--scan".to_string(), secs.to_string(), "--quiet".to_string()];
    let mut child = proc::spawn_child(&exe, &args).map_err(|e| {
        restore(&mut outcome);
        e
    })?;
    *cancel_slot.lock().unwrap() = Some(proc::CancelHandle(child.process));

    let stdout = child.stdout.take().expect("spawn_child 一定带 stdout");
    let mut reader = std::io::BufReader::new(stdout);
    loop {
        let mut line = Vec::new();
        match reader.read_until(b'\n', &mut line) {
            Ok(0) => break,   // EOF：子进程结束（或被取消 kill 掉）
            Ok(_) => {}
            Err(_) => break,
        }
        if stop.load(Ordering::Relaxed) {
            child.kill();
            continue; // 管道很快会 EOF，走下面统一的取消收尾
        }
        let text = String::from_utf8_lossy(&line);
        for part in text.split('\n') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            match parse_line(part) {
                Some(Parsed::Device { mac, name }) => {
                    on_event(ScanEvent::Device { mac, name });
                }
                Some(Parsed::Done { count }) => outcome.count = count,
                None => {} // 解析不了的行（版本差异）直接忽略
            }
        }
    }

    let code = child.wait(5000);
    *cancel_slot.lock().unwrap() = None;

    if stop.load(Ordering::Relaxed) {
        restore(&mut outcome);
        outcome.cancelled = true;
        return Ok(outcome);
    }
    if code != 0 {
        restore(&mut outcome);
        return Err("扫描起不来（蓝牙适配器关了吗？原因见 hr-daemon.log）".into());
    }

    on_event(ScanEvent::Done { count: outcome.count });
    Ok(outcome)
}

fn restore(outcome: &mut ScanOutcome) {
    if outcome.stopped_daemon && !outcome.daemon_restored {
        if ctl::start().is_ok() {
            outcome.daemon_restored = true;
        }
        // 拉不回来就留给调用方报：面板上有明确的 daemon 状态，别静默
    }
}

/// 可中断的 sleep：取消时别傻等 2 秒
fn sleep_cancellable(stop: &AtomicBool, ms: u32) {
    let mut waited = 0u32;
    while waited < ms && !stop.load(Ordering::Relaxed) {
        unsafe { crate::win::Sleep(100) };
        waited += 100;
    }
}

enum Parsed {
    Device { mac: String, name: String },
    Done { count: usize },
}

fn parse_line(line: &str) -> Option<Parsed> {
    let v: Value = serde_json::from_str(line).ok()?;
    match v.get("type")?.as_str()? {
        "device" => Some(Parsed::Device {
            mac: v.get("mac")?.as_str()?.to_string(),
            name: v.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string(),
        }),
        "done" => Some(Parsed::Done {
            count: v.get("count")?.as_u64()? as usize,
        }),
        _ => None,
    }
}

/// CLI 扫描：events 打到控制台，扫完把 daemon 拉回原样。
pub fn run_scan_cli(secs: u32) -> i32 {
    use std::io::Write as _;
    use std::sync::atomic::AtomicBool;

    println!("正在扫描心率广播设备（{} 秒）… 手表请停在\"心率广播\"页面并保持亮屏。", secs);
    let stop = AtomicBool::new(false);
    let cancel_slot: CancelSlot = Mutex::new(None);
    let mut devices: Vec<(String, String)> = Vec::new();
    let result = run_scan(secs, &stop, &cancel_slot, |ev| match ev {
        ScanEvent::Stopping => {
            print!("hr-daemon 正在运行，先停掉它（手表连着时不广播）...");
            let _ = std::io::stdout().flush();
        }
        ScanEvent::Waiting => println!("已停，等 2 秒让手表恢复广播"),
        ScanEvent::Device { mac, name } => {
            if let Some(slot) = devices.iter_mut().find(|(m, _)| m.eq_ignore_ascii_case(&mac)) {
                slot.1 = name;
            } else {
                devices.push((mac.clone(), name.clone()));
                println!("  发现 {:<17} {}", mac, if name.is_empty() { "（还没拿到名字）" } else { &name });
            }
        }
        ScanEvent::Done { .. } => {}
    });

    match result {
        Ok(out) => {
            println!("扫描完成：{} 台设备。", out.count);
            if out.stopped_daemon && !out.daemon_restored {
                match ctl::start() {
                    Ok(()) => println!("已把 hr-daemon 按原配置拉起来。"),
                    Err(e) => {
                        eprintln!("hr-daemon 没能自动拉起：{}", e);
                        return 1;
                    }
                }
            }
            if devices.is_empty() {
                println!("没扫到设备？检查手表是否停在\"心率广播\"页面并保持亮屏、蓝牙是否打开。");
            }
            0
        }
        Err(e) => {
            eprintln!("扫描失败：{}", e);
            1
        }
    }
}
