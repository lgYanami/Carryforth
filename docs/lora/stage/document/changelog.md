# Project Document 分阶段交付记录

## 2026-08-07 — 解除 Document 对 Project View 初始化状态的错误依赖

- Document 仍是可被 Project View 直接引用的独立版本化资产；schema-v3 Community 尚未建立
  `project_view_state` 时，只要自身 Document capability ready，Document CRUD 不应被拒绝。
- migration 0048 让 Document deferred trigger 复用统一的 v3 bootstrap lifecycle 判定：合法
  bootstrap 状态只做 Document 自身校验；Project View state 建立后恢复完整跨资产一致性校验。
- Desktop Tauri 改为从 `buzz-project-document-v1` 与 NIP-11 `self` 独立解析 Document identity，
  不再调用 Project View `require_runtime_ready`。测试 fixture 固定“仅广告 Document、不广告
  Project View”仍可完成 verified Document read。
- 变更不更新、删除或重建任何现有 Document、Project View、消息、Meeting 或 Agent 数据。
- Document deployment readiness 同时统计并拒绝 active + enabled 的旧 Project View schema，
  防止 Relay 健康检查为绿色但 NIP-11 静默隐藏 Document capability；新增
  `buzz_project_document_migration_required_communities` 供部署告警。
- Document bootstrap 底层仅接受 Project View schema 2/3：schema 2 是显式、
  capability-disabled 的 operator cutover 输入，schema 3 是唯一普通运行时；schema 1
  不再接受。`buzz-admin project-document bootstrap/reproject` 在 schema 2 上额外要求
  `--for-v3-cutover`，避免把迁移 seam 当成普通兼容入口。

## 2026-08-07 — Stage 6 Context canary改为独立schema-v3 greenfield fixture

- `scripts/test-project-view-stage6-canary.sh`不再调用Stage 5或显式legacy v2→v3 migration canary，也不再读取
  其`acceptance-summary.json`、旧Role/Assignment、Resource或Guide坐标。
- Stage 6现在自行创建scratch schema-v3 Community与owner/managed Agent identity，固定执行
  `prepare-v3 → owner init-v3 → checked enable`，随后bootstrap/enable Project Document，创建Guide、Resource、
  Role，完成Offer/Accept与supervisor binding，再执行原有Context、Role Brief与Runtime fence验收。
- Runtime部分按当前边界验收binding与ACP lease generation生命周期；Context普通写只服从Community/Role
  governance，不再把旧Runtime fence误当作Context ACL。Assignment结束后的预签名写仍必须拒绝。
- Just/release入口与v3静态门禁要求Stage 6脚本只使用当前v3普通运行面；历史链式canary证据明确退役。此次仅做
  shell语法和静态门禁检查，未运行Docker、数据库或真实canary，也未删除任何数据。

## 2026-08-07 — Stage 2普通canary全面切换到greenfield Project View v3

- `scripts/test-project-document-e2e.sh`不再调用已删除的v1 `project-view init`或v2 cutover。
  独立scratch database现在显式创建schema-v3空Community，并执行
  `prepare-v3 → direct Human owner签名init-v3 → checked enable`。
- canary将Human owner与Relay projection signer拆成两把测试身份；固定验证初始化前只广告
  `buzz-project-view-v3-bootstrap`、初始化后disabled时不广告普通Project View、enable后只广告
  `buzz-project-view-v3`，再进入原有Document bootstrap、CRUD、撤权、incident与recovery语义。
- 新增`PROJECT_DOCUMENT_E2E_SCRATCH_DATABASE=1`证据标记；原有受限database命名、独立restore
  命名和最终清理边界保持不变。不接触现有Community，不恢复旧runtime、dual-read或dual-write。
- Stage 2 release contract和全局v3静态守卫同时固定这条链路，防止普通Document验收脚本再次
  悄然依赖legacy Project View版本。

## 2026-08-03 — 修复 Desktop Document mutation 字段命名不一致

- 修复 Desktop 前端以camelCase提交`contentMarkdown`、`documentId`与
  `expectedDocumentRevision`，而Tauri Rust嵌套mutation仍按snake_case反序列化的问题。
- `ProjectDocumentMutation`现在保持`create/update/delete`的既有variant名称，同时统一按
  camelCase接收variant字段；不改变Relay协议、Document revision或数据库数据。
- 增加真实JSON边界回归，覆盖Desktop的create、update与delete payload，避免TypeScript
  mock bridge绕过Rust Serde后再次掩盖契约漂移。

## 2026-08-03 — 修复 managed Agent 普通 Document 写入授权边界

- 修复 `buzz documents` 将所有 managed Agent create/update/delete 误判为 Role-bearing
  command 的问题。第一方 CLI 现在默认同时省略 `acting_assignment_id` 与 `runtime_fence`，
  不再要求 Project View v2、active Assignment 或 Runtime supervisor。
- DB writer 将 Community eligibility 与可选 Role/Runtime 归因拆开：Human direct member或
  owner仍合格的verified managed Agent均可执行普通Document CRUD；Project Role不建立第二套
  Document ACL。
- wire 中已有的Assignment/Runtime成对字段保持兼容。managed caller显式携带时仍严格验证
  active Assignment与exact supervised Runtime；stale/ended/wrong claim拒绝且不能静默降级。
- Relay不再把全部授权失败误报为`runtime_fence`，现可区分Community未授权、显式Assignment
  冲突和Runtime fence失败。
- 新增真实PostgreSQL回归，覆盖无Assignment/Runtime的managed Community writer、显式无效
  Assignment、Human伪造归因、owner撤权，以及现有stale Runtime拒绝路径。
- 详细事故与方案见
  [`../bug/project-document-managed-agent-community-write-fix-design.md`](../bug/project-document-managed-agent-community-write-fix-design.md)。

## 2026-08-03 — 澄清 Document strict writer 与 Project View Community writer 边界（已被后续修复覆盖）

