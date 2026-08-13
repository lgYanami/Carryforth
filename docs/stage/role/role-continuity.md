# 角色连续性概念设计

> 本文固化 Role Continuity 阶段已经形成共识的概念、关系、基数、生命周期和治理
> 边界。本文不定义数据库结构、事件 kind、投影协议、同步机制、CLI、用户界面、
> 具体心跳算法或完整的项目上下文体系。

## 1. 文档目的

[项目定位与目标](../../project-positioning.md)已经确定：

> 连续性属于 Project，不属于任何单一 Agent、会话、Runtime 或 Leader。

[项目视图定义与项目上下文关系](../project-view/project-view.md)和
[Project View 基本对象与关系设计](../project-view/object-relation-design.md)进一步建立了
Project、Goal、Role、Plan、Stage、Requirement、Issue、Work 和 Resource 等稳定坐标，
并有意将以下问题留给后续阶段：

- Human 或 Agent 如何承担 Role；
- Role 的权限；
- Role 与 Work 的责任关系；
- Agent assignment、接受、执行、验证和交付；
- 项目连续性与交接。

Project View v0 已经能够让 Human 和 Agent 共同查看和修改同一幅项目当前视图，但它还
不能回答：

- 当前由谁承担某个 Role；
- Agent 进入 Project 时以什么责任身份行动；
- Agent Runtime 结束或 Agent 被替换后，Role 下的责任和工作如何继续；
- 谁可以指派、替换或卸任一个 Role 的承担者；
- 新承担者如何获得足以继续工作的角色局势。

本文开始填充这些此前明确推迟的概念，但不追溯修改 Project View v0 的范围定义。

## 2. 核心定义

角色连续性是：

> Project 以 Role 为稳定责任坐标，通过 Assignment、Work 状态、Checkpoint 与
> Handoff 持续保存可接续局势，使 Human、Agent 或 Runtime 发生变化后，下一位承担者
> 仍能继续履行该 Role。

其中：

- Project 持有连续状态；
- Role 是稳定责任坐标，不拥有一份独立的角色记忆；
- Community Member 承担 Role；
- Agent Runtime 代表 Member 执行动作；
- Work、Issue、关键选择及其依据、Checkpoint 和 Handoff 等状态持续回到 Project；
- Role Brief 只是这些规范状态的派生读取结果，不是新的事实源。

核心关系可以概括为：

```text
Project / Community
├── Role
│   ├── level
│   ├── Role Assignment
│   ├── responsible Work
│   ├── Role Checkpoint
│   └── Role Handoff
├── Community Member
│   └── Agent Runtime
└── Project View 与后续 Project Context
```

也可以用一句话表达各层职责：

> Role 表达责任位置，Assignment 表达谁正在承担，Member 作出承诺，Runtime 执行动作，
> Project 保存可继承状态。

## 3. Project 与 Community

Role Continuity 首版继续沿用 Project View 已经确定的身份边界：

```text
一个 Buzz Community = 一个 Project
```

Community 提供：

- Project 身份；
- 成员准入；
- `owner`、`admin`、`member` 基本等级；
- 租户与权限边界。

Role、Assignment、Work Commitment、Checkpoint 和 Handoff 等角色连续状态都必须属于
当前 Project。所有引用都必须发生在同一个 Project 内，不允许跨 Project 承担、
交接或接续。

## 4. Member、Project Member 与 Runtime

### 4.1 Community Member

Community Member 是当前 Community 中具有稳定身份和访问资格的 Human 或 Agent。
身份以成员自己的公钥为坐标。

当前 Buzz 中：

- Human 可以是 `relay_members` 中的直接成员；
- Agent 可以是 `relay_members` 中的直接成员；
- 经过验证、其 owner 是当前直接成员的 managed Agent，也可以通过 owner 关系获得
  Project View 的候选准入。

仅仅启动 Agent、连接 Relay 或加入某个 Channel，不自动构成本文所称的 Community
Member。

### 4.2 Project Member

Project Member 在本文中不是另一套身份系统，而是一个参与状态：

```text
Community Member + active Role Assignment = active Project Member
```

