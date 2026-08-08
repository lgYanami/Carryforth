# Meeting Action Finalization 逻辑主持人 ACK 与同步简化实现设计

> 状态：方案确认，待实现
>
> 日期：2026-08-08
>
> 范围：Meeting V2 Agent 主持调度、`buzz-acp` 工作槽与 ACP Session 生命周期、
> Action Finalization lease、runtime capability、System Contract、测试与发布；保留现有 Relay
> Action Run wire/DB 模型、Project Context、Human 主持和历史 Meeting 数据
>
> 关联设计：
> [Meeting V2 直接行动收口后端修正方案](./meeting-v2-direct-action-finalization-backend-plan.md)、
> [Meeting V2 Floor Decision 空等与 Action Finalization 硬超时修复设计](../../bug/meeting-v2-floor-decision-and-action-finalization-timeout-fix-design.md)、
> [Meeting Action Finalization 中维护 Project Context 的实现设计](../../project-context/meeting-action-finalization-context-write-implementation-design.md)

## 1. 结论

Action Finalization 的完成权威应与 Human 主持保持一致：

```text
冻结最终 Board
  -> 进入 finalizing_actions
  -> 主持身份直接完成 View / Document / Context 等业务写入
  -> 主持身份显式提交 actions-recorded ACK
  -> Relay 原子完成 Action Run 并关闭 Meeting
```

本文把 `End(attestation=actions-recorded)` 称为 **Action completion ACK**，避免与 Meeting Floor
中的 Offer ACK 混淆。Human 的 completion ACK 来自 Desktop 的“确认行动产出已完成并结束会议”；
Agent 的 completion ACK 来自模型返回 `COMPLETE` 后由 Harness 签署的同一种 End。二者不应拥有
两套不同的完成语义。

现有 Agent 路径额外把以下本地事实提升成了协议正确性前提：

- 执行最终 Board Maintenance 的物理工作槽；
- 该槽中的精确 ACP Session ID；
- `FinalControlCycle -> PendingAction -> Action -> ModeratorMeeting` 本地 binding phase；
- Board、Floor、Action 与 Return-to-Board 之间的 phase promotion；
- Action Turn 派发和返回时的 exact slot/session matching。

这些事实不是 Relay 可验证的授权边界，也不是 Action Finalization 的业务输入。正常情况下，Agent
Pool 本来就会优先选择已经拥有当前 Meeting channel Session 的工作槽；只有该槽不存在、繁忙、自然
轮换或进程重启时才会选择其他健康槽。该默认偏好足以提供常见情况下的上下文连续性，不需要再用一套
显式同步协议把它变成 fail-closed 条件。

本次修改因此采用以下边界：

1. 完整移除 Action-capable Meeting 的显式物理槽/ACP Session continuity binding；
2. Board、Floor、Action 都按逻辑主持 Agent 调度，优先复用已有 Meeting Session，但允许任意健康槽；
3. 冻结 Board 和 canonical Meeting envelope 是 Action Turn 的完整行动合同；
4. Action lease 继续保留，但只表达逻辑主持人仍在线工作，不绑定槽或 Session，也不代表完成；
5. `COMPLETE` / Human Confirm 继续生成相同的 `actions-recorded` ACK；
6. Relay 继续以主持身份、current run/window/Board fence 和事务 CAS 决定是否关闭；
7. 不新增 Materialization Plan、Step、Manifest、后台补写任务或跨系统事务。

本设计有意同时删除 Board Maintenance → Floor Decision → Action Finalization 之间的显式 exact
binding，而不是只删除 Action phase。原因不是三个 Turn 可以共享任意隐式上下文，而是三个 Turn 的
协议输入均已由 Relay-verified envelope、current Board、candidate/attempt fence 与 canonical State
自包含。已有 channel Session 只保留为 Agent Pool 的默认选择偏好。跨槽时 Relay fence 继续保证旧
Board、旧 Attempt 和旧 Action 结果不能提交，因此删除 binding 不降低协议 correctness。

## 2. 本次变更为什么是 Meeting 生命周期调整

### 2.1 当前 Human 路径

Human 主持进入 Action Finalization 后：

1. Relay 冻结最终 Board 并创建当前 Action Run；
2. Desktop/Tauri 以 `{relay, signer, meeting, run, window, board}` 注册后台续期；
3. Human 可离开 Meeting 页面，在 Project View 或其他界面完成操作；
4. Human 返回后点击 Confirm；
5. Desktop 签署带 current run/window/Board fence 的 `actions-recorded` End；
6. Relay 在同一事务中结束 Action Run、Meeting 并归档 Meeting Channel。

Human 路径不验证浏览器页面、React component、Desktop window 或某个 UI Session。续期只维持
Action Run 活性，真正完成由显式 Confirm 决定。

### 2.2 当前 Agent 路径额外增加的门禁

Agent 路径除了使用同一个 Action Run 和 ACK，还会：

```text
Board Turn 返回
  -> 记录 {agent_index, acp_session_id}
  -> 绑定 FinalControlCycle
Floor 选择 FINALIZE_ACTIONS
  -> promote PendingAction
Relay 接受 Action Begin
  -> promote Action
Action Turn
  -> claim_exact_meeting
  -> 返回后再次校验 agent_index + resolved_session_id
```

任意 binding 缺失、ACP Session 自然轮换、进程替换或状态竞态都会被收敛为 `affinity_lost`，即使：

- 逻辑主持 Agent 身份没有变化；
- frozen Board、Action Run 和 Meeting State 全部有效；
- Agent 有其他健康工作槽；
- Relay 仍接受该主持 Agent 的续期与 ACK。

