# Project Role 治理授权与 admin Role 创建缺口修复设计

> 状态：已实现并通过定向验证
>
> 设计日期：2026-08-03；实现交付：2026-08-03
>
> 范围：Project View schema v2/v3、Role Continuity、Relay/DB、Agent-first CLI、Desktop/Tauri

## 1. 结论

本修复采用以下权威边界：

1. `Role` 定义及其等级、启停和删除属于 Project 治理对象，不再属于所有 Community
   member 都可执行的普通 Project View CRUD；
2. Community owner 是治理根，并且必须是可识别的 Human identity；只有 owner 可以创建或
   治理 `admin` Role；
3. non-owner 的 active Leader/admin 可以创建和治理 `member` Role，也可以向 `member`
   Role 发出或授权 Assignment Proposal；
4. 普通 member 或尚未分配的 Agent 不能创建、修改、删除、启停 Role，也不能发出或授权
   Offer；它仍可读取 Role、请求 Role，并处理属于自己的 Proposal；
5. 任意合格的 Community Human/Agent 都可以成为 admin Role 候选者，不需要预先成为
   “admin Agent”。admin 权限在 Assignment 激活时原子投影；
6. non-owner admin 的治理 authority 必须同时有 Community `admin` 和 exact active admin
   Assignment。Community `admin` 孤立记录或单独的 Assignment 都不能通过授权；
7. Supervisor binding、Runtime lease 和 Runtime fence 不授予 Role 治理权限，也不作为本次
   owner/Leader 授权的前置条件；显式携带 Runtime attribution 时仍严格验证；
8. Goal、Plan、Stage、Requirement、Issue、Work、Resource、Context Reference、Project
   Document 等非 Role 资产继续使用已经确认的 Community member 写入边界，本修复不能把
   Assignment 重新扩大为整个 Project 的 ACL。

一句话概括：

```text
Community member 可以维护项目内容
Role governor 才能维护项目的责任与授权结构
```

## 2. 问题表现

当前可以复现以下不一致：

1. 普通 member Agent 通过 Desktop 或 `buzz project-view create role` 成功创建一个 Role；
2. Project revision 增加，Role projection 已经存在；
3. 同一个 Agent 尝试向该 Role 发出 Offer；
4. Role Continuity 正确返回：

```text
403 restricted:project_view:authorization
```

5. Offer 没有产生 Proposal，Project revision 不再增加。

Offer 被拒绝本身是正确行为。真正的缺陷是第 1 步：Role 创建仍走普通 Project View 对象
mutation，只验证 Community writer，没有执行 Role governor 授权。因此系统允许普通 member
先创建一个自己无权治理的责任位置。

同时，当前还存在第二个缺口：

- schema v3 初始化可以生成 `admin` Role；
- 初始化完成后，Desktop 的 Add Role 没有 level 选项；
- v2/v3 普通 Role create 都固定落成 `member`；
- `buzz roles` 没有 Role definition/lifecycle 命令；
- 已设计的 `set_role_level`、`deactivate_role`、`reactivate_role` 没有完整实现。

因此当前 Community 只能保留初始化时生成的 admin Role，不能在后续正常扩展 Leader 结构。

## 3. 根因

### 3.1 Role definition 被混入 ordinary-object mutation

当前以下路径把 Role 与 Goal、Issue、Resource 等对象放在同一命令中：

- `crates/buzz-project-view/src/v2/project_object.rs`
- `crates/buzz-project-view/src/v3/project_object.rs`
- `crates/buzz-db/src/project_view_v2.rs::prepare_project_object_command()`
- `crates/buzz-db/src/project_view_v3.rs::prepare_project_object_command()`

schema v3 当前只执行：

```text
validate_v3_community_writer_in_tx
    -> validate_v3_optional_assignment_fence_in_tx
    -> ordinary object reducer
```

这能正确回答“actor 是否可以写当前 Community 的普通 Project 数据”，但不能回答：

