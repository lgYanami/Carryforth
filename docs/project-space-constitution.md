# Carryforth 项目空间宪章

> 状态：现行设计与实现基线
>
> 对齐日期：2026-08-13
>
> 上位定位：[项目定位与目标](project-positioning.md)
>
> 模型导览：[Carryforth 核心模型](cn/core-model.md)
>
> 能力状态：[当前状态与能力边界](cn/current-status.md)

## 1. 文档目的与适用边界

本文定义 Carryforth 项目空间当前共同遵守的上层治理原则，并说明这些原则如何落在已经实现的
Project View v3、Role Continuity、Project Documents、Project Context、Meeting、消息、签名和
Community 权限模型上。

本文回答：

- 什么状态应当由 Project 持有，什么可以保留在成员本地；
- Human、Agent、Role、Assignment 与 Runtime 如何区分；
- Agent 可以在什么边界内自主行动，Human 保留哪些治理责任；
- 对话、文档、上下文关系、Meeting 结果和外部行动如何进入项目；
- 哪些读取结果是规范状态，哪些只是带来源的派生视图；
- 当前实现没有提供哪些通用治理对象，因而不能从旧概念推断系统能力。

本文不是 wire schema、数据库表、权限矩阵、部署手册或生产资格声明。精确对象关系、签名、
Revision、状态机和错误语义以对应领域规范与当前代码为准；功能是否可用还取决于 capability、
durable gate、成员权限和运行 readiness。

### 1.1 两类约束

本文同时包含两类内容：

- **宪章原则**：产品设计、实现和协作不应违反的上层约束；
- **现行合同**：当前 Relay、数据库、CLI 或 Desktop 已经采用并验证的边界。

除非明确写为“现行合同”，本文中的“必须”首先表示治理和设计要求，不等于声称代码已经自动
执行一套通用策略引擎。代码存在也不等于功能已经启用或完成生产资格化。

### 1.2 规范用语

- **必须**：不可默默绕过的边界；违反时不能把结果描述为符合 Carryforth 项目治理。
- **应当**：当前默认做法；偏离时需要明确理由、影响和后续处理。
- **可以**：宪章允许，但不自动产生权限、状态或产品承诺。

## 2. 当前项目模型

Carryforth 的核心判断是：

> 连续性属于项目，而不是某个 Agent。

当前实现中，一个 Carryforth Community 构成一个 Project 的根身份、成员准入、授权和数据边界。
Community 是当前技术边界，Project 是它所承载的长期产品与协作含义。

```text
Project / Community
│
├── Project View          当前一阶项目状态
├── Role Continuity       稳定责任与可替换执行者
├── Project Documents     稳定身份与不可变修订
├── Project Context       显式保存的二阶关系
├── Meetings              有边界的结构化协作
├── Channels / Messages   签名的日常协作记录
└── Members               Human 与 Agent 的稳定身份
```

Project 不等于某个代码仓库、Agent 会话、Leader、团队进程或 Desktop 中名为 **Projects** 的 Git
协作预览面。一个 Project 可以关联多个仓库和外部事实系统；这些外部系统仍然持有各自的权威事实。

## 3. 第一条：Project 是连续性的所有者

### 3.1 成员可以变化，Project 必须可继续

Human、Agent、Leader、模型、Provider、Persona、会话和 Runtime 都可以进入、离开、停止或被替换。
任何一个成员都不得成为项目身份、项目记忆或项目解释权的唯一持有者。

新成员应当能够从经授权的规范状态、当前 Revision、Work、Checkpoint、Handoff、Document 和
Context 恢复工作，而不是依赖旧 Runtime 仍然在线或提供最后一次总结。

Handoff 是重要的交接入口，但不是连续性成立的必要前提。没有 Handoff 时，Project 仍必须能够
依靠自身规范状态支持恢复；后续成员需要以新的 Assignment 和 Commitment 显式接续责任。

### 3.2 成员消失测试

判断一项信息能否只留在成员本地时，应当追问：

> 如果该成员此刻永久离开，这项信息的丢失是否会导致项目无法继续、其他成员形成错误判断、
> 某项承诺消失，或重要风险无法被发现？

答案为“是”时，成员必须把最小充分信息写回一个当前支持的项目表面。写回不要求公开完整聊天、
草稿、提示词、模型推理或工作过程。

