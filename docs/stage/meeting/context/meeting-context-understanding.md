# Meeting 上下文问题理解

> 状态：阶段性理解记录，不是设计方案；后续讨论已继续修正本文结论
>
> 代码基线：`version/v1.0.0` @ `f5cec1716`
>
> 日期：2026-08-08

后续纠正过程和当前收敛结论见
[Meeting 上下文讨论历程](meeting-context-discussion-history.md)。本文保留为讨论过程中的阶段性
认识，不应单独作为最终问题定义。

## 1. 文档目的

本文只记录当前对 Meeting 本质、Project Context 已有能力和 Meeting 上下文问题的理解，
用于约束后续讨论不再偏离问题。

本文不定义：

- Meeting 协议、阶段或事件；
- 新的数据结构、领域对象或持久状态；
- 上下文选择、路由、转移或注入方案；
- Agent 调度、Session 复用或 Provider 接口改动；
- Prompt、CLI、Desktop 或 Relay 的具体改法。

## 2. 基本前提

Buzz 中的 Agent 长期自主运行，并在同一 Project Space 内分别承担 Role 和 Work。任意时点，
每个 Agent 交给 LLM 的上下文窗口都是有限的；随着工作持续、话题切换、上下文压缩、Session
重建和 Runtime 重启，Agent 当前能够直接使用的上下文也会变化。

一个项目问题的相关上下文通常不会集中在一处，也不会完整存在于一个 Agent 的当前窗口中。
它可能分散在：

- Project View 中的 Goal、Requirement、Issue、Work、Role 和 Resource；
- Project Documents 与 Resource Guide；
- Project Context Edge 关联的 Context Documents；
- 过去的 Meeting Board 与正式 Speech；
- 普通 Channel、Thread 和消息；
- 代码、Git 历史、工具观察和工作区文件；
- 不同 Agent 当前或过去的工作 Session。

不同 Agent 因为承担了不同 Role、Work，或者沿不同路径调查过同一问题，会分别接触并保留其中
不同部分。单个 Agent 很难在有限窗口内重新发现、读取并同时承载全部相关上下文。

## 3. Meeting 的本质作用

Meeting 的本质作用是**分担同一个问题的上下文**。

可以把一个问题的可用上下文近似表示为：

```text
问题上下文
  ≈ 小型共享上下文
  + Agent A 持有的相关分片
  + Agent B 持有的相关分片
  + Agent C 持有的相关分片
  + 尚未覆盖的缺口
```

其中每个 Agent 只需要承载自己与议题相关的有限分片。各 Agent 带着不同分片参会，在讨论中：

- 暴露当前问题需要的事实、经验、约束和证据；
- 发现彼此掌握的上下文不一致或不完整；
- 向持有对应上下文的 Agent 追问；
- 逐步建立足以判断问题的共同理解；
- 在不要求任何单个 Agent 装下全部上下文的前提下形成方案或决定。

因此，Meeting 的价值不在于制造多个相同模型的重复回答，而在于让多个有限上下文窗口共同
承载一个更大的问题上下文。

Meeting 也不应先把所有上下文合并成一份全集，再把全集注入每个 Agent。这样会重新受到单个
窗口上限约束，并消除上下文分担本身的价值。正确理解应是：相关上下文保持分布在不同 Agent
中，会议只逐步形成一个小于全部原始上下文的共享前沿。

## 4. Project View 与 Project Context 已经解决的问题

[Project View](../../project-view/project-view.md) 已经被定义为项目上下文的稳定坐标系。Goal、
Role、Plan、Stage、Requirement、Issue、Work 和 Resource 等对象提供项目当前状态与稳定入口。

[Project Context](../../project-context/project-context.md) 已经建立最小的上下文关联能力：

```text
两个或多个稳定项目坐标的精确无向集合
  + 一份或多份普通 Project Document
  = 一条 Project Context Edge
```

当前坐标包括：

- Project View 对象；
- Project Document；
- Meeting。

Agent 已经可以从一个或多个已知坐标出发，通过 `exact`、`incident` 和 `contains-all` 发现
相关 Edge，先读取轻量坐标与 Document 元数据，再按需读取 Document 正文、Meeting Board 和
正式 Speech。

因此，Project View 与 Project Context 已经回答：

