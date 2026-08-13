# Role Continuity：让责任跨越 Agent 任期持续存在

> 本文解释 Carryforth 的一项核心设计：如何用 Project 持有的 Role、Assignment、
> Work Responsibility、Commitment、Checkpoint、Handoff 和 Role Brief，让责任与工作在
> Human、Agent、模型、Session 或 Runtime 更换后仍可接续，同时保留每一段任期和贡献的真实归因。
>
> 本文讨论产品心智模型，不重新定义事件、数据库约束、CLI 或 Runtime supervisor 协议。
> 精确领域合同见 [Role Continuity](../../stage/role/role-continuity.md)。

## 1. 核心判断

> Role Continuity 不是把前任 Agent 的记忆迁移给继任者，而是让 Project 持续保存责任、
> 承担任期、工作承诺和已经外化的局势，使继任者能够从项目状态重新构造职责现场。

长期项目里，会变化的是承担者和执行载体：

- Human 或 Agent Member 会进入、离开或被替换；
- Agent 使用的模型、Provider 或 Persona 会改变；
- Session 会结束，上下文窗口会压缩；
- Runtime 会停止、失败、恢复或重建；
- Leader 也不是永久存在的单点。

但 Project 需要的责任不会因为这些变化自动消失。一个模块仍要维护，一项 Work 仍要推进，
尚未解决的阻塞、风险和下一步仍然属于 Project。

```text
Project
│
├── Role                         长期稳定的责任位置
│
├── Assignment A                Member A 承担 Role 的一段任期
│   ├── Work Commitment         A 在本任期明确接受的 Work
│   ├── Checkpoints             A 持续外化的局势
│   └── Handoff                 可选的交接补充
│
├── Assignment B                Member B 的新任期
│   └── 新的 Work Commitment    B 显式接续遗留 Work
│
└── Role Brief                  从当前规范状态派生的接续视图
```

这套机制不试图延长某个进程的生命，而是把连续性从进程、会话和个体记忆中移回 Project。

## 2. 先把五个问题分开

Role Continuity 能成立，是因为它没有用一个“当前负责人”字段同时表达所有事情。

| 模型 | 回答的问题 | 状态所有者 |
|---|---|---|
| Role | 项目长期需要什么责任位置？ | Project |
| Assignment | 当前哪个 Member 在哪一段任期承担它？ | Project |
| Work Responsibility | 哪个 Role 对这项 Work 长期负责？ | Project / Work |
| Work Commitment | 哪个 Assignment 的 Member 在本任期明确接受了它？ | Project |
| Runtime | 当前由哪个短生命周期执行实例运行？ | 运行控制面 |

Checkpoint、Handoff 和 Role Brief 则分别解决：

- **Checkpoint**：在任期内持续外化当前局势；
- **Handoff**：在需要交接时补充入口和未决事项；
- **Role Brief**：从当前规范状态重新生成有界的接续视图。

这些概念不能互相替代。Role 不是 Agent，Assignment 不是 Runtime，Commitment 不是 Work 状态，
Checkpoint 不是 Role 私有记忆，Role Brief 也不是第二份事实源。

## 3. Role：Project 持有的稳定责任位置

Role 表达 Project 内长期、稳定且可识别的责任边界：

- 为什么存在；
- 负责什么；
- 明确不负责什么；
- 当前是否 active；
- 当前治理等级。

Role 不直接保存：

- 当前承担者；
- Agent Runtime、Session、模型或 Persona；
- Work 的执行状态；
- 某个成员的自由文本记忆。

例如，“Desktop 负责人”可以在项目中长期存在。今天由 Agent A 承担，下一阶段由 Human B 或
Agent C 承担，Role 的身份、职责和 responsible Work 都不需要跟着换名字。

```text
Role: Desktop 负责人
  purpose          保持 Desktop 体验与 Relay 合同一致
  responsibilities 交互、恢复路径、客户端验证
  boundaries       不单方面改变 Relay 授权合同
```