> 本节记录修复前的阶段性边界。上方同日决策已确认普通 Document CRUD 应与普通 Project
> View对象一致地使用Community ACL；Runtime仅在显式Assignment-bearing command中强制。

- Project View 修复了把 Assignment 当作 managed Agent 普通 CRUD 前置权限的错误；Resource
  等普通 Project View 对象现在遵循 Community member授权，只有显式 Assignment-bearing
  command才校验对应 Runtime。
- 当时 Project Document 尚未随之放宽；managed Document create/update/delete仍被要求携带
  active Assignment与exact current Runtime fence。该限制随后被上方缺陷修复取代。
- 实现设计中原先“Document v1与Project View v3使用完全相同 managed write gate”的表述已
  修正为：两者共享Community eligibility和wire-neutral RuntimeFence，但各自决定何时必须
  进入Assignment/Runtime gate。本条仅澄清边界，不改变Document wire、数据或权限。

## 2026-08-02 — Project Context 最小核心语义完成

阶段目标：在不扩张Role Brief v3、Context closure、协议、权限或客户端范围的前提下，让所有Buzz
managed Agent从session开始就具备Document、Resource、Context Reference与显式写回的稳定核心认知。

### 已交付

- `buzz-acp`的platform-owned`[Project Space]` contract从v2提升到v3。固定说明Project Document是
  可由Project View直接引用的版本化长文本资产，Resource是通过mandatory Guide Document说明用法的
  Project View资产坐标，Project View对象可通过Context Reference关联二者。
- 固定契约要求Agent只按需读取相关Document正文，并在工作实质改变Project View、Resource信息或Guide
  关联、Document正文或Context Reference时，通过Buzz显式写回；聊天、本地文件和模型记忆不会自动更新
  Project。
- 新语义保持capability-neutral，不注入当前Community、Project、Role、Resource、Document或revision等
  动态事实。动态相关坐标继续由verified Role Brief / Binding交付，授权继续由Buzz工具与Relay执行。
- modern`session/new.systemPrompt`、legacy user-context compatibility和`--no-base-prompt`路径均由测试固定。
  contract version与内容hash继续触发现有旧session失效；replacement session继续强制获取Full Role Brief。
- Base prompt已有`buzz documents`、正文按需读取和`buzz resources guide ... --content-only`操作闭环，因此
  未重复增加命令说明。Role Brief v3 Context来源路径补强仍按决议延期观察。

### 本地验证

- `cargo test -p buzz-acp --lib`：641项通过；
- `cargo clippy -p buzz-acp --all-targets -- -D warnings`通过；
- `cargo fmt --all -- --check`与`git diff --check`通过；
- 测试批次使用`CARGO_INCREMENTAL=0`，交付结束再次清理workspace与Tauri Cargo incremental缓存。

## 2026-08-01 — 阶段 7 单机预发布 hardening 完成

阶段目标：在不假装存在生产环境的前提下，用一台开发机证明 Project Document 的 signer轮换、
完整历史重投影、backup / restore、安全事件与10万级revision增长路径可恢复、可测量且不会失控。
阶段 7不授权发布、部署、默认启用或broad rollout。

### 已交付

- 新增 additive migration `0035_project_document_reproject.sql`：新 generation的完整revision、head与
  reset meta先进入普通查询不可见的staging表；固定source basis并验证完整覆盖后，在capability disabled
  状态下用一个Community独占事务原子激活。失败全部回滚，成功只切换projection pointer、signer与
  generation，不修改canonical business字段。
- `buzz-admin project-document reproject --all-revisions`支持1–1000条有界分页、staging / ready恢复、
  after-commit replay与target signer校验；`status`新增reproject状态、orphan与pointer mismatch计数。
  `verify`对全部历史revision执行有界分页的canonical、generation、pointer与cryptographic parity。
- SDK新增显式revision / head reprojection builder；DB补齐全历史repair、atomic activation、closed keyset
  query plan与generation-aware read fence。正常readiness继续使用轻量索引检查，昂贵全历史校验只进入
  operator verify / enable / reproject路径。
- 新增[`stage7-hardening-runbook.md`](stage7-hardening-runbook.md)，固定signer rotation、恢复、容量、
  retention / compliance待决项与未来Adapter观察标准；当前明确不实现在线无停机rotation、hard delete、
  Secret Store、installer或隐式external action。
- 新增`project-document-stage7-recovery`、`project-document-stage7-capacity`与聚合
  `project-document-stage7-test`。所有Cargo批次关闭incremental，并精确清理scratch数据库、key、backup、
  临时文件及workspace / Tauri incremental目录。

### 本地真实验收

- 真实PostgreSQL、Redis、Relay、CLI与admin栈完成generation 1 → 2 signer rotation；完整历史重投影、
  after-commit replay、独立database `pg_dump` / restore parity、新 signer Relay读取与最终disable均通过。
  同一演练完成Secret incident流程和低配额HTTP burst，结果为3次接受、3次429拒绝。证据位于
  `test-results/stage7-recovery/20260801T160349Z-106900`，JSON报告SHA-256为
  `c1424ab6385b91914971f5650fdd3a567408f8ce342fc52a4d11d4bcf0324206`。
- 100,000条revision容量验收通过：50,000条hot Document历史 + 50,000个wide catalog Document，
  256–1,024 byte正文；seed耗时15,481 ms，database增长627,736,576 bytes。1,000个closed keyset page
  每页上限50，最慢239 ms（门槛2 s），使用revision主键backward Index Scan、无revision表Seq Scan；
  RSS peak / retained增长952 KiB。强制250 ms Community writer lock时shared wait为253 ms，暂无证据支持
  引入per-Document lock。证据位于`test-results/stage7-capacity/20260801T155849Z_95746_24750`，JSON报告
  SHA-256为`146c74c62ed5d0e91d72a8edd2a586cbe54a3133ab8d30ea34a26dca623b335a`。
