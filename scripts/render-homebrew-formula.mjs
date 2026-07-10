#!/usr/bin/env node

import fs from "node:fs";
import { pathToFileURL } from "node:url";

export function renderFormula({ version, sha256 }) {
  if (!/^\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?$/.test(version)) {
    throw new Error(`invalid version: ${version}`);
  }
  if (!/^[a-f0-9]{64}$/.test(sha256)) {
    throw new Error("SHA-256 must contain exactly 64 lowercase hexadecimal characters");
  }

  return `class Beacon < Formula
  desc "Conservative development toolchain update manager"
  homepage "https://github.com/LioRael/beacon"
  url "https://github.com/LioRael/beacon/releases/download/v${version}/beacon-v${version}-aarch64-apple-darwin.tar.gz"
  sha256 "${sha256}"
  license "MIT"

  depends_on macos: :sequoia
  depends_on arch: :arm64

  def install
    bin.install "beacon"
  end

  test do
    system "#{bin}/beacon", "--version"
  end
end
`;
}

function main() {
  const [, , version, sha256, output] = process.argv;
  if (!version || !sha256) {
    console.error("usage: render-homebrew-formula.mjs <version> <sha256> [output]");
    process.exitCode = 2;
    return;
  }
  try {
    const formula = renderFormula({ version, sha256 });
    if (output) {
      fs.writeFileSync(output, formula);
    } else {
      process.stdout.write(formula);
    }
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) {
  main();
}
