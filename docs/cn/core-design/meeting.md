# Meeting：让分布式上下文形成共同结论与项目产出

> 本文解释 Carryforth 的一项核心设计：为什么 Agent 自治的 Project Space 仍然需要
> Meeting，以及 Meeting 如何让 Human 与 Agent 从各自 Role、Work 和项目经历出发，
> 把不同但相关的上下文交叉聚合，形成可行动的共同结论，并将结果显式写回 Project。
>
> 本文讨论的是产品心智模型，不重新定义 Meeting wire、数据库状态机或权限协议。
> 精确协议语义见 [Meeting V2](../../stage/meeting/v2/meeting-v2.md)。

## 1. 核心判断

> Meeting 的本质，是让 Human 与 Agent 将分布在不同责任、工作路径和有限上下文窗口中的
> 相关信息带入同一个有边界的协商过程，形成一份可行动的共同结论，再把需要长期生效的结果
> 显式写回 Project。

可以把这个过程概括为：

```text
Human / Agent 各自持有的相关上下文
  │
  ├── Role、Work 与当前责任
  ├── 已读取的 Project View / Document / Context
  ├── 代码、工具观察和外部事实
  └── 对问题的不同判断、约束与风险认识
          │
          ▼
受控 Floor + canonical Speech + Directed Handoff
          │
          ▼
主持人维护的共享 Board
          │
          ▼
冻结的最终 Board：本次 Meeting 的可行动共同结论
          │
          ▼
普通领域命令 + 当前权限与 Revision 校验 + canonical 回读
          │
          ▼
Work / Document / Context / Checkpoint 等长期 Project 状态
```

Meeting 不是为了让更多 Agent 重复回答同一个问题，而是让不同参与者所接触的上下文发生作用。
它也不是把所有原始材料合并成一份巨大 Prompt；共享部分应当随着协商逐步收敛，而不是要求
任何一个 Agent 预先装下完整项目历史。

## 2. 为什么 Agent 自治仍然需要 Meeting

Agent 可以自主读取项目状态、执行 Work、维护 Document，也可以通过 Project Context 发现相关材料。
但一个跨领域问题往往大于任何单个 Agent 当前能够直接使用的上下文：

- 后端 Agent 了解数据模型、事务、迁移和故障边界；
- Desktop Agent 了解交互、用户状态、恢复路径和客户端约束；
- 开源维护 Agent 了解许可证、发布面、兼容合同和贡献者体验；
- Human 了解尚未完全写入系统的业务方向、价值取舍和现实风险；
- 每个参与者的 Runtime Context 都可能经过压缩、重建或进程替换。

如果只让一个 Agent 汇总全部材料，它需要重新发现并承载所有上下文；如果只依靠普通聊天，讨论又
容易散落在消息流中，没有稳定的共同状态、发言边界和结果收口。

Meeting 提供的是第三种方式：

1. 参与者仍保留各自有限、不同的上下文；
2. 只把当前问题真正需要的部分正式外化；
3. 通过受控发言发现冲突、缺口和互补信息；
4. 用共享 Board 保存当前已经收敛的内容；
5. 把最终需要长期生效的结果写回项目，而不是继续依赖参会者记忆。

因此，Meeting 的价值不来自 Agent 数量，而来自**上下文互补、显式协商和项目化产出**。

## 3. 三层状态不能混为一体

Meeting 同时接触三层状态，但每一层有不同的所有者和生命周期。

### 3.1 参与者上下文

每个 Human 或 Agent 从自己的位置进入 Meeting：

- 稳定 Member 身份；
- 当前 Role 与 Assignment；
- 正在承担或关注的 Work；
- 已经读取的项目对象与 Context 路径；
- 当前 Session 中仍然可用的工作材料；
- 通过代码、工具或外部系统取得的观察。

这些内容不会被自动复制成一份会议总记忆。隐藏推理、完整 Session 历史和没有被正式表达的材料，
不会因为加入 Roster 就成为 Meeting 状态。

当前 Agent 合同会在完整 Turn 前提供新鲜的 Role Context，并允许参与者在需要时有界读取 Project
View、Documents、Project Context、消息、代码和相关资源。语义路径查询也可以使用 Role、Work 等
坐标作为软相关性环境，但系统不会在每一个 Turn 自动运行查询，也不会为了制造差异而强迫不同
Agent 得到不同路径。

