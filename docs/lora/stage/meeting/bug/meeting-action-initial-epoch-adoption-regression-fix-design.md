# Meeting Action 首 Epoch 接管与首次 Progress 回归修复设计

> 状态：代码实现完成；自动化回归已通过，真实 Provider Meeting 3/3 验收待完成
>
> 日期：2026-08-09
>
> 范围：Agent-hosted Meeting Action Finalization、Action Begin receipt、canonical State 关联、
> process-level renewal、首次 Action Turn 调度与错误分类
>
> 关联设计：
> [Action Context Attach 与首次调度 Permit 修复设计](meeting-action-context-attach-and-initial-dispatch-permit-fix-design.md)、
> [逻辑主持人 ACK 与同步简化实现设计](../fix/meeting-action-finalization-logical-host-ack-simplification-implementation-design.md)

## 1. 结论

Action Finalization 首 execution window 的接管问题已经连续两场独立 Meeting 复现，当前样本为 2/2。
它不能再被分类为 Provider 偶发或单次时序噪声。

两次共同事实：

- Relay 正常接受 Action Begin；
- epoch 1 的 `progress_seq = 0`；
- epoch 1 没有任何 lease renewal；
- 没有证据表明 Action provider/tool Turn 已开始；
- 同一 Action Run Retry 到 epoch 2 后，立刻能够持续续租、执行业务物化并正常关闭 Meeting。

不同之处只是最终表象：

- 第一次：epoch 1 在 90 秒后 `action_lease_expired`；
- 第二次：epoch 1 在约 170ms 后被 Harness 主动 BLOCK 为 `provider_failure`。

所以问题位于**首次 Begin 被接受以后、首次 Action provider Turn 派发以前**。`provider_failure` 是错误分类，
不是根因。

现有实现把同一个本地 Begin 的接管拆成两个异步集合：

```text
ownership_ready  <- Relay-signed canonical State
timing_ready     <- HTTP Begin response
dispatch_ready   <- 两者同时存在
```

同时，canonical `finalizing_actions` 一旦在集合尚未闭合时被投影成 `orphaned`，reconcile 会立即提交
`BLOCK(provider_failure)`；orphaned 记录也不会续租。尽管单元测试覆盖了两种理想到达顺序，真实 handler、
WebSocket projection、HTTP result queue、ledger mutation 与 reconcile 的组合仍存在丢失接管证据或过早
orphan 的窗口。

修复方向不是恢复原槽/原 ACP Session affinity，而是进一步兑现逻辑主持人的简化模型：

- 本进程签署并提交的 exact Begin 是本地来源事实；
- Relay accepted response 是 Begin 已创建 Action Run 的权威 receipt；
- canonical State 是 current run/window/Board fence 的权威投影；
- 两者由一个 Begin adoption record 原子汇合；
- 汇合期间保持 `adopting`，启动 process-level renewal，不得伪装为 Provider 失败。

## 2. 事故证据

### 2.1 第一次独立 Meeting

- Meeting：`3d1be41f-0ff3-4488-807a-f365e5df96ec`；
- Action Run：`ec36f9c4-7cf5-4018-85df-ee08f16cb022`；
- epoch 1：`action_lease_expired / progress_seq=0`；
- epoch 1 renewal：0；
- epoch 2：3 次 renewal，最终 `completed_closed`。

这证明首窗口没有被活跃 Harness 接管；不是模型工作 90 秒后失败。

### 2.2 第二次独立 Meeting

- Meeting：`e7e45686-ef96-4bbd-a9d5-edbe3d803a19`；
- Action Run：`9cac0b94-495d-46fa-abc1-d478b501a321`；
- Begin Event：`82ce05c6f419be279922fb404e5ed5dd878b55750a0ad687278af1a40d43a465`；
- Begin receipt：accepted，epoch 1，90 秒 lease，run/window/Board/timing 字段完整；
- canonical Begin State revision 45 正确引用同一 Begin Event、Run 与 Board；
- 约 170ms 后 BLOCK Event `369eba...` 被接受；
- epoch 1：`provider_failure / progress_seq=0`，renewal 0；
- epoch 2：13 次 renewal，最终 `completed_closed`。

