#!/usr/bin/env node
// 获取钉版本的 python-build-standalone 发行物（A1 Python 宿主的内嵌
// CPython，docs/architecture/03「Python 宿主」）。
//
// 用法：node scripts/fetch-python.mjs [--target <rust-triple>]
//   省略 --target 时按本机平台推断。产物解压到 target/python/install，
//   并写 target/python/pyo3-config.txt（绝对路径，供 PYO3_CONFIG_FILE
//   指向；.cargo/config.toml 已固定该路径）。
//
// 版本钉死：升级 = 改这里的 TAG / PYTHON_VERSION / SHA256 三处并全量
// 重测（docs/architecture/07：CPython 版本随构建锁定）。
import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import os from "node:os";

const TAG = "20260901";
const PYTHON_VERSION = "3.13.15";
const SHA256 = {
  "aarch64-apple-darwin":
    "b9054a9d3d54f4cb5573d44907fddb29874b08909bde73f29f2868cf872223ee",
  "x86_64-apple-darwin":
    "49f0d97f506b855eed60b74a8ac138595c5b39799a6aa5e0d7ca8abe1019a4d4",
  "x86_64-pc-windows-msvc":
    "9bcc038a0bf180612ed56dec93d4977d035e80b8d9320ef51a38c287baf134b7",
};

function detectTarget() {
  if (process.argv.includes("--target")) {
    const i = process.argv.indexOf("--target");
    return process.argv[i + 1];
  }
  const plat = `${process.platform}-${process.arch}`;
  const map = {
    "darwin-arm64": "aarch64-apple-darwin",
    "darwin-x64": "x86_64-apple-darwin",
    "win32-x64": "x86_64-pc-windows-msvc",
  };
  if (!map[plat]) {
    console.error(`fetch-python: 不支持的平台 ${plat}（目标平台见 docs/architecture/07）`);
    process.exit(2);
  }
  return map[plat];
}

const target = detectTarget();
const root = path.resolve(import.meta.dirname, "..");
const distRoot = path.join(root, "target", "python");
const install = path.join(distRoot, "install");
const stamp = path.join(distRoot, "stamp.txt");
const configPath = path.join(distRoot, "pyo3-config.txt");
const stampContent = `${TAG} ${PYTHON_VERSION} ${target}`;

if (fs.existsSync(stamp) && fs.readFileSync(stamp, "utf8").trim() === stampContent) {
  console.log(`fetch-python: 已就绪 ${stampContent}（缓存命中）`);
  process.exit(0);
}

const asset = `cpython-${PYTHON_VERSION}+${TAG}-${target}-install_only.tar.gz`;
const url = `https://github.com/astral-sh/python-build-standalone/releases/download/${TAG}/${asset}`;
const archive = path.join(distRoot, asset);

fs.mkdirSync(distRoot, { recursive: true });

// cargo 可能并行调用本脚本；用锁文件保证只下载/解压一次。
const lock = path.join(distRoot, ".lock");
try {
  while (true) {
    try {
      fs.writeFileSync(lock, String(process.pid), { flag: "wx" });
      break;
    } catch (e) {
      if (e.code !== "EEXIST") throw e;
      // 陈旧锁（持有进程已退出）直接抢。
      try {
        const pid = Number(fs.readFileSync(lock, "utf8"));
        process.kill(pid, 0);
      } catch {
        fs.rmSync(lock, { force: true });
        continue;
      }
      Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 200);
    }
  }
} catch {
  /* 单进程场景不需要 */
}

try {
  if (fs.existsSync(archive)) {
    fs.rmSync(archive);
  }
  console.log(`fetch-python: 下载 ${url}`);
  for (let attempt = 1; attempt <= 3; attempt++) {
    try {
      execFileSync(
        process.platform === "win32" ? "curl.exe" : "curl",
        ["-fL", "--retry", "3", "-o", archive, url],
        { stdio: "inherit" },
      );
      break;
    } catch (e) {
      if (attempt === 3) throw e;
      console.error(`fetch-python: 下载失败，重试 ${attempt}/3`);
    }
  }
  const expect = SHA256[target];
  const got = createHash("sha256").update(fs.readFileSync(archive)).digest("hex");
  if (got !== expect) {
    console.error(`fetch-python: sha256 不符\n  期望 ${expect}\n  实际 ${got}\n  发行物可能被篡改或上游重传，请核实后更新钉版哈希。`);
    process.exit(1);
  }
  fs.rmSync(install, { recursive: true, force: true });
  console.log("fetch-python: 校验通过，解压…");
  const tmpExtract = path.join(distRoot, ".extract");
  fs.rmSync(tmpExtract, { recursive: true, force: true });
  fs.mkdirSync(tmpExtract, { recursive: true });
  // Windows 10+ 与 macOS 都自带 bsdtar，统一 -xzf。
  execFileSync("tar", ["-xzf", archive, "-C", tmpExtract], { stdio: "inherit" });
  // 压缩包顶层目录固定为 python/。
  fs.renameSync(path.join(tmpExtract, "python"), install);
  fs.rmSync(tmpExtract, { recursive: true, force: true });

  const win = target.includes("windows");
  const libDir = win ? path.join(install, "libs") : path.join(install, "lib");
  const executable = win
    ? path.join(install, "python.exe")
    : path.join(install, "bin", "python3.13");
  const libName = win ? "python313" : "python3.13";
  const config = [
    "implementation=CPython",
    `version=${PYTHON_VERSION.split(".").slice(0, 2).join(".")}`,
    "shared=true",
    "abi3=false",
    `lib_name=${libName}`,
    `lib_dir=${libDir}`,
    `executable=${executable}`,
    "pointer_width=64",
    "build_flags=",
    "suppress_build_script_link_lines=false",
    "",
  ].join("\n");
  fs.writeFileSync(configPath, config);
  fs.writeFileSync(stamp, `${stampContent}\n`);
  console.log(`fetch-python: 完成 ${stampContent}`);
  console.log(`fetch-python: ${install}`);
} finally {
  fs.rmSync(lock, { force: true });
}
