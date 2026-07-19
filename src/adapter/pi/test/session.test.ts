import assert from "node:assert/strict";
import test from "node:test";
import { cacheCandidates, createSessionCache, resolveSelection } from "../src/session.js";
import type { MatchCandidates, Memory } from "../src/types.js";

const memory: Memory = {
  id: "m1",
  name: "Rule",
  content: { type: "rule", text: "Keep it small." },
  status: "enabled",
  source_type: "user",
  body_version: 1,
  embedding_status: "ready",
  created_at: "2026-01-01T00:00:00Z",
  updated_at: "2026-01-01T00:00:00Z",
};

function match(id: string, expiresAt: string): MatchCandidates {
  return {
    match_id: id,
    expires_at: expiresAt,
    hits: [{ memory, score: 1, sources: [] }],
    degraded: false,
  };
}

test("candidate cache evicts expired and consumed matches", () => {
  const cache = createSessionCache();
  cacheCandidates(cache, match("expired", "2000-01-01T00:00:00Z"));
  cacheCandidates(cache, match("active", "2099-01-01T00:00:00Z"));
  assert.equal(cache.matches.has("expired"), false);
  assert.equal(cache.matches.has("active"), true);

  resolveSelection(cache, {
    match_id: "active",
    status: "accepted",
    accepted_ids: ["m1"],
    rejected: [],
    retryable: false,
  });
  assert.equal(cache.matches.has("active"), false);
  assert.deepEqual([...cache.used.values()], [memory]);
});

