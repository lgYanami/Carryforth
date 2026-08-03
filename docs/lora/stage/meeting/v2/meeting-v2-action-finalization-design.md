# Meeting V2：会议行动收口实现设计

> 状态：核心产品决策已确认，协议细节待阶段实现时冻结
>
> 日期：2026-08-03
>
> 基线：Meeting V2 后端交付提交 `16ef9d68631323fd5622efb003454d86fc3b1b7e`
>
> 范围：Meeting、Relay、DB、SDK、CLI、ACP 与 Project View 后端集成；不包含 Desktop、Web、Mobile 或其他前端。

## 1. 背景

Meeting V2 当前已经具备从创建、邀请、入会、讨论、看板维护、发言权传递到正常关闭或异常
终止的完整后端生命周期。当前正常关闭路径是：主持人在最终 Board Maintenance 后，于 Floor
Decision 中选择 `CLOSE`，ACP 随即提交 `End(outcome=closed)`。

这条路径能够记录会议结论，但不能表达以下常见结果：

- 会议决定新增一个 Requirement；
- Requirement 下需要拆出若干 Work；
- 部分参会者分别承接这些 Work；
- 主持人需要在会议结束前把这些决定物化到 Project View。

会议行动不能作为 `End` 之后由另一个新槽启动的普通 Agent Turn。那样只能复用主持 Agent
身份，不能保证复用真正参与会议的执行槽和 ACP Session，因而会丢失已经积累的会议上下文。

本设计把会议行动收口加入 `closed` 之前，作为 Meeting 生命周期中的最后一个可选非终态
阶段。

## 2. 已确认的核心决策

以下决策是本设计的前提：

1. 主持人在讨论过程中把会议目标、结论、行动项、承接人和可选外部上下文持续写入当前
   Board。
2. Board 仍由主持人维护，所有参会者按需读取；不增加 Board 历史版本、订阅或通知机制。
3. Board Maintenance、Floor Decision 与 Action Finalization 是三个独立阶段，分别使用
   独立 deadline。Action Finalization 可包含一个或多个同 ACP Session 的受限语义 Turn。
4. 只有主持人显式判断需要物化会议行动时，会议才进入行动收口；Project View 引用本身不
   触发任何写入。
5. Agent 主持的最终 Board Maintenance、Floor Decision，以及 Action Finalization 中所有需要
   模型理解或判断的 Turn，必须使用同一槽、同一 ACP Session；仅复用相同 Agent pubkey 不足
   以满足上下文连续性。计划冻结后的纯机械 event apply/replay 不启动另一个 Agent Turn。
6. 行动执行完成后才提交 `End(outcome=closed)`。`End` 成功前，会议仍未关闭。
7. 行动阶段只物化会议决定，例如创建 Requirement、Work 并设置责任归属；它不执行这些
   Work 本身，也不表示承接人已经完成或接受了 Work。
8. Meeting 不增加权限、审批或承接确认模型。写入继续使用主持 Agent 现有身份和 Project
   View 的既有命令路径。
9. Project View 始终是可选项。没有可执行收口操作的会议仍可沿现有路径直接关闭。

## 3. 术语

### 3.1 行动项

`Action Item` 是会议形成的后续责任记录，表达“哪位参会者接下来需要做什么”。不是每个
参会者都必须有行动项，主持人也可以是承接人。

行动项首先是 Board 中的会议内容。Meeting 不要求固定 Board 模板，也不通过代码解析某个
Markdown 标题来推断行动项。

### 3.2 收口操作

`Closing Operation` 是主持人在正常闭会前立即完成的外部物化操作。例如：

- 创建 Requirement；
- 创建处理该 Requirement 的 Work；
- 把 Work 的责任归属设置到承接人当前承担的 Role。

行动项和收口操作不是一回事。行动项描述会后要做的工作；收口操作只负责把这个决定写入
承载系统。Board 可以有行动项而没有任何收口操作。

### 3.3 物化意图

`Materialization Intent` 是主持 Agent 在最终 Board 已冻结后，结合 Board 和权威外部状态
形成的有界结构化语义结果。它表达“会议决定要在什么承载系统中创建或关联什么，以及由谁
承接”，但不包含 event、revision、重试、receipt 等执行机制。

物化意图由参与了本次会议的主持槽、同一个 ACP Session 生成。Relay 和 Harness 不从 Board
正文推断该语义，也不以规则程序替代主持 Agent 的判断。

### 3.4 执行计划与技术步骤

`Action Plan` 是 ACP Harness 对已经通过 schema 校验的 Materialization Intent 做确定性编译
后得到的内部执行清单。它包含稳定的 `action_id`、有序 `step_id`、预分配目标对象 ID、承接
人和 typed 目标操作，用于持久化恢复、去重和关闭校验。

Plan 和 step 都不是 Board 内容，也不是 Requirement、Work 等 Project View 对象。Harness
只能按固定 adapter 规则展开主持 Agent 已表达的意图，不能新增、删除或改写会议语义。

### 3.5 行动物化

`Materialization` 是把行动计划中的收口操作应用到目标系统，并取得可验证 receipt 的过程。
首个实现只提供 Project View materializer；协议为以后增加其他 target adapter 保留边界。

## 4. 范围与非目标

### 4.1 本次包含

- 新增带行动收口能力的 Meeting V2 policy/capability；
- 在正常关闭前增加 `finalizing_actions` 权威阶段；
- 主持 Agent 的同槽、同 ACP Session 连续执行；
- 主持 Agent 从最终 Board 生成结构化物化意图；
- Harness 将物化意图确定性编译为执行计划并冻结；
- Project View 的 Requirement、Work 和 Work responsibility 物化；
- 跨 Meeting 与 Project View 写入的幂等、部分成功恢复和关闭 gate；
- Human 主持人的后端/CLI 操作路径；
- 后端测试、可观测性和灰度能力声明。

### 4.2 本次不包含

- 任何前端页面、交互或通知；
- 会议类型、会议模板、投票、主持权转移或动态 roster；
- 强制 Board Markdown 格式或从 Markdown 做规则解析；
- Board 历史版本管理；
- 新的权限、审批、授权委托或承接人接受流程；
- 在会议生命周期内真正完成被分配的 Work；
- 自动创建 Role、Role Assignment 或 Work Commitment；
- Project View 之外的第二个 materializer；
- 对既有 `moderated-board-v1` 活跃会议做原地协议升级；
- 外部写入的全局事务或自动回滚。

## 5. 与既有 Meeting V2 的关系

现有 Meeting V2 文档和阶段四、阶段五交付记录描述的是
`v=3 + policy=moderated-board-v1`。它们明确把 Project View 写回排除在外，并规定 `CLOSE`
直接产生 End。这些记录是已经交付的历史事实，不应被静默改写。

本设计增加新的协议变体：

```text
v=3
policy=moderated-board-actions-v1
capability=meeting-v2-action-finalization-v1
```

兼容规则如下：

| 协议 | 正常关闭语义 |
|---|---|
| `moderated-board-v1` | 保持现状，Floor `CLOSE` 后直接 End |
| `moderated-board-actions-v1`，无收口操作 | Floor `CLOSE` 后直接 End |
| `moderated-board-actions-v1`，有收口操作 | Floor `FINALIZE_ACTIONS` → 行动物化 → End |

新 policy 仍属于 Meeting V2 产品语义，但不会改变已经创建的旧 policy Session。待 Relay、CLI
和主持 ACP fleet 都声明新 capability 后，再单独决定是否把新建会议的默认 policy 切换到
它。

这里的 policy 是 wire 兼容代际，不是会议类型，也不要求发起者提前判断会议是否会产生
行动。完成部署后，所有新 Meeting V2 预期统一使用具备可选行动收口能力的新 policy；真正
走 `CLOSE` 还是 `FINALIZE_ACTIONS`，由主持人在会议末尾根据实际结果决定。兼容期的显式
选择只用于灰度和测试，不暴露为长期产品概念。

