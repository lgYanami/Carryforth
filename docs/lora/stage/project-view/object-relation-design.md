# Project View 基本对象与关系设计

> 本文定义 Project View 首版对象之间的基本关系、基数、约束和呈现规则。
> 它服务于 Project View 接入 Buzz，使 Human 与 Agent 能够查看和修改同一幅项目视图。
> 本文不定义项目连续性、项目上下文、成员承担、权限治理、存储结构、事件 kind、同步机制或完整用户界面。

## 1. 文档目的

[项目视图定义与项目上下文关系](./project-view.md)已经确定，项目视图应先让 Human
或 Agent 无需复杂推断地回答项目是什么、要做什么、进行到哪里，以及当前有哪些
Goal、Role、Plan、Stage、Requirement、Issue、Work 和 Resource。

在把 Project View 接入 Buzz 之前，需要进一步固定这些对象之间最简单、直接的关系，
避免实现阶段出现以下歧义：

- Project、Project Profile 和 Buzz Community 分别是什么；
- Plan 是否必须属于 Goal；
- Stage 是否可以脱离 Plan；
- Requirement 和 Issue 如何进入规划；
- Issue 所描述的问题可以发生在哪些对象上；
- Work 究竟处理什么；
- 未绑定对象应放在哪里；
- 父对象变化是否会自动改变或删除子对象；
- 同一对象在树形界面中出现时，哪一处是规范位置，哪一处只是引用。

本文只固化首版已经形成共识的关系。未来如果真实使用证明一对多或单一归属不足，
再通过后续设计扩展，不在首版预先建立通用关系图。

## 2. 首版核心结论

首版采用以下核心模型：

1. Project 是所有 Project View 对象最终且唯一的所有权边界。
2. 一个 Project 恰好具有一个 Project Profile，并具有一个或多个 Goal。
3. 一个 Goal 可以关联多个 Plan；一个 Plan 最多关联一个 Goal，也可以不关联任何 Goal。
4. 一个 Plan 可以包含多个 Stage；一个 Stage 必须且只能属于一个 Plan。
5. 一个 Stage 可以规划多个 Requirement 和 Issue。
6. 一个 Requirement 或 Issue 最多进入一个 Stage，也可以尚未进入任何 Stage。
7. Issue 可以通过独立的 `about` 关系指出任何 Project View 元素上存在的问题。
8. Work 是处理 Requirement 或 Issue 的基本执行单位。
9. 一个 Work 必须且只能处理一个 Requirement 或一个 Issue。
10. 一个 Requirement 或 Issue 可以没有 Work，也可以由多个 Work 处理。
11. 对象关系和对象状态相互独立，不发生隐式状态级联。
12. 除 Plan–Stage 外，组织关系均可显式解除；解除后对象仍然直接属于 Project。

## 3. Project 是隐含根对象

### 3.1 Project 与 Buzz Community

Project 是 Project View 的隐含根对象。Buzz 接入首版采用：

```text
一个 Buzz Community = 一个 Project
```

Community 提供项目的身份边界、成员空间和租户隔离；Project View 表达这个 Project
当前可直接看到的项目状态。

Project 的服务端身份由当前请求所属的 Buzz Community 隐式确定，客户端不能提交或
选择另一个 Project 身份。Desktop 本地保存的 Community 连接 ID 和连接名称也不构成
共享的 Project 身份或 Project Profile。

Buzz Desktop 当前名为 `Project` 的对象实际表示 Git Repository。它不能与本文的
Project 混为一谈，而应作为一种 Repository Resource 出现在 Project View 中。

### 3.2 未初始化与已初始化

已有 Buzz Community 在首次创建 Project View 前可以处于“Project View 未初始化”
状态。未初始化不等于一个缺少 Project Profile 或 Goal 的合法 Project View。

第一次初始化必须一次建立：

- 唯一的 Project Profile；
- 至少一个 Goal。

