// src/scan_job.rs —— 一次完整的"选表扫描"编排。
//
// 物理限制绕不开：手表被 daemon 连着的时候就不再广播，所以扫描前必须先停
// daemon（等 2 秒让手表恢复广播）。谁停的谁负责拉回来：
//   * 用户取消 / 扫描失败   → 立即把 daemon 拉回来（沿用原配置）
//   * 正常扫完              → 保持停止，等用户选表（选定后重启生效）
//                             或放弃（点"放弃"时由调用方拉回）
// 设备列表由调用方按 mac 去重、后到的名字覆盖前面的（广播里名字可能后到）。
//
// 广播里往往不带设备名，识别靠逐台探测：扫描读流线程发现新 mac 就入队
// （mpsc），探测线程串行拉起 `hr-daemon --probe-address` 子进程取真名和第一拍
// 心率，事件照常推给面板回填。扫描读完后不急着报 Done，等识别队列排空才发
// ——面板在「识别设备中」阶段保持扫描进行态。
//
// 取消路径：调用方（GUI 主线程）把 stop 置位并 kill 子进程（句柄经 cancel_slot
// / probe_cancel 交给调用方，scan_job::cancel_scan 两个坑位一起杀）。扫描线程
// 和探测线程都阻塞在读管道上，只置位不 kill 是叫不醒它们的。
use crate::daemon_ctl as ctl;
use crate::proc;
use crate::state::Core;
use serde_json::Value;
use std::collections::HashSet;
use std::io::BufRead;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

/// 取消用的坑位：扫描/探测期间放着可 kill 的子进程句柄，其他时候是 None
pub type CancelSlot = Mutex<Option<proc::CancelHandle>>;

#[derive(Clone, Debug)]
pub enum ScanEvent {
    /// 正在停 daemon（手表连着时不广播，必须先断开）
    Stopping,
    /// daemon 已停，等手表恢复广播
    Waiting,
    /// 发现设备（或某台的名字补全了，调用方按 mac 覆盖）
    Device { mac: String, name: String },
    /// 单台识别：name 非空 = 真设备名；bpm>=0 = 读到实时心率（-1 = 尚未读到）
    Probe { mac: String, name: String, bpm: i32 },
    /// 单台识别收尾（含失败；此后这台不会再有 Probe 事件）
    ProbeDone { mac: String },
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
    /// 非空 = 扫描失败原因。不做成 Err 是为了把上面几个字段一起带回给调用方：
    /// "停了 daemon 却没拉回来"的善后信息不能跟着 Err 一起丢掉。
    pub error: Option<String>,
}

/// 扫描恢复标记：停 daemon 前写、daemon 成功回来后删。
/// manager 在扫描中途死掉（崩溃/被结束）时，daemon 会留在停止态——下次
/// manager 启动看到这个标记就自动把 daemon 拉回来。
fn marker_path() -> std::path::PathBuf {
    // 放 config\ 子目录而不是 exe 旁：发行目录的根只放 exe 和 README.md，
    // 运行期产物（配置/日志/这个标记）一律进子目录。
    crate::config::exe_dir().join("config").join("scan-pending")
}

fn write_marker() {
    // 标记写不出来（目录只读之类）只是少了崩溃兜底，不影响本次扫描
    let _ = std::fs::write(marker_path(), b"scan stopped hr-daemon\n");
}

/// daemon 确认恢复后清标记。
pub fn clear_marker() {
    let _ = std::fs::remove_file(marker_path());
}

/// manager 启动时调用：上次在扫描中死掉留下的标记说明 daemon 还停在停止态，
/// 这里把它拉回来。起不来则保留标记，下次启动再试（宁可多试也不静默丢采集）。
pub fn restore_pending() {
    if !marker_path().exists() {
        return;
    }
    std::thread::spawn(|| {
        if ctl::running() {
            clear_marker();
            return;
        }
        if ctl::start().is_ok() {
            clear_marker();
        }
    });
}

