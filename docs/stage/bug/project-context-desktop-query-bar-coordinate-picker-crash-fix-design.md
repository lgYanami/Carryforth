# Project Context Desktop Query Bar 坐标选择崩溃修复设计

> 状态：修复已实现，自动化回归通过，等待 Human 在真实 Desktop 中验收
>
> 记录日期：2026-08-07
>
> 范围：Project Context Desktop Query Bar、Coordinate Picker、查询草稿状态、页面级错误隔离与回归测试
>
> 关联设计：
> [Project Context Desktop 产品规格](../project-context/desktop-spec.md)、
> [Project Context Desktop 分阶段实现计划](../project-context/desktop-implementation-plan.md)、
> [Project Context Desktop 阶段七验收证据](../project-context/desktop-stage7-acceptance.md)

## 1. 结论

Project Context 页面进入 `Incident` 查询后，Human 选择第一个 Coordinate，已经打开的
Coordinate Picker 没有关闭。虽然父组件随后禁用了 Picker 触发按钮，但 Popover 内容仍然可操作；
Human 再点击第二个 Coordinate 时，前端查询草稿 helper 从 React state updater 中直接抛出：

```text
Incident accepts exactly one Coordinate.
```

异常没有被 Query Bar 局部处理，最终冒泡到 TanStack Router 默认错误边界，整个内容区域被
`Something went wrong!` 错误页替代。

这不是 Relay、Tauri、Project Context 查询协议或数据库故障，也不是 Desktop 原生进程退出。错误发生在
前端草稿状态更新阶段，尚未调用 Tauri/Relay，因此没有创建、修改或删除任何 Project Context Edge，
也没有影响 Project View、Project Document 或其他 Community 数据。

本次修复应同时解决两个问题：

1. 修正 Coordinate Picker 的打开/禁用生命周期；
2. 禁止可恢复的用户交互从 React state updater 抛出异常。

只关闭 Popover 不能覆盖快速双击、重复 Enter 和 stale render；只把异常吞掉又会保留错误交互。两层修复
必须一起交付。

## 2. 故障记录

### 2.1 用户可见现象

1. 进入 Project Context 页面；
2. 选择 `Incident`；
3. 打开 `Add Coordinate`；
4. 选择 Coordinate A；
5. Picker 没有关闭，列表中的 Coordinate B 仍可点击；
6. 点击 Coordinate B 后，页面显示：

```text
Something went wrong!
Incident accepts exactly one Coordinate.
```

从界面上看类似 Desktop 崩溃，实际是当前 React route 被全局错误页替换。Desktop 原生进程和数据层没有
因该异常崩溃。

### 2.2 确定的调用链

```text
CoordinatePicker.selectOption(B)
  -> ProjectContextQueryBar.onSelect(B)
  -> setDraft(current => addProjectContextDraftCoordinate(current, B))
  -> current.mode == incident && current.coordinates.length == 1
  -> throw Error("Incident accepts exactly one Coordinate.")
  -> Router CatchBoundary
  -> 默认全页错误 UI
```

关键位置：

- `../../../desktop/src/features/project-context/queryModel.ts`
  - `addProjectContextDraftCoordinate()` 把 All、重复 Coordinate 和 Incident 超量输入作为异常抛出；
- `../../../desktop/src/features/project-context/ui/ProjectContextQueryBar.tsx`
  - `CoordinatePicker.selectOption()` 选中后只清空搜索和高亮，没有关闭受控 Popover；
  - Incident 首次选择后只禁用 trigger，没有同步收起已经打开的 Popover；
  - `setDraft()` updater 直接调用会抛异常的 helper；
- `../../../desktop/src/app/routes/project-context.tsx`
  - route 没有 Project Context 专用的可恢复错误 UI，异常最终由默认 Router 边界展示。

### 2.3 现有测试为何没有发现

现有 Project Context E2E 覆盖了以下正常路径：

```text
Incident -> 打开 Picker -> 键盘选择一个 Coordinate -> Run
```

它没有断言第一次选择后 Popover 必须关闭，也没有覆盖：

- 在仍打开的列表中选择第二个 Coordinate；
- 鼠标快速双击；
- 键盘重复 Enter；
- Picker 打开时切换到 `All Context` 或已填满的 `Incident`；
- 可恢复输入不得进入 Router 错误页。

