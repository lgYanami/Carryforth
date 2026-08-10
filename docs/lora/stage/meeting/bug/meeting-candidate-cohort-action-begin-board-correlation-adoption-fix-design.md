# Meeting Candidate-Cohort Action Begin Board 关联与首 Epoch 接管修复设计

> 状态：代码实现与 ACP 自动化完成；Relay-handler scratch 集成和真实 Provider 3/3 验收待完成
>
> 日期：2026-08-10
>
> 范围：Agent-hosted Meeting、Candidate-Cohort Floor Decision、Action Begin 构造与回执关联、
> `ActionBeginAdoption`、首 epoch 调度与续租
>
> 关联设计：
> [Meeting Action 首 Epoch 接管与首次 Progress 回归修复设计](meeting-action-initial-epoch-adoption-regression-fix-design.md)、
> [Action Context Attach 与首次调度 Permit 修复设计](meeting-action-context-attach-and-initial-dispatch-permit-fix-design.md)、
> [逻辑主持人 ACK 与同步简化实现设计](../fix/meeting-action-finalization-logical-host-ack-simplification-implementation-design.md)

## 1. 结论

本次 Summary 验收没有进入 Summary、Project View、Document 或 Project Context 业务逻辑。真正的阻塞发生在
Action Finalization 首 epoch 的本地接管阶段：Relay 已正确接受 Action Begin，但 ACP 在
Candidate-Cohort 路径中混用了两个不同坐标：

```text
board_event_id       = 冻结 Final Board 的坐标
decision_attempt_id  = 本次主持决策尝试的坐标
```

签名 Action Begin Event 中的 `board` tag 是正确的 Board ID；但是 ACP 同时把
`PreparedModeratorAction.object_id` 写成 Decision Attempt ID。HTTP receipt handler 随后又把这个
`object_id` 当作预期 Board ID，于是稳定产生 `board_event_id_mismatch`。

Candidate-Cohort 路径还有第二个必须同时修复的缺口：它通过通用 moderator event 入口提交 Begin，没有调用
统一的 `prepare_v2_action_begin()`，因此没有创建 process-local `ActionBeginAdoption`。即使只把
`object_id` 改成 Board ID，HTTP receipt 与 canonical State 仍没有 adoption record 可以汇合，不能生成
唯一 Action dispatch permit。

所以本次是 **ACP 内部命令元数据与接管入口的契约漂移**，不是：

- Relay lease 或 renew 算法失效；
- Provider 执行超过 90 秒；
- 工作槽或 ACP Session affinity 问题；
- Project View Summary 改动导致业务物化失败；
- Desktop 本地化引起 Relay 能力回退。

修复不新增协议状态，也不恢复原槽/原 ACP Session 约束。两种 Floor 路径必须统一经过同一个 typed Action
Begin 入口，始终分别保存 Board ID 与可选 Decision Attempt ID，并建立同样的 adoption 生命周期。

## 2. 事故记录

### 2.1 失败 Meeting

- Meeting：`12c6655f-ebd1-4a67-a610-0fe8c6e2991d`；
- Final Board Event：`3d65bc61f54934035ebaf633e669484733e9f04c001d403c08904fcdb2e38809`；
- 最终 Decision Attempt：`5a2eed9261f66a5a00149923a5fb5823563b4a02019c196f9f5ee9e1e279727d`；
- Action Begin Event：`133146290f8392f838e6fb359500376c02e0a524e8fd25bfe0f7d707bddcf75a`；
- Action Run：`00c1cbf8-89e0-4ba4-beab-7c395c69b920`；
- action window / epoch：`1`；
- 最终状态：`blocked / action_lease_expired`；
- `progress_seq=0`、`last_progress_at=null`、renewal 数量为 0。

DB 与 ACP 时间线如下，时间为 UTC：

```text
15:16:14.567  Candidate-Cohort Decision Attempt 开始
15:16:20.403  Relay 接受 Action Begin，创建 Action Run epoch 1
15:16:20.447  私有 Begin receipt 持久化，run/window/Board/timing 完整
15:16:20.814  ACP 拒绝 receipt：board_event_id_mismatch
15:17:50.488  90.085 秒后 Relay 将 run 标记为 action_lease_expired
```

Begin receipt 中的 Board ID 与签名 Begin Event 的 `board` tag 都是：

```text
3d65bc61f54934035ebaf633e669484733e9f04c001d403c08904fcdb2e38809
```

但本地 ledger 同一 prepared action 为：

