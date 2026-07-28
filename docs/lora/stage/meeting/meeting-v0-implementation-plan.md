# Meeting V0 分阶段开发计划

> 状态：阶段 1–3 已完成，阶段 4–5 未开始
>
> 本文只规划 Meeting V0 的开发阶段、阶段交付物和完成条件。
> 产品与协议语义以 [Meeting V0 设计](./meeting-v0.md)为准；具体代码方案在进入对应阶段
> 时讨论并形成该阶段的实现任务。

## 1. 计划目标

本计划用于把 Meeting V0 拆成能够独立开发、审查和验收的纵向阶段，最终交付一个可由
Human 与 Agent 共同使用的共享文字会议室。

计划不提前固定每个模块的代码结构、数据库实现或 PR 拆分。每个阶段只回答：

1. 这一阶段解决什么问题；
2. 完成后可以运行和验证什么；
3. 以什么成果作为阶段交付。

## 2. 交付原则

- 先稳定 Relay 权威协议，再接入 Agent 和 Desktop；
- 每个阶段都要形成可运行场景和自动化测试，不以“代码已经写了一部分”作为交付；
- 每个阶段可以由一个或多个可审查 PR 完成，避免把全部 V0 堆在一个大型 PR 中；
- 后续阶段不得绕过前一阶段已经建立的权限、状态机和消息入口；
- 设计中尚未确定的实现细节，在开始对应阶段时讨论，不提前写入本计划；
- 阶段 1–4 的成果默认只用于开发和集成验证，不作为完整 Meeting V0 对用户启用；
- Project View 与 Meeting V0 没有实现依赖，可以继续并行开发。

## 3. 阶段关系

```text
阶段 1：会议身份与生命周期
                ↓
阶段 2：发言权与共享文字协议
                ↓
        ┌───────┴───────┐
        ↓               ↓
阶段 3：Agent 模式   阶段 4：Desktop
        └───────┬───────┘
                ↓
阶段 5：集成验收与发布
```

阶段 3 和阶段 4 都以阶段 2 的版本化协议基线、CLI 行为和跨客户端一致性测试夹具为共同
输入，之后可以并行开发。阶段 3 增加的 Ready/Pass/Yield 与提前仲裁是对 Floor 控制面的
加法扩展，不改变 Desktop 已经依赖的 Meeting、Claim、Grant 和 kind `9` speech 主路径。

## 4. 分阶段计划

### 阶段 1：会议身份与生命周期基座

目标：

> 让 Buzz 能够可靠地区分普通 Channel 与 MeetingSession，并完成会议的创建、发现、名单
> 读取和结束。

本阶段做什么：

- 建立 Meeting V0 的协议、SDK 和持久化基线；
- 实现私有会议室、固定参会者名单和会议生命周期；
- 提供创建、查询、列出参会者和结束会议的 CLI 操作；
- 建立会议权限隔离和归档只读规则。

阶段交付：

- 一组可独立审查的协议、Relay、SDK、CLI 和迁移 PR；
- CLI 可创建会议、让所有参会者读取相同名单，并将会议结束为只读状态；
- 生命周期、名单原子性和非参会者隔离的自动化测试通过。

完成标志：

> 不依赖 Desktop 或 Agent，多个测试身份已经能够通过 CLI 验证同一个 MeetingSession
> 的身份、名单和生命周期。

### 阶段 2：发言权与共享文字协议

目标：

> 完成 Meeting V0 的核心证明：所有参会者都能申请发言权，但每轮只有抢夺成功者可以
> 向共享时间线写入一条消息。

本阶段做什么：

- 实现 SpeechRound、Claim、Grant、租约和下一轮推进；
- 实现 Grant-bound 文字消息、共享历史和当前 Floor 状态读取；
- 完成 Relay 重启恢复、幂等重试和并发写入保护；
- 补齐全部 Agent-facing Floor CLI 操作。

阶段交付：

- CLI-only 的完整会议文字链路；
- 多个测试身份可以同时 Claim、看到唯一 winner、由 winner 发言并进入下一轮；
- 一份版本化协议基线，以及供 Agent 与 Desktop 共用的正常、lost、expired 和重连
  一致性测试夹具；
- 发言权竞争、Grant 校验、超时、重启和双写保护的自动化测试通过。

完成标志：