初始化成功后，Project View 才进入本文定义的合法状态。后续任何修改都必须持续满足
“恰好一个 Project Profile、至少一个 Goal”的约束。

### 3.3 Project 所有权

以下所有对象都直接、最终且唯一地属于一个 Project：

- Project Profile；
- Goal；
- Role；
- Plan；
- Stage；
- Requirement；
- Issue；
- Work；
- Resource。

概念上，每个对象都具有唯一的 Project 归属。所有对象间引用必须发生在同一个
Project 内，不允许跨 Project 引用。

Goal、Plan、Stage 等关系只负责组织和呈现，不改变对象的 Project 所有权。

例如：

- Plan 与 Goal 解除关联后，Plan 仍属于原 Project；
- Requirement 从 Stage 移出后，Requirement 仍属于原 Project；
- Issue 指向另一个对象时，Issue 并不归该对象所有；
- Work 处理 Requirement 时，Work 仍是 Project 直接拥有的对象。

## 4. 四类基本关系

首版只定义四类关系。

### 4.1 Project 所有权关系

```text
Project owns ProjectProfile
Project owns Goal
Project owns Role
Project owns Plan
Project owns Stage
Project owns Requirement
Project owns Issue
Project owns Work
Project owns Resource
```

它回答：

> 这个对象属于哪个 Project？

这是所有对象都必须具有的关系。

### 4.2 规划组织关系

```text
Plan        optionally belongs under Goal
Stage       belongs under Plan
Requirement optionally belongs under Stage
Issue        optionally belongs under Stage
```

它回答：

> 这个对象在当前规划结构中放在哪里？

规划组织关系不表示所有权转移，也不自动表达目标贡献、依赖、阻塞、因果或完成条件。

### 4.3 Issue 问题定位关系

```text
Issue optionally about ProjectViewElement
```

它回答：

> 这个 Issue 描述的是哪个对象上出现的问题？

`about` 只表达问题定位，不表示：

- 这个对象拥有该 Issue；
- 这个对象导致了该 Issue；
- 该 Issue 阻塞了这个对象；
- 该对象会受到什么影响；
- 该 Issue 应在哪个 Stage 处理；
- 谁负责解决该 Issue。

这些含义如未来确有需要，应由独立关系或项目上下文表达，不能从 `about` 自动推断。

### 4.4 Work 执行关系

```text
Work handles Requirement XOR Issue
```

它回答：

> 这个 Work 实际为了处理哪个 Requirement 或 Issue 而存在？

Work 不直接处理 Goal、Plan、Stage、Role、Resource 或 Project Profile。若这些对象上
出现需要实际解决的问题，应先形成 Issue，再由 Work 处理该 Issue。

## 5. 关系总览

### 5.1 所有权视图

```text
Project
├── Project Profile
├── Goal
├── Role
├── Plan
├── Stage
├── Requirement
├── Issue
├── Work
└── Resource
```

这幅图表达所有权，不表达界面嵌套。

### 5.2 规划与执行视图

```text
Project
├── Goal
│   └── Plan
│       └── Stage
│           ├── Requirement
│           │   └── Work
│           └── Issue
│               └── Work
├── 未关联 Goal 的 Plan
│   └── Stage
│       ├── Requirement
│       │   └── Work
│       └── Issue
│           └── Work
├── 未规划 Requirement
│   └── Work
└── 未规划 Issue
    └── Work
```

这幅图表达首版的主要阅读顺序。未显示 Role、Resource 和 Issue `about` 引用。

### 5.3 Issue 问题定位视图

```text
Issue ── about? ──> ProjectViewElement

ProjectViewElement =
  Project Profile | Goal | Role | Plan | Stage |
  Requirement | Issue | Work | Resource
```

Issue 的规划位置、问题定位和执行工作是三个相互独立的维度：

```text
Issue
├── planned_in  → Stage?              在哪里安排处理
├── about       → ProjectViewElement? 问题出现在哪里
└── handled_by  → Work[]              实际安排什么工作处理
```

因此，以下情况都合法：

