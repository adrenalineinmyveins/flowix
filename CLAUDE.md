# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

Flowix 是一款桌面笔记应用（**Tauri 2 + Rust 后端，React 19 + TS + Tiptap 前端**）。对话 agent 全部为外部 CLI runtime（codex / claude / hermes / opencode / pi / DeepSeek Harness，经 `agent_external/runtime_registry.rs` 统一注册派发）；早期的进程内 LLM runtime（rllm / `openai_compatible` provider）已整体移除。


## 命令

```bash
export PATH="$HOME/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:$PATH"
npm run tauri:dev        # 推荐：独立 dev bundle ID (com.flowix.app.dev / "Flowix Dev")，可与生产 app 并存
npm run tauri:dev:win    # Windows 开发启动：使用 app/flowix-desktop/tauri.windows.dev.conf.json
npm run tauri dev        # ⚠️ 走默认 tauri.conf.json，与生产同 bundle ID (com.flowix.app)，已被生产占住时会立刻 exit 0
npm run dev              # 仅前端 (localhost:1420)
npm run tauri build      # 生产构建
npm run cli:build:dev    # CLI debug build
npm run cli:build:prod   # CLI production build for the current platform
npm run dsh:build:dev    # Build the local dev runtime from the pinned DSH source
npm run tauri:build:prod # Flowix production bundle + updater artifacts
pkill -f "node.*vite" 2>/dev/null   # 端口冲突时
sudo xcode-select -r                 # 首次运行
```

Rust 测试（在 `app/` 目录跑）：

```bash
cd app
cargo test -p flowix-core <module>::tests           # 跑某 crate 某模块
cargo test -p flowix-core <module>::tests::test_xxx # 跑单个
cargo test --workspace --lib                         # 跑全部
```

## Dev / Prod 并存打包

通过差异化 Tauri 配置，让 dev 版与已安装的生产版同时运行：

- **dev**：`npm run tauri:dev` → `app/flowix-desktop/tauri.conf.dev.json` → bundle ID `com.flowix.app.dev` / `Flowix Dev`
- **生产**：`npm run tauri:build:prod` → `tauri.conf.json` + 平台覆盖层 + 签名覆盖层 → 平台专用 `tauri.*.production.local.json` → bundle ID `com.flowix.app` / `Flowix`

`tauri:dev` 通过 `--config` 指向独立配置，**不要**改 `tauri.conf.json` 的 `identifier` / `productName` / `mainBinaryName` / `bundle.macOS.bundleName` —— 这四个字段是生产身份的锚点。`tauri.conf.production.json` 作为覆盖层被 `tauri build --config` 深合并在 `tauri.conf.json` 之上，因此 dev 配置改动不会污染生产链路。

dev 与生产现使用不同 bundle ID（`com.flowix.app.dev` vs `com.flowix.app`），可同时运行且互不冲突（Tauri `app_data_dir` / `tauri-plugin-single-instance` lock 都按 identifier 派生）。代价：dev 首次运行需要重新授予一次 user-selected folder 授权（TCC 按 identifier 记忆授权），prod 已授予的不会带过来。视觉上仍通过 bundle name / 窗口标题区分（`Flowix Dev` vs `Flowix`）。URL scheme `flowix://` 仍共用，让浏览器深链能落到任一已装实例。

### macOS 本地生产包 ad-hoc 签名

构建 macOS 生产包后，如果没有 Developer ID，也要对 bundle 内 sidecar 和 `.app` 做一次本地 ad-hoc codesign，让 `entitlements.plist` 写进可执行产物；否则 security-scoped bookmarks / user-selected folder 权限相关 entitlement 不会实际生效。

```bash
npm run tauri:build:prod

codesign --force --options runtime --sign - \
  --entitlements app/flowix-desktop/entitlements.plist \
  "app/flowix-desktop/target/release/bundle/macos/Flowix.app/Contents/MacOS/flowix-cli"

codesign --force --deep --sign - \
  --entitlements app/flowix-desktop/entitlements.plist \
  "app/flowix-desktop/target/release/bundle/macos/Flowix.app"
```

