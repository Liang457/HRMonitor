// src/config.rs —— hr-daemon.ini 的读写。
//
// 键名/取值/注释必须和 C++ 侧（common/hr_config.cpp）保持一致：
// 那边是 daemon 的**读**端（宽容解析 + 越界兜底），这边是**写**端。
// 这里只管把配置写成人能看懂的样子，以及做友好的输入校验。
//
// 注意：OSD 的外观（颜色/字号/量程/是否显示）不在这个文件里，那归 MSI Afterburner
// 的监控设置管，见 ab-plugin/ 与 README。
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------- 配置结构

#[derive(Clone, Debug)]
pub struct Config {
    // [source]
    pub demo: bool,
    pub address: String,
    pub scan_timeout_ms: i32,
    pub backoff_min_sec: i32,
    pub backoff_max_sec: i32,

    // [display]
    pub timeout_ms: i32,
    pub refresh_ms: i32,

    // [log]
    pub log_max_kb: i32,
    pub log_debug: bool,

    // [integration]
    pub tm_dir: String,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            demo: false,
            address: String::new(),
            scan_timeout_ms: 20000,
            backoff_min_sec: 1,
            backoff_max_sec: 30,

            timeout_ms: 15000,
            refresh_ms: 1000,

            log_max_kb: 4096,
            log_debug: false,
            tm_dir: String::new(),
        }
    }
}

/// 一个可编辑项的元信息：键名 + 说明。（取值方式/控件类型由面板的 JS 表负责，
/// 那边维护着自己的同名清单。）
pub struct Field {
    pub key: &'static str,
    pub desc: &'static str,
    pub unit: &'static str,
}

/// 面板/CLI 里能改的所有项。顺序就是展示顺序。
pub const FIELDS: &[Field] = &[
    Field { key: "demo",         desc: "模拟心率源（不用手表）",       unit: "" },
    Field { key: "address",      desc: "直连的手表 MAC（留空=不连接）", unit: "" },
    Field { key: "scan_timeout", desc: "每轮扫描最长",                 unit: "毫秒" },
    Field { key: "backoff_min",  desc: "重连退避下限",                 unit: "秒" },
    Field { key: "backoff_max",  desc: "重连退避上限",                 unit: "秒" },
    Field { key: "timeout",      desc: "多久没数据就显示 --",          unit: "毫秒" },
    Field { key: "refresh",      desc: "刷新/采样周期",                unit: "毫秒" },
    Field { key: "log_kb",       desc: "日志文件上限",                 unit: "KB" },
    Field { key: "debug",        desc: "日志打印每条心率（排查用）",   unit: "" },
    Field { key: "tm_dir",       desc: "TrafficMonitor 安装目录",      unit: "" },
];

impl Config {
    /// 读一项（给人看的字符串：布尔显示 是/否）
    pub fn get(&self, key: &str) -> Option<String> {
        Some(match key {
            "demo" => yn(self.demo),
            "address" => self.address.clone(),
            "scan_timeout" => self.scan_timeout_ms.to_string(),
            "backoff_min" => self.backoff_min_sec.to_string(),
            "backoff_max" => self.backoff_max_sec.to_string(),
            "timeout" => self.timeout_ms.to_string(),
            "refresh" => self.refresh_ms.to_string(),
            "log_kb" => self.log_max_kb.to_string(),
            "debug" => yn(self.log_debug),
            "tm_dir" => self.tm_dir.clone(),
            _ => return None,
        })
    }

    /// 读一项（给表单/脚本的机器字符串：布尔是 1/0）
    pub fn get_raw(&self, key: &str) -> Option<String> {
        Some(match key {
            "demo" => b01(self.demo).to_string(),
            "debug" => b01(self.log_debug).to_string(),
            _ => return self.get(key),
        })
    }

