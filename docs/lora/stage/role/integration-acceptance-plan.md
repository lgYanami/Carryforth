# 角色连续性集成验收方案

> 状态：待确认，尚未按本方案正式执行。
>
> 本文面向已经交付的 Project View v0、Desktop `View`、Role Continuity v0、
> Runtime supervision 与 Role Brief 增量刷新。它定义验收范围、环境、场景、证据和
> 发布判定，不修改既有概念设计或实现设计。
>
> 2026-08-07 覆盖说明：本文的 v2 Role runtime 场景只保留为历史验收记录。当前发布门槛
> 以 v3-only Role runtime 为准；schema v2 只作为 operator migration 输入，不能作为 CLI、
> Desktop 或 ACP fallback。见
> [Project View v3 Role History 运行时全量迁移修复设计](../bug/project-view-v3-role-history-runtime-migration-fix-design.md)。
>
> 关联文档：
>
> - [项目定位与目标](../../project-positioning.md)
> - [角色连续性概念设计](./role-continuity.md)
> - [角色连续性实现设计](./implementation-design.md)
> - [角色连续性变更记录](./changelog.md)

## 1. 验收目的

本轮验收不是再验证某个函数是否返回正确值，而是回答以下系统级问题：

1. Human、Agent、CLI 和 Desktop 是否读取同一份 Relay 签名的 Project View；
2. Role、Assignment、Work、Checkpoint 和 Handoff 是否真的跨成员、session 和 Runtime
   持续存在；
3. Assignment 替换、权限变化、Runtime 故障和 Project revision 变化时，旧身份是否被
   可靠 fencing，新成员是否可以接续；
4. 数据库规范状态、NIP-43 membership、Relay projection、CLI 输出、ACP Role context
   和 Desktop 展示之间是否保持一致；
5. 任何可信读取、Runtime 监督或并发写入失败时，系统是否 fail closed，而不是复用旧授权
   或产生部分提交。

验收目标是给出可追溯的发布结论：

- `通过`：所有必选门禁与场景通过；
- `有条件通过`：必选正确性与安全性通过，仅存在已明确的非阻塞环境或体验限制；
- `不通过`：任一必选安全、原子性、连续性或跨端一致性场景失败。

## 2. 验收范围

### 2.1 必选范围

- Project View v1 初始化、读写、CAS、分页、实时投影和 v2 cutover；
- Project View v2 Role、Role level 与 Community `owner/admin/member` 一致性；
- Proposal、Assignment 激活、人工结束、原子替换与旧 Assignment fencing；
- Work Responsibility、Commitment 与 waiting-for-continuation；
- append-only Checkpoint、Handoff、引用和 Role 历史分页；
- Human 与 Agent 交替修改同一 Project View；
- Desktop `View`、Role Inspector、Work Inspector 和 Community 切换隔离；
- ACP candidate/assigned/unavailable Role context；
- 新 session 的完整 Role Brief 与未变化 revision 的 compact Role Binding；
- Runtime supervisor binding、runtime ID/epoch、动态 fence、恢复和自动
  `unrecoverable` 的 fail-closed 条件；
- schema migration、冷建库、旧版本升级、并发迁移和 checked-in schema drift；
- NIP-98、Relay signer、Community scope、projection revision 和历史 cursor 校验。

### 2.2 非目标

- 不评价真实 LLM 的专业能力、推理质量或自然语言稳定性；
- 不把某个外部模型供应商是否在线作为角色连续性的发布门禁；
- 不覆盖尚未设计的“项目认知连续性”对象，例如正式的 Decision、Evidence、Assumption
  和 Conflict 生命周期；
- 不进行生产容量、长期 soak、跨地域延迟或多集群灾备测试；
- 不操作 staging/production Community，不复用真实用户或 Agent 私钥；
- 不把 mobile 作为本里程碑的必选 Human 管理端；
- 验收过程中不顺手修改实现。发现失败后先形成证据和缺陷结论，再单独决定是否修复。

## 3. 验收原则

### 3.1 规范状态优先

不能只看 Desktop 卡片或 CLI 文本。关键状态至少要在下列边界中交叉验证：

```text
PostgreSQL canonical state
        ↓
Relay-signed 40903 / 40904 / NIP-43 projections
        ↓
shared verified SDK assembler
        ├── buzz CLI
        ├── ACP Role Brief / Role Binding
        └── Desktop View
```

