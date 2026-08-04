# Meeting V2：主持人直接完成行动收口的后端修正方案

> 状态：待实现
>
> 日期：2026-08-03
>
> 范围：Meeting、Relay、DB、SDK、CLI、ACP 与后端验收；不包含 Desktop、Web、Mobile。
>
> 关系：本文取代
> [会议行动收口实现设计](../v2/meeting-v2-action-finalization-design.md) 中强制
> `Materialization Intent → Action Plan → Step → Materializer` 的设计。后端修正完成前，
> 不修改 Desktop spec；后端完成后再单独适配 Desktop。

## 1. 结论

Meeting 仍然需要一个属于会议生命周期的 `finalizing_actions` 阶段，但这个阶段不应再引入
一套独立于目标系统的业务模型。

本次修正采用以下模型：

1. 最终 Board 继续记录会议结论、行动产出、承接决定和必要上下文。
2. 主持人选择 `FINALIZE_ACTIONS` 后，Meeting 冻结最终 Board，并进入独立 deadline 的
   `finalizing_actions`。
3. Agent 主持人继续使用参与会议的同一个槽、同一个 ACP Session，直接调用现有业务工具，
   例如 `buzz project-view` 和 `buzz roles`。
4. Human 主持人直接使用已有业务管理界面或 CLI；Meeting 后端不要求 Human 填写另一份
   结构化意图或 Plan。
5. 主持人认为 Board 中需要在闭会前登记的行动产出已经处理完毕后，签署一次
   `actions-recorded` 完成声明；该声明与 `End(outcome=closed)` 是同一个协议操作。
6. Relay 只校验主持身份、Meeting 状态、最终 Board、action run 和 retry window 等生命周期
   fence，不解析 Board，也不验证或枚举主持人做过哪些外部写入。
7. Project View 继续自行校验对象类型、关系、revision 和命令合法性。Meeting 不再拥有
   Requirement、Work、Role responsibility 的专用 adapter。

因此，新路径中不存在 Meeting 内部的 `Materialization Intent`、`Action Plan`、`action_id`、
`step_id`、`step-prepared`、`step-applied` 或 `ready_to_close`。

这里删除的是 Meeting 行动收口内部的 Action Plan。Project View 自身的 `Plan` 对象仍是普通
业务对象，不受影响。

## 2. 为什么需要修正

### 2.1 当前实现形成了第二个业务入口

当前实现要求：

```text
最终 Board
  → 主持 Agent 输出 Materialization Intent
  → Harness 编译 Meeting Action Plan
  → Relay 冻结 Plan
  → Harness 按 Step 构造 Project View event
  → Relay 验证每个 Step receipt
  → ready_to_close
  → End
```

这条链路原本用于提供确定性恢复和精确完成证明，但也产生了三个根本问题：

- Board 之外又出现一份必须表达相同会议决定的结构化业务输入；
- Meeting 必须理解并跟随 Project View 的对象和命令模型；
- 每增加一种合法业务操作，都要同步扩充 intent schema、plan schema、step kind、materializer、
  Relay 校验和 Human 表单。

所以当前只能创建 Requirement、Work 和 responsibility，不是 Project View 本身只能做这些，
而是 Meeting adapter 把开放的业务能力缩成了一个封闭子集。

### 2.2 现有业务入口已经足够

现在已经存在两条权威业务操作路径：

- Human 可以在 Project View 管理界面中直接操作；
- Agent 可以通过 `buzz project-view`、`buzz roles` 等 CLI 直接操作。

这些路径已经负责签名、revision、typed payload、冲突和领域约束。Meeting 再把同一操作编译
成 Plan/Step，不会增加业务表达能力，只会增加耦合和限制。

### 2.3 仍需保留最小 Meeting 阶段

不能简单删除 `finalizing_actions` 并在 Floor Turn 中顺手写外部系统，因为仍需保证：

- 行动登记属于 Meeting 生命周期，成功前 Meeting 尚未关闭；
- Board Maintenance、Floor Decision 和行动登记不共享 deadline；
- 行动登记期间最终 Board 不再变化；
- Agent 主持人继续使用原会议槽和 ACP Session；
- Human 和 Agent 都能明确完成、阻塞、重试、返回 Board 或 abort；
- 过期或旧 action window 的关闭命令不能关闭当前 Meeting。

保留的是生命周期 fence，不是另一套业务执行计划。

## 3. 设计原则

### 3.1 Board 是会议决定的唯一内容载体

Meeting 后端只冻结并引用最终 `board_event_id`。它不要求固定 Markdown 标题，不从 Board
解析 Requirement、Work、Issue、承接人或目标系统，也不把 Board 转换成另一份业务 JSON。

### 3.2 主持人负责解释并执行会议决定

主持人维护了 Board，并拥有完整会议上下文。行动收口时仍由主持人判断：

- 哪些 Board 记录需要在闭会前登记；
- 应该写入 Project View，还是使用其他已经存在的业务工具；
- 是创建、更新、删除还是确认现有状态已经满足决定；
- 何时可以声明行动产出已经完成。

