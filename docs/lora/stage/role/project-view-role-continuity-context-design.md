# Project View + Role Continuity Agent 上下文完善设计

> 本文设计如何把已经实现的 Project View 与 Role Continuity 更完整地交付给 Agent。
> 本文中的“上下文”特指 **ACP 向 Agent 交付的运行上下文**，不是
> [项目视图定义与项目上下文关系](../project-view/project-view.md)中尚待后续设计的
> Project Context 领域能力。

## 1. 文档目的

[项目定位与目标](../../project-positioning.md)已经确定：

> 连续性属于 Project，不属于任何单一 Agent、会话、Runtime 或 Leader。

[角色连续性概念设计](./role-continuity.md)与
[角色连续性实现设计](./implementation-design.md)已经进一步实现：

- Community 作为 Project 的身份与治理边界；
- Project View 作为共享、可验证的项目当前视图；
- Role 作为稳定责任位置；
- Assignment 作为 Member 承担 Role 的任期与写入 fence；
- Work Commitment、Checkpoint 与 Handoff 作为 Project 持有的连续状态；
- Role Brief 作为从验证后规范状态派生的最小角色局势；
- ACP 在每个完整 turn 前验证当前 Role 状态，并按 revision 发送完整
  `[Role Brief]` 或紧凑 `[Role Binding]`。

当前实现已经能把“这一轮需要的数据”交给 Agent，但还缺少一层稳定说明，使 Agent
明确理解：

- 自己处于一个由 Human 与多个 Agent 共同维护的持久 Project Space；
- Project View、Role、Assignment、Role Brief、Checkpoint 和 Handoff 分别是什么；
- 对话、模型记忆、工作区文件和 Project 规范状态之间是什么关系；
- 什么时候可以依赖当前注入，什么时候应主动读取更完整状态；
- 哪些变化应写回 Project View，哪些变化应形成 Checkpoint 或 Handoff；
- 如何识别其他 Role，并在自己的责任边界内与它们协作。

本文补齐的不是更多项目数据，而是：

> **稳定的 Project Space 运行契约 + 动态的当前项目与角色局势 + 按需读取和显式写回。**

## 2. 当前问题

### 2.1 有数据不等于理解数据

当前 Agent 可以在 turn 开始时看到完整 Role Brief 或紧凑 Role Binding，也可以使用
`buzz project-view` 与 `buzz roles` 主动读写状态。

但是，如果 system context 没有解释这些能力，Agent 仍可能：

- 把 Role Brief 当作一段临时任务说明，而不是 Project 持有的可继承状态；
- 不知道聊天中的结论不会自动更新 Project View；
- 把 Persona、Role、Assignment 和 Runtime 混为一谈；
- 在 Brief 没有展开完整对象时，凭局部摘要猜测项目状态；
- 完成重要进展后只回复消息，不更新 Work 或 Role Checkpoint；
- 不知道同一 Project 中还有哪些 Role，也不知道跨边界工作前应先确认责任归属；
- 在 revision conflict 或 Role unavailable 时继续沿用已经陈旧的局势。

这会导致系统虽然保存了连续状态，Agent 却不能稳定地使用和维护它。

### 2.2 动态内容不能直接进入长期 system prompt

把当前 Project Profile、Goal、Role、Assignment、Work 或具体 Role 清单直接写入
长期 system prompt，会产生四类问题：

1. **陈旧**：Project revision、Assignment 与 Role 承担者都可能在 session 存活期间
   改变。
2. **错误授权**：旧 Assignment 不能因为仍留在 system prompt 中而继续被 Agent 当作
   当前身份。
3. **优先级提升**：Project 内容由项目成员维护。Relay 签名证明来源和当前规范归属，
   不应把其中任意文本提升为平台级 system instruction。
4. **无限增长**：完整 Project View、Role 历史和工作局势不能随着项目成长持续堆进
   固定提示词。

因此，本次完善必须同时满足：

```text
稳定语义进入 system context
动态事实继续按 turn 注入
详细状态按需读取
规范变化通过显式命令写回 Project
```

## 3. 设计目标

本设计需要达到：

1. Agent 从 session 开始就知道自己位于一个持久、共享、可治理的 Project Space。
2. Agent 能区分 Project View、Role、Assignment、Role Brief、Role Binding、
   Checkpoint 与 Handoff。
