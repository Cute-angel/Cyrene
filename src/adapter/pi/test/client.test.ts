import assert from "node:assert/strict";
import { EventEmitter } from "node:events";
import test from "node:test";
import { createRpcClient } from "../src/client.js";
import type { PiAdapterConfig } from "../src/types.js";

const config: PiAdapterConfig = {
  core_url: "http://127.0.0.1:46371",
  core_path: "C:\\Program Files\\Cyrene\\cyrene-core.exe",
  data_dir: "C:\\Users\\A\\AppData\\Local\\Cyrene",
  actor_id: "actor-1",
  token: "cyr_secret",
  protocol_version: "1.0",
};

test("posts an authenticated protocol envelope and returns data", async () => {
  let received: { url?: string; init?: RequestInit; body?: Record<string, unknown> } = {};
  const fetchMock: typeof fetch = async (input, init) => {
    received = {
      url: String(input),
      init,
      body: JSON.parse(String(init?.body)) as Record<string, unknown>,
    };
    return Response.json({
      request_id: received.body?.request_id,
      protocol_version: "1.0",
      ok: true,
      data: { value: 7 },
    });
  };
  const client = createRpcClient({ loadConfig: async () => config, fetch: fetchMock });

  assert.deepEqual(await client.request("memory.get", { id: "m1" }), { value: 7 });
  assert.equal(received.url, "http://127.0.0.1:46371/v1/rpc");
  assert.equal(new Headers(received.init?.headers).get("authorization"), "Bearer cyr_secret");
  assert.equal(received.body?.action, "memory.get");
  assert.deepEqual(received.body?.payload, { id: "m1" });
});

test("does not spawn Core for authenticated RPC errors", async () => {
  let spawned = false;
  const fetchMock: typeof fetch = async (_input, init) => {
    const request = JSON.parse(String(init?.body)) as { request_id: string };
    return Response.json({
      request_id: request.request_id,
      protocol_version: "1.0",
      ok: false,
      error: { code: "forbidden", message: "denied" },
    }, { status: 403 });
  };
  const client = createRpcClient({
    loadConfig: async () => config,
    fetch: fetchMock,
    spawn: (() => {
      spawned = true;
      throw new Error("must not spawn");
    }) as never,
  });

  await assert.rejects(() => client.request("memory.delete", { id: "m1" }), /denied/);
  assert.equal(spawned, false);
});

test("starts Core on demand after a connection failure and retries", async () => {
  let fetchCount = 0;
  let spawnCall: { file: string; args: string[] } | undefined;
  const fetchMock: typeof fetch = async (_input, init) => {
    fetchCount += 1;
    if (fetchCount === 1) throw new TypeError("fetch failed");
    if (fetchCount === 2) return Response.json({ status: "ok", protocol_version: "1.0" });
    const request = JSON.parse(String(init?.body)) as { request_id: string };
    return Response.json({
      request_id: request.request_id,
      protocol_version: "1.0",
      ok: true,
      data: { started: true },
    });
  };
  const spawnMock = ((file: string, args: string[]) => {
    spawnCall = { file, args };
    const child = new EventEmitter() as EventEmitter & { unref(): void };
    child.unref = () => {};
    queueMicrotask(() => child.emit("spawn"));
    return child;
  }) as never;
  const client = createRpcClient({
    loadConfig: async () => config,
    fetch: fetchMock,
    spawn: spawnMock,
    delay: async () => {},
  });

  assert.deepEqual(await client.request("health"), { started: true });
  assert.deepEqual(spawnCall, {
    file: config.core_path,
    args: ["--data-dir", config.data_dir, "serve"],
  });
});