- migration / desired-schema、Document unit / PostgreSQL transaction与race门禁全部通过；Stage 5与Stage 6
  真实本地canary已在最终业务代码上重跑，证据分别位于
  `test-results/stage5-canary/20260801T155950Z-97709`与
  `test-results/stage6-canary/20260801T160105Z-101707`。scratch资源已清理。

百万revision extended soak未运行，按当前单机预发布合同属于通过磁盘preflight后才执行的non-blocking
项目。capability最终保持disabled，首次真实部署前仍需单独定义deployment / rollout gate。

## 2026-08-01 — 阶段 7 调整为单机预发布交付基线

Buzz尚未发布或部署，当前只有一台开发机。阶段 7不再以生产dashboard、真实用户观察或百万级
revision作为完成条件：必做容量门降为至少100,000条小正文revision，覆盖hot Document与宽catalog；
百万级仅作为通过磁盘preflight后的non-blocking extended soak，上限正文只做小规模边界case。

生产dashboard改为可归档的本地JSON / Markdown报告；多节点 / HA、生产错误率窗口和Adapter真实usage
evidence延期到首次部署规划。signer rotation、backup / restore、projection parity、权限、安全与Secret
incident仍保留本地真实scratch验收。阶段 7完成不授权发布、v3默认启用或broad rollout，首次部署前需
另立deployment / rollout gate。

## 2026-08-01 — 阶段 6 软件与本地真实 Context canary 完成

阶段目标：让 Project View v3 的 Resource、Live / Pinned Document成为可沿Project / Goal / Role /
Work等项目坐标发现的Context，同时只交付可信坐标和无正文metadata，不把Context误当权限或自动执行来源。
第一方图形客户端只覆盖Desktop；Mobile与Web不在本阶段范围。

### 已交付

- CLI新增`project-view context list / add / remove`。便利写入先读取同一verified v3 snapshot，在本地构造
  完整canonical replacement，再携带exact global revision、active Assignment与Runtime fence提交；v3
  `objects[]`回执被strict解析并核对operation、object type / ID / revision与delete状态，v1/v2 flat回执保持可用。
- Desktop Inspector新增Context chips与picker，支持Resource、Live Document与Pinned Document；Resource source
  不显示Resource target。capability关闭时保留verified只读坐标，只允许subset cleanup，不开放add / retarget；
  native与TypeScript adapter round-trip同一closed canonical set。
- SDK完成`RoleBriefV3.context`有界一跳closure：Profile、Goals、当前Role、nonterminal responsible Work、相关
  Issue / handling Work、latest Checkpoint与最近3个Handoff可贡献Context；Resource只展开primary Guide和直接
  Document。最多64个Resource、64个mandatory Guide、64个supplementary Document，最终escaped Context block
  不超过64 KiB；Resource / Guide pair不可拆分。
- Role Brief只输出Resource / Document coordinate、可信来源revision、无正文title / summary与显式fetch command。
  Pinned只输出exact historical coordinate，不后台查询含Markdown的revision event；不可信metadata被单行化并
  转义delimiter，不能提升为system instruction。
- CLI与ACP都以Document meta A → required heads → meta B构造稳定窗口并有限重试。ACP cache key包含Relay、
  member / Assignment、PV meta / revision / generation与Document meta / catalog revision / generation；Document
  编辑不推进PV revision，但下一次resolve会刷新Live metadata。metadata失败显式降级为`unavailable`，不复用
  stale值，也不撤销已验证Assignment或当前Runtime fence。
- `buzz-admin project-view context status / enable / disable`通过Community exclusive lock原子控制
  `project_context_enabled`。enable要求schema 3、normal maintenance、Project View / Document signer与projection
  parity、normalized reference parity全部ready；idempotency replay先于mutable readiness，控制receipt与audit
  durable保存。NIP-11只在ready后广告`buzz-project-context-v1`；disable不删除refs。
- Document delete在同一Community锁内查询Resource Guide与Live Context反向索引，并在pure reducer阶段返回
  `still_referenced`；Pinned不阻止普通delete且历史revision继续可读。Resource delete同样受normalized反向索引
  保护。
- ACP base prompt只指导Agent按当前task显式读取`buzz resources guide ... --content-only`，并明确Guide不能授权
  external action、安装Skill / Plugin、修改MCP、请求Secret或运行代码。Context出现本身不会触发任何外部动作。
- 新增[`stage6-context-canary.md`](stage6-context-canary.md)与可重复脚本
  `scripts/test-project-view-stage6-canary.sh`。Stage 5/6 canary及开发测试批次关闭Cargo增量编译，并在退出时清理
  workspace与Tauri的`target/**/incremental`。

### 已完成的本地真实运行交付

- 在真实本地PostgreSQL、Redis、Relay、`buzz-cli`、`buzz-admin`、`buzz-acp`、Runtime supervisor和ACP child
  栈上，从独立Stage 5 v3前置状态原子启用Context并验证NIP-11 advertisement与idempotent replay。
- managed Agent通过当前Runtime fence为Role加入Resource、Live与Pinned坐标；已退休Runtime和已结束Assignment
  均不能重放预签名Context mutation。disable期间坐标保持可见、add拒绝、subset remove成功，re-enable后
  CLI / Role Brief立即观察同一canonical set。
- strict Role Brief输出`ready + verified`、primary Guide、Live revision与Pinned coordinate且不含正文；Agent-facing
  CLI显式读取Guide。Document revision 1 → 2后PV revision保持不变而Live metadata刷新；移除Live后Document
  delete成功，Pinned revision 1仍可读取。
- Live Document、mandatory Guide与Context-referenced Resource的删除保护均返回明确conflict；normalized refs最终
  清空，3次Context控制操作对应3条durable operation与3条hash-chain audit，replay未追加重复记录。
