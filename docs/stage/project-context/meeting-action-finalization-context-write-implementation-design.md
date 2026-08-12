# Meeting Action Finalization 中维护 Project Context 的实现设计

> 状态：Context 写回语义已实现；物理 affinity 条款已被逻辑主持人 ACK 设计取代，current
> `7/4/meeting-context-v3` 生命周期迁移实施中
> 日期：2026-08-08  
> 范围：Meeting / Project Context Relay 与 DB、ACP System Contract、逐 Turn Meeting Envelope、CLI、Desktop、测试与发布  
> 前置设计：[Meeting 作为 Project Context 坐标与 Community 可见性实现设计](meeting-coordinate-implementation-design.md)、[Project Context](project-context.md)、[Meeting Project Context TODO](TODO.md)

## 1. 结论

会议的项目物化发生在主持人的 `action_finalization` Turn。若物化创建或修改了 Project View、
Project Document 等具有长期上下文价值的坐标，主持人应在**同一个 Action Finalization Turn、同一逻辑
主持 Agent 身份**下：

1. 读取目标的 canonical 状态；
2. 按冻结 Board 完成 Project View / Document 等业务写入；
3. 创建或更新用于解释关系的普通 Project Document；
4. 显式建立包含当前 Meeting 与物化结果的 Project Context Edge；
5. 回读 canonical Edge 后再返回 `COMPLETE`。

为使第 4 步可执行，Meeting Coordinate 的新建关联条件从“仅终态 Meeting”放宽为：

```text
verified terminal Meeting
OR
verified active Meeting whose runtime_phase = finalizing_actions
```

这不是自动物化，也不是 Relay 根据变更日志推断关系。Edge 的语义仍由 Human / Agent 显式选择，
Project Document 仍负责解释“这些坐标为什么相关”。

本方案明确不引入：

- 新的 Meeting 生命周期状态；
- Context Manifest；
- 关闭后的后台补写任务；
- 与 current ActionRunKey 并行的第二个 Agent Turn；
- Project Context schema / coordinate / edge-key 版本；
- Relay 自动推断、自动创建或自动扩展 Edge；
- Project View、Document 与 Project Context 的跨领域分布式事务；
- 因 Context 写入回滚已经完成的外部物化。

## 2. 问题与设计动机

当前两个正确但组合后冲突的约束是：

1. 只有主持人的 `action_finalization` Turn 可以使用业务写工具；
2. Meeting Coordinate 只有在 Meeting 已经 `ended` 后才能 attach。

正常关闭顺序是：

```text
冻结最终 Board
  -> action_finalization
  -> 主持人确认 actions recorded
  -> Meeting End / Channel archive
```

因此，若 Edge 需要包含当前 Meeting，主持人在唯一合法的业务写窗口中反而无法 attach；Meeting End 后，
主持 Action Turn 已结束，也不应再创建另一个 Agent Turn 或一个仅凭摘要工作的后台任务。Action Turn
可以由同一逻辑主持 Agent 的任意健康槽执行，但必须从 frozen Board 与 canonical envelope 自包含地取得
全部协议输入。

`finalizing_actions` 已经具备建立稳定来源关系所需的事实边界：

- frozen roster 不再变化；
- 最终 Board 已冻结；
- Discussion Floor、Intent、Offer、Grant 与 Speech 已停止推进；
- 主持人只能物化冻结 Board 中已经决定的动作；
- Relay 已有独立 Action Run、lease、BLOCK / Retry、RETURN_TO_BOARD 和终止语义。

所以无需再增加一个重复表达“会议已冻结”的状态。正确的最小改动是让 verified
`finalizing_actions` Meeting 成为**可 attach 的 Meeting Coordinate**。

## 3. 核心语义

### 3.1 Meeting attachability

为 prospective Meeting Coordinate 定义统一判定：

```text
meeting_attachable(meeting) =
  verified_terminal(meeting)
  OR verified_finalizing_actions(meeting)
```

其中：

```text
verified_terminal =
  session.status = ended
  AND normalized terminal_outcome IN (closed, aborted)
  AND Create -> State -> End evidence chain valid

verified_finalizing_actions =
  session.status = active
  AND session.schema_version = 3
  AND session.floor_policy_version = moderated-board-actions-v3
  AND channel.room_kind = meeting
  AND runtime.runtime_phase = finalizing_actions
  AND one current non-terminal Action Run exists
  AND Action Run references the current frozen Board
  AND Action Run control_epoch / board_window equal Runtime control_epoch / board_window
  AND the current Relay-signed Meeting State records the same action transition
```

