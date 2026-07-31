# Project Document 分阶段交付记录

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