`--sign -` 是 ad-hoc 签名，只适合本机开发 / 本地试装，不能替代 Developer ID 签名与 notarization。先签 `Contents/MacOS/flowix-cli`，再签外层 `.app`；若实际产物路径不同，以 `target/release/bundle/macos/*.app` 为准。

## macOS 发布流水线（Developer ID 直分发 + Notarization）

production 发版走 Apple Developer ID 直分发（不走 Mac App Store），完整脚本在 `scripts/apple-signing/`：

```
scripts/apple-signing/
├── gen-csr.sh            # 生成 CSR + private key → ~/.flowix-signing/
├── make-p12.sh           # .cer + .key → 可导入 Keychain 的 .p12（legacy 路径）
├── sign-and-notarize.sh  # 完整发版：tauri build → verify final DMG → notary → staple → Gatekeeper verify
└── README.md             # 流程图 + 5 步走 + 隐私边界
```

### 一次性配置

#### 1. Apple Developer Account
- 账号邮箱（注册时绑定的，**不能改**）：Apple ID 主邮箱
- Membership Details → **Legal Entity Name**（精确复制粘贴）
- notary 凭据的源邮箱必须跟 Apple Developer Account 邮箱一致

#### 2. 生成本地私钥 + CSR
```bash
bash scripts/apple-signing/gen-csr.sh \
  "<Apple ID 邮箱>" \
  "<Common Name，ASCII>" \
  "<Legal Entity Name 精确>" \
  "CN"
```
输出：
- `~/.flowix-signing/devid.key`（private key，**永不丢失**）
- `~/.flowix-signing/devid.csr`（后续可重用来 renewal）

#### 3. Apple Developer Portal 创建 Developer ID Application 证书
- 登 <https://developer.apple.com/account/resources/certificates/list>
- `+` → **Developer ID Application**（**不是** Apple Development / Apple Distribution / Developer ID Installer）→ 上传 `devid.csr`
- 下载 `developerID_application.cer`（Safari 可能保存为 `development.cer`，**以 CN 字段的 `Developer ID Application:` 前缀为准**判断）
- 把 .cer 放进 `~/.flowix-signing/`

#### 4. 合 .p12 + 导入 Keychain

```bash
# 合包（绕过 make-p12.sh 交互式 prompt，一次性 inline）
openssl x509 -in ~/.flowix-signing/development.cer -inform DER -out /tmp/devid.pem
PASS="$(openssl rand -base64 24 | tr -dc 'A-Za-z0-9' | head -c 24)"
printf '%s' "$PASS" > ~/.flowix-signing/.p12pass
chmod 600 ~/.flowix-signing/.p12pass

openssl pkcs12 -export \
  -inkey ~/.flowix-signing/devid.key \
  -in /tmp/devid.pem \
  -out ~/.flowix-signing/devid.p12 \
  -name "$(openssl x509 -in /tmp/devid.pem -noout -subject | sed 's/^subject=//')" \
  -passout "file:$HOME/.flowix-signing/.p12pass"
chmod 600 ~/.flowix-signing/devid.p12
rm -f /tmp/devid.pem

# 导入 login keychain（加 -T 让 codesign 不再要求密码）
PASS="$(cat ~/.flowix-signing/.p12pass)"
security import "$HOME/.flowix-signing/devid.p12" \
  -k "$HOME/Library/Keychains/login.keychain-db" \
  -P "$PASS" \
  -T /usr/bin/codesign \
  -T /usr/bin/security \
  -T /usr/bin/codesign_allocate \
  -T /usr/bin/productbuild \
  -A
```

验证：
```bash
security find-identity -v -p codesigning
# 形如：
#   1) A3D249298A0E... "Developer ID Application: <Name> (<TEAMID>)"
```

#### 5. notarytool 凭据存到 Keychain（避免每次粘 App-Specific Password）
```bash
HISTFILE=/dev/null xcrun notarytool store-credentials "flowix-notarize" \
  --apple-id "<Apple ID 邮箱>" \
  --team-id  "<10 位 Team ID>" \
  --password "<App-Specific Password 16 位>"
```