- 2026-08-01T12:37:18Z验收通过，证据位于
  `test-results/stage6-canary/20260801T123618Z-1542000`；`artifact-digests.sha256`自身SHA-256为
  `424fc05ce7e321a40b7bb7dd4d4101028bd96b01f94a7a8f963f74c65c2361f8`。scratch数据库与进程已清理，
  `target/**/incremental`计数为0；没有执行broad rollout或真实外部部署。

## 2026-08-01 — 阶段 5 软件与本地真实 canary 完成

阶段目标：交付可安全迁移到 Project View v3 的 dual clients、Resource → Guide 使用闭环与
maintenance-aware ACP runtime；第一方图形客户端仅覆盖 Desktop。Mobile、Web、Context Reference
与阶段 6 enrichment均不在本阶段软件交付范围。

### 已交付

- SDK新增strict `RoleBriefV3`与v3 Role/Resource surface；CLI可按signed metadata在v2/v3间严格分派
  snapshot、Role continuity与object write，不进行dual write。`buzz resources guide`先验证Resource，
  再按exact Document coordinate读取Guide，并在输出前复验v3 meta，避免读取窗口中的authority漂移。
- CLI提供v3 Resource detached Human approval；Relay bridge提供versioned v3 current-entity分页，未知
  `resource_kind`在SDK、CLI、Tauri和Desktop中原样往返。
- Tauri提供strict v3 verified snapshot与write adapter；TypeScript拒绝v2/v3 discriminator混用、
  nonempty Context和错误Document metadata状态。Desktop交付locator-free Resource form、metadata-first
  Guide picker、Guide-first create saga、冲突保留以及inspector中的Resource → Guide入口。
- ACP解析`ResolvedRoleBrief::V2 | V3`；base v3只包含bounded Project/Role/Assignment/Work continuity，
  Context固定为`not_advertised_empty`，Document metadata固定为`not_required`。base prompt加入
  `project-view get → get-object resource → resources guide --content-only`按需发现链，不自动读取Guide正文。
- runtime supervisor在启动和持续轮询中优先处理maintenance，停止新turn admission，取消并join全部
  lifecycle工作，回收active/idle child process group，再按Runtime、Assignment不可交换顺序提交durable
  ACK并逐字段读回。轮询deadline不会被持续命令流重置；恢复只创建新的Runtime/fence。
- child registry持久化PGID leader start-time和每次spawn的随机marker；异常恢复只有在证明进程身份后
  才杀进程组，PID/PGID复用或无身份的leaderless group均fail closed。
- `buzz-admin`提供fleet readiness与automation-safe `maintenance ack-probe`。数据库readiness把协议、
  supervisor poll、baseline、durable ACK和invalidation分别报告；maintenance begin使用确定的
  Assignment/binding加锁顺序，避免PostgreSQL禁止window function与`FOR UPDATE`同层组合的问题。
- 新增[`stage5-cutover-canary.md`](stage5-cutover-canary.md)，固定reviewed Resource export、Guide publish、
  detached approval、exact maintenance、freeze/cutover/verify/resume、旧客户端unsupported观察、
  migration archive与empty-state direct-v3 canary合同。

### 已完成的自动化与 isolated 证据

- ACP全量单测619项通过；相关Rust包Clippy以`-D warnings`通过。
- Project View unit/property/relation/wire门禁通过；PostgreSQL集成门禁20项通过，包含真实
  maintenance begin → supervisor poll → durable ACK → ready-to-freeze状态推进。
- 真实本地Relay的Project View WebSocket/HTTP读写、分页与live revocation E2E通过。
- Desktop `pnpm check`与全量测试通过；strict RoleBriefV3 contract测试及Project View
  Playwright E2E 30项通过，其中2项覆盖v3 Resource/Guide saga。Tauri Project View测试17项通过。

### 已完成的本地真实运行交付

- 2026-08-01T10:58:58Z在同一真实本地PostgreSQL、Redis、Relay、`buzz-cli`、`buzz-admin`、
  `buzz-acp`与ACP child栈完成bounded v2 → v3 canary：一个active legacy Resource发布active Guide、
  经Human detached approval与operator重验后，在maintenance epoch 1完成Runtime → Assignment durable
  ACK、freeze、cutover、verify和resume。Resource revision从1变为2，未知`resource_kind`原样保留，
  locator仅保留在Guide/review archive；恢复后的Runtime ID与旧ID不同，strict base `RoleBriefV3`通过。
- 在同一进程中的独立empty-state Community完成prepare-v3、Human签名initialize-v3与enable：revision 1、
  generation 1、3个active对象、1个exact Human governance Assignment，v2-only操作明确unsupported。
- 真实运行发现并修复schema-v3 enable错误分派、Document `BIGINT`解码、CLI Role Brief/current v3分派、
  ACP child identity启动竞态，以及greenfield prepare中间态、active-object双计数和已消费prepare指针未清除。
- 完整证据位于`test-results/stage5-canary/20260801T105825Z-1319826`；
  `artifact-digests.sha256`自身SHA-256为
  `246aa8a657b2ef4f8f931291557db421cad3d4ea5ab33876e1c3cf0e1001516a`，逐文件校验通过。

`project_context_enabled`保持`false`，NIP-11不广告Context，未进入阶段6或broad rollout；本地scratch
数据库已删除，证据目录保留。

## 2026-08-01 — 阶段 4 完成

阶段目标：交付默认关闭的 Project View v3 backend 与可审计 cutover control plane，使
Resource 的 legacy locator 能在后续阶段经 Human review迁移为“资产坐标 + Guide”，但本阶段不切换
任何真实 Community。

### 已交付

- 新增 `buzz-project-view::v3` closed domain：locator-free Resource、canonical Context set、
  sparse Document target proof、RoleDefinitionV3单 head、ordinary object与continuity-only Role
  command、greenfield InitializeV3，以及 deterministic projection plan。Context capability默认关闭，
  nonempty替换 fail closed。
- 新增 v3 SDK command / object / entity / reset-or-incremental meta builder与 strict parser；所有 projection
  绑定 stable Relay signer、Project、generation、revision、source、coordinate和canonical time，v1/v2/v3
  wire互相 fail closed。