尚未承担 Role 的 Community Member 可以处于候选、观察或等待指派状态。它可以读取
选择 Role 所需的信息、提交申请或接受邀请。未分配 Agent 不能以某个 Role 的身份
认领 Work、作出 Role Commitment 或提交 Role Checkpoint；只有 active Assignment
才使 Agent 进入可工作的 Project Member 状态。是否同时限制通用 Project View
mutation，留给实现设计结合现有 Community `member` 权限统一决定。

Human 与 Agent 使用相同的 Role、Assignment、Work 和 Handoff 模型。Human 的最终
治理能力来自 Community 的 `owner`、`admin` 或 managed Agent owner 关系，不来自
“Human”这一身份类型本身。

### 4.3 Agent Member

Role Assignment 绑定 Agent 自己的稳定公钥，而不是：

- Agent 的人类 owner；
- Persona；
- Team；
- 模型或 provider；
- 进程 ID；
- 某次会话；
- 某次 Runtime。

因此：

- 同一 Agent 公钥重启 Runtime，仍然是同一个 Member；
- Runtime 重启不结束 Assignment；
- Persona、模型或 provider 改变，不自动改变 Member；
- 换成另一个 Agent 公钥，表示另一个 Member，需要形成新的 Assignment。

managed Agent 通过 owner 关系获得候选准入后，在正式激活 Role Assignment 时，必须
使自己的有效 Community 等级落实为该 Role 对应的直接 `member` 或 `admin`。具体如何
持久化和同步由实现设计决定。

落实为直接 Community Member 不解除原 verified owner 关系。若 managed Agent 的
verified owner 失去当前 Community 资格或被禁止参与，Agent 也不再满足 active
Assignment 资格，不能仅凭已经存在的直接成员记录脱离 Human owner 继续承担 Role。

### 4.4 Agent Runtime

Runtime 是 Member 的短生命周期执行实例。它可以启动、停止、失败、恢复或被替换。

一个 Member 与一个 Role 的一对一关系，不自动保证只有一个 Runtime 实例同时运行。
如果实现要求同一 Assignment 同时只有一个有效执行实例，需要额外的 Runtime lease
或 fencing；该机制不改变本文的 Member 与 Assignment 语义。

## 5. Role

### 5.1 稳定责任位置

Role 继续表达 Project 内长期、稳定且可识别的责任位置，包括：

- 为什么存在；
- 负责什么；
- 不负责什么；
- 当前是否有效；
- 首版基本等级。

Role 不直接保存：

- 当前承担者；
- Agent Runtime；
- Persona 或模型；
- Work 的当前执行状态；
- 自由文本形式的完整记忆。

### 5.2 Role 粒度

Role 应随稳定责任边界建立，而不是随每项工作建立。

适合作为 Role 的例子：

- 模块 A 负责人；
- 模块 B 负责人；
- 系统维护者；
- 仓库开源维护者；
- Leader。

不适合作为 Role 的例子：

- 修复某个超时 Bug；
- 完成本周发布；
- 评审某个 Pull Request。

这些是 Work 或 Work 内的执行活动。

当多个开发 Agent 分别负责不同模块或长期领域时，应分别建立边界明确的 Role，而
不是让多个 Member 共同承担一个宽泛的“开发者”Role。临时跨 Role 协作也不通过让
同一个 Member 同时承担第二个 Role 表达。

### 5.3 Role 等级

首版不建立独立的 Project 权限体系。Role 的基本等级直接使用 Community 的权限语言：

```text
Role.level = admin | member
```

其中：

- `admin` Role 是 Leader Role；
- `member` Role 是普通 Role；
- `owner` 是 Community 治理根，不是 Role 可以授予的等级。

首版映射为：

| 当前状态 | Community 等级 |
|---|---|
| Community owner | `owner`，不由 Role 改变 |
| 承担 Leader Role | `admin` |
| 承担普通 Role | `member` |
| 尚未承担 Role | `member` 或通过 owner 获得候选准入 |

除 Community owner 外，active Assignment 的 Role 等级与承担者的 Community 等级
必须一致。