> 不依赖 Agent Harness 或 Desktop，Meeting V0 的房间、名单、抢夺、发言、历史和结束已经
> 可以通过 CLI 端到端运行。

### 阶段 3：Agent 会议模式

目标：

> 让真实 Agent 作为参会者持续观察会议，并能在没有被点名时自主决定 `CLAIM` 或
> `PASS`。

本阶段做什么：

- 让 ACP/Harness 发现会议、同步完整上下文并跟踪当前 Floor；
- 接入 Ready、持久 Speech Intent、`CLAIM/PASS`、Claim 调度和 Grant-bound 发送路径；
- 让 Claim 竞争在 Agent 全部完成决定后提前仲裁，以 5 分钟作为最长等待而非固定等待；
- 只让 winner 在最长 5 分钟 Grant 内使用只读工具生成完整发言，成功发言或 Yield 时
  立即换轮；
- 提供 Meeting 专用上下文、提示词协议、严格结构化输出和工具权限边界；
- 保证 Agent 重启、重复事件和 Floor 状态变化不会造成重复发言；
- 让 Agent 与 Human、其他 Agent 使用同一份名单和消息记录。

阶段交付：

- [阶段 3 Agent 会议模式设计](./meeting-v0-stage3-agent-mode.md)；
- Floor Signal、提前仲裁、真实 ACP 与 Agent Harness 的会议模式 PR；
- Human 发言、Agent 主动 Claim、Agent PASS 和 Agent 获权发言的可运行演示；
- Agent 历史补齐、重启恢复、意图去重、只读工具、提前 Say/Yield 和专用发送路径的
  自动化测试通过。

完成标志：

> 至少一个真实 Agent 能在会议中可靠地观察、保持沉默或申请发言，获权后能读取项目
> 上下文形成发言，并且无法绕过 Grant 或工具权限直接写入会议消息或修改项目。

### 阶段 4：Desktop 会议产品面

目标：

> 让 Human 不依赖 CLI 即可完成 Meeting V0 的全部日常操作。

本阶段做什么：

- 提供会议创建入口、会议列表、会议室和参会者面板；
- 展示轮次、抢夺状态、winner、租约和消息作者；
- 提供草稿、申请发言权、获权发送、失败重试和结束会议的交互；
- 处理重连恢复、状态乱序和会议结束后的只读显示。

阶段交付：

- 可操作的 Desktop Meeting V0 界面；
- Human 可以从 Desktop 与 CLI 测试身份创建会议、参与抢夺、发言、查看完整记录并结束
  会议；
- Desktop E2E 覆盖主要 Floor 状态、重连和归档场景。

完成标志：

> Human 已经可以仅通过 Desktop 完成自身的全部会议操作；与真实 Agent 的组合验收留到
> 阶段 5。

### 阶段 5：集成验收与发布

目标：

> 将前面各阶段收敛为可以启用和验证的 Meeting V0 交付版本。

本阶段做什么：

- 完成两个独立 Human 身份/客户端会话与两个真实 ACP Agent 实例的完整端到端场景；
- 集中验证权限、断线重连、Relay/ACP/Desktop 重启、并发和会议结束边界；
- 补齐运行观测、启用方式、故障处置和回滚说明；
- 完成发布前质量检查，并清理只适用于开发阶段的临时路径。

阶段交付：

- Meeting V0 集成测试和真实运行验证报告；
- 至少一次 `2 Human + 2 real ACP Agent` 的真实 smoke 记录；
- 全量质量门禁通过；
- 面向开发者和操作者的启用、验证与回滚说明；
- 一个满足设计文档验收标准的 V0 发布候选。

完成标志：

