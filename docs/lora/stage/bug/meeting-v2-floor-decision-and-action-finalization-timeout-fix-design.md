# Meeting V2 Floor Decision 空等与 Action Finalization 硬超时修复设计

> 状态：已实现；Floor Decision 现场验收通过；后续 Board→Action continuity
> 核心修复已落地，Action lease 与完整关闭路径待现场重验
>
> 修订日期：2026-08-06
>
> 范围：Meeting V2 moderated Board、Floor Decision、direct Action Finalization、
> `buzz-acp`、Relay、DB、SDK/CLI 与 Desktop 状态展示
>
> 关联设计：
> [主持人乐观决策设计](../meeting/v1/meeting-v1-moderator-optimistic-decision-design.md)、
> [主持人直接完成行动收口的后端修正方案](../meeting/fix/meeting-v2-direct-action-finalization-backend-plan.md)、
> [现场验收后续：Board→Action 连续性、Return-to-Board 投影与 Desktop 终态收敛修复设计](meeting-v2-board-action-continuity-return-to-board-and-directory-convergence-fix-design.md)

## 1. 结论

本次会议暴露了两个表现相似、根因不同的超时问题。

### 1.1 Floor Decision：模型已经决定，但输出契约不完整

主持 Agent 通常在 8～11 秒内已经给出决定。异常发生在模型同时输出：

```text
拒绝若干 pending Intent
+ finalize_actions
```

Prompt 同时展示 cleanup 数组和 `next_action`，却没有说明 terminal action 必须携带空 cleanup；
解析器又严格禁止该组合。解析错误随后被 `.ok()` 丢弃，并被错误记录为 `no_action`。Relay 保留
原 3 分钟 decision deadline，会议因此空等到 deadline 后才执行 fallback。

本 BUG 不需要新增横跨 Action Begin、Meeting End 和 Abort 的通用复合协议。现有
`FINALIZE_ACTIONS` 和 Close/Abort 事务已经会结束全部存活 Floor 对象。正确修复是：

1. 补齐 Prompt 的互斥约束；
2. 对 `cleanup + terminal` 做确定性规范化；
3. 复用现有 Action Begin / Meeting End；
4. 保留结构化解析错误；
5. 区分显式 `IDLE`、无效输出、provider failure 与 runtime loss；
6. 对无法修复的无效输出，在 Relay 事务内结束 Attempt 并执行明确恢复，不再遗留旧 deadline。

### 1.2 Action Finalization：业务仍在执行，却被固定硬上限终止

主持 Agent 在 Action Finalization 内持续调用工具，并成功写入 Work、responsibility 和 Role
Checkpoint；但 Meeting V2 Turn 仍共享 270 秒 `max_turn_duration`。ACP 在 Relay 的 300 秒
deadline 之前终止了仍有活动的进程，最后一次成功写入后不足 3 秒，Agent 尚未来得及返回
`COMPLETE`，Action run 就因 continuity 丢失而 blocked。

Action Finalization 时 Board 已冻结、Floor 只读，不存在发言权公平性竞争。该阶段应改为：

- Relay 保存可续约的当前 lease expiry；
- Agent host 的 ACP 或 Human host 的 Desktop 定期续约；
- ACP 的 provider/tool idle watchdog 独立判断真正失活；
- lease 过期、Human 取消或 operator circuit breaker 才停止执行；
- Progress/renewal 不授予业务权限，也不等于 `COMPLETE`。

### 1.3 本次兼容边界

当前没有仍在运行的 Meeting。因此本次不设计：

- active Meeting 的 `legacy_fixed` 双轨；
- fixed → renewable 的运行中迁移；
- 存量 Action run 自动 Retry；
- 同一 policy 下的渐进 timing mode 切换。

新建 Meeting 一次性切换到新的 renewable lease policy。已结束 Meeting 只需保持历史数据可读，
不重新执行、不续约、不回填运行状态。

## 2. 故障记录

### 2.1 事件范围

本次排查对应：

- Meeting：`420a4716-5018-4bc5-bb60-25d2c36ce800`；
- schema：`3`；
- policy：`moderated-board-actions-v2`；
- 主持 Agent：`test-1`，pubkey 后缀 `f06d...b204`；
- Action run：`2fb860fe-4e47-42dd-bfe2-8460603d0b02`。

### 2.2 Floor Decision 时间线

一次健康的直接传递：

```text
19:53:04.014  decision attempt 开始
19:53:11.649  模型返回 select_intent
19:53:12.347  offer 创建
```

从 attempt 到 offer 约 8.3 秒，证明直接传递路径不依赖 3 分钟到期。

随后三次异常 decision：

| Attempt 开始 | 模型完成 | 被记为 `no_action` | fallback | 无效等待 |
|---|---|---|---|---|
| 19:54:04.014 | 19:54:14.238 | 19:54:14.870 | 19:57:04.078 | 约 169 秒 |
| 19:59:02.558 | 19:59:13.438 | 19:59:13.912 | 20:02:02.622 | 约 169 秒 |
| 20:02:14.128 | 20:02:21.923 | 20:02:22.424 | 20:05:14.202 | 约 172 秒 |

三次输出均表达“清理若干候选后进入 `finalize_actions`”，并非没有决定。

当前代码链路为：

```text
Prompt 未声明跨字段互斥约束
  → parser 拒绝 cleanup + terminal
  → parse_control_output(...).ok() 丢弃错误
  → no_action
  → Attempt 被当作 Completed
  → 原 moderator deadline 保留
  → 到期后 deterministic fallback
```

