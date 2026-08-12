# 本地 Owner Bootstrap 误替换成员快照导致 Project View v3 能力消失修复设计

> 状态：已修复并完成本地现场验收
>
> 记录日期：2026-08-09
>
> 范围：Carryforth Desktop 本地 Owner 首次认领、NIP-43 成员快照、Project View v3
> 严格就绪检查、Project Context 能力公告
>
> 关联方案：[Desktop 本地化方案](../desktop-localization-plan.md)

## 1. 结论

本地化版本启动后，Agent 执行 `buzz project-view get` 返回：

```text
unsupported: relay does not advertise buzz-project-view-v3
```

这不是 Project View 被删除、降级到旧 schema，也不是关闭 Builderlab、远程 Relay 或远程账号
认证的直接结果。

直接原因是新增的本地 Owner bootstrap 接口在当前 Desktop 身份**已经是 Owner**时，仍无条件重新
发布了一份 NIP-43 成员快照。发布器会软删除上一份快照，而 Project View v3 canonical state 仍
精确引用上一份快照。严格就绪检查因此按设计 fail closed，Relay 随即停止公告
`buzz-project-view-v3`；依赖 Project View 就绪状态的 Project Context 能力也一并停止公告。

故障发生后，Project View canonical 数据仍保持 schema 3、Project revision 69、20 个 active
对象；20 份 Project Document 和 15 条 active Project Context Edge 也仍存在。此次事故是
**能力公告与写入门禁失效**，不是业务数据丢失或协议回滚。

## 2. 用户可见影响

- Desktop Project View 页面可能显示 Relay 不支持 Project View；
- Agent/CLI 在发出读取或写入命令前，被 capability discovery 门禁拒绝；
- Role、Assignment、Checkpoint 以及 Meeting Action Finalization 中的 Project View 物化无法
  继续；
- Project Context 因前置 Project View strict readiness 失败而不可用；
- Project Document 可独立继续公告，但不能代表整个 Project Space 已健康；
- Channel、消息、Nostr 身份、Agent 本地配置和既有 Project canonical rows 不会因此被删除。

该缺陷仅由精确的本地 Owner bootstrap 路径触发；远程部署没有暴露该 loopback-only HTTP
入口。但只要本地 Community 已初始化 Project View v3，一次正常 Desktop 启动就足以触发，
不需要并发或重复请求。

## 3. 现场证据

受影响 Community：

```text
host:          localhost:3000
community_id:  28c75f0f-670a-4dd8-a66d-17d093616c16
```

Project View 状态：

```text
project_view_enabled:        true
project_view_schema_version: 3
initialized:                 true
project_revision:            69
active_object_count:         20
strict_ready:                false
maintenance_state:           normal
```

Project Context 状态仍完整，但其上游不再就绪：

```text
context_revision:       20
active_edge_count:      15
integrity_ready:        true
structural_read_ready:  false
advertised_ready:       false
reproject_required:     true
```

Project View state 原本引用的成员快照为：

```text
f0279c5b6e7b2234efbdf957a3063f260391432fcf79c89be45feb0ac6b901fb
```

本地 Owner bootstrap 于 2026-08-09 15:11:02 CST 发布了新快照：

```text
ebdc7a7da6fb7c96cba69875a87dd0a6fdc572b7660d9fc7f2e7b14e3c61aa38
```

新旧快照的成员与角色内容一致，但旧快照被设置了 `deleted_at`。Project View v3 strict readiness
要求 `project_view_state.membership_snapshot_event_id` 指向的事件仍是当前、未删除、Relay 签名的
有效快照；因此“内容相同”不能替代“精确事件坐标相同”。

当前 `localhost:3000` 的 NIP-11 `/info` 仍公告 `buzz-project-document-v1`，但不再公告
`buzz-project-view-v3` 和 Project Context Edge，和数据库 strict readiness 结果完全一致。

## 4. 根因

### 4.1 DB 的幂等成功没有表达“是否发生写入”

`bootstrap_owner()` 当前返回 `Result<()>`。在 schema-v3 Community 中，如果唯一 Owner 已经是
同一个 pubkey，函数会 rollback 并返回 `Ok(())`：

```text
same sole Owner -> rollback -> Ok(())
```

这对 Owner 行本身是正确的幂等行为，但调用者无法区分：

- 本次真正创建了第一个 Owner；
- 当前身份原本已经是 Owner，数据库没有发生任何变化。

