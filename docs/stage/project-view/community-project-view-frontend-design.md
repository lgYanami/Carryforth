# Community 展示页中的 Project View 前端设计

> 版本说明（2026-08-07）：本文的信息架构继续有效；底层 Project View 普通运行时现已
> 固定为 schema v3，不再进行 v1/v2 客户端 fallback。见
> [Project View 普通运行时全面收敛到 v3](../bug/project-view-v3-only-runtime-migration-fix-design.md)。

> 本文重新对齐 Buzz Desktop 中 Community 与 Project View 的前端信息架构。
> 核心变化是：Project View 不再被表现为与 Inbox、Pulse、Projects 并列的独立功能，
> 而是成为 Community 自身展示页中的项目当前视图。
>
> Community 展示页是新增的稳定空间入口，不等同于 Project View preview，也不能以
> “进入项目摘要”为由抹除每个 Community 已有的工作位置连续性。Project View 关闭、
> 尚未初始化或暂不可用时，Community 展示页仍必须是一个可用页面。
>
> 本文定义产品语义、页面结构、导航行为、渐进展开和状态边界，不深入规定组件拆分、
> 路由文件、缓存实现或具体视觉参数。本次设计不拆分开发阶段。

## 1. 文档目的

[Project View 客户端 v0 设计](frontend-design.md)已经交付完整的 Human 客户端：

- Human 与 Agent 读取和修改同一份 Relay 权威 Project View；
- Desktop 可以展示 Project Profile、Current Focus、Project Map、Role、Resource
  和 Object Inspector；
- Role Continuity 进一步提供 Assignment、Responsible Work、Role Brief、Checkpoint
  与 Handoff；
- `/view` 支持对象定位、可信刷新、并发冲突恢复和 Community 隔离。

但是，客户端 v0 把 `View` 放在主侧栏中，与 Inbox、Pulse、Projects、Agents 和
Workflows 并列。这个入口能够交付功能，却没有正确表达领域关系：

```text
一个 Buzz Community = 一个 Project
Project View = 这个 Project 的当前直接可见面
```

当 Community 选择和 `View` 入口分离时，Human 为了查看某个 Project，需要先切换
Community，再进入 View。界面同时暗示 View 是一个跨 Community 的应用模块，而不是
当前 Community 对自身的结构化展示。

本文修正这一信息架构，使 Human 进入 Community 时就能感知项目空间，并在需要时从
同一个展示面展开完整 Project View。

## 2. 当前问题

### 2.1 导航层级与领域层级不一致

当前界面近似表达：

```text
Buzz
├── Community selector
├── Inbox
├── Pulse
├── View
├── Projects
├── Agents
└── Workflows
```

这把两种不同层级混在一起：

- Community selector 决定“当前位于哪个协作空间”；
- Inbox、Pulse、Projects、Agents 和 Workflows 是这个空间中的工作入口；
- Project View 则是这个空间本身的项目描述和当前状态。

Project View 不只是另一个工作入口。它回答当前 Community 是什么、为什么存在、
正在推进什么、由哪些 Role 负责以及有哪些需要关注的事项。

### 2.2 两步进入削弱了项目空间感

Human 切换到一个 Community 后，如果还要再次点击 `View` 才能看到项目上下文，默认
体验仍然更接近“进入一个聊天工作区”，而不是“进入一个保有共同现实的 Project
Space”。

这会造成：

- Community 名称与项目定位彼此割裂；
- Project Profile、当前 Goal 和 Role 状态缺少环境性可见度；
- Human 容易直接进入 Channel 或 Inbox，却没有先形成项目整体认识；
- View 看起来像可选工具，而不是 Community 的共享当前状态。

### 2.3 完整 View 不适合作为每次进入时的全部内容

另一方面，现有完整 Project View 会随着 Goal、Plan、Stage、Requirement、Issue 和
Work 增长。每次进入 Community 都直接展开整幅地图，会让常用操作被长页面淹没。

因此不能简单地把当前 `/view` 页面原样设为 Community 首页，而需要：

