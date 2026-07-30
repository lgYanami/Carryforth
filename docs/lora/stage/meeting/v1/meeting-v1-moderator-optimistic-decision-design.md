# Meeting V1 主持人乐观决策设计

> 状态：补充设计已确认；阶段一至三已实现并通过本地确定性验收；待阶段四针对性
> 真实 Codex qualification 与正式签收
>
> 日期：2026-07-30
>
> 前置概念设计：
> [Meeting V1：主持式发言权接力协议](./meeting-v1.md)
>
> 原后端实现设计：
> [Meeting V1 后端实现设计](./meeting-v1-backend-implementation-design.md)
>
> 问题证据：
> [Meeting V1 真实 Codex Qualification 报告](./meeting-v1-live-acceptance-report-2026-07-29.md)
>
> 决策记录：[Meeting V1 Changelog](./changelog.md)
>
> 范围：Meeting V1 后端、Relay、数据库、SDK、CLI、ACP Controller 与验收工具；
> 不包含 Desktop、Web、Mobile

## 1. 文档目的

本文重新定义 Agent 主持人的判断时机、状态快照、候选批次、结果校验和重判规则，解决
真实 Codex 验收中暴露的 Moderator State churn 与 ACP Cancel/respawn 问题。

本文优先于以下旧语义：

- `meeting-v1.md` 第 11 节中“其他人发言期间生成完整 ModeratorPlan，过时后取消”的描述；
- `meeting-v1-backend-implementation-design.md` 第 10.2 节中
  `AgendaRanking + ControlDecision` 双层 LLM 判断；
- `meeting-v1-backend-implementation-design.md` 第 14.3 节中把 Controller 调度优先级
  解释为必须物理取消正在运行的 Moderator Decision 的描述；
- 任何把完整 `intent_revision`、完整候选 fingerprint 变化直接解释为“必须重调 LLM”
  的实现；
- 任何因 Meeting State 变化而物理 Cancel 正在运行的 Moderator Decision Turn 的实现。

历史 qualification 报告保持不变，它记录的是变更前的真实执行证据。

## 2. 核心结论

主持人判断采用以下协议：

> 主持人取得 Control Token 后冻结本轮候选批次，基于快照完成一次不中断的 LLM 判断；
> 模型返回后 Full Sync，并只校验该输出实际依赖的前提。低层 CAS 冲突可以重提，语义
> 前提失效才重新调用 LLM。

完整流程是：

```text
Control Token 回到 Moderator
  -> Relay 打开 decision window 并冻结 Candidate Cohort
  -> ACP Full Sync
  -> 注册权威 DecisionAttempt，取得完整 attempt deadline
  -> 基于 Cohort 和 DecisionSnapshot 调用 LLM
  -> LLM 自然完成；期间不因 Meeting State 变化 Cancel
  -> ACP 再次 Full Sync
  -> 校验 control、Human priority、speech 和输出引用的 source
     -> 语义仍有效：用最新 CAS revision 提交
     -> 仅低层 CAS 冲突：再次 Full Sync，rebase 后重提，不调 LLM
     -> selected source 失效：刷新 attempt deadline，重新判断
     -> Human/控制权/会议生命周期变化：丢弃，等待下一控制窗口
```

## 3. 目标与非目标

### 3.1 目标

1. 会议 State 可以在模型判断期间持续推进，不依赖 ACP Cancel 成功来保证协议正确性；
2. 晚到的普通 Agent Intent 不反复推翻当前判断；
3. 真正影响本次输出的变化仍能被权威地识别并收敛；
4. Full Sync 与提交之间的竞态继续由 Relay CAS 拦截；
5. 每次必要重判获得完整的 3 分钟预算，同时不能被无限续期；
6. 行为可以通过确定性测试、结构化日志和真实 Codex 注入场景共同证明。

### 3.2 非目标

本文不改变：

- Meeting V1 的 Offer、Grant、Speech、Human Request 和 Directed Handoff 基本协议；
- 同一时间至多一个有效 Offer 或 Grant；
- Human Floor Request 的协议优先级；
- 5 分钟 Grant、确定性 Offer ACK 和最多五次直接 Handoff；
- Agent 的 advisory 工具策略；
- 单场 Agent/Participant 容量；
- 发言内容质量、主持策略或 prompt 的具体会议方法论。

本文只约束 Moderator Decision Turn。普通 Participant Intent Turn 与 Granted Speech
之间是否也全面取消物理抢占，需要单独审计，不能由本文的结论自动外推。

## 4. 判断时机

### 4.1 只在主持人持有控制权时判断

完整主持判断只能在以下条件同时成立时启动：

```text
actor == moderator_pubkey
phase in {moderator_control, moderator_idle}
active_offer_id == null
active_grant_id == null
human_priority_active == false
meeting_ended == false
```

`moderator_idle` 只有在新的可处理工作已经使 Controller 需要作出决定时才启动 LLM；纯粹
没有候选的 idle 不调用模型。

在 `offered | granted` 阶段，主持人只执行：

- 接收并验证共享会议消息；
- 更新 State、Intent、Human Request 和 Handoff 投影；
- 维护历史游标和 ACP 私有账本；
- 等待 Control Token 返回。

不得在这个阶段启动 `V1ModeratorAgenda` 或任何等价的投机 LLM Turn。

### 4.2 为什么不提前形成完整判断

其他 speaker 尚未完成时，最终 speech 还不存在。任何包含“下一位应该是谁”的完整判断都
缺少最新会议内容，并会在 `speech_revision` 增加后天然过时。

可以异步完成消息同步、候选索引和纯机械过滤，但这些操作不能调用 LLM 形成可执行选择，
也不能被称为一次 Moderator Decision。

## 5. Candidate Cohort

### 5.1 定义

Candidate Cohort 必须是 Relay 可验证的权威调度批次，不能只存在于 ACP 私有账本。否则
Relay 无法判断 late moderator self Intent 是否仍具有本轮优先级，也无法让 deterministic
fallback 限定在本轮候选中。

每个 Intent 和需要主持人处理的 open Handoff 保存：

```text
eligible_decision_epoch
```

Relay 打开新的 `decision_epoch` 时冻结本轮 eligibility。ACP 注册 DecisionAttempt 后，
Relay 根据权威投影保存该 attempt 实际看到的 source ID 和版本：

```text
CandidateCohort
- session_id
- control_epoch
- decision_epoch
- speech_revision
- opened_at
- intent_refs[]
    - intent_id
    - current_event_id
    - author_pubkey
    - moderator_self
    - eligible_decision_epoch
- handoff_refs[]
    - handoff_id
    - attempt_count
    - target_pubkey
    - eligible_decision_epoch
```

模型 prompt 只包含本 Cohort 中的可处理候选。模型输出引用 Cohort 之外的 ID 时，Harness
直接判为非法输出；Relay Select、moderator self priority、Deferral 和 fallback 查询也
必须按 `eligible_decision_epoch <= current decision_epoch` 过滤。

Candidate Cohort 不是新的发言权对象，也不需要新的 Nostr kind。它是
`decision_epoch` 对应的权威候选投影。

