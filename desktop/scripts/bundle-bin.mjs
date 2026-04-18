// desktop/scripts/bundle-bin.mjs
//
// Build the three dutopia CLIs (duscan, dusum, dudb) from ../rs and copy
// them into src-tauri/bin/ with the target-triple suffix that Tauri's
// `externalBin` config expects (e.g. duscan-x86_64-pc-windows-msvc.exe).
//
// Run before `tauri dev` / `tauri build`.

import { spawnSync } from "node:child_process";
import { copyFileSync, mkdirSync, statSync } from "node:fs";
import { join, resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = dirname(fileURLToPath(import.meta.url));
const desktopRoot = resolve(__dirname, "..");
const rsRoot = resolve(desktopRoot, "..", "rs");
const binOut = join(desktopRoot, "src-tauri", "bin");

// Resolve the host target triple via rustc.
const tripleProc = spawnSync("rustc", ["-vV"], { encoding: "utf8" });
if (tripleProc.status !== 0) {
  throw new Error("rustc -vV failed — is Rust installed?");
}
const triple = tripleProc.stdout
  .split("\n")
  .find((l) => l.startsWith("host: "))
  .slice("host: ".length)
  .trim();

const exeSuffix = triple.includes("windows") ? ".exe" : "";

function isFile(p) {
  try { return statSync(p).isFile(); } catch { return false; }
}

// Prefer the workspace-aware cargo.bat on Windows if present (sets up MSVC env).
const cargoWrapper =
  process.platform === "win32" && isFile("c:/Dev/cargo.bat")
    ? ["cmd", "/C", "c:\\Dev\\cargo.bat"]
    : ["cargo"];

console.log(`Building duscan/dusum/dudb for ${triple}…`);
const args = ["build", "--release", "--bin", "duscan", "--bin", "dusum", "--bin", "dudb"];
const res = spawnSync(cargoWrapper[0], [...cargoWrapper.slice(1), ...args], {
  cwd: rsRoot,
  stdio: "inherit",
});
if (res.status !== 0) {
  process.exit(res.status ?? 1);
}

mkdirSync(binOut, { recursive: true });

for (const name of ["duscan", "dusum", "dudb"]) {
  const src = join(rsRoot, "target", "release", `${name}${exeSuffix}`);
  const dst = join(binOut, `${name}-${triple}${exeSuffix}`);
  copyFileSync(src, dst);
  console.log(`  ${src}  →  ${dst}`);
}

console.log("done.");
