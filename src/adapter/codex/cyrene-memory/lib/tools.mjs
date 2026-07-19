import { cacheMatch, memoriesFromMatch, readMatch, readUsed, recordUsed } from "./cache.mjs";
import { loadConfig, pluginDataPath } from "./config.mjs";
import { rpc } from "./core-client.mjs";
import { visibleCandidates, visibleMemory } from "./matching.mjs";

const objectSchema = (properties, required = []) => ({
  type: "object",
  properties,
  required,
  additionalProperties: false,
});

const queryDescriptionSchema = objectSchema({
  stage: { type: "string" },
  action: { type: "string" },
  object: { type: "string" },
  task_type: { type: "string" },
  environment: { type: "array", items: { type: "string" } },
  tools: { type: "array", items: { type: "string" } },
  keywords: { type: "array", items: { type: "string" } },
  explicit_constraints: { type: "array", items: { type: "string" } },
});

const annotations = (readOnlyHint, destructiveHint = false) => ({
  readOnlyHint,
  destructiveHint,
  openWorldHint: false,
});

export const TOOL_DEFINITIONS = [
  {
    name: "cyrene_match_operation",
    description: "Recall Cyrene candidates when beginning a distinct operation or task phase. Candidates are not active until selected.",
    inputSchema: objectSchema({ description: queryDescriptionSchema }, ["description"]),
    annotations: annotations(false),
  },
  {
    name: "cyrene_select_memories",
    description: "Submit 0-3 semantically relevant candidate IDs for Core hard validation, then return only accepted cached originals.",
    inputSchema: objectSchema({
      match_id: { type: "string", minLength: 1 },
      selected_ids: { type: "array", maxItems: 3, uniqueItems: true, items: { type: "string" } },
    }, ["match_id", "selected_ids"]),
    annotations: annotations(false),
  },
  {
    name: "cyrene_memory_used",
    description: "Show recently accepted Cyrene memories for this Codex adapter.",
    inputSchema: objectSchema({ limit: { type: "integer", minimum: 1, maximum: 100 } }),
    annotations: annotations(true),
  },
  {
    name: "cyrene_memory_search",
    description: "Search procedural memories manually with broad recall.",
    inputSchema: objectSchema({
      query: { type: "string" },
      description: queryDescriptionSchema,
      filters: objectSchema({
        status: { type: "string", enum: ["enabled", "disabled", "archived"] },
        kind: { type: "string", enum: ["rule", "procedure"] },
        source_agent: { type: "string" },
      }),
      limit: { type: "integer", minimum: 1, maximum: 200 },
      offset: { type: "integer", minimum: 0 },
    }, ["query"]),
    annotations: annotations(true),
  },
  {
    name: "cyrene_memory_list",
    description: "List Cyrene memories using optional status, kind, and source filters.",
    inputSchema: objectSchema({
      status: { type: "string", enum: ["enabled", "disabled", "archived"] },
      kind: { type: "string", enum: ["rule", "procedure"] },
      source_agent: { type: "string" },
      limit: { type: "integer", minimum: 1, maximum: 200 },
      offset: { type: "integer", minimum: 0 },
    }),
    annotations: annotations(true),
  },
  {
    name: "cyrene_memory_get",
    description: "Get one Cyrene memory by ID.",
    inputSchema: objectSchema({ id: { type: "string", minLength: 1 } }, ["id"]),
    annotations: annotations(true),
  },
  {
    name: "cyrene_memory_change_prepare",
    description: "Prepare and preview a user-confirmed create, update, status, or delete change. This does not commit the memory change.",
    inputSchema: objectSchema({
      operation: { type: "string", enum: ["create", "update", "set_status", "delete"] },
      id: { type: "string" },
      draft: { type: "object" },
      status: { type: "string", enum: ["enabled", "disabled", "archived"] },
    }, ["operation"]),
    annotations: annotations(false),
  },
  {
    name: "cyrene_memory_change_commit",
    description: "Commit a prepared Cyrene change after the user has explicitly confirmed its preview.",
    inputSchema: objectSchema({ change_id: { type: "string", minLength: 1 } }, ["change_id"]),
    annotations: annotations(false, true),
  },
];

export async function callTool(name, args = {}, dependencies = {}) {
  const loadConfigImpl = dependencies.loadConfigImpl ?? loadConfig;
  const rpcImpl = dependencies.rpcImpl ?? rpc;
  const config = await loadConfigImpl();
  const dataDir = dependencies.dataDir ?? pluginDataPath(process.env, config.configPath);

  switch (name) {
    case "cyrene_match_operation": {
      const match = await rpcImpl(config, "match.candidates", { description: args.description });
      if (!match?.match_id) throw new Error("Core response did not include match_id");
      await (dependencies.cacheMatchImpl ?? cacheMatch)(dataDir, match);
      return {
        match_id: match.match_id,
        expires_at: match.expires_at,
        candidates: visibleCandidates(match),
        degraded: Boolean(match.degraded),
      };
    }
    case "cyrene_select_memories": {
      const match = await (dependencies.readMatchImpl ?? readMatch)(dataDir, args.match_id);
      const result = await rpcImpl(config, "match.select", {
        match_id: args.match_id,
        selected_ids: args.selected_ids,
      });
      const acceptedIds = result?.status === "accepted" && Array.isArray(result.accepted_ids)
        ? result.accepted_ids
        : [];
      const memories = memoriesFromMatch(match, acceptedIds);
      if (acceptedIds.length !== memories.length) {
        throw new Error("accepted memory is missing from the local candidate cache; match again");
      }
      if (result?.status === "accepted") {
        await (dependencies.recordUsedImpl ?? recordUsed)(dataDir, {
          match_id: args.match_id,
          accepted_ids: acceptedIds,
          memories,
        });
      }
      return { ...result, memories: memories.map(visibleMemory) };
    }
    case "cyrene_memory_used":
      return { used: await (dependencies.readUsedImpl ?? readUsed)(dataDir, args.limit) };
    case "cyrene_memory_search":
      return rpcImpl(config, "search.manual", {
        query: args.query,
        description: args.description ?? {},
        filters: args.filters ?? {},
        limit: args.limit ?? 50,
        offset: args.offset ?? 0,
      });
    case "cyrene_memory_list":
      return rpcImpl(config, "memory.list", compact(args));
    case "cyrene_memory_get":
      return rpcImpl(config, "memory.get", { id: args.id });
    case "cyrene_memory_change_prepare":
      return rpcImpl(config, "memory.change.prepare", changePayload(args));
    case "cyrene_memory_change_commit":
      return rpcImpl(config, "memory.change.commit", { change_id: args.change_id });
    default:
      throw new Error(`unknown Cyrene tool: ${name}`);
  }
}

function changePayload(args) {
  switch (args.operation) {
    case "create":
      return { operation: "create", draft: required(args.draft, "draft") };
    case "update":
      return { operation: "update", id: required(args.id, "id"), draft: required(args.draft, "draft") };
    case "set_status":
      return { operation: "set_status", id: required(args.id, "id"), status: required(args.status, "status") };
    case "delete":
      return { operation: "delete", id: required(args.id, "id") };
    default:
      throw new Error(`unsupported change operation: ${args.operation}`);
  }
}

function required(value, name) {
  if (value === undefined || value === null || value === "") throw new Error(`${name} is required`);
  return value;
}

function compact(value) {
  return Object.fromEntries(Object.entries(value).filter(([, item]) => item !== undefined));
}