### 3.2 确定性测试优先

ACP 与 Runtime 的必选验收使用确定性的 fake ACP child 或本地受控模型响应，验证：

- prompt 中注入了什么；
- CLI 使用了什么 Assignment/runtime fence；
- Relay 接受或拒绝了什么；
- Runtime supervisor 如何恢复。

真实 Codex、Goose 或其他外部 LLM 只做可选 smoke test，不能替代确定性断言。

### 3.3 故障必须 fail closed

断言重点不仅是成功路径，还包括：

- meta 或 membership 无法验证；
- Relay identity 改变；
- stale project revision；
- stale Assignment；
- stale runtime epoch；
- supervisor binding 被撤销；
- recovery 证据不完整；
- monitor 不健康；
- projection 或 membership 写入失败。

上述情况均不得把旧 Brief、旧 Assignment 或旧 Runtime 当作当前授权。

### 3.4 环境与数据隔离

- 使用唯一 run ID、临时数据库、随机 Relay/health/metrics 端口和随机 Community host；
- Redis key 依赖随机 Community host 隔离；
- Relay signer 固定在本次临时环境中，以便客户端验证；
- Human、Agent A、Agent B、supervisor 使用不同的临时密钥；
- 不执行 `just reset`，不删除共享开发数据库；
- 执行前记录已有 `buzz_pv_*` 临时数据库，只清理由本次 run 创建的资源；
- 所有临时私钥和 supervisor 状态存放于 `mktemp -d`，结束后删除；
- 验收前后比较 `git status --short`，不得产生未预期的仓库修改。

## 4. 验收拓扑与身份

### 4.1 运行拓扑

```text
Desktop / Human CLI
          │
          ├───────────────┐
          ▼               ▼
      WebSocket        HTTP bridge
          │               │
          └────── Buzz Relay ────── Redis live fan-out
                       │
                       ▼
                  PostgreSQL
                       ▲
                       │
        ACP harness + deterministic Agent child
                       │
                       ├── buzz CLI
                       └── Runtime fence file

        trusted Runtime supervisor identity
                       │
                       └── status / evidence / recovery
```

### 4.2 测试身份

| 身份 | 用途 | 约束 |
|---|---|---|
| Relay signer | 签署 Project View 与 NIP-43 projection | 本次 run 固定，不复用生产密钥 |
| Human Owner | 初始化 Project、创建 Role、治理 Assignment | Community owner |
| Human Leader | 验证 admin Role 与既有 Community admin 等同 | 必须持有当前 Leader Assignment |
| Agent A | 第一任普通 Role 承担者 | 不得自行结束 Assignment |
| Agent B | Agent A 的继任者 | 必须获得新的 Assignment |
| Observer | 只读、实时订阅与撤权验证 | 不承担 Role |
| Supervisor A | 监督 Agent A/B 的 managed Runtime | 私钥不能进入 Agent 子进程 |
| Invalid/old Runtime | 重放旧 epoch、旧 Assignment 和迟到写入 | 所有角色写入必须被拒绝 |

## 5. 验收层级

| 层级 | 目的 | 主要入口 | 是否必选 |
|---|---|---|---|
| L0 领域与协议 | closed schema、状态机、Role/Work/Runtime 不变量 | `just project-view-test-unit` | 是 |
| L1 数据库与迁移 | 事务原子性、并发、membership、升级、schema drift | `project-view-test-db`、`test-migrations` | 是 |
| L2 Relay 与真实 CLI | WS/HTTP、签名 projection、live fan-out、cutover、Assignment | `project-view-test-e2e` | 是 |
| L3 Desktop | Tauri 签名边界、View/Inspector、live invalidation、Community 隔离 | Rust/JS tests、Playwright | 是 |
| L4 ACP 与 Runtime | Role context、动态 fence、binding、epoch、恢复与故障结束 | 确定性 ACP/runtime 编排 | 是 |
| L5 真实模型 smoke | 一个真实 Agent 读取 Brief、写 Checkpoint 并被 Desktop 看到 | 本地已配置模型 | 否 |
| L6 全仓回归 | 确认本里程碑未破坏其他 Buzz 能力 | `just ci` | 是 |

