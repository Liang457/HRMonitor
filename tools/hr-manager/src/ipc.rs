// src/ipc.rs —— 面板（WebView）→ 管理器的命令分发。
//
// 前端 window.ipc.postMessage(JSON 字符串)；这里解析、执行、用
// Core::reply（evaluate_script）把结果配对 id 送回去。
// 快命令（读配置、注册表开关）就地执行；慢命令（重启 daemon、扫描）
// 丢线程，结果经 proxy 送回 —— ipc 回调线程不能被 20 秒的 daemon 收尾卡住。
use crate::autostart;
use crate::config;
use crate::daemon_ctl as ctl;
use crate::state::Core;
use crate::{scan_job, win};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;

pub fn dispatch(core: &Arc<Core>, body: &str) {
    let v: Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(_) => return, // 心跳/未知格式，忽略
    };
    let id = v.get("id").and_then(|x| x.as_u64()).unwrap_or(0);
    let cmd = v.get("cmd").and_then(|x| x.as_str()).unwrap_or("").to_string();
    let arg = |key: &str| v.get(key).cloned().unwrap_or(Value::Null);

    match cmd.as_str() {
        "hello" => {
            core.reply(id, json!({ "ok": true }));
        }

        "get_config" => {
            // 读 ini + 注册表 + FindWindow 不快，丢线程，别卡住 IPC 回调（主线程）
            let core2 = core.clone();
            std::thread::spawn(move || {
                match config::load() {
                    Ok((cfg, path, exists)) => {
                        let mut fields = serde_json::Map::new();
                        for f in config::FIELDS {
                            fields.insert(
                                f.key.to_string(),
                                json!(cfg.get_raw(f.key).unwrap_or_default()),
                            );
                        }
                        core2.timeout_ms.store(cfg.timeout_ms as u32, Ordering::SeqCst);
                        core2.reply(
                            id,
                            json!({
                                "ok": true,
                                "config": fields,
                                "iniPath": path.display().to_string(),
                                "iniExists": exists,
                                "daemonExe": ctl::daemon_path().display().to_string(),
                                "daemonRunning": ctl::running(),
                                "autostart": autostart::get().is_some(),
                                "oldTask": core2.old_task.load(Ordering::SeqCst),
                                "logDir": config::exe_dir().join("log").display().to_string(),
                                "version": env!("CARGO_PKG_VERSION"),
                            }),
                        );
                    }
                    Err(e) => core2.reply(id, json!({ "ok": false, "error": e })),
                }
            });
        }

        "set_config" => {
            let values = arg("values");
            match apply_values(&values) {
                Ok((cfg, saved)) => {
                    core.timeout_ms.store(cfg.timeout_ms as u32, Ordering::SeqCst);
                    core.reply(id, json!({ "ok": true, "saved": saved }));
                }
                Err(e) => core.reply(id, json!({ "ok": false, "error": e })),
            }
        }

        "restart_daemon" | "start_daemon" | "stop_daemon" => {
            let core2 = core.clone();
            std::thread::spawn(move || {
                let result = match cmd.as_str() {
                    "restart_daemon" => ctl::restart().map(|m| json!({ "ok": true, "message": m })),
                    "start_daemon" => ctl::start().map(|_| json!({ "ok": true, "message": "已启动 hr-daemon" })),
                    _ => ctl::stop().map(|_| json!({ "ok": true, "message": "已停止 hr-daemon" })),
                };
                match result {
                    Ok(p) => core2.reply(id, p),
                    Err(e) => {
                        core2.toast(&e);
                        core2.reply(id, json!({ "ok": false, "error": e }));
                    }
                }
            });
        }

        "scan_start" => {
            let secs = arg("secs").as_u64().unwrap_or(10).clamp(1, 120) as u32;
            if core.scan_running.swap(true, Ordering::SeqCst) {
                core.reply(id, json!({ "ok": false, "error": "已有一次扫描在进行" }));
                return;
            }
            let core2 = core.clone();
            std::thread::spawn(move || {
                core2.scan_stop.store(false, Ordering::SeqCst);
                let out = scan_job::run_scan(secs, &core2.scan_stop, &core2.scan_cancel, |ev| {
                    let payload = match ev {
                        scan_job::ScanEvent::Stopping => json!({ "type": "stopping" }),
                        scan_job::ScanEvent::Waiting => json!({ "type": "waiting" }),
                        scan_job::ScanEvent::Device { mac, name } => {
                            json!({ "type": "device", "mac": mac, "name": name })
                        }
                        scan_job::ScanEvent::Done { count } => json!({ "type": "done", "count": count }),
                    };
                    core2.eval(format!("window.__hr.scanEvent({});", payload));
                });
                core2.scan_running.store(false, Ordering::SeqCst);
                if let Some(e) = &out.error {
                    // 失败：停了没拉回来的（本次或上次扫描留下的）都就地兜底
                    if out.stopped_daemon && !out.daemon_restored {
                        core2.scan_needs_restore.store(true, Ordering::SeqCst);
                    }
                    if let Err(e2) = restore_daemon_if_needed(&core2) {
                        core2.toast(&e2);
                    }
                    core2.toast(&e.clone());
                    core2.reply(id, json!({ "ok": false, "error": e }));
                    return;
                }
                if out.cancelled {
                    // 取消 = 放弃：立即恢复。本次停的 run_scan 已经拉过；
                    // 没拉成、或上次扫描留下的停止态由这里兜底。
                    if out.stopped_daemon && !out.daemon_restored {
                        core2.scan_needs_restore.store(true, Ordering::SeqCst);
                    }
                    if let Err(e2) = restore_daemon_if_needed(&core2) {
                        core2.toast(&e2);
                    }
                    core2.reply(id, json!({ "ok": true, "cancelled": true }));
                    return;
                }
                // 正常扫完：daemon 保持停止等用户选表（标志/标记都留着）。
                // 面板已经没了就没人来选了——直接恢复，别把标志留给空气。
                if out.stopped_daemon {
                    core2.scan_needs_restore.store(true, Ordering::SeqCst);
                    if !core2.panel_open.load(Ordering::SeqCst) {
                        if let Err(e2) = restore_daemon_if_needed(&core2) {
                            core2.toast(&e2);
                        }
                    }
                }
                core2.reply(id, json!({ "ok": true, "count": out.count, "daemonStopped": out.stopped_daemon }));
            });
        }

        "scan_cancel" => {
            core.scan_stop.store(true, Ordering::SeqCst);
            // take + 强杀同一个锁区间完成；CancelHandle 持有独立句柄副本，
            // 和扫描线程 Child 的 Drop 互不影响
            let h = core.scan_cancel.lock().unwrap().take();
            if let Some(h) = h {
                h.terminate();
            }
            core.reply(id, json!({ "ok": true }));
        }

        "scan_discard" => {
            // 用户看完结果没选：把 daemon 拉回来
            let core2 = core.clone();
            std::thread::spawn(move || {
                let ok = match restore_daemon_if_needed(&core2) {
                    Ok(()) => true,
                    Err(e) => {
                        core2.toast(&e);
                        false
                    }
                };
                core2.reply(id, json!({ "ok": ok, "error": if ok { Value::Null } else { json!("hr-daemon 没能拉起来，可点“启动”重试") } }));
            });
        }

        "apply_address" => {
            let mac = arg("mac").as_str().unwrap_or("").to_string();
            let core2 = core.clone();
            std::thread::spawn(move || {
                let result = (|| -> Result<(String, String), String> {
                    let mut note = String::new();
                    {
                        // 锁只护住 load→set→save 事务；restart 最长能卡 20 秒，
                        // 别抱着 ini 锁等它收尾
                        let _guard = config::ini_lock();
                        let (mut cfg, path, _) = config::load()?;
                        if cfg.demo {
                            cfg.demo = false;
                            note = "（已顺带关闭 demo 模拟源）".to_string();
                        }
                        cfg.set("address", &mac)?;
                        config::save(&cfg, &path)?;
                    }
                    let msg = ctl::restart()?;
                    Ok((msg, note))
                })();
                match result {
                    // 重启成功 = daemon 已经在跑，扫描留下的"欠恢复"一笔勾销
                    Ok((msg, note)) => {
                        core2.scan_needs_restore.store(false, Ordering::SeqCst);
                        scan_job::clear_marker();
                        core2.reply(id, json!({ "ok": true, "message": format!("{} 已保存并重启 hr-daemon{}", msg, note) }))
                    }
                    Err(e) => core2.reply(id, json!({ "ok": false, "error": e })),
                }
            });
        }

        "reset_config" => {
            let core2 = core.clone();
            std::thread::spawn(move || {
                let r = (|| -> Result<(), String> {
                    let _guard = config::ini_lock();
                    let (_, path, _) = config::load()?;
                    config::save(&config::Config::default(), &path)
                })();
                match r {
                    Ok(()) => core2.reply(id, json!({ "ok": true })),
                    Err(e) => core2.reply(id, json!({ "ok": false, "error": e })),
                }
            });
        }

        "autostart_set" => {
            let enable = arg("enabled").as_bool().unwrap_or(false);
            let r = if enable { autostart::enable() } else { autostart::disable() };
            match r {
                Ok(()) => core.reply(id, json!({ "ok": true, "enabled": enable })),
                Err(e) => core.reply(id, json!({ "ok": false, "error": e })),
            }
        }

        "open_logs" => {
            // 日志在 exe 同级 log\ 子目录；还没跑过 daemon 时目录不存在，先建，
            // 否则资源管理器会弹"找不到路径"。
            let dir = config::exe_dir().join("log");
            let _ = std::fs::create_dir_all(&dir);
            let d = win::wide(&dir.display().to_string());
            let verb = win::wide("open");
            let rc = unsafe { win::ShellExecuteW(std::ptr::null_mut(), verb.as_ptr(), d.as_ptr(), std::ptr::null(), std::ptr::null(), win::SW_SHOWNORMAL) };
            core.reply(id, json!({ "ok": rc > 32, "error": if rc > 32 { Value::Null } else { json!("打开文件夹失败") } }));
        }

        "open_folder" => {
            // 打开第三方宿主的插件目录，引导手动部署。目录定位失败回 error，
            // 面板会 toast（安装目录可在上方手填，留空则按常见位置探测）。
            let which = arg("which").as_str().unwrap_or("").to_string();
            match open_plugin_folder(&which) {
                Ok(()) => core.reply(id, json!({ "ok": true })),
                Err(e) => core.reply(id, json!({ "ok": false, "error": e })),
            }
        }

        _ => {
            core.reply(id, json!({ "ok": false, "error": format!("未知命令：{}", cmd) }));
        }
    }
}

