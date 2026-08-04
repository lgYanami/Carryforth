# Meeting Desktop 发布候选验收

> 状态：阶段六代码与自动化已完成；真实 Tauri 人工穿行待执行且只执行一次，不以重复观察形成无限验收循环。
>
> 范围：Desktop Meeting V2。后端及真实 Agent 证据见
> [`meeting-v2-qualification-report.md`](../v2/meeting-v2-qualification-report.md)。

## 1. 判定原则

Meeting Desktop 的发布证据由三层组成：

1. native Rust 测试证明签名、身份、Community、协议和 opaque command fence；
2. Playwright E2E 证明 Desktop 产品状态、Human 操作、响应式、恢复和隔离；
3. 既有真实 Provider qualification 证明 Agent 主持/参会、同 Session 行动收口和终态。

Mock E2E 不替代 native wire 验证，真实 Agent qualification 也不冒充 Desktop UI 验收。三层证据按
各自边界组合，不要求用人工操作重复穷举所有已自动化分支。

## 2. 自动化矩阵

| 场景 | 主要证据 |
|---|---|
| read/compatibility/privacy | `meeting-read-only.spec.ts`、native Meeting tests |
| Human Create 与 capability | `meeting-create.spec.ts` |
| Human Request/Offer/Grant/Speech/Yield | `meeting-floor.spec.ts` |
| Human Board/Intent/Handoff/self Speech/Close/Abort | `meeting-host.spec.ts` |
| Human Action confirm/block/retry/return/abort | `meeting-actions.spec.ts` |
| 断线、重连失败、成功 resync | `meeting-recovery.spec.ts` |
| Community A→B→A 与草稿/面板隔离 | `meeting-recovery.spec.ts` |
| 65 个会议的 64-ID 批次与分页渲染 | `meeting-recovery.spec.ts` |
| 窄窗口 Board/participant Sheet 与草稿保持 | `meeting-recovery.spec.ts` |
| Desktop preview gate 与 Meeting/Channel 隔离 | `meeting-recovery.spec.ts` |
| Agent host、Agent participant、真实 Provider | `../v2/meeting-v2-qualification-report.md` |
| Agent direct-action Project View CLI 与同 Session | `../v2/meeting-v2-backend-operations.md` 第 11 节 |

阶段六针对性命令：

```bash
. ./bin/activate-hermit
cd desktop
pnpm build:e2e
pnpm exec playwright test \
  tests/e2e/meeting-read-only.spec.ts \
  tests/e2e/meeting-create.spec.ts \
  tests/e2e/meeting-floor.spec.ts \
  tests/e2e/meeting-host.spec.ts \
  tests/e2e/meeting-actions.spec.ts \
  tests/e2e/meeting-recovery.spec.ts \
  --project=smoke
```

最终门禁：

```bash
. ./bin/activate-hermit
just ci
```

### 2.1 当前候选自动化结果（2026-08-04）

- Meeting Desktop 合并回归：34/34 通过；
- 阶段六 recovery/Community/bounded query/narrow sheet/preview gate：5/5 通过；
- Desktop JavaScript 单元测试：3528/3528 通过；
- Desktop `check`、production build 与 E2E build：通过；
- 窄窗口 Board、participant scoped PNG：hash 不重复；
- 仓库级 `just ci`：通过，包括 workspace/Desktop clippy、Desktop native 1680 项通过、Web build
  与 Mobile 568 项通过（另有 1 项按既有配置跳过）。

这些结果冻结本候选的自动化门槛；除非后续真实穿行发现新的语义、数据安全或恢复缺陷，不重复运行
相同成功矩阵来制造额外发布门槛。

## 3. 一次性真实 Tauri 穿行

在支持 Meeting V2 direct actions 的真实 Relay 上，用当前 release candidate 只执行一轮：

| 场景 | 必须观察 |
|---|---|
| Human host + Human participant | Create → Request → Offer → Grant → Speech → Board → direct close |
| Human host + Agent participant | Agent Intent/Speech 可见；Human 不能替 Agent ACK 或 Speech |
| Agent host + Human participant | Human Floor 可用；Host Console 只读；Agent 推进 Board/Floor |
| Human host action | 打开现有 Project View、返回、confirm；另做一次零写入 confirm |
| Agent host action | 原 ACP Session 完成或 blocked；Desktop 不出现接管按钮 |
| Recovery | 断开 Relay、重连、A→B→A；重读成功前所有 window 写入保持禁用 |
| Terminal | direct closed、actions-recorded closed、discussion abort、action abort |

穿行只记录：Desktop commit、Relay capability、参与身份、Meeting ID、每个场景 PASS/FAIL 和失败
截图。不得记录私钥、完整 Board/Speech、Prompt 或模型原始输出。

若某项失败，只在发现新的语义、数据安全或恢复缺陷时重开开发；纯布局偏好作为后续问题记录。
同一候选版本不通过重复运行相同成功场景制造额外门槛。

## 4. 发布阻断项

以下任一项出现即阻断：

- Meeting 落入普通 Channel Composer、Thread 或 Reaction 路径；
- 非 roster 看到 title、Board、roster 或 Speech；
- 断线或重连 snapshot 完成前仍能提交当前 window 命令；
- Community 切换后旧 Meeting 草稿、pending command 或 query 内容可提交；
- Human 能替 Agent 执行 ACK、Speech、主持或行动完成声明；
- action close 绕过最终 Board 或重新引入 Plan/Step/materializer；
- 重连、双击或 response loss 产生重复 Speech 或重复完成声明。

## 5. 非阻断观察

- Project View 不可用不等于 Meeting action capability 不可用；Human 仍可确认无需新增登记；
- Meeting 不验证 Board 与外部系统语义一致，也不要求至少一次外部写入；
- timeline 滚动位置属于本地体验；重新进入时可以不精确恢复；
- 真实 Tauri 穿行中的纯视觉偏好不重开已经通过的协议阶段。
