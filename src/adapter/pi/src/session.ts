import type { ChangePreview, MatchCandidates, MatchSelection, Memory } from "./types.js";

export interface SessionCache {
  matches: Map<string, MatchCandidates>;
  used: Map<string, Memory>;
  changes: Map<string, ChangePreview>;
}

export function createSessionCache(): SessionCache {
  return { matches: new Map(), used: new Map(), changes: new Map() };
}

export function clearSessionCache(cache: SessionCache): void {
  cache.matches.clear();
  cache.used.clear();
  cache.changes.clear();
}

export function cacheCandidates(cache: SessionCache, candidates: MatchCandidates): void {
  const now = Date.now();
  for (const [id, cached] of cache.matches) {
    if (Date.parse(cached.expires_at) <= now) cache.matches.delete(id);
  }
  if (Date.parse(candidates.expires_at) <= now) return;
  cache.matches.set(candidates.match_id, candidates);
}

export function resolveSelection(
  cache: SessionCache,
  selection: MatchSelection,
): { selection: MatchSelection; memories: Memory[] } {
  const candidates = cache.matches.get(selection.match_id);
  if (!candidates) throw new Error(`candidate cache missing for match ${selection.match_id}`);
  if (Date.parse(candidates.expires_at) <= Date.now()) {
    cache.matches.delete(selection.match_id);
    throw new Error(`candidate cache expired for match ${selection.match_id}`);
  }
  const byId = new Map(candidates.hits.map((hit) => [hit.memory.id, hit.memory]));
  const memories = selection.accepted_ids.map((id) => {
    const memory = byId.get(id);
    if (!memory) throw new Error(`Core accepted unknown memory ${id}`);
    cache.used.set(id, memory);
    return memory;
  });
  if (selection.status === "accepted" || !selection.retryable) {
    cache.matches.delete(selection.match_id);
  }
  return { selection, memories };
}
