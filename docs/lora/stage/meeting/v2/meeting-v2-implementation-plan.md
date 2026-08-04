# Meeting V2 后端分阶段实现计划

> 状态：阶段一至阶段五已完成；Meeting V2 后端发布候选已形成
>
> 产品语义基线：
> [Meeting V2：主持人维护的共享会议看板](./meeting-v2.md)
>
> 复用基线：
> [Meeting V1 后端实现设计](../v1/meeting-v1-backend-implementation-design.md)
>
> 本文只规划后端关键实现点、阶段交付物和验收门槛。事件 kind、完整 wire JSON、SQL
> 字段、函数签名、Prompt 全文、超时数值和 PR 拆分，在进入对应开发阶段时单独讨论。
>
> 本文不规划 Desktop、Web 或 Mobile。前端将在后端契约稳定后单独设计和开发。

## 1. 计划目的

本计划把 Meeting V2 后端拆成可以独立开发、审查和验收的阶段，在不破坏 Meeting V0/V1
的前提下交付：

- 一份会议范围内、由主持人单写、固定参会者可按需读取的当前看板；
- 创建者即主持人的 V2 创建语义；
- Relay 权威的 `Board Maintenance → Floor Decision` 主持控制周期；
- 相互独立的看板处理和发言权决策时间预算；
- Agent 在 Intent、Granted Speech、Board Maintenance 和 Floor Decision 节点按需取得
  当前看板；
- 主持 Agent 的独立看板维护 Turn；
- 正常 `closed` 与异常 `aborted` 的终态分类；
- CLI-only、混合 Human/Agent 和全 Agent 的后端验收路径；
- 可恢复、可观测、可灰度和可回滚的发布边界。

本文不提前完成阶段级实现设计。每个阶段开始前，只冻结该阶段所必需的协议、数据、并发和
测试细节。

## 2. 后端范围

### 2.1 纳入本计划

本计划覆盖：

- `buzz-core` 的协议 kind 注册和分类；
- `buzz-sdk` 的类型、builder 和兼容 fixture；
- `buzz-db` 的当前看板、V2 协调状态、终态分类和迁移；
- `buzz-relay` 的协议路由、权限校验、事务、恢复和灰度开关；
- `buzz-cli` 的 Agent-first 与 CLI-only 验收操作；
- `buzz-acp` 的按需看板读取、普通参会 Agent 和主持 Agent；
- Meeting outbox、sweeper、lazy recovery 和安全撤权的必要扩展；
- 协议、数据库、Relay、CLI、ACP、迁移和真实 Agent 验收；
- 低基数指标、日志、发布、故障处置和回滚说明。

### 2.2 不纳入本计划

本计划不包含：

- Desktop、Web、Mobile 页面或交互；
- 前端 Tauri bridge、React hooks、Flutter provider 或浏览器状态管理；
- 会议模板、会议类型或程序引擎；
- 看板版本历史、实时协作、订阅或变更通知；
- 投票、法定人数或 Human 确认；
- 正式主持权转移或副主持人；
- 将现有活动 V1 Session 原地升级成 V2；
- Project View、Workflow、Git 或第三方系统写回；
- 根据看板或 speech 自动产生任何外部副作用；
- 对会议结论正确性或质量的自动判断。

## 3. 交付原则

### 3.1 V1 是稳定基线

V2 是新的持久协议能力，不是对已有 `v=2 + moderated-baton-v1` Session 的静默改义。

- V0、V1 和 V2 必须由 Session 持久协议标识明确分流；
- 旧 V0/V1 Session 创建、主持、发言、恢复和 End 行为保持不变；
- 初版只允许新建 V2 Session，不提供 V1 → V2 原地转换；
- 产品名称“V2”不能直接用作 wire schema 数值；
- 新二进制即使关闭 V2 Create，也必须继续读取、恢复和结束已有 V2 Session；
- 每个阶段都必须运行 V1 回归，不能把兼容验证留到最后。

### 3.2 Relay 保持程序权威

Agent 和 CLI 只能提交签名命令或读取当前状态，不能自行推进权威程序。

Relay/数据库继续负责：

- 主持人、参会名单和当前访问权校验；
- 当前 Control Token、Offer、Grant 和终态校验；
- Board Maintenance 与 Floor Decision 的顺序；
- 两类 deadline 的起点和恢复；
- Human Floor Request 的优先级；
- Directed Handoff 和 Recall 的 V1 语义；
- 迟到、重复和冲突命令的收敛；
- 正常闭会与异常终止的权威分类。

没有任何 LLM 输出可以成为安全不变量的唯一依据。

### 3.3 看板是当前投影，不是版本化文档

后端需要可靠地保存和读取当前看板，但不得把内部并发机制暴露成产品版本模型。

- 每场 V2 会议有且只有一份当前看板；
- 主持人更新完整的当前内容，不要求参会者重放 patch；
- 读取者不提交或比较 board revision；
- 底层可以使用事件 ID、事务锁、control epoch 或 receipt 保证幂等和防迟到；
- 这些内部标识不得进入 Agent 的业务判断或成为调用方必须维护的看板版本；
- 看板正文更新不得进入 Meeting live outbox 或普通 live fan-out，也不得触发 Agent Turn；
- 不包含看板正文的 Baton 程序状态仍继续使用既有权威 State 和 outbox。

### 3.4 按需读取，不做看板同步器

Agent 不维护一条独立的看板订阅流。每个语义 Turn 在启动前读取一次当前看板：

- Intent Turn 与 Granted Speech Turn 分别读取；
- Board Maintenance Turn 与 Floor Decision Turn 分别读取；
- 读取失败不能使用已知旧副本启动 Turn；
- 看板内容变化本身不唤醒 Agent；
- speech/control 的 V1 同步器继续存在，但不把看板变成新的长期同步流。

### 3.5 每阶段形成可运行证据

每个阶段必须交付可运行场景和自动化测试，不能以“接口已经定义”或“代码已基本完成”作为
退出条件。阶段可以由多个小型 PR 组成，但在阶段完成前必须共同满足对应验收门槛。

## 4. 总体实现结论

Meeting V2 后端保持以下分层：

```text
V2 Create / Board / Close commands
                 ↓
        Relay protocol routing
                 ↓
   Meeting Session transaction boundary
        ├── current Meeting Board
        ├── V1-derived Baton/Floor state
        ├── Board Maintenance gate
        ├── independent deadlines
        └── terminal classification
                 ↓
      current-board read contract
                 ↓
       CLI / ACP on-demand reads
```

关键实现方向是：

1. 为 V2 冻结新的持久协议 discriminator，具体 schema/policy 值在阶段一确定；
2. 复用 V1 的私有 Session、固定名单、Baton、唯一 Grant、Human 优先和恢复机制；
3. 增加每场 Session 一份当前看板，但不增加产品 board revision；
4. 在 V1 主持决策之前增加耐久的 Board Maintenance gate；
5. Board Maintenance 完成或超时后才启动完整 Floor Decision deadline；
6. Human Floor Request 可以绕过或抢占 Board Maintenance；
7. Directed Handoff 链不插入 Board Maintenance；
8. 控制权丢失后，迟到看板结果不能生效；
9. ACP 在规定节点拉取当前看板，而不是接收变更通知；
10. 正常 close 与 admin/security/failure abort 共用不可恢复 End 边界，但保留不同产品结果；
11. 初版通过 CLI 提供完整 Human 后端操作面，前端不在本计划内。

## 5. 现有能力复用

