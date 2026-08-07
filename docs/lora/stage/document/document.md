# 项目文档与资源说明概念设计

> 本文固化 Project Document 与 Resource Guide 阶段已经形成共识的概念、关系、生命
> 周期和首版边界。本文只回答“它是什么”，不定义事件 kind、wire schema、数据库表、
> 事务与锁、CLI、用户界面、迁移步骤或分阶段实现方案。

## 1. 文档目的

[项目定位与目标](../../project-positioning.md)把 Buzz 定位为一个以项目为持续主体、
面向 Human 与 Agent 的共享认知与协作基座。[项目视图定义与项目上下文关系](../project-view/project-view.md)
进一步建立了 Project、Goal、Role、Plan、Stage、Requirement、Issue、Work 和 Resource
等稳定坐标；[角色连续性概念设计](../role/role-continuity.md)则让责任、当前承担者、
工作局势与交接能够独立于某次 Agent Runtime 持续存在。

这些能力仍未完整回答另一组基础问题：

- 项目共同维护的说明、规范、架构、决定和运行手册以什么稳定身份存在；
- Resource 除了名称和入口之外，如何告诉新成员“这是什么、什么时候使用、如何使用”；
- Repository、MCP、Skill、Plugin、服务器或密钥管理服务等不同对象，如何在不为每种
  类型先建设专用管理基础设施的前提下进入同一个项目资源面；
- Project View 如何稳定引用这些内容，并让 Role Brief 沿引用提供相关坐标；
- Agent 如何按需取得内容，而不是在进入 Project 时一次性加载全部正文。

最初的讨论从“资源层”开始。一个直接方案是为 Repository、MCP、Skill、Plugin 等资源
分别定义强类型配置、Resolver、安装器和运行时 Adapter。但这种方案要求 Buzz 提前承担
Git 获取、MCP 配置、SkillHub、Plugin Registry、Agent Runtime 适配、健康检查和密钥
交付等大量基础设施，也会迫使资源类型跟随外部生态持续扩展协议。

讨论随后形成了一个更适合首版的判断：

> 首版 Resource 的核心不是由 Buzz 自动管理资源本体，而是为项目保存一份可被稳定
> 引用、由 Human 与 Agent 共同维护的资源使用说明；Agent 阅读说明后，使用自己已有的
> 能力取得和操作真实资源。

当资源使用说明成为核心时，它不应继续只是 Resource 中的一段特殊文本。使用说明与
项目定位、架构、ADR、规范、报告和运行手册一样，本质上都需要一个项目共同拥有、可
独立引用和追溯版本的内容坐标。因此，设计顺序进一步收敛为：

1. 先建立最小的 Project Document 内容内核；
2. 再让 Resource 通过明确关系指向一份 Guide Document；
3. 让 Project View 对象稳定引用 Resource 或 Document；
4. 由 Agent 按需读取 Guide，并用已有工具访问资源；
5. 等真实使用暴露出重复且稳定的自动化需求后，再为特定资源增加可选 Adapter。

本文记录这一概念模型。

## 2. 核心结论

当前阶段采用以下一句话边界：

> Project Document 是项目共同拥有的内容坐标；Resource 是项目关联的资产或能力坐标，
> 并通过 Guide 指向 Document。Buzz 首版负责身份、引用、修订和读取，不负责资源本体、
> 秘密或执行。

二者在项目坐标系中的位置可以表示为：

```text
Project / Community
├── Project View
│   ├── Role            项目内稳定责任位置
│   ├── Resource        项目关联的资产或能力坐标
│   └── 其他项目对象    项目的直接对象、结构和当前状态
└── Project Document    独立的知识与内容坐标

Project View Resource ── guide_document_id ──► Project Document
```

Resource 与 Document 相互关联，但不能合并为同一个概念：

- Repository 是 Resource；“如何拉取和使用这个仓库”是 Document；
- MCP Server 是 Resource；“如何配置、验证和排障”是 Document；
- 云服务器是 Resource；“如何访问、有哪些限制”是 Document；
- 一份 ADR 本身是 Document，不需要为了成为项目知识再包装成 Resource；
- 一个外部文档空间可以是 Resource，其 Guide 说明地址、访问方式和使用边界；
- Project Document 是 Buzz 原生管理的项目内容对象，不等同于现有 Resource 类型枚举中的
  `document`。

## 3. 术语

### 3.1 Project Document

Project Document 是：

> 由一个 Community / Project 共同拥有，具有稳定身份、明确的创建者、逐 Revision 的
> 变更者与规范时间，并可追溯修订的 Markdown 内容对象。

它可以承载：

- Resource Guide；
- 项目定位和范围说明；
- 架构说明；
- ADR 与重要决定；
- 开发、测试和发布规范；
- 运行与故障处理手册；
- 调研和分析报告；
- 需求、Issue、Work 或 Handoff 的补充材料。

