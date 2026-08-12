# Meeting V1 决策变更记录

本文记录在概念设计和后端实现设计冻结后，经讨论确认的语义调整。新决策优先于旧文档中
与其冲突的描述；实现文档应同步更新为当前语义。

## 2026-07-31：Moderator Turn 的 terminal disposition 必须一次性

### 状态

已修复并纳入真实 Codex C12 硬门禁；由最终规模验收发现。

### 问题

一次主持判断自然结束后，若主 action 与并发控制权变化冲突，ACP 会正确把结果标记为
`discarded` 并继续提交 DecisionAttempt Finish。后续 Full Sync/准备下一 Attempt 的竞态
可能再次走到同一 `mark_moderator_result_stale`，为同一个 `turn_id + attempt_id` 重复发出
`meeting_v1_moderator_decision_discarded`。

该样本的 Relay 协议投影仍然一致（Offer 全部 ACK、Grant 全部 spoken、outbox 无积压），
但一个主持 Turn 出现两个终结 disposition，会破坏可观测性、统计和恢复判断，因此必须是
qualification 硬失败，不能在 Runner 中去重掩盖。

### 新决策

1. 每个 Moderator Turn 只能 claim 一次 terminal disposition：
   `committed`、`discarded` 或 `retry_required`；
2. claim 状态写入 durable `ModeratorDecisionRecord`，重启恢复时保留；
3. Meeting ledger 已终止清理后到达的晚结果由有界的 4096-turn 本地 fence 去重，避免为了
   可观测性无限增长内存；
4. 重复 reconcile 仍可完成 Attempt Finish、Full Sync 和其他协议清理，但不得再次发出
   terminal disposition；
5. Runner 的 `moderator_turn_has_exactly_one_disposition` 门禁保持不变。

### 验证

- 新回归测试复现第一次 `control_changed` discard、ledger 状态随后变化、再次 stale
  reconcile 的顺序，并证明只产生一个 discard，且首次 reason 不被覆盖；
- 全部 Meeting V1 controller 测试 59/59 通过；
- 发现该问题的 C12 样本保留为失败证据，不计入后续三次连续通过。

## 2026-07-31：Moderator Attempt schema 必须同时覆盖迁移与 fresh install

### 状态

已修复并纳入后端权威门禁；由阶段四最终复跑的空数据库 Relay E2E 发现。

### 问题

迁移 `0042_meeting_v1_moderator_attempts.sql` 已完整定义主持人乐观判断所需的配置列、
Candidate eligibility、DecisionAttempt、RetryTicket 和外键，但仓库的 desired-state
`../../../../schema/schema.sql` 没有同步。brownfield 数据库通过 migration 升级后可以工作，直接应用
desired schema 的 fresh install 则会在 V1 Create 时因
`meeting_baton_config.moderator_max_rejudgments` 不存在而返回 HTTP 500。

### 新决策

1. migration 与 `../../../../schema/schema.sql` 都是受支持的安装路径，任何新增持久化协议状态都必须
   同步维护；
2. desired schema 现已包含 migration 0031 的全部最终结构，而不是只补发生错误的单个列；
3. `buzz-db` 增加静态回归测试，固定关键列、Attempt/RetryTicket 表和循环外键必须出现在
   desired schema；
4. `just test-meeting-backend` 继续从空数据库应用 `../../../../schema/schema.sql` 并运行 V0/V1
   Relay E2E，作为实际可执行的 fresh-install 门禁；
5. Postgres Meeting 单测共享隔离数据库时，测试 sweeper 必须扫完该测试进程创建的 due
   Session，并按目标 Session 的终态断言，不能让前序测试占用小批次后误报失败。生产
   sweeper 的 batch limit 与行为不变。

### 验证

- 全新数据库成功创建 68 张表，并包含
  `meeting_moderator_decision_attempts`、`meeting_moderator_retry_tickets` 及新增列/外键；
