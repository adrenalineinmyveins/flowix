# Flowix 安卓 App 执行方案

> 版本：v1.0（2026-10-17）
> 状态：已定稿，待启动 M0
> 前置评估结论：基于现有云同步体系的安卓客户端，低风险、高复用

## 1. 背景与产品决策

Flowix 桌面版（Tauri 2 + Rust + React 19）已具备生产级云同步能力（`flowix-sync`，V2 协议，后端 `cloud.flowix.cc`）。本方案为安卓版本的开发执行计划。

已确认的产品决策：

| 决策点 | 结论 |
|---|---|
| 安卓端存储模式 | **纯云端**：不做本地笔记管理，本地仅 App 私有目录缓存 |
| 桌面端云端功能 | **现有同步即所需**，桌面端零新功能开发 |
| 安卓端登录方式 | **M1：邮箱+密码（复用现有 `/v1/auth/login`/`register`，后端零改动）**；M2 升级邮箱验证码 |
| 安卓端离线策略 | 离线优先：缓存可写 + 联网后同步（V2 冲突合并支撑） |
| 技术路线 | **Tauri 2 mobile**（复用 Rust 核心 + React 前端，复用率最高） |

## 2. 总体架构

```
桌面端：本地文件夹笔记本 ──同步──┐
（不改，现有功能即所需）          ├── cloud.flowix.cc (V2 协议 + 新增邮箱验证码登录)
安卓端：仅云笔记本（私有目录缓存）──┘
```

产品流：用户在桌面端为笔记本开启同步 → 安卓端登录同一账号即可查看/编辑。

### 复用与新增盘点

| 层 | 复用 | 新增 |
|---|---|---|
| 后端 | V2 协议、附件 blob、会员体系 | 邮箱验证码登录接口 |
| Rust | `flowix-core`（缓存读写/FTS 搜索）、`flowix-sync`（整套协议，均无 Tauri 依赖） | `flowix-mobile` 薄壳：IPC 命令面、安卓版 `cache_adapter`（远简于桌面版，无 watcher 竞态）、`RefreshTokenStore` 文件实现 |
| 前端 | Tiptap extensions、Zustand stores、主题、`lib/` 工具层 | 移动端单窗口壳（三屏）、触摸/键盘适配 |
| 裁掉（feature-gate） | — | `agent_external`（约 58 文件，Node 子进程）、CLI sidecar、updater、single-instance |

## 3. 里程碑与任务分解

### M0 — 验证 spike（决策门槛，通过才进 M1）

| # | 任务 | 验证标准 |
|---|---|---|
| 0.1 | 开发环境：Android SDK + NDK + JDK，`rustup target add aarch64-linux-android` | 环境可重复搭建，步骤记入脚本 |
| 0.2 | 交叉编译验证：`cargo check -p flowix-core -p flowix-sync --target aarch64-linux-android` | 编译通过。已知必改项见 §4.2 |
| 0.3 | `flowix-mobile` 骨架：`tauri android init`，WebView 渲染占位页，Rust 侧调通 flowix-core 在私有目录建缓存 | 真机/模拟器上 App 启动，能看到私有目录里生成的 SQLite 索引 |
| 0.4 | 同步回路：写死测试账号 token，跑一次 bootstrap → 列笔记 → 改一篇 → push | 桌面端能看到安卓 push 的修改；双向回路闭合 |
| 0.5 | 渲染性能抽查：Tiptap 编辑 + Mermaid 渲染跑在中端真机（骁龙 7 系） | 输入延迟、Mermaid 首帧可接受 |

**止损规则：0.2 或 0.5 失败且无低成本解法 → 停止，评估原生 + UniFFI 备选路线。**

### M1 — MVP（可发布的最小闭环）

**后端（邮箱验证码）：**

- `POST /auth/email/request`、`POST /auth/email/verify`，签发与现有会话模型兼容的 refresh/access token
- 防滥用：验证码 TTL、频率限制、IP 限流、图形验证码兜底
- 账号互通设计：桌面 Apple ID 登录账号与同邮箱验证码登录是否合并为同一账号——后端定稿，客户端无需感知

**Rust（flowix-mobile crate）：**