```text
默认可见的项目摘要 + 按需展开的完整 Project View
```

### 2.4 新展示页不能抹除已有工作位置

改造前没有独立的 Community 展示路由。Community Rail 的既有行为是：

- 切换到另一个 Community；
- 保存离开 Community 时所在的 Inbox 或 Channel；
- 返回该 Community 时恢复并校验上次位置；
- 同 Relay 重连或应用恢复不主动改变 Human 当前打开的工作面。

这种行为保存的是 Human 的工作位置，不是 Project View 的领域状态。新增 Overview
不能通过删除或覆盖该记录来实现“一次进入项目摘要”。否则 Human 每次查看另一个
Community 都会丢失上次协作位置，Community 连续性反而下降。

### 2.5 Preview 开关不能决定 Community 页面是否可用

Project View 当前仍是默认关闭的 preview feature。Community Overview 如果始终存在，
却在 preview 关闭时只显示一个占据整页的启用提示，就会把稳定的 Community 导航降级
为实验功能的空壳。

因此需要区分：

```text
Community 展示页：稳定的空间外壳和导航入口
Project View 区域：受 capability、初始化、权限和 preview 状态约束的项目内容
```

preview 关闭时可以不查询 Project View，但不能让 Community 身份、继续上次工作、
Inbox、Channel 和其他稳定入口消失。

## 3. 核心设计结论

### 3.1 Community 是 Project Space 的产品容器

前端继续采用已经确定的身份边界：

```text
一个 Community = 一个 Project Space
```

Community 负责：

- 项目身份和 Relay 边界；
- 成员与 owner、admin、member 治理；
- Channel、消息和协作入口；
- Project View 与 Role Continuity 的状态归属。

Community 不是 Project View 之外的另一个业务对象。Project View 是 Community
作为 Project Space 的结构化当前可见面。

### 3.2 Project View 属于 Community 展示页

新的产品层级为：

```text
Community / Project Space
├── Community 展示页
│   ├── Community 身份与“继续上次工作”
│   ├── Project View 摘要（功能可用时默认可见）
│   └── 完整 Project View（按需展开）
├── Inbox
├── Pulse
├── Projects
├── Agents
├── Workflows
└── Channels
```

`View` 不再作为与这些入口平级的主侧栏模块。Human 选择 Community，就是选择
Project；打开 Community 展示页，就能看到可用的 Project View，同时仍能继续该
Community 中上次打开的工作位置。

### 3.3 默认摘要，按需完整展开

Community 展示页默认显示足以建立项目认识的一屏摘要，不要求 Human 主动打开隐藏的
折叠区才能知道项目是什么。

完整对象树、未规划对象、全部 Role、Resources 和 Inspector 继续按需展开。摘要与完整
View 是同一份 verified snapshot 的两种呈现密度，不是两套状态。

### 3.4 Role 是 Community 协作结构的一等摘要

Role 不再只作为完整 Project Map 之后的 Supporting Object 出现。Community 展示页
需要直接呈现：

- Leader Role；
- 当前主要 active Role；
- 当前承担者或 vacant 状态；
- 当前 Human 或 Agent 自己的 Role；
- 需要接续的 Work 或 Role 连续性信号。

Role 的完整 Purpose、Responsibilities、Boundaries、Assignment、Role Brief、
Checkpoint、Handoff 和历史仍在 Inspector 中按需查看。

### 3.5 保留现有能力，不建立第二套 View

本设计不修改 Project View 或 Role Continuity 的事实源。默认摘要、完整展开和
Inspector 必须继续使用同一份：

- Relay 签名；
- project revision 一致；
- projection generation 一致；
- native 边界验证后的 Project View 与 Role Continuity read model。

客户端不得为 Community 首页复制一份 Profile、Role 列表或 Markdown 项目摘要。
已有的 per-Community Inbox/Channel destination、目标校验和失效回退也属于必须保留
的客户端能力，不应因 Overview 改造而删除。自动恢复改为 Overview 中的显式继续，
只是入口变化，不是丢弃这份连续性。