系统不以规则程序或另一次 LLM 调用替代该判断。

### 3.3 目标系统拥有领域规则

Meeting 不校验 Project View 对象拓扑。所有普通业务命令继续进入原处理链路，并由对应系统
决定是否合法。

新 direct action 模块不得依赖 `buzz_project_view` 领域类型，不得构造 Project View event，
也不得调用 Project View DB transaction。

### 3.4 完成是主持人声明，不是 Meeting 的外部状态证明

`actions-recorded` 表示：

> 主持人确认，最终 Board 中需要在正常闭会前登记的行动产出，已经按主持人的判断完成登记
> 或确认无需新增登记。

它不表示 Work 已经完成，不表示承接人已经接受，也不表示 Relay 逐项证明了外部效果。

这与现有正常关闭的信任边界一致：Relay 也不会解析讨论内容来证明会议目标真的已经达到，
而是接受合法主持人的 `CLOSE` 判断。

### 3.5 不增加权限模型

Meeting 不授予也不削弱外部写权限。主持 Agent 或 Human 继续使用自身已有身份，目标系统沿用
当前 Community 权限规则。本次不设计新的授权、审批或委托机制。

### 3.6 外部效果不回滚

普通业务命令一经目标系统接受，就不会因为 Meeting 随后 retry、return-to-board 或 abort 而
回滚。Meeting 必须如实展示这一边界，不能再声称它能证明“零外部效果”。

## 4. 范围与非目标

### 4.1 本次后端修正包含

- 新 direct action policy、runtime capability 和 Create gate；
- 最小 action run 状态、独立 deadline、block/retry/return；
- 由主持人完成声明直接关闭 Meeting 的协议；
- Agent 主持人的同槽、同 ACP Session 直接工具执行；
- Human 主持人的无 Plan 后端完成路径；
- SDK、Relay、DB、CLI、ACP、测试和运维门禁；
- 旧 planned action runtime、Materializer、协议命令、存储、CLI 和测试的完整移除。

### 4.2 本次不包含

- Desktop 界面或现有 Desktop spec 修改；
- Human 行动物化表单；
- Meeting 内部 Action Plan 或 Step 的通用化；
- 新 Project View API、批处理协议或 Meeting 专用 Project View endpoint；
- 对外部写入做跨系统事务、补偿或回滚；
- 自动判断 Board 与 Project View 是否语义一致；
- 自动要求至少发生一次外部写入；
- 新权限、审批、承接确认或 Work 执行；
- 为每个 Project View event 强制添加 Meeting provenance。

## 5. 修正后的生命周期

### 5.1 主路径

```text
discussion
    ↓
final Board Maintenance
    ↓
Floor Decision
    ├── CLOSE
    │     └── End(outcome=closed)
    │
    └── FINALIZE_ACTIONS
          ↓
       finalizing_actions / runnable
          ↓
       主持人直接使用现有业务操作入口
          ↓
       End(
         outcome=closed,
         attestation=actions-recorded,
         current action run/window/Board fence
       )
```

`FINALIZE_ACTIONS` 只在主持人认为闭会前还需要登记行动产出时使用。Project View 引用本身不
触发该阶段；如果行动已经在讨论期间登记完毕，或者会议没有任何外部登记需求，主持人可以
直接 `CLOSE`。

### 5.2 action run 状态

新 direct policy 不再拥有 `planning | applying | ready_to_close`。权威状态只需要：

```text
runtime_phase    = finalizing_actions
action_condition = runnable | blocked
```

`runtime_phase` 表达 Meeting 正在行动收口；`action_condition` 表达当前窗口能否继续。不存在
额外的“计划已冻结”或“步骤已全部应用”阶段。

### 5.3 状态转换

| 当前状态 | 主持操作或系统结果 | 下一状态 | 说明 |
|---|---|---|---|
| `floor_ready` | `CLOSE` | `ended/closed` | 无需专门行动收口 |
| `floor_ready` | `FINALIZE_ACTIONS` / `begin` | `finalizing_actions/runnable` | 冻结最终 Board，启动独立 deadline |
| `finalizing_actions/runnable` | `actions-recorded` End | `ended/closed` | 主持声明与正常 End 原子提交 |
| `finalizing_actions/runnable` | `block` 或 deadline | `finalizing_actions/blocked` | Meeting 保持未关闭 |
| `finalizing_actions/blocked` | `retry` | `finalizing_actions/runnable` | action window 增一，使用新 deadline |
| `finalizing_actions/*` | `return-to-board` | `board_pending` | 外部效果保留，打开新 Board window |
| 任意非终态 | 合法 `abort` | `ended/aborted` | 外部效果保留 |

### 5.4 一次确认直接结束

新路径没有先 `complete`、再 `close` 的两段提交。`End(outcome=closed)` 本身携带
`actions-recorded` 声明和 action fence；Relay 在一个数据库 transaction 中同时：

1. 校验主持身份和 direct action run；
2. 校验当前 action window、最终 Board 和 runnable condition；
3. 把 action run 标记为 `completed_closed`；
4. 把 Meeting 标记为 `ended/closed`；
5. 归档 Meeting Channel；
6. 生成终态 State 和 outbox 事件。

