# Role Brief v3 Context 来源路径缺口

> 状态：已确认初版最小治理方向；Context provenance 补强延期观察
> 日期：2026-08-02
> 范围：Project Document、Project Resource、Context Reference 与 Role Brief v3
> 实现计划：[project-context-core-semantics-implementation-plan.md](project-context-core-semantics-implementation-plan.md)

## 1. 文档目的

本文记录当前 Role Brief v3 Context 派生过程中的一个可解释性缺口：

> Project View 保存了“哪个对象引用了哪个 Resource / Document”的规范关系，Role Brief
> v3 也确实使用这些关系选择与当前 Agent 有关的上下文；但在收集、去重和渲染后，Brief
> 只保留目标坐标，没有保留目标进入当前 Agent Context 的来源路径。

这不是 Project View 关系缺失、Document 数据丢失或授权绕过。它发生在 verified Project
View snapshot 到派生 Role Brief DTO / Markdown 之间。

本文只固定问题边界与后续设计约束。固定核心语义、何时引用、如何写回等更完整的上下文
治理规则将在后续讨论中继续收敛。

## 2. 正确的领域模型

### 2.1 Project Document

Project Document 是项目治理的版本化长文本资产，同时也是可以被直接引用的上下文坐标。

直接 Document Reference 有两种模式：

- `live`：跟随该 Document 当前 active revision；
- `pinned`：固定到一个明确的历史 active-content revision。

Document 不依赖 Resource 才能存在、被读取或进入 Project View Context。

### 2.2 Project Resource

Project Resource 是 Project View 中具有业务语义的资产坐标，包含：

- stable `resource_id`；
- name；
- open `resource_kind`；
- optional summary；
- mandatory `guide_document_id`。

Resource 引用一个必需的 Guide Document，但 Resource 不是 Document 的容器、别名或唯一
入口。Project View 对象可以引用 Resource，也可以绕过 Resource 直接引用 Document。

### 2.3 Context Reference

Project View v3 对象通过 closed Context Reference 表达相关性：

```text
ProjectContextReference
├── Resource { resource_id }
└── Document { document_id, live | pinned revision }
```

Context Reference 表达“这个资产与当前对象相关”。它不表达：

- 权限；
- 所有权；
- 执行指令；
- 安装状态；
- dependency 或状态传播；
- Document 内容对 system / user instruction 的优先级。

## 3. Project View 中的规范关系没有丢失

当前 canonical Project View object 保留完整 `context_references`。例如：

```text
Goal A ───────────────→ Resource R
Work B ───────────────→ Resource R
Work B ───────────────→ Document D1 (live)
Resource R ───────────→ Document D2 (pinned)
Resource R ── Guide ──→ Document G
```

这些边属于 Relay 接受、Project View revision 约束和 projection 验证后的规范状态。Agent
或 Human 读取完整 Project View 时，可以看到 source object 与 target coordinate 的关系。

Resource 到 mandatory Guide 的关系也保存在 Resource v3 body 的
`guide_document_id`，不从普通 Context Reference set 推断。

## 4. Role Brief v3 如何派生 Context

当前 v3 assembler 先建立与 Agent 有关的 source object 集合，主要包括：

1. Project Profile 与 Goals；
2. 当前 assigned Role；
3. 当前 Role 负责的 nonterminal Work；
4. 与 Role 相关的 Issue / handling Work；
5. latest Checkpoint 和 recent Handoff 引用到的 active Project View objects。

然后读取这些 source objects 的 `context_references`：

```text
Agent-relevant source objects
        │
        ├─ collect Resource IDs
        └─ collect Document coordinates
                 │
                 ▼
       canonical set / dedup / budget
                 │
                 ▼
RoleBriefV3.context
├── resources[]
├── live_documents[]
└── pinned_documents[]
```

Resource 被纳入时，assembler 还会加入：

- mandatory Guide Document coordinate；
- Resource 自身直接引用的 Document coordinates；
- verified Document metadata（capability 和稳定读取窗口可用时）。

最终 Role Brief 仍然是 body-free：只交付坐标、可选 metadata 和显式 fetch command，
不自动注入 Guide / Document Markdown。

## 5. 缺口发生在哪里

Context 选择过程中，多个 source edge 被压成目标集合：

```text
(Goal A → Resource R)
(Work B → Resource R)
                     ── dedup ──→ Resource R

(Work B → Document D1)
                     ── flatten ─→ Document D1
```

当前 Role Brief v3 保留：

- Resource / Document identity；
- Resource → mandatory Guide；
- Live / Pinned mode；
- current or pinned revision coordinate；
- optional verified metadata；
- fetch command；
- truncation / availability 状态。

