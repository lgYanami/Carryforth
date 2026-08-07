# Project Runtime Supervisor Binding 与 Role 授权解耦修复设计

> 状态：实现完成；自动化验收与本地未监督纵向验收通过
>
> 记录日期：2026-08-03
>
> 范围：Project View schema v2/v3、Role Continuity、Runtime supervision、
> `buzz-cli`、`buzz-admin`、managed ACP 与 Desktop

## 1. 结论

本次修复保留 Runtime supervisor 子系统，但重新划定它和业务授权之间的边界：

1. Community 身份决定 Project View、Document、Resource 等普通项目资产的基础读写资格；
2. active Assignment 决定 Member 是否可以代表某个 Role 执行 Checkpoint、Handoff、Work
   Commitment、Leader 治理等 Role-bearing 行为；
3. supervisor binding、Runtime lease 与 Runtime fence 不再授予或撤销上述业务权限；
4. Runtime fence 改为**可选、显式的运行来源归因**：命令不携带 fence 时不进入 Runtime
   校验；一旦显式携带，Relay 必须严格验证，不能忽略错误后降级；
5. supervisor binding 继续承担 Runtime evidence、lease、恢复、自动
   `unrecoverable` 与 Project View maintenance 协调；
6. binding 缺失、私钥缺失、身份不匹配或 lease 过期时，ACP 必须降级为未监督运行，不能
   终止 Agent、污染 Role Brief 或阻止普通 Role 操作；
7. Relay、ACP 与 Desktop 必须共同呈现一套可查询、可解释、可修复的 supervision 状态；
8. binding 的注册、撤销和替换继续属于 Relay operator 控制面。Desktop/Agent 不得因为
   自己是 Community member 就静默创建或接管 binding。

这是一项同时修复**授权耦合**和**Supervisor 控制面缺口**的纵向变更。只删除
`runtime_unavailable` 报错而不补状态与 provisioning，或者只补 binding 而继续用它阻断
Role command，都不算完成。

本设计在 Runtime fence 是否为普通 Role 写入必需条件这一点上，取代
[Project View Community 授权与 Assignment Fence 边界修复设计](project-view-assignment-authorization-boundary-fix-design.md)
中“managed Assignment-bearing 写入必须有 Runtime fence”的旧结论。Assignment、candidate、
owner/Leader 与 Community 授权边界保持不变。

## 2. 当前问题

### 2.1 普通 Role 操作被 Supervisor 配置阻断

当前 `buzz roles` 对 managed Agent 的 Assignment-bearing command 自动执行：

```text
读取 verified Role state
    -> 找到 active Assignment
    -> 从动态文件或环境读取 Runtime fence
    -> fence 不存在时返回 runtime_unavailable
    -> 命令未签名、未发送到 Relay
```

即使绕过 CLI，DB writer 仍会根据 schema 和 binding 状态强制要求 exact Runtime lease。结果是
一个 Community 合格、Assignment 有效的 Agent，仅因为本地没有 supervisor 私钥或 Relay
没有 binding，就不能追加 Checkpoint。

这把两个独立问题混在了一起：

- “该 Agent 是否是当前 Role assignee”；
- “本次命令是否来自 Relay 当前认可的受监督 Runtime epoch”。

首版产品只需要前者作为 Role authority。后者应是可选的运行归因和治理能力。

### 2.2 binding 存在会隐式切换授权模式

`RuntimeCommandFencePolicy::LegacyOptionalSupervision` 的现有含义并非真正可选：

- Assignment 没有 active binding：不要求 Runtime fence；
- Assignment 一旦存在 active binding：没有 fence 就拒绝。

因此 operator 在控制面注册一个 binding，会在没有协议协商、没有 Desktop 提示的情况下，
改变 Agent 的业务写入条件。binding 从运行治理配置变成了隐式权限开关。

`RequireSupervisedRuntime` 更严格：只要 managed command 携带 Assignment，就同时要求 binding
和当前 lease。schema v3 的 Role/Object 路径使用该策略后，进一步扩大了阻断范围。

### 2.3 ACP 把部分配置当成整个 Agent 的启动失败

`RuntimeSupervisor::prepare()` 当前行为是：

1. 读取 Assignment Runtime status；
2. 如果 Relay 表示 Assignment 已有 binding，则要求 ACP 启动前已经收到
   `BUZZ_RUNTIME_SUPERVISOR_PRIVATE_KEY`；
3. 未配置私钥时返回错误；
4. startup 或 turn-boundary reconciliation 把错误提升为
   `runtime_supervision_unavailable`；
5. Role Brief、Agent pool 或后续 turn 因此 fail closed。

Supervisor 故障由此扩大成整个 Agent 故障。