| 现有能力 | 主要位置 | V2 用法 |
|---|---|---|
| kind 注册与 command/relay-only 分类 | `buzz-core/src/kind.rs` | 注册 V2 所需协议能力并保持 ingest gate 一致 |
| Meeting SDK builders 与 fixtures | `buzz-sdk/src/builders.rs`、`buzz-sdk/tests/meeting_v1_*` | 建立 V2 创建、看板、主持和终态的严格契约基线 |
| 私有 Session、Channel 与固定名单 | `buzz-db/src/meeting.rs` | 继续作为看板和 speech 的访问边界 |
| V1 Baton 事务与控制状态 | `buzz-db/src/meeting_baton.rs` | 复用唯一 Offer/Grant、Control Token、Human 优先和终态迁移 |
| V1 command 事务与 receipt | `buzz-db/src/meeting_baton/commands.rs` | 复用 Session 行锁、幂等和迟到命令校验模式 |
| Meeting command executor | `buzz-relay/src/handlers/command_executor.rs` | 按持久协议分流 Create、End 和 V2 command |
| V1 Relay handler | `buzz-relay/src/handlers/meeting_baton.rs` | 复用鉴权、解析、事务结果和 State fan-out 模式 |
| Meeting runtime | `buzz-relay/src/meeting_runtime.rs` | 扩展 Board deadline、lazy recovery 和已有 Session 继续运行语义 |
| Meeting durable outbox | `meeting_event_outbox` 及 runtime worker | 只继续投递不含看板正文的控制 State；看板正文更新不得写 outbox 或普通 live fan-out |
| Agent-first Meeting CLI | `buzz-cli/src/commands/meetings.rs` | 提供最先可验收的 V2 创建、读取、维护和终止操作 |
| ACP Meeting 协调器与 ledger | `buzz-acp/src/meeting.rs`、`meeting_v1.rs` | 增加按需看板加载和 Board Maintenance Controller |
| V1 Prompt 分层 | `buzz-acp/src/meeting_*_prompt.md` | 向各语义 Turn 注入当前看板并保持结构化输出 |
| Meeting E2E 与发布入口 | `buzz-test-client`、`scripts`、`Justfile` | 增加 V2 gates，同时持续运行 V0/V1 回归 |

不应继续把所有新增逻辑堆入已经很大的 V1 command 文件。阶段级实现设计应优先保持
“当前看板领域操作”与“Baton 程序协调”边界清晰，但本文不提前冻结具体模块拆分。

“复用 V1 Baton”不表示放宽 V1 public mutation、handler 或 ACP 对
`schema=2 + moderated-baton-v1` 的双层断言。V2 应通过提取协议中立的共享 Baton kernel、
增加 V2 adapter，或建立独立 V2 模块来复用能力；具体选择在阶段二确定。V1 handler 和
数据库入口始终只接受 V1，V2 入口始终按新的持久 discriminator fail closed。

## 6. 关键实现边界

### 6.1 协议身份与路由

阶段一必须选择新的持久 V2 协议身份，并满足：

- Create 时冻结，后续事件不能改变；
- Relay、DB、CLI 和 ACP 从持久 Session 识别 V2；
- 不能依赖产品名称推导 wire 数值；
- 不能让旧 V1 handler 接受 V2 专属命令；
- 不能让 V2 recovery 扫描或处理 V0/V1 Session；
- 不识别的 schema/policy 组合继续 fail closed；
- 新建开关只限制新的 V2 Create，不停止已有 V2 runtime。

具体 schema 数值、policy 名称、是否复用现有 command kind、是否增加新 kind，在阶段一的
协议设计中确定。优先使用 Nostr command 和现有通用查询面，不新增 Meeting 专用 HTTP
端点。

### 6.2 创建与初始看板

V2 Create 必须在一个一致性边界内完成：

- 创建私有 Meeting Session；
- 创建固定参会名单；
- 强制 event author = owner = moderator；
- 创建初始当前看板；
- 初始化 V2 Baton 与 Board Maintenance 状态；
- 形成可恢复的初始权威状态；
- 失败时不能留下 active 但没有看板的半成品会议。

初始看板允许没有 Project View 或任何外部引用。看板内容封装、逻辑区块、最大尺寸、文本
规范化和空值规则在阶段一确定，不在本计划中固定。

### 6.3 当前看板读写

当前看板后端需要提供两个稳定能力：

```text
get_current_board(meeting)
update_current_board(meeting, complete_board)
```

这里是能力描述，不是预定函数或 API 名称。

读取必须：

- 只返回当前内容，不要求调用者重建历史；
- 复用当前 Community、Session 固定名单和安全撤权 gate；
- 允许主持人和普通参会者读取；
- 拒绝非参会者；
- 在会议结束后继续遵循 V1 的只读历史权限；
- 不因为引用的外部资源不可用而失败整个看板读取。

更新必须：

- 只允许主持人；
- 只允许 active V2 Session；
- 只允许主持人持有 Control Token 且没有活动 Offer/Grant 的窗口；
- 以完整当前内容替换，不要求产品 patch/revision；
- 具备事件级幂等和控制窗口 fencing；
- 在 Human 抢占、End 或控制权丢失后拒绝迟到结果；
- 提交结果不明确时先读取权威当前状态或 receipt，再决定是否重试，不能盲目覆盖当前内容；
- 不增加 speech revision，不计作 canonical speech 或 thread reply；
- 不隐式写入 Project View 或任何外部系统；
- 不进入 Meeting outbox 或普通 live fan-out。

### 6.4 Board Maintenance gate

Relay 必须能区分主持控制机会中的两个程序步骤：

```text
board_pending
    ├── updated
    ├── unchanged
    └── timed_out
            ↓
floor_decision
```

这些名称只描述内部职责，不预定 wire phase 或数据库枚举。

关键规则：

- Control Token 返回主持人后，先开放 Board Maintenance；
- 主持人已 idle、后来被新 Intent 或可处理工作唤醒时，也先开放 Board Maintenance；
- 主持人可以更新完整看板或明确保持不变；
- Board Maintenance timeout 保持原看板，但不能冒充主动 `unchanged`；
- Floor Decision deadline 只能在 Board Maintenance terminal 后开始；
- Floor Decision 必须取得完整预算，不能继承 Board Maintenance 剩余时间；
- 已有 V1 moderator attempt、decision epoch 和 fallback 不能在 Board Maintenance 期间
  提前启动；
- 同一主持控制机会不能并行运行多个 Board Maintenance；
- Board terminal、Floor Decision start 和必要 State 变化必须可重启恢复；
- Board Maintenance 与 Floor Decision 在权威状态中不得同时 active；
- V1 Candidate Cohort 和 moderator attempt 只在 Board terminal 后的 Floor Decision
  window 中冻结和注册；
- Board 期间到达的普通 Intent 不取消当前 Board Maintenance，Human Request 仍按 V1
  立即抢占。

### 6.5 Human 与直接接力

Board gate 不能破坏 V1 的两条直接路径：

- Human Floor Request 仍拥有下一席优先级；
- 合法 Directed Handoff 仍可直接产生下一次 Offer。

实现计划必须覆盖：

- Human Request 在 Board Maintenance 期间到达时不等待主持 Agent；
- Human 抢占后，尚未提交的 Board 结果失效；
- 若 Board 更新已经先提交，则 Human Request 在更新后的状态继续按 V1 前进；
- Human FIFO 不能被 Board deadline 重排；
- Handoff 链中不启动主持 Board Turn；
- Recall 仍不能越过已排队 Human；
- Control Token 最终返回主持人后，才开始新的 Board Maintenance；
- 这些路径不要求 board revision 或 Grant 携带 board revision。

### 6.6 Deadline、sweeper 与 lazy recovery

V2 增加 Board Maintenance deadline，但继续复用 V1 的持久时间和恢复原则：

- 权威时间来自数据库；
- 当前 Session 只暴露最早需要处理的程序 deadline；
- sweeper 领取 due Session 后重新锁定并复查；
- 任意 V2 写命令先执行 lazy recovery，再验证自身；
- Board timeout 保持当前看板并原子进入 Floor Decision；
- Floor Decision 从该迁移取得完整 deadline；
- 多 Relay 副本不能重复终结 Board window 或重复启动 decision window；
- Relay 重启不能丢失 Board pending、重复更新看板或提前 fallback；
- 安全撤权和 End 始终高于 Board/Floor 状态。

精确时长、重试次数、`next_action_at` 表达和 recovery effect shape 在阶段二讨论。

### 6.7 正常关闭与异常终止

V2 在同一个不可恢复 Meeting End 边界中区分产品结果：