### 5.2 晚到 Intent

判断已经开始后新增的普通 SpeechIntent：

- 正常持久化并广播给所有参会者；
- 保持 `pending`；
- 保存 `eligible_decision_epoch = current decision_epoch + 1`；
- 不加入正在运行的 Cohort；
- 不触发 Cancel；
- 不触发本次 LLM 重判；
- 在下一次 Candidate Cohort 中进入候选。

该规则不区分普通 Agent 与主持 Agent。主持人的 late self Intent 也不能凭 self priority
插入已冻结的 Cohort。

普通 Human SpeechIntent 若保留，同样遵循该规则。Human 需要直接取得下一轮发言权时，
必须提交 Human Floor Request。

Relay 在 Intent Submit 事务中根据当前 phase/epoch 设置 eligibility：

- 已有活动 Decision Cohort：设置为 `current decision_epoch + 1`；
- `offered | granted`：设置为下一次控制权返回时将建立的 epoch；
- `moderator_idle` 且该 Intent 原子打开新 window：设置为新递增后的 epoch。

Intent Refresh 保留稳定 `intent_id` 和 `eligible_decision_epoch`，只更新
`current_event_id`；Withdraw 终结该 Intent，不把同一 ID 移动到下一 Cohort。

Handoff 在创建后直接进入 Offer/Grant 时不参与主持 Cohort；需要归还主持人处理的 open
Handoff，在控制权返回或 `handoff_unblocked` 的同一事务中绑定即将打开的
`decision_epoch`。后续 attempt 变化不改变其 eligibility。

### 5.3 下一批次何时建立

通常，当前判断选择一名 speaker，等其 speech 完成、Control Token 再次回到主持人后，
Relay 打开新的 `decision_epoch`，所有晚到 Intent 自然进入新 Cohort。

Relay 必须区分两类查询，不能用当前 Cohort filter 判断是否需要建立下一窗口：

```text
current_cohort_candidates:
    eligible_decision_epoch <= current decision_epoch

next_window_exists:
    pending source exists with
    eligible_decision_epoch <= current decision_epoch + 1
```

Grant/Human speech 结束、`complete_cohort` 或 fallback 完成当前 Cohort 时，Relay 在同一
Session 事务内先用 `next_window_exists` 探测，再递增 `decision_epoch`，最后按新 epoch
冻结 Cohort。否则 `e + 1` 的 late Intent 会因为旧查询永远不可见而饿死。

这里的“不饿死”只表示 late Intent 不会因 eligibility 过滤而永久不可见：它必须进入
紧接着建立的下一 Cohort。该 Intent 最终何时被 Select、Reject 或 Withdraw，继续服从
Meeting V1 既有公平 gate；本文不新增跨多个 Cohort 的强制终结策略。

若当前判断通过 Reject、Dismiss 等动作已经使当前 Cohort 不再有待处理主候选，同时已有
上一 Cohort 之后到达的 pending Intent，则 Moderator 使用现有 Moderator Action kind
提交 `complete_cohort`。Relay 在 Session 锁内再次确认当前 Cohort 已空，然后：

- 结束当前 `decision_epoch`；
- 递增 `decision_epoch`；
- 让此前标记为下一 epoch 的 pending source 进入新 Cohort；
- 建立新的基础 3 分钟 control deadline。

这不是旧判断被打断，也不回滚已提交的管理动作。late Intent 本身不能刷新旧 Cohort 的
deadline；只有旧 Cohort 已经完成后，新的权威 Cohort 才获得自己的窗口。

`idle` 不等于“跳过仍 pending 的当前候选”。若模型输出 idle 但本 Cohort 仍有主候选：

- ACP 以 `committed(reason=idle_wait_fallback)` 终结本次 attempt；
- Relay 把当前 epoch 标记为 `llm_closed_waiting_fallback`，拒绝同一 Cohort 的新
  AttemptStart；
- 当前 Cohort 和原 deadline 保持不变；
- 不立即再次调用 LLM，也不执行 `complete_cohort`；
- deadline 到期后由确定性 fallback 处理当前 Cohort；
- late Intent 继续等待下一 Cohort。

## 6. DecisionSnapshot 与单飞

每次模型调用保存：

```text
ModeratorDecisionAttempt
- attempt_id
- turn_id
- session_id
- control_epoch
- decision_epoch
- attempt_number
- speech_revision
- candidate_cohort
- snapshot_state_event_id
- snapshot_intent_revision
- started_at
- deadline_at
- state
    running | completed | validating | rebasing |
    retry_required | committed | discarded | timed_out | abandoned
```

同一 Meeting 同一时间至多有一个运行中的 Moderator Decision Turn。新的 State 只更新
最新投影和观测原因，不得在旧 Turn 仍运行时排队第二个 Moderator Turn。

LLM 返回后必须先 Full Sync，不能直接使用 Turn 启动时的 State 构造协议命令。

### 6.1 权威 AttemptStart

ACP 在真正 dispatch LLM 前，先提交 `decision_attempt_start`。Relay 在 Session 锁内：

1. 先按数据库时间执行 lazy deadline recovery；
2. 验证 moderator 仍持有 control、Human priority 不活跃；
3. 为本次 attempt 生成稳定 `attempt_id`；
4. 从当前 Cohort 计算并持久化 source ID、版本和 eligibility；
5. 增加 `attempt_number`；
6. 把本次 attempt deadline 设置为 `database_now + 3 minutes`；
7. 写入 canonical `moderator_decision_attempt_started` effect。

`POST /events` 的 submit response 不负责返回 CandidateCohort。Relay 签名的 kind `42103`
State 必须在 `active_decision_attempt` 中公开可读的：

```text
attempt_id
control_epoch
decision_epoch
attempt_number
speech_revision
deadline
candidate_refs[]  # source type, stable ID, current version, eligibility,
                  # summary/reason/addressed target 等最小 prompt payload
candidate_snapshot_hash
```

AttemptStart 的 effect 同时携带 `attempt_id` 和 snapshot hash。完整 refs 来自持久化
AttemptSnapshot，并保留在 State history 中，可以用 `#h=<session_id>` 和精确
State event ID 回读。ACP 必须等到订阅或 Full Sync 得到 Relay 签名、hash 匹配的 State
后才构造 prompt；仅凭本地最新 Intent 投影不得 dispatch LLM。

Snapshot 中的最小 prompt payload 使用 Meeting 私有 State 的既有可见性，不扩大到会议
外部。若 payload 只保存版本引用，ACP 必须按精确 event ID 回读该历史版本，不能用
Refresh 后的 latest projection 替代；任何 ref/payload 缺失都 fail closed，不启动模型。

若 ACP 因上一 epoch 的已失效 Turn 仍在自然运行而无法立即开始，新 epoch 仍先保留原有
3 分钟基础 control deadline。旧 Turn 在该基础 deadline 前结束后，新的
`decision_attempt_start` 可以为实际模型调用取得完整 3 分钟；若基础 deadline 已到，
fallback 先赢，新的 start 被拒绝。