### 3.3 Relay 是当前规范边界

Desktop、`cf`、Managed Agent 和其他客户端不能各自维护一份可竞争的项目真相。当前支持的架构中，
Relay 验证并持久化 Community-scoped 规范状态；客户端缓存、模型生成的摘要和局部视图只能作为
派生读取。Project View、Meeting 等规范对象自身的 source-owned `summary` 字段不属于这里所说的
派生摘要。

这里的“项目中心”不是隐藏一个负责理解和调度一切的中央 Agent。Relay 是状态与授权边界，
不是超级 Agent，也不垄断专业判断。

## 4. 第二条：规范状态、派生视图与外部事实必须分开

### 4.1 当前规范状态

当前主要规范面包括：

- Project View v3 的 Project Profile、Goal、Role、Plan、Stage、Requirement、Issue、Work、Resource；
- Role Proposal、Assignment、Work Responsibility、Work Commitment、Checkpoint 和 Handoff；
- Project Document 的稳定身份、Revision、current head 与 tombstone；
- Project Context 的 Edge / Hyperedge 及其 Context Document 绑定；
- Meeting 的 roster、Board、Floor、Speech、Handoff、关闭或中止等经验证状态；
- Community / Channel 范围内的签名消息和必要的治理记录。

这些对象并不形成一套可任意互相推导的状态机。只有对应领域命令明确执行且通过签名、权限、
Revision、生命周期和 Community 校验后，规范状态才会改变。

### 4.2 派生视图不是第二份事实源

Role Brief、客户端或模型生成的摘要、搜索索引、图布局、语义路径、健康或 readiness 读数以及
UI cache 都是派生读取。这里不包括 Project View、Meeting 等规范对象自身由来源维护的 `summary`
字段。派生读取必须保留来源、Revision、currentness 或适用边界，不能在冲突时覆盖规范对象，
也不能创造权限、责任、Project Context Edge 或项目决定。

语义查询得到的结果 Event 由 Relay 签名并绑定精确请求；`cf` 验证结果后派生的 `read_commands`
本身不带签名。调用者仍应通过这些命令回读当前权威对象。

### 4.3 外部事实仍属于外部权威系统

代码仓库、设计工具、工单系统、部署平台、客户系统和其他外部资源继续持有各自的现实状态。
Carryforth 可以保存 Resource、Guide、稳定坐标、版本、观察或项目意义，但不会因为登记了引用就：

- 复制外部权威；
- 自动安装、执行或取得秘密；
- 证明外部动作已经发生；
- 把工具返回成功解释成业务目标完成。

## 5. 第三条：影响项目的状态必须最小充分地写回

### 5.1 必须写回的情形

当信息或行为开始影响其他成员、未来行动或项目连续性时，应当写回。例如：

- 对 Work 作出承诺、改变进展或发现无法继续；
- 发现会改变项目判断的重要事实、未知、风险、Issue 或冲突；
- 建立会影响他人的依赖、约束或外部承诺；
- 形成需要后续成员继承的设计、解释、依据或适用边界；
- 准备修改共享对象或调用可能产生重大、外部、并发或不可逆效果的能力；
- 暂停、交接、请求替换或退出责任；
- 需要把 Meeting 或消息中的结果变成规范项目状态。

### 5.2 最小充分写回

适用时，写回应当包含：

- 结论、意图或状态变化；
- 作用范围与不适用范围；
- 必要依据和关键假设；
- 风险、不确定性与已知分歧；
- 责任、下一步和回读坐标。

写回必须使用现有领域对象，而不是把所有内容塞进一个自由文本“记忆”。例如：当前直接状态进入
Project View，长内容进入 Document，二阶关系进入 Project Context，责任局势进入 Checkpoint / Handoff，
正式协作进入 Meeting，日常协调进入 Channel / Message。

### 5.3 可以保留在成员本地的状态

尚未产生项目影响的草稿、假设、推演分支、提示词、模型内部分析、临时工具过程和个人呈现偏好
可以保留在成员本地。它们一旦成为项目行动、承诺或风险判断的依据，就必须留下足以支持后续检验
和继续工作的项目记录。

成员可以私下思考，但不能私下约束项目。

