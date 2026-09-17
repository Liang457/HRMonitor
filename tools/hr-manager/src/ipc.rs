// src/ipc.rs —— 面板（WebView）→ 管理器的命令分发。
//
// 前端 window.ipc.postMessage(JSON 字符串)；这里解析、执行、用
// Core::reply（evaluate_script）把结果配对 id 送回去。
// 快命令（读配置、注册表开关）就地执行；慢命令（重启 daemon、扫描、部署脚本）
// 丢线程，结果经 proxy 送回 —— ipc 回调线程不能被 20 秒的 daemon 收尾卡住。
use crate::autostart;
use crate::config;
use crate::daemon_ctl as ctl;
use crate::state::Core;
use crate::{scan_job, win};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
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
            match config::load() {
                Ok((cfg, path, exists)) => {
                    let mut fields = serde_json::Map::new();
                    for f in config::FIELDS {
                        fields.insert(
                            f.key.to_string(),
                            json!(cfg.get_raw(f.key).unwrap_or_default()),
                        );
                    }
                    core.timeout_ms.store(cfg.timeout_ms as u32, Ordering::SeqCst);
                    core.reply(
                        id,
                        json!({
                            "ok": true,
                            "config": fields,
                            "iniPath": path.display().to_string(),
                            "iniExists": exists,
                            "daemonExe": ctl::daemon_path().display().to_string(),
                            "daemonRunning": ctl::running(),
                            "autostart": autostart::get().is_some(),
                            "oldTask": core.old_task.load(Ordering::SeqCst),
                            "logDir": config::exe_dir().display().to_string(),
                        }),
                    );
                }
                Err(e) => core.reply(id, json!({ "ok": false, "error": e })),
            }
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
                core2.scan_needs_restore.store(false, Ordering::SeqCst);
                let result = scan_job::run_scan(secs, &core2.scan_stop, &core2.scan_cancel, |ev| {
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
                match result {
                    Ok(out) if out.cancelled => {
                        core2.reply(id, json!({ "ok": true, "cancelled": true }))
                    }
                    Ok(out) => {
                        // 正常扫完：daemon 保持停止等用户选表；用户放弃时要拉回来
                        if out.stopped_daemon {
                            core2.scan_needs_restore.store(true, Ordering::SeqCst);
                        }
                        core2.reply(id, json!({ "ok": true, "count": out.count, "daemonStopped": out.stopped_daemon }));
                    }
                    Err(e) => {
                        core2.toast(&e);
                        core2.reply(id, json!({ "ok": false, "error": e }));
                    }
                }
            });
        }

        "scan_cancel" => {
            core.scan_stop.store(true, Ordering::SeqCst);
            if let Some(h) = core.scan_cancel.lock().unwrap().take() {
                unsafe { win::TerminateProcess(h.0, 1) };
            }
            core.reply(id, json!({ "ok": true }));
        }

        "scan_discard" => {
            // 用户看完结果没选：把 daemon 拉回来
            if core.scan_needs_restore.swap(false, Ordering::SeqCst) {
                let core2 = core.clone();
                std::thread::spawn(move || match ctl::start() {
                    Ok(()) => core2.reply(id, json!({ "ok": true })),
                    Err(e) => {
                        core2.toast(&e);
                        core2.reply(id, json!({ "ok": false, "error": e }));
                    }
                });
            } else {
                core.reply(id, json!({ "ok": true }));
            }
        }

        "apply_address" => {
            let mac = arg("mac").as_str().unwrap_or("").to_string();
            let core2 = core.clone();
            std::thread::spawn(move || {
                core2.scan_needs_restore.store(false, Ordering::SeqCst);
                let result = (|| -> Result<String, String> {
                    let (mut cfg, path, _) = config::load()?;
                    let mut note = String::new();
                    if cfg.demo {
                        cfg.demo = false;
                        note = "（已顺带关闭 demo 模拟源）".to_string();
                    }
                    cfg.set("address", &mac)?;
                    config::save(&cfg, &path)?;
                    let msg = ctl::restart()?;
                    Ok(format!("{} 已保存并重启 hr-daemon{}", msg, note))
                })();
                match result {
                    Ok(m) => core2.reply(id, json!({ "ok": true, "message": m })),
                    Err(e) => core2.reply(id, json!({ "ok": false, "error": e })),
                }
            });
        }

        "reset_config" => {
            let r = (|| -> Result<(), String> {
                let (_, path, _) = config::load()?;
                config::save(&config::Config::default(), &path)
            })();
            match r {
                Ok(()) => core.reply(id, json!({ "ok": true })),
                Err(e) => core.reply(id, json!({ "ok": false, "error": e })),
            }
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
            let dir = config::exe_dir();
            let d = win::wide(&dir.display().to_string());
            let verb = win::wide("open");
            let rc = unsafe { win::ShellExecuteW(std::ptr::null_mut(), verb.as_ptr(), d.as_ptr(), std::ptr::null(), std::ptr::null(), win::SW_SHOWNORMAL) };
            core.reply(id, json!({ "ok": rc > 32, "error": if rc > 32 { Value::Null } else { json!("打开文件夹失败") } }));
        }

        "run_script" => {
            let which = arg("which").as_str().unwrap_or("").to_string();
            if !matches!(which.as_str(), "tm" | "ab") {
                core.reply(id, json!({ "ok": false, "error": "未知脚本" }));
                return;
            }
            let core2 = core.clone();
            std::thread::spawn(move || match run_deploy_script(&which) {
                Ok((code, output)) => {
                    let tail: String = {
                        // 只留尾部一段，界面显示得下；完整输出在控制台跑也能拿到
                        let chars: Vec<char> = output.chars().collect();
                        let start = chars.len().saturating_sub(3000);
                        chars[start..].iter().collect()
                    };
                    core2.reply(id, json!({ "ok": code == 0, "code": code, "output": tail }));
                }
                Err(e) => core2.reply(id, json!({ "ok": false, "error": e })),
            });
        }

        "run_script_elevated" => {
            let which = arg("which").as_str().unwrap_or("").to_string();
            match script_path(&which) {
                Some(script) => {
                    let args = powershell_command_args(&script, &tm_dir_arg());
                    let params = args.iter().map(|a| crate::proc::quote_arg(a)).collect::<Vec<_>>().join(" ");
                    let exe_w = win::wide("powershell.exe");
                    let verb = win::wide("runas");
                    let params_w = win::wide(&params);
                    let rc = unsafe {
                        win::ShellExecuteW(std::ptr::null_mut(), verb.as_ptr(), exe_w.as_ptr(), params_w.as_ptr(), std::ptr::null(), win::SW_SHOWNORMAL)
                    };
                    if rc > 32 {
                        core.toast("已请求管理员权限运行，请看弹出的窗口；完成后回到这里确认状态");
                        core.reply(id, json!({ "ok": true }));
                    } else {
                        core.reply(id, json!({ "ok": false, "error": format!("提权启动失败（错误码 {}；用户可能拒绝了 UAC）", rc) }));
                    }
                }
                None => core.reply(id, json!({ "ok": false, "error": "找不到部署脚本（scripts 目录不在程序旁边）" })),
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
    let (mut cfg, path, _) = config::load()?;
    for (k, v) in map {
        let s = v.as_str().unwrap_or_default();
        cfg.set(k, s).map_err(|e| format!("{}：{}", k, e))?;
    }
    config::save(&cfg, &path)?;
    Ok((cfg, true))
}

fn script_path(which: &str) -> Option<PathBuf> {
    let name = match which {
        "tm" => "configure-trafficmonitor.ps1",
        "ab" => "deploy-afterburner-plugin.ps1",
        _ => return None,
    };
    let dir = config::exe_dir();
    let candidates = [dir.join("scripts").join(name), dir.join("..").join("scripts").join(name)];
    candidates.into_iter().find(|p| p.exists())
}

fn tm_dir_arg() -> String {
    config::load()
        .map(|(cfg, _, _)| cfg.tm_dir)
        .unwrap_or_default()
}

/// powershell 的完整参数表：先强制 UTF-8 输出（面板里中文不乱码），再跑脚本。
/// 每个元素独立成参，交给 quote_arg 转义，不会被命令行解析拆散。
fn powershell_command_args(script: &PathBuf, tm_dir: &str) -> Vec<String> {
    let mut extra = String::new();
    if script.file_name().map(|n| n == "configure-trafficmonitor.ps1").unwrap_or(false) && !tm_dir.is_empty() {
        extra = format!(" -TmDir '{}'", tm_dir.replace('\'', "''"));
    }
    let inner = format!(
        "[Console]::OutputEncoding=[System.Text.Encoding]::UTF8; & '{}'{}",
        script.display().to_string().replace('\'', "''"),
        extra
    );
    vec![
        "-NoProfile".into(),
        "-ExecutionPolicy".into(),
        "Bypass".into(),
        "-Command".into(),
        inner,
    ]
}

/// 隐藏窗口跑部署脚本，收走全部输出（stdout+stderr 同一根管道）
fn run_deploy_script(which: &str) -> Result<(u32, String), String> {
    let script = script_path(which)
        .ok_or_else(|| "找不到部署脚本（scripts 目录不在程序旁边）".to_string())?;
    let args = powershell_command_args(&script, &tm_dir_arg());
    crate::proc::run_and_wait_capture(Path::new("powershell.exe"), &args, 600_000)
}