### 3.6 Community 外壳独立于 Project View 状态

Community 展示页必须先成立，再在其中呈现 Project View。稳定外壳只使用已有的
Community 配置、成员身份和导航状态，不创建第二个 Community Profile。

当 Project View preview 关闭、Relay 不支持、尚未初始化、无权读取或完整性失败时：

- Community 名称、图标和当前空间身份仍然可见；
- Inbox、Channel 与“继续上次工作”仍然可访问；
- Project View 区域使用与状态相称的紧凑说明；
- 失败只约束项目内容，不把整个 Community 渲染成空白或不可用。

## 4. 设计目标

本设计需要达到：

1. Human 选择一个 Community 后，无需再进入独立 `View` 模块，就能看到项目基本上下文。
2. Community、Project Space 和 Project View 的从属关系在视觉和导航中保持一致。
3. 默认页面足够简洁，可以快速回答项目是什么、当前关注什么、由谁负责。
4. 完整 Project View 仍可在一次明确操作后展开，并保留现有对象维护能力。
5. Role、Assignment 和连续性信号成为 Community 展示的一等内容。
6. 摘要与完整 View 始终来自同一个可信 revision，不产生第二份项目现实。
7. Community 切换不会携带前一个 Community 的对象选择、草稿或 Role 状态。
8. 现有 `/view` 对象深链接、浏览器前进后退和外部定位继续可用。
9. 现有 `Projects` 的名称、路由和 Git/NIP-34 语义不发生变化。
10. 新增 Overview 不删除或覆盖每个 Community 的 Inbox/Channel 工作位置连续性。
11. Project View preview 关闭时，Community 展示页仍提供稳定身份和工作入口。
12. 标准 Desktop 首屏优先呈现项目身份、当前方向、Role 与最重要的显式注意事项，
    避免重复状态标识和大面积空白。

## 5. 非目标

本设计不：

- 修改 Project View 或 Role Continuity 的领域对象、事件 kind、数据库或 Relay 权限；
- 新增独立的 Community Profile 来复制 Project Profile；
- 把完整 Project View 永久铺满每次进入 Community 的首屏；
- 把 Community 展示页变成通用仪表盘搭建器；
- 引入 AI 推断的“项目健康度”、自动优先级或唯一当前 Stage；
- 新增 Kanban、甘特图、自由关系图或拖拽编排；
- 重新命名或迁移现有 `Projects`；
- 删除现有 per-Community destination、目标 Channel 校验或失效回退能力；
- 让 Project View preview 开关决定 Community 本身是否存在或是否可进入；
- 规定 Web 或 Mobile 的具体布局；
- 改变 Agent 的 Project Space system context 与 turn 注入协议；
- 拆分新的分阶段开发计划。

## 6. 导航与入口

### 6.1 Community Rail 选择 Project Space

左侧 Community Rail 的职责保持单一：

> 选择当前 Project Space。

Human 主动选择一个 Community 后，主内容区打开该 Community 的展示页。点击当前已经
激活的 Community，也应能返回其展示页。

因此，查看目标 Community 的项目摘要只需要一次 Community 选择，不再需要：

```text
选择 Community → 再点击 View
```

这一行为只改变切换后的可见入口，不丢弃目标 Community 已保存的工作位置。Overview
需要提供清楚的“继续上次工作”动作，例如 `Continue in #general` 或 `Open Inbox`。
Human 选择该动作后，继续使用既有 destination 校验与恢复路径。

### 6.2 Community 展示页的可恢复入口

除了 Community Rail，当前 Community 的名称或页头也应提供可访问的“返回 Community
展示页”入口，避免 Community 展示页只能通过图标或鼠标进入。

当前 Community 名称与 `Overview` 文案可以组成一个固定入口；点击后打开 Community
展示页，但不能把这次导航写成该 Community 的“上次 Inbox/Channel 位置”。产品语义
必须保持为“打开当前 Community 自身”，而不是“打开一个新的全局模块”。

### 6.3 主侧栏

独立 `View` 主侧栏入口不再是最终信息架构的一部分。主侧栏继续承载当前 Community
中的操作面，例如：