Document 的稳定身份不由标题、作者、当前正文、文件名或某一次 Nostr event ID 决定。
这些信息变化后，它仍然是同一份 Project Document。

### 3.2 Document Revision

Document Revision 是：

> Project Document 在一次成功创建、更新或删除后形成的不可变规范状态。

active Revision 同时包含当时的标题、摘要和完整 Markdown 正文；删除形成不含正文的
tombstone Revision。Revision 一旦形成，后续操作只能产生新的 Revision，不能改写旧
Revision 的业务内容。

`document_revision` 是 Document 自己的有序修订号，不等同于：

- 整个 Project View 的 `project_revision`；
- 某个 Project View 对象的 `object_revision`；
- Nostr `event_id`；
- Canvas 的内容事件 ID；
- NIP-23 Note 用于 last-write-wins 的 `created_at`。

### 3.3 Resource

Resource 是：

> Project 中一个可稳定识别的资产、能力或外部入口，使成员知道项目拥有什么或可以
> 使用什么，并能取得理解和使用它的说明。

Resource 可以表示：

- Repository；
- MCP；
- Skill；
- Plugin；
- API 或服务；
- Database 或 Dataset；
- 云服务器、开发环境或生产环境；
- 密钥管理服务；
- 设计空间；
- 外部文档空间；
- 构建产物或其他项目资产。

Resource 不是资源本体。Repository 的 Git objects、MCP 进程、Skill 文件、Plugin 包、
服务器、数据库和密钥仍由各自的事实源和基础设施管理。

### 3.4 Resource Guide

Resource Guide 是：

> 一份被 Resource 明确指定为主要使用说明的普通 Project Document。

Guide 可以回答：

- 这个资源是什么；
- 什么时候应当使用；
- 地址或来源在哪里；
- 如何获取、安装、配置或连接；
- 使用前需要哪些前置条件；
- 如何验证已经取得或配置成功；
- 存在哪些限制、风险和注意事项；
- 出现问题时如何排查；
- 还需要阅读哪些相关 Resource 或 Document。

Guide 不是特殊的文档存储格式。它使用与其他 Project Document 相同的身份、修订和读取
语义，只是通过 `guide_document_id` 与 Resource 建立了明确关系。

### 3.5 Project Context Reference

Project Context Reference，简称 Context Reference，是：

> Project View 对象对 Resource 或 Document 的轻量上下文引用，用于指出理解或处理该
> 对象时哪些项目资产和内容相关。

Context Reference 不表示权限、所有权、依赖、执行顺序、状态传播或自动加载。它也不
替代 Project View 已有的结构关系，不是当前 `ProjectViewRelations` / `ObjectRef` 已经
存在的关系槽。首版需要为它增加明确的新 schema / capability。

## 4. 在 Buzz 坐标系中的位置

### 4.1 Project View 表达直接状态

Project View 继续表达：

- 项目是什么、要达成什么；
- 当前有哪些 Role、Plan、Stage、Requirement、Issue 和 Work；
- 这些对象的直接状态和明确结构关系；
- 项目登记了哪些 Resource。

Project Document 属于独立的 Document domain / capability，不是新增的普通
`ProjectViewObjectType`。Project View 只保存对稳定 `document_id` 的结构化引用；
Document 正文不进入 Project View 的完整对象快照。

修改 Document 不推进 `project_revision`。新增、修改或移除某个 Project View 对象上的
Context Reference，仍然是该 Project View 对象自身的更新，会推进其 `object_revision`
和整个 Project View 的 `project_revision`。

### 4.2 Role 表达责任

Role 继续表达稳定责任位置。Role 可以引用 Resource 和 Document，使当前承担者知道履行
该责任时通常需要哪些资产和内容，但 Context Reference 本身不赋予权限，也不把正文
复制进 Role。

### 4.3 Resource 表达资产与能力

Resource 提供稳定资产坐标和轻量元数据。它不把外部资源复制进 Buzz，也不承诺 Buzz
已经能够自动访问该资源。

### 4.4 Document 表达知识与内容

Document 提供稳定内容坐标。它既可以作为 Resource Guide，也可以独立成为 Project
中的规范、决定、说明或报告。

### 4.5 Project Context 在这些坐标上继续生长

Document 不是完整 Project Context 的同义词。Document 是可承载内容的基础对象；
Project Context 还需要定义内容的作用域、证据、状态、新鲜度、冲突、选择和交付方式。

后续 Context 可以引用 Project、Role、Work、Resource 和 Document，但不能因为有了
Document 就把自由 Markdown 自动视为已经治理、已经验证或当前有效的项目认知。

## 5. 责任边界

### 5.1 Buzz 负责什么

Buzz 首版负责：

