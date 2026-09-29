# 第三方软件接入

hr-daemon 把实时心率写进一块 Windows 共享内存，**任何软件都可以零依赖地读它**——不需要链接
本项目的任何库，也不需要管理员权限，按本文的 64 字节布局解字节即可。适合接入监控工具、
游戏 OSD、脚本、自研客户端等。

- 协议的权威定义是 [`common/hr_shared.h`](common/hr_shared.h)，本文与其保持同步；若有出入以头文件为准。
- 现成的参考实现（三个读端）：`tm-plugin/plugin.cpp`（C++）、`tools/hr-manager/src/shared_mem.rs`（Rust）、
  以及本文的 Python 示例。
- 安全边界一句话：共享内存在 `Local\` 会话命名空间、默认 DACL，校验只防"版本错配"和
  "意外垃圾"，不防同会话进程伪造；读端从不把数据当代码/指针解析，最坏后果只是显示错的心率值。

## 总览

- **名字**：`Local\BleHR_SM`（文件映射），记录**定长 64 字节**，小端，`#pragma pack(4)` 布局。
- **写入节奏**：daemon 每 `refresh_ms`（`hr-daemon.ini` 的 `[display]` 段，默认 1000ms，
  钳位 200–60000）整条重写一次；心率/状态/设备名有变化时也会立即重写。
- **打开方式**：`OpenFileMappingW(FILE_MAP_READ, FALSE, "Local\\BleHR_SM")` +
  `MapViewOfFile(…, FILE_MAP_READ, 0, 0, 64)`。