```text
Inbox
Pulse
Projects
Agents
Workflows
Channels
```

在迁移期间是否短暂保留 `View` 作为兼容入口属于实现选择；即使保留，它也只能导航到
当前 Community 展示页的完整展开状态，不能继续表达一个独立产品模块。

只有在 Community 展示页于 preview 关闭和 capability 不可用时仍然可用，且现有工作
位置可继续恢复后，才可以移除独立 `View` 入口。迁移不能制造一个始终可见、实际却只
通往实验功能空状态的新主入口。

### 6.4 `/view` 与深链接兼容

现有 `/view` 和 `/view?object=<id>` 可以继续保留：

- `/view` 表示当前 Community 展示页的完整 Project View 状态；
- `object` 继续定位并打开相应 Inspector；
- 外部链接、历史记录和已有测试不需要因信息架构变化立即失效。

内部路由是否未来改成 Community-scoped URL 不在本文固定。无论 URL 如何组织，页面
必须持续显示当前 Community 的身份，使 Human 不会把 `/view` 理解成跨 Community
全局状态。

### 6.5 工作位置连续性

每个 Community 继续保存一个非权威的客户端 destination：

- Inbox；
- 或一个经过目标 Community Channel 列表校验的 Channel。

Overview、完整 Project View、Inspector 和 Settings 都不应覆盖这个 destination。
它们是临时查看面，不代表 Human 已经放弃原来的协作位置。

切换 Community 时：

1. 保存离开 Community 的有效 Inbox/Channel destination；
2. 进入目标 Community Overview；
3. 保留目标 Community 之前保存的 destination；
4. 在目标 Channel 数据验证完成后启用“继续上次工作”；
5. 目标 Channel 不再可用时，把继续入口安全回退为 Inbox。

这个 destination 只是导航便利，不进入 Relay、Project View、Role Brief 或 Agent
上下文。

## 7. Community 展示页信息结构

Community 展示页采用以下渐进结构：

```text
┌──────────────────────────────────────────────────────────────┐
│ Community identity                 Continue in #channel     │
├──────────────────────────────────────────────────────────────┤
│ Project identity · positioning · purpose    verified · rev  │
│ Current direction                              Open full View│
├──────────────────────────────┬───────────────────────────────┤
│ Current focus                │ Roles                         │
│ active plans / stages        │ Leader · current Role        │
│ open issues / current work   │ assignees · vacancies        │
├──────────────────────────────┴───────────────────────────────┤
│ Needs attention / stable entry points                       │
│ blockers · urgent issues · waiting continuation · resources │
├──────────────────────────────────────────────────────────────┤
│ 完整 Project View（展开后）                                  │
│ Goal → Plan → Stage → Requirement / Issue → Work            │
│ Not yet placed · Roles · Resources                          │
└──────────────────────────────────────────────────────────────┘
                                      ┌────────────────────────┐
                                      │ Object / Role Inspector│
                                      └────────────────────────┘
```

这张图表达信息层级，不固定最终像素和列宽。

### 7.1 Community 与 Project 身份

页头首先让 Human 确认：

- 当前 Community；
- 可继续的上次 Inbox/Channel 工作位置；
- Project Profile 的项目名称；
- Positioning 或 Purpose 的短摘要；
- 当前 verified / syncing / offline-stale 状态；
- 当前 project revision 与最近更新时间；
- 展开或收起完整 Project View 的操作。

Community 配置名称与 Project Profile 名称可能暂时不同。前端不应静默同步或制造第二份
可编辑名称：

- Community Rail 继续使用 Community 配置的识别名称和图标；
- Community 展示页先使用 Community 配置表达空间身份，再以 Project Profile 表达
  项目业务描述；
- 两者不一致时，可以把 Community 名称作为空间标识、Project 名称作为主标题；
- 是否提供显式同步能力留给未来设计。

Project View 的 verified / stale / integrity 状态只在项目区域以一个主要状态标识
表达。Community 页头可以提供简短的辅助文本，但不应重复渲染两个同等权重的
`VERIFIED` badge。