- Issue 既没有 Stage，也没有 `about`；
- Issue 有 `about`，但尚未进入 Stage；
- Issue 已进入 Stage，但没有具体 `about` 对象；
- Issue 已进入 Stage，同时指向另一个分支中的对象；
- Issue 指向 Resource，但在某个 Plan 的 Stage 中被处理；
- Issue 指向一个 Work，并由另一个 Work 处理。

## 6. 关系基数

| 来源对象 | 关系 | 目标对象 | 来源到目标 | 目标到来源 | 是否可为空 |
|---|---|---|---:|---:|---|
| Project Profile | `profile_of` | Project | 1 | 1 | 否 |
| Goal | `belongs_to` | Project | 1 | 1..* | 否 |
| Role | `belongs_to` | Project | 1 | 0..* | 否 |
| Plan | `belongs_to` | Project | 1 | 0..* | 否 |
| Plan | `under_goal` | Goal | 0..1 | 0..* | 是 |
| Stage | `belongs_to` | Project | 1 | 0..* | 否 |
| Stage | `under_plan` | Plan | 1 | 0..* | 否 |
| Requirement | `belongs_to` | Project | 1 | 0..* | 否 |
| Requirement | `planned_in` | Stage | 0..1 | 0..* | 是 |
| Issue | `belongs_to` | Project | 1 | 0..* | 否 |
| Issue | `planned_in` | Stage | 0..1 | 0..* | 是 |
| Issue | `about` | Project View 元素 | 0..1 | 0..* | 是 |
| Work | `belongs_to` | Project | 1 | 0..* | 否 |
| Work | `handles` | Requirement 或 Issue | 1 | 0..* | 否 |
| Resource | `belongs_to` | Project | 1 | 0..* | 否 |

表中的 `Project View 元素` 包括 Project Profile、Goal、Role、Plan、Stage、
Requirement、Issue、Work 和 Resource。

Issue 不得将自身设为自己的 `about` 对象。Issue 指向另一个 Issue 时只形成普通引用，
不形成递归所有权或嵌套结构。

## 7. 对象设计边界

### 7.1 Project Profile

Project Profile 是 Project 的一对一描述面，回答：

- 这是什么项目；
- 项目为什么存在；
- 项目大致解决什么问题；
- 项目的基本范围是什么。

首版关系：

- 必须且只能属于当前 Project；
- 一个 Project 恰好具有一个 Project Profile；
- 不拥有 Goal、Plan 等其他对象；
- 可以成为 Issue 的 `about` 对象。

Project Profile 不等同于 Project。Project 是长期存在的根对象，Project Profile 是
可以查看和修改的项目描述。

### 7.2 Goal

Goal 表达 Project 当前希望达到的上层结果。

首版关系：

- 必须属于当前 Project；
- 一个 Project 至少具有一个 Goal；
- 一个 Goal 可以关联零个或多个 Plan；
- Goal 不拥有 Plan；Plan 只是可选地组织在 Goal 下；
- Goal 可以成为 Issue 的 `about` 对象。

首版不定义：

- Goal 之间的优先级、依赖、冲突或父子关系；
- Goal 的自动达成判断；
- Goal 与 Requirement、Issue 或 Work 的直接关系。

### 7.3 Role

Role 表达 Project 内稳定、可识别的语义责任位置。

首版关系：

- 必须属于当前 Project；
- 一个 Project 可以具有零个或多个 Role；
- Role 可以成为 Issue 的 `about` 对象。

首版不定义：

- Human 或 Agent 对 Role 的承担；
- Role 的权限；
- Role 与 Goal、Plan、Stage、Requirement、Issue 或 Work 的责任关系；
- Role 与 Buzz relay owner、admin、member、Persona 或 Team 的等价关系。

Role 在首版中保持为 Project 的独立直接对象。

### 7.4 Plan

Plan 表达 Project 为推进整体而形成的规划逻辑和结构。

首版关系：

