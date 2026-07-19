---
description: Prepare a reusable memory from the current successful task
---
From the visible current task fragment, propose only reusable operating rules or verified procedures that the user explicitly wants to remember. Do not store raw conversation, source code, logs, secrets, one-off project facts, reasoning, or failure history.

Use `cyrene_memory_change_prepare` with `operation: "create"` for each suitable candidate. Show the returned preview. Then use `cyrene_memory_change_commit`; that tool must obtain my confirmation before anything is saved. Do not auto-write and do not perform a memory review.