### 4.2 Relay 把所有 `Ok(())` 都当成“Owner 刚刚写入”

`../../../../crates/buzz-relay/src/api/local_desktop.rs` 当前使用：

```text
bootstrap_result.is_ok() && caller_role == owner
    -> publish_nip43_membership_list()
```

因此，即使 DB 已经走了幂等 no-op，Relay 仍执行成员快照发布。

### 4.3 通用成员快照发布器会无条件替换当前事件

`publish_nip43_membership_locked()` 在同一个替换坐标下：

1. 读取当前成员；
2. 构造一份新的 Relay 签名事件；
3. 软删除所有当前成员快照；
4. 插入新快照。

该行为适合真实成员变化后的快照收敛，但不能用作已初始化 Project View v3 Community 的无条件
“启动刷新”。Project View v3 将 NIP-43 快照 Event ID 作为 canonical 证明的一部分，独立替换
快照会破坏 Project View state、meta projection 与成员快照之间的原子关系。

### 4.4 NIP-11 正确地隐藏了不健康能力

Relay 没有误读 schema，也没有启动旧二进制。`project_view_ready_for_host()` 会调用
`project_view_v3_advertised_write_ready()`；后者发现原成员快照已删除，于是返回 false。

隐藏 `buzz-project-view-v3` 是正确的 fail-closed 行为。错误发生在更早的 Owner bootstrap 副作用，
不能通过放宽 readiness 或强制公告能力来掩盖。

## 5. 修复不变量

修复必须满足：

1. `AlreadyOwner` 是严格无副作用操作，不创建、替换或删除任何成员快照；
2. 首次 Owner 真正创建后，本地 Community 必须最终拥有与 canonical members 一致的 NIP-43
   快照；
3. 首次发布失败后，后续 bootstrap 可以安全补齐快照，不能因为 Owner 已存在而永久卡死；
4. 一旦 Project View v3 已初始化，成员快照只能由 Project View 成员治理协调路径原子推进；
5. Owner bootstrap 不得直接修改 Project View state、Document 或 Context；
6. 重复请求、React StrictMode 双调用和 Desktop 重启均不得改变 snapshot Event ID；
7. 不放宽 Project View strict readiness，不伪造 `/info` 能力公告；
8. 不重新执行 `init-v3`，不降级 schema，不直接改表，不删除或重建现有业务数据。

## 6. 代码修复方案

### 6.1 为本地 Owner 认领提供 typed outcome

不要再让本地接口从 `Result<()>` 和一次事后成员列表读取推断本次是否发生写入。增加范围明确的
typed outcome，例如：

```text
LocalOwnerBootstrapOutcome::Created
LocalOwnerBootstrapOutcome::AlreadyOwner
LocalOwnerBootstrapOutcome::Closed
```

- `Created`：本事务真正插入了 greenfield 首个 Owner；
- `AlreadyOwner`：同一身份已经是唯一 Owner，事务无写入；
- `Closed`：已有其他 Owner、已有不允许自动认领的治理状态，调用者不得提升身份。

该判断必须在现有 Community advisory lock 和 Owner 行锁内完成，不能在 Relay 先查再写，否则会
重新引入并发双认领窗口。若不修改通用 `bootstrap_owner()` 的现有调用契约，应新增仅供本地
bootstrap 使用的 DB API，避免扩大部署启动路径的改动范围。

### 6.2 将“发布”改成 greenfield reconcile，不做启动刷新

Relay 根据 outcome 与 canonical root 处理：

- `AlreadyOwner` 且 Project View state 已存在：直接回读并返回 `ready`，不调用成员快照发布器；
- `AlreadyOwner` 但仍处于无 Project View state 的 greenfield：只允许补齐首次失败的成员快照；
- `Closed`：返回 `already_initialized`，不产生任何成员或快照写入；
- `Created`：只在仍无 Project View canonical state 的 greenfield 边界内完成成员快照收敛。

首次 Owner 写入与成员快照发布当前跨两个事务。为保证发布失败可恢复，不能只写成
“仅 `Created` 时发布一次”。建议复用已有 NIP-43 reconciliation 检查：

```text
no Project View state
    AND current active snapshot != canonical relay_members
        -> publish one replacement snapshot

active snapshot already matches
        -> no_change

Project View state exists
        -> local bootstrap never publishes
```