```text
actor 是否是 owner 或 active Leader
target Role 是 member 还是 admin
本次是否涉及 Role level 或 lifecycle
```

schema v2 同样只验证可选 Assignment fence，没有对 Role object 分支执行 governor gate。

### 3.2 Role definition 与 Assignment 使用了两套授权路径

`offer_role`、`authorize_proposal`、Assignment replacement 等动作由 Role Continuity reducer
校验 owner/Leader authority。普通 member 因此会在 Offer 时被正确拒绝。

Role create/update/delete 却由 ordinary-object reducer处理。两个路径对同一个 Role 使用不同
授权规则，形成了“能创建但不能治理”的不一致。

### 3.3 已有 Role governance 领域规则没有接入写路径

`crates/buzz-project-view/src/v2.rs` 已经定义：

- `RoleLevel::Admin | Member`；
- `RoleGovernanceState`；
- `authorize_role_creation()`；
- `authorize_role_governance_transition()`。

但普通 Role create 默认写入 `RoleLevel::Member`，上述 helper 没有获得完整 actor governor
状态，也没有成为 v2/v3 ordinary-object commit 的必经门禁。

### 3.4 客户端把 Role 当成普通对象

Desktop 当前：

- 对所有 viewer 显示 Add Role；
- 全局 Add 类型列表始终包含 Role；
- Role 表单没有 level；
- Role Edit/Delete 只看对象生命周期，不看 actor authority；
- Role mutation走通用 `mutate_project_view`。

CLI 当前：

- `buzz project-view create/update/delete role` 使用普通对象命令；
- 这些命令默认不携带 active Leader Assignment；
- `buzz roles` 只有 Proposal、Assignment、Work、Checkpoint 和 Handoff，没有 Role 定义与
  lifecycle 命令。

UI 隐藏不是安全边界，但客户端结构放大了后端授权缺口。

## 4. 修复后的权限模型

### 4.1 权威 actor 分类

在 Community Project lock 内，将 actor 分类为：

```text
Owner
    当前唯一 relay_members.owner
    必须不是已登记的 managed Agent identity

ActiveLeader
    relay_members.role = admin
    + exact active Assignment 属于 signer
    + Assignment 指向 active admin Role
    + managed Agent 的 verified Human owner 仍合格

Member
    合格的直接成员，或 owner-backed managed Agent
    但不满足 Owner / ActiveLeader
```

授权不能只读取 Desktop 缓存，也不能只相信 `relay_members.admin`。DB 必须在同一事务内读取
canonical membership、Role level 和 active Assignment。

对于 non-owner Leader，命令必须携带 `acting_assignment_id`。它用于证明并归因当前 Leader
任期，而不是 Runtime supervisor 授权。缺少、已结束、属于他人或指向 member Role 的
Assignment 都应 fail closed。

### 4.2 Role definition 与 Assignment 矩阵

| 操作 | Owner Human | Active Leader/admin | 普通 member |
|---|---:|---:|---:|
| 创建 `member` Role | 允许 | 允许 | 拒绝 |
| 修改 `member` Role 定义/Context | 允许 | 允许 | 拒绝 |
| 停用、启用、删除 `member` Role | 允许 | 允许 | 拒绝 |
| 创建 `admin` Role | 允许 | 拒绝 | 拒绝 |
| 修改 `admin` Role 定义/Context | 允许 | 拒绝 | 拒绝 |
| admin/member 等级转换 | 允许 | 拒绝 | 拒绝 |
| 停用、启用、删除 `admin` Role | 允许 | 拒绝 | 拒绝 |
| 向 `member` Role 发 Offer/Authorize/Replace | 允许 | 允许 | 拒绝 |
| 向 `admin` Role 发 Offer/Authorize/Replace | 允许 | 拒绝 | 拒绝 |
| 读取 Role Directory | 允许 | 允许 | 允许 |
| 请求一个 Role | 允许 | 允许 | 允许 |
| 接受/拒绝发给自己的 Proposal | 允许 | 允许 | 允许 |
| 撤回自己创建的 Proposal | 允许 | 允许 | 允许 |