- 新增 additive migration `0033_project_view_v3.sql` 与 desired schema：schema 3 / Context flag、Guide FK、
  normalized Resource / Document Context indexes、immutable per-object provenance、review staging / committed
  Resource ledger、cutover receipt、durable maintenance epoch / baseline / ack / operation / invalidation ledger、
  greenfield provisioning receipt，以及 deferred cross-domain validators和append-only/monotonic triggers。
- Relay接入 schema-v3 initialize、ordinary object和Role continuity handler；每次写入在 Community exclusive
  lock内重验 Human或managed Agent权限。schema 3 对managed Agent强制 owner、active Assignment与exact
  Runtime fence，不再沿用 v2 optional-supervision兼容分支。
- readiness拆为 structural、pre-enable与advertised-write三层；NIP-11只对 enabled + normal + signer / pointer
  完整的 host广告 `buzz-project-view-v3`。`project_context_enabled = false`时不广告
  `buzz-project-context-v1`，raw nonempty Context command仍拒绝。
- `buzz-admin project-view v3 resources export / validate` 实现受限本地 review bundle、closed canonical
  manifest codec、detached reviewer signature、legacy Resource / Guide / membership / base pointer重新验证与
  immutable staging。输出目录和文件在Unix上限制为 owner-only，并拒绝symlink与覆盖。
- `buzz-admin project-view v3 cutover` 实现 replay-first exact receipt、frozen epoch preflight、一次 global
  revision与generation推进、每个Resource object revision +1、reviewer归因、全部current/bounded-history v3
  重投影、reset meta、deferred parity和事务回滚；commit后的Redis故障明确报告并保持 frozen。
- maintenance提供 `begin / status / freeze / abort / verify / repair / reproject / resume`。begin固定
  Assignment / Runtime / supervisor baseline；online-idle Assignment也必须durable quiesced ack，旧协议、
  活跃lease、scheduler claim、新Runtime或security invalidation都会阻止freeze。
- `repair`只接受三种canonical sorted action，并绑定postcard plan digest；一次plan只推进一个Project
  revision和受影响object revision。`reproject`只推进generation。两者都要求exact epoch、stable signer、
  Human operator、audit与idempotency receipt，成功后仍保持 frozen；只有后续structural verify与显式
  resume可恢复服务。
- runtime evidence、binding、scheduler claim与Project View writer共享同一 Community lock / maintenance
  fence；ban / unban / timeout、archive / unarchive等security path写audit-backed pre/post-cutover
  invalidation。generic schema-2/3 membership writer继续fail closed，不能绕过Role continuity + NIP-43
  coordinator；ban owner或仍承载active Assignment的Human/Agent同样拒绝。
- 新增idempotent `prepare-v3` 与owner-signed empty-state InitializeV3；未初始化Community不需要伪造一次
  legacy cutover，且preparation / initialize失败不会留下半初始化状态。

### 本阶段验证的安全与一致性边界

- migration从旧schema additive升级、并发migrator、desired-schema drift与全部Rust schema分支inventory
  通过；既有Community不自动切换schema或开启Context。
- Resource manifest任一base、legacy body、Guide revision/head/content、mapping digest、reviewer或signature
  变化都会fail closed；cutover重试只能返回exact receipt，不能重复推进revision。
- active与inactive Role都只有一个RoleDefinitionV3 head；Role tombstone才使用ordinary object head。
- normalized Context与JSON body在deferred validator中保持exact parity，Resource/Document删除受反向FK
  保护；Context flag关闭时初始化和cutover集合均为空。
- maintenance ack绑定完整Assignment / Runtime / supervisor coordinate和客户端协议版本；abort不复活旧
  Runtime fence，committed cutover不可回滚，post-cutover invalidation必须由之后的verify/repair/reproject
  显式resolve。
- v2 regression、v3 domain/SDK/Relay/admin unit gates以及fresh/upgrade/concurrent migration gate通过；未执行
  任何真实Community cutover。

### 明确未进入阶段 5

- 没有CLI/Tauri/Desktop v3 dual reader/writer、Resource Guide picker或`buzz resources guide`；
- 没有ACP RoleBriefV3 resolver、maintenance watcher / full-lifecycle child reap或fleet probe；
- 没有发布reviewed Guide、运行真实cutover、执行empty-state canary或扩大任何canary cohort；
- `project_context_enabled`保持false，没有Context chips、Role Context或Document正文注入。

阶段 4 exit后，阶段 5可以部署dual clients与ACP maintenance-aware runtime，并只对声明过的有界canary
执行reviewed Guide + cutover流程。

## 2026-08-01 — 阶段 3 完成

阶段目标：让 Human 在 Desktop 中不依赖 CLI 即可维护、审阅和回看可靠的 Markdown
Document revision，同时把 Relay signer、signed pointer、Community switch 与 ambiguous write
边界留在 native verified API 内。

### 已交付

- Tauri 注册并实现 `get_project_document_meta`、`list_project_documents`、
  `get_project_document`、`get_project_document_history` 与 `mutate_project_document` 五个命令。
  每次调用在首个 await 前捕获 opaque Community key、Relay endpoint和 signer key，再从捕获的
  endpoint验证 NIP-11 identity与 `buzz-project-document-v1`；切换 Community不能把进行中的读取或
  写入重定向到新 Relay。
- native读取只接受 expected Relay signer的 strict meta / head / revision projection，并验证 signed
  Project ID、projection generation、catalog pin、Document coordinate、current head pointer、完整
  history与 current/pinned边界。TypeScript只收到 verified read model，不解析 raw Nostr event。
- native写入使用 closed create / update / delete full snapshot、per-Document expected revision与同一
  exact signed event retry。成功结果同时验证 stable receipt和 immutable revision read-back，并把
  receipt的 actor、canonical time、catalog revision与 signed revision绑定；无法确定是否送达时只返回
  body-free `delivery_unknown` coordinate，不把 ambiguous结果误报为拒绝。
