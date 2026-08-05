# Meeting V2 真实验收缺陷清单

> 状态：已确认，待分阶段修复。
>
> 记录日期：2026-08-05。
>
> 范围：Meeting 创建与容量、ACP Agent 上下文和工具边界、Meeting Desktop 与
> [`meeting-desktop-spec.md`](../desktop/meeting-desktop-spec.md) 的差距，以及相应真实验收。
>
> 本文是统一缺陷清单，不在此冻结完整实现细节。每项进入开发阶段时再讨论具体协议、组件、
> 工具隔离和迁移方式。

## 1. 背景与结论

一次真实使用中，Agent 为 1 名 Human 和 5 名 Agent 组织“SkillHub 下一步规划会”。因为当前
Meeting 最多允许 4 个 Agent，发起 Agent 没有创建 Meeting V2，而是创建了一个普通 private
stream Channel，并使用普通消息、Thread 和 Channel Canvas 模拟会议。

截图中的房间为：

```text
f9a7dc47-b559-4d75-9ab4-11227ff63fbf
```

只读数据库核对结果为：

- `channels.room_kind = standard`；
- `meeting_sessions` 中没有该房间；
- 该房间没有 kind `42100..=42112` Meeting 协议事件；
- 对应 Community 当时没有任何 Meeting Session。

因此截图中出现以下现象并不是一个真实 Meeting 被 Desktop 错误渲染，而是发起路径根本没有
进入 Meeting 协议：

- 房间出现在 `Channels`，`Meetings` 显示 `No active meetings`；
- 中央页面出现 `Create agent`、`Add people` 和普通 Channel composer；
- 对话通过普通 post、reply 和 Thread 进行，不是 canonical Speech；
- 右侧 `Canvas` 是 Channel Canvas，不是主持人维护的 Meeting Board；
- Human 没有 Human Floor Request、Offer、Grant 或 Speech 控件；
- Agent 没有受到 Meeting V2 Session System Contract 和逐 Turn Meeting envelope 的约束。

这次真实体验同时暴露了三个层面的缺陷：

1. 容量限制过低，并且失败后错误降级成了“看起来像会议”的普通 Channel；
2. Meeting Agent 的写入边界虽然已写入 System Contract，但表达和技术执行边界仍需补强；
3. 真正的 Meeting Desktop 已有主要生命周期 UI，但仍有若干与已确认 spec 不一致的展示缺口。

## 2. 本轮冻结的产品不变量

后续修复不得改变以下已确认设计：

1. 面向 Human 和 Agent 的正常产品入口只提供**当前最新、生命周期完整的 Meeting**。调用
   `buzz meetings create` 不要求发起者理解或选择 V0/V1/V2；当前内部 wire policy 为
   `v=3 + moderated-board-actions-v2`。V0/V1 等历史协议不得继续出现在默认值、CLI help 示例、
   Agent 稳定操作说明或 Desktop 创建界面中；如果为兼容、迁移或测试暂时保留，只能作为内部
   实现细节。
2. 单场 Meeting 最多允许 **8 个 Agent 身份**，Agent 主持人计入这 8 个；这里统计的是冻结
   roster 中的 Agent identity，不是某个 Agent 的并发槽数量。
3. 总 roster 上限暂时仍为 12，包含主持人、Human 和 Agent。因此合法组合可以包括
   `8 Agent + 4 Human`，但总人数不能超过 12。
4. 超过容量时必须明确失败并让发起者调整 roster；不得自动创建普通 Channel、普通 Thread、
   Huddle 或其他“模拟会议”作为降级结果。
5. Meeting 期间，Agent 可以按当前讨论需要进行小范围、权威的只读调查，例如读取代码、
   Project View、Document、消息、仓库状态或其他引用。
6. `participant_intent`、`granted_speech`、`board_maintenance` 和 `floor_decision` 不得产生持久
   外部写入，也不得直接执行会议讨论中刚形成的后续行动。
7. 参会 Agent 发现需要后续行动时，应在合法 Speech 中提出；主持人负责把已经形成的行动决定
   记录到 Meeting Board。
8. 只有主持人的 `action_finalization` Turn 可以依据精确冻结的最终 Board，使用原有业务工具
   直接登记行动产出。