```text
prepared.action_kind = action_begin
prepared.object_id   = 5a2eed...9727d   # Decision Attempt ID
prepared.attempt_id  = 5a2eed...9727d
event.board tag      = 3d65bc...38809   # 正确 Board ID
canonical Board      = 3d65bc...38809
```

因此不是 Relay 返回了错误 Board，而是 ACP 用错误的本地字段校验了正确回执。

### 2.2 Action Turn 实际没有开始

主持 ACP 在 `15:16:15` 派发的是产生 `FINALIZE_ACTIONS` 决策的 Floor/Moderator Turn。Begin 被接受后，日志中
没有新的 Action Finalization `meeting_turn_dispatched`，也没有任何 renewal 或 provider progress。

这说明 90 秒不是模型执行时间，而是一个无人接管的 runnable Action Run 等待 lease 到期的时间。

Meeting 保持 `active / finalizing_actions` 是正确的 fail-closed 结果：没有 `COMPLETE`，Harness 就不能生成
`End(attestation=actions-recorded)`，也不能伪造 Summary 或三域物化已经完成。

### 2.3 对照成功样本

成功 Meeting `ef3f5528-09d7-4c41-b430-61cadf31686e` 的 Action Run
`8d326e36-36f6-4a1a-adc6-0b9dae4ee3bd` 在 epoch 1 正常派发、续租、物化并关闭。

该 Meeting 结束前没有一个 `terminal_reason=action_finalization` 的 Candidate-Cohort Decision Attempt；它走的
是无候选人的简单 Floor 路径。简单路径调用 `prepare_v2_action_begin()`，其 `object_id` 本来就是 Board ID，
并会预先建立 `ActionBeginAdoption`。

这解释了为什么此前干净样本可以成功，而本次零干预样本仍然失败：两场会议命中了不同的 Begin 构造路径，
并非同一代码路径随机失效。

## 3. 根因

### 3.1 通用 `object_id` 承担了不相容的语义

Candidate-Cohort 的 `ModeratorActionSpec::FinalizeActions` 当前同时生成：

```text
action_kind = action_begin
object_id   = decision.attempt.attempt_id
event.board = decision.next_action.id  # Final Board ID
```

这组 tuple 在最初加入时，`object_id` 更接近“主持动作的 subject”。后续首 epoch adoption 修复把 Action Begin
的 `object_id` 统一解释为 Board ID，用于：

- `action_begin_timing_receipt()` 的 `expected_board_event_id`；
- `ActionRunKey.board_event_id`；
- `ActionDeadlineHint.board_event_id`；
- prepared Begin 与 current/finalizing view 的 replay 匹配；
- canonical transition 的 ownership 关联。

旧构造器没有随新不变量迁移，形成内部字段契约漂移。

### 3.2 Candidate-Cohort 绕过统一 adoption 初始化

简单 Floor 使用：

```text
prepare_v2_action_begin(...)
  -> build exact signed Begin
  -> insert ActionBeginAdoption(begin_event_id, board_event_id, session_epoch)
  -> persist prepared Begin
  -> submit
```

Candidate-Cohort 使用：

```text
prepare_moderator_action(...)
  -> build_moderator_action_event(FinalizeActions)
  -> prepare_and_submit_moderator_event(...)
```

第二条路径没有插入 `ActionBeginAdoption`。当前生产代码中 adoption 的生产入口只覆盖前一条路径；测试中的
手工 insertion 不代表真实 Candidate-Cohort 会执行相同步骤。

### 3.3 两个缺陷共同阻止首 epoch 接管

当前实际先在 receipt 校验阶段失败：

```text
receipt.board_event_id = Final Board ID
expected object_id     = Decision Attempt ID
=> board_event_id_mismatch
```

即使只改成：

```text
object_id = Final Board ID
```

后续 `record_process_local_action_begin_timing()` 仍会因为 adoption 不存在而返回
`process_correlation_missing`。canonical State 一侧同样没有 record 可以写入 `canonical_key`，最终仍不会形成
dispatch permit。

因此字段修复与 adoption 初始化必须作为一个交付完成，不能只补其中一半。

### 3.4 现有测试制造了简单路径的理想前提

当前 response-first / State-first 测试手工构造：

```text
ProtocolSubmissionContext::Moderator {
  action_kind: action_begin,
  object_id: board_event_id,
  attempt_id: None,
}
```

它验证了简单 Floor 的乱序汇合，却没有从 Candidate-Cohort 的真实 `FinalizeActions` 输出开始，也没有使：

