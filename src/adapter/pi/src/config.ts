import { stat, readFile } from "node:fs/promises";
import { homedir } from "node:os";
import { isAbsolute, posix, win32 } from "node:path";
import type { PiAdapterConfig } from "./types.js";

export interface ConfigPathOptions {
  platform?: NodeJS.Platform;
  env?: NodeJS.ProcessEnv;
  home?: string;
}

export function userConfigPath(options: ConfigPathOptions = {}): string {
  const platform = options.platform ?? process.platform;
  const env = options.env ?? process.env;
  const home = options.home ?? homedir();
  const pathApi = platform === "win32" ? win32 : posix;
  const cyreneHome = env.CYRENE_HOME || pathApi.join(home, ".Cyrene");
  return pathApi.join(cyreneHome, "config", "adapters", "pi.json");
}

function requireString(value: unknown, field: keyof PiAdapterConfig): string {
  if (typeof value !== "string" || value.trim() === "") {
    throw new Error(`Cyrene PI config field ${field} must be a non-empty string`);
  }
  return value;
}

export function validateConfig(value: unknown): PiAdapterConfig {
  if (!value || typeof value !== "object") {
    throw new Error("Cyrene PI config must be a JSON object");
  }
  const raw = value as Record<string, unknown>;
  const config: PiAdapterConfig = {
    core_url: requireString(raw.core_url, "core_url"),
    core_path: requireString(raw.core_path, "core_path"),
    data_dir: requireString(raw.data_dir, "data_dir"),
    actor_id: requireString(raw.actor_id, "actor_id"),
    token: requireString(raw.token, "token"),
    protocol_version: requireString(raw.protocol_version, "protocol_version"),
  };

  const url = new URL(config.core_url);
  const loopbackHost = url.hostname === "localhost"
    || url.hostname === "[::1]"
    || url.hostname.startsWith("127.");
  if (url.protocol !== "http:" || !loopbackHost) {
    throw new Error("Cyrene core_url must be an HTTP loopback URL");
  }
  if (!isAbsolute(config.core_path) || !isAbsolute(config.data_dir)) {
    throw new Error("Cyrene core_path and data_dir must be absolute paths");
  }
  return config;
}

export async function readUserConfig(
  options: ConfigPathOptions = {},
): Promise<PiAdapterConfig> {
  const path = userConfigPath(options);
  if ((options.platform ?? process.platform) !== "win32") {
    const info = await stat(path);
    if ((info.mode & 0o077) !== 0) {
      throw new Error(`Cyrene PI config ${path} must be readable only by its owner; use chmod 600`);
    }
  }
  let parsed: unknown;
  try {
    parsed = JSON.parse(await readFile(path, "utf8"));
  } catch (error) {
    throw new Error(`could not read Cyrene PI user config ${path}: ${String(error)}`);
  }
  return validateConfig(parsed);
}