    /// 改一项。值先做类型/范围校验，通过才写入；返回错误说明。
    pub fn set(&mut self, key: &str, raw: &str) -> Result<(), String> {
        let v = raw.trim().to_string();
        match key {
            "demo" => self.demo = parse_bool(&v)?,
            "address" => {
                if v.is_empty() {
                    self.address.clear();
                } else {
                    self.address = normalize_mac(&v)?;
                }
            }
            "scan_timeout" => self.scan_timeout_ms = range_i32(&v, "扫描超时", 2000, 600_000)?,
            "backoff_min" => self.backoff_min_sec = range_i32(&v, "退避下限", 1, 600)?,
            "backoff_max" => {
                let n = range_i32(&v, "退避上限", 1, 3600)?;
                if n < self.backoff_min_sec {
                    return Err(format!("退避上限不能小于下限（{} 秒）", self.backoff_min_sec));
                }
                self.backoff_max_sec = n;
            }
            "timeout" => self.timeout_ms = range_i32(&v, "数据超时", 2000, 600_000)?,
            "refresh" => self.refresh_ms = range_i32(&v, "刷新周期", 200, 60_000)?,
            "log_kb" => self.log_max_kb = range_i32(&v, "日志上限", 64, 1_048_576)?,
            "debug" => self.log_debug = parse_bool(&v)?,
            "tm_dir" => {
                // 文本项要挡住换行：写下去就是往 INI 里注入行甚至整个 [section]
                if v.contains('\r') || v.contains('\n') {
                    return Err("不能包含换行".into());
                }
                self.tm_dir = v;
            }
            _ => return Err(format!("未知配置项：{}", key)),
        }
        Ok(())
    }

    /// 生成整份带注释的 INI（与 common/hr_config.cpp 的 ToIniText 对齐）
    pub fn to_ini_text(&self) -> String {
        let mut s = String::new();
        let _ = write!(
            s,
            "; hr-daemon.ini —— hr-daemon / hr-manager 的配置\r\n\
             ; 这个文件由 hr-manager.exe 生成，也可以手改（UTF-8）。改完重启 hr-daemon 生效。\r\n\
             ; 以 ; 或 # 开头的行是注释。\r\n\
             ; OSD 的外观（颜色/字号/量程/是否显示）不在这个文件里配 ——\r\n\
             ; 那归 MSI Afterburner 的监控设置管，见 README。\r\n\
             \r\n\
             [source]\r\n\
             ; demo=1 使用模拟心率源（60~180 随机游走），不需要手表\r\n\
             demo={}\r\n\
             ; 留空 = 不连接（避免连错设备，手表地址必须明确指定）；填了 = 直连该地址\r\n\
             address={}\r\n\
             ; --scan 不带秒数参数时的默认扫描时长\r\n\
             scan_timeout_ms={}\r\n\
             ; 失败后重连退避，从 min 秒开始翻倍到 max 秒封顶\r\n\
             backoff_min_sec={}\r\n\
             backoff_max_sec={}\r\n\
             \r\n\
             [display]\r\n\
             ; 超过这么多毫秒没有新数据就显示 \"--\"\r\n\
             timeout_ms={}\r\n\
             ; 采样/推送周期（毫秒）\r\n\
             refresh_ms={}\r\n\
             \r\n\
             [log]\r\n\
             ; 单个日志文件超过这么多 KB 就轮转（磁盘上最多留 3 份，见 README）\r\n\
             max_kb={}\r\n\
             ; debug=1 连每条心率都写进日志（排查用；平时别开，心率几乎每秒都变，日志会涨很快）\r\n\
             debug={}\r\n\
             \r\n\
             [integration]\r\n\
             ; TrafficMonitor 安装目录（只给 hr-manager 的部署按钮/脚本用）\r\n\
             tm_dir={}\r\n",
            b01(self.demo),
            self.address,
            self.scan_timeout_ms,
            self.backoff_min_sec,
            self.backoff_max_sec,
            self.timeout_ms,
            self.refresh_ms,
            self.log_max_kb,
            b01(self.log_debug),
            self.tm_dir
        );
        s
    }
}

// ---------------------------------------------------------------- ini 解析

/// 宽容解析：只取 [section] key=value，其它行忽略；BOM 自动跳过。
/// 解析不了的值保持默认，绝不报错（daemon 侧同样宽容）。
/// 旧版配置里遗留的 [osd] 段会被自然忽略。
pub fn parse_ini(text: &str) -> BTreeMap<String, String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut map = BTreeMap::new();
    let mut sec = String::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
            continue;
        }
        if let Some(rest) = line.strip_prefix('[') {
            if let Some(name) = rest.strip_suffix(']') {
                sec = name.trim().to_string();
            }
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            map.insert(format!("{}.{}", sec, k.trim()), v.trim().to_string());
        }
    }
    map
}

fn geti(m: &BTreeMap<String, String>, k: &str, def: i32) -> i32 {
    m.get(k).and_then(|v| v.parse::<i32>().ok()).unwrap_or(def)
}

fn gets(m: &BTreeMap<String, String>, k: &str, def: &str) -> String {
    m.get(k).cloned().unwrap_or_else(|| def.to_string())
}

