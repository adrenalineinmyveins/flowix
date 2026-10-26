import { invoke } from '@tauri-apps/api/core';

// F4 本机互咨 IPC（后端 commands/consult.rs）。cwd = 笔记本根目录绝对路径。
// 事件为 tagged union：kind = 'result' | 'failed' | 'askback' | 'budgetUsed' | 'started'。

export type ConsultEvent =
  | { kind: 'result'; answer: string; artifactSuggestion: string | null }
  | { kind: 'failed'; reason: string }
  | { kind: 'askback'; question: string; options: string[] }
  | { kind: 'budgetUsed'; tokens: number; remaining: number }
  | { kind: 'started'; materialCount: number };

export interface ConsultMeta {
  version: number;
  consultId: string;
  fromAgentId: string;
  toAgentId: string;
  status: string;
  createdAt: number;
  updatedAt: number;
  childThreadId: string | null;
  tokenBudget: number;
  tokensUsed: number;
}

export const consult = {
  /** 发起咨询（异步编排，事件经 events 轮询）。 */
  initiate: (cwd: string, fromAgentId: string, toAgentId: string, question: string) =>
    invoke<ConsultMeta>('consult_initiate', { cwd, fromAgentId, toAgentId, question }),

  /** 拉取 seq 之后的事件（断线重放语义）。 */
  events: (cwd: string, fromAgentId: string, consultId: string, afterSeq?: number) =>
    invoke<{ seq: number; event: ConsultEvent }[]>('consult_events', {
      cwd, fromAgentId, consultId, afterSeq,
    }),

  /** 提交反问答复。 */
  askbackReply: (cwd: string, fromAgentId: string, consultId: string, answer: string) =>
    invoke<boolean>('consult_askback_reply', { cwd, fromAgentId, consultId, answer }),

  /** 取消咨询（幂等）。 */
  cancel: (cwd: string, fromAgentId: string, consultId: string) =>
    invoke<void>('consult_cancel', { cwd, fromAgentId, consultId }),
};
