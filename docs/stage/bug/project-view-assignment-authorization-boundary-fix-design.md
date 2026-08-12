# Project View Community 授权与 Assignment Fence 边界修复设计

> 状态：已实现并通过定向验证
>
> 设计确认：2026-08-02；实现交付：2026-08-03
>
> 范围：Project View schema v2/v3、Role Continuity、Agent-first CLI 与 managed ACP 纵向链路

> 2026-08-03 局部覆盖：本文“Role definition CRUD 属于所有 Community member 的 ordinary
> write”结论已被
> [Project Role 治理授权与 admin Role 创建修复设计](project-role-governance-authorization-and-admin-role-creation-fix-design.md)
> 收敛。Assignment 仍不是整个 Project 的 ACL，但 Role 定义本身现在属于 owner/Active
> Leader 治理对象；非 Role 普通对象边界不变。

## 1. 结论

本修复采用以下权威边界：

1. Community 身份与 Buzz 既有 `owner/admin/member` 是 Project View 基础读写权限来源；
2. Project Role 不建立第二套 Project ACL；
3. Assignment 表示 Member 承担 Role 的任期，只授权并 fence **以该 Role 行动**的写入；
4. Runtime fence 约束某个 managed Agent Assignment 的当前执行实例，不授予 Community
   权限，也不能替代 Assignment；
5. 候选者在接受 Offer 之前必然没有目标 Assignment，因此申请、接受、拒绝或撤回属于自己
   的 Proposal 不能要求目标 Assignment；
6. Role level 与 Community `admin/member` 的同步继续保留。这是 Community 权限一致性，
   不是独立 Project ACL；
7. 普通 Project View 对象写入不要求 Assignment。若命令显式携带 Assignment，则 Relay
   仍验证它和对应 Runtime fence，但它不是普通写入成立的前提。

本次修复不是简单为 `accept_proposal` 增加一个特例。需要拆开当前混合在同一函数中的：

- Community 写入资格；
- candidate identity；
- owner / Leader 治理 authority；
- role-bearing Assignment；
- managed Runtime fencing。

## 2. 事故与直接原因

当前 schema v3 场景中：

1. Community owner 为尚未承担 Role 的 managed Agent 创建 Offer；
2. Agent 以自己的稳定公钥执行 `accept_proposal`；
3. CLI 读取 verified snapshot，正确发现当前没有 active Assignment；
4. CLI 因此提交 `acting_assignment_id = null`；
5. Role Continuity 纯领域校验允许候选者处理自己的 Proposal；
6. DB coordinator 随后调用通用 `validate_v3_actor_in_tx()`；
7. 该函数仅因为 actor 是 managed Agent 且没有 `acting_assignment_id`，返回
   `ActingAssignmentRequired`；
8. Relay 将其映射为
   `restricted:project_view:acting_assignment_required`。

形成了不可满足的循环：

```text
接受 Offer
    └── 成功后才创建 Assignment

当前 v3 gate
    └── 已有 Assignment 才允许接受 Offer
```

这不是 Proposal 授权失败，也不是 Community membership 失败。命令已经由正确候选者签名，
真正失败的是 DB 层把 Assignment 前置条件无条件应用到了 candidate operation。

## 3. 设计依据与现有不一致

### 3.1 Project View 原始边界

[Project View 对象与关系设计](../project-view/object-relation-design.md)规定 Human 与 Agent
可以对同一组对象和关系执行等价操作，首版沿用 Community 成员边界，并明确“权限角色不是
Project Role”。

[Project View 后端实现设计](../project-view/backend-implementation-design.md)进一步规定：

> 所有合法 Community 成员等权修改；Project Role 不参与权限。

对应的基础授权实现也把以下 principal 视为 Project View member：

- 当前直接 `relay_members` 成员；或
- 已验证 managed Agent，且其 Human owner 是当前直接成员；
- actor 和 owner 均未被 ban，写入还需满足 timeout、Community-global credential 和
  `MessagesWrite`。

### 3.2 Role Continuity 边界

[Role Continuity 设计](../role/role-continuity.md)明确：

```text
Community Member + active Role Assignment = active Project Member
```

但不能反向推出：

```text
Community Member => 一定具有 active Role Assignment
```

未分配 Member 可以是候选者、观察者或等待指派者。Assignment 使其能够以某个 Role 认领
Work、作出 Commitment、提交 Checkpoint 或执行 Leader 治理，不是 Community 身份本身。

