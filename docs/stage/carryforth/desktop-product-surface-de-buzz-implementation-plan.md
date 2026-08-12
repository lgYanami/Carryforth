# Carryforth Desktop 产品层去 Buzz 化实现计划

> 状态：代码实现与自动化验证完成，待 Human 现场验收
> 日期：2026-08-11
> 范围：Desktop 的用户可见产品层、系统展示层与仍可到达的产品入口
> 关联：[Desktop 本地化方案](../local/desktop-localization-plan.md)、
> [`cf` CLI 去 Buzz 化实现计划](cli-cf-cutover-implementation-plan.md)

## 1. 结论

本阶段先把 Desktop 对 Human 呈现的产品从 **Buzz** 切换为 **Carryforth**。

这里的“产品层”包括：

- 应用名称、窗口、安装包与系统通知中的品牌；
- Desktop 页面、设置、错误、空状态和帮助文案；
- favicon、应用图标、字标、启动页和安装背景等可见资产；
- Desktop 对外生成和接收的应用深链；
- 仍可能把用户带回 Builderlab、`block/buzz` 或旧 Buzz 产品面的入口；
- 面向用户和本地开发者的 Desktop 日志前缀与产品描述。

本阶段不追求全仓库 `buzz` 字符串归零，也不修改持久化身份、内部 crate、运行时二进制或
Nostr/Project/Meeting 协议命名。产品已经叫 Carryforth，不代表必须在同一个交付中重写所有历史技术坐标。

## 2. 当前问题

CLI 已经完成 `buzz -> cf` 的一次性切换，Desktop 也已经固定为 localhost local-only，但当前 Desktop
仍然公开呈现或携带多处 Buzz 产品身份：

- Tauri `productName` 仍为 `Buzz`；
- Desktop Cargo 描述和前端 package metadata 仍以 Buzz 命名；
- 设置、通知、Profile、Project View、Document、归档、配对和错误页面仍存在 Buzz 文案；
- 应用 favicon、图标、字标和 DMG 资产仍是 Buzz 蜂形品牌；
- 对外深链仍注册并生成 `buzz://...`；
- 更新页面仍可能把用户引向 `github.com/block/buzz`；
- 已不可达的 Builderlab / Hosted Communities 产品代码仍保留旧 Buzz 登录和托管文案；
- Native 日志仍普遍使用 `buzz-desktop:` 前缀。

这使用户虽然通过 `cf` 和本地 Relay 使用系统，仍会把 Desktop 识别为 Buzz 的一个构建版本。

## 3. 目标

1. Desktop 中所有正常可见的产品名称统一为 `Carryforth`。
2. 操作系统、安装包、窗口、通知和应用内页面均不再把当前产品展示为 Buzz。
3. Desktop 不再提供通往 Builderlab、Buzz 托管社区或 `block/buzz` 发布页的产品入口。
4. 当前生成的应用深链使用 `carryforth://`，不再生成或注册 `buzz://`。
5. 用明确门禁阻止后续在用户可见 Desktop surface 中重新引入 Buzz 产品文案。
6. 保持本地身份、Community、消息、Agent、Project、Document、Context 和 Meeting 数据完全不变。

## 4. 本阶段边界

### 4.1 纳入范围

- `../../../desktop/src-tauri/tauri.conf.json` 的产品展示名和深链 scheme；
- `../../../desktop/src-tauri/Cargo.toml`、`../../../desktop/package.json` 中可见产品描述；
- React UI 中面向 Human 的标题、说明、错误、通知、空状态和帮助文本；
- Native 中会出现在终端或诊断面板的 Desktop 产品日志前缀；
- 当前应用生成、复制、解析和打开的 message deep link；
- Buzz favicon、字标、应用图标、DMG 背景和启动品牌资产；
- 已被 local-only 产品策略关闭的 Builderlab / Hosted Communities UI 与 Native 入口；
- 指向 `block/buzz`、`squareup/buzz-*`、`communities.buzz.xyz` 或 Builderlab 的 Desktop 运行时链接；
- Desktop 用户文档、E2E mock 文案、截图基线和品牌防回退检查。

