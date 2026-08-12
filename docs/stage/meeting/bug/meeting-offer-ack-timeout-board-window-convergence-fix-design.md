# Meeting Offer ACK 超时与 Board Window 状态收敛修复设计

> 状态：代码实现完成；自动化回归已通过，3 次真实 ACK 超时现场验收待完成
>
> 日期：2026-08-09
>
> 范围：Meeting V2 Offer ACK deadline、Baton / Board 双状态机、Decision Attempt 门禁、
> deadline recovery 与受控故障注入验收
>
> 关联记录：
> `RESEARCH/AGENT_MEMORY_THREE_DOMAIN_MATERIALIZATION_REGRESSION_ACCEPTANCE_2026_08_09.md`、
> `RESEARCH/AGENT_MEMORY_MEETING_OFFER_ACTION_REPEATABILITY_ACCEPTANCE_2026_08_09.md`

## 1. 结论

Offer 问题是一个**已确认的条件性、确定性状态收敛缺陷**。

它不会在及时 ACK 的正常路径中随机出现。第二次验收的 6 个 managed Offer 均在 5 秒窗口内自动 ACK，
因此没有进入超时分支；这只能证明正常路径健康，不能证明超时路径已修复。

但第一次真实超时样本、Relay 重复恢复日志与当前代码可以共同证明：一旦状态组合已经是：

```text
Baton phase = offered
Meeting V2 runtime phase = board_pending
Offer ACK deadline 已到
```

当前 deadline recovery 会先尝试使 Offer 失败，再无条件打开一个新的 Board window。由于已有 Board window
仍为 `board_pending`，`open_board_window_tx()` 拒绝本次调用；整个事务回滚，Offer 继续保持 pending，
Baton 继续保持 `offered`，恢复任务随后重复失败。

因此，触发该组合是否依赖时序；但组合一旦出现，失败不是概率性的。修复必须同时：

1. 阻止 Board pending 时启动新的 Decision Attempt / Offer；
2. 让超时恢复能够复用已经存在的 Board window，并幂等提交 Offer 终态。

只做其中一项不能形成完整防线。

## 2. 事故证据

### 2.1 真实超时 Meeting

- Meeting：`842e628b-1ce9-4f92-a0a4-70d3c2d4f5ea`；
- Offer：`1aa12b23f31693c03c186424b70bafa04a40fc05abcb2cd7f4068bb24ed17eda`；
- Offer deadline：2026-08-09 16:48:02 CST；
- deadline 后没有 `offer_timed_out` canonical transition；
- Offer 在 Human abort 前始终保持 `pending`；
- 最终由 Human 以 `offer_timeout_state_invariant_failure` 安全 abort。

deadline 后 Relay 反复记录：

```text
Meeting V2 ... cannot open a Board window from its current phase
```

ACK、recall、decline 与 Board completion 都因为相同事务错误无法推进。

### 2.2 第二次验收没有覆盖超时分支

Meeting `e7e45686-ef96-4bbd-a9d5-edbe3d803a19` 中 6 个 Offer 全部在约 0.2～0.4 秒内被 managed runtime
自动 ACK，均远早于 5 秒 deadline。该 Meeting 最终正常关闭，但没有生成新的 Offer-expired 样本。

所以当前证据应解释为：

- 正常 managed ACK 路径：6/6 通过；
- 真实 ACK 超时路径：已有 1 个失败样本，并得到静态代码证明；
- 尚未通过受控故障注入验证修复后的超时路径。

## 3. 根因

### 3.1 Board 与 Floor 的启动门禁发生漂移

新 Intent 可以使 `ensure_moderator_window_tx()` 打开 Board window。随后一个已经排队或延迟到达的
`ModeratorDecisionAttemptStart` 只检查 Baton 是否为 `moderator_control | moderator_idle`，没有同时要求
Meeting V2 runtime 为 `floor_ready`。

这允许出现：

```text
Board 已重新进入 board_pending
  +
Decision Attempt 仍被接受
  +
Selection 创建 Offer
```

即 Baton 与 Board 两个 canonical 状态机合法状态的非法组合。

### 3.2 Offer 失败路径无条件打开新的 Board window

Offer deadline 到期后：

```text
advance_due_locked_tx
  -> fail_active_offer_tx
  -> return_control_to_moderator_tx
  -> open_board_window_tx
```

`return_control_to_moderator_tx()` 对 Meeting V2 无条件调用 `open_board_window_tx()`。后者只接受
`bootstrap_locked | floor_ready | finalizing_actions`，不接受已经存在的 `board_pending`。

因此 SQL UPDATE 返回零行并产生错误，包含 Offer `timed_out`、Baton 回到 moderator、handoff unblock 等
全部修改一起回滚。