```text
decision_attempt_id != board_event_id
```

Candidate-Cohort 现有测试只验证 `finalize_actions` JSON 能被解析，没有贯穿 signed Begin、HTTP receipt、Relay
State、adoption、dispatch 和 renewal，所以形成假覆盖。

## 4. 因果排除

### 4.1 Project View Summary 合并不是直接原因

`feat/project-view-summary` 对 `meeting_v1.rs` 的生产改动只扩展了 Action Finalization 可用工具与 Summary
提示词，没有修改 Action Begin builder、receipt validator、adoption、dispatch permit 或 renewal。

本次 Action Turn 从未派发，新 Summary 提示词也从未进入 Provider。因此本次只能判定 Summary **未验收**，
不能据此判定 Summary 实现失败。

### 4.2 429 不是首因

ACP 在 Begin accepted 后约 0.4 秒已经记录 `board_event_id_mismatch`。HTTP 429 从约 45 秒后才出现，可能加剧
后续 backfill 噪声，但不能解释最初的 receipt 拒绝、零 dispatch 与零 renewal。

429 需要作为独立容量/查询节流观察处理，不应借此掩盖确定的 ID 与 adoption 缺陷。

### 4.3 不涉及 physical slot / ACP Session

本次没有发生 `affinity_lost`，也没有证据表明 Provider child、ACP Session 或主持 Agent 身份改变。Action
Turn 在任何槽 claim 之前就被本地接管门禁阻断。

修复后仍按 logical host 模型选择任意健康槽；不得恢复已删除的 exact-slot/session 正确性约束。

## 5. 修复不变量

1. `board_event_id` 与 `decision_attempt_id` 是两个独立坐标，禁止复用一个无类型 `object_id` 表达二者；
2. 简单 Floor 与 Candidate-Cohort 必须调用同一个 Action Begin preparation/submission 入口；
3. 每个本进程新签的 Begin 在发送前必须建立且只建立一个 `ActionBeginAdoption`；
4. adoption 必须绑定 exact Meeting、Begin Event、session epoch/process generation 和 Board；
5. 可选 Decision Attempt 只参与 Relay decision fence，不参与 Board receipt 比较；
6. receipt 与 State 任意顺序、重复或延迟只能产生一个 `ActionRunKey` 和一个 dispatch permit；
7. accepted receipt 等待 State/槽期间仍由 logical-host process renewer 保持 lease；
8. cold restart 看到历史 runnable run 仍不得自动重做外部物化；
9. Retry、Return-to-Board、Abort、End 与新 window 必须 fence 旧 adoption；
10. Board/Run/window/Begin Event 任一不匹配时继续 fail closed，不得选择“最接近”的 Board；
11. 不改变 Human Action Finalization、Relay Action Run CAS 或 completion ACK 语义；
12. 不删除、重置或自动恢复当前 blocked Meeting 与既有业务数据。

## 6. 实现方案

### 6.1 所有 Action Begin 统一进入专用函数

保留并收敛一个专用入口，例如：

```rust
prepare_v2_action_begin(
    meeting_id,
    origin_turn_id,
    view,
    board_event_id,
    decision_attempt_id: Option<&str>,
    hard_deadline,
)
```

调用关系调整为：

```text
no-candidate Floor FINALIZE_ACTIONS
  -> prepare_v2_action_begin(..., decision_attempt_id=None)

Candidate-Cohort finalize_actions
  -> prepare_v2_action_begin(..., decision_attempt_id=Some(attempt_id))
```

`ModeratorActionSpec::FinalizeActions` 不再通过通用 `build_moderator_action_event()` 返回一个含糊的
`object_id` tuple。通用 `prepare_and_submit_moderator_event()` 应拒绝或断言不能直接接收新的
`action_kind=action_begin`，防止第三条旁路再次出现。

### 6.2 明确持久字段语义

为兼容现有 ledger，可保留 `PreparedModeratorAction` 结构，但建立不可变约束：

```text
action_kind == action_begin 时：
  object_id == board_event_id
  attempt_id == optional decision_attempt_id
  event.board tag == object_id
  event.decision-attempt tag == attempt_id（若存在）
```

更推荐在内部 submission context 中使用 typed variant：

```text
ActionBegin {
  board_event_id,
  decision_attempt_id,
}
```

receipt handler 不再从通用 `object_id` 猜测 Board，而是用 exact Begin Event ID 查找
`ActionBeginAdoption.board_event_id`，并交叉验证签名 Event 的 `board` tag。

