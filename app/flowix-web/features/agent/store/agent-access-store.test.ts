import { beforeEach, describe, expect, it, vi } from "vitest";

const agentAccessMock = vi.hoisted(() => ({
  config: {
    version: 1,
    entries: [] as Array<Record<string, unknown>>,
  } as {
    version: number;
    entries: Array<Record<string, unknown>>;
    defaults?: { runtime?: { pi?: { piCodeMode?: boolean } } };
  },
  get: vi.fn(),
  set: vi.fn(),
  addFolderFromPicker: vi.fn(),
}));

vi.mock("@platform/tauri/client", () => ({
  agentAccess: {
    get: agentAccessMock.get,
    set: agentAccessMock.set,
    addFolderFromPicker: agentAccessMock.addFolderFromPicker,
  },
}));

type TestEntry = {
  id: string;
  kind: "folder" | "notebook";
  path: string;
  name: string;
  enabled: boolean;
  workspace: boolean;
  missing: boolean;
  addedAt: number;
  updatedAt: number;
};

function makeFolder(
  overrides: Partial<TestEntry> & { id: string; path: string },
): TestEntry {
  return {
    id: overrides.id,
    kind: "folder",
    path: overrides.path,
    name: overrides.name ?? overrides.path,
    enabled: overrides.enabled ?? true,
    workspace: overrides.workspace ?? false,
    missing: overrides.missing ?? false,
    addedAt: overrides.addedAt ?? 1,
    updatedAt: overrides.updatedAt ?? 1,
  };
}

function entriesWithWorkspace(items: TestEntry[]): {
  version: 1;
  entries: TestEntry[];
} {
  return { version: 1, entries: items };
}

describe("agent-access-store workspace selection", () => {
  beforeEach(async () => {
    vi.clearAllMocks();
    agentAccessMock.config = { version: 1, entries: [] };
    agentAccessMock.get.mockImplementation(async () => agentAccessMock.config);
    agentAccessMock.set.mockImplementation(async (config) => {
      agentAccessMock.config = config;
    });
    agentAccessMock.addFolderFromPicker.mockImplementation(async () => null);
    const { useAgentAccessStore } =
      await import("@features/agent/store/agent-access-store");
    useAgentAccessStore.setState({
      config: { version: 1, entries: [] },
      isLoading: false,
      hasLoaded: false,
      loadError: null,
    });
  });

  it('exposes initial load failures and clears them after retry', async () => {
    const { useAgentAccessStore } = await import('@features/agent/store/agent-access-store');
    agentAccessMock.get.mockRejectedValueOnce(new Error('read failed'));
    await useAgentAccessStore.getState().loadInitial();
    expect(useAgentAccessStore.getState()).toMatchObject({
      hasLoaded: false, isLoading: false, loadError: 'read failed',
    });
    agentAccessMock.config = {
      version: 1, entries: [], defaults: { runtime: { pi: { piCodeMode: true } } },
    };
    await useAgentAccessStore.getState().loadInitial();
    expect(useAgentAccessStore.getState()).toMatchObject({
      hasLoaded: true, isLoading: false, loadError: null,
      config: agentAccessMock.config,
    });

    agentAccessMock.get.mockRejectedValueOnce(new Error('refresh failed'));
    await useAgentAccessStore.getState().loadInitial();
    expect(useAgentAccessStore.getState()).toMatchObject({
      hasLoaded: true, isLoading: false, loadError: 'refresh failed',
      config: agentAccessMock.config,
    });
  });

  it.each([true, false])('keeps the newest overlapping snapshot (newest resolves first: %s)', async (newestFirst) => {
    const { useAgentAccessStore } = await import('@features/agent/store/agent-access-store');
    let resolveOld!: (config: typeof agentAccessMock.config) => void;
    let resolveNew!: (config: typeof agentAccessMock.config) => void;
    agentAccessMock.get
      .mockReturnValueOnce(new Promise((resolve) => { resolveOld = resolve; }))
      .mockReturnValueOnce(new Promise((resolve) => { resolveNew = resolve; }));
    const oldLoad = useAgentAccessStore.getState().loadInitial();
    const newLoad = useAgentAccessStore.getState().loadInitial();
    const oldConfig = { version: 1 as const, entries: [], defaults: { runtime: { pi: { piCodeMode: false } } } };
    const newConfig = { version: 1 as const, entries: [], defaults: { runtime: { pi: { piCodeMode: true } } } };
    if (newestFirst) {
      resolveNew(newConfig);
      await newLoad;
      resolveOld(oldConfig);
      await oldLoad;
    } else {
      resolveOld(oldConfig);
      await oldLoad;
      expect(useAgentAccessStore.getState().hasLoaded).toBe(false);
      resolveNew(newConfig);
      await newLoad;
    }
    expect(useAgentAccessStore.getState()).toMatchObject({
      config: newConfig, hasLoaded: true, isLoading: false, loadError: null,
    });
  });

  it("does NOT auto-promote a folder to workspace on initial load", async () => {
    // 新契约: workspace 完全由 setWorkspace 显式触发, loadInitial 不会把
    // 第一个 enabled folder 自动升为主空间。 磁盘原值原样落库。
    const { useAgentAccessStore } =
      await import("@features/agent/store/agent-access-store");
    agentAccessMock.config = entriesWithWorkspace([
      makeFolder({
        id: "folder-1",
        path: "D:\\projects\\first",
        name: "First",
      }),
      makeFolder({
        id: "folder-2",
        path: "D:\\projects\\second",
        name: "Second",
      }),
    ]);

    await useAgentAccessStore.getState().loadInitial();

    const entries = useAgentAccessStore.getState().config.entries;
    expect(entries.every((entry) => entry.workspace === false)).toBe(true);
  });

  it("keeps a Pi writing selection made while initial settings are loading", async () => {
    const { useAgentAccessStore } =
      await import("@features/agent/store/agent-access-store");
    let resolveGet!: (config: typeof agentAccessMock.config) => void;
    agentAccessMock.get.mockReturnValueOnce(
      new Promise((resolve) => { resolveGet = resolve; }),
    );

    const load = useAgentAccessStore.getState().loadInitial();
    await useAgentAccessStore.getState().setDefaultRuntime("pi", {
      piCodeMode: false,
    });
    resolveGet({
      version: 1,
      entries: [{ id: "loaded-entry" }],
      defaults: { runtime: { pi: { piCodeMode: true } } },
    });
    await load;

    expect(
      useAgentAccessStore.getState().config.defaults?.runtime?.pi?.piCodeMode,
    ).toBe(false);
    expect(useAgentAccessStore.getState().config.entries).toEqual([
      { id: "loaded-entry" },
    ]);
    expect(agentAccessMock.config.defaults?.runtime?.pi?.piCodeMode).toBe(false);
  });


  it("addFolder does not persist the removed workspace field", async () => {
    const { useAgentAccessStore } =
      await import("@features/agent/store/agent-access-store");
    useAgentAccessStore.setState({
      config: { version: 1, entries: [] },
      isLoading: false,
    });

    const result = await useAgentAccessStore
      .getState()
      .addFolder("D:\\projects\\new");
    expect(result.ok).toBe(true);

    const entries = useAgentAccessStore.getState().config.entries;
    expect(entries).toHaveLength(1);
    expect(entries[0]?.enabled).toBe(true);
    expect(entries[0]?.workspace).toBeUndefined();
  });

  it("does not write legacy global file defaults without a notebook id", async () => {
    const { useAgentAccessStore } =
      await import("@features/agent/store/agent-access-store");
    const result = await useAgentAccessStore.getState().setDefaultFiles(undefined, {
      workspace: null,
      folders: ["D:\\projects\\legacy"],
      notebooks: [],
    });
    expect(result).toBe(false);
    expect(agentAccessMock.set).not.toHaveBeenCalled();
    expect(useAgentAccessStore.getState().config.defaults?.files).toBeUndefined();
  });
});

