# Desktop Project Context 来源 Summary 展示实现设计

> 状态：已实现
>
> 日期：2026-08-09
>
> 代码基线：`feat/project-view-summary` @ `fb43b80a3`
>
> 范围：Project Context 右侧 Coordinate 内容页的来源 summary 展示，以及 Edge Inspector
> 中 Context Document summary 的点击展开与 Documents 跳转
>
> 明确排除：summary 生成或修改、summary 数据模型、Project Context 协议、Coordinate / Edge
> 结构、图节点与 Picker、来源对象写入协议、Edge 中 Document 正文读取
>
> 关联文档：
> [Project View 来源对象摘要实现设计](project-view-summary-implementation-design.md)、
> [Meeting 来源摘要实现设计](meeting-summary-implementation-design.md)、
> [Project Context V2 领域规范](../project-context.md)

## 1. 目标

本次只实现两项 UI 行为：

1. 用户点击 Project Context 图中的 Coordinate 后，右侧内容页展示该 Coordinate 所指来源对象
   自己的 `summary`；
2. 用户打开一条 Edge 后，Context Documents 以列表呈现；点击一份 Document 展开它自己的
   `summary`，并提供 `Open in Documents`。

两处 summary 都使用现有 `Markdown` 组件渲染。

本次不创建“Coordinate summary”或“Edge summary”。summary 仍然只属于来源对象：

```text
Project View Coordinate  -> Project View object.summary
Document Coordinate      -> Project Document.summary
Meeting Coordinate       -> Meeting.summary
Edge Context Document    -> Project Document.summary
```

## 2. 术语与数据边界

### 2.1 Coordinate 不拥有 summary

`ProjectContextCoordinate` 只是来源对象的稳定坐标。它不保存 summary，也不拥有 summary 生命周期。

Project Context 查询结果中现有的：

```ts
ProjectContextCoordinateDetail.summary
```

是图查询为了节点预览而水合的来源 summary 观察值。它不是第二份 canonical summary，但也不是本次
右侧内容页的读取来源。

右侧内容页本来就会读取 Coordinate 所指的当前来源内容，因此应直接从这份来源内容读取 summary，
不再从 `ProjectContextCoordinateDetail.summary` fallback。

### 2.2 各类型的唯一展示来源

| Coordinate 类型 | 右侧内容页已加载的数据 | Summary 展示来源 |
|---|---|---|
| Project Profile / Goal / Role / Plan / Stage / Requirement / Issue / Work / Resource | `ProjectViewObject` | `object.data.summary` |
| Document | verified current `ProjectDocument` | `currentDocument.summary` |
| Meeting | verified current Meeting detail | `verifiedMeeting.summary` |

不存在两个 summary 之间的合并、比较或优先级。

如果当前来源内容尚未加载、加载失败、已 tombstone，或者自身 `summary = None`，右侧内容页不展示
Summary block，也不使用图查询中的 preview、description、正文截断或其他字段替代。

### 2.3 Edge Context Document

Edge Inspector 不加载完整 Document。它直接使用 Edge 查询已经返回的：

```ts
ProjectContextDocumentDetail.summary
```

这个字段来自对应 Project Document 的 metadata summary，只用于显示该 Document 自己的摘要。
它不是 Edge 的摘要，也不代表 Edge 上其他 Context Documents。

## 3. 当前实现需要调整的地方

### 3.1 Project View

`ProjectViewContent` 已经从完整 `ProjectViewObject` 读取 `object.data.summary`，但当前以普通文本展示。

本次只需：

- 改用 `Markdown` 渲染；
- 保持 summary 只出现一次；
- Resource 的 header subtitle 不再用 summary 作为 description，避免和 Summary block 重复。

### 3.2 Document

`ProjectContextDocumentContent` 已经读取 verified current Document，但当前使用：

```ts
currentDocument?.summary ?? detail.summary
```

这里的 `detail.summary` 是 Context 查询观察值。二选一只是旧的 fallback 行为，不是两类 summary。

本次改为：

```ts
const summary = currentDocument?.summary;
```