普通 member 的 candidate identity 操作不能因为本次收紧而要求 Assignment。Offer 的候选者
正是在成功接受后才获得新 Assignment。

### 4.3 admin Role 承接与 Community 等级

创建一个空缺 admin Role 不会产生任何 Community admin。只有 Proposal 完成并激活
Assignment 时才执行：

```text
eligible candidate accepts owner-authorized admin Offer
    -> consume Proposal
    -> create unique active admin Assignment
    -> upsert/promote relay_members.role = admin
    -> publish membership + Role projections
    -> advance Project revision exactly once
```

Assignment 结束时，非 owner 承担者降回 `member`；Community owner 始终保持 `owner`。

默认 Offer 候选者应当没有 active Assignment。当前协议已有显式 compound replacement
能力时，可以在 Proposal 中精确记录并替换 candidate 的旧 Assignment；不能把隐式覆盖当成
“未分配”。

### 4.4 managed Agent 创建边界

本修复允许 Active Leader 向现有、合格的 Agent 委派 member Role，但不允许形成：

```text
managed Agent -> owns -> another managed Agent
```

managed Agent 的密钥、Desktop runtime 和 `agent_owner_pubkey` 仍由 verified Human/Desktop
控制。Active Leader 可以表达人员需求、邀请合格 identity 或发出 member Role Offer，但不能
成为子 Agent 的治理 owner。

## 5. 协议与领域实现

### 5.1 保留 closed object command，增加 create-only Role level

本次不新建另一套 mutation kind，也不复制 Project object reducer。继续使用 v2/v3 现有
`ProjectObjectCommand` / `ProjectObjectCommandV3`，但增加一个只在 Role create 时合法的签名
字段：

```text
initial_role_level?: member | admin
```

规则：

- `Create(Role)` 缺省该字段时按 `member` 处理，兼容现有已签名客户端；
- 新版 Desktop/CLI 创建 Role 时总是显式发送 level；
- `initial_role_level=admin` 只能由 owner 提交；
- 非 Role create、Role update/delete 携带该字段一律作为 invalid command拒绝；
- level不能进入 Role patch，不能通过 update静默提权；
- reducer把签名请求中的初始level写入canonical `role_levels`，不再无条件插入
  `RoleLevel::Member`；
- v3 create仍支持Role完整Context Reference，v2不接受v3-only字段；
- schema v1保持legacy，不读取该治理字段。

字段放在schema-v2/v3 command envelope而不是v1共享Role body中，可以避免改变schema-v1
对象语义，并与数据库中独立的 `role_level` canonical列保持一致。

初始化后的 `set_role_level` 仍必须使用未来的专用治理命令，不能借本次 create 字段扩展为
通用 patch。本次首先完成直接创建 admin Role和Role definition全生命周期授权收敛；等级
转换单列为后续能力，不阻塞当前缺陷修复。

### 5.2 generic Role mutation 本身成为受治理入口

安全门不能依赖所有客户端同时升级。Relay 对schema v2/v3的generic Role create/update/
delete识别Role target，并在receipt lookup之前执行governor gate：

- create从签名的 `initial_role_level` 取得目标level；
- update/delete从locked canonical object取得当前level；
- member Role由owner/ActiveLeader治理；
- admin Role只由owner治理；
- Role update中的`active`变化也受同一矩阵及现有lifecycle不变量保护；
- 普通member返回403且不能产生object、change或receipt；
- 非Role普通对象仍走Community writer路径，不要求Leader Assignment。

第一方Desktop可以提供Role专用的UI/API helper，`buzz roles`也可以提供更易发现的命令，但
wire层继续复用closed Project object command、object reducer、Context proof、projection plan
和receipt pipeline。这样可以用较小改动关闭安全旁路，也避免维护第二套Role对象校验。

### 5.3 扩展纯领域授权模型

涉及：