/// 跑一次扫描（阻塞，适合放线程里）。
/// standalone=false（GUI 面板路径）拉回的 daemon 带默认的 manager 监视；
/// standalone=true（CLI 扫描）用 --watch-manager=0 拉回——CLI 进程马上退出，
/// 没人常驻，带监视的 daemon 会在宽限后被自己关掉。
/// probe_cancel 是逐台探测子进程的取消坑位，与 cancel_slot 一起由
/// cancel_scan 强杀。失败情形（daemon 停不掉 / 扫描进程起不来 / 蓝牙适配器
/// 不可用）记在 outcome.error 里；三种情形里 daemon 都会被尽力拉回原样
/// （停过的话）。
pub fn run_scan(
    secs: u32,
    stop: &AtomicBool,
    cancel_slot: &CancelSlot,
    probe_cancel: &CancelSlot,
    standalone: bool,
    mut on_event: impl FnMut(ScanEvent),
) -> ScanOutcome {
    let mut outcome = ScanOutcome {
        count: 0,
        stopped_daemon: false,
        daemon_restored: false,
        cancelled: false,
        error: None,
    };

    // ---- 1. 手表被连着就不广播：先停 daemon
    let was_running = ctl::running();
    if was_running {
        on_event(ScanEvent::Stopping);
        write_marker();
        if let Err(e) = ctl::stop() {
            outcome.error = Some(e); // 停不掉就放弃扫描；daemon 原样在跑
            clear_marker();
            return outcome;
        }
        outcome.stopped_daemon = true;

        if stop.load(Ordering::Relaxed) {
            restore(&mut outcome, standalone);
            outcome.cancelled = true;
            return outcome;
        }
        on_event(ScanEvent::Waiting);
        // 断开之后手表要过一下才重新开始广播
        sleep_cancellable(stop, 2000);
        if stop.load(Ordering::Relaxed) {
            restore(&mut outcome, standalone);
            outcome.cancelled = true;
            return outcome;
        }
    }

    // ---- 2. 起 --scan 子进程，逐行读 JSON
    let exe = ctl::daemon_path();
    if !exe.exists() {
        restore(&mut outcome, standalone);
        outcome.error = Some(format!("找不到 {}，先构建：cmd /c daemon\\build.cmd", exe.display()));
        return outcome;
    }
    let args = vec!["--scan".to_string(), secs.to_string(), "--quiet".to_string()];
    let child = match proc::spawn_child(&exe, &args) {
        Ok(c) => c,
        Err(e) => {
            restore(&mut outcome, standalone);
            outcome.error = Some(e);
            return outcome;
        }
    };
    *cancel_slot.lock().unwrap() = Some(proc::CancelHandle::from_process(child.process));
    if stop.load(Ordering::Relaxed) {
        // 取消刚好赶在坑位写入前：补一刀，让读流线程马上 EOF
        child.kill();
    }
    let mut child = child;

    // ---- 3. 双线程编排：扫描读流 + 逐台探测，事件经 mpsc 汇到本线程统一
    // on_event。两个通道：
    //   req：读流线程 → 探测线程。每发现一台新 mac 入队一次；已知 mac 的名字
    //        补全不重复入队（探测同一台只会白占串行队列，真名随首台回来）。
    //   ev ：两个线程 → 本线程。本线程不留发送端，ev 通道全关（recv Err）=
    //        两个线程都收尾——正常 EOF 与取消 kill 都走这里，阻塞 recv 天然被
    //        叫醒，不需要轮询。
    // 用 thread::scope 让两个工作线程借用 stop / probe_cancel 等局部引用，
    // 作用域结束时自动 join；Child 留在本线程，wait/清坑位在收尾段做。
    let (req_tx, req_rx) = mpsc::channel::<String>();
    let (ev_tx, ev_rx) = mpsc::channel::<ScanEvent>();

    let mut held_done: Option<ScanEvent> = None;
    std::thread::scope(|scope| {
        // 读流线程只接管 stdout（File 是 Send）。取消的 kill 由 cancel_scan 经
        // CancelHandle 副本完成，这里只管把管道读到 EOF。
        let reader_tx = ev_tx.clone();
        let stdout = child.stdout.take().expect("spawn_child 一定带 stdout");
        scope.spawn(move || {
            let mut reader = std::io::BufReader::new(stdout);
            let mut queued: HashSet<String> = HashSet::new();   // 已入队探测的 mac
            let mut count = 0usize;
            loop {
                let mut line = Vec::new();
                match reader.read_until(b'\n', &mut line) {
                    Ok(0) => break,   // EOF：子进程结束（或被取消 kill 掉）
                    Ok(_) => {}
                    Err(_) => break,
                }
                if stop.load(Ordering::Relaxed) {
                    continue; // 取消后丢弃残余行，等 EOF 收口
                }
                let text = String::from_utf8_lossy(&line);
                for part in text.split('\n') {
                    let part = part.trim();
                    if part.is_empty() {
                        continue;
                    }
                    match parse_line(part) {
                        Some(Parsed::Device { mac, name }) => {
                            let _ = reader_tx.send(ScanEvent::Device { mac: mac.clone(), name });
                            // 发现即入队：集合按大写去重，同一台的名字补全不再排
                            if queued.insert(mac.to_ascii_uppercase()) {
                                let _ = req_tx.send(mac);
                            }
                        }
                        Some(Parsed::Done { count: n }) => count = n,
                        None => {} // 解析不了的行（版本差异）直接忽略
                    }
                }
            }
            drop(req_tx);   // 关请求通道 = 告诉探测线程不会再有新设备
            let _ = reader_tx.send(ScanEvent::Done { count });
        });

        // 探测线程：串行消费新设备请求，一台一发 --probe-address 子进程（蓝牙
        // 栈上并发连多台不可靠，识别又只是锦上添花，排队最稳）。
        let probe_tx = ev_tx.clone();
        let probe_exe = ctl::daemon_path();
        scope.spawn(move || {
            while let Ok(mac) = req_rx.recv() {
                probe_one(&probe_exe, &mac, stop, probe_cancel, &probe_tx);
            }
        });
        drop(ev_tx);   // 本线程不留发送端：两个线程都收尾后 recv() 才会 Err

        // ---- 4. 事件转发。Done 行先攒着：等探测线程退出（队列排空、最后一台
        // 也发了 ProbeDone）再发 on_event(Done)——识别中的面板要一直停留在扫描
        // 进行态，扫描按钮的禁用语义保持不变。收到 Done 与发 Done 之间，探测
        // 事件照常逐条转发，前端实时回填。
        loop {
            match ev_rx.recv() {
                Ok(ScanEvent::Done { count }) => {
                    outcome.count = count;
                    held_done = Some(ScanEvent::Done { count });
                }
                Ok(ev) => on_event(ev),
                Err(_) => break,   // 两个线程都收尾了
            }
        }
    });

    // ---- 5. 收尾：作用域已 join 两个工作线程；Child 的 wait/清坑位在这里做
    let code = child.wait(5000);
    *cancel_slot.lock().unwrap() = None;

    // 取消/失败路径都不发 Done：面板会被 cancelled 回复或错误提示整体复位，
    // 迟到的「扫描完成」闪一下反而误导（与旧代码"先查 stop/退出码再 Done"的
    // 次序一致）。只有识别队列排空、扫描进程正常退出才报完成。
    if stop.load(Ordering::Relaxed) {
        restore(&mut outcome, standalone);
        outcome.cancelled = true;
        return outcome;
    }
    if code != 0 {
        restore(&mut outcome, standalone);
        outcome.error = Some("扫描起不来（蓝牙适配器关了吗？原因见 hr-daemon.log）".into());
        return outcome;
    }
    if let Some(ev) = held_done {
        on_event(ev);
    }

    outcome
}

