# Desktop 聚焦式 Full Project View 实现设计

> 状态：已确认，待实现。
>
> 日期：2026-08-14；最后更新：2026-08-15。
>
> 本文只修改 Carryforth Desktop 中 `/view` 的信息组织和交互。Community Overview、
> Project View v3 领域模型、Relay 权威状态、Role Continuity、Project Documents 和
> Project Context 的事实边界保持不变。

## 1. 文档目的

当前 Desktop 已经交付两种 Project View 阅读密度：

- `/community` 提供 Community / Project Space 的默认摘要；
- `/view` 提供完整 Project View、对象维护和 Inspector。

[Community 展示页中的 Project View 前端设计](../community-project-view-frontend-design.md)
已经确认“默认摘要，按需完整展开”的产品层级。当前需要修正的是第二层：现有 Full
Project View 把 Goal、Plan、Stage、Requirement、Issue 和 Work 同时展开在一个滚动
平面中。数据关系虽然是分层的，界面却让多个深度的卡片、状态、说明、修改来源和操作
同时争夺注意力，随着对象增长会退化为难以阅读的卡片墙。

本文把 Full Project View 改造成一个聚焦式 Project Explorer：

```text
右侧可折叠 Project Outline：全局结构和当前位置
主内容区：一个当前 Project View 对象的完整内容
           + 直属下一层对象的轻量摘要
           + 挂在当前对象上的 Issue、Resource 和 Document 轻量摘要

或者：一个已选 Document coordinate 的只读预览
```

本文给出可以直接进入实现的路由、读取模型、组件、交互、响应式、迁移和测试设计。
它不重新定义 [Project View 基本对象与关系](../object-relation-design.md)。

## 2. 已确认的核心结论

实现必须遵守以下结论：

1. Community Overview 保持项目摘要入口，不在本次重做。
2. `/view` 不再一次渲染完整对象树，而是一次只聚焦一个目录项。
3. 当前项是 Project View 对象时，主内容区只完整展示该对象；当前项是 Document 时，
   主内容区只显示该精确 Document coordinate 的只读预览。
4. 当前对象的直属结构子对象只显示类型、标题和 `summary`。
5. 通过 `Issue.about` 挂在当前对象上的 Issue 也只显示类型、标题和 `summary`。
6. 当前对象直接引用的 Resource 和 Project Document 也只显示类型、标题和 `summary`。
7. Resource Guide Document 以保留 Guide 语义的 Document 摘要显示。
8. 主内容区绝不继续渲染上述对象的子对象，即不渲染孙级内容。
9. 右侧提供可折叠 Project Outline，负责全局目录和唯一的位置指示。
10. 当前目录节点高亮并自动展开祖先；不新增面包屑、左侧位置栏或其他位置组件。
11. 删除现有右侧 Object Inspector；Inspector 能力进入当前对象主页面。
12. Summary 缺失时不得用 Description、Purpose、Document 正文或模型生成文本代替。
13. 摘要与完整内容继续来自同一份经过验证的 Project View / Document 状态，不建立
    第二份客户端项目现实。
14. Current Item 存在父级视图对象时，在 Main header 右上角提供“向上箭头 + 父级标题”
    的单步跳转；不显示父级 Summary，也不新增父级卡片、面包屑或位置栏。

一句话概括：

> 右侧目录始终提供全局结构和当前位置；主内容始终只完整展示一个当前项；页面中的其余
> 对象全部只是类型、标题和 Summary 组成的下一步入口。

## 3. 当前实现与问题

### 3.1 当前页面组成

当前 `/view` 由
[`ProjectViewScreen.tsx`](../../../../desktop/src/features/project-view/ui/ProjectViewScreen.tsx)
组织，Ready 状态依次渲染：

- 顶部 Add 操作；
- Project Profile；
- Current Focus 四项指标；
- [`ProjectViewMap.tsx`](../../../../desktop/src/features/project-view/ui/ProjectViewMap.tsx)
  中的全量对象树；
- Role 和 Resource；
- 页脚验证信息；
- 选中对象时的
  [`ProjectViewInspector.tsx`](../../../../desktop/src/features/project-view/ui/ProjectViewInspector.tsx)。

`ProjectViewMap` 在同一页面递归展开：

```text
Goal
└── Plan
    └── Stage
        ├── Requirement
        │   └── Work
        └── Issue
            └── Work
```

每个节点复用较完整的对象卡片。视觉缩进表达了关系，但没有形成逐层阅读行为。

### 3.2 当前问题不是领域层级本身

Goal、Plan、Stage、Requirement、Issue 和 Work 的直接关系仍然有效。问题是所有深度
同时展开，而且不同深度使用近似相同的视觉重量：

- 多层边框、背景和缩进叠加；
- 每层重复类型、状态、优先级、描述和修改来源；
- 每层同时出现上下文创建操作；
- 双列 Requirement / Issue 又引入横向扫描；
- 打开 Inspector 后主地图进一步变窄；
- 当前对象和当前阅读路径缺少唯一视觉焦点。

因此本设计改变的是阅读和导航投影，不改变领域对象及关系。

## 4. 目标与非目标

### 4.1 目标

- 让 Full Project View 在对象增长后仍然可以逐层阅读。
- 任意时刻只有一个 Current Item 拥有完整视觉重量。
- 让 Human 能从主内容或 Outline 快速进入直属下一层。
- 让 `Issue.about`、Resource / Document Context Reference 在来源对象下可见。
- 保留 `/view?object=<id>` 深链接、前进后退和外部对象定位。
- 保留现有初始化、可信读取、实时刷新、离线陈旧、权限和完整性状态。
- 保留对象创建、编辑、删除、Context、Role Continuity 和 Work Continuity 能力。
- 在宽 Desktop 中固定为右侧目录，在窄窗口中保持可访问的单面板体验。

### 4.2 非目标

- 不修改 Project View v3 schema、Nostr kinds、Relay projection 或数据库。
- 不把目录顺序提升为领域优先级、依赖、因果或执行顺序。
- 不增加拖拽排序、Kanban、甘特图或通用关系图。
- 不把 Project Context Edge 混入 Outline 的结构关系。
- Document 是 Full Project View Explorer 中的一等关联实体和可选节点，但不是
  `ProjectViewObject`；其内容与版本由 Project Documents 领域独立管理，Project View
  通过精确坐标引用它。
- 不在 `/view` 中复制 Document 编辑器、Revision History 或完整 Documents 工作区；只
  提供只读预览和进入 `/documents` 的操作。
- 不重新设计 Community Overview。
- 不把 Outline 展开状态写入 Relay 或 Project View。

## 5. 领域与可信状态不变量

### 5.1 Project View 是当前一阶状态

Full Project View 仍然读取同一个 schema-v3 verified snapshot。客户端不得依据未验证的
live event 正文局部修改 Outline 或主内容。现有 live subscription 继续只作为重新读取
完整快照的 invalidation 信号。

### 5.2 结构关系

主结构继续使用以下已登记关系：

```text
Goal?
  └── Plan
       └── Stage
            ├── Requirement
            │    └── Work[]
            └── Issue
                 └── Work[]
```

