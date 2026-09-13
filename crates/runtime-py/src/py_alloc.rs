//! Python 配额分配器（docs/architecture/03「Python 宿主」资源限制）。
//!
//! 覆盖 RAW / MEM / OBJ 三个分配域（OBJ 域被接管即绕过 pymalloc——分配
//! 变慢，但配额覆盖包括解释器自身、标准库缓存与绑定对象在内的全部
//! 分配）。超额分配返回 NULL，玩家代码收到 `MemoryError`（Python 侧归
//! 脚本级，docs 03 故障分级——与 JS OOM 归环境级不同）。
//!
//! 记账方式与 crates/runtime/limit_alloc.rs 同构：每次分配前置 Header
//! 记录请求字节数，free/realloc 按头部归还；计数与标志全部原子，回调
//! 可能来自任意线程。**安装必须在解释器初始化之前**（main 首次 attach
//! 前），之后 CPython 的全部分配经由此处。

use std::ffi::{c_int, c_void};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// 分配头：记录请求字节数（free/realloc 归还计数用）。
/// magic 用于防御非本分配器来源的指针（CPython 保证不会发生，出现即
/// 记账错乱的硬错误——宁可 abort 也不静默污染计数）。
#[repr(C)]
struct Header {
    magic: u64,
    size: u64,
}

const MAGIC: u64 = 0x5A545750_59504D45; // "ZTWPYME"
const HEADER_BYTES: u64 = std::mem::size_of::<Header>() as u64;
const ALIGN: usize = 16;

/// 首次限额拒绝后一次性抬高的诊断余量：给 MemoryError 的构造、堆栈
/// 格式化与异常展开留出分配头寸。玩家最多再挤出这么多字节，有界。
pub const DIAG_RESERVE: u64 = 16 * 1024 * 1024;

pub struct AllocState {
    /// 当前允许的动态上限（首次拒绝后 += DIAG_RESERVE，只加一次）。
    limit: AtomicU64,
    pub total: AtomicU64,
    pub peak: AtomicU64,
    /// 限额拒绝标志（粘性，随执行复位）：MemoryError 分类的互证。
    pub limit_hit: AtomicBool,
    bump_applied: AtomicBool,
}

pub static STATE: AllocState = AllocState {
    limit: AtomicU64::new(0),
    total: AtomicU64::new(0),
    peak: AtomicU64::new(0),
    limit_hit: AtomicBool::new(false),
    bump_applied: AtomicBool::new(false),
};

impl AllocState {
    /// 每执行复位拒绝标志（诊断余量与已用计数不回退——解释器常驻，
    /// 上一执行的分配可能仍被命名空间持有）。
    pub fn reset_for_exec(&'static self) {
        self.limit_hit.store(false, Ordering::SeqCst);
    }

    /// 当前动态上限（含诊断余量）。
    pub fn limit_bytes(&'static self) -> u64 {
        self.limit.load(Ordering::SeqCst)
    }
}

fn try_alloc(size: u64) -> *mut u8 {
    let Some(gross) = size.checked_add(HEADER_BYTES) else {
        STATE.limit_hit.store(true, Ordering::SeqCst);
        return std::ptr::null_mut();
    };
    // CAS 记账：并发下每个成功分配恰好被计数一次。
    let gross = loop {
        let limit = STATE.limit.load(Ordering::Relaxed);
        let cur = STATE.total.load(Ordering::Relaxed);
        if cur.checked_add(gross).is_none_or(|n| n > limit) {
            STATE.limit_hit.store(true, Ordering::SeqCst);
            apply_diag_bump();
            // 余量生效后重试一次比较；仍超限即拒绝。
            if STATE.total.load(Ordering::Relaxed) + gross > STATE.limit.load(Ordering::Relaxed) {
                return std::ptr::null_mut();
            }
            continue;
        }
        if STATE
            .total
            .compare_exchange_weak(cur, cur + gross, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            break gross;
        }
    };
    let layout =
        std::alloc::Layout::from_size_align(gross as usize, ALIGN).expect("配额分配布局合法");
    let base = unsafe { std::alloc::alloc(layout) };
    if base.is_null() {
        // 系统内存不足：回滚计数。
        STATE.total.fetch_sub(gross, Ordering::Relaxed);
        return std::ptr::null_mut();
    }
    unsafe {
        let h = base as *mut Header;
        (*h).magic = MAGIC;
        (*h).size = size;
    }
    let total = STATE.total.load(Ordering::Relaxed);
    let mut peak = STATE.peak.load(Ordering::Relaxed);
    while total > peak {
        match STATE
            .peak
            .compare_exchange_weak(peak, total, Ordering::Relaxed, Ordering::Relaxed)
        {
            Ok(_) => break,
            Err(actual) => peak = actual,
        }
    }
    // 头部之后的用户指针。
    unsafe { base.add(HEADER_BYTES as usize) }
}

fn apply_diag_bump() {
    if !STATE.bump_applied.swap(true, Ordering::SeqCst) {
        STATE.limit.fetch_add(DIAG_RESERVE, Ordering::SeqCst);
    }
}