只有 verified current Document 加载成功后才展示其 summary。Document 正文的现有读取与展示保持
不变。

### 3.3 Meeting

`ProjectContextMeetingContent` 当前使用：

```ts
verified?.summary ?? summary
```

其中 `summary` prop 来自 `ProjectContextCoordinateDetail.summary`。这同样是旧的 fallback。

本次改为只读取：

```ts
verified?.summary
```

同时删除 `summary` prop。Meeting 当前详情加载失败时，不展示 preview summary 代替它。

### 3.4 Edge Context Documents

当前 Edge Inspector 会自动选择第一份 Context Document，并挂载
`ProjectContextDocumentContent`，从而读取并展示完整 Document body。

本次将它改为简单的 Document disclosure 列表：

- 初始时所有 Document 都折叠；
- 点击某一项后展开该项已有的 metadata summary；
- 不请求 Document body；
- 通过 `Open in Documents` 进入完整 Document 页面。

## 4. Markdown 展示规则

所有 Summary block 复用现有组件：

```tsx
import { Markdown } from "@/shared/ui/markdown";

{summary ? (
  <section data-testid="project-context-source-summary">
    <h3>Summary</h3>
    <Markdown
      className="mt-2 text-sm leading-6"
      content={summary}
      interactive={false}
    />
  </section>
) : null}
```

规则固定为：

- Markdown 完整展示，不做 line clamp；
- `summary = None` 时不渲染空 block；
- 不把 summary 预转换或另存为 HTML；
- 不从其他字段生成 fallback；
- `interactive={false}`，Document 导航使用独立按钮。

各来源内容组件可以复用相同视觉结构，但不增加新的 summary domain model 或 React state。

## 5. Coordinate 内容页实现

### 5.1 Project View Coordinate

修改 `ProjectContextCoordinateInspector.tsx` 中的 `ProjectViewContent`：

```text
Project View badges / title
Source-owned summary (Markdown, if present)
Open in Project View
Canonical object content
Relations / revisions
```

Summary 直接使用 `object.data.summary`。

所有九类 Project View 对象已经通过同一个 `ProjectViewObject` union 提供 summary，不需要按对象类型
建立额外字段或读取路径。

### 5.2 Document Coordinate

修改 `ProjectContextDocumentContent.tsx`：

```text
Document badges / title
Verified current Document summary (Markdown, if present)
Open in Documents
Metadata
Verified current body
```

Summary 只使用 `currentDocument.summary`。现有 `useProjectDocument()`、verified body、revision 与
`Open in Documents` 保持不变。

### 5.3 Meeting Coordinate

修改 `ProjectContextMeetingContent.tsx`：

```text
Meeting badges / title / discussion goal
Verified current Meeting summary (Markdown, if present)
Open Meeting
Lifecycle / participants / action state
```

Summary 只使用 `verified.summary`。

`ProjectContextCoordinateInspector.tsx` 不再向该组件传入 `detail.summary`。

## 6. Edge Context Documents 实现

### 6.1 展示结构

每份 Context Document 使用独立的 `<details>/<summary>`：

```text
Context Documents · N

▸ Document A
  document-id

▾ Document B
  document-id
  Summary
  └── Markdown(document.summary)
  [Open in Documents]
```

折叠行显示：

- Document title；
- active / tombstoned / unavailable 状态；
- Document ID。

展开区显示：

- `document.summary` 的 Markdown block（存在时）；
- active Document 的 `Open in Documents` 按钮。

无 summary 时不显示 Summary block。active Document 仍可通过按钮打开。

### 6.2 删除正文读取链路

`ProjectContextEdgeInspector.tsx` 删除：

- `selectedDocumentId` 与默认选择逻辑；
- `firstReadableProjectContextDocumentId()`；
- `projectContextDocumentIdentity()`；
- Edge 内的 `ProjectContextDocumentContent`；
- 因 Document body 读取产生的 loading / error / unavailable body panel。

点击 disclosure 只展开当前 `ProjectContextDocumentDetail.summary`，不发起 Tauri 或 Relay 请求。

