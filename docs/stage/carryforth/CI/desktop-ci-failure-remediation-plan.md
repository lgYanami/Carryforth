# Carryforth Desktop CI 失败修复方案

> 状态：修复已实施并完成本地验证；等待一次完整 GitHub push CI 验收
> 日期：2026-08-14
> 范围：公开源码仓库 `main` 上的 Desktop Smoke E2E、Desktop Relay-backed E2E、Windows Rust 与聚合门
> 现场基线：[`CI run 31764144779`](https://github.com/lgYanami/Carryforth/actions/runs/31764144779)，提交 `c3fbce2070d9880debfbbe9f58056fc5f4bc1abe`
> 数据边界：本方案不访问或修改 Live Community、数据库、Desktop 身份、keyring、Agent 状态或用户内容

## 1. 结论

本轮 CI 失败不是同一个缺陷，也不是此前 CI 修复全部失效。已经完成的 source gate、依赖安全修复和
macOS Mesh-LLM 构建修复在本轮均已通过；新的失败分为五类：

1. Desktop E2E 仍断言旧 Buzz 文案或旧 UI 名称；
2. 两个测试仍假设 Desktop 支持多个可切换 Community，与当前 local-only 产品合同冲突；
3. 三个侧栏几何断言需要根据失败截图判断是测试滚动状态不确定，还是实际布局回归；
4. 新公开仓库没有 Relay artifact cache，冷构建超过现有 40 分钟上限，导致集成测试没有运行；
5. Windows Clippy 编译到了只供 Unix 测试使用的 helper/import，产生 `-D warnings` 失败。

因此不删除 `Desktop`、`Desktop Smoke E2E`、`Desktop E2E Integration` 或 Windows 门禁。只退役两个已经
不成立的“跨 Community 切换”测试片段，并以当前 local-only 合同测试替代。其余项目应修复或更新。

```text
当前失败
├── 确定性断言漂移              → 更新测试
├── 旧多 Community 产品假设      → 删除旧场景，补 local-only 替代覆盖
├── 布局几何证据尚未判定          → 先看 artifact，再修 UI 或测试前置状态
├── 冷构建 / shard 超时           → 修缓存边界与有界 timeout
└── Windows cfg 不完整            → 修条件编译
```

## 2. 本轮 CI 事实

### 2.1 已通过的门

以下门在同一提交上通过，不能把本次结论描述为“Desktop 或仓库整体无法构建”：

- Dead Token Reference Guard；
- Detect Changed Paths 及公开源码合同检查；
- Rust Lint；
- Unit Tests；
- Project Context Semantic Foundation and Query；
- Web；
- Desktop Core，包括 lint、unit、build、Tauri clippy/check/test；
- Security；
- Desktop Build (macOS)；
- 两个 Linux musl Server Cross-Compile。

### 2.2 用户看到的五个失败项

| CI 项 | 真实状态 | 直接原因 |
| --- | --- | --- |
| `CI / Desktop` | 聚合任务运行约 3 秒后失败 | `Desktop Core` 成功，但 Smoke matrix 不是全成功 |
| `CI / Desktop E2E Integration` | 聚合任务运行约 2 秒后失败 | 上游 Relay artifact job 被取消，两个 integration shard 均未运行而是 `skipped` |
| `Desktop Smoke E2E (1)` | 194 passed / 6 failed / 1 flaky / 2 skipped | 旧品牌断言与侧栏几何断言 |
| `Desktop Smoke E2E (2)` | 215 passed / 5 failed | 旧品牌/运行时名称断言与旧多 Community Meeting 场景 |
| `Desktop Smoke E2E (3)` | 191 passed / 3 failed / 2 flaky / 1 skipped | 旧品牌、旧 onboarding 终点与旧多 Community Document 场景 |

另有两项必须纳入修复范围：

- `Desktop Smoke E2E (4)` 在测试步骤运行约 19 分钟后触及 20 分钟 job 上限，被取消；没有形成最终测试结论；
- `Windows Rust (x86_64-pc-windows-msvc)` 因两个条件编译 warning 被 `-D warnings` 拒绝。

### 2.3 Integration 并没有执行失败

[`Desktop E2E Relay`](https://github.com/lgYanami/Carryforth/actions/runs/31764144779/job/94656722986)
在空 cache 下执行 Relay binaries 与 nextest archive 的冷构建。40 分钟到期时日志仍处于正常 Rust 编译过程，
没有编译器错误；job 被超时取消后：

- 没有上传 `desktop-e2e-relay` artifact；
- 两个 Desktop integration shard 没有开始；
- Backend Integration、Relay E2E、Project View / Document / Context integration 也随依赖链被跳过；
- 最终聚合门按 fail-closed 规则将 `skipped` 判为失败。

聚合门的失败语义是正确的：没有运行不能冒充通过。需要修复的是 artifact 冷构建容量和诊断信息，而不是删除聚合门。

## 3. 根因与处置决定

### 3.1 旧产品文案断言

当前产品源码、相邻 unit tests 与 E2E fixture 已一致使用新文案，但下列 E2E 仍保留旧断言：

| 测试 | 旧断言 | 当前产品合同 | 决定 |
| --- | --- | --- | --- |
| `agent-readiness-screenshots.spec.ts` | `Buzz shared compute` | `Carryforth shared compute` | 更新断言与测试标题 |
| `config-bridge-screenshots.spec.ts` | `Set in Buzz` | `Set in Carryforth` | 更新两处断言 |
| `doctor-states.spec.ts` | `Older Buzz releases...` | `Older Carryforth releases...` | 更新断言 |
| `global-agent-config-screenshots.spec.ts` | `Carryforth Agent` | 内置 runtime 在 UI 中规范显示为 `Built-in Agent` | 更新可见名称断言；保留 raw runtime ID 兼容值 |
| `mesh-compute.spec.ts` | `Buzz downloads...` | `Carryforth downloads...` | 更新断言 |

不得为了让旧测试通过而把当前 UI 改回 Buzz 品牌，也不得机械重命名 `buzz-agent` 等兼容 runtime ID。

### 3.2 已失效的多 Community 测试假设

当前 Desktop 启动时通过 `resolveLocalOnlyCommunityState()` 保留一个 canonical `Local Dev`，并通过
`projectConnectableCommunities()` 阻止远程或重复记录成为可连接 Community。下列测试却同时 seed
`ws://localhost:3000` 和 `ws://localhost:3001`，随后等待第二个 rail button：

- `meeting-recovery.spec.ts` 中的 `community-rail-button-meeting-b`；
- `project-document.spec.ts` 中的 `community-rail-button-documents-b`。

这不是产品回归。处置方式为：

1. 删除两个测试中“切到 B、再切回 A”的旧产品场景；
2. 保留 Meeting recovery 和 Project Document 的本地恢复主路径；
3. 在 Community storage/bootstrap 单元或 E2E 中补充替代断言：重复/远程记录会被清理，只有 canonical Local Dev 可连接；
4. 验证被清理记录不会把 Meeting/Document 状态泄漏到当前本地 Community；
5. 不恢复远程 Community UI，不为测试制造隐藏开关。

这里退役的是两个无效场景，不是 Meeting、Document 或 Community 隔离测试面。

### 3.3 Onboarding 终点漂移

`onboarding-agent-defaults.spec.ts` 在点击 Finish 后等待已经不存在的
`Join or create a community`。当前 local-only 启动会预先建立 canonical Local Dev，因此旧页面不再是合法终点。

测试应改为验证稳定状态，而不是另一个容易漂移的标题字符串：

- onboarding completion marker 已持久化；
- 选择的 `codex` runtime 已保存；
- 应用进入当前主 shell；
- canonical Local Dev 可用；
- 页面没有重新进入 onboarding 循环。

### 3.4 侧栏几何断言

`buzz-theme-screenshots.spec.ts` 的三项失败为：

- light/dark 两种状态下，primary menu 与 pinned search 的预期间隔为 `8`，实际为 `-206`；
- settings 状态下，back-to-app 与 search 的预期 Y 对齐，实际相差 `108`。

不能直接把期望值改成 `-206` 或 `108`。实施时必须先下载并查看本轮 Playwright screenshot/trace：

1. 确认失败前是否因选中 channel 的 `scrollIntoView` 让 sidebar scroll container 留在非零位置；
2. 若是 fixture 状态不确定，在测量前显式恢复正确 scroll position、等待订阅与动画完成；
3. 若 fresh state 仍发生重叠、遮挡或错误 inset，修复 UI layout；
4. 几何合同应表达“可见、不重叠、属于正确容器、保持设计间距”，不接受任意放大 tolerance；
5. light/dark 必须分别保留，不能只删掉失败主题。

### 3.5 Windows 条件编译

`crates/buzz-dev-mcp/src/meeting_read.rs` 中：

- `tempdir` import 只被 `#[cfg(unix)]` 的 fake-binary tests 使用；
- `make_state()` 也只被这些 Unix tests 调用。

修复为对 import、`TempDir` 和 helper 使用一致的 `#[cfg(unix)]`。生产代码、Windows 功能与测试语义均无需删除。
Windows 当前不是稳定发行承诺，但保留跨平台编译门可以阻止无意的 cfg 腐化。

## 4. CI 冷启动与超时设计

### 4.1 Relay artifact 冷构建

新仓库首次运行没有 Actions cache，必须把冷构建视为受支持路径。当前 cache key 还包含整份
`.github/workflows/ci.yml`；任何无关 workflow 文案或 timeout 修改都会使昂贵 artifact cache 失效。

实施方案：

1. 把 Relay binaries 与 nextest archive 的构建命令提取到一个受版本控制的专用脚本；
2. cache key 哈希实际构建输入、该脚本与一个显式 cache schema version，不再哈希整份 workflow；
3. 修改 artifact 内容或构建命令时显式升级 schema version；
4. 将 `Desktop E2E Relay` job 上限暂定为 75 分钟，build step 上限低于 job 上限，为日志和清理保留时间；
5. 首次空 cache 必须能独立成功；后续 cache hit 只用于加速，不能成为正确性的前置条件；
6. 保持 artifact 缺失时下游 fail closed，不允许回退到陈旧或未验证 binary。

75 分钟是首次修复的采集上限，不是永久性能目标。获得一次真实冷构建数据后，应记录各阶段耗时并冻结更合理的上限。

### 4.2 Smoke shard 超时

Shard 4 的 job-level timeout 会直接取消后续 summary/artifact upload，导致无法知道是慢测试还是 hang。

实施方案：

1. job 上限暂定为 35 分钟；
2. Playwright test step 使用更短的有界上限，例如 28–30 分钟；
3. test step 超时后让 job 进入普通 failure，使 summary、trace 与 screenshot upload 仍有机会运行；
4. 第一次取得 shard 4 的完整失败证据后，按真实耗时重新平衡 shard 或拆分重测试；
5. 不用无限延长 timeout 掩盖 hang；稳定后每个 shard 的正常耗时应不超过 job 上限的约 70%。

### 4.3 聚合门诊断

保留 `Desktop` 和 `Desktop E2E Integration` 的 fail-closed 行为，但增强输出：

- Desktop 聚合门继续分别报告 Core 与 Smoke matrix 结果；
- Integration 聚合门同时报告 `desktop-e2e-relay` 与 integration shards 的结果；
- `cancelled`、`skipped`、`failure` 必须明确区分；
- 聚合门不得把上游未运行映射为 success。

## 5. 实施阶段

### Phase A：确定性、低风险修复

1. 更新五组旧品牌/显示名称断言；
2. 将 onboarding 测试改为稳定状态断言；
3. 修复 Windows test-only cfg；
4. 删除两个跨 Community 切换片段并补 local-only 替代覆盖；
5. 运行定向 Desktop/unit/Windows 可用静态检查。

这一阶段不修改产品行为。

### Phase B：布局证据判定

1. 下载 shard 1 artifacts；
2. 对 light、dark、settings 三个失败状态逐张检查；
3. 根据证据选择“稳定测试滚动前置”或“修复布局”；
4. 重新生成或确认 screenshot evidence；
5. 保留非重叠与 inset 合同。

如果证据无法判断，停止并记录，不通过放宽断言强行转绿。

### Phase C：CI 容量和可观测性

1. 提取 Relay artifact 构建脚本与稳定 cache schema；
2. 调整 Relay cold-build 与 Smoke step/job timeout；
3. 确保失败时上传已有 Playwright 证据；
4. 增强 integration aggregator 的上游结果输出；
5. 验证 workflow 没有降低任何 required gate。

### Phase D：一次完整 push 验收

按一个完整 push 运行观察：

1. Smoke 1–4 全部实际运行；
2. Relay artifact 在空 cache 或明确 cache miss 下完成；
3. 两个 Desktop integration shards 实际运行，而不是 `skipped`；
4. downstream Backend/Relay/Project integration 正常展开；
5. Windows job 完整走过 clippy/check/test；
6. 两个聚合门最终为 success。

若这次完整 CI 仍失败，停止自动推进，按新的首个失败证据向维护者报告；不得连续盲目重跑或继续删除测试。

## 6. 文件改动矩阵

| 文件/区域 | 计划改动 |
| --- | --- |
| `.github/workflows/ci.yml` | cold-build/Smoke 有界 timeout、artifact cache key、上传与聚合诊断 |
| `scripts/` 下新的 CI artifact 构建脚本 | 固定 Relay binaries 与 nextest archive 的真实构建合同 |
| `desktop/tests/e2e/agent-readiness-screenshots.spec.ts` | Carryforth shared compute 断言 |
| `desktop/tests/e2e/config-bridge-screenshots.spec.ts` | Set in Carryforth 断言 |
| `desktop/tests/e2e/doctor-states.spec.ts` | Older Carryforth releases 断言 |
| `desktop/tests/e2e/global-agent-config-screenshots.spec.ts` | Built-in Agent 可见名称合同 |
| `desktop/tests/e2e/mesh-compute.spec.ts` | Carryforth downloads 文案 |
| `desktop/tests/e2e/onboarding-agent-defaults.spec.ts` | 当前 local-only completion 状态 |
| `desktop/tests/e2e/meeting-recovery.spec.ts` | 移除跨 Community 片段，保留本地恢复 |
| `desktop/tests/e2e/project-document.spec.ts` | 移除跨 Community 片段，保留本地 Document 行为 |
| Community storage/bootstrap tests | 补唯一 canonical Local Dev 与旧记录清理合同 |
| `desktop/tests/e2e/buzz-theme-screenshots.spec.ts` 或对应 UI | 仅按 artifact 证据修滚动前置或真实布局 |
| `crates/buzz-dev-mcp/src/meeting_read.rs` | Unix-only test helper/import 的 cfg 对齐 |

不计划修改数据库、migration、Relay 协议、Project View、Document、Context、Meeting 领域数据或语义检索实现。

## 7. 验证矩阵

### 7.1 本地定向检查

按实际改动运行：

```bash
. ./bin/activate-hermit
just desktop-check
just desktop-test
pnpm -C desktop build:e2e
```

对变更的 Playwright spec 先做定向运行，再运行完整 smoke project。Tauri 不在根 Cargo workspace；若改到 Tauri
代码，必须另跑 `just desktop-tauri-check` / `just desktop-tauri-test`。

Windows cfg 至少执行 formatter、Linux clippy 对应 crate，并由 GitHub Windows job 完成真实 target 验证。不得用
Linux 上的成功推断 Windows 已通过。

### 7.2 CI 必须证明的内容

- Smoke 1–4：0 deterministic failure；
- 新增 flaky 项为 0，既有 flaky 不因本修复扩散；
- Shard 4 有明确最终结果和可下载 artifact；
- Relay artifact cold path 成功，并上传非空的四个 binary/archive 目标；
- Desktop integration 两个 shard 都实际执行并成功；
- Windows Rust 成功；
- `Desktop` 与 `Desktop E2E Integration` 聚合门成功；
- 其他本轮已通过的门不回退。

## 8. 删除与保留判定

| 项目 | 判定 |
| --- | --- |
| 整个 Desktop Smoke E2E | 保留 |
| 整个 Desktop E2E Integration | 保留 |
| Desktop / Integration 聚合门 | 保留并增强诊断 |
| Windows Rust | 保留并修 cfg |
| 品牌/名称断言 | 更新，不删除 |
| 侧栏 geometry contract | 保留；证据驱动修复 |
| Meeting 跨 Community 切换片段 | 删除，并补 local-only 替代覆盖 |
| Document 跨 Community 切换片段 | 删除，并补 local-only 替代覆盖 |
| 因 timeout 未形成结论的 Shard 4 | 不删除；先取得完整证据 |

## 9. 完成标准

本方案只有同时满足以下条件才可标记完成：

1. 所有确定性旧文案、旧终点和旧多 Community 假设已处理；
2. 布局失败已经由 screenshot/trace 证明并关闭，而不是仅放宽数字；
3. Windows cfg warning 消失；
4. 新仓库无 cache 的 Relay artifact 路径至少成功一次；
5. Smoke 4 与两个 integration shards 都实际执行并成功；
6. 两个聚合门保持 fail closed 且最终成功；
7. 没有通过删除整个测试套件、取消 required job 或把 failure 映射成 success 达标；
8. 没有为恢复已退役的多 Community 产品面而修改 Desktop；
9. 若完整复跑仍失败，已经停止并向维护者报告新的第一失败原因。

“只把 timeout 调大但仍没有测试结论”“只删除失败断言”“集成 shard 继续 skipped”都不构成完成。

## 10. 实施记录

### 10.1 2026-08-14 本地实施

本方案随同一修复提交落地，未修改 Desktop 产品行为、数据库、Relay 协议或项目数据：

- 将遗留 Buzz 文案、内置 Runtime 显示名和 onboarding 终点断言更新为当前 Carryforth / local-only 合同；
- 用“远程 Community 记录被清理且状态不泄漏”替代两个已经失效的跨 Community 切换场景；
- 根据失败截图确认侧栏异常值来自独立滚动容器的非零 `scrollTop`，测量前恢复顶部并等待稳定；
- 删除 Settings 返回按钮与旧 Search 位置之间已经不存在的跨视图对齐假设，保留可见性和内容面 inset 合同；
- 为 Unix-only Meeting 测试 import/helper 补齐条件编译；
- 将 Relay artifact 构建提取为受版本控制的脚本，以构建输入和显式 schema 形成 cache key；
- 为 Relay cold build 与 Smoke test step/job 设置分层有界超时，并让 integration 聚合门显式报告 artifact job 结果。

本地验证结果：

- 受影响行为的 17 条定向 Playwright E2E 全部通过；首次运行暴露的 3 条过度更新断言已纠正并复测通过；
- `just desktop-check`：通过；
- `just desktop-test`：3707 passed，0 failed；
- `cargo clippy --locked -p buzz-dev-mcp --tests -- -D warnings`：通过；
- `bash -n scripts/build-ci-relay-artifacts.sh`：通过；
- workflow YAML 解析、`cargo fmt --all` 与 `git diff --check`：通过。

本地没有冒充完成以下 GitHub runner 证据：真实 Windows target、四个完整 Smoke shard、空 cache 的 Relay artifact
冷构建、两个 Relay-backed Desktop integration shard，以及后续聚合门。这些只由本次提交触发的一次完整 push CI
验收；若仍失败，按 Phase D 停止继续修改并记录首个新失败原因。