### 2.3 与 Floor 同期出现的 continuity 症状

首个 Floor epoch 还出现：

```text
19:49:23.319  attempt 1 开始，随后 runtime_lost
19:49:24.721  attempt 2 开始，随后 runtime_lost
19:49:57.122  attempt 3 开始，随后 runtime_lost
attempt 4       因尝试次数上限被拒绝
19:52:23.347  deadline fallback
```

Board Turn 在 19:49:22.933 已成功写入 Board，ACP 随后收到 Cancel，对应 Codex turn 被标为
`turn_aborted`。现有 INFO 日志不足以证明 Cancel 的精确触发源。

本文仅把它记录为相关 investigation：

- 已确认症状：同一 Board → Floor 转换连续产生 `runtime_lost`；
- 待验证假设：canonical Board 回流、Pool Result、affinity 安装与 preemption 之间存在竞态；
- 本次只增加关联日志和真实 AgentPool 回归测试；
- 在确认触发源前，不把推测性 continuity 修改列为上述两个 BUG 的完成条件。

### 2.4 Action Finalization 时间线

```text
20:05:56.420  Action run 开始
20:05:57.114  ACP Action task 开始
20:09:27      Work update 成功，Project revision 93
20:09:40      Work responsibility 成功，Project revision 94
20:10:24      Role Checkpoint 成功，Project revision 95
20:10:27      ACP 触发 hard turn timeout 并替换 Agent 进程
```

ACP 日志记录：

```text
hard turn timeout exceeded (silence 2.759s)
```

这不是长时间无输出或 provider 挂死。最后一次 canonical 写入距离进程被终止仅约 2.86 秒。

该 turn 内可观察到：

- 34 次 tool call；
- 约 16 次 `buzz` CLI 调用；
- 约 16 次源码、help 或搜索调用；
- 一次网络 sandbox 拒绝后的重试；
- 多次 Project View v3 typed patch、Role Work 与 Checkpoint 探索。

Relay 数据库中的 Action deadline 并没有被缩短。是 ACP 从 Relay deadline 派生本地安全边界时减去
15 秒，再与固定 270 秒 `max_turn_duration` 取更小值；本次由 270 秒先触发。

进程替换后，continuity 模块正确检测到 affinity 丢失并把 Action blocked。错误点不是 continuity
fail-closed，而是上游使用了不适合长 Action Turn 的固定总时长。

### 2.5 用户可见后果

Desktop 显示：

```text
Action finalization
The Agent host's action-recording window is blocked and needs recovery.

Action output
Action recording is blocked
```

外部业务写入已经发生。Meeting 不能回滚它们，也不能假装它们没有发生。Retry 必须回读 canonical
业务状态，再决定是否需要补写。

## 3. 根因

### 3.1 Floor Prompt 缺少跨字段约束

Prompt 展示了：

- `rejections`；
- `handoff_dismissals`；
- `deferrals`；
- `next_action`。

但没有告诉模型：`close`、`finalize_actions` 和 `abort` 必须携带空 cleanup。解析器随后严格执行
了未在 Prompt 中表达的约束。

这不是模型拒绝遵守已知协议，而是模型可见协议描述不完整。

### 3.2 解析失败、provider failure 与正常 idle 被折叠

当前控制结果没有保留：

```text
valid_action
valid_idle
invalid_json
invalid_semantics
provider_failure
runtime_lost
```

解析失败和 `succeeded=false` 都落入同一个 `None` 分支，并被记录为 `no_action`。系统无法选择正确
恢复策略，也无法解释为什么模型输出没有被执行。

### 3.3 Attempt 结束与 deadline 没有闭环

`no_action` 被当作正常 Completed。Attempt 结束后，原 moderator deadline 仍然有效，deadline
sweeper 只能等待到期。

显式 `IDLE` 保留原 deadline 是既有产品语义；无效输出或 provider failure 不属于显式 `IDLE`。

### 3.4 客户端串行 cleanup 后 terminal 确实会产生 revision 冲突

Action Begin 要求 Attempt 冻结的 `intent_revision` 仍等于当前 revision。若 ACP 先逐条提交
rejection，再用原 Attempt 提交 Action Begin，cleanup 已改变 revision，Action Begin 会被拒绝。

但这不代表必须新增复合 terminal 协议。现有 terminal transition 会把剩余 Floor 对象标记为
`ended`，不会再执行 fallback。初版可以把 terminal 下的 cleanup 视为被终止转换 supersede。

只有产品未来明确要求“Meeting 结束前仍必须为每个候选保存 rejected/dismissed 理由”时，才需要扩展
现有 Action Begin / Meeting End，使 annotation 与 terminal 在同一事务提交。

### 3.5 Action 使用了不适合该阶段的固定总时长

发言 Grant 需要硬 deadline，以防参与者长期占用 Floor。Action Finalization 时 Board 已冻结，
不存在同样的公平性竞争。该阶段需要的是：

- 确认当前 host runtime 仍拥有精确 Action fence；
- provider/tool 长期失活时可停止；
- app/runtime 消失后 lease 自动到期；
- 防止旧 action window 完成新 window；
- 允许正确身份执行 Retry、ReturnToBoard 或 Abort。

### 3.6 ACP 和 Relay 都只有固定 deadline

当前：

