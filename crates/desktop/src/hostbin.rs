//! 宿主二进制路径解析（docs/architecture/01：宿主进程为 runtime 家族的
//! ztw-host-js / ztw-host-py，可随时终止重启；语言切换重启宿主）。
//!
//! 查找顺序：语言专属环境变量（ZTW_HOST_BIN / ZTW_HOST_PY_BIN）→
//! 可执行文件同目录（dev 时与 tauri / cargo 产物同在 target/debug）→
//! 可执行文件上两级（集成测试的 deps 目录）→ 编译期 workspace target
//! 目录。找不到时返回占位路径；世界线程仍可启动，加载代码时报
//! SPAWN_FAILED 故障（面板可见、可解释）。

use std::path::PathBuf;

/// 玩家代码语言（docs/architecture/03：同一存档同一时间只运行一种语言）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Language {
    Js,
    Py,
}

impl Language {
    pub fn as_str(&self) -> &'static str {
        match self {
            Language::Js => "js",
            Language::Py => "py",
        }
    }

    /// 前端命令参数解析（未知值显式报错，不静默纠正）。
    pub fn parse(s: &str) -> Result<Language, String> {
        match s {
            "js" => Ok(Language::Js),
            "py" => Ok(Language::Py),
            other => Err(format!("未知语言 {other}（可用：js / py）")),
        }
    }

    fn bin_name(&self) -> &'static str {
        match (cfg!(windows), self) {
            (true, Language::Js) => "ztw-host-js.exe",
            (true, Language::Py) => "ztw-host-py.exe",
            (false, Language::Js) => "ztw-host-js",
            (false, Language::Py) => "ztw-host-py",
        }
    }

    fn env_override(&self) -> &'static str {
        match self {
            Language::Js => "ZTW_HOST_BIN",
            Language::Py => "ZTW_HOST_PY_BIN",
        }
    }
}

/// 两种语言的宿主二进制（语言切换时直接换 bin 重建会话）。
#[derive(Debug, Clone)]
pub struct HostBins {
    pub js: PathBuf,
    pub py: PathBuf,
}

fn resolve_one(lang: Language) -> PathBuf {
    let name = lang.bin_name();
    if let Ok(p) = std::env::var(lang.env_override())
        && !p.is_empty()
    {
        return PathBuf::from(p);
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        let cand = dir.join(name);
        if cand.is_file() {
            return cand;
        }
        // 集成测试二进制位于 target/debug/deps：上一级即 target/debug。
        if let Some(up) = dir.parent() {
            let cand = up.join(name);
            if cand.is_file() {
                return cand;
            }
        }
    }
    for profile in ["debug", "release"] {
        let cand = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target")
            .join(profile)
            .join(name);
        if cand.is_file() {
            return cand;
        }
    }
    // 占位：load_code 将报 SPAWN_FAILED 并带可读路径。
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/debug")
        .join(name)
}

pub fn resolve_host_bins() -> HostBins {
    HostBins {
        js: resolve_one(Language::Js),
        py: resolve_one(Language::Py),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_under_cargo_test() {
        // cargo test --workspace 会先构建全部成员二进制；此处 js/py 均应
        // 解析到真实文件（占位路径时 CI 集成测试会以 SPAWN_FAILED 暴露）。
        let bins = resolve_host_bins();
        assert!(bins.js.is_file(), "{:?} 应存在", bins.js);
        assert!(bins.py.is_file(), "{:?} 应存在", bins.py);
    }
}