不得依据 Desktop 文案、Agent 自述、单条 Nostr Event、缓存中的 Meeting summary 或客户端传入的 phase
判定。Relay 必须在 Context 写事务中锁定并读取 canonical Meeting Session、Runtime、Action Run 和 Board
状态。

下列 active phase 仍不可新建关联：

- `bootstrap_locked`；
- `board_pending`；
- `floor_ready`；
- 任何 legacy / 未知 / 证据不完整的 active 状态。

### 3.2 可 attach 不等于 Meeting 行动授权

Project Context 写入继续复用现有 Community / Project Context 权限：当前有效 Community member 可以按
既有规则提交 Context mutation。Meeting roster、主持身份、Role、Assignment、Action Run lease 和 ACP
affinity 不进入 Project Context 的权限判断。

这里的“Assignment 不进入权限判断”不表示忽略调用者主动提交的 attribution。沿用现有严格规则：普通
Community 写入可以不携带 Assignment / Runtime fence；managed Agent 一旦显式声明
`acting_assignment_id`，Relay 仍必须验证该 Assignment 与 Runtime fence，非法声明不能静默降级为普通
Community 写入。该校验验证 attribution 的真实性，不是取得 Context 权限的前置来源。

`finalizing_actions` 只回答“这个 Meeting 的正式讨论记录是否已经稳定到可以成为坐标”，不回答“调用者
是否有权主持会议”。主持人的 Action Finalization 恰好是本流程的正常执行者，但不是唯一拥有 Context
写权限的身份。

这保持以下边界不变：

- Community member 可读取 Meeting、Project View、Document 和 Context；
- frozen roster 继续控制 Meeting 参与、发言、主持和 Action Finalization；
- 非 Community member 不得通过 Meeting ID 或 Context mutation 枚举会议；
- 知道 Meeting UUID 不会获得任何额外权限。

### 3.3 Edge 表达关系，不表达会议成功

Meeting Coordinate 表示一场可回看的项目会议记录。Edge 表示坐标之间存在由 Context Document 解释的
关系，不自动表示：

- Meeting 已成功关闭；
- Board 中的结论必然正确；
- 所有物化动作均已成功；
- 关联内容已经成为项目共识；
- `aborted` Meeting 的内容应被当作正向结论。

因此，在 Action Finalization 中建立 Edge 后，即使随后发生 `RETURN_TO_BOARD`、`BLOCK` 或 `ABORT`，
Relay 也不自动 detach 或回滚 Edge。Project View / Document 的外部写入同样不会被 Meeting 事务回滚。

为减少错误关系，主持人的规定顺序是：

1. 先完成并回读业务物化；
2. 再写解释文档和 Context Edge；
3. 最后返回 Action Finalization 结果。

如果 attach 后才发现 Board 必须改变，主持人应在能力允许时先更新 Context Document 或 detach 不再准确的
binding，再返回 `RETURN_TO_BOARD`。无法修正时应如实 `BLOCK`；不得伪造回滚成功。

### 3.4 什么情况下必须写 Context

ACP 合同采用以下操作语义：

- 冻结 Board 要求创建或修改持久 Project View / Project Document 坐标，并且这些结果来源于本次会议时，
  主持人必须显式维护包含当前 Meeting 的 Context；
- 已有准确 Edge 时使用幂等 attach / 必要的 Document revision，不制造重复 Edge；
- Board 明确没有外部物化结果时，不为“形式完整”伪造对象、Document 或 Edge；
- 只有单一对象的普通字段修改、且不存在值得长期解释的跨坐标关系时，仍应把 Meeting 与该对象及解释
  Document 建立最小合法关系，而不是把会议来源丢失；
- Context Document 应解释来源、决定、适用范围和必要的失败/部分完成状态，不复制完整 Speech 或 Board。

这是 Agent 的平台操作合同，不是 Relay 对其他写入的自动推断。当前 Direct Action output 不携带完整
mutation receipt，因此 Relay 不尝试通过观察“是否写过 Project View”来自动阻止 `COMPLETE`。若未来需要
密码学可验证的“所有物化结果均已纳入 Context”，必须单独设计结构化 output receipt，不在本次范围内。

## 4. 生命周期与并发

### 4.1 状态矩阵