- 同一空库按顺序运行 V0、V1 lifecycle E2E，2/2 通过；
- 完整 `just test-meeting-backend` 通过：ACP 685、Relay Meeting 27、Postgres Meeting
  50，以及全部 Meeting lifecycle/floor/baton/rollout/revocation E2E 均无失败。

## 2026-07-31：Agent Offer 验收区分可受理 ACK 与可恢复容量 Decline

### 状态

已确认并纳入真实 Codex 规模验收门禁；由 C10 qualification 的主持判断与 Human 定向
Handoff 并发样本发现。

### 问题

Meeting V1 已明确规定 Moderator Decision 不因会议状态变化被物理取消。若 Human 在主持
Agent 正进行不可中断判断时发言，并把下一轮定向交给同一个 Agent，新的 Offer 会与该
Runtime 当前占用的物理 turn slot 发生短暂冲突。

ACP 按既有容量策略确定性 Decline，Relay 接受签名 Decline，并把原 Handoff 保持为 open；
主持判断自然结束后，主持人可以重新选择该 Handoff，目标随后 ACK、获得 Grant 并完成
回答。这是非抢占语义的正常恢复路径，不是 ACK 提交失败。

原规模 Runner 把所有 `state <> acked` 的 Agent Offer 都判为硬失败，因此会把上述完整恢复
误报为协议故障。

### 新决策

1. 可受理的 Agent Offer 仍必须不调用 LLM，并在默认 5 秒窗口内 ACK；
2. 当 Agent Runtime 正被不可中断主持判断或已经保留的 Agent turn 占用时，可以确定性
   Decline，不能为了 ACK 抢占或取消正在运行的主持判断；
3. 可恢复 Decline 必须同时满足：
   - Decline 事件已被 Relay 接受，并持久化 `response_event_id`；
   - `response_reason` 只能是 Harness 定义的受控容量原因；
   - 原 SpeechIntent 最终进入 `consumed`，或原 Directed Handoff 最终进入 `answered`；
4. Offer timeout、未确认提交、任意其他 Decline 原因，以及源对象未完成，仍是
   qualification 硬失败；
5. Runner 必须单独记录 `agent_offer_declines` 和
   `recovered_agent_offer_declines`，并保存逐 Offer 的状态、原因与源对象终态证据；
6. 该规则不改变 Relay 的唯一 Offer/Grant、deadline 或 holder 校验，也不允许自动重放
   Decline 的旧 Offer。后续机会仍由新的 canonical 主持选择形成。

### 验证

C10 真实 Codex 样本观察到一次上述竞态：第一次 Offer 被目标主持 Agent 显式 Decline，
原 Handoff 保持有效；主持判断自然结束后，该 Handoff 被重新选择并回答。修正后的门禁
得到 `decline=1 / recovered=1 / failed=0`，同时 C10 的 10 个 Agent 全部完成两次
canonical speech，协议、运行时与结构化门禁均为 0。

## 2026-07-30：主持人判断改为候选批次上的乐观并发

### 状态

已确认。阶段一“权威 retry 与提交语义”、阶段二“ACP 单飞与 Candidate Cohort”和
阶段三“可观测性与验收 Runner”均已实现并通过本地确定性验收；针对性真实 Codex
qualification 与正式签收仍按设计文档阶段四推进。

### 问题

原设计把主持 Agent 的工作拆成发言权不在主持人时运行的 `ModeratorAgenda`，以及发言权
回到主持人后运行的 `ControlDecision`。同时，ACP 使用完整 Intent/Handoff fingerprint
判断结果是否过时，并可能在会议 State 变化时向正在运行的 ACP Turn 发送 Cancel。

这带来三个问题：

1. 当前 speaker 的最终发言尚未出现时，提前完成的完整主持判断天然容易过时；
2. 晚到的普通 Agent Intent 会改变 `intent_revision` 或完整 fingerprint，但它完全可以
   留到下一轮，不应该迫使本轮重新调用 LLM；
