import assert from "node:assert/strict";
import test from "node:test";

import { handleMessage } from "../scripts/mcp-server.mjs";

test("MCP exposes tools and returns structured call results", async () => {
  const listed = await handleMessage({ jsonrpc: "2.0", id: 1, method: "tools/list" });
  assert.ok(listed.result.tools.some((tool) => tool.name === "cyrene_select_memories"));

  const called = await handleMessage({
    jsonrpc: "2.0",
    id: 2,
    method: "tools/call",
    params: { name: "cyrene_memory_get", arguments: { id: "memory-a" } },
  }, { callToolImpl: async () => ({ id: "memory-a" }) });
  assert.deepEqual(called.result.structuredContent, { data: { id: "memory-a" } });
  assert.equal(called.result.isError, undefined);
});

test("MCP negotiates versions and rejects malformed tool calls", async () => {
  const initialized = await handleMessage({
    jsonrpc: "2.0",
    id: 1,
    method: "initialize",
    params: { protocolVersion: "2099-01-01" },
  });
  assert.equal(initialized.result.protocolVersion, "2025-06-18");

  const invalidRequest = await handleMessage({ id: 2, method: "tools/list" });
  assert.equal(invalidRequest.error.code, -32600);

  const unknownTool = await handleMessage({
    jsonrpc: "2.0",
    id: 3,
    method: "tools/call",
    params: { name: "cyrene_missing", arguments: {} },
  });
  assert.equal(unknownTool.error.code, -32602);

  const invalidArguments = await handleMessage({
    jsonrpc: "2.0",
    id: 4,
    method: "tools/call",
    params: { name: "cyrene_select_memories", arguments: { match_id: "m", selected_ids: ["a", "a"] } },
  });
  assert.equal(invalidArguments.error.code, -32602);
});