每个 epoch 的 initial start 只能成功一次；retry 和进程恢复另按第 8 节计数。这样可以
保证真实模型预算，又不能靠重复 start 无限续期。

### 6.2 权威 terminal

所有由模型结果产生的 Moderator Action 都携带 `attempt_id`。Attempt 只能有一个权威
terminal：

- 主计划成功消费：`committed`；
- Human/control/speech/lifecycle 变化使结果不再需要：`discarded`；
- selected source 冲突：由 `decision_retry` 原子标记为 `retry_required`；
- deadline fallback：`timed_out`；
- Runtime 丢失：`abandoned`。

没有其他协议动作需要提交时，ACP 使用 `decision_attempt_finish` 把
`completed/discarded` 结果写回 Relay。该操作只终结已注册 attempt，不要求 actor 当前
仍持有 Control Token，也不能改变 Offer/Grant；它仍要求 actor 是冻结 moderator 且
`attempt_id` 与 attempt row 中保存的 Session、moderator 和历史 epoch 完全匹配。它不
要求该历史 epoch 等于当前 Baton State 的 epoch，否则 Human 已推进控制状态时旧
attempt 将永远无法 terminal。

Relay 在旧 attempt 到达 terminal 前拒绝同一 Session 的新 AttemptStart。Human 抢占后，
旧 Turn 因此可以自然完成，但新的 Decision 不会与它重叠；旧 attempt 被权威
`discarded` 后才允许注册新 epoch 的 attempt。

## 7. 乐观校验

### 7.1 两类冲突

必须区分：

1. **提交冲突**：Relay 的 `intent_revision` 等低层 CAS token 发生变化；
2. **语义冲突**：本次输出实际依赖的 control、speech、Human priority 或 selected
   source 已经失效。

提交冲突不自动等于语义冲突。ACP 在 CAS 被拒绝后再次 Full Sync：

- selected source 仍满足本次输出的前提：使用最新 revision 重建同一命令并重提；
- selected source 已失效：才重新调用 LLM；
- control 已转移或 Human 已接管：丢弃结果并等待，不重提。

因此，Relay 可以继续把 `expected_intent_revision` 作为事务级 CAS。需要改变的是 ACP 对
CAS 失败的解释，而不是放弃 Relay 的并发保护。

### 7.2 提交前共同校验

所有 Moderator Decision 结果在提交前必须满足：

```text
meeting is active
moderator identity is unchanged and authorized
phase in {moderator_control, moderator_idle}
control_epoch == snapshot.control_epoch
decision_epoch == snapshot.decision_epoch
speech_revision == snapshot.speech_revision
no queued/offered Human Floor Request
attempt_id is the current registered attempt
database_now < authoritative deadline, when the attempt has one
```

### 7.3 输出依赖校验

不同输出只校验自身依赖：

- `select_intent | moderator_speak`
  - `intent_id` 属于当前 Cohort；
  - `eligible_decision_epoch <= current decision_epoch`；
  - Intent 仍为 `pending`；
  - `current_event_id` 与模型所见版本一致；
  - 作者仍是合格参会者；
- `select_handoff`
  - `handoff_id` 属于当前 Cohort；
  - `eligible_decision_epoch <= current decision_epoch`；
  - Handoff 仍 open、`blocked_by IS NULL`；
  - `attempt_count` 与模型所见一致；
  - 目标仍是合格参会者；
- `reject | withdraw_self | defer`
  - 每个 action 独立校验对应 Intent 的 ID 和版本；
- `dismiss_handoff`
  - 独立校验 Handoff 的 open 状态和 attempt；
- `idle`
  - 只校验共同前提，不要求整个 Intent 池保持不变。

辅助 Reject、Dismiss 或 Deferral 的单个目标已经失效，可以跳过该子动作并记录
`dependency_stale`；它不必推翻一个仍有效的主选择。主选择的 source 失效时，整个主选择
不能提交，并进入重判流程。

### 7.4 变化处理矩阵

| 判断期间发生的变化 | 当前 LLM 是否 Cancel | 完成后处理 | 是否重调 LLM |
|---|---|---|---|
| 新增普通 Agent Intent | 否 | 留到下一 Cohort；当前结果可提交 | 否 |
| 新增主持人 self Intent | 否 | 留到下一 Cohort | 否 |
| 新增普通 Human SpeechIntent | 否 | 留到下一 Cohort | 否 |
| 未被选择的 Intent refresh/withdraw | 否 | 当前主选择仍可提交 | 否 |
| 被选择的 Intent refresh/withdraw | 否 | 旧主选择不可提交 | 是 |
| 被选择 Intent 的作者失去资格 | 否 | 旧主选择不可提交 | 是或停止 |
| 被选择 Handoff 关闭、blocked 或 attempt 改变 | 否 | 旧主选择不可提交 | 是 |
| Human Floor Request 到达 | 否 | 旧结果丢弃，先服务 Human | 否；控制返回后新判断 |
| `speech_revision` 改变 | 否 | 旧结果丢弃 | 控制返回后新判断 |
| Control Token 转移 | 否 | 旧结果丢弃 | 否；等待控制返回 |
| Meeting End / moderator 被撤权 | 否 | 旧结果丢弃 | 否 |
| ACK、Progress、soft lease、outbox 状态 | 否 | 忽略 | 否 |
| 仅 `intent_revision` CAS 变化，source 仍有效 | 否 | rebase 并重提命令 | 否 |

Relay 当前 Handoff Select 还需要补齐 `blocked_by IS NULL` 的权威校验，不能只依赖 ACP
prompt 或本地过滤。

## 8. 重判与 deadline

### 8.1 3 分钟是单次有效 attempt 的最大窗口

Relay 继续以数据库时间维护 3 分钟 Moderator deadline。初次
`decision_attempt_start` 和一次有效的 `decision_retry` 都为即将 dispatch 的真实模型
attempt 取得完整窗口。以下事件不刷新 deadline：

- 新增普通 Intent；
- 未选中 Intent 的 refresh/withdraw；
- ACK、Progress、outbox 或历史同步；
- 格式修复；
- 单纯的 CAS rebase/re-submit。

若格式修复需要再次调用 LLM，它必须注册一个引用原 attempt 的 replacement provider
attempt，使用原 deadline 的剩余时间并计入总模型 attempt 上限；纯解析修复不需要。
格式问题永远不能刷新 canonical deadline。

只有同时满足以下条件时才允许 source-conflict retry：

1. 引用一个未消费、未过期的 Relay retry ticket；
2. ticket 对应的失败主 action 绑定一个已注册 `attempt_id`；
3. 输出的主 source 属于该 attempt 的权威快照；
4. 该 source 的快照版本与当前权威版本确实不同，或对象已经 withdraw/ineligible；
5. moderator 仍持有 Control Token；
6. Human priority 不活跃；
7. `database_now < current deadline`；
8. 当前 `decision_epoch` 的重判次数未超过上限。

“模型已经自然完成”由 ACP runtime terminal 与结构化日志证明；Relay 不伪装成 LLM
attestation。Relay 负责证明的是：attempt 真实注册过、source 属于它的快照、source
当前确实冲突，而且续期次数有界。