不同上下文应来自参与者长期承担的真实责任和工作经历，而不是由 Meeting 临时分配一份人为切割的
“Role 私有知识库”。如果多个参与者面对的相关事实确实相同，得到相同判断并不是失败。

### 3.2 Meeting 共享状态

被参与者正式带入协商的内容，通过 Meeting 协议成为共享状态：

- Roster 确定谁可以正式参与和行动；
- Intent 表达参与者是否需要发言；
- Floor、Offer、Grant 和 Handoff 控制发言推进；
- canonical Speech 保存正式表达；
- 当前 Board 保存主持人对目标、议程、进展、结论和未决问题的当前归纳；
- Close 或 Abort 给 Meeting 一个明确终态。

Speech 是“谁正式说了什么”的时间线，Board 是“当前共同走到了哪里”。二者不能互相替代。

Board 采用单主持人维护，而不是多人并发编辑。这样做不是把真理交给主持人，而是为多参与者协商
提供一个明确收敛点，避免共享状态因竞态、重复总结或相互覆盖而失去含义。当前实现持久保存
current / final Board 和 Meeting 控制记录，但不应把它描述成保证保留每一次 Board 替换的完整版本史。

### 3.3 Project 长期状态

即使一项结论已经进入 Board，它仍然只是 Meeting 内的共同结论。只有通过相应领域的普通写入，
它才会成为 Project 的规范状态，例如：

- 新建或更新 Requirement、Issue、Work、Resource 等 Project View 对象；
- 创建或修订 Project Document；
- 设置 Work Responsibility 或建立符合现有授权的 Commitment；
- 追加 Role Checkpoint；
- 用 Context Document 和 Edge 解释 Meeting 与物化坐标之间的真实关系。

项目状态属于 Project / Community，不属于主持人、参会 Agent 或执行这些写入的某个 Session。

## 4. “交叉聚合”不是把所有上下文混在一起

上下文交叉聚合包含三个动作。

### 4.1 暴露相关分片

参与者只需要带入足以支持当前贡献的事实、约束、证据和判断。其他参与者可以追问来源、要求读取
当前对象或指出缺口，但 Meeting 不要求先建立一份完整上下文全集。

### 4.2 让不同上下文互相约束

真正有价值的协商通常发生在交叉处：

- 后端方案满足事务正确性，但与 Desktop 的恢复体验冲突；
- 产品方向清晰，但受现有兼容合同或 Resource 能力限制；
- 一个 Work 局部可完成，却会破坏另一 Role 负责的发布边界；
- 两位 Agent 使用了不同 Revision，导致看似一致的结论实际基于不同事实。

Meeting 让这些差异在同一个正式过程里被发现、验证和修正，而不是等它们分别写入项目后再发生
隐蔽冲突。

### 4.3 压缩成可行动的共同状态

共享 Board 不复制每段原始材料。它应保留当前行动真正需要的内容：

- 问题和目标；
- 已确认的约束；
- 仍然存在的分歧或未知；
- 形成的选择及其适用边界；
- 需要显式写回的结果；
- 不能继续推进时的阻断原因。

这是一种有来源的压缩：详细事实仍应回到其 Project View 对象、Document、Meeting Speech、代码或
外部事实系统读取，Board 负责保存本次协商的当前前沿。

## 5. “共识”在这里意味着什么

本文所说的“共识”，是**足以指导本次 Meeting 后续行动的共同结论**。

它不必表示：

- 所有参与者逐字同意；
- 通过了投票、法定人数或多 Human 确认；
- 系统已经证明结论真实、完整或最优；
- 形成了一个独立的通用 Project Decision 对象；
- 所有风险和未知都已经消失。

当前 Meeting 没有通用投票、quorum、多 Human 审批或 Project Decision 领域。主持人负责判断讨论是否
已经形成足够明确的最终 Board，或是否应继续讨论、中止、保持未决。

因此，更准确的术语是“可行动共识”或“共同结论”：

```text
参与者贡献并不要求完全一致
        +
关键异议、约束和未知已被显式记录
        +
主持人能够形成一份明确、可执行或可停止的最终 Board
        =
本次 Meeting 的可行动共同结论
```

Close 只记录主持人对 Meeting 目标已经完成的协议声明，不自动证明事实正确、项目工作已经完成，
也不证明所有参会者达成了社会意义上的一致同意。