/// 前端一次提交一批值：逐项校验，全部合法才写文件。
/// 返回 (校验后的配置, 是否实际保存了)。
fn apply_values(values: &Value) -> Result<(config::Config, bool), String> {
    let map = values.as_object().ok_or("values 得是对象")?;
    // load→改→save 全程持锁：别的线程同时保存时不会读出半新半旧再写回去
    let _guard = config::ini_lock();
    let (mut cfg, path, _) = config::load()?;
    for (k, v) in map {
        let s = v.as_str().unwrap_or_default();
        cfg.set(k, s).map_err(|e| format!("{}：{}", k, e))?;
    }
    config::save(&cfg, &path)?;
    Ok((cfg, true))
}

/// daemon 因扫描停着（粘滞标志）→ 就地拉回来。成功清标志；已经在跑只清标志
/// （比如 run_scan 内部已经恢复过）；失败把标志放回去，留给 close_panel /
/// 下次启动重试，并返回错误让调用方决定怎么提示。
fn restore_daemon_if_needed(core: &Arc<Core>) -> Result<(), String> {
    if !core.scan_needs_restore.swap(false, Ordering::SeqCst) {
        return Ok(());
    }
    if ctl::running() {
        scan_job::clear_marker();
        return Ok(());
    }
    match ctl::start() {
        Ok(()) => {
            scan_job::clear_marker();
            Ok(())
        }
        Err(e) => {
            core.scan_needs_restore.store(true, Ordering::SeqCst);
            Err(e)
        }
    }
}