fn getb(m: &BTreeMap<String, String>, k: &str, def: bool) -> bool {
    match m.get(k).map(|s| s.as_str()) {
        Some("1") | Some("true") | Some("True") | Some("TRUE") | Some("yes") | Some("on") => true,
        Some("0") | Some("false") | Some("False") | Some("FALSE") | Some("no") | Some("off") => false,
        _ => def,
    }
}

impl Config {
    pub fn from_ini_text(text: &str) -> Config {
        let d = Config::default();
        let m = parse_ini(text);
        Config {
            demo: getb(&m, "source.demo", d.demo),
            address: gets(&m, "source.address", &d.address),
            scan_timeout_ms: geti(&m, "source.scan_timeout_ms", d.scan_timeout_ms),
            backoff_min_sec: geti(&m, "source.backoff_min_sec", d.backoff_min_sec),
            backoff_max_sec: geti(&m, "source.backoff_max_sec", d.backoff_max_sec),

            timeout_ms: geti(&m, "display.timeout_ms", d.timeout_ms),
            refresh_ms: geti(&m, "display.refresh_ms", d.refresh_ms),

            log_max_kb: geti(&m, "log.max_kb", d.log_max_kb),
            log_debug: getb(&m, "log.debug", d.log_debug),
            tm_dir: gets(&m, "integration.tm_dir", &d.tm_dir),
        }
    }
}

// ---------------------------------------------------------------- 取值校验

fn yn(b: bool) -> String {
    if b { "是".into() } else { "否".into() }
}

fn b01(b: bool) -> i32 {
    if b { 1 } else { 0 }
}

fn parse_bool(s: &str) -> Result<bool, String> {
    match s {
        "1" | "y" | "Y" | "yes" | "true" | "on" | "是" => Ok(true),
        "0" | "n" | "N" | "no" | "false" | "off" | "否" => Ok(false),
        _ => Err(format!("要填 1/0（或 y/n），不能是「{}」", s)),
    }
}

fn parse_i32(s: &str, what: &str) -> Result<i32, String> {
    s.parse::<i32>().map_err(|_| format!("{} 得是整数，不能是「{}」", what, s))
}

fn range_i32(s: &str, what: &str, lo: i32, hi: i32) -> Result<i32, String> {
    let n = parse_i32(s, what)?;
    if n < lo || n > hi {
        return Err(format!("{} 应在 {}~{} 之间（填的是 {}）", what, lo, hi, n));
    }
    Ok(n)
}

/// "AA:BB:CC:DD:EE:FF" / "AA-BB-..." → 规范化成冒号分隔
pub fn normalize_mac(s: &str) -> Result<String, String> {
    let hex: String = s.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    if hex.len() != 12 {
        return Err(format!(
            "MAC 地址要 12 位十六进制（形如 AA:BB:CC:DD:EE:FF），填的是「{}」",
            s
        ));
    }
    let up = hex.to_uppercase();
    let mut out = String::new();
    for i in 0..6 {
        if i > 0 {
            out.push(':');
        }
        out.push_str(&up[i * 2..i * 2 + 2]);
    }
    Ok(out)
}

// ---------------------------------------------------------------- 文件读写

/// 读 hr-daemon.ini。
///
/// `Ok(None)` 只表示"文件不存在"。读不了、解不出来一律 Err —— 以前所有失败都
/// 塌缩成 None，上层当成"用默认值"，于是文件暂时读不到（被编辑器锁着、被同步
/// 客户端或杀软占着、存成了 UTF-16）时面板显示默认值，用户随便改一项保存，
/// 就用默认值把真实配置整个覆盖掉了。
pub fn read_ini(path: &Path) -> Result<Option<String>, String> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("读不了 {}：{}", path.display(), e)),
    };

    // 和 C++ 侧（common/hr_config.cpp）一样兼容 UTF-16：记事本"另存为 Unicode"
    // 就是这么存的，而写端一直是 UTF-8。
    if bytes.len() >= 2 && bytes[0] == 0xFF && bytes[1] == 0xFE {
        return decode_utf16(&bytes[2..], true).map(Some);
    }
    if bytes.len() >= 2 && bytes[0] == 0xFE && bytes[1] == 0xFF {
        return decode_utf16(&bytes[2..], false).map(Some);
    }

    String::from_utf8(bytes).map(Some).map_err(|_| {
        format!(
            "{} 既不是 UTF-8 也不是 UTF-16 —— 请存成 UTF-8，或删掉它让 hr-manager 重建",
            path.display()
        )
    })
}

