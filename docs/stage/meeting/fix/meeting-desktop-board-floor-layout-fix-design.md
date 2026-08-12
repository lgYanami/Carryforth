# Meeting Desktop Board 与进程面板布局修复设计

> 状态：代码与自动化已完成，待真实 Desktop 视觉验收
>
> 日期：2026-08-07
>
> 缺陷编号：`MFX-018`
>
> 产品基线：
> [Meeting Desktop 产品规格](../desktop/meeting-desktop-spec.md)
>
> 缺陷索引：
> [Meeting V2 真实验收缺陷清单](meeting-v2-defect-list.md)

## 0. 交付结果（2026-08-07）

本方案已经完成代码和自动化交付：

- `MeetingScreen` 已将 timeline 与 `MeetingFloorDock` 组合为完整左列，宽屏 Board 作为右列
  兄弟节点覆盖主工作区完整高度；
- Board 收起后左列自动占满工作区，既有 Board draft、宽度和 timeline 滚动位置保持不变；
- `MeetingHostObservation` 已移除 `mx-auto max-w-4xl`，填满 Floor Dock 的可用内容宽度；
- 新增 `meeting-work-area` 与 `meeting-left-workspace` 稳定测试定位点；
- Playwright 已增加宽屏上下边界、Dock/Board 列边界、观察面全宽、Board 收起扩展以及窄屏
  单列降级的几何断言。

已完成检查：

- Biome targeted check；
- TypeScript typecheck；
- Desktop E2E build；
- `meeting-host`、`meeting-recovery`、`meeting-floor` 共 23 条 Playwright 用例；
- Desktop JavaScript 单元测试 3562 条；
- file-size、px-text 与 pubkey-truncation guards。

上述检查全部通过。本次没有修改 Relay、数据库、Meeting 数据或运行中会议；在真实 Desktop
重新构建并启动后的视觉验收通过前，缺陷状态保留为“代码修复完成，待视觉验收”。

## 1. 结论

当前宽屏 Meeting 页面没有把“讨论与进程”和“Meeting Board”组织成两个完整的工作区。
`Meeting Board` 只与 Speech timeline 处于同一横向容器，`MeetingFloorDock` 则位于该容器之外，
因此出现以下错误布局：

- Board 只能占据时间线右侧的局部高度，无法延伸到页面工作区底部；
- 会议进程面板横跨整个页面并进入 Board 下方，而不是只位于左侧讨论区下方；
- Agent 主持观察面带有独立的 `max-w-4xl` 限宽，在较宽的左侧空间内仍居中显示并留下大块空白。

本次修复只调整 Desktop React 的布局层级和宽度约束，不修改 Relay、Meeting 协议、Tauri
数据模型、Board/Floor 状态机或任何权限语义。

## 2. 目标布局

宽屏下，Meeting 主工作区固定为左右两列：

```text
┌──────────────────────────────────────┬──────────────────────┐
│                                      │                      │
│ canonical Speech timeline            │                      │
│                                      │    Meeting Board     │
│                                      │                      │
├──────────────────────────────────────┤                      │
│ Meeting process / Floor Dock         │                      │
└──────────────────────────────────────┴──────────────────────┘
```

布局语义为：

1. 左列是会议讨论工作区；上部为可独立滚动的正式 Speech timeline，下部为
   `MeetingFloorDock` 和 Agent 主持进程观察面；
2. 右列只承载 Meeting Board，并占满整个主工作区高度；
3. Board 左边缘仍可拖拽调整宽度；关闭 Board 后，左列自动占满全部可用宽度；
4. 页面标题、会议状态条和终态摘要继续位于主工作区上方，不纳入 Board/Floor 的滚动区域；
5. 中窄窗口继续使用现有 Board Sheet，不在窄屏强行保留双列。

## 3. 当前实现与根因

### 3.1 DOM 层级错误

当前 `MeetingScreen.tsx` 的结构等价于：

```text
MeetingScreen
├── header / status / terminal summary
├── horizontal row
│   ├── Speech timeline
│   └── wide Meeting Board
└── MeetingFloorDock
```

Board 的高度只能跟随 `horizontal row`。`MeetingFloorDock` 是该 row 之后的全宽兄弟节点，
所以 Board 不可能覆盖 Dock 对应的右侧空间。这个问题不能通过给 Board 增加 `h-full` 解决，
因为其父容器本身已经在 Dock 之前结束。

### 3.2 进程观察面的重复限宽

`MeetingFloorDock` 已经决定了面板所在区域，但 `MeetingHostObservation.tsx` 又使用：

```text
mx-auto w-full max-w-4xl
```

这会让 Agent 主持进程面板在左列足够宽时仍被压缩并居中。该限制属于旧的全页 Dock 布局，
不适用于修复后的左列工作区。

### 3.3 Board 内容组件不是根因

