# Meeting V2 Board→Action 连续性、Return-to-Board 投影与 Desktop 终态收敛修复设计

> 状态：核心修复已实现；自动化验证通过；待新建 Meeting 现场验收
>
> 记录日期：2026-08-06
>
> 范围：Meeting V2 `moderated-board-actions-v3`、`buzz-acp` Meeting coordinator、
> AgentPool/ACP Session affinity、Return-to-Board State 投影校验、Desktop Meeting directory
>
> 关联设计：
> [Meeting V2 Floor Decision 空等与 Action Finalization 硬超时修复设计](meeting-v2-floor-decision-and-action-finalization-timeout-fix-design.md)、
> [主持人直接完成行动收口的后端修正方案](../meeting/fix/meeting-v2-direct-action-finalization-backend-plan.md)

## 1. 结论

最近一次现场验收没有复现已经修复的 Floor Decision `no_action` 空等 BUG：主持人的有效决策均在
6.8～15.3 秒内提交，没有任何 moderator decision attempt 进入 `timed_out`。

本次实际暴露了三条新的、彼此相连但边界不同的缺陷：

1. **Board→Action 连续性被自产生的 canonical State 回流破坏。**
   canonical Board 推进后，coordinator 发出了只携带 Meeting `session_id` 的通用
   preemption；主循环按 channel 查找任意 in-flight turn 并发送 `Cancel`。现有日志没有
   `target_turn_id`/turn kind，因此无法证明被取消的是产生命令的 turn A，还是已快速派发的
   相邻 turn B。但可以确认：通用取消路径删除了 ACP Session，continuity binding 仍引用旧
   Session，Action exact claim 随后必然得到 `affinity_lost`。
2. **Return-to-Board 后的合法投影被 ACP 错误拒绝。**
   Relay 按协议保留终止 Action 的原 `board_window`，同时打开下一 Board window；ACP 却无条件要求
   `action.board_window == board.board_window`，因此持续拒绝合法 State，协调器无法恢复推进。
3. **Desktop 主视图和左侧目录缺少确定性终态收敛。**
   主视图 snapshot 与 sidebar directory 是两份独立 React Query 缓存。实时失效信号一旦漏收或与
   refetch 竞态，主视图可显示 `closed`，目录仍长期保留 `active / In progress`。

本次 Action renewable lease 实现没有得到有效验收：Action Run 在正式 Action turn 派发之前就因
`affinity_lost` 被 blocked，`progress_seq=0`，accepted renewal 数为 0。Project View 的实际写入由
已存在的 Agent/tool 执行链完成，不能替代正式的 Action `confirm-recorded` 路径验收。

## 2. 故障记录

### 2.1 事件范围

- Meeting：`678ecf1a-d07a-419e-a041-ef80c502b3b2`
- 标题：`对话聊天 Agent 开发：项目启动会议`
- schema：`3`
- policy：`moderated-board-actions-v3`
- 主持 Agent：`test-1`，pubkey 后缀 `f06d...b204`
- Action Run：`abf1d3b6-584e-4e6c-89d8-a4a06ace37b2`
- 最终 Board：`17384e307d864d5ea3661776065338037c22642754f177cd9dfe7b693056b943`
- End Event：`17a7a958559e5698a34699d1171abbfb788c9e4886515102675cef7d8ea58cd4`
- canonical 终态：`status=ended`、`terminal_outcome=closed`、State Revision `50`

### 2.2 Floor Decision 实际时间线

本次共有 7 条 decision attempt 记录，其中 6 条正常 committed，1 条因 `runtime_lost` 立即 abandoned
并在约 0.3 秒后开始替代 attempt：

| 开始时间 | 结束时间 | 耗时 | 结果 |
|---|---|---:|---|
| 20:42:32.603 | 20:42:40.815 | 8.2 秒 | committed |
| 20:43:13.438 | 20:43:14.435 | 1.0 秒 | abandoned / `runtime_lost` |
| 20:43:14.700 | 20:43:25.453 | 10.8 秒 | committed |
| 20:44:01.703 | 20:44:17.040 | 15.3 秒 | committed |
| 20:44:57.716 | 20:45:08.089 | 10.4 秒 | committed |
| 20:46:07.224 | 20:46:14.077 | 6.9 秒 | committed |
| 20:47:27.259 | 20:47:34.572 | 7.3 秒 | committed |

结论：

- 没有 attempt 等待原 3 分钟 decision deadline；
- 没有 `invalid output → no_action → deadline fallback`；
- 前一份设计中的 Floor Prompt/解析/恢复修复已经生效；
- 本文不重新调整 Floor deadline，也不回滚已经落地的 parser 修复。

### 2.3 Board→Action 与恢复时间线

