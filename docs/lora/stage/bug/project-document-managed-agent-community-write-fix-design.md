# Project Document managed Agent Community 写入授权修复设计

> 状态：已实现并通过定向验证，待本地真实交互验收
> 记录日期：2026-08-03
> 范围：Project Document v1、Agent-first CLI、Relay/DB 写入鉴权与本地真实验收

## 1. 结论

`test-1` 无法创建 Project Document 不是 capability 初始化、Community membership 或
Document 数据损坏导致的，而是 Project View 授权修复后遗留的两处实现不一致：

1. `buzz documents` 的 managed Agent 写入路径仍只接受 Project View schema v2，当前
   schema v3 Community 会在 CLI 本地预检阶段失败；
2. Project Document DB writer 仍把 active Assignment 与 supervised Runtime fence 作为
   所有 managed Agent 写入的必要条件，未采用已经确认的 Community ACL 边界。

本次修复采用以下权威语义：

- Project Document 是 Community / Project 的版本化长文本资产，也是可以被 Project View
  直接引用的独立坐标；
- Human 与 verified managed Agent 的普通 Document create/update/delete 都由 Community
  写入资格授权；
- Project Role 不建立第二套 Document ACL，Assignment 的存在或结束都不授予、也不撤销
  普通 Document 写入能力；
- 第一方 `buzz documents` 默认以 Community principal 身份写入，不主动附加 Assignment 或
  Runtime fence；
- wire 中现有的 `acting_assignment_id + runtime_fence` 成对字段保持兼容。若调用者显式携带，
  Relay 必须严格验证，失败后不能静默删除字段并降级为 Community 写入；
- Runtime `supervisor` 继续服务需要明确 Role / Runtime 归因的高保证路径，但不是普通
  Document CRUD 或本地开发的前置条件。

该决策与
[Project View Community 授权与 Assignment Fence 边界修复设计](project-view-assignment-authorization-boundary-fix-design.md)
一致，并补上该文档第 6 节当时有意留待单独确认的 Project Document 范围。

## 2. 故障表现与事实

当前本地 Community 已满足：

- Relay 广告 `buzz-project-view-v3`、`buzz-project-document-v1` 与
  `buzz-project-context-v1`；
- Project View v3、Project Document v1 与 Project Context 均已初始化并处于 ready；
- `test-1` 是 owner 仍为 Community member 的 verified managed Agent；
- `test-1` 已有 active Assignment；
- Document catalog revision 为 `0`、active Document 数量为 `0`，属于合法空目录，不是异常
  状态。

`test-1` 执行创建命令时，CLI 在签名和发送事件之前进入
`crates/buzz-cli/src/commands/documents.rs::attach_managed_runtime()`。该函数将读取到的 Project
View identity 强制过滤为 `ProjectViewSchema::V2`：

```text
当前 Community = Project View v3
    -> v2-only filter 返回 None
    -> assignment_unavailable: managed Document writes require Project View v2
    -> command 未发送到 Relay
```

因此当前最先暴露的是 CLI schema 迁移遗漏，而不是 Relay 拒绝。

即使只删除该 v2 filter，当前 DB writer 仍会形成第二层阻断：

```text
known managed Agent
    -> 强制要求 acting_assignment_id
    -> 强制 RequireSupervisedRuntime
    -> 当前没有 supervisor binding / lease / runtime fence
    -> Unauthorized
    -> Relay 统一映射为 restricted:project_document:runtime_fence
```

这说明仅把 CLI 从 v2 改为 v2/v3 并不能真正修复问题，也不应通过临时配置 supervisor 来
掩盖错误的授权前置条件。

## 3. 根因

### 3.1 CLI 把普通 Document 写入误判为 Role-bearing 写入

`attach_managed_runtime()` 对所有 managed Agent Document create/update/delete 自动执行：

