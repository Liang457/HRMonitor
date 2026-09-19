# AGENTS.md

BLE 心率广播设备（服务 `0x180D`，手表、手环等）→ Windows：`hr-daemon.exe`（C++）采集后写入中立共享内存
`Local\BleHR_SM`（64 字节），三个互不依赖的读端各自去读——Afterburner 监控数据源插件
（游戏内 OSD，由 RTSS 渲染）、TrafficMonitor 任务栏插件，以及 `hr-manager.exe`（Rust）的
实时状态面板。仅 Windows。仓库文档与注释全为中文，新代码的注释/日志/UI 字符串请保持中文。

## 构建与验证

无测试套件，验证 = 构建 + demo 模拟源 + 探针：

```cmd
cmd /c daemon\build.cmd           :: build\hr-daemon.exe             (C++, x64)
cmd /c ab-plugin\build.cmd        :: build\plugins\HeartRate.dll     (C++, x86，必须 x86)
cmd /c tm-plugin\build.cmd        :: build\plugins\hr_plugin.dll     (C++, x64)
cmd /c tools\osd_test\build.cmd   :: build\osd_test.exe              (C++, x64)
cmd /c tools\hr-manager\build.cmd :: build\hr-manager.exe            (Rust，GUI 栈依赖见下)
```

- C++ 的 build.cmd 用 vswhere 自动定位 MSVC（不绑死 VS 版本）；exe/PDB 落 `build\` 根、
  两个 DLL 落 `build\plugins\`（已 gitignore，布局与发行目录一致）。
  编译统一带 `/W4 /sdl /guard:cf /Zi`（PDB 落 `build\`，崩溃转储要靠它符号化），保持零警告；
  两个插件的 build.cmd 末尾有 dumpbin 自查（机器码架构 + 未修饰导出名），改了导出签名先想过这关。
  每个组件还链一份版本资源：各自的 version.rc 壳 → `common\version.rc.in`，rc.exe 编译。
- C++ 全部 `/MT` 静态 CRT，目标机器不装 VC++ 运行库。
- 不用手表即可验证整条链路：`build\hr-manager.exe set demo 1 && build\hr-manager.exe restart`，
  然后 `pwsh -File tools\mahm-probe\mahm-probe.ps1 -Filter Heart` 看 Afterburner 共享内存里的值；
  hr-manager 面板的状态卡也应显示模拟心率。
- **发行版由 GitHub Actions 编译，不在本地出包**：`.github/workflows/release.yml`
  （push main / 打 v* 标签 / 手动触发）在 windows-latest 上复用仓库的 build.cmd 构建
  四个发行组件（osd_test 不进发行；dumpbin 自检在 CI 同样生效），再调
  `scripts\pack.ps1` 组装压 zip。push main 只产 dev artifact
  （`HRMonitor-dev-<短SHA>-win64.zip`）供下载，打 v* 标签才创建 GitHub Release。
  本地构建只做开发验证；本地跑 `powershell -ExecutionPolicy Bypass -File
  scripts\pack.ps1` 走的是同一条打包路（校验版本双副本一致 + 发行布局 + zip 到
  dist\），但那只是拿包，不算发行。

## 架构边界（改代码前必读）

- **`common/hr_shared.h` 是共享内存协议的唯一权威定义**：64 字节 `HrSharedData`、
  `#pragma pack(4)`、magic/version。daemon 写，两个插件和 hr-manager 读。改布局必须升
  `HRSM_VERSION` 并**所有读端一起重建**——读端校验 version，不匹配就直接读不到数据。
  写端无锁 memcpy 是已接受的取舍（每 refresh_ms 整条重写自愈），动一致性前先读该文件内注释。
- daemon 只采集、只写共享内存，不碰 RTSS/Afterburner/TrafficMonitor；本项目与 RTSS 零耦合
  （不写它的共享内存、不引用它的头文件）。
- 三个读端互不依赖，只通过共享内存解耦；插件只依赖 kernel32。
- `common/hr_config.{h,cpp}`：INI 读写 + 配置结构，daemon 和 tm-plugin 共用（C++ 读端）。
  `tools/hr-manager/src/config.rs` 是 Rust 写端副本：键名/注释/钳位必须与 C++ 读端保持一致。
- `common/hr_names.h` 与 `tools/hr-manager/src/names.rs` 是一对跨语言常量副本
  （daemon 互斥体名、窗口类名）——改任何一个必须两边同步。
- `common/version.h` 与 `tools/hr-manager/Cargo.toml` 的 `[package] version` 是产品版本号的
  跨语言双副本——改版本必须两边同步；`scripts\pack.ps1` 打包时强制校验一致（HEAD 带 v* 标签
  时连标签一起校验）。各 exe/DLL 的 PE 版本资源、daemon `--version`、hr-manager
  `version` 子命令、面板底部与托盘提示都从这里取。它与 `hr_shared.h` 的 `HRSM_VERSION`
  （共享内存协议版本）是两回事，别混着改。