这使 Human 产品操作可以真实地只有一个判断：“是否确认行动产出已完成并结束会议”。

## 6. Agent 与 Human 路径

### 6.1 Agent 主持人

Agent action turn 必须满足：

- 使用最终 Board Maintenance 和 Floor Decision 已绑定的同一个物理槽；
- 使用同一个 ACP Session ID；
- 在 Turn 开始时注入精确冻结的最终 Board；
- 开放该 Agent 原本拥有的标准工具面；
- 由 Agent 自己读取目标系统最新状态并执行普通命令；
- 完成后只返回 Meeting 控制决定，不返回业务 Plan。

以 Project View 为例，Agent 可以根据实际需要使用：

```text
buzz project-view get
buzz project-view create ...
buzz project-view update ...
buzz project-view delete ...
buzz roles ...
```

它可以操作当前 CLI 已支持的 Goal、Plan、Stage、Requirement、Issue、Work、Resource、Role
及其关系，而不受 Meeting adapter 的 Requirement/Work 子集限制。

行动 Turn 的模型终态输出只包含控制结果：

```json
{"action":"COMPLETE"}
```

或：

```json
{
  "action":"BLOCK",
  "reason_code":"external_state_conflict",
  "reason":"optional bounded diagnostic"
}
```

还可选择 `RETURN_TO_BOARD` 或 `ABORT`。这些输出表达主持人的决定；Harness 负责签署和提交
对应 Meeting 协议事件，但不编译、补充或重放业务操作。

### 6.2 Human 主持人

Human 路径不需要 ACP slot，也不需要专用结构化表单：

1. Human 进入行动收口；
2. 直接使用现有 Project View 管理界面或 CLI；
3. 回到 Meeting；
4. 选择“确认行动产出已完成并结束会议”；
5. 客户端签署带 direct action fence 的 End。

Human 可以不做任何 Project View 写入。例如最终状态已经存在、行动应记录在其他系统，或者
主持人确认 Board 本身已经是足够的产出。Relay 不以写入计数阻止关闭。

### 6.3 其他参会者

只有冻结的主持人执行 action turn 和完成声明。其他 Agent 参会者不获得新的行动物化 Turn，
也不因为行动收口改变其身份或权限。

这不表示其他参会者已经执行 Board 中分配给他们的 Work。主持人的动作只是把会议决定登记
到承载系统。

## 7. Wire 协议

### 7.1 使用新的 wire 代际，但不保留双引擎

当前 `moderated-board-actions-v1` 的 wire 语义明确要求 Plan/Step。直接在同一 policy 下改变
命令集合和 close gate，会让混合版本 Relay、CLI 和 ACP 对同一 Session 得出不同解释。

因此新增：

```text
v=3
policy=moderated-board-actions-v2
capability=meeting-v2-action-finalization-v2
```

建议代码常量命名为：

```text
MEETING_V2_DIRECT_ACTIONS_POLICY
MEETING_V2_DIRECT_ACTIONS_CAPABILITY
```

`moderated-board-actions-v1` 不再提供 Create、恢复或命令执行能力。部署前必须确认其 active
Session 数量为零；若不为零，迁移直接失败，而不是在新版本中保留旧 handler。

这里的 policy 代际不是会议类型或模板，也不作为长期用户选项。部署完成后，新 Meeting V2
统一创建 direct policy；主持人仍在会末自由选择 `CLOSE` 或 `FINALIZE_ACTIONS`。

### 7.2 复用现有 event kind

不增加新的 Nostr kind：

- kind `42112` 继续承载 action run 控制命令；
- kind `42101` 继续承载 Meeting End；
- `policy` tag 必须是当前 direct v2；旧 v1 policy 和旧命令被明确拒绝。

### 7.3 direct action 命令

新 policy 的 kind `42112` 只接受：

- `begin`；
- `block`；
- `retry`；
- `return-to-board`。

它不接受：

- `plan`；
- `step-prepared`；
- `step-applied`；
- `complete`。

`complete` 被带声明的 End 取代。

### 7.4 action fence

除 `begin` 外，direct action 控制命令使用：

```text
action-run=<uuid>
action-window=<positive integer>
board=<final board event id>
```

不再出现 `action-plan` tag。

`begin` 继续绑定：

- expected control epoch；
- completed Board window；
- exact authoritative State event；
- exact final Board event；
- 可选的 moderator decision attempt。

### 7.5 完成声明 End

从 `finalizing_actions` 正常关闭时，kind `42101` 至少携带：

```text
h=<meeting uuid>
v=3
policy=moderated-board-actions-v2
e=<create event id>
outcome=closed
attestation=actions-recorded
action-run=<uuid>
action-window=<positive integer>
board=<final board event id>
```

close content 保持为空。Human 不需要提交操作摘要；Agent 的可选诊断也不成为关闭前置条件。

从 `floor_ready` 直接关闭时不得携带 action attestation 或 action fence。从
`finalizing_actions` 关闭时则必须全部携带，避免普通 Close 绕过完成声明。

