# Cyrene Core v1 基础协议

本文描述当前 `cyrene-core` 基础实现的启动方式、认证边界和公开调用协议。协议版本为 `1.0`。

## 初始化与运行

首次初始化：

```powershell
cargo run -p cyrene-core -- --data-dir .\local-data init --admin-name owner
```

命令会创建 `config.toml`、SQLite 数据库和首个管理员 Actor，并且只显示一次管理员 Token。数据库只保存 Argon2 哈希。

HTTP 模式只允许监听回环地址：

```powershell
$env:OPENAI_API_KEY = "..."
cargo run -p cyrene-core -- --data-dir .\local-data serve
```

默认地址为 `127.0.0.1:46371`。`GET /v1/health` 不需要认证，`POST /v1/rpc` 必须携带：

```text
Authorization: Bearer <Cyrene Actor Token>
```

`serve` 会在 data-dir 上取得跨平台单实例锁。多个 Adapter 并发按需启动时只有一个 Core 继续监听；其他启动请求输出 `already_running` 后正常退出，随后都连接同一个回环地址。

JSON Lines stdio 模式从环境变量读取 Token，每行接受一个请求并返回一行响应：

```powershell
$env:CYRENE_ACCESS_TOKEN = "cyr_..."
$env:OPENAI_API_KEY = "..."
cargo run -p cyrene-core -- --data-dir .\local-data stdio
```

`OPENAI_API_KEY` 只由进程读取，不写入配置或数据库。默认配置下没有设置时，结构化和全文检索仍然可用，语义索引保持 `pending`。

Embedding 使用 OpenAI-compatible 的 `POST /v1/embeddings` 协议。`config.toml` 的默认配置仍连接 OpenAI：

```toml
[embedding]
enabled = true
base_url = "https://api.openai.com/v1"
api_key_env = "OPENAI_API_KEY"
model = "text-embedding-3-small"
dimensions = 512
request_timeout_seconds = 30
```

`base_url` 是 API 根地址，Core 会在其后追加 `/embeddings`；`api_key_env` 是读取 API Key 的环境变量名，不会把密钥写入配置。若服务不鉴权，可将其设为空字符串。例如 Ollama：

```toml
[embedding]
enabled = true
base_url = "http://localhost:11434/v1"
api_key_env = ""
model = "nomic-embed-text"
dimensions = 768
request_timeout_seconds = 30
```

vLLM 可使用同样的格式，将 `base_url` 改为其 OpenAI-compatible 服务地址（通常为 `http://localhost:8000/v1`），并按服务启动参数决定是否配置 `api_key_env`。`dimensions` 必须与服务实际返回的向量维度一致。切换模型或维度后，执行 `index.rebuild` 会重建不匹配的已有向量。

## 请求与响应

两个传输共用同一个请求 envelope：

```json
{
  "request_id": "req-1",
  "protocol_version": "1.0",
  "action": "memory.get",
  "payload": { "id": "01900000-0000-7000-8000-000000000000" }
}
```

成功响应：

```json
{
  "request_id": "req-1",
  "protocol_version": "1.0",
  "ok": true,
  "data": {}
}
```

失败响应：

```json
{
  "request_id": "req-1",
  "protocol_version": "1.0",
  "ok": false,
  "error": {
    "code": "not_found",
    "message": "resource not found: memory ..."
  }
}
```

## 记忆数据

规则正文：

```json
{
  "name": "保持最小改动范围",
  "content": {
    "type": "rule",
    "text": "修改现有内容时，只修改请求涉及的部分。"
  },
  "index": {
    "actions": ["edit"],
    "objects": ["code"],
    "task_types": ["code_change"],
    "environments": ["windows"],
    "tools": [],
    "keywords": ["最小改动"],
    "retrieval_text": "修改已有代码并保持无关行为不变"
  }
}
```

流程正文将 `content` 替换为：

```json
{
  "type": "procedure",
  "steps": ["创建备份。", "解包目标文件。", "替换指定内容。", "重新打包。"]
}
```

## Actions