- 新增 body-free structured native errors和 typed HTTP transport category；Relay response body、
  Document正文、title与summary不会进入跨层错误对象。
- React Query实现 meta、catalog metadata、current、immutable pinned revision和history的权威分层
  cache key。页面先取得 verified meta再拉 metadata list；只有用户选中文档或revision时才读取正文。
  pinned cache不被普通 live/write invalidation破坏，signer/generation/Community变化会自然切换 authority
  key；没有新增 Community-scoped module singleton。
- Desktop新增 Documents route与侧栏入口，以及 metadata list、safe Markdown viewer、create/update
  editor、delete确认、body-free history和current/pinned revision切换。编辑器持续提示“Documents are
  not a Secret Store”，提交完整 snapshot并展示 exact line diff。
- 409 conflict保留 base snapshot和完整本地 draft，展示 latest与本地差异；只有用户显式选择
  “Edit on latest”后才以新 base再次提交，不自动 rebase或静默覆盖。
- live subscription只把 Relay-signed head/meta到达当作 native refetch hint，不把 event body注入 UI；
  close-race取消已调度 invalidation。Community remount后旧响应、旧列表、旧正文与未提交 draft都不会
  出现在新 Community。
- 新增 Desktop contract unit tests和 Playwright E2E，覆盖 metadata-first、safe Markdown、
  current/pinned隔离、完整 CRUD/tombstone、Secret warning、conflict draft preserve/diff、live raw body
  隔离、Community switch与 tampered signer/pointer fail closed；E2E产出 reader、editor warning和
  conflict diff三张 hash互异截图。

### 本阶段验证的安全与一致性边界

- local Community key只用于客户端隔离；signed Project ID与expected Relay pubkey才是协议 authority，
  两者不会互相替代。
- catalog与history读取固定 signer/generation/snapshot；比已验证 meta更新的 head/revision触发显式
  snapshot conflict，不拼接竞态快照。
- current读取必须解析 head精确指向的 revision；wrong signer、wrong pointer、cross-Document或
  cross-generation结果全部 fail closed。
- Markdown按不可信项目内容安全渲染；raw HTML不会变成可执行 DOM，list/history/live路径不携正文。
- receipt不是单独的成功依据；只有 exact command source对应的 immutable signed revision完成回读后
  才报告 applied。

### 明确未进入阶段 3

- 没有 Project View v3 Resource / Context backend、migration或 legacy Resource cutover tooling；
- 没有 Resource Guide picker、Context chips、Role Brief正文或 Agent prompt正文注入；
- 没有改变阶段 2 的 managed Agent owner、active Assignment与 Runtime fence写入授权；
- 没有 Secret Store、scrub / hard delete、全文搜索、semantic search、CRDT、external sync或 Mobile /
  Web Documents UI。

阶段 3 exit后，阶段 4可以继续实现 flag-off Project View v3 backend与 cutover tooling；阶段 5仍同时
依赖阶段 3和阶段 4完成。

## 2026-07-31 — 阶段 2 完成

阶段目标：在阶段 1 flag-off canonical kernel 上完成 Human / managed Agent 可用的 Relay 与
Agent-first CLI 纵向闭环，同时保持 Community-private、stable signer、Runtime fence 和
ambiguous delivery 的 fail-closed 边界。

### 已交付

- Relay `44301` command adapter 已接入现有 global `messages:write` gate；在解析正文前完成
  principal / membership / capability检查，再在 Community exclusive lock 内重验 Human或
  managed-owner membership、ban、active Assignment与 supervised Runtime fence。reducer、三类
  Relay-signed projection、receipt、command/event/history/current/meta在单 transaction原子提交，
  response bytes在 commit前构造，commit后只做不可失败的 transport delivery。
- Community-private read gate统一接入 WS REQ / COUNT、HTTP `/query` / `/count`、kindless / mixed /
  by-ID result guard、local fan-out和 Redis fan-out。读取要求 global或 `messages:read` credential，
  再检查当前 Human membership或 managed Agent owner membership；ban拒绝，timeout不撤销读取；
  disabled、schema、signer或 projection parity异常时 exclusive请求稳定 fail closed。
- 新增 closed `buzz_project_document` query extension：active-head分页绑定 projection generation +
  catalog revision + canonical UUID cursor，默认 100 / 最大 500；history绑定 generation + Document +
  max revision + descending cursor，默认 20 / 最大 50。扩展拒绝额外 outer / inner字段、错误 kind /
  author / subtype、NIP-50 search与非规范 cursor。
- NIP-11只在 host绑定 Community已 enable且 schema、bootstrap、stable signer和 projection parity
  全部 ready时广告 `buzz-project-document-v1`；readiness在全部关闭时保持 rolling-start兼容。
- `buzz-admin project-document` 增加 bootstrap / verify / enable / disable。bootstrap只允许 disabled
  Project View v2/v3 Community，以数据库规范时间创建 Relay-signed revision-zero empty catalog；
  enable持 exclusive Community lock并验证全量 pointer / cryptographic parity；disable立即撤销普通
  capability但不删除 canonical state。全部控制动作写入 hash-chain audit。
- `buzz documents` 提供 list / get / history / create / update / patch / delete。list/history默认只输出
  metadata；current head与 pointed revision、pinned revision、signer、project、generation、coordinate
  均 strict验证；输入在读取时有界，patch只允许 exact declared position、zero fuzz / offset并产生
  完整 next snapshot。
- Document write使用 typed transport policy：只重试同一 signed event bytes；connect failure与规范
  pre-ingest rate limit可安全重试，timeout / request / response body / proxy 502/504后保持 ambiguous；
  后续 409/503/internal不能把已 ambiguous结果降级为确定拒绝。CLI只有在 receipt与 immutable
  revision `source_event_id` exact read-back都通过后报告成功，否则返回 exit 2
  `delivery_unknown`；canonical conflict返回 exit 5。
