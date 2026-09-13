//! 宿主二进制路径解析（docs/architecture/01：宿主进程为 ztw-runtime 的
//! `ztw-host-js`，可随时终止重启）。
//!
//! 查找顺序：`ZTW_HOST_BIN` 环境变量 → 可执行文件同目录（dev 时与
//! tauri / cargo 产物同在 target/debug）→ 可执行文件上两级（集成测试的
//! deps 目录）→ 编译期 workspace target 目录。找不到时返回 Err；此时
//! 世界线程仍可启动，加载代码时报 SPAWN_FAILED 故障（面板可见、可解释）。

use std::path::PathBuf;

pub fn host_bin_name() -> &'static str {
    if cfg!(windows) {
        "ztw-host-js.exe"
    } else {
        "ztw-host-js"
    }
}

pub fn resolve_host_bin() -> Result<PathBuf, String> {
    if let Ok(p) = std::env::var("ZTW_HOST_BIN")
        && !p.is_empty()
    {
        return Ok(PathBuf::from(p));
    }
    let name = host_bin_name();
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        let cand = dir.join(name);
        if cand.is_file() {
            return Ok(cand);
        }
        // 集成测试二进制位于 target/debug/deps：上一级即 target/debug。
        if let Some(up) = dir.parent() {
            let cand = up.join(name);
            if cand.is_file() {
                return Ok(cand);
            }
        }
    }
    for profile in ["debug", "release"] {
        let cand = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target")
            .join(profile)
            .join(name);
        if cand.is_file() {
            return Ok(cand);
        }
    }
    Err(format!(
        "找不到宿主二进制 {name}：先 cargo build -p ztw-runtime，或设置 ZTW_HOST_BIN"
    ))
}

/// 解析失败时给出可启动的占位路径（load_code 将报 SPAWN_FAILED 并带路径）。
pub fn resolve_host_bin_or_default() -> PathBuf {
    resolve_host_bin().unwrap_or_else(|_| {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/debug")
            .join(host_bin_name())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_under_cargo_test() {
        // cargo test --workspace 会先构建全部成员二进制；此处至少不 Err
        // （Err 时信息可读，便于诊断 CI 环境问题）。
        match resolve_host_bin() {
            Ok(p) => assert!(p.is_file(), "{p:?} 应存在"),
            Err(msg) => panic!("{msg}"),
        }
    }
}