describe("agent-access-store tag scope", () => {
  beforeEach(async () => {
    vi.clearAllMocks();
    agentAccessMock.config = { version: 1, entries: [] };
    agentAccessMock.get.mockImplementation(async () => agentAccessMock.config);
    agentAccessMock.set.mockImplementation(async (config) => {
      agentAccessMock.config = config;
    });
    const { useAgentAccessStore } =
      await import("@features/agent/store/agent-access-store");
    useAgentAccessStore.setState({
      config: { version: 1, entries: [] },
      isLoading: false,
      hasLoaded: false,
      loadError: null,
    });
  });

  it("addTagScope creates a tag entry (kind=tag, path empty, tagPath set) and persists", async () => {
    const { useAgentAccessStore } =
      await import("@features/agent/store/agent-access-store");
    const result = await useAgentAccessStore.getState().addTagScope("finance/财报");
    expect(result.ok).toBe(true);
    if (!result.ok) return;

    const entries = useAgentAccessStore.getState().config.entries;
    expect(entries).toHaveLength(1);
    expect(entries[0]).toMatchObject({
      kind: "tag",
      path: "",
      tagPath: "finance/财报",
      name: "finance/财报",
      enabled: true,
      missing: false,
    });
    expect(agentAccessMock.set).toHaveBeenCalledTimes(1);
  });

  it("addTagScope trims slashes and rejects empty paths", async () => {
    const { useAgentAccessStore } =
      await import("@features/agent/store/agent-access-store");
    expect((await useAgentAccessStore.getState().addTagScope("  ")).ok).toBe(false);
    const result = await useAgentAccessStore.getState().addTagScope("/finance/财报/");
    expect(result.ok).toBe(true);
    if (!result.ok) return;
    expect(result.entry.tagPath).toBe("finance/财报");
  });

  it("addTagScope returns already-tracked for a duplicate tagPath without writing", async () => {
    const { useAgentAccessStore } =
      await import("@features/agent/store/agent-access-store");
    const first = await useAgentAccessStore.getState().addTagScope("finance/财报");
    expect(first.ok).toBe(true);
    const second = await useAgentAccessStore.getState().addTagScope("finance/财报");
    expect(second).toMatchObject({ ok: false, code: "already-tracked" });
    expect(useAgentAccessStore.getState().config.entries).toHaveLength(1);
    expect(agentAccessMock.set).toHaveBeenCalledTimes(1);
  });

  it("addTagScope rolls back the optimistic entry when the write fails", async () => {
    const { useAgentAccessStore } =
      await import("@features/agent/store/agent-access-store");
    agentAccessMock.set.mockRejectedValueOnce(new Error("disk full"));
    const result = await useAgentAccessStore.getState().addTagScope("industry/电商");
    expect(result).toMatchObject({ ok: false, code: "save-failed" });
    // 回滚走 loadInitial, 内存回到磁盘真值 (空 entries)。
    expect(useAgentAccessStore.getState().config.entries).toHaveLength(0);
  });
});
