# Meeting Action Finalization Context Attach 与首次调度 Permit 修复设计

> 状态：代码实现完成；自动化回归已通过，真实 Provider Meeting 现场验收待完成
>
> 日期：2026-08-08
>
> 范围：Meeting V2 Action Finalization、Project Context Meeting Coordinate resolver、
> `buzz-db`、`buzz-relay`、`buzz-acp`、SQL 延迟约束、真实 ingest 测试与端到端关闭验收
>
> 关联设计：
> [Meeting Action Finalization 中维护 Project Context 的实现设计](../../project-context/meeting-action-finalization-context-write-implementation-design.md)、
> [Meeting Action Finalization 逻辑主持人 ACK 与同步简化实现设计](../fix/meeting-action-finalization-logical-host-ack-simplification-implementation-design.md)、
> [Meeting 作为 Project Context 坐标与 Community 可见性实现设计](../../project-context/meeting-coordinate-implementation-design.md)

## 1. 结论

“Agent Memory：恢复安全阶段启动与证据契约评审”现场验收没有端到端通过。新逻辑主持人路径已经证明：

- Action Finalization 可以跨槽调度；
- epoch 2 连续完成 11 次 lease renewal；
- Project View 与 Project Document 物化和 canonical 回读成功；
- 全程没有再次产生 `affinity_lost`。

但本次同时暴露了三个实现缺陷：

1. Project Context 的 finalizing Meeting resolver 错把私有 Action Begin command 当成普通公开 Event；
2. 同一 resolver 错把 Relay-signed canonical Board 当成主持人签名的 command；
3. ACP 在首次 Begin 已被 Relay 接受时，没有建立 process-local Action dispatch permit，并把尚未派发的
   Action Turn 错误记录为 `provider_failure`。

前两项使任何真实 `finalizing_actions` Meeting 都无法作为 Project Context 坐标 attach。第三项使首次
Action window 可能在模型和工具尚未启动时被错误 BLOCK；Retry 恰好通过另一条 permit 路径绕过了该缺陷。

会议未关闭不是第四个独立 BUG，也不是 completion ACK 丢失。真实因果链是：

```text
Meeting Context attach
  -> meeting_not_attachable
  -> Agent 返回 BLOCK(external_operation_failed)
  -> 没有 COMPLETE
  -> Harness 不生成 End(attestation=actions-recorded)
  -> Relay 正确保留 active / finalizing_actions
```

正确修复必须一次解决上述三个缺陷，并用真实生产路径替换当前制造假阳性的测试夹具。不得通过把私有
Action command 写入普通 `events`、让主持人伪签 Board、放宽到只看 phase，或在 BLOCK 后自动关闭 Meeting
来绕过问题。

## 2. 事故记录

### 2.1 对象与最终状态

- Meeting：`bd6922e5-2175-427f-94a2-105dac5bf8a4`；
- Action Run：`a1062973-f418-4555-86f4-6c49a023e6a7`；
- Action Begin Event：`3ba4...571c`；
- current Board Event：`6e2e...ebd1`；
- Meeting：`active`；
- Runtime phase：`finalizing_actions`；
- Action Run：epoch 3，`blocked / external_operation_failed`；
- `terminal_status`、`completion_event_id`、Meeting End 均为空；
- Project Context revision 保持 19，目标 Edge 为 0。

Project View 与 Document 的真实物化已经发生：

- 新增 Requirement `ad9cf5ce-6d7d-497a-ba7a-f06d9624c13c`；
- Stage `a1499cd4-114e-4206-88f3-f112bc7e552a` 从 `planned` 更新为 `active`；
- 新增解释 Document `8e172640-6638-48a8-b55c-7ea8344e6d09`；
- Project revision 推进到 64，Document 总数从 18 推进到 19；
- 以上对象均完成 canonical 回读。

因此，本次失败发生在 Context attach，不应回滚或重复创建已经成功的 View / Document 对象。