Role 应对应稳定责任，不应为每一项临时任务创建。修复一次超时是 Work；长期维护 Desktop
恢复体验才适合作为 Role。

当前一个 Role 同一时点最多有一个 active Assignment，一个 Member 同一时点也最多有一个
active Assignment。历史上则可以存在多段不可改写的任期。

## 4. Assignment：Member 承担 Role 的有界任期

Assignment 表达：

> 某个稳定公钥身份的 Community Member，在一段明确任期内正式承担某个 Role。

“有界任期”不是指必须提前设置到期时间，而是指每次承担都有独立、不可复用的 Assignment 身份，
以及明确的开始和结束事实。

```text
Role R
  ├── Assignment A1: Member A, 2026-07 → 2026-08
  └── Assignment A2: Member B, 2026-08 → active
```

Assignment 绑定 Member 自己的稳定公钥，而不绑定：

- Human owner；
- Persona；
- Team；
- 模型或 Provider；
- 进程 ID；
- 某次 Session；
- 某个 Runtime。

因此：

- 同一 Agent 公钥更换模型、Provider、Session 或 Runtime，不自动改变 Member 或 Assignment；
- 换成另一个公钥就是另一个 Member，必须建立新的 Assignment；
- Runtime 停止、断线或重启不会自动结束 Assignment；
- ended Assignment 不会被重新激活，也不会被继任者复用。

### 4.1 Proposal 先于 Assignment

申请、邀请和协商先形成 Role Assignment Proposal。Proposal 只表达候选意愿和项目授权状态，
本身不授予 Role 权限。

Request 在创建时已经表达 candidate acceptance；Offer 在创建时已经表达 governor authorization，
另一侧仍须完成自己的确认。当前只禁止同一个 `(Role, candidate)` 同时存在多条 open Proposal：
同一 Role 可以面对多个候选者，同一候选者也可以面对多个 Role。严格的一对一约束只适用于
active Assignment；一个 Proposal 完成后，其他 Proposal 仍可能保持 open，但其旧 consistency fence
不能绕过当前 Assignment 状态完成激活。

只有候选接受、当前治理授权和 Project consistency fence 都满足时，Project 才原子创建 active
Assignment。Member 自报 Role、客户端 tag、加入 Channel 或启动 Runtime 都不能代替这一过程。

### 4.2 Assignment 是 Role-bearing 行为的授权坐标

当操作代表某个 Role 执行，例如追加 Checkpoint、Handoff、接受 Work Commitment 或执行 Leader
治理时，系统要求 signer 持有对应的 exact active Assignment。

Assignment 不是所有普通 Project 内容读写的统一 ACL。普通 Project View、Document 或 Context
操作仍按 Community membership、具体 operation authority、对象状态和领域 gate 分别授权。

### 4.3 当前治理根与 Leader 边界

Community owner 是唯一治理根，不由 Role 授予，也不要求 Assignment。Active Leader 则必须同时
拥有 Community `admin` 身份和 exact active `level=admin` Assignment。

当前治理边界是：

- owner 可以治理 `admin` 与 `member` Role / Assignment；
- Active Leader 只能治理普通 `member` Role / Assignment；
- `admin` Role 的创建、等级和生命周期变化只允许 owner 执行；
- Leader 不能结束自己的 Assignment，也不能结束同级 Leader；
- verified human owner 可以结束自己管理的 managed Agent Assignment，包括 Leader Assignment；
- managed Agent Leader 不能据 Role 文本职责治理 Human 或未知主体；
- 当前多个 Leader 都映射 Community 级 admin，没有按 Role 文字描述形成领域 ACL。

因此，“前端 Leader”或“后端 Leader”的职责描述不会自动限制其现有 admin 权限范围。若未来需要
领域级 authority，必须另行设计，不能从 Role 文本推断。

### 4.4 承担者不能用“停止工作”改写治理事实