### 4.2 明确不纳入范围

以下内容本阶段必须保持原值：

- Tauri bundle identifier：`xyz.block.buzz.app`；
- OS keyring service：`buzz-desktop`、`buzz-desktop-dev*`；
- Desktop app-data 目录和迁移标记；
- `~/.buzz`、`~/.buzz-dev`、REPOS、Agent 配置与 runtime state；
- 已存在的 localStorage / IndexedDB key，例如 `buzz-*`；
- Rust package/library：`buzz-desktop`、`buzz_lib`；
- sidecar：`buzz-acp`、`buzz-agent`、`buzz-dev-mcp`；
- `BUZZ_*` Relay/ACP/Desktop 内部环境变量；
- Docker project、container、volume、数据库用户和数据库名；
- `buzz-project-view-v3`、`buzz-project-context-*`、event kind、tag、`d` coordinate；
- Rust 内部 crate：`buzz-core`、`buzz-sdk`、`buzz-db` 等；
- Web 与 Mobile 产品面。

这些内容分别属于持久化身份、运行时工具链或 wire/storage 合同，需要后续独立设计。它们暂时保留不代表
提供 Buzz 产品兼容路径。

## 5. 核心不变量

### 5.1 数据连续性

- 不新增数据库 migration；
- 不删除、复制、重建或重定位 app-data；
- 不清理 keyring、身份、Nest、Community 或 Docker volume；
- 不修改本地 Relay 数据；
- 不重写历史消息中的文本、附件、event 或旧 deep link；
- 不调用 reset、truncate、drop、`docker compose down -v` 或其他破坏性入口。

仅改变显示名称不会成为一次“新安装”。已有身份和本地数据在更新前后必须从同一 bundle identifier、
keyring service 和数据目录读取。

### 5.2 业务行为

- Channel、Agent、Project View、Document、Project Context 和 Meeting 行为不变；
- `cf` CLI 及其 `CARRYFORTH_*` 公开合同不变；
- Relay NIP-42/NIP-98、Community 权限和本地 Owner 认领不变；
- 本阶段不借品牌替换修改状态机、capability、schema 或 revision 语义；
- raw protocol/error/capability 标识在诊断详情中保持原值。

### 5.3 不做机械替换

遇到 `Buzz` 时按语义处理：

| 原含义 | 产品层处理 |
| --- | --- |
| 当前应用名称 | 改为 `Carryforth` |
| 当前应用执行的动作 | 使用 `Carryforth` 或具体组件名 |
| Relay 的拒绝/校验 | 优先写成 `Relay rejected...`，避免错误归因给品牌 |
| 内置 Agent/ACP | UI 显示中性名称，诊断详情保留真实 binary |
| 历史协议 ID / raw error code | 原样保留 |
| CSS class、测试 ID、storage key | 本阶段不改 |
| 历史事故或迁移说明 | 保留事实，不改写历史 |

## 6. 目标产品合同

### 6.1 名称

- 正式产品名：`Carryforth`；
- Agent-first CLI：`cf`；
- Desktop 用户文案统一使用 `Carryforth`；
- 不使用 `CarryForth`、`Carry Forth` 或其他大小写变体；
- 不向用户展示“Buzz-based”“Buzz fork”等过渡品牌。

### 6.2 系统展示

- Tauri `productName = "Carryforth"`；
- 安装包、应用菜单、系统通知来源和任务切换器显示 Carryforth；
- Desktop package description 改为 `Carryforth desktop app`；
- Native 日志前缀改为 `carryforth-desktop:`；
- bundle identifier 在本阶段继续保持 `xyz.block.buzz.app`，不得因显示名变化而建立第二份应用数据。

### 6.3 深链

当前产品只注册并生成：

```text
carryforth://message?channel=<uuid>&id=<event-id>
```

要求：

- Tauri 只注册 `carryforth` scheme；
- Desktop 复制、解析、导航和通知 action 统一生成 `carryforth://`；
- `buzz://` 不作为别名或 fallback 继续注册；
- 不扫描或重写历史消息内容；历史中已有的 `buzz://` 文本保留原样，但不再属于当前产品合同；
- 远程 community connect/join/add 与 nostr-bind deep link 不因改名重新开放。