[Role Context 设计](../role/project-view-role-continuity-context-design.md)也把 Assignment
定义为“角色写入 fence”，并明确：

```text
Assignment 决定当前是否有权以该 Role 行动
```

### 3.3 后续实现偏移

[Role 实现设计](../role/implementation-design.md)后来增加了更严格的 managed Agent 策略：

- known managed Agent 修改普通 Project View 对象必须携带 active Assignment；
- 未分配 managed Agent 只能读取、请求 Role 和处理自己的 Proposal。

当前代码形成两种结果：

1. schema v2 的 Role command 仍由纯领域模型保留 candidate exception；
2. schema v2 普通对象写入要求 managed Agent 必须有 Assignment；
3. schema v3 又把该要求提升为 Role command 和普通对象 command 共用的无条件 gate；
4. 因而 schema v3 连明确允许的 candidate operation 也被阻断。

第 4 点是确定的实现 Bug。第 2、3 点则把 Assignment 从“Role 任期/fence”扩大为 managed
Agent 的普通 Project View ACL，与原始 Community 授权模型和 Role Context 语义不一致。

## 4. 修复后的权威模型

### 4.1 五个独立判断

每次写入按以下顺序独立判断：

```text
1. Community principal eligibility
   actor 是否是当前直接成员，或 owner 仍合格的 verified managed Agent

2. Buzz write admission
   签名、principal、Community-global credential、MessagesWrite、ban/timeout、rate limit

3. Operation authority
   candidate identity、Community owner、active Leader、当前 assignee 等领域关系

4. Assignment attribution/fence（仅适用时）
   命令是否以某个当前 Role 任期行动，Assignment 是否 active 且属于 signer

5. Runtime fence（仅 managed + Assignment-bearing 时）
   supervisor binding、runtime ID、epoch 和 lease 是否为当前值
```

任何一层都不能代替另一层。特别是：

- 通过第 1、2 层不意味着可以执行 Leader 或 Role-bearing 操作；
- 具有 Assignment 不会让一个已失去 Community 资格的 actor 继续写入；
- Runtime fence 不能把非成员变成成员；
- 没有 Assignment 不会自动撤销 Community member 的普通 Project View 能力。

### 4.2 操作矩阵

| 操作类型 | 示例 | Community gate | Assignment | managed Runtime fence |
|---|---|---:|---:|---:|
| 普通 Project View 读取 | snapshot、对象详情、Role Directory | 必须 | 不需要 | 不需要 |
| 普通 Project View 写入 | Goal/Plan/Stage/Issue/Work/Resource、非 Role Context Reference 更新 | 必须 | 不需要 | 不需要 |
| Role definition 治理 | Role create/update/deactivate/delete、Role Context Reference 更新 | owner 或 admin + active Leader | non-owner Leader 必须 | 仅显式归因时验证，不授予权限 |
| Candidate identity | `request_role`、接受自己的 Offer、候选者拒绝、创建者撤回 | 必须 | 不需要 | 不需要 |
| Community owner 治理 | Offer/Authorize、admin Role、结束/替换 Assignment | owner | 不需要 | 不需要 |
| Leader 治理 | 普通 Role Offer/Authorize、允许范围内的结束/替换、Work responsibility | admin + active Leader | 必须 | managed Leader 必须 |
| Role-bearing 行为 | Work Commitment、Checkpoint、Handoff、replacement/unable report | 必须 | 必须 | managed assignee 必须 |

Role definition 仍存储为 Project View 对象，但不再属于所有 member 的普通 CRUD。涉及
`member` Role 的定义治理要求 owner 或 Active Leader；涉及 `admin` level 的定义治理只允许
直接 Human owner。已有 active Assignment、开放 Proposal、Community 等级同步等领域不变量
继续生效。

### 4.3 Candidate Proposal 的精确规则

以下动作基于稳定 Member identity，而不是 Role identity：

- `request_role`：申请者本人；
- `accept_proposal`：Proposal 的 `candidate_pubkey`；
- `reject_proposal`：候选者本人时；
- `withdraw_proposal`：Proposal 创建者本人；
- 已到期 Proposal 的处理继续遵循当前领域规则。

这些动作可以在 `acting_assignment_id = null`、`runtime_fence = null` 时提交。Relay 必须在
同一 Project lock 和事务内重新验证 Proposal、candidate/creator、当前 Community 资格、过期
状态、authorizer authority 和 Proposal 中保存的目标/候选旧 Assignment fence。