- `closed`：主持人在合法控制窗口完成最后一次 Board Maintenance 后，声明目标已达成并
  形成有效结论；
- `aborted`：主持人声明无法继续或无法形成有效结论，或者管理员、安全撤权、故障等
  非成功路径结束会议。

后端不判断结论文本的质量，但必须保证：

- 普通参会者不能正常 close；
- 普通参会者不能主动 abort；
- 主持人可以主动 abort，但不能把它记录为目标达成的 normal close；
- 正常 close 不能发生在活动 Offer/Grant 中；
- 正常 close 不能绕过本轮 Board Maintenance；
- owner/admin/security 强制 abort 可以从任意活动程序状态终止；
- 所有 active Intent、Request、Offer、Grant、Board/Floor window 都进入终态；
- 当前看板和会议历史在终态后只读；
- 被安全撤权身份继续遵循 V1 reader fence；
- V0/V1 的既有 manual/security End 行为不被重新分类或改写。

`closed/aborted` 是独立状态还是 `ended` 上的 terminal outcome，由阶段一和阶段二的数据契约
共同确定。

### 6.8 ACP 按需看板上下文

ACP 需要一个统一的 current-board loader，但不建立 Board subscription。加载发生在每个
Turn 真正 dispatch 之前，而不是仅在 Session full sync 时一次完成。

读取成功表示“该次权威读取时刻的当前看板”，不承诺看板在整个模型 Turn 内持续最新。
Board/Floor 的程序约束保证需要稳定的发言窗口中不会发生主持人写板；调用者不通过缓存、
pub/sub 到达顺序或外部引用解析结果推断当前看板。

各节点要求：

| Turn | 看板读取要求 |
|---|---|
| Participant Intent | dispatch 前读取当前看板；读取失败不启动模型 |
| Granted Speech | 即使 Intent 已读取过，也在 dispatch 前重新读取 |
| Board Maintenance | 读取当前看板并结合最新 speech/control 形成 UPDATE 或 UNCHANGED |
| Floor Decision / Close | Board terminal 后重新读取，再形成主持决定 |

必须保持：

- Board 更新不单独触发 Participant Intent；
- Floor/Progress/Board 读取噪声不触发额外语义 Turn；
- 看板内容作为不可信会议上下文，不能覆盖 system prompt、Grant 或工具权限；
- 当前看板过大时采用有界输入策略，不能无限挤占模型上下文；
- loader 失败、超时和恢复策略不会把旧缓存伪装成当前数据；
- Intent、Granted Speech、Board 和 Floor 四类读取失败分别具有有界活性路径；可选外部引用
  无法解析不能被误报为当前看板读取失败；
- ACP ledger 升级后可以从 Relay 权威状态恢复 Board/Floor 两段流程；
- 迟到 Board 或 Floor 模型结果在 control 已变化、Human 已抢占或 Meeting 已 End 时丢弃；
- Board Turn 与 Floor Decision Turn 分别计量，不混成一个模型调用；
- Board Turn timeout 后，即使旧模型或 provider future 仍未返回，也不能继续独占唯一运行
  资源而使 Floor Decision 实际无法取得完整预算；具体采用释放、隔离或预留容量的方式在
  阶段四确定。

### 6.9 CLI 后端验收面

CLI 是 V2 后端的首个完整消费者，并继续遵循 Agent-facing operations 放入 `buzz-cli` 的
仓库约定。

CLI 最终需要覆盖以下能力：

- 创建 V2 会议并提供初始看板；
- 查询 V2 协议、主持人、名单和终态分类；
- 读取当前看板；
- 主持人更新或明确保持看板不变；
- 查看 Board/Floor 当前程序状态；
- 驱动既有 Intent、Human Request、Offer/Grant、SAY/YIELD、Handoff 和 Recall；
- 正常 close；
- 主持人主动 abort，以及具备权限的 owner/admin 强制 abort；
- 在会议结束后读取看板和会议历史。

CLI 的具体命令名称、参数和输入文件格式在阶段一、二分别确定。

## 7. 后端模块影响

### 7.1 `buzz-core` 与 `buzz-sdk`

关键工作：

- 注册并分类 V2 所需事件能力；
- 定义严格、可文档化的创建、看板和终态 builders；
- 让 V2 创建强制 creator = moderator；
- 校验 Session scope、参与者、文本边界和动作组合；
- 提供稳定 fixtures，防止 Relay、CLI 和 ACP 对 wire 的理解漂移；
- 保证 V1 常量、builders 和 fixtures 完全不变。

阶段一只冻结最小 wire 契约；所有 action、tag、content 上限和错误码不在本文预定。

### 7.2 `buzz-db`

关键工作：

- additive migration；
- 每场 V2 Session 一份当前看板；
- V2 协议与创建原子性；
- Board Maintenance gate、deadline 和终态；
- 与 Baton 状态、Session 行锁和 reader fence 集成；
- 当前看板读取、主持写入和 CLI/Relay 所需查询；
- Board timeout、Floor Decision start、close/abort 和安全撤权；
- 扩展 `meeting_revocation.rs`，让 V2 撤权终结 Board/Floor 并分类为 aborted；
- 并发、幂等、恢复和终态测试；
- 同步 `schema/schema.sql`、`buzz-db/src/migration.rs` 断言，以及 migration fresh、
  upgrade、concurrent 和 schema drift gates。

当前 migration 基线结束于 `0042_meeting_v1_moderator_attempts.sql`。V2 使用其后的 additive
migration；精确文件拆分在阶段一确定。

### 7.3 `buzz-relay`

关键工作：

- V2 Create gate 与持久协议路由；
- 更新 `handlers/ingest.rs` 的 kind scope/command gate，确保新命令先经过统一验签和授权；
- 更新 `handlers/meeting.rs` 的 kind 9 speech 协议分流，确保 V2 进入 V2 adapter，而不是落入
  V0 fallback 或放宽 V1 handler 的协议断言；
- Board command 和 current-board read 的权限边界；
- V2 command handler 与数据库结果映射；
- Board/Floor State、outbox 和 non-notifying board write 的边界；
- Board deadline sweeper、lazy recovery 和多副本收敛；
- Human 抢占、Handoff、End 和 revocation 竞态；
- 低基数 metrics 与隐私安全日志；
- 关闭 Create 后已有 V2 Session 继续运行。

### 7.4 `buzz-cli`

关键工作：

- V2 协议检测；
- 更新 `buzz-cli/src/lib.rs` 的命令枚举/参数入口和 Meeting 命令实现；
- 创建、当前看板读写和终态操作；
- Board/Floor 状态的 compact 与 JSON 输出；
- 明确区分 input、auth、network、conflict 和 terminal 错误；
- 支持脚本化、多身份和真实 Agent 验收。

### 7.5 `buzz-acp`

关键工作：

- V2 Session 发现和协议选择；
- current-board loader；
- 调整 `src/lib.rs` 的 `dispatch_meeting_pending` / `MeetingTurnRequest` dispatch seam，保证
  看板在排队结束、模型真正 dispatch 前读取，而不是在请求入队时提前冻结；
- Intent 与 Granted Speech 的逐 Turn 注入；
- 独立 Board Maintenance Controller、Prompt 和严格输出；
- Board terminal 后的 Floor Decision/Close Controller；
- 运行容量优先级、deadline、安全余量和读取失败处理；
- ledger schema、重启恢复、迟到结果 fencing 和 prepared command 幂等；
- 不因 Board 更新产生额外 Agent Turn；
- 不记录看板正文或 prompt 内容到默认 telemetry。

### 7.6 测试、脚本和运维

关键工作：

- SDK fixture 和协议 registry tests；
- DB contract、race、deadline 和 migration tests；
- Relay/CLI E2E；
- ACP 确定性模型测试与真实 Agent qualification；
- V0/V1 回归入口扩展；
- V2 专项后端 gate；
- metrics、告警、灰度、故障处置和回滚说明。

## 8. 并发与故障矩阵

阶段二和阶段四开始前，必须为以下边界形成详细设计与确定性测试矩阵：

