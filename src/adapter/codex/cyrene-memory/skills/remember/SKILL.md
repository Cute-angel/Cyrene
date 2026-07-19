---
name: remember
description: Use when the user explicitly asks Codex to remember a reusable rule or procedure in Cyrene, including `$remember` requests. Do not trigger for ordinary task execution or automatic task-success writing.
---

# Remember with Cyrene

Create one concise, cross-project procedural memory from the user's explicit request or the verified reusable workflow in the current task.

1. Draft either a rule (`content.type: rule`, `content.text`) or a procedure (`content.type: procedure`, `content.steps`). Do not store conversation transcripts, reasoning, logs, source code, secrets, project facts, one-off values, or failure narratives.
2. Include a short `name` and an `index` with only useful actions, objects, task types, environments, tools, keywords, and retrieval text.
3. Call `cyrene_memory_change_prepare` with `operation: create` and the draft.
4. Show the returned preview to the user and ask for explicit confirmation. Do not treat tool approval alone as content confirmation.
5. Only after confirmation, call `cyrene_memory_change_commit` with the returned `change_id`.

If the prepared change expires or becomes stale, prepare it again and show the new preview. Do not automatically write memories after task success.
