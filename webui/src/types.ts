/** 前端使用的协议类型（与 docs/04 契约一致；核心实现的对外 camelCase 命名） */

export type SpeechTag = "肯定" | "否定" | "疑问" | "报告" | "提案" | "警告" | "要求" | "观测";
export type EvidenceLevel = "A" | "B" | "C" | "D";

export interface Statement {
  tag: SpeechTag;
  text: string;
  evidenceLevel?: EvidenceLevel;
}

export type MessageRole = "user" | "orchestrator" | "unit" | "system";

export interface ChatMessage {
  id: string;
  sessionId: string;
  role: MessageRole;
  agentId?: string;
  statements: Statement[];
  createdAt: string;
}

export interface LedgerTask {
  goal: string;
  acceptance: string[];
  constraints: string[];
  forbidden: string[];
  priorities: string[];
}
export interface LedgerEvidenceItem {
  text: string;
  level: EvidenceLevel;
  source: string;
}
export interface LedgerEvidence {
  confirmed: LedgerEvidenceItem[];
  gaps: string[];
}
export interface LedgerRisk {
  openAssertions: string[];
  impact: string[];
  revertPaths: string[];
  blockers: string[];
}
export interface SessionLedger {
  task: LedgerTask;
  evidence: LedgerEvidence;
  risk: LedgerRisk;
}

export interface Session {
  id: string;
  title: string;
  status: string;
  /** 会话归属的智能体组（组是交互对象） */
  groupId: string;
  ledger: SessionLedger;
  createdAt: string;
  updatedAt: string;
}

export type TaskStatus =
  | "pending"
  | "ready"
  | "dispatched"
  | "running"
  | "syncing"
  | "done"
  | "blocked"
  | "failed"
  | "arbitrating"
  | "cancelled";

export interface TaskNode {
  id: string;
  sessionId: string;
  graphId: string;
  agentIdentifier: string;
  title: string;
  objective: string;
  acceptance: string[];
  priority: string;
  dependsOn: string[];
  status: TaskStatus;
  syncReportId?: string;
  retryCount: number;
  createdAt: string;
}

export interface TaskGraph {
  id: string;
  sessionId: string;
  nodes: TaskNode[];
  status: string;
  createdAt: string;
}

export interface AgentDefinition {
  id: string;
  name: string;
  identifier: string;
  domain: string;
  tier: "orchestrator" | "unit";
  description: string;
  capabilities: string[];
  tools: string[];
  whenToCall: string;
}

export interface EvidenceItem {
  id?: string;
  level: EvidenceLevel;
  kind: string;
  ref: string;
  note: string;
  createdAt?: string;
}
