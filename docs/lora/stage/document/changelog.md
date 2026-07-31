# Project Document 分阶段交付记录

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