- 为 Document 和 Resource 提供 Project-scoped 稳定身份；
- 保存 Document 元数据、当前 Revision 和历史 Revision；
- 保存 Resource 元数据和 Guide Document 引用；
- 记录创建者、更新者和规范时间；
- 处理并发更新和 Revision 冲突；
- 让 Project View 对象结构化引用 Resource 和 Document；
- 让 Human 与 Agent 发现元数据并按需读取正文；
- 保证同 Project 引用、删除保护和无隐式级联；
- 让固定 Revision 在后续成员进入、交接或审计时仍可读取。

### 5.2 Buzz 不负责什么

Buzz 首版不负责：

- 托管或复制 Resource 本体；
- 自动 clone Repository；
- 自动安装 Skill 或 Plugin；
- 自动启动或注入 MCP；
- 自动连接 Database 或云服务器；
- 建设 SkillHub、Plugin Registry 或通用 Resource Resolver；
- 判断 Guide 中的地址和命令是否仍然有效；
- 保存密码、Token、私钥或受限基础设施凭据；
- 因为某对象引用了 Resource 就自动执行 Guide 中的操作；
- 把所有 Document 正文自动放入 Project View、Role Brief 或 Agent Context。

### 5.3 Agent 负责什么

Agent：

1. 从 Project View、Role、Work 或其他对象发现相关 Resource / Document；
2. 查看轻量元数据；
3. 在需要时读取 Guide 或指定 Document Revision；
4. 理解说明；
5. 在已有工具、Runtime、安全策略和授权范围内执行说明中的步骤；
6. 验证结果，并在说明过期或失败时反馈、创建 Issue 或更新 Document。

Agent 是首版中的通用解释者，不是 Buzz 已经内建了所有资源类型的 Adapter。

### 5.4 外部系统继续作为事实源

Git 服务器、MCP 实现、Skill 或 Plugin 来源、云平台、Database、Vault 和外部文档系统
继续拥有各自资源的规范事实。Resource Guide 只保存项目如何识别和使用它们，不成为
这些系统的镜像。

## 6. Project Document 的领域模型

### 6.1 身份与范围

每份 Project Document：

- 必须属于一个且仅一个 Project / Community；
- 具有一个稳定 `document_id`；
- 标题和正文变化不改变 `document_id`；
- 结构化 Resource / Document Context Reference 必须发生在同一个 Project 内；
- 删除后 ID 仍被保留，不能重新用于另一份 Document。

Markdown 中的普通链接不构成结构化规范关系，可以指向外部内容，并继续受目标系统自身
的访问控制。

首版继续采用：

```text
一个 Buzz Community = 一个 Project
```

Document 是 Community 共同维护的内容，不绑定某一位作者的地址坐标。不同成员先后
编辑时，它仍然是同一份 Document。

### 6.2 元数据与正文

Project Document 至少具有以下概念信息：

```text
ProjectDocument
├── document_id
├── current_revision
├── lifecycle state
├── title（active）
├── summary（active，可选）
├── created_by / created_at
└── updated_by / updated_at
```

每个 Revision 具有：

```text
DocumentRevision
├── document_id
├── document_revision
├── state: active | deleted
├── title（active）
├── summary（active，可选）
├── content_markdown（active）
├── actor
└── canonical time
```

active ProjectDocument 中的标题、摘要、更新时间和当前 Revision 是 Current Revision
的轻量投影，不能独立于 Revision 修改而形成第二份事实。删除后的 Current Head 是
tombstone：只保留身份、Revision、lifecycle state、创建信息和删除 actor/time，不再把
此前的标题、摘要或正文暴露为当前业务状态。

Document 列表和客户端派生读取可以按稳定引用 hydrate 这些轻量元数据；规范 Project
View 对象只保存结构化 Reference（目标 ID，以及 Pinned Document Reference 的
Revision），不复制一份可能独立漂移的 Document 元数据。Markdown 正文始终按需读取。

### 6.3 独立 Revision

每份 Document 拥有自己的 `document_revision`：

- 创建成功后形成第一个 Revision；
- 每次成功更新或删除只增加该 Document 的 Revision；
- 修改 Document A 不与修改 Document B 发生无关冲突；
- 修改 Document 不推进 Project View 的 `project_revision`；
- 更换 Resource 的 `guide_document_id` 会修改 Resource；
- 只修改 Guide 正文不会修改 Resource 所在 Project View 对象的 `object_revision`。

### 6.4 Current Revision 与 Live / Pinned Reference

Document 有两种读取语义：

#### Live Reference

只引用 `document_id`，读取 active Document 的 Current Revision。

适合持续演进的规范、Guide、Role 和 Work 相关材料。

#### Pinned Reference

引用：

```text
document_id + document_revision
```

Pinned Reference 适合决定依据、已经完成工作的证据和历史事实。后续正文变化不会改变
它原本引用的内容。首版只有 active Revision 可以成为 Pinned Reference 的目标；
tombstone Revision 只表达生命周期变化，不能作为内容证据被新引用。

删除后 Live Reference 不再解析为可用的当前文档；显式 Pinned Reference 仍然可以读取
历史 active Revision。