- 必须属于当前 Project；
- 可以关联零个或一个 Goal；
- 不得同时关联多个 Goal；
- 可以包含零个或多个 Stage；
- 可以成为 Issue 的 `about` 对象。

没有关联 Goal 的 Plan 是合法的 Project Plan，应直接显示在 Project 下的
“未关联目标的 Plan”区域。

Plan 通过 Stage 组织 Requirement 和 Issue。首版不允许 Requirement 或 Issue
跳过 Stage 直接关联 Plan。

Plan 与 Goal 的关联只表示当前组织位置，不自动表示：

- Plan 是 Goal 达成的充分或必要条件；
- Plan 完成会使 Goal 达成；
- Goal 的状态控制 Plan 的状态。

### 7.5 Stage

Stage 是 Plan 内部用于表达分段、位置或推进状态的结构。

首版关系：

- 必须属于当前 Project；
- 必须且只能属于一个 Plan；
- 不允许脱离 Plan 成为“未关联 Stage”；
- 可以规划零个或多个 Requirement；
- 可以规划零个或多个 Issue；
- 可以成为 Issue 的 `about` 对象。

Stage 必须具有稳定身份，使 Requirement、Issue 和 Issue `about` 关系能够稳定引用。
Stage 的稳定身份不表示它必须成为独立聚合或采用独立存储；它的生命周期和规范位置
仍然从属于一个 Plan。

首版不规定 Plan 内 Stage 必须是单一线性序列。Stage 的顺序、并行、分支及其他
Plan 内部结构由后续 Plan 对象设计决定。

### 7.6 Requirement

Requirement 表达 Project 希望实现、改变或满足什么。

首版关系：

- 必须属于当前 Project；
- 可以进入零个或一个 Stage；
- 不得同时进入多个 Stage；
- 可以由零个或多个 Work 处理；
- 可以成为 Issue 的 `about` 对象。

没有进入 Stage 的 Requirement 是合法的未规划 Requirement，应直接显示在 Project
下的“未规划 Requirement”区域。

Requirement 不直接关联 Plan。若需要把 Requirement 放入某个 Plan，必须选择该 Plan
内的一个 Stage。

### 7.7 Issue

Issue 表达 Project 已经发现的问题、缺口、异常或反馈。

Issue 与 Requirement 同处于“项目有事项需要处理”的层次，但语义不同：

- Requirement 表达希望做到什么；
- Issue 表达已经发现哪里存在问题。

Issue 不需要先转换成 Requirement 才能进入规划或产生 Work。

首版关系：

- 必须属于当前 Project；
- 可以进入零个或一个 Stage；
- 不得同时进入多个 Stage；
- 可以通过 `about` 指向零个或一个 Project View 元素；
- 可以由零个或多个 Work 处理。

没有进入 Stage 的 Issue 是合法的未规划 Issue，应直接显示在 Project 下的
“未规划 Issue”区域。

没有 `about` 对象的 Issue 被解释为 Project 级的一般问题。为避免两种等价表达，
首版不再额外使用显式 Project 根引用表达 Project 级 Issue；如果问题针对的是项目
描述本身，可以将 `about` 指向 Project Profile。

Issue 的 `about` 对象可以是另一个 Issue，但不得是自身。客户端必须把
Issue-to-Issue 关系显示为普通链接，不得递归复制完整 Issue 树。

### 7.8 Work

Work 是 Project 为处理 Requirement 或 Issue 而安排的基本执行单位。

后续协作中，Human 或 Agent 接受并执行的是 Work，而不是直接接受 Goal、Plan、
Stage、Requirement 或 Issue。

首版关系：

- 必须属于当前 Project；
- 必须且只能处理一个 Requirement 或一个 Issue；
- 不得同时处理 Requirement 和 Issue；
- 不得同时处理多个 Requirement；
- 不得同时处理多个 Issue；
- 不允许成为无处理对象的悬空 Work；
- 可以成为 Issue 的 `about` 对象。