### 7.2 Project Profile 摘要

默认首屏应至少直接显示：

- Project name；
- Positioning；
- Purpose。

Problem 与 Scope 可以使用紧凑摘要、展开内容或在 Project Profile Inspector 中查看。
默认摘要不能只显示一个项目名称和对象数量，否则仍不足以建立项目认识。

### 7.3 Current Focus

Current Focus 继续只从显式对象状态派生，可以摘要显示：

- active Plan；
- active Stage；
- open / in-progress Issue；
- in-progress Work；
- ready / in-progress Requirement；
- high / urgent 的 Requirement、Issue 或 Work。

摘要应优先帮助 Human 定位对象，而不只是显示四个孤立数字。对象名称可以作为可点击
入口，打开 Inspector 或展开后定位到其规范位置。

Current Focus 不：

- 选择唯一当前 Plan 或 Stage；
- 推断对象优先级；
- 保存独立状态；
- 自动改变任何 Project View 对象。

### 7.4 Role 摘要

Role 区域是默认展示的一等部分，至少回答：

- 当前有哪些 active Role；
- 哪些是 Leader Role；
- 每个 Role 由谁承担或是否 vacant；
- 当前查看者是否承担某个 Role；
- 是否存在等待接续的 responsible Work。

展示时必须区分：

- Community owner：治理根，不是一个 Role；
- Leader Role：`level=admin` 的 Project Role；
- 普通 Role：`level=member`；
- Assignment：当前谁承担 Role；
- Agent Runtime：不在 Role 摘要中冒充 Assignment。

摘要可以优先呈现当前查看者的 Role、Leader 和 vacant Role，但这种顺序只是导航便利，
不能被解释为新的领域优先级。

点击 Role 后继续使用现有 Role Inspector，查看：

- Role Purpose、Responsibilities 与 Boundaries；
- Current tenure；
- Verified Role Brief；
- Collaboration Role Directory；
- Responsible Work；
- Latest Checkpoint；
- Proposals、Tenure history 与 Continuity timeline；
- 当前身份允许的 Request、Assign、Replace、End tenure、Checkpoint 和 Handoff 操作。

### 7.5 Needs Attention

Community 展示页可以提供一个有界的注意事项摘要，只使用已经明确登记的状态：

- high / urgent 且未终结的 Issue；
- Role 最新 Checkpoint 中明确登记的 blocker 或 risk；
- vacant Role；
- 等待 continuation 的 responsible Work；
- 离线、陈旧、冲突或完整性失败。

该区域不进行自然语言推断，不把聊天内容自动解释为风险，也不生成新的项目健康分数。
点击条目应回到对应 Project View 或 Role Continuity 对象。

没有需要关注的明确状态时，区域可以保持紧凑或不渲染，不需要人为制造“全部正常”的
推断结论。

### 7.6 Resources

默认摘要只需要显示少量稳定入口，例如主要仓库、设计文档、服务或环境。完整 Resource
列表和 locator 细节继续在展开后的 Project View 或 Inspector 中查看。

摘要不自动把现有 Buzz `Projects` 当作 Resource，也不复制资源内容。

### 7.7 继续上次工作

Community 展示页应提供一个轻量、明确的恢复入口：

- 有有效 Channel destination 时显示 Channel 名称；
- destination 是 Inbox 或目标 Channel 尚未验证时显示 `Open Inbox`；
- 入口不得携带前一个 Community 的 Channel ID；
- 点击后沿用既有 Channel 可见性、成员资格和路由校验；
- Project View preview 关闭或项目区域失败时，该入口仍然可用。

该入口不需要占据项目摘要的主要视觉区域，但不能隐藏在只有鼠标才能发现的菜单中。

## 8. 渐进展开

### 8.1 默认状态

进入 Community 展示页时，稳定的 Community 身份与“继续上次工作”默认可见。
Project View 功能可用时，其摘要默认可见，完整 Project Map 默认收起。

“收起”只改变信息密度：