这把“尽量复用上下文”的性能/体验优化错误地提升为了“能否完成会议”的 correctness gate。

### 2.3 本次事故证明该门禁没有提供可靠安全性

最近一次验收中，Action Begin 两次因 `moderator_floor_not_idle` 被 Relay 拒绝。ACP 把命令拒绝错误
解释为 continuity 终止并删除 `MeetingSlotBinding`。Relay 后续接受 Begin 时，本地 promotion 无法
重建已删除的 binding，Action Turn 在执行任何业务工具前即以 `affinity_lost` 阻塞：

```text
progress_seq = 0
lease renewals = 0
business writes = 0
```

Project Context 提示词、Document/Edge 写入和 Action lease 均尚未开始执行。事故来自本地 binding
生命周期，而不是物化工作本身。

## 3. 目标生命周期

### 3.1 主路径

```text
discussion
  -> final Board Maintenance
  -> Floor Decision = FINALIZE_ACTIONS
  -> Relay 接受 Action Begin
  -> finalizing_actions / runnable
  -> ACP 为逻辑主持 Agent 调度唯一 Action Turn
  -> 优先选择已有 Meeting Session 的健康槽，否则选择其他健康槽
  -> 注入 frozen Board + canonical action fence + Role/Project Context
  -> Agent 直接调用普通业务工具并 canonical 回读
  -> Agent 返回 COMPLETE
  -> Harness 签署 End(attestation=actions-recorded, current fence)
  -> Relay 原子写入 completed_closed + ended/closed + Channel archive
```

Human 与 Agent 的差异只剩 ACK 输入来源：

| 主持类型 | 执行业务动作 | 完成输入 | Relay 终态操作 |
|---|---|---|---|
| Human | Desktop、CLI 或其他现有业务界面 | 点击 Confirm | `actions-recorded` End |
| Agent | Action Turn 中的普通 `buzz` CLI | 返回 `COMPLETE` | `actions-recorded` End |

### 3.2 工作槽选择

Board、Floor 与 Action Turn 都不再要求 exact slot。Agent Pool 使用普通 Meeting channel 偏好：

1. 优先选择已经持有该 Meeting channel ACP Session 的空闲槽；
2. 若没有，则选择符合 Meeting 预留容量规则的其他健康槽；
3. 若暂时没有槽，保持 request pending，不改变 Relay canonical Action Run；
4. 槽可用后只派发一个 Action Turn。

这保证常见情况下仍然自然复用原槽/原 Session，但没有任何协议语义依赖该结果。

### 3.3 Action Turn 自包含

无论使用旧 Session 还是新 Session，每次 Action Turn 必须从 canonical 状态构造完整输入：

- `meeting_id`、当前主持人和 policy；
- current `action_run_id`、`action_window_epoch`、`board_event_id`；
- frozen final Board 的完整正文；
- 当前 Meeting State / control fence；
- 当前 Role Brief 或 Role Binding；
- Action Turn 可写工具策略；
- Project View、Document、Project Context 的定位、写回和 canonical readback 要求；
- `COMPLETE | BLOCK | RETURN_TO_BOARD | ABORT` 输出契约。

前一 ACP Session 中没有进入 frozen Board 的隐式聊天记忆不再是协议输入。frozen Board 是完整的
**Meeting 决策输入**，但不是业务授权凭证；Community membership、Role/Assignment、工具策略及各
业务域 CAS 仍从 current canonical 状态读取并由原有门禁校验。若 Board 信息不足以安全物化，Agent
必须 `RETURN_TO_BOARD`，不得依赖旧 Session 猜测。

### 3.4 显式 ACK 是唯一完成权威

下列事实都不能自动完成 Meeting：

- lease 持续续约；
- Agent Turn 已启动或结束；
- 发生一个或多个 Project View / Document / Context 写入；
- 工具进程退出；
- Desktop 或 ACP 显示“已物化”；
- Board 中列出的对象似乎已经存在。

只有 current frozen moderator 身份提交的、通过 current action fence 校验的
`actions-recorded` End 才能完成 Action Run 和 Meeting。

## 4. 保留的最小同步与安全边界

简化不等于删除所有生命周期 fence。以下机制继续保留。

### 4.1 冻结主持身份

- host-originated completion ACK、renew、block、retry 和 return 继续校验 frozen moderator
  pubkey；
- 同一逻辑 Agent 的不同槽共享该 Agent 身份，因此不需要以 slot/session 作为第二授权层；
- 其他 Community Agent、Human member 或旧主持身份不能接管 Action Run。

既有 Community owner/admin administrative abort 保持原授权边界，不得在本次实现中误收紧为
moderator-only；主持人主动 abort 与 operator abort 继续由 Relay 区分来源并生成真实终态。

### 4.2 Finalization fence

继续复用：

```text
ActionRunKey {
  meeting_id,
  action_run_id,
  action_window_epoch,
  board_event_id
}
```

该 key 同时用于：

- 本地 pending/running 去重；
- renewal 单调序列；
- BLOCK / RETRY / RETURN_TO_BOARD；
- `actions-recorded` ACK；
- 拒绝旧 window、旧 Board 或旧 run 的迟到操作。

它不包含 `agent_index`、ACP Session ID、Codex process ID 或 provider session ID。

### 4.3 单执行 Turn

同一个 Meeting 在一个逻辑 Agent Harness 中最多存在一个 pending 或 running host Turn；同一个
`ActionRunKey` 最多存在一个 Action Turn。Coordinator 维护简单的 key 集合或直接复用现有
request/running-turn 去重：