| 当前状态/操作 | 并发或故障 | 必须保持的结果 |
|---|---|---|
| V2 Create | 重复提交、部分失败 | 最多一场完整 Session；不能缺看板或 Baton |
| Board pending | Human Request 到达 | Human 不等待；未生效 Board 结果失效 |
| Board pending | 普通 Intent 到达 | Intent 保持 pending；不取消 Board、不启动 Floor deadline/fallback |
| Board update | Human Request 同时提交 | 以 Session 权威顺序收敛；不能出现迟到覆盖 |
| Board pending | End/安全撤权 | 直接终态；Board 结果不得复活会议 |
| Board pending | Relay/ACP 重启 | 恢复同一窗口或确定性超时；不重复更新 |
| Board timeout | Floor Decision start | 原看板保留；Decision 获得完整预算 |
| Board completed | Decision dispatch 失败 | 可恢复重试；不重新消耗 Board 更新 |
| moderator idle | 新 Intent 到达 | 先 Board Maintenance，再 Floor Decision |
| active Offer/Grant | Board update | 拒绝，不改变当前 speaker 上下文 |
| 任意 Board action | speech timeline | 不增加 speech revision，不形成 canonical speech/thread reply |
| Directed Handoff | Board gate | 继续 V1 直接路径，不插入 Board Turn |
| Human FIFO | Recall/Board gate | FIFO 保持，Recall 不能越过 Human |
| Intent 已生成 | Board 后续变化 | Grant Turn 重新读取当前看板 |
| current-board read | 失败或超时 | 不使用旧副本启动相应 Agent Turn |
| normal close | active Offer/Grant | 拒绝；等待合法主持窗口 |
| moderator abort | 任意 active phase | 进入 aborted；不得记录为目标达成的 normal close |
| admin/security abort | 任意 active phase | 进入 aborted，不要求最终看板 |
| 任意 V2 command | 重放或结果丢失 | 幂等，不重复 Board/Floor/End effect |

本表只冻结必须成立的结果；锁顺序、SQL、attempt 字段和错误码在阶段开发时确定。

## 9. 测试策略

### 9.1 协议与 SDK

至少覆盖：

- V2 discriminator 与 V0/V1 隔离；
- creator = moderator；
- 初始看板和 Board command 的合法/非法组合；
- Meeting scope 和签名作者约束；
- close/abort 的角色与原因分类；
- fixtures 与 builder 输出一致；
- V1 fixture 零变化。

### 9.2 数据库

至少覆盖：

- V2 Create、名单、看板和初始程序状态原子提交；
- 每场 active V2 Session 恰好一份当前看板；
- 主持人写、参会者读、非参会者隔离；
- 无 active Offer/Grant 的写 gate；
- Board update/unchanged/timeout；
- 独立 Board/Floor deadline；
- Board pending 期间普通 Intent 只进入 pending，Board terminal 后才进入 Floor cohort；
- Human 抢占和迟到 Board fencing；
- idle wake；
- Handoff/Human 直接路径；
- close/abort 和终态只读；
- security revocation；
- command replay、重启和多 worker recovery；
- V0/V1 行为不变。

### 9.3 Relay 与 CLI E2E

至少覆盖：

- 两个以上签名身份创建、读取和推进同一 V2 Session；
- 初始看板无 Project View 的有效路径；
- 普通参会者可读但不可写；
- 非参会者无法读取；
- Board 正文更新不产生对应 outbox row 或 live frame；若程序推进产生 Baton State，该 State
  不携带看板正文；
- Board action 不改变 speech revision，也不形成 canonical speech/thread reply；
- Board Maintenance 后才能进行主持 Select/self/close；
- Board timeout 后 Floor Decision 仍有完整预算；
- Human Request 抢占 Board；
- Directed Handoff 不插入 Board Turn；
- Relay 重启、lazy recovery 和 outbox；
- 主持人 normal close、主持人主动 abort、admin/security 强制 abort 与历史读取；
- 关闭 V2 Create 后已有 Session 继续并结束；
- V0/V1 全套 E2E 回归。

### 9.4 ACP 确定性测试

至少覆盖：

- Intent Turn 读取并收到当前看板；
- Intent 后看板改变，Granted Turn 读取新内容；
- 看板读取失败时不调用模型；
- Board 更新不触发额外 Intent Turn；
- Board Maintenance 输出 UPDATE/UNCHANGED；
- Board terminal 后重新读取再启动 Floor Decision；
- Board timeout 不消耗 Decision 时间；
- 注入不会结束的 Board provider future，证明 timeout 后 Floor Turn 实际取得可用运行资源
  和完整预算；
- Human 抢占使迟到 Board 结果作废；
- Human 抢占后即使旧 Board future 继续运行，Human speech 结束后的新 Board/Floor cycle
  仍能取得运行资源和完整预算；
- control 变化或 End 使迟到模型结果作废；
- ACP 重启恢复但不重复更新或决定；
- idle 不热循环，新工作可以唤醒；
- 全 Agent 多轮会议最终 closed；
- 看板文本不能改变 Grant 或工具权限。

### 9.5 真实 Agent 验收

后端签收前至少完成：

- `2 Human CLI + 2 Agent` 混合会议；
- `1 moderator Agent + 2 participant Agent` 全 Agent 会议；
- 主持 Agent 至少两次更新看板并多轮选择 speaker；
- Intent 与 Grant 之间发生一次看板变化；
- 一次 Human 抢占 Board Maintenance；
- 一段 Directed Handoff 链后由主持人统一归纳；
- 主持 Agent 写入最终结论并正常 close；
- 一场由管理员或故障路径 aborted 的会议；
- 一场由主持 Agent 主动判定无法形成有效结论并 aborted 的会议；
- 全部场景不依赖 Project View，也不产生外部写入。

真实 LLM 验收验证运行行为和恢复，不把模型内容质量作为 Relay 协议正确性的唯一证据。

### 9.6 质量门禁

各阶段按影响范围运行：

- Rust fmt、clippy、unit 和 workspace build/check；
- `buzz-core`、`buzz-sdk`、`buzz-db`、`buzz-relay`、`buzz-cli`、`buzz-acp` 针对性测试；
- migration fresh、upgrade、concurrent 和 schema drift；
- Meeting V0/V1 backend regression；
- 涉及 Relay、DB 或 Auth 时运行完整 integration tests；
- 阶段五运行完整 `just ci`、`just test` 和 V2 专项后端 gate。

本计划不新增 Desktop、Web 或 Mobile 的实现与专项验收；但 `just ci` 是仓库统一质量门禁，
其中已有的客户端回归检查仍需照常运行并通过。

## 10. 可观测性与隐私

V2 后端至少需要观察：

- Board command 的 accepted、rejected、duplicate 和 conflict；
- current-board read 的 success、denied、not-found、error 和 latency；
- Board Maintenance 的 update、unchanged、timeout、preempted 和 discarded；
- Board Maintenance 和 Floor Decision 各自耗时；
- Board timeout 到 Floor Decision start 的恢复延迟；
- Human 抢占和迟到 Board result；
- normal close 与不同 abort 原因；
- sweeper、lazy recovery、outbox 和 worker error；
- ACP board-load、Board Turn、Decision Turn 和 stale-result outcome。

还应提供不会引入高基数的硬不变量观测，并要求以下计数在验收中始终为零：

- Board Maintenance 与 Floor Decision 同时 active；
- Board 未 terminal 就启动 Floor Decision；
- active Offer/Grant 期间接受 Board 写入；
- 未成功读取当前看板却 dispatch 语义 Turn；
- 已超时或被抢占的 Board 结果落地；
- Board action 改变 speech revision 或形成 canonical speech/thread reply；
- idle 无新工作却重复启动 Board/Floor 循环；
- 非参会者成功读取，或非主持人写入被 accepted。

指标必须保持低基数。metric label 和默认日志不得包含：

- meeting/session ID；
- pubkey；
- event ID；
- 看板正文、目标、议程、记录或结论；
- Intent、speech 或 prompt 内容；
- 外部引用正文；
- 原始错误正文。

