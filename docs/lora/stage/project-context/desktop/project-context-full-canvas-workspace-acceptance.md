# Desktop Project Context 全画布工作区验收记录

> 状态：纯前端实现与自动化/视觉验收通过；实现提交待回填
>
> 日期：2026-08-12
>
> 对照计划：
> [Desktop Project Context 全画布与可折叠右侧工具栏分阶段实现计划](./project-context-full-canvas-workspace-implementation-plan.md)
>
> 实现提交：待提交后回填

## 1. 验收边界

本记录只验收 Desktop Project Context 的 presentation 改造：全画布 workspace、画布 HUD、单一
Tool Rail、Structure / Semantic / Details 共用面板、响应式 Drawer / Sheet、viewport 连续性、focus
与可访问性。

本次交付是纯前端改造，没有修改 Relay、DB、Tauri wire、Project Context 或 semantic query DTO、URL
schema、权限、currentness、semantic ranking 或 provider egress 合同。本记录不替代既有 semantic
qualification，也不把布局自动化结果解释为 semantic production readiness。

## 2. 自动化验收结果

| 证据 | 结果 | 边界 |
|---|---:|---|
| Desktop unit | `3715 / 3715` 通过 | Desktop JavaScript / TypeScript 单元测试 |
| `pnpm typecheck` | 通过 | TypeScript 静态类型检查 |
| `pnpm check` | 通过 | Biome、file-size、px-text 与 pubkey guards |
| `pnpm build:e2e` | 通过 | production-mode E2E bundle |
| `project-context-workspace.spec.ts` | `12 / 12` 通过 | 全画布、panel、viewport、semantic 与响应式矩阵 |
| 既有 Project Context spec | `34 / 34` 通过 | 排除一项本分支既有 Community rail baseline，见第 6 节 |
| Document live announcement 定向门 | `1 / 1` 通过 | 唯一 live owner 与 content-free 错误公告 |

完整 workspace 矩阵确认：

- `1280×800` 与 `1440×900` dense default 均只提交一次初始 Fit；
- fixture 中 `44 / 44` Coordinate 全部渲染，并且没有与 HUD 或 Tool Rail 的实测矩形相交；
- graph slot 从 Header / banner 下缘占满剩余 workspace，页面无多余纵向或横向 overflow；
- Structure / Semantic draft、普通 selection、semantic session 与 graph DOM 在 tool switch / collapse 后保持；
- presentation 操作不增加 trusted Project Context 或 semantic query 调用；
- docked panel resize 前后 zoom 保持到三位小数，graph world-center 两轴误差均不超过 2 px；
- ResizeObserver 两帧窗口和 220 ms Fit 动画期间发生 Human Zoom 时，旧 correction / Fit 均不会回写；
- pending Fit 同一 microtask 遇 root scale `1.1` 时，以 Human focal viewport 胜出，world-center 两轴误差
  不超过 2 px，且 500 ms 后无迟到回写；
- expanded docked panel 在 root scale `1.5` 后切为 Drawer 时，canvas 扩宽、zoom 保持，映射后的
  world-center 两轴误差均不超过 2 px；
- Drawer / Sheet 始终只有一个 Rail，具备 modal、focus trap、分层 Escape 与正确 focus restore；
- Sheet 内执行 Fit paths 后会先关闭 modal，再把焦点恢复到可编程聚焦的 graph slot，且 Fit 正常完成；
- topology 变更会同步撤下不匹配 overlay，同时保留 stale session HUD、禁用 Fit 并继续提供 Clear；
- collapsed semantic HUD 显示 path/root/revision、partial coverage 与 budget exhaustion；
- Project Context workspace 只有一个 keyed `aria-live` owner；Drawer / Sheet 中该 owner 随面板进入
  `SheetContent` portal，不位于任何 `aria-hidden` ancestor 下；Document verification failure 通过它播报固定
  content-free 文案。

## 3. 布局与 viewport 合同实证

在默认 root text scale（`1rem = 16px`）下：

- Rail 宽 `3rem = 48px`；
- panel content 默认 `27.5rem = 440px`，允许 `22.5rem..35rem`；
- dock 至少保留 `40rem = 640px` canvas，因此默认 dock threshold 为 `1128px`；
- workspace 小于 `37.5rem = 600px` 时使用 full-width Sheet；
- 默认宽度下 `600px..1127px` 使用覆盖 canvas 的 Drawer；
- Drawer / Sheet 打开时暂停 Fit，pane 中的 Fit / Focus 先建立 generation、关闭 modal、等待新 chrome
  稳定后再提交。