9. Meeting Board、Project View 和 Channel Canvas 是三个不同对象；任何一个都不能因为界面或
   文案相似而冒充另一个。
10. Human 非主持参会者不提交 SpeechIntent。其发言路径是
   `Request → Offer → ACK → Grant → Speech/Yield`；Human 主持人的自发言才使用 self Intent。

## 3. 优先级定义

| 优先级 | 含义 |
|---|---|
| P0 | 会绕过 Meeting 协议、破坏阶段性写入边界或使真实会议无法成立；必须先修复 |
| P1 | 核心 Meeting 信息或操作不可见，导致 Human 无法按 spec 理解和使用会议 |
| P2 | 不阻断生命周期，但与已确认产品信息架构、文案或诊断边界不一致 |

## 4. 创建、容量与协议身份缺陷

### MFX-001（P0）：Agent “发起会议”可以退化为普通 Channel

**状态：代码修复完成，待真实 Agent 创建失败路径验收。**

**现状**

当 roster 超过当前 Agent 上限时，本次发起 Agent 创建了普通 Channel，并在其中使用普通消息、
Thread 和 Canvas 模拟会议。普通 Channel 标题或用途包含“会议”不会使它成为 Meeting，Desktop
也不能根据自然语言标题推断 `room_kind=meeting`。

**风险**

- 绕过 frozen roster、Board/Floor 顺序、Intent、Offer、Grant、Speech 和终态；
- 普通消息和 Thread 形成无发言权的旁路讨论；
- Channel Canvas 可能被误认为 Meeting Board；
- Meeting Agent System Contract 不会安装；
- 无法进入 action finalization，也无法得到 `closed | aborted` 的 Meeting 终态；
- Human 会把普通 composer 误认为提交 Intent 或正式 Speech 的入口。

**期望**

- Agent 在用户明确要求“发起会议/拉会”时只需使用正式的 `buzz meetings create`；
- 正常创建命令默认生成当前最新的完整 Meeting，内部固定为
  `v=3 + moderated-board-actions-v2`，不得要求 Agent 或 Human 选择协议版本；
- V0/V1 等历史协议不得继续作为当前产品能力向 Agent 或 Human 解释、推荐或展示；
- Create 被 Relay 拒绝时，Agent 向发起者说明稳定原因并请求调整 roster；
- 不允许用 `channels create`、普通 Thread 或 Canvas 作为透明降级；
- 如果用户明确只想建立普通协作频道，则仍可创建 Channel，但必须明确说明它不是 Meeting。

**关键修复面**

- Agent 的稳定 Project Space/System 操作说明需要加入“正式 Meeting 与普通 Channel 的创建
  区别”，并且只说明当前规范创建命令；
- `buzz meetings create` 的默认行为改为当前最新完整 Meeting；CLI help 只展示当前创建方式，
  删除 V0/V1 和无行动版 V2 的示例，不把 wire policy 版本选择暴露为正常使用决策；
- 旧协议如因兼容、迁移或自动化测试暂时保留，不得进入 Agent 稳定上下文、Desktop 创建界面或
  面向用户的当前操作文档；
- Create 失败返回需要保留可理解的容量错误，供 Agent 和 Desktop 展示；
- 增加真实 Agent 发起后对 `room_kind`、Meeting Session、Create event 和 Desktop 导航的
  端到端检查。

**验收**

1. Agent 收到“邀请这些身份开会”后创建 kind `42100` Meeting，而不是 kind `9007` Channel；
2. `buzz meetings create` 不带 `--policy` 时创建
   `v=3 + moderated-board-actions-v2`，Agent 不需要知道协议版本；
3. CLI help、Agent 稳定操作说明和 Desktop 创建面不再展示或推荐 V0/V1/无行动版 V2；
4. Relay 投影 `room_kind=meeting`，Desktop 只在 Meetings 分组展示；
5. 9 个 Agent 的请求明确失败，不产生任何普通 Channel；
6. 失败后只有在用户明确选择减少 roster 或改为普通频道时才继续下一动作。

### MFX-002（P0）：单场 Agent 上限仍为 4，应统一提高到 8

**状态：代码修复完成，待真实 8-Agent Meeting 验收。**

