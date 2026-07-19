#!/usr/bin/env node
import { pathToFileURL } from "node:url";

import { cacheMatch } from "../lib/cache.mjs";
import { loadConfig, pluginDataPath } from "../lib/config.mjs";
import { rpc } from "../lib/core-client.mjs";
import { candidateContext, taskDescription } from "../lib/matching.mjs";

export async function handleUserPromptSubmit(input, dependencies = {}) {
  const loadConfigImpl = dependencies.loadConfigImpl ?? loadConfig;
  const rpcImpl = dependencies.rpcImpl ?? rpc;
  const cacheMatchImpl = dependencies.cacheMatchImpl ?? cacheMatch;
  try {
    const config = await loadConfigImpl();
    const prompt = input?.prompt ?? input?.user_prompt ?? "";
    if (!String(prompt).trim()) return { continue: true };
    const match = await rpcImpl(config, "match.candidates", {
      description: taskDescription(prompt, dependencies.platform),
    }, {
      timeoutMs: 4_000,
      attempts: 8,
      initialHealthTimeoutMs: 300,
      startupHealthTimeoutMs: 200,
    });
    if (!match?.match_id) throw new Error("Core response did not include match_id");
    await cacheMatchImpl(pluginDataPath(process.env, config.configPath), match);
    const additionalContext = candidateContext(match);
    if (!additionalContext) return { continue: true };
    return {
      continue: true,
      hookSpecificOutput: {
        hookEventName: "UserPromptSubmit",
        additionalContext,
      },
    };
  } catch (error) {
    return {
      continue: true,
      systemMessage: `Cyrene memory is unavailable; continuing without it. ${safeMessage(error)}`,
    };
  }
}

async function main() {
  const chunks = [];
  for await (const chunk of process.stdin) chunks.push(chunk);
  const input = JSON.parse(Buffer.concat(chunks).toString("utf8") || "{}");
  process.stdout.write(`${JSON.stringify(await handleUserPromptSubmit(input))}\n`);
}

function safeMessage(error) {
  return String(error?.message || error).replace(/cyr_[A-Za-z0-9_-]+/g, "[redacted]");
}

if (import.meta.url === pathToFileURL(process.argv[1] || "").href) {
  main().catch((error) => {
    process.stdout.write(`${JSON.stringify({ continue: true, systemMessage: `Cyrene hook failed: ${safeMessage(error)}` })}\n`);
  });
}
