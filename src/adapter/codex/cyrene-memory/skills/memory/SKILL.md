---
name: memory
description: Use for explicit `$memory` requests to inspect, search, list, edit, enable, disable, archive, or delete Cyrene procedural memories. Memory review and automatic writing are not supported in v1.
---

# Manage Cyrene memories

Map the user's request to the narrowest tool:

- Recent applied memories: `cyrene_memory_used`.
- Broad lookup: `cyrene_memory_search`; browsing/filtering: `cyrene_memory_list`; exact lookup: `cyrene_memory_get`.
- Create, edit, enable/disable/archive, or delete: first call `cyrene_memory_change_prepare` with the exact operation and fields.

For every change, show the Core preview and ask the user for explicit confirmation. Call `cyrene_memory_change_commit` only after confirmation, using the returned `change_id`. A delete commit is permanent and must be described clearly before confirmation.

Do not use direct Core CRUD actions, invent IDs, or modify memory text while selecting a match. `memory review`, relationship governance, and automatic task-success writing are outside v1; state that limitation plainly if requested.