**现状**

`crates/buzz-db/src/meeting.rs` 中：

```rust
pub const MAX_MEETING_PARTICIPANTS: usize = 12;
pub const MAX_MEETING_AGENTS: usize = 4;
```

`MAX_MEETING_AGENTS` 被所有内部 Meeting 创建路径共同使用，因此它不是某个历史实现的局部
常量。Agent 的判断与 Relay 的实际拒绝一致。

**期望**

- `MAX_MEETING_AGENTS` 从 4 改为 8；
- 所有内部创建路径使用一致的 8-Agent 上限；
- 总 participant 上限继续为 12；
- Agent 主持人计入 Agent 数量；
- Desktop roster picker 在已知 participant type 时提前显示 8-Agent gate；
- Relay 继续做最终权威校验，不能只依赖 Desktop 或 CLI；
- CLI/Desktop/Relay 的错误文案均使用同一上限语义。

**需要覆盖的边界**

- `8 Agent + 0..4 Human` 且总人数 `2..=12`：允许；
- `9 Agent`：拒绝；
- `8 Agent + 5 Human`：按总人数 13 拒绝；
- Agent host + 7 Agent participant：Agent 总数为 8，允许；
- Human host + 8 Agent participant：Agent 总数为 8，只要总人数不超过 12 即允许；
- participant type 来自 Relay 权威身份，不由显示名、头像或 managed-by 文案猜测。

**测试与文档影响**

- DB 当前创建路径和保留兼容路径的边界测试；
- Relay Create command 测试；
- CLI Create 错误与帮助示例；
- Desktop roster/capability E2E；
- ACP 8-Agent 同场 Intent、Offer、Grant、Speech 和主持调度压力；
- 所有仍声明“最多 4 Agent”的 Meeting 设计、验收计划和报告需标注历史值或更新为当前值。

## 5. Agent System Context 与工具边界缺陷

### MFX-003（代码完成）：System Contract 已说明讨论阶段不得写入

**状态：代码修复完成，待真实 Provider 行为验收。既有禁止写入语义已保留，并在 Contract v2
中补齐正向只读与角色边界。**

当前 `crates/buzz-acp/src/meeting_context.rs` 的 V2 System Contract 已明确包含：

- Board 是主持人维护的目标、议程、进展、结论和 follow-up actions 共享记录；
- Board text 不是外部业务事实；
- 讨论和控制 Turn 的外部工具仅用于 `bounded read-only inspection`；
- 明确禁止 create、update、delete、publish、assign、commit、upload、send 等持久化动作；
- `Only the moderator's action_finalization Turn` 可以使用普通业务写工具；
- action finalization 只能执行最终冻结 Board 已有决定，不能发明第二份 Plan/Step；
- Project View 和其他外部引用是可选项。

逐 Turn envelope 也已有以下约束：

- `participant_intent` 和 `granted_speech` 仅允许有界只读调查，禁止外部写入和 Meeting event
  publishing；
- `board_maintenance` 仅额外允许通过输出 schema 返回完整 Board；
- `floor_decision` 仅允许有界只读调查；
- `action_finalization` 才切换到 `direct-business-actions-v2` 并允许业务写入。

因此“讨论阶段不得持久写、只有行动收口可写”的基本语义不是完全缺失。Contract v2 进一步
明确：只读限制针对外部工具和外部业务状态，不禁止 Agent 按当前输出 schema 提交 Intent、
Speech、Board 或 Floor 结果；Board Maintenance 是讨论阶段唯一的状态编辑例外，但 Board 仍由
Harness/Relay 发布，不因此获得外部业务写权限。

### MFX-004（P0，prompt-only 范围收口）：工具策略仍是 advisory

**状态：prompt-only 代码修复完成，待真实 Provider 行为验收。保留 `advisory-v1`，接受本阶段
不做确定性工具隔离的风险。**

**现状**

讨论阶段 envelope 使用 `tool_policy.mode = advisory-v1`。Agent 仍可能看到原本暴露的 Shell、CLI
或其他可写工具；System Prompt 告诉它不要写，但 Harness 没有从工具调用层证明写入必然被阻止。

**风险**