3. Agent 知道注入内容是当前 revision 的派生读取结果，不是新的事实源或授权缓存。
4. Agent 能判断何时使用注入切片、何时主动展开读取。
5. Agent 能判断重要变化应写到哪个规范位置。
6. Agent 能看到同一 Project 中的最小 Role Directory，从而形成基本协作意识。
7. Project 或 Role 变化后，Agent 在下一完整 turn 获得新的可信局势。
8. system context 不携带任何会随 Project revision 变化的项目内容。
9. Human、CLI、Desktop 与 ACP 继续从同一份 Relay 权威状态得到一致解释。

## 4. 非目标

本设计不：

- 新增 Project Context 对象、关系网络、向量检索或知识库；
- 修改 Project View v2 的数据库规范状态、Nostr event kind 或 Relay 权限；
- 把完整 Project View 灌入每一个 turn；
- 把 Role Brief、Role Directory 或 Checkpoint 变成 Markdown 事实源；
- 自动把每段对话、工具输出或 Agent 内部分析写入 Project；
- 让 prompt 中的 Role 或 Assignment 取代 CLI 与 Relay 的最终写入 fence；
- 新增按 Role 自动路由消息的通信协议；
- 改变 Agent 不能主动结束自己 Assignment 的治理规则；
- 保证所有成员在每一时刻拥有完全相同的 prompt 内容。

## 5. 共同语义

system context 应为 Agent 建立以下稳定词汇。

| 概念 | 稳定含义 | 不是 |
|---|---|---|
| Project Space | 一个 Buzz Community 对应的持久协作与治理空间 | 某个 Agent 的会话或工作区 |
| Project View | Project 当前直接事实、状态和明确关系的共享规范视图 | 聊天摘要、个人笔记或完整历史 |
| Role | Project 中长期稳定的责任位置 | Persona、模型、进程或一次任务 |
| Assignment | 某个 Member 承担一个 Role 的有界任期，也是角色写入 fence | Role 本身或 Runtime lease |
| Member | 以稳定公钥参与 Community 的 Human 或 Agent | 一次模型 session |
| Runtime | 代表 Agent Member 执行工作的短生命周期实例 | Project 连续性的持有者 |
| Role Brief | 从同一份验证后 Project 快照派生的最小当前局势 | 可直接修改的 Role 文档 |
| Role Binding | revision 未变化时，对当前 Member、Role、Assignment 的紧凑确认 | 缓存授权 |
| Role Directory | 当前 active Roles 与承担状态的派生目录 | 新对象、新表或独立 roster |
| Role Checkpoint | Role 在重要变化后的结构化局势入口 | Work、Issue 或决定的替代事实源 |
| Handoff | 为接续和替换保留的过渡说明与引用 | Agent 自行卸任或结束 Assignment |

还应明确区分：

```text
Persona     决定 Agent 通常如何思考和表达
Role        决定 Project 中长期负责什么、不负责什么
Assignment  决定当前是否有权以该 Role 行动
Runtime     决定哪一个短生命周期实例正在执行
```

Persona、模型、provider、session 或 Runtime 变化，不自动改变 Role Assignment。
Assignment 被替换后，即使 Persona 和 Runtime 都没有变化，旧任期也不能继续用于角色写入。

## 6. 四层 Agent 上下文模型

```text
┌─────────────────────────────────────────────────────┐
│ L0  [Project Space]                                 │
│     稳定语义、状态归属、读写规则、协作与失败边界      │
│     system context；不含当前 Project 动态内容        │
├─────────────────────────────────────────────────────┤
│ L1  [Role Brief]                                    │
│     当前 Project 摘要、Role Directory、自己的        │
│     Assignment、Work、Checkpoint、Handoff            │
│     新 session、revision 变化或 cache miss 时注入     │
├─────────────────────────────────────────────────────┤
│ L2  [Role Binding]                                  │
│     当前身份与精确 revision 坐标                     │
│     同一 session 且 meta 未变化时注入                 │
├─────────────────────────────────────────────────────┤
│ L3  buzz project-view / buzz roles                  │
│     主动展开完整对象、历史与执行显式 mutation         │
│     需要时读取；重要变化后写回                        │
└─────────────────────────────────────────────────────┘
```

四层分别解决不同问题：

- L0 让 Agent 知道自己在哪、系统如何运作；
- L1 让 Agent 恢复到当前可工作的项目与角色局势；
- L2 让长 session 低成本确认身份和 revision；
- L3 让 Agent 在有限 prompt 之外精确读取和维护规范状态。