- `crates/buzz-project-view/src/v2.rs`
- `crates/buzz-project-view/src/v2/role_continuity.rs`
- `crates/buzz-project-view/src/v2/project_object.rs`
- `crates/buzz-project-view/src/v3/project_object.rs`

将当前只区分“是否 owner”的 helper 扩展为 closed actor authority：

```rust
enum RoleGovernorAuthority {
    Owner,
    ActiveLeader { assignment_id: Uuid },
    Member,
}
```

纯函数根据请求 target/current/next level执行矩阵：

```text
member Role lifecycle -> Owner | ActiveLeader
admin Role lifecycle  -> Owner
任何涉及 admin 的 level transition -> Owner
Member -> stable governance error
```

Role Continuity继续作为Offer/Authorize/Replace的事实源。ordinary object reducer继续负责
Role body/Context不变量；definition授权gate与continuity reducer必须共享Role level/actor
authority语义，不能各自维护漂移的条件列表。

### 5.4 Role lifecycle 不变量

授权通过后仍必须保留：

- active Assignment 存在时不能普通删除或停用 Role；
- open Proposal 存在时不能删除或停用 Role；
- 未完成 responsible Work/active Commitment 不能被静默遗弃；
- 已有 Assignment 历史的 Role 默认只允许 tombstone-safe 的治理流程，不物理删除历史；
- 本次不开放level update；未来 `set_role_level` 若Role有active Assignment，必须在同一
  事务同步承担者Community `admin/member`；
- Role ID、object revision、project revision、projection generation 和 source provenance
  继续遵循现有单头/单 revision 规则；
- Project View 不级联删除 Document、Resource、Work 或其他引用对象。

## 6. DB/Relay 实现

### 6.1 事务内 authority resolver

在 `buzz-db` 增加 v2/v3 共用的 transaction-local resolver，职责为：

1. 取得 Community Project advisory lock；
2. 重新验证 direct member 或 eligible owner-backed managed Agent；
3. 检查 actor/managed owner ban、timeout 和 membership；
4. 查找当前唯一 Community owner；
5. 对 non-owner signer，使用 signed `acting_assignment_id` 联结：

```text
project_role_assignments
    -> project_view_objects(role)
    -> role_level = admin
    -> role active
    -> member_pubkey = signer
    -> ended_at IS NULL
```

6. 同时确认 `relay_members.role = admin`；
7. 返回 `Owner | ActiveLeader | Member`，或稳定错误。

仅有 `relay_members.admin` 但没有 active admin Assignment，或仅有 Assignment 但 membership
没有同步，都视为 canonical consistency failure并 fail closed，不能降级为普通 admin。

### 6.2 prepare 顺序

v2/v3 Role definition prepare统一为：

```text
Community writer revalidation
    -> load canonical Role/object/continuity state under Project lock
    -> classify request and target/current/next Role level
    -> resolve owner/ActiveLeader authority
    -> validate optional explicit Runtime attribution
    -> receipt lookup
    -> pure Role governance + object reduce
    -> lifecycle/reference/Context proof
    -> persist object/role-level/membership changes
    -> projection + membership snapshot + receipt
```

authority 必须在 receipt lookup 之前重验。过去成功的事件不能在 signer 被降级、Assignment
结束、owner 关系失效或被 ban 后利用 receipt replay继续获得治理结果。

### 6.3 初始 level 的 canonical commit

Role create通过授权后，事务必须把签名的初始level同时用于：

- pure reducer中的 `role_levels`；
- `project_view_objects.role_level`；
- Role projection / Role Directory；
- receipt result和后续verified snapshot。

空缺admin Role不会修改任何member的Community等级，因此create不触发membership更新。
后续实现专用 `set_role_level` 时，如果Role已有active Assignment，必须在一个事务内同步
承担者membership；该能力不通过本次create-only字段伪装实现。

### 6.4 Offer/Authorize 路径

现有 Role Continuity 对 Offer 的核心授权方向正确，应保留并补测试：