```text
20:47:44.191  最后一次 Speech accepted，控制权返回主持人
20:48:32.422  Board window 7 updated
20:48:32.656  ACP 对 Meeting channel 发送 ControlSignal::Cancel（无 target turn metadata）
20:48:38.115  Action Run 开始
20:48:38.562  Action Run 因 affinity_lost blocked
20:51:21.450  Human/Agent Retry
20:51:22.054  相同 affinity_lost 再次 blocked
20:52:31.730  正式 Return-to-Board，打开 Board window 8
20:52:31.787  ACP 首次拒绝合法 State：invalid authority fields
20:52:32～20:55:29
                ACP 每约 5.5～6 秒 Full Sync 失败一次
20:55:28.713  Board window 8 updated
20:55:33.574  Meeting 正式 closed
```

Action Run 的最终数据库状态为：

```text
condition:           blocked
terminal_status:     returned_to_board
last_error_code:     affinity_lost
progress_seq:        0
action_window_epoch: 2
action.board_window: 7
current board_window:8
accepted renewals:   0
```

这证明：

- Action lease 没有先到期；
- provider/tool idle watchdog 没有触发；
- 不是 Action 执行时间不足；
- 正式 Action prompt 根本没有开始执行。

### 2.4 用户可见后果

1. Action Finalization 立即进入 blocked，Retry 立即复现；
2. 已完成的 Project View 写入无法生成 `actions-recorded` attestation；
3. Return-to-Board 后协调器停止推进，只能依赖残留执行链或人工命令收口；
4. Meeting 虽然已 canonical closed，Desktop 左侧仍显示 `In progress`；
5. ACP 在已结束 Meeting 上持续重试 Full Sync 并刷出同一校验错误。

## 3. 根因

### 3.1 Preemption 丢失了“为什么取消”的语义

当前 coordinator 只向主循环交付一个 `session_id` 集合。主循环既无法区分原因，也无法
指定应停止的 turn：

1. 已完成的 turn A 自己促成的 canonical Board 推进；
2. Human 或另一权威执行者造成的外部 supersession；
3. Meeting End/Abort；
4. 新候选、Offer/Grant 或其他确实需要停止旧 turn 的状态变化。

所有情况最终都变成：

```text
ControlSignal::Cancel
```

通用 Cancel 的既有语义是“当前 channel session 不再可信”，因此 clean cancel 后仍执行：

```text
agent.state.invalidate(PromptSource::Channel(meeting_id))
```

这个语义适用于 Human 强制停止、模型切换或 continuity 真正失效，但不适用于
“同一主持身份提交的 Board 已被 Relay 接受，现在需要沿用同一 ACP Session 进入下一控制
阶段”，更不能因为 turn A 的延迟 directive 而误杀新一代 turn B。

### 3.2 Continuity binding 与 ACP Session 生命周期不是原子状态

Action-capable Meeting 同时维护：

- Agent slot index；
- ACP Session ID；
- continuity phase；
- AgentState 中 `meeting_id → acp_session_id` 的 session 映射。

当前 Cancel 可以删除最后一项，却不会同步释放或重新建立前三项。之后 `claim_exact_meeting()` 严格
比较 binding 与 AgentState，正确地 fail-closed 为 `AffinityLost`。

因此错误不在 exact claim。错误发生在上游：系统制造了一个内部不一致的 binding/session 状态，
随后让精确校验替它暴露故障。

### 3.3 现有测试没有覆盖真实 Cancel 组合路径

当前测试分别证明了：

- binding 可以让 Board/Floor/Action 使用同一 slot/session；
- Session ID 改变时 exact claim 会 fail-closed；
- coordinator 可以识别 stale Board/Floor request；

但没有贯穿：

```text
canonical State 回流
  → coordinator 生成 preemption
  → 主循环发送 Cancel
  → clean cancel 修改 AgentState
  → PromptResult 返回 pool
  → continuity directive 应用
  → 下一阶段 exact claim
```

因此隔离测试全部通过，真实主循环仍然破坏 Session。

### 3.4 Return-to-Board 的窗口关系被错误建模为恒等关系

Relay 的正确事务语义是：

```text
Action 基于 Board window N
  → Action terminal_status = returned_to_board
  → 保留 Action 的原始 fence：action.board_window = N
  → 打开新的 Board window：board.board_window = N + 1
```

保留原始 Action fence 是必要审计信息，不能把 Action 改写成“基于尚未存在的新 Board 执行”。
刚完成 Return-to-Board 的第一个 State 通常是 `N + 1`，但该 Action 随后已成为历史
provenance；如果会议又经历了 Board/Speech 循环，current Board window 可以继续增长，
control epoch 也可能前进。