- Relay action run 保存固定 `action_deadline_at`；
- due recovery 到期即写入 `action_deadline_exceeded`；
- ACP `PromptExecution` 只接受固定 absolute deadline；
- `AcpClient` 的 prompt API 只接受固定 Duration；
- 同一 action window 的 deadline reconciliation 只取更早值，主动禁止延长；
- provider 活动只能重置 idle timeout，不能延长 hard timeout。

所以仅新增 Relay 字段不足以修复；ACP prompt 执行层也必须支持 renewable deadline。

## 4. 修复边界与不变量

### 4.1 Terminal transition supersede cleanup

对本次初版：

```text
terminal action 合法
  → cleanup 不再作为逐对象管理命令执行
  → 全部 live Floor objects 由既有 terminal transaction 统一 ended
  → 不再进入 fallback
```

这会丢弃逐对象 rejection/dismissal 理由，但保留 terminal reason 和规范化诊断。它不会丢失会议
状态，也不会让被清理候选再次获得发言权。

`deferrals` 在 terminal transition 中没有稳定业务意义：对象刚 defer 就会 ended，因此不进入未来
terminal annotation 设计。

### 4.2 Explicit IDLE 与 invalid output 不同

- `IDLE`：主持人有意保留当前候选并等待，Attempt 正常 Completed，保留原 deadline；
- `invalid_json/invalid_semantics/provider_failure`：Attempt Discarded，进入有界修复或原子恢复；
- `runtime_lost`：Attempt Abandoned，按 continuity replacement 规则恢复；
- 合法 action：立即提交，不等待 deadline。

### 4.3 Action renewal 不授予业务权限

Lease renewal 只延长当前 Meeting Action window。它不得：

- 授予 Community、Project View、Role 或 Document 权限；
- 替代目标系统自己的授权；
- 修改 Board；
- 让旧 action window 复活；
- 替代最终 `COMPLETE/BLOCK/RETURN_TO_BOARD/ABORT`。

### 4.4 Agent 签名是 cooperative liveness，不是 harness 密码学证明

当前 Agent 私钥会进入 Agent 可调用的 MCP/tool 环境。Relay 看到 Agent 签名时，不能从密码学上
区分事件来自 ACP coordinator 还是模型/tool 子进程。

因此初版边界为：

- renewal builder 只在 ACP/Desktop 内部实现，不增加普通 `buzz` CLI subcommand；
- Relay 把签名解释为当前 moderator 身份的 cooperative liveness；
- ACP 本地保证同槽、同 ACP Session、同 turn 和同 fence；
- Relay 只能权威保证 actor、run/window/Board fence、sequence、expiry 和 operator cap；
- 默认启用较高的 operator circuit breaker，限制失控续约造成的资源占用；
- 本次不重新引入 Project Runtime supervisor binding 作为 Meeting 权限条件。

如果未来需要 Relay 可验证的“只由 harness 续约”，应单独设计 action-scoped 临时 attester；不能假装
同一个 Agent key 已经提供这种证明。

### 4.5 External effects 是 at-least-once + reconciliation

Meeting 不记录外部业务步骤，也无法为任意系统提供 exactly-once。Retry 只能保证：

- 首先回读 canonical 状态；
- 对支持稳定 ID、CAS 或 idempotency key 的目标使用原生机制；
- 避免已知重复操作；
- 承认在途写入可能已经成功。

不得承诺任意外部系统绝不会重复创建对象。

### 4.6 Host 权限边界

- Agent moderator：ACP 执行和续约；Desktop 只读，不允许 Human 代签 Retry/Return/Abort；
- Human moderator：Desktop/Tauri 使用当前 Human 身份续约，并展示 Human 可签名恢复操作；
- Community admin/security abort 若存在，继续走其独立管理路径，不能伪装成 moderator action。

## 5. Floor Decision 修复方案

### 5.1 补齐 Prompt 输出契约

在 Floor Prompt 中明确：

- `select_intent`、`select_handoff` 等非终止动作可携带协议允许的 cleanup；
- `close`、`finalize_actions`、`abort` 必须携带空 `rejections`、`handoff_dismissals` 和
  `deferrals`；
- terminal transition 会结束所有仍存活的 Floor 对象；
- `IDLE` 仅表示有意等待；
- 模型不得用 cleanup 表达 terminal action 的必要前置步骤。

### 5.2 对 cleanup + terminal 做确定性规范化

为兼容模型偶发输出，解析顺序改为：

1. 严格解析 JSON、字段集合、总大小和 cleanup 数量上限；
2. 识别 `next_action`；
3. 若是 terminal action，只校验 terminal 自身前置条件；
4. 不解释、不发布附带的逐对象 cleanup；
5. 将 cleanup 清空并提交现有 Action Begin 或 Meeting End；
6. 记录：

```text
result=normalized
reason=terminal_cleanup_superseded
cleanup_counts
decision_attempt_id
output_hash
```

规范化不得把原始模型全文写入普通日志。

本次不新增 `ModeratorTerminalDisposition`，也不改变既有 Action Begin/End 的签名审计语义。

### 5.3 使用 typed parse outcome

删除 `parse_control_output(...).ok()`，引入稳定结果：

```text
Valid(ControlOutput)
ExplicitIdle(ControlOutput)
InvalidJson(code)
InvalidSemantics(code)
ProviderFailure(code)
```

稳定错误码至少包括：