- owner 可治理 admin/member Role；
- ActiveLeader 只能治理 member Role；
- member 返回 403 且不产生 Proposal；
- candidate 接受自己的 owner-authorized admin Offer不要求预先 Assignment；
- acceptance 原子创建 Assignment并投影 membership；
- supervisor binding缺失不阻断业务授权或 acceptance。

需要修正的不是把 Offer 放宽给 Role creator，而是禁止无 governor authority 的 actor 创建
Role。`created_by` 是审计事实，不是未来 Offer 权限。

### 6.5 错误语义

建议固定：

| 条件 | 错误 |
|---|---|
| 普通 member 写 Role definition/lifecycle | `restricted:project_view:authorization` |
| ActiveLeader 尝试 admin Role 生命周期 | `restricted:project_view:owner_required` |
| non-owner admin 没有 signed Assignment | `restricted:project_view:acting_assignment_required` |
| Assignment stale/错误/属于他人 | `conflict:project_view:acting_assignment` |
| Role 有 active Assignment/open Proposal | 现有稳定 lifecycle conflict/invalid 错误 |
| Community principal 不合格 | 现有 membership/authorization 错误 |

现有 `offer_role` 的 `restricted:project_view:authorization` 可以保持 wire 兼容；客户端不得靠
错误字符串删除 fence或降级重试。

## 7. Desktop/Tauri

### 7.1 单一 capability 派生

从同一份 verified Role Continuity snapshot和当前 Community membership派生：

```text
isOwner
isActiveLeader
actingAssignmentId
canCreateMemberRole
canCreateAdminRole
canGovernMemberRole
canGovernAdminRole
```

`isActiveLeader` 必须同时要求 Community `admin` 和 active admin Assignment。仅检查 Role
level 或仅检查 membership 都不够。数据不完整时 Role 治理 UI fail closed，但不影响普通
Project View/Document/Resource UI。

### 7.2 UI 行为

普通 member：

- 隐藏或禁用 Supporting Roles 的 Add Role；
- 全局 Add 类型列表移除 Role；
- Role Inspector 保留查看和 Request Role；
- 隐藏 Edit/Delete/Offer/End 等治理按钮；
- 仍可创建和编辑非 Role Project View 对象。

ActiveLeader：admin

- 可以创建 member Role；
- level固定为 `member`，admin选项不可选并说明 owner-only；
- 可以编辑、停用、删除和 offer member Role；
- admin Role只读；
- mutation携带 exact `acting_assignment_id`，不要求 supervisor binding。

Owner Human：

- Add Role 可以选择 `member | admin`；
- 可以治理两级 Role；
- 直接以 owner authority提交，不要求 Assignment。

Role 表单中现有“Active role 只是 semantic state，不是 Buzz authorization”文案已经过时。
启停必须迁到专用治理动作，并明确 admin Role/Assignment 会影响 Community 等级。

### 7.3 Tauri mutation 组装

在现有Project View mutation基础上增加Role专用的typed helper，负责：

- create时写入显式 `initial_role_level`；
- ActiveLeader治理member Role时附加exact `acting_assignment_id`；
- owner路径不要求Assignment；
- update/delete前保留target Role level与capability检查；
- 继续使用现有canonical receipt/object回读，成功后选择新Role。

可以暴露独立的 `mutate_project_view_role` Tauri command，也可以在现有command内部按
`object_type=role` 分流；无论采用哪种文件组织，最终签名wire command必须是同一closed
Project object协议，Relay gate才是安全事实源。

涉及文件预计包括：

- `desktop/src-tauri/src/commands/project_view_role.rs`
- `desktop/src-tauri/src/commands/project_view_role_receipt.rs`
- `desktop/src/features/project-view/ui/ProjectViewObjectDialog*.tsx`
- `desktop/src/features/project-view/ui/ProjectViewScreen.tsx`
- `desktop/src/features/project-view/ui/ProjectViewInspector.tsx`
- `desktop/src/features/project-view/ui/ProjectRoleInspector.tsx`
- `desktop/src/shared/api/tauriProjectViewRoleMutation.ts`
- `desktop/src/testing/e2eBridge.ts`

