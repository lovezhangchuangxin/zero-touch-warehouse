//! 玩家多文件模块（ESM）加载：内存文件集的归一化与装载。
//!
//! 机制经宿主外探针验证（records 探针 S1–S12）：
//! - `Runtime::set_loader` 注册内存 Resolver/Loader，quickjs-ng 未注册
//!   loader 时无文件系统回退（import 直接 ReferenceError），文件/网络
//!   面保持全封。
//! - 模块体运行错误不作为 Err 从 `Module::eval` 抛出，而是让 eval 返回
//!   的 Promise 变 Rejected；链接期错误（说明符非法/模块缺失）才是
//!   Err + pending 异常——宿主 run_player 按此分流收割。
//! - 动态 `import()` 与顶层 await 的续体走微任务队列，须在 `ctx.with`
//!   块外 drain（with 借用期间 execute_pending_job 会 RefCell 重入）。

use std::collections::BTreeMap;

use rquickjs::loader::{ImportAttributes, Loader, Resolver};
use rquickjs::{Ctx, Error, Module, Persistent, Value};

/// 玩家文件名校验：模块名即文件名（如 `main.js`、`lib/geo.js`），只允许
/// 相对形态的安全路径 + `.js` 扩展。返回可读错误供 init 故障上报。
pub fn validate_files(files: &BTreeMap<String, String>) -> Result<(), String> {
    for name in files.keys() {
        if name.is_empty() {
            return Err("文件名为空".to_string());
        }
        if !name.ends_with(".js") {
            return Err(format!("玩家文件 {name:?} 必须以 .js 结尾"));
        }
        if name.starts_with('/') || name.contains('\\') || name.contains('\0') {
            return Err(format!("玩家文件 {name:?} 含非法路径分量"));
        }
        for seg in name.split('/') {
            if seg.is_empty() || seg == "." || seg == ".." {
                return Err(format!("玩家文件 {name:?} 含非法路径分量"));
            }
        }
    }
    Ok(())
}

/// 归一化：仅接受相对说明符（`./x.js`、`../x.js`），按 base 所在目录
/// 拼接并规范化。裸名（npm 式包名）与越出玩家文件集根目录的 `..` 一律
/// 可读拒绝——玩家文件集之外没有世界。
///
/// 宽严取舍：说明符里的空段（`./a//b.js`）与结尾斜杠被规范化接受，
/// 而文件集键里的同形态会被 validate_files 拒——装载只查内存 map，
/// 规范化后的名字要么命中已校验的键、要么可读报错，无逃逸面。
#[derive(Clone, Copy, Default)]
pub struct PlayerResolver;

impl Resolver for PlayerResolver {
    fn resolve<'js>(
        &mut self,
        _ctx: &Ctx<'js>,
        base: &str,
        name: &str,
        _attributes: Option<ImportAttributes<'js>>,
    ) -> rquickjs::Result<String> {
        if !name.starts_with("./") && !name.starts_with("../") {
            return Err(Error::new_loading_message(
                name,
                format!("仅支持相对说明符（如 \"./lib.js\"），收到 {name:?}；裸模块名与包名不可用"),
            ));
        }
        let mut parts: Vec<&str> = base.split('/').collect();
        parts.pop(); // 去掉 base 文件名，留下目录
        for seg in name.split('/') {
            match seg {
                "" | "." => {}
                ".." => {
                    if parts.pop().is_none() {
                        return Err(Error::new_loading_message(
                            name,
                            "相对说明符越出玩家文件集根目录",
                        ));
                    }
                }
                other => parts.push(other),
            }
        }
        Ok(parts.join("/"))
    }
}

/// 装载：文件集查名 → compile-only 声明。缺失模块给带名字的 loading
/// 错误（rquickjs 把 Err 转成 JS 异常，quickjs 层呈现为
/// ReferenceError: could not load module）。
pub struct PlayerLoader(BTreeMap<String, String>);

impl PlayerLoader {
    pub fn new(files: BTreeMap<String, String>) -> PlayerLoader {
        PlayerLoader(files)
    }
}

impl Loader for PlayerLoader {
    fn load<'js>(
        &mut self,
        ctx: &Ctx<'js>,
        name: &str,
        _attributes: Option<ImportAttributes<'js>>,
    ) -> rquickjs::Result<Module<'js, rquickjs::module::Declared>> {
        match self.0.get(name) {
            Some(src) => Module::declare(ctx.clone(), name.to_string(), src.clone()),
            None => Err(Error::new_loading(name)),
        }
    }
}

/// 未处理 Promise 拒绝的账本：宿主端 rejection tracker 的收集面。
///
/// 配对计数语义：每个 promise 至多产生一次 `rejected`（无 handler 拒绝）
/// 与一次 `handled`（事后挂上 handler）事件，故 `rejected - handled`
/// 恰等于执行结束时仍未处理的 promise 数——与事件交错顺序无关。
///
/// tracker 回调在引擎 promise 机械内部同步执行，绝不能跑玩家 JS：
/// 账本只做 C 级引用接管（`Persistent::save`）与计数，首个未处理
/// reason 保留为持久句柄，文本化推迟到 harvest 的受控点。展示取首个
/// 未处理事件的原因（多个并存时不逐一配对，教学面足够）。
#[derive(Default)]
pub struct RejectLedger {
    rejected: u64,
    handled: u64,
    first_reason: Option<Persistent<Value<'static>>>,
}

impl RejectLedger {
    pub fn note_unhandled(&mut self, reason: Persistent<Value<'static>>) {
        self.rejected += 1;
        if self.first_reason.is_none() {
            self.first_reason = Some(reason);
        }
    }

    pub fn note_handled(&mut self) {
        self.handled += 1;
    }

    pub fn reset(&mut self) {
        self.rejected = 0;
        self.handled = 0;
        self.first_reason = None;
    }

    pub fn unhandled_count(&self) -> u64 {
        self.rejected.saturating_sub(self.handled)
    }

    /// 取出首个未处理原因的持久句柄（收割时文本化，随后即弃）。
    pub fn take_first_reason(&mut self) -> Option<Persistent<Value<'static>>> {
        self.first_reason.take()
    }
}