L0–L3 的现成测试不能替代 L4。特别是 Runtime supervisor adapter 的 unit/mock 测试已经
较完整，但当前仓库没有覆盖“真实 Relay + ACP harness + 子进程 + supervisor 故障注入”
的一键脚本；正式验收时需要在临时目录中编排这一条纵向路径。

## 6. 核心业务验收故事

以下故事是本轮验收的主线。所有后续负向测试都从这条主线的规范状态派生。

### E2E-01：Human 建立项目，Agent A 承担 Role

1. Human Owner 启用并初始化 Project View；
2. 创建 Project Profile、Goal、普通 Role、Issue 和 Work；
3. Human Desktop 与 Human CLI 读取相同 project revision 和对象集合；
4. Owner 向 Agent A 发出 Role offer；
5. Agent A 接受后获得唯一 active Assignment；
6. Desktop Role 卡显示 Agent A，CLI `roles current` 返回同一 Assignment；
7. Agent A 的下一次新 ACP session 收到完整 `[Role Brief]`；
8. Agent A 接受该 Role responsible Work，形成 active Commitment；
9. Desktop Work Inspector 显示同一 Commitment。

通过标准：

- 数据库、meta、entity head、NIP-43、CLI、ACP 和 Desktop 的 Role/Member/Assignment
  坐标一致；
- 普通 Role 对应 Community `member`；
- 整个流程没有客户端自行拼装未验证 Role 状态。

### E2E-02：Agent A 持续外化局势

1. Agent A 追加包含 progress、blocker、risk、open question、next step 和对象引用的
   Checkpoint；
2. Agent A 追加 context-only Handoff note，但不结束自己的 Assignment；
3. Desktop Role timeline 实时显示新 Checkpoint/Handoff；
4. CLI Role Brief 选择最新 Checkpoint，并保留最近 Handoff；
5. 再追加一个带 `supersedes_checkpoint_id` 的修正 Checkpoint。

通过标准：

- 历史记录 append-only，旧记录仍可分页读取；
- Handoff 不产生 Agent 自卸任；
- 规范引用不能跨 Project，不能引用不存在的对象；
- JSON、Markdown、ACP 和 Desktop 使用同一 verified assembler。

### E2E-03：Human 原子替换 Agent A 为 Agent B

1. Owner 或当前 Leader 向 Agent B 发出替换 offer；
2. Agent B 接受；
3. 同一个 Project transaction 结束 A 的 Assignment 与 Commitment，创建系统 Handoff，
   激活 B 的 Assignment，并更新 membership/projection/meta；
4. Desktop 只观察到最终一致状态，不出现双 active Assignment；
5. Agent B 读取到 A 的最新 Checkpoint、Handoff、responsible Work 和
   waiting-for-continuation；
6. Agent B 显式接受遗留 Work，形成自己的 Commitment；
7. Agent A 使用旧 Assignment ID、旧 runtime epoch 和迟到命令分别尝试写入。

通过标准：

- Role 与 Member 两个方向都始终满足 active Assignment 0..1；
- A 的所有旧角色写入被 Relay 拒绝；
- B 的写入只归因于 B 的新 Assignment；
- A 的历史 Assignment、Commitment 和贡献不被改写；
- Work status 不因 Assignment 替换自动改变。

### E2E-04：Leader 与 Community 权限一致

1. Owner 创建 admin-level Leader Role；
2. Human Leader 接受后成为 Community admin；
3. Leader 使用精确 Assignment fence 治理普通 Role；
4. Leader 尝试替换自己、同级 Leader，或使用 stale Assignment 治理；
5. Owner 结束 Leader Assignment；
6. 检查 Community role 同事务降回 `member`。

通过标准：

- 非 owner Community admin 当且仅当存在一个 active admin Role Assignment；
- Leader 不能用通用 member 设置旁路获得或保留 admin；
- owner 始终保持 owner；
- 权限、Assignment、NIP-43 snapshot 和 meta revision 原子一致。

## 7. 专项验收矩阵

### 7.1 Project View、投影与并发