实现中不能把新 policy 仅作为旧 V2 常量的别名。ACP、SDK、Relay 和 DB 都需要独立的
`V2Actions` protocol discriminator，并把它贯穿 Create、Meeting view、State parser、ledger、
command parser、DB lock/load、End gate、recovery 和 revocation。由于所有参会 Agent 都必须
能解析 `finalizing_actions` State，rollout capability gate 覆盖完整 roster 的 Agent runtime，
不只检查主持 ACP。

### 5.1 对既有 Spec 的受限覆盖

对 `moderated-board-actions-v1`，本文是 [Meeting V2 Spec](./meeting-v2.md) 的规范性扩展；
两者冲突时，仅以下条款以本文为准：

- 生命周期可在 `closed` 前进入 `finalizing_actions`；
- 主持人的正常关闭可以先进入行动收口，而非立即 End；
- 主持人显式声明的 closing operations 可以在 Meeting 生命周期内写 Project View；
- normal close 可额外受已声明行动全部物化的 gate 约束。

原 Spec 的固定 roster、主持身份、Board 维护顺序、看板按需注入、Speech Grant、Human
priority、abort、安全边界和 Project View 可选性继续完整继承。Board、Speech 或 Project
View 引用本身仍不产生隐式外部效果。对 `moderated-board-v1`，原 Spec 和阶段四、阶段五记录
继续原样生效。

## 6. 生命周期

### 6.1 正常路径

```text
discussion
    ↓
moderator maintains Board and action items
    ↓
final Board Maintenance
    ↓
Floor Decision
    ├── CLOSE
    │     └── End(outcome=closed)
    │
    └── FINALIZE_ACTIONS
          ↓
       finalizing_actions / planning
          ↓
       finalizing_actions / applying
          ↓
       finalizing_actions / ready_to_close
          ↓
       End(outcome=closed)
```

`FINALIZE_ACTIONS` 不是 `CLOSE` 的别名，也不能在同一 Floor Turn 中顺手写 Project View。
Relay 接受它后，结束 Floor deadline，创建独立的 action deadline，并把 Session 转入
`finalizing_actions`。

### 6.2 权威状态

Meeting V2 当前 runtime phase 为：

```text
bootstrap_locked | board_pending | floor_ready | ended
```

新 policy 增加：

```text
finalizing_actions
```

行动 run 使用两个正交字段：

```text
action_phase     = planning | applying | ready_to_close
action_condition = runnable | blocked
```

`blocked` 不是独立 phase，因此不会丢失被阻塞前处于 planning 还是 applying。Meeting End
提交后，action run 审计 status 记录为 `completed_closed | completed_aborted`；零效果返回
Board 的 run 记录为 `returned_to_board`。Meeting 的终态仍由既有 `closed | aborted` 表达。

### 6.3 状态转换约束

| 当前状态 | 命令/结果 | 下一状态 | 说明 |
|---|---|---|---|
| `floor_ready` | `CLOSE` | `ended/closed` | 没有需要立即物化的收口操作 |
| `floor_ready` | `FINALIZE_ACTIONS`/`begin` | `planning/runnable` | 冻结最终 Board 引用并创建独立行动窗口 |
| `planning/runnable` | 合法 Action Plan | `applying/runnable` | 计划先持久化，之后才允许外部写入 |
| planning 或 applying，`runnable` | `BLOCK`、deadline 或服务端失败 | 同 phase、`blocked` | 保留计划和 applied steps |
| planning 或 applying，`blocked` | `RETRY_ACTIONS` | 同 phase、`runnable` | 创建新的独立行动窗口和 deadline |
| `applying/runnable` | 所有 required steps 已验证 | `ready_to_close/runnable` | 尚未结束 Meeting |
| `ready_to_close/runnable` | 合法 End | `ended/closed` | 正常终态 |
| planning 或 applying，且零 accepted/applied 外部效果、无不确定写入 | `RETURN_TO_BOARD` | `board_pending` | 封禁 prepared attempts，终结当前 run并修正 Board |
| 任意非终态 | 合法安全/主持 abort | `ended/aborted` | 保留已经发生的外部效果和 receipts |

未完成 required step 时，Relay 必须拒绝 `End(outcome=closed)`。部分成功、超时、ACP 不可用
或 materializer 错误都不能被伪装成正常关闭。

### 6.4 Board 冻结

进入 `finalizing_actions` 时，Relay 记录最终 `board_event_id`。行动计划必须绑定这个事件，
行动执行期间不再接受 Board Maintenance。

若计划尚未产生任何外部效果，主持人可以显式 `RETURN_TO_BOARD`，进入新的 Board window
修正结论。只要已有 step 成功应用，就不允许用返回讨论来掩盖部分外部效果；此时只能继续
协调剩余步骤，或明确 `aborted`。

“零外部效果”不能只看 `applied=0`。所有 prepared/published attempt 必须已有确定结果；响应
不明时先按 event ID查 receipt。`RETURN_TO_BOARD` 或 abort transaction 会把全部未 applied
prepared event ID标为 abandoned，Project View handler 若之后收到这些 exact event，必须按
该 durable fence 拒绝尚未 accepted 的迟到写入；此前已经 accepted 的 exact event 重放仍返回
原 receipt，避免破坏幂等语义。

合法 `RETURN_TO_BOARD` transaction 还会把当前 run 的 `terminal_status` 置为
`returned_to_board`、清除 action deadline、解除“每个 Session 只能有一个 active run”的部分
唯一约束，并原子创建新的 `board_pending` window；旧 run 不覆盖、不删除。

进入 `finalizing_actions` 也会冻结 speech-floor：新的 Intent、Human Request、Offer、ACK、
Grant、Speech、Handoff 和普通 Floor 命令均以稳定原因 `meeting_finalizing_actions` 拒绝。
行动阶段不是讨论和外部写入并行的旁路；如确需继续讨论，必须在零 accepted/applied 外部
效果且没有 in-flight/indeterminate attempt 时显式返回新的 Board window。

### 6.5 Floor 竞态与命令准入

模型输出的 `FINALIZE_ACTIONS` 与 wire `begin` 是同一个语义决定：ACP 解析私有 Floor 输出后
构建一个主持人签名的 `begin` event，不会先提交一个 Floor command 再提交第二次语义决定。

`begin` 与 Human Request、End 和其他控制命令在同一个 Meeting Session lock 下串行化：

- Human priority/Request 先提交成功，`begin` 以 stale/human-priority 结果拒绝；
- `begin` 先提交成功，随后到达的 Human Request 以 `meeting_finalizing_actions` 拒绝；
- 有活动 Offer 或 Grant 时不能 `begin`；
- 若 Floor 绑定当前 Decision Attempt，`begin` 原子把该 attempt 记为
  `completed/action_finalization`，冻结 Candidate Cohort，并使未选择 Intent/Handoff 只保留为
  历史记录；
- 无 candidate 的 Floor 可以在没有 Decision Attempt 时直接 `begin`；
- stale attempt、Board window、control epoch 或 State event 均不能进入行动阶段。

进入 `finalizing_actions` 后的准入矩阵为：

| 命令 | 结果 |
|---|---|
| `begin` 重放、合法 plan/step/block/retry/complete/return-to-board | 按 action run CAS 处理 |
| current Board/State/action status read | 允许 |
| `End(outcome=closed)` | 仅 `ready_to_close` 允许 |
| 主持人或既有安全主体的 abort | 允许 |
| Intent、Human Request、Offer/ACK/Grant、Speech/Yield、Handoff | receipt-backed 拒绝 |
| Recall、Board command、普通 Floor/Decision Attempt command | receipt-backed 拒绝 |
| 已在本地排队或运行的旧 Meeting Turn | 取消；迟到结果由 phase/window fence 丢弃 |

`CLOSE` 本身是主持人签名的“没有需要立即物化的收口操作”声明。Relay 不解析 Board 来证明
这个判断；一旦主持人改为声明 `FINALIZE_ACTIONS`，则必须满足行动关闭 gate。

## 7. Board、物化意图与执行计划

### 7.1 Board 仍是会议内容来源

主持人在讨论期间可以按自己的表达方式记录：

- 已达成的结论；
- 后续行动内容；
- 承接参会者；
- 可选 Project View 上下文；
- 尚未解决的阻塞。

系统不提供会议模板，也不要求固定的“行动项”标题。参会 Agent 在 Intent 和 Speech 等既有
节点继续按需读取最新 Board。