unsafe fn header_of(ptr: *mut c_void) -> (std::alloc::Layout, u64) {
    let (layout, gross) = unsafe {
        let base = (ptr as *mut u8).sub(HEADER_BYTES as usize);
        let h = base as *mut Header;
        assert_eq!((*h).magic, MAGIC, "配额分配器收到外来指针（记账错乱）");
        let size = (*h).size;
        let gross = size + HEADER_BYTES;
        (
            std::alloc::Layout::from_size_align(gross as usize, ALIGN).expect("布局合法"),
            gross,
        )
    };
    (layout, gross)
}

unsafe fn raw_free(ptr: *mut c_void) {
    if ptr.is_null() {
        return;
    }
    unsafe {
        let (layout, gross) = header_of(ptr);
        std::alloc::dealloc((ptr as *mut u8).sub(HEADER_BYTES as usize), layout);
        STATE.total.fetch_sub(gross, Ordering::Relaxed);
    }
}

// ---------------------------------------------------------------------------
// PyMemAllocatorEx 回调（三域共用）
// ---------------------------------------------------------------------------

extern "C" fn py_malloc(_ctx: *mut c_void, size: usize) -> *mut c_void {
    try_alloc(size as u64) as *mut c_void
}

extern "C" fn py_calloc(_ctx: *mut c_void, nelem: usize, elsize: usize) -> *mut c_void {
    let Some(size) = (nelem as u64).checked_mul(elsize as u64) else {
        return std::ptr::null_mut();
    };
    let p = try_alloc(size);
    if !p.is_null() {
        unsafe { std::ptr::write_bytes(p, 0, size as usize) };
    }
    p as *mut c_void
}

extern "C" fn py_realloc(_ctx: *mut c_void, ptr: *mut c_void, new_size: usize) -> *mut c_void {
    if ptr.is_null() {
        return py_malloc(_ctx, new_size);
    }
    if new_size == 0 {
        unsafe { raw_free(ptr) };
        return std::ptr::null_mut();
    }
    // 分配-拷贝-释放：绕开 realloc 对旧布局的约束；代价一次拷贝，配额
    // 语义（按请求字节记账）保持精确。
    let newp = py_malloc(_ctx, new_size);
    if newp.is_null() {
        return std::ptr::null_mut();
    }
    unsafe {
        let (_, old_gross) = header_of(ptr);
        let old_size = (old_gross - HEADER_BYTES) as usize;
        std::ptr::copy_nonoverlapping(ptr as *const u8, newp as *mut u8, old_size.min(new_size));
        raw_free(ptr);
    }
    newp
}
extern "C" fn py_free(_ctx: *mut c_void, ptr: *mut c_void) {
    unsafe { raw_free(ptr) };
}

struct SyncAllocator(pyo3::ffi::PyMemAllocatorEx);
// ctx 为 null、其余为纯函数指针：结构在安装后只被 CPython 读取，
// 跨线程共享安全。
unsafe impl Sync for SyncAllocator {}

static PY_ALLOCATOR: SyncAllocator = SyncAllocator(pyo3::ffi::PyMemAllocatorEx {
    ctx: std::ptr::null_mut(),
    malloc: Some(py_malloc),
    calloc: Some(py_calloc),
    realloc: Some(py_realloc),
    free: Some(py_free),
});

/// 安装三域配额分配器。**必须在解释器初始化前调用**（CPython 拷贝结构
/// 内容，安装后结构须永久有效——static 保证）。
pub fn install(limit_bytes: u64) {
    STATE.limit.store(limit_bytes, Ordering::SeqCst);
    STATE.total.store(0, Ordering::SeqCst);
    STATE.peak.store(0, Ordering::SeqCst);
    STATE.limit_hit.store(false, Ordering::SeqCst);
    STATE.bump_applied.store(false, Ordering::SeqCst);
    unsafe {
        // PYTHONMALLOC 环境变量会在解释器初始化期间改写默认分配域选择，
        // 与显式接管互斥——宿主进程环境完全由主进程控制，直接清除。
        std::env::remove_var("PYTHONMALLOC");
        // FFI 形参是 *mut（C 头文件如此），CPython 实际只读取结构；
        // 以 *const 转 *mut 传入。
        let alloc = &PY_ALLOCATOR.0 as *const pyo3::ffi::PyMemAllocatorEx
            as *mut pyo3::ffi::PyMemAllocatorEx;
        pyo3::ffi::PyMem_SetAllocator(pyo3::ffi::PyMemAllocatorDomain::PYMEM_DOMAIN_RAW, alloc);
        pyo3::ffi::PyMem_SetAllocator(pyo3::ffi::PyMemAllocatorDomain::PYMEM_DOMAIN_MEM, alloc);
        // OBJ 域被接管即绕过 pymalloc（docs 03：分配变慢换取全覆盖）。
        pyo3::ffi::PyMem_SetAllocator(pyo3::ffi::PyMemAllocatorDomain::PYMEM_DOMAIN_OBJ, alloc);
    }
}

/// SIGINT 编号：macOS 与 Windows MSVC 一致为 2。
pub const SIGINT: c_int = 2;