3. 真实 C10/C12 验收已经证明，快速的 State 变化、Cancel、重新建 Session 和再次 Cancel
   会在真实 `codex-acp` 下产生 `cancel_drain_timeout`、子进程 respawn 和不确定提交。

### 新决策

1. 只有 Control Token 已经回到主持人，且 canonical phase 为
   `moderator_control | moderator_idle` 时，才启动完整的主持人 LLM 判断；
2. 取消投机性的 `ModeratorAgenda` LLM Turn。发言权不在主持人时只同步会议消息、Intent、
   Human Request 和 Handoff，不提前形成可执行的传递判断；
3. 每次判断开始时冻结一个本轮候选批次。判断期间新增的普通 Agent Intent 不进入本轮，
   不使本轮结果失效，也不触发重判；它在下一候选批次中处理；
4. 主持人自己的晚到 SpeechIntent 同样按普通 Agent Intent 处理。主持人 self Intent 的
   优先级只在同一候选批次内生效；
5. Human Floor Request 不属于普通 Intent 批次。它仍具有协议优先级，可以使旧判断失去
   提交资格，但不会物理取消正在运行的 LLM；
6. 普通 Human SpeechIntent 若被保留，采用和普通 Agent Intent 相同的批次规则。Human
   需要直接取得下一轮发言权时必须使用 Human Floor Request；
7. LLM 判断期间不因 Meeting State 变化发送 `session/cancel`。模型自然完成后，ACP 先
   Full Sync，再按该输出实际依赖的对象进行乐观校验；
8. 被选中的 Intent/Handoff 已撤回、刷新、失效或失去资格时才需要重新调用 LLM。未选中
   Intent 的新增、刷新或撤回，以及 Progress、ACK、outbox 等无关变化，都不触发重判；
9. `intent_revision` 继续作为 Relay 提交时的低层 CAS token。CAS 冲突后，若 Full Sync
   证明被选 source 仍有效，ACP 只用最新 revision 重建并重提命令，不重新调用 LLM；
10. 若 source 冲突确实要求重判，且主持人仍持有控制权，Relay 为新 attempt 刷新完整的
    3 分钟判断期限；若 Human 已接管、控制权已转移或会议已结束，则丢弃旧结果，等待新的
    canonical 控制窗口，不立即重判；
11. 物理 Cancel 只保留给进程关闭、操作者终止或 Runtime 故障处理，不再用它维持主持
    判断的协议正确性。正确性由 Relay CAS、source 校验和迟到结果 fencing 保证。
12. Candidate Cohort 的 eligibility 必须由 Relay/数据库持久化；moderator self priority、
    Deferral、Select 和 deterministic fallback 都要遵守同一 Cohort，不能只依赖 ACP
    私有快照；
13. 每次真实 Moderator LLM 调用前先注册权威 DecisionAttempt。初次判断、selected-source
    重判和 Human 抢占后的新判断通过有界 AttemptStart/Retry 取得完整 3 分钟窗口；若基础
    deadline 已被 fallback 消费，则不得事后续期。
14. selected-source 重判必须引用 Relay 对 attempt-bound 失败主 action 签发的一次性
    retry ticket；普通 `intent_revision` CAS 冲突不签发 ticket，只允许有界 rebase。
15. AttemptSnapshot 必须通过 Relay 签名 State 可回读；HTTP bridge/RestClient 必须把
    Relay 确定性拒绝解析为 typed rejection，不能统一降级为 `outcome=uncertain`。

### 保证边界

- “状态变化不打断”只表示 Moderator Decision Turn 不因会议语义变化被物理 Cancel；
  deadline 到期后 Relay 仍可执行确定性 fallback，迟到的模型结果必须被丢弃；
- 晚到 Agent Intent 不影响当前判断，但不会丢失。当前 speaker 完成、当前批次结束或新的
  主持控制窗口建立后，它必须进入下一候选批次；