| ID | 操作 | 预期结果 |
|---|---|---|
| PV-01 | 初始化未启用/已启用 Community | 未启用 fail closed；启用后一次原子初始化 |
| PV-02 | WS 与 `POST /events` 交替写入 | 都进入同一 revision/CAS/receipt 路径 |
| PV-03 | 同 revision 并发写入 | 恰有一个成功，另一个 conflict，不产生部分对象 |
| PV-04 | revision-pinned 分页期间发生新写入 | 旧页请求返回 conflict，客户端不得拼接快照 |
| PV-05 | 伪造 Relay signer 或未知 generation/schema | 客户端拒绝，不显示可写旧状态 |
| PV-06 | projection insert 或 membership snapshot 失败 | canonical、receipt、projection 全部回滚 |
| PV-07 | Observer 被撤销 membership 后保持旧订阅 | 不再收到 live/history Project View 数据 |
| PV-08 | v1→v2 cutover 与 generation reset | NIP-11 capability、所有 heads 和 meta 一致切换 |

### 7.2 Role、Assignment 与 Work

| ID | 操作 | 预期结果 |
|---|---|---|
| RC-01 | 一个 Role 同时指派 A/B | 数据库和 Relay 均拒绝双 active Assignment |
| RC-02 | 一个 Member 同时承担两个 Role | compound fence 拒绝或原子替换，不出现双任期 |
| RC-03 | Agent 自行 end/leave/remove/deactivate | 全部拒绝，Assignment 保持 active |
| RC-04 | 通用 member/admin 设置绕开 Leader | 拒绝并保持 NIP-43/Assignment 一致 |
| RC-05 | stale authorizer 完成 Proposal | Proposal 不被部分消费，Assignment 不激活 |
| RC-06 | active Assignment/Work 引用下停用 Role | 拒绝并返回稳定错误 |
| WORK-01 | 非 responsible Role 接受 Work | 拒绝 |
| WORK-02 | active Commitment 下直接改 responsible Role | 拒绝 |
| WORK-03 | Assignment 结束 | Commitment 原子结束，Work status 不变 |
| WORK-04 | B 接续 A 的 Work | 初始 waiting-for-continuation，B 显式接受后归因于 B |

### 7.3 Checkpoint、Handoff 与历史

| ID | 操作 | 预期结果 |
|---|---|---|
| CT-01 | active assignee 追加 Checkpoint | 成功并成为最新 Brief 入口 |
| CT-02 | ended Assignment 追加 Checkpoint | 拒绝 |
| CT-03 | 修改或删除旧 Checkpoint/Handoff | append-only trigger 拒绝 |
| CT-04 | 缺失、跨 Community 或错误 owner reference | 领域层或 deferred constraint 拒绝 |
| CT-05 | 没有 member-authored Handoff 的替换 | system Handoff + Project/Work/Checkpoint 仍可接续 |
| CT-06 | revision-pinned history keyset 分页 | newest-first、无重复、跨 Role cursor 拒绝 |

### 7.4 ACP Role context 与增量刷新

| ID | 操作 | 预期结果 |
|---|---|---|
| ACP-01 | candidate 启动 | Brief 为 candidate，不允许 role-bearing 写入 |
| ACP-02 | assigned Agent 创建新 session | 注入完整 `[Role Brief]` |
| ACP-03 | 同 session、meta 完全未变化 | 只注入 `[Role Binding]`，不重读完整 heads/membership |
| ACP-04 | Role/Assignment/Work/Checkpoint/membership 改变 | 下一完整 turn 重读并注入完整 Brief |
| ACP-05 | meta 或 Relay identity 读取失败 | 注入 unavailable，不复用旧 Binding |
| ACP-06 | session rotate/rebuild | 即使 meta 未变化也重新注入完整 Brief |
| ACP-07 | native steer 期间 Assignment 改变 | steer 不伪装成新授权；写入由最新 Relay fence 拒绝 |
| ACP-08 | Community 切换 | cache、session 和 Assignment 不跨 Relay/Project 泄漏 |

证据组合：

- 真实 Relay 环境检查 observer frame 中的 `mode`、Assignment、revision 和 meta event；
- stage 10 的 HTTP spy 测试核对实际 query 次数；
- Relay/CLI 负向写入证明 cache 从未成为授权事实源。

### 7.5 Runtime supervisor 与故障恢复