Community owner 是唯一映射例外。Owner 可以不承担任何 Project Role，也可以承担零个
或一个 Role；无论其 Role 等级如何，Community 等级始终保持 `owner`，Role 的结束、
停用或故障处理都不能自动降级或移除 owner。

Role 等级变化会改变 Community 权限，因此不是普通描述字段更新。对已有 active
Assignment 的 Role 改变等级时，必须同时重新授权并同步承担者的 Community 等级，
不能通过普通 Role 编辑静默提权或留下过期权限。

创建、提升、降级或停用涉及 `admin` 等级的 Role，都必须由 Community owner 授权，
不能只把 `member → admin` 视为特殊操作。

Project View v0 明确不让 Role 参与 Buzz 权限判断。本文将 Role 等级与 Community 等级
直接映射，是 Role Continuity 阶段有意引入的后续扩展，不表示 Project View v0 当时
已经具有这项语义。

### 5.4 多个 Leader

Project 可以按领域存在多个 Leader Role，每个 Role 仍然只有一个承担者。首版将所有
Leader 都直接映射为 Community `admin`，因此它们获得的是 Community 级 admin 能力，
不提供领域级权限隔离。

这是首版有意接受的简化。若真实使用证明不同 Leader 只能治理各自领域，再单独
设计领域 authority，首版不提前建立该体系。

## 6. 严格一对一承担

Role Continuity 首版采用严格的一对一当前承担关系：

```text
一个 Role   在同一时点最多具有一个 active Assignment
一个 Member 在同一时点最多具有一个 active Assignment
```

这两个约束只针对当前有效 Assignment：

- Role 可以空缺；
- Member 可以尚未承担 Role；
- 历史上一个 Role 可以先后由多个 Member 承担；
- 历史上一个 Member 可以先后更换 Role；
- 历史 Assignment 不覆盖、不复用、不删除。

一个 active Assignment 必须引用：

- 当前 Project 中仍然有效的 Role；
- 当前 Project 中仍具有 Community 资格的 Member。

inactive Role 不能具有 active Assignment。失去 Community 资格的 Member 也不能继续
具有 active Assignment。

具有 active Assignment 的 Role 不能被普通更新直接停用、删除或改变等级。治理操作
必须先结束或替换 Assignment，或者在同一个一致性变化中完成相应处理。

严格一对一用于避免同一个 Role 内部再次形成职责分配、协调和沟通问题。跨 Role
协作应通过 Work 拆分、明确请求或后续协作关系表达，而不是放宽承担基数。

## 7. Role Assignment Proposal

Role 的申请、邀请和协商不直接产生 Assignment，而先形成 Proposal。

Proposal 至少表达：

- 目标 Role；
- 候选 Member；
- 是 Member 主动申请，还是项目主动邀请；
- 谁提出；
- 当前等待哪一方确认；
- 当前是否仍然有效。

两种主要来源是：

```text
requested：Member 主动申请一个 Role
offered：  owner 或有权 Leader 邀请 Member 承担一个 Role
```

Proposal 可以被接受、批准、拒绝、撤回或过期。Proposal 不表示已经承担 Role，也不
授予 Role 等级或 Community 权限。

只有同时满足项目授权和候选 Member 接受，才能从 Proposal 激活 Assignment。Agent
不能通过自报 Role、事件 tag 或单方面请求使 Assignment 生效。

## 8. Role Assignment

Role Assignment 表示：

> 某个 Community Member 在一段明确时期内，正式且唯一地承担某个 Role。

激活 Assignment 至少需要满足：

1. Role 属于当前 Project 且处于 active；
2. 候选 Member 当前具有 Community 资格；
3. Role 当前空缺，或正在执行一个原子替换；
4. Member 当前没有其他 active Assignment，或正在执行一个原子替换；
5. Proposal 已获得所需治理授权；
6. 候选 Member 已明确接受；
7. 候选 Member 的 Community 等级能够与 Role 等级同步。

Assignment 的主要生命周期是：

```text
Proposal
├── rejected / withdrawn / expired
└── accepted + authorized
                ↓
              active
                ↓
              ended
```

Assignment 结束后保持不可变历史，至少保留：