若实现采用 Nostr current / revision projection，event ID 只标识一次具体投影，不是
Document 的长期业务坐标。

### 6.5 更新与局部编辑

每个成功的 active Revision 保存标题、摘要和 Markdown 正文的完整快照。规范提交语义
是：

```text
基础 document_revision
        +
完整的新 Document 快照
        ↓
新的 document_revision
```

这不要求 Human 或 Agent 每次手工重写整份文档。客户端可以基于明确的基础 Revision
提供局部编辑，但规范提交结果仍是完整的新快照。基础 Revision 已变化时必须报告冲突，
不能静默覆盖他人的更新。

具体 Patch 表示、编辑器交互和冲突呈现属于客户端实现设计。首版不采用模糊 Patch，也
不自动把修改 rebase 到他人已经产生的新 Revision。

### 6.6 删除

普通删除：

- 结束 Document 的 active 生命周期；
- 产生一个不含 Markdown 正文的 tombstone Revision；
- 保留稳定 `document_id`；
- 保留此前所有 active Revision；
- 不允许 ID 复用；
- 不表示隐私擦除；
- 不级联删除 Resource 或 Project View 对象。

存在 Resource Guide 或 Live Document Reference 时，删除应被拒绝，调用者必须先解除
活跃引用。

Pinned Reference 不阻止普通删除，因为它引用的历史 Revision 继续可读。若未来需要
隐私擦除、合规删除或受限内容清除，应设计独立治理流程，不能混入普通 tombstone。

### 6.7 正文按需读取

active Document 列表和客户端派生的 Project View 读取只暴露轻量坐标：

```text
document_id
state: active
title
summary
current_revision
updated_by
updated_at
```

删除后的 Document 不出现在 active 列表中；按 ID 查询其 tombstone Head 时只返回身份、
Revision、删除状态和 actor/time。

规范 Project View Object 只保存结构化 Reference：Resource 目标保存 `resource_id`，
Document 目标保存 `document_id`，Pinned 时再保存 `document_revision`。它不复制上面的
Document 元数据或正文。正文只在 Human 或 Agent 明确打开、读取或引用某个 active
Revision 时取得。这样可以避免：

- Project View 随文档数量和正文长度膨胀；
- Agent 进入 Project 时一次性加载全部文档；
- 无关文档占用 Context；
- Document 编辑与结构化项目状态争用同一个 Revision。

## 7. Resource 的领域模型

### 7.1 最小模型

首版 Resource 收敛为：

```text
ProjectViewObject
├── id                         # resource_id
├── object_revision
└── data: ProjectResource
    ├── name
    ├── resource_kind
    ├── summary
    └── guide_document_id
```

Resource 的身份、Revision、创建者和更新时间来自 Project View 对象的公共语义，不再
建立另一套 Resource Revision。

`guide_document_id` 指向同一 Project 中一份 active Project Document，并默认跟随其
Current Revision。

### 7.2 开放的 resource_kind

`resource_kind` 是面向 Human 与 Agent 的开放描述性字符串，用于列表展示、筛选和形成
基本预期，不是 Nostr event kind，也不驱动 Relay 行为。

推荐词汇可以包括：

```text
repository
mcp
skill
plugin
server
database
dataset
secret_manager
service
environment
design
external_document
artifact
```

未知 `resource_kind` 仍然可以保存、显示、引用和读取 Guide，不需要等待协议支持新的
资源类型。

### 7.3 Resource Guide 是主要使用入口

每个 Resource 通过 `guide_document_id` 指定一份主要说明。它使 Agent 不需要从多份
相关文档中猜测哪一份解释如何使用该资源。

Resource 还可以通过普通 Document Context Reference 关联补充材料，但这些引用不替代
Guide。

Guide 和 Resource 具有独立生命周期：

- 更新 Guide 正文只产生新的 Document Revision；
- 更换 `guide_document_id` 才修改 Resource；
- 删除 Resource 不删除 Guide；
- Guide 被 active Resource 使用时不能删除；
- Resource 名称与 Guide 标题是否由客户端协助同步，不属于领域层的隐式状态传播。

### 7.4 Resource 示例

| `resource_kind` | Guide 可以包含 |
|---|---|
| `repository` | Git / NIP-34 地址、clone 方式、默认入口、初始化步骤、验证命令 |
| `mcp` | 来源、配置示例、支持的 Runtime、重启要求、验证和排障方式 |
| `skill` | Skill 来源、安装位置、加载方式、版本要求和验证步骤 |
| `plugin` | Plugin 地址、目标 Agent Runtime、安装、启用、升级和卸载方式 |
| `server` | 地址、访问前提、连接方式、允许操作、环境限制和故障处理 |
| `database` | 数据库用途、连接入口、Schema 入口、通过何种服务取得临时凭据 |
| `secret_manager` | 使用哪种身份登录、如何请求所需密钥、不得泄露的边界 |