/// 探测一台设备：拉起 `hr-daemon --probe-address` 子进程，把 probe/done 行
/// 翻译成 ScanEvent 发回 ev 通道。ProbeDone 统一在子进程收尾（EOF）后发
/// ——被 kill 时子进程吐不出 done 行，EOF 是唯一保证恰好一条的收口。
/// 起不来（exe 缺失/管道失败）只发 ProbeDone，不算扫描失败。
fn probe_one(
    exe: &Path,
    mac: &str,
    stop: &AtomicBool,
    probe_cancel: &CancelSlot,
    ev_tx: &mpsc::Sender<ScanEvent>,
) {
    if stop.load(Ordering::Relaxed) {
        let _ = ev_tx.send(ScanEvent::ProbeDone { mac: mac.to_string() });
        return;
    }
    let args = vec![
        "--probe-address".to_string(),
        mac.to_string(),
        "--probe-sec".to_string(),
        "6".to_string(),
        "--quiet".to_string(),
    ];
    let mut child = match proc::spawn_child(exe, &args) {
        Ok(c) => c,
        Err(_) => {
            let _ = ev_tx.send(ScanEvent::ProbeDone { mac: mac.to_string() });
            return;
        }
    };
    *probe_cancel.lock().unwrap() = Some(proc::CancelHandle::from_process(child.process));
    if stop.load(Ordering::Relaxed) {
        // 取消刚好赶在坑位写入前：补一刀，别让这一台拖到自然超时
        child.kill();
    }
    let stdout = child.stdout.take().expect("spawn_child 一定带 stdout");
    let mut reader = std::io::BufReader::new(stdout);
    loop {
        let mut line = Vec::new();
        match reader.read_until(b'\n', &mut line) {
            Ok(0) => break,
            Ok(_) => {}
            Err(_) => break,
        }
        if stop.load(Ordering::Relaxed) {
            child.kill();
            continue;
        }
        let text = String::from_utf8_lossy(&line);
        for part in text.split('\n') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            match parse_probe_line(part) {
                Some(ProbeParsed::Probe { name, bpm }) => {
                    let _ = ev_tx.send(ScanEvent::Probe { mac: mac.to_string(), name, bpm });
                }
                // done 行不直接转发：收尾事件走上面的 EOF 路径
                Some(ProbeParsed::Done) | None => {}
            }
        }
    }
    let _ = child.wait(5000);
    *probe_cancel.lock().unwrap() = None;
    let _ = ev_tx.send(ScanEvent::ProbeDone { mac: mac.to_string() });
}

