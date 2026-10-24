// src/integrations.rs —— 第三方接入插件的元数据（exe 旁 plugins\*.json）。
//
// 每个插件 DLL 旁边放一个同名 .json：宿主名、DLL 名、安装步骤、宿主目录设置
// 都在 json 里，面板"第三方程序接入"区完全由这些数据驱动，不再硬编码
// TrafficMonitor / MSI Afterburner。json 随仓库走（ab-plugin\ / tm-plugin\
// 源目录），插件 build.cmd 复制进 build\plugins\，pack.ps1 从源目录取——
// 不经 build\，免得本机填过的目录值混进发行包。
//
// json 是给人改的数据文件，解析必须宽容：单个文件坏了只坏它自己的卡片，
// 绝不拖垮整个接入区；未知字段原样保留（写回时不能丢）。

use crate::config;
use serde_json::Value;
use std::path::{Path, PathBuf};

/// 一个插件的接入信息（plugins\ 下一个 json 对应一条）。
#[derive(Clone, Debug)]
pub struct Integration {
    pub file: String, // json 文件名，面板回传用它定位，如 "trafficmonitor_hr_plugin.json"
    pub name: String, // 宿主显示名，如 "TrafficMonitor"
    pub dll: String,  // 插件 DLL 文件名
    pub host_exe: String, // 自动探测宿主安装目录时认的 exe
    pub plugin_subdir: String, // 宿主目录下的插件子目录
    pub steps: Vec<String>,
    pub note: String,
    pub dir: String, // 宿主安装目录；空 = 按常见安装位置自动探测
}

/// list() 的一条结果：解析失败的 json 也占一条（error 非空），
/// 面板显示"解析失败"而不是整块消失。
pub struct ListItem {
    pub file: String,
    pub integration: Option<Integration>,
    pub dll_present: bool,
    pub error: Option<String>,
}

fn plugins_dir() -> PathBuf {
    config::exe_dir().join("plugins")
}

/// 列出 plugins\ 下全部 *.json。目录不存在返回空列表（新部署、dev 布局
/// 还没跑过插件构建都属正常），按文件名排序保证面板展示顺序稳定。
pub fn list() -> Vec<ListItem> {
    let dir = plugins_dir();
    let rd = match std::fs::read_dir(&dir) {
        Ok(rd) => rd,
        Err(_) => return Vec::new(),
    };
    let mut names: Vec<String> = rd
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().map_or(false, |x| x.eq_ignore_ascii_case("json")))
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    names.sort();
    names
        .into_iter()
        .map(|name| match load_one(&name) {
            Ok(it) => {
                let dll_present = plugins_dir().join(&it.dll).is_file();
                ListItem {
                    file: name,
                    integration: Some(it),
                    dll_present,
                    error: None,
                }
            }
            Err(e) => ListItem {
                file: name,
                integration: None,
                dll_present: false,
                error: Some(e),
            },
        })
        .collect()
}

/// 面板序列化。字段名与 index.html 接入区渲染一一对应。
pub fn integration_json(it: &Integration, dll_present: bool) -> Value {
    serde_json::json!({
        "file": it.file,
        "name": it.name,
        "dll": it.dll,
        "steps": it.steps,
        "note": it.note,
        "dir": it.dir,
        "dllPresent": dll_present,
    })
}

/// 读一个插件的 json。file 必须是纯文件名——它来自 WebView，带路径分隔符
/// 或 .. 的一律拒绝，不给面板的输入留目录穿越的口子。
pub fn load_one(file: &str) -> Result<Integration, String> {
    check_file_name(file)?;
    let path = plugins_dir().join(file);
    let text = std::fs::read_to_string(&path).map_err(|e| format!("读不了 {}：{}", file, e))?;
    parse_integration(file, &text)
}

/// 解析 + 校验一份 json 文本（load_one 的纯函数部分，测试用）。
fn parse_integration(file: &str, text: &str) -> Result<Integration, String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text); // 记事本可能存出 BOM

    let v: Value = serde_json::from_str(text)
        .map_err(|e| format!("{} 不是合法的 JSON：{}", file, e))?;
    let get_str = |key: &str| -> Result<String, String> {
        v.get(key)
            .and_then(|x| x.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| format!("{} 缺少 {}", file, key))
    };
    let steps = v
        .get("steps")
        .and_then(|x| x.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|s| s.as_str())
                .map(|s| s.to_string())
                .collect()
        })
        .unwrap_or_default();
    Ok(Integration {
        file: file.to_string(),
        name: get_str("name")?,
        dll: get_str("dll")?,
        host_exe: get_str("host_exe")?,
        plugin_subdir: get_str("plugin_subdir")?,
        steps,
        note: v
            .get("note")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
        dir: v.get("dir").and_then(|x| x.as_str()).unwrap_or("").trim().to_string(),
    })
}

