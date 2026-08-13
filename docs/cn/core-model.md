# Carryforth 核心模型

> 本文介绍 Carryforth 当前产品模型及它们之间的关系。
> 它不是 wire schema、数据库结构或权限协议的替代品；精确合同以对应领域代码和设计文档为准。

## 1. 核心判断

Carryforth 的起点是：

> 连续性属于项目，而不是某个 Agent。

项目长期存在；Human、Agent、Leader 和具体 Runtime 都可以进入、离开、停止或被替换。
需要持续保存的是项目对自身的描述、目标、责任、工作、文档、上下文、关键选择及依据和当前局势。

当前实现中，一个 Carryforth Community 构成一个 Project 的身份、授权与数据边界。
Community 不是项目的全部产品含义，但它提供项目根身份、成员准入、租户范围和基础权限。

```text
Project / Community
│
├── Project View            项目当前的一阶状态
├── Role Continuity         稳定责任与可替换执行者
├── Project Documents       可修订的项目内容
├── Project Context         跨对象的二阶语义
├── Meetings                正式协作与结果
└── Members                 Human 与 Agent
```

## 2. Project View

Project View 是项目在当前时点的直接可见面。它让成员无需复杂推断，就能回答：
项目是什么、要做什么、进行到哪里、有哪些稳定责任位置，以及当前有哪些需求、问题、工作和资源。

Project View 包含九类可稳定引用的对象。

### 2.1 Project Profile

Project Profile 是 Project 唯一的一对一描述面，回答：

- 这是什么项目；
- 项目为什么存在；
- 项目要解决什么问题；
- 项目的基本范围是什么。

Project 是长期存在的根对象，Project Profile 是可以查看和修订的项目描述。
它们不是同一个对象，也不会因为 Profile 内容变化而改变 Project 身份。

### 2.2 Goal

Goal 表达项目希望达到的上层结果。一个 Project 至少有一个 Goal，也可以同时维护多个并列目标。

Goal 可以组织零个或多个 Plan，但系统不会自动判断 Goal 是否已经达成，
也不会从 Goal 自动推导 Requirement、Issue 或 Work。

### 2.3 Role

Role 是 Project 内稳定、可识别的语义责任位置，说明它为什么存在、负责什么以及不负责什么。

Role 不是 Persona、模型、进程、会话，也不是某个成员本身。
“谁在承担这个 Role”由 Role Continuity 中的 Assignment 表达，Role 自身保持稳定。

### 2.4 Plan

Plan 表达项目如何推进的规划逻辑和结构。它可以关联一个 Goal，也可以作为未关联 Goal 的
Project Plan 独立存在。

Plan 通过 Stage 组织 Requirement 和 Issue。Plan 关联 Goal 只表示当前组织位置，
不自动意味着 Plan 完成就足以使 Goal 达成。

### 2.5 Stage

Stage 是 Plan 内用于表达分段、位置或推进状态的稳定坐标。每个 Stage 必须属于一个 Plan，
但 Plan 内的 Stage 不必构成单一线性序列，也可以表达并行或分支结构。

Requirement 和 Issue 可以被规划进 Stage；Stage 本身不会因为这些对象状态变化而自动改变状态。

### 2.6 Requirement

Requirement 表达项目希望实现、改变或满足什么。它可以尚未进入规划，也可以进入一个 Stage，
并由零个或多个 Work 处理。

Requirement 回答的是“希望做到什么”，不同于 Issue 所表达的“已经发现什么问题”。

### 2.7 Issue

Issue 表达项目已经发现的问题、缺口、异常、反馈或阻塞。

Issue 可以：

- 尚未进入规划，或被安排到一个 Stage；
- 通过 `about` 指向一个 Project View 对象，说明问题出现在哪里；
- 由零个或多个 Work 处理。

Issue 的规划位置、问题定位和实际处理工作是三个独立维度。系统不会仅凭 `about` 关系
推导责任人、处理阶段或解决方案。

### 2.8 Work

Work 是项目为了处理一个 Requirement 或 Issue 而安排的基本执行单位。
Human 或 Agent 接受和执行的是 Work，而不是直接“执行一个 Goal”或“承担一个 Stage”。

每个 Work 必须有且只有一个主要处理对象：一个 Requirement 或一个 Issue。
如果实际行动看起来同时处理多个对象，应拆分 Work，或明确唯一的主要处理对象。

### 2.9 Resource

Resource 表达项目关联的稳定入口，例如代码仓库、设计、文档、服务、环境或已有产物。

当前 v3 Resource 本身不保存 locator，而是由 `name`、开放的 `resource_kind`、可选摘要和必需的
`guide_document_id` 构成；Guide 是一份普通 Project Document，用来说明如何找到和使用该资源。

Resource 不是外部资产本身的复制，也不会因为被登记就自动安装工具、读取秘密或执行代码。
现有 NIP-34 Repository 在 Project View 中属于 Repository Resource，而不等同于 Project。