- 命令面（Tauri IPC）：`auth_*`（发码/验码/登出/会员状态）、`notebook_list`、`memo_list/read/create/update/delete`、`search`、`sync_trigger/status`
- `cache_adapter`：flowix-sync 的安卓版本地适配器（写入方只有 App 自身，无 watcher 竞态）
- `RefreshTokenStore` 实现：MVP 落私有目录文件（Android 沙箱隔离），M2 迁 Android Keystore
- 同步触发策略：App 前台定时拉 + 编辑后防抖 push + 手动下拉刷新

**前端（复用为主，新壳）：**

- 移动端单窗口三屏：笔记列表 → 编辑器 → 设置（账号/同步状态/会员）
- 复用：Tiptap extensions、Zustand stores、主题系统、`lib/` 工具层
- 移动端专项：虚拟键盘 visualViewport 适配、safe-area、触摸手势、列表虚拟滚动（`@tanstack/react-virtual`）
- 不引入：agent 面板、preferences 独立窗口、桌面三栏布局

**构建与分发：**

- 签名 keystore 生成与保管规范（参照 `scripts/apple-signing/` 的密钥纪律：永不入库、泄露即整体重建）
- CI：Windows/macOS 开发机均可出 APK；版本号沿用 workspace 1.5.x

**M1 验收（发布门槛）：**

1. 邮箱收码 → 登录 → 笔记本列表出现（桌面端已开同步的笔记本）
2. 断网编辑多篇 → 恢复网络 → 与桌面端并发修改自动合并，零数据丢失（用 V2 冲突物料 + text_merge 的既有测试场景回归）
3. 全文搜索结果与桌面一致（缓存 FTS）
4. 中端真机冷启动 ≤ 3s、编辑无可感知卡顿
5. 登出后缓存清空，无账号数据残留

### M2 — 商业化与体验

- 会员购买：邮箱账号优先接 Stripe 网页支付（跳外部浏览器）；若上 Play 商店则换 Play Billing——M1 末期二选一
- 附件 blob 同步启用（协议层 `V2BlobReservation` 已就绪，客户端做缓存与 LRU 淘汰）
- 大笔记本懒加载（替代全量 bootstrap）
- Token 迁移 Android Keystore；崩溃上报与日志脱敏（`flowix-redaction` 可复用）
- deep-link `flowix://` intent filter 打开指定笔记

## 4. 关键执行细节

### 4.1 不把移动端加进 `flowix-desktop`

该 crate 深度绑定桌面（agent_external 的 Node 子进程、single-instance、updater、NSIS/公证流水线），cfg-gate 成本高于新建薄壳 `flowix-mobile`。

### 4.2 已知必改项（M0.2）

- **reqwest TLS**：workspace 默认 native-tls（OpenSSL）在 Android 交叉编译失败，切 `rustls-tls` feature。只影响 Android 目标与桌面二进制体积，不影响协议行为
- **dirs crate**：Android 上 home_dir 回退行为需补 cfg 处理，指向 App 私有目录
- **machine-uid / hostname**：安卓行为存疑，M0 验证后替换

### 4.3 协议不动

`PROTOCOL_EPOCH = 2` 不变，安卓是纯客户端，后端除邮箱登录外零改动。

### 4.4 裁剪不删码

桌面专属功能全部 feature-gate，保持桌面链路零回归。

## 5. 风险与应对（执行期盯防项）

| 风险 | 应对 |
|---|---|
| Tauri 2 Android 插件坑（dialog/opener/deep-link 移动端差异） | M0.3 起逐个真机验证，替换为原生插件或自写 Kotlin 桥 |
| WebView 碎片化（低端机内核老旧） | M1 验收项 4 为硬指标；Mermaid/Shiki 做能力检测降级 |
| 全量缓存撑爆存储 | MVP 明示"仅同步开启的笔记本"，M2 懒加载根治 |
| 邮箱验证码被刷 | 后端限流 + 图形验证码兜底，M1 上线前压测 |
| 同步引擎的桌面假设（围绕文件 watcher 设计） | 安卓写入方是 App 自身，竞态更少，预计是简化；M0.4 实测确认 |

## 6. 启动动作

下一步从 **M0.2（aarch64 交叉编译验证）** 开始——成本最低、信息量最大。通过后按 0.3 → 0.4 → 0.5 顺序推进。

---

## 7. M0 执行记录（2026-10-18）

### 状态

