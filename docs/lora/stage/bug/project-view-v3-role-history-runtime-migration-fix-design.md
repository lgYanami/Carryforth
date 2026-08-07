# Project View v3 Role History 运行时全量迁移修复设计

状态：已批准并实施
日期：2026-08-07

> 本文处理 Role Continuity 与完整历史的直接事故。Project View 普通运行时不再保留
> v1/v2 fallback 的整体收敛见
> [Project View 普通运行时全面收敛到 v3 的修复设计](project-view-v3-only-runtime-migration-fix-design.md)。

## 1. 问题

Meeting `0ed366aa-6f94-4eff-83db-b8bf081fbf35` 的 Action Finalization 已成功追加
Role Checkpoint，并把 Project revision 推进到 25；但主持 Agent 随后执行
`buzz roles checkpoint list` 时得到：

```text
relay does not advertise buzz-project-view-v2
```

这不是 Checkpoint 写入失败，也不是 Meeting 关闭失败。写入使用 schema v3，CLI 的历史
回读却仍要求 schema v2。进一步检查发现 Role Continuity 运行时存在混合 major：

- CLI 的多个 `buzz roles` 读入口仍使用 v2 snapshot/history；
- Desktop Role mutation/history 同时保留 v2/v3 分支；
- ACP Role Brief 会在 NIP-11 上回退到 v2；
- Relay 的 `role_history` bridge scope 强制要求 v2 entity tag；
- v2→v3 cutover/reproject 只重投影当前 Proposal、Assignment、Commitment、最新
  Checkpoint 和最近 Handoff，而历史分页读取全部 canonical 行。

因此，只把 Relay 的 tag 从 v2 改成 v3 并不能正确修复。较旧的 ended/rejected/superseded
历史仍可能指向 v2 projection，纯 v3 parser 翻页到这些记录时仍会失败。

## 2. 决策边界

Role Continuity 的普通运行时只支持 schema v3，不再做 v2 fallback：

- `buzz roles` 全部读写使用 v3；
- Desktop Role mutation/history 只使用 v3；
- ACP managed Role Brief 只接受 Relay 广告的 v3 capability；
- Relay Role history 只接受显式 `v3_role_history` scope 和 v3 entity tag；
- schema v1/v2 对上述运行时返回 `migration_required` 或 unsupported，不静默降级；
- v2 代码只可作为 operator cutover/recovery 的迁移输入，以及必要的历史审计实现存在。

Role domain DTO 目前仍由 v3 reducer 复用 `buzz_project_view::v2` 命名空间中的成熟类型。
这属于内部 domain 复用，不是 v2 wire compatibility，不能据此重新开放 v2 parser、tag、
builder 或 capability fallback。

## 3. 数据迁移

### 3.1 全量 continuity projection

v3 cutover 和 v3 reproject 必须覆盖以下表的全部 canonical 行：

- `project_role_assignment_proposals`；
- `project_role_assignments`；
- `project_work_commitments`；
- `project_role_checkpoints`；
- `project_role_handoffs`。

不得再使用 `status = 'open'`、`ended_at IS NULL`、latest Checkpoint 或 recent Handoff
过滤。迁移为每一行生成严格 v3 projection，并在同一维护事务中更新原行的
`projection_event_id`。canonical UUID、内容、revision、关系和历史行数量保持不变；不删除、
重建或压缩 canonical 数据。

### 3.2 Readiness

Relay 只有在全部 Project object 和全部 Role Continuity pointer 均满足以下条件时才可广告
v3：

- pointer 非空且指向 live event；
- event 由当前 Relay projection key 签名；
- kind 正确；
- content 为 schema v3；
- projection generation 等于当前 generation。

这使不完整 cutover/reproject 在能力广告之前 fail closed，而不是把混合历史暴露给客户端。

## 4. 运行时协议

共享 SDK 固定以下常量，客户端和 Relay 不再复制字符串：

```text
v3_current_entities
v3_role_history
buzz-project-view-v3-entity
```

Role history 请求固定携带 pinned `project_revision`、`projection_generation`、closed
entity types 和可选 Role/Assignment/Member filters。服务端读取时同时验证 Community schema
和 `project_view_state.schema_version` 均为 3。

每个返回事件必须通过 SDK strict v3 parser，并与 canonical pointer 逐项核对：

- entity type；
- entity UUID；
- project revision；
- projection generation；
- Relay signer、Project coordinate、kind、tag 和签名。

`v3_current_entities` 使用独立 v3 DB 入口并执行相同校验，不再借用一个未检查 schema 的
v2-named reader。

## 5. 客户端迁移

### CLI

`roles list/get/current/brief/proposals/assignment/checkpoint/handoff` 全部从 verified v3
snapshot/history 读取；所有 write 直接构造 `RoleCommandV3`。接受写回执时必须看到
`schema_version = 3`，然后以 v3 projection 做 canonical readback。

### Desktop

Role mutation 只构造和签名 `RoleCommandV3`，metadata confirmation 只解析
`V3MetaProjection`。Role history 只发送 `v3_role_history` + v3 entity tag，并只解析 v3
projection。UI 可以继续消费稳定的 Role continuity DTO，但 native bridge 不再接受 v2 wire。

### ACP

Managed Role Brief 只接受 `buzz-project-view-v3`，只解析 v3 meta/object/entity。v2-only
Relay 进入 unavailable/migration-required 状态，不能复用旧 cache binding。Full/compact
Brief、Assignment reconciliation、Document/Context enrichment 行为保持不变。

## 6. 防回归

仓库检查增加 Role runtime v3-only 静态门禁，覆盖 CLI、Desktop 和 ACP 的关键文件，禁止
重新引入以下运行时 token：

- v2 capability fallback；
- v2 entity/meta parser；
- v2 Role command builder；
- legacy `role_history` scope；
- v2 entity tag。

Relay parser tests 固定：v3 scope/tag 成功，legacy scope、v2 tag 和跨类型 cursor 必须拒绝。
DB 结构测试固定 cutover/reproject/readiness 查询包含五类 continuity 全历史，且不得重新出现
open/active/latest/recent 截断条件。

## 7. 验收

至少满足：

1. schema v3 Community 中 append Checkpoint 后，`buzz roles checkpoint list` 立即回读成功；
2. rejected/expired Proposal、ended Assignment、多个 Checkpoint、超过三个 Handoff 经
   cutover/reproject 后仍全部可分页读取；
3. 每个历史 pointer 都能被 strict v3 parser 验证，canonical ID/revision/数量不变；
4. Desktop Role Inspector 能读取同一历史，Role mutation/readback 均为 v3；
5. ACP 下一次 Full Brief 能看到最新 Checkpoint/Handoff；
6. legacy scope/tag、schema2 snapshot 或混合 generation 全部 fail closed；
7. 迁移和测试不删除 Community 消息、Project View canonical 数据、Meeting 数据或 Agent
   配置。
