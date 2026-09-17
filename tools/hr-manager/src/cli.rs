// src/cli.rs —— 命令行子命令。
//
// hr-manager 本体是 GUI 子系统的 Windows 程序（双击/自启出托盘和面板），
// 带子命令时走这里：先附加父控制台恢复标准句柄，再干活。
// GUI 才是主交互，CLI 是给脚本、CI 和愿意敲命令的人兜底的。
use crate::autostart;
use crate::config::{self, Config, FIELDS};
use crate::daemon_ctl as ctl;
use crate::scan_job;
use crate::win;
use std::io::Write as _;
use std::path::Path;

pub fn run(args: &[String]) -> i32 {
    match args[0].as_str() {
        "show" | "list" => cmd_show(),
        "get" => {
            if args.len() < 2 {
                eprintln!("用法: hr-manager get <配置项>");
                2
            } else {
                cmd_get(&args[1])
            }
        }
        "set" => {
            if args.len() < 3 {
                eprintln!("用法: hr-manager set <配置项> <值>");
                2
            } else {
                cmd_set(&args[1], &args[2..].join(" "))
            }
        }
        "reset" => cmd_reset(args.iter().any(|a| a == "-y" || a == "--yes")),
        "restart" => cmd_restart(),
        "start" => cmd_start(),
        "stop" => cmd_stop(),
        "status" => cmd_status(),
        "scan" => match args.get(1) {
            None => scan_job::run_scan_cli(5),
            Some(s) => match s.parse::<u32>() {
                Ok(v) if (1..=120).contains(&v) => scan_job::run_scan_cli(v),
                _ => {
                    eprintln!("扫描时长要 1~120 的整数秒，收到「{}」", s);
                    2
                }
            },
        },
        "path" => cmd_path(),
        "help" | "--help" | "-h" | "/?" => {
            usage();
            0
        }
        other => {
            eprintln!("未知命令：{}", other);
            usage();
            2
        }
    }
}

fn usage() {
    println!(
        r#"hr-manager —— 心率采集的管理器（托盘 + 配置面板；以下子命令是脚本/CI 兜底）

用法:
  hr-manager                     打开图形界面（托盘 + 面板）
  hr-manager --minimized         只出托盘，不弹面板（自启就是这么调的）
  hr-manager show                显示当前配置
  hr-manager get <配置项>        读一项
  hr-manager set <配置项> <值>   改一项（只写文件，重启后生效）
  hr-manager reset [-y]          恢复默认配置（-y 跳过确认）
  hr-manager status              daemon 状态 + 共享内存里的实时值
  hr-manager start | stop        启动 / 停止 hr-daemon
  hr-manager restart             重启 hr-daemon，让改动生效
  hr-manager scan [秒数]         扫描附近的心率广播设备（默认 5 秒）
  hr-manager path                显示各文件位置
  hr-manager help                显示本帮助

配置项（hr-manager show 也会列出来）:
  心率来源   demo  address  scan_timeout  backoff_min  backoff_max
  显示       timeout  refresh
  日志       log_kb  debug
  集成       tm_dir

例:
  hr-manager set demo 1          # 换成模拟心率源
  hr-manager restart             # 改完重启 daemon 生效
  hr-manager scan 10             # 扫 10 秒看看手表在不在广播

保存配置会先备份 <ini>.bak 再原子替换；写入中途不会留下半份配置。"#
    );
}

// ---------------------------------------------------------------- 控制台附加