- 模型误操作、旧 Session 上下文、恶意 Board/Speech 或工具选择偏差可能在讨论阶段产生写入；
- “不授权持久效果”目前主要是行为契约，而不是可验证的能力边界；
- Agent 可能在 Speech 前直接完成本应由主持人记录并在 action finalization 物化的行动；
- Meeting 无法证明一次外部写入发生在合法阶段。

**本阶段决策**

- 本阶段只从稳定 System Contract 和逐 Turn envelope 约束 Agent 行为；
- 不裁剪 Harness 工具、不增加 Shell/CLI 命令分类，也不增加确定性写入拒绝；
- `participant_intent`、`granted_speech`、`board_maintenance` 和 `floor_decision` 保留
  `advisory-v1`，但明确只能进行有界只读调查；
- `board_maintenance` 仅允许主持人通过输出 schema 返回完整 Board，不授权外部业务写入；
- `action_finalization` 是唯一声明可用普通业务写工具的 Turn，且只有主持人进入；
- Meeting protocol event 始终由 Harness/Relay 发布，任何 Turn 都不允许模型直接发布。

**prompt-only 验收**

1. System Contract 明确讨论与控制 Turn 只允许有界只读调查；
2. 每类 V2 Turn envelope 显式携带与角色和阶段一致的 advisory/direct policy；
3. Board Maintenance 明确 Board UPDATE 例外不等于外部业务写权限；
4. action finalization 明确只有主持人可按冻结 Board 直接使用业务工具；
5. 自动化锁定 prompt 合约，但不宣称实际工具调用已被 Harness 强制隔离。

如果后续要求从“行为契约”升级为“能力保证”，需要重新开启工具隔离设计；本次状态不得作为
Harness 已能确定性阻止写入的证据。

### MFX-005（代码完成）：System Contract 对“允许读、行动写入 Board”的正向说明不够明确

**状态：代码修复完成，待真实 Provider 行为验收。Contract v2 和全部 V2 Turn envelope 已采用
统一角色/阶段矩阵。**

**修复前现状**

当前 Contract 用“no persistent external effects”和“gathering context or evidence”表达边界，
但没有在稳定 System 层直接列出以下完整心智模型：

- 可以按需查看代码、Project View、Document、消息和其他引用；
- 这些读取应有界，只服务于当前 Intent、Speech、Board 或 Floor 决定；
- 普通参会者发现后续行动时不直接执行，而是在正式 Speech 中提出；
- 主持人把已经形成的行动决定写入 Board；
- 最终只有 action finalization 根据冻结 Board 物化。

设计文档已经表达“Agent 仅在当前决定确实需要时做小范围权威读取”和“不因建议一个行动而在
讨论 Turn 中直接执行”，但实际稳定 Contract 的正向措辞还可以更直接。

**期望 System 语义**

```text
During discussion Turns, you may perform bounded read-only inspection of code,
Project View, Documents, messages, repository state, and referenced resources
when needed to form a grounded Intent, Speech, Board update, or Floor decision.
Do not create, update, delete, publish, assign, commit, or otherwise persist
external state. Propose needed follow-up in canonical Speech; the moderator
records agreed actions on the Board. Only action_finalization may materialize
those frozen Board decisions.
```

Contract v2 已同时包含允许的读取、禁止的写入、行动进入 Board 和最终物化四层含义，并明确
普通参会 Agent 不进入物化阶段；主持 Agent 在 Intent、Speech 和 Floor Turn 中也保持外部只读。

**验收**

- System Contract 测试断言上述四类语义同时存在；
- 五类 V2 Turn envelope 均显式带与阶段相符的 tool policy；
- connector reset、Session rebuild 和旧 contract ID 不会继续使用缺少新边界的旧 System；
- Agent 质量验收包含“先只读调查、Speech 提议、主持人写 Board、最终物化”的完整场景。

### MFX-006（已解决）：伪会议不会安装 Meeting Contract

**状态：已解决。Meeting Contract 的严格安装判定是正确不变量；MFX-001 已阻断 Agent 在正式
Meeting 创建失败后用普通 Channel、Thread、Canvas 或 Huddle 进行隐式降级。**

**现状**