ACP 当前却对所有 Action 状态无条件要求：

```text
action.board_window == board.board_window
```

Desktop 投影没有这一错误约束，因此同一 canonical State 在 Desktop 可读、在 ACP 不可读，形成客户端
协议漂移。

### 3.5 Desktop 只有 best-effort 实时失效，没有终态收敛闭环

Desktop 当前维护两份独立缓存：

- `meetingSnapshotQueryKey(..., meetingId)`：主页面；
- `meetingDirectoryQueryKey(..., meetingIds)`：左侧目录。

它们具有不同的 stale time，且 sidebar directory 没有非终态轮询。WebSocket 事件只作为
`invalidateQueries(meetingQueryRoot)` 信号；一旦终态事件漏收、订阅短暂重连或 refetch 发生竞态，
selected snapshot 可以因 mount/focus 单独更新，而 directory 保留旧 lifecycle。

canonical 数据没有错误，错误是 Desktop 缺少“主 snapshot 已确认终态后，目录必须最终得到同一终态”
的不变量。现有日志没有记录 subscription ID 到 End Event callback 的关联，因此本文不武断认定某一个
WebSocket 帧被丢弃；无论是漏收、重连窗口还是 invalidation/readback 竞态，客户端都必须自愈。

## 4. 修复边界与不变量

### 4.1 必须保持的安全边界

1. Action Finalization 仍必须精确复用同一 Agent slot 和 ACP Session；
2. moderator 身份/资格真正失效、进程替换、cancel drain timeout 或真实 Session 变化仍必须
   `affinity_lost`；单个 turn 被外部事件抢占不得自动等同于整条 continuity 失效；
3. 不允许通过“忽略 Session ID”或退化为任意空闲 slot 修复；
4. renewal 不授予业务权限，也不能修复已经丢失的 affinity；
5. 不伪造 `actions-recorded`，外部写入成功不等于 Meeting Action 已正式确认；
6. Return-to-Board 保留外部效果和原 Action fence，不重写历史；
7. Desktop 只展示 Relay 校验后的 canonical snapshot，不直接信任 WebSocket payload；
8. 不修改已结束 Meeting，不回滚 Project View revision，不需要数据库迁移或数据清理。

### 4.2 自证式推进、turn 抢占与连续链失效必须分开

定义四类状态处置：

```text
canonical in-place update
  同一 active Action run/window 的 lease renewal/progress State
  → 只更新 deadline/stage/revision
  → 不发送 control signal，不改变 binding phase

canonical self-advance
  当前主持身份/当前控制 epoch 的合法命令已被 Relay 接受
  → 仅在精确 target turn 仍存在且已过时时结束它；target 已终止则 no-op
  → 保留可复用 ACP Session
  → 推进 continuity phase

external turn supersession, continuity still valid
  Human priority/外部 Board 更新/新 candidate 使当前 turn 过时，但本地 moderator 身份仍有效
  → fence/cancel 旧 prompt 及其延迟输出
  → FinalControlCycle/PendingAction 可结束当前尚未进入 Action 的 exact-affinity chain
  → Action/ModeratorMeeting 不得被通用 ReleaseFinalControl 释放

continuity invalidated
  Meeting 终止、moderator 身份失效、进程替换或真实 slot/session 不匹配
  → 取消/fence 旧 prompt
  → 释放 binding
  → 只在 Session 本身不再可信或需执行 deferred rotation 时清除 Session
  → 对非正常终止的 active Action 标记 affinity_lost
```

不能仅凭“事件 pubkey 与 Agent 相同”判断 self-advance。判定必须同时使用当前 in-flight request、
control epoch、Board window、canonical transition 类型，以及可用的 prepared command/event receipt。
其中核心证据是：

```text
raw_state.transition.caused_by_event_id
  == local prepared_moderator_action.event_id
```

该等式还必须与同一 Meeting、prepared receipt 的 `origin_turn_id`、control epoch、
Board window/Action fence 一起成立。需要停止的 `target_turn_id` 可能是已经快速开始的相邻
Floor/Action turn，不必与 `origin_turn_id` 相同；但它必须精确指向被该 State 使之过时的
in-flight request。deadline/fallback transition 没有 `caused_by_event_id`，不能走 self-advance。

`transition.primary_type=action_lease_renewed` 且 run/window/fence 精确匹配时，必须走
`canonical in-place update`。即使其 `caused_by_event_id` 匹配本地 renewal event，也不能取消正在
执行的 Action turn。

当前 `apply_view_to_ledger()` 会在 `discard_stale_v2_host_requests()` 分类之前清掉已确认的
`prepared_moderator_action`。修复时必须在清理前完成分类，或持久化最小的
`confirmed_prepared_event_id`/确认回执；不能先丢失证据再根据 phase 猜测来源。