旧 ledger 中 `action_begin + object_id=attempt_id` 的记录不自动重写或重放。若对应 run 已 blocked，维持
fail-closed；不得为修复验收数据而伪造 adoption。

### 6.3 将 adoption 建立纳入提交前置条件

统一入口应按以下顺序执行：

1. 从 verified current view 取得 exact Board ID、control epoch、board window 与 State Event；
2. 构造并签名唯一 Begin Event；
3. 校验签名 Event 的 Meeting、Board 与可选 Decision Attempt tags；
4. 持久化 prepared Begin；
5. 在当前 process/session epoch 下注册 `ActionBeginAdoption`；
6. 才允许异步提交 Event；
7. 若持久化或 adoption 注册失败，不发送 Begin。

若异步提交明确 rejected：终止该 adoption 并按 canonical State reconcile。若 response uncertain：保留 exact
signed Begin，只允许幂等重交同一 Event。

### 6.4 HTTP receipt 与 canonical State 的关联

HTTP accepted 一侧必须校验：

```text
accepted.event_id == adoption.begin_event_id
response.meeting_id == adoption.meeting_id
response.outcome == action_finalization_began
response.action_window_epoch == 1
response.board_event_id == adoption.board_event_id
timing fields valid
```

canonical State 一侧必须校验：

```text
transition.caused_by_event_id == adoption.begin_event_id
phase == finalizing_actions
action.condition == runnable
action.window == 1
action.board_event_id == adoption.board_event_id
action.run_id == receipt.run_id
```

两侧完全相同后才生成 permit。Decision Attempt ID 不进入 `ActionRunKey`；它只证明 Begin 是在对应
Candidate-Cohort Attempt fence 下提交的。

### 6.5 Replay 与 current-view matcher

`prepared_action_begin_matches_view()` 与
`prepared_action_begin_matches_finalizing_view()` 不应继续假设任意 moderator `object_id` 都是 Board。

Action Begin replay 应从 typed fields 或签名 Event 的 `board` tag 取得 Board，并同时验证：

- expected control epoch；
- board window；
- expected State；
- optional decision-attempt；
- current host identity；
- exact Begin Event transition。

任何旧 prepared record 若 event tag 与本地字段不一致，记录结构化 `prepared_begin_identity_mismatch`，请求
canonical reconcile，并停止重放；不得新签第二个 Begin。

### 6.6 Renewal 与错误分类

一旦 accepted timing receipt 已验证，process-level renewal 可以在 State/dispatch 汇合期间运行；它只表达
logical host 仍在线，不表示 Provider 已开始或业务已经完成。

以下情况不得记为 `provider_failure`：

- adoption 尚未建立；
- Board/Attempt 字段内部不一致；
- receipt/State 尚未汇合；
- Action Turn 从未派发。

内部 observer 应记录明确 reason；若无法安全接管，停止续租并让 Relay 以真实 lease 状态 fail closed。公开
协议本次不新增 reason code。

## 7. 测试方案

### 7.1 必须先加入一个当前代码会失败的回归

测试必须从真实 Candidate-Cohort `ModeratorActionSpec::FinalizeActions` 开始，而不是手工构造理想化的
`ProtocolSubmissionContext`。

固定：

```text
decision_attempt_id != board_event_id
```

然后断言：

1. signed Begin 的 `board` tag 等于 Board ID；
2. `decision-attempt` tag 等于 Attempt ID；
3. prepared Begin 的 Board 与 Attempt 字段没有混用；
4. Begin 发送前 adoption 已存在；
5. accepted receipt 不产生 `board_event_id_mismatch`；
6. receipt-first 与 State-first 都只生成一个 dispatch permit；
7. Action Turn 恰好派发一次；
8. 首次 lease 内至少成功一次 renewal/progress；
9. `COMPLETE` 后只产生一个 completion End 并正常关闭。

### 7.2 路径矩阵

- no-candidate Floor + `decision_attempt_id=None`；
- Candidate-Cohort + `decision_attempt_id=Some`；
- response-first；
- State-first；
- duplicate accepted receipt；
- duplicate/replayed State；
- wrong Board receipt；
- wrong Decision Attempt tag；
- Begin response 延迟但在 lease 内到达；
- old epoch response 在 Retry 后迟到；
- Return-to-Board / Abort / End 清理 adoption；
- cold restart 不自动重做 runnable Action；
- 两场 Meeting 并发不串 event/run/Board。

### 7.3 真实 handler 集成测试