这些尺寸随 Desktop root text zoom 的 rem scale 一起变化；panel open / close / resize 不改变 graph topology、
query identity 或 semantic request identity。

## 4. 视觉验收

六张截图位于本地 E2E artifact 目录 `desktop/test-results/project-context-workspace/`。每张截图前均等待
动画完成，SHA-256 全部互异：

| 场景 | 文件 | SHA-256 |
|---|---|---|
| dense light | `01-dense-light.png` | `86581ffc6af6fa53776326899709109e898bde170b4cfabc8eb778700b0f05cb` |
| dense dark | `02-dense-dark.png` | `d94b71a154fef9f3b81bd14c27e511304e37b55aaf19809434fd0253f528a09a` |
| docked Details | `03-details-docked.png` | `dfb4190ba376cac51f8f9368977523ea1e34551ec7c50977366da95ac6ffad8d` |
| semantic + selection | `04-semantic-selection.png` | `fc1c0f12630fc9e8e47849e1b2a17a6e4c92b925e9fbd7f2e19c2c0d946a8f20` |
| 1040px Drawer | `05-narrow-drawer.png` | `7538bb965dbf39e3ff86047867dc53144af55bc3ac8db8153756ff403e470ce5` |
| stale semantic snapshot | `06-semantic-stale.png` | `7bb05464dcce2a5f25b8832e169a832c1bef4dbf14b93d7cdb95d2602df0bd5b` |

人工复核结论：

- light / dark 默认态均以完整关系图为主体，44 个 Coordinate 可见，HUD 与默认折叠 Rail 不遮挡图；
- docked Details 使用单一右槽，canonical Inspector 可读，画布仍保留足够空间；
- semantic 路径、root、普通 selection 与非路径弱化可以同时辨识；
- stale semantic 态清楚显示旧 revision、partial / budget 状态、禁用的 Fit paths、可用的 Clear 与
  Rail warning badge，同时不再显示已失配的 semantic overlay；
- 1040px 使用覆盖式 Drawer，背景 canvas 保留，不被压缩或重排；560px full-width Sheet 由同一
  workspace E2E 的后续自动化断言覆盖，但不由该截图声称；
- 截图中没有残留 Rail tooltip、调试 overlay 或焦点测试标记。

## 5. 文件与范围审计

| 字段 | 结果 |
|---|---|
| `ProjectContextScreen.tsx` | 794 行，低于 1000 行硬上限并保留约 20% 余量 |
| `ProjectContextGraph.tsx` | 789 行，低于 1000 行硬上限并保留约 20% 余量 |
| Native / Relay / DB diff | 无 |
| Wire / route schema diff | 无 |
| 新 module-level Community cache | 无 |
| Semantic structural join | 不变；只增加 HUD 所需的 content-free presentation flags |

## 6. 已知边界

- `project-context.spec.ts` 中既有 `Community switch never paints...` baseline 在本次改造前就因当前分支
  `projectConnectableCommunities()` 只投影一个 Local Community 而失败；本次没有更改 Community 领域行为，
  也没有弱化该测试。其余相关旧 spec 为 `34 / 34` 通过。
- 仓库级 `just ci` 已执行两次：格式、全 workspace Clippy、Desktop / Tauri / Web / Mobile 静态门及
  `test-unit` 的其余 `28 / 29` 组通过；两次都只在未改动的 `buzz-acp`
  `spawn_does_not_inherit_a_missing_carryforth_auth_tag` 并行环境测试失败，而该测试单独精确复跑
  `1 / 1` 通过。本记录不把它伪报为完整 repo gate 通过，也不越过本次纯前端范围修改 ACP。
- 截图是本地 ignored E2E artifact，不作为源码提交；本记录保存文件名与不可变 hash。
- 本验收证明全画布前端布局与交互完成，不改变 semantic qualification 中关于 known-negative、quality
  floor 校准、source / revision stale smoke 或 production multi-pod qualification 的结论。
- 当前实现尚未提交；提交 SHA 应在 commit 后回填。

## 7. 最终判定

Phase F0–F6 的纯前端实现、定向自动化、旧领域回归（除上述既有 baseline）和六态视觉验收均通过。
Project Context 默认呈现全画布关系图，Structure / Semantic / Details 已统一到右侧可折叠工具区；panel、
selection 与 semantic overlay 的状态边界及 viewport authority 合同均有自动化证据。