### 4.3 Terminal projection 必须可被所有客户端一致解析

Action/Board window 关系按 lifecycle 判定：

| Action 状态 | 合法关系 |
|---|---|
| active runnable/blocked | Board phase 为 `finalizing_actions`；control epoch 和 Board window 与 Action fence 精确相等 |
| `completed_closed` | Board phase/outcome 为 `ended/closed`；原 Action fence 精确相等；`completion_event_id` 匹配 canonical End event |
| `completed_aborted` | Board phase/outcome 为 `ended/aborted`；原 Action fence 精确相等；`completion_event_id` 不存在 |
| `returned_to_board` | Action 是历史 provenance；`board.board_window > action.board_window` 且 `board.control_epoch >= action.control_epoch` |

`returned_to_board` 的 current Board phase 可为 `board_pending`、`floor_ready` 或 `ended`。不允许
window 相等/回退或 control epoch 回退；但也不应拒绝已合法经历多个后续窗口的 State。

## 5. Board→Action continuity 修复方案

### 5.1 用有类型的 preemption directive 替代裸 Session ID

将 coordinator 当前的 `BTreeSet<Uuid>` preemption 输出改为有类型指令，至少包含：

```text
session_id
origin_turn_id
target_turn_id
reason
session_disposition
```

建议 reason：

```text
canonical_self_advance
external_authority_change
meeting_terminal
floor_superseded
runtime_recovery
```

建议 session disposition：

```text
preserve_on_clean_cancel
invalidate
release_binding
```

必须指向具体 `turn_id`，不能只按 channel 查找任意 in-flight task，避免相邻 turn 快速切换时把取消发给
新一代 turn。

coordinator 必须先解析 State `transition`，并在修改 ledger 前生成分类结果。建议指令和
诊断至少携带：

```text
state_event_id
state_revision
transition.primary_type
transition.caused_by_event_id
matched_prepared_event_id
origin_turn_id
target_turn_id
binding_phase
preserve_session
```

`RawBatonState` 目前会忽略 `transition`，实现时需增加 typed minimal Transition view，并严格
验证当前分类所依赖的 type/outcome/event ID。但应允许未知扩展字段，不能因为本地只用
最小子集就破坏 State 的 forward compatibility。

### 5.2 增加 Meeting 专用的 clean supersede 控制信号

新增区别于用户 `!cancel` 的内部控制信号，例如：

```text
ControlSignal::MeetingCanonicalAdvance
```

行为：

1. 若信号在 `session/prompt` 发出前到达，跳过该 request 并返回 typed preserved outcome，
   不得退化为普通 pre-prompt Cancel；
2. 若 prompt 已在执行，发送 ACP `session/cancel` 并等待 bounded drain；
3. clean cancel 成功时不调用 `AgentState::invalidate(source)`；
4. PromptResult 保留 `resolved_session_id`，并携带 `canonical_self_advance` outcome/reason；
5. PromptResult 进入主循环时，不得再由 cancelled/superseded 分支触发
   `ReleaseFinalControl`或清除已确认的 binding；
6. cancel drain timeout、ACP protocol error 或进程退出仍替换进程并报告 affinity lost；
7. 用户 Cancel、Rotate、SwitchModel 和确定的 continuity invalidation 继续使用原有
   invalidate 语义。

不能把所有 `ControlSignal::Cancel` 都改为保留 Session，否则会破坏现有人工停止和模型轮换边界。

### 5.3 分开 provisional reservation 与 canonical-confirmed phase

对 `canonical_self_advance` 的 clean result，不再要求模型最后一段文本必须可被
`v2_actions_board_output_is_holdable()` 重新证明已经发生的 Relay 状态变化。

大部分权威 phase 由 coordinator 已验证的 canonical State 确认，但 `PendingAction` 是一个必要的
本地 provisional reservation：

```text
Board accepted → floor_ready           → FinalControlCycle
已验证 Floor output + 已准备 Action Begin 命令
                                        → PendingAction（provisional）
canonical Action Begin                 → Action
canonical Return-to-Board              → ModeratorMeeting（保留 binding/session）
canonical Meeting End                  → Release
```

主循环在将 Agent 放回 pool 前，以保存的 `resolved_session_id` 安装或更新 binding。这样第一次 Board
推进也不依赖更早阶段已经存在 binding。

phase 推进不得只根据模型文本或“已发起命令”猜测：

- 首次 FinalControl 中，只有已验证 Floor output 并已持久化完整签名 Action Begin 命令后，
  才先把 binding 置为 `PendingAction`；该 reservation 必须在 Agent 返回普通 idle capacity 前安装，
  防止 slot 被 ordinary work 抢占；