“支持这些 Resource”在首版只表示 Guide 可以描述它们，不表示 Buzz 已经提供对应的
安装器、连接器或运行时集成。

### 7.5 Resource 不是 Secret Store

Resource 元数据和 Guide 都不得保存：

- 密码；
- Bearer Token；
- 私钥；
- 长期数据库凭据；
- 受限基础设施 Secret。

密钥管理服务本身可以成为 Resource。若外部密钥系统已经配置为接受某种 Buzz identity、
Workload Identity、Vault CLI 或其他可信入口，Guide 可以说明如何利用它取得短期凭据；
Buzz 首版本身不签发或兑换外部凭据。

这条说明不能凭空创造初始权限。最初的信任入口仍然来自 Agent Runtime、Human 环境或
外部身份系统。

## 8. Project View 的上下文引用

### 8.1 结构关系与上下文引用分开

Project View 已有的直接结构关系继续回答：

- Stage 属于哪个 Plan；
- Work 处理哪个 Requirement 或 Issue；
- Issue about 哪个 Project View 对象；
- Role、Assignment 与 Work 的责任关系。

Resource / Document Context Reference 只回答：

> 理解或处理这个对象时，哪些项目资产和内容相关？

它不改变对象状态，不建立权限，也不产生级联。

### 8.2 首版目标

首版结构化 Context Reference 只支持：

```text
ResourceContextReference
└── resource_id

DocumentContextReference
├── document_id
└── document_revision（可选）
```

Project Profile、Goal、Role、Plan、Stage、Requirement、Issue、Work 和 Resource 等
active Project View 对象都可以成为 Context Reference 的来源。Resource 首版只允许
引用 Document，不允许通过通用 Context Reference 指向另一个 Resource；资源之间的
依赖或组合关系需要在真实使用证明必要后另行设计。

不直接支持任意外部 URL。外部资产应先成为 Resource；自由链接仍可写在 Markdown 中。

首版也不通过 Context Reference 表达：

- `uses`；
- `produces`；
- `depends_on`；
- `blocked_by`；
- 权限或 capability；
- 自动执行和安装。

### 8.3 Live 与 Pinned Document Reference

未指定 `document_revision` 的 Document Context Reference 是 Live Reference，在目标
active 时解析其 Current Revision。

指定 Revision 的 Document Context Reference 是 Pinned Reference。它只能指向一个
active Revision，并始终解析该历史快照；tombstone Revision 不能成为 Pinned
Reference 的目标。

一般来说：

- Project Profile、Goal、Plan、Requirement、Issue、Role 和进行中的 Work 更适合 Live；
- 已完成 Work 的依据、决定证据和历史事实更适合 Pinned。

这只是使用原则，不把对象类型和引用方式永久绑定。

首版 Context Reference 的来源只包括 8.2 列出的 active Project View 对象。Role
Checkpoint 与 Handoff 可以继续引用相关 Project View 对象，Role Brief 再沿这些对象
解析 Resource / Document；若未来要让 Checkpoint 或 Handoff 直接保存
DocumentContextReference，需要另行扩展其 schema。

### 8.4 Guide 使用显式字段

Resource 的主要 Guide 使用明确的 `guide_document_id`，不依赖通用 Context Reference
推断。

通用 Context Reference 可以补充故障手册、设计说明或其他材料，但 Agent 总能从
Resource 的显式字段找到主要使用说明。

### 8.5 删除与引用

当前采用：

- active Resource 被任何结构化 Context Reference 引用时不能删除；
- active Document 被 Live Reference 或 `guide_document_id` 引用时不能删除；
- Pinned Document Reference 不阻止普通删除；
- 删除引用来源不删除目标；
- 删除 Resource 不删除 Guide；
- 不做隐式级联。

### 8.6 Markdown 链接不是规范关系

若后续定义 Resource / Document 对应的 Buzz deep link，Markdown 可以包含它；在首版
关系语义中，这类链接仍然只是内容导航，不自动提升为结构化 Context Reference：

- Relay 不需要解析 Markdown；
- 内容链接可以暂时悬空；
- 它不参与删除保护；
- 它不自动进入 Role Brief；
- 它只提供导航。

需要系统验证和投影的关系必须由结构化 Context Reference 明确登记。

## 9. Human 与 Agent 如何使用

### 9.1 发现

Human 或 Agent 从 Project View、Role、Work、Issue 或派生的 Role Brief 看到相关
Resource 和 Document 的轻量元数据。

### 9.2 按需读取

Role Brief 和其他派生读取只提供：

```text
Resource
- resource_id
- name
- resource_kind
- summary
- guide_document_id

Document
- document_id
- title
- summary
- reference_mode: Live | Pinned
- document_revision（Pinned 时）
```

正文不批量注入。成员只有在当前工作需要时才读取 Guide 或具体 Revision。