> ⚠️ `store-credentials` 的 profile 名是**位置参数**，不是 `--keychain-profile`（后者是 `submit` 的 flag）。

`App-Specific Password` 在 <https://appleid.apple.com> → Sign-In and Security → App-Specific Passwords → **+**（标签填 `flowix-notarize`）生成，**16 位**仅生成时显示一次。

### 每次发版（一条命令打完）

```bash
cd /Users/rop/Desktop/vibe/flowix-main

PATH="$HOME/.cargo/bin:$PATH" \
APPLE_SIGNING_IDENTITY="Developer ID Application: <Name> (<TEAMID>)" \
APPLE_TEAM_ID="<TEAMID 10 位>" \
APPLE_ID="<Apple ID 邮箱>" \
APPLE_KEYCHAIN_PROFILE="flowix-notarize" \
bash scripts/apple-signing/sign-and-notarize.sh
```

`sign-and-notarize.sh` 内部步骤：

1. `npm run tauri:build:prod`（生成 `.build/cargo-target/<target>/release/bundle/{macos,dmg}/`）
2. 挂载最终 DMG，校验内嵌 `flowix-cli` 的可执行位、架构、Developer ID 与 nested signature
3. 对这个未经后续修改的 DMG 执行 `xcrun notarytool submit --keychain-profile flowix-notarize --wait`
4. `xcrun stapler staple` 钉 ticket，并再次验证 stapler + Gatekeeper
5. 打印 SHA-256 + 最终 DMG 路径

CLI staging binary 在 Tauri 打包前准备；Tauri 随后负责封装并签名 nested CLI 和外层 `.app`，再生成 DMG。DMG 生成后禁止重新签或修改 `.app`，否则公证的将不是最终实际分发内容。


### 4 个 env var 各自去哪

| env var | 给哪步用 |
|---|---|
| `APPLE_SIGNING_IDENTITY` | `scripts/prepare-tauri-production-config.mjs` 写进 `tauri.conf.macos.production.local.json` |
| `APPLE_TEAM_ID` | 同上 |
| `APPLE_ID` | notarytool 备用（仅 keychain-profile 模式不需要） |
| `APPLE_KEYCHAIN_PROFILE` | notarytool 走 keychain auth（推荐）；可替换为 `APPLE_APP_SPECIFIC_PASSWORD` fallback |

### 关键路径与发现

| 知识点 | 详细 |
|---|---|
| **CARGO_TARGET_DIR** | `scripts/build-cli.mjs` 把它设到 `$REPO_ROOT/.build/cargo-target`，**不是** `app/flowix-desktop/target/release`。`sign-and-notarize.sh` 内部已经按这个路径找 |
| **Tauri 自带 notarization 跳过** | Tauri 读 `APPLE_PASSWORD` env var 才走内部 notarize。我们不设，Tauri warn 但不拒；手动 `notarytool submit` 在 `sign-and-notarize.sh` 里完成 |
| **updater 私钥** | macOS 本地由 `com.flowix.minisign.private-key` Keychain 条目提供；发布脚本不会把私钥写入仓库或磁盘 |
| **已发布 DMG 的签名 cert 寿命** | 6 个月 Developer ID Application cert（Apple 写死，不能 1 年） |

### 半年后续 cert（renewal）

Developer ID Application cert **有效期 6 个月，Apple 写死，不支持 1 年**。到期前 ~2 周 revoke 旧 cert，用同一个 `.key` 重出一份 `.csr` 重新颁发：

```bash
# 1. 用现成私钥重出 CSR（也可换新 key，Apple 都接受）
openssl req -new -key ~/.flowix-signing/devid.key \
  -out ~/.flowix-signing/devid-v2.csr \
  -subj "/emailAddress=<Apple ID 邮箱>/CN=<Common Name>/O=<Legal Entity>/C=CN"

# 2. Apple Developer Portal: Certificates → 勾旧 cert → Revoke → + → Developer ID Application → Upload devid-v2.csr → 下载新 .cer

# 3. 重复上文「一次性配置 #4」即可（make-p12 + import keychain）
```