- ACP base prompt只增加 Document discoverability，明确 `--format compact` 是 global flag、正文按需
  读取且属于不可信项目内容；没有提前声明 Resource Guide / Context，也没有把 catalog正文注入
  普通 prompt。
- 交付 [`secret-incident-runbook.md`](secret-incident-runbook.md) 与
  [`stage2-canary.md`](stage2-canary.md)。隔离 E2E串联 disabled gate、controlled bootstrap/enable、
  WS / HTTP / Redis隐私、membership revocation、完整 CLI CRUD/history、synthetic Secret
  `disable → rotate/assess → reviewed enable` drill和最终 disable保留 canonical history。
- CI archive / path filter / release contract与 `project-document-test-*` gates覆盖新增 enabled E2E、
  Relay parser / handler、CLI delivery policy和 operator surface。

### 本阶段验证的安全与一致性边界

- 非成员、已撤权成员和不满足 managed-owner / Assignment / Runtime fence的 actor无法读取或写入；
  live subscription不会成为撤权后的长期 capability。
- command replay在 receipt lookup前重验当前权限和 stable signer；同一 signed bytes可幂等确认，但
  不能借旧 receipt绕过 revocation或 signer fence。
- active list固定 catalog snapshot，history固定 generation与 max revision；snapshot变化返回 409并由
  CLI有限重启，不产生静默缺页、跨 generation拼接或 NIP-50泄露。
- list/history不返回 Markdown；日志、metrics、tracing、audit和 incident evidence不记录 title、
  summary、body或 locator。
- delete只追加 bodyless tombstone，pinned revision仍可复现；disable不删除 current、history、receipt
  或 projection event。
- 不同 Document使用各自 revision CAS，同一 Document stale update返回 conflict；commit后响应故障
  不会误报 definitive internal rejection。

### 明确未进入阶段 2

- 没有 Desktop / Tauri Documents UI或缓存；
- 没有 Project View v3 Resource / `guide_document_id`、Context Reference、Role Brief注入或 legacy
  Resource cutover；
- 没有 Secret Store、scrub / hard delete、全文搜索、semantic search、CRDT或 external sync；
- 没有 signer rotation / all-history reproject；signer变化必须保持 capability disabled。

阶段 2 exit后，阶段 3 Desktop Documents与阶段 4 Project View v3 backend / cutover tooling可以按
设计并行推进。

## 2026-07-31 — 阶段 1 完成

阶段目标：建立 Project Document 的 flag-off 可信内核，使领域状态、wire adapter、数据库
规范状态和 Relay 隐私边界都可独立验证，同时不向普通客户端暴露半成品 capability。

### 已交付

- 完成 `buzz-project-document` pure kernel：create / update / delete reducer、per-Document
  revision CAS、完整 snapshot、tombstone、no-op / ID reuse / overflow / reference保护、确定性
  projection plan和 property invariant tests。
- 完成 `buzz-sdk::project_document`：成员 command builder / parser、Relay head / revision /
  meta strict builder / parser、三类 projection bundle绑定校验和 Stage 0 golden event ID回归；
  `SdkError::InvalidProjection` 已泛化为协议中性的 `invalid Relay projection`。
- 新增 additive migration `0032_project_document.sql` 与 desired schema：默认关闭的
  `communities.project_document_enabled`，catalog state、current Document、不可变 revision、
  typed change / receipt表，以及 source shape、partial unique index、hard-delete、append-only、
  monotonic revision、pointer / count / projection parity约束。
- 新增 `buzz-db::project_document` restricted transaction：Community lock、canonical reload、
  当前成员 / ban / managed Assignment / Runtime fence检查、receipt前重新鉴权和 stable signer /
  pointer parity验证、pure reducer复算、strict projection bundle验证，以及 command / change /
  revision / current / meta / event的单事务 commit与 rollback。
- 提供 empty catalog bootstrap builder和 transaction tests，但阶段 1 没有任何 admin或 Relay
  路径调用它；生产 Community不会在本阶段被 bootstrap。
- 新增 `buzz-admin project-document status / preflight` 只读命令，检查 migration、flag、
  Project View schema、stable signer和 canonical/projection pointer parity；没有 enable、disable
  或 bootstrap命令。
- Relay 为 `44301` 固定 global `messages:write` auth seam，完成鉴权后稳定返回
  `unavailable:project_document:disabled`；`40905 / 40906 / 40907` 继续是 Relay-only，且没有
  Document public handler。
- 新增 Community-private deny skeleton：exclusive REQ / COUNT / HTTP query / count稳定 unavailable；
  mixed、kindless、by-ID、search result和 final fan-out都排除四个 Document kinds，因此测试误插
  event也不能从旧 wildcard路径泄露。
- 新增 `project-document-test-unit / test-db / test-e2e / test` recipes、独立 DB和真实 Relay
  脚本、release contract与 CI integration job；无基础设施 slice已接入现有 `test-unit`，测试
  artifact / cache hash / paths filter同时覆盖 DB、Relay、SDK、CLI、ACP和 admin。
- 共享 migration gate已推进到 version `32`，并同时检查 Project View与 Project Document
  migration / desired-schema drift。

### 本阶段验证的安全与一致性边界

- migration可从 `0031` additive升级，新旧 Community的 Document flag都保持 false；
- atomic failure不会退休旧 head / meta，也不会推进 canonical revision；
- 同一 command exact replay返回同一 receipt，但成员撤权后的 replay先失败于当前权限检查；
- 配置 signer与 catalog generation signer不一致时，在返回 canonical snapshot或查询 receipt前
  fail closed，因此 signer rotation不能借旧 receipt绕过 fence；
- 同一 Document并发更新只有一个成功，不同 Document仍使用独立 revision；
- update / delete形成完整不可变 history，tombstone不保留正文，hard delete和历史语义改写被拒绝；
- wrong signer使 preflight / projection parity fail closed；
- 真实 flag-off Community没有 canonical Document row或 projection event，普通成员通过 WS /
  HTTP都不能提交 command、伪造 projection或读取后台误插的 Document event；