最终文件拆分以现有模块边界为准，避免继续扩大通用 Object Dialog 中的 Role 特例。

## 8. Agent-first CLI 与 ACP

`buzz roles` 建议增加更易发现的定义入口：

```text
buzz roles create --level member|admin ...
buzz roles update --role <uuid> ...
buzz roles deactivate --role <uuid>
buzz roles reactivate --role <uuid>
buzz roles delete --role <uuid>
```

规则：

- owner command可以省略 Assignment；
- managed/Human ActiveLeader治理 member Role时附带 verified current admin Assignment；
- 不从 supervisor binding推导 authority，不因没有 Runtime lease本地拒绝；
- 若显式附带 runtime fence，Relay继续验证；
- 普通 member在本地可以得到清晰提示，但最终仍提交给 Relay的路径必须有服务端门禁；
- 上述命令内部仍构造受治理的Project object command，而不是第二套wire协议；
- `buzz project-view create/update/delete role` 对schema v2/v3必须使用同一个Role-aware
  assembler：create显式写level，ActiveLeader自动附带Assignment；
- 其他对象的 `buzz project-view` 行为保持不变。

`set-level` 不在本次交付范围；在专用领域命令实现前，CLI不得通过JSON patch提供该能力。

Role Brief/ACP 在每 turn读取的 admin/member Assignment语义不变。本修复不新增固定 system
prompt，也不要求重新生成 Agent identity。

## 9. 数据、兼容与迁移

本修复：

- 不需要重新初始化 Project View；
- 不恢复或删除旧数据；
- 不修改已有 Project revision、receipt、Role、Proposal、Assignment或Document；
- 不需要为权限策略本身新增数据库表；
- 新的create-only字段继续使用现有mutation/change/receipt结构，不需要新增表或直接改库；
- 既有由普通 member创建的 member Role全部保留，从发布后开始只能由 owner/ActiveLeader
  继续治理；
- 初始化生成的 admin Role继续有效；
- open Proposal在最终 authorize/accept时重验当前 authority，不信任创建时缓存；
- 当前 Community admin/Assignment投影不批量重写；启动/测试可增加一致性审计，发现分裂
  时 fail closed并报告。

旧客户端提交 generic Role mutation时，后端仍执行同一 governor gate，因此不会成为降级
绕过。第一方客户端必须与 Relay协调发布，确保 ActiveLeader能够携带 Assignment。

## 10. 安全边界

### 10.1 本次能力变化

普通 member失去的是：

- Role definition/lifecycle写入；
- Role Assignment治理。

它没有失去：

- 普通 Project View数据写入；
- Document/Resource/Context使用；
- Role读取和申请；
- 自己的 candidate/creator Proposal动作。

### 10.2 不允许的旁路

- 不能通过 generic Project View update/delete绕过 Role governor gate；
- 不能通过 patch修改 `level` 或 `active`；
- 不能只凭 `relay_members.admin` 绕过 active admin Assignment；
- 不能只凭 stale Assignment绕过 membership降级；
- 不能把 Role creator身份当成 Offer authority；
- 不能因为 supervisor binding存在或缺失而授予/撤销 Role治理权；
- 不能让 managed Agent递归成为另一个 managed Agent的 owner；
- 不能在收到 403后删除 Assignment字段并以普通 Project mutation重试同一治理意图。

## 11. 测试方案

### 11.1 纯领域测试

至少覆盖：

1. owner创建 member/admin Role成功；
2. ActiveLeader创建 member Role成功；
3. ActiveLeader创建或修改 admin Role返回 owner-required；
4. member创建、更新、删除、启停任意Role返回authorization；
5. member Role与admin Role的全部 transition矩阵；
6. generic patch不能修改level，active变化必须经过同一governor与lifecycle gate；
7. candidate无 Assignment接受自己的 admin Offer成功；
8. Role creator但非 governor发 Offer仍失败。