纯函数测试目前还明确断言第二个 Incident Coordinate 应该抛异常。这条测试固定了领域约束，却把预期的
UI 拒绝路径错误地建模成了进程级异常路径。

## 3. 根因

### 3.1 Picker 的可见状态与可选状态分离

`CoordinatePicker` 使用本地受控 `open` 状态。父组件将 `disabled=true` 传给 trigger，并不会自动把已经
打开的 Popover 关闭或卸载。因此出现：

```text
trigger.disabled = true
popover.open = true
popover.options 仍可激活
```

这使“Incident 已经选满”只表现为不能再次打开 Picker，却不能阻止当前 Picker 内继续选择。

### 3.2 预期交互拒绝被实现为异常

以下输入都是可预期、可恢复的 UI 状态：

- `All Context` 下添加 Coordinate；
- 添加已选择的 Coordinate；
- `Incident` 已有一个 Coordinate 后再次添加。

当前 helper 对这些输入直接 `throw`。异常又发生在 React state updater 内，普通的按钮 disabled、事件层
`try/catch` 或后续表单校验都不能可靠兜底；在 React 重放 updater 或 stale handler 场景下尤其危险。

### 3.3 页面缺少最后一级局部恢复边界

Project Context 已经能够把非法 URL search 显示为专用恢复状态，但运行期组件异常没有 Project Context
级 fallback。于是一个 Query Bar 局部错误替换了整个应用内容区。

局部错误边界只能作为最后防线，不能替代前两项根因修复。

## 4. 数据、协议与影响边界

### 4.1 数据安全

本次异常发生在 `onRun()` 之前：

- 没有调用 Project Context Tauri command；
- 没有向 Relay 发布 Nostr event；
- 没有修改 Context revision；
- 没有产生半写入；
- 不需要数据库恢复、迁移、重建或清理。

修复与测试不得重置本地主数据库，也不得复用会清理主数据库的测试入口。

### 4.2 后端协议保持不变

现有闭合查询协议是正确的：

- `Incident` wire DTO 只有单数 `coordinate`；
- `Exact` 接受至少两个不同 Coordinates；
- `Contains all` 接受一个或多个 Coordinates；
- 空集合由 `All Context` 表达；
- Tauri/Rust 继续保留严格 canonicalize 与结构化 `invalid_input` 校验。

本次不放宽 Incident arity，不修改 Relay capability、事件 kind、数据库 schema 或 URL canonical shape。

### 4.3 相邻风险

Incident 是当前可稳定复现的路径。相同的异常式 helper 还使以下交互存在全页错误风险：

- 同一选项快速双击；
- 搜索框中连续按 Enter；
- React render 尚未收敛时触发 stale option；
- Picker 打开期间切换到 `All Context`；
- 以后新增快捷选择入口时重复提交同一 Coordinate。

因此不能只为本次截图添加一个 Incident 特判。

## 5. 修复后的交互语义

| 查询模式 | Coordinate 约束 | Picker 行为 |
|---|---|---|
| `All Context` | 0 | trigger 禁用；若模式切换前 Picker 已打开，立即关闭 |
| `Incident` | 恰好 1 | 第一次成功选择后立即关闭并禁用；移除 chip 后可重新打开 |
| `Exact` | 至少 2 个不同坐标 | 保持连续多选体验；重复选择安全忽略，不关闭页面 |
| `Contains all` | 至少 1 个不同坐标 | 保持连续多选体验；重复选择安全忽略，不关闭页面 |

共同约束：

- 用户操作不能从 React state updater 抛异常；
- invalid draft 只禁用 `Run` 并显示现有局部校验文案；
- stale 或重复输入必须是幂等 no-op，必要时显示局部、非阻断提示；
- 只有通过校验的 closed query union 可以交给 Tauri；
- Query Bar 故障不能替换整个 Desktop 应用壳。

## 6. 修复实现方案

### 6.1 将草稿变更改为安全 transition

为 Query Bar 提供不抛异常的坐标添加 transition，例如：

```ts
type ProjectContextDraftTransition =
  | { status: "changed"; draft: ProjectContextQueryDraft }
  | {
      status: "unchanged";
      draft: ProjectContextQueryDraft;
      reason: "mode_all" | "duplicate" | "incident_full";
    };
```

实现要求：