- 低层 CAS 仍保护 Full Sync 与协议命令提交之间的竞态。CAS 冲突不等于语义冲突；
- 重判期限刷新必须由 Relay 在 Session 锁和数据库事务内完成，不能只延长 ACP 的本地
  timer；刷新次数必须有上限，避免恶意 refresh/withdraw 无限延长会议。

详细设计和针对性真实验收见
[`meeting-v1-moderator-optimistic-decision-design.md`](meeting-v1-moderator-optimistic-decision-design.md)。

## 2026-07-29：真实 Codex 验收采用 qualification 与正式签收两级口径

### 状态

已确认并执行。真实 Codex 和 canonical 协议闭环得到证明；C10/C12 的 ACP runtime
stability gate 未通过。

### 决策

1. 真实调用链固定为 `buzz-acp -> @agentclientprotocol/codex-acp -> Codex`；
2. Moderator 请求 `gpt-5.6-sol[max]`，其他 Agent 请求
   `gpt-5.6-sol[high]`，每个 Meeting Session 都必须在发言前记录成功应用；
3. model catalog、adapter 应用日志和真实 prompt 共同证明本轮确实使用 Codex；证据口径
   记为 `requested_catalog_supported_and_adapter_session_log`，不冒充 provider
   attestation；
4. C6/C10/C12 指当前协议下的跨 Meeting 总 Agent 数，不代表单场 6/10/12 Agent；
5. 单个 qualification 样本通过只允许继续验收，不能签收发布。正式签收仍需每 Tier 三次
   独立重复、故障矩阵、C12 60 分钟 soak 和人工质量评分；
6. 真实 Agent 使用 prompt 约定只读。worktree 零变化是本轮副作用审计结果，不是工具层
   安全隔离证明；
7. `agent_returned — respawning` 和 Meeting action `outcome=uncertain` 属于
   qualification 硬失败，不能只因 Meeting 最终恢复并 End 就忽略。

详细数据见
[`meeting-v1-live-acceptance-report-2026-07-29.md`](meeting-v1-live-acceptance-report-2026-07-29.md)。

## 2026-07-29：Human priority 结束时恢复被延迟的 Directed Handoff

### 状态

已确认并交付；由真实 Codex C4 验收发现。

### 问题

Agent 持有 Grant 发言时，Human Request 可以异步排队。若该次 Agent speech 同时创建
Directed Handoff，Relay 会正确保存 open Handoff，并以
`blocked_by=human_request` 阻止它抢在 Human 之前自动获得 Offer。

原实现没有在 Human priority 结束后清除这个瞬时阻塞。Handoff 虽仍为 open，但主持
Agent 会合理地把 `blocked_by=human_request` 理解为当前仍不可调度，从而可能保持 idle
直到 3 分钟 moderator fallback；fallback 又只处理 Intent，不处理 Handoff，会议会失去
连贯性。

### 新决策

1. `blocked_by=human_request` 表示当前 Human priority 屏障，不是永久处置；
2. 当最后一个 queued/offered Human Request 终结并把控制权归还 moderator 时，Relay 在
   同一个 Session 锁和数据库事务中清除相关 open Handoff 的 `blocked_by`；
3. 首次被 Human 延迟的事实继续由 `initial_disposition=blocked` 保存；
4. 同一 canonical State 写入 `handoff_unblocked` effect，使用
   `from=human_request, to=null`；
5. 主持人随后可以立即 Select 或 Dismiss 该 Handoff；Relay 不自动替主持人重放旧
   Handoff Offer。

### 验证

Postgres 回归测试覆盖：

- Agent Grant 期间 Human Request 入队；
- Agent speech 创建被 Human priority 延迟的 Directed Handoff；
- Human 撤回当前 Offer；
- Relay 原子归还主持人控制权并发布 `handoff_unblocked`；
- 主持人随后成功 Select 该 Handoff。

## 2026-07-29：Meeting Turn 工具策略改为 advisory

### 状态

已确认，纳入 Stage 3 交付。

### 原设计