### 2.2 时间线

```text
20:23:50.821  Relay 接受 Action Begin，创建 epoch 1
20:23:51.039  epoch 1 被 BLOCK(provider_failure)，间隔约 218ms
20:25:40      Human/host Retry，进入 epoch 2
20:26:06
  ...         epoch 2 连续 11 次 renewal 成功
20:30:21
20:28:56      第一次 Context attach：meeting_not_attachable
20:30:33      epoch 2 BLOCK(external_operation_failed)
20:31:23      Retry，进入 epoch 3
20:31:58      第二次 Context attach：meeting_not_attachable
20:32:50      epoch 3 BLOCK(external_operation_failed)
```

epoch 1 的关键事实：

- `progress_seq = 0`；
- renewal 数量为 0；
- Begin 后没有 `meeting_turn_dispatched`；
- Codex/provider 没有执行 Action prompt；
- 没有工具调用或业务写入；
- ACP 日志记录：`Action Begin response did not contain a verifiable lease timing receipt`。

所以 `provider_failure` 不是外部 Provider 失败，而是 ACP 本地 Begin receipt / permit 链的错误分类。

### 2.3 会议为何没有关闭

当前协议只有以下成功路径可以正常关闭 Action Finalization：

```text
Agent 返回 COMPLETE
  -> Harness 构造 current-fence actions-recorded End
  -> Relay 原子完成 Action Run、Meeting End 与 Channel archive
```

本次 Agent 在 Context attach 失败后返回的是 `BLOCK(external_operation_failed)`，不是 `COMPLETE`。Relay
没有收到 completion ACK，保持 Meeting 非终态是正确的安全行为。修复不得把“Context 写失败”改造成
“仍然自动关闭”，否则会把未完成的 Board 物化伪装成已记录。

## 3. 根因

### 3.1 Resolver 从错误的数据域读取 Action Begin

当前 finalizing Meeting resolver 使用 `meeting_coordinate_event_exists_tx()`，要求
`begin_event_id` 对应的 `KIND_MEETING_ACTION_COMMAND` 存在于普通 `events` 表，并由主持人签名。

真实 ingest 边界不是这样：

```text
signed Action Begin command
  -> Meeting command handler
  -> execute_action_command
  -> meeting_v2_action_runs
  -> meeting_v2_action_command_receipts
  -> Relay-signed canonical State
```

Action command 是私有控制命令，不写入普通事件流。现场 Begin 在
`meeting_v2_action_command_receipts` 中具备：

- 相同 Community 与 Meeting；
- `command_event_id = run.begin_event_id`；
- `author_pubkey = immutable host`；
- `action = begin`；
- `accepted = true`；
- `outcome_code = action_finalization_began`；
- `action_run_id = current run`。

普通 `events` 中不存在该 Begin，符合现行隐私设计。resolver 把“私有 receipt 不在公共事件表”误判为
Meeting 证据不完整。

### 3.2 Resolver 使用了错误的 Board 签名身份

当前 resolver 还要求 current Board Event 由 frozen host 签名。生产路径中的 Board 是 Relay 根据已接受的
主持操作构造的 canonical projection，明确使用 Relay keys 签名：

```text
host command / accepted control transition
  -> DB canonical Board
  -> Relay-signed Meeting Board Event
```

现场 Board 存在于普通 `events`，kind、Meeting 与 current Board pointer 均正确，但签名者是 Relay，且与
current State projection 的签名者一致。它不是主持人的原始 command，也不应由主持人签名。

因此即使只把 Begin 改为查询私有 receipt，现有 Board signer 检查仍会稳定返回
`meeting_not_attachable`。

### 3.3 测试夹具复制了两个错误假设

当前 Project Context 正向 fixture 没有走真实 Board 和 Action command 执行路径，而是：

1. 用 host keys 手工签 Meeting Board；
2. 把 Action Begin 手工插入普通 `events`；
3. 再直接拼装 Runtime、Action Run 与 State 表记录。