## 6. 第四条：身份、成员、Role、Assignment 与 Runtime 必须分离

### 6.1 稳定身份

Human 与 Agent 以稳定公钥身份成为 Community Member。相同公钥重启 Runtime，仍是同一个 Member；
更换公钥就是新的 Member，不能冒用前任身份或改写其历史贡献。

连接 Relay、加入 Channel、启动进程、配置模型或持有外部凭据，都不会自动形成 Community membership、
Role Assignment 或业务权限。

### 6.2 Role 与 Assignment

Role 是长期存在的责任位置；Assignment 是某个 Member 承担该 Role 的有界任期。当前现行合同包括：

- 一个 Role 同一时点最多有一个 active Assignment；
- 一个 Member 同一时点最多有一个 active Assignment；
- Proposal 需要候选者接受，并取得有权治理者授权后，才会原子激活或替换 Assignment；
- Assignment 任期保留历史，不能被后续承担者复用或改写；
- assignee 不能单方面结束自己的 Assignment，只能请求替换或报告无法继续；
- Runtime 停止、断线或更换模型不会自动结束 Assignment。

结束或替换 Assignment 仍需现行治理权限：Community owner 可以治理；Active Leader 只能治理
member Assignment，不能结束自己或同级 Leader；verified human owner 可以结束自己拥有的
managed Agent Assignment，即使该 Agent 当前承担 Leader Role；普通 Role 不能治理 Assignment。
自动 `unrecoverable` 仅适用于可监督且满足客观不可恢复条件的 Runtime，不能从沉默推断。

Community Member 可以没有 Assignment。当前通常把同时具备 Community membership 和 active Assignment
的成员称为 active Project Member，但 Assignment 不是所有普通 Project 内容读写的前置 ACL。

### 6.3 Runtime 与来源归因

Runtime 是 Agent 的短生命周期执行实例。Runtime supervision、binding、lease 和 fence 用于运行证据、
租约、恢复、maintenance 协调或显式来源归因，不是业务权限来源。

现行合同不保证同一 Assignment 同时只有一个可写 Runtime。只要旧进程仍持有 Member 私钥且
Assignment 仍然 active，它仍可能提交 Role-bearing command；真正的硬撤销坐标是 Assignment、
Community 权限和 ban，写入并发继续依靠 Revision / CAS 与追加式历史处理。

当前命令在显式携带 Runtime attribution 时必须精确校验；没有显式携带时，Runtime attribution
缺失本身既不授予、也不撤销 otherwise-valid 的业务权限，且不能用它替代 Community、Assignment
和领域权限检查。

### 6.4 当前授权层次

一次操作是否允许，必须按相关表面独立验证：

1. host / workspace 到 Community 的受信任绑定；
2. Community membership、ban 和 owner / admin / member 基础权限；
3. 领域 capability、durable gate 与运行 readiness；
4. 当操作代表 Role 时，exact active Assignment；
5. 对象 Revision、generation、生命周期和引用 currentness；
6. 调用者签名与必要的精确请求绑定；
7. 显式提供时的 Runtime attribution。

客户端 event tag、UI 开关、工具可达性、共享凭据和语义 query context 都不能创建或扩大权限。
Community 必须从受信任 host / workspace 解析，不能由客户端标签决定。

## 7. 第五条：责任通过 Role 与 Work 延续

### 7.1 治理根与 Leader

现行 Role Continuity 合同把唯一 Community owner 作为 Human 治理根；owner 权力不由 Role 授予，
owner 也不需要承担一个虚构的“Owner Role”。

Leader 是 `level=admin` 的 Role。Active Leader 必须同时具备 Community admin 与精确的 active admin
Assignment。当前可以存在多个 Leader，但没有领域级 Leader 权限隔离；不能把 Role 的文字职责
误读成技术上已经实现的细粒度 ACL。

Owner 或 Active Leader 可以治理 member Role；任何涉及 admin Role 的创建、等级或生命周期变化
只能由 owner 执行。普通成员可以请求 Role、处理自己的 Proposal，但不能自行改变 Role 定义。

### 7.2 Work、责任与 Commitment

Work 的长期责任锚定在 responsible Role，具体承担由该 Role 当前 assignee 的 Work Commitment 表达。
当前合同包括：