Meeting Contract 只在已经识别为 V2 Meeting channel 的 ACP Session 安装。普通 Channel 即使
标题、purpose 或 Canvas 写着“会议”，也仍使用普通 Channel Agent 上下文。

这本身是正确的 fail-closed 设计。此前的问题来自 MFX-001：Agent 曾把普通 Channel 当作降级
会议继续运行，使用户误以为上述 Meeting 约束已经生效。该来源现已修复，不应通过放宽 Contract
安装判定来处理伪会议。

**期望**

- 不根据名称推断或向普通 Channel 注入 Meeting Contract；
- 从源头禁止隐式伪会议降级；
- Desktop 和 Agent 在必要时明确提示“这是普通 Channel，不具备 Meeting Board/Floor/终态”；
- 真实验收必须从 kind `42100` Create 和 `room_kind=meeting` 开始，不能用普通频道对话替代。

真实创建到 Desktop MeetingScreen 的跨边界证明仍归 MFX-017，不作为重新打开本项的理由。

## 6. Meeting Desktop 与 spec 的缺陷

以下缺陷针对真正进入 `MeetingScreen` 的页面。截图中的普通 Channel 不用于证明这些组件不存在，
但代码与 spec 对照后确认仍有差距。

### MFX-007（已解决）：Meeting 标题栏信息与操作不完整

**状态：已解决。Desktop 已使用独立 `MeetingHeader` 展示 Meeting 图标、生命周期、来自可信
snapshot 的冻结主持人身份与 Human/Agent 类型、participant 头像组合与人数，并提供持续可用的
Board trigger。Meeting 更多菜单统一提供 participants、source context、activity、复制当前受支持
hash route 和终态结果入口。Human 主持人的 Abort 仍通过原有受控确认流程及同一个 Floor
controller/native fence 执行；Agent 主持、非主持人和终态没有该入口。标题栏未带回任何 Channel
管理操作，宽屏与中窄屏专项 E2E 和不同哈希截图均已通过。**

**缺少或不完整**

- 明确的 Meeting 图标；
- 主持人身份；
- participant 头像组合，目前主要显示图标和人数；
- 宽屏持续可用的 Board 展开/收起入口；
- 更多菜单；
- 复制 Meeting 链接；
- 查看来源上下文的统一入口；
- 查看 Meeting 活动记录；
- 主持人中止入口和终态只读信息在标题栏菜单中的组织。

**期望**

遵循 spec 9.2；不得重新带回普通 Channel 的成员增删、归档、可见性、类型编辑或 Huddle 主操作。

### MFX-008（已解决）：宽屏 Board 无法收起

**状态：已解决。宽屏 Board 现在默认打开，并可通过持续可见的 Board trigger 收起和恢复；
可见性按 Meeting 隔离，收起和恢复不会丢失 Board/Speech 草稿、timeline 位置或已调整宽度。
新的 Board Maintenance control token 会在对应宽屏或中窄屏模式中安全自动打开一次。**

**原现状**

宽屏 Board 默认展示且支持调整宽度，但 Board trigger 只在 `xl` 以下显示。宽屏用户不能收起
Board，也没有收起后持续可见的恢复入口。

**期望**

- 宽屏默认打开；
- 可以收起和重新打开；
- 收起不丢失 timeline 位置、Floor/Speech 草稿或 Board 草稿；
- Board Maintenance 需要主持人操作时可自动打开，但不能破坏用户草稿和权威窗口绑定。

### MFX-009（已解决）：主状态条泄漏原始 revision

**状态：已解决。Meeting 主状态条已只保留产品状态；原始 `speechRevision` 和 `stateRevision`
继续作为 Desktop 可信投影内部字段使用，不再显示给普通用户。专项 E2E 同时断言当前发言产品
状态仍然可见且 `Speech r...`、`State r...` 不会出现。**

**原现状**

`MeetingScreen` 主状态条直接展示：

```text
Speech r{speechRevision} · State r{stateRevision}
```

这与 spec 9.3“状态条不展示原始 epoch、revision 或 event ID”直接冲突。

**期望**

- 主界面只显示产品状态；
- revision 仅可放入明确的诊断/活动记录入口；
- 普通用户不需要理解 revision 才能操作会议。

### MFX-010（已解决）：Agent 主持时缺少完整只读主持观察面