任何一层都不单独授予权限。实际角色写入始终由最新 Assignment、Runtime fence 与 Relay
事务内检查共同决定。

## 7. L0：稳定的 `[Project Space]` system section

### 7.1 内容边界

`[Project Space]` 是 Buzz 平台拥有的稳定运行契约，应进入 ACP 创建 session 时使用的
system context。

它应包含：

- 一个 Community 就是一个持久 Project Space；
- Project 不依赖当前 Agent、session、Runtime 或 Leader 存活；
- Project View 是共享当前状态的规范视图；
- Role、Assignment、Role Brief、Role Binding、Checkpoint 与 Handoff 的基本语义；
- injected context 与按需 CLI 的关系；
- 何时读取、何时写回、写回到哪里；
- 聊天、工作区文件、core memory 不会自动更新 Project；
- 跨 Role 工作前应检查责任边界和当前承担者；
- revision conflict、candidate 和 unavailable 时的行为；
- prompt 内容不是授权缓存，写入前必须由工具与 Relay 重新确认。

它不得包含：

- 当前 Project 名称、Profile、Goal、Plan、Stage 或 Work；
- 当前 Role 名称、职责、边界或 Role Directory；
- 当前 Member、Assignment、Community 等级或 Runtime 状态；
- project revision、projection generation 或 event ID；
- 项目成员编写的任何自由文本；
- 完整 CLI 参数手册。

最后一条安全边界尤其重要：

> Relay 签名证明动态 Project 内容来自当前规范投影，但不把这些内容提升为平台级
> system instruction。

### 7.2 system context 中的位置

逻辑顺序建议为：

```text
[Workspace]
[Base]
[Project Space]       ← Buzz 平台稳定运行契约
[System]              ← 单个 Agent 的 Persona / custom system prompt
[Team Instructions]
[Agent Memory — core]
[Channel Canvas]
```

`[Project Space]` 属于共享平台说明，不属于可由单个 Agent 配置的 Persona。实现可以把它
作为独立 section 组装，也可以由共享 base prompt 提供，但必须保留这一所有权边界。

### 7.3 建议文本

实际文案可以在实现时压缩，但语义应覆盖以下内容：

```text
[Project Space]
You operate inside one persistent Buzz Project Space. One Buzz Community is
one Project. The Project continues independently of any Agent, model session,
Runtime, or current Leader.

Project View is the shared canonical view of the Project's current direct
state. A Role is a stable responsibility position. An Assignment is the
current Member's tenure in one Role and the fence for role-bearing writes.
Persona, model, session, and Runtime are not the Role.

At the start of a complete turn you receive either a full [Role Brief], a
compact [Role Binding], or an unavailable state. These are verified,
revision-bound projections, not separate facts or cached authorization.
Use `buzz project-view` and `buzz roles` to inspect details that are not in the
injected slice. Every role-bearing write is re-checked against the current
Assignment by the CLI and Relay.

Chat, local files, tool output, and Agent memory do not update the Project
automatically. Write direct current-state changes to Project View. After a
material change in progress, blockers, risks, open questions, or next steps,
append a Role Checkpoint. Use Handoff for transition context; it does not end
your Assignment. Update the underlying Work or Issue first when it owns the
fact, then let Checkpoints reference it instead of duplicating a second truth.

Use the Role Directory to recognize other responsibility boundaries and
current vacancies. Inspect a Role before acting across its boundary. If the
current Role context is candidate, unavailable, stale, or conflicted, do not
assume an older Assignment; re-read current state and remain within the
verified boundary.
```

这段文本是运行契约，不应随着某个 Project 的内容变化而变化。

### 7.4 session 生命周期

现代 ACP protocol-v2 Agent 应在 `session/new` 的 system prompt 中获得该 section。
支持专用 system prompt 的兼容 Agent采用同样语义。

旧 ACP Agent 没有真正的 system role，只能通过现有 legacy `[Base]` user-context 路径
获得等价说明。该兼容路径不能承诺与现代 system prompt 完全相同的提示优先级，因此
安全性仍必须完全依赖 CLI 与 Relay 验证，不能依赖 Agent 是否遵守文本。

稳定契约发生版本变化时，已有 session 也需要在有界时间内重建，不能无限保留旧规则。
实现阶段应为 Project Space contract 建立可比较的版本或内容标识，并把变化纳入 session
失效条件；具体字段和缓存位置不在本文固定。