测试数据刚好满足错误 resolver，因此没有发现生产路径必然失败。该 fixture 测试的是一套生产环境不会生成
的状态，不能继续作为 finalizing Meeting 的权威正向夹具。

### 3.4 首次 Begin receipt 被折叠为无信息的 `None`

ACP 的 `action_begin_timing_receipt()` 当前返回 `Option`。以下任一条件失败都会得到相同的 `None`：

- HTTP bridge response envelope 解析；
- accepted / outcome；
- run UUID 或 window；
- Board Event ID；
- `server_now_ms` / `lease_expires_at_ms` / `lease_ttl_ms`；
- operator hard deadline 字段。

现场私有 receipt 的 `response_json` 包含合法 run、window、Board 与 lease timing，但 ACP 仍打印通用警告。
由于失败谓词没有结构化记录，目前无法从日志唯一确认是 response shape、字段类型还是 timing inequality
导致 `None`。

随后发生：

```text
Begin 已被 Relay 接受
  -> action_begin_timing_receipt() = None
  -> 没有登记 ActionRunKey permit
  -> process-local Begin correlation 被清理或未及时命中
  -> canonical finalizing_actions 到达
  -> 被归类为 orphaned runnable run
  -> 自动 BLOCK(provider_failure)
```

Retry window 能执行，是因为它通过 `process_blocked_action_windows` 建立 permit，而不是因为首次 Begin 路径
恢复了。

### 3.5 SQL 延迟约束仍只接受终态 Meeting

Project Context 写入在 Rust resolver 之后还有一层 deferred constraint trigger。该触发器仍调用
`project_context_meeting_is_terminal()`，所以即使 Rust 已经把真实 finalizing Meeting 解析为
`FinalizingActions`，事务提交仍会被 SQL 层拒绝。

这不是新的业务规则，而是同一契约在两处实现后发生漂移：Rust 已支持“已冻结且有可信 Action Run 的
finalizing Meeting”，SQL 仍停留在“只有 terminal Meeting”。修复必须同时更新两层；只改 Rust resolver
会在提交阶段继续得到 `meeting_not_attachable` 的等价失败。

## 4. 修复边界与不变量

### 4.1 必须保持的边界

1. Action Begin / renew / block / retry / return command 继续保存在私有 receipt 域，不写普通 Event 流；
2. Board 与 State 继续是 Relay-signed canonical projections；
3. frozen host signature 继续验证原始 command 与 completion ACK，不用于伪造 projection signer；
4. Meeting attachability 仍以锁内 canonical DB 证据判断，不能只信客户端 phase；
5. `runnable` 与 `blocked` 的 current non-terminal Action Run 均可证明讨论已冻结；
6. retry epoch 继续沿用最初 Begin receipt。不得错误要求 Begin receipt epoch 等于 current retry epoch；
7. Context attach 失败必须零部分写入、零 Context revision 推进；
8. Agent 只有返回 `COMPLETE` 才生成 completion ACK；
9. BLOCK 不自动关闭 Meeting，不回滚 View / Document / Context 已有外部效果；
10. 不恢复 physical slot / ACP Session affinity 门禁。

### 4.2 本次不做

- 不新增 Meeting、Project Context 或 Action Run schema；
- 不新增 capability / protocol version；
- 不把 Action command 广播到普通订阅；
- 不为 resolver 创建第二份重复 Begin Event；
- 不改变 Community member 的 Context 权限；
- 不自动修复、Retry、Abort 或关闭当前事故 Meeting；
- 不清理或重建现有数据库；
- 不回滚本次已经写入的 Project View / Document。

## 5. Project Context resolver 修复

### 5.1 增加私有 accepted Begin receipt verifier

在 `buzz-db` Meeting / Action 模块提供事务内 verifier，按 current Action Run 精确校验：