### 3.3 恢复任务不是幂等收敛操作

deadline recovery 当前假定“需要创建 Board window”，而不是表达“恢复后必须存在一个可维护的 Board
window”。两者在已有 `board_pending` 时语义不同。重复 recovery 每次执行同一失败路径，无法自行收敛。

## 4. 修复不变量

1. `board_pending` 时不得开始新的 Candidate-Cohort 或 no-candidate Floor Decision；
2. 创建 Offer 时，Board 必须仍为同一 `control_epoch / board_window` 的 `floor_ready`；
3. Offer 超时必须在一个事务中写入 Offer 终态并清除 Baton active Offer；
4. 恢复后的目标是“恰好一个 Board window 可供主持维护”，不是“必须新建一个 Board window”；
5. 已经 `board_pending` 时复用现有 window，不递增 `board_window`，不改写其 deadline；
6. `floor_ready` 时才创建下一个 Board window；
7. 重复 recovery 必须 no-op 或返回同一个 canonical 结果，不得持续报错；
8. 不放宽 `open_board_window_tx()` 的通用状态门禁来掩盖调用者错误；
9. 不自动 ACK、不延长 Offer deadline，也不改变 Human/Agent 的既有 ACK 时长；
10. 不删除或重写既有 Meeting 数据。

## 5. 修复方案

### 5.1 为 Decision Attempt 增加 V2 Board readiness fence

在 `apply_moderator_attempt_start_tx()` 的 Community / Meeting 锁内读取 V2 runtime，并要求：

```text
runtime_phase == floor_ready
active Board control_epoch == Baton expected_control_epoch
不存在 active Offer / Grant
不存在 active Decision Attempt
不存在 pending Human priority / unresolved floor work
```

若 Board 已是 `board_pending`，返回稳定拒绝码，例如 `moderator_floor_not_ready`，而不是接受一个会与 Board
维护并行的 Decision Attempt。

同一 fence 还应在 Selection / Offer 创建前再次验证，防止 Attempt Start 后 Board 被 Human priority 或其他
canonical transition 抢先推进。

### 5.2 增加“确保 Board 可维护”的内部原语

不要让 `return_control_to_moderator_tx()` 直接调用 `open_board_window_tx()`。增加锁内 helper，例如：

```text
ensure_board_pending_for_moderator_tx(...)
```

按 runtime phase 处理：

- `board_pending`：复用当前 window，返回 `Existing`；
- `floor_ready`：调用严格的 `open_board_window_tx()`，返回 `Opened`；
- `bootstrap_locked`：只有初始化路径允许打开，否则 fail closed；
- `finalizing_actions | ended`：拒绝过期的 Offer recovery；
- runtime 与 Baton 终态不一致：返回结构化 invariant error。

该 helper 只负责收敛 Board 目标状态，不降低 `open_board_window_tx()` 本身的 CAS 约束。

### 5.3 让 Offer deadline recovery 原子且幂等

在同一事务和固定锁序中：

1. 锁定 Meeting session、Baton state、V2 runtime 与 active Offer；
2. 再次确认 Offer 仍为 pending 且 deadline 已到；
3. 将 Offer 写为 `timed_out`；
4. 清除 active Offer、Grant、Decision Attempt 与相关 handoff block；
5. 将 Baton 收敛到 `moderator_idle`；
6. 调用 `ensure_board_pending_for_moderator_tx()`；
7. 生成唯一 `offer_timed_out` State / history / outbox transition；
8. 提交后重新读取 canonical Baton + Board 组合。

同一 recovery 重放时，如果 Offer 已为 `timed_out` 且目标 State 已存在，应返回既有结果或 no-change，不能
再次打开 Board 或推进 revision。

### 5.4 增加跨状态机不变量检查

在测试与内部诊断中明确以下非法组合：

```text
board_pending + active Decision Attempt
board_pending + offered
board_pending + granted
finalizing_actions + active Offer/Grant
ended + nonterminal Offer/Grant
```

生产读取不应因为历史异常数据崩溃，但新写入必须 fail closed。deadline recovery 对能够安全收敛的
`board_pending + offered` 执行上述专用恢复；其他非法组合记录低基数 reason 并要求 operator 处理。

## 6. 测试与验收

### 6.1 DB 状态机测试

