import { readFile, stat } from "node:fs/promises";
import os from "node:os";
import path from "node:path";

export const PROTOCOL_VERSION = "1.0";

export function defaultConfigPath({
  platform = process.platform,
  env = process.env,
  homedir = os.homedir(),
} = {}) {
  const pathApi = platform === "win32" ? path.win32 : path.posix;
  const cyreneHome = env.CYRENE_HOME || pathApi.join(homedir, ".Cyrene");
  return pathApi.join(cyreneHome, "config", "adapters", "codex.json");
}

export async function loadConfig(options = {}) {
  const platform = options.platform ?? process.platform;
  const configPath = options.configPath ?? defaultConfigPath(options);
  if (platform !== "win32" && process.platform !== "win32") {
    const metadata = await stat(configPath);
    if ((metadata.mode & 0o077) !== 0) {
      throw new Error(`Cyrene config must be readable only by its owner: ${configPath}`);
    }
  }
  const raw = JSON.parse(await readFile(configPath, "utf8"));
  const config = {
    configPath,
    coreUrl: raw.core_url ?? raw.coreUrl ?? "http://127.0.0.1:46371",
    corePath: raw.core_path ?? raw.corePath,
    dataDir: raw.data_dir ?? raw.dataDir,
    actorId: raw.actor_id ?? raw.actorId,
    token: raw.token,
    protocolVersion: raw.protocol_version ?? raw.protocolVersion ?? PROTOCOL_VERSION,
  };
  if (!config.token || typeof config.token !== "string") {
    throw new Error(`Cyrene adapter token is missing from ${configPath}`);
  }
  if (config.protocolVersion !== PROTOCOL_VERSION) {
    throw new Error(`Unsupported Cyrene protocol ${config.protocolVersion}; expected ${PROTOCOL_VERSION}`);
  }
  const pathApi = platform === "win32" ? path.win32 : path.posix;
  if (config.corePath && !pathApi.isAbsolute(config.corePath)) {
    throw new Error("Cyrene core_path must be absolute");
  }
  if (config.dataDir && !pathApi.isAbsolute(config.dataDir)) {
    throw new Error("Cyrene data_dir must be absolute");
  }
  const url = new URL(config.coreUrl);
  if (url.protocol !== "http:" || !isLoopback(url.hostname)) {
    throw new Error("Cyrene Core URL must use HTTP on a loopback address");
  }
  config.coreUrl = url.href.replace(/\/$/, "");
  return config;
}

function isLoopback(hostname) {
  const host = hostname.replace(/^\[|\]$/g, "").toLowerCase();
  return host === "localhost" || host === "::1" || /^127(?:\.\d{1,3}){3}$/.test(host);
}

export function pluginDataPath(env = process.env, configPath) {
  const configured = env.CYRENE_PLUGIN_DATA || env.PLUGIN_DATA;
  if (configured) return configured;
  if (!configPath) throw new Error("PLUGIN_DATA is not set");
  return path.join(path.dirname(configPath), "codex-data");
}