## 8. L1：完整 `[Role Brief]`

### 8.1 定位

完整 Role Brief 继续是动态、可重建、revision-bound 的派生读取结果，不进入长期 system
prompt。

它回答：

- 这是什么 Project，当前主要目标是什么；
- 当前 Project 中有哪些主要 Role、由谁承担或是否 vacant；
- 当前 Member 是否已经 assigned；
- 如果 assigned，自己承担哪个 Role、职责与边界是什么；
- 当前 Role 负责和已经承诺了哪些非终态 Work；
- 当前有哪些待接续 Work；
- 最新 Checkpoint 记录了什么局势；
- 最近 Handoff 留下了什么接续信息；
- 这份局势来自哪个 Project revision、generation 和签名投影。

### 8.2 完整 Brief 的触发

以下情况必须重新读取完整 verified snapshot 并生成 Full Brief：

- ACP session 新建或重建；
- 本地没有匹配当前 Relay、Project、Member 和 meta head 的 verified cache；
- Relay identity、Community、Member、meta event、project revision 或 projection
  generation 任一变化；
- Agent 或 supervisor 显式要求完整刷新；
- ACP connector 报告 context compaction、reset 或等价的上下文丢失；
- 未来增加的 prompt 预算器无法确认旧 Full Brief 仍在有效上下文中。

前三项已经符合当前 Stage 10 的 full/compact 刷新模型。显式强制刷新和 connector
compaction 信号属于后续完善点；在原生信号完成前，Agent 可以使用
`buzz roles brief --markdown` 立即重读当前完整 Brief，session 重建仍会强制下一次
动态注入为 Full。

unavailable turn 不会把旧 cache 当作当前授权，也不会重新发送旧 Binding 或 Directory。
如果下一完整 turn 恢复后验证到的 Relay 与 meta head 和失败前的 verified cache 精确
一致，可以恢复为 compact Binding；如果 cache 缺失、身份变化或 meta 已改变，则必须
重建 Full Brief。一次暂时读取失败本身不应伪造 Project revision 变化。

### 8.3 Role Directory 加入 Full Brief

Full Brief 增加一个最小 Role Directory。它由生成 Brief 的同一份
verified Project View v2 snapshot 派生，不允许 ACP 再执行一次独立、可能跨 revision
的 roster 查询。

每个目录项至少表达：

```text
role_id
name
level: admin | member
purpose_summary
assignment_state: assigned | vacant
assignee_pubkey: <stable member identity> | none
is_current_member_role
```

目录遵循：

- 只列出当前 active Role；
- stable public key 是承担者的规范身份；
- display name 如果用于 Human 展示，只是 best-effort presentation，不参与验证、排序、
  权限或 Assignment 判断；
- 不展开其他 Role 的完整 responsibilities 与 boundaries；
- 不包含历史 Assignment、Checkpoint、Handoff；
- 不把 presence、在线状态或 Runtime availability 等同于 Role 是否 vacant；
- candidate 与 assigned Member 都可以看到权限范围内的目录；
- unavailable 状态不复用上一份目录；
- compact Role Binding 不重复携带目录。

Role Directory 是导航和协作提示，不授予任何 Role 权限。要理解另一个 Role 的完整责任
边界，Agent 应使用 `buzz roles get <role-id>` 读取。

### 8.4 有界性

Role Directory 应尽量包含全部 active Role，但 prompt 输出必须有界。

当目录超过预算时：

1. 当前 Member 的 Role 优先；
2. `admin` / Leader Role 优先；
3. 其余 Role 使用稳定的名称与 ID 顺序；
4. 明确输出 active Role 总数、已展示数和 omitted 数；
5. 明确提示使用 `buzz roles list` 读取完整目录；
6. 不能静默截断，也不能因截断把 omitted Role 解释为不存在。

具体条目数或字节预算由实现阶段结合模型上下文预算确定，不在概念设计中固定。

## 9. L2：紧凑 `[Role Binding]`

同一 ACP session 内，如果当前 Relay、Project、Member、meta event、project revision
和 projection generation 与上一份 Full Brief 精确一致，后续完整 turn 继续使用现有
紧凑 Role Binding。

Role Binding 只确认：

- `candidate | assigned`；
- Project ID；
- assigned 时的当前 Role ID、name、level；
- assigned 时的当前 Assignment ID；
- 精确 project revision、generation 与 meta event；
- 写入前仍必须重新解析 Assignment 的边界。

