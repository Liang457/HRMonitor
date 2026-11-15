// entry.js —— 面板全部页面逻辑：IPC 桥、状态渲染、扫描、表单、第三方接入、
// 确认对话框、toast。由 build.mjs 用 esbuild 打包压缩后内联进
// ../src/assets/index.html（单文件、零外部引用，产物不入库不手工编辑）。
// 组件一律用 @material/web 官方实现，动效与 Material 规范一致。

import { styles as typescaleStyles } from "@material/web/typography/md-typescale-styles.js";
import "@material/web/tabs/tabs.js";
import "@material/web/tabs/primary-tab.js";
import "@material/web/button/filled-button.js";
import "@material/web/button/filled-tonal-button.js";
import "@material/web/button/outlined-button.js";
import "@material/web/button/text-button.js";
import "@material/web/switch/switch.js";
import "@material/web/textfield/filled-text-field.js";
import "@material/web/progress/linear-progress.js";
import "@material/web/dialog/dialog.js";

// 排版基线注入一次。styles 是 lit 的 CSSResult，能进 adoptedStyleSheets 的只有它
// 惰性构造出的 CSSStyleSheet——直接塞对象本体在任何引擎都会 TypeError 并把整个
// 模块炸掉（页面全死）。构造式样式表不可用时退回普通 style 标签，行为等价。
let typescaleSheet = null;
try { typescaleSheet = typescaleStyles.styleSheet || null; } catch { typescaleSheet = null; }
if (typescaleSheet && "adoptedStyleSheets" in Document.prototype) {
  document.adoptedStyleSheets = [...document.adoptedStyleSheets, typescaleSheet];
} else {
  const fallback = document.createElement("style");
  fallback.textContent = typescaleStyles.cssText;
  document.head.appendChild(fallback);
}

// ---------------------------------------------------------------- IPC 桥
window.__hr = {
  _seq: 0,
  _pending: {},
  recv(m) {                       // Rust 回应（按 id 配对）
    const p = this._pending[m.id];
    if (p) { delete this._pending[m.id]; p(m.payload); }
  },
  call(cmd, params) {
    return new Promise((resolve) => {
      const id = ++this._seq;
      this._pending[id] = resolve;
      window.ipc.postMessage(JSON.stringify(Object.assign({ id: id, cmd: cmd }, params || {})));
    });
  },
  status(s) { renderStatus(s); },
  scanEvent(e) { renderScanEvent(e); },
  toast(msg) { showToast(msg); }
};

// ---------------------------------------------------------------- 工具
function $(id) { return document.getElementById(id); }
function esc(s) {
  return String(s).replace(/[&<>"]/g, function (c) {
    return { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c];
  });
}
function showToast(msg) {
  const t = document.createElement("div");
  t.className = "toast";
  t.textContent = msg;
  $("toasts").appendChild(t);
  setTimeout(function () { t.remove(); }, 4000);
}

// ---------------------------------------------------------------- 标签页
function bindTabs() {
  const tabs = $("tabs");
  const panels = [$("tabHeart"), $("tabConfig"), $("tabPlugins")];
  tabs.addEventListener("change", function () {
    const idx = tabs.activeTabIndex;
    panels.forEach(function (p, i) { p.hidden = i !== idx; });
  });
}

// ---------------------------------------------------------------- 复用确认对话框
// 一次只开一个；returnValue 等于点掉的按钮 value，Esc/遮罩关闭时为空 = 取消。
function confirmDialog(message, okText) {
  return new Promise(function (resolve) {
    const d = $("confirmDialog");
    $("confirmText").textContent = message;
    $("confirmOk").textContent = okText || "确认";
    let settled = false;
    d.addEventListener("closed", function (e) {
      if (settled) return;
      settled = true;
      resolve(e.detail.action === "ok");
    }, { once: true });
    d.show();
  });
}

// ---------------------------------------------------------------- daemon 开关
// 目标态非空 = 有一次启停正在进行：期间开关保持禁用，status 推送与目标一致
// 后恢复可用；20 秒兜底放开，别把开关锁死。扫描期间一律禁用不响应。
let daemonPending = null;   // true/false = 等待中的目标态；null = 没有
let daemonTimer = 0;
let daemonSyncing = false;  // 程序性赋值 checked 时挡掉 change 误触发