> [Meeting V0 设计的验收标准](./meeting-v0.md#12-验收标准)全部通过，真实
> `2 Human + 2 real ACP Agent` smoke 成功，不存在发布阻断问题，Meeting V0 可以按
> 既定发布流程启用。

## 5. 阶段状态

| 阶段 | 状态 | 主要交付证据 |
|---|---|---|
| 1. 会议身份与生命周期 | 已完成 | `e2e_meeting`、数据库事务测试、双身份 CLI 生命周期演示 |
| 2. 发言权与共享文字协议 | 已完成 | `e2e_meeting_floor`、数据库重启/原子性测试、共享协议夹具、双身份 CLI 抢夺与发言演示 |
| 3. Agent 会议模式 | 已完成 | `meeting-v0-stage3-agent-mode.md`、Ready/Pass/Yield 与提前仲裁 E2E、真实 ACP Agent CLAIM/SAY 与 PASS 演示 |
| 4. Desktop 会议产品面 | 未开始 | Desktop 可操作流程与 E2E |
| 5. 集成验收与发布 | 未开始 | 2 Human + 2 real ACP Agent 验收报告与发布候选 |

每个阶段开始时，把状态更新为“进行中”；交付物和完成标志均满足后，更新为“已完成”，
并在本表中补充对应 PR、测试或验证记录链接。

## 6. 阶段 1 交付记录

阶段 1 已于 2026-07-27 完成交付，形成以下可运行基线：

- 协议登记 kind `42100–42103`，阶段 1 启用 Meeting Create 与 Meeting End，并将
  Round State 登记为 Relay-only；
- 增加稳定的 `room_kind=meeting` Channel 投影和 `meeting_sessions` 生命周期表；
- 在同一数据库事务中提交 Meeting Create 事件、私有 Stream Channel、完整固定名单和
  `active` 投影；无效名单会整体回滚；
- 在同一数据库事务中提交 Meeting End 事件、`ended` 投影和 Channel 归档；重复结束
  幂等且不产生第二条 End 事件；
- 名单角色由 Relay 权威判定：创建者为 `owner`，Human 为 `member`，Agent 为 `bot`；
- 通用加人、移除、加入、离开、角色变更、归档、解归档、删除房间和删除会议历史均被
  Meeting policy 拒绝；
- 私有 Channel 访问边界继续覆盖 Query、订阅、Count 和 Search；归档后原参会者仍可
  读取，非参会者仍不可发现或读取；
- 提供 `buzz meetings create|list|show|participants|end` 五个 CLI 操作；
- 启动 reconciliation 会检查完整的 `39000/39001/39002` 发现投影集合，进程若在
  提交后通知期间停止，重启扫描仍可恢复会议发现和名单读取。

迁移使用 `0026_meeting_v0_lifecycle.sql`。`0025` 已由并行开发的 Project View 使用，
两项工作因此不会复用同一个已发布迁移版本。

自动化验证：

```text
just ci
# passed

BUZZ_TEST_DATABASE_URL=postgres://.../buzz_meeting_v0_test \
  cargo test -p buzz-db meeting::tests -- --ignored --test-threads=1
# 2 passed

DATABASE_URL=postgres://.../buzz_meeting_v0_test RELAY_URL=ws://localhost:3000 \
  cargo test -p buzz-test-client --test e2e_meeting -- --ignored
# 1 passed
```

`e2e_meeting` 覆盖三名参会者（包含 Agent）名单一致、第四名非参会者不可见、固定名单、
结束归档、归档只读、禁止解归档、重复 End 幂等，以及无效创建的事件/房间/名单/投影
四项零残留。

CLI 实机验证使用两个独立身份完成：

```text
buzz meetings create --title "CLI Stage One" --participant <PUBKEY>
buzz --format compact meetings list
buzz meetings show --meeting <UUID>
buzz meetings participants --meeting <UUID>
buzz meetings end --meeting <UUID>
buzz meetings show --meeting <UUID>
```

两个身份读取到相同的 `owner + member` 名单；结束前状态为 `active`，结束后为 `ended`，
原参会者仍可在 `--include-ended` 列表中发现并打开会议。

阶段 1 只交付会议身份、名单和生命周期基座。它尚未启用 Claim、Grant、轮次或规范会议
发言；这些能力属于阶段 2，因此阶段 1 完成不等于 Meeting V0 已可面向用户启用。

## 7. 阶段 2 交付记录

阶段 2 已于 2026-07-27 完成交付，形成以下 CLI-only 共享文字会议链路：

- 启用 `42102` Claim；`42103` Round State 仍只允许 Relay 签发；
- 建立 `open → claiming → granted → closed/expired → next open` 的持久化 Floor
  状态机；阶段 2 交付时默认 Claim 窗口为 3 秒、Grant 租约为 10 秒，选择策略版本为
  `uniform-v0`。阶段 3 已将其扩展为最短 3 秒、最长 5 分钟的事件驱动 Claim 窗口和
  最长 5 分钟 Grant；
- 每名参会者每轮只能提交一个规范 Claim，同一事件重试幂等，第二个不同 Claim 会被
  拒绝；每轮最多生成一个 winner 和一个 Grant；
- 会议发言复用 kind `9`，但必须绑定当前 `round + grant`；允许规范 `p` mention，
  禁止 `e`、`q` 和 thread/reply 标签，非 holder、过期 Grant 和重复消费均被拒绝；
- Claim、Round State、发言和发言权推进在数据库事务中提交，并通过持久 outbox
  分发；Relay 重启后会从数据库时间和已提交状态恢复过期窗口、租约与待分发事件；
- 会议结束会把 Floor 收敛到 `closed/ended`，之后不能再 Claim 或发言；
- `buzz meetings` 扩展为
  `create|list|show|participants|history|say|floor|end`，其中 Floor 提供
  `status|history|claim`，Claim 支持等待并返回 `won|lost|ended`；
- 增加版本化共享夹具
  `crates/buzz-test-client/fixtures/meeting_v0_floor_v1.json`，覆盖
  `normal`、`lost`、`expired` 和乱序/缺口/重复事件下的 `reconnect`。

迁移使用 `0027_meeting_v0_floor.sql`，新增轮次、Claim 和持久 outbox 表，并给
`meeting_sessions` 增加当前轮次、Floor revision 和策略版本投影。

自动化验证：

```text
just ci
# passed

BUZZ_TEST_DATABASE_URL=postgres://.../buzz_meeting_v0_stage2_test \
  cargo test -p buzz-db \
  meeting_floor::tests::claim_grant_say_expiry_restart_and_idempotency_are_atomic \
  -- --ignored --nocapture --test-threads=1
# 1 passed

DATABASE_URL=postgres://.../buzz_meeting_v0_stage2_test RELAY_URL=ws://localhost:3000 \
  cargo test -p buzz-test-client \
  --test e2e_meeting --test e2e_meeting_floor \
  -- --ignored --nocapture --test-threads=1
# stage 1: 1 passed; stage 2: 1 passed
```

`e2e_meeting_floor` 使用两名 Human 与两名 Agent 的固定名单，覆盖四方并发 Claim、
唯一 Grant、同事件重试、重复 Claim 冲突、非 winner 发言、非法 thread 标签、合法
`p` mention、Grant 单次消费、窗口/租约超时、下一轮推进、伪造 Relay 状态、非参会者
隔离、结束后拒绝，以及所有参会者观察到相同且单调连续的 Floor revision。

数据库原子性测试额外覆盖并发双写和在 `claiming`、`granted` 两个阶段断开并重建
数据库连接后的恢复。共享夹具的版本与四类场景也由非 ignored 测试固定。

CLI 实机验证使用两个独立身份完成：两者并发 Claim 后看到同一个 winner；首轮
Grant 主动过期后，另一身份在下一轮获权并通过 `meetings say` 写入
`The CLI-only shared floor works.`；两个身份读取到相同作者和历史，最后结束会议并
共同观察到 `closed/ended`。

本阶段尚未把真实 ACP Agent 或 Desktop 接入会议。它们分别属于阶段 3 和阶段 4，
但现在可以共同基于本阶段的 Relay 权威协议、CLI 行为和版本化夹具并行开发。

## 8. 阶段 3 交付记录

阶段 3 已于 2026-07-28 完成交付，真实 ACP Agent 已接入阶段 2 的共享会议协议：

- 新增 participant-signed kind `42104`，提供 `ready`、`pass`、`yield` 三种 Floor
  Signal；SDK 与 CLI 提供 `meetings floor ready|pass|yield|status`；
- 新增 `meeting_floor_signals` 和 `meeting_round_decision_cohort` 持久投影。第一份
  Claim 冻结已 Ready 的 Agent cohort；最短 3 秒后，cohort 全部完成 Claim/Pass 即
  提前仲裁，最长窗口和 Grant lease 均为 5 分钟；
- ACP 增加独立 `MeetingCoordinator`，自动发现活动会议，分页补齐完整会议历史，验证
  签名并按 event ID 去重，不把 Meeting 事件送入普通 mention/reply 队列；
- Intent Turn 只输出严格的 `CLAIM/PASS` 决定；只有 winner 才运行 Granted Turn 并
  生成完整 speech。结构化输出仅允许一次格式纠正，第二次失败会安全地 Pass/Yield；
- Agent 私有持久账本记录 basis、Intent、Claim、Grant 和预签名事件。进程恢复时复用
  原逻辑记录和同一 signed event，不重新生成内容，也不重复 Claim 或 speech；
- Intent 使用 Relay Claim deadline 与本地 5 分钟上限的较早者；Granted Turn 使用
  lease 减 30 秒安全余量。绝对 deadline 从排队和会话建立阶段开始扣减；
- Meeting Turn 强制 ACP `Plan` 模式，只挂载 allowlist 中的 `buzz-dev-mcp`，并由 MCP
  再次拒绝 shell、文件替换和 todo 写操作。正式发言只能经过专用 Meeting sender，
  Relay 仍以当前 round、holder、Grant 和单次消费作最终授权；
- Ready/Pass、speech 和 Yield 一旦被接受就立即推进，不会固定等待完整 5 分钟；
- 迁移使用 `0028_meeting_v0_agent_floor.sql`，共享协议夹具同步登记 kind `42104`。

自动化验证：

```text
just ci
# passed

just test
# 8 groups passed

cargo test -p buzz-db meeting_floor::tests:: \
  -- --ignored --nocapture --test-threads=1
# 2 passed

cargo test -p buzz-test-client --test e2e_meeting_floor \
  -- --ignored --nocapture --test-threads=1
# 2 passed
```

数据库测试覆盖 Ready cohort、提前结算、唯一 Grant、Say、Yield、过期、重启恢复和幂等；
Relay E2E 覆盖 Agent Ready/Pass 后提前仲裁、Yield 立即换轮，以及既有四方竞争和共享
历史路径。E2E 在观察到有效 Grant 后只推进测试投影的 lease 来验证过期，因此无需把
生产 5 分钟配置缩短为测试值。

真实 Agent 验证使用 `claude-agent-acp`：

- 在会议 `77b68261-57f3-47db-ac89-ffcb4ea4c9c9` 中，Agent 自动同步上下文、读取
  `docs/lora/project-positioning.md`、选择 CLAIM、获权，并在约 13 秒内提交 speech
  `d91f8cb4e6204985eb48b10856e94f060875aa98e746d0170718c87ed0f13afb`；
- 数据库核验该 speech 只有规范的 `h`、`meeting-round=4` 和 `meeting-grant` 绑定，
  没有 `e`、`q` 或 thread 标签；提交后立即进入下一轮；
- 在空会议 `0bd057ab-14ee-49c2-8d6a-68238888a2d6` 中，同一 Agent 自动选择 PASS。
  Floor 记录包含 Ready/Pass，但没有 Claim、holder 或公开 speech。

阶段 3 已证明 Agent 能自动观察、主动沉默或争取发言，并在获权后使用只读工具形成正式
发言。Desktop 产品面和 `2 Human + 2 real ACP Agent` 的发布级组合验收仍分别属于阶段
4、阶段 5，因此阶段 3 完成不等于 Meeting V0 已面向用户启用。

## 9. V0 总体完成条件

Meeting V0 只有在以下结果同时成立时才算完成：

- Human 与 Agent 可以进入同一个私有会议室并读取相同参会者名单；
- 所有参会者都可以申请发言权，但每轮最多一个 holder 和一条规范发言；
- 所有参会者最终看到相同消息、作者和 Floor 状态；
- Agent 可以主动 `CLAIM`，也可以产生可恢复的 `PASS`；
- Agent 完成决定或发言时立即推进，5 分钟 Claim/Grant 只作为故障上限；
- 只有 winner 生成完整发言，并且会议 Turn 只允许读取上下文而不执行项目修改；
- 断线和进程重启不会造成记录缺失、重复发言或发言权分叉；
- 非参会者不能发现、读取或写入会议；
- Desktop、CLI 和 ACP 使用同一套 Relay 权威协议；
- 会议结束后不可继续 Claim 或发言，原参会者仍可读取归档记录。

## 10. 不纳入本计划

以下能力不进入 Meeting V0 的任何开发阶段：

- 音频、视频、TTS、STT 和录音；
- 候选决议、投票、人类确认和决定写回；
- 动态增删参会者和主持权转移；
- 连续发言优先、身份权重、Leader 权重和 Human 直接获权；
- 自动纪要、行动项和 Project View 自动更新；
- Project View 本身的实现。

这些能力需要在 Meeting V0 被真实使用后，根据新的设计和开发计划单独推进。