```text
community_id
session_id
command_event_id == action_run.begin_event_id
author_pubkey == meeting.host_pubkey
action == "begin"
accepted == true
outcome_code == "action_finalization_began"
action_run_id == current action_run_id
action_window_epoch == 1
```

其中 receipt 的 epoch 1 表达创建 Action Run 的初始窗口。current run 经 Retry 已进入 epoch 2/3 时，仍使用同一
accepted Begin receipt；current epoch 的真实性由 `meeting_v2_action_runs`、Runtime 和 current State
共同验证。

verifier 必须查询 `meeting_v2_action_command_receipts`，不得 fallback 到普通 `events`。缺失、拒绝、错误
host、错误 Meeting、错误 run 或错误 outcome 都返回不可 attach。

### 5.2 用 projection identity 校验 Board 与 State

Project Context write coordinator 已持有当前 Community 的 `expected_projection_pubkey`。将该身份显式传给
Meeting resolver 的 current v3 finalizing 分支，并用于验证：

- current Board Event 的 kind、Meeting scope、event id 和 Relay signer；
- current State Event 的 kind、Meeting scope、event id 和 Relay signer；
- Board pointer、Action Run `board_event_id` 与 State action projection 三者一致。

host pubkey 仍用于校验 Create、私有 Begin receipt author 以及 immutable moderator identity。不要用同一个
`expected_signer` 参数同时表示 host command 和 Relay projection。

建议把参数命名区分为：

```text
host_pubkey
expected_projection_pubkey
```

避免以后再次把 command identity 与 canonical projection identity 混在一起。

本次只修正 current v3 `finalizing_actions` resolver。既有 terminal / legacy Meeting resolver 的 signer
兼容边界不在本 BUG 中顺带改变；若要把 terminal State 也收紧为 projection signer，必须先独立审计历史
Meeting 数据，不能因统一函数签名而意外使既有终态坐标失效。

### 5.3 保留完整 canonical fence

修复不得把 resolver 简化成只查 receipt。最终 `verified_finalizing_actions` 至少仍要求：

- Meeting `active`、schema/policy current、Channel `room_kind=meeting`；
- Runtime phase 为 `finalizing_actions`；
- 恰好一个 current non-terminal Action Run；
- run condition 为 `runnable | blocked`；
- run 与 Runtime 的 `control_epoch` / `board_window` 一致；
- accepted private Begin receipt 匹配 current run；
- current Board pointer、run Board、Relay-signed Board 一致；
- Relay-signed current State 的 action projection 匹配 run、window、Board 与 condition。

所有读取继续在现有 Project Context write transaction 与锁顺序内完成。

### 5.4 内部诊断不改变外部隐私

外部继续统一返回 `meeting_not_attachable`，避免利用错误细节枚举 Meeting。内部日志/测试应能区分：

- `begin_receipt_missing`；
- `begin_receipt_rejected`；
- `begin_receipt_mismatch`；
- `board_projection_missing`；
- `board_projection_signer_mismatch`；
- `state_projection_mismatch`；
- `runtime_run_fence_mismatch`。

日志只记录 Meeting/Run/Event ID 与低基数 reason，不记录 prompt、Document 正文、密钥或 auth tag。

## 6. ACP 首次 Action dispatch permit 修复

### 6.1 改为结构化解析结果

将 `action_begin_timing_receipt()` 从无信息的 `Option` 改为可诊断结果，例如：

```text
Result<VerifiedActionBeginTiming, ActionBeginReceiptError>
```

错误至少区分：

- envelope 不可解析；
- accepted / outcome 不匹配；
- run / window 不合法；
- Board ID 不匹配；
- lease timing 缺失；
- lease timing 自相矛盾；
- operator hard deadline 非法。

日志只输出错误分类及 event/run/Meeting ID，不输出完整 Relay response。

### 6.2 同时接受 HTTP bridge 的两种合法 envelope

解析器需要覆盖 Relay 当前真实返回：

1. bridge 已把 response details 合并到顶层；
2. 兼容内部调用中 `message = "response:{...}"` 的 envelope。