- 一项 Work 最多有一个 responsible Role；
- 一项 Work 最多有一个 active Commitment；
- Commitment 的 Assignment Role 必须匹配 responsible Role；
- Assignment 或 Commitment 结束不会自动完成、取消、重新分配或改写 Work；
- 继任者必须通过自己的 Assignment 和新 Commitment 显式接续遗留 Work。

设置或清除 responsible Role 是治理动作，只能由 Community owner 或 Active Leader 执行；assignee
只能通过与自己 exact active Assignment 匹配的 Commitment 接受或释放 Work，不能借普通 Work 编辑
绕过责任治理。

Checkpoint 与 Handoff 是追加式连续性记录，不是 Project View、Document、Issue 或 Work 的复制品。
系统不把某次总结当作新的全局事实源。

### 7.3 Leader 可替换

Leader 可以协调优先级、处理责任空缺和跨成员协作，但不是其他 Agent 的父进程、项目上下文的所有者
或项目继续存在的必要条件。A2A 在 Carryforth 中表示 Agent 共同依附 Project 协作，并不承诺某种
直接 peer-to-peer 协议；Agent 主要通过 Relay、Channel、项目对象、Meeting 和 `cf` 共享状态协作。
ACP 只连接 Managed Runtime 与 harness / client，不是 Agent-to-Agent 协议。

## 8. 第六条：Agent 在授权内自主，Human 保留治理责任

### 8.1 Agent 自主边界

Agent 是一等项目成员，可以在已验证的 Community、Role、Assignment、capability、对象生命周期和
风险边界内发现、提出、认领、拆分、执行和交接工作。

Agent 自主是一项治理原则，不是系统中已经存在的通用策略引擎。工具能够调用、模型能够生成、
Runtime 正在运行或多个 Agent 得到相同答案，都不能替代授权、证据或项目写回。

### 8.2 Human 保留的治理责任

Human 治理者继续负责：

- 项目目的、范围和价值边界；
- owner 级权限、成员准入与重大授权；
- 法律、伦理、安全、隐私、商业和最终责任；
- 重大、不可逆或长期外部风险的接受；
- 无法仅靠事实验证解决的价值冲突。

Human 治理不表示每位 Human 自动拥有所有权限，也不表示 Human 可以把未经验证的偏好写成事实。
权限仍由 owner、Community 等级、active Assignment、签名和具体领域合同表达。

### 8.3 紧急性不扩张权限

当前没有通用的 Emergency State 对象或自动扩权引擎。任何成员都可以停止自己的行动、报告风险、
请求帮助或触发现有的 disable / fail-closed 路径；但“紧急”标签不会授予读取、修改他人状态或操作
外部系统的新权限。进一步行动仍需使用既有授权，并留下适当记录。

## 9. 第七条：Project View 只表达明确登记的一阶状态

Project View v3 当前包含九类稳定对象：

- Project Profile；
- Goal；
- Role；
- Plan；
- Stage；
- Requirement；
- Issue；
- Work；
- Resource。

它回答项目是什么、希望达成什么、如何推进、处于哪里、有哪些职责、需求、问题、工作和资源。
它不自动解释所有原因、影响和隐含关系。

Project View 中的关系具有精确基数和 Revision 合同。例如 Work 必须处理一个 Requirement 或 Issue，
Stage 属于 Plan，Issue 的 `about`、规划位置和处理 Work 是不同维度。任何关系都不得依靠标题、文本
相似度或客户端猜测自动成立。

对象之间禁止隐式级联：改变 Plan / Stage 关系不会自动改变 Requirement、Issue 或 Work 状态；
完成 Work 不会自动完成 Requirement、Goal 或 Project；删除、替换或修订必须遵守对应生命周期。

完整合同见 [Project View](stage/project-view/project-view.md) 和
[对象关系设计](stage/project-view/object-relation-design.md)。

## 10. 第八条：Document、Resource 与 Context 分别承担内容、入口和关系

### 10.1 Project Document

Project Document 具有稳定 `document_id`、不可变完整 Revision 和明确 current Revision。并发冲突
不得静默覆盖；删除通过可验证的 tombstone 表达。

Document 可以承载设计、约束、解释、Meeting 结果和 Context 含义，但不是 API Key、私钥或凭据的
存储位置。完整合同见 [Project Document](stage/document/document.md)。