接受 Offer 仍然是一个原子 compound transition：

```text
重新验证 Proposal 与候选资格
    -> 结束目标 Role 的旧 Assignment（如果存在）
    -> 结束候选者的旧 Assignment（如果 Proposal 明确包含替换）
    -> 同步 Community member/admin 等级
    -> 创建唯一的新 active Assignment
    -> 消费 Proposal
    -> 更新 projection / receipt / project revision
```

因此不需要用 `acting_assignment_id` 重复保护 candidate replacement；Proposal 自身已有
`expected_target_assignment_id` 与 `expected_candidate_assignment_id` 的 compound fence。

### 4.4 普通写入携带可选 Assignment

协议字段保持兼容：普通 Project View command 仍可带可选的 `acting_assignment_id` 和
`runtime_fence`，但规则调整为：

- 两者都省略：以 Community member 身份执行普通写入；
- 提供 Assignment：Relay 必须验证其 active、属于 signer，并在 managed v3 下验证 exact
  Runtime fence；
- 提供无效、已结束或他人的 Assignment：拒绝，不能静默降级为 Community write；
- 只提供 Runtime fence、不提供 Assignment：保持 invalid；
- Assignment/Runtime 校验失败后不能自动删除字段重试。

第一方 CLI 对普通 Project View CRUD 默认使用 Community member 模式，不再为了通过通用
gate 自动要求 Assignment。未来若需要“明确以 Role 归因普通对象修改”，应增加显式模式，
不能根据 fence 文件是否存在静默切换。

### 4.5 Assignment 结束后的语义

Assignment 结束后：

- 旧 Assignment 和旧 Runtime 不能继续执行任何 role-bearing 或 Leader 操作；
- actor 若仍满足 Community member/verified-owner 边界，可以继续读取和执行普通 Project
  View CRUD；
- 若需要撤销其全部 Project View 能力，应通过 Community remove、owner 关系撤销、ban 或
  timeout 表达，不能用 Assignment 结束隐式代替 membership revocation。

这是本修复必须用测试固定的语义，不应被当成“旧 Runtime 绕过”。该 actor 仍可进行的只是
Community 已授权的普通动作，不能声称代表已结束的 Role。

## 5. 实现设计

### 5.1 领域层：保持一个 Role authority 事实源

涉及：

- `../../../crates/buzz-project-view/src/v2/role_continuity.rs`
- `../../../crates/buzz-project-view/src/v3/role_continuity.rs`

要求：

1. 保留现有纯 reducer 对 candidate、owner、Leader、assignee 的操作级判断；
2. 保留 candidate 处理 Proposal 时不要求 Assignment 的规则；
3. 保留 `require_governor_fence()` 和 `require_assignee_action()` 对 Leader/Role-bearing
   操作的精确 Assignment 检查；
4. v3 继续复用同一 Role Continuity reducer，不复制另一份操作列表；
5. 将 v3 注释中“schema 3 的每个 managed Agent 都必须携带 Runtime fence”修正为
   “每个 managed Assignment-bearing command 必须携带 exact Runtime fence”。

若 DB、CLI 需要判断某个请求是否为 candidate/role-bearing，应从领域 crate 暴露共享的
closed classification，不能在三处维护容易漂移的 `matches!` 列表。分类至少表达：

```rust
enum RoleActorIntent {
    CandidateIdentity,
    Governor,
    RoleBearing,
}
```

其中 `reject_proposal` 等 actor-dependent 操作最终仍由当前 verified state 判断；枚举只用于
路由所需上下文，不能替代 reducer 授权。

### 5.2 DB：拆分 v3 通用 actor validator

涉及：

- `../../../crates/buzz-db/src/project_view_v3.rs`
- `../../../crates/buzz-db/src/project_view_v2.rs`
- `../../../crates/buzz-db/src/project_runtime.rs`

当前 `validate_v3_actor_in_tx()` 同时负责 Community、Assignment 和 Runtime，导致调用方
无法表达 candidate 或 Community-only command。将其拆成两个职责：