- canonical State/outbox 重放不得重复派发；
- Relay reconnect 不得重复派发；
- 两个空闲槽不得同时取得同一个 ActionRunKey；
- Retry 产生新 window 后，旧 key 终止，新 key 才可派发。

该去重不保存物理槽或 Session，也不承担权限判断。

Retry、Return-to-Board、Abort 或新 canonical Board 到达时，必须先建立**旧 Turn 停止屏障**：

1. 对旧 pending/running Turn 发送 canonical cancel；
2. 等待 task join 或 PromptResult 明确返回；
3. 若 provider/tool 无法确认停止，则替换该旧 Agent process 并等待退出；
4. 只有确认旧 Turn 不再可能发起工具调用后，才允许新 window 或新 Board Turn 派发；
5. 屏障超时则保持 blocked/pending，不得让旧、新两个槽并行物化。

process generation 可用于诊断和停止确认，但不得重新成为 Relay 权限或 ACP Session affinity。

### 4.4 ACK 原子性与幂等

- 相同签名 End 重放返回同一 receipt；
- 两个不同 ACK 并发时只允许一个事务提交终态；
- Meeting `ended/closed`、Action Run `completed_closed`、Channel archive 和终态 outbox 必须原子提交；
- blocked、旧 run/window/Board 或已 return-to-board 的 ACK 必须拒绝且不得部分关闭。

### 4.5 外部业务 CAS

Meeting 不尝试为 View、Document、Context 提供跨域 exactly-once：

- Project View 继续使用 object/project revision；
- Document 继续使用 document/catalog revision；
- Context Edge 继续使用 exact coordinate key、Context revision 和 `no_change`；
- Retry 或进程恢复必须先回读 canonical 状态，只补缺失项；
- 已接受的外部写入不因 BLOCK、RETURN_TO_BOARD 或 ABORT 回滚。

## 5. Action lease 的新定位

### 5.1 保留 lease，但不再参与 affinity

Human 当前也使用后台 Action lease renewal。本次保留现有 renewable lease，因为它可以区分：

- 主持人仍在线且正在工作；
- Desktop/ACP 已离线或停止；
- current window 与旧 window；
- 可继续的 Action Run 与需要显式恢复的 blocked run。

Agent renewal 改为由 ACP 进程级 registry 绑定逻辑主持人和 `ActionRunKey`，不得读取或校验：

- 物理工作槽；
- ACP Session ID；
- provider process generation；
- 上一个 Board/Floor Turn 所在槽。

registry 在首次观察到 canonical `finalizing_actions/runnable` 时注册，不等待工作槽实际可用。它覆盖：

- Action request 等待工作槽；
- Action Turn 正在运行；
- Action Turn 已返回 `COMPLETE`、但 completion ACK receipt 仍不确定；
- 页面、observer 或普通 channel 导航变化。

terminal、blocked、Return-to-Board、Abort 或新 action window 会按 exact ActionRunKey 停止/替换旧
renewal claim。首版保持 wire 不变，pending 等槽时复用现有 `reasoning` progress stage；不得因没有
`turn_id` 而停止续期。prepared signed renewal 和 `progress_seq` 继续按现有幂等/单调规则处理。

### 5.2 lease 不决定完成

- renewal 只更新 cooperative liveness 与可观察 progress；
- renewal 不授予 Community 或业务写权限；
- renewal 不证明任何外部写入成功；
- renewal 不替代 `COMPLETE` / Confirm；
- renewal 不允许旧 action window 复活；
- 现有 operator safety cap 保持为独立运维边界；即使触发也只能把 run 转为 blocked，不能代替
  主持 ACK、自动关闭 Meeting 或重新绑定某个槽/Session。本次不调整该 cap 的取值与配置模型。

### 5.3 lease 停止

当 lease 停止并到期时：

- Relay 可继续把 current run 收敛为 `blocked`；
- Meeting 保持 `finalizing_actions`，不得自动 closed；
- 已发生的外部效果保持；
- 恢复不尝试寻找原 ACP Session；
- Retry 创建新 window，Coordinator 基于新 ActionRunKey 调度一个新 Turn；
- 新 Turn 首先回读所有目标域 canonical 状态。

`affinity_lost` 退役为历史只读 reason code：历史 Meeting 中已有值继续解析和展示；current v4
capability 的 ACP、SDK、Desktop 和 Relay BLOCK write whitelist 不再生成或接受新值。真实 provider/
process failure 使用现有 `provider_failure` 等可恢复原因，不再伪装成 Session affinity 失败。

ACP/Harness 进程重启是一个特殊边界，恢复顺序必须固定：

1. canonical Meeting 或 Action Run 已终态：停止恢复，不重放任何本地命令；
2. ledger 为 **current ActionRunKey + current Board fence** 保存了 exact prepared signed End：只幂等发布或
   重放这一个 End，禁止重新执行物化 Turn，也不先把 run 收敛为 BLOCK；
3. 不满足前两项、但启动时发现一个在本进程启动前已经存在的 runnable Action Run：不能自动重做可能
   已产生部分外部效果的 Turn。Coordinator 把它视为 orphaned execution，提交或等待收敛为普通可恢复
   BLOCK，再由 frozen moderator 身份显式 Retry。

prepared renewal 表示旧进程当时仍存活，跨进程重启后已经失真，因此不得恢复重放。prepared End 若与
current run/window/Board 任一 fence 不一致，也不得发布，转入第 3 项。只有当前进程已经观察、去重并
确认“从未 dispatched”的 pending request，才能在同一进程中等待槽后首次派发。

## 6. `buzz-acp` 实现修改

### 6.1 删除显式 Meeting slot binding

从 `crates/buzz-acp/src/pool.rs` 删除只服务于该机制的生产结构和方法：