**解决状态（2026-08-05）**

Desktop 现已为 Agent 主持、Human 参会的 Meeting 提供独立只读观察面。观察面直接组合可信
Meeting snapshot 中的 Board control、pending Intent、open Directed Handoff、Floor 和 Action
状态，展示当前主持阶段、稳定结果与有效期限；它不建立第二份主持状态机，也不展示 reasoning、
草稿、工具日志、ACP slot/session 或 opaque protocol fence。

观察面使用与 Participant 面板相同的状态表达，但其 DOM 不包含按钮、输入框或 Human Host
Console handler。Human 无法通过 Desktop 代理 Agent 更新 Board、选择 Intent/Handoff、关闭会议
或确认行动产出；当前 Human 自己的 Floor Request、Offer response、Speech 与 Yield 仍由既有
Floor Dock 按冻结身份提供。

**原现状**

当前 Human 不是主持人时主要看到自己的 Floor Dock 和简要状态。Agent 主持的 Meeting 中，Human
无法完整查看：

- pending Intent；
- open Directed Handoff；
- 当前 Board Maintenance/Floor Decision 的详细产品状态；
- Agent participant 的 pending、Offer、Grant 等稳定状态。

**期望**

- 为非主持 roster participant 提供只读的 Intent、Handoff 和主持进程观察面；
- 不显示任何可冒充 Agent 签名的 update/select/ACK/Speech/Close/Action 按钮；
- Human 自己的 Floor Request、Offer、Grant 控件仍按冻结 participant type 正常工作；
- 不展示模型隐藏推理、草稿或工具日志。

### MFX-011（已解决）：Participant 面板没有表达冻结 roster 的会议状态

**解决状态（2026-08-05）**

Desktop Participant 面板现已按 Host、Human participants、Agent participants 分组；初始化阶段尚未
得到 Relay 冻结分类的非主持成员进入独立 Pending classification 分组，不会被猜测为 Human。Host
只出现一次并保留其冻结 Human/Agent 类型。

面板直接从可信 Meeting snapshot 派生唯一主状态，固定优先级为 Speaking、Waiting for ACK、Floor
request、Pending Intent、Idle。状态随权威 snapshot 更新，不依赖 raw event arrival order；Profile
只补充名称和头像。当前没有带明确 freshness 的可靠 Agent runtime 来源，因此不展示 online/offline，
也不把未知 runtime 解释为离会或释放 roster。面板保持纯只读，不包含 roster 管理、身份代操作或
Channel role 修改入口。

**原现状**

Participant 面板是平铺列表，当前主要显示头像、Human/Agent、Channel role、host crown 和
Speaking。

**缺少**

- 按主持人、Human participants、Agent participants 分组；
- Human Request；
- pending Intent；
- Offer/ACK waiting；
- Grant/当前发言；
- 可用时的可靠 Agent runtime 状态。

**期望**

面板只做观察，不提供添加、移除、换主持、修改 Channel role 或把离线解释为退会的操作。

### MFX-012（已解决）：canonical Speech timeline 缺少身份与 Handoff 语义

**解决状态（2026-08-05）**

Desktop native Speech projection 现已使用同一份可信 snapshot 的冻结 roster 与 immutable moderator
身份，为每条 canonical Speech 输出 Human/Agent 类型和 Host 标识；同时严格解析原子
`handoff-to`、`handoff-type`、`handoff-reason`，并校验字段完整性、唯一性、协议类型、原因文本及
target 的冻结 roster 归属。普通 mention 不会被推断为 Handoff，非法或冲突 Handoff 数据 fail
closed。Speech timeline 直接显示上述 typed DTO，并在正文后展示 Handoff 的目标、类型和原因；
Intent、Offer、Grant、Board command 等控制事件仍只进入 Meeting Activity，不建立普通消息旁路。

**原现状**

Speech row 主要显示头像、作者、时间和 Markdown 正文。Native `MeetingSpeech` DTO 也只包含 author、
content、revision、grant 和 mentions，没有保留可供 UI 展示的 Directed Handoff 目标与原因。

**缺少**