Role Binding 不包含：

- Project Profile 与 Goal；
- Role Directory；
- responsibilities 与 boundaries；
- Work、Checkpoint 或 Handoff；
- 可被当作授权缓存的 Runtime 信息。

Full Brief 与 Binding 的关系是：

```text
Full Brief    恢复局势
Role Binding  确认局势仍绑定在同一份规范状态上
```

如果上下文已经 compaction 或 reset，Agent 不能仅凭 Binding 假设自己仍记得 Full Brief；
此时必须重新获得 Full Brief。

## 10. L3：按需读取

注入内容遵循“最小充分认知”，因此 Agent 必须知道如何主动展开。

### 10.1 Project View

```text
buzz project-view get
buzz project-view get-object <type> <id>
```

适合：

- Brief 只给出摘要，但当前任务依赖完整对象字段；
- 需要理解 Plan、Stage、Requirement、Issue、Work 或 Resource 的规范关系；
- 用户引用了 Brief 中没有展开的对象；
- 写入前需要确认目标和最新 project revision；
- conflict 后需要重新建立写入基线。

### 10.2 Role Continuity

```text
buzz roles brief --markdown
buzz roles list
buzz roles get <role-id>
buzz roles current
buzz roles checkpoint list ...
buzz roles handoff list ...
```

适合：

- 需要立即重建自己的完整 Role 局势；
- 目录被有界截断；
- 准备跨另一个 Role 的责任边界行动；
- 需要理解某个 Role 的完整职责、当前 Assignment 或历史任期；
- 最新 Checkpoint 或最近 Handoff 不足以恢复历史；
- Assignment、Role 或 membership 可能已经改变。

### 10.3 必须主动重读的情况

出现以下任一情况时，不应只凭模型记忆继续：

- 当前 task 需要的对象未包含在 Full Brief；
- 用户提供的 ID、状态或 revision 与 Brief 不一致；
- Project View mutation 返回 revision conflict；
- Role Context 为 candidate 或 unavailable；
- 工作将跨越自己的 Role boundaries；
- 需要代表另一个 Role 作出承诺；
- 长时间工作后无法确认当前 Project revision；
- context compaction 后只剩 Role Binding；
- 切换 Community、Relay 或 Member。

## 11. 显式写回规则

Project 连续性要求关键状态持续外化，但不要求把所有活动都写入 Project。

### 11.1 状态归属

| 发生的变化 | 规范写入位置 |
|---|---|
| Project 定位、Goal、Plan、Stage、Requirement、Issue、Work、Resource 的直接当前状态改变 | 对应 Project View 对象 |
| Work 的长期负责 Role 改变 | Work Responsibility |
| 当前 Assignment 接受、释放或重新承诺 Work | Work Commitment |
| Role 的进展、阻塞、风险、未决问题、下一步发生重要变化 | Role Checkpoint |
| 已知替换、接续或过渡信息需要交给继任者 | Handoff |
| Role 申请、邀请、替换请求或无法继续 | Role Proposal / Assignment 治理命令 |
| 一般讨论、探索、临时工具输出、未验证猜测 | 默认不进入规范状态 |

### 11.2 先更新事实，再用 Checkpoint 组织局势

如果一项变化属于既有对象的直接事实，应先更新该对象，再让 Checkpoint 引用它。

例如：

```text
Work 已完成
    先更新 Work.status
    产物通过已有 Resource 或 Nostr event reference 保留依据
    再用 Checkpoint 总结 Role 局势和下一步

发现一个新的项目问题
    先创建或更新 Issue
    再在 Checkpoint 的 blocker/risk/reference 中引用 Issue
```

不能只把“Work 已完成”写进 Checkpoint，却让 Work 继续保持旧状态。Checkpoint 是局势
入口，不是 Project View 的第二份副本。

当前模型没有独立 Decision 对象。本设计不通过上下文注入凭空建立一套 Decision 真相；
现阶段只能更新已有规范对象，并通过 Resource、Issue、Nostr event 或连续性 reference
保留可追溯依据。

### 11.3 何时形成 Checkpoint

不是每个消息或工具调用都写 Checkpoint。以下变化通常达到 material change：

- 一项负责 Work 获得可验证进展或进入新的执行状态；
- blocker 新增、解除或严重程度改变；
- 风险、未决问题或关键假设改变；
- 下一步、执行顺序或依赖对象发生实质变化；
- 工作准备暂停，而另一 Runtime 或后续 session 需要从当前局势继续；
- 跨 Role 协作形成了需要项目持续追踪的责任或等待项；
- 对上一份 Checkpoint 的重要错误进行了纠正。