- verified snapshot 仍然已经读取；
- live subscription 仍保持当前状态；
- Role 摘要与 Current Focus 仍可更新；
- 不产生独立摘要 revision。

Project View preview 关闭时，不发起 Project View query 或 live subscription。页面
保留 Community 外壳与工作入口，并在项目区域使用紧凑的 preview 说明；不能用一个
大面积空状态替代整张 Community 页面。

### 8.2 完整展开

Human 选择 `Open full View` 或等价操作后，在同一 Community 展示上下文中展开：

- 完整 Project Profile；
- Current Focus；
- Goal → Plan → Stage → Requirement / Issue → Work；
- Not yet placed；
- 全部 Roles；
- 全部 Resources；
- revision 与 projection provenance；
- Add、Edit、Delete 和关系维护操作。

展开不是进入另一个 Project，也不需要再次选择 Community。Community 身份和返回摘要的
位置在完整状态中保持可见。

### 8.3 对象定位与 Inspector

摘要中的 Project、Issue、Work、Role 或 Resource 都可以直接打开 Inspector。实现可以
选择：

- 在摘要状态旁打开 Inspector；
- 自动展开完整 View 并定位对象；
- 在窄窗口中使用完整宽度抽屉。

无论采用哪种表现，都必须满足：

- 对象 ID 写入可恢复 URL；
- 关闭 Inspector 后焦点返回来源；
- 关系跳转仍在同一 Community 内；
- 不复制第二个对象详情模型。

### 8.4 收起

从完整 View 收起时：

- 返回同一 Community 的项目摘要；
- 已经成功写入的状态不会改变；
- 未提交表单不能被静默丢弃；
- 如果存在编辑弹窗或 conflict 草稿，应先要求 Human 明确处理；
- Object Inspector 的关闭或保留策略必须可预测，并与 URL 同步。

## 9. Community 切换行为

### 9.1 主动切换

Human 从 Community Rail 选择另一个 Community 时：

1. 当前 Community 的展示、订阅和临时选择离开可见范围；
2. 保存当前 Community 的有效 Inbox/Channel destination；
3. 打开目标 Community 的展示页；
4. 保留目标 Community 之前保存的 destination，并提供“继续上次工作”；
5. Project View 可用时默认呈现目标 Community 的可信摘要；
6. 使用目标 Relay 重新取得 verified snapshot；
7. 不要求再次点击独立 `View` 入口。

完整展开状态默认不跨 Community 继承。这样可以避免 Human 把前一个 Project 的滚动
位置、对象选择或编辑局势误认为目标 Project 的延续。

“默认进入 Overview”与“记住上次工作位置”不是互斥行为：前者决定切换后的第一屏，
后者保存 Human 在目标空间内可以继续的工作。实现不得为了简化第一屏而删除 destination
storage、目标 Channel 验证或失效回退。

### 9.2 状态隔离

Community 切换必须继续清理或重新绑定：

- Project View query；
- Role Continuity read model；
- live projection subscription；
- selected object 与 URL 定位；
- 初始化、创建和编辑草稿；
- conflict 基线；
- Actor 与 Role Directory 解析结果。

不得为了让 Community 首页更快而增加缺少 reset 的 module-level Community cache。

目标 Community 自己的 Inbox/Channel destination 不属于需要清除的跨 Community
临时状态。它必须按 Community ID 隔离保留，且不能被前一个 Community 的值覆盖。

### 9.3 连接恢复不是主动导航

同一 Community 的 Relay 断线恢复、应用重启恢复或后台重新验证，不应强制把 Human 从
Inbox、Channel 或完整 View 跳回 Community 展示页。只有 Human 主动选择 Community
或打开 Community 展示入口时，才发生相应导航。

## 10. 页面状态

Project View 成为 Community 展示页的一部分后，状态应在项目区域内清楚表达，但不把
整个 Community 错误地解释为不可用。

### 10.1 Preview disabled

Project View preview 关闭时：