当前实现禁止任何 assignee 单方面结束自己的 Assignment，包括 Human 与 Agent。承担者可以请求
替换或报告无法继续，但 Assignment 仍保持 active，直到有权治理者完成替换、撤销或可信恢复流程
认定不可恢复。

这样可以避免某个 Agent 通过关闭 Runtime、断开连接或提交一条自报状态，单方面把项目责任丢回空中。

## 5. Work Responsibility 与 Commitment：责任和承诺必须分离

这是 Role Continuity 最关键的分层之一。

```text
Work
├── responsible Role             跨 Assignment 持续的项目责任
└── active Work Commitment       某一 Assignment 当前的明确接受
```

### 5.1 Work Responsibility 跨任期持续

`responsible_role_id` 回答的是：**哪一个稳定 Role 对这项 Work 负责？**

它属于 Work 的当前 Project 状态，不属于当前 assignee。一个 Work 最多有一个 responsible Role，
一个 Role 可以负责多项 Work。

设置或清除 responsible Role 是治理动作，只能由 Community owner 或合格的 Active Leader 完成。
普通成员不能通过编辑 Work 绕过这一责任边界。只要 Work 仍有 active Commitment，系统就禁止
直接设置、清除或改派 responsible Role；必须先由 exact assignee release Commitment，
或者让 Assignment 的合法生命周期变化先终止 Commitment。Work 进入终态也会关闭 Commitment，
但终态 Work 本身不能再被重新承诺。

### 5.2 Commitment 只属于具体任期

Work Commitment 回答的是：**哪个 Assignment 的 Member 在这段任期内明确接受了这项 Work？**

一个 Work 当前最多有一个 active Commitment，一个 Assignment 可以同时持有多项 active Commitment。
Commitment 必须同时满足：

- Work 仍可执行；
- Work 已有 responsible Role；
- Assignment 仍 active；
- Assignment 的 Role 与 responsible Role 相同；
- signer 正是该 Assignment 的 Member。

Commitment 不是：

- Work 的所有权；
- Work 已完成的证明；
- Runtime 执行锁；
- 可以在成员之间转让的承诺；
- 对 Work 状态的替代。

Commitment 的结束原因也保持精确：assignee 主动结束为 `released`；同一 Assignment 原子重承诺为
`replaced`；Assignment 结束为 `assignment_ended`；Work 进入终态为 `work_closed`。

### 5.3 继任者接续责任，不继承前任承诺

Assignment 结束时：

- Work 不自动 completed、cancelled 或 reassigned；
- responsible Role 保持不变；
- 前任 Commitment 以 `assignment_ended` 结束；
- 前任的 Assignment、Commitment 和历史贡献继续归前任；
- 继任者必须以新的 Assignment 创建新的 Commitment。

```text
Work W ──responsible──> Role R

Assignment A / Member A
  └── Commitment C1 ──ended(assignment_ended)

Assignment B / Member B
  └── Commitment C2 ──active
```

因此，项目可以连续地说“Role R 仍负责 Work W”，同时诚实地说“Member A 承诺过 C1，
现在由 Member B 通过 C2 接续”。

## 6. Checkpoint：把连续性从退出总结改成持续外化

如果所有进展只存在于 Agent 的内部上下文，Role Continuity 仍然会在 Runtime 消失时失效。

Checkpoint 用结构化、追加式记录持续外化一个 Role 的当前局势，包括：

- 简洁的局势摘要；
- 当前关注点；
- 已有进展和证据；
- 阻塞；
- 风险；
- 未决问题；
- 下一步；
- 指向 Work、Issue、Assignment、Commitment 或项目事件的 typed references。

Checkpoint 应在工作发生重要变化时形成，而不是等到成员退出前才补一份总结。

### 6.1 Checkpoint 是追加历史，不是可覆盖的 Role Memory

每次 append 都产生新的 `checkpoint_id` 和 Project revision。旧 Checkpoint 不会被编辑或删除。