| Meeting canonical 状态 | 新 attach | 已有 Edge | 说明 |
|---|---:|---:|---|
| active / discussion phases | 拒绝 | 保留 | 正式记录仍可变化 |
| active / `finalizing_actions` | 允许 | 保留 | Board 与正式讨论已冻结 |
| `finalizing_actions` / Action Run blocked | 允许 | 保留 | blocked 不改变已冻结事实；权限仍由 Context 规则决定 |
| concurrent End 后 verified `closed` | 允许 | 保留 | 走 terminal resolver |
| concurrent End 后 verified `aborted` | 允许 | 保留 | Inspector 必须显示 outcome / reason |
| RETURN_TO_BOARD 后恢复 discussion phase | 拒绝新的 attach | 保留 | 不自动撤销先前外部效果或 Edge |
| invalid terminal evidence | 拒绝 | 保留但 hydration 可降级 | 不猜测终态 |
| missing / foreign Meeting | 拒绝且无侧信道 | 保留既有稳定坐标记录 | detach 不依赖 live resolver |

“已有 Edge 保留”是 durable provenance 语义。Meeting 暂时回到 discussion phase 不删除 Context history；
后续查询应显示当前 Meeting 生命周期状态，让读者结合 Context Document 判断。

### 4.2 锁与提交时复核

沿用当前 Project Context write coordinator：

1. 获取 Community / Project Context mutation scope；
2. 按既有 Meeting 锁顺序锁 `meeting_sessions`，再锁对应 Channel；
3. 对 active Meeting 继续锁 Runtime、当前 Action Run、current Board 与当前 Meeting State；
4. 在同一事务内重新计算 attachability；
5. 通过后才写 binding、Edge projection 与 Context revision。

并发结果必须是确定的：

- attach 先获得 Meeting lock：按看到的 verified `finalizing_actions` 提交；后续 End 或
  RETURN_TO_BOARD 等待，Edge 保留；
- End 先提交：attach 等待后按 verified terminal 路径通过；
- RETURN_TO_BOARD 先提交：attach 等待后看到 discussion phase，拒绝且不产生部分写入；
- Meeting 状态在客户端预检与 Relay 提交之间变化：只以 Relay 提交事务中的复核为准。

Meeting End / Return 路径不得反向获取 Project Context lock。若以后引入反向写入，必须重新审查锁图，
不能在本设计上直接叠加。

## 5. Relay 与数据库实现

### 5.1 统一 resolver 结果

扩展 `../../../crates/buzz-db/src/meeting.rs` 的 `MeetingCoordinateResolution`：

```rust
MeetingCoordinateResolution =
  Terminal { ... }
  | FinalizingActions {
      meeting_id,
      state_revision,
      state_event_id,
      board_event_id,
      action_run_id,
      control_epoch,
      board_window,
    }
  | Active
  | OrdinaryChannel
  | MissingOrForeign
  | InvalidTerminal
```

`FinalizingActions` resolver 必须验证 Session / protocol、Runtime、Action Run、current Board、
`control_epoch`、`board_window` 与 Relay-signed current Meeting State / action transition 的完整 parity。
任一行缺失、版本不符或指针漂移都不能降级为普通可 attach 状态。

这些字段只用于 Relay 校验、诊断与测试，不进入 coordinate identity，也不进入 Edge key。resolver 仍是
security-neutral；调用者必须先完成 Community write authorization，才可把详细结果映射为外部错误。

### 5.2 Project Context attach

修改 `../../../crates/buzz-db/src/project_context.rs`：

- Attach 接受 `Terminal` 或 `FinalizingActions`；
- `Active` 返回不可 attach；
- `coordinate_active_in_tx()` 与专用 Meeting 分支共用同一 predicate，避免未来再次出现两个判断漂移；
- Detach 保持现状：只依据已持久化 exact set 与 binding，不调用 live Meeting resolver；
- Edge key、canonical coordinate ordering、Context revision 和 projection schema 均不变；
- 任一坐标失败时，整个 Context mutation 不写 binding、不写 projection、不推进 revision。

原错误 `invalid:project_context:meeting_not_terminal` 已不能准确表达新合同。全量迁移为：

```text
invalid:project_context:meeting_not_attachable
```

错误 detail 应说明允许的两种状态是 verified terminal 或 verified `finalizing_actions`，但不得向未通过
Community authorization 的调用者泄漏 Meeting 是否存在或处于何种 phase。不保留 V2 名称兼容分支；
CLI、Desktop、文档和测试在同一次交付中全部更新。

### 5.3 不新增协议与迁移

以下内容保持不变：

- `MeetingCoordinate { meeting_id }` wire shape；
- Meeting coordinate family rank / variant byte；
- Project Context schema 2；
- `buzz-project-context-edge-v2` capability；
- attach / detach command 与 projection event kind；
- 已有 Edge key、binding、Context revision 和 Document revision。