/// GUI 子系统的 exe 从 cmd 里跑时没有标准句柄。附加父控制台并把
/// CONIN$/CONOUT$ 挂回标准句柄，println!/stdin 才有去处。
/// 必须赶在任何打印/读输入之前调用（Rust 的 std 句柄是惰性初始化的）。
pub fn attach_console() {
    unsafe {
        if !win::GetStdHandle(win::STD_OUTPUT_HANDLE).is_null() {
            return; // 本来就有（调试构建是控制台子系统）
        }
        if win::GetConsoleWindow().is_null() {
            win::AttachConsole(0xFFFF_FFFF /* ATTACH_PARENT_PROCESS */);
        }
        if win::GetConsoleWindow().is_null() {
            return; // 没有可附加的控制台（比如从注册表自启直接带参数），静默退出
        }
        let conout = win::CreateFileW(
            win::wide("CONOUT$").as_ptr(),
            win::GENERIC_WRITE,
            1 | 2, // FILE_SHARE_READ | FILE_SHARE_WRITE
            std::ptr::null_mut(),
            win::OPEN_EXISTING,
            0,
            std::ptr::null_mut(),
        );
        let conin = win::CreateFileW(
            win::wide("CONIN$").as_ptr(),
            win::GENERIC_READ,
            1, // FILE_SHARE_READ
            std::ptr::null_mut(),
            win::OPEN_EXISTING,
            0,
            std::ptr::null_mut(),
        );
        // CreateFileW 失败返回的是 INVALID_HANDLE_VALUE（-1）不是 null；
        // 把它塞给 SetStdHandle 会让 println!/read_line 静默失败
        let conout = if win::is_invalid(conout) { std::ptr::null_mut() } else { conout };
        let conin = if win::is_invalid(conin) { std::ptr::null_mut() } else { conin };
        if !conout.is_null() {
            win::SetStdHandle(win::STD_OUTPUT_HANDLE, conout);
            win::SetStdHandle(win::STD_ERROR_HANDLE, conout);
        }
        if !conin.is_null() {
            win::SetStdHandle(win::STD_INPUT_HANDLE, conin);
        }
    }
}

// ---------------------------------------------------------------- 加载/保存

use config::load;

macro_rules! load_or_exit {
    () => {
        match load() {
            Ok(v) => v,
            Err(e) => {
                eprintln!("{}", e);
                return 1;
            }
        }
    };
}

fn save(cfg: &Config, path: &Path) -> bool {
    match config::save(cfg, path) {
        Ok(()) => true,
        Err(e) => {
            eprintln!("保存失败：{}", e);
            false
        }
    }
}

fn desc_of(key: &str) -> String {
    FIELDS
        .iter()
        .find(|f| f.key == key)
        .map(|f| {
            if f.unit.is_empty() {
                f.desc.to_string()
            } else {
                format!("{}（{}）", f.desc, f.unit)
            }
        })
        .unwrap_or_else(|| key.to_string())
}

fn show_value(cfg: &Config, key: &str) -> String {
    let v = cfg.get(key).unwrap_or_default();
    match key {
        "address" if v.is_empty() => "(空，不连接)".into(),
        _ if v.is_empty() => "(空)".into(),
        _ => v,
    }
}

// ---------------------------------------------------------------- 子命令

fn cmd_show() -> i32 {
    let (cfg, path, exists) = load_or_exit!();
    println!(
        "配置文件: {}{}",
        path.display(),
        if exists { "" } else { "（不存在，下面是默认值）" }
    );
    println!();
    for f in FIELDS {
        println!("  {:<13} {} = {}", f.key, desc_of(f.key), show_value(&cfg, f.key));
    }
    0
}

fn cmd_get(key: &str) -> i32 {
    let (cfg, _, _) = load_or_exit!();
    match cfg.get(&key.to_ascii_lowercase()) {
        Some(v) => {
            println!("{}", v);
            0
        }
        None => {
            eprintln!("未知配置项：{}（用 hr-manager show 看有哪些）", key);
            2
        }
    }
}

fn cmd_set(key: &str, value: &str) -> i32 {
    // 键名统一小写（跟 get 一致）：`set DEMO 1` 和 `set demo 1` 应该是一个意思
    let key = &key.to_ascii_lowercase();
    let (mut cfg, path, _) = load_or_exit!();
    if let Err(e) = cfg.set(key, value) {
        eprintln!("{}", e);
        return 2;
    }
    if !save(&cfg, &path) {
        return 1;
    }
    println!("{} = {}  已保存", key, show_value(&cfg, key));
    println!("改动要重启 hr-daemon 才生效：hr-manager restart");
    0
}