- `floor_ready -> DecisionAttempt -> Offer -> ACK` 正常；
- `board_pending` 时 Attempt Start 被 `moderator_floor_not_ready` 拒绝；
- Attempt Start 后 Board fence 改变时 Selection / Offer 创建被拒绝；
- `offered + floor_ready + deadline`：Offer timed_out，并打开一个新 Board window；
- `offered + board_pending + deadline`：复用当前 window，Offer timed_out，事务成功；
- deadline recovery 重放不增加 State revision、Board window 或 history；
- timeout 与迟到 ACK 并发只有一个 canonical 结果；
- timeout 与 recall / decline / abort 并发保持单终态；
- 任一拒绝都没有部分 Offer、Baton、Board 或 outbox 写入。

### 6.2 真实 handler / Relay 测试

通过真实 command handler 建立 Meeting，不手工拼表：

1. 创建 Offer；
2. 在 test-only fault barrier 中暂停目标 managed runtime 的自动 ACK；
3. 把权威时钟推进到 deadline 后；
4. 运行 deadline recovery；
5. 验证 `offer_timed_out`、`moderator_idle`、`board_pending` 与后续 Board completion；
6. 验证迟到 ACK 得到稳定 expired/rejected 回执。

故障注入只能存在于测试/`meeting-acceptance` 构建，不提供生产环境“关闭自动 ACK”的产品开关。

### 6.3 现场验收

修复部署后至少执行 3 次独立的真实 ACK 超时：

- 每次都保存 deadline 前、deadline 后和 recovery grace 后的 canonical snapshot；
- 3/3 都产生唯一 `offer_timed_out`；
- 3/3 都能继续 Board / Floor / Offer / Speech；
- 不出现 `cannot open a Board window from its current phase`；
- 不需要 Human abort 才能解除状态。

## 7. 数据安全与发布

本修复不需要 schema migration 或数据回填。它只修改新命令门禁和 deadline recovery 的事务逻辑。

不得使用 reset、truncate、drop、`docker compose down -v` 或破坏性迁移测试。DB 集成测试必须使用显式
scratch database；开发主库只做只读基线检查。既有 aborted Meeting 保持历史事实，不重写成 timed_out 或
closed。

## 8. 预期代码落点

- `../../../../crates/buzz-db/src/meeting_baton/commands.rs`
  - Decision Attempt / Selection Board readiness fence；
  - `return_control_to_moderator_tx()` 的 Board 收敛调用；
  - Offer deadline 幂等恢复测试。
- `../../../../crates/buzz-db/src/meeting_v2.rs`
  - 新增严格的 `ensure_board_pending_for_moderator_tx()`；
  - 保持 `open_board_window_tx()` 的现有严格 CAS。
- `../../../../crates/buzz-relay/src/meeting_runtime.rs` 或现有 recovery 测试
  - deadline 重放与结构化日志。
- `../../../../crates/buzz-acp/src/meeting_v1.rs`
  - 仅增加 test-only 自动 ACK fault barrier；不改变正常 ACK 行为。

## 9. 完成标准

1. Board pending 时不再接受新的 Decision Attempt / Offer；
2. 已存在 Board window 的 Offer timeout 能原子收敛；
3. timeout 与 ACK/recall/abort 竞态具有唯一终态；
4. recovery 重放幂等且没有 revision 漂移；
5. 正常 managed ACK、Human ACK 与 6 轮 Meeting 路径不回退；
6. 3 次受控真实超时均可继续会议；
7. 无数据重置、无历史改写。

## 10. 实施记录（2026-08-09）

本轮已完成代码修复：

- `return_control_to_moderator_tx()` 不再无条件新开 Board window，而是调用严格的
  `ensure_board_pending_for_moderator_tx()`：已有 `board_pending` 时原位复用 window，并只把
  `control_epoch` 收敛到新的主持控制代际；`floor_ready` 时才新开 window；其他阶段 fail closed；
- `ModeratorDecisionAttemptStart` 与 `ModeratorSelect` 在事务内重新核验 V2 runtime 必须为同一
  `control_epoch` 的 `floor_ready`，Board 维护期间稳定返回 `moderator_floor_not_ready`；
- `open_board_window_tx()` 的既有严格 CAS 没有放宽；
- 新增真实 Postgres 回归，覆盖 Board pending 下 Attempt/Select 拒绝、Offer deadline recovery、既有
  Board window/deadline 复用以及重复 recovery no-op。

已通过：

```text
cargo test -p buzz-db --lib
  142 passed; 223 ignored

BUZZ_TEST_DATABASE_URL=<独立 buzz_test_* scratch DB> \
  cargo test -p buzz-db --lib offer_timeout -- --ignored --nocapture
  2 passed

cargo clippy -p buzz-db -p buzz-acp --all-targets -- -D warnings
```

测试只使用独立 scratch database；执行后已删除该临时库，未修改或重置开发主库。第 6 项所要求的
3 次真实 managed runtime 超时故障注入尚未执行，因此当前不能宣称现场验收已经全部完成。