// md-switch v2 的状态属性是 selected：checked 是旧名，读写只会落在页面自建
// expando 上，组件不认。开关统一走这两个助手，免得散落读写错属性。
function swGet(el) {
  return el.tagName === "MD-SWITCH" ? el.selected : el.checked;
}
function swSet(el, v) {
  if (el.tagName === "MD-SWITCH") { el.selected = !!v; } else { el.checked = !!v; }
}

function renderDaemonSwitch(s) {
  const sw = $("daemonSwitch");
  if (daemonPending !== null) {
    if (!!s.daemon === daemonPending) {
      clearTimeout(daemonTimer);
      daemonPending = null;
      sw.disabled = !!s.scanning;
    }
    return;
  }
  daemonSyncing = true;
  swSet(sw, !!s.daemon);
  daemonSyncing = false;
  sw.disabled = !!s.scanning;
}

function bindDaemonSwitch() {
  $("daemonSwitch").addEventListener("change", function () {
    if (daemonSyncing) return;
    const sw = $("daemonSwitch");
    if (scanning || daemonPending !== null) {
      // 扫描中/已有启停在途：不响应切换，把误翻转的勾弹回去
      daemonSyncing = true;
      swSet(sw, !swGet(sw));
      daemonSyncing = false;
      return;
    }
    const target = swGet(sw);
    daemonPending = target;
    // 不禁用开关：md-switch 点下去的滑动动画要靠组件保持正常态才能播完，
    // 同步置 disabled 会把轨道/滑块颜色瞬切成 disabled 灰，动画被吞掉。
    // pending 期间再点会走上面的弹回分支，不会重复发命令。
    clearTimeout(daemonTimer);
    daemonTimer = setTimeout(function () {
      // 20 秒兜底：状态一直没推过来（推送线程被卡）也把 pending 清掉
      if (daemonPending !== null) {
        daemonPending = null;
        sw.disabled = scanning;
      }
    }, 20000);
    __hr.call(target ? "start_daemon" : "stop_daemon").then(function (r) {
      if (!r.ok) {
        clearTimeout(daemonTimer);
        daemonPending = null;
        daemonSyncing = true;
        swSet(sw, !target);   // 失败回滚
        daemonSyncing = false;
        sw.disabled = scanning;
        toast("失败：" + (r.error || "未知错误"));
      }
    });
  });
}

// ---------------------------------------------------------------- 状态卡
const STATUS_TEXT = {
  ok: "采集中",
  connecting: "连接中或等待重连",
  timeout: "数据超时",
  nodata: "无数据，未配置手表或未连接",
  off: "hr-daemon 未运行"
};
function renderStatus(s) {
  const bpmEl = $("bpm");
  if (s.bpm >= 0) {
    bpmEl.innerHTML = s.bpm + "<small>bpm</small>";
    bpmEl.classList.add("live");
  } else {
    bpmEl.textContent = "--";
    bpmEl.classList.remove("live");
  }
  let txt = STATUS_TEXT[s.status] || s.status;
  if (s.device) txt = "<b>" + esc(s.device) + "</b> · " + txt;
  if (s.ageSec > 3 && s.bpm >= 0) txt += " · " + s.ageSec + " 秒前更新";
  $("stext").innerHTML = txt;

  const badge = $("daemonBadge");
  if (s.daemon) { badge.textContent = "运行中"; badge.className = "chip on"; }
  else { badge.textContent = "未运行"; badge.className = "chip warn"; }
  $("daemonBanner").hidden = !!s.daemon;
  renderDaemonSwitch(s);
}