| ID | 操作 | 预期结果 |
|---|---|---|
| RT-01 | 有 binding 的 Assignment 启动 ACP | Agent 子进程启动前取得 epoch 并发布 fence |
| RT-02 | 检查 Agent/MCP 环境 | 只有 fence path，无 supervisor 私钥或恢复状态路径 |
| RT-03 | 同一健康 Runtime 续租 | runtime ID/epoch 不变化 |
| RT-04 | revoke binding | 下一完整 turn 暂停续租、删除 fence、Role context unavailable/受限 |
| RT-05 | 重新注册 binding | 无需重启 ACP，启动/恢复新的可信 Runtime fence |
| RT-06 | Assignment A→B | 旧 pair 停止，新 Assignment 使用新 runtime coordinate |
| RT-07 | 杀死受监督 harness 并在窗口内重启 | 同一 Assignment 进入 recovering 后恢复，不卸任 |
| RT-08 | 重放旧 epoch 或结束后的 fence | Relay 拒绝所有 role-bearing 写入 |
| RT-09 | 单纯 lease 过期、presence 离线或普通断线 | 只影响 availability，不自动结束 Assignment |
| RT-10 | recovery 有限重试全部失败 | 满足全部 fail-closed 条件后才执行 `unrecoverable` |
| RT-11 | monitor 不健康或另一 Runtime 仍健康 | 不得自动结束 Assignment |
| RT-12 | 自动结束成功 | 一个 system change 原子结束 Assignment/Commitment、生成 Handoff、同步 membership |

RT-10 至 RT-12 只在临时 Community 中显式开启自动结束和短测试窗口。不得通过缩短生产默认
策略或修改正式代码来让测试更快。

## 8. Desktop 验收

Desktop 采用三类证据，避免把 mock UI 等同于真实后端：

### 8.1 React/Playwright 交互

运行 `project-view.spec.ts` 的全部场景，至少覆盖：

- verified canonical map 与 Inspector；
- v2 Role/Assignment、Work/Commitment、Checkpoint/Handoff；
- owner 指派、offer、Human/Agent 交替写入；
- concurrent replacement 后刷新但不重放旧 intent；
- loading、invalid snapshot、trusted-read failure 和 stale state；
- revision live invalidation；
- Community 切换不泄漏 View、draft、selection 或 Assignment。

### 8.2 Tauri native boundary

验证：

- Desktop 只提交 revision-fenced intent；
- Tauri 使用 SDK builder 和当前 Human key 签名；
- Relay signer、meta、membership、object/entity heads 都经过共享 parser；
- conflict 后返回新状态，不自动重放旧 intent；
- Community 切换清空 community-scoped singleton/cache。

### 8.3 真实 Relay smoke

至少执行一次实际 Desktop dev build 连接临时 Relay：

1. 打开 `View`；
2. 读取由真实 CLI 初始化的 Project；
3. Human 发起一次 Role offer 或 Work responsibility 修改；
4. Agent CLI 读取同一 revision；
5. Agent 追加 Checkpoint；
6. Desktop live 显示该 Checkpoint。

如果当前自动化无法驱动真实 Tauri webview，这一项必须在报告中标记为“人工/半自动 smoke”，
不能用 mock Playwright 结果冒充。

## 9. 执行顺序

### 阶段 A：冻结基线与环境预检

记录：

- Git commit SHA、branch 和 `git status`；
- Rust/Node/pnpm/Docker/Postgres/Redis 版本；
- 已占用端口；
- 执行前已经存在的 `buzz_pv_*` 数据库；
- 是否存在外部 `BUZZ_AUTH_TAG`、`BUZZ_RELAY_URL`、`BUZZ_PRIVATE_KEY`；
- 本次 run ID、临时目录、数据库、Community host 和端口。

外部 Buzz 环境变量必须从测试进程中显式移除，不能误连 staging。

### 阶段 B：现有自动化门禁

```bash
. ./bin/activate-hermit
just project-view-test
cargo test -p buzz-acp --lib
cargo test -p buzz-cli --lib
just desktop-test
just desktop-tauri-test
pnpm -C desktop build:e2e
cd desktop
pnpm exec playwright test --project=smoke tests/e2e/project-view.spec.ts
```

其中 `just project-view-test` 必须完整通过：

1. domain/protocol/SDK/Relay/CLI unit contract；
2. PostgreSQL Project View/Role/Runtime transaction tests；
3. fresh/upgrade/concurrent migrations；
4. checked-in schema drift；
5. 独立 Relay + 真实 buzz CLI E2E。

### 阶段 C：Human–Agent 黄金路径

在一个新的临时 Community 中依次执行 E2E-01 至 E2E-04，保存每一步：