- `meeting_slot_bindings`；
- `MeetingSlotBindingPhase`；
- `MeetingSlotBinding`；
- `ExactMeetingClaimError`；
- `bind_meeting_slot()`；
- `claim_exact_meeting()`；
- `meeting_slot_binding()`；
- `idle_bound_meeting_ids()`；
- `promote_meeting_slot_binding()`；
- `release_meeting_slot_binding()`；
- `slot_is_meeting_bound()` 及因此产生的 claim 排除规则。

移除后 `try_claim_inner(Some(meeting_id))` 自然保留“优先已有 channel Session，否则任意健康槽”的
默认行为。

### 6.2 删除 continuity phase orchestration

从 `crates/buzz-acp/src/meeting.rs`、`meeting_v1.rs` 和 `lib.rs` 删除：

- `MeetingContinuityDirective`；
- `ReleaseFinalControl`；
- `PromoteAction`；
- `PromoteModeratorMeeting`；
- `apply_meeting_continuity_directives()`；
- `continue_meeting_continuity_failure()`；
- `continuity_phase_name()`；
- `superseded_meeting_binding_phase()`；
- canonical State 驱动的 binding promotion/release；
- rejected Action Begin 驱动的 binding release。

Meeting terminal、Return-to-Board 和 canonical preemption 仍需结束或取消过期 Turn，但只依据
`turn_id + ActionRunKey + canonical State`，不再操作 slot binding。

### 6.3 删除结果侧 ACP Session 匹配

`PoolEvent::Result` 对 V2 moderator/action Turn 不再：

- 读取 `resolved_session_id` 作为 Meeting 正确性证据；
- 比较 result `agent_index` 与历史 binding；
- 因 provider/session rotation 直接令业务结果 `meeting_succeeded=false`；
- 调用 `mark_continuity_lost()` 自动发布 Action BLOCK；
- 为 Meeting continuity 延迟正常 Session rotation。

如果 `PromptOutcome` 本身失败，仍按真实原因处理：provider failure、timeout、lease expiry、明确
cancel 或 canonical supersession。不能再把不同故障统一改写成 `affinity_lost`。

若 `resolved_session_id`、`rotation_deferred`、`deferred_channel_rotations` 和
`defer_session_rotation` 在移除 continuity 后没有其他调用者，应连同字段、分支和测试一起删除，避免
留下无效的半套状态机。

### 6.4 简化 dispatch

`dispatch_meeting_pending()` 对 Action-capable moderator Turn 使用普通 channel-aware claim：

```text
claim_action_turn(meeting_id, ActionRunKey):
  if key already pending/running:
    no-op
  else if preferred existing-session slot is available:
    claim it
  else if an eligible Meeting slot is available:
    claim it
  else:
    requeue without changing canonical state
```

初版统一使用 `try_claim_meeting_board(meeting_id)`：优先复用 channel Session，并继续尊重更强的
Offer/Grant reservation floor。不要让实现者在 `try_claim_meeting()` 与
`try_claim_meeting_board()` 间自由选择。同步修正 `available_agent_slots`、
`board_request_needs_extra_slot()`、`claimable_unleased_count()`、普通 channel claim 排除和 Pool
exhaustion 判断，确保删除 bound slot 后既不超卖，也不把可用槽错误排除。

### 6.5 canonical request 去重

Action Turn request 必须携带或可稳定导出完整 `ActionRunKey`。下列来源产生同一个 key 时只保留一个：

- Relay-signed State；
- outbox/reconnect replay；
- coordinator periodic reconcile；
- Begin receipt 先于或晚于 canonical State；
- 同一 State 的重复订阅事件。

late result 只有在其 key 仍是 current running key 时才能解释模型输出。若 canonical State 已进入新
window、Return-to-Board、ended 或 aborted，旧 result 只记为 superseded，不签 ACK、不执行恢复动作。
新 key 的派发还必须通过第 4.3 节的旧 Turn 停止屏障；仅把旧 result 标记 superseded 不足以阻止旧
tool call 在后台继续落盘。

### 6.6 readiness 与 Begin rejection 同步修正

删除 binding 可以避免本次 `affinity_lost`，但不能保留产生提前 Begin 的 predicate drift。ACP 的
no-candidate Floor readiness、request matcher 和 prepared Begin replay gate 必须与 Relay 统一：

```text
simple FINALIZE_ACTIONS Begin allowed only when:
  baton.phase = moderator_idle
  AND active_decision_attempt = none
  AND next_action_at = none
  AND unresolved_floor_work = none
  AND frozen Board / control fence are current
```

Candidate-Cohort Floor 继续使用明确的 `expected_decision_attempt_id`。`moderator_floor_not_idle`、
`floor_work_pending` 或 stale fence rejection 只触发 canonical reconcile，不得改变 ACP Session、不得
BLOCK Action、不得重复派发业务 Turn。

### 6.7 本地 ledger 与 observer

从 Meeting host ledger 删除 `v2_continuity` 及其：

- `agent_index`；
- `acp_session_id`；
- `phase`；
- `meeting_v2_continuity_bound`；
- `meeting_v2_continuity_lost`。

旧本地 JSON 中多余的 `v2_continuity` 字段按 serde 默认未知字段策略忽略；不删除整个 ledger，不影响
Meeting、Channel 或 Project 数据。

不新增第二份持久化 dispatch 真相源。复用现有 `V2ActionFinalizationRecord` 保存 canonical run/window/
Board、prepared renewal/End 和当前 turn ID；pending/running 单执行集合及停止屏障留在进程内。可增加
以下低复杂度诊断字段，但不得与 `V2ActionFinalizationRecord` 重复保存 canonical fence：