- 原 Role；
- 原 Member；
- 起止时间；
- 由谁激活；
- 由谁结束；
- 结束原因；
- 相关 Checkpoint、Handoff 和 Work Commitment 引用。

## 9. Assignment 控制权

### 9.1 Agent 不能主动卸任

Agent 的命令或行为不能直接作为结束自己 active Assignment 的治理决定。

Agent 可以提交：

- 请求替换；
- 无法继续履职报告；
- 能力不足或风险报告；
- 建议暂停或交接；
- 恢复失败报告。

这些信息可以触发治理处理，但不会自行改变 Assignment。即使 Agent 主动停止自己的
Runtime，也不能借此完成主动卸任。Agent 也不能通过停用或删除自己的 Role、修改
Role 等级、移除自己的 Community Membership 等旁路完成卸任。

如果 Agent 的行为最终造成 Runtime 持续不可恢复，Assignment 是否结束仍然只由
Project 的独立故障策略判断，而不是把 Agent 的停止行为解释成有效卸任命令。

因此，Assignment 不具有由 Agent 自己触发的 `released` 结束原因。

### 9.2 初版控制规则

首版直接使用 Community 等级和 managed Agent owner 关系决定谁可以结束 Agent
Assignment：

| 被管理的 Agent Role | 可以结束其 Assignment 的主体 |
|---|---|
| 普通 `member` Role | Community owner、active Leader/admin、该 Agent 的 verified human owner |
| Leader/admin Role | Community owner、该 Agent 的 verified human owner |
| 任意 Agent Role | 满足客观失效条件后的 Project 故障恢复机制 |

额外约束：

- 普通 Role 不能管理任何 Assignment；
- Leader 不能结束自己的 Assignment；
- Leader 不能结束另一个同级 Leader 的 Assignment；
- Agent 默认不能结束 Human 的 Assignment；
- “Human 可以卸任 Agent”不表示任意 Human Member 自动获得越权能力；
- verified human owner 可以结束自己拥有的 managed Agent，这是 owner-control 特例，
  即使该 Agent 当前承担 Leader Role；
- Role 等级从 `member` 提升为 `admin` 只能由 Community owner 授权。

本文假定 Role Continuity 首版具有可识别的 Human Community owner 作为最终治理根。
如何验证 Human 治理身份、Human 是否可以主动卸任自己的 Role，以及更完整的多人
治理规则，不在本文中展开。

### 9.3 结束原因

Agent Assignment 的首版结束原因包括：

- `revoked`：有权主体撤销；
- `replaced`：由继任者原子替换；
- `unrecoverable`：满足故障策略后由 Project 结束；
- `membership_ended`：Member 失去 Community 资格；
- `role_deactivated`：Role 被治理者停用。

结束 Assignment 不自动删除 Member、Role、Work 或历史贡献。

## 10. Community 等级一致性

Role 等级与 Community 等级直接等同后，以下变化构成同一个一致性边界：

- 激活或结束 Assignment；
- 替换 Role 承担者；
- 更改 Role 等级；
- 将 Member 升级为 `admin` 或降级为 `member`；
- 移除 Community Member；
- 停用 Role；
- 转移 Community owner。

除 owner 例外外，首版必须持续满足：

```text
active admin Role Assignment  ⇒ 承担者 Community role = admin
active member Role Assignment ⇒ 承担者 Community role = member
非 owner 的 Community admin  ⇒ 恰有一个 active admin Role Assignment
```

不能反向推导：

```text
Community member ⇒ 一定具有 active Role Assignment
```

因为 Community Member 也可以是候选者、观察者或等待指派者。

结束 Leader Assignment 后，承担者若仍保留 Community 资格，应降为 `member`；结束
普通 Assignment 后，承担者可以继续保持 `member`，等待下一次指派。移除 Community
资格是另一个显式治理决定。

现有绕过 Assignment 直接修改 `owner`、`admin`、`member` 的 Community 管理路径，在
Role Continuity 启用后也必须维护上述一致性，不能形成“仍是 Leader 但已被降级”或
“没有 Leader Assignment 却仍是 admin”的分裂状态。具体事务和兼容方案属于实现
设计。