### 9.3 理解与行动

以 Repository 为例：

```text
Work 引用 Repository Resource
        ↓
Agent 取得 Resource 元数据
        ↓
Agent 读取 Guide Document 的当前 Revision
        ↓
Guide 给出地址、前置条件和 clone 步骤
        ↓
Agent 使用已有 Git 工具拉取仓库
        ↓
Agent 按 Guide 验证结果
```

MCP、Skill、Plugin、服务器、Database 和密钥管理服务使用相同模式。

### 9.4 读取没有执行副作用

读取或引用 Guide 不会：

- 自动运行其中命令；
- 自动安装 Skill 或 Plugin；
- 自动启动 MCP；
- 自动改变 Agent 配置；
- 自动连接服务器或 Database；
- 自动获取 Secret。

若未来增加 Adapter，它也只是 Resource 之上的可选自动化能力，不能改变“Context
Reference 本身没有执行副作用”这一边界。

### 9.5 接替与连续性

当 Role 的承担者或 Agent Runtime 发生变化时，新成员仍然可以沿同一组稳定坐标取得：

- 当前相关 Resource；
- 每个 Resource 的最新 Guide；
- 当前工作的 Live Document；
- 与已完成 Work、Issue 或决定证据关联的历史 Revision。

这部分资源入口与说明的连续性因此属于 Project，而不依赖旧 Agent 保留本地配置、聊天
上下文或退出前总结。

## 10. 与 Buzz 现有内容对象的边界

### 10.1 当前 Project View Resource

当前 Project View Resource 是具有名称、封闭 `resource_type`、惰性 `locator` 和描述的
稳定目录项。Locator 只表示“在哪里”，Relay 不解析、连接或获取目标。

本文定义的是后续 Resource Guide 方向：

- Resource 继续是稳定资产坐标；
- `resource_kind` 成为开放描述性词汇；
- Guide Document 成为主要使用说明；
- 不再要求为每个 `resource_kind` 预先实现专用 Resolver。

上述 v1 Resource body 和 v2 Role Continuity 是该概念方案形成时的历史前提。
当前普通运行时已收敛到 Project View v3，`resource_kind` 与 Guide-backed Resource
已是 v3 规范能力；旧 locator 只作为显式迁移输入，不得被客户端当作 fallback。

在目标模型中，Guide 是资源地址和使用步骤的规范项目来源。现有 required `locator` 是
legacy / 迁移输入，不与 Guide 长期并列为两个权威来源。具体如何演进、兼容和迁移属于
实现设计。

### 10.2 Channel Canvas

Channel Canvas 是现有 Channel 范围的一份共享 Markdown 叙事：

- 作用域是 Channel；
- 一个 Channel 读取一份当前 Canvas；
- 没有独立的多文档目录；
- 没有 Project Document UUID 和规范 `document_revision`；
- 更新是带 `h` scope 的普通成员签名内容事件，没有 `expected_document_revision`
  冲突门；
- 不提供按 Document Revision 读取的项目历史。

Project Document 不替代 Canvas。首版二者并存，可以复用 Canvas 的 Markdown 编辑、
渲染和“只给 Agent 坐标、正文按需读取”的产品经验，但不复用其身份和生命周期语义。
Canvas 空内容由客户端解释为 cleared，不会因此回退读取更早事件。

### 10.3 NIP-23 Note

NIP-23 Note 是以：

```text
kind + author pubkey + slug
```

为坐标的作者拥有长文。它已经具有 Markdown、标题、摘要和基本 CLI 操作，但更换作者
就会形成另一个地址，更新采用 NIP-33 的 `created_at` last-write-wins 语义。

被替换的旧行可以在存储中 soft-retire，但普通读取只返回 live head；当前没有按有序
Revision 读取旧正文的正式接口，因此它不能直接满足 Pinned Reference。

Project Document 是 Community 共同拥有的内容。不同成员编辑不改变 Document 身份，
并使用显式 `document_revision`。首版二者并存；导入、提升或同步不属于本文范围。

### 10.4 Project Document 不内联进 Project View

Document 正文不作为普通 Project View 对象 body，原因是：

- Project View 的完整 active snapshot 会读取每个 active object 的完整 body；
- 文档数量和正文增长会使项目快照持续膨胀；
- 每次编辑都会与无关的 Role、Work 或 Resource 修改争用全局 `project_revision`；
- Project View current head 和 tombstone 语义不是正式的 Document Revision 历史。

Project Document 可以复用 Project View 已经验证过的 Community 边界、稳定 UUID、
规范 actor/time、并发控制和删除保留原则，但拥有独立 Revision 生命周期。具体事件、
head 和投影方式由实现设计决定。

## 11. 安全与信任边界

### 11.1 Community 基线

首版不设计 Document 或 Resource 的细粒度 ACL。Document 选择与 Project View 相同的
Community-global 成员边界作为概念基线，而不是继承 Canvas 的 Channel 权限：