- Plan 可以没有 Goal；
- Requirement / Issue 可以没有 Stage；
- Stage 必须属于一个 Plan；
- Work 必须处理一个 Requirement 或 Issue；
- Role 和 Resource 直接属于 Project，不属于规划链。

### 5.3 Issue 挂载不是结构所有权

Issue 可以通过 `about` 指向任意 active Project View 对象。`about` 表示问题发生在哪里，
不改变 Issue 的 `planned_in_stage_id`、Project 所有权或 Work 处理关系。

因此同一个 Issue 可以同时：

- 在 Stage 或 Unplanned Issues 中拥有一个规范结构位置；
- 在它的 `about` 目标下拥有一个关联出现位置。

Outline 必须用不同 occurrence 表达这两个位置，不能复制 Issue 状态或把 `about` 解释成
移动。

无 `about` 的 Issue 是 Project 级一般问题，但协议中不存在可由此推导出的“指向 Project
根”关系。它只出现在 Unplanned Issues 的规范位置，不能伪造成 Project Profile 的
Related Issue。

### 5.4 Resource Context Reference

除 Resource 自身外，active Project View 对象可以通过 Context Reference 指向同 Project
的 active Resource；Resource 自身不能引用另一个 Resource。该关系不改变 Resource 在
Project 根下的规范位置。

因此 Related Resource 与 `about` Issue 一样是指向规范对象的引用 occurrence。它在 Main
中是三字段摘要，在 Outline 中是叶节点；点击后进入同一个 Resource 的完整当前对象页，
不能在引用位置复制 Resource 的 Guide 和 Context 子树。

### 5.5 Document 挂载不是 Project View 子对象

Project View 对象通过 Context Reference 指向 live 或 pinned Document Revision。
Document 保持独立身份、Revision 和权限；挂载只表示该 Document 与当前对象相关。

Resource 的 `guide_document_id` 是独立的必需关系，不得从普通 Context Reference 推导。
Resource 页面可以把 Guide Document 作为第一项 Document 摘要，但必须保留其 Guide
语义和精确 Document 身份。

Document 可以成为 Explorer 的 Current Item，但不会因此进入 Project View schema 或
`objectsById`。它的主内容只是精确 coordinate 的只读预览，编辑、History 和 Documents
列表仍属于 `/documents`。

### 5.6 Summary 只来自显式字段

Project View v3 和 Project Document 都允许显式 `summary`。摘要项目的 Summary 必须：

- 使用目标对象或目标 Document 自己的 `summary`；
- 缺失时显示中性的 `No summary provided.` 空状态，或者保留固定高度的空摘要位；
- 不回退到 Description、Purpose、Desired Outcome、正文首段或语义模型输出；
- 不从父对象、Role Brief、Checkpoint 或聊天内容推断。

这可以防止“轻量摘要”悄悄变成另一套派生项目事实。

Pinned Document 是特殊边界：Project View projection 和 current Document catalog 都不含
其历史 Revision 的独立 metadata；读取 exact Revision 又会同时取得完整 Markdown。
因此未选中的 pinned 摘要和 Outline 不得为了填充 Title / Summary 后台读取正文，也不得
借用 current head 的 Title / Summary。它使用明确的坐标标题（例如 `Document <short-id> ·
Pinned revision 4`），Summary 保持缺失状态。只有它成为 Current Item 后才读取 exact
Revision 并呈现只读预览。

## 6. 页面布局

### 6.1 宽窗口

App 的现有主侧栏保持不变。`/view` 内容区只有两个平面：

```text
┌──────────────────────────────────────────────┬────────────────────────┐
│ Current Item                                 │ Project Outline        │
│                                              │                        │
│ 当前对象完整内容       [↑ Parent title]      │ 当前 occurrence 高亮   │
│                                              │ 祖先自动展开           │
│ Direct children：type / title / summary      │ 可独立滚动             │
│ Related issues：type / title / summary       │ 可整体折叠             │
│ Related resources：type / title / summary    │                        │
│ Documents：type / title / summary            │                        │
└──────────────────────────────────────────────┴────────────────────────┘
```

选择 Document 时，同一个 Main 位置切换为该 live / pinned / guide coordinate 的只读预览，
Outline 继续高亮其引用 occurrence。Document 是叶节点，没有下一层对象。

`[↑ Parent title]` 位于 Main 的 Current Item header 右上角。它只是一个导航操作：

- 只显示向上箭头和父级对象 Title；
- 不显示父级 Type、Summary、Status、Revision 或其他字段；
- 固定在 Current Item header 的最右侧；Edit / Delete 等对象操作使用独立 action group，
  不插入箭头与父级 Title 之间；
- 没有父级的 Project Profile 不渲染该操作，也不保留空占位；
- 它不构成第二套当前位置展示，当前位置仍只由 Outline 高亮表达。

页面其余布局约束：

- Main 是唯一内容滚动面。
- Outline 是独立滚动的右侧辅助面板。
- Outline 打开时不得覆盖或截断当前对象内容。
- Outline 折叠时 Main 占用全部宽度。
- 不显示额外的当前位置组件或面包屑。

优先复用
[`AuxiliaryPanel`](../../../../desktop/src/shared/layout/AuxiliaryPanelShell.tsx)
的 Desktop / overlay 边界、焦点和宽度约束。Project Outline 首版可以采用固定默认宽度；
是否允许拖动调整宽度不是本设计的必要条件。

### 6.2 顶部 Chrome

现有 Full Project View 顶部 Chrome 保留：

- 返回 Community Overview；
- 当前 Community 名称；
- Verified、Syncing、Offline / stale 等状态；
- Outline 展开 / 折叠操作。

`Editable` 只表示当前可信快照可进入维护流程，不替代具体对象和 Role 的授权检查。

### 6.3 窄窗口

当共享辅助面板断点判定为单栏时：

- Main 保持全宽；
- Outline 由顶部操作打开为右侧 Sheet / overlay；
- Sheet 使用 `role="dialog"`、焦点约束和 Escape 关闭；
- 选择节点后关闭 Outline，并把焦点移到新 Current Item 标题；
- 再次打开时恢复当前 occurrence、展开祖先并滚动到高亮节点。

窄窗口也不增加独立当前位置组件。

## 7. 路由与当前选择

### 7.1 Current Item

`/view` 的规范选择规则为：

- `/view`：当前项是唯一 Project Profile；
- `/view?object=<id>`：当前项是该 active Project View 对象；
- `/view?document=<id>`：当前项是一个 live 或 Guide Document 的只读 current-head 预览；
- `/view?document=<id>&revision=<n>`：当前项是 pinned Document Revision 的只读预览；
- 初始深链接中的 object 不存在或不属于当前 verified snapshot：以 replace 导航回 `/view`；
- 已显示的 object 在 verified refresh 后消失：以 replace 导航到之前 occurrence 的最近有效
  父对象，父链均失效时回 `/view`；
- 初始深链接中的 Document coordinate 未被任何 active 对象引用：以 replace 导航回
  `/view`；已显示的 coordinate 在 refresh 后失效时先回原 owner 的最近有效父链；
- Community 切换后使用新 Community 的 Project Profile，不复用旧对象 ID。