第二次样本排除了 Relay Begin 拒绝、Board fence 错误、Provider 长时间执行和工具物化失败。

### 2.3 恢复路径健康不等于首次路径健康

epoch 2 通过 `process_blocked_action_windows` 的 Retry 接管路径获得 dispatch permit。两次 Retry 都成功，
说明：

- logical host 的可用槽、ACP provider 与工具链本身健康；
- Action Run retry、renew、业务 CAS 与 completion ACK 健康；
- 缺陷集中在 initial Begin 的 adoption path，不能用 epoch 2 成功掩盖。

## 3. 根因边界

### 3.1 本地来源事实被不必要地拆成两套易失集合

`prepare_v2_action_begin()` 已经在当前进程生成 exact signed Event，并记录
`process_action_begin_events[event_id] = meeting_id`。HTTP result 与 canonical State 随后分别写入：

```text
action_begin_timing_ready
action_begin_ownership_ready
```

只有两套 `ActionRunKey` 完全同时存在时才生成 `action_dispatch_permits`。这些集合与
`prepared_moderator_action`、Meeting ledger、live view 分属不同更新路径，任何提前清理、State snapshot 缺少
transition、result queue 延迟或 reconcile 插入都可能让关联永远无法闭合。

### 3.2 `orphaned` 同时表示冷启动风险和活跃 Begin 尚未汇合

冷启动时看到一个历史 runnable Action Run，不能自动重做外部写入；将它标记为 orphaned 并 fail closed 是
正确的。

但当前 live process 刚刚提交 Begin、HTTP / State 证据仍在飞行时，也可能先被投影为相同的 orphaned。
这把两种完全不同的状态混为一谈：

```text
true orphan:
  当前进程没有创建该 Begin，不能证明执行权

live adoption pending:
  当前进程刚创建 Begin，正在等待 receipt / State 汇合
```

`reconcile()` 对二者统一立即提交 `BLOCK(provider_failure)`，导致第二次样本在 170ms 内失败。

### 3.3 Renewal 错误地依赖 adoption 已完成

process-level renewal 会跳过 `record.state == orphaned`。第一次样本中接管没有闭合，epoch 1 因此 90 秒内
零续租并自然过期。

一旦 Relay 已接受本进程的 Begin 并返回可信 timing，续租表达的是“逻辑主持 Harness 仍在线”，不应等待
Provider Turn 已派发，也不应依赖旧槽/Session。

### 3.4 pre-dispatch 失败被误报为 Provider failure

`provider_failure` 应表达已经尝试调用 Provider，而 Provider/ACP 协议失败。当前 `runtime-recovery` orphan
分支在没有派发 Action Turn 时也提交同一 reason，破坏诊断，并让两个相同的首次接管问题分别表现为
`provider_failure` 与 `action_lease_expired`。

### 3.5 当前遥测不足以唯一定位丢失的子分支

DB 可以证明 accepted Begin、正确 State、零 renewal 与立即 BLOCK，但当前持久日志没有完整保存：

- exact Begin result 解析 reason；
- HTTP result 与 State 的到达顺序；
- adoption record 每次状态变化；
- prepared Begin 被清理的原因；
- orphan 判定前已有的 process-local evidence。

因此不能诚实地把本轮唯一归因于某一个 `BTreeSet` 插入失败；修复应移除这类脆弱组合，而不是只给某一
分支加延时。

## 4. 修复不变量

