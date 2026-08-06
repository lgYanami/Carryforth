# Project Context Desktop 阶段七验收证据

> 状态：阶段七已交付，待 Human 确认；本文件记录验收事实，不定义新的产品或领域语义。
>
> 对照计划：[Project Context Desktop 分阶段实现计划](./desktop-implementation-plan.md)。
>
> 对照规格：[Project Context Desktop 产品规格](./desktop-spec.md)。

## 1. 验收边界

阶段七只完成 Desktop 自动化、真实数据穿行和回归收口：

- Desktop 仍只有 Project Context trusted read boundary，没有 attach / detach 或 Document 写入 UI；
- 真实 fixture 由现有 `buzz` CLI 写入，Desktop 只读取同一 Relay 的已签名投影；
- 没有修改 Relay、数据库 schema、kind、wire protocol、领域语义、权限或 Mobile；
- 没有引入 Gap、过期、冲突、Island 健康度或“应该连接”之类系统推断；
- 没有发布、迁移、灰度或 rollout 工作。

## 2. 自动化结果

### 2.1 Project Context 与相邻功能

- Project Context Playwright：`27 / 27`；
- Project Context + Project View + Project Documents 联合 Playwright：`74 / 74`；
- Project Context 纯函数测试：`43 / 43`；
- Desktop 前端测试：`3599 / 3599`；
- Desktop Tauri：`1715` passed、`15` 个依赖显式外部环境的测试 ignored；
- Tauri mixer diagnostics：`3 / 3`；
- Project Context native 定向测试：`14 / 14`，真实 Relay probe 默认 ignored、由隔离脚本显式
  执行五次并全部通过。

联合 Playwright 覆盖 route / sidebar / deep link、四种查询入口、binary Edge、hyperedge、
重叠坐标集、Island merge / split、Fit Island、多 Document lazy Markdown、Document 的两种
结构角色、tombstone、unavailable、verification failure、selection 不触发 query、reconnect、
Community 隔离、窄窗口、keyboard，以及 Project View / Context References / Documents 回归。

### 2.2 Mock trusted-read 能力

`e2eBridge` 现在可以按 canonical query 与 Relay 返回不同结果，并支持独立 delay、结构化错误
和 successive read sequence。顺序化用例实际执行：

1. 保留 Revision 7 的 last verified graph；
2. 下一次 refresh 返回结构化 `unavailable`，图保留并标记 stale；
3. 再下一次 refresh 返回 Revision 9，整体替换结果并清除 stale。

测试不依赖定时猜测 selection 是否查询：query 同步稳定后清空调用记录，再证明 Coordinate
selection 没有产生新的 native Context read。

### 2.3 质量门禁

以下检查均通过：

- Biome、file-size、px-text 与 pubkey-truncation checks；
- TypeScript typecheck；
- production build 与 E2E build；
- Tauri `cargo fmt --check` 与 `cargo clippy --all-targets -- -D warnings`；
- shell syntax、`git diff --check`；
- Desktop 完整前端与 Tauri tests。

构建仍会报告仓库既有的 Vite chunk-size / mixed static-dynamic import 警告；本阶段没有观察到
Project Context 特有的非阻断体验问题。

## 3. 视觉验收

所有截图都在 `waitForAnimations(page)` 后获取。最终一次运行产生 7 个不同 SHA-256：

| 状态 | 文件 | SHA-256 |
|---|---|---|
| 完整双岛图 | `project-context-two-islands.png` | `a3f2a705d838a7dcf1fc21c68436b64ea2f522e261a7eb89895a20fba4a0ced6` |
| AB + ABC 重叠 Edge | `project-context-overlapping-edges.png` | `5beb8e51aab168f461a78bc8ce689b2f48b347306188ac591d46b1b837738852` |
| Incident Anchor | `project-context-incident-anchor.png` | `0e3938547daca24e30772338bda07290b535bff27544cca5f5d272534836e608` |
| 多 Document Edge Inspector | `project-context-edge-inspector-multi-document.png` | `762f2310121261dac00f103f806a2b4ae343faefa129ce281b5d05af7b94b76d` |
| tombstone / unavailable | `project-context-tombstone-unavailable.png` | `2e834a10bab4e5153b477d735b58c2dad725018412d4e78b0645428aae3ad696` |
| 真实 Buzz Dark theme | `project-context-dark-islands.png` | `a3721cf1e6de1d9ec5b3547010746ec1f68062125434c48e6fd1074030915415` |
| 窄窗口 Sheet | `project-context-narrow-sheet.png` | `a6768d702a9a5206885917cd87a94d4b7ba8f3bcb0211d0bc785712ae8790c69` |

Dark 截图通过持久化 `buzz-dark` theme 后 reload 取得，不是只给 graph 临时添加 `.dark` class；
因此同时验证页面文字、背景、Island、Node 与 Edge 的完整暗色对比度。

## 4. 真实 Tauri / Relay 穿行

`scripts/test-project-context-stage3-e2e.sh` 使用独立 scratch database、随机 Relay 端口和固定
测试身份；退出时停止测试 Relay 并删除该数据库。`PROJECT_CONTEXT_E2E_STAGE7=1` 的实际轨迹：

1. 在 projection generation 2、Context Revision 14 的 verified empty catalog 上开始；
2. CLI attach 第一份 Context Document，Revision 15；坐标集合复用阶段五既有稳定 Edge 身份；
3. CLI attach 第二条不相连 Edge，Revision 16；CLI All 为 2 Edges，Desktop native 计算为
   2 connected components；
4. 同一 Revision 16 下，CLI 与 Desktop 分别执行 exact、incident、contains-all，返回的
   Edge key、Coordinate membership 与 Context Document membership 都是 All 的同一子集；
5. CLI attach 跨岛 Edge，Revision 17；All 为 3 Edges，Desktop native 计算为 1 component；
6. CLI 将 Context Document A 从 Document Revision 1 更新为 2；Context Revision 仍为 17，
   Desktop 读取到新 title / revision，body 不进入 graph DTO；
7. CLI 将作为 Coordinate 的 Document tombstone 到 Revision 2；Context Revision 仍为 17，
   Edge 与 membership 保留，Desktop detail 为 `tombstoned`；
8. 删除仍绑定的 Context Document 返回 write-conflict exit code 5 和
   `conflict:project_document:still_referenced`；
9. 关闭 Context Edge capability 后，CLI 与 Desktop 仍从 verified meta 读取 Revision 17 的
   3 Edges，Desktop 明确返回 `capabilityEnabled = false`；
10. capability-off 状态下依次 detach 三份 Context Document，Revision 18 → 19 → 20；最终
    verified catalog 为空、capability 关闭、projection parity 与 integrity 均为 true。

最终 canonical 统计为 7 个稳定 Edge 行、10 个 Document binding 行和 20 条 Context change；
第一条阶段七 Edge 没有新增 Edge 行，正是“同一坐标集合只有一条稳定 Edge”的实际证明。

## 5. 结论

阶段七验收支持以下结论：Desktop 读取、查询、图形呈现、Inspector、live recovery 与相邻页面
回归都已闭环；真实 Relay 上的 CLI 与 Desktop 观察到同一份 Project Context；Context
Document 正文和 Coordinate 生命周期变化没有篡改 Edge 结构语义；Desktop 没有获得写入、
Gap 推断或可信边界旁路。