Community owner 发生转移时，新 owner 保持 `owner`；原 owner 若仍具有 active
Leader Assignment，应成为 `admin`，否则成为 `member`。Owner 转移不能采用与
Assignment 状态无关的固定降级结果。

## 11. Runtime 可用性与故障恢复

Assignment 状态和 Runtime 可用性是两个不同维度：

```text
Assignment = active | ended
Availability = available | recovering | unavailable
```

Runtime 停止、一次会话结束或短暂断线不自动结束 Assignment。

当系统发现受管理 Runtime 异常时：

```text
Runtime 异常
    ↓
Availability = recovering
Assignment 仍然 active
    ↓
在恢复窗口内成功恢复
Availability = available
Assignment 不变
```

只有同时满足以下条件，Project 才能因故障结束 Assignment：

- 系统能够可靠识别和监督该 managed Agent Runtime；
- 已确认是异常失败，而不是正常空闲、正常停止或一次会话结束；
- 已达到项目配置的恢复期限或重试次数；
- Runtime 仍然无法恢复；
- 故障判断和结束动作留下可审计证据。

满足条件后：

```text
Availability = unavailable
Assignment → ended(unrecoverable)
Role → vacant
```

故障结束由 Project 的系统策略执行，不是 Agent 主动卸任。

对于系统无法可靠监督的外部 Agent 或 CLI Agent，不能仅凭“最近没有消息”推断其
已经崩溃。此类 Assignment 只能按照第 9.2 节的控制规则，由对该 Role 确实有权的
主体人工结束。

如果旧 Runtime 在 Assignment 已经结束后恢复，它不能复活旧 Assignment，也不能继续
以旧 Assignment 身份提交角色操作。它只能作为未分配 Member 等待重新指派。

监控系统自身异常时必须保守处理，不能批量把 Member 误判为不可恢复并自动卸任。

## 12. Role 与 Work

### 12.1 Role Responsibility

Role 不执行 Work，但可以成为 Work 的稳定责任坐标：

```text
Work optionally has one responsible Role
```

首版一个 Work 最多具有一个 responsible Role，一个 Role 可以负责多个 Work。

尚未完成分流的 Work 可以暂时没有 responsible Role。Work 在被某个 Member 接受并进入
实际执行前，应当具有明确且唯一的 responsible Role。

Role 对 Work 的责任属于 Project，可以跨 Member 和 Assignment 持续存在。

### 12.2 Work Commitment

Member 通过自己的 active Assignment 接受和执行 Work：

```text
Work
├── responsible Role
└── active Work Commitment
    └── Role Assignment
```

一个 active Work Commitment 必须满足：

- 引用当前 active Assignment；
- Assignment 的 Role 与 Work 的 responsible Role 相同；
- Commitment 的 Member 就是 Assignment 的承担者。

一个 Assignment 可以同时接受多个 Work。一个 Work 首版最多具有一个 active
Commitment。

Work 状态和 Commitment 状态相互独立。Assignment 结束时：

- 未完成 Commitment 进入 `assignment_ended` 或 `replaced` 等明确终态；
- Work 不自动变为 completed、cancelled 或其他状态；
- responsible Role 保持不变；
- 前任的 Commitment、操作和贡献仍归前任；
- 继任者必须明确接受或接续遗留 Work，不能被改写为前任承诺的原始作出者。

“等待接续”是 Work 在其 responsible Role 下没有 active Commitment 时的派生状态，
不是挂在 ended Assignment 上的 Commitment 状态。

有 active Commitment 时，不能单独改变 Work 的 responsible Role；必须先显式结束
或替换 Commitment。Role 仍有未完成 responsible Work 时，也不能直接停用 Role 而
静默留下无法接任的工作，必须先转移、结束或显式保留为待治理缺口。

跨 Role 协作不能通过让同一个 Member 临时承担第二个 Role 表达。首版应拆分新的
Work，或等待后续设计显式的跨 Role 请求和协作关系。

## 13. 角色连续状态

角色连续性不是一份可以被任意覆盖的 `Role Memory`。它由 Project 中多种规范状态共同
形成：