1. 不恢复 physical slot / ACP Session affinity；
2. 同一 logical moderator、Action Run、window、Board 最多派发一个 Action Turn；
3. 冷启动看到的历史 runnable run 不得自动执行业务物化；
4. 当前进程签署的 Begin 在 receipt / State 汇合期间不得被当作 true orphan；
5. Relay accepted Begin timing 到达后，即使尚未取得槽，也必须由进程级 renewer 保持 lease；
6. Provider Turn 派发前不得产生 `provider_failure`；
7. HTTP result / State 任意顺序、重复或延迟都只能产生一个 adoption 与一个 dispatch；
8. exact signed Begin 只可幂等重交，不可为同一 Board 新签第二个 Begin；
9. Retry、Return-to-Board、Abort、Meeting End 和 host identity 变化必须 fence 旧 adoption；
10. 部分 View / Document / Context 写入继续按各域 CAS 幂等恢复；
11. completion 仍以显式 `End(attestation=actions-recorded)` 为唯一成功 ACK；
12. 不删除或重置现有 Meeting 数据。

## 5. 修复方案

### 5.1 用单一 Begin adoption record 替代三个松散集合

为当前进程刚提交的 Action Begin 保存一个内聚记录，key 至少包含：

```text
meeting_id
begin_event_id
board_event_id
submitted_process_generation
```

接到 Relay accepted response 后补全：

```text
action_run_id
action_window_epoch
verified_timing
```

接到 canonical State 后补全：

```text
state_event_id
transition.caused_by_event_id
canonical ActionRunKey
```

状态建议保持最小：

```text
submitted
accepted_waiting_state
state_seen_waiting_receipt
ready
rejected | superseded | terminal
```

该记录同时承载 prepared Begin 的生命周期；不要再让
`process_action_begin_events`、`ownership_ready`、`timing_ready` 与 `prepared_moderator_action` 独立决定是否
清理。

### 5.2 accepted HTTP response 直接证明 process-local source

`RestClient::submit_event_outcome()` 已返回 `ProtocolSubmitAccepted { event_id, response }`。Action Begin
处理路径不应丢弃 typed `event_id` 后再要求 JSON body 自己携带相同字段。

将 protocol result 保留为 typed accepted result，校验：

```text
accepted.event_id == adoption.begin_event_id
response.meeting_id == adoption.meeting_id
response.outcome == action_finalization_began
response.board_event_id == adoption.board_event_id
response.window == 1
timing fields valid
```

这已经证明“当前进程提交的 exact signed Begin 被 Relay 接受”。canonical State 的职责是验证 current
run/window/Board fence，而不是再次证明事件来自哪个进程。

### 5.3 State 先到时进入 adopting，不得立即 orphan

若 canonical State 的 `caused_by_event_id` 匹配一个 live adoption：

- 写入 canonical evidence；
- ledger state 设为 `adopting`；
- 保留 prepared signed Begin；
- 请求快速读取 HTTP duplicate receipt，或幂等重交同一 Begin；
- 不调度 Provider Turn，也不提交 BLOCK。

只有进程启动时根本没有 matching adoption，或 adoption 明确属于旧 process generation，才可标为 true
orphan。

### 5.4 Response 先到时立即启动进程级续租

accepted response 提供可信 Action Run、window、Board 与 monotonic TTL 后：

- 安装 `ActionDeadlineHint`；
- 注册 process-level renewal；
- 请求 canonical fast backfill；
- 等 State fence 匹配后把 adoption 原子推进为 ready；
- 再按统一 Meeting claim 策略选择任意健康槽派发一次 Action Turn。

renewal 绑定 logical host + `ActionRunKey`，不绑定 slot / ACP Session，也不表示业务已经完成。

### 5.5 延迟 orphan 与错误分类

移除 live adoption 的即时：

```text
orphaned -> BLOCK(provider_failure)
```

正确分类：

- Provider Turn 已派发并返回 provider/ACP failure：`provider_failure`；
- 没有可用工具/槽且已达到明确本地容量边界：`tool_unavailable`；
- receipt / State 暂未汇合：保持 `adopting`，继续 bounded reconcile；
- 无法在可信 lease 内完成汇合：停止续租，让 Relay 产生 `action_lease_expired`，并记录内部
  `action_begin_adoption_failed` reason；不得伪称 Provider 已失败；