- 经授权 Community 成员可以读取；
- 满足现有基线写条件的成员可以创建和更新；
- Managed Agent 仍受 Buzz 的认证主体、owner/current-membership、ban/timeout 和 Runtime
  安全边界约束；
- Role、Assignment 和 Context Reference 本身都不自动授予 Document 权限。

具体 MessagesRead / MessagesWrite 等 scope、是否需要 active Assignment、管理动作和
异常治理由实现设计固定。

### 11.2 作者与 Revision 提供来源，不提供安全证明

Document 应展示创建者、更新者、时间和 Revision，使成员知道说明来自谁、何时变化。
但签名和来源不能证明 Guide 中的命令一定安全、正确或仍然有效。

Agent 阅读 Guide 后仍然受：

- Sandbox；
- Approval；
- Runtime capability；
- 外部系统权限；
- 不可逆操作安全规则；
- Human 治理边界。

Guide 不能通过普通 Markdown 内容提升自己的指令优先级或绕过这些约束。

### 11.3 不保存秘密

Project Document 和 Resource 都不是 Secret Store。即使内容只在 Community 内可读，
也不能把 Secret 直接写入 Guide、Checkpoint、Handoff 或普通项目文档。

Resource 可以指出如何通过受治理系统取得 Secret，但实际值只应存在于专门的密钥服务
和短生命周期运行环境中。

## 12. 首版设计原则

1. **先内容坐标，后完整文档产品**：先建立可靠的 Markdown 身份、Revision 和读取，
   不直接建设 Notion 或 Google Docs。
2. **资源本体留在事实源**：Buzz 保存项目如何识别和使用资源，不复制或接管资源本体。
3. **开放说明优于封闭类型执行**：首版用开放 `resource_kind` 和 Guide 容纳新资源，
   不为每种类型扩协议。
4. **Document 与 Resource 分离**：内容变化不等于资产变化，资产变化也不隐式改写内容。
5. **正文按需交付**：列表、Project View 和 Brief 只携带轻量坐标。
6. **Revision 可复现**：Current 服务持续工作，Pinned 服务历史证据。
7. **局部编辑不改变完整快照语义**：客户端可以提供局部编辑，规范 Revision 始终
   自包含。
8. **Context Reference 没有执行副作用**：发现和读取不自动运行、安装或连接。
9. **不隐式级联**：删除和更新必须显式，不能悄悄改变其他对象。
10. **从真实使用生长自动化**：只有重复、稳定且值得机器化的 Guide 步骤，才成为后续
    Adapter 候选。

## 13. 主要场景

### 13.1 Repository Resource

1. 成员建立 Repository Resource；
2. 为它建立或选择一份 Guide Document；
3. Guide 写明 Git / NIP-34 地址、初始化和验证步骤；
4. Work 引用该 Resource；
5. Agent 按需读取 Guide；
6. Agent 用现有 Git 能力把仓库拉到工作空间。

### 13.2 MCP、Skill 或 Plugin

1. Project 登记对应 Resource；
2. Guide 说明来源、支持的 Runtime、配置或安装步骤；
3. Role 或 Work 引用 Resource；
4. Agent 阅读说明；
5. Agent 在自身 Runtime 能力允许时完成配置；
6. 如果变更需要重启或新 Session，Guide 明确说明。

Buzz 首版不声称已经自动完成步骤 5。

### 13.3 云服务器与密钥管理服务

1. Server Resource 的 Guide 给出地址和访问前提；
2. 同一 Work 或 Role 同时引用 Secret Manager Resource；Server Guide 也可以用普通
   内容链接帮助成员导航到它，但不建立 Resource-to-Resource 结构关系；
3. Secret Manager Guide 说明如何利用既有身份取得短期凭据；
4. Agent 在外部权限允许时访问服务器；
5. Secret 不回写 Buzz。

### 13.4 独立项目文档

架构说明或 ADR 可以不依附任何 Resource 独立存在。Requirement、Issue 或 Work 可以
使用 Live 或 Pinned Document Reference。

### 13.5 Agent 接替

1. 新 Agent 取得 Role Brief；
2. Brief 提供相关 Resource 和 Document 坐标；
3. Agent 按需读取最新 Guide；
4. 对历史判断读取相关 Work、Issue 或决定证据固定的 Pinned Revision；
5. 即使旧 Agent 已退出，也能恢复资源入口、使用方式和当时依据。

## 14. 首版范围

首版概念范围包括：

- Community / Project-scoped Project Document；
- 稳定 `document_id`；
- Markdown 标题、摘要和正文；
- 独立、单调增加的 `document_revision`；
- Current 与按 Revision 读取；
- active Revision 的完整快照，以及删除时不含正文的 tombstone Revision；
- 基于基础 Revision 的并发更新；
- tombstone 与历史保留；
- 轻量元数据列表和正文按需读取；
- Resource 的开放 `resource_kind`、摘要和 `guide_document_id`；
- Project View 对 Resource / Document 的结构化 Context Reference；
- Live 与 Pinned Document Reference；
- Agent 发现、读取和解释 Guide 的闭环；
- Community 基本读写边界和 Secret 禁止规则。