### 6.4 品牌资产

需要替换：

- `desktop/public/buzz.svg` 与 favicon 引用；
- landing wordmark；
- Tauri 全平台 icon 集；
- macOS `.icns`、Windows `.ico`、Linux PNG；
- DMG background；
- onboarding 或设置中直接呈现的蜂形/蜂群品牌图形。

新资产必须是 Carryforth 资产，不应仅把旧蜂标改名后继续使用。若最终 Logo 尚未确认，可以先实现文案与代码
切换，但不能把“Desktop 产品层去 Buzz 化”标记为全部完成。

## 7. 分阶段实现

每阶段结束后 review 本文边界与实际 diff；发现需要改 bundle identifier、keyring、Nest 或协议 ID 时必须停止，
转入后续独立设计，不得在本阶段顺手处理。

### 阶段一：建立产品命名基线

1. 将 Tauri `productName` 改为 Carryforth；
2. 更新 Desktop 的公开 package description 和 HTML metadata；
3. 在前端建立单一产品名常量，避免各 feature 重复硬编码；
4. Native 用户可见日志统一使用 `carryforth-desktop:`；
5. 保留内部 package/lib/sidecar 名称，并以测试明确其暂留原因。

验收重点：显示名改变后，Desktop 仍从原 app-data 和原 keyring 读取同一身份。

### 阶段二：用户可见文案迁移

按功能面逐项处理：

- Settings、Profile、通知、反馈、Sign out、Doctor；
- Project View、Document、Project Context 与 Meeting 状态页；
- Onboarding、Community、Channel、Agent 和本地归档；
- 系统通知、权限提示、错误和恢复页面；
- Clipboard / snapshot 中面向 Human 的描述；
- E2E fixture 和截图文案。

文案不得盲目把所有 `Buzz` 替换成 `Carryforth`。例如 “Buzz rejected the snapshot” 应改为
“The Relay rejected the snapshot”，因为执行拒绝的是 Relay，不是产品品牌。

Doctor 中若必须展示 `buzz-acp`、`buzz-agent` 等真实命令，应放在“技术详情”中；主标签使用“ACP adapter”、
“Built-in Agent runtime”等中性产品文案。

### 阶段三：移除旧远程产品入口

在已经交付 local-only 的前提下，删除或退出 Desktop 产品树：

- Hosted Communities 设置卡片、创建流程和 onboarding；
- `hostedCommunityApi` 与 Builderlab 登录/身份绑定调用；
- Native Builderlab session、login、command registration 和网络请求实现；
- 指向 `app.builderlab.xyz`、`communities.buzz.xyz` 的运行时 URL；
- 指向 `github.com/block/buzz/releases` 的更新/下载入口；
- 依赖旧 Buzz release endpoint 的 Update Checker、sidebar card 和 indicator；
- “Buzz mobile app”配对入口；底层本地配对协议留待 Mobile 阶段重新决定，不在本阶段改 wire。

本地 Relay 内提交的 Product Feedback 可以保留，但标题、诊断头与隐私说明改为 Carryforth；不得把反馈描述为
发送给 Block/Buzz 服务。

删除旧远程入口后，Desktop 不得通过隐藏 feature、build env 或深链重新到达它们。

### 阶段四：深链与品牌资产切换

1. 将 message link 的生成、解析、OS 注册和测试切换为 `carryforth://`；
2. 确认 remote community 与 nostr-bind action 仍被拒绝；
3. 替换 favicon、wordmark、应用图标、DMG 背景与 onboarding 品牌图形；
4. 删除不再引用的 Buzz 视觉资产；
5. 更新截图与打包快照，验证各平台展示名称和图标一致。

### 阶段五：防回退门禁

增加 Desktop 产品面静态检查，至少验证：

