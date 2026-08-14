# Carryforth CI 完整复核与一次性修复方案

> 状态：**本地实施完成并通过；等待一次远端 cold-path CI 验收**
> 日期：2026-08-14
> 范围：GitHub source contracts、共享 Relay 测试产物、Desktop Smoke / Integration、Windows Rust、路径过滤、聚合门与失败证据
> 远端证据：[`run 31764144779`](https://github.com/lgYanami/Carryforth/actions/runs/31764144779) 与 [`run 31770286297`](https://github.com/lgYanami/Carryforth/actions/runs/31770286297)
> 当前代码基线：`7e53094c1c1f674c650dfb996fcee16574926239`
> 数据边界：本方案不访问或修改 Live Community、数据库、Desktop 身份、keyring、Agent、Project 数据或语义索引

## 1. 结论

修复前的 CI 不能通过，也不能再用“修一个报错、推一次、再看下一个”的方式处理。完整复核确认问题分布在五层；Phase A–D 现已一次性实施并通过本地组合门，尚待一次远端 cold-path CI 验收：

1. **静态合同所有权漂移**：Relay 测试目标已经从 workflow 移到构建脚本，但 Project View / Document 合同仍检查旧位置；
2. **Desktop 测试仍携带旧产品假设**：部分断言仍使用旧显示值，部分场景仍假设可切换远程 Community；
3. **存在真实测试竞态和有效行为失败**：侧栏拖拽排序需要稳定的 pointer/collision seam；视频菜单和长历史滚动场景的旧 E2E 驱动不再作为当前 source-only 门，已按维护者决定暂停并保留待重建场景；
4. **共享产物、cache 与 job graph 不闭合**：cache hit 不验证完整性，部分消费者仍重复冷编译，专项 PR 可能因 producer 被跳过而假绿；
5. **Windows、路径过滤、超时和可观测性仍有盲区**：下一轮会继续暴露确定性失败，Playwright flaky summary 当前实际上没有输入文件。

因此，本方案已经一次性修正合同、其余有效测试、共享构建和 job graph；下一步只发起**一次完整 push CI**。如果该轮仍失败，立即停止，报告新的第一失败证据，不继续盲修或连续重跑。

```text
当前阻断链
│
├── Detect Changed Paths
│   ├── Project View contract 检查旧位置（当前已触发）
│   └── Project Document contract 同样检查旧位置（被前者遮住）
│
├── Desktop Smoke
│   ├── Project View 两条显示值漂移
│   ├── Project View 两条旧跨 Community 场景
│   ├── virtualization 拖拽排序未提交
│   └── video 第二播放器同步竞态
│
├── Desktop Integration
│   └── Profile 仍执行旧跨 Community round trip
│
├── Shared Relay artifacts
│   ├── cold path 尚未成功证明
│   ├── cache hit 不校验五项输出
│   └── Relay E2E 下载 archive 后仍重复冷编译三项测试
│
└── Windows / workflow graph
    ├── Unix-only Shim import 在 Windows 变成 warning
    ├── filters 与真实构建输入不完整
    └── producer / consumer / aggregate 条件存在 skipped 假绿风险
```

## 2. 证据时间线

### 2.1 首轮公开仓库 CI：run 31764144779

提交 `c3fbce2070d9880debfbbe9f58056fc5f4bc1abe` 上：

- Rust lint、unit、Web、Desktop Core、Security、macOS Desktop build 与 Linux musl cross-compile 等门已通过；
- Desktop Smoke shards 1、2、3 有确定性断言失败；
- shard 4 在旧 20 分钟 job 上限到达时被取消，没有形成完整结论；
- Windows Rust 因 Unix-only 测试代码的条件编译不完整而失败；
- `Desktop E2E Relay` 在空 cache 下仍正常编译，但 40 分钟到期被取消；
- 两个 Desktop Integration shard 及后续 Backend / Relay / Project 集成因缺少共享产物而 `skipped`；
- 聚合门正确地把 `skipped` 判为失败，没有制造假绿。

提交 `7e53094c1` 已处理了其中一部分旧文案、onboarding、sidebar geometry、Meeting / Document local-only 场景、部分 Unix cfg，并提取了共享构建脚本。但本地验证没有覆盖 GitHub `changes` job 的全部静态合同，也没有完整运行 shard 4。

### 2.2 第二轮 CI：run 31770286297

提交 `7e53094c1c1f674c650dfb996fcee16574926239` 上，CI 在 `Detect Changed Paths` 阶段提前失败：

- `scripts/test-project-view-release-contract.sh` 仍要求 `--test e2e_project_view` 直接存在于 `.github/workflows/ci.yml`；
- 真实 owner 已经是 `scripts/build-ci-relay-artifacts.sh`；
- 因该 step 失败，Project Document contract 和全部下游任务都没有执行；
- 用户看到的 `Desktop` 与 `Desktop E2E Integration` 约 2–3 秒失败只是聚合门对上游未运行的 fail-closed 结果，不是新的 Desktop 运行时故障。

独立执行被遮住的 Project Document contract 已确认它也会失败：

- `e2e_project_document_disabled`；
- `e2e_project_document_enabled`。

两者同样已经迁入构建脚本，合同却仍检查 workflow 内联文本。只修 Project View 一处，下一次 CI 会立即在 Project Document 失败。

### 2.3 当前基线的完整本地 shard 4

在当前基线完成一次完整、单 shard 诊断：

```text
183 passed
5 failed
2 flaky
耗时约 11.9 分钟
```

五条确定性失败为：

| 测试 | 证据 | 判定 |
| --- | --- | --- |
| Project View overview | 期望 `E2E Test`，实际 canonical 名为 `Local Dev` | 更新显示值断言，保留 overview 行为覆盖 |
| Project View preview-disabled shell | 同样期望 `E2E Test`，实际为 `Local Dev` | 更新显示值断言，保留 disabled-preview 覆盖 |
| Project View data isolation | 等待已被 local-only bootstrap 清除的 Community B rail | 用 local-only 清理与不泄漏合同替代旧切换场景 |
| Project View Assignment isolation | 同样等待不存在的 Community B rail | 用 local-only cache/reset 合同替代旧切换场景 |
| virtualization custom-section DnD | drag 后顺序仍为 `[Priority, Archive]` | 有效行为合同；先修稳定手势，必要时修产品 DnD |

本次完整 shard 4 中两条 retry 后通过的 flaky 为：

- Relay reconnect 首次未取得 relay state seam；
- thread focus 首次未把目标 anchor 带入 viewport。

它们不是当前确定性 blocker，但必须进入可见的 flaky 证据；不能让 retry 把它们从报告中消失。

### 2.4 被现有失败遮住的后续问题

完整复核还确认：

- `profile.spec.ts` 仍执行 `Alpha → Bravo → Alpha` Community round trip；local-only bootstrap 会删除 Bravo，当前本地可稳定复现 30 秒等待失败；
- 视频右键菜单测试存在竞态：第二条视频发送后立即用 `.last().toBeVisible()` 等待，但第一条播放器已经满足条件。第二条异步渲染会令右键目标或菜单失效；本地 repeat 5 次得到 4 pass / 1 fail，旧 CI 为 3/3 fail；
- Windows 上 `use crate::shim::Shim;` 仍未和 Unix-only helper 一起加条件编译，会在 Clippy `-D warnings` 下成为下一条确定性失败；
- 当前 Playwright 配置没有 JSON reporter，但 workflow 读取 `playwright-report.json`；summarizer 对文件不存在静默成功，所以 flaky 汇总实际上一直没有工作。

## 3. 必须保持的修复原则

1. 不删除 Desktop、Smoke、Integration、Windows job 或聚合门来换取绿色状态；维护者明确判断当前不影响功能、但测试驱动不可靠的单个 E2E 场景，可以暂停并记录覆盖缺口；
2. 聚合门继续 fail closed：`failure`、`cancelled`、`skipped` 都不能冒充成功；
3. 当前产品是 local-only Desktop，不恢复已经退役的远程 Community 切换 UI；
4. 对已失效的跨 Community E2E，补充当前合同的替代覆盖，但不得声称两者语义完全等价；
5. DnD 与 Profile 持久化仍保留有效最终结果；视频菜单与长历史滚动的旧场景本轮暂停，不能冒充已修复或已有等价覆盖；
6. cache 只用于加速，cache miss 必须能够从零成功，cache hit 必须重新验证完整性；
7. 构建目标、产物清单与 source contracts 只保留一个 canonical owner；
8. timeout 必须分为 test step 与 job 上限，让失败后仍有时间生成 summary 和上传证据；
9. retry 只能收集 flaky 证据，不能被描述为问题已修复；暂停执行的场景必须明确出现在实施记录中；
10. 本轮不修改数据库、migration、Project 数据、Relay 协议或产品授权语义。

## 4. 修复设计

### 4.1 统一 source contract 入口

新增唯一入口 `scripts/test-ci-source-contracts.sh` 与 `just ci-source-contracts`：

- GitHub `Detect Changed Paths` 调用这一个入口；
- 本地 `just ci` 也调用同一个入口；
- 每个子合同带清楚的开始、成功和失败标签；
- runner 必须执行完全部顶层子合同、收集每项 exit status，最后统一返回失败；不得使用普通 `set -e` 在首项失败时停止；
- 不再由 workflow 与 Justfile 分别维护两份清单。

这样即使 Project View 再次失败，Project Document 和其他合同仍会给出独立结论，不会重演“第一项遮住下一项”的事故。

当前顶层清单为：

1. 固定版本的 `actionlint` workflow contract；
2. release ref contract；
3. open-source release surface；该脚本继续作为 current-product、source asset、public package metadata、retired compose entrypoint 与 release-asset source gate 的聚合 owner；
4. local deployment contract；
5. source first-start contract；
6. `cf` CLI cutover contract；
7. 新的 Relay artifact static / behavior contract；
8. Project View release contract；
9. Project View v3-only runtime contract；
10. Project Document release contract。

`Justfile` 的 `check` 改为依赖这一个 `ci-source-contracts`，移除其中重复列出的 current-product、open-source、local-deployment、first-start、CLI-cutover 与 Project View v3 单项依赖；`ci` 继续经 `check` 调用它。各单项 recipe 可以保留供开发者定向执行，但完整门的路由清单只存在于 unified runner。

抽取路由时必须同步迁移仍在检查“workflow 直接调用”的旧合同：

- `test-release-ref-contract.sh` 改为验证 `workflow → unified runner → release-ref contract`，不再 grep release-ref 脚本必须直接位于 workflow；
- `test-project-view-release-contract.sh` 改为验证 `workflow → unified runner → Project View v3 runtime contract`；它对 Project View changed-path literal 的检查仍留在 workflow；
- Project View / Document artifact targets 则验证 `workflow → artifact builder` 与 manifest membership；
- workflow 本身只直接调用 unified runner，不为满足旧 grep 再复制子命令。

unified runner 还必须有自身行为回归：使用 `mktemp -d` 创建可注入的 stub contract manifest，验证 `fail / pass / fail` 三项全部按序执行且最终返回非零，all-pass 返回零，未知参数返回非零。这样未来误加 `set -e` 或错误循环时会在 source gate 自己被发现。

Project View / Document 合同修改为：

- 检查 workflow 确实调用 canonical artifact builder；
- 检查 builder 可执行；
- 在 builder 内验证各自 test target；
- 不把 target token 复制回 workflow 以满足 grep。

### 4.2 把共享 Relay artifact 变成封闭合同

`scripts/build-ci-relay-artifacts.sh` 改为声明式清单，并增加 `--verify-only`：

#### 四个 binary package

- `buzz-relay`；
- `carryforth-cli`；
- `buzz-admin`；
- `git-credential-nostr`。

#### 三个 archive library package 与十四个显式 integration test binary

archive 必须始终包含以下三个 package 的 `--lib` tests：

- `buzz-db`；
- `buzz-relay`；
- `buzz-test-client`。

Backend 的 Meeting / claim、Project View / Document / Context DB 与 migration 测试会从这部分 archive 运行；它们不能被十四个显式 `--test` binary 取代。

当前 archive 的十一项显式 test binary：

- event reminder；
- Project Document disabled / enabled；
- Project Context stage 1 / stage 3；
- Meeting、Floor、Baton、V2 stage 1、rollout；
- Project View。

再加入 Relay E2E 当前单独冷编译的三项显式 test binary：

- `e2e_persona`；
- `e2e_nostr_interop`；
- `e2e_relay`。

#### 五个非空输出

- 四个 binary；
- 一个 nextest archive。

artifact manifest / arrays 是实际清单的唯一 owner，明确保存：4 个 binary build packages、3 个 archive lib packages、`--lib`、14 个显式 test binaries 与 5 个输出。Project View / Document 合同只验证自己的领域必需项属于该 manifest；workflow 不复制 target 清单。

`--verify-only` 接受可注入的 artifact directory，并对五项逐一执行存在且非空检查。workflow 在 restore / build 之后、save / upload 之前**无条件**调用它，所以 cache hit 与 miss 使用相同验证。`if-no-files-found: error` 只作为上传层的第二道保护。

构建与 archive 使用 `--locked`。新增 `scripts/test-ci-relay-artifact-contract.sh`，静态验证：

- 4 个 binary build packages、3 个 archive lib packages、`--lib`、14 个显式 test binaries、5 个输出没有漏项或重复；
- workflow 调用 build 和 verify；
- producer、cache、upload 与 consumers 的 wiring 完整；
- Project View / Document 的领域合同指向同一个 owner；
- Relay E2E 不再另行执行 `cargo test` 冷编译上述三项，而是安装 nextest 后消费 archive；
- 迁移前后的 Relay E2E test-ID 选择集合精确一致：Persona 与 Nostr Interop 两个 binary 的全部 ignored tests，`e2e_relay` 中名称匹配 `invite` 的原集合，以及精确的 `nip43_membership_snapshots_are_rejected`；
- 用 `cargo nextest list --archive-file ...` 或等价列表合同比较实际选中的 test IDs，防止把整个 `e2e_relay` 意外展开，也防止过滤翻译漏测；保留共享 Relay 所需的执行顺序约束。

behavior contract 使用 `mktemp -d`，不接触真实 `target/ci`，至少覆盖：

- 五项非空时 PASS；
- 逐一删除任一输出时 FAIL；
- 逐一把任一输出置为零字节时 FAIL；
- 未知参数时 FAIL。

### 4.3 修复 cache 的真实输入与冷启动边界

cache key 必须包含真实编译输入，而不是偶然的 workflow 文本：

- `crates/**`、migrations、schema；
- `Cargo.toml`、`Cargo.lock`、Rust toolchain 与 `.cargo/config.toml`；
- artifact builder；static / behavior contract 由 source gate 验证，它不改变 artifact bytes，因此不进入昂贵 cache key；
- `Dockerfile`，因为 Relay lib test 通过 `include_str!` 在编译期读取它；
- nextest 固定版本；
- runner OS 与 architecture；
- 显式 cache schema version。

restore 与 save 只计算一次 primary key，并复用同一值，避免两处表达式漂移。修改产物格式或 target 集合时升级 schema。

新公开仓库当前没有 `relay-artifacts-*` cache，所以下一次 push 必须作为真实 cold-path qualification。首次验收给 Relay artifact job 90 分钟、build step 80 分钟，保留验证、保存与上传时间；取得真实分阶段耗时后，再冻结更紧的长期上限。不能把 cache 预热当作通过前提。

### 4.4 闭合 job graph 与路径过滤

Relay artifact producer 的条件加入：

- `project-view`；
- `project-document`。

否则专项 docs 或领域输入可令 consumer 条件为真而 producer 被跳过，形成静默 `skipped`。

路径过滤按“真实运行输入 → 所有受影响任务”补齐：

| 输入 | 至少触发 |
| --- | --- |
| `Dockerfile` | Rust / shared archive / Project View / Project Document |
| `scripts/build-ci-relay-artifacts.sh` 与新 artifact contract | Rust / Project View / Project Document |
| `desktop/src/features/agents/ui/effortTable.fixture.json` | Desktop + Rust（Rust test 有 `include_str!`） |
| `docker-compose.yml`、`scripts/attach-schema-partitions.sql` | Backend / Relay / Desktop Integration |
| `scripts/setup-desktop-test-data.sh` | Desktop Integration |
| `desktop/scripts/summarize-flaky-tests.mjs` | 已被 `desktop/**` 覆盖；保持 Desktop Smoke / Integration 触发，不添加错误的根目录 pattern |
| 根 `package.json`、`pnpm-workspace.yaml`、`patches/**` | Desktop + Web |

聚合门必须先检查 `needs.changes.result`。其 `if` 条件在 PR 上遇到 changes failure 时也必须运行并失败，不能因为拿不到 path outputs 而被跳过。push 仍执行完整门；PR 只执行受影响面，但任何必需 producer/consumer 关系必须闭合。

### 4.5 修复 Desktop 的确定性失败

#### 4.5.1 Project View 显示值

两条测试把 `E2E Test` 更新为当前 canonical `Local Dev`，同时保留原始行为断言：

- overview 能回到完整 Project View；
- preview-disabled 时稳定 shell 仍存在且不发 Project View read。

不得只删标题断言而丢掉入口或 disabled-preview 合同。

#### 4.5.2 Project View 的旧跨 Community 场景

两条旧测试不再点击不存在的 B rail，改为证明：

- bootstrap 清除或忽略非 canonical remote Community；
- B 的 Project View 数据、selection 与 Assignment 不会绘制到 Local Dev；
- Local Dev 只读取自身 canonical View；
- module-level cache/reset 与 query key 的 Community 作用域仍由低层测试覆盖。

这验证的是当前 local-only 隔离合同，不声称继续支持已退役的跨 Relay UI 切换。

#### 4.5.3 Profile Integration

保留“已保存的 profile description 不丢失”这一有效目的，将旧 round trip 改为：

1. 在 Local Dev 保存 profile；
2. 返回应用并重新打开 Settings，使 Profile 面板重新挂载，但不执行 `page.reload()`；
3. 确认注入的 remote Community 已被 local-only bootstrap 清除；
4. 重新打开 Profile，确认 mock bridge 已接受的值仍存在。

该测试当前使用 mock bridge，profile 值保存在页面内的测试 seam；整页 reload 会重置 seam，因此不能把 reload 后仍存在写成这一测试的合同。相邻 Profile 测试同样只证明 UI 到 mock bridge 的更新，不证明真实 Relay 写入后的 reload 持久性；后者不属于本次旧 Community 场景迁移，方案也不冒充已有该覆盖。

#### 4.5.4 Virtualization DnD

最终顺序 `[Archive, Priority]` 是有效合同，不能删除或改成允许 no-op。实施分两步：

1. 使用真实 pointer 序列，从 sortable handle 开始，跨过 PointerSensor activation distance；
2. 等待 drag overlay / dragging state，移动到目标 sortable shell 的有效 drop 区域，释放后等待持久化与 DOM 顺序更新。

若稳定手势可以触发 drag，但 UI 仍不提交 reorder，则修复 `SidebarDnd` 的 collision / over target / `onDragEnd`，而不是继续改测试。

#### 4.5.5 暂缓的 Video 菜单场景

旧场景用 `.last().toBeVisible()` 等待第二条视频，但第一条播放器已经可以提前满足条件；继续增加 timeout 不能修复目标选错。维护者确认该检查当前不影响产品功能后，本轮将该单个场景标为 `test.skip`，保留测试源码与根因记录，后续按需要以 message/article 或 URL 为稳定身份重建。当前结论是“覆盖暂缓”，不是“视频菜单已经被重新证明”。

同样，长历史级联加载时的 viewport snap 场景因旧驱动不稳定而暂停；普通 virtualization、anchor 与 DnD 覆盖继续运行。

### 4.6 Flaky 证据必须真正可见

Playwright reporter 同时保留 `list`、HTML，并新增 JSON reporter，固定输出 `desktop/playwright-report.json`。CI 在运行 summarizer 前执行严格 JSON 解析与最小 schema 校验，不能只检查文件非空；summarizer 也应提供严格模式，让 parse / schema 错误返回非零，并同时写入 `$GITHUB_STEP_SUMMARY` 与可上传的 `desktop/flaky-summary.md`。严格校验只在依赖安装与 E2E build 已成功、Playwright test step 确实开始后运行，避免早期 install/build 失败再制造无关的 missing-report 错误。证据上传仍使用 `always()`。

需要上传：

- JSON report；
- HTML report；
- `test-results`、trace、screenshots；
- flaky summary。

summarizer 不负责改变主测试结果，但不能再因输入不存在而静默给出空证据。

已观察到的 flaky 包括当前 shard 4 的 relay reconnect、thread focus，以及首轮远端记录过的 channel-browser、overscroll、Project PR review。实施后对仍处于门内的每一项做 `--retries=0 --repeat-each=10` 的定向运行；任何一次失败都必须先修明确的 seam、等待条件或真实产品竞态。retry 后通过不计作“已修复”，已知 flaky 不能原样带入下一次 push。

### 4.7 Windows 条件编译一次闭合

在 `crates/buzz-dev-mcp/src/meeting_read.rs` 中，把以下同一 Unix-only 测试依赖全部放在一致的 `#[cfg(unix)]` 下：

- `crate::shim::Shim` import；
- `tempdir` / `TempDir`；
- `make_state` 与 fake-binary helpers；
- 对应 Unix tests。

Windows job 上限由 45 分钟增至 60 分钟；历史成功运行约 42 分 31 秒，原上限没有合理余量。真实 Windows Clippy、check、`buzz-dev-mcp` tests、Git Bash smoke 与 Tauri check/test 必须全部执行，不能用 Linux cfg 成功代替。

### 4.8 有界超时与证据上传

| job | 初次修复上限 | step 上限 | 原因 |
| --- | ---: | ---: | --- |
| Detect Changed Paths | 5 分钟 | 不单设 | 已观察约 1 分 40 秒，原 2 分钟余量不足 |
| Desktop Smoke shard | 35 分钟 | 30 分钟 | 当前 shard 4 约 11.9 分钟；保留 5 分钟总结与上传 |
| Desktop Integration shard | 40 分钟 | 30 分钟 | 73/81 条、单 worker、最多重试，且新仓尚未真正跑到 |
| Desktop E2E Relay cold build | 90 分钟 | 80 分钟 | 首次空 cache qualification；成功后按实测收紧 |
| Windows Rust | 60 分钟 | 按现有阶段 | 历史成功已接近旧 45 分钟上限 |

Playwright step timeout 应先于 job timeout。summary 和 artifact upload 使用 `always()`，但不能覆盖或掩盖主测试 exit status。真正的 job-level cancellation 无法保证后续步骤运行，所以不以无限增加 job timeout 替代测试级上限。

## 5. 实施顺序

### Phase A：先闭合静态合同（已完成）

1. 新增 unified source-contract runner；
2. 增加 runner 的 fail/pass/fail 与 all-pass 行为回归；
3. 新增 Relay artifact static contract；
4. 同时迁移 release-ref、Project View v3、Project View archive 1 项和 Document archive 2 项的所有权检查；
5. 让 workflow 与 `just ci` 共用入口；
6. 在不构建项目的前提下先让全部 source contracts 通过。

这一阶段必须同时解决当前失败和被它遮住的下一条失败。

### Phase B：闭合 artifact、cache 与 job graph（已完成）

1. builder 使用声明式“4 个 binary packages + 3 个 archive lib packages + `--lib` + 14 个显式 tests + 5 个输出”清单并支持 `--verify-only`；
2. Relay E2E 改用 shared archive；
3. cache key、hit/miss verification、producer conditions 与 path filters 对齐；
4. aggregate 在 changes failure 时 fail closed；
5. 增加分层 timeout 和可达的证据上传路径。

### Phase C：一次处理 Desktop 与 Windows 已知问题（已完成）

1. 修两条 Project View 显示值；
2. 迁移两条 Project View local-only isolation；
3. 迁移 Profile persistence；
4. 修 DnD 手势或真实产品缺陷；
5. 按维护者决定暂停 video 菜单与长历史 snap 两个不稳定场景，保留源码和覆盖缺口记录；
6. 启用 JSON flaky report；
7. 补齐 Windows `Shim` cfg。

### Phase D：本地完整门（已完成）

先做静态、格式和定向验证，再运行完整 Desktop smoke。任何确定性失败都不得带入 push：

```bash
. ./bin/activate-hermit
just ci-source-contracts
for script in \
  scripts/build-ci-relay-artifacts.sh \
  scripts/test-ci-relay-artifact-contract.sh \
  scripts/test-ci-source-contracts.sh; do
  bash -n "$script"
done
# 使用纳入仓库工具链、冻结版本的 actionlint；YAML parser 不能替代它。
actionlint .github/workflows/ci.yml
git diff --check
```

`actionlint` 必须以固定版本进入 Hermit 或等价的受控仓库工具脚本，并由本地与 CI 共用；不能在验证时临时安装 `latest`。它负责验证 GitHub Actions expression、`needs` context、job/step schema 与 workflow 语义，普通 Ruby YAML parse 只可作为补充。

随后至少执行：

- Project View 四条目标场景；
- Profile persistence；
- virtualization DnD；
- video 菜单与长历史 snap 不计入本轮通过面，并保持显式 `skip`；
- 每项已知 flaky 的 `--retries=0 --repeat-each=10`，均为零失败；
- 四个完整 Smoke shards，0 deterministic failure；
- Integration Profile 定向场景；
- `cargo clippy --locked -p buzz-dev-mcp --tests -- -D warnings`；
- 与实际改动相称的 `just desktop-check`、`just desktop-test`。

完成上述定向诊断后，push 前还必须运行一次 `just ci`，证明 root format、lint、unit/build、Desktop/Web 与新 source-contract 入口的本地组合门没有回退；不能只以定向 Desktop 检查替代它。

Linux 本地不能证明 Windows target；共享 artifact cold build 与完整 Relay-backed fan-out 由下一次 GitHub push 提供最终证据。

### Phase E：只发起一次完整 push（待执行）

下一次 push 必须逐项确认：

1. `Detect Changed Paths` 的统一 source contracts 全部实际运行并成功；
2. Relay artifacts 明确走 cache miss；Build、Verify、Save、Upload 全部成功；
3. 五项输出全部非空；
4. Smoke 1–4 全部实际运行，0 deterministic failure；
5. Desktop Integration 1/2 与 2/2 都实际运行；
6. Backend、Relay、Project View、Project Document、Project Context integration 都不是 `skipped`；
7. `Project View Pre-feature Database Boundary` 实际下载 `buzz-admin` 并运行 rollback smoke，不是 `skipped`；
8. Relay E2E 从 archive 执行，不发生隐藏冷编译超时；
9. Windows 完整走过 Clippy、check、tests、Git Bash 与 Tauri；
10. `Desktop` 与 `Desktop E2E Integration` 两个 aggregate 均为 success；
11. Detect、Rust Lint、Unit Tests、Desktop Core、四个 Smoke matrix cells、Desktop aggregate、Relay artifact producer、两个 Integration matrix cells、Integration aggregate、Backend / Relay / Project View / Document / Context integration、Semantic Foundation / Query、Pre-feature Database Boundary、Web、Security、Dead Token Guard、两项 Server Cross-Compile、Windows 与 macOS Desktop build 均按 push 预期展开并成功；
12. 整个 workflow 的最终 conclusion 为 `success`，不存在未解释的 `skipped` 或 `cancelled`。

如果本次 CI 仍失败：**停止，不自动重跑，不继续提交下一处猜测性修复；先向维护者说明首个新失败、已运行范围和未运行范围。**

## 6. 文件改动矩阵

| 文件 / 区域 | 计划改动 |
| --- | --- |
| `.github/workflows/ci.yml` | 统一 contracts、filters、producer/consumer、cache、timeout、report 与 aggregate 条件 |
| `Justfile` | 新增 `ci-source-contracts` 并接入 `just ci` |
| Hermit manifest 或受控 actionlint wrapper | 固定 actionlint 版本并供本地/CI 共用 |
| `scripts/test-ci-source-contracts.sh` | GitHub 与本地共用的静态合同入口 |
| unified source runner 的 self-test / fixture | 证明失败不短路后续合同、all-pass 成功、未知参数失败 |
| `scripts/test-ci-relay-artifact-contract.sh` | 4 binary packages / 3 archive lib packages / 14 explicit tests / 5 outputs、verifier 行为与 workflow wiring 合同 |
| `scripts/build-ci-relay-artifacts.sh` | 声明式清单、`--locked`、`--verify-only` |
| `scripts/test-release-ref-contract.sh` | 从 workflow 直调断言迁移为 unified runner 路由断言 |
| `scripts/test-project-view-release-contract.sh` | 在 canonical builder 中检查 `e2e_project_view` |
| `scripts/test-project-document-release-contract.sh` | 在 canonical builder 中检查 enabled / disabled targets |
| `desktop/playwright.config.ts` | 增加非空 JSON reporter |
| `desktop/scripts/summarize-flaky-tests.mjs` | 严格解析并输出 job summary 与可上传的 Markdown 证据 |
| `desktop/tests/e2e/project-view.spec.ts` | Local Dev 显示值与 local-only isolation |
| `desktop/tests/e2e/profile.spec.ts` | Profile persistence 的 local-only 场景 |
| `desktop/tests/e2e/virtualization.spec.ts` 与对应 DnD UI | 稳定真实 drag/drop；长历史 snap 场景暂缓并保留源码 |
| `desktop/tests/e2e/video-attachment.spec.ts` | 暂缓不稳定的多视频菜单场景，保留源码与根因记录 |
| Community reset / bootstrap 的现有低层测试 | 补当前 local-only cache 与不泄漏合同 |
| `crates/buzz-dev-mcp/src/meeting_read.rs` | `Shim` 与 Unix-only helper cfg 对齐 |
| 本文档 | 记录实际实施、验证与最终 CI run；未实施前不得写成已完成 |

不在本修复中新增产品功能，不恢复多 Community Desktop，不修改 database schema、migration、Project 领域模型或 semantic query。

## 7. 完成标准

| Blocker | PASS 条件 |
| --- | --- |
| Source contracts | GitHub 与 `just ci` 使用同一入口；release-ref / PV-v3 路由合同已迁移；fail/pass/fail 全部执行后统一失败；PV 与 Document 合同都通过 |
| Artifact contract | 4 binary packages、3 archive lib packages、`--lib`、14 explicit tests、5 outputs 单一 owner；verifier 行为矩阵与 Relay E2E test-ID 等价合同通过，hit/miss 都验证 |
| Cold build | 新仓 cache miss 至少成功一次，真实耗时已记录 |
| Job graph | 专项变更不会令 producer skipped；aggregate 在 changes failure 时 fail closed |
| Desktop deterministic | Project View、Profile、DnD 与仍启用的场景通过；四个 smoke shard 0 failure；video 菜单与长历史 snap 明确为暂缓覆盖 |
| Flaky evidence | 已知项定向 10 次零 retry 均通过；JSON 经严格解析/schema 校验，JSON/HTML/trace 可下载，retry 不再隐藏 flaky |
| Integration | 两个 shard 与所有下游集成实际运行并成功 |
| Windows | 全部 Windows steps 实际运行并成功 |
| Regression | Detect、Semantic、Rust lint/unit、Web、Desktop Core、Dead Token、Security、macOS build、两项 cross-compile、Pre-feature Database Boundary 与 source-only 既有通过面不回退；workflow 总结论为 success，无 unexpected skipped |

以下情况仍保持 **BLOCKED**：

- 只修 Project View contract，Project Document 仍失败；
- cache hit 通过，但 cold build 从未成功；
- artifact 上传了部分文件，但五项没有逐一验证；
- Integration 或 Windows 为 `skipped`；
- 仅靠 retry、扩大 timeout 或放宽 DnD 顺序转绿；暂停单项场景必须有维护者决定、根因和覆盖缺口记录；
- aggregate 被移除或把未运行映射为 success；
- 本地定向通过，但完整 push 又失败后仍继续自动提交。

## 8. 数据安全与回滚

- 本方案不需要数据库 migration、schema 改动、volume reset、Community 删除或 Desktop 数据清理；
- 本地 E2E 只使用 mock bridge 或 disposable test services；
- 不运行 `docker compose down -v`，不删除用户 volume/keyring/app data；
- CI workflow、scripts 与 tests 都是普通 Git 变更，可通过新提交回滚；
- 已推送的 `7e53094c1` 保留为审计基线，不改写公开历史；
- 如果实现过程中发现必须改变产品行为或扩大到新的领域，先停止并更新方案，不顺手混入。

## 9. 实施记录

### 9.1 已发生的部分修复

`7e53094c1` 修复了首轮暴露的一部分问题，并完成了若干本地定向检查，但它不是本方案的完成实现。第二轮 run 31770286297 证明本地验证没有覆盖 GitHub changes job 的完整合同。

### 9.2 Phase A：静态合同

已完成：

- 引入冻结版本的 `actionlint` 与 workflow wrapper；
- 新增 aggregate-all source-contract runner 及 fail/pass/fail、all-pass 自测；
- workflow 与 `just ci` 共享同一入口；
- release-ref、Project View v3、Project View archive 与 Project Document archive 的 owner 检查迁移到当前 canonical 脚本；
- 全部 source contracts 本地通过。

### 9.3 Phase B：共享产物与 job graph

已完成：

- builder 固化 4 个 binary packages、3 个 archive lib packages、`--lib`、14 个显式 tests 与 5 个输出；
- 增加 `--verify-only` 及缺失、空文件、未知参数行为合同；cache hit / miss 均验证；
- Relay E2E 改为消费共享 nextest archive，并保留原 test-ID 选择；
- cache key、真实输入、producer conditions、path filters、aggregate fail-closed 与分层 timeout 已闭合；
- Playwright JSON / Markdown flaky evidence 改为严格解析，不再静默吞掉缺失或损坏报告。

### 9.4 Phase C：Desktop 与 Windows

已完成：

- 修复 Project View canonical 名称、local-only isolation 与 Profile remount 场景；
- 稳定 Sidebar DnD pointer/collision seam并保留最终顺序断言；
- 修复 channel-browser、Relay reconnect、thread focus 与 Project PR review 的确定性等待/排序 seam；
- 补齐 Windows Unix-only `Shim` 条件编译；
- 按维护者决定将 video 多播放器菜单和长历史 viewport snap 两个不稳定场景标为 `test.skip`。这两项是显式覆盖缺口，不计作功能修复；
- 发现 Tauri `relay_admission` 测试共享静态 gate 在多 Tokio runtime 并行执行时会挂起，现将该模块以 `--test-threads=1` 独立运行；其余 Tauri 测试仍保持正常并行。

### 9.5 Phase D：本地验收

本地已通过：

- `just ci-source-contracts` 与固定版本 `actionlint`；
- `git diff --check`、脚本逐文件 `bash -n`；
- Project View、Profile、DnD 定向场景；
- Relay reconnect 10 次、thread focus 20 次和修复后的 Project PR review 10 次，均 `retries=0`；
- 四个完整 Smoke shards：分别为 `201 passed / 2 skipped`、`220 passed`、`196 passed / 1 skipped`、`188 passed / 2 skipped`，没有 failure / flaky；
- `just desktop-check`、`just desktop-test`（3707 passed）；
- `just desktop-tauri-test`（常规 1716 passed、14 ignored；串行 admission 13 passed）；
- `cargo clippy --locked -p buzz-dev-mcp --tests -- -D warnings`；
- 最终完整 `just ci`，exit 0。

### 9.6 Phase E：远端待验收

尚未获得本次改动的 GitHub cold-path 证据。提交后只推送一次并观察完整 workflow；在远端结论为 success、且预期 job 无 unexplained skip 前，整体状态仍不是远端 PASS。若该轮失败，按维护者要求立即停止，不自动重跑、修改或再次推送，只报告首个新失败及实际运行范围。