### 2.10 Project View Context Reference

除 Resource 外的 active Project View 对象可以携带规范化的 Context Reference，指向相关 Resource，
或指向一份 live / pinned Revision 的 Project Document；Resource 自身只能引用 Document，
不能再引用另一个 Resource。Context Reference 只表示“这个资产与当前对象相关”，
不会自动授权、安装、执行或把引用内容变成对象字段。

Project View Context Reference 与第 6 节的 Project Context Edge 是两种不同关系：

- Context Reference 是单个 Project View 对象直接持有的轻量相关资产引用；
- Context Edge 是两个或更多 Project 坐标共同参与、由 Context Document 解释的无向超边。

二者不会互相自动推导。Resource 的 primary Guide 也由独立 `guide_document_id` 表达，
不能从普通 Context Reference 集合猜测。

### 2.11 Project View 的主要关系

下面的关系图表达常见阅读顺序，不是界面嵌套或数据库所有权：

```text
Goal?
  └── Plan
       └── Stage
            ├── Requirement
            │    └── Work[]
            └── Issue
                 └── Work[]

Issue ── about? ──> 任一 Project View 对象
```

- Plan 可以不关联 Goal；
- Requirement 或 Issue 可以尚未进入 Stage；
- Stage 必须属于 Plan；
- Work 必须处理一个 Requirement 或 Issue；
- Issue 的 `about` 引用不会复制目标对象，也不会形成隐式业务状态变化。

完整关系与基数见 [Project View 对象关系设计](../stage/project-view/object-relation-design.md)。

## 3. Role Continuity

Role Continuity 解决的是：承担者或 Runtime 改变后，责任和工作如何继续。

```text
Role                   稳定责任位置
  └── Assignment       谁正在承担
       └── Member      谁作出承诺
            └── Runtime  谁正在执行

Project 持续保存 Work、Checkpoint、Handoff 与规范状态
```

其中：

- Role 表达责任位置；
- Assignment 表达哪个 Community Member 当前承担它；
- Member 是具有稳定公钥身份的 Human 或 Agent；
- Runtime 是 Member 的短生命周期执行实例；
- Checkpoint 和 Handoff 把当前局势、风险与未完成责任留在 Project；
- Role Brief 是从规范项目状态派生的读取结果，不是第二份事实源。

同一个 Agent 公钥重启 Runtime，仍是同一个 Member；换成另一个 Agent 公钥，则是新的 Member，
需要新的 Assignment 与明确交接。Persona、模型或 Provider 改变不会自动改变成员身份。

当前连续性合同还保持以下边界：

- 一个 Role 同时最多有一个 active Assignment；
- 一个 Member 同时最多有一个 active Assignment；
- 一项 Work 最多有一个 responsible Role 和一个 active Commitment；
- Assignment 结束不会自动完成、取消或重新分配 Work；
- 继任者必须通过新的 Assignment / Commitment 显式接续责任；Handoff 提供交接入口，
  但不是连续性成立的必要前提。

因此，active Project Member 可以理解为“具备 Community 成员资格，并持有 active Role Assignment
的成员”。候选 Agent、已连接 Runtime 或加入某个 Channel 本身都不足以形成 active Assignment。

完整设计见 [Role Continuity](../stage/role/role-continuity.md)。

## 4. Members：Human 与 Agent

Human 和 Agent 都以 Community Member 身份进入 Project，并使用同一套项目对象与协作模型。
Agent 不是隐藏在某个 Leader 下面的临时函数；它可以拥有稳定身份、Role、Assignment 和历史贡献。

这不意味着所有成员权力相同。Community 的 owner、admin、member 基础等级，
以及具体领域的能力门、签名和状态检查，继续决定成员可以观察、建议、写入或批准什么。

人类仍负责项目目标、价值边界、权限、高风险事项和不可逆决定的治理。
“Agent 是一等成员”不等于系统移除人的最终责任。

## 5. Project Documents

Project Document 是具有稳定 `document_id` 的 Markdown 文档。每次保存产生一个不可变完整修订，
同时维护明确的当前 Revision。

它适合承载：

- 设计、约束和操作说明；
- 决策依据和适用边界；
- Meeting 结果；
- Project Context 的解释性内容；
- 可供未来成员继承的项目认知。

Document 的身份不依赖标题或某次事件 ID。并发冲突不会静默覆盖或自动 rebase；
删除也通过可验证的 tombstone 表达。

Project Document 是持久项目记录，不是秘密存储。API Key、私钥、真实凭据和不应共享的用户内容
都不应写入其中。

完整设计见 [Project Document](../stage/document/document.md)。

## 6. Project Context

Project View 回答一阶问题：是什么、有哪些、当前在哪里。Project Context 保存二阶语义：
为什么相关、有哪些特殊依赖、可能影响什么、适用边界是什么。

最小模型是一条无向 Edge / Hyperedge：

```text
ProjectContextEdge
├── coordinates            两个或更多项目坐标
└── context_documents      一份或多份解释文档
```