- Return-to-Board 使 `Action → ModeratorMeeting`，只终结 Action authority/lease，不删除 binding、
  不清除 ACP Session，也不执行 deferred rotation；
- `ModeratorMeeting` 的下一轮 Board/Floor 继续 exact-claim 同一 tuple；若再次选择 Action，
  在 canonical Action Begin 前保持 `ModeratorMeeting`，不必先退回 `PendingAction`；
- 只有 canonical Action Begin State 才能推进为 `Action`；
- 只有权威 Meeting terminal/teardown、moderator 身份撤销、物理进程替换或真实
  slot/session 丢失才完整释放 binding；如有 deferred rotation，只在此时执行。

这一不变量保证 `FinalControlCycle → PendingAction → Action → ModeratorMeeting`
始终引用同一 `{agent_index, acp_session_id, process generation}`。

### 5.4 外部 turn 抢占不等于整条 continuity 失效

以下情况不得被分类为 `canonical_self_advance`：

- control epoch 改变；
- moderator identity 改变；
- Board window/State revision 不符合当前 request 的下一合法状态；
- canonical transition 无法关联到当前 prepared command 或当前主持控制动作；
- Human 明确 Abort/Close/Board override；
- Agent slot 被替换或 ACP Session ID 已变化；
- cancel drain timeout 或 ACP transport failure。

但“不是 self-advance”不能再被简化为“一律清 Session/释放 binding”：

- Human priority、外部 Board override 或新 candidate 只使当前 Board/Floor turn 过时时，
  fence 其输出；`ReleaseFinalControl` 只允许释放 `FinalControlCycle | PendingAction`；
- `Action | ModeratorMeeting` 不得被 `ReleaseFinalControl` 释放。Meeting 仍 active 且本地
  moderator identity 仍有效时，应依 canonical lifecycle 降级/继续；
- Meeting terminal/teardown 会完整释放 binding；moderator 被移除/撤销、物理进程替换、
  明确 rotation/owner teardown 或真实 tuple 丢失还必须 invalidate 不再可信的 Session；
- 若此时存在 active Action Run，则按协议写入结构化 `affinity_lost` block。

一句话边界：**Return-to-Board 释放的是 Action authority/lease，不是 moderator ACP affinity。**

### 5.5 幂等与竞态

- 同一 canonical State 重放不得重复发送 cancel；
- directive 使用 `(session_id, origin_turn_id, target_turn_id, state_event_id, caused_by_event_id)` 去重；
- 旧 turn 的延迟 result 不得覆盖新 binding；
- clean supersede result 到达前，下一 Action/Floor request保持 queued，不得退化到其他 slot；
- exact slot 仍在 drain 时应返回 Busy 并重排，不得把短暂 Busy 写成 `affinity_lost`；
- binding 安装和 Agent 返回 idle pool 在同一主循环临界区完成；
- Meeting End 优先于所有 preserve directive，并最终释放 binding。

## 6. Return-to-Board 投影修复方案

### 6.1 修正 ACP validator

把无条件窗口相等改为 lifecycle-aware helper：

```text
validate_action_board_relation(
    terminal_status,
    action_control_epoch,
    action_board_window,
    board_phase,
    current_control_epoch,
    current_board_window,
)
```

分支规则：

1. active Action：Board phase 必须为 `finalizing_actions`，control epoch/window 严格相等；
2. `completed_closed`：Board 必须为 `ended/closed`，control epoch/window 与原 Action fence
   精确相等，`completion_event_id=Some` 并匹配 canonical End event、`terminal_at_ms=Some`、
   deadline=None；
3. `completed_aborted`：Board 必须为 `ended/aborted`，control epoch/window 与原 Action fence
   精确相等，`completion_event_id=None`、`terminal_at_ms=Some`、deadline=None；
4. `returned_to_board`：Board phase 必须为 `board_pending | floor_ready | ended`，
   `current_board_window > action_board_window`，
   `current_control_epoch >= action_control_epoch`。

第 4 条校验的是历史 provenance 的单调性，不是“当前仍停在刚 Return 的下一窗口”。
因此不使用精确 `N + 1`，也无需 checked add。

对 `returned_to_board` 还同时校验：

- `returned_to_board` 必须 terminal；
- `action_deadline_at_ms` 必须为空；
- `completion_event_id` 必须为空；
- `terminal_at_ms` 必须存在；
- `condition` 可保留 Return 前的 `runnable` 或 `blocked`，不用它推断当前 Board 是否可执行；
- common identity、ID 形状、revision/timestamp 和 terminal outcome 校验仍严格 fail-closed。