fn cmd_reset(assume_yes: bool) -> i32 {
    let (_, path, exists) = load_or_exit!();
    if exists && !assume_yes {
        print!("这会用默认值覆盖 {}，继续？[y/N] ", path.display());
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        // 读不到输入（EOF/管道喂空）一律当"不做"：以前 EOF 被当成默认答案，
        // 破坏性更强的提问反而按了"是"，这个教训要记住
        let confirmed = matches!(std::io::stdin().read_line(&mut line), Ok(_))
            && matches!(line.trim(), "y" | "Y" | "yes" | "是");
        if !confirmed {
            println!("已取消");
            return 0;
        }
    }
    let cfg = Config::default();
    if !save(&cfg, &path) {
        return 1;
    }
    println!("已写入默认配置：{}", path.display());
    println!("改动要重启 hr-daemon 才生效：hr-manager restart");
    0
}

fn cmd_path() -> i32 {
    println!("配置文件 : {}", config::ini_path().display());
    println!("程序目录 : {}", config::exe_dir().display());
    println!("daemon   : {}", ctl::daemon_path().display());
    println!("日志     : {}", config::exe_dir().join("hr-daemon.log").display());
    0
}

fn cmd_restart() -> i32 {
    match ctl::restart() {
        Ok(msg) => {
            println!("{}：{}", msg, ctl::daemon_path().display());
            0
        }
        Err(e) => {
            eprintln!("{}", e);
            1
        }
    }
}

fn cmd_start() -> i32 {
    if ctl::running() {
        println!("hr-daemon 已经在运行");
        return 0;
    }
    match ctl::start() {
        Ok(()) => {
            println!("已启动 hr-daemon：{}", ctl::daemon_path().display());
            0
        }
        Err(e) => {
            eprintln!("{}", e);
            1
        }
    }
}

fn cmd_stop() -> i32 {
    if !ctl::running() {
        println!("hr-daemon 没在运行");
        return 0;
    }
    match ctl::stop() {
        Ok(()) => {
            println!("已停止 hr-daemon");
            0
        }
        Err(e) => {
            eprintln!("{}", e);
            1
        }
    }
}

fn cmd_status() -> i32 {
    let running = ctl::running();
    println!("hr-daemon : {}", if running { "运行中" } else { "未运行" });

    if running {
        let timeout_ms = load()
            .map(|(cfg, _, _)| cfg.timeout_ms as u64)
            .unwrap_or(15_000);
        let mut reader = crate::shared_mem::Reader::new();
        match reader.read() {
            Some(s) => {
                let now = crate::shared_mem::now_ms();
                let bpm = s.effective_bpm(now, timeout_ms);
                let age = now.saturating_sub(s.tick_ms);
                let state = describe_status(&s, bpm >= 0);
                println!(
                    "共享内存  : {}  状态={}（{} 秒前更新）",
                    s.device.is_empty().then(|| "（无设备名）".to_string()).unwrap_or_else(|| s.device.clone()),
                    state,
                    age / 1000
                );
                println!("心率      : {}", if bpm >= 0 { format!("{} bpm", bpm) } else { "--".to_string() });
            }
            None => println!("共享内存  : 读不到（daemon 刚起还没写过，或版本不匹配）"),
        }
    }

    match autostart::get() {
        Some(v) => println!("开机自启  : 已开启（{}）", v),
        None => println!("开机自启  : 未开启"),
    }
    if autostart::old_task_exists() {
        println!("提示      : 检测到旧版计划任务 HuaweiHRDaemon，建议用 scripts\\uninstall-task.ps1 清掉");
    }
    0
}

fn describe_status(s: &crate::shared_mem::Snapshot, fresh: bool) -> &'static str {
    use crate::shared_mem::*;
    match s.status {
        HRS_OK if fresh => "正常",
        HRS_OK => "数据超时",
        HRS_CONNECTING => "连接中",
        HRS_TIMEOUT => "已超时",
        _ => "无数据",
    }
}