```text
validate_project_view_community_writer_in_tx(...)
    -> 校验 direct member / verified owner-backed Agent
    -> 校验 actor + owner ban/timeout
    -> 返回 actor classification（human / managed）

validate_optional_assignment_runtime_fence_in_tx(...)
    -> acting_assignment_id=None：要求 runtime_fence=None，成功
    -> acting_assignment_id=Some：校验 active + signer ownership
    -> managed schema v3：进一步 RequireSupervisedRuntime
    -> v2：维持 LegacyOptionalSupervision 兼容策略
```

Role v3 command 的准备顺序调整为：

```text
Community transaction gate
    -> 加载当前 continuity state
    -> 纯领域 replay actor 校验
    -> 校验命令实际携带的 optional Assignment/Runtime
    -> current-authority receipt replay gate
    -> 完整 reduce / persist / projection / receipt
```

关键点：

- DB 不再出现 `managed && acting_assignment_id.is_none() => reject` 的无条件规则；
- role-bearing/Leader 缺少 Assignment 仍由纯领域规则稳定返回
  `acting_assignment_required`；
- candidate command 不携带 Assignment 时进入 reducer，而不是在前置通用 gate 被拒绝；
- receipt lookup 之前仍重验当前 Community 和操作 authority，旧 receipt 不能绕过撤权；
- v3 ordinary object command 使用相同 Community gate和 optional fence，而不是 Role gate。

### 5.3 v2 普通对象路径同步修正

`validate_project_object_actor_fence()` 当前对 managed Agent 无条件要求
`acting_assignment_id`。为消除版本间不一致，需要：

- 删除该无条件要求；
- 没有 Assignment 时按 Community member 普通写入继续；
- 显式提供 Assignment 时继续验证 active、signer ownership 和 v2 Runtime policy；
- 保留 active Assignment Role 不能被通用停用/删除、Role level 不能被普通 patch 提权等
  现有领域保护。

这样 schema v2 和 v3 的授权语义一致，升级不再改变“同一 Community member 是否能修改
普通 Project View 对象”。

### 5.4 CLI：按操作语义组装 fence

涉及：

- `crates/buzz-cli/src/commands/roles.rs`
- `crates/buzz-cli/src/commands/project_view.rs`
- `crates/buzz-cli/src/commands/documents.rs`
- `crates/buzz-cli/src/commands/project_view_snapshot.rs`

调整如下：

1. `buzz roles proposal accept/reject/withdraw` 在 actor 以 candidate/creator 身份操作时默认
   不注入 Assignment/Runtime；
2. `request_role` 不要求 Assignment；Proposal 的 compound fence继续记录申请者已有任期；
3. managed Leader 和 role-bearing command 仍在签名前读取 verified snapshot，注入当前
   Assignment 与 exact Runtime fence；
4. `buzz project-view create/update/delete` 对 v2/v3 默认构造
   `acting_assignment_id=None, runtime_fence=None`；
5. 不允许读取 Assignment 或 fence 失败后静默从 Role-bearing 模式降级为 Community 模式；
6. 将“读取可选当前 Assignment”和“强制需要当前 Assignment”拆成不同 helper，避免
   `v2_acting_assignment()` 同时服务普通 Project View 与严格 Document 写入；
7. `buzz documents` 当前 managed Assignment + Runtime 强制策略保持不变，不因 helper
   重构被意外放宽。

CLI 只做 fail-fast 和正确组包，最终 Community、Proposal、Assignment、Runtime 与 revision
判断仍由 Relay 事务完成。

### 5.5 ACP 与 Runtime 生命周期

Candidate Agent 接受 Offer 后：

1. 接受命令本身不需要 Assignment 或 Runtime fence；
2. 事务成功后产生新的 active Assignment 和 Community membership projection；
3. ACP/managed supervisor 观察到 revision/Assignment 变化；
4. 为新 Assignment 建立或刷新 supervisor binding、runtime epoch 和 fence file；
5. 下一完整 turn 注入 assigned Role Brief；
6. 在 exact Runtime fence 可用前，role-bearing command继续 fail closed；
7. 普通 Project View CRUD 不依赖该 provisioning 完成。

本修复不能把“接受成功”与“当前 Runtime 已完成 Assignment supervision”混成同一前置条件。
如果 supervisor provisioning 失败，应表现为 Assignment 已建立但 role-bearing Runtime
暂不可用，并提供明确诊断；不能回滚已经由双方确认的 Proposal，也不能再次要求候选者用
不存在的 Assignment 接受。

### 5.6 Relay 错误语义

保留现有稳定错误族，但保证触发条件准确：