`supersedes_checkpoint_id` 只表示同一个 Role、同一个 Assignment、同一个作者追加了纠正记录；
被纠正的条目仍保留在规范历史中。Role Brief 选择最新 Checkpoint 作为当前入口，但完整历史仍可分页读取。

### 6.2 Checkpoint 的基线是“作者复核到哪里”

`based_on_project_revision` 记录作者形成该 Checkpoint 时复核过的 Project revision。
它不是“此后永远 current”的保证。Checkpoint 落地后 Project 仍可能继续变化，读取者需要结合
Checkpoint 自身的 `project_revision` 和当前 Project head 判断新旧。

### 6.3 Checkpoint 不复制规范事实

Checkpoint 负责组织局势，不替代事实所有者：

- Work 状态仍更新 Work；
- 阻断性问题仍更新 Issue；
- 长内容、方案和证据仍进入 Document；
- 跨对象原因和影响仍进入 Project Context；
- 外部执行结果仍由外部权威系统持有。

例如，Checkpoint 可以写“数据库资源门仍阻断验收，并引用 Issue I-7 与修复文档 D-4”，
但不能只在 Checkpoint 中把 I-7 宣布为已关闭而不更新 Issue。

## 7. Handoff：提高交接质量，但不能成为接续前置条件

Handoff 是追加式交接记录，可以关联：

- 来源 Assignment；
- 目标 Assignment（如果已经发生直接替换）；
- 最新 Checkpoint；
- 受影响的 Commitment；
- 未决事项和相关项目引用；
- 交接原因。

当前有两类 Handoff：

### 7.1 Member 主动补充的计划性交接

active assignee 可以在任期内追加 planned / other Handoff，提供更丰富的背景和未决事项。
这条记录本身**不会结束 Assignment**，也不会把 Work 或权限转给另一个 Member。

### 7.2 Project 生成的最小 cutover 记录

正式 replacement 或可信 `unrecoverable` 流程结束 Assignment 时，当前实现会生成最小
system Handoff，关联来源 Assignment 的最新 Checkpoint、终止的 Commitment 和等待接续的 Work。
普通 revoke 不会自动生成 Handoff；`membership_ended` 与 `role_deactivated` 是现有模型保留的结束原因，
也不能据此推断当前所有相关路径都会自动生成 Handoff。

因此，前任提交一份完整 Handoff 不是替换的必要条件。旧成员可能失联、拒绝总结或已经无法运行；
Project 仍必须能够依靠持续写回的 Work、Issue、Document、Context、Checkpoint 和系统 cutover
记录完成恢复。

> Handoff 改善交接，但连续性成立的前提是 Project 平时已经持有足够的规范状态。

## 8. Role Brief：从 Project 状态重新编译接续视图

Role Brief 是面向当前承担者或候选者生成的有界派生读取，不是持久化的 Role Memory。

当前 v3 machine-readable Brief 可以组合：

- Project、projection generation、Project revision 和 membership snapshot；
- Project Profile 与 Goals；
- bounded active Role directory；
- 当前 Assignment 或 open Proposals；
- 非终态 responsible Work，以及 committed / waiting 状态；
- Role-related Issues 和处理它们的 Work；
- 该 Role 的最新 Checkpoint；
- 最近三条 Handoff；
- 有界的一跳 Context 与 Document metadata / fetch commands；
- 每项来源的签名 projection 和 currentness 边界。

```text
Role Brief =
  verified Project snapshot
  + Role / Assignment
  + responsible Work / Commitment view
  + latest Checkpoint / recent Handoffs
  + bounded related objects and Context
  + source revisions
```

Role Brief 的作用是把分散在 Project 中的当前状态重新编译成一个最小、可验证的 Role 视角。
它可以重建，不应在冲突时覆盖 Project View、Document、Context 或 Role Continuity 的规范事实。

### 8.1 Role Brief 不是完整记忆

Role Brief 不包含：

