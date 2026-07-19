import assert from "node:assert/strict";
import test from "node:test";

import { ensureCore, rpc } from "../lib/core-client.mjs";

test("RPC sends the versioned envelope and bearer token", async () => {
  const requests = [];
  const fetchImpl = async (url, init = {}) => {
    requests.push({ url, init });
    if (url.endsWith("/v1/health")) {
      return new Response(JSON.stringify({ status: "ok", protocol_version: "1.0" }), { status: 200 });
    }
    return new Response(JSON.stringify({
      request_id: "request-test",
      protocol_version: "1.0",
      ok: true,
      data: { accepted_ids: ["memory-a"] },
    }), { status: 200 });
  };
  const data = await rpc({ coreUrl: "http://127.0.0.1:46371", token: "secret" }, "match.select", {
    match_id: "match-a",
    selected_ids: ["memory-a"],
  }, { fetchImpl, requestId: "request-test" });

  assert.deepEqual(data, { accepted_ids: ["memory-a"] });
  assert.equal(requests[1].init.headers.authorization, "Bearer secret");
  assert.deepEqual(JSON.parse(requests[1].init.body), {
    request_id: "request-test",
    protocol_version: "1.0",
    action: "match.select",
    payload: { match_id: "match-a", selected_ids: ["memory-a"] },
  });
});

test("starts Core on demand and leaves single-instance arbitration to Core", async () => {
  let healthCalls = 0;
  let spawned;
  const health = await ensureCore({
    coreUrl: "http://127.0.0.1:46371",
    corePath: "C:\\Cyrene\\cyrene-core.exe",
    dataDir: "C:\\Cyrene\\data",
  }, {
    fetchImpl: async () => {
      healthCalls += 1;
      if (healthCalls === 1) throw new Error("connection refused");
      return new Response(JSON.stringify({ status: "ok", protocol_version: "1.0" }), { status: 200 });
    },
    spawnImpl: (command, args, options) => {
      spawned = { command, args, options };
      return { unref() {} };
    },
    attempts: 1,
  });
  assert.equal(health.status, "ok");
  assert.deepEqual(spawned.args, ["--data-dir", "C:\\Cyrene\\data", "serve"]);
  assert.equal(spawned.options.detached, true);
  assert.equal(spawned.options.windowsHide, true);
});