```text
invalid_json
unknown_action
missing_required_field
invalid_candidate_reference
invalid_terminal_precondition
provider_failure
```

Attempt receipt、日志和 telemetry 保存错误码、Attempt/frozen revision、输出 hash、是否规范化或
format repair，以及最终恢复方式。

### 5.4 Format repair 不刷新 deadline

对无法规范化的格式问题，允许至多一次结构修复，但必须：

- 继续使用原 Attempt 的剩余 deadline；
- 不刷新 canonical moderator deadline；
- 保持同一逻辑 Agent 和 ACP Session；
- 按既有 replacement/provider attempt 规则计入尝试上限；
- 只把精确 validation error 和原始输出的受限上下文交给 repair turn；
- repair 完成后重新执行全部 semantic guard。

若剩余时间不足，不再调用模型，直接进入 invalid-output recovery。

### 5.5 原子 invalid-output recovery

扩展现有 `DecisionAttemptFinish` 语义，而不是增加第二个松散 fallback 命令：

```text
outcome=discarded
reason_code=invalid_output | provider_failure
recovery=deterministic
```

Relay 在同一 Session transaction 中：

1. 锁定并确认相同 active Attempt、control/decision epoch、speech revision 和 deadline；
2. 重新检查 Human priority；
3. 将 Attempt 标为 Discarded；
4. 若 Human Request 已到达，让 Human priority 胜出；
5. 否则调用现有 deterministic fallback policy；
6. 明确处理 handoff-only cohort，不留下悬空 handoff；
7. 清除或替换旧 moderator deadline；
8. 发布一个 canonical transition。

不得“先 Finish，再异步 fallback”，否则两步之间会与 Human Request、新 Intent 或控制状态变化竞态。

显式 `IDLE` 不走该路径，继续保留既有等待语义。

### 5.6 Continuity investigation

增加关联日志：

- Board command prepared/submitted/accepted；
- canonical Board event 回流；
- Pool Result 到达；
- slot/session affinity 安装与释放；
- Cancel/preemption 的来源和目标 turn；
- Floor attempt dispatch 与实际领取槽。

增加贯穿真实 AgentPool 主循环的测试，覆盖：

```text
Board Result
  → binding install
  → Board command accepted
  → canonical State 回流
  → preemption 计算
  → Floor claim
```

测试既要证明自产生 Board event 不会错误取消连续 Turn，也要保留合法外部 Board preemption。
只有日志确认触发源后，才实施对应 continuity 代码修复。

## 6. Action Finalization renewable lease 方案

### 6.1 单一新协议代际

新建 action-capable Meeting 统一使用：

```text
policy:     moderated-board-actions-v3
capability: meeting-v2-action-finalization-v3
```

新 capability 代表 Agent 能够：

- 解析 renewable Action State；
- 处理 Action lease renewal；
- 保持 Board/Floor/Action continuity；
- 在 lease 到期、Retry 或 terminal 状态时停止旧 turn。

由于全部 Agent participant 都会解析 Meeting State，Create gate 继续要求完整 frozen Agent roster
声明 v3 capability；Human participant 不需要 Agent runtime capability。

Relay 只在 DB migration、due recovery、command handler、State projection 和 create gate 全部就绪后
广告对应 NIP-11 runtime/create capability。旧 v2 policy 不再用于创建新 Meeting。

### 6.2 复用 `action_deadline_at` 作为当前 lease expiry

当前没有 active Meeting，不需要同时维护 fixed 和 renewable 两套 live shape。保留现有列名以减少
迁移面，但在 v3 中将其语义定义为：

```text
action_deadline_at = 当前 action window 的 canonical lease expiry
```

新增：

```text
progress_seq             BIGINT NOT NULL DEFAULT 0
last_progress_stage      nullable text
last_progress_at         nullable timestamptz
operator_hard_deadline   nullable timestamptz
```

另建 renewal audit：

```text
meeting_v2_action_lease_renewals
  community_id
  session_id
  action_run_id
  action_window_epoch
  progress_seq
  renewal_event_id
  stage
  accepted_at
  lease_expires_at
```

唯一性至少覆盖：

```text
(community_id, session_id, action_run_id, action_window_epoch, progress_seq)
(community_id, renewal_event_id)
```

Retry 保持同一 `action_run_id`、递增 `action_window_epoch`，并将新 window 的 `progress_seq` 重置为
0。已结束历史行不参与运行语义。

### 6.3 ActionLeaseRenew command

新增 action command：

```text
ActionLeaseRenew
  meeting_id
  action_run_id
  action_window_epoch
  board_event_id
  progress_seq
  stage: reasoning | tool_call | tool_result | finalizing | waiting_human
  last_activity_seq
```

Agent moderator 由 ACP 构造并签名；Human moderator 由 Desktop/Tauri 后台构造并使用当前 Human
身份签名。模型输出本身不构造该命令。

Relay 事务顺序必须固定：

1. 按既有锁顺序锁定 Meeting Session 和 action run；
2. 使用数据库时间执行 lazy due recovery；
3. 若 `clock_timestamp() >= action_deadline_at`，expiry 获胜，run 进入 blocked，拒绝 renewal；
4. 校验 actor 是 frozen moderator；
5. 校验 Meeting 仍处于 `finalizing_actions` 且 run runnable；
6. 精确校验 run/window/Board fence；
7. 要求 `progress_seq == current_progress_seq + 1`；
8. 校验 operator hard deadline；
9. 以 DB now 计算新的 lease expiry；
10. 更新 run head、写 audit/receipt，并发布 canonical State。