`object` 与 `document` 是互斥的 discriminated selection。search validator 遇到二者同时
存在时保留 `object` 并移除 Document 参数；`revision` 只有在 `document` 存在时才保留。
Document 的编辑、History 或列表入口导航到现有
`/documents?document=<id>&revision=<n?>`。

现有
[`view.tsx`](../../../../desktop/src/app/routes/view.tsx)
继续拥有 search 参数验证和导航回调。

### 7.2 Outline occurrence

同一对象或 Document coordinate 可能在 Outline 中出现多次。例如 Issue 同时拥有结构
位置和 `about` 关联位置，同一 live Document 也可被多个对象引用。当前选择因此分成：

```text
Current item identity       object ID 或 Document coordinate，决定 Main 展示什么
Current outline occurrence occurrence key，决定 Outline 高亮哪里
```

建议 occurrence key 使用纯展示坐标：

```text
object:<object-id>:canonical
issue-about:<target-object-id>:<issue-id>
resource-context:<owner-object-id>:<resource-id>
document-context:<owner-object-id>:<document-id>:live
document-context:<owner-object-id>:<document-id>:pinned:<revision>
resource-guide:<resource-id>:<document-id>
```

路由增加可选的 `via` search 参数来保留具体 occurrence：

- Object alias 点击写入 `object=<id>&via=<occurrence-key>`；
- Document 点击写入 `document=<id>&revision=<n?>&via=<occurrence-key>`；
- 已有只有 `object` 的链接继续有效，并选择 canonical occurrence；
- 没有 `via` 的 Document 深链接选择匹配 coordinate 的第一个确定性 occurrence；
- `via` 只影响 Outline 高亮，不参与 Relay 查询、权限或领域对象身份；
- `via` 已失效但 identity 仍有效时，Object 回退 canonical，Document 回退第一个匹配
  occurrence，并用 replace 清理 URL。

Document 仍不是 Project View Object，也绝不能写入 `object`。`document + revision` 表达
Document coordinate；`via` 表达它在 Project Outline 中的引用位置。

`via` 是不可信的展示输入。实现必须在当前 snapshot 生成的 occurrence registry 中精确
解析，并验证 occurrence 指向当前 identity / coordinate；不得用它拼 Relay filter、文件
路径或权限判断。没有 `object` / `document` 时移除孤立的 `via` 和 `revision`。

导航回调必须构造完整 canonical search，不能沿用当前 `...previous` 后只覆盖一个字段：

| 目标 | 写入 search | 必须清除 |
|---|---|---|
| Project Profile | 空 search | object、document、revision、via |
| canonical Object | object | document、revision、via |
| Object alias | object、via | document、revision |
| Document occurrence | document、可选 revision、via | object |

这样切换 Object / Document 时不会产生两个 selection 同时残留的中间 URL。

### 7.3 父级解析与跳转

父级按 Current Outline occurrence 解析，而不是只按 object ID 猜测。目录中的 group 节点
不是视图对象，计算时跳过，取最近的 object ancestor：

| Current occurrence | 父级视图对象 |
|---|---|
| Project Profile | 无 |
| Goal、Role、Resource | Project Profile |
| bound Plan | 所属 Goal |
| unbound Plan | Project Profile |
| Stage | 所属 Plan |
| planned Requirement / Issue | 所属 Stage |
| unplanned Requirement / Issue | Project Profile |
| Work | 它 `handles` 的 Requirement / Issue |
| `about` Issue alias | `about` target |
| Resource Context alias | 引用它的 source object |
| Document Context / Guide occurrence | 引用它的 source object / Resource |

因此，同一个 Issue 从 canonical Stage 位置进入时向上回 Stage；从某个 `about` alias 进入时
向上回该 alias 的 target。没有 `via` 的 Object 深链接使用 canonical parent；没有 `via`
的 Document 深链接先选择确定性 occurrence，再使用该 occurrence 的 owner。

点击父级按钮使用 push 导航到父级的 canonical Object occurrence，清除 document、revision
和 via。浏览器 Back 返回刚才的 Current Item 及其原 occurrence。关系更新或 `via` 失效
后，按钮必须随已验证的新 occurrence 重新计算，不能保留旧父级。

### 7.4 历史记录

- 点击 Outline 节点或 Main 中的摘要使用 push 导航；
- Outline 展开 / 折叠不写浏览器 history；
- Back / Forward 必须恢复 Current Item 和 occurrence 高亮；
- Edit Dialog、Delete Dialog 和 Context 管理 Dialog 不改变 Current Item search；
- 当前对象删除成功后导航到最近的有效结构父对象；没有父对象时回到 Project Profile。

## 8. Project Outline 读取模型

### 8.1 纯 presentation model

新增纯函数模块，例如：

```text
desktop/src/features/project-view/explorerModel.ts
```

它只接受经过验证并已经组装的 `ProjectView`、Role Continuity 的必要引用以及可用的
Document catalog，输出 Outline 和 Current Page presentation model。它不得发起查询、
读取全局状态或改变对象顺序。

建议的节点联合类型：

```ts
type ProjectViewOutlineNode =
  | {
      kind: "object";
      occurrenceKey: string;
      relation: "root" | "structural";
      objectId: string;
      objectType: ProjectViewObjectType;
      title: string;
      children: ProjectViewOutlineNode[];
    }
  | {
      kind: "object_reference";
      occurrenceKey: string;
      relation: "about" | "context";
      ownerObjectId: string;
      objectId: string;
      objectType: "issue" | "resource";
      title: string;
    }
  | {
      kind: "document";
      occurrenceKey: string;
      relation: "context" | "resource_guide";
      ownerObjectId: string;
      documentId: string;
      mode: "live" | "pinned";
      documentRevision?: number;
      title?: string;
    }
  | {
      kind: "group";
      occurrenceKey: string;
      label: string;
      children: ProjectViewOutlineNode[];
    };
```

`object_reference` 和 `document` 在类型上没有 `children`，从模型层保证引用 occurrence
是叶节点。`group` 是不可导航的目录分组，例如 Goals、Roles、Related Issues、Related
Resources、Documents。它只帮助阅读，不是 Project View 对象，也不能进入 URL 的
`object`。

Outline row 只承担目录导航：disclosure、类型图标 / 短标签和 Title。它不显示 Summary、
Status、Priority、Revision、actor、计数或内联操作，避免右侧重新变成第二套内容卡片墙。

### 8.2 根目录

根节点是 Project Profile，并按现有 read model 组织：

```text
Project Profile
├── Goals
│   └── Goal → Plan → Stage → Requirement / Issue → Work
├── Roles
├── Resources
└── Unplaced Objects
    ├── Unbound Plans
    ├── Unplanned Requirements
    └── Unplanned Issues
```

这是一份导航 read model，不是新的所有权树。Project Profile 并不拥有 Goal，Goal 也不拥有
Plan；节点位置只投影 `under_goal_id`、`under_plan_id`、`planned_in_stage_id`、`handles`
等规范关系。

空分组默认不渲染。为了避免大树重新制造平面噪声：

