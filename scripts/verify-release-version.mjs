#!/usr/bin/env node

import fs from "node:fs";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

export function verifyReleaseVersion({ tag, cargoVersion, npmVersion }) {
  if (!/^v\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?$/.test(tag)) {
    throw new Error(`invalid release tag: ${tag}`);
  }
  const tagVersion = tag.slice(1);
  if (tagVersion !== cargoVersion || tagVersion !== npmVersion) {
    throw new Error(
      `release tag ${tagVersion} does not match Cargo ${cargoVersion} and npm ${npmVersion}`,
    );
  }
}

function versionsFromRepository(repositoryRoot) {
  const cargo = fs.readFileSync(path.join(repositoryRoot, "Cargo.toml"), "utf8");
  const cargoMatch = cargo.match(/\[workspace\.package\][\s\S]*?version = "([^"]+)"/);
  if (!cargoMatch) {
    throw new Error("workspace package version not found in Cargo.toml");
  }
  const npm = JSON.parse(
    fs.readFileSync(path.join(repositoryRoot, "packages", "npm", "package.json"), "utf8"),
  );
  return { cargoVersion: cargoMatch[1], npmVersion: npm.version };
}

function main() {
  const [, , tag] = process.argv;
  if (!tag) {
    console.error("usage: verify-release-version.mjs <tag>");
    process.exitCode = 2;
    return;
  }
  try {
    const repositoryRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
    verifyReleaseVersion({ tag, ...versionsFromRepository(repositoryRoot) });
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main();
}