### 7.2 不解析任意 Markdown

Relay 和 ACP Harness 不使用正则或固定标题从 Board 自动提取 Project View 命令。最终
Floor 选择 `FINALIZE_ACTIONS` 后，由同一个主持 ACP Session 在新的 Action Finalization
阶段理解最终 Board，并通过一个或多个有界语义 Turn 输出、必要时修正严格 JSON 物化意图。
主持 Agent 是 Board 语义到结构化语义的转换者；Harness 只在意图通过校验后按固定规则编译
技术执行计划。

这样既保留 Board 的自由表达，也避免恢复时重新解释自然语言而重复创建对象。

每个 Action Finalization 语义 Turn 都按 action run 冻结的 `board_event_id` 做 exact read，而不
读取“查询时碰巧最新”的 Board。event 缺失、signer/policy 不符或返回其他 Board ID时 fail
closed；同一 ACP Session 的记忆不能替代这次权威注入。

### 7.3 最小物化意图模型

首个 Project View adapter 的物化意图至少包含：

```json
{
  "version": 1,
  "board_event_id": "<hex>",
  "target": "project_view.v2",
  "requirement": {
    "title": "支持批量导出",
    "description": "<optional>"
  },
  "works": [
    {
      "title": "实现批量导出 API",
      "description": "<optional>",
      "assignee_pubkey": "<participant-pubkey>"
    }
  ]
}
```

该 schema 表达主持 Agent 的语义决定，不要求它生成技术 ID 或 Project revision。实现阶段
需要为 Work 数量、单字段和总 content 设置有界上限，但具体数值不在本设计中冻结。

### 7.4 Harness 编译出的执行计划

Harness 校验物化意图、roster 和 adapter 后，确定性生成并提交以下内部计划：

```json
{
  "version": 1,
  "action_run_id": "<stable-id>",
  "board_event_id": "<hex>",
  "items": [
    {
      "action_id": "<stable-id>",
      "summary": "实现批量导出 API",
      "assignee_pubkey": "<participant-pubkey>"
    }
  ],
  "steps": [
    {
      "step_id": "<stable-id>",
      "action_id": "<stable-id-or-null>",
      "kind": "project_view.create_requirement",
      "target_object_id": "<uuid-v4>",
      "payload": {}
    }
  ]
}
```

`action_run_id` 由 Relay 在 `begin` 时确定；`action_id`、`step_id` 和对象 ID 由 Harness 按
固定编译规则分配并在首次提交后保持不变。Harness 不调用模型来生成这份技术拓扑，也不从
Board 重新推断缺失语义。

### 7.5 意图与计划约束

- `action_id` 在同一 action run 内唯一，并在 step retry 中保持稳定；返回 Board
  后的新 run 由 `action_run_id` 明确隔离，可保留相同的 Board 展示标签；
- `action_run_id` 绑定一次 `FINALIZE_ACTIONS` 进入的行动收口尝试；
- `step_id` 在同一 action run 内唯一且稳定；
- Materialization Intent 和 Action Plan 中的 `assignee_pubkey` 必须来自 Create 时冻结的
  roster；
- 不是每个 roster participant 都需要出现在 `items` 中；
- `FINALIZE_ACTIONS` 的 v1 plan 至少包含一个 item 和一个 closing-operation step；主持 Floor
  原本应在无 step 时选择 `CLOSE`。若进入 planning 后才发现计划为空，则拒绝空 plan并通过
  零写入 `RETURN_TO_BOARD` 重新决策；
- `steps` 是确定顺序，不能由重试重新排序或重新生成对象 ID；
- Action Plan v1 的每个 step 都是 required，不支持 optional step；
- 两个 create step 不能复用同一个目标对象 ID；create Work 与随后设置该 Work responsibility
  的 step 可以引用同一 Work ID；
- Project View 对象 ID 在第一次写入前预分配并持久化；
- 合法 plan 一旦被 Relay 接受即不可修改；
- Relay 只校验 Materialization Intent/Action Plan 的结构、引用和执行结果，不判断 Board 与
  意图的自然语言语义是否完全一致；该语义责任属于主持 Agent；
- Harness 编译结果必须可追溯到已接受的物化意图，不能自行补充 Requirement、Work、承接人
  或其他业务决定。

该行动台账是外部写入恢复记录，不是 Board 版本历史。

若需要改变行动内容、承接人、step 拓扑、payload 或目标对象，必须先修改 Board：零外部效果
且没有 in-flight/indeterminate attempt 时走 `RETURN_TO_BOARD`；已经存在 accepted 外部效果时
只能继续原计划或明确 abort。初版不提供在冻结 Board 后静默改写会议决定的 `revise-plan`。

## 8. ACP 同槽、同 Session 连续性

### 8.1 当前能力不足

当前 `AgentPool::try_claim_inner` 只做两遍选择：优先选拥有该 channel session 的空闲槽，否则
选择任意空闲槽。这是 best-effort affinity，不能保证最终 Floor Turn 之后的行动 Turn 使用
同一个槽。

每个 `OwnedAgent` 又独立拥有自己的 `AcpClient` 和 `SessionState`；即使两个槽使用同一个
Agent pubkey，它们也不是同一个 ACP Session。因此行动收口不能只记录 Agent 身份。

### 8.2 连续性键

ACP 本地为可能成为最终收口周期的主持控制机会建立以下绑定：

```text
(community runtime generation, meeting_id, moderator_pubkey)
    → (agent_index, acp_session_id)
```

绑定元数据从 Board Maintenance Turn dispatch 时开始。模型产生合法 Board command 后，ACP
先暂时保留该槽再发布命令；只有 Relay 权威 State进入对应 `updated | unchanged` 的
`floor_ready`，Floor Decision 才通过 `claim_exact` 使用同一个槽和 ACP Session。Board
command 被拒绝或窗口失效时释放 hold。若 Floor 继续讨论，临时绑定在其命令收敛后释放；若
Floor 返回 `FINALIZE_ACTIONS`，它升级为 action lease。

因此真正的最终周期满足：

```text
final Board Maintenance
    └── same agent_index + same acp_session_id
        Floor Decision
            └── same agent_index + same acp_session_id
                Action Finalization
```

### 8.3 Slot Lease

pool 增加三级本地占用：

- `FinalControlCycleHold`：从 Board 成功返回持续到对应 Floor 结果收敛；
- `PendingActionLease`：Floor 返回 `FINALIZE_ACTIONS` 后、发布 `begin` 前建立的 exclusive
  continuity guard；
- `MeetingActionSlotLease`：`begin` 被 Relay 接受并由权威 State确认后，持续到 Meeting 终态。

三者都遵循：

- 通用 `try_claim`、普通 Meeting claim 和其他 Meeting 均跳过 leased slot；
- 后续 Board、Floor 或 Action 只能通过 `claim_exact(agent_index, meeting_id)` 取得它；
- claim 后核对 channel 对应的 `acp_session_id` 与 lease 完全相同；
- 仅 continuity-preserving cancel 成功时，action deadline 取消物理 in-flight Turn 后槽可以
  回到 leased idle，逻辑 lease 与原 Session 仍保留；
- `End` 或 `Abort` 被 Relay 接受后才释放 action lease 并允许清理 Meeting ACP Session；
- 行动期间禁止该 channel 的主动 session rotation、隐式 invalidation 或换槽 fallback。

Board/Floor 结果到 hold/lease 建立必须是本地主循环中的原子交接。当前成功 Turn 会先把
`OwnedAgent` 放回 pool，再把输出交给 Meeting Coordinator；实现时需要调整为：

1. `PromptResult` 带回 `agent_index` 和实际 ACP Session ID；
2. Coordinator 在 pool 可再次 claim 之前解析 action-capable V2 Board/Floor 结果；
3. 合法 Board 结果先安装 `FinalControlCycleHold`，再把 Agent 放回 held idle slot；
4. Floor 必须 exact claim 该槽；Floor provider 返回后，槽仍保持在当前 claim 中；
5. 若 Floor 输出 `FINALIZE_ACTIONS`，先核验 session 未被结果后 rotation/invalidation 改变，
   再把当前 claim 升级为 `PendingActionLease`；核验失败时不得提交 `begin`；