- 初次进入只展开 Project Profile 和当前路径；
- 切换 Current Item 时只保证其祖先展开，不自动展开其全部后代；
- 已由用户展开且在新 snapshot 中仍有效的其他分支保持展开；
- 折叠祖先不会改变 Current Item；若当前高亮被折叠，最近可见祖先显示“包含当前
  选择”的视觉状态，但 Current Item 不跳转；
- 重新展开后恢复精确高亮。

### 8.3 每个对象下的关联节点

每个 canonical object occurrence 可以在结构 children 之后附加：

```text
Related Issues
└── issueReferencesByTarget[object.id]

Related Resources
└── object.contextReferences 中的 Resource

Documents
├── Resource guide（Resource only）
└── object.contextReferences 中的 Document
```

引用 occurrence 必须是叶节点：

- Related Issue 不展开自己的 Work 或其他引用；
- Related Resource 不展开自己的 Guide 或其他 Context；
- Document 和 Guide 本来就是叶节点；
- 只有 canonical object occurrence 才递归组织结构 children 和关联分组。

这条限制既避免复制同一个对象的完整子树，也保证 Issue-to-Issue 相互 `about`、对象之间
交叉引用 Resource 等合法图关系不会在 Outline 中形成递归或无限重复。点击 Issue /
Resource 引用叶节点后，Main 切换到该规范对象；它自己的直属下一层和引用只在 Main 及
canonical occurrence 下读取。

### 8.4 多处出现与高亮

- canonical object occurrence 由结构关系决定；
- `about` Issue 是指向同一 object ID 的 alias；
- Context Resource 是指向根部同一 Resource 的 alias；
- Document 每一个 live / pinned Context coordinate 是独立 occurrence；
- Guide 是独立 relation，即使它与普通 live Context 指向同一 Document；
- 点击 Object alias 时 Main 仍读取同一个 Project View Object；
- 点击 Document occurrence 时 Main 读取该精确 coordinate 的只读预览；
- alias 不拥有独立编辑、Revision 或缓存；
- canonical 和 alias 都可以在目录存在，但同一时刻只高亮路由指定的 occurrence。

不同父节点下不做全局去重，因为每个 occurrence 表达不同引用来源。同一父节点的 Main
摘要采用以下展示去重：

- Issue 同时是直属结构 Issue 和 `about` 引用时，结构项优先；
- Resource 同时是 Project Profile 的根部 Resource 和其 Context 引用时，结构项优先；
- Resource Guide 与同一 Document 的 live Context 重合时，Guide 项优先；
- 同一 Document 的 live、不同 pinned Revision 是不同 coordinate，不得合并。

关系去重只影响摘要呈现，当前对象完整 Relations / Context 中仍保留所有规范关系。

### 8.5 顺序

Outline 不发明新的排序：

- 结构对象沿用 `assembleProjectViewV3` 的确定性顺序；
- Related Issues 沿用 verified read model 中的确定性引用顺序；
- Related Resources 和 Documents 使用 canonical Context Reference 顺序（Resource 在前，
  Document 在后；同类按协议坐标排序），Resource Guide 固定在普通 Document 之前；
- 不按 UI 推断的紧急程度、最近访问或当前 assignee 重排目录。

## 9. Main 单层读取模型

### 9.1 页面组成

Main 对任意 Current Project View Object 只生成五类内容：

```text
Current object full content
Direct structural children summaries
Related Issue summaries
Related Resource summaries
Attached Document summaries
```

Current Item 是 Document 时，Main 只生成一个只读 Document Preview，不再生成上述对象
分组。

建议模型：

```ts
type ProjectViewExplorerPage =
  (
    | {
      kind: "object";
      currentObject: ProjectViewObject;
      structuralGroups: Array<{
        label: string;
        items: ProjectViewSummaryItem[];
      }>;
      relatedIssues: ProjectViewSummaryItem[];
      relatedResources: ProjectViewSummaryItem[];
      documents: ProjectViewDocumentSummaryItem[];
    }
    | {
      kind: "document";
      coordinate: ProjectViewDocumentCoordinate;
      occurrenceKey: string;
      document?: ProjectDocument;
      openInDocumentsSearch: {
        document: string;
        revision?: number;
      };
    }
  ) & {
    parent?: {
      objectId: string;
      title: string;
    };
  };
```

`parent` 由 current occurrence 的最近 object ancestor 生成。它只为右上角按钮提供 identity
和 Title，不携带 Summary 或完整父对象展示模型。

### 9.2 直属结构对象

| Current Object | Main 中的直属结构摘要 |
|---|---|
| Project Profile | Goals、Roles、Resources、Unplaced Objects（Unbound Plans、Unplanned Requirements、Unplanned Issues） |
| Goal | 直属 Plans |
| Plan | 直属 Stages |
| Stage | 直属 Requirements 和规划在此处的 Issues |
| Requirement | Handles 该 Requirement 的 Work |
| Issue | Handles 该 Issue 的 Work |
| Work | 无结构子对象 |
| Role | 无 Project View 结构子对象 |
| Resource | 无 Project View 结构子对象 |
| Document Preview | 无子对象，只显示当前精确 coordinate 的只读内容 |

Role 的 Assignment、Proposal、Brief、Checkpoint 和 Handoff 是当前 Role 的完整连续性
内容，不是 Project View 结构子对象。Role 中引用的 Work 如果作为入口出现，必须使用
相同的轻量 Summary 组件。

### 9.3 Summary item 的严格可见字段

所有非 Current Item 的 Project View 对象摘要只显示：

```text
Type
Title
Summary
```

不得显示：

- Status；
- Priority；
- Description、Purpose 或 Desired Outcome；
- Revision；
- 创建者、修改者或时间；
- Assignment；
- related issue count；
- Context 详情；
- 子对象数量或孙级预览；
- 内联 Edit / Delete / Add 操作。

卡片本身可以具有进入箭头、hover、focus ring 和无障碍名称；这些是交互提示，不是新的
对象内容字段。

建议新增：

```text
ProjectViewSummaryItem.tsx
ProjectViewSummaryGroup.tsx
```

并新增只读 helper：

```ts
projectViewObjectSummary(object): string | undefined
```

该 helper 只能返回 `object.data.summary`，不能复用当前会返回 Description / Purpose 的
`projectViewObjectDescription()`。

Summary 按现有安全 Markdown 规则做非交互渲染；链接、附件和 mention 在摘要卡中不应
产生嵌套交互。`summary` 缺失只显示统一空状态，不截取完整字段。

### 9.4 挂载 Issue

Main 使用 `view.issueReferencesByTarget[currentObject.id]` 找到挂载 Issue，并通过
`objectsById` 解析 Type、Title、Summary。

如果一个 Issue 同时是当前 Stage 的直属结构 Issue，并且 `about` 也指向当前 Stage：

- Main 只渲染一次；
- 结构位置优先，放在 Stage 的 Issues 组；
- Related Issues 组排除已经在 structural groups 中出现的 object ID；
- Outline 仍可分别保留 canonical 和 `about` occurrence。

### 9.5 挂载 Resource

Main 从 `currentObject.contextReferences` 解析 Resource coordinate，并用 `objectsById` 读取
同一规范 Resource 的 Type、Title、Summary：