// ---------------------------------------------------------------- 扫描
let scanning = false;
function setScanProgress(on) { $("scanProgress").hidden = !on; }
function renderScanEvent(e) {
  const st = $("scanStatus");
  if (e.type === "stopping") { st.textContent = "正在停止 hr-daemon…"; }
  else if (e.type === "waiting") { st.textContent = "等待手表恢复广播…"; }
  else if (e.type === "device") { upsertDevice(e.mac, e.name); st.textContent = "扫描中，已发现 " + deviceCount() + " 台"; }
  else if (e.type === "done") {
    // UI 已被选表/取消复位（scanning 已清零）后才迟到的过期 done，忽略，
    // 免得把"扫描完成"和放弃按钮又顶出来
    if (!scanning) return;
    scanning = false;
    setScanProgress(false);
    $("btnScan").disabled = false;
    $("btnScanCancel").hidden = true;
    st.textContent = "扫描完成，共 " + e.count + " 台。点一台选定，或点放弃选择。";
    $("btnScanDiscard").hidden = false;
  }
}
function deviceCount() { return $("scanList").children.length; }
function upsertDevice(mac, name) {
  const list = $("scanList");
  let row = null;
  for (const d of list.children) {
    if (d.dataset.mac === mac) { row = d; break; }
  }
  if (!row) {
    row = document.createElement("div");
    row.className = "dev";
    row.dataset.mac = mac;
    row.innerHTML = '<span class="mac"></span><span class="nm"></span><span class="tag"></span>';
    row.onclick = function () { pickDevice(mac); };
    list.appendChild(row);
  }
  row.children[0].textContent = mac;
  row.children[1].textContent = name || "未命名";
  if (row.dataset.mac === currentAddress) row.children[2].textContent = "当前";
}
function resetScanUi() {
  $("scanList").innerHTML = "";
  $("scanStatus").textContent = "";
  setScanProgress(false);
  $("btnScanDiscard").hidden = true;
  $("btnScanCancel").hidden = true;
  $("btnScanCancel").disabled = false; // 别带着 disabled 态被下次扫描重新显示
  $("btnScan").disabled = false;
  scanning = false;
  if (daemonPending === null) $("daemonSwitch").disabled = false;   // 没有启停在途才放开
}
async function pickDevice(mac) {
  const ok = await confirmDialog(
    "把手表设为 " + mac + "？\n将保存地址并重启 hr-daemon。", "选定");
  if (!ok) return;
  $("btnScan").disabled = true;
  __hr.call("apply_address", { mac: mac }).then(function (r) {
    resetScanUi();
    $("btnScan").disabled = false;
    if (r.ok) { toast(r.message || "已保存"); currentAddress = mac; }
    else toast("失败：" + (r.error || "未知错误"));
    refreshMeta();
  });
}

// ---------------------------------------------------------------- 配置表单
const FIELDS = [
  { key: "demo",         kind: "bool",   desc: "模拟心率源",     sub: "不用手表，心率在 60 到 180 间随机变化" },
  { key: "address",      kind: "text",   desc: "手表 MAC",       sub: "留空则不连接" },
  { key: "scan_timeout", kind: "number", desc: "扫描超时",       sub: "毫秒" },
  { key: "backoff_min",  kind: "number", desc: "重连退避下限",   sub: "秒" },
  { key: "backoff_max",  kind: "number", desc: "重连退避上限",   sub: "秒，不小于下限" },
  { key: "timeout",      kind: "number", desc: "数据超时",       sub: "毫秒，超时后显示 --" },
  { key: "refresh",      kind: "number", desc: "刷新周期",       sub: "毫秒" },
  { key: "log_kb",       kind: "number", desc: "日志文件上限",   sub: "KB" },
  { key: "debug",        kind: "bool",   desc: "日志打印每条心率", sub: "仅排查时开启" }
];
let currentAddress = "";