两种输入归一化后必须走相同的 typed 校验，不能在测试中只构造内部裸 JSON。

### 6.3 Permit 以本进程签署的 Begin 与 canonical 接受事实为依据

首次 Action dispatch permit 的正确事实是：

```text
本进程生成并提交 exact signed Begin Event
AND
Relay 接受该 Event
AND
canonical Action Run / State 引用同一 caused_by_event_id、run、window 与 Board
```

correlation 必须精确绑定：

```text
HTTP submitted event_id == 本进程保存的 signed Begin event_id
Meeting/session_id == prepared Begin 的 Meeting
canonical transition.caused_by_event_id == 同一 Begin event_id
ActionRunKey == {meeting_id, action_run_id, action_window_epoch, board_event_id}
```

只匹配 run UUID 或 Board ID 不足以授予 permit。epoch 1 的迟到 response 在已经进入 epoch 2 Retry 后不得
恢复旧窗口，也不得清理 epoch 2 的 permit；另一场 Meeting 的并发 response 不得命中本 Meeting。

HTTP accepted response 与 Relay-signed State 可能乱序到达。实现必须支持两种顺序：

```text
response -> State
State -> response
```

`process_action_begin_events` 的 correlation 不能在另一条证据尚未到达时提前销毁。只有以下情况可清理：

- exact accepted response / canonical transition 已建立 permit；
- definitive rejection；
- Meeting terminal / Return-to-Board / host identity 改变；
- 本地进程生命周期结束。

冷启动后仅看到一个既有 runnable run，仍不能自动重放业务物化；继续遵守 logical-host 设计的 orphan
fail-closed 规则。该规则只修复“本进程刚刚提交的 Begin 被接受却未获 permit”，不把任意历史 run
认领为本进程工作。

### 6.4 Timing 只决定本地 deadline，不应伪装成 Provider 结果

Begin 已被 Relay 接受但 timing decoration 无法验证时：

- 不得记录外部 Provider failure；
- 首先保留 exact signed Begin Event 与 process-local correlation，不得提前清理；
- 快速 canonical backfill，确认同一 Begin 已建立 current Action Run；
- 幂等重交**同一个已签名 Begin Event**，由私有 duplicate receipt 路径重新生成当前可信 timing
  decoration；不得新签第二个 Begin；
- 能用 exact process-local Begin + canonical run/state 建立 ownership 时，只标记 `ownership_ready`；
- 只有可信 timing 同时就绪后才标记 `dispatch_ready` 并派发工具 Turn；
- 在取得可信 lease deadline 前不派发工具 Turn，也不把 run 当成 provider orphan；
- 若最终无法取得 timing，记录 `action_begin_receipt_invalid` 或同等内部 reason，不使用
  `provider_failure`。

因此首次调度至少具有两道独立门：

```text
ownership_ready = exact process-local Begin + canonical accepted transition
timing_ready    = verified lease timing for the same ActionRunKey
dispatch_ready  = ownership_ready AND timing_ready
```

幂等重交只恢复 receipt/timing，不重复创建 Action Run，也不推进 Action window。prepared Begin 必须在
`dispatch_ready`、definitive rejection、Meeting terminal/Return-to-Board 或进程结束后才可释放。

现行 public BLOCK reason 若不扩展 wire，可继续选择现有 `tool_unavailable` 或内部 fatal/reconcile 路径，
但 observer/日志必须保留精确内部分类。是否新增 public reason 应在实现 review 时单独决定；本修复不因
诊断需要强制升级 wire。

## 7. 测试修复

### 7.1 删除虚假正向 fixture 语义

finalizing Meeting 正向测试必须通过生产函数建立状态：

1. 创建真实 Meeting / Channel / roster；
2. 通过真实 Board action 生成 Relay-signed current Board；
3. 通过 `execute_action_command(Begin)` 创建 Action Run、私有 receipt 与 Relay-signed State；
4. 断言 Begin 不存在于普通 `events`；
5. 调用 production Meeting coordinate resolver；
6. 提交包含 Meeting 的真实 Project Context attach。