## 15. 首版非目标

首版不实现或不承诺：

- 富文本或 Block Document；
- CRDT、OT 或多人实时协作；
- 模糊 Patch 和自动 rebase；
- 文件夹、空间、目录树或知识库层级；
- 评论、批注、建议模式和审批流；
- 附件、图片和大文件内容管理；
- 全文搜索、语义搜索和向量索引；
- 自动摘要和 Context Compiler；
- Document 或 Resource 细粒度 ACL；
- 外部文档双向同步；
- Canvas / NIP-23 Note 自动迁移或合并；
- Resource dependency graph；
- Resource health、availability 或 freshness 检测；
- Repository clone、MCP 注入、Skill / Plugin 安装的系统级自动化；
- SkillHub、Plugin Registry、Secret Store 或通用 Resource Resolver；
- 自动执行 Guide；
- 隐私擦除和合规删除；
- 任意关系图和自定义 Context Reference 语义。

这些能力只有在 Project Document 和 Resource Guide 已经被 Human 与 Agent 实际使用后，
才根据真实问题继续设计。

## 16. 领域不变量与验证清单

后续协议和实现至少需要保持：

1. 一个 Document 必须且只能属于一个 Project。
2. Document 使用稳定 ID；标题、编辑者和正文变化不改变身份。
3. 不同成员编辑同一 Project Document 时不会产生新的作者坐标。
4. `document_revision` 只属于该 Document，不等同于 Project View Revision 或 event ID。
5. 每个 active Revision 都是完整、独立、不可被业务更新改写的快照；删除 Revision
   是不含正文的 tombstone。
6. 修改一份 Document 不与其他 Document 或 Project View 对象产生无关并发冲突。
7. Pinned Reference 只能指向 active Revision，并在后续更新和普通删除后仍能读取该
   Revision。
8. 删除后的 Document ID 不能重新用于另一份内容。
9. 普通删除不是隐私擦除。
10. Document 正文不进入 Project View 全量快照。
11. Document 正文不默认进入 Role Brief 或 Agent Context。
12. Resource 与 Guide Document 必须属于同一 Project。
13. 在新 Resource schema 生效并完成迁移后，每个 active Resource 都有明确的主要
    Guide。
14. 更新 Guide 正文不增加 Resource 所在 Project View 对象的 `object_revision`。
15. 更换 Resource Guide 是 Resource 自身的明确更新。
16. 删除 Resource 不删除 Guide Document。
17. active Guide 或 Live Reference 不能被删除成悬空引用。
18. Pinned Reference 不阻止保留历史的普通删除。
19. Context Reference 不授予权限、不改变状态、不触发执行。
20. Markdown deep link 不自动成为规范 Context Reference。
21. 未知 `resource_kind` 仍然能够保存、展示和读取 Guide。
22. 任何 Project Document（包括 Guide）和 Resource 元数据都不能包含 Secret。
23. Agent 读取 Guide 不自动执行其中命令。
24. Buzz 不因 Resource 存在而声称已经托管、连接或验证真实资源。

## 17. 后续实现设计

在本文概念基础上，后续实现设计需要继续固定：

- 独立 Document capability 与协议版本；
- 成员 mutation、Current Head 和 Revision 的 wire contract；
- 可信 signer、projection generation、hash 和读取验证；
- Document 当前态与 Revision 的持久化；
- Revision 冲突、幂等和删除事务；
- Project View Context Reference 的 schema 与跨域完整性；
- Resource schema 演进和现有 locator 迁移；
- SDK、Agent CLI 与 Desktop 的读取和编辑体验；
- 客户端 exact Patch 与完整快照提交；
- rollout、兼容、回滚、测试与验收。

这些属于“如何实现”，不改变本文已经明确的领域边界。

## 18. 当前结论

当前阶段不建设一个预先理解所有 Resource 类型的资源控制面，也不把新的 Resource
Guide 设计成 Resource body 内的一段自由 Markdown。

采用的设计顺序是：

```text
先建立 Project Document
        ↓
让 Resource 通过 Guide 引用 Document
        ↓
让 Project View 对象引用 Resource / Document
        ↓
Agent 按需读取并使用已有能力行动
        ↓
从真实使用中识别值得自动化的 Adapter
```

Project Document 使项目拥有可继承、可修订、可固定引用的内容；Resource 使项目拥有
稳定的资产和能力坐标；Guide 把二者连接起来。成员可以变化，真实资源可以继续由外部
系统管理，但项目对“它是什么、如何使用、当时依据哪一版说明”的认知不会随单一 Agent
或会话消失。