如果一项实际行动看起来同时处理多个 Requirement 或 Issue，首版应：

- 拆成多个 Work；或
- 选择唯一主要处理对象，并把其他关系留待后续设计。

Work 与未来 Agent assignment、接受、执行、验证和交付关系不属于本文范围。

### 7.9 Resource

Resource 表达 Project 关联的代码仓库、文档、设计、服务、环境和已有产物等稳定入口。

首版关系：

- 必须属于当前 Project；
- 一个 Project 可以具有零个或多个 Resource；
- 可以成为 Issue 的 `about` 对象。

首版不定义 Resource 与 Goal、Plan、Stage、Requirement 或 Work 之间的
`uses`、`produces`、`depends_on` 等关系。这些对象如果需要表达某个 Resource 上的
问题，可以通过 Issue `about` 关系完成问题定位。

Buzz 中现有的 NIP-34 Repository 在 Project View 中映射为 Repository Resource，
而不是本文的 Project。

## 8. 规范位置与引用位置

一个对象在 Project 内只有一个规范身份，但可以因关系而在多个界面位置被引用。

### 8.1 Plan

- 有 Goal：完整 Plan 显示在该 Goal 下；
- 无 Goal：完整 Plan 显示在 Project 根的“未关联目标的 Plan”中；
- 其他位置只能显示引用，不复制 Plan。

### 8.2 Requirement

- 有 Stage：完整 Requirement 显示在该 Stage 下；
- 无 Stage：完整 Requirement 显示在 Project 根的“未规划 Requirement”中；
- Work 显示在 Requirement 下；
- Issue 通过 `about` 指向 Requirement 时，只在 Requirement 上显示 Issue 引用或标记。

### 8.3 Issue

- 有 Stage：完整 Issue 显示在该 Stage 下；
- 无 Stage：完整 Issue 显示在 Project 根的“未规划 Issue”中；
- Work 显示在 Issue 下；
- `about` 目标处只显示 Issue 引用、数量或状态标记，不复制完整 Issue。

这条规则保证：

- Issue 的规划位置由 `planned_in` 唯一决定；
- Issue 的问题位置由 `about` 独立表达；
- 同一 Issue 不会因多个视图关系形成多份可独立修改的副本。

### 8.4 Work

- Work 始终完整显示在它处理的 Requirement 或 Issue 下；
- Project 可以提供全局 Work 索引，但其中仍引用同一个 Work；
- 不存在“未关联 Work”区域。

### 8.5 Role 与 Resource

- Role 和 Resource 作为 Project 的直接集合显示；
- Issue 指向它们时只增加 Issue 引用，不改变其规范位置。

### 8.6 所有 Issue `about` 目标

无论 `about` 指向 Project Profile、Goal、Role、Plan、Stage、Requirement、Issue、
Work 还是 Resource，目标位置都只显示反向 Issue 引用、数量或状态标记。

Issue 的完整对象始终由 `planned_in` 决定规范位置：

- 有 Stage 时在该 Stage；
- 无 Stage 时在“未规划 Issue”。

客户端不得沿 `about` 关系递归嵌套完整 Issue。即使两个 Issue 相互指向，也只显示
普通链接。

## 9. 关系变更规则

### 9.1 绑定、解绑与移动

首版允许以下显式操作：

- 将 Plan 关联到一个 Goal；
- 将 Plan 从 Goal 解绑；
- 将 Plan 从一个 Goal 移动到另一个 Goal；
- 在 Plan 内创建 Stage；
- 将 Stage 显式移动到同一 Project 的另一个 Plan；
- 将 Requirement 或 Issue 放入一个 Stage；
- 将 Requirement 或 Issue 从 Stage 移出；
- 将 Requirement 或 Issue 从一个 Stage 移动到另一个 Stage；
- 设置、替换或清除 Issue 的 `about` 对象；
- 创建 Work 时设置其唯一的 Requirement 或 Issue 处理对象。

