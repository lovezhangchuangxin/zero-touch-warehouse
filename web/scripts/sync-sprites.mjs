// 从 docs/art/sprites 同步精灵与 manifest 到 app public 目录（单一事实源，
// 构建产物不入库）。runbook 见 docs/art/atlas-v4.md。
import { cp, mkdir, readdir, rm } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import path from "node:path";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const src = path.join(repoRoot, "docs/art/sprites");
const dest = path.join(repoRoot, "web/apps/game/public/sprites");

const entries = await readdir(src);
const files = entries.filter((f) => f.endsWith(".png") || f === "manifest.json");
if (files.filter((f) => f.endsWith(".png")).length < 60) {
  throw new Error(`精灵源目录异常：${src} 仅 ${files.length} 个文件`);
}
await rm(dest, { recursive: true, force: true });
await mkdir(dest, { recursive: true });
for (const f of files) {
  await cp(path.join(src, f), path.join(dest, f));
}
console.log(`synced ${files.length} files -> web/apps/game/public/sprites`);