1. 读取 Project View identity；
2. 强制要求 schema v2；
3. 查找 active Assignment；
4. 从环境读取 Runtime fence；
5. 将两者附加到 Document command。

这把“谁在 Community 中写文档”和“是否代表某个 Role / Runtime 写文档”混成了同一个判断。
Document 本身可以被 Project、Goal、Role、Work、Resource 等任意 Project View 对象引用，
但它不因此天然属于某一个 Role 任期。

### 3.2 DB 把 Assignment 当作 managed Agent 的 Document ACL

`crates/buzz-db/src/project_document.rs::validate_actor_in_tx()` 已正确检查：

- Human 是否为直接 `relay_members` 成员；
- managed Agent 是否有持久化 owner，且 owner 仍为直接 Community member；
- actor 与 owner 是否被 ban 或 timeout。

但随后又无条件要求 managed Agent 提供 active Assignment，并调用
`RequireSupervisedRuntime`。这让 Assignment 从“可选的明确归因/fence”扩张成了普通 Document
写入许可，和 Project View 普通对象已经修复后的 Community ACL 不一致。

### 3.3 Relay 错误映射掩盖真实原因

`ProjectDocumentWriteError::Unauthorized` 同时承载 Community 失权、Assignment 无效和 Runtime
fence 失败，Relay 又统一返回 `restricted:project_document:runtime_fence`。因此 Human、Agent 和
诊断工具无法判断究竟是哪一层失败。

## 4. 修复后的授权模型

### 4.1 分层判断

Document 写入依次执行以下判断：

```text
1. Protocol shape
   schema、closed JSON、字段配对、正文限制、目标 revision

2. Community principal eligibility
   Human 是直接成员，或 managed Agent 的 owner 仍是直接成员

3. Buzz write admission
   签名、credential、MessagesWrite、ban / timeout、rate limit

4. Optional role/runtime attribution
   仅当 command 显式携带 Assignment + Runtime fence 时验证

5. Document concurrency and invariants
   expected_document_revision、tombstone、引用保护、projection parity
```

Assignment 和 Runtime fence 不能替代 Community 资格；Community 资格也不能让一个显式声称
旧 Assignment 的命令通过。

### 4.2 写入矩阵

| Actor 与 command | Community gate | Assignment / Runtime | 结果 |
|---|---:|---:|---|
| 合格 Human，不携带归因字段 | 必须通过 | 不需要 | 允许 |
| 合格 Human，携带任一归因字段 | 必须通过 | 不适用 | 拒绝 |
| 合格 managed Agent，两字段都省略 | 必须通过 | 不需要 | 允许普通写入 |
| 合格 managed Agent，成对携带当前 Assignment 与 exact Runtime | 必须通过 | 严格验证 | 允许并保留归因 |
| managed Agent 携带 stale、ended、他人 Assignment 或错误 Runtime | 必须通过 | 验证失败 | 拒绝，不降级 |
| actor / owner 已失去 Community 资格或受限 | 失败 | 不论是否有 Assignment | 拒绝 |
| 任意 actor 只携带成对字段之一 | 不进入业务授权 | wire shape 非法 | 拒绝 |

第一方 CLI 在本阶段只使用“不携带归因字段”的普通 Community 写入模式，不新增
`--as-role` 一类用户入口。保留显式字段是 wire 兼容和 fail-closed 需要，不代表当前产品必须
部署 Runtime supervisor。

### 4.3 多 ACP 实例的语义

同一逻辑 Agent 的旧、新 ACP 进程使用同一个稳定 Agent 公钥，因此对普通 Community Document
写入是同一个 principal。首版不把短暂进程重叠提升为额外授权维度：

- 同一 Document 的并发更新由 `expected_document_revision` CAS 决定，只有一个基于相同旧
  revision 的写入能提交；