### 10.2 Resource 与 Guide

Resource 表达 Project 关联的资产或能力坐标，而不是资源本体。当前 v3 Resource 由名称、开放的
`resource_kind`、可选摘要和必需的 `guide_document_id` 组成；该字段必须指向一份 active
Project Document，作为 Resource 的 Guide。

登记 Resource 不会授予访问权限、下载内容、安装工具、读取秘密或执行命令。

### 10.3 Project View Context Reference

除 Resource 外的 active Project View 对象可以引用 Resource，或引用 live / pinned Revision 的
Document；Resource 本身只能引用 Document，不能再引用另一个 Resource。

这类 Context Reference 是对象直接持有的轻量相关资产引用，不等于 Project Context Edge，也不会
自动创建 Edge、复制内容或产生权限。

### 10.4 Project Context Edge

Project Context 显式保存“为什么这些对象相关”的二阶语义。当前模型是一条无向 Edge / Hyperedge：

- 同一 Community 内至少两个 Project View、Document 或 Meeting 坐标；
- 一份或多份普通 Project Document 作为 Context Document 解释关系；
- 同一精确归一化坐标集在一个 Project 中至多一条 Edge；
- 一份 Document 至多作为 Context Document 属于一条 Edge；
- active Edge 至少保留一份 Context Document，最后一份 detach 后 active Edge 消失；
- 坐标 tombstone 不会静默缩小或删除既有 Edge。

系统不会从聊天、标题或文本相似度自动推断或创建 Edge。完整合同见
[Project Context](stage/project-context/project-context.md)。

### 10.5 图语义路径查询

图语义查询是在已验证 Project Context 上运行的可选派生能力。自然语言问题、起始坐标以及 Role、
Work 等 query context 可以影响召回和排序，但 query context 不是过滤器、权限、行动门槛或语义正确性
保证。

查询需要独立 Provider、索引 generation、Community index/query gate、成员授权和问题数据出境确认。
它目前仍在相关性、资源隔离、长期稳定性和生产部署方面资格化，不能被描述为项目真相或生产承诺。

## 11. 第九条：消息与 Meeting 必须显式物化结果

### 11.1 消息不会自动成为项目状态

Channel / Message 是有签名和 Community / Channel 边界的协作记录，但一段对话、多个成员表示赞同、
模型总结或消息已送达，都不会自动修改 Project View、Document、Context 或 Work。

影响项目未来的结论必须由有权成员通过相应领域命令显式写回。系统不要求保存完整聊天、草稿或
模型内部推理。

### 11.2 Meeting 的当前定位

Meeting V2 是有边界的结构化协作对象，不是项目创建仪式、宪法法院、Human 唯一入口或所有项目
选择的必经审批。

当前模型采用固定 roster，由发起者担任主持人，并包含 Board、Floor、Speech、handoff、租约、
timeout、关闭或中止等状态。Human 和 Agent 都可以主持或参加，也允许 all-Agent Meeting。
roster 控制参与和行动资格；Community read 能力由独立 gate 和权限合同决定。

当前没有通用 Meeting 类型模板、法定人数、投票、多 Human 确认、动态 roster、主持权转移、
“创立 Meeting”或“宪章修改 Meeting”协议。

### 11.3 Meeting 不自动成立项目决定

Board 是主持人维护的当前归纳，Speech 和 handoff 记录协作过程，Close / Abort 表达 Meeting 生命周期。
这些内容本身不会自动成立一个通用 Project Decision，也不会修改 Project View。

Meeting 结果只有在有权成员通过现有业务界面或普通领域命令显式写入并回读后，才会成为 Work、
Document、Context、Checkpoint 或其他规范状态。对于 action-capable Meeting，Action Finalization
只是主持人执行这些普通业务操作并提交 `actions-recorded` ACK 的有界阶段；Meeting 不代理业务
写入，也不验证外部结果的语义。主持权不等于业务对象的写权限。

完整模型见 [Meeting V2](stage/meeting/v2/meeting-v2.md)。

## 12. 第十条：当前没有独立的通用 Decision 领域

观察、假设、建议、提案、聊天共识、Meeting Board、Close 和工具结果必须与规范项目变化分开。