```text
上下文锚定在哪里
哪些稳定对象共同关联一段上下文
如何从已知坐标发现相邻上下文
上下文正文或历史来源到哪里按需读取
```

Project Context 有意不自动判断哪条上下文对当前推理必然相关，也不自动执行语义搜索、完整性
判断、上下文编译或每 Turn 正文注入。相关性判断仍由实际工作的 Human / Agent 完成。

所以 Meeting 当前的问题不是缺少一套新的项目上下文、认知图谱、Claim 模型或知识存储。
重新建立这些模型会重复 Project View、Project Context 和 Project Document 已经承担的职责。

## 5. 最新 Meeting Coordinate 完成的边界

最新的 [Meeting Coordinate](../../project-context/meeting-coordinate-implementation-design.md) 使
Meeting 本身成为 Project Context 中可稳定引用的项目记录：

- verified terminal Meeting 可以成为 Context 坐标；
- formal discussion 和 Board 已冻结、处于 `finalizing_actions` 的 Meeting 也可以在严格条件下
  建立关联；
- Context 查询只 hydration 轻量 Meeting metadata；
- Board 与 Speech 继续按需读取；
- Action Finalization 可以把 Meeting、物化出的项目坐标和解释它们关系的 Context Document
  显式关联起来。

这解决的是**一场会议结束后如何成为未来工作的上下文来源**，以及未来 Agent 如何从项目坐标
回溯到该 Meeting。

它不解决当前这场 Meeting 开始时，各参会 Agent 如何找到并带入会前已经接触过的相关上下文。

## 6. 当前 Meeting 入场时实际发生什么

当前 ACP Session 按 `channel_id -> session_id` 隔离。Meeting 使用自己的 Channel，因此会形成
独立的 Meeting Session。参见
[`SessionState`](../../../../crates/buzz-acp/src/pool.rs)。

Meeting 调度优先寻找已经拥有**当前 Meeting Channel Session** 的运行槽，否则选择其他空闲槽。
它不会因为某个槽持有相关 Work、Document 或来源 Channel 的 Session，就优先把首次 Meeting Turn
交给该槽。

当前 Meeting Create 的主要输入是：

- title；
- description；
- frozen participant pubkeys；
- 一个只用于导航的可选 source Channel；
- 自由 Markdown initial Board。

参见 [`MeetingV2CreateParams`](../../../../crates/buzz-sdk/src/builders.rs)。这些输入没有表达一组
结构化的议题坐标，也没有表达每个 participant 应从自己的哪个来源 Session 取得上下文。

新的 Meeting Session 可以看到当前 Role Context、Meeting Board 和有界 Meeting 历史，也可以使用
已有工具按需读取 Project View、Project Documents、Project Context、消息、代码和 Git 历史。但是它
不会继承原工作 Session 中已经激活的上下文。

正式 participant Intent 又被明确限制为轻量发言判断，只允许在必要时做一次小型定向读取，不能
在此时执行广泛、多步的上下文调查。参见
[`meeting_participant_intent_prompt.md`](../../../../crates/buzz-acp/src/meeting_participant_intent_prompt.md)。

因此，当前流程实际是：

```text
根据 participant identity 召集 Agent
  → 为 Meeting Channel 建立或复用 Session
  → 注入会议当前状态
  → 直接判断是否发言
```

它还不是：

```text
根据议题找到相关上下文
  → 找到各 Agent 持有这些上下文的来源 Session
  → 让每个 Agent 取得自己的相关分片
  → Agent 带着分片进入 Meeting
```

## 7. 当前问题的准确表述

当前真正需要解决的是 **Meeting 入场前的上下文发现、选择、路由和携带问题**。

它至少包含以下尚未回答的问题。

### 7.1 议题如何获得上下文查找起点

Project Context 的查询从稳定坐标开始，但当前 Meeting 议题主要是 title、description 和自由 Board。
需要明确自然语言问题、触发它的 Work / Issue / Document / 消息与已有项目坐标之间如何建立本次
查找所需的起点。

这里缺少的是本次 Meeting 的查找入口，不是新的 Project Context 坐标体系。

### 7.2 如何识别每个 Agent 已经接触的相关部分

同一问题可能被多个 Agent 从不同 Work、Document、Channel、代码路径或历史 Meeting 接触过。当前
participant identity、Role 和 Assignment 只能说明身份与责任，不能直接说明某个 Agent 的哪个
Session 当前保留了哪些议题相关上下文。