Meeting Turn 强制使用 Agent 的 `Plan` permission mode，只向 Agent 暴露带
`BUZZ_DEV_MCP_READ_ONLY=1` 的 `buzz-dev-mcp`。Harness 要求目标 ACP Agent 明确支持并
成功切换到 Plan；否则 Meeting session 创建失败。

该方案试图把“会议只做讨论，不执行任务”实现为代码级只读边界。

### 变更原因

会议发言可能需要从任务、工作流、项目状态、Buzz CLI、第三方 MCP、HTTP 或 Agent 原生
工具中获取上下文。只允许一个专用只读 MCP 会显著限制 Agent 的调查能力。

此外，Plan 和 Agent 原生工具权限属于具体 ACP Runtime 的实现语义。Buzz 的 MCP 配置只能
约束经该 MCP 发起的调用，无法统一约束 Codex 等 Runtime 自带的文件、Shell、网络和其他
工具。对不支持 Buzz 专用权限策略的 Runtime 强行建立通用硬限制，会造成兼容性失败，也
不能形成所宣称的完整安全边界。

### 新决策

Meeting V1 初版采用 **advisory 工具策略**：

1. Meeting Turn 不再强制切换 Plan mode；
2. 不再要求 ACP Agent 支持特定 permission mode；
3. Meeting Turn 继承该 Agent 的正常 MCP、CLI、HTTP 和原生工具能力；
4. Meeting system prompt 明确要求工具只用于获取发言所需证据，不执行任务，不产生持久
   写操作或会议外部副作用；
5. 如果发现需要执行的事项，Agent 应把它作为结论、问题或后续行动建议写入发言，而不是
   在 Meeting Turn 中直接执行；
6. Meeting prompt 要求 Agent 不得通过工具自行发布 Meeting speech 或控制事件。Harness
   管理的自动路径仍只根据结构化模型结果构造、签名并提交 Intent、ACK、Progress、SAY、
   YIELD 和 Handoff；
7. 工具输出、会议内容和项目内容继续按不可信证据处理，不能覆盖 Meeting system prompt、
   Grant、deadline 或输出 schema。

该变更只适用于 Meeting V1。Meeting V0 保留原有的强制 Plan mode 和
`BUZZ_DEV_MCP_READ_ONLY=1` 行为；ACP Harness 根据 turn 的协议版本选择独立运行上下文，
避免 V1 的工具策略隐式改变 V0。

### 保证边界

本次变更只放宽 Agent 的上下文获取能力，不放宽 Meeting 协议。

Relay 与 Harness 仍硬性保证：

- 同一时间至多一个有效 Offer/Grant；
- 只有当前 Grant holder 可以消费 Grant；
- revision、epoch、deadline、名单、mention 和 Handoff 目标必须有效；
- Agent 原始输出不能直接发布，只有通过严格 schema 和最新权威 State 校验的结果才能
  进入 Harness 管理的自动发布路径；
- Relay 对来自 Harness、CLI 或其他客户端的协议事件执行相同的授权、revision、Grant 和
  deadline 校验；
- 迟到、过期、重复或格式错误的结果不会形成有效 speech。

Meeting V1 初版**不保证**模型在受到提示注入或行为失控时无法通过自身工具产生副作用。
“只调查、不写入”是参会 Agent 的行为约定，不是 OS、Runtime 或 MCP 层的安全隔离。
同理，若普通工具面包含带参会身份凭据的 Buzz CLI，V1 也不保证模型无法绕过 Harness
自动路径尝试提交协议事件；prompt 负责禁止这种行为，Relay 负责拒绝不满足协议条件的事件。

### 后续方向

如果后续需要代码级工具限制，可以设计 MCP Gateway、受信 Agent Runtime 的逐调用权限
策略或独立沙箱。其中 MCP Gateway 只能覆盖 MCP 调用；对于 Codex 等 Runtime 的原生工具，
还必须结合 Runtime 权限策略或进程级隔离，不能把 Gateway 单独描述为完整方案。