### 8.2 权威续期

续期不能只修改 ACP 本地 timer。使用现有 Moderator Action kind 增加
`decision_retry` operation，至少携带：

```text
attempt_id
retry_ticket_id
failed_action_event_id
expected_control_epoch
expected_decision_epoch
expected_attempt_number
conflict_source_type
conflict_source_id
observed_source_version
reason = source_refreshed | source_withdrawn | source_ineligible |
         handoff_changed
```

Retry ticket 由一次真正失败的主选择产生：

1. 模型形成 `select_intent | moderator_speak | select_handoff` 后，Harness 构造携带
   `attempt_id` 和快照 source version 的正常 Moderator Select；
2. 即使提交前 Full Sync 已经发现 source 冲突，也沿用乐观提交路径把该 action 交给
   Relay 校验；它不能创建 Offer，因为 source/version 或 CAS 已失效；
3. Relay 只有在确认该 action 引用本 attempt 的主 source，且 source 确实发生
   refresh/withdraw/ineligible/handoff change 时，才持久化一个一次性 retry ticket；
4. 仅全局 `intent_revision` 改变而 selected source 仍有效时不签发 ticket，ACP 只做
   CAS rebase；
5. `decision_retry` 必须引用该 ticket 和失败的 signed action event ID。

Ticket 绑定 Session、attempt、source、snapshot version、control/decision epoch 和当前
deadline，只能消费一次，且本身不形成 Offer/Grant。

Relay 从持久化的 AttemptSnapshot 和失败 action 读取该 source 的原始版本，不接受 ACP
临时声明一个没有被选中的 Cohort source。验证通过后，在 Session 行锁和数据库事务内：

- 把旧 attempt 标记为 `retry_required`；
- `decision_attempt += 1`，生成新的 `attempt_id`；
- 从同一权威 Cohort 重新读取当前有效 source 及版本；
- `moderator_decision_deadline = database_now + 3 minutes`；
- 保持 `decision_epoch` 不变；
- 写入 State effect `moderator_decision_retried`；
- 发布新的 canonical State。

新 AttemptSnapshot 的演化规则固定为：

- refreshed selected source：保留同一个稳定 ID，使用最新 `current_event_id`；
- withdrawn/ineligible selected source：从新快照删除；
- 其他原 Cohort source：读取当前有效状态和最新版本；
- `eligible_decision_epoch > current decision_epoch` 的 late source：继续排除。

若旧 Cohort 已无有效主候选，Relay 不启动一个空的 LLM retry，而是完成当前 Cohort；存在
next-epoch source 时进入下一 `decision_epoch`，否则回到 `moderator_idle`。

默认每个 `decision_epoch` 最多重判两次，即最多三个真实 LLM attempt。超过上限后使用
确定性 fallback。Intent fallback 只能从
`eligible_decision_epoch <= current decision_epoch` 的 pending Intent 中选择；open
Handoff 不得被 fallback 静默重放。当前 Cohort 已空时结束它并处理下一 Cohort。该上限
进入冻结的 BatonConfig，不能由 ACP 自行放宽。

### 8.3 deadline 与 retry 的竞态

Relay 对 retry/start 命令和 sweeper 使用同一个 Session 行锁，并在处理任何命令前按
数据库时间执行 lazy recovery：

- retry/start 在 `database_now < deadline` 时先取得锁：可以按规则刷新 deadline；
- sweeper 或任意 lazy recovery 在 `database_now >= deadline` 时先取得锁：fallback
  获胜，后到的 retry/start 被拒绝；
- 不存在“deadline 已过但因为 sweeper 还没跑，所以仍可续期”的灰区。

超过 retry 上限的请求在同一事务中立即触发 fallback，不继续等待旧 deadline。

deadline 到期后，正在运行的 LLM 不需要被 Cancel；它返回后因 `decision_epoch`、
control、deadline 或 source 前提不成立而被丢弃。

这意味着“模型不中断”不等于“会议必须等待模型”。会议的活性仍由权威 deadline 和
fallback 保证。

### 8.4 Handoff-only 窗口

现有语义中，只有 open Handoff、没有 deterministic Intent fallback 时可能处于
`moderator_idle + deadline=null`。新实现必须显式区分：

- open、unblocked Handoff 是 Agent moderator 的可判断工作，可以注册一个有 3 分钟
  attempt deadline 的 DecisionAttempt；
- 若当前没有活动 decision window，AttemptStart 原子递增 `decision_epoch`，让该
  Handoff 进入新 Cohort，但 phase 仍可保持 `moderator_idle`；
- Handoff deadline 到期时不能自动重放旧 Offer；Relay 把 attempt 标为 `timed_out`，
  Handoff 继续 open，并回到 moderator idle；
- Relay 在 Handoff 上保存 `moderator_retry_blocked_fingerprint` 和
  `moderator_retry_not_before`。同一个 source/version/attempt fingerprint 超时后，
  Controller 不得自动每 3 分钟重开 LLM；
- 只有 Handoff fingerprint 发生权威变化、新的普通 Cohort 同时需要主持判断，或者
  moderator/operator 显式提交一次有界 `retry_handoff`，才解除该抑制；
- selected Handoff 在提交和 retry 时必须权威校验 `blocked_by IS NULL`；
- Handoff-only attempt 不冒充具有 Intent fallback 的 `moderator_control` window。

共同 guard 中的 deadline 因此是“若本 attempt 存在权威 deadline，则必须未过期”，不是
要求所有 `moderator_idle` State 都预先带有 deadline。

### 8.5 CAS rebase 的合并与上限

`intent_revision` CAS 失败但 selected source 仍有效时不调 LLM。ACP 重新 Full Sync，
使用最新 revision 重建同一命令。

为避免持续 late Intent churn 形成热循环：

- 单次提交最多连续快速 rebase 三次；
- 三次后进入至少 250 ms 的 quiescence/coalescing window，再 Full Sync；
- 每个 attempt 的总 rebase 次数默认最多八次；
- 达到总上限后，以 `discarded(reason=cas_churn)` 终结 attempt，停止提交并等待当前
  deadline/fallback；当前 epoch 标记为 `llm_closed_waiting_fallback`，不得因此重调
  LLM 或刷新 deadline；
- rebase 始终受当前权威 deadline 约束；
- deadline 到期或 control/Human/source 前提变化时立即停止；
- rebase 次数进入结构化日志，但不计入 LLM attempt 次数，也不刷新 deadline。

### 8.6 Human 抢占后的新窗口

Human priority 使旧 attempt 逻辑失效，但不 Cancel 其 provider Turn。Human speech 完成
后 Relay 建立新的基础 control deadline；ACP 仍遵守单飞，等待旧 Turn 自然 terminal。

若旧 Turn 在新基础 deadline 前完成，ACP 为新 `decision_epoch` 提交
`decision_attempt_start`，Relay 把实际新模型 attempt 的 deadline 设置为
`database_now + 3 minutes`，因此模型获得完整窗口。若旧 Turn 直到基础 deadline 后仍未
完成，fallback 获胜，新 start 被拒绝；该 run 同时违反健康场景的 Moderator latency
目标，不能算作专项验收通过。