- **会话命名空间**：`Local\` 前缀意味着每个登录会话各有一份——只有**同一登录会话**里的进程
  读得到。Windows 服务（会话 0）和其他用户的会话读不到。
- **写端无锁**：写端是裸 memcpy，理论上可能读到"半条"记录（例如新 magic 配旧 bpm、8 字节的
  tick_ms 撕裂）。daemon 下一拍（≤1 秒）整条重写自愈，读端按[读端规矩](#读端必守的规矩)容错即可。

## 64 字节布局

小端（x86/x64 内存字节序），`pack(4)`。任何语言都对照这份定义解：

| 偏移 | 大小 | 类型 | 字段 | 语义 |
|---|---|---|---|---|
| 0 | 4 | u32 | magic | 恒 `0x4D535248`，内存里前 4 字节是 `48 52 53 4D`（ASCII "HRSM"） |
| 4 | 4 | u32 | version | 恒 `1`；不认识的版本必须拒绝读取 |
| 8 | 4 | i32 | bpm | 心率（次/分）；`-1` = 无效 |
| 12 | 8 | u64 | tick_ms | 采样时刻，`GetTickCount64()` 毫秒（系统开机起算） |
| 20 | 4 | u32 | status | 0/1/2/3，见下表 |
| 24 | 4 | u32 | battery | 保留，当前恒 0（未来填 BLE 电量 0–100） |
| 28 | 36 | char[36] | device_name | 设备名，UTF-8、NUL 结尾，最长 35 字节；空串 = 未知 |

C 结构体原文（`common/hr_shared.h`）：

```c
#pragma pack(push, 4)
struct HrSharedData {
    DWORD     magic;              // HRSM_MAGIC = 0x4D535248 ('HRSM')
    DWORD     version;            // HRSM_VERSION = 1
    LONG      bpm;                // -1 = 无效
    ULONGLONG tick_ms;            // GetTickCount64() 采样时刻
    DWORD     status;             // HrStatus
    DWORD     battery;            // 保留（未来读 0x180F）
    char      device_name[36];    // 手表名，NUL 结尾
};
#pragma pack(pop)                 // 4+4+4+8+4+4+36 = 64
```

配套常量：`HRSM_TIMEOUT_MS = 15000`——数据超过 15 秒没更新视为超时（daemon 侧对应
`[display] timeout_ms`，默认 15000、钳位 2000–600000；读端按 15000 兜底即可）。

## 字段语义

### status

| 值 | 含义 | 展示建议 |
|---|---|---|
| 0 | NODATA：daemon 还没拿到过数据，且未配置手表地址 | `--` |
| 1 | OK：正常 | 显示 bpm |
| 2 | CONNECTING：正在扫描/连接/重连 | `--` 或"连接中" |
| 3 | TIMEOUT：曾有数据，但已超时 | `--` |

### bpm 的有效性判定

与 daemon 侧 `HrEffectiveBpm()` 同口径，一条公式（任一命中就显示 `--`）：

```
status == 3 (TIMEOUT)                         → 无效
bpm < 0 或 bpm > 300                          → 无效
now - tick_ms > 15000                         → 无效（超时自查）
否则                                           → 有效，显示 bpm
```

- `bpm > 300` 也拒收：无锁写的撕裂脏帧可能落在 bpm 上，显示一个 65536 bpm 的毛刺毫无意义，
  下一拍必然自愈。
- **超时必须自查**：daemon 异常退出（崩溃/被杀任务管理器）时没有"临终帧"，status 会冻结在
  最后一帧（通常还是 1=OK），读端必须自己拿 `now - tick_ms` 兜底判超时，别指望 status 变 3。

### tick_ms

时基是 `GetTickCount64()`——**从系统开机起算的毫秒数，不是 Unix 时间戳**。跨机器、跨重启
都没有绝对意义，只能在本机算"距今多久"。它虽然是个 u64，但在布局里偏移 12、只有 4 字节
对齐：32 位进程直接按 64 位类型读可能撕裂，稳妥做法是按字节拷出来再拼。

### device_name

UTF-8 编码（**不是** ANSI/本地代码页），NUL 结尾，最长 35 字节，超长截断在 UTF-8 字符边界。
解码时防两种情况：字段里没有 NUL（取到字段尾就停）、解码失败（用替换字符兜底）。另注意
daemon 断开后名字保留最后一次连接的设备名，不会清空。

## 读端必守的规矩

1. **校验 magic 和 version**。version 不认识（新 daemon 配旧读取端，或反过来）时明确提示
   "版本不匹配，要一起更新"，**别当成"没数据"**——用户会去排查蓝牙，白折腾。
2. **超时自查**（上一节），status 不可信。
3. **打开后长期持有，别反复 close/重开**。daemon 退出后映射对象因读端握着句柄依然存活
   （内容是最后一帧），daemon 重启时写端 `CreateFileMappingW` 拿到的是同一对象，读端无需重开。
   多线程程序尤其注意：读失败就 unmap 重开，可能 unmap 掉另一个线程正在读的视图直接崩宿主
   ——TrafficMonitor 插件在这上面翻过车。
4. **打不开映射就隔一会儿重试**（建议 1 秒限流），daemon 可能只是还没起。
5. **别拿"能读到数据"当"daemon 在跑"**（第 3 条的原因）。要探活 daemon 进程是否存活，用
   互斥体 `Local\BleHR_daemon`：`OpenMutexW(SYNCHRONIZE, FALSE, L"Local\\BleHR_daemon")`
   成功即在跑。
6. **每次读取先把 64 字节整块拷出来再解**（快照），别在映射视图上边解边读。

## Python 示例

Windows 专用、只用标准库（Python 3.6+），需与 hr-daemon 在同一登录会话运行。
每秒读一次并打印：

```python
# 读取 HRMonitor 实时心率 —— 仅标准库，仅 Windows；
# 需与 hr-daemon 处于同一登录会话（共享内存在 Local\ 命名空间）。
import ctypes
import struct
import time
from ctypes import wintypes