### 11.2 PostgreSQL/Relay 纵向测试

schema v2/v3分别覆盖：

1. member generic Role create返回 403，Project revision、object count和receipt均不变；
2. owner/ActiveLeader创建 member Role成功并只增加一次 revision；
3. 只有 owner可以创建 admin Role；
4. ActiveLeader command携 exact Assignment成功；缺失、stale、他人或 member Assignment失败；
5. `relay_members.admin` 与 active admin Assignment任一缺失时fail closed；
6. supervisor binding/lease缺失不阻断合法 Leader创建 member Role或发 Offer；
7. admin Role候选者接受后 Assignment与membership在同一事务变成admin；
8. Assignment结束后该actor不能继续治理 Role，但仍可按member权限写普通对象；
9. admin/member初始level正确进入canonical row、projection和verified snapshot；
10. open Proposal、active Assignment、responsible Work继续阻止非法停用/删除；
11. receipt replay前actor被降级、ban或owner关系失效时仍拒绝；
12. Goal/Plan/Issue/Work/Resource/Context普通member CRUD继续成功；
13. Document权限和当前Community writer边界不被意外改变。

### 11.3 Desktop/Tauri

覆盖：

- member看不到Role Add/Edit/Delete/Offer，但仍能Request并创建普通对象；
- ActiveLeader只能创建member Role，payload携 exact Assignment；
- owner可以选择admin并且不要求Assignment；
- ActiveLeader查看admin Role时治理按钮不可用；
- membership=admin但无admin Assignment时UI fail closed；
- admin Assignment存在但membership不同步时UI fail closed；
- Relay 403/409仍以稳定错误展示，不乐观写入本地状态；
- E2E mock bridge和serializer精确覆盖snake_case、initial_role_level、role_id、receipt。

### 11.4 CLI

覆盖：

- clap参数和level枚举；
- owner command无Assignment；
- ActiveLeader自动选择exact current Assignment；
- member本地提示与Relay最终403；
- legacy `project-view ... role`不能发送无治理上下文的mutation；
- admin Role与member Role命令不可降级重试，JSON patch不能修改level；
- v2/v3 command shape互相fail closed。

## 12. 实施顺序

1. 先增加“member创建Role成功”的失败回归测试，并断言拒绝不增加revision；
2. 增加 owner/ActiveLeader/member 与 target level的纯授权矩阵；
3. 给v2/v3 command增加create-only `initial_role_level`并固定wire validation；
4. 实现transaction-local Role governor resolver；
5. 在v2/v3 generic Role路径接入同一门禁，先关闭安全旁路；
6. 让create reducer/DB/projection使用签名初始level，保留现有update/delete lifecycle约束；
7. 强化receipt replay前的Role definition及Offer/Authorize当前authority重验；
8. 调整`buzz roles`与`buzz project-view ... role`的Role-aware assembler；
9. 实现Tauri Role mutation helper和Desktop capability/UI；
10. 完成DB/Relay/CLI/Tauri/Desktop纵向测试；
11. 更新Role、Project View changelog和被本决策覆盖的旧设计表述；
12. 清理增量构建产物后重新构建并启动Relay/Desktop进行人工验收。

建议提交拆分：

```text
test(role): capture role-definition governance gap
fix(role): enforce owner and active-leader definition authority
feat(role): add post-init admin role creation
fix(cli): assemble role-aware project-view mutations
fix(desktop): gate role management by verified governance capability
docs(role): reconcile role governance with community-write boundary
```