## 9. ACP Turn 生命周期

### 9.1 删除投机 Agenda

实现应删除或关闭：

- `V1ModeratorAgenda` 的 queue、prompt 和结果状态；
- `offered | granted` 阶段的 Moderator LLM dispatch；
- 因 Agenda fingerprint 变化产生的 stale/preemption；
- Agenda stale 后向 ACP 发送的 State-driven Cancel。

可以保留不调用模型的候选索引函数，供建立 Candidate Cohort 时复用。

### 9.2 不做 State-driven Cancel

Moderator Decision Turn 运行期间，Meeting State 变化只能：

- 更新最新权威投影；
- 记录潜在 conflict reason；
- 阻止旧结果直接提交；
- 在 Turn 自然完成后触发 Full Sync 和分类。

不得因此发送 `ControlSignal::Cancel` 或 ACP `session/cancel`。

物理 Cancel 只保留给：

- 操作者显式停止；
- buzz-acp 进程关闭；
- Runtime/transport 已不可继续使用；
- 独立的进程级 hard watchdog。

这些是运行时终止，不是会议状态收敛机制。

### 9.3 迟到结果 fencing

即使 ACP Cancel 不存在，所有模型结果仍必须经过：

1. Turn identity 与 Session identity 校验；
2. Full Sync；
3. common guard 校验；
4. source dependency 校验；
5. Relay CAS；
6. Relay 的最终授权、deadline、Offer/Grant 唯一性校验。

任何一步失败都不能产生 canonical Offer、Grant、Speech 或控制动作。

## 10. 数据、wire 与兼容调整

### 10.1 Relay/数据库

建议增加：

- `meeting_speech_intents.eligible_decision_epoch`；
- open Handoff 投影的 `eligible_decision_epoch`；
- `meeting_sessions.decision_attempt`、`active_decision_attempt_id` 和 attempt deadline；
- `meeting_moderator_decision_attempts`，保存 attempt identity、Cohort source/version
  snapshot、terminal state 和时间；
- 一次性 retry ticket 投影，绑定失败 action、attempt、source、epoch 和 deadline；
- BatonConfig 中的 `moderator_max_rejudgments`，默认 `2`；
- BatonConfig 中的 `moderator_max_cas_rebases_per_attempt`，默认 `8`；
- `decision_attempt_start`、`decision_attempt_finish`、`decision_retry`、
  `complete_cohort` 和 `decision_attempt_abandon` 的事务命令与 canonical effect；
- retry deadline、attempt 和 conflict reason 的审计记录。

kind `42103` State 增加 `active_decision_attempt` 可读投影及 snapshot hash；State history
必须能够按历史 State event ID 回读 attempt 启动时的完整 candidate refs。

`intent_revision` 继续在 Intent/Human Request 投影发生变化时递增，也继续作为 Select
提交的 CAS token。它不再直接决定 ACP 是否重调 LLM。

所有 moderator self priority、Deferral required-set、Select 和 fallback 查询都必须按
当前权威 Cohort 过滤。否则 late self Intent 仍会在 Relay 层推翻已完成的旧 Cohort 判断。

### 10.2 SDK、CLI 与 Relay handler

- 复用现有 Moderator Action kind，不增加 HTTP endpoint；
- SDK 增加严格的 attempt start、finish、retry、cohort complete 和 abandon builder；
- CLI 增加仅供 Agent/验收使用的对应操作；
- HTTP bridge/RestClient 增加 typed Accepted/Rejected/Uncertain 结果，保留稳定 rejection
  code、canonical object 和 retry ticket；
- Agent moderator 的模型驱动 Select、Reject、Dismiss、Deferral 和 self-withdraw 都必须
  携带 `attempt_id`；Intent/Handoff 主选择还携带 AttemptSnapshot 中的 source version；
- 模型驱动 `withdraw_self` 使用 attempt-bound Moderator Action，不能走不含 epoch/attempt
  的普通 Participant IntentWithdraw 路径；
- Relay 在 selected-source action 因 source dependency 冲突被拒绝时签发一次性 retry
  ticket；普通 CAS 冲突不签发；
- Relay 校验 actor 必须是冻结的 moderator，并验证 conflict evidence、epoch、attempt、
  deadline 和重判上限；
- Relay 的 Handoff Select 增加 `blocked_by IS NULL` 校验；
- retry、Select、Reject、Dismiss 继续在同一个 Session 锁和数据库事务边界内执行。

Human moderator 的直接操作不需要伪造 LLM attempt；Relay 根据冻结 roster 中的权威
participant type 区分 Human manual action 与 Agent model-driven action。Agent moderator
缺少有效 `attempt_id` 的旧命令必须 fail closed，防止迟到 Turn 绕过 fencing。

### 10.3 Typed protocol submit outcome

当前 Meeting 提交路径不能把 Relay 的确定性 CAS rejection 统一折叠为
`ProtocolSubmitFailure::Uncertain`。Stage 1 必须先改造 HTTP bridge 和 RestClient：

```text
ProtocolSubmitOutcome
- Accepted
    event_id
    canonical_object_id?
- Rejected
    event_id
    code
    canonical_object_id?
    retry_ticket_id?
- Uncertain
    transport_reason
```

要求：

- Relay 的非 2xx 或 `accepted=false` body 都带稳定 machine-readable `code`；
- RestClient 保留 HTTP status 和 response body，并解析为 `Rejected`；
- `stale_moderator_revision`、`selected_source_changed`、`human_request_has_priority`、
  `deadline_expired` 等确定性结果不能记为 uncertain；
- 只有请求是否到达 Relay 无法判断、连接在响应前断开等传输歧义才是 `Uncertain`；
- selected-source rejection 在同一响应中返回一次性 `retry_ticket_id`；
- ACP 根据 typed code 决定 rebase、retry、discard 或停止。

没有这层 typed outcome，乐观提交无法可靠区分“无关 CAS 冲突”和“selected source
失效”，专项真实验收也不能通过 `uncertain=0`。

### 10.4 ACP 私有账本

账本保存 CandidateCohort、DecisionAttempt、主选择依赖和 CAS rebase 次数。重启恢复时：

- 不恢复一个无法证明仍在运行的 provider Turn；
- Full Sync 后通过 `decision_attempt_abandon(reason=runtime_lost)` 把旧 `running` attempt
  权威标记为 `abandoned`；
- replacement start 计入同一个 epoch 的 attempt 上限，不能通过反复重启绕过；
- runtime-lost replacement 使用原 deadline 的剩余时间，不刷新完整 3 分钟；
- 若仍处于相同 control/decision epoch 且 deadline 未到，从同一权威 Cohort
  重新建立 attempt；
- late Intent 的 eligibility 由 Relay 持久化，因此重启不会把它插入旧 Cohort。

## 11. 可观测性

至少输出以下结构化事件：