如果首次发布失败，接口返回稳定错误；下一次请求虽然得到 `AlreadyOwner`，但在**无 Project View
state**的 greenfield 恢复边界内仍可发现 snapshot 缺失并补齐。接口只有在 snapshot 回读一致后
才返回整体 `ready`。

### 6.3 已初始化 v3 的成员变化必须走治理协调器

本修复不能尝试让通用 `publish_nip43_membership_list()` 自动修改 Project View 指针。对于已初始化
schema 3：

- bootstrap 只确认 Owner，不做成员事件写入；
- 真正的成员/Role level 变化继续通过 Project View v3 command/coordinator；
- coordinator 在一个受 Community lock 保护的提交中，同时生成成员 projection、更新
  `membership_snapshot_event_id`、meta projection 和计数；
- 任何绕过协调器的成员快照替换必须 fail closed，并留下结构化日志。

### 6.4 增加 bounded membership-snapshot restore recovery

现有 `project-view maintenance reproject` 会沿用
`project_view_state.membership_snapshot_event_id`，因此不能单独修复“旧指针已经指向 deleted
event”的事故。不能在操作手册中把现有 reproject 描述成足够的恢复手段。

进一步核对现场事件后发现：误发布 candidate 的成员集合虽然相同，但标签顺序是按成员创建时间，
不是 Project View v3 要求的 canonical `(pubkey, role)` 顺序。因此把 state/meta 直接重绑定到 candidate
并不能恢复 strict readiness。最安全的恢复方式是恢复 Project View 已经引用、且 wire 仍合法的旧
snapshot，同时退役错误 candidate。

增加一个仅用于这一事故形态的 typed operation：`restore-membership-snapshot`。当前 Community 是
greenfield v3，没有 schema2→v3 cutover epoch，因此该命令不伪造 maintenance epoch；它以独占
Community lock、精确坐标、完整 wire 校验和 append-only recovery receipt 构成一次 one-shot bounded
maintenance transaction。命令必须显式绑定：

```text
community_id
expected_project_revision
expected_projection_generation
expected_old_membership_event_id
candidate_current_membership_event_id
expected_relay_pubkey
idempotency_key
```

执行前必须在 Community lock 内验证：

- old ID 正是 Project View state 和当前 meta projection 引用的 ID；
- old event 的异常仅为已被替换/soft-deleted；
- old snapshot 仍通过完整 v3 canonical 顺序、签名、kind、空 content 与 protected-tag 校验；
- candidate 是该 Community 唯一 current、Relay 签名、kind 13534、无 Channel、带 NIP-70
  protected 标记的成员快照；
- candidate 的全部 `(pubkey, role)` 与 canonical `relay_members` 完全一致；
- old snapshot、candidate 与 canonical members 的成员语义一致；若成员或角色真的发生变化，拒绝
  该恢复操作，要求走正常 Project View membership governance；
- Project coordinate 与请求的 revision/generation 完全一致，maintenance state 为 normal 且没有活动
  epoch；Document、Context 和业务对象均不在本操作的写集合中。

提交时应在一个事务中：

1. 退役唯一 current、但 wire 顺序不适合作为 v3 coordinate 的 candidate；
2. 恢复 state/meta 已引用且完整 canonical 的旧 snapshot 为唯一 current head；
3. 保持 Project View state、meta、object/entity pointers、revision 与 generation 不变；
4. 在同一事务中执行完整 v3 structural/wire validation；
5. 写入 audit-backed、append-only、可精确幂等重放的 recovery receipt。

该操作不推进 Project revision 或 projection generation，不修改任何业务对象或 Role/Assignment
语义。若 candidate 反映真实成员变化，或任何 expected coordinate 不匹配，必须零部分写入地返回
conflict。错误 candidate 保留为 soft-deleted 审计证据。

### 6.5 保留认证与本地边界

本次不取消：

- loopback peer 校验；
- 精确 `Host: localhost:3000`；
- NIP-98 请求签名与 replay 防护；
- managed Agent 不能成为 Owner；
- 已有 Owner 永不被其他本地身份自动替换。

问题与认证强弱无关，不能通过移除 NIP-98 或改回远程账号来解决。

## 7. 当前数据的无损恢复方案

必须先修复并部署上述 bootstrap 代码，否则恢复后下一次 Desktop 启动会再次替换快照。

随后使用本方案新增的 bounded recovery operation 恢复当前 Community：

