//! ztw-host-py 构建脚本：校验 vendored CPython 发行物并在链接期接线。
//!
//! 发行物由 scripts/fetch-python.mjs 下载（钉版本 + sha256），
//! 位于 target/python/install；pyo3 的发现配置在
//! target/python/pyo3-config.txt（.cargo/config.toml 固定指向）。
//! 本脚本不负责下载——pyo3 的 build script 先于本脚本运行，缺失时
//! 那边会先报错；此处给出可读指引。

use std::path::PathBuf;

fn main() {
    let target = std::env::var("TARGET").unwrap_or_default();
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let root = manifest
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root")
        .to_path_buf();
    let install = std::env::var("ZTW_PY_DIST")
        .map(PathBuf::from)
        .unwrap_or_else(|_| root.join("target/python/install"));

    let marker = install.join("lib/python3.13");
    let marker_win = install.join("Lib/os.py");
    if !marker.exists() && !marker_win.exists() {
        panic!(
            "vendored CPython 发行物缺失（{}）。\n\
             先运行：just python-dist\n\
             或：node scripts/fetch-python.mjs\n\
             （下载钉版本的 python-build-standalone，约 25-47MB，只需一次）",
            install.display()
        );
    }
    println!("cargo:rerun-if-env-changed=ZTW_PY_DIST");

    // 运行期 PYTHONHOME：解释器以发行物前缀为家定位标准库。
    println!("cargo:rustc-env=ZTW_PY_HOME={}", install.display());

    if target.contains("apple-darwin") {
        // libpython 的 install name 是 @rpath/libpython3.13.dylib。
        let lib_dir = install.join("lib");
        println!("cargo:rustc-link-arg=-Wl,-rpath,{}", lib_dir.display());
    } else if target.contains("windows") {
        // Windows 无 rpath：把 python DLL 复制到 target 目录（ztw-host-py.exe
        // 所在地），DLL 搜索顺序首先命中。
        let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
        let target_dir = out_dir
            .ancestors()
            .nth(3)
            .expect("OUT_DIR 位于 target/<profile>/build/<pkg>/out")
            .to_path_buf();
        for entry in std::fs::read_dir(&install)
            .unwrap_or_else(|e| panic!("读取发行物目录失败 {}: {e}", install.display()))
        {
            let entry = entry.expect("读目录项");
            let name = entry.file_name().to_string_lossy().to_string();
            if name.ends_with(".dll") {
                let dest = target_dir.join(&name);
                if !dest.exists() {
                    std::fs::copy(entry.path(), &dest)
                        .unwrap_or_else(|e| panic!("复制 {name} 到 {}: {e}", target_dir.display()));
                }
                println!("cargo:rerun-if-changed={}", entry.path().display());
            }
        }
    }
}
