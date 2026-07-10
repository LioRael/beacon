import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import { launch, resolveBinaryPath } from "../bin/beacon.mjs";

test("resolves the bundled Apple Silicon macOS binary", () => {
  assert.equal(
    resolveBinaryPath({ platform: "darwin", arch: "arm64", packageRoot: "/package" }),
    path.join("/package", "vendor", "beacon"),
  );
});

test("rejects unsupported platforms", () => {
  assert.throws(
    () => resolveBinaryPath({ platform: "linux", arch: "x64", packageRoot: "/package" }),
    /supports Apple Silicon macOS 15 or newer/,
  );
});

test("passes arguments and the native exit code through", () => {
  const calls = [];
  const code = launch({
    binaryPath: "/package/vendor/beacon",
    args: ["check", "--json"],
    exists: () => true,
    spawn: (...args) => {
      calls.push(args);
      return { status: 17, signal: null, error: null };
    },
    kill: () => assert.fail("kill should not be called"),
    pid: 123,
  });

  assert.equal(code, 17);
  assert.deepEqual(calls, [["/package/vendor/beacon", ["check", "--json"], { stdio: "inherit" }]]);
});

test("forwards native termination signals", () => {
  const signals = [];
  const code = launch({
    binaryPath: "/package/vendor/beacon",
    args: [],
    exists: () => true,
    spawn: () => ({ status: null, signal: "SIGTERM", error: null }),
    kill: (...args) => signals.push(args),
    pid: 456,
  });

  assert.equal(code, 1);
  assert.deepEqual(signals, [[456, "SIGTERM"]]);
});

test("fails clearly when the packaged binary is missing", () => {
  assert.throws(
    () =>
      launch({
        binaryPath: "/package/vendor/beacon",
        args: [],
        exists: () => false,
        spawn: () => assert.fail("spawn should not be called"),
        kill: () => {},
        pid: 123,
      }),
    /does not contain the Beacon binary/,
  );
});

test("runs when invoked through an npm-style bin symlink", () => {
  const temporary = fs.mkdtempSync(path.join(os.tmpdir(), "beacon-launcher-"));
  const link = path.join(temporary, "beacon");
  const launcher = fileURLToPath(new URL("../bin/beacon.mjs", import.meta.url));
  fs.symlinkSync(launcher, link);

  const result = spawnSync(link, ["--version"], { encoding: "utf8" });

  assert.equal(result.status, 1);
  assert.match(result.stderr, /does not contain the Beacon binary/);
});