不得让 sweeper 尚未来得及执行成为迟到 renewal 复活已过期 run 的窗口。

幂等语义：

- 同一 signed event ID 重放：返回原 canonical mutation receipt；
- 不同 event 复用已消费 sequence：`progress_sequence_conflict`；
- sequence 跳号或回退：拒绝；
- 新 action window 从 sequence 1 开始；
- 旧 window 的 Renewal/Complete 一律拒绝。

Relay response envelope 在首次接受和幂等重放时都使用当前 DB time 重新计算 remaining duration，并返回：

```text
server_now_ms
lease_expires_at_ms
lease_ttl_ms
operator_hard_remaining_ms  // nullable
accepted_progress_seq
```

这里的 `lease_ttl_ms` 是生成本次 response 时仍剩余的 lease 时长，不是 immutable receipt 中缓存的
旧 TTL。Action Begin 的初始 response 也必须提供 lease TTL 和 operator hard remaining，不能只给绝对
墙钟时间。

ACP/Desktop 在发送 Begin/Renewal 前记录 `request_started_at: Instant`；accepted response 的本地安全
边界按以下方式保守换算：

```text
local_lease_deadline = request_started_at + lease_ttl_ms - safety_margin
local_operator_deadline = request_started_at + operator_hard_remaining_ms - safety_margin
```

减法必须使用 checked/saturating 语义；remaining duration 小于等于 safety margin 时直接视为本地到期。
`operator_hard_remaining_ms = null` 时不创建本地 operator deadline。

不得使用“收到 response 时的 `Instant::now() + lease_ttl_ms`”，否则会把返程延迟多算进 lease，导致
Relay 已 blocked 后本地仍启动 tool。若 response 到达时本地安全边界已经过去，客户端立即停止新
tool、执行 Full Sync，不得尝试用迟到 renewal 复活 window。`server_now_ms` 和绝对 expiry 仅用于
诊断/展示，不能假设本机墙钟与数据库时钟一致。

### 6.4 Renewal 与 activity 分离

Lease renewal 表示“精确 Action Turn 仍由当前 runtime 持有”，不表示业务已经取得进展。

Agent host 的 ACP 在以下条件同时成立时周期续约：

- 同一 agent slot；
- 同一 ACP Session；
- 同一 Action turn 仍在 in-flight；
- run/window/Board fence 未变化；
- 没有收到 cancel、abort、return-to-board、blocked 或 shutdown；
- 本地 provider/tool idle watchdog 尚未触发。

`last_activity_seq` 和 `stage` 用于诊断。provider 暂时没有新 frame 时，它们可以保持不变，不能要求
每次 renewal 都必须消费新的业务活动。否则 60～90 秒 lease 会变成新的静默硬超时。

独立 idle watchdog 负责真正失活：

- provider frame、tool start、tool result 更新 activity；
- 长时间无输出的 in-flight tool 服从 tool timeout；
- idle timeout 到达后停止 renewal，并受控取消 prompt；
- renewal tick 自身不更新 activity；
- PID 存在不能绕过 idle timeout。

初始参数建议作为配置并经真实 adapter 测试确定，例如：

```text
renew cadence          20～30 秒
soft lease             90 秒
action idle timeout    独立配置
operator hard cap      生产默认 30～60 分钟，本地开发可显式关闭
```

operator hard cap 是资源泄漏与弱信任边界下的 circuit breaker，不是正常行动预算。

### 6.5 ACP 动态 DeadlinePolicy

保持 participant、moderator、action 使用同一 pool slot、ACP Session 和稳定系统合同。不得通过新
ACP Session 或新进程绕过 270 秒。

为单个 PromptExecution 增加：

```text
PromptDeadlinePolicy
  Fixed(Instant)
  Renewable {
    lease_updates: watch::Receiver<LeaseDeadline>
    operator_hard_deadline: Option<Instant>
  }
```

`V2ActionFinalization` 使用 Renewable，其他 Meeting Turn 保持 Fixed。AcpClient reader/select loop
根据 watch 更新当前 hard boundary。

同时必须修改：

- 同一 action window 的 deadline reconciliation：允许权威 lease 向后移动；
- hard timeout outcome：lease expiry 使用专用 outcome，不伪装成普通 `HardTimeout`；
- 取消路径：先发受控 cancel 并 drain，只有失败时才替换进程；
- 本地 lease 到期：立即禁止启动新 tool，不等待 canonical State 回流；
- Retry：新 Action Turn 必须等待旧 prompt cancel/drain 完成，禁止两个 window 并行执行。

### 6.6 Human moderator renewal

Human 进入 Action Finalization 后，由 Desktop/Tauri 的 Community-scoped 后台 runtime 持有本地
renewal claim。该 claim：

- 绑定当前 Human identity、Community、Meeting、run/window/Board；
- 在 Human 导航到 Project View 后仍可继续；
- identity/Community 切换、App 退出、terminal 或明确停止时释放；
- 只能由 frozen Human moderator 签名；
- 仍受 operator hard cap；
- lease 到期后由 Human 明确 Retry，不能静默复活旧 window。

Agent-host Meeting 的 Desktop 只显示进度和 blocked 原因，不创建 renewal，也不显示 Human 代签
Retry/Return/Abort 按钮。

### 6.7 Canonical progress projection

每次 accepted renewal：

