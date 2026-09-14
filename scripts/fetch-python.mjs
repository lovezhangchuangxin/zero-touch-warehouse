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

// 下载与等锁上限：停滞连接（curl 默认无整体超时）与持锁进程卡死都
// 必须有界退出，否则 cargo 表现为无输出挂死。
const CONNECT_TIMEOUT_S = 20;
const DOWNLOAD_MAX_S = 600;
const LOCK_WAIT_MS = 10 * 60 * 1000;

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
if (!SHA256[target]) {
  console.error(`fetch-python: 目标 ${target} 无钉版哈希（支持：${Object.keys(SHA256).join(", ")}）`);
  process.exit(2);
}
if (!import.meta.dirname) {
  console.error("fetch-python: 需要 Node >= 20.11（import.meta.dirname）；项目要求 Node >= 24");
  process.exit(2);
}

const root = path.resolve(import.meta.dirname, "..");
const distRoot = path.join(root, "target", "python");
const install = path.join(distRoot, "install");
const stamp = path.join(distRoot, "stamp.txt");
const configPath = path.join(distRoot, "pyo3-config.txt");
const stampContent = `${TAG} ${PYTHON_VERSION} ${target}`;

function stampMatches() {
  return fs.existsSync(stamp) && fs.readFileSync(stamp, "utf8").trim() === stampContent;
}

if (stampMatches()) {
  console.log(`fetch-python: 已就绪 ${stampContent}（缓存命中）`);
  process.exit(0);
}

const asset = `cpython-${PYTHON_VERSION}+${TAG}-${target}-install_only.tar.gz`;
const url = `https://github.com/astral-sh/python-build-standalone/releases/download/${TAG}/${asset}`;
const archive = path.join(distRoot, asset);

fs.mkdirSync(distRoot, { recursive: true });

// 并发调用（如两个终端同时 just python-dist）用锁文件保证只下载/解压
// 一次。锁内容 "PID ISO时间"：PID 探测在 PID 复用/内容异常（空文件、
// 非数字）时须判死可抢，等锁上限兜底；持锁进程退出后锁可被抢回。
const lock = path.join(distRoot, ".lock");
const lockDeadline = Date.now() + LOCK_WAIT_MS;
let lockAcquired = false;
class LockTimeout extends Error {}
try {
  while (true) {
    try {
      fs.writeFileSync(lock, `${process.pid} ${new Date().toISOString()}`, { flag: "wx" });
      lockAcquired = true;
      break;
    } catch (e) {
      if (e.code !== "EEXIST") throw e;
      let alive = false;
      try {
        const pid = Number(fs.readFileSync(lock, "utf8").split(" ")[0]);
        // 非整数/非正 PID（空文件、垃圾内容）一律判死：process.kill(0, 0)
        // 探测的是本进程组，恒"存活"。
        if (Number.isInteger(pid) && pid > 0) {
          process.kill(pid, 0);
          alive = true;
        }
      } catch {
        /* 持有进程已退出：直接抢 */
      }
      if (!alive) {
        fs.rmSync(lock, { force: true });
        continue;
      }
      if (Date.now() > lockDeadline) {
        throw new LockTimeout(
          `等锁超时（${LOCK_WAIT_MS / 60000} 分钟）——持锁进程可能卡死（合法持锁上限约 36 分钟：慢网下载重试），确认后删除 ${lock} 重试`,
        );
      }
      Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 200);
    }
  }
} catch (e) {
  if (e instanceof LockTimeout) throw e;
  // 锁机制失效（如杀软瞬时占用锁文件）：退化为无锁执行并警示——
  // 互斥可能被击穿，但 stamp 复查与 sha256 仍保证最终一致性。
  console.error(`fetch-python: 锁不可用（${e.message}），退化为无锁执行`);
}

function main() {
  // 拿到锁后复查 stamp：锁外检查通过的并发输家在此退出——否则会重复
  // 下载 30-60MB，且 rmSync(install) 可能删掉赢家正在链接的 libpython。
  if (stampMatches()) {
    console.log(`fetch-python: 已就绪 ${stampContent}（并发缓存命中）`);
    return;
  }
  if (fs.existsSync(archive)) {
    fs.rmSync(archive);
  }
  console.log(`fetch-python: 下载 ${url}`);
  for (let attempt = 1; attempt <= 3; attempt++) {
    try {
      execFileSync(
        process.platform === "win32" ? "curl.exe" : "curl",
        [
          "-fL",
          "--retry",
          "3",
          "--connect-timeout",
          String(CONNECT_TIMEOUT_S),
          "--max-time",
          String(DOWNLOAD_MAX_S),
          "-o",
          archive,
          url,
        ],
        { stdio: "inherit", timeout: (DOWNLOAD_MAX_S + 60) * 1000 },
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
    // throw 而非 process.exit：finally 得以清锁。
    throw new Error(
      `sha256 不符\n  期望 ${expect}\n  实际 ${got}\n  发行物可能被篡改或上游重传，请核实后更新钉版哈希。`,
    );
  }
  fs.rmSync(install, { recursive: true, force: true });
  console.log("fetch-python: 校验通过，解压…");
  const tmpExtract = path.join(distRoot, ".extract");
  fs.rmSync(tmpExtract, { recursive: true, force: true });
  fs.mkdirSync(tmpExtract, { recursive: true });
  // 相对路径 + cwd：Git bash 的 PATH 里 GNU tar 遮蔽系统 bsdtar，会把
  // "D:\..." 的冒号解析为远程主机（"Cannot connect to D"）；相对路径
  // 对 GNU tar 与 bsdtar 都无歧义。
  execFileSync("tar", ["-xzf", asset, "-C", ".extract"], {
    cwd: distRoot,
    stdio: "inherit",
    timeout: 180_000,
  });
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
}

try {
  main();
} finally {
  // 只清自己的锁：无锁裸奔（锁机制失效）时删掉别人的锁会击穿互斥——
  // 第三个进程即可与持锁者并发。
  if (lockAcquired) {
    fs.rmSync(lock, { force: true });
  }
}