/// 打开"第三方程序部署"相关目录（部署改成手动复制后的入口）：
///   plugins —— 本程序旁边的 plugins\（发行包里两个插件 DLL 就在这）
///   tm      —— TrafficMonitor 的 plugins\
///   ab      —— MSI Afterburner 的 Plugins\Monitoring\
/// tm/ab 的安装目录优先取 ini（integration.tm_dir / ab_dir），留空则按常见
/// 安装位置探测；找不到不打开任何窗口，返回给人看的错误让面板提示。
fn open_plugin_folder(which: &str) -> Result<(), String> {
    let dir = match which {
        "plugins" => config::exe_dir().join("plugins"),
        "tm" | "ab" => {
            let cfg = config::load()?.0;
            let configured = if which == "tm" { &cfg.tm_dir } else { &cfg.ab_dir };
            let configured = configured.trim().to_string();
            let root = if configured.is_empty() {
                detect_host_dir(which)?
            } else {
                PathBuf::from(configured)
            };
            let sub = if which == "tm" { "plugins" } else { r"Plugins\Monitoring" };
            let target = root.join(sub);
            if target.is_dir() { target } else { root }
        }
        _ => return Err("未知目录".to_string()),
    };
    if !dir.is_dir() {
        return Err(format!("目录不存在：{}", dir.display()));
    }
    let d = win::wide(&dir.display().to_string());
    let verb = win::wide("open");
    let rc = unsafe {
        win::ShellExecuteW(std::ptr::null_mut(), verb.as_ptr(), d.as_ptr(), std::ptr::null(), std::ptr::null(), win::SW_SHOWNORMAL)
    };
    if rc > 32 { Ok(()) } else { Err("打开文件夹失败".to_string()) }
}

/// 按常见安装位置探测宿主目录（探测条件沿用原部署脚本：认宿主 exe）。
/// 故意不硬编码 D:\ 之类的机器布局——找不到就让用户在面板里填目录。
fn detect_host_dir(which: &str) -> Result<PathBuf, String> {
    let (name, marker) = if which == "tm" {
        ("TrafficMonitor", "TrafficMonitor.exe")
    } else {
        ("MSI Afterburner", "MSIAfterburner.exe")
    };
    for var in ["ProgramFiles", "ProgramFiles(x86)"] {
        if let Some(base) = std::env::var_os(var) {
            let cand = PathBuf::from(base).join(name);
            if cand.join(marker).exists() {
                return Ok(cand);
            }
        }
    }
    Err(format!("没找到 {}（按常见安装位置探测过了）。在上方填一下安装目录再试", name))
}