### 7.6 return-to-board 的诚实语义

direct 模式无法知道普通业务入口是否已经产生效果。因此 `return-to-board` 不再要求 Relay
证明零效果，而是要求主持人显式携带：

```text
external-effects=preserved
```

该 tag 是对“可能已有外部效果且不会回滚”的确认，不是“确实存在外部效果”的声明。命令成功
后终结当前 run、打开新的 Board window，并在响应中返回同一语义。

### 7.7 幂等与 stale 防护

- 相同签名 event 重放返回相同 receipt；
- 旧 action run、旧 action window 或错误 Board 的命令被拒绝；
- blocked 状态不能直接正常关闭，主持人必须先 retry；
- retry 增加 window 并创建新 deadline，旧窗口的迟到 End 不能关闭 Meeting；
- Agent COMPLETE 后，Harness 必须先持久化精确签名 End，再发布；响应丢失时重放同一 event，
  不重新要求模型判断。

普通 Project View 命令继续使用自身的 event ID、revision 和 receipt 语义。Meeting 不提供跨
多个普通命令的 exactly-once 语义。

## 8. 数据库设计与迁移

### 8.1 使用前向迁移

不得改写已经存在的 `0040_meeting_v2_action_finalization.sql`。新增：

```text
migrations/0041_meeting_v2_direct_action_finalization.sql
```

保留 `0040` 文件只是维护 migration ledger 和已执行数据库的 checksum，不代表保留旧 runtime。
应用完 `0041` 后，最终 schema 和代码中都不再存在旧 Plan/Step 执行结构。

迁移扩展 `meeting_sessions` 的 policy constraint，使其接受
`moderated-board-actions-v2`。现有 `finalizing_actions` runtime phase 和
`action_finalization_ms` 配置可直接复用。

迁移开始时必须检查不存在 `floor_policy_version=moderated-board-actions-v1` 的 active Meeting。
检查失败就中止迁移，不能静默转换或遗留一个无法继续的 Session。当前已确认线上和进行中的
Meeting 均为零，因此不需要在应用层保留 drain handler。

### 8.2 用最小 direct schema 替换旧 action 表

`0041` 按外键依赖顺序删除旧结构：

1. `meeting_v2_action_step_attempts`；
2. `meeting_v2_action_steps`；
3. `meeting_v2_action_command_receipts`；
4. `meeting_v2_action_runs`。

随后以同一个通用名称重新创建最小的 `meeting_v2_action_runs`。它只表达当前 direct action
生命周期，核心字段为：

| 字段 | 用途 |
|---|---|
| `community_id`, `session_id` | Community 与 Meeting 边界 |
| `action_run_id` | 一次行动收口的稳定 ID |
| `begin_event_id` | begin 幂等与审计 |
| `board_event_id` | 精确冻结的最终 Board |
| `control_epoch`, `board_window` | 进入收口时的控制 fence |
| `action_window_epoch` | retry CAS fence |
| `action_condition` | `runnable | blocked` |
| `action_deadline_at` | 当前 runnable window 的独立 deadline |
| `last_error_code` | 低基数阻塞原因 |
| `completion_event_id` | 成功关闭时的 End event |
| `terminal_status` | `completed_closed | completed_aborted | returned_to_board` |
| `created_at`, `updated_at`, `terminal_at` | 审计时间 |

约束要求每个 direct Session 最多一个 active run。runnable run 必须有 deadline；blocked run
不得有 deadline；terminal run 必须有 `terminal_at` 且不得有 deadline。

表中不允许出现 `plan_json`、`plan_event_id`、Project revision、target object、action item 或 step
字段。

### 8.3 direct command receipt

重新创建最小的 `meeting_v2_action_command_receipts`，只记录
begin/block/retry/return-to-board 的
签名命令、run/window、结果和稳定响应。正常完成继续使用既有 End 事件与 Meeting 终态记录，
并把 End ID 写入 `completion_event_id`。

### 8.4 旧数据处理

旧 Plan、Step 和 attempt 的关系型投影随 `0041` 删除，不迁移到新表。原因是：

- 当前不存在需要恢复的线上或进行中 Session；
- 这些字段在新语义中没有对应物；
- 为无使用者的旧模型设计 archive schema 会重新引入长期负担。

已经持久化在通用 Nostr event store 中的原始签名事件不需要专门删除；但 Relay、DB 和 CLI
不再解释旧 action policy 或提供旧 Plan/Step 恢复能力。开发环境若存在旧验收数据，可以重建
数据库或让 `0041` 清理这些 obsolete projection。

### 8.5 事务边界

以下操作继续在 Meeting Session row lock 下串行：

- begin 与 Floor/Human priority/End 的竞争；
- block、retry、return-to-board；
- 带 attestation 的正常 End；
- abort、deadline recovery 和 participant revocation。

Project View 普通写入不加入该 transaction。这是明确的边界，不尝试跨域原子提交。

## 9. Relay 与权威 State

### 9.1 protocol dispatch