```text
meeting_v1_moderator_decision_started
meeting_v1_moderator_decision_completed
meeting_v1_moderator_decision_validated
meeting_v1_moderator_decision_rebased
meeting_v1_moderator_decision_retry_requested
meeting_v1_moderator_decision_retry_started
meeting_v1_moderator_decision_committed
meeting_v1_moderator_decision_discarded
```

共同字段：

```text
session_id
turn_id
attempt_id
control_epoch
decision_epoch
attempt_number
speech_revision
snapshot_intent_revision
current_intent_revision
candidate_count
selected_source_type?
selected_source_id?
outcome
reason
model_latency_ms
```

不得记录 Intent 正文、私钥或完整工具输出。验收 artifact 可以记录 event ID 和 source ID，
但必须删除所有身份私钥。

另需对 ACP wire 增加计数：

- `session/cancel`，按 Turn kind 和 reason 分类；
- Prompt terminal outcome；
- `cancel_drain_timeout`；
- agent respawn/backoff；
- Meeting action `outcome=uncertain`；
- 子进程 PID、启动时间和退出原因。

只检查 `cancel_drain_timeout=0` 不足以证明没有 State-driven Cancel，因为 Cancel 也可能
成功 drain。

## 12. 确定性测试

实现至少补充：

1. Moderator 在 `offered | granted` 阶段不产生 LLM Turn；
2. late Agent Intent 不使运行中的 Decision stale，也不排队第二个 Turn；
3. late self Intent 在 ACP、Relay self-priority 查询和 fallback 中都不能绕过 Cohort；
4. 非 selected Intent refresh/withdraw 后，原主选择 rebase 成功且不重调模型；
5. selected Intent refresh/withdraw 后，旧结果不提交并产生一次 retry；
6. 只有 attempt-bound、selected-source 失败 action 获得一次性 retry ticket；
7. Human Floor Request 使旧结果失去提交资格，但不发送 Cancel；
8. CAS 在 Full Sync 后再次冲突时，只 rebase，不重调模型；
9. 多个无关 State 变化合并处理，不形成 retry storm；
10. AttemptStart/retry 使用数据库时间刷新 3 分钟，并受最大次数限制；
11. idle 且 Cohort 非空时等待 fallback，不能错误 complete/推进下一 Cohort；
12. deadline fallback 与迟到模型结果竞争时只有一个 canonical 结果；
13. End、撤权、控制权转移后迟到结果全部被 fencing；
14. Human 抢占后，旧 Turn 自然结束，新 AttemptStart 获得完整窗口；
15. Handoff-only timeout fingerprint 不自动重开，blocked Handoff 不能 Select；
16. ACP 重启后旧 attempt 变为 `abandoned`，replacement 计数且不刷新 deadline；
17. CAS rebase 的 burst、coalescing、总次数和 deadline 上限生效；
18. Agent moderator 缺少有效 `attempt_id` 的 Select/Reject/Dismiss/self-withdraw
    全部被拒绝；
19. `e + 1` eligibility 能被 next-window probe 发现并原子推进，不能永久 pending；
20. AttemptStart State 的完整 candidate refs/hash 可回读，ACP prompt 与其完全一致；
21. Relay 确定性 rejection 被 RestClient 解析为 typed `Rejected`，不能落入
    `Uncertain`。

已有 Meeting backend、Relay E2E、2 Human + 4 Agent 多轮测试和 outbox/recovery 门禁必须
全部继续通过。

## 13. 针对性真实 Codex 验收

### 13.1 验收目的

本验收不重新证明整个 Meeting V1 功能，而是专门证明：

- Moderator 不再在无控制权时投机判断；
- State 变化不会物理 Cancel Moderator Decision；
- late Agent Intent 不触发本轮重判；
- selected source 真正失效时才重判；
- Human priority 能覆盖旧结果而不依赖 Cancel；
- 修复后真实 `codex-acp` 不再出现 cancel-drain/respawn churn。

通用环境、模型证据、隔离、artifact 和 canonical 不变量继续遵循
[真实 Agent 验收与规模压测方案](./meeting-v1-live-acceptance-plan.md)。本节增加更严格的
Moderator 专项门槛；两者冲突时采用更严格者。

### 13.2 固定真实调用配置

每场使用当前协议容量内的拓扑：

| 角色 | 数量 | 配置 |
|---|---:|---|
| Moderator Agent | 1 | `gpt-5.6-sol[max]` |
| Participant Agent | 3 | `gpt-5.6-sol[high]` |
| Human Operator | 1 | 脚本化提交 Human Request/Speech |
| Observer | 1 | 独立订阅 `#h=<session_id>`，不消费竞争队列 |

所有 Agent 必须使用：

```text
buzz-acp -> @agentclientprotocol/codex-acp -> Codex
```

不得使用 fake ACP、预录输出或本地规则模型。Adapter、Codex CLI、仓库 commit、工作区
diff、模型目录和每个真实 ACP Session 的 model/effort 应写入 run manifest。模型配置
不匹配时 fail closed。

### 13.3 注入与证据工具

Runner 必须订阅结构化 Controller 事件，并保存脱敏 NDJSON。仅靠固定 sleep 制造竞态不
可作为 qualification 证据。

为稳定复现“模型已经选中、提交前候选发生变化”，验收构建提供一次性的
`PreSubmitAcceptanceBarrier`：

1. 真实模型结果已经解析，Moderator Select 已按生产 schema 签名；
2. 在 `RestClient.submit_event` 和 2 秒 `PROTOCOL_SUBMIT_TIMEOUT` 开始之前，Barrier
   通过本地 Unix socket 向 Runner 公布 Session、attempt、Candidate Cohort 和
   selected source ID；
3. Runner 根据场景让 selected source 或一个明确未被 selected 的 source 作者，通过
   独立正常连接提交 Refresh 或 Withdraw；
4. 等待 Observer 看到新的 Relay 签名 canonical State；
5. Runner 释放 Barrier；
6. buzz-acp 通过正常 NIP-98 `POST /events` 提交原 Select；
7. Relay 返回 typed CAS rejection；只有 selected source 失效时才同时返回 retry
   ticket，ACP 随后 Full Sync 并分类。

Barrier 只控制时序，不修改事件、不伪造模型输出、不代理 NIP-98 authority，也不占用
协议提交 timeout。它只在显式 acceptance build feature 下存在；生产构建没有 socket
监听或暂停分支。feature、Runner、放行时间和二进制 hash 必须进入 artifact。

R-MOD-03/04 还必须使用机器可判的场景获取规则，避免 Runner 因真实模型没有产出 Select
而无限等待：

- 使用生产 Moderator prompt；fixture 的会议目标和 Human 消息明确要求主持人从当前
  候选中选择一人，候选只包含可直接选择、互不冲突的有效 Intent；
- Barrier 只等待当前 `attempt_id`，最长等待到该 attempt 的 Relay 权威 deadline 或 ACP
  terminal，以先到者为准；
- 模型输出 idle、只有管理辅助动作、非法格式或在 deadline 前没有 parseable Select 时，
  该次记为 `INCONCLUSIVE(model_did_not_exercise_primary_select)`，不能记为 PASS；