- Related Resource 只显示三字段摘要；
- 点击后切换到该 Resource 的完整 Current Object 页面，并通过 `via` 高亮引用叶节点；
- Current Project Profile 已在直属 Resources 组出现的同一 Resource 不重复显示；
- Resource 本身不能再产生 Resource Context Reference。

### 9.6 挂载 Document

Main 显示：

- 当前对象 Context References 中的 live Document；
- 当前对象 Context References 中的 pinned Document Revision；
- Current Resource 的 Guide Document。

Document Summary 也严格只显示 Type、Title、Summary。其 Type 可以是 `Document`、
`Pinned Document` 或 `Guide Document`，以便在不增加第四个字段的情况下表达坐标类型。

解析规则：

- live reference 使用同一 Community、同一 projection generation 下的 verified current
  Document catalog；
- pinned reference 在未选择时不得读取 exact Revision，也不能显示 current head 的 Title /
  Summary 冒充历史 Revision；Title 位使用明确 coordinate label，Summary 保持空状态；
- Guide Document 使用当前 Guide head，除非未来领域契约增加 pinned guide；
- 正在读取时使用固定尺寸 skeleton；
- 无权、已删除、暂不可用或完整性失败时显示不可用摘要项，不展示猜测内容；
- 点击后成为 `/view` 的 Current Document Item，并保留 pinned revision 与 occurrence；
- Guide 与同一 live Context coordinate 重合时，Main 只显示语义更强的 Guide 项；
- 同一 Document 的 live、多个 pinned Revision 是不同 coordinate，分别显示。

Outline 和 Summary 列表只读取 live current catalog，不得为了 pinned Title / Summary
预取任何 exact Revision 正文。只有 Document 成为 Current Item 时，才按 Community、
projection generation、Document ID 和可选 Revision 发起 `getProjectDocument`。

### 9.7 Current Document Preview

Document 成为 Current Item 后，Main 显示：

- `Document` / `Pinned Document` / `Guide Document` 类型语义；
- verified Title、Summary、Revision 和只读 Markdown；
- live / pinned 模式和来源对象；
- `Open in Documents` 操作，进入 `/documents?document=<id>&revision=<n?>`；
- loading、unavailable、deleted head 和 integrity error 的明确状态。

它不显示编辑器、Revision History、Document 列表或 Project View Object 操作。Pinned
Revision 即使 current head 已 tombstone 仍可合法预览；live / Guide current head 已删除时
显示不可用，不回退到缓存正文。Document 是 Outline 叶节点，所以 Preview 下没有下一层
摘要。

## 10. Current Object 完整内容

### 10.1 通用内容

当前对象主页面承接原 Inspector 的通用能力：

- Type、Title、Summary；
- 对象类型专用完整字段；
- Status、Priority；
- 直接 Relations；
- Context References 的管理入口；
- `Show in Project Context`；
- Object Revision、Project Revision；
- Created / Updated 时间和 actor；
- Edit、Delete。

页面可以使用分区、Disclosure 或 Dialog 降低密度，但这些内容属于 Current Object，
不受“摘要只显示三个字段”的限制。

“当前对象完整内容”只允许完整展开当前对象自己的字段、关系语义和维护信息，不允许借
Relations / Context 区再次完整展开目标对象。任何可导航的 Project View target 仍复用
Type / Title / Summary 入口；Document target 仍复用三字段摘要。

### 10.2 Role

Current Role 页面继续展示：

- Role Purpose、Responsibilities、Boundaries；
- Role level 和 active 状态；
- Current Assignment；
- Role Brief；
- Role Directory；
- Responsible Work；
- Latest Checkpoint；
- Proposal、Tenure 和 Continuity timeline；
- Request、Offer、Replace、End、Checkpoint、Handoff 等授权操作。

其中任何作为 Project View 对象入口展示的 Work 只使用 Type、Title、Summary。Assignment、
Checkpoint、Handoff 等 continuity record 可以展示自身必要字段。

Assignment、Proposal、Brief、Checkpoint 和 Handoff 不是 Project View Object，不进入
Outline，也不生成伪结构 children。Role Brief 继续作为带 Revision / 来源的派生读取，
不能被复制成新的规范事实。

### 10.3 Work

Current Work 页面继续展示 verified Responsibility 和 Commitment，并保持 Assignment fence、
revision conflict 和治理授权。相关 Role 可以作为关系入口，但不得在 Work 页面嵌套完整
Role 内容。

Work 状态、Assignment 和 Commitment 状态继续各自独立：结束 Assignment 不自动完成或
取消 Work；`Waiting for continuation` 仍是派生状态。Explorer 重排不得改变这些生命周期
语义。

### 10.4 Context 管理

现有 `ProjectViewContextSection` 同时负责展示和修改。为了避免 Related Resources /
Attached Documents 在主页面重复出现，建议拆分为：

```text
ProjectViewRelatedContextItems      Resource / Document 三字段摘要展示
ProjectViewContextManagementDialog 添加、移除和精确坐标管理
```

Current Object 页面提供 `Manage Context` 操作打开 Dialog。Dialog 可以显示 live / pinned
模式、Revision、Resource coordinate 和删除操作，因为它是当前对象的维护面，不是子对象
摘要列表。

Context capability 不可用时仍允许按现有契约清理 preserved coordinate；不得隐藏已经
保存的引用，但禁止新增或 retarget。

## 11. 创建、编辑和删除

### 11.1 创建

保留 `ProjectViewObjectDialog` 的类型化表单和 revision fence。创建入口移动到 Current
Object 页面上下文：

| Current Object | 主要创建入口 |
|---|---|
| Project Profile | Goal、Role、Resource、Unbound Plan、Unplanned Requirement / Issue |
| Goal | Plan |
| Plan | Stage |
| Stage | Requirement、Issue |
| Requirement | Work |
| Issue | Work |
| Work | 无结构 child create |
| Role | Role governance / continuity 操作，不创建结构 child |
| Resource | Context / Guide 操作，不创建结构 child |

此外，每个 Current Project View Object 都提供独立的 `Add related issue` 入口，将新 Issue
的 `about` 预填为当前对象。该入口不自动推断规划关系：即使当前对象是 Stage，也不能把
`about` 悄悄复制为 `planned_in_stage_id`；Stage 的结构 `Add Issue` 与关联 Issue 是两个
明确动作。对应 `ProjectViewCreateContext` 增加可选 `about`，Dialog 仍允许 Human 检查和
修改关系。

全局 Add 可以保留为次要入口，但打开后仍需明确对象类型和关系，不得根据当前视觉位置
隐式生成不合法关系。

创建成功后：

- 默认导航到新对象，使其成为 Current Object；
- Outline 展开新对象的祖先并高亮；
- 浏览器 Back 返回创建前的 Current Object；
- 不在父页面内联展开新对象的后代。

### 11.2 编辑

- Current Object 顶部提供 Edit；
- 继续使用 `expectedProjectRevision` 和 object revision；
- conflict 保留草稿并要求 Human 显式采用新基线；
- refresh 后 Current Object 仍存在时保持选择；
- 对象关系移动后，Outline 根据新 snapshot 重建并把当前对象滚动到新的 canonical 位置。