fn check_file_name(file: &str) -> Result<(), String> {
    let bad = file.is_empty()
        || file.len() > 255
        || file.contains(['\\', '/', ':'])
        || Path::new(file).file_name().map_or(true, |f| f != file);
    if bad {
        return Err(format!("非法文件名：{}", file));
    }
    Ok(())
}

/// 把面板改的宿主目录写回 json。整体读成 Value 再改再写：
/// 未知字段不丢，用户手写的键、注释性的内容都原样保留。
pub fn set_dir(file: &str, dir: &str) -> Result<(), String> {
    check_file_name(file)?;
    let dir = dir.trim();
    if dir.contains('\r') || dir.contains('\n') {
        return Err("目录不能包含换行".into());
    }
    if dir.chars().count() > 1024 {
        return Err("目录太长".into());
    }
    let path = plugins_dir().join(file);
    let text =
        std::fs::read_to_string(&path).map_err(|e| format!("读不了 {}：{}", file, e))?;
    let mut v: Value = serde_json::from_str(text.strip_prefix('\u{feff}').unwrap_or(&text))
        .map_err(|e| format!("{} 不是合法的 JSON：{}", file, e))?;
    if let Some(obj) = v.as_object_mut() {
        obj.insert("dir".to_string(), Value::String(dir.to_string()));
    } else {
        return Err(format!("{} 的顶层不是对象", file));
    }
    let out = serde_json::to_string_pretty(&v).map_err(|e| format!("序列化 {} 失败：{}", file, e))?;
    std::fs::write(&path, out).map_err(|e| format!("写 {} 失败：{}", file, e))
}

/// 宿主的插件目录：json 里填了目录就用它，留空按常见安装位置探测；
/// 插件子目录存在就进到它，不存在退回安装根目录。
pub fn host_plugin_dir(file: &str) -> Result<PathBuf, String> {
    let it = load_one(file)?;
    let root = if it.dir.is_empty() {
        detect_host_dir(&it.name, &it.host_exe)?
    } else {
        PathBuf::from(&it.dir)
    };
    let target = root.join(&it.plugin_subdir);
    if target.is_dir() {
        Ok(target)
    } else {
        Ok(root)
    }
}

/// 按常见安装位置探测宿主目录（认 json 里的 host_exe）。
/// 故意不硬编码 D:\ 之类的机器布局——找不到就报错让用户填目录。
fn detect_host_dir(name: &str, host_exe: &str) -> Result<PathBuf, String> {
    for var in ["ProgramFiles", "ProgramFiles(x86)"] {
        if let Some(base) = std::env::var_os(var) {
            let cand = PathBuf::from(base).join(name);
            if cand.join(host_exe).exists() {
                return Ok(cand);
            }
        }
    }
    Err(format!("未找到 {}，已按常见位置探测，填写目录后重试", name))
}

// ---------------------------------------------------------------- 测试

#[cfg(test)]
mod tests {
    use super::*;

    /// 仓库里随发行走的两个 json 必须能被面板解析：手改坏了这里先红，
    /// 而不是发行出去后面板接入区悄悄变空。
    #[test]
    fn repo_plugin_jsons_parse() {
        for (file, text, expect_dll, expect_sub, expect_steps) in [
            (
                "afterburner_hr_plugin.json",
                include_str!("../../../ab-plugin/afterburner_hr_plugin.json"),
                "afterburner_hr_plugin.dll",
                r"Plugins\Monitoring",
                4,
            ),
            (
                "trafficmonitor_hr_plugin.json",
                include_str!("../../../tm-plugin/trafficmonitor_hr_plugin.json"),
                "trafficmonitor_hr_plugin.dll",
                "plugins",
                3,
            ),
        ] {
            let it = parse_integration(file, text).unwrap_or_else(|e| panic!("{}：{}", file, e));
            assert_eq!(it.dll, expect_dll);
            assert_eq!(it.plugin_subdir, expect_sub);
            assert_eq!(it.steps.len(), expect_steps);
            assert!(!it.name.is_empty());
            assert!(!it.host_exe.is_empty());
        }
    }

    /// 面板回传的 file 是攻击面：路径分隔符、盘符、.. 一律拒绝。
    #[test]
    fn rejects_bad_file_names() {
        for bad in ["", "..\\x.json", "../x.json", "C:x.json", "a/b.json", "a\\b.json"] {
            assert!(check_file_name(bad).is_err(), "应拒绝：{:?}", bad);
        }
        assert!(check_file_name("ok.json").is_ok());
    }

    /// 解析必须宽容：缺 steps/note/dir 用默认，坏 JSON 报带文件名的错。
    #[test]
    fn parse_is_lenient() {
        let it = parse_integration("t.json", r#"{"name":"N","dll":"d.dll","host_exe":"h.exe","plugin_subdir":"p"}"#)
            .unwrap();
        assert_eq!(it.steps.len(), 0);
        assert_eq!(it.note, "");
        assert_eq!(it.dir, "");
        assert!(parse_integration("t.json", "{oops").is_err());
        assert!(parse_integration("t.json", r#"{"name":""}"#).is_err()); // 缺必填键
    }
}