1. transition 是纯函数；
2. 可恢复输入返回当前 draft，不抛异常；
3. canonical 顺序和去重语义保持不变；
4. Query Bar 使用 reducer 或等价的原子状态转换，同时维护可选的局部提示；
5. 不在 React state updater 内执行 toast、日志或其他副作用；
6. 严格 wire canonicalize 继续保留在 Tauri 边界。

现有 `addProjectContextDraftCoordinate()` 可以改为安全 transition，或保留为只供严格内部边界使用并新增
`tryAddProjectContextDraftCoordinate()`。无论采用哪种命名，UI 不得再调用会抛异常的版本。

### 6.2 收口 Popover 生命周期

`CoordinatePicker` 增加以下行为：

1. `Incident` 成功选择第一个 Coordinate 后立即关闭 Popover；
2. `disabled` 从 `false` 变为 `true` 时强制 `setOpen(false)`；
3. 切换到 `All Context` 时关闭已打开的 Popover；
4. `selectOption()` 在提交前再次检查 `disabled` 和当前可选集合；
5. 关闭时统一清空 search 与 highlighted index；
6. `Exact` 与 `Contains all` 成功选择后默认保持打开，便于连续多选。

可以通过显式的 `closeOnSelect` / `selectionPolicy` prop 表达模式差异，避免 Picker 读取 Project Context
领域状态或把所有模式都改成单选体验。

### 6.3 防御 Run 路径

`Run` 继续由 `projectContextDraftValidationMessage()` 控制 disabled，但点击处理还应再次执行安全转换：

1. validation 失败时不调用 `onRun()`；
2. 查询转换失败时在 Query Bar 内显示可恢复错误；
3. 不允许 `projectContextQueryFromDraft()` 的异常穿透事件处理器；
4. 成功时仍只提交 canonical closed union。

这层检查用于防御 stale click 和未来调用点，不替代按钮 disabled。

### 6.4 增加 Project Context 局部错误恢复

为 `/project-context` route 或 `ProjectContextScreen` 增加功能域级错误 fallback：

- 保留 Desktop 导航与 Community 上下文；
- 显示简洁错误摘要；
- 提供 `Retry Project Context` / `Reset query`；
- reset 后回到 canonical `All Context`；
- 不自动修改任何 Project Context 数据。

该边界只兜底未知编程错误。重复 Coordinate 等预期交互仍必须在 Query Bar 内正常处理，不能依赖错误边界
作为主流程。

## 7. 自动化测试方案

### 7.1 纯函数测试

更新 `../../../desktop/src/features/project-context/queryModel.test.mjs`：

1. Incident 第一次添加返回 changed；
2. Incident 第二次添加返回 unchanged / `incident_full`，且不抛异常；
3. 重复 Coordinate 返回 unchanged / `duplicate`；
4. All 下添加返回 unchanged / `mode_all`；
5. Exact 与 Contains all 连续添加保持 canonical 顺序；
6. 有效 draft 转换出的 closed query wire shape 不变。

### 7.2 Query Bar 组件/E2E

扩展 `../../../desktop/tests/e2e/project-context.spec.ts`：

1. Incident 选择一个 Coordinate 后 Picker 关闭、trigger 禁用、只显示一个 chip；
2. 快速双击同一选项不进入 `Something went wrong!`；
3. 搜索框重复 Enter 不进入 Router 错误页；
4. 尝试第二个 Incident Coordinate 不改变 draft，也不发 native 请求；
5. 删除 Incident chip 后 Picker 重新可用；
6. Picker 打开时切换到 All，Popover 关闭且 Coordinates 清空；
7. Exact 可以连续选择至少两个不同 Coordinates 后 Run；
8. Contains all 可以连续选择多个不同 Coordinates 后 Run；
9. 草稿阶段不调用 native，只有 Run 后按 canonical query 调用；
10. Project Context 局部 fallback 可以 reset，不替换整个应用壳。

### 7.3 回归门禁

至少执行：

- Project Context 纯函数测试；
- Project Context 定向 Playwright E2E；
- Desktop typecheck；
- Desktop lint/check；
- Desktop production build。

本次不需要启动真实 Relay 或修改本地 canonical 数据。真实 Desktop 验收只执行只读查询，并保留当前
Community 数据。

## 8. 实施顺序

1. 增加安全 draft transition，并改写原有 throw 测试；
2. 将 Query Bar 接入安全 transition；
3. 修复 Incident、disabled 和 mode change 的 Popover 生命周期；
4. 防御 Run 路径；
5. 增加 Project Context 局部错误 fallback；
6. 补齐纯函数与 E2E 回归；
7. 运行 Desktop 定向质量门禁；
8. 在不清理数据库的前提下重新构建，由 Human 执行真实 Desktop 验收。

