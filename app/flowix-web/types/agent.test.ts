import { describe, expect, it } from 'vitest';

import { AGENT_TYPE_KEYS } from './agent';

/**
 * TA1（AGENT-ASSET-ROADMAP P0-B）：前端 `AGENT_TYPE_KEYS` 与 Rust
 * `ExternalRuntimeKind::ALL` 的 key() 集合必须一致（预告位除外）。
 * 下方数组是 Rust 侧镜像 —— 变更 runtime_registry.rs 时同步这里，
 * 反之亦然；两侧漂移会让本测试变红。
 */
const RUST_RUNTIME_KEYS = [
  'codex',
  'claude',
  'hermes',
  'opencode',
  'pi',
  'deepseek-harness',
  'mobile',
] as const;

/** 前端预告位（releaseStatus: coming-soon），Rust 侧暂无 runtime。 */
const FRONTEND_ONLY_KEYS = ['gemini', 'openclaw'] as const;

describe('AGENT_TYPE_KEYS 对齐（TA1）', () => {
  it('contains no duplicates', () => {
    expect(new Set(AGENT_TYPE_KEYS).size).toBe(AGENT_TYPE_KEYS.length);
  });

  it('covers every Rust runtime key', () => {
    for (const key of RUST_RUNTIME_KEYS) {
      expect(AGENT_TYPE_KEYS).toContain(key);
    }
  });

  it('differs from Rust only by the coming-soon whitelist', () => {
    const frontend = new Set<string>(AGENT_TYPE_KEYS);
    for (const key of RUST_RUNTIME_KEYS) frontend.delete(key);
    expect([...frontend].sort()).toEqual([...FRONTEND_ONLY_KEYS].sort());
  });
});