阶段五再确定具体 metric 名称、SLO、告警阈值和运维查询。

## 11. 灰度、兼容与回滚

### 11.1 迁移

- 只使用 additive migration；
- 新 schema 允许 V0/V1 和 V2 并存；
- fresh install、从当前 V1 schema 升级和 schema drift 都必须通过；
- 不回填或改写现有 V1 Session；
- migration 失败不能留下部分 V2 schema；
- 回滚目标必须能够识别或安全容忍数据库中已有的 V2 schema 和数据。

### 11.2 发布顺序

建议发布顺序：

1. V2 Create 默认关闭；
2. 部署能够读取、恢复和结束 V2 的全部 Relay 路由实例与 deadline worker/sweeper；
3. 部署 SDK、CLI，以及所有可能处理任意 V2 Turn 的 ACP 实例；
4. 确认上述运行单元均报告预期 V2 capability，完成全 fleet 能力收敛；
5. 运行迁移、V0/V1 回归和 V2 后端 gate；
6. 仅在测试 Community 开启 V2 Create；
7. 运行 CLI-only、混合和全 Agent smoke；
8. 观察 Board/Floor deadline、read failure、recovery 和 End 指标；
9. 再逐步扩大灰度。

V2 Create 的开启条件必须同时覆盖 Relay 路由、deadline worker/sweeper，以及可能处理
participant 或 moderator V2 Turn 的全部 ACP，不能只检查单个实例。若存在不具备 V2
capability 的 ACP，调度层必须权威保证它永不接收 V2 Session。Session 创建后，其持久协议
和处理能力要求保持冻结；后续版本或 feature gate 漂移不能把运行中的 V2 Session 降级为
V1，也不能跳过 Board gate。

### 11.3 关闭与降级

- 关闭 Create 只阻止新 V2 会议；
- 已有 V2 Session 的 Board、Floor、Agent、recovery 和 End 继续运行；
- 单纯停止 Board worker 或 ACP 不得让 Relay 接受非法 Floor Decision；
- 安全回滚只能回到前向兼容的二进制：它必须容忍 V2 schema/data、读取终态 V2，并能继续
  处理运行中的 V2，或在 active V2 全部结束后安全跳过其 runtime；
- 完全不识别 V2 的旧 fleet 即使在 active V2 全部结束后也不是安全回滚目标；
- 不通过把 V2 Session 改写为 V1 来完成回滚；
- 不执行破坏性 down migration 来配合二进制回滚；
- 终态 V2 数据保持可读，具体长期保留策略沿用 Meeting 既有规则。

## 12. 阶段关系

```text
阶段一：协议、当前看板与 CLI 基础
                   ↓
阶段二：Relay 权威主持控制周期与终态
                   ↓
阶段三：普通参会 Agent
                   ↓
阶段四：主持 Agent
                   ↓
阶段五：后端综合验收与发布
```

五个阶段严格按 `1 → 2 → 3 → 4 → 5` 推进。普通参会 Agent 只有在阶段二的 V2 Floor
权威时序、看板写入窗口和 CLI-only 生命周期稳定后，才能得到可验证的 Intent/Grant 运行
环境；主持 Agent 再建立在同一时序与阶段三 current-board loader 之上。阶段五收敛全部
后端成果。

## 13. 分阶段开发规划

### 阶段一：协议、当前看板与 CLI 基础

目标：

> 建立 V2 的兼容隔离和当前看板纵向链路，但暂不交付完整 Agent 主持流程。

阶段开始前需要讨论并冻结：

- V2 持久协议 discriminator；
- 最小 Create 与 current-board read wire 契约；
- 看板内容 envelope、上限和空值语义；
- 当前看板的持久化边界；
- CLI 最小命令面；
- V2 Create 灰度开关。

本阶段做什么：

- 增加 core kind/registry、SDK builders 和 fixtures；
- 增加 additive migration 和 current-board DB 能力；
- V2 Create 强制 creator = owner = moderator；
- Create、固定名单、初始看板和初始控制状态原子提交；
- 复用 Meeting reader fence，实现主持/参会者/非参会者权限矩阵；
- 提供 current-board pull read；
- 提供 CLI 创建、查询和读板能力；
- 建立 V2 协议路由和默认关闭的 Create gate；
- 保持初始看板无 Project View 依赖和无外部写回；
- 暂不暴露独立 Board update 命令；该写入口随阶段二的权威 Board gate 一起交付；
- 阶段一 Create 只允许在可丢弃的隔离测试环境开启；V2 Floor、speech、ACP dispatch 和其他
  推进入口全部 fail closed，测试产生的 active Session 由 fixture teardown 清理；
- 补齐 migration、protocol、DB、Relay 和 CLI tests。

阶段交付：

- 一份阶段一 wire/data 细化设计；
- V2 协议与数据 migration；
- SDK fixture；
- CLI-only 的 Create → Get Board 路径；
- 读取权限、创建原子性、协议隔离和 V1 兼容测试。

完成标志：

> 在 V2 Create 开关下，CLI 可以创建带初始看板的 V2 会议；主持人和普通参会者能按需
> 读取，非参会者不能读取；尚无独立 Board update 公共入口；调用者不管理 board revision；
> V0/V1 行为不变。该开关尚不得在共享或实际运行环境开启，且测试 Session 不得跨 fixture
> 保留；只有阶段二验收完成后，V2 Create 才具备完整后端生命周期。

### 阶段二：Relay 权威主持控制周期与终态

目标：

> 不依赖 ACP，仅用 CLI 完成 `Board Maintenance → Floor Decision → speech →
> close/abort` 的完整后端生命周期。

阶段开始前需要讨论并冻结：

- Board Maintenance 的内部状态和 terminal outcome；
- 独立 Board/Floor deadline 的起点；
- no-change、timeout 和 Human preemption 的权威语义；
- Board update/unchanged 命令、幂等和 control-window fencing；
- idle wake、decision epoch 和 fallback 的协调；
- normal close 与 abnormal abort 的 wire/data 分类；
- recovery、outbox 和 State effect 边界；
- 并发矩阵中的锁顺序和错误类别。

本阶段做什么：

- 在主持控制机会前建立 Board Maintenance gate；
- 补齐 Board action、close/abort 的 SDK builders、fixtures 与 Relay/CLI 命令；
- 完成 `handlers/meeting.rs` 的 V2 kind 9 speech 分流和协议隔离；
- 只允许主持人在合法 Board window 更新完整当前看板或声明 unchanged；
- 支持 update、主动 unchanged 和 timeout；
- 只在 Board terminal 后启动完整 Floor Decision window；
- 调整 moderator decision/fallback，使其不能在 Board pending 时提前运行；
- 让 idle 后的新工作重新经过 Board Maintenance；
- 保持 Human Floor Request 和 Directed Handoff 的 V1 路径；
- 处理 Human 抢占、End、撤权和迟到 Board result；
- 扩展 sweeper、lazy recovery、restart recovery 和 `next_action_at` 协调；
- 支持正常 close 和异常 abort；
- 终态化全部 Board/Floor 活动对象，并保持当前看板与会议历史只读；
- 提供 CLI Board action、Floor status、close 和 abort 操作；
- 断言 Board 正文更新本身不写 Meeting outbox、不产生 live frame，也不计入 speech；程序
  推进所需的 Baton State 仍可进入 outbox，但不得携带看板正文；
- 建立 CLI-only 多身份 E2E 和竞态测试。

阶段交付：

- 一份阶段二状态/并发细化设计；
- Relay/DB/CLI 的完整 V2 控制闭环；
- Board/Floor 双 deadline 和恢复测试；
- Human/Handoff/V1 fallback 兼容证据；
- close/abort 生命周期证据。

完成标志：

> 两个以上 CLI 身份可以完成多轮 V2 会议；Relay 能证明主持人的 Board Maintenance 先于
> Floor Decision，二者时间预算独立；Human 优先和 Directed Handoff 未改变；会议可以
> 正确 closed 或 aborted；Relay 重启后仍可继续。

### 阶段三：普通参会 Agent

