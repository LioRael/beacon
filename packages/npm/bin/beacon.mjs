#!/usr/bin/env node

import fs from "node:fs";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath, pathToFileURL } from "node:url";

const packageRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

export function resolveBinaryPath({ platform, arch, packageRoot: root }) {
  if (platform !== "darwin" || arch !== "arm64") {
    throw new Error(
      `@liorael/beacon supports Apple Silicon macOS 15 or newer; received ${platform}/${arch}`,
    );
  }
  return path.join(root, "vendor", "beacon");
}

export function launch({ binaryPath, args, exists, spawn, kill, pid }) {
  if (!exists(binaryPath)) {
    throw new Error(
      `@liorael/beacon does not contain the Beacon binary at ${binaryPath}; reinstall the package`,
    );
  }

  const result = spawn(binaryPath, args, { stdio: "inherit" });
  if (result.error) {
    throw result.error;
  }
  if (result.signal) {
    kill(pid, result.signal);
    return 1;
  }
  return result.status ?? 1;
}

function main() {
  try {
    const binaryPath = resolveBinaryPath({
      platform: process.platform,
      arch: process.arch,
      packageRoot,
    });
    process.exitCode = launch({
      binaryPath,
      args: process.argv.slice(2),
      exists: fs.existsSync,
      spawn: spawnSync,
      kill: process.kill,
      pid: process.pid,
    });
  } catch (error) {
    console.error(`beacon: ${error.message}`);
    process.exitCode = 1;
  }
}

const entryPoint = process.argv[1] ? fs.realpathSync(process.argv[1]) : null;
if (entryPoint && import.meta.url === pathToFileURL(entryPoint).href) {
  main();
}