小型 fixture 可以用于穷举负向证据，但必须严格镜像生产存储边界：Begin 只能进入私有 receipt、Board / State
必须由 Relay identity 签名，并且必须另有一条通过真实 `execute_action_command(Begin)` 建立状态的集成测试，
防止 fixture 与生产 ingest 再次共同漂移。

### 7.2 DB / resolver 测试矩阵

正向：

- initial runnable Action Run 可 attach；
- blocked Action Run 可 attach；
- Retry 到 epoch 2/3 后仍沿用原 accepted Begin receipt，并可 attach；
- Board、State 由 Relay 签名；
- attach 后 exact / incident / contains-all 回读包含 Meeting；
- duplicate attach 返回 no-change，不推进 Context revision。

负向：

- receipt 缺失；
- receipt `accepted=false`；
- receipt `action_window_epoch != 1`；
- action/outcome/host/session/community/run 任一不匹配；
- receipt 只存在于伪造的普通 Event；
- Board 由 host 或其他 key 签名；
- State signer 错误；
- current Board pointer、run Board、State Board 不一致；
- Runtime/run control epoch 或 board window 不一致；
- phase 已 Return-to-Board；
- attach 失败时 binding、Edge 与 Context revision 均不变化。

并发：

- attach 与 Return-to-Board：Return 先提交则 attach 拒绝；attach 先提交则 Edge 保留；
- attach 与 End：End 先提交则按 verified terminal 路径判定，attach 先提交则 End 等待且 Edge 保留；
- attach 与 Retry / Block：锁内读取到的 current run/window/condition 决定结果，不接受混合 fence；
- 任一竞态失败均不得产生半条 binding、Edge 或错误的 Context revision。

### 7.3 ACP permit 测试矩阵

- 使用真实 HTTP bridge 顶层 response，首次 Begin 建立唯一 permit；
- 使用内部 `message=response:{...}` envelope，得到相同结果；
- response 先于 State与 State 先于 response 均只派发一个 Action Turn；
- accepted 但 timing 非法时，幂等重交 exact Begin 后取得 timing，再且仅派发一次；
- 延迟 response 到达时仍按 exact Event/Meeting/ActionRunKey 归属；
- epoch 1 response 在 epoch 2 Retry 后迟到，不得恢复旧窗口或清理新窗口 permit；
- 两场 Meeting 并发 Begin response 不得串 permit；
- flattened details、整数边界和 nullable operator deadline 均覆盖；
- 每个 parser 失败谓词返回结构化 reason；
- accepted Begin 不再产生 200ms 级 `provider_failure`；
- definitive rejection 不建立 permit；
- cold restart / replayed receipt 不建立新的业务执行权；
- Retry 仍需先观察当前进程内 blocked window，再允许新 window；
- 同一 run/window 的重复 response、State replay 与 reconnect 不重复派发。

### 7.4 端到端关闭回归

至少覆盖一次真实完整路径：

```text
Board freeze
  -> Action Begin accepted
  -> first epoch Action Turn dispatch
  -> View / Document materialization + canonical readback
  -> Context attach(Meeting + materialized coordinates)
  -> Context exact/incident readback
  -> Agent COMPLETE
  -> actions-recorded End
  -> Action Run completed_closed
  -> Meeting ended/closed
  -> Channel archived
```

同时断言：

- 没有 `affinity_lost`；
- 没有首次窗口的伪 `provider_failure`；
- Context revision 只按真实 attach 推进；
- completion/end 事件各只有一个；
- Desktop 最终显示 completed，而不是 active / finalizing。

## 8. 实施顺序

### 阶段一：修复 resolver 证据模型