目标：

> 普通 Agent 在 Intent 与正式发言时分别使用当时的当前看板，不建立 Board 订阅。

阶段开始前需要讨论并冻结：

- current-board loader 契约；
- Prompt 中看板的边界、截断和不可信上下文表达；
- 读取失败、重试和最终 PASS/YIELD 策略；
- ACP ledger 的最小兼容升级；
- Turn capacity 和既有 V0/V1 调度关系；
- 确定性测试注入点。

本阶段做什么：

- 以阶段二已验收的 V2 Floor/Board 生命周期作为唯一程序基线；
- 让 ACP 识别和发现 V2 Session；
- 增加统一 current-board loader；
- Participant Intent dispatch 前读取并注入当前看板；
- Granted Speech dispatch 前重新读取并注入，不复用 Intent 时内容；
- 读取失败时不使用旧内容启动模型；
- 看板更新不触发 Agent Turn；
- 保持 Offer ACK、Progress、SAY/YIELD 和 Handoff 的 V1 行为；
- 防止看板内容覆盖 Grant、工具和外部授权；
- 增加 ledger/restart、隐私和多会议隔离测试；
- 完成普通 Agent 的确定性 E2E。

阶段交付：

- 一份阶段三 ACP read/prompt 细化设计；
- V2 participant Intent 与 Granted Speech 路径；
- Intent 与 Grant 读取不同当前看板的证据；
- read failure、无订阅和 V1 regression tests。

完成标志：

> 普通 Agent 可以基于当前看板提交 Intent；获 Grant 后总是重新读取，并可在议程变化时
> 发言或 Yield；看板更新不产生额外 Turn；读取失败不会让旧看板冒充当前内容。

### 阶段四：主持 Agent

目标：

> 主持 Agent 通过独立 Board Maintenance Turn 和 Floor Decision Turn 持续维护会议，
> 安排发言并结束会议。

阶段开始前需要讨论并冻结：

- Board Maintenance Prompt、严格输出和提交动作；
- Board Turn 与 Floor Decision 的运行容量和优先级；
- Board provider 卡住或超时后的资源释放、隔离或预留策略；
- 两类 Turn 的 deadline、安全余量和重试；
- Human preemption、control loss、timeout 和 End 后的迟到结果处理；
- moderator idle wake 和防热循环；
- ledger 状态、prepared command 和重启恢复；
- 主持 Agent close/abort 的结构化输出边界。

本阶段做什么：

- 增加独立 Board Maintenance Controller；
- 读取当前看板和最新会议上下文，输出 UPDATE 或 UNCHANGED；
- Board terminal 后再次读取当前看板，再启动 Floor Decision/Close；
- 保证 Board/Floor 两类 Agent Turn 不共享 Relay 时间预算；
- Human 抢占时停止等待 Board，迟到结果不提交；
- Board timeout 后让 Floor Decision 获得完整预算；
- 即使旧 Board provider future 仍运行，也保证 Floor Decision 实际获得可用运行 slot；
- 支持 select、moderator speak、idle、close 和主动 abort；
- 保持 V1 Candidate Cohort、Human priority、Handoff 和 fallback 语义；
- 让 idle 状态可被新工作唤醒但不产生热循环；
- 完成 ACP/Relay 双重重启和网络结果不明确恢复；
- 增加低基数 telemetry，不记录看板正文；
- 完成确定性全 Agent 多轮会议。

阶段交付：

- 一份阶段四 Agent moderator 细化设计；
- Board Turn → reload → Floor Decision 的 Agent 闭环；
- Human preemption、timeout、late result、restart 和主持 abort tests；
- 至少一场确定性全 Agent 正常闭会证据和一场主持 Agent 主动 abort 证据。

完成标志：

> 一名主持 Agent 与至少两名参会 Agent 可以持续多轮讨论；主持 Agent 能更新看板、安排
> speaker、归纳 Handoff 链并写入最终结论后正常 close，也能在无法继续或无法形成有效结论
> 时主动 abort；任何 Board Turn 失败都不会侵占 Floor Decision 时间或破坏 V1 安全不变量。

### 阶段五：后端综合验收与发布准备

目标：

> 将协议、Relay、CLI 和 ACP 收敛为可恢复、可观测、可灰度发布的 Meeting V2 后端。

阶段开始前需要讨论并冻结：

- V2 专项 test gate 和 acceptance runner；
- 真实 Agent 场景、模型配置和证据格式；
- metrics、SLO、告警和日志；
- feature gate、部署顺序、关闭和降级 runbook；
- 后端协议、运行能力与已知边界。

本阶段做什么：

- 完成 CLI-only、混合 Human CLI/Agent 和全 Agent 综合场景；
- 覆盖 Human FIFO、Handoff、Recall、Board、Floor、close 和 abort 组合；
- 覆盖 Relay/ACP 重启、网络不明确、读取失败、超时和安全撤权；
- 完成多 Session 并发和 worker backlog 验证；
- 验证 Relay/worker/ACP capability gate，以及 participant/moderator V2 Turn 的能力路由；
- 建立 V2 专项后端测试入口并纳入 V0/V1 回归；
- 完成低基数 metrics、告警建议和运维说明；
- 验证关闭 Create 后已有 V2 继续运行和结束；
- 执行真实 Agent qualification 并形成可复现报告；
- 运行完整仓库质量门禁；
- 固化 CLI/ACP 使用的后端协议与错误语义，并记录已知边界。

阶段交付：

- V2 后端专项测试入口；
- CLI-only、混合和全 Agent 验收证据；
- 真实 Agent qualification 报告；
- 发布、观测、故障处置和回滚说明；
- 后端协议、能力和已知边界清单；
- 满足产品 spec 的后端发布候选。

完成标志：

> Meeting V2 产品 spec 中所有后端语义都有自动化或真实运行证据；V0/V1 无回归；V2 可以
> 安全灰度、关闭新建并恢复已有 Session；全 fleet capability 与回滚约束已经验证，不存在
> 后端发布阻断问题。

## 14. 阶段状态

| 阶段 | 状态 | 主要交付证据 |
|---|---|---|
| 1. 协议、当前看板与 CLI 基础 | 已完成（2026-08-02） | wire/data 细化设计、migration 0043、SDK fixture、CLI 当前看板链路、真实 Relay E2E |
| 2. Relay 权威主持控制周期与终态 | 已完成（2026-08-02） | CLI-only 生命周期、双 deadline、竞态、重启与恢复测试 |
| 3. 普通参会 Agent | 已完成（2026-08-02） | Intent/Grant 独立按需读板、read failure、无订阅与 V2 wire 测试 |
| 4. 主持 Agent | 已完成（2026-08-02） | Board/Floor 双 Turn、Human preemption、全 Agent 闭会与主动 abort 证据 |
| 5. 后端综合验收与发布准备 | 已完成（2026-08-03） | 专项 gate、真实 qualification、运维与发布候选 |

每个阶段开始时把状态更新为“进行中”；对应交付物、测试门槛和完成标志全部满足后更新为
“已完成”，并补充 PR、测试命令、报告或运行证据链接。

### 14.1 阶段一交付记录（2026-08-02）

阶段一已经冻结并实现：

- `v=3 + moderated-board-v1` 的持久协议身份，以及 creator = owner = moderator；
- kind `42110` Relay-only 当前看板、严格 Markdown envelope 和 65,536-byte 上限；
- migration `0043_meeting_v2_stage1.sql` 中的 V2 Session、当前看板与 bootstrap 状态；
- SDK Create builder、稳定 wire fixture、DB 原子创建与 Meeting reader fence；
- 默认关闭的 `BUZZ_MEETING_V2_CREATE_ENABLED` Relay gate；
- CLI `meetings create --policy moderated-board-v1 --board ...` 与
  `meetings board get`；
- ACP 不接管 bootstrap V2，以及 Board Update、Floor、speech、End 等未实现动作的
  fail-closed 边界。