- Human/Agent 类型；
- speaker 同时为主持人时的主持标识；
- Directed Handoff 的目标、类型和原因；
- 必要的轻量控制转换行或对应活动记录入口。

**期望**

- 扩展可信 read projection/DTO，而不是从 Speech Markdown 猜测 Handoff；
- 不把 Intent、Offer、Grant、Board command 混成普通 Speech；
- 继续禁止 Reply、Thread、Reaction 和普通消息编辑旁路。

### MFX-013（已解决）：Meetings 侧栏 attention 与排序不完整

**解决状态（2026-08-05）**

Native `MeetingListItem` 现已针对当前 Desktop 冻结 Human 身份直接投影有限的
`needsAttention` 和 `attentionReason`，覆盖当前 Human 的 Offer/Grant、Human 主持人的 Board
Maintenance、Floor Decision、action runnable/blocked，以及需要查看的 aborted 终态。投影不再
返回由 React 对比 pubkey 猜测的 `humanFloorAttentionPubkey`，Agent 身份和其他 participant 的待办
不会成为当前 Human 的 attention。

列表的 `updatedAt` 现在取 Create、权威 State、Board、canonical Speech 和 End 的最近时间，而非
只看 Board/End。Desktop 使用稳定顺序：当前身份 attention、非终态 active、最近权威活动、
Meeting ID。需要查看的 aborted Meeting 在确认前保留在主列表，查看后进入 history；确认记录按
Community、Meeting、attention reason 和终态时间隔离，并且不写 Speech read marker。

Speech unread 仍只由 `latestSpeechAt` 与共享 read marker 派生。Board、Intent、Floor、Action 或
attention 确认不会生成、清除或修改 Speech unread。

**原现状**

当前 attention 只从指向 Human 的 active Offer/Grant 派生。active list 只按 `updatedAt` 排序。

**缺少**

- Human 主持人 Board Maintenance attention；
- Human 主持人 Floor Decision attention；
- Human 主持人 action runnable/blocked/retry attention；
- blocked/aborted 等需要关注的稳定状态；
- “需要当前身份操作 → active → 最近活动”的排序规则。

**期望**

- unread 只由新 canonical Speech 产生；
- attention 独立于 unread，并随权威状态解除；
- Native list projection 应提供足够且不泄漏隐私的产品级 attention，不由 Desktop 猜事件日志。

### MFX-014（已解决）：Meeting 活动记录未实现

**解决状态（2026-08-05）**

Desktop 已增加独立、可信且有界的 Meeting Activity 投影与 Sheet 入口。投影从签名、scope、冻结
roster 和状态连续性均已验证的 Meeting Create/State/cause events 生成产品级活动，覆盖 Board、
Floor、Directed Handoff、Action Finalization 与终态转换；读取使用有上限的稳定 opaque cursor，
普通 DTO 不暴露 event ID、revision、epoch、lease、control token 或 raw State。活动按权威分页顺序
展示，不进入 canonical Speech、不增加 Speech unread，也不带 Reply、Reaction 或普通消息操作。
当前入口是供本阶段使用的稳定 Activity trigger；最终标题栏组织仍留给 `MFX-007`。

**原现状**

spec 允许把重要控制转换压缩为轻量系统行，并要求更多菜单提供“会议活动记录”。当前没有统一的
Meeting activity 入口。

**期望**

- 活动记录展示产品级控制变化，如 Board 完成/超时、Offer/Decline、Yield、Recall、进入 action、
  blocked、retry、return-to-board、closed/aborted；
- 不要求普通用户理解 event ID、epoch、revision、lease 或 raw State；
- 不把活动记录计入 canonical Speech unread；
- 诊断信息与普通产品活动分层展示。

### MFX-015（已解决）：终态摘要信息不完整

**解决状态（2026-08-05）**

Native Meeting End 投影已增加基于 verified signer 的 `host | relay | unknown` 终止来源，并继续保留
Human 可读 reason。Desktop 终态摘要现在分别表达主持人判断目标达成的正常关闭、未作为目标达成
正常结束的中止、原因类别、可读说明、结束人、结束时间和可信来源；`actions-recorded` 明确只表示
最终 Board 的行动产出已确认登记，不表示结果 Work 已完成。权威 Action 状态存在的中止会提示外部
效果可能保留，但不虚构 operation list、receipt 或 Relay 对外部行动的验证。终态没有 reopen 或任何
Meeting 写入口。

