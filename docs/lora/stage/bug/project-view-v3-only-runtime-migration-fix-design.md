# Project View 普通运行时全面收敛到 v3 的修复设计

状态：已实现并完成静态、编译与定向自动化验证
日期：2026-08-07

## 1. 背景

Role Checkpoint 已经通过 schema v3 成功写入，但历史读取仍请求
`buzz-project-view-v2`。修复 Role 链路后继续审查发现，通用 Project View 的 CLI、
Desktop 和 Relay 仍保留 v1/v2 普通运行时分支。这样即使当前 Community 已经完成 v3
cutover，后续新增命令仍可能误接旧 parser、tag、scope 或 builder，再次形成“写入 v3、
读取 v2”的混合 major。

本设计在
[Role History 修复](project-view-v3-role-history-runtime-migration-fix-design.md)
基础上，把边界扩大到整个 Project View 普通运行时。

## 2. 最终边界

Project View 普通运行时只支持 schema v3：

- Relay 只为 ready 的 schema-v3 Community 广告 `buzz-project-view-v3`；
- Relay 普通 Project View command handler 只接受 v3 command；
- CLI 的 `get/get-object/create/update/delete/context/roles` 只使用 v3 wire 和 verified
  v3 snapshot；
- Desktop 的 Project View load、普通对象 mutation、Role mutation/history 只使用 v3；
- ACP Role Brief 只从 verified v3 snapshot 组装；
- schema v1/v2 Community 对普通客户端表现为 unsupported 或 `migration_required`，不得
  fallback。

旧版本只允许存在于显式 operator migration/recovery 边界：

- `buzz-admin` 的 v1→v2、v2→v3 cutover、preflight、reproject 和 repair；
- v2→v3 Resource review 对冻结源快照的只读校验；
- DB/SDK 中被 v3 reducer 或迁移实现复用的 domain DTO、canonical loader 和 projection
  helper；
- 明确标记为 legacy migration 的测试、fixture，以及首次初始化前的 additive schema
  安全验证。

这些保留项不得被普通 CLI、Desktop、ACP 或 Relay capability discovery 当作运行时
fallback。

这里的“保留”不是运行时兼容。生产编译中的普通 v2 transaction writer 不再暴露；只为
历史 reducer 回归测试保留的 writer 必须由 `cfg(test)` 隔离。运维侧保留的只是命名明确、
Human-only、disabled/frozen 的迁移读取与 cutover API，用来把存量数据一次性、无损地推进
到 v3。删除这些入口不会让系统更“v3-only”，只会让旧数据失去可验证的升级路径。

## 3. Relay

### 3.1 Capability

NIP-11 仅在以下条件全部成立时广告 `buzz-project-view-v3`：

1. Community 已启用且 schema version 为 3；
2. Project View v3 readiness 通过；
3. 全部普通对象和 continuity canonical pointer 均指向 live、当前 generation、Relay
   签名的严格 v3 projection。

schema 1/2 不再广告 Project View 普通能力。它们必须先通过 operator cutover；不能靠旧
Desktop 或旧 CLI 继续运行。

全新 schema-v3 Community 在 disabled 且尚无 `project_view_state` 时只广告
`buzz-project-view-v3-bootstrap`。该 marker 仅用于让 Desktop 展示 operator `prepare-v3`
和 Human owner `init-v3` 指引，不授权 projection query/subscription、普通 mutation、Role
history 或 Context。Project Document 是独立资产能力，只由自身
`buzz-project-document-v1` capability 与 Relay `self` signer 决定，不依赖 Project View
runtime marker。初始化一旦创建 canonical state，marker 即消失；只有 checked
enable 与完整 strict readiness 通过后才广告普通 `buzz-project-view-v3`。两个 marker 不得同时
出现，initialized-but-disabled 的 maintenance 状态也不得伪装成 greenfield。

### 3.2 写入口和查询