新 cert 的 SHA-1 跟旧的不同，但 CN 和 Team ID 不变，所以 `APPLE_SIGNING_IDENTITY` 字符串**不变**（codesign 内部按 SHA-1 找）。

### 故障排除

#### `xcrun notarytool store-credentials`: Unknown option '--keychain-profile'
profile 名是**位置参数**：
```bash
xcrun notarytool store-credentials "flowix-notarize" --apple-id ... --team-id ... --password ...
```
（submit/log/info 才是 `-p, --keychain-profile` flag。）

#### Keychain Access 报「The specified item could not be found in the keychain」/「在钥匙串中找不到指定的项」
不要双击 `.cer` 单独导入。改成 `.cer + .key → .p12 → security import` 整套。具体走本文档「一次性配置 #4」。`.p12` PKCS#12 把证书 + 私钥作为整体入 Keychain，绕开单 .cer 跟 private key 不在同一个 keychain 引起的查找失败。

#### Tauri build 时跳过 notarization（看到 `Warn skipping app notarization` 日志）
Tauri 读 `APPLE_PASSWORD`，我们不设。这是预期行为；真正的 notarize 在 `sign-and-notarize.sh` 里跑 `notarytool submit`。

#### Notarytool submission 卡 In Progress（超 60 分钟超 Apple SLA）
- 正常情况下 1-5 分钟 verdict
- 同一 hash 多次提交可能在 Apple 后端 stuck
- **不要尝试 cancel** —— Apple 没给 client-side cancel API
- 处理路径：
  1. 等 Apple SRE 自动清（4-24 小时常见）
  2. 重新 `notarytool submit`（不同 submission ID，可能 front-of-queue）
  3. 切到 **App Store Connect API key auth**（`--key /Users/<user>/.appstoreconnect/private_keys/AuthKey_XXXXXX.p8 --key-id XXXXXX --issuer UUID`），走不同 auth 后端
  4. **fallback**：发布**已签未 notarize** 的 DMG，让用户跑 `xattr -d com.apple.quarantine /Applications/Flowix.app` 手工旁路 Gatekeeper（公开分发不能这么做，内部/技术用户可以）

#### `entitlements.plist` 那 3 条 JIT 相关 entitlement 在 notarization 被 Apple 审到
答复模板（Apple 公认可接受）：
> The application uses JIT-compiled and runtime-generated executable memory for the embedded WebKit-based editor surface (Tauri WebView) and a Rust-native LLM runtime. `allow-jit` and `allow-unsigned-executable-memory` are required for runtime code generation in these components. `disable-library-validation` is required to load third-party Rust dynamic libraries (`rllm` and dependencies) that are not signed by Apple. The main binary remains properly signed with a valid Apple Developer ID Application certificate.

#### `xcrun altool` 在新 macOS 不存在
从 macOS Sequoia 起 `altool` 被移除，**notarytool 是唯一可用 API 通道**。如果看到 `unable to find utility "altool"` 就是这条。

#### `PATH` 找不到 `cargo`
每次发版 bash 调用前手动 `PATH="$HOME/.cargo/bin:$PATH"`（notarytool 也不在 `$PATH` 默认内会找不到）。或在 `~/.zshrc` 里 `export PATH="$HOME/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:$PATH"` 永久加。

### 永远不会过期但**永久保密**

下列任何一项**泄露 = 撤销旧 cert + 重建整套**：
- `~/.flowix-signing/devid.key`（private key）
- `~/.flowix-signing/devid.p12`（cert + key 合包）
- `~/.flowix-signing/.p12pass`（chmod 600）
- macOS Keychain 里 `flowix-notarize` profile
- App-Specific Password 16 位 token
- `APPLE_APP_SPECIFIC_PASSWORD` env var 内容

`scripts/apple-signing/README.md` 里有完整的「never paste into chat」清单。