因此不需要 SQL 数据迁移、reprojection 或 bootstrap。实现不得清理数据库、重建 Community、重置
Project View / Document / Context 状态，也不得运行会指向主开发数据库的 destructive migration test。

## 6. ACP 提示词设计

### 6.1 两层上下文模型

采用已经存在的两层模型，不创建第三套提示词：

```text
ACP Session System Context
  - [Project Space]：稳定的项目资产与 Context 语义
  - [Meeting]：稳定的 Meeting 操作合同

每个完整 Turn
  - [Role Brief] / [Role Binding]
  - Relay-verified Meeting envelope：当前 turn_kind、控制坐标、deadline、tool policy、output schema
```

System 层回答“Meeting、Document、Project Context 是什么，以及 Action Finalization 的固定责任”；
逐 Turn envelope 回答“当前是哪一场 Meeting、现在允许什么操作、该如何返回”。动态 Meeting ID、Action
Run、deadline、Board event、Context revision 或完整图数据绝不能写入 System Contract。

### 6.2 Project Space System Contract

当前 `../../../crates/buzz-acp/src/project_space.rs` 的 Project Space Contract 版本为 `7`。本设计首次引入
Meeting Context 写回语义时由 `5` 提升为 `6`；后续逻辑主持人切换再次升至 `7`。稳定语义至少包含：

```text
A verified terminal Meeting, or an active Meeting whose formal discussion is
frozen in action_finalization, may be used as a Project Context coordinate.

During action_finalization, when frozen-Board decisions create or update durable
Project View or Project Document coordinates, explicitly maintain the explanatory
Project Context in the same turn, including the current Meeting coordinate.

Buzz never infers an Edge from a Meeting, a Board, an Action Run, or observed
materialization writes. If no durable output or meaningful relation exists, do
not fabricate one.
```

同时把当前所有“terminal Meeting coordinate”的绝对表述改成“verified terminal or
action-finalizing Meeting coordinate”。保留以下既有原则：

- Project Document 是解释正文；
- 按需查询，不把全图、完整 Board 或 Speech 注入每一 Turn；
- 显式写回；
- System prompt 不授予权限。

### 6.3 Meeting System Contract

当前 `../../../crates/buzz-acp/src/meeting_context.rs` 的 Meeting Contract 版本为 `4`。本设计首次交付时由
`2` 提升为 `3`；后续逻辑主持人切换再次升至 `4`。在
`Only the moderator's action_finalization Turn...` 之后加入操作规则：

```text
If materialization creates or updates durable project coordinates derived from
this Meeting, maintain their Project Context before COMPLETE. Use a normal
Project Document to explain the relationship, include the current Meeting
coordinate, and read the canonical Edge back. Perform these operations in this
same action_finalization Turn as the same logical moderator Agent. Physical
work-slot or ACP Session continuity with discussion Turns is not required.

Do not infer or invent an Edge when the frozen Board has no durable output. A
recoverable Context write or readback failure is BLOCK. If the Board itself is
insufficient or must change, use RETURN_TO_BOARD. Never claim COMPLETE for a
required Context update that was not canonically accepted.
```

该合同只规定工作流，不把 Context 命令提升成 Meeting protocol event，也不改变
`COMPLETE | BLOCK | RETURN_TO_BOARD | ABORT` 输出 schema。

### 6.4 逐 Turn Meeting envelope

`../../../crates/buzz-acp/src/meeting_v1.rs` 的 current envelope contract version 为：

```text
meeting-context-v3
```

本设计首次交付时完成了 `meeting-context-v1 -> meeting-context-v2`；后续逻辑主持人切换要求
`meeting-context-v3`，且不保留旧 envelope 作为 current runtime 分支。

所有 Meeting Turn 都继续注入当前 `turn_kind` 与 Relay-verified control。增加一个小型、闭合的
`project_context_policy` 控制块，避免 Agent 从 System 文本猜测当前窗口：

非 Action Finalization：

```json
{
  "project_context_policy": {
    "meeting_coordinate": {
      "type": "meeting",
      "meeting_id": "<verified meeting UUID>"
    },
    "project_context_writes_allowed_in_this_turn": false,
    "reason": "project_context_writes_not_allowed_in_this_turn"
  }
}
```

Action Finalization：