- 不同 Document 的独立写入可以并行，这是既有 Community 权限允许的正常行为；
- 所有提交继续记录 actor、公钥、command event、revision、receipt 与 audit；
- 若要撤销普通 Document 能力，应通过 Community remove、owner 失权、ban 或 timeout 表达；
- Assignment 结束只阻止 Role-bearing 行为，不能被误用为 Community membership revocation。

Runtime supervisor 可以在以后存在不可逆外部操作、合规要求或必须保证单 active executor 时
作为单独的高保证模式启用，但不属于本缺陷修复的交付前置。

## 5. 实现方案

### 5.1 Agent-first CLI

涉及：

- `crates/buzz-cli/src/commands/documents.rs`

修改：

1. 普通 `documents create/update/delete` 不再调用 `attach_managed_runtime()`；
2. 删除该 v2-only helper 以及仅由它使用的 Project View identity、Assignment 和 Runtime
   fence imports；
3. `ProjectDocumentCommand::new(...)` 保持
   `acting_assignment_id=None, runtime_fence=None` 并直接签名提交；
4. 保留 Document capability identity、exact revision、ambiguous delivery read-back 与 receipt
   核验；
5. CLI 不根据环境中是否碰巧存在 fence 文件静默改变 command 语义；
6. 本次不新增显式 Role-attributed CLI 参数，避免扩大产品面。

结果是 v2 与 v3 Community 的普通 Document 命令走同一条稳定路径，也不再要求 Agent 先拥有
Role。

### 5.2 DB transaction-local writer gate

涉及：

- `crates/buzz-db/src/project_document.rs`

保留 `prepare_command()` 与 `commit()` 在 Community lock 内的两次 authority recheck，避免
准备后、提交前发生撤权仍可提交。将 `validate_actor_in_tx()` 调整为两个独立阶段：

1. **Community gate**
   - Human：要求 actor 为直接 member；
   - managed Agent：要求持久化 owner 为直接 member；
   - actor 与 owner 的 active ban / timeout 均 fail closed；
   - 不读取 Project Role 或 Assignment 作为普通写入资格。
2. **Optional attribution gate**
   - `(None, None)`：Community gate 通过后直接允许；
   - managed Agent 的 `(Some assignment, Some runtime)`：验证 Assignment active、属于 signer，
     再使用 `RequireSupervisedRuntime` 验证 exact fence；
   - Human 携带归因字段：拒绝；
   - stale/ended/wrong Assignment 或 Runtime：拒绝且不重组命令重试；
   - partial pair 继续由 Project Document closed command validation 拒绝，并在 DB 层保持防御性
     fail closed。

本次可以在现有函数内做最小重构。若提取共享 Community gate，必须是 crate-private、错误类型
中立的 helper，并由 Project View 与 Project Document 同时加回归测试；不能让 Document 依赖
schema v2/v3 的 Role writer error 类型。

### 5.3 Relay 错误分类

涉及：

- `crates/buzz-db/src/project_document.rs`
- `crates/buzz-relay/src/handlers/project_document.rs`

将单一 `Unauthorized` 至少拆成以下稳定类别：

| DB 类别 | Relay message | 含义 |
|---|---|---|
| Community actor 不合格 | `restricted:project_document:not_authorized` | actor/owner 非成员或受限 |
| 显式 Assignment 无效 | `conflict:project_document:acting_assignment` | stale、ended、他人 Assignment |
| 显式 Runtime fence 无效 | `restricted:project_document:runtime_fence` | 缺失、stale 或无有效 supervision |

字段只出现一半等 closed-wire 错误继续映射为 `invalid:project_document:*`。错误信息不泄露其他
成员、Assignment 或 supervisor 的具体数据，只帮助调用者定位失败层。

### 5.4 协议与数据兼容

无需执行以下操作：

- 不新增或修改 Nostr event kind；
- 不提升 Project Document schema version；
- 不修改现有 command、receipt、revision 或 attribution JSON shape；
- 不新增数据库 migration；
- 不重新初始化 Project View、Document 或 Context；
- 不删除、重投影或恢复历史数据；
- 不改变 Resource、Guide、Context Reference 或 Role-bearing command 的权限。

