import assert from "node:assert/strict";
import { mkdir, mkdtemp, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { readUserConfig, userConfigPath, validateConfig } from "../src/config.js";

test("uses the unified Windows user config location", () => {
  assert.equal(
    userConfigPath({ platform: "win32", env: {}, home: "C:\\Users\\A" }),
    "C:\\Users\\A\\.Cyrene\\config\\adapters\\pi.json",
  );
});

test("uses ~/.Cyrene or CYRENE_HOME on Linux", () => {
  assert.equal(
    userConfigPath({ platform: "linux", env: { CYRENE_HOME: "/cfg/cyrene" }, home: "/home/a" }),
    "/cfg/cyrene/config/adapters/pi.json",
  );
  assert.equal(
    userConfigPath({ platform: "linux", env: {}, home: "/home/a" }),
    "/home/a/.Cyrene/config/adapters/pi.json",
  );
});

test("rejects remote core URLs", () => {
  assert.throws(
    () => validateConfig({
      core_url: "https://example.com",
      core_path: "C:\\Cyrene\\cyrene-core.exe",
      data_dir: "C:\\Users\\A\\AppData\\Local\\Cyrene",
      actor_id: "actor",
      token: "cyr_secret",
      protocol_version: "1.0",
    }),
    /loopback/,
  );
});

test("accepts IPv4 and IPv6 loopback core URLs", () => {
  const base = {
    core_path: "C:\\Cyrene\\cyrene-core.exe",
    data_dir: "C:\\Users\\A\\AppData\\Local\\Cyrene",
    actor_id: "actor",
    token: "cyr_secret",
    protocol_version: "1.0",
  };
  assert.equal(validateConfig({ ...base, core_url: "http://127.23.4.5:46371" }).core_url, "http://127.23.4.5:46371");
  assert.equal(validateConfig({ ...base, core_url: "http://[::1]:46371" }).core_url, "http://[::1]:46371");
});

test("rejects a Linux config readable by other users", async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), "cyrene-pi-config-"));
  const configPath = path.join(root, "config", "adapters", "pi.json");
  await mkdir(path.dirname(configPath), { recursive: true });
  await writeFile(configPath, "{}", { mode: 0o644 });
  await assert.rejects(
    () => readUserConfig({ platform: "linux", env: { CYRENE_HOME: root } }),
    /readable only by its owner/,
  );
});