6. 建立 pending lease 后，ACP 构建并提交同一个语义决定对应的 `begin` command；
7. 只有同步到匹配的权威 `finalizing_actions` State 后才升级为 durable action lease；
8. `begin` 被 Human priority、stale fence、End 或其他合法竞态拒绝时，立即释放临时 hold 并
   重新同步；其他 Floor 结果按其既有提交结果释放或终结 hold。

若 `begin` 响应不明确，ACP 保持临时 hold，先查询权威 State并精确重放同一个签名 event，
不能把“不知道是否已接受”当作拒绝而提前归还槽。

同理，Final Control Cycle 的 Board/Floor Turn 结束时不能立即执行当前的
max-turn/max-request session invalidation；先把 rotation 标记为 deferred，待确认该控制周期
不会进入行动收口后再执行。否则模型刚完成最终 Board 或作出 `FINALIZE_ACTIONS`，其 ACP
Session 就可能在 hold/lease 创建前被销毁。

`RETURN_TO_BOARD` 不释放 action lease，而是把它降级为同一槽的
`ModeratorMeetingBinding`；新的 Board/Floor 继续走 exact claim。这样不会出现普通 Meeting
claim 跳过 leased slot、而返回后的 Board 又无法取得槽的死锁。只有 Meeting 进入
`closed | aborted` 才最终释放绑定。

lease/hold 不是 `try_claim_inner` 的一个局部过滤条件。pool 需要提供
`claimable_unleased_count`，并让普通 claim、Meeting Board claim、Offer/Grant reservation
floor、Coordinator `available_agent_slots`、Heartbeat、panic recovery、respawn、runtime remove
和 terminal cleanup 全部识别 leased slot。只有 exact owner claim 可以消费该槽，否则容量
统计会把不可用槽错误报告为可用。

Relay 不感知本地 slot index 或 ACP Session ID。Relay 负责权威 Meeting phase 和关闭 gate；
同槽连续性由 ACP 自身保证，并通过 observer evidence 和测试证明。

### 8.4 模型与 Harness 的职责

同一 ACP Session 负责：

1. 理解最终 Board；
2. 读取当前 Project View 的必要权威状态；
3. 形成或修正结构化 Materialization Intent；
4. 在语义冲突时基于返回结果决定下一步。

Materialization Intent 通过校验后，ACP Harness 按固定 adapter 规则把它编译为 Action Plan
和技术 steps；机械性的 Nostr event 构建、签名、精确重放和 receipt 登记也继续由 Harness
完成，沿用现有 Board/Floor 的模式。Harness 不解释 Board、不补充会议决定，不是另一个
Agent，也不会启动另一个槽。

同一 ACP Session 不能在中途安全替换 system prompt：当前 system policy 只在
`session/new` 安装，而 channel Session 还可能先由主持人的 participant/self Turn 创建。因此
新 policy 从该 Meeting 的首次 Agent Turn 起安装统一的 action-capable Meeting system policy，
再由每次 user turn envelope 的严格 `turn_kind` 分支约束输出：

- participant Intent/Speech 继续使用 advisory 读语义；
- moderator Board/Floor 继续禁止外部持久写入；
- 只有 `action_finalization` turn kind 可以生成受 schema 约束的 Materialization Intent；
- Action Plan 和技术 steps 只能由 Harness 根据该 intent 确定性编译；
- 任意 Board 或讨论内容都不能改变当前 turn kind、输出 schema 或 typed materializer 边界。

实现可以新增统一的 `meeting_v2_actions_prompt.md`，并复用现有各 prompt 的具体规则片段；不能
在保留同一 ACP Session 的同时声称切换到另一个 Action system prompt。

某槽在首次处理 `V2Actions` Turn 前若已经有使用其他 policy contract 的 channel Session，
必须在它参与本 Meeting 前显式轮换并安装统一 policy；这次边界轮换不冒充会议上下文连续。
一旦该 Session 已参与 Final Control Cycle，就受 hold/lease 保护，不再允许隐式轮换。

ACP 提交 `begin` 后不能仅凭写响应在本地猜测 phase 已切换。只有重新同步到 Relay 发布的
权威 `finalizing_actions` State，且其中的 action run、Board 和 control fence 都匹配，才可
在 leased slot 上 dispatch Action Finalization 的首个语义 Turn。

### 8.5 连续性丢失

以下情况视为 `affinity_lost`：

- 原槽进程退出并被新进程替代；
- runtime generation 改变；
- channel 对应 ACP Session ID 消失或变化；
- provider 要求不可恢复的 session reset；
- exact claim 得到了其他 slot。

ACP 必须停止新的语义执行，不能静默创建新槽、新 Session，再仅靠重新注入 Board 冒充上下文
连续。若 plan 尚未冻结，或后续步骤需要新的语义判断，则把 Meeting 置为
`finalizing_actions`、`action_condition=blocked`；只有下面定义的冻结计划机械路径可以继续
保持 `applying/runnable`。

连续性丢失后的边界取决于计划是否已经冻结：

- Materialization Intent 尚未生成，或其编译出的 Action Plan 尚未被 Relay 接受：必须停止，
  等待原 Session 恢复或 abort；
- Action Plan 已冻结：Harness 可以继续不需要模型判断的确定性物化，包括从固定 plan、对象
  ID和当前 Project revision 构建/签名 typed event，或精确重放 prepared event；
- revision rebase 只有在 desired operation、对象缺失/关系、assignee evidence 都未改变时才是
  机械动作；一旦当前态出现多个合法选择、角色映射变化或语义冲突，立即 durable block；
- 机械 executor 不启动新 Agent 槽或模型 Turn，也不能修改 plan；它只是完成参与会议的原
  Session 已冻结的决定。

若 provider 明确支持恢复同一 ACP Session ID，也可以在核验后恢复语义 Turn。否则 Agent
主持会议不能切换为 Human 接管；只有 moderator 原本就是 Human 的 Session 才使用 Human
主持路径。

## 9. 独立 deadline

一次最终收口至少有三份互不借用的预算：

```text
Board deadline
    ↓ terminal Board outcome
Floor deadline
    ↓ FINALIZE_ACTIONS accepted
Action deadline
    ↓ plan/apply/complete
End submission budget
```

关键规则：

- Board 超时不消耗 Floor 时间；
- Floor 超时不自动进入行动阶段；
- Action deadline 只在 Relay 接受 `FINALIZE_ACTIONS` 后创建；
- Action 超时不自动 normal close；
- End 提交保留独立的短提交余量；
- 同一 action attempt 的重复 State sync 只能保持或提前本地 hard deadline，不能延后；
- 超时后 Relay 保留持久行动进度，ACP 只重试尚未验证的 step。

当前普通 clean cancel、idle timeout 和 hard timeout 可能失效 channel Session 或 respawn 整个
Agent，不能直接用于需要连续性的 Action Finalization。实现需要新增
`continuity_preserving_cancel`：只有 provider 明确保证取消后原 ACP Session 仍可继续，且取消
后核验 session ID 不变，才把 action condition 置为 blocked 并允许后续 `retry` 重新进入语义
Turn。

若 provider 不支持这种取消、取消后 session 消失，或 hard timeout 触发了 respawn，则立即记
为 `affinity_lost`。此后只允许执行已冻结 plan 的确定性机械 apply/replay、等待可验证的原
Session 恢复，或 abort；需要模型判断时必须 blocked，不能承诺在新 Session 上执行
`RETRY_ACTIONS`。

具体 deadline 数值、最大尝试次数和退避参数在实现阶段结合现有 timing profile 决定。

## 10. Meeting 行动协议

### 10.1 Event-first

该能力继续使用 Nostr command，而不增加专用 HTTP endpoint。建议在 Meeting kind 范围增加：

```text
KIND_MEETING_ACTION_COMMAND = 42112
```

它由主持人签名，使用严格的 `h`、`v`、`policy`、`action` 和 expected-state/window tags。
初版 action 为：