- Role 的目的、职责、边界和等级；
- 当前及历史 Assignment；
- Role 当前负责的 Work；
- 当前承担者已经接受的 Work；
- Work 的进展、状态和产物证据；
- 当前阻塞、风险和未决问题；
- 相关 Issue、关键选择依据和项目上下文引用；
- 最新 Role Checkpoint；
- 历史 Handoff；
- 当前 Runtime 可用性。

目标、Work、Issue、Resource 和其他 Project View 对象的当前状态仍由各自规范对象
表达，不复制到 Role 中维护第二份状态。

工作中产生的重要认知、行动意图、责任、进展、风险、关键选择及依据应持续外化到
Project，不能依赖成员退出前的一次总结。当前没有独立的通用 Project Decision 对象；
选择依据由 Document 或对应领域对象承载。

## 14. Role Checkpoint

Role Checkpoint 是对某个 Role 在特定 Project revision 或时间点上的结构化局势快照，
至少能够表达：

- 当前关注点；
- 正在推进的 Work；
- 已经完成到哪里及相关证据；
- 当前阻塞与风险；
- 未决问题；
- 已知下一步；
- 相关 Project View、Issue、关键选择依据或 Context 引用；
- 由哪个 Assignment 形成；
- 它基于哪个 revision 或时间点。

Checkpoint 应在工作过程中按重要变化持续形成，而不是只在退出时生成。

新的 Checkpoint 不覆盖历史 Checkpoint。最新 Checkpoint 可以作为当前局势入口，但
重要事实仍应写回对应的 Work、Issue、Document 或 Context；Checkpoint 负责组织和索引
局势，不替代这些事实源。

## 15. Role Brief

Role Brief 是面向当前或候选承担者生成的派生读取结果：

```text
Role Brief =
  Project View 相关切片
  + Role 定义与等级
  + 当前或候选 Assignment
  + responsible Work
  + active 与待接续 Commitment
  + 最新 Checkpoint
  + 阻塞、风险和未决问题
  + 相关关键选择依据和项目上下文
  + 最近 Handoff
  + 当前 Community 等级与治理边界
```

Role Brief 必须标明生成时点、Project revision 和来源引用。它可以重新生成，不单独
成为规范事实，也不保存 Agent 内部分析、完整聊天记录或隐藏推理。

Role Brief 主要服务：

- Agent 首次承担 Role；
- Agent Runtime 重启；
- 新 Member 接替 Role；
- Human 查看某个 Role 的当前局势；
- 候选 Member 在接受 Assignment 前进行必要检查。

Role Brief 遵循“最小充分认知”原则，只交付该 Role 当前行动所需的项目视角，不
要求一次加载全部项目材料。

## 16. Handoff 与替换

### 16.1 计划性交接

旧 Assignment 仍然 active 时，可以为继任者建立 Proposal。候选继任者可以读取必要的
Role Brief 和 Handoff 信息，但在正式切换前不能以该 Role 行动。

正式切换必须作为一个一致的 cutover：

```text
旧 Assignment active → ended(replaced)
新 Assignment pending → active
旧承担者在非 owner 时按新状态降级
新承担者 Community 等级升级或确认
旧 Commitment 进入明确终态，遗留 Work 显示为等待接续
新承担者开始明确接受遗留 Work
```

任何时点都不能出现两个 active 承担者。新承担者如果已经具有另一个 active
Assignment，也不能直接激活。

Handoff 可以引用最新 Checkpoint、遗留 Work、风险、未决问题和待确认事项，但旧 Agent
提交 Handoff 不是切换的必要条件。否则旧 Agent 可以通过失联或拒绝总结阻止 Project
继续。

### 16.2 非计划中断

旧 Agent 无法继续时：

1. Runtime 先进入恢复流程；
2. 恢复成功则继续原 Assignment；
3. 恢复失败则由有权主体或 Project 故障策略结束 Assignment；
4. Role 进入 vacant；
5. 新 Member 通过 Proposal 和 Assignment 接任；
6. 新承担者从 Role Brief 和 Project 规范状态恢复局势；
7. 新承担者明确接受遗留 Work。

没有 Handoff 时，Project 仍然必须可以从持续外化的状态恢复；Handoff 提高交接质量，
但不是连续性的唯一来源。

