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
  assert.match(skill, /Homebrew.*Rust.*Node.*npm.*pnpm.*Go.*Bun.*Deno.*uv/i);
  assert.match(metadata, /display_name: "Beacon"/);
  assert.match(metadata, /default_prompt: "[^"]*\$beacon[^"]*"/);
});

test("skill keeps read-only checks automatic and upgrades confirmed", () => {
  assert.match(skill, /beacon check --json/);
  assert.match(skill, /beacon doctor.*--json/);
  assert.match(skill, /beacon history.*--json/);
  assert.match(skill, /explicit confirmation/i);
  assert.match(skill, /beacon upgrade <targets> --yes/);
  assert.match(skill, /Report missing tools for diagnosis only/i);
  assert.match(skill, /schema_version: 2/);
  assert.match(skill, /data\.tools.*data\.inventories/i);
  assert.match(skill, /installation\.source.*update\.manager/i);
});

test("skill excludes missing and unmanaged from upgrade targets", () => {
  assert.match(skill, /Never pass missing or unmanaged tools to `upgrade`/i);
  assert.match(skill, /status is `outdated` and whose `action` is present/);
});

test("skill understands exact, floating, and rolling verification modes", () => {
  assert.match(skill, /`exact`, `floating`, or `rolling` target mode/);
  assert.match(skill, /`exact`:.*equal the confirmed expected version/i);
  assert.match(skill, /`floating`:.*newer than the old version/i);
  assert.match(skill, /no lower than the confirmed candidate/i);
  assert.match(skill, /`rolling`:.*observed revision must change/i);
});

test("skill interprets inspect envelope outcomes without scraping human terminal output", () => {
  assert.match(skill, /Never scrape colored or human terminal/i);
  assert.match(skill, /status: "ok"/);
  assert.match(skill, /status: "partial"/);
  assert.match(skill, /status: "error"/);
  assert.match(skill, /exit 2/);
  assert.match(skill, /exit 1/);
});

test("skill handles upgrade stop-on-first-failure via JSON status and recovery errors", () => {
  assert.match(skill, /stops at the first failed command or verification/i);
  assert.match(skill, /some targets succeeded.*status: "partial".*exit 2/is);
  assert.match(skill, /no target succeeded.*status: "error".*exit 1/is);
  assert.match(skill, /structured `errors` field/i);
  assert.match(skill, /Do not scrape human terminal output or auto-retry/i);
});

test("skill excludes project dependencies and uses the agreed install order", () => {
  assert.match(skill, /Do not use.*project dependenc/i);
  const homebrew = skill.indexOf("brew install LioRael/tap/beacon");
  const npm = skill.indexOf("npm install --global @liorael/beacon");
  const cargo = skill.indexOf("cargo install beacon-cli --locked");

  assert.ok(homebrew >= 0 && homebrew < npm && npm < cargo);
});
