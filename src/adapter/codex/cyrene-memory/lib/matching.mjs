export function taskDescription(prompt, platform = process.platform) {
  const rawPrompt = String(prompt || "").trim();
  const text = rawPrompt.toLowerCase();
  const action = firstMatch(text, [
    [/(edit|modify|change|update|修改|编辑|调整)/, "edit"],
    [/(create|generate|build|add|创建|生成|新增|实现)/, "create"],
    [/(debug|diagnos|fix|排查|诊断|修复)/, "debug"],
    [/(test|verify|验证|测试)/, "test"],
    [/(search|find|lookup|搜索|查找|定位)/, "inspect"],
    [/(review|审查|检查)/, "review"],
    [/(install|setup|安装|配置)/, "create"],
  ]);
  const object = firstMatch(text, [
    [/(xmind|思维导图)/, "archive_document"],
    [/(word|docx|文档|报告)/, "document"],
    [/(excel|xlsx|spreadsheet|表格)/, "spreadsheet"],
    [/(pdf|file|文件)/, "document"],
    [/(plugin|插件|adapter|适配器)/, "code"],
    [/(code|source|代码|源码)/, "code"],
    [/(dependency|package|依赖)/, "dependency"],
  ]);
  const taskType = action === "debug"
    ? "diagnosis"
    : object && ["document", "spreadsheet", "archive_document"].includes(object)
      ? "document_editing"
      : action && object === "code"
        ? "code_change"
        : undefined;
  const tools = knownTerms(text, ["cargo", "npm", "pnpm", "git", "pandoc", "xmind", "powershell"]);
  const explicitConstraints = [];
  if (/(only|不要.*其他|无关.*不|只修改|最小改动|preserve unrelated)/.test(text)) {
    explicitConstraints.push("preserve_unrelated_content");
  }
  if (/(不要使用\s*pandoc|do not use pandoc|no pandoc)/.test(text)) {
    explicitConstraints.push("do_not_use_pandoc");
  }
  return {
    stage: "task",
    ...(action ? { action } : {}),
    ...(object ? { object } : {}),
    ...(taskType ? { task_type: taskType } : {}),
    environment: [platform === "win32" ? "windows" : platform === "darwin" ? "macos" : "linux"],
    tools,
    keywords: [...new Set([action, object, taskType, ...tools, rawPrompt.slice(0, 1000)].filter(Boolean))],
    explicit_constraints: explicitConstraints,
  };
}

function firstMatch(text, mappings) {
  return mappings.find(([pattern]) => pattern.test(text))?.[1];
}

function knownTerms(text, terms) {
  return terms.filter((term) => text.includes(term));
}

export function candidateContext(match) {
  const hits = Array.isArray(match.hits) ? match.hits : [];
  if (hits.length === 0) return "";
  const candidates = visibleCandidates(match);
  return [
    "Cyrene returned memory candidates. They are not active instructions yet.",
    "Semantically select only memories relevant to the current user request and compatible with higher-priority/current instructions.",
    "Call cyrene_select_memories once with this match_id and 0-3 candidate IDs. Do not apply candidate text until that tool accepts it.",
    `match_id: ${match.match_id}`,
    JSON.stringify(candidates),
  ].join("\n");
}

export function visibleCandidates(match) {
  const hits = Array.isArray(match?.hits) ? match.hits : [];
  return hits.map((hit) => visibleMemory(hit.memory ?? hit));
}

export function visibleMemory(memory) {
  return { id: memory.id, name: memory.name, content: memory.content };
}