### 2.4 Relay 状态不足以诊断 binding

当前 `AssignmentRuntimeStatus` 主要返回：

- `managed`；
- aggregate availability；
- Runtime ID、epoch、lease 与 recovery 状态。

它没有返回当前 active binding 的：

- `binding_id`；
- `supervisor_pubkey`；
- policy；
- registration time。

因此 ACP/Desktop 无法在提交 evidence 前区分：

- 没有 binding；
- 有 binding，但本地没有私钥；
- 有 binding，但绑定的是另一个 supervisor；
- 身份匹配，只是还没有建立 lease；
- lease 已经过期或恢复已耗尽。

### 2.5 Desktop 只有环境变量注入，没有产品状态

Desktop 当前只在父进程环境存在 `BUZZ_RUNTIME_SUPERVISOR_PRIVATE_KEY` 时将其传给 trusted
ACP harness，并为 Agent/Relay pair 派生本地状态文件。Desktop 没有：

- 独立 supervisor identity 的持久管理；
- Relay binding 查询；
- 本地 key 与 binding 公钥匹配检查；
- supervision badge、错误原因和修复建议；
- operator 可用的幂等管理入口。

用户只能在真正执行 Role command、ACP 启动或 maintenance 时看到间接错误。

## 3. 权威边界

### 3.1 四层独立判断

普通 Project/Role command 按以下顺序判断：

```text
1. Community admission
   signer 是否是合格 Human，或 owner 仍合格的 verified managed Agent

2. Operation authority
   candidate、owner、Leader、assignee 等领域关系是否允许该动作

3. Assignment authority（仅 Role-bearing / Leader 行为）
   acting_assignment_id 是否 active、属于 signer，并匹配当前对象/任期

4. Optional Runtime attribution（仅命令显式携带 fence）
   binding、supervisor、runtime ID、epoch 与 lease 是否精确有效
```

任何一层都不能代替另一层：

- Runtime fence 不能让非成员获得 Community 权限；
- binding 不能让没有 Assignment 的 Agent 代表 Role 行动；
- Assignment 仍不能绕过 ban、timeout、Community remove 或 owner 失权；
- 没有 Runtime fence 不再撤销 active assignee 的 Role authority。

### 3.2 操作矩阵

| 操作 | Community | Assignment | Runtime supervisor |
|---|---:|---:|---:|
| Project View / Document / Resource 普通读写 | 必须 | 不需要 | 不参与 |
| Candidate 申请、接受/拒绝自己的 Proposal | 必须 | 不需要 | 不参与 |
| owner 直接治理 | owner | 不需要 | 不参与 |
| Leader 治理 | admin + active Leader | 必须 | 不参与授权 |
| Commitment、Checkpoint、Handoff、replacement/unable | 必须 | 必须 | 不参与授权 |
| 显式 Runtime-attributed command | 依操作而定 | 必须 | exact fence 必须有效 |
| Runtime evidence / lease / recovery | 不作为业务 command | exact binding | 必须 |
| 自动 `unrecoverable` | system policy | active Assignment | 必须 |
| maintenance drain/freeze/ack | operator maintenance | baseline Assignment | 必须 |

### 3.3 接受的安全取舍

解耦后，同一逻辑 Agent 的旧 ACP/Codex 进程只要仍持有 Agent 私钥，且 Assignment 仍然
active，就可能继续提交 Role-bearing command。首版接受这一点：

- active Assignment 是 Role authority 的硬撤销坐标；
- Community remove、owner 失权、ban/timeout 是 Community authority 的硬撤销坐标；
- revision CAS、append-only history 与 command receipt 继续处理并发和审计；
- supervisor 不再承诺“同一 Assignment 只有一个可写进程”。

未来若重新引入 strict single-runtime write fencing，必须由 Relay 显式广告一个独立、版本化
且可配置的 capability，并先完成 provisioning/readiness；不能再次根据数据库中是否碰巧存在
binding 隐式启用。

## 4. 修复后的 Supervision 状态模型

### 4.1 状态定义

| 状态 | Canonical / local 条件 | 对普通 Agent 的影响 |
|---|---|---|
| `not_applicable` | 没有 active Assignment | 无影响 |
| `disabled` | Assignment 没有 active binding，本地也未准备身份 | 正常未监督运行 |
| `awaiting_binding` | Desktop 已有 supervisor identity，但 Relay 没有 binding | 正常未监督运行 |
| `starting` | binding 与本地身份匹配，正在提交 start/recovery evidence | 正常运行，不发布 fence |
| `active` | binding 匹配，存在当前 available lease | 正常运行，可发布 fence |
| `recovering` | 进入受信 recovery episode | 正常业务不被授权层阻断；Supervisor 功能恢复中 |
| `degraded_missing_key` | Relay 有 binding，Desktop/ACP 没有对应私钥 | 正常未监督运行 |
| `degraded_mismatch` | Relay binding 公钥与本地 supervisor 公钥不同 | 正常未监督运行 |
| `expired` | 当前 lease 已过期，尚未建立新 epoch | 正常未监督运行 |
| `unavailable` | recovery policy 已耗尽 | 正常业务由 Assignment 决定；自动恢复不可用 |
| `unknown` | 当前 status/evidence 请求无法可靠完成 | 正常运行，清除本地 fence |