| 任务 | 状态 |
|---|---|
| 0.1 环境配置 | ✅ 完成（rust targets ×4、cargo-ndk 4.1.2、JAVA_HOME→AS JBR 21、ANDROID_HOME/NDK_HOME→27.1、adb 入 PATH） |
| 0.2 aarch64 交叉编译 | ✅ 通过（7m21s）。唯一改动：workspace reqwest 切 `rustls-tls-native-roots`；flowix-desktop 改为继承 workspace 定义 |
| 0.3 壳工程 + APK | ✅ 构建成功，产物 `app/flowix-mobile/gen/android/app/build/outputs/apk/universal/debug/app-universal-debug.apk`（151MB debug 版） |
| 0.3 真机验证 | ✅ 通过（2026-10-18）：WebView 渲染 + IPC 调用 + flowix-core 私有目录构造全部 OK |
| 0.4 同步回路 | ⏳ 待做：需要测试账号 token，跑 bootstrap → 编辑 → push → 桌面端可见 |
| 0.5 性能抽查 | ⏳ 待做：中端真机 Tiptap/Mermaid 渲染帧率 |

### Windows 开发机踩坑清单（重装环境必读）

1. **符号链接权限**：Tauri 把 `libflowix_mobile.so` 以符号链接放进 jniLibs，需开启 Windows 开发者模式（注册表 `HKLM\...\AppModelUnlock\AllowDevelopmentWithoutDevLicense=1`）
2. **Gradle 下载超时**：`gen/android/gradle/wrapper/gradle-wrapper.properties` 的 distributionUrl 改华为云镜像；`gen/android/build.gradle.kts` 的 repositories 前置阿里云 maven（google/central/gradle-plugin 三个仓）
3. **Kotlin 跨盘符崩溃**：tauri 的 Kotlin 源在 C: 盘 cargo registry、工程在 E: 盘，`kotlin.incremental` 的 `relativeTo` 报 "different roots"。`gen/android/gradle.properties` 追加 `kotlin.incremental=false` 和 `kotlin.compiler.execution.strategy=in-process`
4. **npm 找不到 tauri 脚本**：Gradle BuildTask 执行 `npm run tauri android android-studio-script`，向上解析到仓库根 package.json（无此脚本）。需 `app/flowix-mobile/package.json` 自带 `{"scripts":{"tauri":"tauri"}}`
5. 注意：2/3 两条改的是 `gen/android` 生成目录，`tauri android init` 重新生成后需重打补丁
6. **DSH memory MCP 锁 sidecar**：DSH dev runtime（`dsh-flowix-memory/launcher.mjs`）常驻运行 `.build/cargo-target/debug/flowix-cli.exe mcp` 且被杀后自动重启，tauri-build `copy_binaries` 删旧副本时报 `PermissionDenied (os error 5)`。规避：`cargo check -p flowix-desktop` 时用独立 `$env:CARGO_TARGET_DIR` 覆盖（验证后删掉临时目录）

### 新增代码位置

- `app/flowix-mobile/`：Cargo.toml、build.rs、src/{main,lib}.rs、tauri.conf.json（identifier `com.flowix.app.mobile`）、capabilities/default.json、web/index.html（占位页）、package.json
- `app/Cargo.toml`：members 增加 `flowix-mobile`；reqwest 切 rustls

---

## 8. M1 执行记录（2026-10-18）

### 状态：代码层全部完成，待真机回路验证（M1-V）与验收（M1-10）

| 任务 | 状态 | 落点 |
|---|---|---|
| M1-1 桌面云集成蓝图 | ✅ | 结论：flowix-sync 零改动全复用；重大发现——**后端已有邮箱+密码登录**（`/v1/auth/login`/`register`），桌面 `cloud_register`/`cloud_login` 命令已存在 |
| M1-2 登录方案 | ✅ | 定稿：M1 用密码登录（后端零改动），邮箱验证码推 M2 |
| M1-3 认证 | ✅ | `cloud.rs`：PrivateFileTokenStore + SyncManager 装配 + run_session_service；命令 login/register/logout/get_state |
| M1-4 缓存适配 | ✅ | `cache.rs`：ensure/snapshot/apply，保留回声抑制+本地领先保护+原子写+先 apply 后提交 cursor |
| M1-5 同步回路 | ✅ | `sync.rs`：全量快照+删除检测+Markdown put 冲突三方合并+bootstrap 分页；自动同步（启动 5s/轮询 30s）+保存后 3s 防抖；SYNC_NOW_LOCK 串行化 |
| M1-6 命令面 | ✅ | `commands.rs`：notebook_list/note_list/read/save/create + 跨笔记本 FTS 搜索 |
| M1-7/8/9 前端 | ✅ | `frontend/` 独立 vite 入口（React 19 + Tiptap 3 + @tiptap/markdown）：登录/笔记列表/编辑器/设置四屏；frontmatter 分离保护；纯 CSS 移动端样式 |
| M1-V 真机回路 | ✅ | 桌面↔安卓双向同步验证（见「三端闭环验证」节） |
| M1-10 验收 | ✅ | 离线编辑→恢复→冲突自动合并（见「M1-10 冲突合并验收」节） |