当前坐标可以引用：

- Project View 对象；
- Project Document；
- Meeting。

Edge 只表达“这组坐标共享上下文”这一结构事实；普通 Project Document 才承载具体解释。
同一组精确坐标在一个 Project 中只有一条 Edge，但可以关联多份解释文档。

Edge 与 Context Document 还保持以下生命周期约束：

- 一份 Project Document 最多作为 Context Document 属于一条 Edge；
- active Edge 至少关联一份 Context Document；最后一份被 detach 后，该 active Edge 一并消失；
- 坐标对象 tombstone 不会静默缩小或删除既有 Edge，历史关系仍保留其原坐标身份。

系统不会自动从文本推断或创建这些关系。Human 或 Agent 在真实工作中发现语义后，
显式维护 Edge 与 Context Document；Relay 负责验证坐标、项目边界和引用完整性。

Desktop 当前提供只读关系画布、检查器和实时更新；规范的 Edge attach / detach 主要由 `cf`
和 Agent 操作完成。

完整语义见 [Project Context](../stage/project-context/project-context.md)。

### 6.1 图语义路径查询

可选的图语义查询在已验证 Project Context 图上工作。调用者可以提供：

- 一个自然语言问题；
- 可选的起始坐标；
- 可选的 Role、Work 等查询上下文，用来影响召回与排序。

结果 Event 由 Relay 签名，并绑定当前 Project、调用者和精确请求正文；结果同时携带来源、
图快照 currentness 与 Revision 证据。`cf` 验证结果后，另外派生未签名但规范化的
`read_commands`，供调用者回读权威对象。结果 DTO 不把源文档正文直接复制出来。

这项能力需要单独配置语义 Provider、索引 generation、Community index/query gate
和问题数据出境确认。它目前仍在相关性、资源隔离、长期稳定性和生产部署资格化中；
“使用了 Role 或 Work 上下文”只表示它会参与召回与排序，不保证每个问题都产生人类预期的唯一答案。

运维与启用边界见 [语义 pgvector 运维](../semantic-pgvector-operations.md)。

## 7. Meetings

Meeting 是有边界的正式协作对象，而不是一组松散聊天消息。

当前 V2 模型包含固定 roster、主持人、议程、共享 Board、Floor、发言时间线、handoff、
主持人决定、租约与超时、关闭或中止，以及 Action Finalization。
Human 与 Agent 可以共同参加，系统优先处理 Human 的 Floor 请求。

Meeting 产生的重要结果应回到 Project：形成 Work、Document、Context、Checkpoint
或其他规范状态，而不是只留在 Meeting Runtime 中。

但 Board、Speech、Close 或主持人文字本身不会自动修改 Project View，也不会自动成立项目决定。
只有有权成员通过现有业务界面或普通领域命令显式写入并回读后，Meeting 结果才会成为规范状态。
Action Finalization 只是主持人执行这些普通业务操作并提交 `actions-recorded` ACK 的有界阶段，
不是 Meeting 代理业务写入的专用 materializer。

Meeting 当前仍是预览能力；创建、direct action 和 Community read 各有独立开关与授权。
默认可见性和后续扩大读取范围不能仅靠客户端声明改变。

概念与阶段设计见 [Meeting V2](../stage/meeting/v2/meeting-v2.md)。

## 8. 一次典型协作

1. 人类在本地启动 Carryforth，创建或进入一个 Project / Community。
2. Human 与 Agent 以稳定身份加入，读取 Project View 中的目标、角色、计划和当前工作。
3. 成员响应 Role Proposal；双边授权满足后形成 active Assignment。Runtime 可以替换，
   责任和项目记录不会随进程结束。
4. 成员按需读取 Documents、Resource Guides 和相关 Project Context，而不是把全部资料塞进每轮对话。
5. 日常讨论在 Channels 中进行；会影响项目未来的结论被显式写回对象、文档、Context 或 Checkpoint。
6. 需要正式讨论时启动 Meeting，由主持人维护议程、共享状态和结果。
7. Agent 离开或被替换后，新 Runtime 通过 Role Brief、当前 Work、Document、Context 和历史继续职责。

这个流程的重点不是让系统保存更多文本，而是让项目拥有可以被验证、读取、修订和交接的持续状态。

## 9. 实现与启用边界

上述模型已经有对应的协议、Relay、CLI 或 Desktop 实现，但并非在新环境中全部自动开放：

- Desktop 中 Projects、Project View（含 Documents 与 Context）和 Meetings 是预览功能；
- Project View v3 需要 Relay operator 准备，再由 Community owner 审核并签名初始化；
- Documents、Context、Meetings 和语义查询都有自己的 readiness 与 durable gate；
- `./start.sh` 只启动本地源码栈，不替代这些治理与授权动作；
- 代码已经存在不等于完成生产资格验证。

当前成熟度和支持范围见[当前状态](current-status.md)。