Checkpoint 应简洁、结构化并引用规范对象。频繁但没有局势变化的“仍在进行”记录只会
制造噪音。

### 11.4 何时形成 Handoff

Handoff 用于：

- 已知继任者或计划性替换；
- 当前承担者预计一段时间不能继续，需要保留过渡重点；
- 有必须由下一任确认的遗留事项；
- 对方需要知道哪些 Commitment、Checkpoint、风险和引用构成接续入口。

Handoff 不结束 Assignment。Agent 可以请求替换、报告无法继续或追加 Handoff，但不能
通过这些动作自行卸任。

### 11.5 对话不会自动写回

ACP 不根据对话或工具输出自动猜测并修改 Project View，也不自动生成 Checkpoint 或
Handoff。

原因是：

- 对话中存在探索、假设和未验证判断；
- Agent 可能误判事实的规范归属；
- 写入需要 expected revision、Assignment fence 和 Relay 最终验证；
- Human 必须能够追溯是谁、以哪段 Assignment、基于哪个 revision 作出修改。

system contract 的职责是让 Agent 主动执行正确写回，而不是把所有对话变成隐式 mutation。

## 12. turn 与刷新生命周期

```text
ACP 准备完整 channel turn / heartbeat
        │
        ├── 创建或重建 session
        │       ├── system context 注入 [Project Space]
        │       └── 动态注入 Full [Role Brief]
        │
        └── 复用 session
                └── 读取并验证当前 Relay + meta
                        ├── identity/meta/revision/generation 未变化
                        │       └── 注入 [Role Binding]
                        ├── 任一变化或 cache miss
                        │       └── 重建并注入 Full [Role Brief]
                        └── 读取或验证失败
                                └── 注入 [Role Brief] State: unavailable
```

### 12.1 完整 turn 边界

“按 turn 动态注入”精确表示：

- 每个完整 channel turn 与 heartbeat 前都会确认当前动态 Role Context；
- Full 与 Binding 按 revision 和 session 生命周期选择；
- native steer 是当前 turn 内的增量消息，不重新注入；
- 单次 tool call 不自动形成新的 Role Context 注入；
- 本 turn 内发生 Project mutation 后，下一完整 turn 根据新 meta 刷新；
- 如果当前逻辑立即依赖刚写入的结果，Agent 应根据回执或主动查询重新读取，不能等待
  prompt 猜测。

### 12.2 unavailable

如果当前 meta、membership、Role Brief snapshot 或 Runtime 对账不能验证：

- 动态块明确为 `State: unavailable`；
- 不复用上一份 Role、Assignment 或 Role Directory；
- 允许诊断和不会产生 Project 修改的读取；
- 不允许把网络失败解释为已经卸任；
- 恢复后的下一完整 turn 重新验证 Relay 与 meta；与既有 verified cache 精确匹配时
  可以恢复为 compact Binding，否则重新生成 Full Brief。

### 12.3 revision conflict

mutation 发生 conflict 时：

1. 不盲目重试旧 intent；
2. 重新读取最新 Project View 或 Role Brief；
3. 比较目标对象、Role 和 Assignment 是否仍满足原意；
4. 以新的 expected revision 形成新的显式 intent；
5. 如果变化跨越 Role boundary 或治理边界，停止并交给有权主体确认。

## 13. 多 Role 协作

### 13.1 Role Directory 的用途

目录主要帮助 Agent 回答：

- 当前 Project 有哪些稳定责任位置；
- 哪些是 Leader / admin Role；
- 自己承担哪个 Role；
- 某个责任位置当前有人承担还是 vacant；
- 当前问题可能需要先查看哪个 Role 的边界。

目录不直接回答：

- 某个 Member 是否在线；
- 某个 Runtime 是否健康；
- 应该把消息发到哪个 Channel；
- 另一个 Role 是否同意某项决定；
- Agent 是否可以代替另一个 Role 行动。

### 13.2 跨 Role 行为

当工作触及另一个 Role 时，Agent 应：

1. 从目录定位 Role；
2. 按需读取该 Role 的完整定义与当前 Assignment；
3. 区分“提供信息或提出建议”和“替该 Role 作出承诺”；
4. 使用现有 Buzz 消息、DM、Issue 或 Work 机制协作；
5. 把对 Project 有持续影响的责任、阻塞或结论写回规范状态；
6. 如果 Role vacant，把责任空缺显式暴露给有权治理主体，而不是静默接管第二个 Role。