function buildForm() {
  const box = $("fields");
  for (const f of FIELDS) {
    const row = document.createElement("div");
    if (f.kind === "bool") {
      row.className = "f bool";
      row.innerHTML =
        '<div class="fl"><div class="t">' + f.desc + '</div><div class="s">' + f.sub + '</div></div>' +
        '<md-switch data-key="' + f.key + '" aria-label="' + f.desc + '"></md-switch>';
    } else {
      row.className = "f";
      row.innerHTML =
        '<md-filled-text-field type="' + f.kind + '" data-key="' + f.key + '" label="' + f.desc + '"></md-filled-text-field>' +
        '<div class="fsub">' + f.sub + '</div>';
    }
    box.appendChild(row);
  }
}
function fillForm(cfg) {
  for (const el of $("fields").querySelectorAll("[data-key]")) {
    const v = cfg[el.dataset.key] || "";
    if (el.tagName === "MD-SWITCH") swSet(el, v === "1");
    else el.value = v;
    if (el.dataset.key === "address") currentAddress = v;
  }
  // 当前列表里标"当前"
  for (const d of $("scanList").children) {
    d.children[2].textContent = d.dataset.mac === currentAddress ? "当前" : "";
    d.classList.toggle("cur", d.dataset.mac === currentAddress);
  }
}
function collectForm() {
  const values = {};
  for (const el of $("fields").querySelectorAll("[data-key]")) {
    const v = el.tagName === "MD-SWITCH" ? (swGet(el) ? "1" : "0") : el.value;
    values[el.dataset.key] = v;
  }
  return values;
}
function saveForm() {
  $("formErr").textContent = "";
  return __hr.call("set_config", { values: collectForm() }).then(function (r) {
    if (!r.ok) {
      $("formErr").textContent = r.error || "保存失败";
      toast("保存失败：" + (r.error || "存在无效值"));
    } else {
      toast("已保存");
    }
    return r.ok;
  });
}

// ---------------------------------------------------------------- 元信息
function refreshMeta() {
  __hr.call("get_config").then(function (r) {
    if (!r.ok) { toast("读配置失败：" + (r.error || "")); return; }
    fillForm(r.config);
    swSet($("chkAutostart"), !!r.autostart);
    $("oldTaskBanner").hidden = !r.oldTask;
    $("paths").textContent =
      "hr-manager v" + r.version +
      " · 配置 " + r.iniPath + (r.iniExists ? "" : "，使用默认值") +
      " · daemon " + r.daemonExe;
    // 开关初始态以 get_config 的 daemonRunning 为准，随后被 status 推送接管
    if (daemonPending === null) {
      daemonSyncing = true;
      swSet($("daemonSwitch"), !!r.daemonRunning);
      daemonSyncing = false;
    }
  });
}

// ---------------------------------------------------------------- 第三方接入
function renderIntegrations(r) {
  const box = $("integrations");
  box.innerHTML = "";
  if (!r.ok) { box.innerHTML = '<div class="integ"><div class="err">' + esc(r.error || "读取失败") + '</div></div>'; return; }
  for (const it of r.items) {
    const div = document.createElement("div");
    div.className = "integ";
    if (it.error) {   // 解析失败的 json：只报错，不给操作
      div.innerHTML =
        '<div class="integ-head"><div class="integ-name">' + esc(it.file) + '</div></div>' +
        '<div class="err" style="margin-top:8px">解析失败：' + esc(it.error) + '</div>';
      box.appendChild(div);
      continue;
    }
    const steps = (it.steps || []).map(function (s) { return "<li>" + esc(s) + "</li>"; }).join("");
    div.innerHTML =
      '<div class="integ-head"><div class="integ-name">' + esc(it.name) + '</div>' +
      '<span class="chip ' + (it.dllPresent ? "on" : "warn") + '">' + (it.dllPresent ? "DLL 就绪" : "DLL 缺失") + '</span></div>' +
      (steps ? '<ol class="steps">' + steps + '</ol>' : "") +
      '<md-filled-text-field data-file="' + esc(it.file) + '" label="' + esc(it.name) + ' 目录" value="' + esc(it.dir || "") + '"></md-filled-text-field>' +
      '<div class="fsub">留空自动探测</div>' +
      '<div class="row"><md-text-button data-open="' + esc(it.file) + '">打开 ' + esc(it.name) + ' 插件目录</md-text-button></div>' +
      (it.note ? '<div class="note">' + esc(it.note) + '</div>' : "");
    box.appendChild(div);
  }
  // 目录输入：失焦或回车时保存（dataset.saved 去重，值没变不重复写）
  for (const el of box.querySelectorAll("md-filled-text-field[data-file]")) {
    const save = function () {
      if (el.dataset.saved === el.value) return;
      __hr.call("set_integration_dir", { file: el.dataset.file, dir: el.value }).then(function (r2) {
        if (r2.ok) { el.dataset.saved = el.value; toast("已保存"); }
        else toast("保存失败：" + (r2.error || ""));
      });
    };
    el.dataset.saved = el.value;
    el.addEventListener("change", save);
    el.addEventListener("keydown", function (ev) { if (ev.key === "Enter") el.blur(); });
  }
  for (const b of box.querySelectorAll("md-text-button[data-open]")) {
    b.onclick = function () {
      __hr.call("open_folder", { file: b.dataset.open }).then(function (r2) {
        if (!r2.ok) toast(r2.error || "打开失败");
      });
    };
  }
}
function loadIntegrations() {
  __hr.call("get_integrations").then(renderIntegrations);
}