`MeetingBoardPanel` 已经使用 `flex min-h-0 flex-col`，正文也是独立的 `overflow-y-auto` 区域；
它具备填满父容器并自行滚动的能力。本次不需要重写 Board 内容或编辑器，只需给它正确的父级
高度和布局位置。

## 4. 修复边界与不变量

### 4.1 必须保持

- 宽屏 Board 默认打开、可关闭、可重新打开；
- `useResizableMeetingBoardWidth` 的持久宽度、拖拽、键盘调整和双击复位行为；
- `1280px` 以下使用 Board Sheet 的现有响应式策略；
- timeline、Board 和进程面板都读取同一份权威 Meeting snapshot；
- Board 草稿、Speech 草稿、stale draft 和 command fence 生命周期不因布局切换而重建；
- Agent host 观察面保持只读，不能因此获得 Human Host Console 操作；
- canonical Speech unread、Meeting attention、Floor 和 Action Finalization 语义不变；
- 既有 `data-testid`、`aria-label`、焦点恢复和 Board resize 键盘语义。

### 4.2 不在本次处理

- Meeting 协议和事件 kind；
- Floor Decision、Action Finalization 或 lease 生命周期；
- Board 数据结构、Markdown 内容或编辑能力；
- 标题栏、Participant Sheet、Activity Sheet 的信息架构；
- Web、Mobile 或普通 Channel 页面；
- 重新设计中窄屏 Board Sheet。

## 5. 实现方案

### 5.1 重组 `MeetingScreen` 主工作区

将宽屏 Board 的直接兄弟从单独的 Speech timeline 改为完整的左侧工作区。推荐使用嵌套 Flex，
以继续直接复用 Board 的像素宽度和现有 resize hook：

```text
work area: flex row, min-h-0, flex-1
├── left workspace: flex column, min-h-0, min-w-0, flex-1
│   ├── timeline: min-h-0, flex-1, overflow-y-auto
│   └── MeetingFloorDock: shrink-0
└── Board aside: shrink-0, min-h-0, persisted width
```

具体调整：

1. 在当前主工作区 row 中增加左侧 `flex min-h-0 min-w-0 flex-1 flex-col` 容器；
2. 将 `MeetingSpeechTimeline` 所在 `main` 和 `MeetingFloorDock` 一并放入该左侧容器；
3. 保持宽屏 Board `aside` 为左侧容器的兄弟节点，使其自然拉伸到主工作区完整高度；
4. 为 Board `aside` 保留 `style={{ width: boardWidth.widthPx }}` 和现有 resize handle；
5. Board 关闭或进入 Sheet 模式时不渲染该 `aside`，左侧容器自然扩展为全宽；
6. 不通过绝对定位或计算 Dock 高度来“补齐”Board，避免缩放、字体放大和内容变化导致错位。

相比跨两行 CSS Grid，嵌套 Flex 对当前代码改动更小，也不需要把可拖拽像素宽度复制到
`grid-template-columns`。它仍明确表达同一个两列产品结构。

### 5.2 让进程面板使用完整左列宽度

`MeetingFloorDock` 作为左列底部区域保留外层背景、上边框和 padding。对
`MeetingHostObservation` 做以下调整：

- 移除外层 `mx-auto` 和 `max-w-4xl`；
- 保留 `w-full`、边框、背景和现有最大高度/内部滚动；
- 为外层补齐 `min-w-0`，避免长内容迫使左列产生横向滚动；
- 普通按钮组、错误提示和输入表单可以继续保留较窄的内容宽度以维持可读性，但
  Agent host progress 这一完整状态面板必须填满 Dock 的可用内容宽度。

这里的“填满”是指占满左侧工作区的可用横向空间，不要求用空内容强行撑高 Dock。Dock 高度仍由
当前阶段和内容决定；过长的主持进程内容继续在既有有界区域内滚动。

### 5.3 明确三个滚动边界

修复后的页面必须保持三个互不劫持的滚动区域：

1. Speech timeline：`min-h-0 flex-1 overflow-y-auto`；
2. Meeting Board 正文：继续由 `MeetingBoardPanel` 内部 `overflow-y-auto`；
3. 过长的 Agent host progress：继续使用自身有界 `overflow-y-auto`。

主工作区本身保持 `min-h-0`，不把整个 Desktop shell 推出视口。Board resize、终态切换和进程内容
增加时，不应使页面产生额外的全局纵向或横向滚动条。

### 5.4 响应式降级

沿用 `boardPanelIsOverlay = useMediaBreakpoint(1280)`：

- 宽屏：渲染双列工作区，Board `aside` 占满右列高度；
- 中窄屏：不渲染 Board `aside`，左侧工作区占满宽度，Board 继续通过 Sheet 打开；
- breakpoint 切换只改变 Board 的呈现容器，不清空 Board/Speech 草稿或 Meeting query 状态；
- 从宽屏切到窄屏再返回时，恢复已持久化的 Board 宽度和既有显隐规则。

## 6. 预计修改位置