| 条件 | 结果 |
|---|---|
| 非当前 Community principal | `restricted:project_view:not_authorized` 或现有 membership 错误 |
| role-bearing/Leader 操作缺少 Assignment | `restricted:project_view:acting_assignment_required` |
| 显式 Assignment 已结束、错误或不属于 signer | `conflict:project_view:acting_assignment` |
| candidate 不是 Proposal candidate | `restricted:project_view:candidate_required` |
| managed Assignment-bearing v3 command 缺失/错误 Runtime | 现有稳定 runtime fence 错误 |
| candidate command 没有 Assignment | 不构成错误 |

不再用 `acting_assignment_required` 表达普通 Community 写权限不足。

## 6. Project Document 与 Resource 的范围边界

Resource 是 Project View ordinary object，因此本修复后的普通 Resource CRUD 和 Resource
Context Reference 更新遵循 Community member 权限，不要求 Assignment。

Project Document 是独立协议、独立 revision 与独立 writer。当前 Document 设计明确要求
managed Agent 写入携带 active Assignment + exact Runtime fence。本次不顺带修改 Document
权限矩阵，原因是：

- 它不经过 `kind:44300` Project View ordinary-object command；
- 它有独立安全设计、CLI 与 DB coordinator；
- 是否把 Document managed write也调整为 Community-only，需要单独确认和威胁分析。

实现时必须防止共享 helper 重构意外放宽 `buzz documents`。同时更新 v3 文档，避免继续用
“Project View v3 与 Document 使用完全相同写入 gate”描述已经拆开的边界。

## 7. 测试方案

### 7.1 纯领域测试

至少固定：

1. 无 Assignment candidate 接受自己的 Offer 成功；
2. 无 Assignment candidate 拒绝自己的 Offer 成功；
3. Proposal creator 无 Assignment 撤回成功；
4. 非 candidate 接受返回 `candidate_required`；
5. Leader Offer/Authorize 缺 Assignment 返回 `acting_assignment_required`；
6. Work、Checkpoint、Handoff 和 Commitment 操作缺 Assignment 仍拒绝；
7. 旧/他人 Assignment 仍拒绝；
8. Proposal 的目标/候选旧 Assignment compound fence变化时不产生部分提交。

### 7.2 PostgreSQL / Relay 纵向测试

schema v2 与 v3 分别覆盖：

1. owner-backed、尚未 direct-materialize、无 Assignment 的 managed Agent 接受 Offer；
2. 同一事务消费 Proposal、创建唯一 Assignment、同步 `relay_members` 等级、增加一次
   project revision；
3. 无 Assignment managed Agent 创建/更新/删除普通 Goal 或 Issue成功；
4. 同一 actor 被 ban、owner 被移除或 timeout 后普通写入失败；
5. ended Assignment 不能再做 role-bearing 写入，但 actor 仍合格时可以 Community-only CRUD；
6. managed assigned Agent 携正确 v3 Assignment + Runtime fence 的 role-bearing 写入成功；
7. 缺失、旧 epoch、错误 binding、错误 Assignment 均 fail closed；
8. candidate accept 不携 Runtime fence成功，随后 role-bearing 写入在 supervision 就绪前失败；
9. 同一 candidate accept event 重放只返回原 receipt，不重复创建 Assignment；
10. receipt replay 前发生 membership/ban 变化时仍拒绝；
11. v2/v3 普通 Role definition 不能绕过 active Assignment、admin level 和 tombstone 不变量。

### 7.3 CLI 测试

断言实际签名 command，而不只匹配 stdout：

1. managed candidate `accept_proposal` 生成 `acting_assignment_id=None`；
2. managed candidate 不读取不存在的 fence file；
3. managed 普通 Project View CRUD 不因没有 Assignment 本地失败；
4. managed Leader/role-bearing command 仍携带 verified Assignment 与 Runtime fence；
5. explicit stale Assignment 不会自动降级重试；
6. managed Document write仍要求 Assignment + Runtime fence；
7. v2 与 v3 command shape均保持 closed、互相 fail closed。

### 7.4 实际验收场景

```text
1. 启动无 Assignment 的 managed Agent A
2. Owner 创建普通 member Role R
3. Owner 向 A 创建 Offer P
4. A 读取 P 并接受
5. 确认 P consumed、A 获得唯一 active Assignment、Community role=member
6. 等待 supervisor 为新 Assignment 发布 exact Runtime fence
7. A 接受 responsible Work 或追加 Checkpoint成功
8. Owner 结束/替换 Assignment
9. A 使用旧 Assignment/Runtime 的角色写入失败
10. A owner 仍合格时执行普通 Project View CRUD成功
11. 移除/ban A 或 owner 后普通 CRUD也失败
```

