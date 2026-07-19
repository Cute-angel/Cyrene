import { spawn } from "node:child_process";
import crypto from "node:crypto";

import { PROTOCOL_VERSION } from "./config.mjs";

export class CoreRpcError extends Error {
  constructor(error) {
    super(error?.message || "Cyrene Core request failed");
    this.name = "CoreRpcError";
    this.code = error?.code || "core_error";
  }
}

export class ProtocolMismatchError extends Error {}

export async function ensureCore(config, options = {}) {
  const fetchImpl = options.fetchImpl ?? globalThis.fetch;
  try {
    return await checkHealth(config, { fetchImpl, timeoutMs: options.initialHealthTimeoutMs ?? 600 });
  } catch (error) {
    if (error instanceof ProtocolMismatchError) throw error;
  }

  if (!config.corePath || !config.dataDir) {
    throw new Error("Cyrene Core is unavailable and its executable/data directory are not configured");
  }
  const spawnImpl = options.spawnImpl ?? spawn;
  const child = spawnImpl(config.corePath, ["--data-dir", config.dataDir, "serve"], {
    detached: true,
    stdio: "ignore",
    windowsHide: true,
  });
  child.on?.("error", () => {});
  child.unref?.();

  const attempts = options.attempts ?? 12;
  const intervalMs = options.intervalMs ?? 100;
  let lastError;
  for (let attempt = 0; attempt < attempts; attempt += 1) {
    if (attempt > 0) await delay(intervalMs);
    try {
      return await checkHealth(config, { fetchImpl, timeoutMs: options.startupHealthTimeoutMs ?? 250 });
    } catch (error) {
      if (error instanceof ProtocolMismatchError) throw error;
      lastError = error;
    }
  }
  throw new Error(`Cyrene Core did not become healthy: ${lastError?.message || "timeout"}`);
}

export async function checkHealth(config, { fetchImpl = globalThis.fetch, timeoutMs = 1200 } = {}) {
  const response = await fetchImpl(`${config.coreUrl}/v1/health`, {
    signal: AbortSignal.timeout(timeoutMs),
  });
  if (!response.ok) throw new Error(`health returned HTTP ${response.status}`);
  const health = await response.json();
  if (health.protocol_version !== PROTOCOL_VERSION) {
    throw new ProtocolMismatchError(
      `Cyrene Core protocol ${health.protocol_version ?? "unknown"} is incompatible with ${PROTOCOL_VERSION}`,
    );
  }
  return health;
}

export async function rpc(config, action, payload = {}, options = {}) {
  const fetchImpl = options.fetchImpl ?? globalThis.fetch;
  await ensureCore(config, { ...options, fetchImpl });
  const requestId = options.requestId ?? crypto.randomUUID();
  const response = await fetchImpl(`${config.coreUrl}/v1/rpc`, {
    method: "POST",
    headers: {
      authorization: `Bearer ${config.token}`,
      "content-type": "application/json",
    },
    body: JSON.stringify({
      request_id: requestId,
      protocol_version: PROTOCOL_VERSION,
      action,
      payload,
    }),
    signal: AbortSignal.timeout(options.timeoutMs ?? 8000),
  });
  const envelope = await response.json().catch(() => null);
  if (!response.ok) {
    throw new Error(`Cyrene Core returned HTTP ${response.status}`);
  }
  if (!envelope?.ok) throw new CoreRpcError(envelope?.error);
  if (envelope.protocol_version !== PROTOCOL_VERSION) {
    throw new ProtocolMismatchError(`Unexpected response protocol ${envelope.protocol_version}`);
  }
  return envelope.data;
}

function delay(milliseconds) {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}