### M1 新增构建链

- 根 `package.json`：`build:mobile` 脚本（vite 独立配置 `app/flowix-mobile/frontend/vite.config.ts`，产物 `.build/web-mobile-dist`，gzip 201KB）
- `flowix-mobile/tauri.conf.json`：`beforeBuildCommand: npm run build:mobile` + `frontendDist: ../../.build/web-mobile-dist`
- `flowix-mobile/package.json`：`tauri` 与 `build:mobile`（--prefix ../.. 委托到根）两个脚本——Gradle BuildTask 与 tauri beforeBuildCommand 都依赖它

### M1 简化决策（相对桌面版）

- 快照走全量+对账（缓存规模可控），不做脏队列增量快照
- 移动冲突（MoveConflict）M1 报错不自动消解（移动端无重命名 UI）
- 附件编辑冲突保留现场报错，Markdown put 冲突自动三方合并
- 无 watcher：编辑命令内直接 record_v2_local_change，无 self-write 标记

---

## 9. 自建后端重建记录（2026-10-19）

### 背景

原计划连 cloud.flowix.cc，但后端 `flowix-cloud` 源码不可得（私有仓库无权限）。决策：**从零重建兼容后端**，客户端代码（flowix-sync）即协议规范。

### 产物：`app/flowix-cloud`（Rust crate，workspace 成员）

- 技术栈：axum 0.7 + rusqlite（bundled）+ scrypt + tokio，与桌面/移动端统一 Rust 技术栈
- 模块：`db.rs`（schema/游标/用量，notes/notebooks 持久化 sync_seq）、`auth.rs`（scrypt + 令牌轮换 + Bearer 鉴权）、`sync.rs`（V2 核心：status/changes/bootstrap/history/push，含冲突与 create-or-adopt 语义）、`blobs.rs`（预约/代理上传校验哈希/access/下载）、`lib.rs`（路由装配 + 信封助手 + x-request-id 中间件）、`main.rs`
- 端点：auth 全套（注册/登录/refresh 轮换/me/logout）、sync（status/changes/bootstrap/push/history）、blobs（预约/代理上传/下载）、entitlements（100GB 免费）；Apple/Google/结账返回 501
- 启动：`cargo run -p flowix-cloud -- --addr 0.0.0.0:8787 --db ./flowix-cloud.db`（默认 127.0.0.1:8787）
- 直接复用 `flowix-sync` 的 wire 类型（models 可见性放宽为 `pub` 并在 lib.rs 重导出）——客户端类型即服务端类型，天然防契约漂移
- 集成测试 `tests/cloud.rs`（tower oneshot 直连 router，2/2 绿）：
  - `full_protocol_roundtrip`：注册 → refresh 轮换 → blob 预约-上传-下载 → push（notebook+note）→ notebook adopt → REVISION_CONFLICT 冲突（校验 currentRevision）→ 正确 base 重推（replacedRevision）→ changes → bootstrap（syncSeq/usage）→ history → status
  - `error_envelope_and_auth_boundaries`：401 未授权 / 400 参数 / 409 体内冲突 / 401 失效 refresh / 501 未实现
- 历史：第一版为零 npm 依赖 Node 实现（`e:\flowix\flowix-cloud`，用于 M1-V 真机回路验证），验证通过后已删除，由本 Rust 版取代

### 契约关键点（重建时踩过的坑，改后端必读）