| 文件 | 修改内容 |
|---|---|
| `../../../../desktop/src/features/meeting/ui/MeetingScreen.tsx` | 重组主工作区，使 timeline 与 Floor Dock 组成左列，Board 成为完整右列 |
| `../../../../desktop/src/features/meeting/ui/MeetingFloorDock.tsx` | 补齐左列宽度和溢出约束，不改变 Floor/Action 逻辑 |
| `../../../../desktop/src/features/meeting/ui/MeetingHostObservation.tsx` | 移除 `max-w-4xl` 居中限宽，让进程面板填满左列 |
| `../../../../desktop/tests/e2e/meeting-host.spec.ts` | 增加 Agent 主持进程面板的宽屏几何布局断言 |
| `../../../../desktop/tests/e2e/meeting-floor.spec.ts` | 验证 Dock 只占左列及宽窄屏切换 |
| `../../../../desktop/tests/e2e/meeting-recovery.spec.ts` | 复核 Board resize、收起/恢复和草稿状态不回归 |

如现有 fixture 更适合放在 `meeting-actions.spec.ts`，可以将终态和 Action Finalization 布局断言放入
该文件；不为布局测试复制一套新的 Meeting 状态机 fixture。

## 7. 测试与验收方案

### 7.1 自动化断言

宽屏 viewport 下使用元素 bounding box 做结构断言，而不是只依赖截图：

1. `meeting-board-wide` 的顶部与主工作区顶部对齐；
2. `meeting-board-wide` 的底部与左侧工作区底部对齐；
3. `meeting-floor-dock` 的右边缘不越过 Board 左边缘；
4. `meeting-host-observation` 填满 Dock padding 内的可用宽度，不再受 `max-w-4xl` 限制；
5. Board 收起后，timeline 和 Dock 一起扩展到主工作区右边缘；
6. Board 重新打开及 resize 后，timeline、Dock 和 Board 均无重叠或横向溢出；
7. timeline 滚动不改变 Board 滚动位置，Board 滚动也不推动 Dock；
8. 窄屏没有 `meeting-board-wide`，Board trigger 仍打开 Sheet；
9. Agent host progress、Human Floor controls、Action Finalization 和 terminal read-only 四类状态至少
   各覆盖一个代表性 fixture。

几何断言允许 1～2 CSS pixel 的取整误差，不使用依赖特定文字长度的固定截图坐标。

### 7.2 视觉验收

至少生成一张宽屏 Agent-host Meeting 截图，确认：

- 右侧 Board 从主工作区顶部延伸到底部；
- 下方进程面板只位于左列；
- 进程面板不再窄居中并留下无意义空白；
- Board、timeline 和进程面板均无内容裁切；
- 字体缩放后布局仍成立。

截图前按仓库规范等待动画结束；如果输出多张状态截图，必须用 hash 检查它们不是相同画面。

### 7.3 建议检查命令

```bash
cd desktop
pnpm exec biome check src/features/meeting tests/e2e/meeting-host.spec.ts tests/e2e/meeting-floor.spec.ts tests/e2e/meeting-recovery.spec.ts
pnpm exec playwright test tests/e2e/meeting-host.spec.ts tests/e2e/meeting-floor.spec.ts tests/e2e/meeting-recovery.spec.ts
```

实现时再按实际测试项目配置选择对应 Playwright project；本次文档交付不触发 Desktop 重建或
运行中 Meeting 数据变更。

## 8. 实施顺序

### 阶段一：布局层级修复

- 重组 `MeetingScreen` 主工作区；
- 将 `MeetingFloorDock` 移入左侧工作区；
- 保持 Board 显隐、resize 和 Sheet 行为不变。

### 阶段二：宽度与滚动收口

- 移除 Agent host observation 的重复限宽；
- 补齐 `min-h-0`、`min-w-0` 和 overflow 边界；
- 手动验证长 Speech、长 Board 和长进程内容。

### 阶段三：自动化与真实 Desktop 验收

- 增加宽屏/窄屏几何断言；
- 覆盖 Board 收起、恢复、resize 和 Agent host progress；
- 在真实 Tauri Desktop 中完成一轮视觉验收。

三个阶段应作为同一小型修复交付，不需要 Relay 迁移、数据迁移或渐进发布开关。

## 9. 完成条件

满足以下条件后可关闭 `MFX-018`：

1. 宽屏 Board 独占完整右列，并覆盖 timeline 与进程面板合计高度；
2. MeetingFloorDock 只位于左列下方，不再横跨 Board 下方；
3. Agent host progress 使用完整左列内容宽度，不再出现 `max-w-4xl` 导致的无意义空白；
4. Board 收起、恢复、resize 和宽窄屏切换均无布局跳位、草稿丢失或状态重建；
5. timeline、Board 和长进程内容保持独立、有界滚动；
6. Human Floor、Agent host observation、Action Finalization 和终态只读状态没有功能回归；
7. 自动化通过，并完成至少一次真实 Desktop 宽屏视觉验收。