- Community 名称、图标、当前成员身份和稳定导航仍然显示；
- “继续上次工作”、Inbox、Channel 与其他已启用功能仍然可访问；
- Project View 区域使用紧凑说明，并可提供 `Open Experiments`；
- 不读取 Project View snapshot，也不建立 projection live subscription；
- Community Rail 切换不能把 Human 强制落在只有启用提示的大面积空页面。

preview disabled 是项目区域的产品开关状态，不是 Community 的 capability failure。

### 10.2 Uninitialized

未初始化时，Community 展示页仍然存在，项目区域说明：

- 这个 Community 是一个 Project Space；
- Project View 尚未建立；
- owner 或有权成员可以初始化 Project Profile 与第一个 Goal。

Human 开始初始化后，可以展开现有原子初始化流程。其他 Community 能力不因 View 尚未
初始化而被描述为不存在。

### 10.3 Unsupported

Relay 不支持 Project View 时，项目区域显示明确的 capability 说明。Inbox、Channel、
Projects 等既有功能继续可用。

独立 `View` 入口被移除后，Unsupported 不能退化为静默隐藏，从而让 Human 误以为
Community 没有项目语义。

### 10.4 Loading 与 Refreshing

- 初次 Loading 只为项目区域使用稳定摘要骨架，Community 外壳与继续入口立即可用；
- Refreshing 保留上一份 verified 摘要与完整 View；
- 新 projection event 仍只作为失效信号；
- UI 只在新的完整 snapshot 验证后原子切换 revision。

### 10.5 Offline / stale

离线时可以保留最后一份 verified 内容，但只使用一个主要 stale 状态标识，并在
Community 导航附近提供必要辅助说明。不能把旧 Role Assignment、Work Commitment 或
Project revision 表现为已确认的最新状态。

### 10.6 Forbidden 与 Integrity failure

- Forbidden 说明当前身份无法读取这个 Community 的 Project View；
- Integrity failure 对项目内容 fail closed，不展示混合或可疑摘要；
- Community 的其他独立能力是否仍可使用，由它们自身的权限与连接状态决定；
- 不允许从本地副本、数据库直读或旧摘要绕过验证失败。

## 11. Responsive Desktop 表现

### 11.1 宽窗口

宽窗口可以采用：

- Project identity 横跨页面顶部；
- Current Focus 与 Role Summary 并列；
- Needs Attention 与 Resources 使用紧凑区域；
- 完整展开后沿用现有宽主视图；
- Inspector 固定在右侧。

在常用的 1280×720 Desktop 视口中，首屏优先保证以下内容可见：

1. Community 与 Project 身份；
2. 当前方向；
3. Current Focus 的主要对象；
4. Leader、当前查看者 Role 或 vacancy；
5. 至少一个最重要的显式 Needs Attention 信号。

Resources、完整对象列表和低优先级统计可以自然滚动到首屏以下。页面不应通过重复
`VERIFIED` badge、大块无内容卡片或过多纯数字统计挤占这些信息。

### 11.2 窄窗口

窄窗口中：

- 摘要区域改为纵向顺序；
- 项目定位与当前 Role 优先于大面积统计卡；
- 完整 Map 使用单列；
- Inspector 继续使用可访问的右侧抽屉或完整页面；
- 展开和收起不依赖 hover；
- 所有可读文本继续使用 rem 字体尺度。

## 12. 与现有实现和文档的关系

本文覆盖 [Project View 客户端 v0 设计](frontend-design.md)中的以下旧结论：

- `View` 是与 Inbox、Pulse、Projects 并列的主侧栏入口；
- Human 必须先进入独立 `/view` 页面才能看到 Project View；
- Role 只在完整地图之后作为 Supporting Object 展示。

以下结论继续有效：

- 一个 Community 只有一幅 Project View；
- Human 与 Agent 使用同一份 Relay 权威状态；
- 完整 View 使用项目地图与 Object Inspector；
- 修改使用类型化 intent；
- conflict、revision、一致性与完整性继续 fail closed；
- `Projects` 保持现有 Git/NIP-34 语义；
- `/view` 和对象定位可以作为兼容路由保留；
- 每个 Community 的 Inbox/Channel destination、目标 Channel 验证和失效回退继续有效；
- 冷启动、同 Community 重连与后台刷新不主动改写 Human 当前路由。