- 每个场景/变体最多使用三个全新 Meeting 获取目标 Select；获取成功后才进入竞态注入，
  未命中的样本不计入要求的通过次数；
- 三次均未获取目标路径时，该 Tier 直接 FAIL，并保留三个样本的模型 terminal 和 prompt
  证据；Runner 不放宽 schema、不改写模型输出，也不无限新建会议。

同时记录 Moderator `codex-acp` 子进程 PID、进程启动时间和 process tree，证明测试期间
没有以 respawn 掩盖问题。

### 13.4 场景矩阵

#### R-MOD-01：无控制权时不判断

1. Participant A 获得 Grant，并通过真实 Codex 生成 speech；
2. A 发言期间，B、C 提交 Intent；
3. 保持 Grant 足够长以观察 Controller。

必须满足：

- `offered | granted` 期间 Moderator Decision dispatch 为 0；
- `meeting_v1_moderator_agenda_started = 0`；
- Control Token 返回后才出现 `moderator_decision_started`。

#### R-MOD-02：late Agent Intent 进入下一批次

1. Control Token 返回，B、C 已在首个 Cohort；
2. Observer 看到 `moderator_decision_started` 后，A 提交一个新的 Agent Intent；
3. Observer 必须在该 Turn 的 ACP terminal 之前看到 A 的 canonical Intent/State；若模型
   先返回，本次样本不计为通过，Runner 重新建立场景；
4. 等待首个真实 Moderator LLM 自然完成。

必须满足：

- A 的 Intent 不在首个 Cohort；
- A 的 `eligible_decision_epoch` 严格晚于首个 Cohort 的 epoch；
- 首个 Turn 不 Cancel、不 stale、不重调 LLM；
- 首个输出不得引用 A 的 Intent；
- A 的 Intent 保持 pending，且本轮 `selection_attempt_count = 0`；
- 下一 Candidate Cohort 必须包含 A；
- 即使 `intent_revision` 改变，只要首个 selected source 有效，ACP 使用最新 CAS token
  提交，不重调 LLM。

#### R-MOD-03：未选中对象变化不重判

1. 首个 Cohort 至少包含三个有效 Intent；
2. PreSubmitAcceptanceBarrier 暂停真实模型产生的 Moderator Select，并公开实际
   selected source；
3. Runner 从同一 Cohort 选择另一个明确未被 selected 的 Intent，让其作者 Refresh
   或 Withdraw；
4. canonical State 可见后放行旧 Select。

必须满足：

- 模型自然完成；
- Relay 可以先因全局 `intent_revision` CAS 变化拒绝旧 Select，但不得签发 retry
  ticket；
- ACP Full Sync 后确认 selected source 仍有效，只重建并重提同一个 Select；
- 未选中 source 的变化不产生新的模型 attempt；
- 原 selected source 最终可以形成 Offer；
- 若输出包含针对已失效对象的辅助动作，只跳过该动作并记录
  `dependency_stale`。

#### R-MOD-04：selected source 失效后恰好重判一次

1. 建立至少包含三个可选 Intent 的 Cohort，确保 selected source 被撤回后仍有可判断
   候选；
2. PreSubmitAcceptanceBarrier 暂停真实模型产生的 Moderator Select；
3. 根据 Select 中的 source ID，让作者 Refresh 或 Withdraw；
4. canonical State 可见后放行旧 Select。

必须满足：

- Relay 拒绝旧 Select，且不创建 Offer；
- Relay 为该 failed action 签发一个绑定 attempt/source 的一次性 retry ticket；
- ACP Full Sync 后确认 selected source 失效；
- `decision_retry` 成功消费 ticket，重复消费被拒绝；
- `decision_attempt` 只增加 1；
- 新 deadline 为 Relay 接受 retry 的数据库时间加 3 分钟；
- 第二次 LLM 在第一次已经自然 terminal 后才启动；
- 第二次 prompt 不再使用被撤回版本；
- 全程 `session/cancel = 0`。

Refresh 与 Withdraw 各独立执行一次，不能用一个场景替代另一个。

#### R-MOD-05：Human Floor Request 覆盖旧结果

1. Runner 收到 `moderator_decision_started`，确认真实 provider Turn 已 dispatch 且尚无
   ACP terminal；
2. Human Operator 立即提交 Floor Request；
3. Observer 必须在原 Turn 的 ACP terminal 之前看到 canonical Human Request/State；
   若模型先返回，本次样本不计为通过，Runner 重新建立场景；
4. Human 获得下一次发言权并完成 speech；
5. 原 Moderator LLM 继续自然运行到 terminal。

必须满足：

- Human Request 不等待 Moderator LLM；
- 旧 Moderator 结果不产生任何 canonical Moderator action；
- 旧 Turn 不 Cancel、不 respawn；
- 旧 attempt 自然完成后以 `discarded(reason=human_priority)` 权威终结；
- Human speech 完成、Control Token 再次回到 Moderator 后，才启动新的 Decision；
- 新 Decision 使用新的 control/decision/speech 快照和完整 3 分钟窗口。

#### R-MOD-06：状态突发只收敛一次

在一个运行中的 Moderator Decision 内制造：

- 多个 late Agent Intent Submit/Refresh；
- 未 selected Intent Withdraw；
- 同一 canonical State 的延迟投递、重复投递和 backfill 重放；
- 一次 selected source 失效。

必须满足：

- 第一个 Turn 自然完成；
- 所有无关变化不产生额外模型调用；
- selected source 冲突最终只合并成一次重判；
- 不出现 `running` Moderator Turn 重叠；
- 不出现热循环或 deadline 被无关变化反复刷新；
- 重复/回填的观察事件不能被误认为新的 canonical 变化。

#### R-MOD-07：C12 并发回归

并行启动三场会议，每场 `1 Moderator Agent + 3 Participant Agent + 1 Human`，合计 12 个
真实 Codex Agent。通过 barrier 让三场 Moderator Decision 同时在途，并分别重复
R-MOD-02、R-MOD-04 和 R-MOD-05 的注入。

必须满足：

- 三场会议全部完成并 End；
- 单场和跨场均无双 Offer、双 Grant、revision 缺口或历史分叉；
- 所有 Moderator Turn 都自然 terminal；
- ACP 子进程身份全程连续；
- 无 Cancel、respawn、uncertain 或 outbox 遗留。

### 13.5 执行级别

分两级执行：

1. **Qualification**
   - R-MOD-01 至 R-MOD-06 各通过一次；
   - R-MOD-04 的 Refresh 与 Withdraw 变体分别通过；
   - R-MOD-07 通过一次；
2. **正式签收**
   - R-MOD-01 至 R-MOD-06 各连续通过三次；
   - C12 并发回归连续通过三次；
   - 在同一配置下运行 60 分钟 churn soak；
   - 任一硬门槛失败即停止后续 Tier，修复后从失败 Tier 重新开始。

R-MOD-03/04 的场景获取 miss 不计入“通过一次”或“连续通过三次”，但仍受第 13.3 节每个
场景/变体最多三个新 Meeting 的上限约束。超过上限按 Tier FAIL，不得改记为跳过。

