import assert from "node:assert/strict";
import { mkdtemp, mkdir, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import { defaultConfigPath, loadConfig } from "../lib/config.mjs";

test("uses the unified per-user Cyrene config location", () => {
  assert.equal(
    defaultConfigPath({ platform: "win32", env: {}, homedir: "C:\\Users\\me" }),
    "C:\\Users\\me\\.Cyrene\\config\\adapters\\codex.json",
  );
  assert.equal(
    defaultConfigPath({ platform: "linux", env: {}, homedir: "/home/me" }),
    "/home/me/.Cyrene/config/adapters/codex.json",
  );
  assert.equal(
    defaultConfigPath({ platform: "linux", env: { CYRENE_HOME: "/srv/cyrene" }, homedir: "/home/me" }),
    "/srv/cyrene/config/adapters/codex.json",
  );
});

test("loads a private Linux config and validates core_path", async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), "cyrene-config-"));
  const configPath = path.join(root, "cyrene", "adapters", "codex.json");
  await mkdir(path.dirname(configPath), { recursive: true });
  await writeFile(configPath, JSON.stringify({
    core_url: "http://127.0.0.1:46371",
    core_path: "/opt/cyrene/cyrene-core",
    data_dir: "/tmp/cyrene-data",
    actor_id: "actor-codex",
    token: "test-token",
    protocol_version: "1.0",
  }), { mode: 0o600 });

  const config = await loadConfig({ platform: "linux", configPath });
  assert.equal(config.corePath, "/opt/cyrene/cyrene-core");
  assert.equal(config.actorId, "actor-codex");
});

test("rejects a remotely hosted Core URL", async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), "cyrene-config-"));
  const configPath = path.join(root, "codex.json");
  await writeFile(configPath, JSON.stringify({
    core_url: "https://example.com",
    core_path: "/opt/cyrene/cyrene-core",
    data_dir: "/tmp/cyrene-data",
    token: "test-token",
    protocol_version: "1.0",
  }), { mode: 0o600 });
  await assert.rejects(() => loadConfig({ platform: "linux", configPath }), /loopback/);
});

test("rejects relative executable and data paths", async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), "cyrene-config-"));
  const configPath = path.join(root, "codex.json");
  const base = {
    core_url: "http://127.0.0.1:46371",
    token: "test-token",
    protocol_version: "1.0",
  };

  await writeFile(configPath, JSON.stringify({
    ...base,
    core_path: "bin/cyrene-core",
    data_dir: "/tmp/cyrene-data",
  }), { mode: 0o600 });
  await assert.rejects(() => loadConfig({ platform: "linux", configPath }), /core_path must be absolute/);

  await writeFile(configPath, JSON.stringify({
    ...base,
    core_path: "/opt/cyrene/cyrene-core",
    data_dir: "relative/data",
  }), { mode: 0o600 });
  await assert.rejects(() => loadConfig({ platform: "linux", configPath }), /data_dir must be absolute/);
});