### 6.2 对齐 ACP 与 Desktop 的协议不变量

将纯窗口关系 helper 放到共享协议层（优先 `buzz-sdk`），由 ACP 和 Desktop Tauri projection 共同调用，
或至少建立共享 fixtures/corpus，防止两个客户端再次漂移。

fixture 至少覆盖：

- active Action / `finalizing_actions` / same fence；
- blocked Action / `finalizing_actions` / same fence；
- completed close / `ended/closed` / completion event present；
- completed abort / `ended/aborted` / completion event absent；
- returned-to-board / 立即 `N+1` / same epoch；
- returned-to-board / 多个后续 Board/Speech 循环 / window `>N+1` / higher epoch；
- returned-to-board / Meeting 后续 `ended`；
- returned-to-board / same 或 lower window（拒绝）；
- returned-to-board / current epoch lower（拒绝）；
- terminal status 与 Board phase/outcome 不匹配（拒绝）。

相同 corpus 必须同时走 live-event fast path 和 Full Sync path，确保两条入口不会再分叉。

### 6.3 终态同步停止

修复后 ACP 必须能够读取 State Revision 48～50，并在 `phase=ended` 时：

- 删除 pending/in-flight Meeting request；
- 停止 Full Sync retry；
- 释放 continuity binding；
- 停止 Action renewal/deadline task；
- 按既有终态规则删除整条 durable per-Meeting coordinator ledger，避免 prompt、private
  reason 和已签名 prepared event 长期累积；
- 必要的历史诊断保留在结构化 observer/log/audit 中，不新增无界 durable 诊断存储，
  也不继续刷同一 warning。

## 7. Desktop 终态收敛修复方案

### 7.1 保留 WebSocket invalidation，但不把它当作唯一真相推进器

WebSocket 事件继续只作为“需要重读”的信号，不能直接写入未验证 lifecycle。现有安全边界保持不变。

同时补充两个自愈入口：

1. **Selected snapshot → directory reconciliation**
   - 主页面读到经过 Tauri 验证的 `closed/aborted` terminal lifecycle 时；
   - 精确 invalidate 包含该 Meeting 的 active directory queries；
   - directory 仍通过 Tauri 重新读取和验证 canonical snapshot；
   - 不在 React 中手工伪造 End projection 或拼装 terminal `MeetingListItem`，避免丢失 Rust 计算的
     viewer-specific attention。
2. **仅对非终态 Meeting 的低频 fallback refetch**
   - sidebar 中存在 `initializing/active/finalizing_actions` 时，每 10～15 秒重读 directory；
   - 全部 terminal 后停止 interval；
   - app 进入后台时不主动轮询；
   - WebSocket 正常时仍可合并/去抖，避免请求风暴。

收敛保证以 Relay/Tauri canonical read 可成功为前提。持续断网或 Relay 不可用时保留上次已验证
数据；在下一次成功 refetch 后收敛，不伪造 terminal。

### 7.2 统一终态后的 UI 行为

当 directory 读到 `closed/aborted`：

- 立即从 active 列表移入 history；
- 不再显示 `In progress` 或 `Recording actions`；
- selected Meeting 页面保持可读；
- final Board、formal Speech 和 Action audit 仍可访问；
- terminal attention 按现有 acknowledgement 语义处理，不因缓存修复重复弹出。

### 7.3 实时同步可观测性

为 Meeting live sync 增加开发态/结构化诊断：

```text
subscription established / replayed
event received: kind, meeting_id, state_revision
invalidation scheduled / coalesced
directory refetch started / succeeded / failed
snapshot-directory lifecycle mismatch
fallback poll convergence
```

不得记录事件正文、私钥、auth tag 或其他敏感内容。

ACP 的对应 preemption 日志必须是 typed record，至少包含 `reason`、turn kind、
State/event ID、`caused_by_event_id`、当前 binding phase 和 `preserve_session`，不再只输出无法
区分来源的 `Cancel`。

## 8. 自动化测试

### 8.1 ACP 主循环与 AgentPool

必须新增贯穿真实组合路径的测试，而不是只调用孤立 helper：

1. 第一次 Board turn 创建 ACP Session；
2. 正常因果顺序为 `PromptResult(A) → prepare/sign/submit → canonical ack`；分别覆盖 ack
   与 binding 安装/Agent return 的调度交错；
3. State `transition.caused_by_event_id` 精确命中本地 prepared event；
4. coordinator 生成指向 turn A 的 `canonical_self_advance`；
5. directive(A) 延迟到 turn B 已派发时，因 `target_turn_id` 不匹配而不得取消 B；
6. 如 canonical State 确实使 B 过时，coordinator 生成另一条精确指向 B 的 directive；
7. pre-prompt 和 in-flight clean cancel 都保留 AgentState session，冗余模型 result 被幂等丢弃
   且不释放 binding；