```json
{
  "project_context_policy": {
    "meeting_coordinate": {
      "type": "meeting",
      "meeting_id": "<verified meeting UUID>"
    },
    "project_context_writes_allowed_in_this_turn": true,
    "required_when_materialized_outputs_exist": true,
    "context_document_required_for_attach": true,
    "canonical_readback_required_after_context_write": true
  }
}
```

该块是 Harness 对当前 Turn 的 Project Context 操作约束，不是 Relay 授权凭证。非
Action Finalization 中的 `false` 不否定 `board_maintenance` Turn 返回合法 Board replacement；它只禁止
Project Context 与其他外部业务写，Board 仍由既有 Meeting output schema 和 Relay 负责发布。该块不注入：

- 当前 Context revision；
- 全部已有 Edge；
- Project View snapshot；
- Document body；
- Board 之外推断出的候选坐标；
- 预先生成的 Edge / Document 内容。

Agent 按需使用 canonical read 命令发现这些数据，避免注入后在同一 Turn 内迅速过期。

### 6.5 Action Finalization tool policy 与步骤

把 `V2_ACTION_FINALIZATION_TOOLS` 从只举例 `buzz project-view`、`buzz roles` 扩展为明确包括：

```text
buzz project-view
buzz documents
buzz project-context
buzz roles
```

Action Finalization prompt 中固定顺序：

```text
read canonical target state
  -> materialize exact frozen-Board decisions
  -> canonical readback of materialized objects/documents
  -> write or revise explanatory Context Document
  -> attach Meeting + materialized coordinates
  -> exact/incident canonical Context readback
  -> COMPLETE
```

结果映射：

| 情况 | 返回 |
|---|---|
| 所有必需写入和 Context 回读完成 | `COMPLETE` |
| Relay / tool 暂不可用、revision conflict、Context 写入或回读失败 | `BLOCK` |
| 冻结 Board 缺少必要决定、坐标关系本身需要重新讨论 | `RETURN_TO_BOARD` |
| 会议无法继续形成有效结果 | `ABORT` |

不得新增 `context_manifest`、`edge_ids` 或 mutation receipt 到 Direct Action output。Harness 继续只解析现有
四种动作，避免再次出现“Agent 已完成工具调用，但因输出 schema 不识别额外字段而被误判超时”的问题。

### 6.6 单一 Action Turn 与逻辑主持人调度

Context 写入属于现有 Action Finalization Turn 的业务工具调用：

- 同一个 ActionRunKey 最多只有一个 pending/running Action Turn；
- 优先复用已有 Meeting channel Session；没有、繁忙或自然轮换时，可使用同一逻辑主持 Agent 的
  其他健康槽；
- 无论使用哪个槽，都注入 current moderator、frozen Board、run/window/Board fence、Role/Project
  Context 与完整工具策略；
- 不在 Meeting End 后用聊天消息、另一个 Turn 或后台 worker 补写；
- 继续使用进程级 Action Run renewable lease 覆盖等待槽、执行和 ACK receipt 不确定窗口；lease
  不表示 Context 已写入或 Meeting 已完成；
- `COMPLETE` 只在业务坐标、解释 Document、Context Edge 与 canonical readback 都完成后返回，并由
  Harness 转换为显式 `actions-recorded` ACK。

这正是放宽 attach 时机的主要收益：Action Turn 已拥有当前 Meeting、冻结 Board、工具结果和 Project
状态，可以在同一逻辑工作窗口内完成关系治理；不需要把讨论阶段的物理 Session 提升为正确性前提。

## 7. CLI 与 Desktop

### 7.1 CLI

`buzz project-context attach` 的 Meeting token 与 JSON input 不变。更新：

- help：Meeting 可在 verified terminal 或 verified `finalizing_actions` 时 attach；
- 错误映射：使用 `meeting_not_attachable`；
- compact / JSON 输出不新增 coordinate 字段；
- Agent runbook 增加 Action Finalization 中 Document -> attach -> readback 示例；
- 不增加 `--force`、`--as-host`、Assignment、Runtime fence 或 lease 参数。

### 7.2 Desktop picker 与 Inspector

Project Context coordinate picker 应显示：

- verified terminal Meeting；
- verified `finalizing_actions` Meeting；
- 不显示其他 active Meeting。

状态必须来自 native / Relay 的 verified Meeting projection，不根据左侧栏 `In progress` 文案或本地计时器
推断。若现有 DTO 未暴露 runtime phase，扩展 Meeting summary / hydration DTO，但不把 phase 放入
coordinate identity。

Inspector 对 finalizing Meeting 显示：

```text
Lifecycle: Finalizing actions
Formal discussion and Board are frozen; Meeting closure is pending.
```