// ---------------------------------------------------------------- 事件绑定
function bind() {
  $("btnScan").onclick = function () {
    if (scanning) return;
    resetScanUi();
    scanning = true;
    $("btnScan").disabled = true;
    $("btnScanCancel").hidden = false;
    setScanProgress(true);
    $("scanStatus").textContent = "准备扫描…";
    $("daemonSwitch").disabled = true;   // 扫描期间禁用 daemon 开关
    __hr.call("scan_start", { secs: 10 }).then(function (r) {
      setScanProgress(false);
      if (!r.ok) {
        resetScanUi();
        $("scanStatus").textContent = "";
        toast("扫描启动失败：" + (r.error || ""));
      } else if (r.cancelled) {
        // 必须整体复位：不调 resetScanUi 的话 scanning 停在 true，
        // 上面的防重入会把扫描按钮锁死，取消按钮也一直挂着
        resetScanUi();
        $("scanStatus").textContent = "已取消，采集已恢复。";
      }
    });
  };
  $("btnScanCancel").onclick = function () {
    $("btnScanCancel").disabled = true;
    __hr.call("scan_cancel").then(function () { $("btnScanCancel").disabled = false; });
    $("scanStatus").textContent = "取消中…";
  };
  $("btnScanDiscard").onclick = function () {
    __hr.call("scan_discard").then(function () {
      resetScanUi();
      $("scanStatus").textContent = "已放弃，采集已恢复。";
    });
  };

  $("btnSave").onclick = function () { saveForm(); };
  $("btnSaveRestart").onclick = function () {
    saveForm().then(function (ok) {
      if (ok) __hr.call("restart_daemon").then(function (r) {
        toast(r.ok ? (r.message || "已重启") : "重启失败：" + r.error);
      });
    });
  };
  $("btnReset").onclick = async function () {
    const ok = await confirmDialog("恢复默认配置并覆盖 hr-daemon.ini？", "恢复默认");
    if (!ok) return;
    __hr.call("reset_config").then(function (r) {
      if (r.ok) { toast("已恢复默认配置，重启 daemon 后生效"); refreshMeta(); }
      else toast("失败：" + r.error);
    });
  };

  // 状态卡右上角动作
  $("btnRestart").onclick = function () {
    __hr.call("restart_daemon").then(function (r) {
      toast(r.ok ? (r.message || "已重启") : "重启失败：" + r.error);
    });
  };
  $("btnLogs").onclick = function () { __hr.call("open_logs"); };

  $("chkAutostart").addEventListener("change", function () {
    __hr.call("autostart_set", { enabled: swGet($("chkAutostart")) }).then(function (r) {
      if (!r.ok) { swSet($("chkAutostart"), !swGet($("chkAutostart"))); toast("失败：" + r.error); }
    });
  });

  $("btnOpenPlugins").onclick = function () {
    __hr.call("open_folder", { which: "plugins" }).then(function (r) {
      if (!r.ok) toast(r.error || "打开失败");
    });
  };

  // 确认对话框的动作按钮（复用一份，show 前 just 改文案）
  $("confirmCancel").addEventListener("click", function () { $("confirmDialog").close("cancel"); });
  $("confirmOk").addEventListener("click", function () { $("confirmDialog").close("ok"); });
}

// ---------------------------------------------------------------- 启动
bindTabs();
bindDaemonSwitch();
buildForm();
bind();
refreshMeta();
loadIntegrations();
