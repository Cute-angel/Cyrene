import assert from "node:assert/strict";
import test from "node:test";

import { callTool, TOOL_DEFINITIONS } from "../lib/tools.mjs";

const config = { coreUrl: "http://127.0.0.1:46371", token: "test", configPath: "/tmp/codex.json" };

test("select sends IDs only and returns accepted originals from the first cache", async () => {
  const calls = [];
  let recorded;
  const memory = {
    id: "memory-a",
    name: "Minimal edits",
    body_version: 2,
    content: { type: "rule", text: "Only edit requested behavior." },
  };
  const result = await callTool("cyrene_select_memories", {
    match_id: "match-a",
    selected_ids: ["memory-a"],
  }, {
    dataDir: "/unused",
    loadConfigImpl: async () => config,
    readMatchImpl: async () => ({ match_id: "match-a", hits: [{ memory }] }),
    rpcImpl: async (_config, action, payload) => {
      calls.push({ action, payload });
      return { match_id: "match-a", status: "accepted", accepted_ids: ["memory-a"], rejected: [], retryable: false };
    },
    recordUsedImpl: async (_dir, value) => { recorded = value; },
  });

  assert.deepEqual(calls, [{
    action: "match.select",
    payload: { match_id: "match-a", selected_ids: ["memory-a"] },
  }]);
  assert.deepEqual(result.memories, [{ id: memory.id, name: memory.name, content: memory.content }]);
  assert.deepEqual(recorded.memories, [memory]);
  assert.doesNotMatch(JSON.stringify(calls), /Only edit requested behavior/);
});

test("operation matching caches full hits but exposes only confirmed originals", async () => {
  const memory = {
    id: "memory-a",
    name: "Minimal edits",
    content: { type: "rule", text: "Keep scope narrow." },
    body_version: 3,
    index: { keywords: ["hidden"] },
  };
  let cached;
  const result = await callTool("cyrene_match_operation", { description: { stage: "operation", action: "edit" } }, {
    loadConfigImpl: async () => ({}),
    rpcImpl: async () => ({
      match_id: "match-a",
      expires_at: "2099-01-01T00:00:00Z",
      degraded: false,
      hits: [{ memory, score: 0.9, sources: ["semantic"] }],
    }),
    cacheMatchImpl: async (_dir, match) => { cached = match; },
    dataDir: "unused",
  });
  assert.equal(cached.hits[0].score, 0.9);
  assert.deepEqual(result.candidates, [{ id: memory.id, name: memory.name, content: memory.content }]);
  assert.equal(JSON.stringify(result).includes("body_version"), false);
  assert.equal(JSON.stringify(result).includes("score"), false);
});

test("a rejected selection injects no cached memory", async () => {
  const result = await callTool("cyrene_select_memories", {
    match_id: "match-a",
    selected_ids: ["memory-a"],
  }, {
    dataDir: "/unused",
    loadConfigImpl: async () => config,
    readMatchImpl: async () => ({ hits: [{ memory: { id: "memory-a", content: { text: "secret body" } } }] }),
    rpcImpl: async () => ({ status: "rejected", accepted_ids: [], rejected: [{ code: "stale" }], retryable: true }),
  });
  assert.deepEqual(result.memories, []);
});

test("prepare uses the exact tagged change payload", async () => {
  let call;
  await callTool("cyrene_memory_change_prepare", {
    operation: "set_status",
    id: "memory-a",
    status: "disabled",
    draft: { ignored: true },
  }, {
    dataDir: "/unused",
    loadConfigImpl: async () => config,
    rpcImpl: async (_config, action, payload) => {
      call = { action, payload };
      return { change_id: "change-a", preview: {} };
    },
  });
  assert.deepEqual(call, {
    action: "memory.change.prepare",
    payload: { operation: "set_status", id: "memory-a", status: "disabled" },
  });
});

test("tool annotations accurately mark reads and destructive commits", () => {
  const byName = new Map(TOOL_DEFINITIONS.map((tool) => [tool.name, tool.annotations]));
  assert.equal(byName.get("cyrene_memory_get").readOnlyHint, true);
  assert.equal(byName.get("cyrene_memory_change_prepare").readOnlyHint, false);
  assert.deepEqual(byName.get("cyrene_memory_change_commit"), {
    readOnlyHint: false,
    destructiveHint: true,
    openWorldHint: false,
  });
});