### 11.3 删除

- 继续执行结构 / `about` / `handles` incoming reference、Resource Context incoming、最后一个
  Goal 和 Role lifecycle 保护；
- Delete Dialog 必须显示真实阻塞关系；
- 删除成功后导航到删除前记录的规范结构父对象，而不是 `via` 引用来源；没有规范父对象
  时回到 Project Profile；
- 被删除对象仅有 `about` alias 时也不能让 alias 残留；
- 对象自身 outgoing Context Reference 随 source tombstone 清除，不删除被引用的 Resource /
  Document，也不构成 incoming blocker；
- 删除 Work 时若存在 active Commitment，确认信息必须说明它会以 `work_closed` 结束并
  保留历史；
- Document 删除不属于 `/view` 的 Project View Delete Dialog。

## 12. 组件与文件设计

### 12.1 保留并收敛的文件

- `ProjectViewScreen.tsx`
  - 保留 query、live sync、状态和 Dialog 协调；
  - Ready 状态改为 Explorer 布局；
  - 不再直接组织完整地图和 Inspector。
- `desktop/src/app/routes/view.tsx`
  - 保留现有 object 深链；
  - 增加互斥的 document / revision 与可选 via search；
  - 负责 push、replace 和失效 selection 清理。
- `ProjectViewObjectDialog.tsx`
  - 保留 create / edit 和 conflict 草稿。
- `ProjectViewDeleteDialog.tsx`
  - 保留删除保护。
- `ProjectRoleInspector.tsx`
  - 作为不依赖 Inspector 容器的 Current Role continuity section 直接复用；
  - 文件名暂时保留以减少无关重命名，职责不再依赖右侧 Inspector；
  - 保留现有 mutations 和 history paging。
- `ProjectWorkContinuity.tsx`
  - 作为 Current Work 的内容区保留。
- `ProjectViewActor.tsx`
  - 继续用于 Current Object provenance。
- `ProjectViewStates.tsx`、`ProjectViewV3SetupGuide.tsx`
  - 保持 loading / unsupported / forbidden / uninitialized / integrity 状态。

### 12.2 新增文件建议

```text
desktop/src/features/project-view/
├── explorerModel.ts
├── explorerModel.test.mjs
├── outlineState.ts
├── outlineState.test.mjs
├── projectViewCreateActions.ts
├── projectViewRoleLifecycle.ts
└── ui/
    ├── ProjectViewExplorer.tsx
    ├── ProjectViewOutline.tsx
    ├── ProjectViewOutlineNode.tsx
    ├── ProjectViewCurrentObject.tsx
    ├── ProjectViewCurrentDocument.tsx
    ├── ProjectViewCreateMenu.tsx
    ├── ProjectViewParentNavigation.tsx
    ├── ProjectViewObjectDetails.tsx
    ├── ProjectViewObjectMaintenance.tsx
    ├── ProjectViewSummaryGroup.tsx
    ├── ProjectViewSummaryItem.tsx
    ├── ProjectViewRelatedContextItems.tsx
    └── ProjectViewContextManagementDialog.tsx
```

命名可以在实现时按现有文件大小门禁调整，但纯 model、Outline、Current Object 和
Summary item 的职责不得重新混回一个巨型组件。

### 12.3 删除或退役

- `ProjectViewMap.tsx`：退役，不再渲染全量嵌套地图；
- `ProjectViewInspector.tsx`：删除容器，通用详情提取到 Current Object；
- `ProjectViewObjectCard.tsx`：不再用于 Full View 的非当前对象；如无其他调用则删除；
- `keyboardNavigation.ts`：退役旧地图卡片循环导航，逻辑由 Outline 可见节点导航取代；
- `ProjectRoleCard.tsx`：Community Overview 继续使用，不因 Full View 重做而删除。

### 12.4 API 与 query 层

Project View 读取协议无需修改。Document 摘要解析复用：

- `useProjectDocumentMeta`；
- `useProjectDocuments`；
- `useProjectDocumentLiveSync`，统一更新 current catalog 和已选 live Preview；
- 只有 Current Document Item 才启用的 `useProjectDocument` exact Revision query；
- `/documents` 已有的 document / revision search contract，以及 `/view` 新增的 discriminated
  selection search。

Document hydration 是可降级的第二条 verified read：Outline 的 Resource / Document
coordinate 先来自 Project View snapshot，Document API 不可用时仍保留 occurrence 并显示
`Metadata unavailable`。不得因无法取得 metadata 隐藏已经保存的引用，也不得让每个 tree
node 各自发查询。

Document query key 必须继续包含 Community、projection generation、Document ID 和可选
Revision，
避免跨 Community 或跨 generation 复用不兼容结果。

不得新增 module-level Community 数据缓存。若确实新增 singleton 或 `Map`，必须提供 reset
并接入 `resetCommunityState()`；优先使用 React Query、`useMemo` 和组件生命周期避免该
需求。

## 13. 导航交互与可访问性

### 13.1 ARIA tree

Outline 使用标准 tree pattern：

- 容器 `role="tree"`；
- 节点 `role="treeitem"`；
- 分组 `role="group"`；
- category label 自身作为只控制 disclosure 的 `treeitem`，其 children 包在 `role="group"`
  中，不能把裸 `group` 直接放到 tree 下；
- 展开节点设置 `aria-expanded`；
- 当前 occurrence 使用 `aria-current="page"` 或等价清晰状态；
- 使用 roving `tabIndex` 保持单一 tree Tab stop；
- 可见选中和 focus 不能只依赖颜色。

### 13.2 键盘

- Arrow Up / Down：移动到前后可见节点；
- Arrow Right：展开，已展开时进入第一个 child；
- Arrow Left：折叠，已折叠时移动到 parent；
- Home / End：移动到首尾可见节点；
- Enter / Space：Object / Document 节点导航，category 节点只展开或折叠；
- 所有边界不循环；
- Escape：窄窗口关闭 Outline；宽窗口不改变 Current Item。

Main 中 Summary item 保持正常 Tab 顺序和 Enter / Space 激活。从 Main 摘要导航时，新
Current Item 标题获得程序化焦点；宽窗口从 Outline 激活时保留 tree focus，并用 live
announcement 告知 Main 已更新；窄窗口选择后关闭 Sheet 并聚焦新标题。Back 后恢复合理的
导航触发点。Outline 整体折叠后焦点返回 toggle。

### 13.3 父级跳转按钮

- 使用语义化 Button / Link，视觉内容只有向上箭头和父级 Title；
- accessible name 使用 `Go to parent: <full title>`；箭头图标设为 `aria-hidden`；
- 长标题单行截断，完整 Title 保留在 accessible name，并可提供非必要 tooltip；
- 不添加父级 Type、Summary、层级路径或 `aria-current`；
- 点击后按 Main 导航处理：新父级标题获得焦点，Back 可返回子级；
- Project Profile 没有父级时完全不渲染该按钮，而不是渲染 disabled control；
- 窄窗口中按钮仍位于 Current Item header 右侧，并设置合理最大宽度，不能造成页面横向
  溢出。

### 13.4 Outline 折叠按钮