任何时点都必须保持本文定义的基数。移动关系应表现为一次关系替换，不能产生短暂或
最终的多父对象状态。

Work 创建后是否允许更换处理对象，以及在何种状态下允许更换，由后续 Work
对象与生命周期设计决定。本文只要求 Work 在任何有效状态下都必须恰好具有一个
处理对象。

### 9.2 不允许隐式级联

关系变化不得自动改变业务状态：

- Plan 关联或离开 Goal，不改变 Plan 或 Goal 状态；
- Stage 移动到另一个 Plan，不改变 Stage 状态；
- Requirement 或 Issue 进入或离开 Stage，不改变其状态；
- Issue 更换 `about` 对象，不改变 Issue 或目标对象状态；
- 本文定义的其他组织关系变化也不改变相关对象状态。

### 9.3 移除与引用保护

首版不允许通过硬删除对象制造悬空引用，也不允许静默级联删除。

在对象被硬删除前，必须显式处理现有关系：

- Project 的最后一个 Goal 不得删除；如需替换，在删除提交前必须已经存在替代 Goal。
  首版可以先用一次 Create 提交创建替代 Goal，再用后续 Delete 提交删除旧 Goal；每次
  已提交状态都合法，不要求引入通用 batch；
- Goal 仍有关联 Plan 时，必须先解绑或移动这些 Plan；
- Plan 仍包含 Stage 时，必须先移动或移除这些 Stage；
- Stage 仍规划 Requirement 或 Issue 时，必须先移出或移动这些事项；
- Requirement 或 Issue 仍有 Work 时，不允许硬删除该 Requirement 或 Issue；
- 任意对象仍被 Issue `about` 引用时，必须先清除或替换这些引用；
- Project Profile 不允许被移除。

归档或 tombstone 不等于硬删除。未来如果支持归档或 tombstone，可以保留稳定引用，
并在引用位置显示已归档或已删除状态。本文只固定硬删除的引用保护规则，不决定首版
是否提供归档或 tombstone。

## 10. 状态独立性

对象间存在结构关系，不代表状态可以从关系自动推导。

以下状态必须相互独立、显式修改：

- Plan 当前处于什么状态；
- Stage 当前处于什么状态；
- Requirement 是否已满足；
- Issue 是否已解决；
- Work 当前处于什么执行状态。

首版不建模 Goal 达成状态。未来如果引入 Goal 达成状态，也必须显式判断，不得从
Plan、Stage、Requirement、Issue 或 Work 的状态自动推导。

首版明确禁止：

```text
所有 Work 完成
  ⇏ Requirement 自动满足
  ⇏ Issue 自动解决
  ⇏ Stage 自动完成
  ⇏ Plan 自动完成
  ⇏ Goal 自动达成
```

同样：

- Issue 解决不自动完成相关 Work；
- Requirement 满足不自动完成相关 Work；
- Stage 完成不自动解决其中所有 Issue；
- Plan 完成不自动完成或关闭所有 Stage。

Project View 可以呈现这些状态之间看起来不一致的情况，但不能静默替项目作出判断。

## 11. 允许的首版场景

### 11.1 完整规划链

```text
Goal: 提升系统可靠性
└── Plan: 可靠性改进计划
    └── Stage: 数据库改进
        └── Requirement: 支持自动故障切换
            ├── Work: 设计故障切换协议
            └── Work: 实现故障切换测试
```

### 11.2 未关联 Goal 的 Plan

```text
Project
└── 未关联目标的 Plan
    └── Plan: 技术探索计划
        └── Stage: 原型验证
```

Plan 合法存在，但当前没有声明它属于哪个 Goal。

### 11.3 未规划 Requirement 直接产生 Work

```text
Project
└── 未规划 Requirement
    └── Requirement: 增加数据导出能力
        └── Work: 验证导出格式
```

进入 Stage 不是产生 Work 的前提。

### 11.4 Resource 上的问题进入规划