- `begin`：消费合法 Floor control，进入 `finalizing_actions/planning`；
- `plan`：提交并冻结结构化 Action Plan；
- `step-prepared`：在目标发布前登记已签名 Project View event 及其 expected revision；
- `step-applied`：引用已接受的目标系统 command receipt；
- `block`：以受限 reason code 把当前 run/window 权威置为 blocked；
- `complete`：声明所有 required step 已物化；
- `retry`：从 `blocked` 创建新的 action attempt/deadline，保留原 plan 和 applied steps；Agent
  路径还要求原 ACP Session 经核验仍存活；
- `return-to-board`：仅在零 accepted/applied 外部效果、无 in-flight/indeterminate attempt，且
  未 accepted attempts 已在同一事务标记 abandoned 时撤销 run并打开新 Board window。

`End` 继续使用既有 kind，但新 policy 的正常 End 校验需要区分：

- 从 `floor_ready` 直接关闭，表示主持人没有声明收口操作；
- 从 `finalizing_actions/ready_to_close` 关闭，必须引用当前 active action run、plan event 和
  window fence，并确认所有 required step 已验证。

`block` 供 ACP Harness/Human CLI 报告 Relay 无法自行观察的 materializer 失败，必须携带当前
run/window 和 expected plan-event fence（planning 尚无 plan 时显式为 none），以及低基数
reason code，例如：

- `project_view_v2_unavailable`；
- `assignee_unresolved`；
- `object_id_conflict`；
- `responsibility_conflict`；
- `provider_failure`；
- `affinity_lost`。

deadline worker、Project View 原子 ingest 或 Relay validator 已经在服务端确定失败时，可以在
同一 DB transition 内部设置 blocked，无需伪造 moderator event。两条路径都必须产生相同的
权威 State/receipt；没有 durable blocked 状态时，`retry` 不得被接受。

### 10.2 `begin` 校验

Relay 原子校验：

- Session 使用 `moderated-board-actions-v1`；
- signer 是冻结的 moderator；
- 当前 phase 是 `floor_ready`；
- 当前 Board outcome 是显式 `updated | unchanged`，不是 `timed_out | preempted`；
- 没有活动 Offer 或 Grant；
- 没有 Human priority 或尚未处理的 Human Request；
- Candidate Cohort/Decision Attempt 要么不存在且没有候选，要么由 `begin` 的 expected attempt
  精确绑定并在同一事务内完成；
- 没有可绕过当前 Floor 的活动 Directed Handoff 直接路径；
- expected control epoch、Board window、State event 和 Board event 均为当前值；
- 没有另一个 action run。

成功后 Relay 转换 phase、冻结 Board 引用、创建 action deadline，并发布新的权威 State。

### 10.3 `plan` 校验

Relay 校验计划 schema、大小、唯一 ID、roster 承接人、step 顺序、target adapter 和预分配对象
ID。计划成功持久化前，ACP materializer 不得启动目标写入；计划外的 Project View command
也不能登记为本次行动 receipt 或帮助越过关闭 gate。

同一签名 event 重放返回原结果；同一 run 只接受第一个合法 plan event。不同 plan event 在
该 run 已冻结后以 conflict 拒绝。

### 10.4 `step-prepared` 与 `step-applied` 校验

每次 Project View 写入尝试都先完成 write-ahead：Harness 或 Human CLI 构建并签名 event 后，
先提交 `step-prepared`，待 Relay 持久化 exact event ID、签名 event 和 expected Project
revision，再向 Project View 发布。这样即使进程在目标写入成功后、登记 receipt 前退出，也能
找到并精确重放或核对此 event。

`step-prepared` 在落库前验证内嵌 event 的 ID、签名、moderator signer、Community、Project
View command schema、expected revision、对象 ID和当前 plan step 完全一致。它只登记
write-ahead intent，不在该 Meeting transaction 中执行 Project View mutation。

`step-applied` 必须引用目标 command event ID、目标对象 ID和返回 Project revision。Relay 从
Project View receipt/event store 读取权威结果，并校验：

- command 已被当前 Community 的 Project View 接受；
- command signer 是 Meeting moderator；
- command 类型、对象 ID、关系和计划 step 相符；
- 同一 receipt 没有被绑定到不相干 step；
- step 尚未由冲突结果完成。

ACP 的成功响应本身不能越过这个 Relay 校验。

### 10.5 `complete` 与 End gate

Relay 只在全部 required step 都是 `applied` 时接受 `complete`，并转为 `ready_to_close`。
随后同一个主持身份提交 `End(outcome=closed)`。

`complete` 还绑定一次 verified Project View snapshot/revision，确认 Requirement、Work、handles
关系和 responsibility 在该 revision 上形成最终投影，并把 `completion_project_revision` 写入
run。之后正常的 Project View 变更不会反向重开 Meeting；End 只校验这个已持久化完成事实。

End DB transaction 原子地把 Meeting 置为 `closed`、归档 channel，并把当前 action run 记为
`completed_closed`；abort transaction 对应记录 `completed_aborted`。不存在一个 live
`completed` action phase。

行动完成与 End 仍然分成两个持久动作，原因是：

- action receipt 落库成功但 End 响应丢失时可以安全重放 End；
- Relay 能明确区分“外部效果已完成”和“Meeting 已终结”；
- End 的既有 channel archive、Baton 清理和 outbox 逻辑不需要混入 Project View 事务。

## 11. Project View Materializer

### 11.1 初版支持范围

首个 adapter 实现需求讨论场景：

1. 创建一个 Requirement；
2. 创建一个或多个 Work；
3. 每个 Work 通过 `handles` 指向且只指向该 Requirement；
4. 按行动项承接人设置 Work responsibility。

当前 Project View 的目标由 Relay 所属 Community 决定，不存在由 Meeting 任意指定的
`project_id`。因此初版 materializer 总是写 Meeting 所属 Community 的当前 Project View；
Board 中的 Project View 引用只提供讨论上下文，不改变目标路由。

Project View v2 是该 materializer 的 run-level 硬前置条件，因为 RoleAssignment、
WorkResponsibility 和可验证 change receipt/source 都属于 v2。计划 preflight 必须确认当前
Community 已初始化且权威 schema 为 v2，并取得完整 verified v2 snapshot。v1、未初始化、
disabled 或 snapshot 无法验证时，在任何 `step-prepared` 前进入
`blocked/project_view_v2_unavailable`；不能降级为 v1 create 后跳过 responsibility。零外部效果
时可以返回 Board，否则只能等待能力恢复或 abort。

Meeting policy 本身不要求 Project View v2：没有 Project View materialization 的会议仍然按
普通 V2 生命周期运行。

Project View 仅是可选 materializer：

- Board 没有 Project View 引用仍然是合法会议；
- Board 有 Project View 引用不表示必须写入；
- 只有主持人显式选择 `FINALIZE_ACTIONS` 并提交 Project View plan 才会写入；
- 普通讨论、Intent、Speech、Board Maintenance 和 Floor selection 都不能隐式写入。

初版不自动更新已有对象、不删除对象，也不创建 Issue、Role、Assignment 或 Commitment。
这些可作为后续 operation kind 扩展，不改变 Meeting 生命周期。

### 11.2 “承接人”的数据映射

Project View 当前不把 Work 直接分配给 member pubkey。正确关系是：

```text
participant pubkey
    ↓ active RoleAssignment
stable Role
    ↓ WorkResponsibility
Work
    ↓ handles
Requirement
```

因此 materializer 在任何写入前执行 preflight：

1. 确认 `assignee_pubkey` 在冻结 roster 中；
2. 查询该 pubkey 的 active Role Assignment；
3. 记录解析得到的 `role_id` 和 `assignment_id`；
4. 将 Work 的 `responsible_role_id` 设置为该 Role；
5. 在 Meeting action ledger 中保留原 `assignee_pubkey` 和解析证据。

初始 preflight 只能防止明显的计划错误，不能把角色映射永久缓存。每个
`set_work_responsibility` step 发布前，都必须在该 command 使用的同一 Project revision
snapshot 上重新确认
`assignee_pubkey → assignment_id → role_id` 仍与计划一致；Relay 登记 receipt 时也按 accepted
revision 验证这份映射。验证与 mutation 由同一个 expected Project revision fence 连接。