协议和数据细节见 [阶段一最小协议与数据设计](./meeting-v2-stage1-design.md)。自动化证据覆盖
fresh/upgrade migration、schema drift、SDK fixture、DB 原子性、主持人/参会者/非参会者读取
矩阵、Relay-only 防伪造、V0/V1/V2 协议隔离和真实 Relay 纵向链路。

阶段交付时通过以下可复现门禁：

```bash
./scripts/run-meeting-backend-tests.sh
cargo clippy -p buzz-core -p buzz-sdk -p buzz-db -p buzz-relay -p buzz-cli -p buzz-acp --all-targets -- -D warnings
cargo fmt --all -- --check
```

阶段一只证明 `Create → Get Current Board`。它不是可投入实际运行的完整会议生命周期；创建
开关仍须保持关闭，仅可在可丢弃的隔离测试环境中临时开启。Board Maintenance、Floor、
speech、close/abort、恢复及 ACP Turn 必须等阶段二及后续阶段交付。

### 14.2 阶段二交付记录（2026-08-02）

阶段二已经冻结并实现：

- kind `42111` 主持人 Board Maintenance command，以及 `update|unchanged`、
  `expected-control-epoch` 和 `board-window` fencing；
- migration `0044_meeting_v2_stage2.sql` 中的 V2 冻结 timing、Board runtime、command
  receipt 与 `closed|aborted` 终态分类；
- V2 对 V1 Baton 的协议隔离复用，包括 Intent、Human Request、Offer、Grant、speech、
  Yield、Recall、fallback 和 Directed Handoff；
- `board_pending → floor_ready` 权威 gate，Board timeout 与 Floor Decision 各自取得完整且
  独立的数据库 deadline；
- Human Request 抢占 Board、Directed Handoff 跳过 Board、Control return 重开 Board，以及
  迟到和重复命令收敛；
- Board 更新正文的 pull-only 单投影替换，不进入 Meeting outbox、State 正文或 speech
  timeline；初始正文仍按阶段一 wire 包含在 Create；
- 主持人 normal close、主持人/Community operator abort，以及重复终止返回已持久化真实结果；
- CLI Board update/unchanged、完整 V2 Baton 操作、close/abort 和终态查询；
- 关闭 V1/V2 Create gate 并重启 Relay 后，已有 Session 仍可继续 Board/Floor 并结束。

协议、状态、并发与错误语义见
[阶段二权威控制周期与终态设计](./meeting-v2-stage2-design.md)。自动化证据覆盖 SDK wire
fixture、阶段一 V2 lazy upgrade、Board update/unchanged/timeout、双 deadline、Human 抢占、
Directed Handoff、正常 close、operator abort、终态幂等、pull-only Board、真实 CLI 多身份
生命周期、Relay 重启、关闭新建、撤权和 V0/V1 回归。

阶段交付时通过以下可复现门禁：

```bash
./scripts/run-meeting-backend-tests.sh
cargo clippy -p buzz-core -p buzz-sdk -p buzz-db -p buzz-relay -p buzz-cli -p buzz-acp -p buzz-test-client --all-targets -- -D warnings
cargo fmt --all -- --check
```

阶段二完成的是不依赖 ACP 的完整后端生命周期。V2 Create 默认开关仍保持关闭；普通参会
Agent 的按需看板注入属于阶段三，ACP Agent 主持人的 Board/Floor 双 Turn 属于阶段四，
前端仍不在本计划范围内。

### 14.3 阶段三交付记录（2026-08-02）

阶段三已经冻结并实现：

- ACP 识别 `v=3 + moderated-board-v1` Session，并由 V1/V2 共用的 Baton coordinator 按
  Session 持久协议严格分流 State、history 与 SDK builder；
- V2 Participant Intent 和 Granted Speech 在各自模型 Turn 前独立查询当前 Board，Grant
  不复用 Intent 快照，format retry 也重新读取；
- Board loader 固定 Relay signer、主持人、`h/v/policy/format` 与严格 content envelope，
  正文采用 UTF-8 安全的有界 head/tail 截断；
- Board 正文仅进入当前 Prompt，不进入 subscription、ACP ledger、observer payload、普通
  日志或跨 Turn cache；ledger v5 只增加 Baton protocol discriminator，v4 缺省迁移为 V1；
- Board 作为 `untrusted_meeting_context`，不能覆盖系统策略、Agent 身份、Grant、输出
  schema、工具权限或外部授权；普通参会 Turn 不执行持久写入；
- 读取使用独立短超时和三次有界尝试；最终失败不启动模型，Intent 私有 PASS，Granted
  Speech 提交 `YIELD(unable_to_answer)`；
- Board load/待派发 Turn 使用短暂容量保护，同时服从更强的 Offer/Grant reservation；若
  dispatch 未取得 Agent，会丢弃刚读快照并在下次出队重新读取；
- Offer ACK、Intent、Progress、SAY/YIELD 和 Directed Handoff 均按 V2 `v=3` wire 构建；
- Board 更新不在 Meeting subscription 中，单纯 Board/State 推进不会形成新的语义 Turn；
- 本地 ACP 身份为 V2 主持人时保持 fail closed，不借用 V1 主持路径；Board Maintenance
  Turn、Floor Decision Turn 和 Agent 闭会仍属于阶段四。

协议、调度、Prompt、失败和恢复边界见
[阶段三普通参会 Agent 看板上下文设计](./meeting-v2-stage3-design.md)。自动化证据覆盖看板
A Intent → 看板 B Granted Speech、读取失败 PASS/YIELD、V2 wire、截断与注入防线、排队
重读、restart、迟到 epoch、Board-only State、容量隔离、本地主持 fail closed，以及完整
`buzz-acp` V0/V1 regression。

阶段交付时通过以下可复现门禁：

```bash
cargo test -p buzz-acp --lib
cargo clippy -p buzz-acp --all-targets -- -D warnings
./scripts/run-meeting-backend-tests.sh
cargo fmt --all -- --check
```

阶段三完成的是普通参会 Agent。它没有实现 ACP Agent 主持，也没有增加前端、会议模板、
Project View 绑定或任何外部系统写回。这里描述的是阶段三交付时的边界；ACP Agent 主持的
fail-closed 限制已经由阶段四解除。

### 14.4 阶段四交付记录（2026-08-02）

阶段四已经冻结并实现：

- ACP 将主持控制机会拆成独立 `Moderator Board Maintenance` 与 `Moderator Floor Decision`
  模型 Turn，并使用不同的本地记录、Prompt、current-board read 和 hard deadline；
- Board Turn 读取协议允许的完整当前 Board，严格输出完整替换 `UPDATE` 或显式
  `UNCHANGED`，但读取、provider、格式或构建失败绝不伪造 unchanged；
- ACP 等待 Relay 将 Board window 收敛为 `floor_ready`，再执行新的权威 Board query，
  Floor 不复用 Board Turn 的输入快照；
- Board Harness deadline 在 Relay deadline 前保留动态安全余量，同一 window 的重复同步
  不得向后延长该边界；绝对 hard deadline 和 Board-reserved pool slot 保证卡住的 Board
  provider 不侵占 Floor 的独立预算；
- Human priority、Offer/Grant、End 或 control/window 变化会取消旧主持 Turn，迟到 Board 或
  Floor 结果无法提交；无候选 Floor 运行期间出现新候选时也立即取消并释放 slot；
- 有候选 Floor 复用 V1 的 Relay-frozen Candidate Cohort、Decision Attempt、CAS、
  Offer/ACK/Grant、Handoff、fallback 和 self Intent 规则，所有命令按 V2 `v=3` wire 构建；
- 无候选 Floor 只允许 `IDLE | CLOSE | ABORT`；idle 在同一 window 不热循环，主持人如需发言
  必须先形成 self Intent 并重新进入 Cohort；
- normal close 只允许显式 `updated|unchanged` Board，`timed_out|preempted` 不能被解释为主持
  确认；主动 abort 使用固定低基数 reason-code allowlist，并生成可区分的 `aborted` End；