Relay 只注册 `moderated-board-actions-v2` 的 direct action handler。Create、action command 或 End
出现 `moderated-board-actions-v1` 时返回明确的 unsupported policy 错误，不进入任何 legacy
执行路径。

不得只相信后续命令自报的 policy。命令 policy 必须与 Session Create 冻结的 direct policy
完全一致。

### 9.2 direct State 投影

`board_control.phase=finalizing_actions` 时，新 policy 的 `action` 只投影：

```json
{
  "mode": "host_direct",
  "action_run_id": "...",
  "board_event_id": "...",
  "control_epoch": 1,
  "board_window": 3,
  "action_window_epoch": 1,
  "condition": "runnable",
  "action_deadline_at_ms": 0,
  "last_error_code": null
}
```

它不投影 plan、step、目标对象、操作数量或 Project revision。

### 9.3 写入冻结

进入 `finalizing_actions` 后继续拒绝 Meeting room 内的 Intent、Request、Offer、Grant、Speech、
Yield、Handoff 和 Board Maintenance。普通 Community Project View/Role 命令不属于 Meeting
room write，因此继续按原业务路径处理。

### 9.4 close gate

Relay 接受 direct action 正常 End 的必要条件：

- actor 是 Create 时冻结的主持人；
- Session policy 是 direct v2；
- runtime phase 是 `finalizing_actions`；
- 存在同 Session 的 active direct run；
- run condition 是 `runnable`，并且在同一 transaction 的 lazy recovery 后 deadline 仍未到期；
- `action-run`、`action-window` 和 `board` 与权威 run 完全一致；
- `attestation=actions-recorded`；
- 最终 Board outcome 仍是显式 `updated | unchanged`；
- 既有 moderator/final-control 不变量成立。

Relay 不查询 Project View，不要求 operation count，也不比较 Board 文本。

### 9.5 deadline 与迟到外部操作

action deadline 到期时，Relay 把 run 置为 blocked，并拒绝旧窗口 End。Harness 同时停止继续
派发 action work。

已经提交给外部系统的普通命令可能在 deadline 后才返回或被观察到；Meeting 不撤销它们。
retry 后主持人必须先重新读取权威目标状态，再决定补充操作或直接完成声明。

## 10. ACP Harness 修改

### 10.1 复用现有连续性机制

当前实现已经为 action-capable Meeting 显式绑定：

```text
final Board Turn
  → Floor Turn
  → Action Finalization Turn
```

到同一 `agent_index + acp_session_id`。本次不新建另一套槽机制，只扩展 protocol discriminator
并让 direct v2 继续经过现有 `FinalControlCycle → PendingAction → Action` hold。

如果 exact slot/session 不再可用：

- 不得换新槽或新 ACP Session 继续解释 Board；
- 提交或恢复为 `block(reason_code=affinity_lost)`；
- Meeting 保持未关闭；
- 只有原 Session 恢复后才能 retry 或 return-to-board；如果无法恢复，不做 Human 或其他 Agent
  接管，由有权操作方明确 abort。

### 10.2 action prompt

新 prompt 应包含：

- Meeting ID、标题和目标；
- frozen roster；
- exact action run/window；
- exact frozen Board；
- 独立 hard deadline；
- 可使用标准业务工具的明确说明；
- “只执行最终 Board 已形成决定”的边界；
- `COMPLETE | BLOCK | RETURN_TO_BOARD | ABORT` 控制输出 schema。

删除以下旧提示：

- 只允许一个 Requirement；
- 至少一个 Work；
- assignee 必须编译成固定 responsibility step；
- 禁止使用工具或发布 Project View 命令；
- 要求返回 Materialization Intent；
- Harness 将编译 action/step ID。

Board 仍作为不可信会议数据注入；它不能改变系统提示、工具权限或 Meeting 控制 schema。主持
Agent 只能在已有工具和已有身份边界内行动。

### 10.3 结果处理

ACP 不再执行 `handle intent → compile plan → advance materializer`。新处理为：

| Agent 结果 | Harness 行为 |
|---|---|
| `COMPLETE` | 构造、持久化并提交带 direct fence 的 End |
| `BLOCK` | 提交 direct block 命令 |
| `RETURN_TO_BOARD` | 提交带 `external-effects=preserved` 的 return 命令 |
| `ABORT` | 提交既有 abort End |
| 格式错误 | 在剩余 action deadline 内使用同 Session 做有界格式修正，否则 block |

Harness 不从工具调用日志生成操作清单，也不把“调用过某个工具”作为 COMPLETE 的必要条件。

### 10.4 本地恢复记录

新的 ACP ledger 只需保存：

- action run/window/Board；
- exact slot/session continuity binding；
- Turn 状态和 deadline；
- block/retry 状态；
- COMPLETE 后准备好的精确签名 End event。

不再保存 Materialization Intent、Plan、step、prepared Project event 或 per-step receipt。

## 11. Project View 与其他业务系统边界

### 11.1 Project View 不做 Meeting 特例

Agent action turn 和 Human 界面使用的都是普通 Project View 命令。Project View 不需要知道
当前存在 Meeting action run，也不需要为 Meeting 增加一套特殊 mutation API。