`disabled` 是合法配置，不是错误。`degraded_*` 表示用户希望或 Relay 声称存在监督，但实际
控制面不完整。

### 4.2 三方事实源

```text
Relay canonical status
    binding / policy / lease / evidence
              +
Desktop local secret status
    key present / keyring locked / configured pubkey
              +
ACP live reconciliation
    matching / active runtime / last failure / fence publication
              ↓
Composite supervision state
```

- Relay 是 binding、lease 和 evidence 的唯一 canonical source；
- Desktop 是本地 supervisor 私钥是否可用的事实源；
- ACP 是某个实际 harness 是否已经建立并续租 Runtime 的事实源；
- Agent 模型只获得简化状态和能力说明，不获得私钥、本地状态路径或可伪造 evidence 的能力。

## 5. 协议与读模型调整

### 5.1 Role command wire 保持兼容

涉及：

- `crates/buzz-project-view/src/v2/role_continuity.rs`
- `crates/buzz-project-view/src/v3/role_continuity.rs`
- `crates/buzz-project-view/src/v2/project_object.rs`
- `crates/buzz-project-view/src/v3/project_object.rs`

保持现有可选字段：

```text
acting_assignment_id: Option<Uuid>
runtime_fence: Option<RuntimeFence>
```

closed wire 规则为：

- `runtime_fence=Some` 必须同时有 `acting_assignment_id=Some`；
- `acting_assignment_id=Some, runtime_fence=None` 是合法的 Assignment-attributed command；
- Role-bearing/Leader request 是否必须有 Assignment 继续由共享领域 intent 决定；
- wire/schema 不再声明 managed Assignment 必然需要 Runtime fence。

不增加 Project View schema v4，也不修改已签名历史事件。

### 5.2 Runtime status 增加 binding 视图

涉及：

- `crates/buzz-project-view/src/v2/runtime_supervision.rs`
- `crates/buzz-db/src/project_runtime.rs`
- `crates/buzz-relay/src/api/project_runtime.rs`

在 `AssignmentRuntimeStatus` 中增加可选的公开 binding read model：

```text
binding: Option<RuntimeSupervisorBindingStatus>
    binding_id
    supervisor_pubkey
    registered_at
    policy
```

保留现有 `managed`、`availability` 和 `runtimes` 字段以兼容当前调用方；后续可以把含义模糊的
`managed` 标记为 deprecated，但本次不做破坏性删除。

状态接口仍要求当前 Project View authorized member。返回值不包含：

- supervisor 私钥；
- Desktop keyring 状态；
- operator 私钥或 Relay 私钥；
- 本地恢复文件路径。

## 6. 后端实现

### 6.1 新增显式归因策略

涉及：

- `crates/buzz-db/src/project_runtime.rs`

为 `RuntimeCommandFencePolicy` 增加语义明确的策略，例如：

```rust
ValidateExplicitRuntimeAttribution
```

其逻辑必须固定为：

```text
(assignment=None, fence=None)
    -> 成功

(assignment=None, fence=Some)
    -> CommandFence / invalid shape

(assignment=Some, fence=None)
    -> 成功；不查询 binding，不因 binding 存在而改变结果

(assignment=Some, fence=Some)
    -> 必须存在 active binding
    -> 必须匹配 exact current available lease
    -> stale/missing/revoked/expired 均拒绝
```

不能直接继续使用 `LegacyOptionalSupervision`，因为它会在 binding 存在时强制 fence；也不能
简单在调用点遇到 `None` 就完全跳过 helper，否则显式伪造 Runtime 归因可能绕过验证。

`RequireSupervisedRuntime` 保留给真正要求成对显式归因的路径，例如当前 Project Document
wire 中调用者主动携带的 `acting_assignment_id + runtime_fence`，以及 Supervisor 自身协议。

### 6.2 Project View v2/v3 writer

涉及：

- `crates/buzz-db/src/project_view.rs`
- `crates/buzz-db/src/project_view_v2.rs`
- `crates/buzz-db/src/project_view_v3.rs`

修改要求：