- 前任完整聊天历史；
- Agent 未外化的内部推理；
- 所有 Project Documents 正文；
- 全部 Checkpoint / Handoff 历史；
- 自动推断的事实或权限。

它只提供接续工作的入口。需要更多材料时，继任者继续使用 canonical read、Role history、
Document 和 Project Context 按需展开。

### 8.2 Role Brief 的 currentness 有明确快照边界

Brief 根据 Relay-signed projections 和精确的 Project meta、generation、membership、Member 与 Relay
身份组装。它只表示生成时验证过的 snapshot；Project head 改变后，需要重新解析，不能永久复用旧 Brief。

客户端的 `generated_at` 是组装时间，不是 Relay canonical write time。

## 9. 三种接续场景

### 9.1 同一 Member 更换 Runtime、Session 或模型

```text
Member A + Assignment A
       │
Runtime 1 / Model X 结束
       │
Runtime 2 / Model Y 启动
       │
重新读取 current Role Brief
       │
继续同一 Assignment 与已有 Commitment
```

Member 公钥和 Assignment 没有变化，因此不需要创建新任期。新 Runtime 重新读取 Project 状态，
但无法自动继承旧模型未写回的思考或临时上下文。

### 9.2 计划性更换承担者

```text
旧 Assignment active
  → 新候选读取必要 Role Brief
  → Proposal 获得候选接受与治理授权
  → 原子结束旧 Assignment / 激活新 Assignment
  → 结束前任 Commitments并保留归因
  → 继任者显式接受遗留 Work
```

计划性 Handoff 可以提供补充，但切换不会因前任没有提交 Handoff 而被阻塞。

### 9.3 非计划中断

对于可可靠监督的 managed Runtime，Project 可以先尝试恢复；只有满足客观不可恢复条件、相应策略
明确启用并留下证据时，才可以结束 Assignment。自动 `unrecoverable` 当前默认关闭。

这里的恢复状态机与 Runtime evidence 不等于“系统已承诺为所有部署自动重新拉起远程进程”。
具体进程恢复能力取决于部署和 supervisor 实现；Role Continuity 只规定何时可以把运行证据用于
保守的 Assignment 恢复判断。

对于无法可靠监督的外部 Agent，不能因为它最近没有消息就推断已经崩溃。需要由有权主体显式处理
Assignment，再让继任者从 Project 状态恢复局势。

## 10. 真实归因：接续责任，但不改写贡献者

Member 主动形成的 Role-bearing 状态至少保留两层归因：

```text
Member 公钥       回答“谁做的”
Assignment ID    回答“以哪个 Role、在哪一段任期中做的”
```

Runtime ID、epoch 或 lease 可以作为额外运行证据，但不能替代 Member 与 Assignment。
system Handoff 是明确标记的系统生成记录：它关联来源 Assignment，但 `created_by` 为空，
不能被描述成前任亲自提交的贡献。

成员替换后：

- Role 身份保持；
- Work 身份与 responsible Role 保持；
- 新 Member 获得新的 Assignment；
- 新 Commitment 归新任期；
- 前任主动提交的 Checkpoint、Handoff、Commitment 和业务写入仍归前任；
- system Handoff 保留系统生成事实和来源 Assignment 关联，不冒充前任 authored contribution；
- 继任者接续责任，但不能冒充前任完成过那些工作。

这使 Project 同时拥有两种连续性：责任可以继续，历史归因不会被抹平。

## 11. Runtime supervision 不是 Role 授权

Runtime supervision、binding、lease 和 fence 属于运行控制面，用于运行证据、恢复、epoch / lease
协调、maintenance 和可选来源归因。

当前授权边界是：

1. Community admission；
2. 具体 operation authority；
3. Role-bearing 行为的 exact active Assignment；
4. 只有命令显式携带 Runtime fence 时，才验证 Runtime attribution。

未携带 Runtime attribution 本身既不授予、也不撤销 otherwise-valid 的业务权限。一旦主动携带，
就必须精确匹配 active binding、Runtime ID、epoch 和未过期 lease。