至少一条测试必须走完整链路：

```text
Candidate-Cohort output
  -> exact signed Action Begin
  -> Relay execute_action_command
  -> private accepted receipt
  -> Relay-signed canonical State
  -> ACP result/state 任意顺序汇合
  -> Action dispatch
  -> renewal/progress
```

禁止通过手工插入理想 Context 或普通 `events` 伪造成功路径。

### 7.4 真实 Provider 验收

修复后召开至少 3 场零干预 Meeting：

- 至少 2 场明确由 Candidate-Cohort 决定 `finalize_actions`；
- 至少 1 场走简单 Floor；
- 3/3 在 epoch 1 产生 dispatch、renewal 与 provider progress；
- 3/3 不需要 DM 调用 `actions begin/retry`；
- Summary、Project View、Document、包含当前 Meeting 的 Project Context 均完成 canonical readback；
- 3/3 以 `completed_closed` 正常关闭；
- 不出现 `board_event_id_mismatch`、`process_correlation_missing`、零进度 lease expiry 或重复物化。

## 8. 可观测性

`Action Begin response did not contain...` 日志应在不记录敏感内容的前提下增加：

```text
meeting_id
begin_event_id
path = simple_floor | candidate_cohort
expected_board_event_id
actual_board_event_id
decision_attempt_present = true | false
adoption_present = true | false
session_epoch/process_generation
```

Board/Event ID 可完整记录，它们是公开坐标；不得记录 prompt、Board 正文、私钥、auth tag 或工具参数。

Observer 需能区分：

- Begin 未提交；
- Begin rejected；
- Begin accepted，等待 State；
- State 已到，等待 receipt；
- adoption ready；
- Action dispatched；
- first progress；
- internal identity mismatch。

## 9. 影响范围与发布

### 9.1 代码范围

主要修改：

- `crates/buzz-acp/src/meeting_v1.rs`
  - Candidate-Cohort FinalizeActions 路由；
  - dedicated Begin preparation；
  - typed Board/Attempt identity；
  - adoption、receipt/State correlation、replay matcher 与测试。

可能的最小配套修改：

- `crates/buzz-acp/src/meeting.rs`
  - 若为 prepared Begin 增加兼容字段或 observer projection；
- `crates/buzz-acp/src/observer.rs`
  - 增加低基数诊断字段；
- `crates/buzz-test-client/tests/`
  - 真实 Candidate-Cohort Action Begin 集成回归。

### 9.2 不需要的修改

本次不需要：

- DB schema migration；
- Meeting wire protocol 或 capability bump；
- Relay Action Run 状态机修改；
- Desktop Summary UI 修改；
- Project View / Document / Project Context 协议修改；
- 恢复 physical slot / ACP Session affinity；
- 回填或删除历史 Meeting 数据。

### 9.3 数据安全

自动化只允许 scratch database/Community。不得对本地主开发数据库执行 reset、truncate、drop、migration
destructive test 或 Desktop app-state 清理。

修复前后的现场验收应记录 Project View、Document catalog、Project Context revision 与 active Meeting
基线，确认只产生验收明确要求的增量。

当前 blocked Meeting 不在本次实现中自动 Retry、Return、Abort 或关闭；是否处理由 Human 另行决定。

## 10. 实施顺序

1. 增加 Candidate-Cohort 当前必失败的回归测试；
2. 统一两条 Floor 的 Action Begin preparation/submission；
3. 分离 Board ID 与 Decision Attempt ID；
4. 让 Candidate-Cohort 建立完整 `ActionBeginAdoption`；
5. 收敛 receipt/State/replay matcher 到 exact typed identity；
6. 跑 ACP unit、integration、clippy、fmt 与 diff check；
7. 使用独立 scratch DB 跑 response-first/State-first 与重复事件矩阵；
8. 重新构建 ACP/Relay/Desktop，但不清除业务数据；
9. 召开 3 场零干预真实 Provider Meeting；
10. Summary 与三域 readback 全部通过后再把状态改为“已实现”。

## 11. 完成标准

1. 简单 Floor 与 Candidate-Cohort 只存在一个 Action Begin 生产入口；
2. 代码中不再把 Decision Attempt ID 作为预期 Board ID；
3. 所有新签 Begin 在发送前都有 exact adoption；
4. receipt-first / State-first 均在 epoch 1 唯一派发；
5. Candidate-Cohort 在无 DM 干预时不再零进度过期；
6. wrong Board/Attempt/Run/window 继续 fail closed；
7. cold restart、Retry、Return、Abort、End 的安全边界不回退；
8. 真实 Provider 3/3 在 epoch 1 完成并正常关闭；
9. Project View Summary 与三域物化获得有效执行样本并通过 canonical 验收；
10. 没有删除、重置或改写既有数据。