步骤 9 与 10 必须同时成立，才能证明 Role fence 与 Community 权限已经真正拆开。

## 8. 兼容性与数据影响

- 不增加 Nostr kind；
- 不修改 schema v2/v3 command JSON 字段；
- 不需要数据库 migration；
- 不改已有 Project revision、projection generation 或历史 receipt；
- 不需要重新初始化 Project View；
- 不改变现有 Assignment、Proposal、Commitment、Checkpoint、Handoff 数据；
- 不改变 Role level 与 Community member/admin 的一致性约束；
- 旧客户端继续可以显式携带 Assignment；Relay 会验证而不是忽略；
- 行为变化只放宽原本已通过 Community gate、但被错误 Assignment gate 阻断的 ordinary/
  candidate managed Agent command。

## 9. 安全分析

### 9.1 预期能力变化

未分配或已结束 Assignment、但仍具有合法 Community 资格的 managed Agent，将能够执行普通
Project View CRUD。这是恢复原始 Community 授权模型，不是通过 Role 提权。

它仍不能：

- 以某个 Role 接受 Work；
- 提交 Role Checkpoint/Handoff；
- 使用 Leader authority；
- 使用已结束 Assignment 或旧 Runtime；
- 在 owner 失去资格、actor/owner 被 ban 或 actor timeout 后写入。

### 9.2 Stale Runtime 风险

Assignment fence只能撤销 Role identity，不能撤销稳定 Member identity。Assignment 结束后旧
进程若仍持有 Agent 私钥并满足 Community 资格，理论上仍可签名普通 Community write。这与
Human/外部 CLI Member 持有长期私钥的语义一致。

如果产品希望“Desktop 停止某个 managed Runtime 后撤销该进程的一切普通写入”，应另行设计
与 Assignment 无关的 Member Runtime fence，例如绑定：

```text
(Community, managed Member, runtime_id, epoch)
```

不能继续借 Assignment 实现这一目标，否则未分配候选者和普通 Community 权限会再次被
Role 生命周期绑死。

### 9.3 不允许的降级行为

- Role-bearing command 缺 fence时不能退化为 Community-only command；
- 显式携带 stale Assignment 时不能删除字段后重试；
- Runtime 校验失败时不能改用无 Assignment command重放相同角色意图；
- 客户端不能仅凭错误字符串决定降级；命令类别由 closed domain schema决定。

## 10. 文档修正

实现交付时同步：

1. 在 `../changelog.md` 记录事故、根因和修复；
2. 在 `..og.md` 记录 Community ACL 与 Role fence重新分层；
3. 修正 `..ntation-design.md` 中“managed Agent 普通对象写入必须
   有 Assignment”的旧结论，并保留历史变更说明；
4. 修正 v3/Document 实现文档中“Project View v3 与 Document 共用完全相同 managed write
   gate”的描述；
5. 保持 `docs/nips/NIP-PV.md` 中“member-signed mutation、Project Role 不授予 Buzz
   permission”的规范语义；
6. 不回写或改造 Document 的独立权限设计，除非后续形成单独决策。

## 11. 实施顺序

1. 先增加 v3 无 Assignment candidate accept 的失败回归测试；
2. 增加 v2/v3 无 Assignment ordinary object write 测试，冻结确认后的授权边界；
3. 从 v3 actor validator 提取 Community gate 与 optional Assignment/Runtime gate；
4. 修复 Role v3 candidate command路径；
5. 修复 v2/v3 ordinary object managed gate；
6. 调整 CLI candidate 与 ordinary Project View command组装；
7. 保持并验证 Document strict helper；
8. 验证 Assignment 激活后 supervisor provisioning和下一 turn Role Brief刷新；
9. 执行定向 Rust、CLI、Relay/PostgreSQL 与 Desktop/ACP 验收；
10. 更新 changelog和被本决策覆盖的实现文档；
11. 完成完整质量门后再构建、重启本地 Relay/Desktop/Agent进行人工验收。

建议实现提交拆为：

