#!/usr/bin/env node

import fs from "node:fs";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

export function stageNpmPackage({ binary, output, version, repositoryRoot }) {
  if (!/^\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?$/.test(version)) {
    throw new Error(`invalid version: ${version}`);
  }
  if (!fs.statSync(binary).isFile()) {
    throw new Error(`Beacon binary not found: ${binary}`);
  }

  const source = path.join(repositoryRoot, "packages", "npm");
  fs.rmSync(output, { recursive: true, force: true });
  fs.mkdirSync(path.join(output, "vendor"), { recursive: true });
  fs.cpSync(path.join(source, "bin"), path.join(output, "bin"), { recursive: true });
  fs.copyFileSync(path.join(source, "package.json"), path.join(output, "package.json"));
  fs.copyFileSync(path.join(repositoryRoot, "LICENSE"), path.join(output, "LICENSE"));
  fs.copyFileSync(binary, path.join(output, "vendor", "beacon"));
  fs.chmodSync(path.join(output, "vendor", "beacon"), 0o755);

  const packagePath = path.join(output, "package.json");
  const metadata = JSON.parse(fs.readFileSync(packagePath, "utf8"));
  metadata.version = version;
  fs.writeFileSync(packagePath, `${JSON.stringify(metadata, null, 2)}\n`);
}

function main() {
  const [, , binary, output, version] = process.argv;
  if (!binary || !output || !version) {
    console.error("usage: stage-npm-package.mjs <binary> <output> <version>");
    process.exitCode = 2;
    return;
  }
  try {
    const repositoryRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
    stageNpmPackage({ binary, output, version, repositoryRoot });
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main();
}