- 必须有可读的 `Show Project Outline` / `Hide Project Outline` 名称；
- 折叠只是 presentation state，不改变路由和 Current Item；
- 首版不要求持久化；宽窗口默认打开，窄窗口默认关闭；
- 如果未来持久化，只能使用明确 Community-scoped 或纯全局显示偏好，不得保存对象数据。

## 14. Live、刷新和异常状态

### 14.1 Verified refresh

新 snapshot 到达时：

1. 完整验证并组装 Project View；
2. 重建 Outline 和 Current Page model；
3. Current Object 仍存在，或 Current Document coordinate 仍有合法引用时保持；
4. occurrence 仍存在则保持；
5. alias 消失但 object 仍存在时退回 canonical occurrence；
6. Document occurrence 消失但相同 coordinate 仍在其他位置时退回确定性的第一个 occurrence；
7. object 消失时导航到有效父对象或 Project Profile；Document coordinate 已无引用时导航
   到原 owner object，owner 也失效时回到 Project Profile。

不得把新 projection event 直接 patch 进 tree。

### 14.2 Refreshing 与 Offline / stale

- 刷新时保留上一份 verified Current Item 和 Outline；
- Offline 时保留 verified 内容并显示可能陈旧；
- stale 状态不禁用纯导航；
- mutation 是否可用继续服从连接、授权和 native boundary，不制造成功状态；
- Document 摘要失败不能污染 Project View snapshot，但对应项必须明确 unavailable。

### 14.3 Integrity failure

首份 snapshot 完整性失败时，不渲染部分 Outline、Current Item 或 Summary items。
已经有 verified snapshot、刷新失败时，保留旧内容并显示 stale / refresh failure。

Document catalog 或 exact Revision 完整性失败属于跨能力降级：保留已验证的 Project View
Outline 和 coordinate，只让对应 Summary / Preview 显示验证失败。只有 Project View
snapshot 自身的结构或 Resource target 完整性失败才 fail closed 整个 Explorer。

### 14.4 Community 切换

Community 切换必须清理：

- Current Item route selection；
- Outline occurrence；
- expanded node set；
- 当前对象相关 Dialog；
- Current Document exact Revision 查询观察者；
- live subscription。

任何旧 Community 的名称、Summary、Issue alias 或 Document label 都不能短暂绘制到新
Community。

## 15. 性能边界

- Outline model 使用 verified snapshot 的稳定 Map，避免在每个 node 内重复扫描所有对象；
- 预先建立 `objectsById`、structural children、issuesByAboutTarget、resourcesByOwner、
  documentsByOwner；
- 只渲染展开分支的 tree DOM；折叠分支不递归挂载其后代；
- Object Main 只渲染 Current Object 和一层 Summary，Document Main 只渲染一个 Preview，
  本身不需要虚拟化；
- 单层对象数量真实达到需要虚拟化的规模后，再对 Summary group 引入现有列表方案；
- 不因 Outline 初次挂载读取所有 Document 正文或 pinned Revisions；
- `React.memo` 只用于 props 确实 reference-stable 的节点，不把 memo 当作结构优化替代品。

## 16. 测试设计

### 16.1 纯 model 单元测试

为 `explorerModel.ts` 覆盖：

- `/view` 默认选择唯一 Project Profile；
- Goal 只返回 Plans；
- Plan 只返回 Stages；
- Stage 只返回 Requirements 和 Issues；
- Requirement / Issue 只返回 Works；
- Work / Role / Resource 不返回伪造结构 children；
- unbound / unplanned 对象只出现在 Project 根组和自己的 canonical occurrence；
- `about` Issue 在目标下生成 alias；
- Resource Context 在来源对象下生成 alias；
- 所有 Issue / Resource 引用 occurrence 都是叶节点，互相引用不产生递归树；
- 同一 Stage 中 structural + about 的 Issue 在 Main 去重；
- Outline 保留两个 occurrence；
- live / pinned / guide Document occurrence key 确定且不冲突；
- live 与不同 pinned Revision 不错误去重；Guide 与相同 live Context 在 Main 以 Guide 优先；
- Summary helper 只读取显式 `summary`；
- 缺少 Summary 时不回退到 Description；
- pinned 摘要不借用 current metadata，也不触发 exact Revision body read；
- Document Current Item 才解析 exact coordinate，并生成 `/documents` 操作；
- canonical Goal / Plan / Stage / Requirement / Issue / Work / Role / Resource 的父级解析；
- unbound / unplanned 对象跳过 group 后以 Project Profile 为父级；
- `about`、Resource Context、Document Context 和 Guide occurrence 使用各自 owner 为父级；
- Project Profile 没有 parent，parent presentation model 不含 Summary；
- 排序对输入顺序和 canonical snapshot 保持确定性；
- object 消失后的 fallback parent 计算。

为 `outlineState.ts` 覆盖：

- 自动展开当前 occurrence 的祖先；
- 切换 selection 时保留仍有效的用户展开分支；
- 折叠祖先不改变 Current Item；
- 当前 occurrence 被祖先折叠后计算最近可见的 contains-current 节点；
- alias 失效后回退 canonical；
- Back / Forward 恢复 occurrence；
- Home / End / Arrow 键只遍历可见节点。

### 16.2 组件与 E2E

更新
[`desktop/tests/e2e/project-view.spec.ts`](../../../../desktop/tests/e2e/project-view.spec.ts)，
至少验证：

1. `/view` 首屏完整展示 Project Profile，但 Goal 只显示 Type、Title、Summary。
2. Plan 页面显示完整 Plan 和 Stage 摘要，不渲染 Stage 内的 Requirement / Issue / Work。
3. 点击 Stage 后 Main 整体替换，Outline 高亮 Stage。
4. 点击 Main Summary、Outline canonical node 和 `about` alias 都能导航。
5. 当前对象下的 Related Issue、Resource 和 Document 可见且只有 Type、Title、Summary。
6. 无 Summary 时显示明确空态，不出现 Description 文本。
7. pinned Document 未选择时只显示安全 coordinate，不借用 current metadata，也不发起 body
   read；选择后 `/view` 才读取指定 Revision 并显示只读预览，`Open in Documents` 保留
   revision。
8. Outline 折叠后 Current Item 不变，重新打开恢复选中位置。
9. Back / Forward 恢复 Object / Document Current Item 和 occurrence。
10. 删除 Current Object 后回到有效父对象。
11. 实时刷新后对象移动，Outline 在新位置继续高亮。
12. Offline 保留 verified Current Item 和 Outline。
13. Integrity failure 不渲染部分树。
14. Community A 的展开、Issue、Document 不泄漏到 Community B。
15. Role 和 Work 的 continuity 操作迁移后保持现有授权与 revision fence。
16. 窄窗口 Outline 是焦点受控的 overlay，选择后关闭并聚焦新标题。
17. 现有 `/view?object=<id>` 深链接和 Community Overview 对象入口继续工作；新增
    document / revision / via search 经过严格验证。
18. Summary 卡不显示 Status、Priority、Revision、actor、Assignment 或子对象数量；Role
    只有成为 Current Object 后才显示 Assignment / Brief / Checkpoint / Handoff。
