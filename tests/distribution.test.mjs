import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import { renderFormula } from "../scripts/render-homebrew-formula.mjs";
import { stageNpmPackage } from "../scripts/stage-npm-package.mjs";
import { verifyReleaseVersion } from "../scripts/verify-release-version.mjs";

test("renders a versioned Apple Silicon formula", () => {
  const formula = renderFormula({ version: "0.3.2", sha256: "a".repeat(64) });

  assert.match(formula, /class Beacon < Formula/);
  assert.match(formula, /releases\/download\/v0\.3\.2\/beacon-v0\.3\.2-aarch64-apple-darwin\.tar\.gz/);
  assert.match(formula, new RegExp(`sha256 "${"a".repeat(64)}"`));
  assert.match(formula, /depends_on macos: :sequoia/);
  assert.match(formula, /depends_on arch: :arm64/);
  assert.match(formula, /bin\.install "beacon"/);
  assert.match(formula, /system "#\{bin\}\/beacon", "--version"/);
});

test("rejects unsafe formula inputs", () => {
  assert.throws(() => renderFormula({ version: "v0.3.2", sha256: "a".repeat(64) }), /version/);
  assert.throws(() => renderFormula({ version: "0.3.2", sha256: "nope" }), /SHA-256/);
});

test("release automation keeps optional publishers separate from the release", () => {
  const workflow = fs.readFileSync(new URL("../.github/workflows/release.yml", import.meta.url), "utf8");

  assert.match(workflow, /actions\/upload-artifact/);
  assert.match(workflow, /actions\/download-artifact/);
  assert.match(workflow, /NPM_TOKEN/);
  assert.match(workflow, /HOMEBREW_TAP_TOKEN/);
  assert.match(workflow, /npm publish/);
  assert.match(workflow, /LioRael\/homebrew-tap/);
});

test("Rust and npm package versions stay synchronized", () => {
  const cargo = fs.readFileSync(new URL("../Cargo.toml", import.meta.url), "utf8");
  const npm = JSON.parse(
    fs.readFileSync(new URL("../packages/npm/package.json", import.meta.url), "utf8"),
  );
  const cargoVersion = cargo.match(/\[workspace\.package\][\s\S]*?version = "([^"]+)"/)[1];

  assert.equal(npm.version, cargoVersion);
});

test("release tags must match Rust and npm package versions", () => {
  assert.doesNotThrow(() =>
    verifyReleaseVersion({ tag: "v0.3.2", cargoVersion: "0.3.2", npmVersion: "0.3.2" }),
  );
  assert.throws(
    () => verifyReleaseVersion({ tag: "v0.4.0", cargoVersion: "0.3.2", npmVersion: "0.3.2" }),
    /does not match/,
  );
});

test("stages a minimal npm tarball with the native binary", () => {
  const temporary = fs.mkdtempSync(path.join(os.tmpdir(), "beacon-npm-"));
  const binary = path.join(temporary, "beacon");
  const output = path.join(temporary, "package");
  fs.writeFileSync(binary, "#!/bin/sh\nexit 0\n", { mode: 0o755 });

  stageNpmPackage({
    binary,
    output,
    version: "0.3.2",
    repositoryRoot: fileURLToPath(new URL("..", import.meta.url)),
  });
  const metadata = JSON.parse(fs.readFileSync(path.join(output, "package.json"), "utf8"));
  const packed = JSON.parse(
    execFileSync("npm", ["pack", "--dry-run", "--json", output], {
      encoding: "utf8",
      env: { ...process.env, NPM_CONFIG_USERCONFIG: "/dev/null" },
    }),
  )[0];
  const files = packed.files.map((file) => file.path);

  assert.equal(metadata.version, "0.3.2");
  assert.equal(fs.statSync(path.join(output, "vendor", "beacon")).mode & 0o777, 0o755);
  assert.ok(files.includes("bin/beacon.mjs"));
  assert.ok(files.includes("vendor/beacon"));
  assert.ok(files.includes("LICENSE"));
  assert.ok(!files.some((file) => file.startsWith("test/")));
});

test("documents prepared install channels and release credentials", () => {
  const readme = fs.readFileSync(new URL("../README.md", import.meta.url), "utf8");

  assert.match(readme, /brew install LioRael\/tap\/beacon/);
  assert.match(readme, /npm install --global @liorael\/beacon/);
  assert.match(readme, /cargo install beacon-cli/);
  assert.match(readme, /NPM_TOKEN/);
  assert.match(readme, /HOMEBREW_TAP_TOKEN/);
  assert.match(readme, /not been created or\s+published/i);
});

test("CI runs distribution, skill, and Formula validation", () => {
  const workflow = fs.readFileSync(new URL("../.github/workflows/ci.yml", import.meta.url), "utf8");

  assert.match(workflow, /node --test packages\/npm\/test\/\*\.test\.mjs tests\/\*\.test\.mjs/);
  assert.match(workflow, /render-homebrew-formula\.mjs/);
  assert.match(workflow, /0\.3\.2/);
  assert.match(workflow, /ruby -c/);
  assert.match(workflow, /verify-release-version\.mjs v0\.3\.2/);
});

test("README documents the two-layer model and release metadata at 0.3.2", () => {
  const readme = fs.readFileSync(new URL("../README.md", import.meta.url), "utf8");

  assert.match(readme, /# Beacon 0\.3/);
  assert.match(readme, /beacon-v0\.3\.2-aarch64-apple-darwin/);
  assert.match(readme, /schema_version: 2/);
  assert.match(readme, /docs\/domain-glossary\.md/);
  assert.match(readme, /docs\/adr\/0001-two-layer-provider-model\.md/);
  assert.match(readme, /docs\/adr\/0002-schema-and-local-state-v2\.md/);
  assert.match(readme, /skills\/beacon\/SKILL\.md/);
});