- `meeting_id`；
- `action_run_id`；
- `action_window_epoch`；
- `board_event_id`；
- `turn_id`；
- `dispatch_state = pending | running | finished | superseded`；
- 可选 `selected_slot` 仅用于诊断，不参与恢复或 Relay 命令。

Meeting ledger 保持 version 7：旧 v7 fixture 必须兼容读取，`v2_continuity` 被忽略，prepared signed
End/renewal 和 Action recovery 状态不得丢失。进程启动后严格按第 5.3 节恢复：current fence 的 exact
prepared End 优先于 orphaned BLOCK，且只重放 End；prepared renewal 不跨重启重放；其余 pre-start
runnable execution 才收敛为 orphaned BLOCK，绝不自动重放业务 Turn。

### 6.8 runtime capability 切换

这是 Meeting 执行合同的代际变化，不能让旧 exact-affinity ACP 继续满足新建 Meeting 的 roster
readiness gate：

- runtime capability `meeting-v2-action-finalization-v3 -> v4`；
- `buzz-sdk` 保留 v3 常量仅用于历史诊断，current 常量改为 v4；
- `buzz-acp capabilities --json` 删除
  `moderatorContinuity=exact_agent_slot_and_acp_session`；
- 新 capability 明确公告：
  `moderatorExecution=logical_agent_channel_session_preferred` 与
  `actionCompletion=explicit_actions_recorded_ack`；
- Agent profile reconcile 必须移除旧 v3 后写入 v4，不能把两个代际同时保留为 active；
- Relay Create gate、Desktop/Tauri capability probe、SDK tests、live acceptance 与运维脚本全部要求 v4；
- 不提供 v3/v4 新建 Meeting 双轨。

## 7. Relay、DB、SDK、CLI 与 Desktop 边界

### 7.1 Relay / DB

本次不改变 Relay Action wire 和数据库模型：

- 保留 `moderated-board-actions-v3`；
- 保留 `meeting_v2_action_runs`；
- 保留 `action_run_id`、window、Board fence、lease、progress、terminal status；
- 保留 Begin/Renew/Block/Retry/Return-to-Board；
- 保留 `End(attestation=actions-recorded)`；
- 保留 Action Run 与 Meeting 原子关闭；
- 保留 finalizing Meeting 的 Project Context attachability；
- Relay 只把新建 roster runtime gate 从 action-finalization-v3 切换为 v4。

Action wire shape 不变，但 BLOCK reason whitelist 停止接受新的 `affinity_lost`；历史 DB 行和 read model
继续支持该值。

Relay 从未验证 ACP slot/session，因此 Action 命令无需新增字段或 migration。runtime capability 代际按
第 6.8 节切换，用于阻止旧 Harness 创建新 Meeting，不改变已结束 Meeting 的读路径。

### 7.2 SDK / CLI

现有命令和 event builders 继续有效：

```text
buzz meetings actions status
buzz meetings actions block
buzz meetings actions retry
buzz meetings actions return-to-board
buzz meetings actions confirm-recorded
```

不新增单独 ACK kind，也不引入 `complete -> close` 两阶段。Agent 的 `COMPLETE` 仍由 Harness 转换为
现有 `confirm-recorded` 语义。SDK 只更新 current runtime capability 常量和历史常量命名，不修改
Action Begin/Renew/End wire schema；同时从 current BLOCK reason builder/input 移除
`affinity_lost`，历史 read model 保持兼容。

### 7.3 Desktop

Human 操作流程保持不变。Desktop 只需：

- 继续展示 Action Run、renewal、blocked、return 和 confirm；
- 从新 BLOCK 输入选项移除 `affinity_lost`；
- 历史记录仍能显示旧 `affinity_lost`；
- Agent host observation 不展示“必须恢复原槽/原 Session”的误导文案；
- UI 不需要知道本次 Action Turn 使用了哪个槽。

## 8. System Contract 与 Prompt 修改

### 8.1 稳定合同

这是稳定运行语义变化，需要完整切换：

- Meeting System Contract `3 -> 4`；
- Project Space Contract `6 -> 7`；
- 逐 Turn Meeting envelope `meeting-context-v2 -> v3`。

合同中删除：

- “same Meeting slot”；
- “same ACP Session”；
- 原 Session continuity 是 Action 正确性前提；
- Session 丢失必须 `BLOCK(affinity_lost)`。

合同中固定：

- frozen Board 是 Action Finalization 的唯一会议行动合同；
- current moderator identity 是主持权威；
- Harness 可在同一逻辑 Agent 的任意健康槽执行 Action Turn；
- Board 只携带 Meeting 决策，不授予业务权限；所有 Community、Role/Assignment、工具与 revision
  门禁必须重新读取 canonical 状态；
- Action Turn 内仍必须按顺序完成 materialize、canonical readback、Document、Context Edge、Edge
  readback；
- 信息不足返回 `RETURN_TO_BOARD`；
- 可恢复业务失败返回 `BLOCK`；
- 只有 `COMPLETE` 会生成 actions-recorded ACK。

三种合同/Envelope 的旧 Session 失效与重建都要有自动化测试；切换不得依赖旧 Session 恰好在首次
Action Turn 前自然轮换。

### 8.2 Turn 内连续性

本方案只取消**跨 Turn、跨阶段**的物理 affinity。一个已经开始的 Action Turn 仍由取得它的单个工作槽
执行到模型结果、明确取消、真实 provider failure 或 lease expiry。系统不会在一个正在运行的 tool call
中途把 Turn 迁移到另一槽。

## 9. Project Context 语义保持

现有 Project Context 设计无需回退：

