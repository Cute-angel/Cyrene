import { spawn as nodeSpawn, type ChildProcess } from "node:child_process";
import { randomUUID } from "node:crypto";
import { readUserConfig } from "./config.js";
import { PROTOCOL_VERSION, type PiAdapterConfig, type RpcEnvelope } from "./types.js";

export class CyreneRpcError extends Error {
  constructor(
    message: string,
    readonly code = "transport_error",
  ) {
    super(message);
    this.name = "CyreneRpcError";
  }
}

type Spawn = typeof nodeSpawn;

export interface ClientDependencies {
  loadConfig?: () => Promise<PiAdapterConfig>;
  fetch?: typeof globalThis.fetch;
  spawn?: Spawn;
  delay?: (milliseconds: number) => Promise<void>;
}

export interface RpcClient {
  request<T>(action: string, payload?: unknown): Promise<T>;
}

function endpoint(coreUrl: string, path: string): string {
  return new URL(path, coreUrl.endsWith("/") ? coreUrl : `${coreUrl}/`).toString();
}

function waitForSpawn(child: ChildProcess): Promise<void> {
  return new Promise((resolve, reject) => {
    child.once("spawn", resolve);
    child.once("error", reject);
  });
}

export function createRpcClient(dependencies: ClientDependencies = {}): RpcClient {
  const loadConfig = dependencies.loadConfig ?? (() => readUserConfig());
  const fetchImpl = dependencies.fetch ?? globalThis.fetch;
  const spawnImpl = dependencies.spawn ?? nodeSpawn;
  const delay = dependencies.delay ?? ((milliseconds) => new Promise((resolve) => setTimeout(resolve, milliseconds)));
  let starting: Promise<void> | undefined;

  async function startCore(config: PiAdapterConfig): Promise<void> {
    const child = spawnImpl(config.core_path, ["--data-dir", config.data_dir, "serve"], {
      detached: true,
      stdio: "ignore",
      windowsHide: true,
    });
    child.unref();
    await waitForSpawn(child);

    let lastError: unknown;
    for (let attempt = 0; attempt < 20; attempt += 1) {
      try {
        const response = await fetchImpl(endpoint(config.core_url, "/v1/health"), {
          signal: AbortSignal.timeout(1_000),
        });
        if (response.ok) return;
        lastError = new Error(`health check returned HTTP ${response.status}`);
      } catch (error) {
        lastError = error;
      }
      await delay(100);
    }
    throw new Error(`Cyrene Core did not become ready: ${String(lastError)}`);
  }

  async function ensureCore(config: PiAdapterConfig): Promise<void> {
    if (!starting) {
      starting = startCore(config).finally(() => {
        starting = undefined;
      });
    }
    await starting;
  }

  async function post<T>(config: PiAdapterConfig, action: string, payload: unknown): Promise<T> {
    const requestId = randomUUID();
    const response = await fetchImpl(endpoint(config.core_url, "/v1/rpc"), {
      method: "POST",
      headers: {
        authorization: `Bearer ${config.token}`,
        "content-type": "application/json",
      },
      body: JSON.stringify({
        request_id: requestId,
        protocol_version: config.protocol_version,
        action,
        payload,
      }),
      signal: AbortSignal.timeout(10_000),
    });
    let envelope: RpcEnvelope<T>;
    try {
      envelope = (await response.json()) as RpcEnvelope<T>;
    } catch {
      throw new CyreneRpcError(`Cyrene Core returned invalid JSON (HTTP ${response.status})`);
    }
    if (!response.ok || !envelope.ok || envelope.data === undefined) {
      throw new CyreneRpcError(
        envelope.error?.message ?? `Cyrene Core request failed with HTTP ${response.status}`,
        envelope.error?.code ?? "http_error",
      );
    }
    if (envelope.request_id !== requestId || envelope.protocol_version !== config.protocol_version) {
      throw new CyreneRpcError("Cyrene Core returned a mismatched request or protocol version", "protocol_error");
    }
    return envelope.data;
  }

  return {
    async request<T>(action: string, payload: unknown = {}): Promise<T> {
      const config = await loadConfig();
      if (config.protocol_version !== PROTOCOL_VERSION) {
        throw new CyreneRpcError(
          `PI adapter supports protocol ${PROTOCOL_VERSION}, config requests ${config.protocol_version}`,
          "protocol_error",
        );
      }
      try {
        return await post<T>(config, action, payload);
      } catch (error) {
        if (error instanceof CyreneRpcError) throw error;
        await ensureCore(config);
        return post<T>(config, action, payload);
      }
    },
  };
}

