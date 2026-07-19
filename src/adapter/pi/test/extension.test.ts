import assert from "node:assert/strict";
import test from "node:test";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { createCyreneExtension } from "../src/extension.js";
import type { RpcClient } from "../src/client.js";
import type { MatchCandidates, Memory } from "../src/types.js";

type Handler = (...args: unknown[]) => unknown;
interface RegisteredTool {
  execute: (...args: unknown[]) => Promise<{ content: unknown[]; details: unknown }>;
}

function fakePi() {
  const handlers = new Map<string, Handler>();
  const tools = new Map<string, RegisteredTool>();
  const api = {
    on(name: string, handler: Handler) {
      handlers.set(name, handler);
    },
    registerTool(tool: RegisteredTool & { name: string }) {
      tools.set(tool.name, tool);
    },
  } as unknown as ExtensionAPI;
  return { api, handlers, tools };
}

const memory: Memory = {
  id: "m1",
  name: "Keep scope narrow",
  content: { type: "rule", text: "Only change requested files." },
  index: {},
  status: "enabled",
  source_type: "user",
  body_version: 1,
  embedding_status: "ready",
  created_at: "2026-01-01T00:00:00Z",
  updated_at: "2026-01-01T00:00:00Z",
};

const candidates: MatchCandidates = {
  match_id: "match-1",
  expires_at: "2099-01-01T00:00:00Z",
  hits: [{ memory, score: 1, sources: ["structured"] }],
  degraded: false,
};

function context(confirm = async () => true) {
  return {
    ui: { confirm, notify() {} },
  };
}

test("task matching is fail-open", async () => {
  const { api, handlers } = fakePi();
  let notified = "";
  createCyreneExtension({
    client: { request: async () => { throw new Error("offline"); } },
  })(api);
  const result = await handlers.get("before_agent_start")?.(
    { prompt: "change code", systemPrompt: "base" },
    { ui: { notify(message: string) { notified = message; } } },
  );
  assert.equal(result, undefined);
  assert.match(notified, /continuing without memory/);
});

test("selection sends IDs only and returns cached original memories", async () => {
  const calls: Array<{ action: string; payload: unknown }> = [];
  const client: RpcClient = {
    async request<T>(action: string, payload?: unknown): Promise<T> {
      calls.push({ action, payload });
      if (action === "match.candidates") return candidates as T;
      if (action === "match.select") {
        return {
          match_id: "match-1",
          status: "accepted",
          accepted_ids: ["m1"],
          rejected: [],
          retryable: false,
        } as T;
      }
      throw new Error(`unexpected ${action}`);
    },
  };
  const { api, handlers, tools } = fakePi();
  createCyreneExtension({ client })(api);
  const injected = await handlers.get("before_agent_start")?.(
    { prompt: "change code", systemPrompt: "base" },
    context(),
  ) as { systemPrompt: string };
  assert.match(injected.systemPrompt, /match-1/);
  assert.doesNotMatch(injected.systemPrompt, /structured|body_version|score/);

  const result = await tools.get("cyrene_select_memories")?.execute(
    "call-1", { match_id: "match-1", selected_ids: ["m1"] }, undefined, undefined, context(),
  );
  assert.deepEqual(calls[1], {
    action: "match.select",
    payload: { match_id: "match-1", selected_ids: ["m1"] },
  });
  assert.deepEqual((result?.details as { memories: Memory[] }).memories, [memory]);

  const operation = await tools.get("cyrene_match_operation")?.execute(
    "call-2", { action: "edit", object: "code" }, undefined, undefined, context(),
  );
  const visible = JSON.stringify(operation?.details);
  assert.match(visible, /Keep scope narrow/);
  assert.doesNotMatch(visible, /structured|body_version|score/);
});

test("commit always asks for confirmation and respects cancellation", async () => {
  const calls: string[] = [];
  const client: RpcClient = {
    async request<T>(action: string): Promise<T> {
      calls.push(action);
      if (action === "memory.change.prepare") {
        return { change_id: "c1", expires_at: "2099-01-01T00:00:00Z", preview: { name: "new" } } as T;
      }
      if (action === "memory.change.commit") return memory as T;
      throw new Error(`unexpected ${action}`);
    },
  };
  const { api, tools } = fakePi();
  createCyreneExtension({ client })(api);
  await tools.get("cyrene_memory_change_prepare")?.execute(
    "call-1",
    { operation: "delete", id: "m1" },
    undefined,
    undefined,
    context(),
  );
  let confirms = 0;
  const cancelled = await tools.get("cyrene_memory_change_commit")?.execute(
    "call-2",
    { change_id: "c1" },
    undefined,
    undefined,
    context(async () => { confirms += 1; return false; }),
  );
  assert.deepEqual(cancelled?.details, { committed: false, reason: "user_cancelled" });
  assert.equal(confirms, 1);
  assert.deepEqual(calls, ["memory.change.prepare"]);

  await tools.get("cyrene_memory_change_commit")?.execute(
    "call-3",
    { change_id: "c1" },
    undefined,
    undefined,
    context(async () => { confirms += 1; return true; }),
  );
  assert.equal(confirms, 2);
  assert.deepEqual(calls, ["memory.change.prepare", "memory.change.commit"]);
});