- 保存 signed command、private receipt 和 renewal audit；
- 更新 action run head；
- 发布 Relay-authored Meeting State；
- 只增加 `state_revision`，不增加 Board、intent 或 speech revision；
- State 暴露 lease expiry、progress sequence、stage 和 last progress time；
- Desktop 和 ACP 都以 canonical State/receipt 为准，不以本地 heartbeat 推测 Relay 已续约。

同一 `action_run_id + action_window_epoch + board_event_id` 下，仅 lease/progress head 前移的 State
属于 in-place liveness update。ACP 收到它时只能更新当前 PromptExecution 的 deadline、stage 和
progress head，绝不能 cancel 当前 turn、释放 affinity、重排 slot、创建新 turn 或重建 ACP Session。
只有 blocked/terminal、Retry 产生新 window，或 run/Board fence 改变时才触发取消与重新调度。

同一 renewal 尚在提交或等待 State 时不得并发生成下一 sequence。response 丢失时重放同一个已签名
event，再 Full Sync；不得生成相同 sequence 的新 event。

### 6.8 到期、完成与恢复

Lease 到期：

1. Relay 原子把 run 标为 `blocked(action_lease_expired)`；
2. 本地 watchdog 停止新 tool，并取消旧 prompt；
3. 已在途或已接受的外部写入不回滚；
4. Agent-host Desktop 只读展示；Human moderator 可选择 Retry/Return/Abort；
5. Retry 递增 `action_window_epoch`；
6. 新 turn 首先回读 canonical Board 和目标系统状态；
7. 旧 window 的延迟 tool result、Renewal 或 Complete 不能操作当前 window。

正常完成仍使用现有原子 `actions-recorded + End(outcome=closed)`。Close gate 必须同时校验：

- 当前 run/window/Board fence；
- runnable；
- DB now 早于当前 lease expiry；
- DB now 早于可选 operator hard deadline。

## 7. 数据库与协议迁移

### 7.1 一次性 v3 切换

新增迁移，例如：

```text
migrations/0047_meeting_v2_action_renewable_lease.sql
```

迁移内容：

- 为 action run 增加 progress/operator 字段；
- 新增 renewal audit 表和唯一约束；
- 更新 runnable/blocked/terminal CHECK；
- 更新 deadline partial index；
- 更新 `meeting_sessions` 的 policy/协议 shape CHECK，使其识别
  `moderated-board-actions-v3`，并停止接受新的 v2 action-capable Meeting；
- 更新 fresh-install desired schema 和 migration 静态门禁；
- 更新 `MeetingProtocol::from_persisted`、Relay command dispatch 与 due scheduler：v3 进入新状态机，
  ended v2 仅进入严格只读 projection；
- 更新 SDK/CLI/Desktop 的默认 Create policy，并将 NIP-11 v2 create capability 下线、v3 runtime/create
  capability 同步启用；
- 将 managed Agent capability reconciliation 从 v2 收敛到
  `meeting-v2-action-finalization-v3`；
- 保留已结束 v2 Meeting、Action run、State/Board/End event、receipt、消息和业务对象数据。

当前没有 active Meeting，因此 migration/启动前门禁应断言不存在任何 active action-capable Meeting
或 live Action run；若断言失败则停止切换，不能猜测如何转换。

### 7.2 删除双轨设计

本次不增加：

```text
timing_mode = legacy_fixed | renewable_lease_v1
action_lease_expires_at 旁路列
legacy Action UI
legacy active-run backfill
```

新 v3 统一把 `action_deadline_at` 解释为滚动 lease expiry。已结束历史只读，不参与新状态机。

“不考虑已结束 Meeting”指不为它们运行 mutation、sweeper、Retry、backfill 或协议恢复，不代表删除
历史读路径。实现必须保留 v2 persisted protocol 的 query/projection 和 Desktop strict read parser，
使 Board、Action、End 与消息历史继续可见；所有 v2 Create 和运行态 mutation/dispatch 则关闭。

### 7.3 Capability 仍是新建门禁，不是渐进双轨

虽然不做渐进发布，capability gate 仍必须存在，用于防止后续旧 ACP/adapter 被加入新 Meeting。

Create 必须同时满足：

- Relay 广告 v3 runtime/create capability；
- 全部 Agent roster 声明 `meeting-v2-action-finalization-v3`；
- Human roster 不要求 Agent capability；
- Desktop 提交前预检，Relay transaction 最终 fail closed。

## 8. 分层修改范围

### 8.1 `buzz-core` / `buzz-sdk`

- 定义 v3 policy/capability；
- 保留 ended v2 的 persisted protocol 只读解码；
- 增加 ActionLeaseRenew action variant、tags 和 builder；
- 扩展 DecisionAttemptFinish 的 invalid recovery reason；
- 定义 exact event replay 与 sequence conflict；
- 增加 payload round-trip、边界和 malformed input 测试。

### 8.2 `buzz-db`

- 实现 DecisionAttempt invalid-output 原子恢复；
- 保持 terminal transition 统一结束 live Floor objects；
- 增加 renewable lease schema、audit 与 run head；
- renewal 前执行 lazy due recovery；
- Progress/Complete/Retry/expiry 使用统一锁顺序；
- State renewal 只推进 state revision；
- 更新 fresh-install schema、constraints 和 indexes。

### 8.3 `buzz-relay`

