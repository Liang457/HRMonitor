// src/shared_mem.rs —— 读中立共享内存 Local\BleHR_SM（64 字节 HrSharedData）。
//
// 布局必须和 common/hr_shared.h 一致（pack(4)）：manager 只是又一个读端，
// 和两个插件地位相同。读端校验 magic + version，不对就当没数据。
// 无锁写带来的撕裂在这里同样可接受：下一个刷新周期（默认 1 秒）自愈。
use crate::win;
use std::ffi::c_void;

/// common/hr_shared.h 里的 HRSM_NAME
const SM_NAME: &str = "Local\\BleHR_SM";
const SM_MAGIC: u32 = 0x4D53_5248; // 'HRSM'
const SM_VERSION: u32 = 1;
const SM_SIZE: usize = 64;

// HrStatus 枚举（common/hr_shared.h）。NODATA 目前没有消费方，但它是协议的一部分，
// 留着和 C++ 侧对照。
#[allow(dead_code)]
pub const HRS_NODATA: u32 = 0;
pub const HRS_OK: u32 = 1;
pub const HRS_CONNECTING: u32 = 2;
pub const HRS_TIMEOUT: u32 = 3;

// pack(4) 布局偏移：magic 0、version 4、bpm 8、tick_ms 12、status 20、battery 24、name 28..64
const OFF_TICK: usize = 12;
const OFF_STATUS: usize = 20;
const OFF_NAME: usize = 28;

#[derive(Clone, Debug)]
pub struct Snapshot {
    pub bpm: i32,      // 原始值，-1 = 无效
    pub tick_ms: u64,  // 采样时刻（GetTickCount64 时基）
    pub status: u32,   // HRS_*
    pub device: String,
}

impl Snapshot {
    /// 对外展示的心率：无效/超时 → -1（和 HrEffectiveBpm 同逻辑）。
    pub fn effective_bpm(&self, now_ms: u64, timeout_ms: u64) -> i32 {
        if self.status == HRS_TIMEOUT {
            return -1;
        }
        if self.bpm < 0 {
            return -1;
        }
        if now_ms > self.tick_ms && now_ms - self.tick_ms > timeout_ms {
            return -1;
        }
        self.bpm
    }
}

/// 共享内存读端。Open 一次长期持有；映射不存在（daemon 还没起过）时每次 read
/// 都会重试打开。线程安全：整把锁在调用方（状态轮询线程独占）。
pub struct Reader {
    map: win::Handle,
    view: *const u8,
}

// view 指向的共享内存只要句柄开着就有效；我们只在持有 &mut self 时用它
unsafe impl Send for Reader {}

impl Reader {
    pub fn new() -> Reader {
        Reader { map: std::ptr::null_mut(), view: std::ptr::null() }
    }

    fn ensure_open(&mut self) -> bool {
        if !self.view.is_null() {
            return true;
        }
        if self.map.is_null() {
            let name = win::wide(SM_NAME);
            self.map = unsafe { win::OpenFileMappingW(0x0004, 0, name.as_ptr()) }; // FILE_MAP_READ
            if self.map.is_null() {
                return false;
            }
        }
        let view = unsafe { win::MapViewOfFile(self.map, 0x0004, 0, 0, SM_SIZE) } as *const u8;
        if view.is_null() {
            return false;
        }
        self.view = view;
        true
    }

    /// 读一条记录。None = 映射不存在 / magic 或版本不认识。
    pub fn read(&mut self) -> Option<Snapshot> {
        if !self.ensure_open() {
            return None;
        }
        let b = unsafe { std::slice::from_raw_parts(self.view, SM_SIZE) };
        let magic = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
        let version = u32::from_le_bytes([b[4], b[5], b[6], b[7]]);
        if magic != SM_MAGIC || version != SM_VERSION {
            return None;
        }
        let bpm = i32::from_le_bytes([b[8], b[9], b[10], b[11]]);
        let mut tick_bytes = [0u8; 8];
        tick_bytes.copy_from_slice(&b[OFF_TICK..OFF_TICK + 8]); // 8 字节按字节拷，管它对齐
        let tick_ms = u64::from_le_bytes(tick_bytes);
        let status = u32::from_le_bytes([
            b[OFF_STATUS],
            b[OFF_STATUS + 1],
            b[OFF_STATUS + 2],
            b[OFF_STATUS + 3],
        ]);
        let name_bytes = &b[OFF_NAME..SM_SIZE];
        let name_len = name_bytes.iter().position(|&c| c == 0).unwrap_or(name_bytes.len());
        let device = String::from_utf8_lossy(&name_bytes[..name_len]).into_owned();

        Some(Snapshot { bpm, tick_ms, status, device })
    }
}

impl Drop for Reader {
    fn drop(&mut self) {
        unsafe {
            if !self.view.is_null() {
                win::UnmapViewOfFile(self.view as *const c_void);
            }
            if !self.map.is_null() {
                win::CloseHandle(self.map);
            }
        }
    }
}

pub fn now_ms() -> u64 {
    unsafe { win::GetTickCount64() }
}