1. 增加私有 accepted Begin receipt verifier；
2. 显式传递并校验 projection pubkey；
3. 修正 Board / State signer 与 fence 校验；
4. 增加内部低基数拒绝分类。

阶段门禁：真实 production fixture 可解析为 `FinalizingActions`，所有伪造证据均 fail closed。

### 阶段二：替换 Project Context 测试夹具

1. 删除手工插入公开 Begin 和 host-signed Board 的正向语义；
2. 通过真实 Board / Action executors 建立测试 Meeting；
3. 补齐 attach、重试 epoch、负向和 revision 原子性测试。

阶段门禁：若 production ingest 与 resolver 再次漂移，集成测试必须失败。

### 阶段三：修复 ACP 首次 permit

1. typed 解析 Begin timing；
2. 支持真实 HTTP bridge envelope；
3. 闭合 response / State 乱序 correlation；
4. 移除“未派发即 provider_failure”的错误分类；
5. 保留冷启动 orphan 与唯一 dispatch 规则。

阶段门禁：first epoch 直接派发一次 Action Turn，拒绝/重放/重启仍 fail closed。

### 阶段四：集成与现场验收

1. 在独立 scratch DB 运行 DB / Relay / Context 失败矩阵；
2. 运行 ACP parser、ordering、permit 与 restart 测试；
3. 运行一场新的真实 Provider Meeting；
4. 验证 Context Meeting coordinate 与 completion ACK 完整关闭；
5. 对开发主库只做只读基线对比，不运行 reset / truncate / destructive migration test。

## 9. 数据安全与发布

本修复不需要表结构迁移或数据回填。现有真实数据已经包含：

- accepted 私有 Begin receipt；
- current Action Run；
- Relay-signed Board；
- Relay-signed State。

但 SQL deferred constraint trigger 也实现了 Meeting attachability 门禁，因此需要一条**纯函数替换的增量
migration**：新增 `project_context_meeting_is_attachable()`，并让
`project_context_validate_new_change()` 调用它。该 migration 不修改表、不更新或删除现有行，也不回填任何
业务数据；它只让 SQL 门禁与 Rust resolver 使用同一 canonical 证据模型。

部署时由 Relay 正常启动流程幂等应用该 migration，再重新构建并重启 Relay / ACP / Desktop 所需进程。
不得执行：

- `just reset`；
- `scripts/dev-reset.sh`；
- `docker compose down -v`；
- `TRUNCATE` / `DROP`；
- 指向主开发数据库的 migration test；
- Desktop app-state 删除。

测试数据库必须显式验证为独立 scratch database。验收前后记录主开发库的 Community、Meeting、Project、
Document 与 Context 基线计数，确保修复过程没有清理业务数据。

## 10. 完成标准

只有同时满足以下条件，本文状态才能改为“已实现”：

1. 真实 Action Begin 只存在私有 receipt，Meeting resolver 仍能验证 finalizing Meeting；
2. 真实 Relay-signed Board / State 通过，host-signed 伪 projection 被拒绝；
3. runnable、blocked 与 Retry 后的 current Action Run 均按设计 attach；
4. false/missing/mismatched receipt 全部 fail closed，且 Context 零部分写入；
5. 首次 accepted Begin 在 epoch 1 派发 Action Turn，不再误报 `provider_failure`；
6. response / State 两种到达顺序均只派发一次；
7. cold restart、旧 receipt 和旧 fence 不获得业务重放权；
8. 新 Meeting 完成 View / Document / Context 物化与 canonical 回读；
9. Agent `COMPLETE` 后唯一 completion ACK 正常关闭 Meeting；
10. 无 `affinity_lost`、无伪造完成、无数据重置或回滚；
11. 相关 Rust fmt、clippy、unit、DB/Relay integration 与真实 Provider smoke 全部通过。

## 11. 预期代码落点

- `crates/buzz-db/src/meeting.rs`
  - finalizing Meeting resolver；
  - host identity / projection identity 分离；
  - accepted private Begin receipt 验证接线。