正常关闭证明的是“责任设置被接受时，会议承接人与 Role Assignment 的映射有效”。之后发生
的正常 Role 换人不会反向使 Meeting 重新打开；Work 随稳定 Role 延续是 Project View 自身的
领域语义，Meeting ledger 保留当时的 participant、assignment 和 Role 作为审计证据。

主持人不能替承接人创建 `WorkCommitment`。Commitment 表示 Role 当前承担者后来主动接受 Work，
属于会议关闭后的 Work 执行生命周期，不是会议正常关闭条件。

若承接人没有可解析的 active Assignment，preflight 失败并保持 `action_phase=planning`、
`action_condition=blocked`；系统不自动发明 Role 或绕过 Project View 领域约束。

### 11.3 写入顺序

当前 Project View 每条 mutation 都单独推进 Project revision，没有“Requirement + 多个 Work
+ responsibility”的原子批量命令。materializer 按固定顺序执行：

```text
read verified Project View snapshot and revision R
    ↓
preflight every assignee and relation
    ↓
persist plan, step IDs and UUID-v4 object IDs
    ↓
create Requirement at R
    ↓
create Work 1 at R+1
    ↓
assign Work 1 responsibility at R+2
    ↓
create Work 2 at R+3
    ↓
assign Work 2 responsibility at R+4
    ↓
re-read and verify final projections
```

实际 revision 必须来自每个 accepted receipt，不能假定总是严格等于示意数字。

### 11.4 Typed 写入路径

Agent-facing 写入继续经过 `buzz-cli`/SDK 的 typed Project View command，不增加旁路数据库
写入。为支持幂等恢复，需要补齐两个小能力：

- `project-view create` 接受可选的显式 `--id <uuid-v4>`；未提供时保持当前自动生成行为；
- verified object/status read 返回 Work 当前的 `responsible_role_id` 以及 verified projection
  source/change ID，供恢复核对；
- Project View create 和 responsibility command 把 Relay `message` 内的 receipt 解析并验证为
  typed compact result，顶层至少输出 event ID、operation、object/work ID、responsible Role
  和 accepted Project revision；malformed receipt 必须 fail closed，不能只凭 `accepted=true`
  把 step 标为成功。

Action Finalization 的同 Session 语义 Turn 产生或修正 Materialization Intent；Harness 先
按固定 adapter 规则将 intent 编译为 Action Plan，再由 materializer 使用这些 typed 能力构建
并签名 Project View 事件。它不会让模型通过自由文本 shell 自行拼接未审计命令。

## 12. 持久化与幂等

### 12.1 为什么需要独立行动台账

Project View 已支持“完全相同 Nostr event”的幂等重放，但当前 create 命令每次都会生成新
UUID 和新 event。若只在超时后重新解释 Board，就可能重复创建 Requirement 或 Work。

因此 Meeting 需要持久化行动执行状态。这是跨两个领域的 saga ledger，不是 Board 版本管理。

### 12.2 数据结构

新增迁移建议为 `0040_meeting_v2_action_finalization.sql`，至少包含：

```text
meeting_v2_action_runs
  community_id
  session_id
  action_run_id
  plan_event_id
  board_event_id
  control_epoch
  action_window_epoch
  action_phase
  action_condition
  terminal_status
  completion_project_revision
  action_deadline
  last_error_code
  created_at / updated_at

meeting_v2_action_steps
  community_id
  session_id
  action_run_id
  action_id
  step_id
  step_order
  step_kind
  desired_payload
  assignee_pubkey
  resolved_role_id
  resolved_assignment_id
  target_object_type
  target_object_id
  accepted_project_revision
  status
  last_error_code
  attempt_count
  created_at / updated_at

meeting_v2_action_step_attempts
  community_id
  session_id
  action_run_id
  step_id
  action_window_epoch
  attempt_number
  project_command_event_id
  signed_project_event
  expected_project_revision
  accepted_project_revision
  status
  error_code
  created_at / updated_at
```

核心唯一约束：

```text
action_runs:
  PRIMARY KEY (community_id, session_id, action_run_id)
  PARTIAL UNIQUE (community_id, session_id) WHERE run is active

action_steps:
  UNIQUE (community_id, session_id, action_run_id, step_id)
  UNIQUE (community_id, session_id, action_run_id, step_order)

action_step_attempts:
  UNIQUE (community_id, session_id, action_run_id, step_id, attempt_number)
  UNIQUE (community_id, project_command_event_id)
    WHERE project_command_event_id IS NOT NULL
```

`RETURN_TO_BOARD` 会终结当前零写入 run；主持人以后再次选择 `FINALIZE_ACTIONS` 时创建新的
`action_run_id`，旧 run 作为审计记录保留。

每个 run 的 `action_window_epoch` 从 1 开始并单调递增；`retry` 创建新 deadline 时推进它。
所有 action command、prepared attempt、模型结果和 End 都携带 expected run/window 和
plan-event fence，旧窗口的迟到结果只能得到稳定 stale receipt。

目标对象 ID 在任何 Project View command 发布前生成并落库。对象仍使用 Project View 要求的
UUID v4，不能改成不被当前 validator 接受的确定性 UUID 版本。

step attempt 保存完整签名 event 是 write-ahead 与精确重放所需的恢复材料。它采用与 ACP
prepared command 相同的敏感数据处理标准，不进入普通日志或开放状态投影。

### 12.3 恢复算法

每个 step 按以下顺序执行：

1. 读取 Meeting action step；
2. 若已 `applied`，直接跳过；
3. 若存在 prepared attempt，先按准确 event ID查询 Project View receipt；
4. accepted receipt 的 signer、operation、subject、对象 ID、关系和 result 全部匹配时，登记
   `step-applied`；
5. receipt 不明确时读取 verified projection source/change ID，只在它指向同一个 prepared
   event 且能回查 accepted receipt 时补记；
6. 不存在 prepared attempt 时，根据 step kind 和权威当前态决定是否创建新的签名 attempt；
7. 新签名 event 在网络发布前先通过 `step-prepared` 持久化；
8. accepted 后记录准确 event ID、实际 Project revision 和结果，再进入下一 step。

仅仅“当前对象内容看起来相同”不足以证明它由本 Meeting action 产生，不能据此补记 receipt。
verified Project View read 因此需要暴露 projection source/change ID，而不只返回对象正文和
`responsible_role_id`。

不同 step kind 的权威当前态处理不同：

| step kind | 当前态 | 行为 |
|---|---|---|
| create Requirement/Work | 目标缺失 | 用预分配 UUID v4 构建并 prepare create |
| create Requirement/Work | 目标存在且 source 是本 step accepted event | 依据 exact receipt 补记 applied |
| create Requirement/Work | 目标存在但来源不同 | `blocked/object_id_conflict`，即使内容相同也不覆盖 |
| assign responsibility | Work 缺失 | `blocked/missing_dependency`，不发布 assign |
| assign responsibility | Work 存在且无目标 responsibility | 重新验证 assignee mapping 后 prepare assign |
| assign responsibility | 已是目标 Role 且 source 是本 step accepted event | 依据 exact receipt 补记 applied |
| assign responsibility | 已是目标 Role 但来源不同 | `blocked/provenance_mismatch`，不冒认 receipt |
| assign responsibility | 已是其他 Role | `blocked/responsibility_conflict`，不覆盖 |

revision conflict 不等于语义失败。materializer 重新读取权威 revision，确认目标 step 尚未
应用后，以相同目标对象和 desired payload 重签新的 revision-bound event。若冲突改变了计划
语义，有活的原 ACP Session 时把结果送回该 Session 决策；Session 已丢失时 durable block，
不得由 Harness 猜测。

### 12.4 跨领域事务边界

Meeting DB transaction 不能跨越模型 Turn 或 Project View 网络/event 写入。实现采用 saga：

- Relay transaction 原子持久化 Meeting phase、plan 和 step 状态；
- 每个 Project View mutation 独立提交并产生 receipt；
- Meeting action command 把经过 Relay 验证的 receipt 绑定到 step；
- normal End 以全部 step 已验证为 gate。

“独立提交”不表示 Project View ingest 可以先检查后写。对于 event ID命中 prepared action
attempt 的 Project View command，同一个 PostgreSQL transaction 必须：