- 接收 ActionLeaseRenew；
- 使用 DB time 计算 expiry；
- 对过期、stale window、错误 actor、sequence conflict fail closed；
- 返回 server time、TTL、expiry 和 sequence receipt；
- 广告 v3 NIP-11 runtime/create capability；
- 下线 v2 create capability，并将 protocol dispatch 一次性切到 v3；
- v3 Create gate 检查完整 Agent roster。

### 8.4 `buzz-acp`

- 补齐 Floor prompt；
- terminal cleanup 规范化；
- typed parse/provider outcome，删除 `.ok()`；
- 有界 format repair 和 invalid recovery；
- ActionLeaseRenew publisher；
- activity/idle 与 renewal 分离；
- Renewable DeadlinePolicy；
- 同 window deadline reconciliation 允许权威延长；
- lease expiry 受控取消和旧/new window 排他；
- 增加 Board → Floor correlation logging。

CLI guide 可补充 Project View v3、Role Work 和 Checkpoint 示例，以减少 Action Turn 的探索成本；
该优化有价值，但不是 lease 正确性的前置条件。

### 8.5 Desktop/Tauri

- Human moderator 的 Community-scoped renewal claim；
- Agent-host Action 全程只读；
- 展示 stage、last progress、lease 状态与累计耗时；
- 对 invalid output、provider failure、runtime lost、lease expiry 使用不同文案；
- identity/Community/App 生命周期正确释放 Human renewal；
- 不把普通 Community admin 操作伪装成 moderator action。

## 9. 可观测性

低基数指标：

```text
meeting_floor_decision_latency_seconds
meeting_floor_result_total{result=valid|idle|invalid|provider_failure|runtime_lost}
meeting_floor_terminal_normalized_total{action}
meeting_floor_recovery_total{reason,outcome}
meeting_action_renewal_total{stage,outcome}
meeting_action_lease_remaining_seconds
meeting_action_runtime_seconds
meeting_action_terminal_total{outcome}
meeting_action_continuity_loss_total{reason}
```

Meeting ID、event ID、pubkey、slot、ACP Session 和 sequence 只进入结构化日志/trace，不进入指标
label。

日志应能回答：

- 模型是否已经输出决定；
- terminal cleanup 是否被规范化；
- invalid output 如何恢复；
- 系统是否在等待显式 IDLE；
- 谁续约、续约到何时、为何停止；
- 最后一次 provider/tool activity 是什么；
- 哪个状态变化导致 Cancel 或 continuity loss。

## 10. 测试与验收

### 10.1 Floor

- Prompt 明确 terminal cleanup 为空；
- `cleanup + finalize_actions` 被规范化，立即复用 Action Begin；
- `cleanup + close/abort` 被规范化，立即复用 Meeting End；
- terminal 成功后全部 live Floor objects ended，永不 fallback；
- 不持久化被 supersede 的逐对象 rejection/dismissal；
- invalid JSON、invalid semantics、provider failure 分类不同；
- 显式 `IDLE` 保留 deadline；
- format repair 不刷新 deadline，且最多一次；
- invalid recovery 原子清除/替换旧 deadline；
- Human Request 与 invalid recovery 并发时 Human priority 胜出；
- handoff-only cohort 有确定恢复结果；
- 无效输出不会再空等约 170 秒。

### 10.2 Floor continuity investigation

- 真实 AgentPool Board Result → affinity → State → Floor claim；
- 自产 Board event 不错误取消同一连续 Turn；
- 外部权威 Board 变化仍可合法 preempt；
- correlation log 能定位每个 Cancel 来源。

该组测试验证 investigation，不把未证实代码修改设为本次 Floor parser BUG 的完成条件。

### 10.3 Action lease DB/Relay

- `seq == current + 1` 才可续约；
- 同 event ID 重放幂等；
- 首次接受与重放 response 都按当前 DB time 返回 remaining duration；
- same-seq different-event 冲突；
- 过期但 sweeper 未运行时，late renewal 不能复活 run；
- Renewal/Complete/expiry 三方并发只有一个合法结果；
- Retry 后旧 window 的 Renewal/Complete 失败；
- operator cap 生效；
- State renewal 不改变 Board/intent/speech revision；
- response 丢失重放相同 prepared event；
- 高 RTT、response 延迟和本机/DB 墙钟偏差下，本地安全 deadline 不晚于 Relay expiry；
- Action Begin 初始 response 可初始化 lease 与 operator hard monotonic deadline；
- fresh install schema 与 migration upgrade 均通过。

### 10.4 ACP

- Action Turn 超过 270 秒但持续续约时可以完成；
- provider 暂时无新 frame 时仍可续约，直到独立 idle timeout；
- renewal tick 不更新 activity；
- silent/hung provider 在 idle timeout 后停止续约；
- 长时间 tool obeys tool timeout；
- local lease expiry 立即停止新 tool；
- controlled cancel 成功时不替换 Agent 进程；
- old prompt drain 前不启动新 window；
- 同一 slot、ACP Session 和系统合同保持不变；
- authoritative renewal 可向后更新同 window deadline；
- 连续多次同 window Renewal State 回流只更新 deadline/stage，始终只有一个相同 turn；

### 10.5 Desktop/Human

- Human moderator 导航到 Project View 后后台续约继续；
- identity/Community/App 生命周期释放 renewal；
- Human lease 到期后必须 Retry，不能复活旧 window；
- Agent-host Desktop 只读且无接管按钮；
- Desktop 实时展示 canonical stage/last progress/blocked reason。

