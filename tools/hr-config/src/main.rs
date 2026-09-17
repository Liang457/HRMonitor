// hr-config —— 华为手表心率 daemon 的配置工具
//
// 职责就三件（刻意保持很小）：
//   1. 读写 hr-daemon.ini
//   2. 可选地重启 hr-daemon，让改动生效
//   3. 改地址时扫一遍附近的心率广播设备，列出来让人挑
// 扫描是借 hr-daemon.exe --scan 做的，自己不碰蓝牙；它也不读共享内存、
// 不动 TrafficMonitor、不碰 OSD —— 那些分别属于 daemon、部署脚本和 MSI Afterburner。
mod config;
mod proc;
mod scan;
mod win;

use config::{Config, FIELDS, Kind};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

const DAEMON_EXE: &str = "hr-daemon.exe";
// daemon 启动时注册的隐藏顶层窗口类名，用来判断它在不在跑 / 让它优雅退出
const DAEMON_WND_CLASS: &str = "HuaWeiHRDaemonWnd";
// daemon 的单实例互斥体（必须和 daemon/main.cpp 里的 kMutexName 一致）
const DAEMON_MUTEX: &str = "Local\\HuaWeiHR_daemon";
// 等 daemon 退出 / 等它起来的上限（它可能正卡在一次 BLE 连接尝试里，收尾要十几秒）
const DAEMON_STOP_TIMEOUT_MS: u32 = 20_000;
const DAEMON_START_TIMEOUT_MS: u32 = 8_000;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = run(&args);
    std::process::exit(code);
}