# ---- 协议常量（与 common/hr_shared.h 一致）----
SM_NAME    = "Local\\BleHR_SM"   # 共享内存名
SM_SIZE    = 64                  # 记录定长 64 字节
SM_MAGIC   = 0x4D535248          # 内存前 4 字节是 ASCII "HRSM"
SM_VERSION = 1                   # 布局版本，不认识就停用
TIMEOUT_MS = 15000               # 数据 15 秒没更新视为超时
STATUS     = {0: "无数据", 1: "正常", 2: "连接中", 3: "超时"}

# "<" = 小端且不加对齐填充，六个字段偏移恰好是 0/4/8/12/20/24：
#       magic(u32) version(u32) bpm(i32) tick_ms(u64) status(u32) battery(u32)
FMT = "<IIiQII"

# ---- Win32 API 声明 ----
kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
kernel32.OpenFileMappingW.restype  = wintypes.HANDLE
kernel32.OpenFileMappingW.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.LPCWSTR]
kernel32.MapViewOfFile.restype     = ctypes.c_void_p
kernel32.MapViewOfFile.argtypes    = [wintypes.HANDLE, wintypes.DWORD, wintypes.DWORD,
                                      wintypes.DWORD, ctypes.c_size_t]
kernel32.UnmapViewOfFile.argtypes  = [ctypes.c_void_p]
kernel32.CloseHandle.argtypes      = [wintypes.HANDLE]
kernel32.GetTickCount64.restype    = ctypes.c_uint64  # 不设 restype 会被按 32 位截断
FILE_MAP_READ = 0x0004

def open_shared_memory():
    """只读打开映射，返回 (句柄, 视图地址)；打不开（daemon 没跑/不在同一会话）返回 (None, None)。"""
    handle = kernel32.OpenFileMappingW(FILE_MAP_READ, False, SM_NAME)
    if not handle:
        return None, None
    view = kernel32.MapViewOfFile(handle, FILE_MAP_READ, 0, 0, SM_SIZE)
    if not view:
        kernel32.CloseHandle(handle)
        return None, None
    return handle, view

def parse(buf):
    """解出一条记录；magic/version 不符时抛 ValueError——按规矩应停用，而不是当没数据。"""
    magic, version, bpm, tick_ms, status, battery = struct.unpack_from(FMT, buf)
    if magic != SM_MAGIC:
        raise ValueError("magic 不符：映射里不是本协议的数据")
    if version != SM_VERSION:
        raise ValueError("版本不匹配：daemon 和读取端要一起更新")
    # device_name：UTF-8、NUL 结尾；split 顺带防了"没写 NUL"的情况
    device = buf[28:64].split(b"\x00", 1)[0].decode("utf-8", "replace")
    return {"bpm": bpm, "tick_ms": tick_ms, "status": status, "device": device}

def effective_bpm(rec, now_ms):
    """有效性判定（与 daemon 侧 HrEffectiveBpm() 同口径）；返回 -1 表示应显示 --。"""
    if rec["status"] == 3:                                    # 超时状态
        return -1
    if rec["bpm"] < 0 or rec["bpm"] > 300:                    # >300 是无锁写撕裂帧的脏值
        return -1
    if now_ms > rec["tick_ms"] and now_ms - rec["tick_ms"] > TIMEOUT_MS:
        return -1                                             # daemon 崩了 status 会冻结，超时必须自查
    return rec["bpm"]

def main():
    handle, view = open_shared_memory()
    if handle is None:
        print("打不开共享内存：hr-daemon 没在跑，或不在同一登录会话。")
        return
    try:
        while True:
            buf = ctypes.string_at(view, SM_SIZE)      # 整条拷成快照再解，别边解边读
            rec = parse(buf)
            bpm = effective_bpm(rec, kernel32.GetTickCount64())
            if bpm < 0:
                print(f"\r心率: -- （{STATUS.get(rec['status'], '未知')}）      ", end="")
            else:
                print(f"\r心率: {bpm} bpm   设备: {rec['device'] or '未知'}      ", end="")
            time.sleep(1)
    except ValueError as e:                            # magic/version 不符：不该继续读
        print(f"\n{e}")
    except KeyboardInterrupt:
        print()
    finally:
        kernel32.UnmapViewOfFile(view)
        kernel32.CloseHandle(handle)