```text
Resource: production-api
    ↑ about
Issue: 生产接口偶发超时
    ├── planned_in → Stage: 稳定性修复
    ├── Work: 收集慢请求样本
    └── Work: 优化数据库查询
```

Issue 的问题对象是 Resource，规划位置是 Stage，两者相互独立。

### 11.5 执行中发现新问题

```text
Work A: 执行数据库迁移演练
    ↑ about
Issue: 回滚脚本无法恢复索引
    └── Work B: 修复并重新验证回滚脚本
```

Issue 可以直接指向 Work A，无需为了 Issue 创建 Requirement。

### 11.6 Stage 中同时存在 Requirement 和 Issue

```text
Stage: 发布准备
├── Requirement: 完成发布说明
│   └── Work: 编写发布说明
└── Issue: 候选版本存在启动错误
    └── Work: 定位并修复启动错误
```

Requirement 与 Issue 语义不同，但都可以成为 Stage 中的待处理事项。

## 12. 首版禁止的关系

以下关系不属于首版：

- 一个 Plan 同时关联多个 Goal；
- 一个 Stage 同时属于多个 Plan；
- 一个 Stage 脱离 Plan 独立存在；
- 一个 Requirement 同时进入多个 Stage；
- 一个 Issue 同时进入多个 Stage；
- 一个 Issue 同时具有多个 `about` 主对象；
- Issue 将自身作为 `about` 对象；
- 一个 Work 同时处理多个事项；
- 一个 Work 同时处理 Requirement 和 Issue；
- 一个 Work 不处理任何 Requirement 或 Issue；
- 任何跨 Project 引用；
- Requirement 或 Issue 跳过 Stage 直接关联 Plan；
- Goal 直接拥有 Requirement、Issue 或 Work；
- Role 直接承担 Work；
- Resource 直接拥有 Requirement、Issue 或 Work；
- 通过任一关系自动推导状态变化；
- 删除父对象时自动删除或关闭关联对象。

## 13. 首版 Project View 读取结构

Project View 的逻辑读取结果至少能够表达：

```text
ProjectView
├── ProjectProfile
├── Goals[]
│   └── Plans[]
│       └── Stages[]
│           ├── Requirements[]
│           │   └── Works[]
│           └── Issues[]
│               └── Works[]
├── UnboundPlans[]
│   └── Stages[]
│       ├── Requirements[]
│       │   └── Works[]
│       └── Issues[]
│           └── Works[]
├── UnplannedRequirements[]
│   └── Works[]
├── UnplannedIssues[]
│   └── Works[]
├── Roles[]
├── Resources[]
└── IssueReferencesByTarget
```

这只是逻辑读取结构，不规定必须以一份文档、一个 JSON、一个数据库对象或一个接口
返回。规范状态仍由唯一对象和关系组成，树形结果只是 read model。

## 14. Human 与 Agent 的修改能力

Project View 接入 Buzz 后，Human 与 Agent 应能对同一组对象和关系执行等价操作：

- 查看完整 Project View；
- 创建和修改 Project Profile、Goal、Role、Plan、Stage、Requirement、Issue、
  Work 和 Resource；
- 初始化尚未建立 Project View 的 Community；
- 设置、解除和移动本文定义的关系；
- 看到未关联 Plan、未规划 Requirement 和未规划 Issue；
- 看到对象上的 Issue 引用；
- 看到关系冲突或不满足基数时的明确错误；
- 在一方修改后，让另一方看到同一份当前视图。

首版可以沿用 Buzz 的现有 Community 成员边界。更细的 Role assignment、领域权限和
治理规则不属于本文。

## 15. Buzz 接入边界

Buzz 中已有一些名称相似但语义不同的对象。首版不得直接等同：