## 6. Human 是关键介入点，但不是强制审批者

Human 在 Meeting 中是第一类项目成员，而不是站在系统外部的旁观者。

Human 可以：

- 创建并主持 Meeting；
- 作为冻结 Roster 中的非主持参与者提交 Human Floor Request、接受或拒绝 Offer、正式发言和
  Handoff；Human 主持人则通过主持人的 self Intent 和 Floor 选择路径发言；
- 将方向、价值、约束、风险承受范围与项目外部现实带入协商；
- 在 Human 主持的 Action Finalization 中通过现有 Desktop、CLI 或其他业务界面完成写入；
- 在现有 Community 权限下执行独立于 Meeting 的项目治理操作。

Human Floor Request 具有确定性的**下一席**优先路径，使非主持 Human 不必等待主持人普通选择。
它不会撤销已经生效的 Grant，也不会打断正在进行的 Speech；当前发言结束后，最早的请求取得
下一次可用 Floor。尚未 ACK 的普通 Offer 则可以被它抢占。

但 Human 不是每场 Meeting 的必选审批者。系统允许全 Agent Meeting，也允许 Agent 主持。Human
介入的价值来自其项目身份、现实信息和治理责任，而不是“Human”标签自动授予额外业务权限。

非 Roster 的 Community owner 或 admin 可以在现有规则下拥有紧急中止等治理能力，但不会因此自动
取得该 Meeting 的发言权、主持权或 Board 编辑权。

## 7. 主持人不是 Project Leader

Meeting 需要一个不可转移的主持人来维护 Board、推进 Floor 并决定何时关闭或中止。主持人是该场
Meeting 的临时收敛角色，不是 Project 的所有者，也不等于 Role Continuity 中的 Leader。

两者的边界是：

| 主持人 | Project Leader |
|---|---|
| 由 Meeting Create 确定 | 由 admin Role、Community admin 和 active Assignment 共同建立 |
| 权力限于该 Meeting 的 Board、Floor、终态和行动收口 | 具备现行规则允许的项目治理能力 |
| 可以是 Human 或 Agent | 是稳定项目 Role 的当前承担者 |
| 不因主持身份获得 Project View、Document 或 Context 写权限 | 仍须服从具体领域权限、Revision 和生命周期 |
| Meeting 结束后主持职责结束 | Role 与责任跨 Meeting、Runtime 和 Session 延续 |

这使 Agent 自治 Meeting 可以在没有 Project Leader 持续在线主持的情况下运行，同时不会创造一条
绕过项目治理的隐式权限路径。

## 8. Action Finalization：从共同结论到显式产出

不是每场 Meeting 都需要产生业务写入。只有 action-capable Meeting 在最终 Board 明确要求行动时，
才进入 Action Finalization。

准确流程是：

```text
最终 Board 已更新或确认不变
  ↓
Floor 决定进入 Action Finalization
  ↓
Relay 冻结讨论和 Board，建立 current Action Run
  ↓
逻辑主持人使用普通领域命令执行 Board 已决定的行动
  ↓
每项写入继续接受原领域的身份、权限、Revision 与生命周期校验
  ↓
主持人按执行合同回读规范结果
  ↓
Human 显式确认，或 Agent 返回 COMPLETE
  ↓
Harness / Desktop 提交带精确 fence 的 actions-recorded ACK
  ↓
Relay 原子关闭 Action Run 与 Meeting
```

Action Finalization 有几个重要边界：

1. 它不能在冻结 Board 之外发明第二套计划或新的决定；
2. Board 和主持身份都不授予业务权限；
3. 它调用的是已有 Project View、Document、Context、Role 等普通业务入口；
4. 多领域写入不是一个跨系统事务，部分成功不会因后续 `BLOCK`、`RETURN_TO_BOARD` 或 `ABORT`
   自动回滚；
5. Agent 合同要求 canonical 回读，Human 通过显式确认承担相应完成判断；
6. Relay 校验的是当前主持身份、最终 Board、Action Run 和 completion ACK 的控制 fence，
   不证明所有业务结果的语义正确性；
7. 零业务写入但主持人确认“无需写入”的行动也可以正常收口。

`actions-recorded` 因而表示：**主持人确认最终 Board 中需要处理的行动已经完成处理。**
它不是 exactly-once 外部执行证明，也不是对现实结果的自动验收。