fn run(args: &[String]) -> i32 {
    if args.is_empty() {
        return menu();
    }
    match args[0].as_str() {
        "show" | "list" => cmd_show(),
        "get" => {
            if args.len() < 2 {
                eprintln!("用法: hr-config get <配置项>");
                return 2;
            }
            cmd_get(&args[1])
        }
        "set" => {
            if args.len() < 3 {
                eprintln!("用法: hr-config set <配置项> <值>");
                return 2;
            }
            cmd_set(&args[1], &args[2..].join(" "))
        }
        "reset" => cmd_reset(),
        "restart" => cmd_restart(),
        "path" => cmd_path(),
        "help" | "--help" | "-h" => {
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
        r#"hr-config —— 心率 daemon 的配置工具

用法:
  hr-config                      进入交互式菜单（不带参数时）
  hr-config show                 显示当前配置
  hr-config get <配置项>         读一项
  hr-config set <配置项> <值>    改一项
  hr-config reset                恢复默认配置（会覆盖现有 ini）
  hr-config restart              重启 hr-daemon，让改动生效
  hr-config path                 显示配置文件位置
  hr-config help                 显示本帮助

配置项（hr-config show 也会列出来）:
  心率来源   demo  address  scan_timeout  backoff_min  backoff_max
  显示       timeout  refresh
  日志       log_kb  debug
  集成       tm_dir

例:
  hr-config set demo 1                    # 换成模拟心率源
  hr-config set address AA:BB:CC:DD:EE:FF # 直连指定手表
  hr-config set timeout 30000             # 30 秒没数据才显示 --
  hr-config set debug 1                   # 排查用：连每条心率都写进日志
  hr-config restart                       # 改完重启 daemon

保存前会先备份一份 <ini>.bak，再原子替换，写到一半不会留下半份配置。

说明: 只改配置文件，不直接操作 daemon（除 restart 外）。
      OSD 的外观（颜色/字号/量程）在 MSI Afterburner 里配，见 README。"#
    );
}

// ---------------------------------------------------------------- 加载/保存

/// 读配置。`Err` 表示"文件在、但读不了或解不出来"，这跟"文件不存在"是两回事：
/// 读不出来时绝不能退回默认值 —— 用户下一次保存就会用默认值把真实配置覆盖掉。
fn load() -> Result<(Config, PathBuf, bool), String> {
    let path = config::ini_path();
    match config::read_ini(&path)? {
        Some(text) => Ok((Config::from_ini_text(&text), path, true)),
        None => Ok((Config::default(), path, false)),
    }
}

/// load() + 出错就打印原因并返回退出码
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

/// 保存；失败时打印原因
fn save(cfg: &Config, path: &Path) -> bool {
    match config::write_ini(path, &cfg.to_ini_text()) {
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
        "address" => {
            if v.is_empty() {
                "(空，不连接)".into()
            } else {
                v
            }
        }
        "demo" => v,
        _ => {
            if v.is_empty() {
                "(空)".into()
            } else {
                v
            }
        }
    }
}

/// 终端显示宽度：CJK / 全角算 2 列，其余算 1 列。
/// （Rust 的 {:<n} 按 char 数补空格，中文就会参差不齐。）
fn display_width(s: &str) -> usize {
    s.chars()
        .map(|c| {
            let u = c as u32;
            let wide = (0x1100..=0x115F).contains(&u)      // 韩文字母
                || (0x2E80..=0x303E).contains(&u)          // 部首扩展
                || (0x3041..=0x33FF).contains(&u)          // 假名 / 注音 / CJK 符号
                || (0x3400..=0x4DBF).contains(&u)          // CJK 扩展 A
                || (0x4E00..=0x9FFF).contains(&u)          // CJK 基本区
                || (0xA000..=0xA4CF).contains(&u)          // 彝文
                || (0xAC00..=0xD7A3).contains(&u)          // 韩文音节
                || (0xF900..=0xFAFF).contains(&u)          // CJK 兼容
                || (0xFE30..=0xFE6F).contains(&u)          // CJK 兼容形式
                || (0xFF00..=0xFF60).contains(&u)          // 全角
                || (0xFFE0..=0xFFE6).contains(&u)
                || (0x20000..=0x3FFFD).contains(&u);       // CJK 扩展 B+
            if wide { 2 } else { 1 }
        })
        .sum()
}

/// 左对齐补到指定显示宽度
fn pad(s: &str, width: usize) -> String {
    let w = display_width(s);
    if w >= width {
        s.to_string()
    } else {
        format!("{}{}", s, " ".repeat(width - w))
    }
}

// ---------------------------------------------------------------- 子命令

fn cmd_show() -> i32 {
    let (cfg, path, exists) = load_or_exit!();
    println!("配置文件: {}{}", path.display(), if exists { "" } else { "（不存在，下面是默认值）" });
    println!();
    for f in FIELDS {
        println!("  {:<13} {} = {}", f.key, desc_of(f.key), show_value(&cfg, f.key));
    }
    0
}

fn cmd_get(key: &str) -> i32 {
    let (cfg, _, _) = load_or_exit!();
    match cfg.get(key) {
        Some(v) => {
            println!("{}", v);
            0
        }
        None => {
            eprintln!("未知配置项：{}（用 hr-config show 看有哪些）", key);
            2
        }
    }
}

fn cmd_set(key: &str, value: &str) -> i32 {
    let (mut cfg, path, _) = load_or_exit!();
    if let Err(e) = cfg.set(key, value) {
        eprintln!("{}", e);
        return 2;
    }
    if !save(&cfg, &path) {
        return 1;
    }
    println!("{} = {}  已保存", key, show_value(&cfg, key));
    if offer_restart(&cfg, &path) {
        0
    } else {
        1 // 重启失败，脚本得能看出来
    }
}

fn cmd_reset() -> i32 {
    let (_, path, exists) = load_or_exit!();
    if exists {
        print!("这会用默认值覆盖 {}，继续？[y/N] ", path.display());
        let _ = io::stdout().flush();
        if !ask_yes() {
            println!("已取消");
            return 0;
        }
    }
    let cfg = Config::default();
    if !save(&cfg, &path) {
        return 1;
    }
    println!("已写入默认配置：{}", path.display());
    if offer_restart(&cfg, &path) {
        0
    } else {
        1
    }
}

fn cmd_path() -> i32 {
    println!("配置文件 : {}", config::ini_path().display());
    println!("程序目录 : {}", config::exe_dir().display());
    println!("daemon   : {}", config::exe_dir().join(DAEMON_EXE).display());
    0
}

fn cmd_restart() -> i32 {
    match restart_daemon() {
        Ok(msg) => {
            println!("{}", msg);
            0
        }
        Err(e) => {
            eprintln!("{}", e);
            1
        }
    }
}

// ---------------------------------------------------------------- 交互式菜单

fn menu() -> i32 {
    let (mut cfg, path, exists) = load_or_exit!();
    let mut dirty = false;

    println!("=== 心率 daemon 配置 ===");
    println!("配置文件: {}{}", path.display(), if exists { "" } else { "（不存在，下面是默认值）" });
    println!("直接回车 = 保持原值；输入 q 放弃退出。");
    println!();

    loop {
        println!("--- 当前配置 ---");
        for (i, f) in FIELDS.iter().enumerate() {
            println!(
                " {:>2}) {} {} = {}",
                i + 1,
                pad(f.key, 13),
                pad(f.desc, 30),
                show_value(&cfg, f.key)
            );
        }
        println!();
        println!("   s) 保存并退出     r) 保存 + 重启 daemon     q) 放弃退出");
        print!("选择> ");
        let _ = io::stdout().flush();

        let choice = match read_line() {
            Input::Line(c) => c,
            Input::Eof => break,
        };
        match choice.as_str() {
            "q" | "Q" | "quit" | "exit" => {
                if dirty {
                    print!("有未保存的改动，确定放弃？[y/N] ");
                    let _ = io::stdout().flush();
                    if !ask_yes() {
                        continue;
                    }
                }
                println!("已退出（未改动文件）");
                return 0;
            }
            "s" | "S" => {
                if dirty && !save(&cfg, &path) {
                    continue;
                }
                println!("已保存：{}", path.display());
                offer_restart(&cfg, &path);
                return 0;
            }
            "r" | "R" => {
                if dirty && !save(&cfg, &path) {
                    continue;
                }
                println!("已保存：{}", path.display());
                return match restart_daemon() {
                    Ok(m) => {
                        println!("{}", m);
                        0
                    }
                    Err(e) => {
                        eprintln!("{}", e);
                        1
                    }
                };
            }
            _ => {
                let idx: usize = match choice.parse::<usize>() {
                    Ok(n) if n >= 1 && n <= FIELDS.len() => n - 1,
                    _ => {
                        println!("看不懂「{}」，请输入 1~{} 或 s/r/q。", choice, FIELDS.len());
                        continue;
                    }
                };
                let f = &FIELDS[idx];
                let cur = cfg.get(f.key).unwrap_or_default();

                // address 特殊一点：先扫一遍心率广播设备，列出来让人挑，不用盲填 MAC
                if f.key == "address" {
                    match edit_address(&cur) {
                        AddrEdit::Keep => println!("  保持原值"),
                        AddrEdit::Clear => match cfg.set("address", "") {
                            Ok(()) => {
                                dirty = true;
                                println!("  已清空（daemon 将不连接）");
                            }
                            Err(e) => println!("  {}（原值未变）", e),
                        },
                        AddrEdit::Set(mac) => match cfg.set("address", &mac) {
                            Ok(()) => {
                                dirty = true;
                                println!("  已改为：{}", show_value(&cfg, "address"));
                                if cfg.demo {
                                    println!(
                                        "  提示：第 1 项 demo 现在是「是」，daemon 仍会用模拟源；要接手表请把它改成 0。"
                                    );
                                }
                            }
                            Err(e) => println!("  {}（原值未变）", e),
                        },
                    }
                    continue;
                }

                let hint = match f.kind {
                    Kind::Bool => "填 1/0",
                    Kind::Text => "输入 - 表示清空",
                    _ => "填数字",
                };
                println!(
                    "  {} （{}）",
                    desc_of(f.key),
                    f.key
                );
                print!("  当前 [{}]，回车保持，{} > ", cur, hint);
                let _ = io::stdout().flush();

                let input = match read_line() {
                    Input::Line(v) => v,
                    Input::Eof => break,
                };
                if input.is_empty() {
                    println!("  保持原值");
                    continue;
                }
                let value = if input == "-" {
                    if f.kind == Kind::Text {
                        String::new()
                    } else {
                        println!("  「-」只能用来清空文本项");
                        continue;
                    }
                } else {
                    input
                };
                match cfg.set(f.key, &value) {
                    Ok(()) => {
                        dirty = true;
                        println!("  已改为：{}", show_value(&cfg, f.key));
                    }
                    Err(e) => println!("  {}（原值未变）", e),
                }
            }
        }
    }
    // 输入流断了（比如管道），静默退出
    0
}

// ---------------------------------------------------------------- 选手表（address）

enum AddrEdit {
    Keep,
    Clear,
    Set(String),
}

/// 交互式改 address：先扫一遍附近的心率广播设备，列出编号让人选。
///   序号 = 选这台；0 = 重新扫描；回车 = 保持原值；- = 清空；其它 = 当手工 MAC 校验。
/// daemon 正跑着时会先问一句要不要停掉它（表被连着时通常就不再广播了），
/// 不管中途怎么退出，返回前都会把它拉回来。
fn edit_address(cur: &str) -> AddrEdit {
    println!("  {} （address）", desc_of("address"));

    let mut paused = false;
    if daemon_running() {
        print!("  hr-daemon 正在运行（表被连着时通常不再广播）。先停掉它再扫描？[Y/n] ");
        let _ = io::stdout().flush();
        if ask_yes_default_true() {
            match stop_daemon() {
                Ok(()) => {
                    // 断开之后手表要过一下才重新开始广播
                    print!("  已停掉 hr-daemon，等 2 秒让手表恢复广播...");
                    let _ = io::stdout().flush();
                    unsafe { win::Sleep(2000) };
                    println!("好");
                    paused = true;
                }
                Err(e) => println!("  停不掉（{}），照常扫描。", e),
            }
        }
    }

    let decision = pick_device(cur);

    if paused {
        let exe = config::exe_dir().join(DAEMON_EXE);
        // 拉起用命令行参数（参数优先于 ini）：刚选了地址就直接直连那一台，
        // 否则 daemon 会按旧 ini 扫描乱连，多设备环境可能连到别的表。
        // --quiet 让 daemon 别附加本窗口的控制台：日志不刷菜单，
        // 这里按 Ctrl+C / 关窗口也不会把 daemon 连带杀掉（日志看 hr-daemon.log）。
        let args: Vec<&str> = match &decision {
            AddrEdit::Set(mac) => vec!["--quiet", "--address", mac.as_str()],
            _ => vec!["--quiet"],
        };
        match proc::spawn_detached(&exe, &args) {
            Ok(()) if wait_daemon_up(DAEMON_START_TIMEOUT_MS) => match &decision {
                AddrEdit::Set(mac) => println!(
                    "  已把 hr-daemon 用刚选的地址 {} 临时拉起（保存后此地址才写入 ini）；日志见 hr-daemon.log",
                    mac
                ),
                _ => println!(
                    "  已把 hr-daemon 重新拉起来（改动保存后还会再问一次重启）；日志见 hr-daemon.log"
                ),
            },
            Ok(()) => eprintln!(
                "  hr-daemon 没能起来，请手动跑一次 {}（或 hr-config restart）。",
                exe.display()
            ),
            Err(e) => eprintln!("  {}（hr-daemon 现在没在跑，请手动起来）", e),
        }
    }
    decision
}

/// 扫描 → 列表 → 选择，直到拿到一个决定。
fn pick_device(cur: &str) -> AddrEdit {
    let mut devices = match scan::scan(scan::SCAN_SECS) {
        Ok(d) => d,
        Err(e) => {
            // 扫不了就别把人卡住，退回手填
            println!("  扫描失败：{}", e);
            return ask_mac_by_hand(cur);
        }
    };
    let mut show_list = true; // 只在刚扫完时打一遍列表，输错了不重复刷屏

    loop {
        if show_list {
            print_devices(&devices, cur);
            show_list = false;
        }
        print!("  选择 > ");
        let _ = io::stdout().flush();

        let input = match read_line() {
            Input::Line(v) => v,
            Input::Eof => return AddrEdit::Keep, // 输入断了（管道），当保持原值
        };

        if input.is_empty() {
            return AddrEdit::Keep;
        }
        if input == "-" {
            return AddrEdit::Clear;
        }
        if input == "0" {
            match scan::scan(scan::SCAN_SECS) {
                Ok(d) => {
                    devices = d;
                    show_list = true;
                }
                Err(e) => {
                    println!("  扫描失败：{}", e);
                    return ask_mac_by_hand(cur);
                }
            }
            continue;
        }
        if let Ok(n) = input.parse::<usize>() {
            if n >= 1 && n <= devices.len() {
                return AddrEdit::Set(devices[n - 1].mac.clone());
            }
            if devices.is_empty() {
                println!("  现在列表是空的，输入 0 重新扫描。");
            } else {
                println!("  列表里没有第 {} 台（只有 1~{}）。", n, devices.len());
            }
            continue;
        }
        // 不是序号，就当手工 MAC 试一次（列表留着，不重扫）
        match config::normalize_mac(&input) {
            Ok(mac) => return AddrEdit::Set(mac),
            Err(e) => println!("  {}", e),
        }
    }
}

/// 打印这次扫描的结果 + 选项 + 提示行。
fn print_devices(devices: &[scan::Device], cur: &str) {
    if devices.is_empty() {
        println!("  没扫到心率广播设备。检查一下：");
        println!("    - 手表停在“心率广播”页面并保持亮屏（离开就停播）");
        println!("    - 蓝牙适配器开着、手表在附近");
        println!("    - 表正被 hr-daemon 连着时通常不再广播（下次可以答 y 让它先停一下）");
    } else {
        println!("  扫到 {} 台：", devices.len());
        for (i, d) in devices.iter().enumerate() {
            let name = if d.name.is_empty() { "（无名字）" } else { d.name.as_str() };
            let mark = if d.mac.eq_ignore_ascii_case(cur) { "   ← 当前" } else { "" };
            println!("   {:>2}) {:<17} {}{}", i + 1, d.mac, name, mark);
        }
    }
    println!("     0) 重新扫描");
    println!("  当前 [{}]，回车保持，- 清空，也可以直接输入 MAC", cur);
}

/// 扫描这条路走不通时的兜底：回到原来的纯文本提示，手填 MAC。
fn ask_mac_by_hand(cur: &str) -> AddrEdit {
    loop {
        print!("  当前 [{}]，回车保持，输入 - 表示清空 > ", cur);
        let _ = io::stdout().flush();

        let input = match read_line() {
            Input::Line(v) => v,
            Input::Eof => return AddrEdit::Keep,
        };
        if input.is_empty() {
            return AddrEdit::Keep;
        }
        if input == "-" {
            return AddrEdit::Clear;
        }
        match config::normalize_mac(&input) {
            Ok(mac) => return AddrEdit::Set(mac),
            Err(e) => println!("  {}", e),
        }
    }
}

// ---------------------------------------------------------------- daemon 重启

fn daemon_hwnd() -> win::Hwnd {
    let cls = win::wide(DAEMON_WND_CLASS);
    unsafe { win::FindWindowW(cls.as_ptr(), std::ptr::null()) }
}

/// daemon 的进程还在吗？它整个生命周期都握着 Local\HuaWeiHR_daemon 这个互斥体
/// （见 daemon/main.cpp 的单实例逻辑），所以能打开就说明还在。
///
/// 这比 "窗口还在吗" 准：窗口是 DestroyWindow 先没的，之后 BLE 线程还要收尾
/// （卡在一次连接尝试里的话得十几秒），这期间进程照样占着互斥体。
fn daemon_alive() -> bool {
    let name = win::wide(DAEMON_MUTEX);
    let h = unsafe { win::OpenMutexW(win::SYNCHRONIZE, 0, name.as_ptr()) };
    if h.is_null() {
        return false; // 没这个互斥体（或打不开）→ 就当它不在跑
    }
    unsafe { win::CloseHandle(h) };
    true
}

/// daemon 在不在跑。窗口没了但进程还在（正在收尾）也算在跑。
fn daemon_running() -> bool {
    !daemon_hwnd().is_null() || daemon_alive()
}

/// 等它彻底退出：窗口和互斥体都没了才算走干净。
fn wait_daemon_gone(timeout_ms: u32) -> bool {
    let mut waited = 0u32;
    let mut said = false;
    while waited < timeout_ms && daemon_running() {
        if !said && waited >= 500 {
            print!("  等 hr-daemon 收尾（正连着表时可能要十几秒）...");
            let _ = io::stdout().flush();
            said = true;
        }
        unsafe { win::Sleep(200) };
        waited += 200;
    }
    if said {
        println!("好了");
    }
    !daemon_running()
}

/// 等新起的 daemon 注册上互斥体（= 真的跑起来了，没被单实例挡回去）。
fn wait_daemon_up(timeout_ms: u32) -> bool {
    let mut waited = 0u32;
    while waited < timeout_ms {
        if daemon_alive() {
            return true;
        }
        unsafe { win::Sleep(200) };
        waited += 200;
    }
    false
}

/// 让正在跑的 daemon 走正常退出路径（发 WM_CLOSE，它会关掉共享内存映射），
/// 然后等它**真的退出**。没在跑就什么都不做。
///
/// 必须等进程退出而不能只等窗口消失：新起的那个会被单实例互斥体挡回来，
/// 只等窗口的话就会落得"一个都没在跑"。
fn stop_daemon() -> Result<(), String> {
    let hwnd = daemon_hwnd();
    if hwnd.is_null() && !daemon_alive() {
        return Ok(());
    }
    if !hwnd.is_null() {
        unsafe { win::PostMessageW(hwnd, win::WM_CLOSE, 0, 0) };
    }
    if wait_daemon_gone(DAEMON_STOP_TIMEOUT_MS) {
        Ok(())
    } else {
        Err(format!(
            "旧的 hr-daemon 没在 {} 秒内退出，请手动结束它再试",
            DAEMON_STOP_TIMEOUT_MS / 1000
        ))
    }
}

/// 先让正在跑的 daemon 退出，再启动一份新的。daemon 没在跑就直接启动。
fn restart_daemon() -> Result<String, String> {
    let exe = config::exe_dir().join(DAEMON_EXE);
    if !exe.exists() {
        return Err(format!("找不到 {}，先构建：cmd /c daemon\\build.cmd", exe.display()));
    }

    let was_running = daemon_running();
    stop_daemon()?;
    // --quiet：restart 出来的 daemon 是常驻后台，不附加 hr-config 的控制台——
    // 日志只进文件，之后在配置窗口按 Ctrl+C / 关窗口也不会把它连带杀掉。
    proc::spawn_detached(&exe, &["--quiet"])?;
    if !wait_daemon_up(DAEMON_START_TIMEOUT_MS) {
        return Err(format!(
            "{} 起来了但没注册上（多半是还有个旧的没退干净，或者启动失败）——看看同目录的 hr-daemon.log",
            exe.display()
        ));
    }

    Ok(if was_running {
        format!("已重启 hr-daemon：{}", exe.display())
    } else {
        format!("已启动 hr-daemon：{}", exe.display())
    })
}

/// 配置改完之后问一句要不要重启。
/// 返回 false 只在"确实去重启但失败了"时 —— 用户回答"不重启"算正常结束，
/// 好让脚本能靠退出码区分这两件事。
fn offer_restart(cfg: &Config, path: &Path) -> bool {
    let _ = cfg;
    let _ = path;
    if !daemon_running() {
        println!("（hr-daemon 当前没在运行）");
        return true;
    }
    print!("要让改动生效需要重启 hr-daemon，现在重启？[Y/n] ");
    let _ = io::stdout().flush();
    if !ask_yes_default_true() {
        println!("改动手动重启 hr-daemon 后生效");
        return true;
    }
    match restart_daemon() {
        Ok(m) => {
            println!("{}", m);
            true
        }
        Err(e) => {
            eprintln!("{}", e);
            false
        }
    }
}

// ---------------------------------------------------------------- 输入

/// 单独分出 Eof：它和"按了回车"必须能区分开。
/// 以前 read_line 返回 Option，EOF 被 ask_yes_default_true 当成"是"，
/// 于是 `hr-config set demo 1 < NUL` 这种非交互调用会一声不响地
/// 停掉再拉起 daemon；而破坏性更强的 reset 反而更严格（EOF = 否），正好反了。
enum Input {
    Line(String),
    Eof,
}

fn read_line() -> Input {
    let mut s = String::new();
    match io::stdin().read_line(&mut s) {
        Ok(0) => Input::Eof,
        Ok(_) => Input::Line(s.trim().to_string()),
        Err(_) => Input::Eof,
    }
}

fn ask_yes() -> bool {
    match read_line() {
        Input::Line(s) => matches!(s.as_str(), "y" | "Y" | "yes" | "是"),
        Input::Eof => false,
    }
}

/// 默认答案是"是"的提问（回车 = 是）。EOF 一律当"不做"。
fn ask_yes_default_true() -> bool {
    match read_line() {
        Input::Line(s) => s.is_empty() || matches!(s.as_str(), "y" | "Y" | "yes"),
        Input::Eof => false,
    }
}