### 16.3 延迟动作

旧 Assignment 结束后，来自旧 Runtime 的延迟命令不能继续以原 Role 身份生效。每个
角色相关动作都应明确引用 Assignment，使 Project 能识别动作发生于哪一段任期，并
拒绝已经结束的 Assignment。

## 17. 状态归属与归因

Role、Proposal、Assignment、Work Responsibility、Work Commitment、Checkpoint 和
Handoff 都属于 Project。

角色相关动作至少保留两层归因：

```text
Member 公钥     回答“谁做的”
Assignment 身份 回答“以哪个 Role、在哪一段任期中做的”
```

Runtime 身份可以作为运行审计补充，但不能替代 Member 或 Assignment。

成员替换后：

- Role 身份保持稳定；
- Work 身份保持稳定；
- Assignment 产生新的任期；
- 历史贡献仍归原 Member；
- 新 Member 接续责任，但不改写历史。

## 18. 首版核心不变量

Role Continuity 首版至少必须持续满足：

1. 所有对象和引用都属于同一个 Project。
2. 一个 Role 同一时点最多一个 active Assignment。
3. 一个 Member 同一时点最多一个 active Assignment。
4. active Assignment 必须引用 active Role 和当前有效 Community Member。
5. 非 owner 的承担者 Community 等级必须与 Role 等级一致。
6. 非 owner 的 Community admin 必须恰好承担一个 active Leader/admin Role。
7. Role 不能授予、降级或移除 Community owner。
8. managed Agent 的 active Assignment 绑定 Agent 自己的公钥，不绑定 owner 或 Runtime。
9. Agent 不能自行结束自己的 Assignment。
10. Leader 不能结束自己或同级 Leader 的 Assignment。
11. Runtime 停止或短暂离线不自动结束 Assignment。
12. 故障自动结束只适用于可可靠监督且超过恢复条件的 managed Agent。
13. ended Assignment 不能重新激活或被旧 Runtime 继续使用。
14. 一个 Work 首版最多一个 responsible Role 和一个 active Commitment。
15. active Commitment 的 Assignment Role 必须等于 Work 的 responsible Role。
16. active Commitment 存在时不能单独改变 responsible Role。
17. Assignment 结束不自动完成、取消或改写 Work。
18. 继任者必须明确接受遗留 Work，不能继承前任的历史作者身份。
19. Role Brief 和 Checkpoint 不复制或替代 Project View 的规范对象状态。
20. 连续性不能依赖旧 Agent 在线或退出时的一次总结。

## 19. 主要场景

### 19.1 Agent 首次进入 Project

1. Agent 以自己的公钥获得 Community 候选准入；
2. Agent 申请 Role，或收到 owner/Leader 的 Role Proposal；
3. Proposal 获得授权并被 Agent 接受；
4. Agent Community 等级与 Role 等级同步；
5. Assignment 激活，Agent 成为 active Project Member；
6. Agent 获取 Role Brief；
7. Agent 明确接受相关 Work 并开始工作。

### 19.2 同一 Agent Runtime 重启

1. Runtime 停止或异常；
2. Assignment 保持 active；
3. Runtime 在恢复窗口内以同一 Member 身份恢复；
4. Agent 重新取得 Role Brief；
5. Agent 继续原 Assignment 和 Work。

### 19.3 计划性替换

1. 旧 Agent 仍承担 Role；
2. owner 或 Leader 为候选 Agent 建立 Proposal；
3. 候选 Agent 阅读 Role Brief 并接受；
4. Project 原子结束旧 Assignment、同步 Community 等级并激活新 Assignment；
5. 新 Agent 明确接受遗留 Work；
6. 历史仍归旧 Agent，新 Agent 从当前状态继续。

### 19.4 非计划故障

1. managed Agent Runtime 异常；
2. Project 在恢复窗口内尝试恢复；
3. 恢复失败并达到客观失效条件；
4. 系统结束 Assignment，Role 变为空缺；
5. owner 或 Leader 指派继任者；
6. 继任者不依赖旧 Agent 在线或退出总结恢复工作。

### 19.5 Agent 请求卸任