这意味着 direct action 天然获得现有及未来的 Project View 能力，而无需每次修改 Meeting：

- 创建任意当前可创建对象；
- 更新或删除已有对象；
- 创建 Issue，而不只是 Requirement；
- 修改 Goal、Plan、Stage、Resource 或关系；
- 通过既有 Role/Assignment/Work 命令记录责任关系。

所有能力仍受 Project View 自身 schema 和 revision 约束。

### 11.2 本次不强制 provenance

初版不要求普通业务命令携带 `source_meeting_id`。现有事件已经记录 signer、时间、对象和
revision；Meeting 则记录 frozen Board 和主持人的完成声明。

如果以后确实需要跨界追溯，可为普通业务命令设计通用 optional provenance，而不是恢复
Meeting Plan/Step 或只服务 Project View 的专用 adapter。

### 11.3 一致性取舍

移除 Plan/Step 后，Meeting 不再提供以下保证：

- Relay 能证明 Board 中每个行动项都有对应外部对象；
- 多个外部命令组成原子事务；
- retry 不会因主持人重新解释而产生语义重复；
- return-to-board 前一定没有外部效果。

保留的保证是：

- 最终 Board、action run 和完成声明有稳定审计记录；
- 只有合法主持人能声明完成；
- stale run/window 不能关闭 Meeting；
- Agent 的解释和操作来自参与会议的同一 ACP Session；
- 每个外部命令仍由目标系统独立校验和幂等处理。

这是本次设计有意选择的边界：业务灵活性和单一入口优先于 Meeting 对外部世界的逐步证明。

## 12. CLI 修改

### 12.1 direct policy 命令

保留或新增：

```text
buzz meetings actions status --meeting <uuid>
buzz meetings actions begin --meeting <uuid>
buzz meetings actions block --meeting <uuid> --reason-code <code>
buzz meetings actions retry --meeting <uuid>
buzz meetings actions return-to-board --meeting <uuid>
buzz meetings actions confirm-recorded --meeting <uuid>
```

`confirm-recorded` 构造的是 End，不是 kind `42112 complete`。命令成功后 Meeting 已关闭。

普通业务操作继续使用已有命令，不嵌套到 `meetings actions` 下。

### 12.2 删除旧 CLI 面

从 `MeetingActionsCmd`、dispatch、help 和测试中删除：

- `plan`；
- `apply`；
- 旧语义的 `complete`。

CLI 不保留 hidden legacy subcommand。若旧脚本继续调用这些命令，应在参数解析阶段失败，从而
尽早暴露已经过期的调用方。`status` 只解析 direct action State。

## 13. 配置、能力与灰度

### 13.1 新能力声明

新增 ACP runtime capability：

```text
meeting-v2-action-finalization-v2
```

Create transaction 继续检查完整 frozen roster 中的所有 managed Agent，而不只检查主持 Agent。
原因是所有 Agent runtime 都必须理解新 State、终止 action 前的讨论冻结和 Session 清理；只有
主持 Agent 会实际执行 action turn。Human 不需要 runtime capability。

### 13.2 Relay NIP-11

建议新增：

```text
buzz-meeting-v2-direct-actions
buzz-meeting-v2-direct-actions-create
```

runtime extension 表示 Relay 能读取和恢复 direct Session；create extension 只有在基础
Meeting V2 gate、新 direct create gate 和 runtime 能力都就绪时才声明。

### 13.3 Create gate

新增默认关闭的：

```text
BUZZ_MEETING_V2_DIRECT_ACTIONS_CREATE_ENABLED=false
```

实现时直接删除旧 `BUZZ_MEETING_V2_ACTIONS_CREATE_ENABLED` 配置项、旧 NIP-11 extension 和旧
ACP capability；不把它们保留为 deprecated alias。

### 13.4 发布顺序

1. 在发布前门禁中确认旧 v1 active Session 数量为零；
2. 发布 `0041`、direct Relay/DB/SDK/CLI，保持 direct Create 关闭；
3. 发布声明 v2 capability 的 ACP fleet；
4. 确认所有 Relay 和 Agent runtime 都只声明 direct 能力；
5. 开启 direct Create gate；
6. 新建 action-capable Meeting 统一使用 direct policy。

## 14. 可观测性

新路径保留低基数指标：

- direct action begin；
- completion attestation accepted/rejected；
- block reason；
- retry；
- action deadline exceeded；
- return-to-board；
- affinity mismatch；
- direct action duration；
- active runnable/blocked runs。

日志和指标可包含 Meeting ID、run ID、window、Board event ID 和 reason code，但不记录 Board
正文、模型完整上下文、Project View payload 或自由文本工具输出。

删除新路径中的 step count、step kind、prepared/applied attempt 和 completion project revision
指标，同时删除旧 planned policy 专属指标。

## 15. 分阶段开发计划

### 阶段一：协议与存储基础

目标：建立 direct policy 和最小 action run，但尚不接入 Agent 自动执行。

关键工作：

