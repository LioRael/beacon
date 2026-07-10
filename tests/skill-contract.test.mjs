import assert from "node:assert/strict";
import fs from "node:fs";
import test from "node:test";

const skill = fs.readFileSync(new URL("../skills/beacon/SKILL.md", import.meta.url), "utf8");
const metadata = fs.readFileSync(
  new URL("../skills/beacon/agents/openai.yaml", import.meta.url),
  "utf8",
);

test("skill metadata is complete and discoverable", () => {
  assert.doesNotMatch(skill, /TODO/);
  assert.match(skill, /^name: beacon$/m);
  assert.match(skill, /Homebrew.*Rust.*Node.*npm.*pnpm.*Go/i);
  assert.match(metadata, /display_name: "Beacon"/);
  assert.match(metadata, /default_prompt: "[^"]*\$beacon[^"]*"/);
});

test("skill keeps read-only checks automatic and upgrades confirmed", () => {
  assert.match(skill, /beacon check --json/);
  assert.match(skill, /beacon doctor.*--json/);
  assert.match(skill, /beacon history.*--json/);
  assert.match(skill, /explicit confirmation/i);
  assert.match(skill, /beacon upgrade <targets> --yes/);
  assert.match(skill, /missing tools.*report/i);
});

test("skill excludes project dependencies and uses the agreed install order", () => {
  assert.match(skill, /Do not use.*project dependenc/i);
  const homebrew = skill.indexOf("brew install LioRael/tap/beacon");
  const npm = skill.indexOf("npm install --global @liorael/beacon");
  const cargo = skill.indexOf("cargo install beacon-cli --locked");

  assert.ok(homebrew >= 0 && homebrew < npm && npm < cargo);
});