```text
test(project-view): capture candidate and community-write authorization boundary
fix(project-view): separate community authorization from assignment fencing
fix(cli): assemble project-view fences by operation intent
docs(project-view): reconcile community ACL and role fence semantics
```

## 12. 质量门

至少执行：

```bash
. ./bin/activate-hermit
cargo fmt --all -- --check
cargo test -p buzz-project-view
cargo test -p buzz-cli
cargo test -p buzz-db project_view -- --nocapture
cargo test -p buzz-relay project_view -- --nocapture
cargo clippy -p buzz-project-view -p buzz-cli -p buzz-db -p buzz-relay \
  --all-targets -- -D warnings
cargo test --manifest-path desktop/src-tauri/Cargo.toml project_view -- --nocapture
```

需要 PostgreSQL/Redis 的纵向用例按仓库现有 runbook 启动基础设施。最终交付前再运行与改动
风险相称的 `just ci`；构建验收完成后清理 Cargo 增量产物，但不删除仍需运行的 release
binary、容器或持久数据。

## 13. 验收标准

本修复完成必须同时满足：

1. 尚无 Assignment 的合法 managed Agent 能接受发给自己的 Offer；
2. 接受后原子产生唯一 active Assignment和正确 Community level；
3. 未分配合法 managed Agent能执行普通 Project View CRUD；
4. Project Role 不成为普通 CRUD 的权限来源；
5. Leader 与 role-bearing 动作仍必须携带精确 active Assignment；
6. managed Assignment-bearing v3 动作仍必须携带 exact current Runtime fence；
7. ended/stale Assignment不能继续产生 Role 身份写入；
8. Community remove、owner失权、ban和timeout仍能阻断普通写入；
9. Resource 的 Project View语义随普通对象修复，Project Document strict writer不被意外
   放宽；
10. v2 与 v3 对相同身份/操作给出相同授权结果；
11. 不需要数据迁移、Project View重新初始化或旧数据恢复；
12. 错误码能够区分 Community 未授权、candidate不匹配、Assignment缺失/陈旧和 Runtime
    fence失败。

## 14. 实现交付记录

本次按上述边界完成以下交付：

- 领域层新增共享的 closed `RoleActorIntent`，Role reducer 与 Agent-first CLI 不再各自维护
  candidate / governor / Role-bearing operation列表；actor-dependent 的 `reject_proposal`
  仍由 verified Proposal state决定最终 authority；
- schema v3 DB coordinator将 Community writer revalidation 与 optional Assignment/Runtime
  fence拆分。Role command先通过Community gate和纯领域replay authority，再验证命令实际
  声称的Assignment；普通对象command复用同一Community/optional-fence边界；
- schema v2普通对象路径删除“managed actor无Assignment即拒绝”的无条件分支；显式
  stale/他人Assignment仍返回冲突；
- CLI普通Project View create/update/delete和Context Reference replacement默认构造
  `acting_assignment_id=None, runtime_fence=None`；candidate identity command不读取Runtime，
  managed Leader/Role-bearing command仍必须读取verified当前Assignment与exact Runtime；
- `buzz documents`继续通过单独的strict helper强制managed Assignment + Runtime，未被本次
  普通Project View权限修复放宽；
- Project View、Role和Document changelog及实现设计已同步修正。

已执行并通过：

- `cargo fmt --all -- --check`；
- `cargo test -p buzz-project-view`：74项通过；
- `cargo test -p buzz-cli`：277项通过；
- `cargo test -p buzz-db`：101项非基础设施测试通过；
- PostgreSQL回归
  `managed_community_writer_does_not_require_role_assignment`：通过，覆盖无Assignment成功、
  显式stale Assignment拒绝、Agent ban拒绝和owner membership失效拒绝；
- `cargo test -p buzz-relay project_view`：8项通过；
- `cargo clippy -p buzz-project-view -p buzz-cli -p buzz-db -p buzz-relay --all-targets -- -D warnings`：通过；
- `cargo test --manifest-path desktop/src-tauri/Cargo.toml project_view`：22项通过。

本地运行交付使用 `../../../scripts/dev-rebuild-start.sh` 强制重建并重启。`localhost:3000` readiness
为 ready，NIP-11 正确广告 `buzz-project-view-v3`，Desktop与4个managed Agent harness均已
运行，启动日志未出现新错误；Docker容器与持久数据未删除。交付后清除了workspace与Tauri
Cargo incremental目录，保留当前debug二进制和依赖产物。