1. Role command 先使用现有 reducer/actor intent 验证 candidate、owner、Leader 与 assignee；
2. Role-bearing/Leader command 继续要求 active Assignment，且 Assignment 必须属于 signer；
3. v2、v3 随后统一调用 `ValidateExplicitRuntimeAttribution`；
4. binding 存在但 command 没有 fence 时不得返回 `CommandFence`；
5. command 显式提供 fence 时继续使用同一 Community lock 和 transaction 验证；
6. 普通 Project object 若显式携带 Assignment 也使用同一可选归因规则；
7. Assignment 结束时继续调用 `fence_ended_runtime_bindings_in_tx()`，撤销旧 binding 和 lease；
8. Community membership、ban/timeout、credential、revision CAS、projection 和 receipt 逻辑不变。

Relay 现有 `conflict:project_view:runtime_fence` 映射保留，但只用于调用方明确声明了无效 Runtime
归因的情况，不再用于“没有部署 Supervisor”。

### 6.3 Project Document 保持当前边界

普通 Project Document CLI 已采用 Community write 模式，不携带 Assignment/Runtime，不应在
本次重新耦合。

Document wire 当前要求显式 `acting_assignment_id + runtime_fence` 成对出现。调用方既然主动
选择 Runtime attribution，DB 继续用 `RequireSupervisedRuntime` 严格验证。该路径不是普通
Document 权限的默认前置条件。

## 7. Agent-first CLI 实现

涉及：

- `crates/buzz-cli/src/commands/roles.rs`
- `crates/buzz-cli/src/commands/project_view_snapshot.rs`

修改 managed Role command 组装逻辑：

1. 保留 verified Project View identity 与 Role snapshot 读取；
2. 保留 supplied Assignment 与 current Assignment 的一致性校验；
3. candidate identity command 继续省略 Assignment；
4. Role-bearing/Leader command 自动附加 current `acting_assignment_id`；
5. 删除 `runtime_fence_from_env()?.ok_or(runtime_unavailable)` 硬要求；
6. 第一方 Role CLI 默认设置 `runtime_fence=None`；
7. 不根据环境中是否恰好存在 fence 文件自动改变命令语义；
8. 没有 active Assignment 时继续返回 `assignment_unavailable`；
9. 本次不新增 `--as-runtime` 用户参数。若以后需要显式 Runtime 归因，应单独设计清晰入口。

`runtime_fence_from_env()` 及 fence 文件解析可以保留给兼容测试、其他显式路径或未来命令，但
不再由默认 Role command 调用。

## 8. ACP Runtime Supervisor 实现

### 8.1 Reconcile 返回状态，不再决定业务可用性

涉及：

- `crates/buzz-acp/src/runtime_supervisor.rs`
- `crates/buzz-acp/src/role_brief.rs`
- `crates/buzz-acp/src/lib.rs`

将当前 `Result<Option<RuntimeSupervisor>, String>` 式“成功或终止”语义重构为可表达状态的
reconcile outcome。具体类型名可在实现时调整，但必须能区分第 4 节的状态。

关键行为：

- 无 Assignment：清除旧 Runtime/fence，返回 `not_applicable`；
- 无 binding：清除旧 Runtime/fence，返回 `disabled/awaiting_binding`；
- binding 存在但 config 缺失：返回 `degraded_missing_key`，不终止 ACP；
- binding pubkey 与本地 key 不同：返回 `degraded_mismatch`，不提交 evidence；
- binding 匹配：沿用现有 start/resume/recovery/lease 状态机；
- status/evidence 暂时失败：停止发布可写 fence，返回 `unknown/degraded`，按 turn boundary
  和 bounded background retry 重试；
- binding revoke、Assignment replacement 或 epoch 变化：暂停 lease、删除 fence 和不再适用的
  pair state，再按新 canonical 状态收敛。

### 8.2 Role Brief 与 turn admission 解耦

当前 `ResolutionFailure::Runtime` 会把 supervision reconciliation 失败转换为不可用 Role
context。调整为：

- Project/meta/Assignment 无法验证仍然 fail closed，不复用旧 Role authority；
- Runtime reconciliation 失败只产生 supervision diagnostic；
- 已验证的 Assignment/Role Brief 仍交给 Agent；
- Runtime state 以附加的 operational note 注入，不伪装成 canonical Project 对象；
- normal Agent pool/turn admission 不因 `degraded_*`、`expired` 或 `unknown` 关闭；
- Project View maintenance 已实际进入 draining/frozen 时，原有 maintenance gate 继续阻止
  受影响的 Project turn。

### 8.3 动态 fence 文件