若它后来 `closed`、`aborted` 或 RETURN_TO_BOARD，Meeting live invalidation 只刷新节点 metadata；Edge key、
Context revision 和 graph layout identity 不因此变化。点击仍使用现有 `Open Meeting` 路由查看 Board 与正式
Speech。

本次不新增 Desktop 自动建 Edge。Human 主持人若需要手动维护 Context，继续使用现有 Project Context
写入能力；若 Desktop 当前只提供 read UI，则单独设计完整写入体验，不在本次借机扩张。

## 8. 测试方案

### 8.1 Domain / DB

至少覆盖：

1. terminal `closed` Meeting 可 attach；
2. terminal `aborted` Meeting 可 attach；
3. active `finalizing_actions` + current Action Run + frozen Board 可 attach；
4. `bootstrap_locked`、`board_pending`、`floor_ready` 均拒绝；
5. active 但缺 Runtime、Action Run、current Board 或 Relay-signed current State 拒绝为 invalid /
   not attachable；
6. Runtime 与 Action Run 的 `control_epoch` / `board_window` 不一致时拒绝；
7. Action Run 的 Board、current Board 与 current State action transition 不一致时拒绝；
8. ordinary Channel、missing、foreign、invalid terminal 保持精确且无侧信道的错误；
9. attach 任一坐标失败时 binding / projection / Context revision 均不变化；
10. detach 在 Meeting 已不可解析时仍成功；
11. existing Edge 在 RETURN_TO_BOARD 后仍可查询，不被自动删除；
12. `coordinate_active_in_tx()` 与 attach resolver 使用同一 eligibility helper。

### 8.2 并发

使用隔离数据库验证：

- attach 与 normal End 竞争；
- attach 与 `RETURN_TO_BOARD` 竞争；
- attach 与 administrative abort 竞争；
- 两次相同 attach 并发仍只有一个 exact Edge，第二次为 no-change / 正常幂等；
- 失败测试不连接或 truncate 主开发数据库。

所有测试必须创建独立数据库或使用事务隔离。不得复用、清空、重建本地验收 Community 的 Postgres
database，遵守 [破坏性迁移测试导致主数据库数据丢失](../bug/destructive-migration-test-main-database-data-loss.md)
中的防护要求。

### 8.3 ACP prompt

固定断言：

- Project Space contract version / hash 更新；
- Meeting contract version / hash 更新；
- current 版本分别为 Project Space `7`、Meeting `4` 与 `meeting-context-v3`，旧合同不能满足
  current runtime；
- System Contract 不包含 Meeting ID、revision、Board、Document body 或 render slot；
- 每个 Turn envelope 含正确 Meeting coordinate 与 Project Context write boolean；
- 只有 Action Finalization 的 Project Context policy 为 writable；
- `board_maintenance` 的 Context boolean 为 false，但合法 Board UPDATE schema 保持可用；
- Action Finalization allowed tools 明确包含 `project-view`、`documents`、`project-context`、`roles`；
- output schema 仍只接受 `COMPLETE | BLOCK | RETURN_TO_BOARD | ABORT`；
- prompt 明确禁止自动推断和伪造 Edge；
- Context failure 映射为 BLOCK，Board semantic failure 映射为 RETURN_TO_BOARD。

### 8.4 Desktop

Mock Bridge / query model 覆盖：

- picker 显示 finalizing 与 terminal Meeting；
- picker 排除普通 active phase；
- Inspector 正确显示 `Finalizing actions`；
- live transition 到 closed / aborted / discussion 后节点就地刷新；
- Edge selection、graph viewport 与 `Open Meeting` 跳转不丢失；
- `meeting_not_attachable` 以可恢复错误呈现，不导致整个 Project Context 页面崩溃。

### 8.5 端到端验收

真实 Agent 主持 Meeting：

1. Board 决定创建或修改至少一个 Project View object；
2. 进入 Action Finalization；
3. 同一 Action Finalization Turn / 逻辑主持 Agent 完成 Project View 写入与回读；测试至少一次
   fallback 槽或 Session 轮换；
4. 创建 Context Document；
5. attach `{Meeting, materialized coordinates}`；
6. exact 与 incident 回读命中同一 edge key；
7. 返回 `COMPLETE`，Meeting 正常 `completed_closed`；
8. 关闭后再次 exact 回读，edge key、Document body 与 Meeting deep link 不变；
9. 日志中每个 ActionRunKey 只有一个 pending/running Turn，不存在 post-close worker、Manifest 或
   伪造 action receipt；物理槽变化不得生成 `affinity_lost`。