## 13. 质量门

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
cd desktop && pnpm test
cd desktop && pnpm lint
```

需要PostgreSQL/Redis的纵向用例按仓库runbook启动基础设施。最终交付前运行与改动风险相称的
`just ci`；人工验收结束后清理Cargo incremental产物，不删除运行所需二进制、Docker容器或
持久数据。

## 14. 验收标准

本修复完成必须同时满足：

1. 普通member不能通过Desktop、CLI、raw event或legacy generic command创建/修改/删除Role；
2. owner可在初始化后创建admin/member Role；
3. ActiveLeader可创建和治理member Role，不能治理admin Role；
4. non-owner admin治理必须有Community admin + exact active admin Assignment；
5. Role Offer权限与Role definition权限一致，不再出现“member可创建但不能发Offer”；
6. 任意合格、未分配候选者可接受owner发出的admin Offer；
7. acceptance原子产生admin Assignment和Community admin projection；
8. supervisor binding不参与业务权限判定；
9. Role生命周期不变量、revision、projection、receipt和membership snapshot保持一致；
10. 普通Project View、Document、Resource和Context写权限没有被意外收紧；
11. 既有Role/Assignment无需迁移或重初始化；
12. Desktop按钮状态与Relay最终授权一致，但Relay仍是唯一安全边界。

## 15. 对既有设计的覆盖关系

本设计局部覆盖：

1. `docs/lora/stage/bug/project-view-assignment-authorization-boundary-fix-design.md`
   中把 `Role definition CRUD` 列入所有Community member ordinary write的结论；
2. `docs/lora/stage/role/implementation-design.md` 权限矩阵中“普通Role沿用Project View
   member能力”的条目；
3. Project View v0设计中将Role完全视为无权限普通对象的部分。

以下结论继续有效：

- Assignment不是整个Project的ACL；
- Community member仍可维护非Role Project数据；
- candidate无需预先Assignment即可处理自己的Proposal；
- Role level与Community admin/member保持原子映射；
- owner是Community治理根，不是Role level；
- Runtime supervisor与Role业务authority解耦；
- Project Document和Resource仍有各自已经确认的资产语义。

实现交付时应在旧文档相关位置增加supersession注记，而不是删除历史，以便解释权限模型为何
从“所有member可写Role definition”收敛为“Role是治理对象”。

## 16. 不在本次范围

- 不引入领域级admin权限；首版admin仍是Community级能力；
- 不允许Agent创建或拥有递归managed Agent；
- 不改变Community owner transfer规则；
- 不让Role控制普通Project对象、Document或Resource的ACL；
- 不设计多个同时active Assignment；仍保持一个Member最多一个active Assignment；
- 不在本次实现初始化后的Role等级转换；后续必须通过专用`set_role_level`命令交付；
- 不恢复旧Project View数据；
- 不以直接SQL修改Role level、Assignment或membership；
- 不重新引入Supervisor binding作为操作权限。

## 17. 实现交付记录

本次按上述边界完成以下纵向交付：

1. `buzz-project-view` 为 v2/v3 ordinary-object command 增加 create-only
   `initial_role_level`，并冻结 owner / Active Leader / member 纯授权矩阵；
2. `buzz-db` 在 v2/v3 locked write transaction 中识别 Role create/update/delete，先执行
   transaction-local governor gate，再验证显式 Assignment/Runtime attribution并查 receipt；
3. Role Continuity receipt replay 对 Offer、Authorize、governor Reject、End Assignment 和
   Work responsibility 重验当前 authority；
4. `buzz-cli` 的 legacy `project-view ... role` 路径升级为 Role-aware assembler，并开放
   `--role-level admin|member`；
5. Desktop/Tauri 增加 Role level、exact Leader Assignment wire 字段和 fail-closed capability
   UI；普通 member 仍可 Request Role；
6. 未增加 migration，既有 Role、Assignment、Document、Resource 与 Project revision 无需
   重建或迁移。

定向验证结果：

- `cargo test -p buzz-project-view`：44 个 crate 单元测试，以及属性、关系、wire 套件通过；
- `cargo test -p buzz-cli`：278 个测试通过；
- 两个需要 PostgreSQL 的 Role governor transaction 用例显式执行通过；
- `cargo check -p buzz-db -p buzz-relay` 通过；
- Desktop Tauri 14 个 Project View mutation 测试、3536 个前端单元测试和 TypeScript 检查
  通过；
- Desktop E2E build 与 member/owner/Active Leader 三个目标 Playwright 场景通过。