- 只有 `active` 且 lease 当前有效时写入 fence 文件；
- 进入任意 disabled/degraded/expired/unknown 状态时立即删除；
- 文件不存在不再影响默认 `buzz roles`；
- 文件权限、随机路径、子进程只读坐标和 supervisor secret 隔离保持不变；
- lease 续租失败后不能继续留下看似有效的旧 epoch。

### 8.4 状态传播

Coordinator 增加独立 supervision state watch，并在状态变化时发出结构化 observer frame，至少
包含：

```text
state
assignment_id?
binding_id?
supervisor_pubkey?
runtime_id?
runtime_epoch?
lease_expires_at?
detail_code?
observed_at
```

observer payload 不能包含 secret、state path 或完整错误堆栈。Desktop 将它视为本地运行诊断，
Relay canonical status 仍是 binding/lease 的权威来源。

## 9. Desktop 实现

### 9.1 独立 supervisor identity

涉及：

- `desktop/src-tauri/src/managed_agents/storage.rs`
- `desktop/src-tauri/src/managed_agents/runtime.rs`
- `desktop/src-tauri/src/managed_agents/runtime_types.rs`
- 对应 Tauri commands、TypeScript API 和 Agent settings UI

Desktop 为每个 Community 懒创建一份独立 supervisor identity：

- 只在用户准备/启用 Runtime supervision 时创建，不为所有 Community 无条件生成；
- 使用 verified Community identity 作为存储作用域，不能把 `localhost` 改写为
  `127.0.0.1` 后当成另一个权限坐标；
- 私钥写入现有 OS keyring/`SecretStore`，沿用 read-back verify 和安全文件 fallback；
- 已存在但暂时无法读取的 keyring entry 不得触发静默密钥轮换；
- supervisor 公钥必须与 Human identity、目标 Agent identity 和已验证 Relay signer 不同；
- 一个 Community 的 supervisor key 可以监督该 Desktop 在该 Community 中的多个 managed
  Assignment，但每个 Assignment 仍有独立 binding、lease 与 Runtime state；
- `BUZZ_RUNTIME_SUPERVISOR_PRIVATE_KEY` 保留为显式开发/headless override。override 与 keyring
  同时存在时必须使用确定的优先级并展示来源，不能静默选择。

ACP 的 durable state 继续按 Agent/Community pair 隔离，并加入 supervisor identity generation
或公钥校验，防止换 key 后误用旧恢复状态。

### 9.2 UI 状态

在 Agent 的 Community runtime 状态中增加独立字段，而不是复用
`ManagedAgentRuntimeLifecycle::Failed`：

```text
runtime lifecycle: starting/listening/waking/ready/failed/stopped
runtime supervision: disabled/awaiting/active/recovering/degraded/expired/unknown
```

Desktop 至少显示：

- Supervisor Off；
- Awaiting binding；
- Active；
- Recovering；
- Degraded，并给出 `missing_key`、`key_mismatch`、`lease_expired`、`relay_unavailable` 等稳定
  detail code；
- Assignment ID、binding ID、supervisor 公钥缩写、lease deadline；
- “普通聊天与 Role 操作仍可用”或“maintenance 当前被阻塞”的准确影响说明。

UI 不提供或复制 supervisor 私钥。可以提供“复制 supervisor 公钥”和适合 operator 使用的
命令模板。

### 9.3 状态刷新

状态来源按以下优先级合并：

1. 当前 Community/Assignment 的 Relay canonical runtime status；
2. Desktop keyring/config 是否存在及其 public key；
3. ACP 最新 observer supervision frame；
4. 进程停止时保留带时间戳的 last-known state，但必须标记 stale。

Community 切换时，该状态属于 community-scoped cache，必须接入
`resetCommunityState()`；不能把另一个 Relay 的 binding 或 last-known frame 显示到当前
Community。

## 10. Operator 管理与本地 provisioning

### 10.1 `buzz-admin project-runtime`

在 `buzz-admin` 增加正式入口：

```bash
buzz-admin project-runtime status \
  --host localhost:3000 \
  --assignment <assignment-uuid>

buzz-admin project-runtime bind \
  --host localhost:3000 \
  --assignment <assignment-uuid> \
  --supervisor-pubkey <hex>

buzz-admin project-runtime revoke \
  --host localhost:3000 \
  --assignment <assignment-uuid>
```

要求：

- `status` 输出 binding、policy、current runtimes、lease 与可操作诊断；
- `bind` 对相同 supervisor pubkey 和 policy 幂等；
- 不同 active binding 返回明确 conflict，不自动替换；
- `revoke` 撤销 binding 并结束所有当前 lease；
- 更换 supervisor 必须显式 revoke 后 bind，不能由 Desktop/ACP 自动抢占；
- 所有 mutation 继续写 hash-chain audit；
- Assignment ended、非 managed Agent、Community 不匹配和 maintenance 非 normal 时保持
  fail closed；