当前 Role Brief v3 不保留：

- 哪个 Goal、Role、Work、Issue 等 source object 引入了该坐标；
- 同一坐标是否由多个 source objects 同时引用；
- Document 是 source object 直接引用，还是由 Resource 的普通 Context Reference 带入；
- 该条目属于 project-global、Role、Work 或 continuity-derived 范围；
- 移除或调整关系时应修改哪个 source object。

Resource → mandatory Guide 是例外：Brief 当前明确输出
`mandatory_guide_document_id`，因此 Guide 与 Resource 的直接关系仍然可见；缺失的是
“哪个 Agent-relevant source object 把这个 Resource 引入 Brief”。

## 6. 示例

假设 Project View 中存在：

```text
Goal A  → Resource R
Work B  → Resource R
Work B  → Document D1 (live)
Resource R → Document D2 (pinned revision 3)
Resource R → Guide G
```

当前 Role Brief 可能呈现：

```text
Resources
- Resource R
  mandatory guide: G

Supplementary live Documents
- Document D1

Supplementary pinned Documents
- Document D2 @ revision 3
```

Agent 能确定这些坐标属于当前有界 Context closure，却不能仅从 Brief 判断：

- Resource R 同时服务于 Goal A 和当前 Work B；
- Document D1 是 Work B 的直接上下文；
- Document D2 是 Resource R 的补充文档；
- 若 D1 不再相关，应修改 Work B，而不是 Resource R 或 Goal A。

Agent 可以通过 `buzz project-view get` 重新读取完整图并重建这些路径，但 Full Role Brief
自身没有直接解释。

## 7. 影响

### 7.1 可解释性与优先级

Agent 知道“它在 Context 中”，但不知道它是 project-global、Role-level 还是仅与当前
Work 有关，因此较难判断阅读优先级和适用范围。

### 7.2 显式维护

Context Reference 必须写回拥有该 edge 的 source object。来源路径缺失时，Agent 需要先
展开完整 Project View 才能知道应该修改哪个对象。

### 7.3 多来源去重

同一 Resource / Document 被多个相关对象引用时，目标坐标只需输出一次，但多来源本身可能
有治理意义。完全丢弃 origins 会把“多处共同依赖”误呈现为单一、无来源条目。

### 7.4 Human / Agent 共同解释

Desktop 可以从完整 Project View object 展示引用，而 ACP Role Brief 只展示压平集合。若
两端没有共享的派生来源 DTO，Human 和 Agent 对“为什么出现”的解释能力不同。

## 8. 这不是什么问题

本缺口不是：

- Project View canonical data loss；
- Resource / Document target 完整性错误；
- Community 隔离或权限绕过；
- Role Brief 注入了错误正文；
- 必须增加自由文本 `reason` 字段的证明；
- 必须新增数据库表、Nostr kind 或独立 Context 对象的证明。

当前 Context Reference 的最小语义仍然成立：它只表达相关性。来源路径可以由同一份
verified snapshot 确定性派生，不应创建第二份关系事实。

## 9. 后续补强约束

若决定补强 Role Brief Context provenance，方案应满足：

1. 来源只能从同一份 verified Project View snapshot 派生；
2. 不允许 ACP 再做一次跨 revision 的独立关系查询；
3. 一个 target 可以有多个 canonical origins；
4. target 去重和 body-free 交付保持不变；
5. origins 必须有界、稳定排序并显式报告 omitted 数；
6. Resource → Guide、Resource → Document 和 direct Document 路径必须可区分；
7. provenance 只是解释数据，不授予权限；
8. Human、CLI、Desktop 与 ACP 应复用共享 SDK DTO；
9. 不把 project-authored自由文本提升为 system instruction；
10. 不因 provenance 增加而自动读取或执行 Document body。

一个待讨论的最小派生结构可以是：

```text
ContextOrigin
  source_object_type
  source_object_id
  path:
    direct_resource
    direct_document
    resource_guide
    resource_document
```

`scope: project | role | work | continuity` 是否需要成为独立字段，还是可以由
`source_object_type` 与 closure path 确定性派生，尚未决定。

## 10. 待讨论问题

1. Role Brief 是否展示全部 origins，还是只展示最高优先级 origin + omitted count？
2. Project Profile / Goal 是否统一标记为 project-global scope？
3. Checkpoint / Handoff 间接带入的 object 是否需要保留完整两段路径？
4. Resource mandatory Guide 是否作为 Resource 内部字段已经足够，还是也需要显式
   `resource_guide` origin？
