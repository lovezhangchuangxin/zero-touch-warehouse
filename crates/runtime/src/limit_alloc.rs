//! 堆限额分配器：std::alloc + 头部精确记账，限额由本层执行。
//!
//! 为什么限额必须在这里执行（而不是 JS_SetMemoryLimit）：quickjs 的
//! JS_ThrowOutOfMemory 带 in_out_of_memory 重入保护——堆极限恰好落在
//! 错误对象自身构造分配上时，它返回**不带异常值的裸 JS_EXCEPTION**
//! （quickjs.c:8553）。此时"分配因限额被拒"这一事实只存在于拒绝发生
//! 的那一刻，事后无论读异常、读占用还是读峰值都不可恢复（无异常的
//! 展开照样释放帧内局部变量；quickjs 每 10000 个回边才轮询一次中断，
//! 大块分配循环根本触发不了采样）。本分配器在拒绝时置 limit_hit 标志，
//! 故障分类据此得到确定性判据，不再依赖消息文本或事后读数。
//!
//! 记账用"请求字节数 + 头部"精确累计（usable_size 同口径），不经过
//! libc 的 malloc_usable_size 舍入——quickjs 对大块的计账正是走 libc
//! （quickjs.c:1889），不同 macOS 构建舍入不同，曾使同一测试在
//! GitHub runner 上确定性失败而在本地永不复现。精确记账让限额判定
//! 跨平台逐字节一致。
//!
//! 结构参考 rquickjs 自带的 RustAllocator（同款 Header 方案）。

use std::alloc::{Layout, alloc, alloc_zeroed, dealloc, realloc as sys_realloc};
use std::ptr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use rquickjs::allocator::Allocator;

/// quickjs 单笔最大分配 u64，全部块按其对齐。
const ALLOC_ALIGN: usize = std::mem::align_of::<u64>();
const HEADER_SIZE: usize = if std::mem::size_of::<usize>() < ALLOC_ALIGN {
    ALLOC_ALIGN
} else {
    std::mem::size_of::<usize>()
};

#[repr(C)]
struct Header {
    /// 舍入到 ALLOC_ALIGN 后的用户区大小（usable_size 同值）。
    size: usize,
}

/// 分配器状态镜像（Arc 共享给 Env 与故障分类；权威计数在分配器内部）。
#[derive(Debug, Default, Clone)]
pub struct AllocState {
    /// 当前生效字节（含头部）。
    pub total: Arc<AtomicU64>,
    /// 自上次复位起发生过"因限额拒绝分配"。
    pub limit_hit: Arc<AtomicBool>,
}

/// 带限额的 Rust 全局分配器：限额拒绝置标志并返回 NULL（quickjs 随后
/// 走自己的 OOM 路径）；系统分配失败同样返回 NULL 但不置标志——只有
/// 限额拒绝才是可确证的环境级 OOM。
pub struct LimitAllocator {
    limit: usize,
    total: usize,
    state: AllocState,
}

#[inline]
fn round_size(size: usize) -> usize {
    size.div_ceil(ALLOC_ALIGN) * ALLOC_ALIGN
}

impl LimitAllocator {
    pub fn new(limit: usize) -> (Self, AllocState) {
        let state = AllocState::default();
        (
            Self {
                limit,
                total: 0,
                state: state.clone(),
            },
            state,
        )
    }

    fn mirror(&self) {
        self.state.total.store(self.total as u64, Ordering::Relaxed);
    }

    /// 限额检查：放不下则置标志返回 true。
    fn over_limit(&mut self, alloc_size: usize) -> bool {
        match self.total.checked_add(alloc_size) {
            Some(t) if t <= self.limit => false,
            _ => {
                self.state.limit_hit.store(true, Ordering::SeqCst);
                true
            }
        }
    }
}

unsafe impl Allocator for LimitAllocator {
    fn calloc(&mut self, count: usize, size: usize) -> *mut u8 {
        if count == 0 || size == 0 {
            return ptr::null_mut();
        }
        let Some(user) = count.checked_mul(size) else {
            return ptr::null_mut();
        };
        let user = round_size(user);
        let Some(alloc_size) = HEADER_SIZE.checked_add(user) else {
            return ptr::null_mut();
        };
        if self.over_limit(alloc_size) {
            return ptr::null_mut();
        }
        let Ok(layout) = Layout::from_size_align(alloc_size, ALLOC_ALIGN) else {
            return ptr::null_mut();
        };
        let p = unsafe { alloc_zeroed(layout) };
        if p.is_null() {
            return p;
        }
        unsafe {
            p.cast::<Header>().write(Header { size: user });
            self.total += alloc_size;
            self.mirror();
            p.add(HEADER_SIZE)
        }
    }

    fn alloc(&mut self, size: usize) -> *mut u8 {
        let user = round_size(size);
        let Some(alloc_size) = HEADER_SIZE.checked_add(user) else {
            return ptr::null_mut();
        };
        if self.over_limit(alloc_size) {
            return ptr::null_mut();
        }
        let Ok(layout) = Layout::from_size_align(alloc_size, ALLOC_ALIGN) else {
            return ptr::null_mut();
        };
        let p = unsafe { alloc(layout) };
        if p.is_null() {
            return p;
        }
        unsafe {
            p.cast::<Header>().write(Header { size: user });
            self.total += alloc_size;
            self.mirror();
            p.add(HEADER_SIZE)
        }
    }

    unsafe fn dealloc(&mut self, ptr: *mut u8) {
        if ptr.is_null() {
            return;
        }
        unsafe {
            let base = ptr.sub(HEADER_SIZE);
            let user = base.cast::<Header>().read().size;
            let alloc_size = user + HEADER_SIZE;
            let layout = Layout::from_size_align_unchecked(alloc_size, ALLOC_ALIGN);
            dealloc(base, layout);
            self.total = self.total.saturating_sub(alloc_size);
            self.mirror();
        }
    }

    unsafe fn realloc(&mut self, ptr: *mut u8, new_size: usize) -> *mut u8 {
        let new_user = round_size(new_size);
        let new_alloc = HEADER_SIZE + new_user;
        unsafe {
            let base = ptr.sub(HEADER_SIZE);
            let old_alloc = base.cast::<Header>().read().size + HEADER_SIZE;
            // 只对增长做限额检查；拒绝时原块保持完整（realloc 失败语义）。
            if new_alloc > old_alloc && self.over_limit(new_alloc - old_alloc) {
                return ptr::null_mut();
            }
            let layout = Layout::from_size_align_unchecked(old_alloc, ALLOC_ALIGN);
            let p = sys_realloc(base, layout, new_alloc);
            if p.is_null() {
                return p;
            }
            p.cast::<Header>().write(Header { size: new_user });
            // 本块从 old_alloc 换成 new_alloc：total += (new − old)，饱和防负。
            self.total = self
                .total
                .saturating_add(new_alloc)
                .saturating_sub(old_alloc);
            self.mirror();
            p.add(HEADER_SIZE)
        }
    }

    unsafe fn usable_size(ptr: *mut u8) -> usize
    where
        Self: Sized,
    {
        unsafe { ptr.sub(HEADER_SIZE).cast::<Header>().read().size }
    }
}