enum ProbeParsed {
    Probe { name: String, bpm: i32 },
    Done,
}

fn parse_probe_line(line: &str) -> Option<ProbeParsed> {
    let v: Value = serde_json::from_str(line).ok()?;
    match v.get("type")?.as_str()? {
        "probe" => Some(ProbeParsed::Probe {
            name: v.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string(),
            bpm: v.get("bpm").and_then(|b| b.as_i64()).unwrap_or(-1) as i32,
        }),
        "done" => Some(ProbeParsed::Done),
        _ => None,
    }
}

fn restore(outcome: &mut ScanOutcome, standalone: bool) {
    if outcome.stopped_daemon && !outcome.daemon_restored {
        let r = if standalone { ctl::start_standalone() } else { ctl::start() };
        if r.is_ok() {
            outcome.daemon_restored = true;
            clear_marker();
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

/// 取消扫描：置位 stop 标志并强杀扫描与探测两个子进程。扫描/探测线程都阻塞
/// 在读管道上，只置位不 kill 叫不醒它们，所以两个坑位必须一起杀。
/// 幂等：没在扫描时调用无害，stop 标志会在下次扫描线程开头复位；没在扫描
/// 时两个坑位本来就是 None。
pub fn cancel_scan(core: &Arc<Core>) {
    core.scan_stop.store(true, Ordering::SeqCst);
    // take + 强杀同一个锁区间完成；CancelHandle 持有独立句柄副本，
    // 和 worker 线程 Child 的 Drop 互不影响（踩过的锁外 take+杀的窗口期）
    let h = core.scan_cancel.lock().unwrap().take();
    if let Some(h) = h {
        h.terminate();
    }
    let h = core.scan_probe_cancel.lock().unwrap().take();
    if let Some(h) = h {
        h.terminate();
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
/// 拉回一律用独立模式（--watch-manager=0）：CLI 进程马上退出，daemon 必须
/// 能独立常驻，否则宽限一到它就自己退了（详见 daemon_ctl::start_standalone）。
pub fn run_scan_cli(secs: u32) -> i32 {
    use std::io::Write as _;
    use std::sync::atomic::AtomicBool;

    println!("正在扫描心率广播设备（{} 秒）… 手表请停在\"心率广播\"页面并保持亮屏。", secs);
    let stop = AtomicBool::new(false);
    let cancel_slot: CancelSlot = Mutex::new(None);
    let probe_cancel: CancelSlot = Mutex::new(None);
    let mut devices: Vec<(String, String)> = Vec::new();
    let out = run_scan(secs, &stop, &cancel_slot, &probe_cancel, true, |ev| match ev {
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
        ScanEvent::Probe { mac, name, bpm } => {
            // 逐台识别结果：真名 / 实时心率边探边打
            if bpm >= 0 {
                println!("  识别 {:<17} {}  {} bpm",
                         mac, if name.is_empty() { "（还没拿到名字）" } else { &name }, bpm);
            } else if !name.is_empty() {
                println!("  识别 {:<17} {}", mac, &name);
            } else {
                println!("  识别中…（{}）", mac);
            }
        }
        ScanEvent::ProbeDone { .. } => {}
        ScanEvent::Done { .. } => {}
    });

    if let Some(e) = &out.error {
        eprintln!("扫描失败：{}", e);
        // 停了 daemon 却没拉回来：CLI 里只能提示，标记文件留着给下次 manager 启动兜底
        if out.stopped_daemon && !out.daemon_restored {
            eprintln!("hr-daemon 还停着，重开一次 hr-manager（面板/托盘）会自动把它拉起来。");
        }
        return 1;
    }
    println!("扫描完成：{} 台设备。", out.count);
    if out.stopped_daemon && !out.daemon_restored {
        match ctl::start_standalone() {
            Ok(()) => {
                println!("已把 hr-daemon 按原配置拉起来。");
                clear_marker();
            }
            Err(e) => {
                eprintln!("hr-daemon 没能自动拉起：{}", e);
                eprintln!("重开一次 hr-manager（面板/托盘）会自动把它拉起来。");
                return 1;
            }
        }
    }
    if out.cancelled {
        println!("扫描已取消。");
    }
    if devices.is_empty() {
        println!("没扫到设备？检查手表是否停在\"心率广播\"页面并保持亮屏、蓝牙是否打开。");
    }
    0
}