- `productName` 精确为 `Carryforth`；
- 当前 UI 文案不得把产品称为 Buzz；
- 运行时代码不存在 `block/buzz`、Builderlab 或 `communities.buzz.xyz` 产品链接；
- 当前生成的深链只使用 `carryforth://`；
- favicon、wordmark和 Tauri icon 不再引用 Buzz 资产；
- 允许保留的 `buzz-*` 仅限明确分类的 storage、bundle/keyring、binary、crate、protocol 和历史测试 fixture。

门禁必须使用按类别维护的 allowlist，不能用全仓库 `rg -i buzz` 要求零命中。

## 8. 主要修改面

| 修改面 | 主要位置 | 目标 |
| --- | --- | --- |
| 产品元数据 | `../../../desktop/src-tauri/tauri.conf.json`、`../../../desktop/src-tauri/Cargo.toml`、`../../../desktop/package.json` | Carryforth 展示名与描述 |
| UI 文案 | `../../../desktop/src/app`、`../../../desktop/src/features`、`../../../desktop/src/shared` | 用户可见 Buzz 文案归零 |
| Native 日志 | `../../../desktop/src-tauri/src` | `carryforth-desktop:` 前缀 |
| 远程死代码 | communities/settings 与 `builderlab.rs` | 删除 Builderlab/Buzz 托管入口 |
| Updater | Tauri build/lib、settings updater hooks | 移除 `block/buzz` 更新链 |
| Deep link | Tauri config、`deep_link.rs`、message link/notification | `carryforth://message` |
| 品牌资产 | `../../../desktop/public`、`../../../desktop/src-tauri/icons` | Carryforth favicon/wordmark/icon |
| 门禁与测试 | `../../../desktop/scripts`、unit/E2E/Tauri tests | 防止产品文案回退 |

## 9. 测试与验收

### 9.1 静态与单元测试

- Desktop typecheck、Biome、file-size、text-size 和现有单元测试；
- Tauri fmt、clippy 与完整 Desktop Rust tests；
- product-name / runtime-URL / deep-link / asset 防回退检查；
- Builderlab 与 old updater command 不再注册；
- raw protocol ID 与内部 sidecar 名仍能正常解析。

### 9.2 Desktop E2E

至少覆盖：

1. 首次启动和已有数据启动都显示 Carryforth；
2. Settings、Profile、通知、Project、Document、Context、Meeting 页面没有 Buzz 产品文案；
3. Settings 不存在 Hosted Communities、Builderlab、Buzz updater 或 Buzz mobile 入口；
4. 复制 message link 得到 `carryforth://message`，点击后能回到正确 thread；
5. remote community / nostr-bind deep link 仍被拒绝；
6. 系统通知来源显示 Carryforth；
7. Sign out / recovery 文案准确，但验收不得实际删除当前主开发身份或数据。

### 9.3 数据连续性现场验收

在同一个已有 localhost 环境中记录更新前基线：

- Desktop pubkey；
- Local Dev Community ID 与 Owner；
- Channel/消息数量；
- managed Agent 定义与运行状态；
- Project View、Document、Project Context revision；
- Meeting 历史数量。

更新后重启 Desktop，逐项回读完全一致。不得通过复制新 profile、重新登录、重新建 Community 或恢复备份来
制造“通过”。

### 9.4 产品与网络验收

- 操作系统应用名称、图标、菜单与通知统一为 Carryforth；
- Desktop 运行期间不访问 Builderlab、`block/buzz` release 或远程 Community；
- 本地 Relay 不可用时仅报告 localhost 错误，不打开 Buzz 页面或远程 fallback；
- `cf`、ACP、Project/Document/Context/Meeting 完整 smoke 不受品牌切换影响。

## 10. 完成标准

- Human 在正常 Desktop 路径中看不到 Buzz 作为当前产品名；
- OS 与安装包将应用展示为 Carryforth；
- 当前生成的产品深链为 `carryforth://`；
- 当前 UI 和 Native command tree 不存在 Builderlab/Buzz 托管入口；
- Desktop 不再链接 `block/buzz` 更新或发布页面；
- Buzz 品牌图形已被 Carryforth 资产替换；
- 产品面防回退门禁通过；
- 更新前后的身份、本地数据和全部业务 revision 保持一致；
- bundle identifier、keyring、Nest、storage key、sidecar 和协议 ID 未被误改。

