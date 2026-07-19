#!/usr/bin/env node
import readline from "node:readline";
import { pathToFileURL } from "node:url";

import { callTool, TOOL_DEFINITIONS } from "../lib/tools.mjs";

const serverInfo = { name: "cyrene-memory", version: "0.1.0" };
const SUPPORTED_PROTOCOL_VERSIONS = ["2025-06-18", "2025-03-26", "2024-11-05"];

export async function handleMessage(message, dependencies = {}) {
  if (!message || typeof message !== "object" || message.jsonrpc !== "2.0" || typeof message.method !== "string") {
    return rpcError(message?.id ?? null, -32600, "Invalid Request");
  }
  if (message.method === "initialize") {
    if (typeof message.params?.protocolVersion !== "string") {
      return rpcError(message.id ?? null, -32602, "initialize requires protocolVersion");
    }
    const protocolVersion = SUPPORTED_PROTOCOL_VERSIONS.includes(message.params.protocolVersion)
      ? message.params.protocolVersion
      : SUPPORTED_PROTOCOL_VERSIONS[0];
    return {
      jsonrpc: "2.0",
      id: message.id,
      result: {
        protocolVersion,
        capabilities: { tools: { listChanged: false } },
        serverInfo,
      },
    };
  }
  if (message.method === "notifications/initialized") return null;
  if (message.method === "ping") return { jsonrpc: "2.0", id: message.id, result: {} };
  if (message.method === "tools/list") {
    return { jsonrpc: "2.0", id: message.id, result: { tools: TOOL_DEFINITIONS } };
  }
  if (message.method === "tools/call") {
    const tool = TOOL_DEFINITIONS.find((candidate) => candidate.name === message.params?.name);
    if (!tool) return rpcError(message.id ?? null, -32602, "Unknown tool");
    const validationError = validateSchema(tool.inputSchema, message.params?.arguments ?? {});
    if (validationError) return rpcError(message.id ?? null, -32602, validationError);
    try {
      const data = await (dependencies.callToolImpl ?? callTool)(
        message.params?.name,
        message.params?.arguments ?? {},
      );
      return {
        jsonrpc: "2.0",
        id: message.id,
        result: {
          content: [{ type: "text", text: JSON.stringify(data, null, 2) }],
          structuredContent: { data },
        },
      };
    } catch (error) {
      return {
        jsonrpc: "2.0",
        id: message.id,
        result: {
          content: [{ type: "text", text: safeMessage(error) }],
          isError: true,
        },
      };
    }
  }
  if (message.id === undefined) return null;
  return {
    jsonrpc: "2.0",
    id: message.id,
    error: { code: -32601, message: `Method not found: ${message.method}` },
  };
}

function rpcError(id, code, message) {
  return { jsonrpc: "2.0", id, error: { code, message } };
}

function validateSchema(schema, value, path = "arguments") {
  if (schema.enum && !schema.enum.includes(value)) return `${path} must be one of ${schema.enum.join(", ")}`;
  if (schema.type === "object") {
    if (!value || typeof value !== "object" || Array.isArray(value)) return `${path} must be an object`;
    for (const required of schema.required ?? []) {
      if (value[required] === undefined) return `${path}.${required} is required`;
    }
    if (schema.additionalProperties === false) {
      const unknown = Object.keys(value).find((key) => !Object.hasOwn(schema.properties ?? {}, key));
      if (unknown) return `${path}.${unknown} is not allowed`;
    }
    for (const [key, item] of Object.entries(value)) {
      const child = schema.properties?.[key];
      if (child) {
        const error = validateSchema(child, item, `${path}.${key}`);
        if (error) return error;
      }
    }
  } else if (schema.type === "array") {
    if (!Array.isArray(value)) return `${path} must be an array`;
    if (schema.maxItems !== undefined && value.length > schema.maxItems) return `${path} has too many items`;
    if (schema.uniqueItems && new Set(value.map((item) => JSON.stringify(item))).size !== value.length) {
      return `${path} must contain unique items`;
    }
    for (let index = 0; index < value.length; index += 1) {
      const error = validateSchema(schema.items, value[index], `${path}[${index}]`);
      if (error) return error;
    }
  } else if (schema.type === "string") {
    if (typeof value !== "string") return `${path} must be a string`;
    if (schema.minLength !== undefined && value.length < schema.minLength) return `${path} is too short`;
  } else if (schema.type === "integer") {
    if (!Number.isInteger(value)) return `${path} must be an integer`;
  } else if (schema.type === "number" && typeof value !== "number") {
    return `${path} must be a number`;
  }
  if (typeof value === "number") {
    if (schema.minimum !== undefined && value < schema.minimum) return `${path} is below minimum`;
    if (schema.maximum !== undefined && value > schema.maximum) return `${path} is above maximum`;
  }
  return null;
}

function safeMessage(error) {
  return String(error?.message || error).replace(/cyr_[A-Za-z0-9_-]+/g, "[redacted]");
}

async function main() {
  const lines = readline.createInterface({ input: process.stdin, crlfDelay: Infinity });
  for await (const line of lines) {
    if (!line.trim()) continue;
    let response;
    try {
      response = await handleMessage(JSON.parse(line));
    } catch (error) {
      response = { jsonrpc: "2.0", id: null, error: { code: -32700, message: safeMessage(error) } };
    }
    if (response) process.stdout.write(`${JSON.stringify(response)}\n`);
  }
}

if (import.meta.url === pathToFileURL(process.argv[1] || "").href) {
  main();
}