1. 记录 Project View、Document、Context、成员和 Meeting 基线；
2. 以精确 revision/generation/old/candidate 坐标执行 `restore-membership-snapshot`；
3. 命令在一个独占事务中恢复旧 canonical snapshot、退役 candidate 并完成 strict wire 验证；
4. 回读 `/info`、Project View、Document 与 Context；
5. 连续重启两次，确认 bootstrap 与 startup reconcile 都不再替换 snapshot。

恢复过程不得：

- 直接 `UPDATE project_view_state`；
- 绕过 typed recovery 直接手工修改事件 current-head 状态；
- 重新执行 `project-view init-v3`；
- 清空或重建 Community；
- 回滚 Project revision 69 或复制对象。

预期只新增 audit/recovery receipt；Project revision 与 projection generation 均保持不变，Goal、Plan、Stage、
Requirement、Role、Work、Document 和 Edge 的 canonical 业务内容保持不变。

## 8. 测试与验收

### 8.1 DB/Relay 回归

1. 已初始化并 strict-ready 的 v3 Community，同一 Owner 连续调用两次：
   - 返回 `AlreadyOwner`；
   - `relay_members` 不变；
   - active NIP-43 snapshot Event ID 不变；
   - `project_view_state.membership_snapshot_event_id` 不变；
   - strict readiness 始终为 true。
2. 全新 Community 首次认领：
   - 只有一个身份成为 Owner；
   - 只生成一份当前成员快照；
   - 重复请求不生成第二份快照。
3. 并发两个身份认领：只有一个 `Created`，另一请求得到 `Closed`，不能发生 Owner 替换。
4. 首次 Owner 已提交、快照发布失败：重试只补齐缺失快照，不重复 Owner、不永久卡死。
5. 已有其他 Owner：调用者不升级、快照不变化。
6. managed Agent identity：继续稳定拒绝。
7. bounded recovery：
   - old/candidate/current members 完全匹配且 old wire canonical 时，原子恢复 old current head；
   - candidate signer、kind、Community、成员内容或 expected coordinate 任一不匹配时零写入拒绝；
   - 恢复后 candidate 保持 soft-deleted，Project revision/generation/pointers 全部不变；
   - 相同 idempotency key 精确重放 receipt，不再次修改事件。

### 8.2 Capability 回归

在 scratch Community 中初始化并启用完整功能后，重复执行本地 bootstrap 与 Desktop 重启：

- `/info` 持续公告 `buzz-project-view-v3`；
- Project Document 与 Project Context capability 不发生回退；
- `buzz project-view get` 正常；
- `buzz documents list` 正常；
- Project Context exact/incident/contains-all 正常；
- 不出现新的 soft-deleted referenced membership snapshot。

### 8.3 本地现场验收

修复现有 `localhost:3000` 后确认：

- Project View schema 仍为 3，Project revision 与 20 个对象完整；
- Document catalog 20 份文档完整；
- Context 15 条 active Edge 可回读；
- Role Brief、Assignment 与 Checkpoint 可读取；
- 新建一次非破坏性 Project View 变更并回读；
- Desktop 连续重启两次，snapshot ID 和 strict readiness 不再变化；
- Agent 可以完成包含 Project View、Document 与 Context 的 Meeting Action Finalization。

所有 DB 集成测试必须使用独立 scratch database，禁止指向 Local Dev 主库；现场验收只允许正常
协议/管理命令，不运行 migration reset、`DROP`、`TRUNCATE` 或 volume 删除。

## 9. 可观测性

本地 Owner bootstrap 日志至少记录低基数字段：

```text
outcome=created|already_owner|closed
snapshot_action=published|already_current|governed_noop|failed
community_id
```

不得记录私钥、NIP-98 payload 或签名凭据。

增加回归指标/断言，确保：

- `already_owner` 不增加 NIP-43 publication counter；
- initialized v3 bootstrap 不产生 snapshot replacement；
- readiness 从 true 变 false 时能定位到 referenced snapshot missing/deleted，而不只显示
  “unsupported”。

## 10. 实施顺序