- ledger v6 恢复未签名的 Board/Floor Turn，并保留、核验和精确重放已经签名的 Board、
  moderator 或 End command；current-board 读取快照不进入 ledger、日志、observer 或跨 Turn
  cache。为精确重放，未决签名 `UPDATE` 命令会在 `0600` 本地 ledger 中暂存完整事件（因此
  暂含 replacement Board 正文），并在 Relay State 前进、终态或撤权后清除；
- V2 主持使用独立系统 Prompt，会议内容只作不可信 evidence，工具保持 advisory read，不能
  写 Project View、Work、Issue、代码、Git 或其他外部系统；
- Meeting 后端 gate 支持显式隔离 Relay、health、metrics 端口和 Redis URL，默认端口行为
  保持不变，可在已有开发 Relay 运行时使用独立数据库与 Redis 完成真实 E2E；
- 阶段四收口补齐主持人 self Speech 的确定性控制器路径：`IDLE → self Intent →
  Candidate Floor → Offer/ACK → Grant → Speech`，各语义 Turn 独立读取当前 Board；同时修正
  candidate Floor Prompt 的 `turn_kind`、`board_control` 和 normal-close 判断信息；
- 确定性的三 Agent 多轮协议轨迹覆盖两名参会 Agent 依次发言、主持多次更新 Board、最终
  close；独立测试覆盖主持主动 abort。

主持 Agent 的控制、Prompt、安全、容量、恢复和验收边界见
[阶段四主持 Agent 双 Turn 设计](./meeting-v2-stage4-design.md)。阶段交付时通过以下可复现
门禁：

```bash
cargo test -p buzz-acp --lib
cargo clippy -p buzz-acp --all-targets -- -D warnings
./scripts/run-meeting-backend-tests.sh
cargo clippy -p buzz-core -p buzz-sdk -p buzz-db -p buzz-relay -p buzz-cli -p buzz-acp -p buzz-test-client --all-targets -- -D warnings
cargo fmt --all -- --check
```

阶段四完成的是 ACP Agent 主持控制器及确定性后端证据。它没有增加前端、模板、Project
View 绑定或外部效果，也不把确定性 wire trace 冒充真实模型 qualification。真实 Agent
矩阵、混合场景、fleet capability、SLO、灰度与发布候选仍属于阶段五。

### 14.5 阶段五交付记录（已完成，2026-08-03）

阶段五已经完成发布级确定性门禁、真实 provider qualification 与运维收口：

- V0/V1/V2 统一 Meeting backend gate，包括真实 Relay + CLI E2E、fresh/upgrade/concurrent
  migration、Meeting schema drift、Create 关闭排空、安全撤权和 qualification verifier
  正反 fixture；
- Relay NIP-11 runtime/create capability、与 active V2 和稳定 signer 联动的 readiness；
- ACP 机器可读 capability，明确 V2 participant/moderator Turn、每 Turn 权威读板、双 deadline
  与 ledger generation；
- V2 current Board read、Board command、End，以及共享 Baton command/recovery/outbox/worker
  的低基数可区分指标；
- 真实 provider 证据契约与硬门禁：核心证据必须完整纳入哈希，workspace 快照必须由 verifier
  独立比对；
- 真实 `@agentclientprotocol/codex-acp 1.1.7` qualification runner，使用模型目录确认的
  `gpt-5.6-sol`，主持 Agent 使用 `max`、参会 Agent 使用 `high`，共实际运行八个 Agent
  Session；
- Create 开启时原子创建四场会议，随后重启同一 Relay 并关闭 Create；存量 mixed、all-agent、
  moderator abort 与 admin/security abort 场景均在 runtime capability 保留时完成；
- mixed 场景覆盖两名 Human、两名 Agent、Human 抢占 Board、Directed Handoff、主持人 self
  Speech、三次 Board 更新与四名不同 speaker，最终正常 closed；
- all-agent 场景覆盖三名 Agent、多轮 Board/Floor、Directed Handoff、主持人 self Speech、三次
  Board 更新与三名不同 speaker，最终正常 closed；
- 主持 Agent 以 `unable_to_form_conclusion` 主动 aborted；安全撤权以
  `participant_revoked` 独立 aborted；
- verifier 的十八项 gate 全部 PASS，十项硬不变量全部为零，非参会者读写、关闭 Create 后
  新建以及 End 后写入均被拒绝；observer 证据不包含 Board、Speech、Prompt、模型原始输出或
  原始错误正文；
- qualification 期间补齐权威状态抢占迟到 Board 结果时的单次
  `meeting_v2_host_turn_discarded` 证据，并增加 Human preemption 的确定性端到端测试；
- 发布顺序、SLO、最小查询、故障处置、关闭新建、排空和前向兼容回滚手册。

真实运行与复核结果见
[Meeting V2 阶段五真实 Agent Qualification 报告](./meeting-v2-qualification-report.md)。阶段
收口通过 `just test-meeting-backend`、`RUST_TEST_THREADS=1 just ci`，以及一次性隔离数据库下
的 `just test`。初次直接运行 `just test` 遇到共享开发库已有 migration 32 校验和来自另一
代码状态；没有改写该数据库，而是在全新数据库重跑并通过。

## 15. 后端完成定义

Meeting V2 后端完成需要同时满足：

1. 可以在独立灰度开关下新建 V2，且既有 V0/V1 行为不变；
2. V2 创建者自动成为 owner 和 moderator；
3. Create、固定名单、初始看板和初始程序状态原子一致；
4. 每场 V2 会议始终有且只有一份可按需读取的当前看板；
5. 主持人可修改，普通参会者只读，非参会者不可读；
6. 调用者不需要看板业务版本、历史同步或变更通知；
7. 主持人的 Board Maintenance 先于其 Floor Decision；
8. Board Maintenance 与 Floor Decision 时间预算完全独立；
9. Board timeout 保持原看板，并为 Floor Decision 提供完整预算；
10. Human Floor Request 和 Directed Handoff 继续遵循 V1；
11. 控制权丢失、Human 抢占或 End 后的迟到 Board 结果不能生效；
12. Agent 在 Intent、Granted Speech、Board Maintenance 和 Floor Decision 前分别读取当前
    看板；
13. 看板读取失败不会让旧副本冒充当前上下文；
14. Board 更新不会单独唤醒 Agent；
15. 主持 Agent 能维护看板、安排发言、idle、正常 close 和主动 abort；
16. 正常 closed 与异常 aborted 在后端结果中可区分；
17. 看板和闭会不会隐式修改 Project View 或其他外部系统；
18. Relay/ACP 重启、网络重试、deadline recovery 和多 worker 不会产生重复更新、重复
    Floor Decision 或会议复活；
19. migration、protocol、DB、Relay、CLI、ACP、V0/V1 regression 和真实 Agent
    qualification 全部通过；
20. 创建开关关闭后已有 V2 可以继续、恢复并结束；
21. 发布、观测、故障处置、回滚和已知边界文档完成。

## 16. 留到后续阶段开发时讨论的细节

阶段一、阶段二的 wire、持久状态、Board/Floor gate、终态和 CLI 契约已经由对应阶段设计
冻结。为避免本计划过早固化后续实现，以下问题只在对应阶段开始时决定：

- ACP Prompt 全文、context budget 和模型输出 schema；
- ACP Turn capacity、抢占和 safety margin 的精确策略；
- metric 名称、告警阈值和 acceptance artifact 格式；
- PR 数量、代码模块拆分和每个阶段的具体开发顺序。

这些细节必须遵守产品 spec 和本文列出的不变量；若阶段开发发现必须改变产品语义，应先
修改并重新确认 spec，而不是在实现中静默偏离。

## 17. 不纳入后端完成判断

Meeting V2 后端完成不要求：

- 任何 Desktop、Web 或 Mobile 代码；
- 前端截图或 Playwright/Flutter UI 验收；
- 看板实时刷新或协同编辑；
- 看板历史浏览；
- 会议模板；
- 主持权转移；
- 投票或正式治理程序；
- Project View 集成可用；
- 外部效果 adapter；
- 模型能够判断结论在业务上绝对正确。

这些能力如需开发，应在 V2 后端稳定后分别建立新的产品设计和实现计划。