1. blob 能力类响应（reservations/access）是 `{data, upload}` **顶层双字段信封**，不是统一 `{data:...}`
2. push 冲突在 **HTTP 200 响应体内**（`ok:false, status:409, error.code=REVISION_CONFLICT, error.details.currentRevision`），不是 HTTP 409
3. `V2BootstrapNote.syncSeq` 必填——实体表要持久化每实体最后 sync_seq
4. **notebook.put 走 create-or-adopt**（base=null + 已存在 → 接受）：移动端认领云端笔记本的 `set_v2_notebook_enabled` 会以 base=null 推 notebook.put，严格检查会死锁客户端（notebook 无三方合并路径）
5. note 内容是内容寻址 blob：push 只带 contentHash，服务端校验 blob 存在（BLOB_MISSING）

### M1-V 双向回路验证（模拟器实测通过）

- 下行：API 推送笔记本+笔记 → 手机 bootstrap → 缓存落盘 → UI 显示 ✓
- 上行：手机 Tiptap 编辑 → 保存 → 3s 防抖自动推送 → 服务端新 revision（97B 含编辑内容）✓
- 环境坑：雷电模拟器（x86_64+ARM 转译，arm64 APK 直接可跑）；不支持 10.0.2.2，用宿主 WLAN IP；Tauri `app_data_dir` 在 Android = `/data/data/<pkg>/` **根目录**（非 files/）

### 客户端修复（本轮发现并落地）

- **TLS**：`rustls-tls-native-roots` → `rustls-tls-webpki-roots`（Android 无 /etc/ssl/certs，native-certs 空库 → 握手失败）
- **锁重入死锁**：flowix-core 排他锁内调 register/prune（内部取共享锁）必死锁；桌面靠 watcher 预建索引绕开，移动端必须**两段式**（持锁做文件操作 → 释放后再做索引注册），涉及 cache.rs apply、sync.rs 冲突消解、commands.rs save/create 三处
- `cloud_api_base` 文件覆盖：App 私有目录（数据根）放该文件即切后端，重启生效

### 待办

- 公网暴露后建议：注册加邀请码或关闭（当前全开放，每人 100GB）
- `CLOUD_CONFLICT_VERSION_UNAVAILABLE` 风暴根因（见三端验证节遗留观察）
- 桌面 apply 死锁修复需合入生产发版（对 cloud.flowix.cc 用户同样致命）

### 部署记录（2026-10-19，8.160.162.251）

- 服务器：Alibaba Cloud Linux 3，x86_64，2C/3.5G，已有 docker（karakeep/Saiye 占 3000，未动）
- 构建：**服务器原生构建**（非交叉编译）。Docker Hub 公共镜像源当晚全部失效（1ms.run 停滞、daocloud 502），改为 rsproxy 装 rustup（`RUSTUP_DIST_SERVER=https://rsproxy.cn`）+ dnf gcc，源码包（3 crate + Cargo.lock + rsproxy 的 .cargo/config.toml）pscp 上传，`cargo build --release` 5m39s
- 产物：`/opt/flowix-cloud/flowix-cloud`（3.5MB，glibc 动态链接）+ `flowix-cloud.db`（SQLite，WAL）
- 常驻：`/etc/systemd/system/flowix-cloud.service`（`--addr 0.0.0.0:8787 --db /opt/flowix-cloud/flowix-cloud.db`，Restart=always，`systemctl enable --now flowix-cloud`）
- 防火墙：firewalld 已 `--permanent --add-port=8787/tcp`
- 服务器本机 `curl 127.0.0.1:8787/healthz` → `{"data":{"ok":true}}` ✓
- **公网 8787 不通**：安全组（控制台层）只放行了 22/3000，80/443/8888/39000-40000 实测全部超时，需控制台加规则
- 踩坑：plink 远程命令里的 `$VAR` 会被本地 PowerShell 先展开，用绝对路径；`pkill -f 'docker xxx'` 会匹配自身命令行导致自杀（拆两条会话或 `[ ]` 转义）
- 安全提醒：当前注册全开放（每人 100GB 配额），公网暴露前建议加邀请码或关闭注册

### 公网端到端验证（2026-10-19，安全组放行 8787 后实测通过）