- SDK 增加 direct policy/capability、Create/Board/action/End builders；
- SDK 删除旧 Action Item/Plan/Step 类型、validator 和 builders；
- 定义 direct run fence 和 `actions-recorded` End；
- 增加 `0041` 前向迁移，删除旧 step schema 并重建最小 run/receipt 表；
- DB 实现 begin、block、retry、return、deadline 和 attested End gate；
- State 投影 direct action shape；
- 删除 planned v1 DB 状态机和 Project View adapter 依赖；
- 单元测试 wire、约束、CAS、幂等和终态 transaction。

阶段验收：可以在 DB/SDK 测试中完成
`final Board → begin → attested End → ended/closed`，全程没有 Plan 或 Step。

### 阶段二：Relay 与 Human/CLI 后端闭环

目标：Human 主持人可通过现有业务 CLI 加一次确认完成完整生命周期。

关键工作：

- Relay 增加 direct action parser、handler、End parser 和严格 policy 校验；
- Relay 删除 plan/step/complete parser、handler 和 step metrics；
- 配置、NIP-11 extension 和 create gate；
- CLI 增加 direct status、begin、block、retry、return 和 `confirm-recorded`；
- CLI 删除 `plan|apply|complete` 及专用 Project View materializer；
- direct action 期间验证普通 Project View/Role 命令不被 Meeting room gate 误拦截；
- 增加 Human 主持 E2E，包括任意对象 create/update 和无写入确认；
- 验证 blocked、retry、stale fence、return 和 abort。

阶段验收：Human 可在无专用表单、无 Plan/Step 的情况下，直接操作 Project View 并用一次
确认关闭 Meeting。

### 阶段三：Agent 同 Session 直接执行

目标：主持 Agent 使用参与会议的原槽和 Session 直接调用标准工具。

关键工作：

- ACP 增加 direct protocol discriminator 和 capability；
- 复用现有 slot/session hold，适配 direct action State；
- 替换 Materialization Intent prompt 为工具执行 prompt；
- 开放标准业务工具，保留精确 Board 注入；
- 处理 `COMPLETE|BLOCK|RETURN_TO_BOARD|ABORT`；
- COMPLETE 后持久化并提交精确 End；
- 删除 Materialization Intent parser、编译器、Project View materializer 和 step recovery ledger；
- 覆盖 affinity loss、deadline、格式修正、重启和重复 receipt 测试。

阶段验收：确定性 E2E 能证明 Board、Floor、工具调用和完成决定使用相同
`agent_index + acp_session_id`，并且没有发出 plan/step 事件。

### 阶段四：切换、回归与后端收口

目标：完成新 policy 的灰度门禁和完整后端验收。

关键工作：

- 扩展 Meeting 后端测试脚本和 capability probe；
- 增加 direct action metrics 与运维说明；
- 验证 migration 在发现旧 active Session 时 fail fast；
- 验证代码、CLI、NIP-11、配置和测试中均无旧 planned 执行入口；
- 使用真实 provider 完成一次有代表性的 smoke acceptance；
- 运行 Meeting 定向测试和仓库质量门禁；
- 更新后端 action design/operations 文档，使 direct v2 成为新建会议的当前模型；
- 保持 Desktop spec 未修改，记录其暂时仍描述旧 Plan/Step 路径。

阶段验收：新建会议默认路径不再依赖 Plan/Step，旧 planned runtime 已被移除，正常关闭、
abort、block、retry 和 return 均有确定性测试。真实 provider 只做一次签收，不建立无限验收
循环。

### 后续独立工作：Desktop 适配

后端四个阶段完成后，再修改 Desktop spec 和实现。预期适配方向是：

- 删除 Human Materialization Intent 表单、Plan 预览和 Step 进度；
- Human 从 Meeting 导航到已有 Project View 管理界面直接操作；
- 回到 Meeting 后只提供“确认行动产出已完成并结束会议”；
- 展示 frozen Board、action deadline、blocked/retry/return/abort 状态；
- 不展示 operation count 或“Relay 已验证全部行动”的误导文案。

该 Desktop 工作不属于本文后端交付。

## 16. 测试矩阵

### 16.1 SDK 与 parser

- direct Create/Board/begin/block/retry/return/End 使用正确 policy；
- direct action fence 不包含 `action-plan`；
- close from finalizing 必须携带 `attestation=actions-recorded`；
- direct parser 拒绝 plan、step-prepared、step-applied 和 complete；
- Relay 对旧 `moderated-board-actions-v1` 返回 unsupported policy；
- 未知、重复、缺失或多余 tag 被拒绝。

### 16.2 DB 状态机

- begin 只接受精确最终 Board 和 control fence；
- active run 唯一；
- runnable deadline 到期收敛到 blocked；
- blocked 不能 close，retry 后新窗口可 close；
- stale run/window/Board End 被拒绝；
- attested End 原子终结 run、Meeting 和 Channel；
- duplicate End 返回相同终态；
- return-to-board 打开新 Board window，外部效果不回滚；
- abort 在任意非终态收敛并保留外部效果；
- participant revocation 和 operator abort 继续 fail closed。