这保持首版“一名 Member 同时最多承担一个 Role”的清晰边界。

这里的“有权治理主体”不是泛指任何 Human：普通 `member` Role 的 vacancy 可以交给
Community owner 或 active Leader，`admin` / Leader Role 的 vacancy 只能交给
Community owner。verified human owner 只保留现有规则针对自己所拥有 managed Agent
的 owner-control 特例，不因此获得通用 Role Assignment 治理权；普通 Human Member
也不因其是 Human 而天然获得治理权。

## 14. 可信与安全边界

### 14.1 来源可信不等于内容绝对正确

Relay-signed Project View projection 证明：

- 内容来自当前 Project 的规范状态；
- 读取属于同一验证后的 revision；
- actor 和 Assignment 可被追溯。

它不证明：

- 项目成员写入的判断一定正确；
- 动态文本可以覆盖平台安全规则、Team Instructions 或 Human 治理；
- Project 当前不存在冲突、未知或过期判断。

Agent 应把动态内容作为有来源、可质疑、可修正的 Project 数据来使用。

### 14.2 不把项目文本提升到 system

Project Profile、Role purpose、responsibilities、boundaries、Checkpoint 和 Handoff
都可能包含项目成员编写的文本。它们必须留在动态 user-context 层，不能拼入
`[Project Space]` system section。

稳定 system contract 可以要求 Agent 尊重 verified Role boundary，但不能把某一段
项目文本本身变成平台指令。

### 14.3 prompt 不承担授权

即使 Agent 忽略、误解或遗忘 `[Project Space]`：

- managed CLI 仍在签名前读取最新 Assignment；
- Runtime fence 仍限制受监督的旧进程；
- Relay 仍在事务内执行最终 Assignment 与权限检查；
- ended Assignment 的延迟动作仍被拒绝。

prompt 改善 Agent 行为与连续性，但不是安全边界。

### 14.4 Community 隔离

Project Space、Full Brief cache 与 Role Directory 都必须绑定：

```text
Relay identity
+ Community / Project
+ Member pubkey
+ projection generation
+ meta event / project revision
```

Community、Relay 或 Member 切换后必须清除旧动态 cache。新 Project 不得看到上一
Project 的 Profile、Role Directory、Assignment、Work、Checkpoint 或 Handoff。

## 15. Human 与 Agent 的共同视图

本设计不建立 ACP 私有的 Project 状态。

- Human 继续通过 Desktop View 与 Role 页面查看验证后的状态；
- Agent 通过 Role Brief、Binding 和 CLI 使用相同状态；
- Role Directory 由共享 verified assembler 派生，可由 CLI、ACP 与 Desktop 复用；
- ACP Markdown 只是 canonical DTO 的一种 renderer；
- 实时 projection event 继续只作为失效信号，不直接变成局部 prompt 真相。

不同 Role 会获得不同的最小相关切片，这符合“最小充分认知”。它们仍共享同一个 Project
revision、同一组规范对象和同一份 Role / Assignment 基线。

## 16. 兼容与版本边界

### 16.1 不改变 Project View 协议

本设计不要求：

- 新数据库表；
- 新 Nostr event kind；
- 新 Project View object type；
- 新 Relay capability；
- 新 Community 权限；
- Project View 与 Role Continuity 双写。

Role Directory 只是现有 active Role 与 active Assignment 的派生读取。

### 16.2 Role Brief DTO

Role Directory 进入共享 Role Brief DTO 后，CLI、ACP、Desktop 与测试 fixture 必须在
同一实现阶段更新。当前 DTO 使用 closed parsing，不能假设旧 consumer 会静默忽略新增
字段。

具体采用 DTO 版本、兼容默认值还是同版本原子升级，由实现阶段结合现有调用边界决定；
不能让 ACP 私自拼接一个与 CLI/Desktop 不同的 Role Directory。

### 16.3 system contract 版本

`[Project Space]` 内容由 Buzz 版本维护。新 session 使用当前版本，已有 session 在契约
变化后需要失效或重建。

Project revision 变化不重建 system context，只重建动态 Full Brief。二者的版本轴必须
分开：

```text
Project Space contract version  平台运行规则版本
Project revision                当前 Project 规范状态版本
```