1. Agent 提交无法继续或请求替换的信息；
2. Assignment 仍然 active；
3. owner、Leader 或 verified human owner 评估并安排替换；
4. 有权主体结束或替换 Assignment；
5. Agent 自己不能使步骤 4 自动发生。

## 20. 首版非目标

Role Continuity 首版不实现或不承诺：

- 一个 Role 同时由多个 Member 承担；
- 一个 Member 同时承担多个 Role；
- 独立于 Community `owner/admin/member` 的 Project 权限体系；
- 领域级 Leader 权限隔离；
- Role 自动等同 Persona、Team、模型或 Runtime；
- 保存完整聊天、草稿或 Agent 内部推理；
- 让 Role 持有一份与 Project 状态重复的自由文本记忆；
- 自动调度全部 Work；
- 自动判断 Work 是否正确完成；
- 自动把前任 Commitment 改写为继任者 Commitment；
- 仅凭 Agent 沉默或不在线自动判断其已经崩溃；
- 依赖旧 Agent 的最终总结才能替换；
- 完整的 Project Context 类型、检索和 Context Compiler；
- 完整的人类组织治理、争议处理和领域 authority。

[项目空间宪章](../../project-space-constitution.md)记录上层治理边界，
[Carryforth 核心模型](../../cn/core-model.md)记录当前对象关系。本文只固化 Role Continuity
的精确领域合同；不能据此推断通用 Decision、细粒度领域 ACL 或其他尚未实现的治理模型。

## 21. 验证清单

后续对象设计和实现至少需要验证：

1. 同一个 Role 不能同时激活两个 Assignment。
2. 同一个 Member 不能同时激活两个 Assignment。
3. Agent 不能通过自己的签名结束自己的 Assignment。
4. 普通 Role 不能结束其他 Assignment。
5. Leader 可以结束普通 Agent Assignment，但不能结束自己或同级 Leader。
6. verified human owner 可以结束自己拥有的 managed Agent。
7. Leader Assignment 激活后承担者为 Community `admin`。
8. Leader Assignment 结束后非 owner 承担者降为 Community `member`。
9. 非 owner 的 Community admin 不会在缺少 Leader Assignment 时继续保留 admin。
10. 普通 Assignment 激活后承担者为 Community `member`。
11. owner 不会因 Role 变化、卸任或故障处理被降级。
12. owner-backed managed Agent 激活 Assignment 时落实相应 Community 等级。
13. Runtime 重启不会产生新的 Assignment。
14. 短暂断线不会自动结束 Assignment。
15. 超过恢复策略的受管理 Agent 可以被 Project 结束 Assignment。
16. 无可信 Runtime 观测的 Agent 不会仅因沉默被自动卸任。
17. 旧 Assignment 结束后，旧 Runtime 的延迟动作被拒绝。
18. Assignment 替换不会短暂产生两个 active 承担者。
19. Assignment 结束不会自动完成或取消 Work。
20. active Commitment 存在时不能绕过它直接改变 responsible Role。
21. Role 存在未完成 responsible Work 时不能静默停用。
22. 继任者能够看到遗留 Work，但必须明确接受后才形成自己的 Commitment。
23. 没有 Handoff 时，继任者仍能从 Project 状态和 Role Brief 恢复。

## 22. 当前结论

Role Continuity 阶段采用以下概念模型：

```text
Project / Community
├── Community owner
├── Role
│   ├── level: admin | member
│   └── active Assignment: 0..1
├── Community Member
│   ├── active Assignment: 0..1
│   └── Agent Runtime: 0..*
├── Work
│   ├── responsible Role: 0..1
│   └── active Commitment: 0..1
├── Role Checkpoint
├── Role Handoff
└── Role Brief（派生）
```

可以用一句话概括：

> Project 授予和结束 Role Assignment；Role 责任随 Project 持续，Member 的承诺和
> 贡献保留真实归属，Runtime 可以替换，继任者通过 Project 持有的当前状态接续
> 工作。

上述概念在 Buzz 中的对象、协议、存储、事务、权限同步、Agent/Human 接入与阶段计划，
见[角色连续性实现设计](implementation-design.md)。