**现状**

终态摘要显示 outcome、结束人、结束时间、reason code 和是否 actions-recorded，但没有完整显示
Human 可读 reason，也没有终止来源的稳定表达。

**期望**

- `closed` 明确表达目标达成和是否经行动产出确认；
- `aborted` 显示 reason category、Human 可读说明和可用时的终止来源；
- 曾进入 action 时提示外部效果可能保留；
- 不声称 Meeting 掌握外部业务操作清单。

## 7. 文档与验收缺陷

### MFX-016（P2）：Desktop spec 状态仍写“待实现”

**现状**

`meeting-desktop-spec.md` 文件头仍写“待实现”，但实现计划记录阶段一至五已完成、阶段六代码与
自动化已完成。

**期望**

- spec 状态改为已进入真实验收修正阶段；
- 不把自动化完成误写成真实 Desktop 已完全验收；
- 本缺陷清单修复完成后再更新最终交付状态。

### MFX-017（P0）：Mock E2E 未覆盖“Agent 创建了普通 Channel”的真实分叉

**现状**

现有 Meeting Desktop E2E seed 直接提供 `room_kind=meeting` 和 Meeting snapshot，因此能证明
MeetingScreen 本身隔离普通 composer，却不能证明真实 Agent 发起动作一定创建 Meeting。

真实 Tauri 截图已经证明：如果上游创建普通 Channel，mock Meeting E2E 不会发现问题。

**期望验收链**

```text
Human/Agent 发起意图
  → buzz meetings create / Desktop Create
  → Relay 接受 kind 42100
  → DB Meeting Session + room_kind=meeting
  → kind 39000 discovery metadata
  → Desktop Meetings sidebar
  → MeetingScreen
  → 无普通 composer/thread/reaction/channel management
```

至少增加一次真实或跨 native boundary 的端到端验收，不允许从 mock `room_kind=meeting` 作为
测试起点替代创建链路。

## 8. 建议修复顺序

按依赖关系建议分为四个阶段：

1. **协议入口与容量**：MFX-001、MFX-002、MFX-006、MFX-017；先确保 8-Agent Meeting 能真实
   创建，超限不降级。
2. **Agent 工具边界**：MFX-003、MFX-004、MFX-005；明确并尽可能确定性执行“讨论只读、Board
   记录、最终物化”。
3. **Desktop 核心可见性**：MFX-007、MFX-008、MFX-010、MFX-011、MFX-012、MFX-013、
   MFX-014。
4. **产品与文档收口**：MFX-009、MFX-015、MFX-016，并执行一次完整真实 Tauri 穿行。

每个阶段分别 review 和提交。真实创建链路未通过前，不能因为 MeetingScreen 的 mock E2E 已通过
而宣称 Meeting Desktop 已完成真实验收。

## 9. 统一完成条件

本清单可以关闭需要同时满足：

1. 单场 8 个 Agent 的 Meeting Create、Intent、Offer、Grant、Speech、Board/Floor 和 action
   路径通过；9 个 Agent 明确拒绝且无普通 Channel 副作用；
2. Agent 发起会议不会隐式降级为普通 Channel；
3. 所有 discussion Turn 可以完成必要只读调查，但持久写入被明确且可验证地限制在
   action_finalization；
4. 参会者提出行动、主持人记录 Board、主持人在最终阶段物化的真实 Agent 场景通过；
5. 真实 Meeting 只出现在 Meetings 分组，并进入 MeetingScreen；
6. Human participant 可以完成 Request → Offer → Grant → Speech/Yield；
7. Human host 可以完成 Board、Intent、Floor、self Speech、Close、Abort 和 Action；
8. Agent host/participant 的必要状态对 Human 可观察，但 Human 不能冒充 Agent；
9. Desktop 标题栏、Board、Speech、participant、attention、activity 和终态满足已确认 spec；
10. 普通 Channel、Channel Canvas 和 Meeting 不再在产品或 Agent 行为上互相冒充；
11. 更新 spec、实现计划和验收记录，明确真实 Tauri 结果与剩余非阻断体验项。