1. 锁住对应 action run、step 和 attempt；
2. 判断 run/window/plan event 仍适用且 attempt 未 abandoned；
3. 执行 Requirement/Work 或 responsibility mutation；
4. 写入 Project View receipt；
5. 把 attempt 标记为 `target_accepted`。

`RETURN_TO_BOARD` 和 abort 锁同一坐标。因此竞态只能收敛为“目标先提交，控制
命令看到已有外部效果”或“控制命令先 abandoned，目标 ingest 被拒绝”，不能在检查与 mutation
之间穿透。Project object command 与 Role responsibility command 两条 DB 写路径都必须接入。

已经 accepted 的 exact event 在 run 返回 Board或终态后重放，仍返回原 Project View receipt；
只拒绝尚未 accepted 的迟到 prepared event，以保持 Project View 现有 event 幂等语义。

初版不做自动补偿删除。自动回滚可能删除已经被其他人引用或继续修改的对象，比保留明确的
部分结果更危险。

## 13. 失败与恢复语义

| 失败 | 行为 |
|---|---|
| Board/Floor 已过期 | 拒绝 `begin`，不产生外部写入 |
| Materialization Intent 格式或语义错误 | 保持 `planning`，同一 Session 可在独立预算内修正；Harness 不猜测补全 |
| assignee 无 active Assignment | preflight 阻塞；零写入时可返回 Board |
| Project revision conflict | 权威重读、核对、仅重试当前 step |
| Project command 响应丢失 | 重放同一 event，或按固定对象 ID核对 |
| 部分 step 成功后失败 | 保留 receipts，仅执行剩余 step；不能直接 closed |
| action deadline 到期且 continuity-preserving cancel 成功 | 保持 durable progress 和 blocked；核验原 Session 后可 retry |
| action deadline 导致 Session invalidation/respawn | `affinity_lost`；禁止新的语义 retry |
| slot/session continuity 丢失 | 禁止新语义执行，不换槽降级 |
| ACP/Relay 重启 | Relay 恢复 action state；仅精确重放已冻结决定，或核验同 Session 后继续 |
| 主持人判断无法完成 | `End(outcome=aborted)`，保留部分外部效果审计 |
| operator/security abort | 继续沿用既有强制 abort 路径 |

`aborted` 不自动撤销已经创建的 Project View 对象。State、CLI 和审计结果应能显示 Meeting
在第几个 step 终止，供后续人工处理。

## 14. Human 主持路径

协议不能因为没有 ACP slot 而让 Human 主持的会议卡死。后端和 CLI 提供同一组能力：

- 查看最终 Board 与 action status；
- 提交或修正结构化 plan；
- 执行下一 pending Project View step；
- 核对并记录 receipt；
- blocked 时显式 retry 并创建新的行动预算；
- complete 后提交 normal End；
- 零 accepted/applied 外部效果、无 in-flight/indeterminate attempt 时返回 Board，或随时显式
  abort。

Human 路径不适用“同槽、同 ACP Session”约束；该约束只保证 Agent 主持的模型上下文连续性。
它仍必须由 Create 时冻结的同一 Human moderator 身份签名，普通 Human 参会者或 operator
不能借此接管 Agent 主持。前端如何呈现这些操作在后续单独设计。

## 15. 权限前置假设

按已确认的当前产品约定，Community Agent 可以修改 Project View。本阶段不新增、重构或讨论
权限、审批和授权继承；主持 Agent 使用自己的 Community 身份，经既有 typed handler 执行。

实现前仍需做一次代码前置核对：当前 Project View 路径包含 managed Agent active Assignment
检查，设置 Work responsibility 还存在 governor fence。如果这些既有校验与上述产品约定不
一致，应在 Project View 侧先行对齐；这不是 Meeting 生命周期要新增的权限分支，也不能通过
Meeting 旁路绕过。

## 16. ACP Ledger 与本地恢复

现有 Meeting ledger 需要升级一个版本，新增：

- active `FinalControlCycleHold`、`PendingActionLease`、`MeetingActionSlotLease` 或
  `ModeratorMeetingBinding`；
- bound `agent_index`、ACP `session_id` 和 runtime generation；
- action run ID、plan event ID和 action window epoch；
- 已签名未提交或结果不明确的 Meeting/Project View events；
- 当前 step、hard deadline 和有限重试状态。

每种 hold/lease 记录都绑定 runtime generation、agent incarnation、agent index、ACP Session ID、
Board window/event 和当前 control/action fence。恢复时必须重新核验所有字段；只恢复“记录”而
找不到同一个活 Session 不构成连续性。

若进程在 final Board 已接受、Floor 尚未执行时重启，并且无法验证原 hold/session，ACP 不得
在新 Session 上执行一个可能返回 `FINALIZE_ACTIONS` 的 recovered Floor。Relay 既有 Floor
deadline/fallback 仍正常收敛该窗口：它可以按既有规则选择 eligible Human/self/普通 Intent
speaker 或进入 idle，但不能替代主持人声明 close 或 action finalization。控制之后返回主持人
时，必须经过新的 Board → Floor control cycle 并重新建立有效 hold，才允许收口；也可以等待
原 Session 可验证恢复或显式 abort。初版不通过换槽或重做旧 Floor prompt 冒充同一最终控制
周期。

持久化规则沿用当前 `0600` 私有 ledger。因为精确重放需要，ledger 可能暂存有界的结构化行动
payload 和完整签名事件；这些内容不能进入普通日志、observer payload 或 metrics label。

Relay State 前进到已验证结果后及时清理对应 prepared event；Meeting 终态后清理 lease 和
非必要执行内容，但保留 Relay/DB 的 durable audit records。

## 17. 可观测性

建议增加低基数指标：

- `meeting_v2_action_command_total{action,outcome,duplicate}`；
- `meeting_v2_action_phase_transition_total{from,to,reason}`；
- `meeting_v2_action_step_total{kind,outcome}`；
- `meeting_v2_action_step_latency_seconds{kind,outcome}`；
- `meeting_v2_action_retry_total{reason}`；
- `meeting_v2_action_affinity_mismatch_total{reason}`；
- `meeting_v2_action_blocked_total{reason}`；
- `meeting_v2_action_close_gate_rejection_total{reason}`。

日志和 observer 可以包含用于本地关联的 Meeting/turn 信息，但 metrics label 禁止放入：

- Board 正文；
- 行动 summary 或 payload；
- pubkey、Meeting ID、对象 ID、event ID；
- Project View 的业务文本。

阶段五原有“Meeting 期间没有外部写入”的零不变量，需要在新 policy 下收窄为：行动执行器
的非计划写入、重复物化、错误 signer、错误 slot/session、End 后写入均为零。旧 policy 的
零外部写入不变量保持不变。

## 18. 关键代码改动面

### 18.1 `buzz-core` / `buzz-sdk`

- 注册 Meeting Action command kind；
- 增加新 policy/capability 常量；
- 为 `V2Actions` 增加独立 protocol discriminator，不能复用并改义旧 V2 常量；
- 增加 begin、plan、step-prepared、step-applied、block、complete、retry、return-to-board
  builders；
- 扩展新 policy 的 End builder 与严格 wire fixtures；
- 为 Materialization Intent 和内部 Action Plan 分别定义有界 typed schema。

### 18.2 `buzz-db`

- 扩展 Meeting V2 runtime phase CHECK；
- 新增 action run/step schema、CAS 和 receipt lookup；
- 在共享 Baton command 的 Session row lock 入口读取 V2 runtime；`finalizing_actions` 时统一拒绝
  所有非 action、End 和既有安全 abort 命令；
- normal End 增加 action completion gate；
- 把 action deadline 合并进现有 due claim/`next_action_at`，并让 sweeper、lazy recovery、
  restart recovery 处理 action window；
- abort 保留 action execution audit。

### 18.3 `buzz-relay`

- Create/End protocol discriminator 支持新 policy；
- Meeting View、State parser、recovery、revocation 和 capability advertisement 传播
  `V2Actions`；
- 新 action command 走既有 Community/Nostr ingest pipeline；
- 发布包含 action run ID、window epoch、phase、condition、plan event 和进度计数的权威
  Meeting State；