### 10.6 端到端

1. 一个持续 8～10 分钟的 Agent Action Turn 成功完成；
2. Work、responsibility、Checkpoint 写入成功后返回 COMPLETE 并原子关闭；
3. provider/runtime 被杀后 lease 到期并 blocked；
4. Retry 回读 canonical 状态，对 Buzz 自身带稳定 ID/CAS 的命令避免重复；
5. 在途外部写入的 at-least-once 语义被正确提示；
6. Human host 完成一次跨 Project View 的长 Action Finalization；
7. 当前无 active Meeting 的迁移不会删除已结束 Meeting、消息、Project View 或 Document 数据；
8. 迁移后可打开一个既有 ended v2 Meeting，完整看到 Board、Action、End 与消息历史；
9. 新 v3 mixed-roster capability gate fail closed。

## 11. 交付顺序

1. Floor Prompt、terminal normalization、typed error 和单元测试；
2. DecisionAttempt invalid-output 原子恢复；
3. Continuity correlation logging 与真实 AgentPool investigation test；
4. v3 policy/capability、数据库 migration、renewal transaction 和 State projection；
5. ACP Renewable DeadlinePolicy、renewal publisher、idle/cancel 排他；
6. Human Desktop renewal 与 Agent-host read-only 展示；
7. Relay/DB/ACP/Desktop 集成测试；
8. 在确认没有 active Meeting 后一次性构建并启用 v3；
9. 创建新 Meeting 做长时 Agent/Human 验收。

Floor parser 修复可以先独立交付。Action v3 必须在 DB、Relay、ACP、capability 和 Desktop/Human
路径全部就绪后统一启用。

## 12. 完成定义

### Floor BUG

- 合法 terminal decision 不等待原 deadline；
- terminal cleanup 被确定性规范化并复用现有 Begin/End；
- 不新增通用 `ModeratorTerminalDisposition`；
- 解析和 provider failure 不再被记录为正常 IDLE；
- invalid recovery 不遗留旧 deadline；
- 显式 IDLE 保持既有语义；
- continuity investigation 有充分日志，但未证实修复不阻塞本 BUG 关闭。

### Action BUG

- Action Turn 不再受 270 秒普通 Turn 固定上限约束；
- renewal 只能作用于未过期的精确 run/window/Board；
- late renewal 不能复活 expired run；
- Agent renewal、Human renewal 和 Agent-host Desktop 权限边界正确；
- ACP 同槽、同 Session、旧/new window 排他；
- lease renewal 与 activity/idle 判断分离；
- Progress 不产生业务权限或 Board 变化；
- Retry 遵循 at-least-once + canonical reconciliation；
- 新 v3 capability 阻止旧 Agent 加入新 Meeting；
- 无 active Meeting 的一次性迁移保留已结束历史数据。

## 13. 非目标

本次不做：

- 为 terminal cleanup 新建跨 Action Begin/Meeting End 的通用复合协议；
- 保证 terminal 时逐条 rejection/dismissal 理由全部持久化；
- 通过 renewal 授予任何业务权限；
- 声称 Agent key 能密码学证明事件只来自 harness；
- 重新把 Project Runtime supervisor binding 绑定到 Meeting 操作权限；
- 为任意外部系统提供 exactly-once；
- 放宽 action fence、slot 或 ACP Session continuity；
- 取消 Speech Grant 的公平性 hard deadline；
- 设计 active Meeting 的 legacy/renewable 双轨；
- 仅调大 3 分钟、270 秒或 300 秒常量。

## 14. 实施记录

实现于 2026-08-06 完成，交付范围包括：

- Floor Prompt 的 terminal/cleanup 互斥说明，以及 `cleanup + terminal` 的确定性规范化；
- Floor 输出的 typed parse/provider outcome、一次有界 format repair、Discarded Attempt 与 Relay/DB
  原子 deterministic recovery；
- `moderated-board-actions-v3` 与 `meeting-v2-action-finalization-v3` 的一次性启用，ended v2
  保持严格只读历史；
- Action Lease Renew SDK、Relay 校验、DB lazy-expiry/sequence/fence/operator-cap 事务、renewal audit
  与 canonical State 投影；
- ACP 动态 deadline、独立 idle watchdog、精确 signed-event replay、lease 到期受控 cancel，以及旧/新
  action window 排他；
- Human moderator 的 Tauri 后台 renewal claim、Community/identity 生命周期释放和 Agent-host 只读边界；
- Desktop 的 canonical progress、lease、blocked reason 展示；
- fresh install、v46→v47 upgrade、DB action transaction、SDK/Relay/ACP、Desktop/Tauri 与前端模型的
  自动化回归。

最后的时序审计额外确认：

- Begin/Renewal 的本地安全 deadline 只从已校验 receipt 的
  `request_started_at + lease_ttl_ms - safety_margin` 派生；
- canonical State 中的绝对墙钟时间只更新诊断状态，不直接延长正在运行的 monotonic deadline；
- State 先于 HTTP receipt 到达时，仍保留原 signed renewal event，随后通过相同 event replay 取得新的
  remaining-duration receipt，不生成冲突 sequence。

尚未由自动化测试替代的最后验收，是创建一场新的 v3 Meeting，分别执行一次 8～10 分钟 Agent Action
和一次跨页面导航的 Human Action。当前没有 active Meeting，因此不执行任何存量运行态迁移或恢复。