### 16.3 业务入口解耦

- `finalizing_actions` 期间可执行普通 Project View create/update/delete；
- 至少覆盖一个旧 adapter 不支持的操作，例如创建 Issue 或更新已有 Goal；
- direct action DB 不产生 plan、step 或 attempt row；
- Project View revision conflict 由 Project View 返回，不被 Meeting 重写；
- action completion 不要求 Project View 存在或 revision 增加。

### 16.4 ACP 连续性

- Agent 主持最终 Board、Floor 和 direct action 使用相同槽与 ACP Session；
- action prompt 注入精确冻结 Board；
- Agent 可调用普通 CLI，并在工具结果后返回 COMPLETE；
- Harness 不生成 Materialization Intent/Plan/Step；
- COMPLETE 只产生带 fence 的 End；
- slot/session mismatch 进入 affinity_lost block，不换槽执行；
- response 丢失重放同一签名 End；
- 非主持 Agent 不获得 action turn。

### 16.5 Human 路径

- Human 可先直接修改 Project View，再确认关闭；
- Human 可在没有任何外部写入时确认关闭；
- Human 不能编辑或提交 Plan JSON；
- 非主持人不能完成声明；
- deadline 后必须 retry 才能确认；
- return-to-board 明确确认 external effects preserved。

### 16.6 旧实现移除与发布门禁

- `0041` 在发现 planned v1 active Session 时中止；
- SDK、DB、Relay、CLI 和 ACP 不再包含 planned action 执行入口；
- 旧 capability、NIP-11 extension 和配置项不再声明或解析；
- direct v2 是唯一 action-capable Meeting policy；
- direct Create gate 默认关闭；
- 关闭 Create gate 不影响已存在 direct Session 的恢复和结束；
- NIP-11 runtime/create extension 与配置一致。

## 17. 主要代码影响面

| 模块 | 修改方向 |
|---|---|
| `buzz-sdk` | 新 policy、capability、direct fence、控制命令和 attested End builders |
| `buzz-db` | `0041` 清理旧 schema、最小 run/receipt、状态投影和终态 transaction |
| `buzz-relay` | 单一 direct parser、配置、NIP-11、指标和错误映射 |
| `buzz-cli` | direct status/control/confirm-recorded；复用普通 project-view/roles 命令 |
| `buzz-acp` | direct protocol、同 Session 工具 Turn、控制结果、恢复 ledger |
| `buzz-test-client` | Human、协议、旧 wire 拒绝与 direct action E2E |
| `scripts` | 后端 gate、capability probe 和一次真实 provider smoke |
| 后端文档 | direct v2 当前语义、运维和旧设计废止说明 |

`buzz-core` 不需要新增 event kind；如仅复用 kind `42112` 和 `42101`，只需保持现有 kind 注册。

## 18. 明确删除的旧实现

以下能力从代码和运行时中直接删除：

- `MaterializationIntent` 及其固定 Requirement/Works schema；
- `MeetingV2ActionItem`、`MeetingV2ActionPlan`、`MeetingV2ActionStep`；
- `MeetingV2ActionStepKind` 的封闭操作集合；
- Harness deterministic compiler；
- ACP/CLI Project View 专用 materializer；
- step write-ahead、step receipt 和 completion revision gate；
- Human plan 提交/修正和 plan JSON 入口；
- `planning/applying/ready_to_close` 状态；
- “只有零外部效果才能 return-to-board”的系统证明；
- `plan|apply|complete` CLI；
- plan/step Relay parser 与 metrics；
- 旧 step/attempt 数据表和约束；
- 旧 policy capability、Create gate、NIP-11 extension 和验收脚本分支。

不建立 `legacy` module，不保留 hidden CLI，也不保留只为旧测试服务的 adapter。通用 event
store 中可能存在的历史签名事件只作为不可执行原始记录，不构成旧 runtime。

## 19. 后端完成定义

满足以下条件后，才可认为本次后端修正完成：

1. 新建 action-capable Meeting 使用 direct v2 policy；
2. 最终 Board 后可进入独立 deadline 的 `finalizing_actions`；
3. Human 不提交 Plan/Step 即可直接完成行动收口并关闭；
4. Agent 在同槽、同 ACP Session 中直接使用现有工具，并由其自己作出完成判断；
5. Relay 只验证 Meeting 生命周期 fence，不限制 Project View 对象和操作类型；
6. 完成声明与 End 是一次主持操作和一个原子 Meeting transaction；
7. block、retry、return-to-board、abort、deadline 和 stale command 均能收敛；
8. 外部效果不回滚的边界在协议响应、CLI 和运维文档中明确；
9. planned v1 runtime、Plan/Step schema、materializer 和 CLI 已全部移除；
10. 迁移门禁、旧 wire 拒绝、定向测试和一次真实 provider smoke 通过。

完成上述后，再以此后端事实为基线修改 Desktop spec。Desktop 不再围绕 Action Plan/Step
设计，而只负责提供已有业务界面的导航、行动阶段状态和主持人的最终确认。