| Action | 权限 | Payload |
| --- | --- | --- |
| `health` | 已认证 Actor | `{}` |
| `memory.create` | `can_create` | 记忆数据 |
| `memory.get` | `can_read` | `{ "id": UUID }` |
| `memory.list` | `can_read` | 可选 `status`、`kind`、`source_agent`、`limit`、`offset` |
| `memory.update` | 管理员 | `{ "id": UUID, "draft": 记忆数据 }` |
| `memory.set_status` | 管理员 | `{ "id": UUID, "status": "enabled|disabled|archived" }` |
| `memory.delete` | 管理员 | `{ "id": UUID }` |
| `search.manual` | `can_read` | 手动查询、查询描述、筛选和分页 |
| `match.candidates` | `can_read` | `{ "description": 标准查询描述 }` |
| `match.select` | `can_read` | `{ "match_id": UUID, "selected_ids": [UUID] }` |
| `memory.change.prepare` | `can_confirm_user_changes` | 带 `operation` 标签的 create、update、set_status 或 delete 变更 |
| `memory.change.commit` | `can_confirm_user_changes` | `{ "change_id": UUID }` |
| `index.status` | `can_read` | `{}` |
| `index.rebuild` | 管理员 | `{}` |
| `actor.create` | 管理员 | Agent 名称、类型以及 `can_read`、`can_create`、`can_confirm_user_changes` 权限 |
| `actor.list` | 管理员 | `{}` |
| `actor.revoke` | 管理员 | `{ "id": UUID }` |
| `core.shutdown` | 管理员 | `{}`；让 HTTP Core 完成当前响应后优雅退出 |

Agent 可以读取和在获得 `can_create` 授权后自动新增记忆，但不能直接修改、停用、归档、删除已有记忆或管理 Actor。Installer 可以为受信任的 Adapter Actor 授予 `can_confirm_user_changes`；该权限只能调用下面的两阶段用户确认协议，不能调用管理员 CRUD。来源类型与来源 Agent 由认证身份和调用路径生成，调用方不能覆盖。

## 检索行为

`match.candidates` 同时执行结构化、FTS5 trigram 全文和 OpenAI-compatible embedding 语义召回，各路最多取 30 条，以 `2.0 / 1.2 / 1.0` 权重进行 RRF 融合，最终返回最多 6 条。

启用 embedding 且鉴权配置可用时，Core 会把由查询描述拼成的检索文本发送给配置的 OpenAI-compatible embeddings API；该文本可能包含 Adapter 截断后的当前用户输入。关闭 `embedding.enabled`，或 `api_key_env` 非空但对应环境变量没有配置时，不发送远程请求，并降级为结构化与全文召回。

自动匹配响应除了 `hits` 和降级状态，还返回短期有效的 `match_id` 与 `expires_at`。每个 hit 中的记忆已有 `body_version`；Adapter 应缓存首次响应，不应在选择请求中回传正文或版本。

选择请求示例：

```json
{
  "match_id": "01900000-0000-7000-8000-000000000001",
  "selected_ids": ["01900000-0000-7000-8000-000000000002"]
}
```

Core 原子校验候选归属、Actor、最多三条、最多一条流程、当前状态和正文版本。全部通过才返回：

```json
{
  "match_id": "01900000-0000-7000-8000-000000000001",
  "status": "accepted",
  "accepted_ids": ["01900000-0000-7000-8000-000000000002"],
  "rejected": [],
  "retryable": false
}
```

业务校验失败时 `status` 为 `rejected`、`accepted_ids` 为空，并在 `rejected` 中返回 `not_a_candidate`、`too_many_selected`、`too_many_procedures`、`memory_not_enabled` 或 `body_version_changed` 等代码。失败不会消费 match，Adapter 可以让 Agent 重选；成功和空选择会消费 match。重复提交相同选择是幂等的。match 默认十分钟过期。

## 用户确认变更

Adapter 内的主动记忆、编辑、状态修改和删除不使用管理员 CRUD，而是先准备变更：

```json
{
  "operation": "set_status",
  "id": "01900000-0000-7000-8000-000000000002",
  "status": "disabled"
}
```

`memory.change.prepare` 返回 `change_id`、`expires_at` 和包含修改前后内容的 `preview`，但不修改记忆。Adapter 必须先使用 Agent 宿主的原生界面让用户确认，再调用：

```json
{ "change_id": "01900000-0000-7000-8000-000000000003" }
```

`memory.change.commit` 会重新校验 Actor、有效期和目标记忆版本，然后原子执行或返回 conflict。相同 change 的成功提交可安全重试。通过该流程创建的记忆来源为 `user`；直接 `memory.create` 的来源仍由 Agent Actor 记录为 `agent`。

语义模型、API 根地址和向量维度由 `[embedding]` 配置。远程请求失败时，响应中的 `degraded` 为 `true`，并继续返回结构化和全文结果。`index.rebuild` 会分批补齐所有 `pending` 记忆，以及模型或维度与当前配置不一致的已有向量。