- **本机 curl 冒烟**：healthz / 注册 200（accessToken+refreshToken）/ status（protocolEpoch=2）/ entitlements（100GB）/ 无令牌 401 ✓
- **模拟器公网回路**（cloud_api_base = `http://8.160.162.251:8787`，pm clear 后全新登录）：
  - 下行：curl 扮演桌面端推 notebook（nb_pub_smoke/PubVerify）+ note（blob 预约→上传→哈希校验→push）→ App ⟳ 下载 2 实体 → create-or-adopt 认领笔记本 → 本地落地 `notebooks/nb_pub_smoke/pub-verify.md` → UI 渲染（笔记本下拉 PubVerify ▾、笔记列表 pub-verify.md）✓
  - 上行：App Tiptap 编辑追加文本 → 保存 → 公网服务器新增 blob（136B，尾部含 `mobile-edit-via-public-cloud-OK`）+ note 新 revision（syncSeq=8）✓
  - 冲突路径顺带验证：App 端三方合并生成冲突副本 `pub-verify (Flowix conflict op_*).md` 并推上服务器（syncSeq=7）✓
  - 会话轮换跨端生效：curl 重新登录把 App 的 refresh token 轮换失效 → App 回落登录页（单会话语义按设计工作）✓
- 后续观察项：移动端本地新建笔记本（首启默认"笔记本"）不会自动上云——移动端定位是消费云端笔记本 + 编辑回推，本地笔记本上云走桌面端路径；旧账号缓存切新账号登录时，被旧云状态绑定的笔记本不推送（账号切换边界，建议后续加"清除本地云状态"或按用户隔离缓存）

### 三端闭环验证（2026-10-19，桌面 + 公网云 + 模拟器实测通过）

- **链路**：桌面（Flowix T3 dev 实例）→ 公网 8.160.162.251:8787 → 模拟器 App，账号 `loop@selfhost.local`
- **桌面→云→移动**：本地 Loop 笔记本（2 篇 md）→ 30s 轮询自动 push（服务器 syncSeq 19-21，notebook+note+blob）→ 移动端登录 loop 后下拉出现 Loop，两篇笔记+摘要完整渲染 ✓
- **移动→云→桌面**：移动端编辑 loop-note-1 追加 `mobile-roundtrip-OK-2026` → 保存推送 → 桌面 30s 轮询拉取，本地 `Documents\flowix\Loop\loop-note-1.md` 出现该文本 ✓
- **桌面端两段式修复**（本次发现并落地的关键 bug）：
  - 症状：桌面同步 30s 轮询每轮 `timed out waiting for notebook shared lock` —— 排他锁内死锁，**任何首次 bootstrap 下载必现**（对生产 cloud.flowix.cc 同样成立）
  - 根因：`remote_apply.rs::apply_v2_note_changes` 持 notebook 排他文件锁期间调用 `register_existing_file_for_notebook_id` / `prune_deleted_memo_for_notebook_id` / `register_preserved_conflict_copy`，这些内部重新获取笔记本共享文件锁——同进程跨 fd 的 flock 互等死锁（移动端 cache/sync/commands 三处早已同款修复，桌面漏了 apply 路径）
  - 修复：两段式——锁内只做纯文件系统变更与冲突决策，索引注册/清理收集进 `DeferredIndexAction`，排他锁释放后统一执行并发事件（`deletion.rs` 拆出 `delete_cloud_note_fs` 文件系统阶段，原 `delete_cloud_note_locked` 保留组合语义，测试零回归 30/30）
- **测试脚手架**（可复用）：
  - `app/flowix-desktop/tauri.conf.t3.dev.json`：独立 identifier `com.flowix.app.t3` + vite 1422，与在跑的其他 dev 实例并存（避开 single-instance 互踢与端口冲突）
  - `device_registration.rs` 新增 `FLOWIX_EXPERIMENTAL=1` 环境变量旁路（boot.json 会被并存的 dev 实例用各自内存态周期性回写，env 是唯一稳定的单实例覆盖；与 `FLOWIX_CLOUD_API_BASE` 同款模式）
  - 状态注入法（绕过 UI 自动化）：`~/.flowix/default.db` secret_kv 写 refresh token + `~/.flowix/sync.db` 写 v2_cloud_account / v2_synced_notebooks(enabled=1) / session_restore_policy(blocked=0)，重启进程即自动恢复登录并同步