已有带有效 Assignment/Runtime 的 Document 历史记录与 receipt 保持有效；新的普通 managed
Agent 写入只是将两个本来就 optional 的字段省略。

## 6. 测试方案

### 6.1 Project Document 领域与 SDK

固定现有 closed-wire 语义：

1. 两个归因字段都省略时合法；
2. 两个字段成对且值合法时可解析；
3. 只出现一个字段、nil Assignment 或非法 Runtime fence 时拒绝；
4. create/update/delete revision 与正文限制不受本修复影响。

### 6.2 DB 集成测试

至少覆盖：

1. 合格 Human 无归因字段 create/update/delete 成功；
2. 无 Assignment 的合格 managed Agent 无归因字段创建成功；
3. 有 Assignment、但没有 supervisor binding/lease/fence 的合格 managed Agent，以无归因
   command 创建和更新成功；
4. 合格 managed Agent 显式携带有效 Assignment + Runtime fence 时成功；
5. 显式携带 stale、ended、他人 Assignment 或 stale Runtime 时拒绝，且不降级；
6. Human 携带归因字段时拒绝；
7. managed Agent owner 被移出 Community、actor/owner 被 ban 或 timeout 后拒绝；
8. prepare 后、commit 前撤权时第二次校验拒绝；
9. 相同 expected revision 的并发更新仍只有一个成功；
10. delete 的 Resource Guide / Live Context 引用保护保持不变；
11. v2 与 v3 Community 对上述普通 Document 写入结果一致。

### 6.3 CLI 与 Relay 测试

至少覆盖：

1. managed Agent 在 Project View v3 下执行 `buzz documents create`，不再触发 v2-only
   `assignment_unavailable`；
2. 没有 active Assignment 或 Runtime fence 环境变量时仍能组装普通 Document command；
3. 实际签名 JSON 同时省略 `acting_assignment_id` 与 `runtime_fence`；
4. Human 与 managed Agent 的成功回执仍做 event、operation、document ID 和 exact revision
   核对；
5. Relay 分别返回 Community、Assignment 和 Runtime 三类稳定错误；
6. ambiguous delivery read-back 不会因授权调整误报成功。

### 6.4 本地真实 canary

使用当前 `localhost:3000` Community 完成：

```text
1. 确认 Document capability ready，记录 catalog revision = 0
2. 不配置 Runtime supervisor，不向 test-1 注入 Runtime fence
3. test-1 创建第一份普通 Document
4. 确认 catalog revision 0 -> 1、Document revision = 1、actor = test-1
5. test-1 基于 exact revision 更新 Document，确认 revision = 2
6. Human 读取相同 current/history，内容与 attribution一致
7. 可选：创建 Resource，并将该 Document 作为 mandatory Guide，验证读取闭环
8. 对无引用测试 Document执行 delete；对仍被 Guide / Live Context引用的 Document确认拒绝
```

canary 的关键证明是“Community 合格 managed Agent 在无 Assignment/Runtime 前置条件下完成
真实写入”，不能只用 Human writer 或 mock 代替。

## 7. 实施顺序

1. 先增加 managed Community Document writer 的失败回归测试；
2. 调整 DB Community gate 与 optional attribution gate；
3. 拆分 DB/Relay 错误类别；
4. 删除 CLI 默认 managed Runtime 注入与 v2-only preflight；
5. 执行 Project Document domain、SDK、CLI、DB 与 Relay 定向测试；
6. 更新 `docs/lora/stage/document/changelog.md` 与
   `docs/lora/stage/document/implementation-design.md` 中旧的 strict-writer 结论；
7. 清理增量构建产物，重新构建并启动 Relay/Desktop/Agent；
8. 执行 `test-1` 本地真实 canary并记录结果；
9. 完成完整质量门后提交。