当前实现没有一个覆盖“候选决议—成立—生效—暂停—替代—废止”的通用 Project Decision 对象和
状态机。因此不能仅给文本标注“Decision”就声称它已获得系统级约束力。

当某项选择影响项目时，应当：

1. 在 Document、Issue、Requirement、Work、Context、Checkpoint 或 Meeting 中保留必要依据；
2. 由有权成员执行真正需要的领域状态变化；
3. 对外部系统的变化在外部权威系统中完成并回读；
4. 明确区分讨论记录、依据、授权动作和实际效果。

约束力来自真实权限、领域对象、签名和外部权威状态，而不是来自文案标签。未来若引入独立 Decision
领域，必须经过单独设计、协议、迁移、权限和兼容性审查。

## 13. 第十一条：行动、验证与 Work 状态不得互相冒充

当前 Work 使用直接生命周期状态，并没有旧版宪章曾描述的通用 ExecutionAttempt、ResultSubmission、
Verification、Acceptance 和 CompletionRecognition 对象链。

即便如此，所有成员仍必须遵守以下原则：

- 行动意图不等于命令已经发送；
- 工具返回成功不等于外部效果已经发生；
- 产出存在不等于满足 Requirement 或解决 Issue；
- 测试通过不等于结果已经被采用、部署或获得风险接受；
- Work 标记完成不自动关闭父对象、依赖、风险或外部副作用；
- 超时、取消和连接失败不证明外部动作没有产生部分效果；
- 重试可能产生副作用时，必须先对账或使用可靠幂等边界。

有项目影响的行动应当把必要意图、结果、证据、限制和后续责任写回当前支持的对象。未来若引入更
完整的执行—验证—接受模型，不能通过隐式级联改写既有事实。

## 14. 第十二条：权限、可见性与秘密必须诚实表达

### 14.1 当前可见性边界

当前实现主要提供：

- Community 范围的 Project View、Document 与 Context 表面；
- Channel membership 范围的消息；
- Meeting roster 与独立 Community read gate 所控制的 Meeting 表面；
- 成员本地且尚未产生项目影响的临时状态。

当前没有一个适用于所有对象和字段的通用细粒度“项目受限状态”ACL，也没有自动生成脱敏依赖信号
或“最小充分上下文证明”的框架。文档不得把这类未来设计描述为现行能力。

Project 内共享不等于公开互联网可见；不可见也不等于对象不存在。客户端必须把无权限、能力未启用、
依赖暂不可用、对象不存在和数据冲突区分处理，不能以空结果掩盖边界。

### 14.2 秘密不进入项目内容

API Key、私钥、真实凭据、访问令牌和不应共享的敏感内容不得写入 Project Document、Project View、
Context、Message、Meeting、日志或测试夹具。它们必须留在受控 secret / keyring / 本地私有环境中。

### 14.3 跨 Project 失败关闭

规范对象、Role Continuity 实体、Document、Context 坐标、Meeting 和引用必须属于同一 Community。
跨 Project 读取、引用、归因或写入必须失败关闭，不能由客户端标签或文本内容重定向租户。

## 15. 第十三条：冲突、修订和历史不得被静默抹平

### 15.1 区分事实、判断和分歧

共享基线不等于强制共识。成员必须区分事实、来源、假设、建议、未知和分歧。多个 Agent 重复同一
结论不自动构成多份独立证据。

发现冲突时，应当保存准确对象、Revision、作用域、证据和受影响行动。无法及时解决的重要歧义
应当保持可见，可以通过 Issue、Document、Meeting 或后续 Work 处理；不得由摘要或模型静默选边。

### 15.2 修正通过新历史发生

已经成为项目依据的签名事件、Document Revision、Assignment 任期、Checkpoint、Handoff 和其他
规范记录不得无痕改写。更正、替代、tombstone、恢复和补偿应当形成新的可追溯状态，并保留原记录
曾经产生的影响。

乐观并发冲突必须显式返回，不能自动覆盖或把旧 Revision 当作当前状态。

### 15.3 审计能力的真实边界

当前审计哈希链用于发现链内不一致和篡改，但不是外部不可变账本、非抵赖证明或合规认证。
拥有底层数据库写权限的攻击者可能重算无密钥哈希链；部分审计路径是异步 best-effort，不能声称
每个外部效果都与领域事务原子记录。