邀请“相关 Agent”不自动等于取得“该 Agent 的相关工作上下文”。

### 7.3 如何选择正确的来源 Session

同一个 Agent 可以在不同 Channel 中拥有不同 ACP Session。相关上下文可能位于其中某一个工作
Session，而首次 Meeting Turn 当前只按 Meeting Channel 选择或创建 Session。

所以需要区分：

```text
participant identity routing
```

与：

```text
relevant source-session routing
```

当前实现具备前者，不具备后者。

### 7.4 如何在固定预算内选择相关分片

即使 Project Context 能发现许多相邻 Edge 和 Document，Agent 也不能把它们全部加载。每个 Agent
需要根据议题和自己已经掌握的局部上下文，判断哪些内容应继续留在自己的会议上下文中，哪些只
保留坐标，哪些不属于自己的上下文分片。

### 7.5 如何让分片进入 Meeting Session

即使相关内容已经存在于原工作 Session，Meeting Session 也不会自动继承它。需要解决的是如何让
每个参会 Agent 在正式讨论前，已经能够在 Meeting Session 中直接使用自己的相关上下文，而不是
进入会议后再从零调查。

### 7.6 如何保持上下文分担而不是重新集中

不同 Agent 的相关分片不应全部广播给所有参会者。Meeting 的共享部分只需要逐步维护：

- 当前共同讨论的坐标；
- 已经对齐的事实与结论；
- 仍然冲突或缺失的部分；
- 当前问题应由哪个持有相关上下文的 Agent 回答。

原始上下文继续由各 Agent 分别承载。否则 Meeting 会重新退化为一个单窗口上下文聚合问题。

## 8. “Agent 带着相关上下文参会”的含义

当前所说的“带着”不是指：

- Agent 拥有完整项目上下文；
- Agent 永久保存自己接触过的全部内容；
- 所有参会者获得相同 Prompt；
- 把所有来源正文复制进 Meeting Board；
- 仅仅邀请曾经承担相关 Role 的 Agent；
- 让参会 Agent 在 Intent / Speech Turn 临时从头调查。

“带着”是指：

> 在正式 Meeting 讨论开始时，每个参会 Agent 的当前 LLM 上下文中，已经存在由该 Agent 承担、
> 与本次议题相关且适合其窗口预算的上下文分片；其他参会者可以通过会议向它取得当前判断所需的
> 信息，但不需要复制并完整持有该分片。

这个分片可能来自仍然存活的原工作 Session，也可能由 Agent 沿 Project View / Project Context 坐标
按需重新取得。两者的上下文质量可能不同，但都不改变 Meeting 的分担作用。

## 9. 需要避免的再次误解

后续讨论不应再次把问题转化为：

1. 为全部项目认知建立新的结构化模型；
2. 让某份新的摘要或 Capsule 成为项目上下文事实源；
3. 让系统自动判断所有 Edge 的相关度、正确性、冲突和新鲜度；
4. 把完整 Project Context 注入每个 Agent；
5. 把全部 participant 上下文合并后注入每个人；
6. 把 Meeting Board 当成所有会前上下文的存储；
7. 把 participant pubkey、Role 或 Assignment 等同于当前仍然存在的相关 LLM 上下文；
8. 认为 Meeting 已成为 Project Context 坐标，就已经解决了本场 Meeting 的入场上下文问题。

这些方向都没有直接回答各 Agent 如何找到并带入自己的相关上下文分片。

## 10. 当前理解结论

当前各能力的边界可以概括为：

```text
Project View
  提供稳定项目对象和直接状态坐标

Project Context
  连接坐标并让解释性上下文可发现、可按需读取

ACP Session
  承载一个 Agent 此刻真正激活的有限上下文

Meeting
  应让同一议题的不同上下文分片由不同 Agent 分别承载，
  并通过受控讨论形成共享理解、方案或决定
```

因此，Meeting 当前需要面对的不是“项目上下文如何建模”，而是：

> 如何从一个会议议题出发，让各参会 Agent 找到与议题相关、由自己接触或负责的上下文，并使
> 这些不同的有限上下文分片在正式讨论开始时分别存在于各自的 Meeting Session 中。