1. verified `finalizing_actions` Meeting 仍可作为坐标；
2. resolver 仍验证 active Action Run 与 frozen Board；
3. Agent 在 Action Turn 中完成 View / Document / Edge 写入；
4. Edge canonical 回读成功后才可 `COMPLETE`；
5. Context write/readback 失败返回 `BLOCK`；
6. Board 无法解释关系时返回 `RETURN_TO_BOARD`；
7. terminal Meeting 仍按原规则可作为坐标。

需要修订关联文档中“同一个 ACP Session / 工作槽”的表述，替换为：

> 在同一个 Action Finalization Turn 中完成业务物化、解释 Document、Context Edge 与 canonical
> readback；该 Turn 属于同一逻辑主持 Agent，但不要求继承讨论阶段的物理槽或 ACP Session。

文档迁移矩阵：

| 文档类别 | 处理方式 |
|---|---|
| `meeting/v2/meeting-v2-action-finalization-design.md` | 更新现行五步规范，指向本文 |
| `meeting/v2/meeting-v2-backend-operations.md` | 更新 runtime capability、运维检查与 ACK 语义 |
| `meeting/fix/meeting-v2-direct-action-finalization-backend-plan.md` | 标注 slot/session 条款被本文 supersede，保留历史设计背景 |
| `meeting/fix/meeting-v2-agent-context-optimization-design.md` | 删除 exact Session correctness，保留 self-contained envelope |
| `meeting/desktop/*spec*`、acceptance、implementation plan | 更新 capability 与用户可见恢复文案 |
| `project-context/meeting-action-finalization-context-write-implementation-design.md` | 把“同槽同 Session”改为“同一 Action Turn/逻辑主持 Agent” |
| 历史 bug 文档 | 保留事故事实，追加“后续由本文取代 affinity 方案”的注记，不改写历史 |

## 10. 故障与恢复语义

### 10.1 原槽或 Session 不存在

这不再是错误。Coordinator 选择其他健康槽，注入完整 frozen Board 和 canonical envelope 后执行。

### 10.2 Agent 进程重启

- 重启后读取 canonical `finalizing_actions` State；
- canonical Meeting/Action Run 已终态时清理本地恢复状态并结束；
- 若 ledger 存在与 current run/window/Board 完全一致的 prepared signed End，只幂等发布/重放该 End，
  不重跑业务 Turn、不先 BLOCK；
- prepared End fence 不一致时不得发布，prepared renewal 一律不得跨重启重放；
- 其余 current run 若在进程启动前已存在且仍 runnable，则视为 execution outcome unknown，不自动调度；
- Harness 使用现有 `provider_failure` 等普通可恢复原因将 orphaned run 收敛为 BLOCK，或停止 renewal
  等待 Relay lease recovery；两条路径只能选择一种固定实现，不得同时竞态；
- frozen moderator 显式 Retry 后，才按新 window key 唯一调度；
- 若 lease 已经 blocked，则直接等待合法 Retry；
- 不查找旧 ACP Session，不生成 `affinity_lost`。

### 10.3 部分外部写入后失败

- Meeting 保持 finalizing/blocked；
- 已发生写入不回滚；
- Retry 新 Turn 首先回读 View、Document、Context；
- 已存在对象按 revision 更新或确认；
- exact Edge 已存在时接受 `no_change`；
- 确认完整后再 ACK。

### 10.4 ACK 响应不确定

同一进程内，或重启后 ledger 仍持有与 current ActionRunKey/Board fence 完全一致的 prepared signed
End 时，只重放同一个已签名 End，不创建第二个 ACK，也不重新执行物化。Relay receipt 或 canonical
terminal State 确认最终结果。若 canonical 已终态则直接结束；若 fence 已变化则旧 End 失效并禁止
发布，按第 10.2 节的 orphaned 路径处理。

### 10.5 Return-to-Board / Abort

- Return 原子终结旧 Action Run 并打开新 Board window；
- 旧 ActionRunKey 的 pending/running Turn被 canonical advance 取消或标记 superseded；
- 旧 ACK、renewal 和结果全部拒绝/忽略；
- Abort 原子结束 Meeting，外部效果保留；
- 两者都不需要恢复原槽。

## 11. 测试与验收矩阵

### 11.1 ACP 调度

1. 原槽可用时优先复用，COMPLETE 正常关闭；
2. 原槽不存在时选择其他健康槽，不得产生 `affinity_lost`；
3. 原 Session 自然轮换后新 Session 可执行 Action；
4. 原槽 Busy 时 request 保持 pending 或按容量策略选择其他槽；
5. 同一 State/outbox/reconnect 重放只派发一个 Action Turn；
6. 两个空闲槽并发时只有一个取得同一 ActionRunKey；
7. 其他逻辑 Agent 不能接管主持 Action；
8. Board 在槽 A、Floor 在槽 B 时，candidate attempt、Board/control fence 与决策均正确；
9. Board rejection/rebase 后的新槽只能看到并提交 current Board；
10. Return 后的新 Board Turn 可在新槽运行，旧 Action Turn 必须先通过停止屏障；
11. 并行度 1 与 4 下，多 Meeting + DM + active Offer/Grant + Board reservation 不饿死、不超卖；
12. 普通 DM/channel Session rotation、上下文复用与容量统计无回归。

### 11.2 Begin rejection 与竞态

1. `moderator_floor_not_idle` rejection 后零业务 Turn、零 BLOCK；
2. Relay 回到 `moderator_idle` 后 Begin 成功并只派发一次；
3. stale State/Board/control rejection 只触发 canonical reconcile；
4. Begin receipt 与 canonical State 任意先后顺序结果一致；
5. 两个不同 Begin 并发只创建一个 current run；
6. late rejection 不得取消已经由新 canonical State建立的 Action request。