- **Windows 踩坑追加**：
  - PowerShell UIA 可穿透 WebView2（需 `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--force-renderer-accessibility` 启动），但坐标点击会被前台锁定/遮挡拦截且难以排查——复杂流程优先用状态注入
  - `~/.flowix`（boot.json/index.db/sync.db/default.db）是**全局共享**的：多个桌面实例并存时锁争用+配置互相回写，测试专用实例务必用独立笔记本避开，必要时清掉干扰实例（含残留的 sidecar `flowix-cli.exe` 孤儿进程）
- 遗留观察：死锁半成品状态会引发 `CLOUD_CONFLICT_VERSION_UNAVAILABLE` 冲突重试风暴（record_rejected_cloud_version 对已有同路径版本的 create 返回 None 分支），换账号清场（全清 v2 状态表）后消失；根因待后续排查

### M1-10 冲突合并验收（2026-10-19，实测通过 ✅）

M1 里程碑收尾：离线并发编辑 → 恢复 → 冲突自动合并，两条路径全过。

- **对端模拟**：`e:\flowix\m1-10\peer.ps1`（PowerShell，repo 外）直接走 HTTP 契约（login → blob 预约/上传 → `note.put` base=当前 revision），充当桌面端。信封坑已修：login 会话在 `data.session.*`、blob 上传能力在**顶层** `upload.*`
- **离线模拟（后端地址切换法）**：`cloud_api_base` 改 `http://127.0.0.1:9` + 重启 App（离线编辑落盘，restore 走可重试网络错误不清会话）→ 对端推云 → 地址改回 + 重启（恢复）。**不用飞行模式**：雷电模拟器上 `cmd connectivity airplane-mode enable` 直接打挂整个系统（adbd 冻死、adb 全灭、实例无法再起）
- **场景 A（分区域编辑 → 三方合并）**：base 双区域 → 移动端离线改 region-mobile 行、对端改 region-desktop 行 → 恢复 → push 体内 409 `REVISION_CONFLICT` → `v2_conflict_material` 拉 base blob → diffy 三方合并 → 主文件**双端修改同存**、零冲突副本 → 合并结果自动回推云端（revision 第三次推进，云端内容 = merged）✓
- **场景 B（同区域编辑 → 冲突副本）**：base 单行、双端改同一行 → 恢复 → 合并失败 → 主文件落远端版本（peer-edit-99），本地版本存 `m1-10-conflict (Flowix conflict op_*).md` → 副本经快照对账（`reconcile_notebook_with_disk_bidirectional` 注册未知 md 文件）→ 下一轮**作为新笔记自动上云** ✓ 双版本零丢失
- **会话窗口管理**（应对"登录即轮换"单会话语义）：重启 App 刷新 access token → 对端 login 一次并缓存复用（每次 login 都作废 App 的 refresh）→ 把对端 refresh token 注入 App `cloud_refresh_token` 文件 → 终末重启 App 从注入 token restore 回健康单会话
- **环境事故与恢复链**（当次实测）：雷电模拟器整机冻死后实例无法再起；Google AVD Saiye_Test（x86_64 API 36）**硬杀丢 qcow2 overlay 未合并数据**（当日安装的 App 全丢）。恢复：`npm run tauri -- android build --target x86_64 --apk --debug`（release-unsigned 可用 debug.keystore zipalign+apksigner 签名直装，但 release 不可 debuggable → run-as 不可用 → 必须 `--debug` 重建）→ run-as 写 `cloud_api_base` → **CDP 登录**（`adb forward tcp:9222 localabstract:webview_devtools_remote_<pid>` + Node WebSocket `Runtime.evaluate` 用原生 value setter 注入 React 受控输入再 click；uiautomator 看不进 WebView DOM）
- 可复用脚手架：`peer.ps1`（status/cat/put，自动解析 Loop 笔记本与 note id）+ `cdp.mjs`（login/state），均在 `e:\flowix\m1-10\`

### 遗留问题清零 + P2 能力补全（2026-10-19 第二轮）

**① CLOUD_CONFLICT_VERSION_UNAVAILABLE 风暴根因（已修复）**
- 根因：`conflict_files.rs::record_rejected_cloud_version` 的 dedup 分支（同一冲突重试时 `create_path_version_bytes` 返回 None）用 `v2_content_hash`（SHA-256 **base64url**）与版本清单里的 **hex** 哈希比对——格式永不相等 → 每次重试都误报。与"死锁半成品反复重试同一冲突"的观察完全吻合
- 修复：flowix-core 新增 `path_version_content_hash()`（清单同款 hex，versions.rs 三处内部用法统一收口），桌面比对改用该函数；新增回归测试 `repeated_rejected_version_recording_is_idempotent`（core versions 2/2 + desktop cloud 31/31 全绿）
- **桌面 apply 死锁修复**（上轮已完成）本轮回归确认，31/31 绿。**发版待办**：修复已在主干，生产发版需在 Mac 上走 `scripts/apple-signing/sign-and-notarize.sh` 流水线（Windows 无法完成 notarization）

**② P2 图片/附件全链路（实测通过）**
- 选图：`<input type="file" accept="image/*">`（wry `RustWebChromeClient` 已实现 `onShowFileChooser`，gen/android 的 Kotlin 自带）；CDP 自动化注意 synthetic click 需 `Runtime.evaluate { userGesture: true }`，否则 Chromium 拒弹 file chooser；`hidden` 属性的 input 不触发选择器，用 CSS 视觉隐藏
- 落盘：`attachment_save` command（base64 → `attachments/<名>`，同名同内容复用、同名不同内容加 hash8 后缀）
- **媒体索引坑**：`refresh_media_resource_path` 会被 FileManagementPolicy 拒绝（默认忽略 `attachments` 目录名）——改用与桌面 `media.rs` 同款 `ensure_media_resource`（不带 policy 检查）
- 同步：附件文件作为**独立实体**推（与桌面 local_snapshot 完全同构，md 笔记的 attachments 元数据清单双端恒为空）；实测 `attachments/18.png` 408B blob + 笔记正文引用（233B）双实体上云
- markdown 语法与桌面互认：`![alt](asset://localhost/<encoded 绝对路径>)`；渲染走 `attachment_read` → dataURL（storageKey 支持绝对路径 / 跨设备按文件名回落到本笔记本 attachments/）
- 已知边界：桌面打开移动端写的图片，storageKey 是安卓绝对路径 → 桌面裂图（文件本身已同步落盘，桌面侧"按文件名回落渲染"为后续增强项，属桌面既有跨设备绝对路径短板）

**③ 历史版本 UI（实测通过）**
- Rust：`note_history` / `note_history_read` 包 `SyncManager::v2_history` / `v2_historical_bytes`（云端 blob 下载校验）
- UI：编辑器顶栏 Clock3 → sheet 列表（时间/字节/最新标记）→ 预览（只读全文）→ 恢复（setContent 复用自动保存链路推云）。实测 m1-10-merge.md 5 个历史版本（base→mobile→peer→merged→restore 全链路可追溯）
- **附带修复一个隐藏 bug**：卸载保护 effect 的 deps 含 `saveState`——dirty 切换的 cleanup 会清掉刚排上的 800ms 自动保存 timer，**单次 setContent**（恢复/插图路径只有一次 onUpdate）被清后永不再保存。修复：saveState 走 ref，deps 只留 performSave

**④ 检查更新（实测通过）**
- 后端：`GET /v2/app/latest`（免认证；systemd drop-in `/etc/systemd/system/flowix-cloud.service.d/latest.conf` 提供 `FLOWIX_APP_LATEST_NAME/_CODE/_URL/_NOTES`，改后 daemon-reload + restart，**已部署**）
- 移动端：`app_check_update`（reqwest GET + 版本比对）+ `app_open_url`（tauri-plugin-opener → 系统浏览器）；设置页"关于"卡（当前版本/检查更新/发现新版本时出现"去下载"+ notes）
- 实测两路径：默认（1.5.6==1.5.6 →"已是最新版本"）与 1.6.0（→"发现新版本 1.6.0"+ 去下载）✓
- 其余踩坑：flowix-cloud 的 blobs.rs/auth.rs 双 BOM（EF BB BF ×2，rustc 只容忍单 BOM）导致本地编译失败，服务器旧版无此问题——编辑器写文件时注意；云端 bash curl 验证登录时 PowerShell 管道写文件默认带 BOM 会毒化注入的 refresh token（38≠35 字节 → restore INVALID → **触发 session_restore_policy blocked**，注入 token 也不恢复，需 UI 重新登录一次）