- **运行期文件布局**（发行目录的根只放 exe 和 README.md，全部相对 exe 所在目录）：配置
  `config\hr-daemon.ini`、日志 `log\hr-daemon.log(.1/.2)`、WebView2 数据 `webview2\`、
  扫描恢复标记 `config\scan-pending`。程序只认新路径、**不做旧位置回退**（C++ 侧
  `HrDaemonIniPath()`、Rust 侧 `config::ini_path()`）；老部署的根目录 hr-daemon.ini 由
  `scripts\migrate-1.1.3.ps1` 一次性搬进 config\（留 .bak），别在程序里加"搬家"逻辑。
- **daemon 是唯一的蓝牙入口**：连接/重连/扫描全在 `daemon/ble.cpp`（C++/WinRT，链接
  windowsapp.lib）。`--scan` 按行往 stdout 吐 JSON（`{"type":"device",...}`/`{"type":"done",...}`），
  hr-manager 起子进程读流，自己不碰蓝牙。
- hr-manager（Rust）= 托盘常驻 + 按需弹出的 WebView2 配置面板 + CLI 子命令：管理 daemon
  生命周期（FindWindowW+WM_CLOSE 优雅停 / OpenMutexW 探活）、读写 ini、读共享内存显示实时
  状态、调度扫描、管理开机自启（注册表 Run 键）。面板关闭即整体销毁 WebView2（内存回落），
  托盘进程本身保留。
- **依赖策略：业务代码不引第三方**。C++ 只用 Windows SDK（BLE 用自带 C++/WinRT）。Rust 端
  仅有的例外是 hr-manager 的 GUI 栈（wry + tao + tray-icon + serde/serde_json）和构建期
  专用的 winresource（build.rs 嵌 PE 版本信息，不进运行时），Cargo.lock 锁
  版本，首次构建需联网；其余（配置/进程/注册表/共享内存/管道）全部是 `src/win.rs` 手写
  `extern "system"` 声明。**不要再扩大依赖面，也不要引 npm/前端构建链**（面板是
  `include_str!` 嵌入的零依赖原生 HTML/CSS/JS 单页）。

## 硬性约束 / 已知坑

- **ab-plugin 必须 x86**：`MSIAfterburner.exe` 是 32 位进程，加载不了 x64 DLL。
  build.cmd 末尾用 dumpbin 自查 x86 + 三个导出名（`GetSourcesNum`/`GetSourceDesc`/
  `GetSourceData`）未修饰。该插件保持 MBCS（故意不定义 UNICODE），因为
  `MONITORING_SOURCE_DESC` 用 `char[]`。
- **所有 C++ 编译必须带 `/utf-8`**：源码是无 BOM UTF-8 且含中文字符串，缺了会按系统
  代码页误读成乱码。
- **`.cmd` 批处理保持 CRLF、注释尽量 ASCII-only**（`.gitattributes` 已强制 eol=crlf）；
  `(...)` 块内 echo 的文本里不能出现裸 `)`，会提前闭合代码块。
- 换行符约定：.cmd/.bat/.ps1 = CRLF；.cpp/.h/.rs/.md/.toml/.ini/.lock = LF。
- **自启用注册表 Run 键**（`HKCU\...\CurrentVersion\Run`，值名 `BleHRManager`，指向
  `hr-manager.exe --minimized`）：无管理员、天然在当前登录会话，共享内存 `Local\` 命名空间
  因此一定对。老版本的计划任务方案已废弃（会话 0 命名空间的坑随之消失），残留清理用
  `scripts\uninstall-task.ps1`，面板会检测残留并提示。
- daemon 命令行参数（`--demo`/`--address`）优先于 hr-daemon.ini；daemon 单实例互斥体
  `Local\BleHR_daemon`，manager 单实例互斥体 `Local\BleHR_manager`。第二个 manager
  实例 SetEvent `Local\BleHR_manager_open` 通知第一个实例打开面板后退出——打开事件要带
  `EVENT_MODIFY_STATE` 权限位，只给 SYNCHRONIZE 的话 SetEvent 会静默失败（踩过）。
- **hr-manager 是 GUI 子系统，CLI 子命令必须先 `cli::attach_console()` 再打印**：附加父
  控制台 + SetStdHandle 要赶在任何打印/读输入之前（Rust std 句柄惰性初始化）；`reset` 的
  确认提示读到 EOF 一律当"不做"。脚本里也别假设 `& hr-manager.exe <子命令>` 会等它退出——
  CI 的 pwsh 对 GUI 进程不等就往下跑，`$LASTEXITCODE` 还是上一条命令的残留值；要拿真实
  退出码用 `Start-Process -Wait -PassThru`（pack.ps1 生成示例 ini 时踩过：Test-Path 和
  写文件赛跑，CI 必挂）。
- **扫描前必须停 daemon**（手表被连着时不广播），停启编排在 `tools/hr-manager/src/scan_job.rs`：
  取消/失败路径要把 daemon 拉回来；正常扫完保持停止等选表，用户放弃时（面板关闭/点放弃）
  再拉回——`Core::scan_needs_restore` 是**粘滞标志**（只在实际恢复成功处清零，扫描线程开头
  不许清），配合 `config\scan-pending` 标记文件（停 daemon 前写、确认回来后删，manager 启动时看到
  就自动恢复）兜住"扫描中途 manager 被杀"。扫描线程阻塞在读管道上，取消必须
  置位 + kill 子进程双管齐下；`--scan` 子进程被 Job Object（KILL_ON_JOB_CLOSE）拴着，
  manager 死则子进程必死。
- TrafficMonitor V1.86 只扫 `plugins\*.dll`，插件必须是 `.dll` 不是 `.tmd`；
  `config.ini` 的 `plugin_disabled` 是黑名单，插件默认即启用。
- **tm-plugin 有两个宿主窗口线程**（主窗口/任务栏各自 timer）并发调 `DataRequired()`：
  `Refresh()` 全程独占 SRWLOCK；返回给宿主的字符串走 `TextCell` 三缓冲原子发布；
  **读失败绝不 Close 共享内存映射**（unmap 掉别的线程正在读的视图会崩宿主，踩过）。
- 真机长测尚未完成（开发期实测设备为华为手表）：扫描→连接→订阅→设备名已验收；心率进共享内存、息屏/
  超时断开后的长时间重连还在测。协议按标准 BLE HR Profile 实现，链路用 demo 源验收过。

## 部署脚本（scripts\）

- 发行 zip 的 `scripts\` 子目录原样带上部署脚本；hr-manager 面板的部署按钮经
  `ipc.rs::run_deploy_script` 调它们（tm 的部署把 `integration.tm_dir` 传给脚本）。
- `deploy-afterburner-plugin.ps1`（Afterburner 4.6.6 实测）：装 HeartRate.dll 进
  `<AB>\Plugins\Monitoring\`、写 Help 说明、置 `Profiles\MSIAfterburner.cfg` 的
  `[Monitoring] HeartRate.dll=1` 与根配置 `EnablePlugins=1`。**故意不动**
  `[Settings] Sources=`/`[Source Heart rate]` 段——那是 Afterburner 自己的数据结构，
  硬改会被覆盖；OSD 显示仍要用户在界面里勾两下。
- `configure-trafficmonitor.ps1`（V1.86 实测）：部署 hr_plugin.dll 进 `<TM>\plugins\`，
  把项目 id（默认 `hr`）并进 `config.ini` 的 `plugin_display_item`。编码按锚点键探测、
  按原编码写回（这份配置的编码在变）；`show_task_bar_wnd` 写 true 不一定生效，
  任务栏没出现要用户手动开一次（开过会被记住）。
- 部署 DLL 的共同细节：先关宿主（宿主退出时会把内存配置整个写回，改文件必须在它
  停着时做）、SHA256 相同则跳过替换、临时文件名不带 .dll 后缀（防宿主扫 `*.dll`
  时加载半成品）、`File.Replace` 原子替换、旧 DLL 留 `.bak`。
- `run-daemon.cmd`：前台调试跑 daemon（日志直出当前控制台，Ctrl+C 停）。

## 文档

- `README.md`：用法、配置、排查（hr-daemon/hr-manager 命令行、ini 键表、部署脚本）。
- `THIRD_PARTY_NOTICES.md`：`tm-plugin/PluginInterface.h` 取自 TrafficMonitor，Anti-996
  许可，分发约束见此。

## 风格

- C++20；成员变量 `m_` 前缀；中文注释（与现有代码一致）。
- Rust：错误一律 `Result<_, String>`（给人看的中文消息）；不做呈现的库模块（daemon_ctl/
  scan_job/autostart/config）与做呈现的 cli/gui 分层，打印/弹提示只在后者；共享状态集中在
  `state.rs::Core`，WebView 只在主线程碰，回推一律走 `UserEvent::Eval` 经主线程执行。
- `scripts\` 里的部署脚本改第三方程序配置前的固定模式：先关目标进程、备份 `.bak`、
  原子写回、只动属于自己的那一行——新脚本沿用这个模式。