8. binding 安装到原 slot/session；
9. Floor turn exact claim 成功；
10. 已验证 Floor output 与已准备 Action Begin 命令使 binding 进入 provisional `PendingAction`；
11. 只有 canonical Action Begin 把 binding 推进为 `Action`，Action 只执行一次；
12. 同 run/window 的 `action_lease_renewed` State 只更新 deadline/stage，不 preempt Action turn；
13. Action renewal至少成功一次；
14. Return-to-Board 后 binding 仍存在且 phase=`ModeratorMeeting`，下一轮
    Board/Floor/再次 Action 都复用原 tuple；
15. 另一路径中 `confirm-recorded` 正常关闭，且只在 Meeting ended 后释放 binding
    并执行 deferred rotation。

反向测试：

- Human Board override/deadline fallback 使用 typed turn preemption，延迟输出被 fence；
- `ReleaseFinalControl` 只释放 `FinalControlCycle | PendingAction`，对 `Action | ModeratorMeeting`
  拒绝释放；
- `caused_by_event_id` 缺失或不匹配不得走 preserve path；
- control epoch 改变使当前 turn 过时，但不能在 moderator identity 仍有效时无条件
  释放 `ModeratorMeeting`；
- preserve-session cancel drain timeout 导致 affinity lost；
- 普通 Cancel/Rotate、ACP process exit 和 cancel-drain failure 都清除 Session 并导致 affinity lost；
- exact slot 暂时 Busy 时重排，不导致 affinity lost；
- 旧 turn 延迟 result 不得重建 binding；
- 同一 State/event 重放幂等，不重复 cancel 或重复执行 Action；
- Retry 在旧 prompt drain 完成前不得派发。

现有 Human preemption 回归必须保留，不能为了 self-advance 而放宽外部抢占边界。

### 8.2 Return-to-Board

1. active Action → blocked → Retry → Return-to-Board；
2. State 中 `action.board_window=N`、`board.board_window=N+1` 可被 ACP/Desktop共同解析；
3. coordinator 派发 Board window `N+1`；
4. 再经历 Board/Speech 循环后，`board.window>N+1` 或 higher control epoch 仍可解析；
5. Board update 后可以重新进入 Floor，后续 `ended` 仍可解析；
6. 不再次 Finalize 时可以从新 Board 正常 Close；
7. Meeting End 后不再出现 Full Sync warning；
8. same/lower window、current epoch lower、wrong phase/terminal shape 均 fail-closed；
9. 以上有效 fixture 在 live fast path 和 Full Sync path 都通过。

### 8.3 Desktop

1. directory 初始为 `active`，selected snapshot 更新为 `closed`，目录最终移动到 history；
2. 模拟漏掉 End live event，fallback refetch 仍在一个 interval 内收敛；
3. selected snapshot 首次 terminal directory refetch 失败、第二次成功时仍收敛；
4. `aborted` Meeting 收敛后保留正确 viewer-specific attention；
5. 模拟 WebSocket reconnect，lookback/replay 后不产生旧 lifecycle 回退；
6. directory refetch 失败时保留上次数据并重试，不伪造 terminal；
7. terminal Meeting 停止 fallback interval，app 后台不轮询；
8. 多 Community 切换时旧 Meeting cache/subscription 不泄漏。

### 8.4 现场验收

新建一场 action-capable Meeting，要求：

1. 至少 4 次 Floor 传递，所有有效主持决策在模型返回后立即提交；
2. 最终 Board→Action 保持同一 slot/session；
3. Action Run 持续超过一个 lease cadence，并观察到 accepted renewal；
4. 写入少量可回读的 Project View 对象；
5. `confirm-recorded` 成功并直接关闭 Meeting；
6. 另一次演练 block → Retry → Return-to-Board → Board update → Close；
7. Desktop 主视图和 sidebar 在终态后保持一致；
8. ACP 日志无持续 `invalid authority fields`、无错误 `affinity_lost`；主持模型在 deadline
   前返回并被 Relay 接受时，无错误/可避免的 deadline fallback。

## 9. 实施顺序

1. 让 ACP Raw State 严格读取 canonical `transition`；
2. 在 `prepared_moderator_action` 被清理前完成 event-ID correlation，并保存最小确认回执；
3. 增加 typed preemption directive 与目标 `turn_id`；
4. 增加 preserve-session 的 Meeting canonical advance 控制结果；
5. 同时修正 clean result 和 `ReleaseFinalControl` 分支，保证其不释放
   `Action | ModeratorMeeting`，并在 AgentPool 返回临界区安装 binding；