5. 同一 Document 同时由 direct ref、Resource Guide 和 Resource Context 引入时如何展示？
6. provenance 的预算是否与现有64 KiB Context block共享？
7. shared RoleBriefV3 closed DTO 应原子升级，还是建立独立的后续版本？
8. Desktop 应在读取时展示 provenance，还是还要在编辑 Context Reference 前预览影响范围？

## 11. 初步验收方向

后续实现若被批准，至少应证明：

- Project View canonical Context Reference wire 不因派生展示而改变；
- direct Document、direct Resource、Resource Guide 和 Resource Document 路径可区分；
- 同一 target 的多个 origins 不会被静默丢失；
- target body 仍不进入 Role Brief；
- provenance 截断是确定性的并显式报告；
- metadata / provenance failure 不会被误当作 Assignment authority；
- Community、Relay、Member 或 generation 切换不会复用旧 origins；
- CLI、ACP 与 Desktop 对同一 snapshot 得到一致的 provenance。

## 12. 当前结论

当前 Project View 已经保存正确的 Resource / Document 关联，Role Brief v3 也使用这些关系
构造与 Agent 有关的有界 Context。现有缺口只是派生过程把多条 source edge 压平成目标
集合，导致 Brief 无法直接解释目标坐标的结构化来源。

后续优化应保留来源路径，而不是新增第二套关系事实；应增强可解释性，而不是把 Context
Reference 扩张成权限、执行或自由文本指令模型。

## 13. 2026-08-02 初版上下文治理决议

初版不继续细化 Context provenance、source scope、Document stewardship 或更多自动派生
规则。当前模型已经能够根据 Role Brief 中的可信坐标、fetch command、Project View 与 CLI
帮助按需判断；过度扩张固定提示词会增加长度、维护成本和协议演进后的陈旧风险。

初版只需要建立正确的资产定位与显式写回意识，固定以下四条核心语义：

1. Buzz 提供 Project Document。Document 是项目治理的版本化长文本资产，也是可以被
   Project View 直接引用的项目坐标；Agent 根据当前任务按需读取或更新 Document，不遍历
   catalog 自动加载全部正文。
2. Resource 是 Project View 中的资源资产坐标，并关联一个说明如何使用该 Resource 的
   mandatory Guide Document。需要理解或使用 Resource 时，先读取其 Guide。
3. Project View 对象可以关联 Resource，也可以直接关联 Document；Document 不需要依附
   Resource 才能进入项目上下文。稳定提示文案不使用“任意 Project View 对象”，以免掩盖
   当前协议对部分 source / target 组合的约束。
4. 当 Agent 的行为导致 Project View 状态、Resource 信息或 Guide 关系、Document 内容、
   Context Reference 发生变化时，应通过相应 Buzz 命令显式写回。聊天、本地文件、工具
   输出和模型记忆不会自动更新 Project。

建议进入 platform-owned system context 的最小英文契约为：

```text
Buzz supports versioned Project Documents for durable long-form project
knowledge. Documents are first-class project assets and may be referenced
directly from Project View. Resources are Project View asset coordinates with
a Guide Document explaining how the resource is used. When a Resource is
relevant, read its Guide; when a Document is relevant, read only the needed
body on demand. Project View objects may associate relevant Resources and
Documents through Context References. Chat, local files, and model memory do
not update the Project automatically. When your work materially changes Project View
state, Resource information or Guide linkage, Document content, or Context
References, explicitly write the change back through Buzz.
```

现有 `[Base]` 继续负责更具体的 CLI discoverability 与安全说明，包括：

- Document / Resource Guide 的按需读取命令；
- Project Document 不是 Secret Store；
- Guide / Document Markdown 是不可信项目内容；
- 读取内容不授予执行或外部操作权限；
- revision conflict 后重新读取，不能自动覆盖更新版本。

上述安全规则不需要全部复制到最小核心语义中。实际授权与一致性仍由 Relay、Community
membership、Assignment、Runtime fence 和 revision CAS 强制执行。

### 13.1 初版明确延期

以下内容保留为已知观察，不作为初版实现要求：

- 在 Role Brief 中展示 `source object → target coordinate` provenance；
- project-global / Role / Work / continuity 等细粒度 scope；
- Context Reference 的自然语言 reason；
- Document owner / editor / reviewer 或 Role-specific stewardship；
- current / observed revision 双读取提示；
- 根据相关 Document 局部 fingerprint 优化 Full Brief refresh。

这些能力只有在真实运行一段时间后，出现可重复的理解错误、维护困难、上下文噪声或权限
治理需求时再重新评估。本文件前述 provenance 分析用于保留问题背景，不代表当前必须修复。