1. 增加 typed local Owner bootstrap outcome 和 DB 单元/并发测试；
2. 收敛 Relay handler 的 publish/reconcile 条件；
3. 实现 bounded `restore-membership-snapshot` operation 及失败原子性测试；
4. 补充 initialized-v3 重复启动与 NIP-11 capability 回归；
5. 在独立 scratch database 完成首次认领、失败重试、指针事故恢复和 Project View 全链验证；
6. 提交代码并重建 Relay/Desktop；
7. 使用新 maintenance operation 无损修复当前 `localhost:3000`；
8. 完成 Project Context 与完整能力公告现场回读；
9. 重启两次确认缺陷不再复现。

## 11. 完成标准

- 本地 Owner 首次认领仍可在全新环境完成；
- 同一 Owner 的任何重复 bootstrap 都是严格 no-op；
- 已初始化 Project View v3 不再被本地启动替换成员快照；
- `/info` 在重复启动后持续公告完整能力；
- 当前 Project View、Document、Context 数据无损恢复；
- 不存在直接 SQL 修补、重新初始化、schema 降级或数据重建；
- 相关自动化测试和两次真实重启验收均通过后，文档状态方可改为“已修复”。

## 12. 非目标

- 不恢复 Buzz 远程 Community；
- 不重新启用 Builderlab 或远程账号认证；
- 不取消本地 Nostr 身份、NIP-42 或 NIP-98；
- 不修改 Project View schema 版本；
- 不新增 Project View v4；
- 不通过放宽 strict readiness 来隐藏投影不一致；
- 不借本修复清理或重建现有 Local Dev 数据。

## 13. 实施与验收记录

2026-08-09 已完成以下交付：

- DB 增加 `LocalOwnerBootstrapOutcome`，本地认领明确区分 `Created`、`AlreadyOwner` 与
  `Closed`；同一 Owner 的重复请求不再从通用 `Ok(())` 推断发生过写入；
- Relay 本地入口只在尚无 Project View root 的 greenfield 状态补齐 NIP-43 快照；已初始化
  v3 返回 `governed_noop`；
- 通用 NIP-43 reconciliation 与真正的发布事务均在 initialized v3 上 fail closed。后者在
  Community lock 内复核，消除了“预检后、发布前初始化 v3”的竞态窗口；
- 新增 migration 0055 和 `restore-membership-snapshot` 管理命令。恢复请求绑定精确
  Community、revision、generation、old/candidate Event ID、Relay signer、Human operator 与
  idempotency key，并写入不可变 audit-backed receipt；
- 恢复操作只切换两份成员快照的 current/retired 状态，不修改 Project View state/meta/object
  pointer、Project revision、projection generation、Document 或 Context 业务数据。

自动化与隔离数据库验收：

- `cargo clippy -p buzz-db -p buzz-relay -p buzz-admin --all-targets -- -D warnings` 通过；
- `cargo test -p buzz-db --lib`：142 passed；
- Relay 本地入口 2 项单测与 `buzz-admin` 10 项测试通过；
- 独立 scratch DB 验证首次 Owner `Created -> AlreadyOwner`、另一身份 `Closed`，且不发生 Owner
  轮换；
- 独立 scratch DB 验证 initialized v3 的通用快照发布在锁内稳定拒绝；
- 主库副本验证 bounded recovery 首次执行成功、同 idempotency key 返回 replay、错误 expected
  revision 零写入拒绝；两份 scratch DB 均已在验证后删除。

`localhost:3000` 现场恢复结果：

```text
Project View:     schema 3 / revision 69 / generation 1 / strict-ready=true
Project objects:  20 active / 29 total
Documents:        catalog revision 20 / 20 active
Project Context:  revision 20 / 15 active Edge / advertised_ready=true
Channels:         16
Relay members:    5
Meetings:         14
```

恢复前后事件总数 `1795`、live event 数 `1517` 均未变化；只新增 1 条不可变 recovery receipt
及对应审计记录。canonical membership snapshot 恢复为：

```text
f0279c5b6e7b2234efbdf957a3063f260391432fcf79c89be45feb0ac6b901fb
```

完成两次停止/启动回归及锁内发布门禁补强后的最终启动回归。每次 React 开发模式产生的两次本地
Owner 请求均为 `AlreadyOwner + governed_noop`，snapshot Event ID 未变化；`/info` 持续公告：

```text
buzz-project-view-v3
buzz-project-document-v1
buzz-project-context-edge-v2
```

本次没有运行 reset、`DROP`/`TRUNCATE`、volume 删除或主库直接 SQL 修补。Relay、ACP 与 Desktop
当前已使用修复后的代码启动。