## 9. 验收标准

修复只有同时满足以下条件才算完成：

1. 原始 Incident 复现步骤不再出现全页错误；
2. Incident 永远只能形成一个 Coordinate 的合法 draft；
3. 重复点击、快速双击和重复 Enter 均为幂等、可恢复操作；
4. Exact 与 Contains all 的连续多选体验没有回退；
5. All 模式不能残留已打开 Picker 或已选 Coordinates；
6. 非法 UI 草稿不会调用 Tauri/Relay；
7. Tauri/Rust 的严格查询协议与 canonicalization 没有被放宽；
8. 未修改、删除、迁移或重建任何现有 Project Context、Project View、Document、Meeting 或消息数据；
9. Project Context 未知组件错误最多降级当前功能域，不再替换整个 Desktop 应用壳；
10. 定向自动化测试、typecheck、check 与 build 全部通过。

## 10. 非目标

本次不处理：

- Project Context Edge 的 attach / detach 语义；
- 新增查询模式；
- 修改 Incident、Exact 或 Contains all 的领域定义；
- 修改 Relay capability 或权限；
- 修改 Project View / Project Document；
- 恢复或迁移任何数据；
- 为终态 Meeting 增加 Project Context 坐标。

## 11. 实现记录

实现日期：2026-08-07。

### 11.1 Query Draft 改为安全状态迁移

`../../../desktop/src/features/project-context/queryModel.ts` 新增了
`tryAddProjectContextDraftCoordinate()`：

- 正常添加返回 `changed`；
- All 模式的 stale 输入返回 `mode_all`；
- 重复坐标返回 `duplicate`；
- 已填满的 Incident 返回 `incident_full`；
- 上述可恢复输入均保持原 draft 引用且不抛异常。

`addProjectContextDraftCoordinate()` 现在复用该 transition，并表现为幂等的 Query Bar 更新。
`projectContextQueryFromDraft()` 和 Tauri/Rust 边界的严格校验没有放宽。

### 11.2 收口 Coordinate Picker 与 Run 生命周期

`../../../desktop/src/features/project-context/ui/ProjectContextQueryBar.tsx` 已完成：

- Incident 首次成功选择后立即关闭 Picker；
- Picker 进入 disabled 状态时强制关闭并清空临时搜索状态；
- All 模式不会保留已打开的 Picker；
- stale、重复点击和重复 Enter 由 Picker 前置检查与 draft transition 双重幂等处理；
- Exact 与 Contains all 仍保持连续多选；
- Run 使用当前 render 中已安全转换的 closed query，转换失败只显示 Query Bar 局部校验信息，
  不再让异常穿透事件处理器。

### 11.3 增加功能域级恢复边界

`../../../desktop/src/app/routes/project-context.tsx` 已注册 Project Context route 专用错误组件：

- 未知组件异常只降级 Project Context 内容区，Desktop 导航和 Community 外壳保持可用；
- 提供 `Retry Project Context` 和 `Reset query`；
- Reset 只把 URL 查询恢复为 canonical All Context，不执行任何数据写入。

## 12. 自动化验收结果

2026-08-07 完成以下自动化验收：

| 门禁 | 结果 |
|---|---|
| Project Context Query Model 定向测试 | 6/6 通过 |
| Project Context Desktop Playwright E2E | 28/28 通过 |
| Desktop TypeScript typecheck | 通过 |
| Desktop `pnpm check` | 通过 |
| Desktop production build | 通过 |

新增回归覆盖了：

- Incident 同一 stale DOM 生命周期内连续选择两个不同坐标；
- Incident 搜索框连续两次 Enter；
- Incident 清空后可重新选择；
- Picker 打开时切换 All 会立即关闭；
- 草稿交互不产生额外 native 请求；
- 页面不进入 `Something went wrong!`；
- 原有 Exact 与 Contains all 连续多选、canonical 提交和 URL 恢复测试继续通过。

测试使用 Desktop E2E Mock Bridge，没有连接真实 Relay，没有启动或清理本地主数据库，也没有修改任何
Project Context、Project View、Document、Meeting、Channel 或消息数据。真实 Desktop 仍保持关闭，待
Human 后续要求重新构建启动后再进行手工验收。