`Open in Documents` 继续调用现有 `onOpenDocument(documentId)`。tombstoned / unavailable Document
不显示该按钮。

## 7. 文件修改清单

| 文件 | 修改 |
|---|---|
| `../../../../desktop/src/features/project-context/ui/ProjectContextCoordinateInspector.tsx` | Project View summary 改为 Markdown；Resource subtitle 不重复 summary；停止向 Meeting 传 `detail.summary` |
| `../../../../desktop/src/features/project-context/ui/ProjectContextDocumentContent.tsx` | 只读取 `currentDocument.summary`，移除 `detail.summary` fallback，改为 Markdown |
| `../../../../desktop/src/features/project-context/ui/ProjectContextMeetingContent.tsx` | 只读取 `verified.summary`，删除 summary prop 与 fallback，改为 Markdown |
| `../../../../desktop/src/features/project-context/ui/ProjectContextEdgeInspector.tsx` | Context Documents 改为点击展开 metadata summary；保留 Documents 跳转；删除 body 读取链路 |
| `../../../../desktop/tests/e2e/project-context.spec.ts` | 更新三个 Coordinate family 的 Markdown summary 与 Edge disclosure 验收 |

本次不修改：

- `ProjectContextCoordinateDetail` 或 `ProjectContextDocumentDetail`；
- Desktop Native Project Context hydration；
- Relay、DB、SDK、Tauri command；
- Project Context Coordinate / Edge / Binding；
- Project View、Document、Meeting summary 写入；
- 图节点与 Picker 中已有的 preview summary。

## 8. 测试设计

### 8.1 Coordinate 内容页

覆盖：

1. Project View Coordinate 从 `object.data.summary` 渲染 Markdown；
2. Document Coordinate 从 verified current Document 渲染 Markdown；
3. Meeting Coordinate 从 verified current Meeting detail 渲染 Markdown；
4. 三类页面都不重复显示 summary；
5. `summary = None` 时不显示 Summary block；
6. 来源读取失败时不使用 `detail.summary` fallback；
7. 标题、列表、强调、inline code 按 Markdown DOM 渲染；
8. Coordinate 原有正文、生命周期、revision 和导航保持不变。

### 8.2 Edge Context Documents

覆盖：

1. 打开 Edge 后没有 Document 自动展开，也没有 body read；
2. 点击一份 Document 后展开该项的 Markdown summary；
3. 多份 Document 分别展示自己的 summary；
4. 无 summary 时不显示 Summary block；
5. active Document 显示 `Open in Documents` 并能正确跳转；
6. tombstoned / unavailable Document 不显示跳转按钮；
7. 展开与折叠不会触发 `get_project_document`；
8. Edge Coordinate 集合、Document 数量和 Edge key diagnostic 保持不变。

删除或改写当前“Edge 自动选中第一份 Context Document 并加载完整 body”的旧断言。

## 9. 实施顺序

1. Project View、Document、Meeting 三个内容组件改为直接 Markdown 渲染各自已加载来源的 summary；
2. 删除 Document 与 Meeting 对 `detail.summary` 的 fallback；
3. Edge Context Documents 改为 disclosure summary 列表；
4. 删除 Edge 内 Document body 读取链路；
5. 更新 Project Context Desktop E2E 并运行 Desktop lint、typecheck 与相关测试。

## 10. 验收标准

实现完成必须满足：

1. 每类 Coordinate 内容页都直接读取已加载来源对象自己的 summary；
2. Summary 使用 Markdown 渲染；
3. 右侧内容页不使用 `ProjectContextCoordinateDetail.summary` fallback；
4. summary 只展示一次，缺失时不生成替代内容；
5. Edge 本身没有 summary；
6. Edge Context Document 点击后只展开该 Document 的 metadata summary；
7. Edge Inspector 不读取或展示完整 Document body；
8. active Context Document 可通过 `Open in Documents` 打开；
9. 没有新增 DTO、协议字段、Graph 状态或第二份 summary 模型。