## 12. 非目标

- 不恢复或处理本次已经 blocked 的验收 Meeting；
- 不把 Summary 提示词调整纳入此次根因修复；
- 不以增加 lease TTL 掩盖接管失败；
- 不用 Retry 自动规避首 epoch；
- 不放宽 Relay 对 Board、Run、window、host 或 State fence 的校验；
- 不把 private Action command 暴露到普通 Event 流；
- 不顺带处理后续出现的 429 查询节流问题。

## 13. 实施记录（2026-08-10）

### 13.1 已完成代码

本轮已在 `crates/buzz-acp/src/meeting_v1.rs` 完成以下收敛：

1. Candidate-Cohort 的 `ModeratorActionSpec::FinalizeActions` 不再经过通用 moderator event builder，
   而是与无候选人的简单 Floor 一样调用 `prepare_v2_action_begin()`；
2. `PreparedModeratorAction.object_id` 在 `action_kind=action_begin` 时固定表示
   `board_event_id`，`attempt_id` 独立保存可选 `decision_attempt_id`；
3. 通用 builder 对 Candidate-Cohort `FinalizeActions` fail closed，通用提交入口也拒绝没有预注册
   `ActionBeginAdoption` 的新 Action Begin，避免重新出现第三条旁路；
4. 新签 Begin 在发送前校验 `board` 与 `decision-attempt` tags，并将 Board、Attempt、Meeting 和
   session epoch 注册到 exact Begin Event 对应的 adoption；
5. HTTP accepted receipt 不再从通用 `object_id` 推测 Board，而是按 Begin Event ID 读取 adoption 的
   typed Board；submission context 中的 Attempt 也必须与 adoption 一致；
6. replay/current-view matcher 同时校验本地 Board/Attempt 字段和签名 Event tags；旧的含糊或不一致
   prepared record 不会被当作新路径重放；
7. 诊断日志增加路径、expected/actual Board、Attempt 是否存在、adoption 是否存在与 session epoch，
   不记录 Board 正文、prompt、密钥或工具参数。

### 13.2 新增回归

新增的回归从真实 Candidate-Cohort decision record 与
`ModeratorActionSpec::FinalizeActions` 开始，而不是手工构造理想 prepared action：

- `candidate_cohort_action_begin_uses_board_identity_and_registers_adoption`
  - 固定 `decision_attempt_id != board_event_id`；
  - 验证 prepared fields、签名 tags、adoption 与 replay matcher；
  - 验证丢失 Attempt fence 的本地记录 fail closed；
- `candidate_cohort_action_begin_adopts_response_and_state_in_either_order`
  - 覆盖 HTTP receipt-first 与 Relay State-first；
  - 两种顺序都只产生一个 `ActionRunKey`/dispatch permit；
  - ledger 从 `adopting` 收敛为 `pending`，并且只排入一个 Action Finalization Turn；
  - 不进入 `orphaned/provider_failure`。

简单 Floor 原有的双时序、renewal、cold restart、Retry、Return、End 等测试继续通过。

### 13.3 已通过门禁

```text
cargo test -p buzz-acp --lib candidate_cohort_action_begin_ -- --nocapture
  2 passed

cargo test -p buzz-acp --lib meeting_v1::tests
  118 passed

cargo test -p buzz-acp --lib
  830 passed

cargo check -p buzz-acp --all-targets
cargo clippy -p buzz-acp --all-targets -- -D warnings
cargo fmt -p buzz-acp -- --check
git diff --check
```

所有命令均通过；未运行会连接本地主开发数据库的集成测试，未执行 migration、reset、truncate、drop，
也未改动当前 blocked Meeting 或现有 Project View、Document、Project Context 数据。

### 13.4 尚未宣称完成的验收

本轮尚未完成：

- 使用独立 scratch database 的真实 Relay `execute_action_command` 全链集成；
- 至少两场 Candidate-Cohort 加一场简单 Floor 的零干预真实 Provider 3/3；
- Summary、Project View、Document、Meeting Context Edge 的现场 canonical readback。

因此当前状态是“代码与 ACP 自动化完成”，不是“真实 Provider 验收完成”。完成上述现场验证后，才能把
第 11 节的第 8～10 项标记为已满足。