因此，历史可信度来自签名、Revision、领域约束、回读和审计证据的组合，而不是单一“已审计”标签。

## 16. 第十四条：能力门与成熟度不能被文档绕过

以下事实必须始终分开：

- 代码已存在；
- 进程已启动；
- Desktop 预览开关已打开；
- Relay 已广告 capability；
- Community durable gate 已启用；
- 当前成员已获授权；
- 数据和依赖处于 ready 状态；
- 功能完成本地、发布或生产资格化。

`./start.sh` 或 `just start` 只建立本地源码开发栈，不会替 owner 或 operator 初始化 Project View、
打开 Community gate、确认 Provider 数据出境或授予成员权限。

当前仓库仍处于活跃开发和首次独立开源发布准备阶段，仅供本地源码构建、评估与参考学习。
语义查询、Meeting、Git Projects 和部分 Desktop 表面仍受预览或资格化边界约束。本文不能被用来
宣称生产就绪、多实例安全、稳定升级或平台支持。

## 17. 当前明确没有实现的通用治理模型

旧版宪章曾把下列概念描述为已经存在的项目制度：

- 通用 Project Decision 及成立、生效、暂停、替代和废止状态机；
- Context Requirement、Dynamic Context View、Context Gap 与充分性证明；
- Execution Attempt、Result Submission、Acceptance 和独立 Completion Recognition；
- Continuity Assurance Requirement、Coverage、Evaluation、Gap 和 Project Health View；
- 通用 Emergency State 与自动扩权；
- 面向所有项目对象的字段级受限状态和脱敏依赖信号；
- Meeting 的模板、法定人数、投票、多人确认、创立项目或修改宪章协议。

这些概念有些仍可作为未来研究方向，但它们不是当前规范对象、权限来源或产品保证。任何后续实现
都必须通过独立领域设计、威胁模型、协议、迁移、兼容性和资格化流程引入，不能仅凭本文件旧版本
恢复为“现行能力”。

## 18. 解释、变更与文档层级

### 18.1 文档层级

- [项目定位与目标](project-positioning.md)回答 Carryforth 为什么存在、希望成为什么；
- 本宪章定义当前不可默默跨越的项目治理边界；
- [核心模型](cn/core-model.md)解释当前对象及关系；
- 领域规范定义精确协议、状态机、权限和生命周期；
- [当前状态](cn/current-status.md)说明哪些能力已实现、需启用、仍在资格化或尚未承诺；
- 代码、migration、schema 与测试是当前可执行合同。

文档与实现发生冲突时，不得挑选更宽松的一方扩大权限。应当记录不一致、按安全边界失败关闭，
并通过同一次修订让文档、代码和测试重新一致。

### 18.2 宪章变更

当前没有专用的“宪章 Meeting”或链上修宪协议。修改本宪章采用仓库正常的审阅和变更流程，必须：

- 说明变更原因、适用范围和兼容性影响；
- 区分治理原则、现行实现和未来设计；
- 同步受影响的领域规范、中文导览、状态文档和测试；
- 不通过措辞变化默默扩大权限、开放 gate 或改写既有历史。

### 18.3 本次修订的取代关系

本版取代“第一版共识”中与当前实现不一致的未来治理体系描述。旧版所保留的核心原则——连续性
属于项目、影响触发写回、身份与 Runtime 分离、Agent 授权内自主、Human 保留治理责任、显式状态
变化和历史可追溯——继续有效，并由当前领域对象承载。

## 19. 参考规范

- [项目定位与目标](project-positioning.md)
- [Carryforth 核心模型](cn/core-model.md)
- [系统概览](cn/system-overview.md)
- [当前状态与能力边界](cn/current-status.md)
- [Project View](stage/project-view/project-view.md)
- [Project View 对象关系设计](stage/project-view/object-relation-design.md)
- [Role Continuity](stage/role/role-continuity.md)
- [Project Document](stage/document/document.md)
- [Project Context](stage/project-context/project-context.md)
- [Meeting V2](stage/meeting/v2/meeting-v2.md)
- [语义 pgvector 运维](semantic-pgvector-operations.md)
- [ARCHITECTURE.md](../ARCHITECTURE.md)
- [SECURITY.md](../SECURITY.md)