- NIP-11不广告 `buzz-project-document-v1`，Relay readiness在所有 Community关闭时保持兼容。

### 明确未进入阶段 1

- 没有 Document public Relay handler、catalog pagination extension或 capability advertisement；
- 没有 `buzz documents` CLI、Desktop / Tauri UI、ACP正文读取或 Role Brief注入；
- 没有 admin bootstrap / enable / disable，也没有真实 Community状态迁移；
- 没有 Project View v3 Resource / Context Reference canonical tables或 legacy Resource cutover。

因此阶段 2 可以在这条已受保护的 transaction / projection seam上接入 Relay与 Agent-first
CLI纵向闭环，而不需要重新定义 Document canonical state或放松阶段 1 的默认拒绝边界。

## 2026-07-31 — 阶段 0 完成

阶段目标：在任何数据库迁移或 Relay public routing 之前，固定 Project Document v1、
Project View v3 跨域依赖和 legacy Resource cutover 的跨 crate 合同。

### 已交付

- 新增 [`NIP-PD`](../../../nips/NIP-PD.md)，固定 Document v1 的 kinds、command、
  projection、receipt、coordinates、limits、errors、read/privacy 规则和 signer rotation
  边界。
- 新增 [`NIP-PV3`](../../../nips/NIP-PV3.md)，固定 Resource、Context Reference、
  RoleDefinitionV3、greenfield InitializeV3、base RoleBriefV3、capability/version matrix、
  reviewed cutover 和 maintenance 状态机。
- 在 `buzz-core` 一次性注册 `44301 / 40905 / 40906 / 40907`，纳入 collision、indexed
  `d`、command、relay-only、Document protocol 和 Community-private protocol 分类。
- 把 wire-neutral `RuntimeFence` 移到 `buzz-core`，由 Project View v2 原路径 re-export，
  保持 source compatibility，Document 不再定义第二套 runtime epoch。
- 加入 `buzz-project-document` 的 Stage 0 pure skeleton：closed command、lifecycle、
  projection、receipt、coordinate、limit 和 validation 类型；不含 reducer、SQL、async、
  signer 或网络。
- 加入 Project View v3 pure contracts：Resource / Context canonical set、RoleDefinitionV3、
  InitializeV3、Role Brief Context/source boundary、migration canonical structs/digests，以及
  `normal → draining → frozen` maintenance transition。
- 共享 fixture 固定在
  [`docs/nips/fixtures/project-document-v1/`](../../../nips/fixtures/project-document-v1/)；
  包含三种 command、active/tombstone head/revision、empty/incremental/reset meta、receipt、
  Resource、三种 Context、RoleDefinitionV3、InitializeV3、base RoleBriefV3、migration bytes /
  digests / signature 和 fail-closed negative cases。
- 独立 golden parser 验证 Nostr event ID/signature、expected Relay signer、exact tags、
  coordinate/content pointer parity、JSON roundtrip、cross-Project/wrong-signer/extra-tag 拒绝，
  以及 Project View v1/v2/v3 与 Document payload 互相 fail closed。

### 本阶段确定的实现选择

- projection subtype tags：
  `buzz-project-document-head`、`buzz-project-document-revision`、
  `buzz-project-document-meta`；lifecycle tags：
  `buzz-project-document-active` / `buzz-project-document-tombstone`。
- head 的 revision marked tag 使用 marker `revision`；普通变更 source marker 使用
  `source`。
- stable receipt 的 exact fields 为：`schema_version`、`change_id`、`actor`、可选
  `acting_assignment_id`、`operation`、`document_id`、expected/committed Document revision、
  `catalog_revision`、`state`、`accepted_at`。不保存 projection event ID。
- reset meta 为空 `changed_heads` 并省略 source；ordinary command meta 恰有一个 changed
  head 和 source。空 catalog 是 generation 1、catalog revision 0 的 reset meta。
- greenfield InitializeV3 在 signed request 中显式绑定
  `preparation_operation_id`，而不是只依赖可变 Community pointer。
- Role Brief v3 availability 和 Document metadata 使用 closed `state` tagged objects；
  Context gate 关闭时完整 v3 shape 仍存在，但列表为空且 metadata 为 `not_required`。
- migration Human envelope 的 fixed bytes 使用 lowercase hex；canonical digest input 使用
  fixed byte arrays + postcard。review signature 是 exact 64-byte BIP-340 signature。
- shared golden fixture 只有一个位置；后续 SDK、CLI、Tauri 不得各自复制一套示例。

### 明确未进入阶段 0

- 没有新增或修改 PostgreSQL migration；下一可用 migration 仍留给阶段 1。
- 没有把 `44301` 加入 Relay scope map、global-only routing 或 Document handler；当前
  `required_scope_for_kind` 继续把它作为未开放 kind 拒绝。
- 没有新增 Document DB repository、reducer、SDK production builder/parser、CLI、Tauri、
  Desktop 或 ACP resolver。
- 没有设置 `project_document_enabled`、`project_context_enabled`，也没有在 NIP-11 广告
  `buzz-project-document-v1`、`buzz-project-view-v3` 或 `buzz-project-context-v1`。
- 没有真实 Community bootstrap、projection event 或 legacy Resource cutover。

### 验证

- `cargo fmt --all -- --check`；
- `cargo test -p buzz-core -p buzz-project-view -p buzz-project-document --no-fail-fast`；
- `cargo clippy -p buzz-core -p buzz-project-view -p buzz-project-document --all-targets -- -D warnings`；
- `cargo check --workspace --all-targets`；
- `git diff --check`。

因此下一阶段可以从 migration `0032` 和 flag-off kernel 开始，不需要重新决定 wire 字段、
canonical field order、digest domain 或 cutover state transition。
