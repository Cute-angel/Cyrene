---
description: Search, inspect, or manage Cyrene memories
argument-hint: "[used|search|list|get|edit|enable|disable|archive|delete] [arguments]"
---
Manage Cyrene memory according to this request: `$ARGUMENTS`.

Use the matching `cyrene_memory_used`, `cyrene_memory_search`, `cyrene_memory_list`, or `cyrene_memory_get` tool for reads. For edit, status, or delete, first use `cyrene_memory_change_prepare`, present its preview, and then use `cyrene_memory_change_commit`, which must ask for my confirmation. If no action was supplied, list enabled memories. Memory review and automatic writing are not supported by this adapter.