6. 补齐两种 State/PromptResult 时序、自产推进与外部抢占的组合回归；
7. 修复并共享 lifecycle-aware Return-to-Board relation validator；
8. 补齐 terminal coordinator cleanup，消除无限 Full Sync retry；
9. 增加 Desktop snapshot→directory reconciliation；
10. 增加非终态低频 fallback refetch 与同步诊断；
11. 运行 Rust/desktop 单元测试和格式检查；
12. 新建 Meeting 完成两条现场验收路径。

前 1～8 项属于协议运行正确性，优先级 P0；第 9～10 项属于状态展示与客户端自愈，优先级 P1。

## 10. 完成标准

- 正常 canonical Board 回流不会删除同一主持控制链的 ACP Session；
- Board、Floor、Action 使用同一 slot/session，外部抢占的延迟输出仍严格 fail-closed；
- Action Run 不再在派发前立即 `affinity_lost`；
- renewable Action 至少产生一次 accepted renewal，并可 `confirm-recorded`；
- Return-to-Board 后 ACP 可读取下一个及更晚的 Board window，保留原 Action provenance
  和 moderator slot/session binding；
- ended Meeting 不再持续 Full Sync；
- Relay canonical closed 且 canonical read 可成功时，Desktop snapshot 与 sidebar directory 在一次
  fallback interval/下一次成功 refetch 后收敛为 terminal；
- Floor Decision 既有修复不回归，没有重新引入 3 分钟空等；
- 无数据迁移、无历史重写、无 Project View revision 回滚。

## 11. 非目标

- 放宽 exact affinity 为“任意同逻辑 Agent 实例均可接管”；
- 让 Desktop/Human 代替 Agent 续约或确认 Agent-host Action；
- 通过延长 timeout 掩盖 continuity 丢失；
- 修改 Floor 3 分钟业务 deadline；
- 自动补造本次事故缺失的 `actions-recorded` attestation；
- 重写已结束 Meeting、Action Run 或 Project View 历史。

## 12. 实施记录（2026-08-06）

### 12.1 已落地

1. ACP typed 读取 Relay State 的最小 `transition` 投影，并在 prepared command 被清理前，按
   `transition.caused_by_event_id == prepared_moderator_action.event_id` 捕获进程内确认回执；
2. coordinator 不再输出裸 Meeting ID，而是输出带 `origin_turn_id`、`target_turn_id`、reason 与
   Session disposition 的精确抢占指令；延迟的 turn A 指令无法取消同一 Meeting 的 turn B；
3. 新增 `MeetingCanonicalAdvance` clean-supersede 路径：pre-prompt 与 in-flight 两种时序都丢弃
   过期输出、保留健康 ACP Session；cancel drain/transport/process failure 仍沿用 fail-closed 替换路径；
4. 首次 Board 被 canonical State 提前收口时，使用已解析的 ACP Session 建立
   `FinalControlCycle` binding；Return-to-Board 已推进为 `ModeratorMeeting` 后，旧 Action result
   不得把 phase 回写成 `Action`；
5. `ReleaseFinalControl` 只允许释放 `FinalControlCycle | PendingAction`，不会释放
   `Action | ModeratorMeeting`；
6. Action projection 改为 lifecycle-aware 校验：active/closed/aborted 保持 exact fence，
   `returned_to_board` 接受严格单调的后续 Board window/control epoch；
7. Desktop 在经过 Tauri 校验的 selected snapshot 到达 terminal 时主动失效 directory；只要目录中
   仍有 readable non-terminal Meeting，就以前台 12 秒低频 canonical reread 自愈，全部 terminal 后
   自动停止；
8. Human Board override 与新候选抢占仍使用精确 turn fencing，既有外部抢占安全边界没有放宽。

### 12.2 自动化验证

- `cargo test -p buzz-acp --lib`：808 passed；
- `cargo clippy -p buzz-acp --all-targets -- -D warnings`：通过；
- `cargo fmt --all -- --check` 与 `git diff --check`：通过；
- `pnpm --dir desktop check`：通过；
- `pnpm --dir desktop typecheck`：通过；
- Desktop Meeting sync policy 定向测试：2 passed。

### 12.3 尚未宣称完成的现场项

自动化已覆盖本次三个根因，但没有在主开发 Relay 上自动创建或改写 Meeting。仍需按 8.4 节新建
验收 Meeting，确认 accepted Action renewal、`confirm-recorded`、Return-to-Board 后续控制循环以及
Desktop sidebar 终态收敛。现场验收前，状态保持“代码已修复、协议闭环待实跑确认”。

本次实现不需要数据库迁移，不删除或重写现有消息、Project View、Meeting、Action Run 或其他历史数据。