单次 qualification 只允许证明实现方向成立，不能单独恢复 production go。

### 13.6 硬门槛

每个 run 必须同时满足：

- `meeting_v1_moderator_agenda_started = 0`；
- Moderator Decision 在 `offered | granted` 阶段 dispatch 数量为 0；
- 每次真实 Moderator LLM dispatch 前恰好有一个 Relay 接受的 AttemptStart/Retry，
  `attempt_id`、epoch、快照和 deadline 完全匹配；
- 每个 Moderator prompt 的 candidate snapshot hash 与 Relay 签名 State 完全一致；
- Agent moderator 形成的 canonical action 缺少/错配 `attempt_id` 的数量为 0；
- 每个 `turn_id` 恰好一次 dispatch、一次正常 ACP terminal，以及一次明确的
  committed/discarded/retry-required 归宿；
- Moderator Decision/Agenda 的 State-driven ACP `session/cancel = 0`；
- Moderator Decision/Agenda 的 `PromptOutcome::Cancelled = 0`；
- `cancel_drain_timeout = 0`；
- Moderator Decision/Agenda 的 `agent_returned(cancelled) = 0`；
- `agent_returned — respawning* = 0`；
- respawn complete/backoff = 0；
- Meeting action `outcome=uncertain = 0`；
- 所有预期 CAS/source rejection 都解析为 typed `Rejected`，不得以 transport error
  掩盖；
- Moderator `codex-acp` PID 和启动时间不变；
- late Agent Intent 导致的 LLM 重判为 0；
- late moderator self Intent、Relay self priority 和 fallback 绕过 Cohort 的数量为 0；
- selected-source 冲突场景的重判次数与预期完全一致；
- retry ticket 非 selected-source 签发、重复消费或跨 attempt 消费的数量为 0；
- 必需的 Select 路径在场景获取上限内未被覆盖的数量为 0；
- Human 抢占后的新健康 Attempt 获得完整 3 分钟 deadline；
- 迟到或冲突结果形成 canonical Offer/Grant/控制事件的数量为 0；
- 双 Offer、双 Grant、非 holder speech、revision 缺口、历史分叉为 0；
- pending/error outbox 为 0；
- 所有会议可 End，End 后无新 canonical control/speech；
- 未授权项目、Git、Buzz、MCP 或 HTTP 写入为 0。

旧 C6 报告中属于 Moderator Decision/Agenda 的“预期取消”在新设计下不再允许。即使
Cancel 成功 drain，没有 `cancel_drain_timeout`，仍应判专项验收失败。Participant
Intent 与 Granted Speech 的物理抢占不在本文变更范围内，必须按 Turn kind 分开统计。

### 13.7 Artifact

每次 run 至少保存：

- 脱敏 manifest；
- Runner、acceptance Barrier、Buzz 二进制和工作区 diff 的 SHA-256；
- Relay、ACP、adapter 和 Observer NDJSON；
- ACP wire 中 `session/cancel` 计数；
- process-tree NDJSON；
- Decision/Cohort/attempt/rebase/retry 时间线；
- canonical State、Offer、Grant、Speech 和 action 时间线；
- DB invariant 查询结果；
- provider/model/effort 应用证据；
- 每个硬门槛的机器可读 PASS/FAIL。

失败 run 保留数据库和脱敏日志用于诊断；成功 run 默认删除私钥、数据库和临时 Redis，
只保留脱敏 artifact。

## 14. 分阶段交付

### 阶段一：权威 retry 与提交语义

- 持久化 Cohort eligibility 和 AttemptSnapshot；
- 实现 current-Cohort 与 next-window 两套查询及原子 epoch 推进；
- 让 kind `42103` State 可回读完整 AttemptSnapshot refs/hash；
- 先完成 HTTP bridge/RestClient typed protocol outcome，消除确定性拒绝的 uncertain；
- 扩展 Agent moderator action 的 attempt binding，并实现一次性 retry ticket；
- 增加 `decision_attempt_start`、`decision_attempt_finish`、有界
  `decision_retry`、`complete_cohort`、`decision_attempt_abandon` 和数据库时间续期；
- 保留 Relay CAS，补 source conflict evidence；
- 让 self priority、Deferral、Select 和 fallback 遵守 Cohort，并补 Handoff blocked
  校验；
- 完成数据库、SDK、Relay handler 和恢复测试。

交付标准：selected-source 冲突可以在不放宽协议校验的前提下获得一次可验证的新
attempt；late self、fallback 和无关 Intent 变化都不能绕过 Cohort 或续期。

### 阶段二：ACP 单飞与 Candidate Cohort

- 移除 `ModeratorAgenda`；
- 只在 Moderator 持有 Control Token 时 dispatch；
- 实现 Cohort、Full Sync、semantic validation、CAS rebase 和 late-result fencing；
- 移除 Moderator Decision 的 State-driven Cancel。

交付标准：确定性 Controller 测试完整覆盖第 12 节。

### 阶段三：可观测性与验收 Runner（已交付）

- 增加 Decision/Cohort/attempt 结构化事件；
- 捕获 ACP wire Cancel、Prompt terminal、子进程身份和 rebase/retry；
- 实现 submit timeout 之前的一次性 PreSubmitAcceptanceBarrier 和场景编排。

交付标准：每项专项硬门槛都能由 artifact 自动判定，不依赖人工读日志猜测。

实现边界：

- `meeting-v1-acceptance` Cargo feature 才编译本地脱敏 NDJSON sink 和 Unix socket
  Barrier；普通 production build 不含暂停分支；
- Barrier 只暂停已经签名的 primary Moderator action，且发生在正常
  `PROTOCOL_SUBMIT_TIMEOUT` 启动之前；等待上限仍是 Relay 权威 attempt deadline；
- `scripts/meeting-v1-moderator-gates.jq` 根据结构化事件判断 model/Prompt 顺序、
  Attempt/Cohort 对齐、自然 terminal、Cancel/respawn/uncertain 和结果归宿；
- `scripts/meeting-v1-moderator-gates-test.sh` 用 PASS、Cancel 和缺失 attempt binding
  fixture 验证硬门禁自身不会静默放过反例；
- `scripts/meeting-v1-live-acceptance.sh` 编排单个 R-MOD 场景，
  `scripts/meeting-v1-moderator-acceptance.sh` 对场景获取实施最多三个全新 Meeting 的
  上限并可顺序执行 qualification 矩阵。

这里的“已交付”只表示验收能力已经可执行且通过本地测试，不表示 R-MOD 真实 Codex
结果已经通过；真实运行和签收仍属于阶段四。

### 阶段四：真实 Codex qualification 与正式签收

- 使用 `gpt-5.6-sol[max]` Moderator 和 `gpt-5.6-sol[high]` Participants；
- 先执行 R-MOD-01 至 R-MOD-07 qualification；
- qualification 全部通过后执行三次重复和 60 分钟 soak；
- 更新真实验收报告并给出 production go/no-go。

交付标准：第 13.6 节硬门槛全部通过；任何恢复或扩容决定只依据正式签收，不依据单个
成功样本。