- 日志和 JSON 输出不得包含 supervisor 私钥。

Relay 现有 operator HTTP binding API 保留。`buzz-admin` 可复用 DB service 或 typed operator
client，但不能复制另一套状态转换规则。

### 10.2 本地辅助入口

为本地开发提供幂等辅助流程，职责限定为：

```text
读取当前 Community 与 managed Agent
    -> 找到唯一 active Assignment
    -> 读取/准备 Desktop supervisor public key
    -> operator bind（相同配置时幂等）
    -> 查询 canonical status
    -> 等待 ACP 在下一完整 turn 建立 lease
    -> 输出 active/degraded 结果
```

辅助入口不能：

- 复用 Human/Agent/Relay key 作为 supervisor；
- 绕过 Relay operator authority；
- 伪造 Runtime lease 或 evidence；
- 遇到不同 binding 时自动 revoke；
- 把 secret 打印到终端或写入仓库。

若 supervisor identity 是 ACP 启动后才创建的，必须安全重启对应 Desktop managed harness 才能
注入 secret；如果 harness 启动时已持有该 identity，只是 binding 后创建，则下一次完整
reconciliation 应自动启用 supervision，无需重建 Codex/Claude 子进程。

## 11. Maintenance 与自动恢复边界

### 11.1 仍然严格依赖 binding 的能力

以下行为没有 supervisor 就没有可信语义，继续 fail closed：

- 提交 start/lease/recovery/graceful-stop/heartbeat evidence；
- 生成和验证 Runtime epoch；
- 自动判定 Assignment `unrecoverable`；
- maintenance assignment/runtime baseline ACK；
- drain/freeze 前确认旧 harness generation 已退出。

### 11.2 Maintenance preflight

当前 maintenance begin 遇到 unbound managed Assignment 时只返回通用 conflict。调整为在任何
状态变更前给出结构化 blocker：

- `assignment_unbound`；
- `supervisor_missing`；
- `supervisor_mismatch`；
- `lease_or_monitor_stale`；
- `runtime_recovery_in_progress`；
- `pending_runtime_ack`。

普通 Project/Role 操作不受这些 blocker 影响；operator maintenance 必须先修复 binding、恢复
supervisor，或显式结束相关 Assignment。首版不提供“忽略未监督 Agent 强制迁移”的旁路。

## 12. 错误与可观测性

使用稳定、分层的错误/状态代码：

| 层级 | 示例代码 | 含义 |
|---|---|---|
| Assignment authority | `assignment_unavailable` | 没有可用于本动作的 active Assignment |
| Explicit Runtime attribution | `conflict:project_view:runtime_fence` | 调用方主动声明了无效 fence |
| Supervisor configuration | `supervision:missing_key` | binding 存在，本地没有 key |
| Supervisor identity | `supervision:key_mismatch` | 本地公钥与 binding 不同 |
| Runtime availability | `supervision:lease_expired` | 当前 lease 已过期 |
| Runtime status transport | `supervision:status_unavailable` | 无法读取 canonical status |
| Maintenance | `maintenance:supervisor_not_ready` | maintenance 的可信协调条件不足 |

移除默认 Role CLI 的 `runtime_unavailable`。同一错误不能再同时表示“无 Assignment”“无
binding”“无私钥”和“stale epoch”。

增加结构化日志/metrics：

- supervision state transition count；
- degraded reason；
- binding mismatch；
- lease start/renew/recovery outcome；
- normal Role command with/without explicit Runtime attribution；
- maintenance blocker count。

日志只能记录公钥缩写或公开 binding ID，不记录 nsec、auth header、fence 文件正文或本地
secret path。

## 13. 测试方案

### 13.1 领域与 wire 单元测试

- Role-bearing request 没有 Assignment 仍被拒绝；
- active Assignment + no Runtime fence 合法；
- Runtime fence without Assignment 非法；
- candidate operation 继续允许两字段都省略；
- v2/v3 actor intent 与 wire 行为一致；
- closed JSON 与历史事件 round-trip 不变。

### 13.2 DB PostgreSQL 测试

对 schema v2/v3 分别覆盖：

1. active managed assignee，无 binding、无 fence，Checkpoint 成功；
2. active managed assignee，有 binding、无 fence，Checkpoint 仍成功；
3. active managed Leader，有 binding、无 fence，合法治理动作成功；
4. stale/ended/wrong-owner Assignment 拒绝；
5. 显式正确 Runtime fence 成功；
6. 显式 fence 但没有 binding 拒绝；
7. 显式 stale/expired/wrong Runtime fence 拒绝；
8. Assignment 结束仍原子 revoke binding 与 lease；
9. Community remove、owner 失权、ban/timeout 仍拒绝；
10. revision CAS、receipt replay 与 projection revision 只推进一次。

