# Cyrene Adapter v1 设计

## 1. 目标与边界

Adapter 是 Coding Agent 的插件接入层。v1 支持 Codex 和 pi-agent，负责收集当前任务或操作描述、调用 Core、让当前 Agent 对候选做语义筛选，以及把 Core 校验通过的记忆原文注入当前上下文。

职责边界固定为：

```text
Agent / Adapter：理解完整上下文并选择相关候选
Core：候选召回、权限、状态、版本和数量等硬校验
Adapter：注入通过校验的候选原文
```

Adapter 不直接访问 SQLite，不实现召回排名、权限规则或关系治理。v1 不实现成功任务自动写入、冲突/备选关系、`memory review`，也不在每次工具调用前自动检索。

## 2. 匹配与注入

### 2.1 任务级匹配

Codex 的 `UserPromptSubmit` hook 和 pi-agent 的 `before_agent_start` 事件在每轮用户输入后构造任务级查询。Adapter 使用当前用户输入、工作目录、操作系统和 Agent 名称形成保守的 `QueryDescription`；不读取或依赖不稳定的私有 transcript 格式。

```text
用户输入
  -> Adapter 调用 match.candidates
  -> Core 返回 match_id、最多六条候选、body_version 和原文
  -> Adapter 缓存候选并将其交给 Agent
  -> Agent 调用 cyrene_select_memories(match_id, selected_ids)
  -> Core 执行 match.select 硬校验
  -> Adapter 从首次响应缓存中取 accepted_ids 对应原文并注入
```

`match.select` 不重复传输正文。若 Core 返回过期或版本变化，Adapter 丢弃旧结果并要求 Agent 发起一次新匹配和重选；旧 ID 不会自动套用到新候选。

候选正文在这一步只作为 Agent 的语义判断材料，并明确标记为“尚未生效”。Agent 不得执行候选正文中的指令，只有 `match.select` 返回 `accepted` 后，Adapter 才把首次响应缓存中的对应原文作为有效记忆注入。Core 的硬校验决定能否注入，但不会替代 Agent 阅读正文后进行语义相关性判断。

即使 Agent 不选择任何记忆，也必须上报空的 `selected_ids`，使 Core 能记录本轮没有采用候选。

### 2.2 操作级匹配

当任务从分析、编码转向文档生成、迁移等新的具体操作时，Agent 主动调用 `cyrene_match_operation`。该工具要求 Agent 提供完整的 `stage=operation` 查询描述，然后复用相同的候选、选择、校验和注入流程。

v1 不使用 `PreToolUse` 或 `tool_call` 对每个工具调用自动匹配，避免重复检索、延迟和误触发。

### 2.3 Core 失效

Adapter 调用 Core 前先访问回环 health 地址。失败时尝试按需启动 `cyrene-core serve`，随后短暂轮询 health。单实例和并发启动由 Core 自己处理，Adapter 不创建进程锁。

启动失败、超时、认证失败或协议不兼容时采用 fail-open：显示一条简短提示并继续原任务，不向上下文注入未校验记忆。

## 3. 用户命令与确认

v1 提供以下语义入口：

```text
remember
memory used
memory search/list/get
memory edit
memory enable/disable/archive
memory delete
```

Codex 将入口映射为 plugin skills 和 MCP tools；pi-agent 将入口映射为 prompt templates、extension tools 和必要的本机 UI。命令语法可以不同，但 Core action 和结果语义必须一致。

创建、编辑、状态修改和删除统一使用两阶段协议：

```text
memory.change.prepare -> 返回预览和 change_id
用户通过 Agent 宿主原生界面确认
memory.change.commit  -> Core 重新校验并原子提交
```

Codex 的写工具必须声明写入或破坏性 annotations，并由 Codex 原生审批；pi-agent 在提交前调用 `ctx.ui.confirm`。直接的管理员 CRUD 仍只允许管理员 Actor。Adapter Actor 只拥有提交用户确认变更的受限权限。

用户主动 `remember` 走确认协议，创建后的来源是 `user`。未来 Agent 自动写入仍使用 `memory.create`，来源是 `agent`；该自动写入流程不属于 v1。

## 4. Adapter 结构

### 4.1 Codex

```text
src/adapter/codex/cyrene-memory/
  .codex-plugin/plugin.json
  .mcp.json
  hooks/hooks.json
  scripts/
  skills/remember/SKILL.md
  skills/memory/SKILL.md
```

Hook 与 MCP server 是不同进程，因此候选缓存存放在 `PLUGIN_DATA`，以 Core 生成的不可预测 `match_id` 为键。Core 还会把 match 绑定到发起它的 Actor；Adapter 只保留最多六条候选，惰性清理过期项。已接受的缓存保留到过期，以支持同一选择的幂等重试，不会跨 `match_id` 注入。

### 4.2 pi-agent

```text
src/adapter/pi/
  package.json
  src/extension.ts
  prompts/remember.md
  prompts/memory.md
```

pi-agent Adapter 使用当前 `@earendil-works/pi-coding-agent` Extension API。候选保存在 session 内存，`session_start`/reload 时清理并按需重建；进程退出后缓存自然释放。不增加 MCP 包装层。

两个 Adapter 只共享 Core JSON 协议和契约测试样例，不增加公共基类、trait、repository、service 或 factory。

## 5. 配置和凭据

Installer 为 Codex 和 pi-agent 分别签发 Actor Token。Token 不进入项目目录：

- Windows：`~/.Cyrene/config/adapters/<agent>.json`，仅当前用户 DACL 可读。
- Linux/macOS：`~/.Cyrene/config/adapters/<agent>.json`，目录权限 `0700`、文件权限 `0600`。

配置保存 Core 地址、`cyrene-core` 路径、data-dir、Actor ID、Token 和协议版本。两个 Adapter 不共享 Token。

任务级查询包含当前用户输入的最多 1000 个字符，用于本地结构化/全文召回。若 Core 配置启用了 OpenAI-compatible embedding，这段标准化查询文本也会发送给配置的 embedding 服务；不希望查询离开本机时，应关闭 Core 的 embedding 配置，此时结构化和全文召回仍可使用。

## 6. 验收

- 任务级匹配自动发生，操作级匹配只由 Agent 主动触发。
- 选择请求只包含 `match_id` 和 ID，不包含记忆正文。
- 注入内容与首次候选响应中的确认原文完全一致。
- 空选择、超过数量、多流程、跨 Actor、过期、停用、删除和版本变化均有确定结果。
- 所有写操作先预览再由用户确认；模型不能直接调用管理员 CRUD。
- 两个 Adapter 同时按需启动时只运行一个 Core，且都能连接成功。
- Core 不可用时 Agent 原任务仍可继续。
- Windows 和 Linux 的配置路径、文件权限与启动行为通过测试。