## 11. 风险与处理

### 11.1 显示名变化被误当作新应用

若安装器或平台因 `productName` 改变而产生第二个快捷方式或安装位置，必须通过相同 bundle identifier 和升级
测试解决，不能通过改 identifier 或复制 app-data 掩盖。

### 11.2 机械替换破坏协议或数据

所有修改按语义 review。禁止对 `../../../desktop` 执行无 allowlist 的全局 `Buzz -> Carryforth` 或
`buzz -> carryforth` 替换。

### 11.3 旧深链失效

`buzz://` 不再是当前产品合同，这是直接切换的预期结果。历史消息不重写；验收要确认新 link 正常，同时旧
scheme 不会意外重新注册。

### 11.4 远程死代码误伤本地能力

删除 Builderlab/Updater UI 时必须按调用图移除，仅删除远程产品链。Local Community、NIP 签名、消息、媒体、
Project、Meeting 和本地 Product Feedback 不能因此失效。

## 12. 后续阶段

完成本阶段后，再分别设计：

1. Desktop 持久化身份迁移：bundle identifier、keyring、app-data、Nest 与 storage key；
2. Runtime 工具链迁移：`buzz-relay`、`buzz-acp`、`buzz-agent`、`buzz-admin` 与 `BUZZ_*`；
3. Docker、日志、metrics、发布与部署命名；
4. 内部 Rust crate 和 wire/storage namespace 的保留或迁移策略。

这些阶段不得反向阻塞 Desktop 先以 Carryforth 产品身份交付。

## 13. 实施记录（2026-08-11）

本计划已经完成代码落地，当前实现结果如下：

- Desktop 产品名、HTML metadata、package description、应用内文案与 Native 日志前缀已切换为
  `Carryforth` / `carryforth-desktop:`；
- Tauri 仅注册 `carryforth://`，message deep link 的生成、解析与通知导航均使用新 scheme，旧
  `buzz://` 不再注册或接受；
- Builderlab、Hosted Communities、远程 Community 添加/加入、Nostr identity binding、Buzz Mobile
  配对入口和旧 updater 产品链已经从 Desktop 的 UI、Native command tree 与依赖中删除；
- favicon、landing wordmark、启动标记和 Tauri 全平台应用图标已替换为 Carryforth 资产，未继续复用旧蜂形品牌；
- 新增 `pnpm check:product-brand` 静态门禁，并接入 `pnpm check`，用于固定产品名、深链、资产、远程入口
  移除状态以及必须保留的内部坐标；
- `xyz.block.buzz.app`、`buzz-desktop` keyring service、原 app-data / Nest / browser storage key、
  `buzz-acp` 等 sidecar 和 `buzz-project-*` wire 标识均保持不变；本次没有数据库 migration，也没有读写或
  清理本地业务数据；
- 为保持本地单元测试 hermetic，Relay override 仅在 `cfg(test)` 构建中继续用于 localhost 测试桩；发布构建
  始终固定到 `ws://localhost:3000`，不存在远程开关或 fallback。

已完成的自动化检查：

- `pnpm check` 全部通过，包括 Biome、文件体积、文字尺寸、产品品牌与公钥截断门禁；
- 前端单元测试 `3628 passed / 0 failed`；
- 前端 production build 与 E2E build 均通过；
- Tauri `cargo check` 通过；完整串行原生测试 `1714 passed / 14 ignored / 0 failed`；最终 Relay 本地目标模块
  拆分后又执行了 `relay::tests`，`29 passed / 0 failed`；
- Desktop smoke E2E 覆盖产品入口、Settings、Profile、路由与 message deep link，结果为
  `12 passed / 1 skipped / 0 failed`；
- `git diff --check` 与 migration 零变更检查列入最终交付门禁。

仍需 Human 现场验收：在现有 localhost 数据上启动 Desktop，核对身份与各业务 revision 连续性；检查操作系统
窗口、任务切换器、通知和图标；执行一次 `carryforth://message` 导航和核心业务 smoke。现场验收不得通过重建
身份、Community 或数据库来制造通过。