19. 右侧 Outline 是唯一位置表达，页面不存在额外 breadcrumb 或 location panel。
20. Outline 引用叶节点在循环 `about` / Context fixture 下不会递归爆炸。
21. Plan 等 canonical 对象右上角只显示向上箭头和正确父级 Title；Project Profile 不显示。
22. 从 `about` Issue alias 进入时父级是 alias target，而不是 Issue 的 canonical Stage；点击
    后进入父级 canonical occurrence，Back 恢复原 alias。
23. Document Context / Guide Preview 的父级按钮回到 owner object。
24. 父级按钮不渲染 Summary、Type 或父级卡片，长标题与窄窗口不产生横向溢出。

现有 E2E 中以 Inspector 为入口的 Role / Work Continuity、Context、Resource Guide、CRUD、
conflict、live refresh 和删除保护场景不得直接删除；统一改为从 Current Object 主页面进入。
旧地图循环键盘测试改为 ARIA tree 的可见节点、非循环导航测试，旧窄窗口 Inspector drawer
测试改为 Outline Sheet 测试。

### 16.3 视觉证据

实现完成后至少捕获：

- Project Profile 根页面；
- Plan 页面，只显示 Stage 摘要，并显示右上角父级跳转；
- Stage 页面，显示 Requirement / Issue 摘要；
- 带 Related Issues、Resources 和 Documents 的对象页面；
- pinned Document 的只读 Preview；
- Outline 折叠状态；
- 窄窗口 Outline overlay。

截图必须使用 mock Tauri bridge，并在截图前调用共享 `waitForAnimations(page)`。多个状态
需要比较 hash，避免重复无效证据。

### 16.4 建议验证命令

```bash
. ./bin/activate-hermit
just desktop-check
just desktop-test
just desktop-e2e-smoke
```

若修改 Tauri Document 读取边界或新增 command，再补充：

```bash
just desktop-tauri-check
just desktop-tauri-test
```

本设计本身不要求修改 Rust 或 Relay，因此正常实现应优先保持在 Desktop React / query
presentation 层。

## 17. 实施顺序

### Slice 1：纯 Explorer model

- 增加 Current Item、direct children、Issue / Resource alias、Document occurrence 的纯
  model；
- 增加 occurrence-based parent object 解析；
- 增加 Summary-only helper；
- 完成模型单元测试；
- 不改变现有 UI。

### Slice 2：Main 单层页面

- 提取原 Inspector 的 Object Details；
- 实现 Current Object 页面；
- 实现 Current Document 只读 Preview，并保证只在选中时读取 exact Revision；
- 实现共用的右上角 Parent Navigation；
- 实现严格三字段 Summary item；
- 接入 structural children、Related Issues、Related Resources 和 Documents；
- 保留原 Inspector 作为短期对照，但不同时交付给用户。

### Slice 3：Project Outline 与路由

- 实现右侧 tree、occurrence key、展开状态和高亮；
- 扩展互斥的 object / document + revision selection 和可选 `via` route search；
- 完成 Summary / Outline / Parent 双向导航和 Back / Forward；
- 完成宽 / 窄响应式。

### Slice 4：维护能力迁移

- 把 Edit、Delete、Context、Role Continuity、Work Continuity 移入 Current Object；
- 拆分 Context 展示和管理；
- 保持 conflict 草稿、治理和删除保护。

### Slice 5：删除旧投影并收口

- 删除 `ProjectViewMap` 和 `ProjectViewInspector` 容器；
- 移除 Full View 重复的 Current Focus 和 Supporting Objects 平面；
- 更新 E2E、空状态、键盘和截图证据；
- 运行 Desktop scoped gates。

每个 Slice 都必须保持 `/view` 可用；不能在维护能力尚未迁移时先删除 Inspector。

## 18. 验收标准

实现只有同时满足以下条件才算完成：

1. Main 任意时刻只有一个 Current Item：一个完整 Project View 对象，或一个 Document
   coordinate 的只读 Preview。
2. Main 中所有其他 Project View 对象只显示 Type、Title、Summary。
3. Object 页面中的所有挂载 Resource / Document 只显示 Type、Title、Summary。
4. Main 不渲染 Current Object 的孙级对象。
5. Plan 页面看不到 Stage 内的 Requirement / Issue / Work 内容。
6. 挂在任意合法对象上的 `about` Issue、Context Resource / Document 和 Resource Guide 都
   可以从 Main 和 Outline 进入。
7. Related Issue / Resource / Guide 与同一父页面的直属或 Context 项按明确优先级去重；
   live 与不同 pinned Revision 不合并。
8. 右侧 Outline 是唯一位置指示；没有额外 breadcrumb 或位置栏。
9. 导航后 Current occurrence 有非纯颜色的明确高亮且祖先自动展开；用户主动折叠其祖先
   时，最近可见祖先显示非纯颜色的“包含当前项”状态。
10. Outline 可以折叠，折叠不改变 Current Item。
11. 原 Object Inspector 已删除，其查看和维护能力没有丢失。
12. `/view`、`/view?object=<id>`、Document / revision / via search、Back / Forward 和
    Community Overview 深入入口继续工作。
13. Human 与 Agent 仍读取和修改同一 verified Project View。
14. Refreshing / offline 保留最后 verified 内容；integrity failure 不展示部分树。
15. Community 切换不会泄漏对象、目录或 Document 状态。
16. Role / Work continuity、revision conflict、删除保护和 Context capability 行为保持正确。
17. Desktop 键盘、窄窗口和命名字体门禁通过。
18. 所有引用 occurrence 都是叶节点，循环 `about` / Context 不会递归复制子树。
19. Pinned Document 在成为 Current Item 前不借用 current metadata，也不预读 exact Revision
    正文；成为 Current Item 后才显示 verified 历史预览。
20. 除 Project Profile 外，Current Item header 右上角提供向上箭头和 occurrence-based 父级
    Title；不显示父级 Summary、Type 或完整卡片。
21. canonical、`about`、Resource Context、Document Context 和 Guide occurrence 均解析到
    正确父级；group 节点不被当作父级对象。
22. 父级跳转使用 push，Back 恢复原 Current Item 和 occurrence；该按钮不构成 breadcrumb
    或第二套当前位置组件。

## 19. 最终实现模型

本设计最终形成三个清晰职责：

```text
Community Overview
└── 回答整个 Project 现在是什么情况

Full Project View Main
├── Object Current Item：完整阅读和维护一个 Project View Object
│   ├── Header 右上角：↑ Parent title（存在父级时）
│   ├── 直属结构对象：Type / Title / Summary
│   ├── Related Issues：Type / Title / Summary
│   ├── Related Resources：Type / Title / Summary
│   └── Documents：Type / Title / Summary
└── Document Current Item：精确 coordinate 的只读预览
    ├── Header 右上角：↑ Owner title
    └── Open in Documents

Project Outline
└── 展示全局结构、关联 occurrence 和唯一当前位置
```

它保留 Project View 的完整结构，但不再把完整结构等同于一次性展开全部内容。全局结构
属于右侧导航；完整视觉重量只属于 Current Item；其余对象始终是轻量、明确、可进入的
摘要。