- 冷启动 true orphan：保持现有 fail-closed，但 observer 必须使用 `runtime_orphaned` 内部分类。

如果未来需要公开区分 runtime orphan，应单独升级 wire reason；本次不为诊断方便扩大协议面。

### 5.6 重放、Retry 与停止屏障

- accepted response 丢失：幂等重交同一 signed Begin，读取 duplicate receipt；
- State 丢失：fast backfill current State；
- epoch 1 已 blocked：旧 adoption 终止；只有本进程观察到 blocked window 后的 Retry epoch 才建立新 dispatch；
- Return-to-Board / Abort / End：立即 fence adoption、停止 renewal、取消 pending turn；
- 旧 result/State 晚到新 epoch：按 `begin_event_id + ActionRunKey + generation` 丢弃；
- 已派发 Turn 的停止屏障继续要求旧进程确认退出后才能派发新 window。

## 6. 可观测性

每个 initial Begin 至少产生以下低基数 observer 事件：

```text
action_begin_submitted
action_begin_http_accepted | rejected | uncertain
action_begin_state_observed
action_begin_adoption_ready
action_renewal_started
action_turn_dispatched
action_first_progress
action_begin_adoption_failed
```

公共字段：Meeting ID、Begin Event ID、Action Run ID、window、Board ID、process generation、相邻阶段耗时、
失败 reason。不得记录 prompt、工具参数、Document 正文、密钥或 auth tag。

Action Run observer/history 应保留每个 epoch 的 immutable summary，避免 current row 在 Retry 后覆盖 epoch 1
的 progress/error/lease 证据。

## 7. 测试与验收

### 7.1 定向 coordinator 测试

- HTTP response 先于 State；
- State 先于 HTTP response；
- State 到达后跨越多个 reconcile tick，仍保持 adopting，不 BLOCK；
- accepted response 延迟但在 lease 内到达；
- exact Begin duplicate receipt 恢复 timing；
- response 丢失、State 存在时只重交同一个 Event；
- State 缺失、response 存在时续租并 backfill，不派发业务 Turn；
- 两场 Meeting 并发不串 adoption；
- duplicate response / State 只派发一次；
- epoch 1 response 在 epoch 2 后迟到不复活旧 Turn；
- cold restart true orphan 不自动物化；
- Return / Abort / End 清理 adoption 与 renewal。

### 7.2 真实 handler / transport 集成测试

不得只向 coordinator 手工注入理想 JSON。测试必须走：

```text
real signed Begin
  -> POST /events
  -> execute_action_command
  -> private receipt
  -> Relay-signed State/outbox/WebSocket
  -> ACP protocol result queue
  -> adoption
  -> Action Turn dispatch
```

通过 test-only barrier 分别冻结 HTTP response 和 State delivery，确定性覆盖两种乱序以及 100～500ms 的
延迟窗口。断言 epoch 1 在首个 lease 内产生 renewal 与 `action_turn_dispatched`，且没有 pre-dispatch
`provider_failure`。

### 7.3 真实 Provider 验收

修复后至少召开 3 场独立 Meeting：

- 3/3 的 Action Run 都在 epoch 1 产生首次 renewal 与 provider progress；
- 不需要 Human Retry 才开始物化；
- View / Document / Context 写入与 canonical readback 正常；
- completion ACK 唯一，Meeting 均 `ended / closed`；
- 不出现 `provider_failure / progress_seq=0` 或 `action_lease_expired / progress_seq=0`；
- active Meeting 最终为 0。

## 8. 数据安全与发布

本修复不需要 DB schema migration 或业务数据回填。若为 epoch observer 增加持久历史，应使用 additive 表或
现有 observer event，不改写既有 Action Run。