另做一次失败验收：让 Context command 返回可恢复失败，确认主持人返回 `BLOCK`、Action Run lease 可续约、
Meeting 不会假 `COMPLETE`，Retry 后可在同一流程完成。

## 9. 实施阶段与 Review 门

### 阶段 1：Relay / DB 语义

- 实现 finalizing resolver 与统一 attachability predicate；
- 更新错误映射；
- 补 DB、Relay、并发和无部分写入测试。

Review 门：确认没有修改 coordinate wire / edge key，没有把 roster 或 Action lease 变成 Context 权限，且
Meeting / Context 锁顺序无环。

### 阶段 2：ACP Contract 与 Turn envelope

- 升级 Project Space、Meeting Contract、Meeting turn context version；
- 更新 Action Finalization tool policy 与 prompt；
- 补 prompt golden / schema tests。

Review 门：确认 System 只含稳定语义，动态事实只在逐 Turn envelope；确认 output parser 未扩展、没有
Manifest、第二个补写 Turn 或 post-close worker。

### 阶段 3：CLI / Desktop / 文档

- 更新 CLI help 与错误；
- 更新 picker、hydration、Inspector 与 live invalidation；
- 更新既有 Project Context / Meeting 文档中的 terminal-only 表述；
- 执行 Desktop 与端到端验收。

Review 门：确认 UI 不根据本地显示猜 phase，既有 terminal Meeting 与 Edge 不回归，且没有任何数据清理
或 destructive migration。

### 阶段 4：无活跃会议的全量切换

本设计首次交付的 Project Space `5 -> 6`、Meeting `2 -> 3` 与后续逻辑主持人切换到 `7/4` 都会
改变 contract hash。现有 Harness 会拒绝继续复用携带旧合同的 ACP Session；这用于确保新 Session
安装 current Contract，而不是维持物理 affinity。

本次不增加 active-session contract pinning。部署前置固定为：

1. 只读确认当前没有 active Meeting，也没有 non-terminal Action Run；
2. 若存在，等待其正常结束；不得为部署自动 abort、删除或伪造完成；
3. 在同一交付中构建并切换 Relay、CLI、ACP 与 Desktop；
4. 启动后验证 Project Context / Meeting capability、合同版本和已有数据 revision；
5. 创建新的验收 Meeting 执行第 8.5 节 E2E，其中包含 fallback 槽或 Session 轮换。

切换期间不得初始化新的 Community、清空数据库或重跑 destructive bootstrap。若未来要求 active Meeting
跨版本无中断升级不在本次切换范围；旧 Contract 不提供 current runtime 兼容分支，也不能用旧 Session
绕过 current capability / contract gate。

每阶段完成后应对照本设计 review 代码与测试；发现偏离时在进入下一阶段前修正，但无需引入阶段间人工
暂停。

## 10. 文档一致性迁移

实现提交必须同步修改以下旧语义：

- [TODO.md](TODO.md) 中“仅允许 closed / aborted”“不因 Action Finalization 修改 Context”的绝对表述；
- [meeting-coordinate-implementation-design.md](meeting-coordinate-implementation-design.md) 第 7.2、9.1、
  9.2、错误矩阵、测试与最终决策中的 terminal-only 限制；
- [project-context.md](project-context.md) 中 Meeting attach eligibility；
- Meeting Action Finalization 相关 backend / ACP / Desktop 文档中的 allowed tool 与完成责任。

修改后的统一措辞必须保留差异：

```text
允许：Action Finalization 中由 Human / Agent 显式维护 Context。
禁止：Relay 因 Meeting End、Board 内容或物化写入而自动推断 Context。
```

本设计只取代上述文档中的“Meeting 必须先终态才能 attach”与“Action Finalization 不显式维护 Context”
部分；Community 可见性、终态验证、Edge 无向集合语义、Context Document 必需、无自动推断和非破坏迁移
等既有结论继续有效。

## 11. 验收标准

满足以下条件才算完成：

- verified `finalizing_actions` Meeting 可通过同一现有 CLI attach 为 Meeting Coordinate；
- 其他 active Meeting 仍被 Relay 拒绝；
- Community 权限、Meeting roster/action 权限边界没有混合；
- 主持 Agent 在同一 Action Finalization Turn / 逻辑主持身份完成物化、Context 写入和 canonical 回读；
  不要求继承讨论阶段的槽或 ACP Session；