建议实现提交可按以下逻辑组织；是否拆分 Git commit 由交付时决定：

```text
test(document): capture managed community writer authorization
fix(document): separate community access from optional runtime attribution
fix(cli): submit ordinary managed document writes without role fence
docs(document): reconcile community ACL and document writer semantics
```

## 8. 风险与回滚

### 8.1 主要风险

- 误把 Community ACL 放宽成 open-relay writer；
- 在 prepare 通过后漏掉 commit-time 撤权复检；
- 对显式 stale Assignment/Runtime 静默降级；
- 只修 CLI，导致其他 SDK client 仍在 DB 层失败；
- 只修 DB，不删除 CLI 的 v2-only 与 Runtime 本地前置；
- 更新权限后破坏 Document revision、引用删除保护或 receipt 幂等语义。

上述风险分别由 transaction-local membership/owner gate、双重校验、closed pair 规则和定向
集成测试约束。

### 8.2 回滚

本修复不改变 wire 或数据库 schema，代码可以直接回滚。但回滚会重新引入“managed Agent
必须配置 supervisor 才能写普通 Document”的已知缺陷，因此只应在发现新的 Community ACL
安全回归时使用；不应把部署 supervisor 当作常规回滚替代方案。

## 9. 完成标准

本修复只有同时满足以下条件才算完成：

1. `test-1` 在当前 Project View v3 Community 中无需 supervisor 即可创建 Document；
2. 无 Assignment 的合格 managed Agent也能执行普通 Document CRUD；
3. Project Role 不再作为普通 Document ACL；
4. Community remove、owner 失权、ban 和 timeout仍能立即阻断写入；
5. 显式 Assignment/Runtime 归因仍严格、不可降级；
6. Human 与 managed Agent 使用相同 revision、引用保护、receipt 与 audit语义；
7. v2/v3 不再影响独立 Document v1 的普通写入资格；
8. 错误码能区分 Community、Assignment 与 Runtime 失败；
9. 不需要 migration、重新初始化、旧数据恢复或 Runtime supervisor provisioning；
10. 定向测试、质量门与本地真实 canary全部通过。

## 10. 实现交付记录

2026-08-03 已完成代码修复：

- `buzz documents` 的普通写入不再探测 Project View schema、Assignment 或 Runtime fence，
  默认同时省略两个可选归因字段；
- DB writer 先执行 Community principal 校验，仅在 managed Agent 显式声明
  `acting_assignment_id + runtime_fence` 时进入严格 Role / Runtime 归因校验；
- Human 伪造归因、无效 Assignment、stale Runtime 与 Community 失权分别保持 fail-closed；
- Relay 将 Community、Assignment 与 Runtime 失败映射为不同的稳定错误类别；
- wire、数据库 schema、Document revision、引用删除保护和 receipt 结构均未改变。

已通过 Project Document domain/SDK、CLI、DB、Relay 的定向测试、Rust 格式检查、相关 crate
`cargo check` 与 `clippy -D warnings`。真实 PostgreSQL 回归同时证明：

1. managed Community writer 无 Assignment、无 Runtime fence 可以完成 create；
2. 无效的显式 Assignment 不会降级为普通 Community 写入；
3. Human 不能伪造 managed Runtime 归因；
4. owner 被移出 Community 后，managed Agent 写入立即失败；
5. 有效的显式 Assignment + Runtime 路径仍可提交，stale Runtime 仍被拒绝。

Relay、Desktop 与 ACP 已重新构建启动，Relay 正确广告
`buzz-project-view-v3`、`buzz-project-document-v1` 和 `buzz-project-context-v1`。尚未冒用
`test-1` 的真实身份向当前项目写入验收数据；第 6.4 节的真实交互 canary 留给 Desktop 中的
`test-1` 验收，因此当前状态不宣称该项已经完成。
