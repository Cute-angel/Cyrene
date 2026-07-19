import { mkdir, readFile, readdir, rename, unlink, writeFile } from "node:fs/promises";
import path from "node:path";

function safeId(value) {
  if (!/^[A-Za-z0-9_-]{1,160}$/.test(value)) throw new Error("invalid match id");
  return value;
}

export async function cacheMatch(dataDir, match) {
  const matchId = safeId(match.match_id);
  const directory = path.join(dataDir, "matches");
  await mkdir(directory, { recursive: true, mode: 0o700 });
  await pruneExpiredMatches(directory);
  await atomicJson(path.join(directory, `${matchId}.json`), {
    ...match,
    cached_at: new Date().toISOString(),
  });
}

async function pruneExpiredMatches(directory) {
  let files;
  try {
    files = await readdir(directory);
  } catch (error) {
    if (error.code === "ENOENT") return;
    throw error;
  }
  await Promise.all(files.filter((file) => file.endsWith(".json")).map(async (file) => {
    const target = path.join(directory, file);
    try {
      const cached = JSON.parse(await readFile(target, "utf8"));
      if (cached.expires_at && Date.parse(cached.expires_at) <= Date.now()) await unlink(target);
    } catch (error) {
      if (error.code !== "ENOENT") await unlink(target).catch(() => {});
    }
  }));
}

export async function readMatch(dataDir, matchId) {
  const file = path.join(dataDir, "matches", `${safeId(matchId)}.json`);
  const match = JSON.parse(await readFile(file, "utf8"));
  if (match.expires_at && Date.parse(match.expires_at) <= Date.now()) {
    throw new Error("cached match has expired; run matching again");
  }
  return match;
}

export async function recordUsed(dataDir, record) {
  await mkdir(dataDir, { recursive: true, mode: 0o700 });
  const file = path.join(dataDir, "used.json");
  let records = [];
  try {
    records = JSON.parse(await readFile(file, "utf8"));
    if (!Array.isArray(records)) records = [];
  } catch (error) {
    if (error.code !== "ENOENT") throw error;
  }
  records.unshift({ ...record, selected_at: new Date().toISOString() });
  await atomicJson(file, records.slice(0, 100));
}

export async function readUsed(dataDir, limit = 20) {
  try {
    const records = JSON.parse(await readFile(path.join(dataDir, "used.json"), "utf8"));
    return Array.isArray(records) ? records.slice(0, Math.max(1, Math.min(limit, 100))) : [];
  } catch (error) {
    if (error.code === "ENOENT") return [];
    throw error;
  }
}

export function memoriesFromMatch(match, ids) {
  const hits = Array.isArray(match.hits) ? match.hits : [];
  const byId = new Map(
    hits
      .map((hit) => hit?.memory ?? hit)
      .filter((memory) => memory?.id)
      .map((memory) => [memory.id, memory]),
  );
  return ids.map((id) => byId.get(id)).filter(Boolean);
}

async function atomicJson(file, value) {
  const temporary = `${file}.${process.pid}.${Date.now()}.tmp`;
  await writeFile(temporary, `${JSON.stringify(value, null, 2)}\n`, { mode: 0o600 });
  await rename(temporary, file);
}