binding 的注册与撤销属于 Relay operator 控制面，Agent 或 Desktop 不能自行取得该能力。
missing binding / key、mismatch、expired 或 unknown supervision 状态会清除当前 fence，并把普通
Role 工作降级为未监督运行；它们不会阻断已经验证的 Role Brief 或普通 Role-bearing 操作。

但以下能力仍严格依赖 supervisor：Runtime evidence、epoch / lease、自动 `unrecoverable`，以及
maintenance 的 drain、freeze 和 ACK。缺少监督不会扩张这些能力，也不能通过“普通 Role 操作仍可用”
绕过 maintenance 的失败关闭边界。

### 11.1 当前不保证单 Runtime 排他写入

现行合同不保证一个 Assignment 同时只有一个可写进程。只要旧进程仍持有同一个 Member 私钥，
Assignment 仍 active，且命令没有显式携带 Runtime fence，它仍可能与新进程并行提交 Role-bearing
命令。Project revision CAS、receipt 和追加式历史提供冲突检测与审计，但不是 exactly-once 或
single-runtime writer 保证。

Assignment 结束后，旧任期的 Role-bearing 命令会被拒绝；如果该 Member 仍具有 Community 资格，
它仍可能执行只要求普通 Community 权限的操作。结束 Assignment 不等于撤销 Member 的全部项目访问。

## 12. 一个完整例子

假设 Project 有一个长期 Role：`Desktop 负责人`，它负责两项 Work：

- `W1：修复语义查询超时状态`；
- `W2：完成上下文图交互验收`。

Agent A 通过 Assignment A 承担该 Role，并分别建立 Commitment C1、C2。工作过程中，它持续追加
Checkpoint：已经修复什么、还被什么阻断、验收证据在哪里、下一步是什么。

后来 Project 通过原子 replacement 用 Agent B 替换 Agent A：

1. `W1`、`W2` 及其 responsible Role 不变；
2. Assignment A 结束，C1、C2 以 `assignment_ended` 结束；
3. Project 生成最小 Handoff，关联 Assignment A 的最新 Checkpoint 和两个受影响 Commitment，
   并明确标记为系统生成而不是 Agent A 的提交；
4. Agent B 通过新的 Proposal / Assignment B 接任；
5. Role Brief 从当前 Project 状态列出 Role、遗留 Work、最新 Checkpoint、近期 Handoff 和相关 Context；
6. Agent B 分别创建自己的 Commitment C3、C4；
7. A 的历史贡献继续归 A，B 从当前状态继续推进。

这里没有任何一步要求迁移 Agent A 的内部记忆，也没有把 C1 / C2 改写成 Agent B 的承诺。

## 13. 当前实现边界

当前代码已经实现：

- Role / Member 的严格 active Assignment 基数，以及同一 Role / candidate 的 open Proposal 唯一性；
- Work Responsibility 和 Work Commitment 分离；
- Assignment replacement 与不可复用历史；
- append-only Checkpoint / Handoff；
- replacement / recovery 的最小 system Handoff；
- v3 Role Brief JSON 的 verified snapshot、latest Checkpoint、recent Handoffs 和 bounded Context；
- Role-bearing Assignment 授权与可选 Runtime attribution 解耦；
- Assignment 结束后的 binding / lease revoke 与旧任期拒绝。

但当前自动接续链还有两个明确缺口：

- v3 machine-readable Role Brief JSON 已包含 `latest_checkpoint`、`recent_handoffs` 和
  `related_objects`；
- ACP full prompt 与 `cf roles brief --markdown` 使用的当前 Markdown renderer 尚未渲染这些字段。

因此，不能声称“新 Runtime 已经自动在 Prompt 中收到全部 Checkpoint 和 Handoff”。这些状态已经
存在并可通过 JSON或显式 Role history 读取，但默认 Markdown / ACP 注入仍需补齐。