- 不新增 Meeting 状态、Manifest、第二个补写 Turn 或 post-close worker；
- 不改变 Direct Action output schema；
- 不自动推断 Edge，不伪造无输出关系；
- RETURN_TO_BOARD / ABORT 不自动回滚已提交的外部结果，UI 能显示真实 Meeting lifecycle；
- 既有 terminal Meeting Edge、edge key、Context revision、Document 和 Project View 数据全部保留；
- 构建、DB/Relay tests、ACP tests、Desktop tests 与真实 Meeting E2E 全部通过；
- 测试与启动过程没有删除或重置本地验收数据。

## 12. 实现交付记录

以下为本设计首次交付（`6/3/meeting-context-v2`）的历史验收记录；数字与运行实例事实不因后续
合同升版而重写。当前规范见本节末尾的取代说明。

截至 2026-08-08，阶段 1～3 已按本设计实现并逐层 review：

- Relay / DB 以现有 canonical Session、Runtime、Action Run、Board 与 Meeting State 证据联合判定
  `finalizing_actions` attachability；未修改 coordinate wire、Edge key、Context schema 或数据库迁移；
- ACP 已升级 Project Space、Meeting System Contract 与逐 Turn envelope，只有
  `action_finalization` Turn 宣告 Context 可写，Direct Action output schema 保持不变；
- CLI、Desktop picker、hydration、Inspector 与 Meeting live invalidation 已迁移到
  `meeting_not_attachable` 和 finalizing / terminal 双路径；
- Mock Bridge E2E 已验证 finalizing Meeting 的冻结、待关闭 Inspector；Rust、Desktop 原生层、Desktop
  类型/静态检查与单元测试已通过；
- 未运行任何会清空、重建或迁移本地验收数据库的命令，也未新增 SQL migration。

阶段 4 的无活跃会议全量切换也已完成：

- 切换前以只读查询确认 `active Meeting = 0`、`non-terminal Action Run = 0`；没有为了部署而
  abort、删除或伪造任何 Meeting / Action Run；
- 从 `version/v1.0.0` 的同一 HEAD 统一重建并启动 Relay、CLI、ACP 与 Desktop；所有 Desktop
  sidecar 均与本次 `../../../target/debug` 产物逐字节一致，运行中的 ACP 进程均在本次切换后启动；
- Project Space contract 已为 `6`，Meeting contract 已为 `3`，Meeting turn context 已为
  `meeting-context-v2`；contract hash 变化会使旧 Session 失效并重建，不保留旧合同兼容分支；
- Relay 在 `localhost:3000` 公告 `buzz-project-view-v3`、`buzz-project-document-v1`、
  `buzz-project-context-edge-v2`、`buzz-meeting-community-read-v1` 与 Meeting V2 / Direct Actions
  能力；readiness 正常；
- 标准 migration runner 确认数据库 migration 仍为 `53`，没有新增或回放 SQL migration；本地
  Community seed 返回 `INSERT 0 0`，未创建新 Community；
- 切换前后业务数据指纹完全一致：Project View `schema 3 / revision 60 / 18 objects / 10 checkpoints`，
  Document `schema 1 / revision 18 / 18 active`，Project Context `schema 2 / revision 19 / 14 active
  edges / 15 bound documents`；`1310` 条 Event、`13` 个 Channel、`11` 场历史 Meeting 与 `4` 个
  Action Run 均无增删，且切换后仍为零 active / non-terminal；
- ACP contract、旧合同拒绝复用与 Action Finalization turn envelope 的聚焦测试共 `19` 项通过；
  启动日志无 error / panic / migration failure。

这里的 Project Space `5 -> 6` 与 Meeting `2 -> 3` 是 ACP System Contract 的版本/hash 切换，
不是 Project View、Project Context 或 Postgres schema 的数据迁移。阶段 4 因而不需要改写已有业务数据；
“迁移干净”体现在旧 ACP 进程/Session 不再继续运行、所有组件来自同一构建，以及既有 canonical revision
在切换前后保持一致。

剩余验收仅为第 8.5 节的真实 Agent 主持 Meeting 流程；该验收会写入真实业务数据，应由 Human 明确发起，
不在实现测试中自动创建或删除 Meeting、Document、Project View 对象或 Context Edge。

后续 lifecycle 简化由
`..meeting-action-finalization-logical-host-ack-simplification-implementation-design.md`
取代了物理 affinity 条款：current generation 为 Project Space Contract `7`、Meeting Contract `4`
与 `meeting-context-v3`。以上 `6/3/v2` 数字只记录本设计首次交付时的历史基线，不再是现行门禁。