### 13.3 CLI 测试

- managed Checkpoint/Handoff/Commitment 自动带 Assignment、不读取 fence 文件；
- fence 环境缺失或文件损坏不影响默认 Role command；
- 没有 Assignment 仍返回 `assignment_unavailable`；
- candidate command 不携带 Assignment/fence；
- v2/v3 签名 JSON 精确符合预期；
- ambiguous delivery 继续通过 canonical receipt/read-back 判定。

### 13.4 ACP 测试

- 无 binding、无 config：`disabled`，Agent pool 正常启动；
- 有本地 config、无 binding：`awaiting_binding`；
- 有 binding、无 config：`degraded_missing_key`，Role Brief 仍可用；
- binding 与 config 不匹配：`degraded_mismatch`，不提交 evidence；
- binding/config 匹配：建立 lease、发布 fence、续租；
- status/evidence 暂时失败：清除 fence、保留 Agent、后续重试恢复；
- revoke/rebind/Assignment replacement：状态在完整 turn 边界收敛；
- graceful/abnormal/recovery 既有证据状态机继续通过；
- supervisor secret、state path 不进入 model/MCP 子进程；
- maintenance holding/freeze 仍关闭相应 turn admission。

### 13.5 Desktop 测试

- keyring identity 创建、读取、read-back verify 和 locked recovery；
- keyring 暂不可用时不生成新 supervisor；
- Community 间 secret/status 隔离；
- Relay binding 与本地 pubkey 的 composite state 矩阵；
- Agent `ready + degraded supervision` 不显示为 Agent Failed；
- observer frame 乱序、旧 harness generation 和 Community switch 不污染状态；
- disabled/awaiting/active/degraded/recovering UI 与说明；
- 不出现 private key、任意 px 文本或不可缩放新增文本。

### 13.6 Operator 与 maintenance 测试

- `status/bind/revoke` happy path 与 JSON 输出；
- 相同 binding 幂等，不同 binding conflict；
- non-managed/ended/wrong Community Assignment 拒绝；
- revoke 同时结束 current leases；
- audit 写入失败时 mutation 回滚；
- maintenance begin 对 unbound/degraded Assignment 返回结构化 blocker；
- active supervisor 能完成原有 drain/freeze/ack；
- 自动 `unrecoverable` 仍要求完整 evidence、monitor 与 deployment kill switch。

## 14. 本地纵向验收

以 `ws://localhost:3000` 和 `test-1` 为验收对象，分两阶段执行。

### 14.1 未监督模式

1. 不配置 supervisor 私钥；
2. 确保当前 Assignment 没有 binding，或保留一个故意缺 key 的 binding；
3. 启动 Desktop/ACP；
4. 确认 Agent 正常回复；
5. 让 `test-1` 追加 Checkpoint；
6. canonical history 与 Role Brief 能回读；
7. Desktop 分别显示 `disabled` 或 `degraded_missing_key`；
8. DB 不因普通 Role command 创建 binding/lease。

### 14.2 受监督模式

1. Desktop 准备独立 Community supervisor identity；
2. 读取 public key，不暴露 secret；
3. 使用 operator 命令为 `test-1` 当前 Assignment 幂等 bind；
4. ACP 下一次完整 reconcile 建立 lease；
5. Desktop 状态转为 `active`；
6. Runtime status、fence file、binding、runtime ID/epoch 一致；
7. lease 续租和 graceful stop 成功；
8. revoke 后 fence 被删除，Agent 仍能聊天和以 active Assignment 追加 Checkpoint；
9. maintenance 仅在监督状态完整时通过 preflight。

真实验收不得恢复旧数据库、重新初始化 Project View 或删除现有 Document/Resource/Context
数据。

## 15. 兼容性、发布与回滚

### 15.1 数据兼容

- 不需要数据库 migration；
- 现有 binding、lease、evidence 和 audit 全部保留；
- 已签名 Role/Project View command wire 不变；
- status response 只增加可选字段；
- Desktop 新增 keyring entry，不迁移或复用 Human/Agent key；
- 历史设计/验收报告保持原记录，通过本设计的 supersession 说明修正当前权威语义。

### 15.2 推荐交付顺序