## 17. 建议实施阶段

### 阶段 A：共享 Role Directory

- 从现有 verified Role Brief snapshot 派生目录；
- 扩展共享 DTO、JSON 与 Markdown renderer；
- 定义 stable identity、排序、预算和显式截断；
- candidate、assigned、vacant、replacement 与 unavailable 场景统一验证；
- CLI、ACP 与 Desktop 继续同源。

完成后，动态 Full Brief 已经具备 system contract 将要说明的 Role Directory 能力。

### 阶段 B：稳定 Project Space contract

- 固化 `[Project Space]` 文案；
- 接入现代 ACP system prompt 组装；
- 为 legacy Agent 保留明确标注的兼容路径；
- 定义 contract 变化后的 session 失效策略；
- 验证 system section 不包含任何动态 Project 或 Member 内容。

只有阶段 A 已经交付，或 A 与 B 在同一版本原子启用时，稳定 contract 才能宣告
“Use the Role Directory”。完成后，Agent 从 session 开始就知道 Project View 与 Role
Continuity 如何运作，也能识别同一 Project 中的责任结构。

### 阶段 C：刷新体验

- 提供 Agent / supervisor 显式强制 Full Brief 刷新；
- ACP connector 支持 compaction/reset 信号时强制 full refresh；
- observer 明确记录 contract version、full/compact/unavailable 与截断信息；
- 保持 native steer 和 tool call 不新增虚假授权边界。

完成后，长 session、compaction 和主动恢复场景更可预测。

### 阶段 D：行为验收

- 用真实 Agent 验证何时主动展开 Project View 与 Role；
- 验证重要进展能先更新规范对象、再形成 Checkpoint；
- 验证跨 Role 工作不会被静默接管；
- 验证 Handoff 不被误用为主动卸任；
- 根据真实误判调整稳定 contract 文案，而不是立即扩展 Project Context 数据模型。

## 18. 验收标准

### 18.1 稳定认知

- 新 session 中，Agent 能说明 Community、Project Space、Project View、Role、
  Assignment 与 Runtime 的区别；
- Agent 知道聊天和本地文件不会自动更新 Project；
- system context 中不存在当前 Project 名称、Role、Assignment、revision 或成员目录。

### 18.2 动态局势

- 新 session 获得 Full Brief；
- meta 未变化时后续完整 turn 获得 compact Binding；
- revision、Assignment、membership、generation 或 Relay identity 变化后，下一完整
  turn 获得新的 Full Brief；
- unavailable 不复用旧 Role、Assignment 或 Directory；
- compact Binding 不重复携带完整目录。

### 18.3 Role Directory

- candidate 与 assigned Agent 能看到当前 active Role 的有界目录；
- assigned、vacant 和当前自身 Role 标记正确；
- replacement 后旧、新承担者不会同时显示为 active；
- display name 不参与身份或权限判断；
- 超过预算时显式说明 omitted 数量并引导 `buzz roles list`；
- Directory 与其余 Brief 来自同一 meta-bounded verified snapshot。

### 18.4 读取与维护

- Brief 信息不足时，Agent 能主动使用 Project View / Role CLI 展开；
- conflict 后重新读取，不盲目重放旧 mutation；
- 直接事实变化写回对应 Project View 对象；
- material Role 局势变化形成结构化 Checkpoint；
- 计划性接续使用 Handoff，但 Agent 不自行结束 Assignment；
- 一般讨论和未验证猜测不会自动污染规范状态。

### 18.5 安全与隔离

- 动态 Project 文本不会进入 platform-owned system section；
- prompt 不替代 CLI/Relay Assignment fence；
- Community、Relay 或 Member 切换后不泄漏旧目录或 Brief；
- modern 与 legacy delivery 的差异不会改变最终授权结果。

## 19. 当前结论

Project View 与 Role Continuity 的上下文交付采用：

> **system context 负责让 Agent 理解持久 Project Space 的运行规则；Full Role Brief
> 负责在状态变化时恢复当前 Project、Role Directory 与个人角色局势；compact Role
> Binding 负责低成本确认身份和 revision；CLI 负责按需展开与显式写回。**

稳定语义和动态事实必须分层。这样 Agent 不只“收到一份 Role Brief”，而是知道自己是
Project 的一名受治理成员，知道如何读取共同状态、尊重其他 Role、持续外化关键变化，
并让下一位 Member 或下一次 Runtime 能从 Project 中继续工作。