| Project View 对象 | Buzz 中名称相似的对象 | 首版关系 |
|---|---|---|
| Project | Community | 一个 Community 映射为一个 Project |
| Project | Desktop `Project` | Desktop `Project` 实际是 Repository，映射为 Resource |
| Role | relay/channel role | 权限角色不是 Project Role |
| Role | Persona / Team | Agent 配置或组合不是 Project Role |
| Plan | Workflow | 自动化定义不是项目规划 |
| Stage | Workflow step | 自动化执行步骤不是 Plan Stage |
| Issue | NIP-34 Git Issue | Git Issue 可作为来源或关联材料，不自动等同项目级 Issue |
| Work | Agent Job / Workflow Run / PR | 执行机制和产物不是 Project Work 本身 |
| Resource | NIP-34 Repository | 可直接适配为 Repository Resource |

具体事件、存储、投影、CLI、测试和发布方案见
[Project View 后端实现设计](./backend-implementation-design.md)。Desktop、Web 和
Mobile 界面不属于该后端首版。

## 16. 首版明确不做

本文和首版 Project View 接入不实现：

- 项目连续性；
- 项目上下文；
- Role assignment；
- Human 或 Agent 对 Work 的接受和承担；
- 权限、authority 和治理；
- Goal、Plan、Stage、Requirement、Issue、Work 的自动状态推导；
- 原因、影响、依赖、阻塞和冲突关系；
- Requirement 或 Issue 的多 Stage 规划；
- Plan 的多 Goal 对应；
- Work 的多处理对象；
- Issue 的多 `about` 对象；
- Resource 的依赖图；
- 外部系统自动同步；
- 语义检索或 Context Compiler。

这些能力只有在 Project View 已经能够被 Human 和 Agent 共同查看、修改并实际使用后，
才根据暴露出的真实问题继续设计。

## 17. 关系验证清单

实现和评审至少需要验证以下行为：

1. Project View 必须只有一个 Project Profile。
2. Project View 至少具有一个 Goal。
3. 未初始化的 Community 不被误认为一个缺少 Profile 或 Goal 的合法 Project View。
4. 最后一个 Goal 不能被单独删除。
5. Plan 可以没有 Goal，但不能同时具有两个 Goal。
6. Stage 不能没有 Plan，也不能同时具有两个 Plan。
7. Requirement 可以没有 Stage，但不能同时进入两个 Stage。
8. Issue 可以没有 Stage，但不能同时进入两个 Stage。
9. 同一个 Stage 可以同时包含 Requirement 和 Issue。
10. Issue 可以没有 `about` 对象。
11. Issue 可以指向任意同 Project 的 Project View 元素，但不能指向自身。
12. Issue 的 `about` 和 `planned_in` 可以指向不同分支。
13. Issue 不需要 Requirement 即可产生 Work。
14. Work 必须且只能处理一个 Requirement 或 Issue。
15. 一个 Requirement 或 Issue 可以具有多个 Work。
16. 任意关系都不能引用另一个 Project 的对象。
17. 解绑 Plan 后，它出现在“未关联目标的 Plan”。
18. 移出 Requirement 或 Issue 后，它出现在对应的“未规划”区域。
19. Issue 的完整对象只按规划位置显示一次，`about` 目标只显示引用。
20. Work 完成不会隐式改变 Requirement、Issue、Stage、Plan 或 Goal 状态。
21. 硬删除被引用对象时不会产生悬空引用或静默级联删除。

## 18. 最终模型

首版关系可以压缩为：

```text
所有对象 ── belongs_to ──> Project

Plan        ── under_goal? ──> Goal
Stage       ── under_plan  ──> Plan
Requirement ── planned_in? ──> Stage
Issue       ── planned_in? ──> Stage

Issue       ── about?      ──> ProjectViewElement

Work        ── handles     ──> Requirement XOR Issue
```

其中：

- `?` 表示关系可为空；
- 不带 `?` 的关系必须存在；
- 所有引用都必须位于同一个 Project；
- 任何关系都不自动改变对象状态；
- 任何可选组织关系解除后，对象仍直接属于 Project；
- Issue 可以直接进入规划并由 Work 处理，无需转换为 Requirement。

这套模型是 Project View 首版接入 Buzz 的关系基础。