if __name__ == "__main__":
    main()
```

想等 daemon 后启动？隔 1 秒重试 `open_shared_memory()` 即可。刷新节奏与 daemon 的写入频率
（默认 1 秒）对齐即可，读得更快没有意义。

## 接入其他语言的通用套路

任何语言都按这 6 步走（上面的 Python 示例就是逐条对应的）：

1. `OpenFileMappingW(FILE_MAP_READ, FALSE, "Local\\BleHR_SM")` 打开映射——C/C++ 直接把
   `common/hr_shared.h` 拷进工程，用现成的 `HrSharedReader`。
2. `MapViewOfFile(handle, FILE_MAP_READ, 0, 0, 64)` 映射 64 字节只读视图，打开后长期持有。
3. 每次读取：先把 64 字节整块拷成快照，再校验 magic、version；version 不认识 → 明确提示
   版本不匹配并停用。
4. 按小端解字段；tick_ms 按字节拷出再拼 u64，别依赖对齐。
5. 有效性判定：status==3 / bpm<0 / bpm>300 / `now - tick_ms > 15000` → 显示 `--`。
6. 解析 device_name（UTF-8、防没有 NUL），空串当未知。

Rust 接入可参考 `tools/hr-manager/src/shared_mem.rs`：手写 `extern "system"` 声明 +
裸字节偏移读取，零第三方依赖。

## 验证与排查

不用手表就能验证整条链路——开 demo 模拟源（60–180 bpm 正弦波动，设备名"DEMO 模拟心率"）：

```cmd
hr-manager.exe set demo 1
hr-manager.exe restart
```

（开发构建在 `build\` 下执行；发行目录则在解压根。）跑起来后运行本文的 Python 示例，应每秒
打印变化的心率和"DEMO 模拟心率"。测完关掉：`hr-manager.exe set demo 0` 再 `restart`。
也可以直接前台跑 daemon：`hr-daemon.exe --demo`（Ctrl+C 停）。

| 现象 | 原因 |
|---|---|
| `OpenFileMappingW` 打不开（GetLastError=2） | daemon 没在跑；或本进程不在同一登录会话（Windows 服务、其他用户） |
| 打得开但 magic 不符 | 映射里不是本协议的数据（同名的别人的映射），按无效数据处理 |
| 一直 `--` 且 status=0 | daemon 没配置手表地址（`config\hr-daemon.ini` 的 `[source] address`） |
| 一直 `--` 且 status=2 | 正在扫描/连接，看 `log\hr-daemon.log` |
| 有值但再也不动 | daemon 进程死了，读端握着句柄所以内容停在最后一帧——自查超时 15 秒后应转 `--`；要确认进程用互斥体探活 |
| 提示版本不匹配 | daemon 和读取端一侧更新了另一侧没更新，两边一起换 |

相关名字速查（都可以在集成时使用）：

| 名字 | 类型 | 用途 |
|---|---|---|
| `Local\BleHR_SM` | 文件映射 | 心率数据本体（本文主角） |
| `Local\BleHR_daemon` | 互斥体 | daemon 单实例标记，可用来探活 |
| `BleHRDaemonWnd` | 隐藏窗口类 | `FindWindowW` 找到后发 `WM_CLOSE` 可优雅停 daemon |

## 版本契约

- 64 字节布局由 `HRSM_VERSION` 守护：**改布局必须升版本号**，且所有读端一起重建/更新——
  读端校验 version，不匹配就读不到数据（这是设计行为）。
- 读端遇到不认识的 version，正确姿势是明确提示"版本不匹配"并停用读取，而不是猜字段继续解。
- `battery` 字段保留给未来的电量显示（BLE 0x180F 电量服务），填 0–100；在那之前请忽略它。
- magic 值和名字 `Local\BleHR_SM` 不随版本变，变的只有 version 和布局。