## Rules

- 在非常确信情况下再进行代码修改
- 保持专业架构设计，不写垃圾代码

## 架构图

```
flowix-main/
├── app/                                  # Rust workspace
│   ├── Cargo.toml                        # workspace 清单
│   │
│   ├── flowix-core/                      # 业务核心（零 Tauri 依赖，CLI + Desktop 共享）
│   │   └── src/
│   │       ├── lib.rs                    # crate 入口
│   │       ├── search.rs                 # 全文搜索
│   │       └── memo_file/                # 笔记存储层
│   │           ├── mod.rs                # 模块入口
│   │           ├── content.rs            # 内容读写
│   │           ├── frontmatter.rs        # 元数据头
│   │           ├── index_store.rs        # 索引存储
│   │           ├── notebook.rs           # 笔记本
│   │           ├── ops.rs                # CRUD
│   │           ├── derivation.rs         # 派生计算
│   │           ├── registration.rs       # 注册管理
│   │           ├── types.rs              # 类型定义
│   │           ├── time.rs               # 时间工具
│   │           └── tests.rs              # 单元测试
│   │
│   ├── flowix-desktop/                   # Tauri 2 桌面壳
│   │   ├── tauri.conf.json               # Tauri 配置（+ dev / production 覆盖层）
│   │   ├── build.rs                      # 构建脚本
│   │   ├── binaries/                     # CLI sidecar 产物
│   │   └── src/
│   │       ├── main.rs                   # 应用入口
│   │       ├── lib.rs                    # 装配 run()
│   │       ├── app/                      # Tauri 装配（bootstrap / state）
│   │       ├── agent_external/           # 外部 agent runtime（全部对话 agent）
│   │       │   ├── runtime_registry.rs   # ExternalRuntimeKind 注册表 + chat 派发
│   │       │   ├── lifecycle.rs / shared.rs / cli_resolver.rs
│   │       │   ├── codex/                # Codex（app_server 协议）
│   │       │   ├── claude/               # Claude CLI
│   │       │   ├── hermes/  opencode/  pi/
│   │       │   ├── deepseek_harness/     # DSH 宿主（manager / host_registry / app_server）
│   │       │   └── mobile.rs             # 云同步跨端会话（OpenAI 兼容 SSE 直连）
│   │       ├── agent_session/            # 会话存储（ThreadManager + 迁移）
│   │       ├── agent_wire.rs             # 前端事件线协议（AgentChunk 等）
│   │       ├── agent_types.rs            # AgentId / 默认 agent
│   │       ├── agent_lifecycle.rs / agent_history.rs / agent_external_config.rs
│   │       ├── nurture/                  # 个人 Agent 喂养（profile 事件闸 / runner）
│   │       ├── consult/                  # 内置咨询入口
│   │       ├── redaction/                # 披露与中继（身份卡体系）
│   │       │   ├── directory.rs          # 身份卡市场发布 / 搜索 / 下架（AGENT-MARKET M1）
│   │       │   ├── subscriptions.rs      # 订阅快照 CRUD（M3）
│   │       │   ├── consult_bridge.rs     # consult_remote 回环 HTTP 桥（FD-12）
│   │       │   └── consult_registry.rs / auto_grant.rs
│   │       ├── plugin.rs                 # 工具策略（consult_remote verdict 等）
│   │       ├── watcher/                  # 文件监听管线
│   │       │   ├── manager.rs / runtime.rs / processor.rs / event.rs
│   │       │   ├── path.rs / whitelist.rs / tombstone.rs / rename_tracker.rs
│   │       │   └── filter/               # 过滤管线（防抖 / ID 去重 / 自写过滤）
│   │       ├── config/                   # 用户配置
│   │       ├── maintenance/              # 维护任务
│   │       └── commands/                 # Tauri IPC 命令
│   │           ├── memo.rs / notebook.rs / tag.rs / file.rs / dialog.rs / kv.rs / window.rs
│   │           ├── agent/                # 代理命令（chat / runtime / model_catalog / terminal…）
│   │           ├── thread.rs / settings.rs / cloud.rs / export.rs / dsh.rs…
│   │           ├── agent_profile.rs      # 身份卡 CRUD（.agents/<id>/profile.md）
│   │           ├── market.rs             # 身份卡市场 IPC（M1 发布 / M3 订阅）
│   │           └── redaction.rs / consult.rs / skill.rs / plugin.rs…
│   │
│   ├── flowix-cli/                       # CLI sidecar（Tauri shell 调用）
│   │   └── src/
│   │       ├── main.rs                   # CLI 入口
│   │       ├── lib.rs                    # 子命令派发
│   │       ├── editor.rs                 # 外部编辑器
│   │       ├── store.rs                  # 复用 core
│   │       ├── paths.rs                  # 路径解析
│   │       ├── fmt.rs                    # 输出格式
│   │       └── errors.rs                 # 错误定义
│   │
│   └── flowix-web/                       # React 19 + Vite 前端
│       ├── entrypoints/                  # desktop.tsx / main.ts（入口挂载）
│       ├── features/                     # 功能域（状态随域走 features/*/store）
│       │   ├── agent/                    # 对话卡 thread-card / 市场 market / 个人 Agent profile / runtime 配置
│       │   ├── editor/                   # Tiptap 编辑器壳 + extensions（mention / agent-reference / slash-menu…）
│       │   ├── memo/                     # 笔记列表与导航
│       │   ├── document/                 # 文档容器与会话
│       │   ├── workspace/                # 工作区导航 / 焦点
│       │   ├── shell/                    # 三栏主布局
│       │   ├── preferences/  properties/  collection/  plugin/  notifications/…
│       │   └── multidimensional-table/  media-library/  onboarding/  shortcuts/  theme/  surface/
│       ├── lib/                          # settings-store / tauri client / i18n / shortcuts / toast
│       ├── platform/tauri/client/        # IPC 封装（按域拆分：agent / market / agent-profile / notes…）
│       ├── shared/                       # 共享 UI 组件
│       └── styles/  types/  mock/
│
├── scripts/
│   ├── build-cli.mjs                     # 编 CLI sidecar
│   ├── gen-icon.mjs                      # 生成图标
│   ├── prepare-tauri-production-config.mjs  # env var → tauri.macos.production.local.json
│   ├── build-tauri-production.mjs       # cli:build + prepare + tauri build 的编排
│   ├── verify-macos-release.sh            # 从最终 DMG 验证权限、架构、签名、公证与 Gatekeeper
│   ├── release.sh                         # 构建产物校验、R2 发布与网站部署
│   └── apple-signing/                    # Developer ID + notarization 流水线
│       ├── gen-csr.sh
│       ├── make-p12.sh
│       ├── sign-and-notarize.sh
│       └── README.md
│
├── vite.config.ts                        # Vite 配置
├── tailwind.config.js                    # Tailwind
├── tsconfig.json                         # TS 配置
└── package.json                          # 前端清单
```

**说明：**
- **`flowix-core`** 是纯 Rust 库，无 Tauri 依赖，被 `flowix-desktop` 与 `flowix-cli` 共享 —— CLI 通过 sidecar 形式打包进 desktop binaries 目录。
- **`flowix-desktop`** 负责 Tauri 装配：commands（IPC）、agent_external（全部对话 agent 的外部 runtime 注册 / 生命周期 / 会话投影）、nurture（个人 Agent 喂养）、redaction（身份卡市场 / 订阅 / consult 桥）、watcher（文件监听管线）。历史架构中的 `providers/`（openai_compatible）、`prompt/`、`open_target/` 等内置 LLM 路径已随 runtime 外部化移除。
- **`flowix-web`** 单窗口（main）：state 用 Zustand（按功能域放在 `features/*/store`），编辑器用 Tiptap + Shiki，IPC 走 `platform/tauri/client/`（按域拆分）。
- 顶层 `skills/`、`dist/`、`node_modules/`、`app/target/` 为产物 / 资源 / 衍生目录，已省略。