### 11.3 lease

1. Action Turn 换槽/换 Session 后 renewal 仍成功；
2. 长物化跨越至少三个 TTL 并最终 ACK；
3. duplicate renewal 幂等；
4. 旧 progress sequence、旧 run/window renewal 被拒绝；
5. renewal 停止只转 blocked，不关闭 Meeting、不回滚外部写入；
6. Retry 后旧 renewal 和旧 ACK 均不能复活旧 window；
7. Human `waiting_human` 与 Agent renewal 具有相同 liveness 语义；
8. 所有槽 Busy 超过三个 TTL 时，pending Action 仍由进程级 registry 续期且不误 blocked；
9. operator safety cap 仍按现有配置独立生效，只能 BLOCK，不能自动 ACK 或关闭。

### 11.4 崩溃与部分写入

1. 当前进程内尚未 dispatched 的 pending request 可在槽可用后首次调度；
2. ACP 重启发现 pre-start runnable run 且无 exact prepared End 时不得自动物化，先收敛 BLOCK，显式
   Retry 后在新槽执行；
3. End 已签名、发布前崩溃时，重启后只发布同一个 End，零业务工具调用；
4. End 已发布、receipt 前崩溃时，重启后重放同一事件并得到同一终态；
5. prepared renewal 不得跨重启重放；
6. View/Document 已写、Context 未写时，只补 Context；
7. Context 已写、End 签名前崩溃时，exact attach 返回 `no_change` 后 ACK；
8. 任意恢复都不依赖旧 ACP Session；
9. 旧 tool call 阻塞时，Retry/Return/Abort 不得派发新 Turn，直到 cancel/join/process-exit 屏障完成；
10. 停止屏障超时保持 blocked/pending，不出现两个并行物化 Turn。

### 11.5 ACK 与终态

1. 非主持人 ACK 拒绝；
2. current host + current run/window/Board ACK 原子关闭；
3. 相同 ACK 重放幂等；
4. 并发 ACK 只有一个终态；
5. stale Board/run/window、blocked run 的 ACK 零部分写入；
6. Board 明确无需新增外部写入时允许主持人确认完成；
7. provider failure 或非法模型输出不得生成 ACK。

### 11.6 Return / Abort / Context

1. 部分写入后 Return 保留外部效果，旧 ACK 失效；
2. 新 Board 再次 Finalize 使用新 run/window；
3. Action 中 Abort 保留外部效果并生成 aborted 终态；
4. finalizing Meeting + current run + frozen Board 仍可 attach Context；
5. Context conflict/unavailable 触发 BLOCK，不关闭 Meeting；
6. View -> Document -> Context -> canonical readback -> ACK 顺序完整；
7. ended/closed 与 ended/aborted Meeting hydration 保持现有真实 outcome；
8. attach 与 End、Return、administrative abort 的提交顺序均确定且无死锁；
9. Context 先提交后 Return/Abort 时 Edge 保留，旧 ACK 失效；
10. View/Document 已写、Context conflict 后 Retry 不重复对象且 revision 精确；
11. fallback 槽 envelope 含 writable Context policy 与 exact Meeting coordinate。

### 11.7 capability、合同与历史兼容

1. roster 中任一 managed Agent 仅公告 v3 capability 时，新 Meeting Create fail closed；
2. 全部 runtime 公告 v4 后 Create 成功；
3. profile reconcile 移除旧 v3，不产生 v3+v4 双 active；
4. Meeting Contract 4、Project Space 7、meeting-context-v3 各自令旧 Session 正确重建；
5. 历史 `affinity_lost` snapshot/terminal record 仍可读；
6. 新验收 DB 无新增 `BLOCK(reason=affinity_lost)`，observer 无 continuity-lost event；
7. 旧 ledger v7 fixture 保留 prepared End/renewal 与 canonical Action state。

### 11.8 真实 Provider 验收

至少完成两场新 Meeting：

1. 默认槽复用路径；
2. 测试环境中主动让 Action 前 Session 轮换或原槽不可用，验证 fallback 槽成功物化、续期和关闭。

验收必须确认：

- 无 `meeting_v2_continuity_bound/lost`；
- 无新 `affinity_lost`；
- 每个 run/window 只有一个 Action Turn；
- renewal 持续推进；
- View、Document、Context canonical 回读通过；
- `actions-recorded` ACK 与 Meeting closed 原子成立。

## 12. 数据、迁移与发布安全

### 12.1 数据库与历史数据

- 不新增、删除或重写数据库表；
- 不删除 Meeting、Speech、Board、Action Run、Document、Context Edge 或 Project View 数据；
- 不回填历史 `affinity_lost`；
- 历史 action lease 和 observer 记录继续只读；
- 不执行 destructive migration test、TRUNCATE、DROP、reset 或重新初始化主开发数据库。

### 12.2 本地 ACP ledger

删除的是 Harness 本地派生 continuity 字段，不是 canonical 数据。读取旧 ledger 时忽略旧字段；写入新
ledger 时不再生成它。不得以简化为由删除整个 `~/.local/state/buzz` 或 `~/.buzz-dev`。

### 12.3 切换方式

本次不维护新旧 ACP continuity 双轨。发布前：

1. 只读确认没有 active Meeting 和 non-terminal Action Run；
2. 若存在则停止切换，等待正常结束或由用户明确处理；
3. 构建并同时重启受管 ACP；
4. Contract 4/7 生效后再创建新 Meeting；
5. 不因部署自动 abort 或修改任何 Meeting；
6. 不清理 Relay/数据库 volume。