- Project View command ingress 在进入 reducer 前要求 schema 3；
- v1/v2 command 返回明确的 migration-required/unsupported 错误，不进入旧 reducer；
- 普通 current/history bridge 只接受 v3 scope 与 v3 tag；
- 若 Resource cutover 仍需要读取 v2 源状态，该入口必须是显式 migration-only 路径，且
  不能被普通 snapshot reader 调用。

Project Context 与 managed Community-private Project View 状态必须以 ready schema v3
为前置条件。Project Document 普通路径只接受 schema-3-era 的独立 Document capability；
它可在 Project View 尚未初始化时读写，等到被 Project View 引用时才进入跨资产一致性校验。

## 4. 客户端

### 4.1 CLI

普通 Project View 命令必须先确认 NIP-11 广告 v3。读取只调用
`read_verified_v3_snapshot`，写入只构造 v3 command，回执和 canonical readback 都要求
schema 3。旧 `init` 不得再创建 schema-v1 数据；初始化只能走 operator prepare +
owner-signed v3 initialize。

v2 Resource approval 是 cutover 工具，不属于普通运行时。其依赖必须封装在 migration-only
模块中，禁止被通用命令导入。

### 4.2 Desktop

Native identity discovery 只把 `buzz-project-view-v3` 识别为可用 Project View；遇到独立
`buzz-project-view-v3-bootstrap` 时只返回 read-only `uninitialized` setup guide。marker-only
状态不打开 Project View projection live subscription，Tauri Project View mutation 与 Role
mutation/history 均在发出 query/event 前 fail closed。Document 使用独立 NIP-11 identity reader，
不借用 Project View runtime readiness。Tauri snapshot/mutation 不再返回或解析 v1/v2 payload；
TypeScript normalizer 只接受 `schemaVersion = 3`。遇到 v1/v2-only Relay 时显示需要升级，
而不是渲染旧视图。

### 4.3 ACP

ACP 不从 v1/v2 capability、cache 或 projection 恢复 Role Brief。每次 session 创建和
刷新均绑定 v3 meta event、project revision 与 projection generation。

Runtime supervisor 的普通 active 状态同样只接受 schema v3。为保证已有数据仍可无损升级，
schema v2 只允许作为 disabled 且处于 `draining/frozen` 的显式 operator cutover 维护信封，
用于让旧 Runtime 完成 quiesce acknowledgement；它不能恢复 enabled/normal 的 v2 Agent
运行时。

## 5. 数据迁移与无损保证

本次代码收敛不删除、重建或清空数据库。v2→v3 cutover/reproject 必须全量重投影：

- 全部 Project objects；
- 全部 Proposal、Assignment、Work Commitment、Checkpoint、Handoff，包括 terminal
  history；
- canonical UUID、revision、关系、历史行数和业务内容保持不变；
- 只更新 projection pointer、generation 和严格 v3 projection event。

如果 readiness 发现任一旧 projection、缺失 pointer、错误 signer 或混合 generation，
Relay 保持 capability off，等待 operator 修复；不得自动清库或回退到 v2。

部署 readiness 还必须把“active + enabled 但 schema 不是 3”的 Community 直接判为
not-ready，并分别输出 Project View 与 Project Document 的 migration-required Community
计数。全部 disabled 的旧 Community 可以保持 deployment-ready，以便 operator 在不恢复旧
运行时的前提下执行迁移；一旦旧 schema 被错误启用，健康检查和指标必须立即暴露，而不能只
让 NIP-11 静默隐藏 capability。