改造前不存在独立的 Community 展示页，因此本文新增的是一个入口和稳定外壳，而不是
覆盖旧 Community 页面。它也不能借此重定义既有 Community Rail 的全部连续性语义：
切换后的第一屏可以改为 Overview，但保存、验证和继续上次工作仍必须保留。

[Project View + Role Continuity Agent 上下文完善设计](../role/project-view-role-continuity-context-design.md)
中“一个 Community 是一个持久 Project Space”的 Agent 语义，与本文的 Human 前端
语义现在保持一致：

```text
Agent 从 system context 知道自己处于 Project Space
Human 从 Community 展示页直接看到 Project Space
二者从同一份 verified Project View 与 Role Continuity 状态工作
```

## 13. 验收标准

本次前端信息架构调整完成后，应满足：

1. Human 选择 Community 后，一次操作即可看到该 Project 的默认摘要。
2. Project View preview 关闭时，Community 展示页仍显示稳定空间身份、“继续上次
   工作”和既有入口，不出现只有启用提示的大面积空页面。
3. Project View 可用时，默认摘要直接呈现 Project name、Positioning 或 Purpose，
   而不是只显示功能入口。
4. Current Focus、Role 承担状态与明确注意事项无需打开独立 `View` 模块即可查看。
5. 完整 Project View 可以从 Community 展示页一次展开，并保留现有全部读写能力。
6. Role 是 Community 展示的一等协作结构，owner、Leader、Role、Assignment 与 Runtime
   不会在文案和视觉上混淆。
7. 摘要与完整 View 来自同一个 verified project revision 和 projection generation。
8. Live update 后摘要和完整 View 原子切换到同一新 revision。
9. Community 切换默认进入目标 Community 摘要，不携带旧对象、Role、草稿或 conflict。
10. 切换前后，各 Community 的有效 Inbox/Channel destination 均被保留；Overview
    提供经过目标空间校验的“继续上次工作”，失效 Channel 安全回退到 Inbox。
11. Overview、完整 View、Inspector 和 Settings 不会静默覆盖工作 destination。
12. `/view?object=<id>` 仍能直接打开当前 Community 的完整 View 与目标 Inspector。
13. Preview disabled、Unsupported、Uninitialized、Offline、Forbidden 和 Integrity
    failure 在 Community 展示页中都有明确且安全的表达。
14. 1280×720 首屏优先显示项目身份、当前方向、Current Focus、关键 Role 和至少一个
    重要注意事项；可信状态不以同等权重重复显示。
15. 现有 `Projects`、Inbox、Pulse、Agents、Workflows 和 Channel 行为不因本设计改变
    其领域语义。
16. 原有 destination 保存、目标 Channel 验证、不可用回退和重连不改路由的测试场景
    必须保留或按 Overview + Continue 语义改写，不能直接删除覆盖。
17. 客户端没有新增第二份 Project Profile、Role Directory、摘要文档或未验证缓存。

## 14. 当前结论

新的前端关系是：

```text
Community 是 Project Space
Project View 是 Community 的当前可见面
Role Continuity 是 Community 的协作连续状态
Inbox、Pulse、Projects、Agents、Workflows 和 Channels 是空间中的工作入口
每个 Community 的上次工作位置是客户端导航连续性，不是 Project View 状态
```

因此，Project View 应首先出现在 Community 展示页中，以默认摘要建立环境感，再由
Human 按需展开完整项目地图和对象细节。Community 展示页同时保留“继续上次工作”，
使项目整体认识与日常协作连续性可以共存。

这不是把一个侧栏按钮移动到另一个位置，而是让 Human 界面与系统已经建立的领域模型
对齐。它也不是让一个 preview 功能接管 Community；稳定空间外壳必须先于 Project
View 的启用状态成立：

> 进入 Community，就是进入一个保有共同项目现实、责任结构和连续状态的 Project
> Space。