## 9. 上下文写回：让 Meeting 成为未来工作的来源

Meeting 结束后，它本身可以继续作为正式项目记录存在。当前 Community Meeting read 能力开启后，
冻结 Roster 控制参与和行动资格，当前 Community 成员可以在权限范围内读取 Meeting 记录；这使
Meeting 不再只是参会者的私有 Session。

当最终 Board 的物化创建或改变 Requirement、Work、Document 等持久坐标，并且这些坐标与本次
Meeting 之间存在值得长期解释的真实关系时，Agent 主持人的当前执行合同要求它在同一次
Action Finalization Turn 内、返回 `COMPLETE` 之前：

1. 完成并回读普通业务写入；
2. 创建或修订一份普通 Project Document，解释关系的原因、影响和边界；
3. 将当前 Meeting 与实际物化出的坐标 attach 到同一精确 Context Edge；
4. canonical 回读该 Edge。

Human 主持人可以通过现有 Project Context 入口显式完成同样的维护，并在确认行动完成时承担判断。
Relay 不会为 Human 路径统一收集这些业务 readback，也不会仅凭 completion ACK 推断 Context 已写回。

```text
Meeting M
  + Work W
  + Document D
        │
        └── exact Project Context Edge
                 │
                 └── Context Document：
                     说明 W / D 如何由 M 中的共同结论产生，
                     以及该结论的适用边界
```

这一步始终是显式的。系统不会因为存在 Speech、最终 Board、Close 或业务写入就自动创建 Edge；
如果没有真实的跨坐标解释关系，也不应为了形式完整而伪造 Context Document。

Meeting 坐标的 attach 还要经过来源领域验证。verified terminal Meeting 可以作为稳定坐标；处于
`finalizing_actions` 的 active Meeting 只有在 current Action Run、冻结 Board 和控制 fence 等条件
完整匹配时才可使用。客户端声明的 phase 或 summary 不能替代这些验证。

## 10. 为什么结果不依赖单一 Session

Meeting 的正确性和连续性来自：

- 稳定 Meeting identity；
- Relay 持有的 canonical roster、Board、State、Floor 和 Action Run；
- 稳定的逻辑主持 Member 身份；
- 精确 Board / run / window fence；
- 普通业务域自身的权限与 Revision 校验。

它不依赖：

- 某个模型进程持续存活；
- 固定物理工作槽；
- 从头到尾复用同一个 ACP Session；
- 主持 Agent 的隐藏历史仍留在上下文窗口；
- Project Leader 持续在线。

系统会优先复用已有 Meeting Session，以获得更好的局部上下文连续性；但当槽、进程或 Session
替换时，新的 Action Turn 必须从 frozen Board、canonical Meeting envelope、当前 Role Context 和
Action fence 重建完整输入。物理 Session 不是授权或正确性的来源。

这正是“连续性属于项目”的具体体现：临时执行者可以变化，规范 Meeting 状态和显式物化的项目结果
仍能被后来的 Human 与 Agent 验证、读取和继续处理。

## 11. 一个贯穿示例

假设项目正在讨论“如何在保持本地数据安全的前提下开放语义查询”：

### 11.1 不同参与者带入上下文

- Relay Agent 带入 query gate、签名、Provider 出域和数据库资源边界；
- Desktop Agent 带入超时体验、重试行为、错误页面和用户操作路径；
- 开源维护 Agent 带入 `.env.example`、启动脚本、公开文档和兼容标识；
- Human 带入当前只支持本地单 Relay、暂不面向生产的方向约束。

这些材料彼此相关，但没有任何一个参与者必须预先承载全部内容。

### 11.2 Meeting 形成共同结论

通过 Speech、追问和 Handoff，参与者发现：

- 不能为了本地易用性删除所有安全门；
- 当前单 Relay 不需要持续维护多 Pod Fleet 租约；
- Provider、Community query gate 和问题出境确认仍必须保留；
- Desktop 应明确区分暂时不可用与永久不支持。

主持人将这些内容整理成最终 Board，并保留尚未完成的生产多实例资格边界。

### 11.3 结果进入 Project

如果最终 Board 要求实施并且 Meeting 进入 Action Finalization，只有该 Meeting 不可变的逻辑主持人
可以执行本轮行动并完成 `actions-recorded` ACK。它仍必须具备每项业务操作自身要求的权限，例如：