1. 领域/wire 注释与回归测试；
2. DB 新 optional-attribution policy，切换 v2/v3 writer；
3. CLI 移除默认 Runtime fence 强制；
4. Relay status 扩展与 operator admin 命令；
5. ACP reconcile 状态机、Role Brief 解耦和动态 fence 清理；
6. Desktop keyring identity、状态合并和 UI；
7. maintenance blocker 诊断；
8. Rust/TypeScript/Playwright/DB 定向测试；
9. 本地两阶段纵向验收；
10. `just ci` 与构建启动验收；
11. 交付后清理仓库增量构建缓存，保留运行所需产物。

### 15.3 回滚

协议和数据库均为兼容修改，代码可以按组件回滚。但如果只回滚 DB/CLI 授权解耦，会重新出现
`runtime_unavailable` 阻断；如果只回滚 Desktop/ACP 状态层，binding 仍可工作但失去可观测性。
因此发布和回滚均应把“授权解耦”和“状态/provisioning 闭环”视为一个交付单元。

## 16. 非目标

本次不包含：

- 删除 Runtime supervision 表、evidence 或 scheduler；
- 让普通 Community member 注册 supervisor binding；
- 复用 Human、Agent 或 Relay identity；
- 自动抢占不同 supervisor 的现有 binding；
- 保证同一 Assignment 只有一个模型进程；
- 新增 Project View schema v4；
- 为普通 Document CRUD 重新增加 Role/Runtime 前置条件；
- 在 maintenance 中增加忽略未监督 Runtime 的不安全 override；
- 重新设计远程部署平台如何实际拉起失败进程。

## 17. 完成定义

只有同时满足以下条件，本缺陷才可关闭：

1. active Assignment 的 Role command 不再依赖 binding/fence；
2. 显式 Runtime attribution 仍严格、不可伪造；
3. Supervisor 缺失或异常不再导致 Agent/Role Brief 整体不可用；
4. Relay、ACP、Desktop 对 binding 状态有一致、可解释的视图；
5. operator 能通过正式入口诊断、幂等绑定和撤销；
6. Desktop 独立、安全地管理 supervisor identity，且不向模型泄露；
7. maintenance 与自动恢复仍保持原有可信边界；
8. v2/v3、CLI、DB、ACP、Desktop 和本地真实 Relay 纵向测试通过；
9. 文档明确记录已接受的“旧进程在 Assignment 结束前仍可写 Role command”取舍；
10. 当前 `test-1` 在 disabled、degraded 与 active supervision 三种关键状态下均得到预期行为。

## 18. 实现与验收记录

2026-08-03 已完成本设计的实现交付：

- Relay/DB 将普通 Role 写入改为 Assignment 授权；只有命令显式携带 Runtime fence 时才校验
  binding、Runtime ID、epoch 与 lease。Project Document 的显式 Runtime-attributed 路径和
  maintenance 路径继续严格校验；
- `buzz-cli` 不再为 managed Agent 的普通 Role command 强制读取 Runtime fence，同时仍附加并
  校验 active Assignment；
- Runtime status 增加不含秘密的 binding 坐标，`buzz-admin project-runtime
  status/bind/revoke` 提供 operator 控制面；
- ACP 将缺 key、未绑定、binding 不匹配、lease 过期和 Relay 暂时不可用表示为独立降级状态，
  不再让这些状态破坏 Role Brief、Agent 启动、聊天或普通 Role 写入；
- Desktop 为每个实际连接 Relay 地址管理独立 supervisor identity，只把私钥交给受信任的
  `buzz-acp`，并展示状态、影响和可复制的幂等绑定/修复命令。用于进程去重的 loopback
  规范化不会再改变 supervisor identity 或 operator command 的 Community 坐标；
- maintenance begin 对未绑定或未就绪 Assignment 返回结构化 blocker，不能绕过原有 drain、
  freeze、ACK 和自动恢复边界。

自动化验收包括 Rust 编译与 Clippy、ACP 降级状态测试、Desktop native 身份隔离测试、
TypeScript 类型检查与状态投影测试，以及真实 PostgreSQL 下的 Runtime supervision 和
maintenance 纵向测试。测试覆盖 disabled、awaiting binding、missing key、mismatch、active、
lease/recovery 和显式 fence 拒绝路径。

本地 `ws://localhost:3000` 未监督纵向验收结果：Relay readiness 正常；`test-1` 的 active
Assignment 为 `4a3fe16d-6946-42d5-9f5e-b5c3ec54aac5`，Runtime status 明确返回 unbound；
新版 ACP 仍成功解析 assigned Role context（Project revision 104）、订阅 Channel 并上线。
这验证了当前缺陷的核心路径：缺少 binding 不再阻断 Agent 与 Role context。受监督的本地
active 状态需要用户在 Desktop 显式执行 `Prepare Supervisor`，再由 operator 执行界面提供的
bind 命令；该流程不在验收脚本中静默创建身份或修改现有 binding。