新建 Community 的数据库默认 schema 为 3，但仍保持 Project View disabled。migration 0048
只修改默认值和约束函数，不更新或删除既有 Community。数据库以一个共享
`project_view_v3_bootstrap_lifecycle_valid` 判定统一约束 deferred trigger、capability discovery、
owner bootstrap、`prepare-v3` 与 `init-v3`：允许没有 canonical state 的纯净 v3 bootstrap
生命周期，同时拒绝 maintenance epoch、迁移 mapping、Context operation、额外 preparation
receipt 或任意 canonical Project View/Role continuity 残留。Community 成员、消息、Agent 和
Project Documents 不计入 Project View footprint；Document row trigger 在这一合法生命周期中
只执行 Document 自身校验，Project View state 建立后恢复完整跨资产校验。

数据库若缺少 migration 0026 的 schema coordinate，普通服务端读取直接失败；不得再把“缺列”
静默解释为 schema v1。v1/v2 只作为显式 operator migration 输入存在。

Project Document 的底层 bootstrap 只接受 schema 3，或显式 operator cutover 所需且保持
capability-disabled 的 schema 2；schema 1 不再是合法输入。`buzz-admin project-document`
对 schema-2 bootstrap/reproject 还必须显式提供 `--for-v3-cutover`，schema-3 普通运维则拒绝
这个迁移确认参数。Document 自身协议名中的 `v1` 是当前 Document wire major，不代表
Project View schema-v1 兼容。

## 6. 防止再次回退

`just check` 增加静态 v3-only 门禁，至少覆盖：

- CLI 通用 Project View 与 Role runtime；
- Desktop native identity/load/mutation/Role history，以及 TypeScript normalizer；
- ACP Role Brief；
- Relay NIP-11、command handler、普通 bridge dispatch；
- DB deployment readiness、migration-required 计数和 Relay 低基数指标；
- v2 transaction writer 必须保持 test-only，Document bootstrap 不得重新接受 schema 1；
- v3 current/history 必须使用严格 v3 DB reader。

门禁禁止普通运行时重新出现 v1/v2 capability fallback、parser、builder、scope、tag 或
schema 分支；同时使用 allowlist 保留明确的 migration/recovery 文件。相关单元测试固定
v2-only Relay 必须 fail closed、v3 成功、混合 tag/generation 被拒绝。

固定旧 Project-View-aware Relay 的 post-mutation compatible rollback smoke 已退役。它会
把已删除的普通 v1/v2 CLI 重新变成发布前提，与 v3-only 边界相矛盾。CI 只保留两条互不
混淆的安全路径：pre-feature smoke 证明“尚未初始化且能力关闭”时 additive migration 不会
阻止旧的无 Project View Relay 启动；显式 legacy canary 在精确命名的 scratch database 中
证明 operator v2→v3 无损迁移。前者不是写入后的应用回滚资格，后者也不开放旧普通运行时。

Meeting 的真实 Provider action-finalization acceptance 属于普通运行时消费者，必须直接以
schema v3 greenfield 生命周期准备：`prepare-v3`、Human owner 签名 `init-v3`、checked
`enable`，随后通过 v3 Role Offer/Accept 建立主持人 Assignment；不得再先初始化 v1、切到
v2 后才进入 Meeting。

## 7. 验收

1. 当前 schema-v3 Community 的 Project View、Document、Context、Role history 和 ACP
   Role Brief 均正常；
2. `buzz roles checkpoint list` 能读取刚写入的 v3 Checkpoint；
3. CLI 与 Desktop 的通用对象读写不再引用 v1/v2 runtime；
4. Relay 不再对 schema1/2 Community 广告普通 Project View capability，也不接受旧
   command；
5. operator 仍能对需要升级的数据执行显式、无损 cutover/reproject；
6. 全历史 pointer 通过 strict v3 校验，canonical 数量和内容不变；
7. 全部门禁、fmt、check、clippy 与定向测试通过，且测试不会指向开发主数据库执行
   destructive reset；
8. 任一 active + enabled 的 schema1/2 Community 会使 deployment readiness 失败，并在
   migration-required 指标中计数；disabled 的迁移输入不恢复旧运行时；
9. 生产构建不再暴露普通 v2 transaction writer，Document schema1 bootstrap 被拒绝。