fn decode_utf16(bytes: &[u8], little_endian: bool) -> Result<String, String> {
    if bytes.len() & 1 != 0 {
        return Err("UTF-16 文件长度是奇数，多半已经损坏".into());
    }
    let mut units = Vec::with_capacity(bytes.len() / 2);
    let mut i = 0;
    while i < bytes.len() {
        let pair = [bytes[i], bytes[i + 1]];   // 长度是偶数，所以 i+1 一定在界内
        units.push(if little_endian {
            u16::from_le_bytes(pair)
        } else {
            u16::from_be_bytes(pair)
        });
        i += 2;
    }
    String::from_utf16(&units).map_err(|_| "UTF-16 文件里有非法的代理对".to_string())
}

fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_os_string();
    s.push(suffix);
    PathBuf::from(s)
}

/// 写成 UTF-8 with BOM（记事本 / VS Code 都能正确认出中文注释）。
///
/// 先备份成 `.bak`，写 `.tmp`，最后原子替换。直接截断重写的话，写到一半
/// 崩溃/断电/磁盘满就会留下一份不完整的 INI，而读端是宽容解析 —— 丢掉的键
/// 会静默变成默认值，非常难查。
pub fn write_ini(path: &Path, body: &str) -> Result<(), String> {
    let mut bytes = Vec::with_capacity(body.len() + 3);
    bytes.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
    bytes.extend_from_slice(body.as_bytes());

    let tmp = sibling(path, ".tmp");
    std::fs::write(&tmp, &bytes).map_err(|e| format!("写不了 {}：{}", tmp.display(), e))?;

    if path.exists() {
        let bak = sibling(path, ".bak");
        if let Err(e) = std::fs::copy(path, &bak) {
            let _ = std::fs::remove_file(&tmp);
            return Err(format!("备份 {} 失败：{}", bak.display(), e));
        }
    }

    if let Err(e) = replace_file(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("替换 {} 失败：{}", path.display(), e));
    }
    Ok(())
}

/// MoveFileExW + MOVEFILE_REPLACE_EXISTING：std 的 rename 在目标已存在时是失败的。
fn replace_file(from: &Path, to: &Path) -> std::io::Result<()> {
    let from_w = win::wide(&from.to_string_lossy());
    let to_w = win::wide(&to.to_string_lossy());
    let ok = unsafe {
        win::MoveFileExW(from_w.as_ptr(), to_w.as_ptr(), win::MOVEFILE_REPLACE_EXISTING)
    };
    if ok == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

/// hr-manager.exe 所在目录。结果缓存：每次读写 ini 都要用，别反复分配。
pub fn exe_dir() -> PathBuf {
    static CACHE: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    CACHE
        .get_or_init(|| exe_dir_uncached().unwrap_or_else(|| PathBuf::from(".")))
        .clone()
}

fn exe_dir_uncached() -> Option<PathBuf> {
    // 先试 512 个 wchar；返回值等于容量说明被截断了（Windows 的行为），
    // 那时再按需翻倍 —— 不能把截断后的路径当成功。
    let mut cap = 512usize;
    loop {
        let mut buf = vec![0u16; cap];
        let n = unsafe { win::GetModuleFileNameW(std::ptr::null_mut(), buf.as_mut_ptr(), cap as u32) };
        if n == 0 {
            return None;
        }
        if (n as usize) < cap {
            let s = String::from_utf16(&buf[..n as usize]).ok()?;
            return Path::new(&s).parent().map(|p| p.to_path_buf());
        }
        if cap >= 32768 {
            return None;
        }
        cap *= 2;
    }
}

pub fn ini_path() -> PathBuf {
    exe_dir().join("hr-daemon.ini")
}

// ---------------------------------------------------------------- 载入/保存（CLI 与 GUI 共用）

/// 读配置 + 路径。`Err` 表示"文件在、但读不了或解不出来"，这跟"文件不存在"
/// 是两回事：读不出来时绝不能退回默认值 —— 用户下一次保存就会用默认值把
/// 真实配置覆盖掉。
pub fn load() -> Result<(Config, PathBuf, bool), String> {
    let path = ini_path();
    match read_ini(&path)? {
        Some(text) => Ok((Config::from_ini_text(&text), path, true)),
        None => Ok((Config::default(), path, false)),
    }
}

/// 保存（备份 + 原子替换）。
pub fn save(cfg: &Config, path: &Path) -> Result<(), String> {
    write_ini(path, &cfg.to_ini_text())
}

use crate::win;