- `crates/buzz-db/src/meeting_v2_actions.rs`
  - 复用或新增事务内 receipt verifier；
  - 保持私有 command receipt 边界。
- `crates/buzz-db/src/project_context.rs`
  - production-path finalizing Meeting fixture；
  - attach 与失败原子性回归。
- `crates/buzz-relay/src/handlers/meeting_baton.rs`、`crates/buzz-relay/src/api/bridge.rs`
  - 只在需要时补真实 response-shape 集成测试，不改变私有 command 可见性。
- `crates/buzz-acp/src/meeting_v1.rs`
  - typed Begin timing receipt；
  - process-local Begin correlation 与 ActionRunKey permit；
  - 首次窗口错误分类、ordering 与 replay 测试。
- `crates/buzz-test-client/tests/`
  - 真实 handler / Relay 级 Begin → Context attach → completion close 回归。

实现 review 必须重新核对调用图，以上是预期落点，不授权顺带重构无关 Meeting、普通 Channel Session
或 Project Context 查询路径。

## 12. 实施记录

### 12.1 已完成代码

- `buzz-db` 增加私有 accepted Begin receipt verifier；finalizing resolver 不再查询普通 `events` 中不存在的
  Action command，并显式区分 host identity 与 Relay projection identity；
- Project Context 写事务把当前 projection pubkey 传给 Meeting resolver；正向 fixture 改为私有 Begin
  receipt + Relay-signed Board，负向覆盖 rejected receipt 与错误 initial window；
- Meeting Action DB 集成测试通过真实 `execute_action_command(Begin)` 验证：普通 Event 流中 Begin 数量为
  0，initial window 与 Retry 后窗口均能解析为 `FinalizingActions`；
- 新增 `0054_project_context_finalizing_meeting_attach.sql`，以 accepted private receipt、current Action Run、
  Relay-signed Board / State 和 Runtime fence 共同验证 finalizing Meeting；migration 不包含数据修改或破坏性
  DDL；
- `buzz-acp` 将首次 Begin dispatch 拆为 `ownership_ready` 与 `timing_ready` 两道门，支持 HTTP response / State
  两种到达顺序；只有同一 `ActionRunKey` 两项都成立才派发一次 Action Turn；
- Begin receipt 改为 typed parser，校验 submitted Event、Meeting、run、initial window、Board 和 lease timing；
  timing 暂不可验证时保留同一个已签名 Begin 供幂等重交，不再把未派发 Turn 误报为 Provider failure；
- Return-to-Board、终态和窗口推进会清理旧的 process-local correlation，冷启动 orphan 与 Retry 许可边界保持
  fail closed。

### 12.2 已完成验证

- `cargo fmt --all -- --check`；
- `cargo check -p buzz-acp -p buzz-db`；
- `cargo clippy -p buzz-acp -p buzz-db --all-targets -- -D warnings`；
- `cargo test -p buzz-acp --lib`：828/828 通过，其中 Meeting coordinator 定向测试 116/116 通过；
- `cargo test -p buzz-db --lib`：140 通过、220 个基础设施测试按定义 ignored；
- `cargo test -p buzz-db migration::tests`：17 通过、8 个基础设施测试按定义 ignored；
- 独立 scratch database 中，真实 Action Begin / Retry resolver 集成测试通过；
- 独立 scratch database 中，finalizing / terminal Meeting Context attach、负向证据与 revision 原子性测试通过；
- 测试临时数据库按精确名称清理；未对主开发数据库执行测试事务、reset、truncate、drop 或数据回填。

### 12.3 待现场验收

代码交付不自动 Retry、Abort 或关闭当前事故 Meeting。完整“新 Meeting 首个 epoch → View / Document /
Context 物化 → `COMPLETE` → completion ACK → closed”仍需在部署新二进制并正常应用 migration 后，用一场
新的真实 Provider Meeting 验收。完成该项前，本文不宣称端到端现场验收已经通过。
