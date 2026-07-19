export const PROTOCOL_VERSION = "1.0";

export interface PiAdapterConfig {
  core_url: string;
  core_path: string;
  data_dir: string;
  actor_id: string;
  token: string;
  protocol_version: string;
}

export interface MemoryContent {
  type: "rule" | "procedure";
  text?: string;
  steps?: string[];
}

export interface MemoryDraft {
  name: string;
  content: MemoryContent;
  index?: {
    actions?: string[];
    objects?: string[];
    task_types?: string[];
    environments?: string[];
    tools?: string[];
    keywords?: string[];
    retrieval_text?: string;
  };
}

export interface Memory extends MemoryDraft {
  id: string;
  status: "enabled" | "disabled" | "archived";
  source_type: "user" | "agent";
  source_agent?: string | null;
  body_version: number;
  embedding_status: "pending" | "ready" | "failed";
  created_at: string;
  updated_at: string;
}

export interface QueryDescription {
  stage?: "task" | "operation";
  action?: string;
  object?: string;
  task_type?: string;
  environment?: string[];
  tools?: string[];
  keywords?: string[];
  explicit_constraints?: string[];
}

export interface SearchHit {
  memory: Memory;
  score: number;
  sources: string[];
}

export interface MatchCandidates {
  match_id: string;
  expires_at: string;
  hits: SearchHit[];
  degraded: boolean;
  degradation_reason?: string | null;
}

export interface MatchSelection {
  match_id: string;
  status: "accepted" | "rejected";
  accepted_ids: string[];
  rejected: Array<{ id?: string; code: string }>;
  retryable: boolean;
}

export interface ChangePreview {
  change_id: string;
  expires_at: string;
  preview: unknown;
}

export interface RpcErrorBody {
  code: string;
  message: string;
}

export interface RpcEnvelope<T> {
  request_id: string;
  protocol_version: string;
  ok: boolean;
  data?: T;
  error?: RpcErrorBody;
}