- command/event ID；
- expected/actual project revision；
- Assignment、Commitment、Checkpoint 和 Handoff ID；
- meta 与 membership event ID；
- CLI JSON；
- Desktop/observer 证据；
- 关键数据库断言。

### 阶段 D：ACP/Runtime 编排

使用确定性 fake ACP child：

1. 捕获 `session/new` 与每次 `session/prompt`；
2. 通过临时 MCP/CLI 驱动受控写入；
3. 记录 `[Role Brief]`、`[Role Binding]` 和 unavailable；
4. 检查 Runtime fence 文件内容、权限、创建/替换/删除时机；
5. 执行 RT-01 至 RT-12 的 binding、epoch、kill/restart 和恢复耗尽故障注入；
6. 检查 supervisor 私钥未出现在子进程环境、observer frame 或日志中。

### 阶段 E：最终回归

专项全部通过后执行：

```bash
just ci
```

若 `just ci` 因与本功能无关且可重复确认的宿主环境问题失败，报告可以给出“有条件通过”；
任何代码回归、数据不变量或安全边界失败都不能降级处理。

### 阶段 F：清理与报告

- 停止本次启动的 Relay、ACP、fake Agent 和 supervisor；
- 只删除本次 run 的临时数据库、Redis namespace、临时文件和私钥；
- 确认共享 Docker 服务和原有数据库未被重置；
- 再次执行 `git status --short`；
- 输出 `integration-acceptance-report.md`，不把密钥或完整认证 header 写入报告。

## 10. 失败分级与重试规则

| 等级 | 定义 | 发布影响 |
|---|---|---|
| P0 | 越权、跨 Community 泄漏、双 active Assignment、旧 Runtime 可写、部分事务提交 | 立即不通过 |
| P1 | 必选主流程、恢复、Role Brief fail-closed、migration 或跨端一致性失败 | 不通过 |
| P2 | 不破坏规范状态的 UI/可观测性/错误文案问题 | 可评估有条件通过 |
| ENV | Docker、模型供应商、系统权限等环境阻塞 | 不算实现通过，标记未验证 |

重试规则：

- 测试失败后先保存日志、数据库状态和 event IDs，再决定是否重跑；
- 不允许无证据地反复重跑直到变绿；
- 同一测试第二次出现时序性失败，按 flaky 缺陷记录，不视为通过；
- 环境问题修复后最多进行一次完整重跑；
- 验收过程中不直接修改生产代码。若必须修复，结束本轮、提交独立修复，再从新的 commit
  重新执行受影响门禁。

## 11. 发布通过标准

同时满足以下条件才判定“角色连续性集成验收通过”：

1. L0、L1、L2、L3、L4 和 L6 全部完成；
2. E2E-01 至 E2E-04 全部通过；
3. PV、RC、WORK、CT、ACP、RT 的必选场景无 P0/P1；
4. Assignment、membership、projection 和 Runtime fence 的负向测试均 fail closed；
5. Agent A→B 后，A 无法写，B 可以从 Project-owned state 接续；
6. Desktop 与 Agent CLI 对同一 Project revision 的 Role/Work/continuity 展示一致；
7. session/Community/Runtime 变化不泄漏旧 Role context；
8. migration-built schema 与 checked-in schema 的 Project View/Role/Runtime 对象无 drift；
9. 最终 `just ci` 通过，或只有书面确认的 ENV/P2 限制；
10. 临时资源完成清理，仓库没有非预期修改。

真实外部 LLM smoke 未执行不阻止通过，但必须在报告中明确标记。确定性 ACP/Runtime L4
未执行则不能判定通过。

## 12. 验收报告格式

正式执行后在同目录输出 `integration-acceptance-report.md`，至少包含：

```text
Run ID / commit / 时间 / 环境
总体结论：通过 | 有条件通过 | 不通过

门禁结果
  L0 ... L6

黄金路径
  E2E-01 ... E2E-04

专项矩阵
  PASS | FAIL | SKIPPED | ENV

关键证据
  revision / event / Assignment / Runtime 坐标

失败与限制
  P0 / P1 / P2 / ENV

清理结果
  processes / databases / temp files / git status
```

长日志、Playwright trace、截图和临时 JSON 放在本地 `test-results` 下，报告只引用相对路径
和必要摘要。任何私钥、NIP-98 header、supervisor secret 或可复用 token 都必须脱敏。