本地 ledger 若改变结构，需要明确版本迁移：保留 current prepared End、清除旧的易失 ready 集合，并把重启
时未知 runnable run判为 true orphan；不得因为 ledger 升级自动重做物化。

测试只允许独立 scratch database。不得 reset、truncate、drop 或删除 Desktop/ACP app state。历史中已经
closed/aborted 的 Meeting 保持原样。

## 9. 预期代码落点

- `crates/buzz-acp/src/relay.rs`
  - 保留 typed `ProtocolSubmitAccepted.event_id` 到 Action Begin handler。
- `crates/buzz-acp/src/meeting_v1.rs`
  - 单一 Begin adoption record；
  - canonical/HTTP 乱序汇合；
  - pending renewal、唯一 dispatch、错误分类与 observer；
  - 删除 initial Begin 对三个松散 ready 集合的正确性依赖。
- `crates/buzz-acp/src/meeting.rs`
  - ledger 字段与兼容恢复（若 adoption 需要短期持久化）。
- `crates/buzz-relay/src/api/bridge.rs`、Meeting handler tests
  - 真实 accepted response / State delivery barriers。
- `crates/buzz-test-client/tests/`
  - epoch 1 端到端 Action / materialization / completion 回归。

## 10. 完成标准

1. initial Begin 只有一个内聚 adoption 生命周期；
2. HTTP/State 任意顺序都在 epoch 1 唯一派发；
3. accepted Begin 在等待槽/State 时能够续租；
4. pre-dispatch 路径不再生成 `provider_failure`；
5. cold restart、旧 epoch 与旧 process generation 仍 fail closed；
6. Retry、Return、Abort、End 不产生双 Turn 或重复物化；
7. 真实 Provider Meeting 3/3 在 epoch 1 完成；
8. 三域物化、Context Meeting coordinate 与 completion ACK 不回退；
9. 无数据重置或历史改写。

## 11. 实施记录（2026-08-09）

本轮已完成首次 epoch adoption 修复：

- ACP protocol submission 保留 `ProtocolSubmitAccepted` 的 typed `event_id`；不再错误要求私有 Begin
  response JSON 重复携带顶层 `event_id`；
- 用一个按 exact Begin Event ID 索引的 `ActionBeginAdoption` 取代
  `process_action_begin_events / ownership_ready / timing_ready` 三套松散集合；
- HTTP accepted receipt 与 canonical State 可任意顺序到达，只有同一 Meeting、进程代际、Run、window
  与 Board 完全匹配时才生成一个 dispatch permit；
- 汇合期间 ledger 使用 `adopting`，不会在 Provider Turn 派发前误报 `provider_failure`；
- HTTP receipt 先到时立即建立本地 Action record、单调 deadline 与逻辑主持人进程级 renewal；等待
  canonical State 或可用槽不再造成 epoch 1 零续租；
- Return-to-Board、窗口推进、Meeting 终态与 runtime removal 都会 fence 旧 adoption；冷启动仍不会凭历史
  runnable State 自动重做业务物化。

自动化覆盖了 response-first、State-first、跨 reconcile 等待、首次 renewal、typed event mismatch、唯一
dispatch、冷启动 orphan、Retry 与既有 Action 生命周期回归。已通过：

```text
cargo test -p buzz-acp --lib
  828 passed

cargo test -p buzz-acp --lib action_begin -- --nocapture
cargo test -p buzz-acp --lib \
  process_local_begin_requires_timing_and_canonical_ownership
cargo test -p buzz-acp --lib \
  action_timing_receipts_use_request_start_and_reject_incomplete_envelopes
cargo test -p buzz-acp --lib \
  protocol_submission_classification_keeps_private_errors_out_of_telemetry

cargo clippy -p buzz-db -p buzz-acp --all-targets -- -D warnings
```

本轮未启动真实 Provider Meeting，也未完成第 7 项的 3/3 现场验证，因此文档保持“代码完成、现场待验收”，
不把自动化结果扩大解释为生产验收。
