import assert from "node:assert/strict";
import test from "node:test";

import { handleUserPromptSubmit } from "../scripts/user-prompt-submit.mjs";

test("hook caches candidates and injects selection instructions", async () => {
  let cached;
  const output = await handleUserPromptSubmit({ prompt: "Edit the existing file without unrelated changes." }, {
    platform: "win32",
    loadConfigImpl: async () => ({ configPath: "C:\\private\\codex.json" }),
    rpcImpl: async (_config, action, payload) => {
      assert.equal(action, "match.candidates");
      assert.deepEqual(payload.description.environment, ["windows"]);
      assert.equal(payload.description.action, "edit");
      assert.equal(payload.description.object, "document");
      assert.ok(payload.description.keywords.some((keyword) => keyword.includes("existing file without unrelated changes")));
      return {
        match_id: "match-a",
        expires_at: "2999-01-01T00:00:00Z",
        hits: [{ memory: { id: "memory-a", name: "Minimal edits", body_version: 1, content: { type: "rule", text: "Keep scope narrow." } } }],
      };
    },
    cacheMatchImpl: async (_dir, value) => { cached = value; },
  });
  assert.equal(cached.match_id, "match-a");
  assert.match(output.hookSpecificOutput.additionalContext, /cyrene_select_memories/);
  assert.match(output.hookSpecificOutput.additionalContext, /memory-a/);
  assert.doesNotMatch(output.hookSpecificOutput.additionalContext, /body_version/);
});

test("hook fails open when Core is unavailable", async () => {
  const output = await handleUserPromptSubmit({ prompt: "Do the task" }, {
    loadConfigImpl: async () => { throw new Error("connection refused"); },
  });
  assert.equal(output.continue, true);
  assert.match(output.systemMessage, /continuing without it/);
});