- 更新配置与 Relay 实现；
- 更新运维 Document；
- 建立或更新 Work；
- 用 Context Document 解释本次 Meeting、配置 Work 和运维 Document 之间的关系；
- 回读所有规范结果并完成行动确认。

其他具备权限的成员可以在 Meeting 之外独立执行普通项目写入，但不能替代主持人完成当前
Action Run，也不能代为提交该 Meeting 的 completion ACK。

之后即使主持 Agent 退出、Project Leader 更换或原 ACP Session 消失，新成员仍能从 Project View、
Document、Context Edge 和 Meeting 记录理解结论及其来由。

## 12. Meeting 不是什么

| Meeting 不是 | 原因 |
|---|---|
| 普通多人聊天 | 它有固定 Roster、受控 Floor、canonical Speech、共享 Board 和明确终态 |
| 多个 Agent 隐藏推理的合并器 | 只有被正式外化的贡献进入 Meeting，共享状态不包含私有推理 |
| 自动上下文分片器 | 上下文差异来自真实 Role、Work 和经历，不应由系统人为制造 |
| 投票或一致同意系统 | 当前没有 quorum、投票或多 Human 确认协议 |
| 通用 Project Decision 域 | 共同结论先存在于 Board，需要通过现有领域对象显式表达 |
| Project Leader 命令 | 主持人负责 Meeting 收敛，但不因此取得项目治理或业务权限 |
| 自动业务工作流 | Action Finalization 调用普通业务入口，不代理或回滚它们 |
| 权限放大器 | Roster、Board、Handoff、相似路径和主持身份都不会扩大原有权限 |
| 正确性证明 | Close、签名和 ACK 证明协议状态与绑定，不证明结论真实或最优 |
| 永久运行的 Agent 团队 | Meeting 是有目标、有终态的临时协作生命周期 |

## 13. 由此得到的设计原则

1. **差异来自真实责任，不由 Meeting 伪造。** Role、Work 和项目经历决定参与者关注的路径。
2. **共享前沿优先于全量上下文合并。** Meeting 只聚合当前问题真正需要的内容。
3. **正式外化优先于隐式记忆。** 只有 Speech、Board 和显式项目写入能够跨 Session 可靠延续。
4. **单点收敛不等于单点所有。** 主持人维护共同状态，但结果属于 Meeting 与 Project。
5. **共同结论不等于自动真相。** 异议、未知、依据和边界必须能够被保留和回读。
6. **讨论与行动分离。** 讨论期外部只读，行动必须在冻结 Board 与现有业务授权下显式进行。
7. **会议产出与项目产出分离。** Board 是 Meeting 结论；领域写入才是 Project 当前状态。
8. **Context 写回必须真实且显式。** 不从会议文本或写入效果自动推断关系。
9. **逻辑身份优先于物理 Session。** 工作槽和模型会话可以替换，Relay fence 与项目状态保持连续。
10. **Human 是关键参与者而非万能审批者。** Human 能介入方向和约束，但仍服从项目身份与权限。

Meeting 最终解决的不是“怎样让 Agent 开会”，而是：

> 当一个项目问题所需的上下文分布在不同 Human、Agent、Role、Work 和有限 Runtime 中时，
> 如何让这些上下文在一个可治理的过程中相互作用，形成可行动的共同结论，并把真正需要长期
> 生效的结果交还给 Project。

## 继续阅读

- [Carryforth 核心模型](../core-model.md)
- [核心设计：Role Continuity](role-continuity.md)
- [核心设计：先有坐标，后有上下文](coordinate-and-context.md)
- [核心设计：Agent 自主的上下文环境感知 Project Context 图检索](context-aware-semantic-graph-retrieval.md)
- [Meeting V2](../../stage/meeting/v2/meeting-v2.md)
- [Meeting Action Finalization](../../stage/meeting/fix/meeting-action-finalization-logical-host-ack-simplification-implementation-design.md)
- [Meeting Action Finalization 中维护 Project Context](../../stage/project-context/meeting-action-finalization-context-write-implementation-design.md)
- [Project Context 领域规范](../../stage/project-context/project-context.md)
- [项目空间宪章](../../project-space-constitution.md)
- [当前状态与能力边界](../current-status.md)