### 12.4 测试数据库 fail-closed 门禁

涉及 migration、truncate、fixture reset 或 destructive lifecycle 的测试只能使用独立 scratch DB：

- runner 创建名称含固定前缀 `buzz_meeting_sync_scratch_` 的临时数据库；
- 解析最终 `DATABASE_URL` 后校验数据库名，不匹配前缀立即退出；
- 显式拒绝已知主开发数据库名及当前 Relay 正在使用的数据库；
- 禁止测试脚本继承未解析/未确认的环境变量后直接执行 drop/reset；
- 测试结束只删除本次生成并精确记录名称的 scratch DB；
- 真实 smoke 前后只读比对主库 Community、Meeting、Project View、Document、Context 基线计数和固定
  对象，发现非验收预期变化立即停止。

文字约束不能代替上述可执行门禁。

## 13. 分阶段实现顺序

### 阶段一：合同与调度骨架

- 增加 ActionRunKey 本地唯一调度语义；
- 让 Action Turn 使用 channel-aware preferred-slot claim；
- 移除 dispatch 的 exact claim 和 missing-binding block；
- 更新 Meeting / Project Space / Turn envelope contracts；
- 切换 runtime capability v4 及 Relay/Desktop roster gate；
- 增加新槽/新 Session Action prompt 自包含测试。

阶段 review：确认 slot/session 不再出现在权限、ACK 或 Action 正确性判断中。

### 阶段二：删除 continuity 状态机

- 删除 MeetingSlotBinding 结构、phase、promotion、release 和 exact slot 集合；
- 删除 result-side affinity matching、rotation defer 和 v2_continuity ledger；
- 删除失效 observer 事件与测试；
- 保留普通 channel Session 偏好和 canonical turn fencing。

阶段 review：确认没有半套 binding、静默 promotion 或 `affinity_lost` 自动 BLOCK 残留。

### 阶段三：readiness、恢复与 UI 文案

- 对齐 ACP/Relay Action Begin readiness；
- 修正 rejection/replay/canonical ordering；
- 实现独立于槽/Turn 的进程级 renewal registry；
- 实现旧 host Turn cancel/join/process-exit 停止屏障；
- 验证 lease、Retry、Return、Abort 和 completion ACK；
- 更新 Desktop 历史/新状态文案；
- 修订关联 Meeting 与 Project Context 文档。

阶段 review：确认 Human/Agent 只在输入来源不同，Relay 完成语义完全一致。

### 阶段四：全量回归与现场验收

- 运行 Rust fmt、相关 clippy/unit/integration；
- 运行 Desktop lint/unit/E2E；
- 使用独立 scratch DB/Community 做失败矩阵；
- 在保留主开发数据的环境做两场真实 Provider smoke；
- 清理增量构建缓存，但不删除运行数据。

阶段 review：逐项对照本文完成标准后再提交交付记录。

## 14. 完成标准

全部满足才可标记已实现：

1. V2 Action-capable Meeting 不再创建或读取 `MeetingSlotBinding`；
2. Board/Floor/Action 不再以 exact ACP Session 为 correctness gate；
3. 原槽或 Session 丢失不会生成新的 `affinity_lost`；
4. Agent Pool 仍默认优先复用当前 Meeting Session；
5. fallback 槽能收到完整 frozen Board 和 canonical action envelope；
6. 每个 ActionRunKey 最多一个 pending/running Turn；
7. Board/Floor 跨槽及 Pool 并行度 1/4、reservation/普通消息回归通过；
8. Agent lease 在 pending、换槽和换 Session 后仍可续约；
9. Retry/Return/Abort 前旧 host Turn 停止屏障生效，不存在并行物化；
10. ACP 重启不自动重放 outcome unknown 的 Action；current-fence prepared End 只做幂等 End 重放；
11. runtime capability v4、Contract 4/7、meeting-context-v3 完整切换；
12. `COMPLETE` 与 Human Confirm 都生成同一 actions-recorded completion ACK 语义；
13. stale ACK/renew/result 不能影响新 window 或新 Board；
14. Return-to-Board、Abort、部分写入和重启恢复不依赖旧 Session；
15. Project Context finalizing attach、写入、回读和终态 hydration 无回归；
16. 新验收 DB 无 `BLOCK(reason=affinity_lost)`，历史记录仍可读；
17. 无数据库破坏、无 canonical 数据删除、无历史协议回写；
18. 真实新槽验收可以完成物化、续期、completion ACK 与正常 closed。

## 15. 非目标

本次不解决：

- 同一 Agent 私钥被多个独立 Harness 同时运行时的跨进程 leader election；
- 任意外部业务系统的 exactly-once 或分布式事务；
- 自动证明 Board 与物化结果语义一致；
- 自动从 Meeting 推断 Project Context Edge；
- Participant Intent、Grant 或 Speech 的既有 Floor lease 设计；
- 以 supervisor binding 重新约束 Meeting 权限；
- 恢复或篡改当前已 blocked 的历史验收 Meeting。

默认部署约束仍是：一个受管逻辑 Agent 在一个 Community 中由一个权威 Harness 实例运行；Harness
内部可有多个工作槽。若未来要支持同一 Agent key 的多 Harness active-active，应单独设计进程级 claim，
不能重新借用 ACP Session affinity 假装解决。

## 16. 实施记录

待实现后补充：

- 分阶段提交；
- 删除和保留的最终代码清单；
- Contract 切换结果；
- 自动化测试命令及结果；
- 真实 Provider Meeting、Action Run、Board、ACK 与 End 证据；
- 数据安全检查与构建缓存清理结果。