- 校验 Project View receipt 与 plan step 的一致性；
- Project View ingest 对命中 prepared action event ID 的 command，在与 mutation/receipt 同一 DB
  transaction、同一锁顺序下校验 run/window/attempt；拒绝尚未 accepted 的 abandoned/迟到
  event，但 accepted exact replay 仍返回原 receipt；普通 Project View command 路径不变；
- 保持 channel archive 只发生在 End commit 后。

### 18.4 `buzz-acp`

- Floor output 增加 `FINALIZE_ACTIONS`；
- 当前新 policy 下不再把该结果直接映射为 End；
- pool 增加 exact-slot claim、logical lease 和 generic claim 排除；
- 记录并核验 ACP Session ID，行动期间抑制主动 rotation；
- 新增统一 action-capable Meeting system policy、Action Finalization turn kind 和 prepared
  action；
- 把新 turn kind 接入 pending/requeue、Board-read capacity 和 dispatch 的所有穷举分支；每个
  Action Turn 按 run 冻结的 `board_event_id` 精确读取 Board，读到其他最新 Board 也必须拒绝；
- 增加 Project View preflight/materializer 与同 Session 冲突反馈；
- ledger 升级并保留精确重放能力；
- terminal/preemption/restart 路径释放或阻塞 lease，不能泄漏槽。

### 18.5 `buzz-cli`

- 提供仅用于灰度/测试的 protocol override；稳定后新 V2 Meeting 默认具备可选行动能力，不把
  它呈现为会议类型；
- `meetings actions status|plan|apply|block|retry|complete|return-to-board`；
- Project View create 增加可选显式对象 ID；
- verified Work read 补齐 responsibility 与 projection source；
- 为 Project View write receipt 补齐经解析验证的 typed compact 输出，包括 event ID、
  operation、object/work ID、responsible Role、revision 和 action status。

## 19. 阶段开发规划

### 阶段一：协议与权威状态

交付：

- 新 policy/capability、kind、SDK builders 和 fixtures；
- DB migration、`finalizing_actions` 状态机和 action tables；
- Relay begin/plan/complete/retry/return-to-board/End gate；
- State query 与 CLI status；
- 不接真实 Project View 写入的状态机集成测试。

阶段一完成后，可以证明“有行动的会议不会提前 closed”，但尚不能由 Agent 自动物化。

### 阶段二：同槽同 Session 的主持 Action Finalization

交付：

- exact-slot lease 与 pool 隔离；
- `FINALIZE_ACTIONS` Floor 输出和独立 action deadline；
- 统一 action-capable system policy 下的 `action_finalization` turn envelope 与严格 intent 输出；
- Harness 的 Materialization Intent → Action Plan 确定性编译；
- continuity mismatch fail-closed；
- ledger 恢复和 observer 证据。

阶段二完成后，可以证明最终 Board、Floor 和物化意图由同一个槽和 ACP Session 连续完成，
技术计划由同一 Harness 确定性编译。

### 阶段三：Project View Materializer

交付：

- Requirement → Work → responsibility 的 typed 写入；
- roster assignee → active Role Assignment 的 preflight；
- 显式 UUID-v4 对象 ID、逐 revision 执行与 verified re-read；
- Project View create/responsibility 的 typed receipt 与 projection-source read；
- step receipt 验证、部分成功恢复和重复执行去重；
- Human/CLI apply 路径。

阶段三完成后，需求讨论会议可以把决定可靠物化到当前 Community Project View。

### 阶段四：恢复、运维与收口验收

交付：

- timeout、restart、partial success、abort 和 affinity loss 场景；
- sweeper/lazy recovery 与低基数 metrics；
- V0、V1、旧 V2 policy 全量 regression；
- 新 policy 确定性三 Agent 端到端测试；
- 一次有界的真实 provider 手动验收记录；
- 覆盖完整 Agent roster 的 capability gate 和默认关闭的 rollout 开关。

阶段四只完成后端资格判断；前端设计和开发仍单独排期。

## 20. 验收矩阵

### 20.1 生命周期

- 无 Project View、无收口操作的会议仍可直接 closed；
- Board 引用了 Project View 但主持人选择 `CLOSE` 时没有写入；
- `FINALIZE_ACTIONS` 后 Meeting 保持 active/non-terminal；
- required step 未完成时 normal End 被拒绝；
- 所有 step 完成后 normal End 成功并归档 channel；
- action 阶段 abort 产生 `aborted`，不冒充 closed。

### 20.2 上下文连续性

- 最终 Board、Floor 和 Action Finalization 所有语义 Turn 的 `agent_index` 完全相同；
- 最终 Board、Floor 和 Action Finalization 所有语义 Turn 的 ACP Session ID 完全相同；
- plan 冻结后的机械 apply/replay 不启动新 Agent Turn；
- leased slot 不会被普通 channel、Heartbeat 或其他 Meeting claim；
- slot 忙时等待，不换槽；
- session rotation、进程 replacement 或 mismatch 时不 dispatch 新语义 Turn；
- final Board 后重启且无法恢复原 hold/session 时，不在新 Session 上 dispatch finalizing Floor；
- `RETURN_TO_BOARD` 后同一 binding 能 exact claim 新 Board，不发生 lease deadlock；
- End 接受后 lease 确定释放。

### 20.3 Project View

- Project View 未初始化、不是 v2、disabled 或 verified snapshot 失败时零写入并 blocked；
- 创建一个 Requirement 和多个 handles 它的 Work；
- 只有被记录行动项的部分参会者获得 Work responsibility；
- responsibility 指向承接人 active Role，不伪造直接 member assignee；
- 不替承接人创建 Commitment；
- revision conflict 后只重试当前 step；
- 响应丢失和进程恢复不产生重复 Requirement、Work 或 responsibility；
- target ingest 与 return/abort 竞态只能收敛为 accepted receipt 或 abandoned reject；
- accepted exact event 在 Meeting 终态后重放仍返回原 receipt；
- 同一固定对象 ID出现不一致内容时 fail closed，不覆盖。

### 20.4 兼容与安全

- `moderated-board-v1` wire、close 和零外部写入行为不变；
- V0/V1 不识别新 action command；
- 普通参会者不能 begin、提交 plan、登记 receipt 或 complete；
- stale Board/window/control epoch 的 action command 被拒绝；
- Board 文字不能改变 action schema、signer、roster 或 target adapter；
- 未计划写入、End 后写入、错误槽执行和 duplicate materialization 均为零。

## 21. 完成定义

本能力只有同时满足以下条件才算后端开发完成：

1. 新 policy 的正常生命周期可以走通
   `discussion → final Board → FINALIZE_ACTIONS → materialize → End(closed)`；
2. 主持 Agent 的最终 Board、Floor、Materialization Intent 生成和必要修正发生在同一槽、
   同一 ACP Session；Action Plan 只由 Harness 对该 intent 确定性编译；
3. Requirement、Work 和责任 Role 与冻结的结构化 Action Plan 一一对应；Board → Intent 的
   语义忠实度仍由主持 Agent 判断，不伪装成 Relay 或 Harness 可以验证的自然语言不变量；
4. 所有重试、响应丢失和允许的恢复路径都不会重复物化；
5. 未完成行动不能 normal close，异常终止不能伪装为 closed；
6. 无行动和无 Project View 的 Meeting 路径不受影响；
7. 旧 Meeting policy 与 V0/V1 regression 全部通过；
8. 自动化验收有明确边界，真实 provider 验收采用一次有界手动签收，不进入无限循环。

## 22. 留待阶段实现时决定的细节

以下内容不影响本设计边界，可在相应阶段开发前冻结：

- action/End 的具体 deadline 数值和 retry backoff；
- Materialization Intent/Action Plan 的最终字段名、字节上限和最大 Work/step 数；
- CLI 参数的最终命名；
- blocked action 的运维展示形式；
- 新 policy 何时成为默认；
- 后续是否支持 Issue、更新已有对象或其他 materializer。

这些细节不得改变三个核心不变量：行动属于 Meeting 关闭前的生命周期、Agent 路径必须同槽
同 ACP Session、Project View 仍然只是可选 materializer。