当前 full Role Brief Markdown 还残留“Assignment plus current Runtime fence is the write fence”之类
旧文案，compact Role Binding 也提示每次写入前解析 Runtime fence；这与现行“Assignment 授权、
Runtime attribution 仅在显式携带时校验”的合同冲突。ACP 会另外追加 supervision 不是业务授权的
正确说明，但同一个 Prompt 仍可能出现相互矛盾的指导。在 renderer 修正前，调用方必须以当前
Relay / DB 授权合同为准，不能把该旧提示解释成 mandatory Runtime fence。

另一个读取边界是：Checkpoint / Handoff append 经 Relay 原子接纳并形成 signed projection，
但通用 `cf roles checkpoint/handoff append` 当前不会强制执行写后 projection readback。
准确说法应是“写入后可以 canonical 回读”，而不是“CLI 已自动完成回读证明”。

## 14. 非目标

Role Continuity 不试图：

- 保存或迁移完整 Agent 会话、隐藏推理或私有草稿；
- 让 Role 拥有一份与 Project 重复的自由文本记忆；
- 把 Role、Member、Assignment、Persona、模型和 Runtime 合并成一个身份；
- 让一个 Role 同时由多个 active Member 承担；
- 让一个 Member 同时承担多个 active Role；
- 让 Runtime 停止、沉默或 lease 过期自动结束 Assignment；
- 让 assignee 单方面卸任；
- 把 Commitment 当作 Work 完成、所有权或执行锁；
- 自动把前任 Commitment 转移给继任者；
- 依赖前任退出总结才能替换；
- 保证一个 Assignment 只有一个模型进程；
- 保证 exactly-once 外部执行；
- 让 Role Brief 取代规范对象或完整项目历史。

## 15. 由此得到的设计原则

1. **责任属于 Project。** Role 不随承担者、Session 或 Runtime 消失。
2. **责任位置与承担任期分离。** Role 稳定，Assignment 有界且不可复用。
3. **长期责任与具体承诺分离。** responsible Role 跨任期持续，Commitment 归属于具体 Assignment。
4. **接续不改写历史。** 继任者建立新 Assignment / Commitment，前任贡献保持原归因。
5. **持续外化优先于退出总结。** Checkpoint 在重要变化时追加，而不是只在离开时补写。
6. **Handoff 是增强，不是依赖。** 没有前任总结，Project 仍必须可恢复。
7. **Brief 是派生入口，不是第二事实源。** 所有关键内容都能回到 signed projection 和 canonical 对象。
8. **Runtime 是执行载体，不是业务授权源。** active Assignment 才是 Role-bearing 行为的授权坐标。
9. **恢复必须保守。** 不能从沉默、断线或监控故障推断 Member 已不可恢复。
10. **连续性不等于单进程排他。** 运行互斥、exactly-once 与责任连续是不同问题。

Role Continuity 最终解决的是：

> 当某个 Human、Agent、模型、Session 或 Runtime 不再继续时，如何让 Project 仍知道需要承担什么、
> 谁曾在哪一段任期承担、哪些 Work 尚未完成、当前局势和风险是什么，以及下一位承担者如何用自己的
> 新任期继续，而无需等待前任重新上线或提供最后一次总结。

## 继续阅读

- [Carryforth 核心模型](../core-model.md)
- [核心设计：先有坐标，后有上下文](coordinate-and-context.md)
- [核心设计：上下文环境感知的图语义检索](context-aware-semantic-graph-retrieval.md)
- [核心设计：Meeting](meeting.md)
- [Role Continuity 精确领域合同](../../stage/role/role-continuity.md)
- [Role Continuity 实现设计](../../stage/role/implementation-design.md)
- [Runtime Supervisor 与 Role 授权解耦](../../stage/bug/project-runtime-supervisor-binding-and-role-authorization-decoupling-fix-design.md)
- [项目空间宪章](../../project-space-constitution.md)
- [当前状态与能力边界](../current-status.md)
