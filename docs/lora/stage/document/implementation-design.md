# 项目文档、Resource Guide 与上下文引用实现设计

> 本文定义[项目文档与资源说明概念设计](./document.md)如何基于 Buzz 当前分支落地。
> 它覆盖现有实现取证、Project Document v1、Project View v3、Nostr wire、PostgreSQL
> 规范状态、Relay 事务、SDK、CLI、Desktop、ACP / Role Brief、迁移、测试、发布与
> 分阶段开发计划。
>
> 本文是一份实现设计，不表示本文描述的能力已经存在，也不在本阶段提交 Rust、SQL
> 或客户端代码。
>
> 实现取证基线：当前 worktree 的 `feat/resource-v0` 分支，基线提交 `26ab4d4e`。本文
> 只检查该分支中的 Buzz 代码、migration、测试脚本和既有设计文档，没有用其他分支或
> 其他 Buzz ecosystem repo 的未合入实现补全假设。

## 1. 文档目的

[项目定位与目标](../../project-positioning.md)已经把 Buzz 定位为持续存在的 Project
协作基座；[项目视图定义与项目上下文关系](../project-view/project-view.md)建立了稳定
项目坐标；[角色连续性概念设计](../role/role-continuity.md)和
[角色连续性实现设计](../role/implementation-design.md)让责任、承担者和工作局势不再
依赖单次 Agent Runtime。

[概念设计](./document.md)进一步确认：

- Project Document 是 Project 共同拥有、可修订和固定引用的 Markdown 内容坐标；
- Resource 是资产或能力坐标，不是资源本体；
- 每个 Resource 通过 `guide_document_id` 指向主要使用说明；
- Project View 对象可以轻量引用 Resource 或 Document；
- Buzz 首版负责身份、修订、引用和读取，不负责 clone、安装、连接、Secret 或执行；
- Agent 只在需要时读取正文，不把全部文档放进每轮 Context；
- 每次规范更新形成完整 Document snapshot；局部编辑只是客户端体验；
- Document 使用自己的 revision，不与无关 Document 或 Project View 写入发生 revision
  冲突。

本文不重新讨论“它是什么”，而是回答：

1. 如何复用 Buzz 当前已经交付的 command、canonical state、receipt 和 Relay projection
   链路；
2. 为什么 Project Document 必须是独立 capability，而 Resource 变化必须进入
   Project View 新 major；
3. Document identity、revision、current head、history 和删除如何实现；
4. Resource Guide 和 Context Reference 如何保持跨域完整性；
5. Human、Agent、CLI 和 Desktop 如何发现、读取和更新它们；
6. 现有 Resource 如何安全迁移，旧客户端如何 fail closed；
7. 后续开发如何拆成可独立测试、部署和验收的阶段。

## 2. 实现结论

首版不做一个同时改完所有客户端和所有 Project View 数据的大爆炸版本，而是交付两个
有先后依赖的协议主体；Context 的可写 / 可消费状态再作为 Project View v3 的 staged
sub-capability单独开放。

### 2.1 独立的 Project Document v1

Project Document 作为新 capability 落地：

```text
buzz-project-document-v1

member-signed command
        ↓
Project Document pure reducer
        ↓
PostgreSQL canonical current + immutable revisions + receipt
        ↓
relay-signed metadata head + full revision projection + catalog meta
```

它拥有：

- 独立 `document_id`；
- per-document `document_revision`；
- active current head；
- 不可变 active / tombstone revision；
- 完整 Markdown snapshot；
- revision CAS；
- Community-global 成员边界；
- Relay 签名的可验证读取模型。

它不成为第十种 `ProjectViewObjectType`，也不推进 `project_revision`。

### 2.2 Resource 与 Context Reference 进入 Project View v3

当前 Project View v1/v2 的 Resource 和 relations 都是 closed schema。目标 Resource：

```text
name + resource_kind + summary + guide_document_id
```

以及新的：

```text
context_references:
  - Resource reference
  - Live / Pinned Document reference
```

必须作为 `buzz-project-view-v3` 实现。v3：

- 继续保留现有九类 Project View object；
- 保留 v2 Role Continuity 与 Runtime fencing；
- 以新 closed wire type 表达 Resource Guide 和 Context Reference；
- 与 v1/v2 复用协议族 kinds，但通过 `schema_version: 3` 和 NIP-11 capability 明确区分；
- 每个 Community 同一时点只写一个 Project View major。

不能给 v2 的 `#[serde(deny_unknown_fields)]` 结构偷偷增加 optional 字段，也不能让 v2
客户端把 v3 Resource 当成旧 Resource。

### 2.3 保持 Nostr-first，不增加 CRUD HTTP API

Document 写入使用新的成员签名 command kind；读取使用 Relay 签名 projection。它们继续
通过 Buzz 已有入口：

- WebSocket `EVENT`；
- `POST /events`；
- WebSocket `REQ` / `COUNT`；
- `POST /query` / `POST /count`；
- 提交后的实时 fan-out。

不增加 `/documents`、`/resources` 或其他 endpoint-specific CRUD API。需要一致分页的
Document list / history 只扩展通用 `/query` 的 Buzz extension，和 Project View 当前
实现保持同一模式。

### 2.4 Agent-first 的最小纵向闭环

第一个可用闭环是：

```text
buzz project-view get
        ↓
取得 Resource + guide_document_id
        ↓
buzz resources guide <resource-id> --content-only
        ↓
Agent 阅读 Guide
        ↓
Agent 使用当前 Runtime 已有的 git / MCP / Skill / Plugin / shell 能力
```

它不依赖 SkillHub、Plugin Registry、Resource Resolver 或新的 Agent tool protocol。
Desktop 文档界面和 Project View v3 在这个纵向闭环之后分阶段接入。

## 3. 当前实现取证

本节描述当前分支已经存在的事实。后文凡标记为“新增”或“目标”的内容都还没有实现。

### 3.1 Project View 的真实边界

当前 `crates/buzz-project-view` 是纯领域 crate：

- `src/model.rs` 定义 closed object / data / relations；
- `src/mutation.rs` 定义 closed command；
- `src/state.rs` 执行 clone → reduce → validate 的原子状态迁移；
- `src/read_model.rs` 组装全部 active object 的完整 snapshot；
- 不包含 PostgreSQL、Nostr、签名、网络或 async 逻辑。

当前普通 object 写入：

- 使用客户端给出的 `expected_project_revision`；
- 每次成功写入增加全局 `project_revision`；
- 更新目标 object 时增加其 `object_revision`；
- 删除保留 ID、revision、actor/time 的 tombstone；
- 删除前检查现有结构关系的 incoming reference。

这条全局 revision 非常适合数量有限、强相关的结构化项目状态，不适合正文经常变化且
彼此无关的大量 Document。

### 3.2 当前 Resource 不是本文目标 Resource

`crates/buzz-project-view/src/model.rs` 中当前 `ProjectResource` 固定为：

```text
name
resource_type       # closed enum
locator             # closed locator type + value
description         # required
```

当前 `ResourceType` 只有：

```text
repository / document / design / service / environment / artifact / url
```

当前 `ResourceLocator` 明确是 inert locator。领域层不 resolve、fetch、connect 或验证
真实资源可用性。`Resource` 作为 relation source 也不能使用现有
`under_goal / under_plan / planned_in_stage / about / handles` 槽。

Desktop 的 TypeScript wire、normalizer、serializer、表单与 Inspector 完整复制了这套
closed shape，因此 Resource Guide 不是只增加一个前端按钮就能完成。

### 3.3 v2 仍然复用旧 Resource shape

`crates/buzz-project-view/src/v2.rs` 明确把 v2 wire 放在独立模块中，原因是向 closed v1
shape 增加字段会导致旧客户端误接收或误拒绝数据。

当前 v2：

- 增加 Role Continuity、Assignment 和 Runtime fence；
- ordinary object reducer 仍复用 v1 的九类对象；
- Resource body 和五个 relation 槽没有变化；
- Community 通过 `project_view_schema_version` 在 v1 / v2 中二选一；
- NIP-11 同一 Community 只广告对应的一个 Project View major。

同样的兼容规则必须用于 v3。

### 3.4 Relay、DB 与 projection 的现有链路

`crates/buzz-relay/src/handlers/project_view.rs` 当前负责：

- exact tags 和 closed content 解析；
- credential scope、Community-global credential、current membership、ban / timeout；
- capability enabled、schema version 与 stable signer readiness；
- managed actor Assignment / Runtime fence；
- Relay projection 构造与签名；
- 错误映射、metrics 和 commit 后 fan-out。

`crates/buzz-db/src/project_view.rs` 与 `project_view_v2.rs` 当前负责：

- 获取 Community advisory lock；
- 在 transaction 内重新验证 canonical state；
- receipt / replay；
- 规范时间；
- canonical row；
- command event；
- Relay projection event；
- current pointer 替换；
- 一次原子 commit。

安全和 readiness gate 在 receipt lookup 之前执行。因此一个曾经有权限的 event 不能在
成员被移除、封禁或 capability 被关闭后，仅凭旧 receipt 绕过当前权限。

PostgreSQL 是 canonical truth。kind `40903 / 40904` 是可重建、Relay 签名的客户端读取
投影；它们不采用 NIP-33 `created_at` last-write-wins，而由领域 revision 和数据库精确
event pointer 管理 current。

当前 managed ordinary-object fence 校验仍是 `project_view_v2.rs` 内部 helper，而且在
active Assignment 没有 runtime supervisor binding 时会返回成功；它实际表达的是
“已注册 supervision 才强制 fence”，不是无条件 managed fence。

Document 实施时把“actor 是否 managed → active Assignment ownership → supervisor
binding → current leased Runtime fence”事务校验提取为 `buzz-db` 内部共用模块，并显式
传 policy：

```text
LegacyOptionalSupervision   # 仅供 v2 兼容
RequireSupervisedRuntime    # Document v1 与 Project View v3
```

Document v1 和 v3 下，managed actor 没有 active binding、没有 current leased runtime
coordinate 或 fence 不匹配都拒绝。v2 旧行为在 cutover 前保持兼容，不能复制两份 SQL 后
分别演进，也不能把当前 helper 原样复用后声称已经强制 fence。

### 3.5 现有内容对象不能直接复用

| 现有对象 | 可以复用 | 不能复用 |
|---|---|---|
| Channel Canvas `40100` | Markdown 编辑/渲染；Agent 只注入坐标并按需读取正文 | Channel scope；单一 latest event；无 UUID、CAS 和正式 revision history |
| NIP-23 Note `30023` | Markdown、title、summary、CLI stdin/file 输入体验 | author + slug identity；`created_at` LWW；无 Community canonical revision |
| Project View object | 稳定 UUID、tombstone、canonical actor/time、projection trust | 全局 revision；完整 snapshot；closed object / relation schema |
| Role Checkpoint / Handoff | append-only history、归因和固定引用经验 | 它们是 Role continuity entity，不是通用 Project Document |

Project Document 应复用 Project View 的事务和信任外壳，而不是复用 Canvas / NIP-23 的
业务身份语义。

### 3.6 当前 Agent 读取方式

`buzz` CLI 已经是 Agent 的主要平台接口。ACP 会向 managed Agent 注入：

- `BUZZ_RELAY_URL`；
- `BUZZ_PRIVATE_KEY`；
- `BUZZ_AUTH_TAG`；
- managed runtime / fence 所需环境。

当前 `buzz project-view get` 已能返回 Resource，但 Resource 不会自动进入 Role Brief。
当前 Role Brief 主要包含 Profile、Goal、当前 Role / Assignment、负责的 Work、相关对象、
Checkpoint / Handoff 和 source revisions。

Canvas 已经采用一个重要模式：ACP 验证 Canvas event 后，只向 Agent 提供坐标、event
revision、更新时间和 `buzz canvas get` 命令，不内联 Markdown 正文。Project Document
沿用这个交付原则。

### 3.7 必须复用与必须避免的实现选择

| 关注点 | 本设计选择 |
|---|---|
| 领域层 | 新建 pure `buzz-project-document`，延续 Project View reducer 边界 |
| 写协议 | typed signed command；不接受自由 JSON patch |
| 规范状态 | PostgreSQL current + immutable revisions + change receipt |
| 客户端信任 | Relay-signed strict projection；不信任 raw DB / 未验证 event |
| 并发 | per-document expected revision；不同 Document 不发生 revision conflict |
| 资源演进 | Project View v3；不改 v1/v2 closed serde |
| 正文读取 | lazy；list / Brief 不携正文 |
| 局部修改 | 客户端 exact patch，服务端仍接收完整 snapshot |
| Secret | 明确禁止、迁移人工审查；不声称正则扫描能证明“无 Secret” |
| 执行 | 读取 Guide 没有执行副作用 |
| API | 复用 Nostr / generic bridge；不新增文档 CRUD HTTP endpoint |

## 4. 总体架构

```text
Human Desktop                         Agent / buzz CLI
      │                                      │
      ├──── signed Document command ─────────┤
      └──── signed Project View v3 command ──┘
                         │
                         ▼
                Buzz 通用 WS / HTTP ingest
                         │
             signature / scope / membership
                         │
                         ▼
        ┌────────────────┴──────────────────┐
        │                                   │
        ▼                                   ▼
Project Document coordinator        Project View v3 coordinator
  ├─ document revision CAS            ├─ project revision CAS
  ├─ full snapshot reducer             ├─ Resource Guide validation
  ├─ delete reference check            ├─ Context Reference validation
  └─ head/revision/meta plan            └─ v2 Role continuity + v3 heads
        │                                   │
        └──────── same Community lock ──────┘
                         │
                         ▼
               PostgreSQL canonical state
        ┌────────────────┴──────────────────┐
        │                                   │
        ▼                                   ▼
project_documents / revisions       project_view_* + normalized refs
changes / receipts                  current objects / continuity state
        │                                   │
        └──────── Relay-signed projections ─┘
                         │
                       commit
                         │
                         ▼
                local / Redis live fan-out
                         │
              verified SDK / CLI / Tauri
                         │
        metadata in View / Brief; body fetched on demand
```

关键一致性边界：

- Document update 不推进 `project_revision`；
- Project View ref update 不推进 `document_revision`；
- Resource Guide / Context Reference 的创建、替换和删除保护必须和 Document lifecycle
  共享 Community lock；
- canonical row、command、receipt 和全部当次 projection 同事务提交；
- fan-out 只发生在 commit 之后，失败通过重新查询恢复。

## 5. Capability 与版本边界

### 5.1 NIP-11 capability

新增：

```text
buzz-project-document-v1
buzz-project-view-v3
buzz-project-context-v1       # PV v3 上的独立 staged sub-capability
```

一个 Community 可以同时广告：

```text
buzz-project-document-v1
buzz-project-view-v2
```

或：

```text
buzz-project-document-v1
buzz-project-view-v3
```

Document capability 与 Project View capability 分开开关、分开 readiness。Project View
仍然只能广告 v1、v2、v3 中的一个。

`buzz-project-context-v1` 只可与 ready 的 `buzz-project-view-v3` 同时广告。v3 wire /
parser 从一开始就认识 `context_references`，但 Community column
`project_context_enabled` 默认 false；在 sub-capability 未 ready 前，Relay 要求 create
的 set 为空，并拒绝任何 add / replace 为非空 set 的 command。这样阶段 5 可以使用
Resource + mandatory Guide，而不会形成“Context 已可写、CLI / UI / Role Brief 却看不见”
的半开放状态。

由于 Document 写入需要复用 v2 的 managed Assignment / Runtime fence，本设计规定：

- Project View v1 Community 可以先部署 Document 表和 reader 代码，但不能 enable 或
  广告 Document capability；
- `project_document_enabled = true` 的前置条件是该 Community 已经在 Project View
  v2 或 v3；
- Human 与 managed Agent 因此始终使用同一套当前 actor / runtime 安全边界。

这不把 Document revision 合并进 Project View；它只是部署 readiness 的安全依赖。

### 5.2 兼容矩阵

| Reader / writer | Document v1 | PV v2 | PV v3 | Context sub-capability |
|---|---:|---:|---:|---:|
| 当前旧客户端 | unsupported | 支持 | unsupported | unsupported |
| Stage 5 base dual client | 支持 | 支持 | 支持 | 不写；strict preserve |
| Stage 6 Context-ready dual client | 支持 | 支持 | 支持 | 支持 |
| v3-only 后续客户端 | 支持 | 可保留只读兼容 | 支持 | 支持 |

Community 切到 v3 之前，SDK、CLI、Desktop、ACP 必须先成为 v2/v3 dual reader。旧客户端
遇到 v3 应明确报 `unsupported:project_view:schema`，不能回退成 v2 写入。支持 PV v3
不自动等于 Context-ready；Stage 5 client必须 round-trip字段但不能在 sub-capability
缺失时新增引用。

### 5.3 kinds 与 schema major 的关系

Project Document v1 使用自己的 kinds。Project View v3 继续复用 Project View 协议族：

```text
44300  Project View command
40903  Project View object / entity projection
40904  Project View meta
```

v3 由 content 中的 `schema_version: 3`、strict tags 和
`buzz-project-view-v3` capability 表达，延续 v2 已经采用的 major 版本方式。

## 6. 代码位置与模块边界

### 6.1 新增 `buzz-project-document`

新增：

```text
crates/buzz-project-document/
└── src/
    ├── lib.rs
    ├── model.rs
    ├── command.rs
    ├── reducer.rs
    ├── projection.rs
    ├── validation.rs
    └── error.rs
```

同时把 crate 显式加入 root workspace members；依赖只允许 domain 所需的
`buzz-core / chrono / serde / thiserror / uuid` 等 pure 库，不引入 `sqlx`、Nostr client
或 async runtime。

职责：

- closed v1 domain types；
- create / update / delete validation；
- pure state transition；
- full snapshot / tombstone；
- revision 与 no-op 规则；
- wire-neutral projection plan；
- stable domain error。

禁止：

- SQL；
- Relay signer；
- Nostr network；
- authorization lookup；
- async；
- Markdown 执行或外部 Resource 解析。

### 6.2 `buzz-core`

`crates/buzz-core/src/kind.rs` 增加 Document kinds 及所有分类器：

- `ALL_KINDS`；
- `has_indexed_d_tag`；
- Document projection / command / protocol classifier；
- `is_command_kind`；
- `is_relay_only_kind`；
- workflow / search 排除所需 classifier。

漏掉任一分类器会导致 `d` tag 不可查询、客户端可以伪造 projection、command 不进入事务
handler，或 wildcard 查询绕过私有读取门禁。

### 6.3 `buzz-sdk`

新增 `crates/buzz-sdk/src/project_document.rs`：

- command `EventBuilder`；
- current / revision / meta coordinate；
- Relay projection builder；
- strict parser；
- signer、kind、created_at、exact tags、content / tag parity 验证；
- changed-head binding；
- golden wire fixture。

SDK 保持 pure：返回 builder，不持有 keys，不建立网络连接。

Project View v3 在现有 `project_view_v2.rs` 旁新增明确 v3 parser / builder；不把 v3
字段加入 v1/v2 parser。

### 6.4 `buzz-db`

新增：

```text
crates/buzz-db/src/project_document.rs
```

它是唯一可写 Project Document canonical tables 的 Rust 模块，提供受限 transaction API：

- begin / lock；
- current actor / fence revalidation；
- receipt lookup；
- load current / target revision；
- prepare transition；
- commit signed projections；
- list / history snapshot；
- capability readiness；
- disable / reproject。

Project View v3 的 normalized ref 读写继续放在 `project_view_v2.rs` 的 v3 successor 或新的
`project_view_v3.rs`，不由 Relay handler 拼接自由 SQL。

### 6.5 `buzz-relay`

新增：

```text
crates/buzz-relay/src/handlers/project_document.rs
```

职责与当前 Project View handler 对齐：

- security / readiness；
- exact command tags；
- pure command 解析；
- DB transaction orchestration；
- Relay signing；
- stable error mapping；
- body-free telemetry；
- post-commit delivery。

同时把当前 `project_view_read_eligible` 和散落在 REQ、COUNT、HTTP query、subscription、
fan-out 的 gate 抽成 Community-global private protocol 共用 helper。Document 和 Project
View 使用相同 credential / membership 判断，但保留各自 kind classifier。

### 6.6 `buzz-cli`、Desktop 与 ACP

新增：

- `buzz documents ...`；
- Tauri native verified Project Document commands；
- Desktop `features/documents/`；
- Project View v3 TS wire / normalizer / serializer；
- ACP v3 Role Brief 和 Document metadata resolver；
- `buzz resources guide` 只读便利命令。

Agent-facing 操作首先进入 `buzz-cli`，不加入 `buzz-dev-mcp`。Agent 仍可通过已有 shell
调用 CLI，ACP 只负责可信坐标和运行时环境。

## 7. Project Document v1 领域模型

### 7.1 Current Document

```rust
ProjectDocument {
    document_id: Uuid,
    current_revision: u64,
    state: Active | Deleted,
    created_at: DateTime<Utc>,
    created_by: PublicKey,
    updated_at: DateTime<Utc>,
    updated_by: PublicKey,
}
```

active title、summary 和 Markdown 是 Current Revision 的投影，不是可以独立修改的第二份
事实。实现读取时从 current revision join 得到这些字段。

Deleted head：

- 保留 ID、current revision、created / deleted actor/time；
- 不返回旧 title、summary 或 Markdown；
- ID 永远不能复用；
- v1 不支持 restore / undelete。

### 7.2 Immutable Revision

```rust
DocumentRevision {
    document_id: Uuid,
    document_revision: u64,
    state: Active | Deleted,
    title: Option<String>,
    summary: Option<String>,
    content_markdown: Option<String>,
    actor: PublicKey,
    canonical_at: DateTime<Utc>,
}
```

shape：

| state | title | summary | content_markdown |
|---|---|---|---|
| active | required | optional | required，可为空字符串 |
| deleted | absent | absent | absent |

每个 active Revision 都自包含完整 snapshot。后续更新不能改写旧 Revision 的业务列。
重投影最多重绑 projection pointer，不能改变 snapshot。

### 7.3 Command

```rust
ProjectDocumentCommand {
    schema_version: 1,
    expected_document_revision: u64,
    acting_assignment_id: Option<Uuid>,
    runtime_fence: Option<RuntimeFence>,
    request: DocumentCommandRequest,
}
```

closed request：

```text
create {
  document_id,
  title,
  summary?,
  content_markdown
}

update {
  document_id,
  title,
  summary?,
  content_markdown
}

delete {
  document_id
}
```

规则：

- create 必须 `expected_document_revision = 0`；
- create 成功形成 revision 1；
- update / delete 必须等于当前 revision；
- update 只接受完整 next snapshot；
- deleted ID 上的 create / update / delete 都失败；
- title、summary、Markdown 完全相同的 update 是 `no_change`，不产生空 revision；
- Human actor 必须同时省略 `acting_assignment_id / runtime_fence`；
- managed Agent 必须同时提供当前 active Assignment ID 和 exact Runtime fence，不能只提供
  其中一个；
- Document managed write 使用 `RequireSupervisedRuntime`；NoBinding、无 current lease、
  stale runtime ID / epoch 都返回 `restricted:project_document:runtime_fence`；
- 一个 command 只修改一份 Document；v1 不提供 batch。

### 7.4 Identity 与 canonical time

- `document_id` 使用客户端生成的 UUID v4；
- nil、非 v4 或非 RFC 4122 variant 被拒绝；
- UUID 与 title、author、event ID 和 revision 无关；
- canonical time 由 transaction 内 PostgreSQL clock 取得；
- member event `created_at` 只参与事件有效性检查，不成为业务更新时间；
- actor 来自已验证 signer，不能从 JSON 指定。

### 7.5 Revision 与 catalog revision

本设计区分：

```text
document_revision
    某一 Document 的 optimistic-concurrency revision

catalog_revision
    整个 Document catalog 的单调观察序号
```

客户端写入只提交 `expected_document_revision`，从不提交 expected catalog revision。
因此 Document A 的更新不会因为 Document B 已更新而返回 conflict。

`catalog_revision` 只用于：

- list pagination 的稳定窗口；
- meta live invalidation；
- projection generation / parity 检查；
- ACP metadata cache key。

v0 为降低事务和跨域完整性风险，所有 Document 写先复用现有 Community exclusive lock，
因此物理上串行；这不形成全局 CAS，也不产生无关业务冲突。只有在 metrics 证明需要时，
后续才优化为 Community shared lock + per-document advisory lock，并保持 Project View /
membership 写使用 exclusive lock。

### 7.6 Validation limits

首版固定以下 byte limits：

| 字段 | 上限 |
|---|---:|
| command JSON content | 65,536 bytes |
| JSON nesting | 16 |
| title | 256 bytes |
| summary | 4,096 bytes |
| `content_markdown` | 49,152 bytes |

补充规则：

- 所有上限按 UTF-8 bytes 计算；
- title trim 后必须非空，并拒绝前后空白的非规范输入；
- summary `None` 与空字符串不并存；空字符串规范化为 `None` 由客户端完成，领域层拒绝；
- Markdown byte-for-byte 保存，不 trim、不改换行；
- 所有字符串拒绝 NUL；
- 即使 raw Markdown 未超过 49,152 bytes，JSON escaping 后 command 超过 65,536 bytes
  仍然被拒绝；
- 所有限制同时在 domain、SDK builder、Relay parser 和 DB constraint / tests 中对齐。

49,152 bytes 为普通 Guide 留出足够空间，并给 Document 协议自身更严格的 64 KiB
command limit 下的 envelope 和通常 JSON escaping 留出余量；当前 Relay 通用 ingest 上限
实际是 256 KiB。Document 的 64 KiB 是产品协议限制，不是对 Relay 全局限制的描述。它
也不是“大文件”承诺。附件、图片和超大文档不在 v1。

### 7.7 Full snapshot 与客户端局部修改

规范更新始终是：

```text
明确 base revision
        +
完整 title / summary / content_markdown
        ↓
新的完整 immutable revision
```

服务端不接受：

- fuzzy patch；
- 行号指令；
- JSON Merge Patch；
- “替换第一次出现的字符串”；
- 自动 rebase。

CLI / Desktop 可以：

1. 精确读取 base revision；
2. 在本地应用零模糊 patch；
3. 得到完整 next snapshot；
4. 使用 base revision 提交 update；
5. 在 409 conflict 时保留本地结果，由用户重新读取、比较并显式重试。

因此客户端可以“只改某一部分”，但协议不会失去完整 snapshot 和 exact CAS。

### 7.8 Delete

普通 delete：

- 精确匹配 current revision；
- 新增一个 bodyless tombstone revision；
- head 进入 Deleted；
- active list 不再返回它；
- 历史 active revision 继续可按 `(document_id, revision)` 读取；
- 不删除 command event、revision 或历史 projection；
- 不表示合规擦除。

存在以下任一 active incoming relation 时拒绝：

- Resource `guide_document_id`；
- Live Document Context Reference。

Pinned Document Reference 不阻止普通 delete。删除保护和 Project View v3 mutation 使用同一
Community lock，不能出现“检查后新建引用”的竞态。

## 8. Document command wire

### 8.1 create 示例

```json
{
  "schema_version": 1,
  "expected_document_revision": 0,
  "request": {
    "type": "create",
    "document_id": "9c23f672-a397-42d1-b933-104ba2674f26",
    "title": "Buzz repository guide",
    "summary": "Clone、初始化和验证当前仓库。",
    "content_markdown": "# Repository\n\n..."
  }
}
```

### 8.2 update 示例

```json
{
  "schema_version": 1,
  "expected_document_revision": 7,
  "acting_assignment_id": "151f2347-7d24-41a0-ab0d-f272e84fcf88",
  "runtime_fence": {
    "runtime_id": "74ad5e95-903b-4488-ac19-d95a73fa62d4",
    "runtime_epoch": 4
  },
  "request": {
    "type": "update",
    "document_id": "9c23f672-a397-42d1-b933-104ba2674f26",
    "title": "Buzz repository guide",
    "summary": "Clone、初始化和验证当前仓库。",
    "content_markdown": "# Repository\n\n..."
  }
}
```

具体 `RuntimeFence` wire 与当前 Project View v2 完全相同。实现时把 wire-neutral
`RuntimeFence { runtime_id, runtime_epoch }` 提取到双方可依赖的 shared core type，并由
v2 原路径 re-export 以保持 source compatibility；不能再定义一套 Document 专用 epoch。

### 8.3 delete 示例

```json
{
  "schema_version": 1,
  "expected_document_revision": 8,
  "request": {
    "type": "delete",
    "document_id": "9c23f672-a397-42d1-b933-104ba2674f26"
  }
}
```

### 8.4 command tags

成员 command tags 必须严格等于：

```text
["-"]
["t", "buzz-project-document-command"]
```

不使用 `h` tag，因为 Project Document 是 host-derived Community-global 对象，不是
Channel 对象。`document_id` 和 revision 的规范值来自已签名 closed content，不从重复
tag 读取，避免 tag / content 两份事实。

command kind 是 append-only。完全相同的签名 event 重试必须保持 event bytes 不变，并由
receipt 返回同一结果。

### 8.5 response / receipt

Relay transport 继续采用 Buzz 已有三字段 shape；规范 receipt JSON 放在 `response:`
message 中：

```json
{
  "event_id": "<command-event-id>",
  "accepted": true,
  "message": "response:{\"document_id\":\"...\",\"document_revision\":8,\"catalog_revision\":31,\"state\":\"active\"}"
}
```

CLI 在验证 receipt 和 read-back 后，可以把这些字段解析为便于 Agent 使用的输出：

```json
{
  "event_id": "<command-event-id>",
  "accepted": true,
  "message": "response:{...}",
  "document_id": "9c23f672-a397-42d1-b933-104ba2674f26",
  "document_revision": 8,
  "catalog_revision": 31,
  "state": "active"
}
```

规范 receipt 至少绑定：

- command event / change ID；
- actor 与 acting Assignment；
- operation；
- document ID；
- expected / committed document revision；
- catalog revision；
- committed lifecycle state；
- canonical accepted time。

Projection event ID 是 signer / generation 相关的可重建 materialization，不进入稳定
receipt。否则 signer reproject 后重放旧 command 会返回已 retired 的 pointer。客户端
验证 receipt 后，使用业务 coordinate 和 committed revision 重新读取当前 generation 的
Relay-signed projection；两者都通过后才报告成功。
网络在提交后中断且无法确认时返回现有 `delivery_unknown`，不能猜测失败并生成新 command。

## 9. Event kinds 与 Relay projection

### 9.1 kind 分配

当前分支中以下编号未被占用，本设计将它们作为 Document v1 的保留值：

| Kind | 用途 | 作者 |
|---:|---|---|
| `44301` | Project Document command | 当前 Community member |
| `40905` | 轻量 current Document head | Relay only |
| `40906` | 完整 immutable Document revision | Relay only |
| `40907` | Document catalog meta | Relay only |

阶段 0 必须在 `buzz-core/src/kind.rs`、协议文档和 collision tests 中一次性注册。若在实现
开始前仓库已经占用候选编号，应先更新本文和全部 fixture，再落代码；不能只在某个 handler
私自换号。

`40905 / 40906 / 40907` 虽使用 indexed `d` tag，但不采用 NIP-33 LWW。它们由
Project Document domain revision、projection generation 和数据库 current pointer 管理。

### 9.2 coordinates

```text
head:
project-document:<project-uuid>:<document-uuid>

revision:
project-document:<project-uuid>:<document-uuid>:revision:<decimal-revision>

meta:
project-document:<project-uuid>:meta
```

长期业务引用仍是 `document_id` 或 `document_id + document_revision`。coordinate 和 event
ID 是 projection 地址，不取代业务 identity。

### 9.3 current head projection

active head content：

```json
{
  "schema_version": 1,
  "projection_type": "document_head",
  "project_id": "<community-uuid>",
  "projection_generation": 1,
  "catalog_revision": 31,
  "document_id": "<document-uuid>",
  "document_revision": 8,
  "state": "active",
  "title": "Buzz repository guide",
  "summary": "Clone、初始化和验证当前仓库。",
  "created_at": "<canonical-time>",
  "created_by": "<pubkey>",
  "updated_at": "<canonical-time>",
  "updated_by": "<pubkey>",
  "revision_coordinate": "<coordinate>",
  "revision_event_id": "<event-id>",
  "source_event_id": "<command-event-id>"
}
```

tombstone head 保留：

- identity；
- generation / catalog / document revision；
- created actor/time；
- deleted actor/time；
- tombstone revision coordinate / event ID；
- source event ID。

它不包含旧 title、summary 或 Markdown。

### 9.4 immutable revision projection

active revision content 包含：

- schema / projection type / project；
- projection generation；
- catalog revision；
- document ID / revision；
- `state: active`；
- 完整 title、summary、`content_markdown`；
- created actor/time；
- 本 revision actor/time；
- source event ID。

tombstone revision 使用 `state: deleted`，不含 title、summary、Markdown。

普通 update 不 retire 旧 active revision projection；它必须继续支持 Pinned read。只有
projection signer generation 重建时，旧 generation 的 projection 才由受限 reproject
流程退出当前可读集合。

### 9.5 catalog meta

Meta 是完整 catalog observation boundary：

```json
{
  "schema_version": 1,
  "projection_type": "document_meta",
  "project_id": "<community-uuid>",
  "initialized": true,
  "projection_generation": 1,
  "catalog_revision": 31,
  "active_document_count": 12,
  "reset": false,
  "changed_heads": [
    {
      "head_coordinate": "...",
      "head_event_id": "...",
      "document_id": "...",
      "document_revision": 8,
      "revision_event_id": "...",
      "deleted": false
    }
  ],
  "source_event_id": "<command-event-id>",
  "updated_at": "<canonical-time>"
}
```

普通 command 的 `changed_heads` 恰有一个元素。operator reproject 的 reset meta 使用
`reset: true`、空 `changed_heads`，要求 reader 丢弃旧 generation cache 并重拉 catalog。
空 catalog bootstrap 使用 `catalog_revision: 0`、`active_document_count: 0`、
`reset: true`；Document head / revision 自身的 revision 始终从 1 开始。

### 9.6 projection tags

所有 projection：

- 包含 `["-"]`；
- 包含唯一 canonical `d`；
- 包含 `["t", "buzz-project-document"]`；
- 包含 projection subtype / active 或 tombstone tag；
- 包含 canonical decimal generation / revision tags；
- 普通变更包含 `["e", "<command-event-id>", "", "source"]`；
- head 包含指向 revision event 的 marked `e` tag。

SDK parser 要求 exact tag multiset 和 content / tag 完全一致，拒绝：

- 重复 tag；
- 非 canonical decimal；
- 不同 project coordinate；
- tag revision 与 content revision 不同；
- source / revision event pointer 不同；
- unexpected tag；
- event kind、signer、created_at 或 content shape 不符。

### 9.7 不增加独立 content hash

v1 wire 不增加一份 `content_hash`：

- Nostr event ID 已经绑定完整 projection content；
- Relay signature 绑定 event ID；
- head 明确引用 revision event ID；
- canonical revision 与 projection 在同一 transaction 做 typed parity 检查。

再增加可独立漂移的 body hash 会形成第三份事实。若未来需要跨系统 content-addressed blob，
应在新的 storage capability 中设计，不能把它混入 v1 Markdown identity。

## 10. PostgreSQL 规范状态

迁移使用下一可用编号；当前分支从 `0032` 起。实际开发可以按阶段拆成多份 additive
migration，但不得重写已经发布的 `0025`–`0031`。

### 10.1 Community capability 字段

```text
communities.project_document_enabled BOOLEAN NOT NULL DEFAULT FALSE
```

默认关闭。只有以下条件全部成立才可 enable：

- Project View schema 为 2 或 3；
- stable Relay signer 存在；
- Document state / meta 已 bootstrap；
- signer、state、head、revision、event pointer parity 通过；
- 当前部署支持 Document kinds、query gate 和 projection parser。

### 10.2 `project_document_state`

```text
community_id                PK / FK communities
schema_version              = 1
catalog_revision            0..MAX_SAFE_REVISION
active_document_count       >= 0
last_change_id              nullable at catalog revision 0
last_actor_pubkey           nullable at catalog revision 0
projection_pubkey           32 bytes
projection_generation       >= 1
meta_projection_event_id    32 bytes
initialized_at
updated_at
```

这张表提供 catalog observation 与 projection readiness，不是写入 CAS。初次 bootstrap
创建 revision 0、active count 0、无 last change / actor 的 Relay-signed meta；首次业务
变更后 two last fields 必须同时存在并与 change / meta source 一致。

### 10.3 `project_documents`

```text
community_id
document_id
current_revision
state                       active | deleted
created_at
created_by
updated_at
updated_by
deleted_at                  nullable
current_source_change_id
current_head_event_id
current_revision_event_id
```

主键：

```text
(community_id, document_id)
```

这张表不保存 active Markdown。title / summary 也从 current revision 读取，避免它们成为
可以独立更新的第二份业务事实。

### 10.4 `project_document_revisions`

```text
community_id
document_id
document_revision
catalog_revision
state                       active | deleted
title                       active required
summary                     active optional
content_markdown            active required
actor_pubkey
canonical_at
source_change_id
source_event_id             member command 时存在
projection_generation
projection_event_id
```

主键：

```text
(community_id, document_id, document_revision)
```

约束：

- revision 1..`2^53-1`；
- catalog revision 1..`2^53-1`；
- active / tombstone shape；
- canonical time 单调不早于 Document create time；
- source / projection ID 长度；
- 同一 Document revision 永不 insert 第二次；
- 普通代码不能 UPDATE / DELETE 语义列；
- 只有受限 reproject transaction 可以变更 generation / projection pointer。

### 10.5 `project_document_changes`

```text
community_id
change_id                   member command 时等于 event ID
source_type                 nostr_event | nip98_request | operator | system
source_event_id             nullable
source_request_hash         NIP-98 source 使用
source_audit_seq            operator / system source 使用
idempotency_key_hash        operator / system source 使用
actor_pubkey
acting_assignment_id        nullable
operation                   create | update | delete
document_id
expected_document_revision
document_revision
catalog_revision
result                      closed JSON receipt
accepted_at
```

head / revision / meta event pointers 只存在于 `project_document_state`、
`project_documents` 和 `project_document_revisions` 的当前 materialization 列，不复制进
stable change receipt。reproject 可以重绑这些 pointer，而不会改变业务 receipt。

它保存归因、幂等和结果，不再复制 Markdown；完整内容已经在 command event、canonical
revision 和 Relay revision projection 中存在。

这张表不重新发明一套弱化的 source shape，而是镜像当前 v2 `ChangeSource`：

- `nostr_event`：只有 `source_event_id`；
- `nip98_request`：同时有 `source_event_id` 与 `source_request_hash`；
- `operator / system`：只有正数 `source_audit_seq` 与
  `idempotency_key_hash`；
- 所有 digest / event ID 都严格为 32 bytes；
- 其他列组合全部由 `CHECK` 拒绝。

同时建立与 `project_view_changes` 相同意图的 partial unique indexes：

```text
(community_id, source_event_id)         WHERE source_event_id IS NOT NULL
(community_id, source_audit_seq)        WHERE source_audit_seq IS NOT NULL
(community_id, idempotency_key_hash)    WHERE idempotency_key_hash IS NOT NULL
```

普通 Document v1 member command 只使用 `nostr_event`。`nip98_request` 与
operator / system shape 先与现有 source kernel 对齐，但阶段 1 不产生这三类业务 change；
empty bootstrap 不分配 business revision。未来 audited repair 若要写入这张 ledger，必须
先显式扩展 closed `operation / receipt` contract，不能把一个 request hash 塞进含义不明
的通用列。

### 10.6 indexes

至少增加：

- active Document list：
  `(community_id, state, document_id)`；
- current revision join：
  `(community_id, document_id, current_revision)`；
- history：
  `(community_id, document_id, document_revision DESC)`；
- catalog observation：
  `(community_id, catalog_revision, document_id)`；
- source / projection event reverse lookup；
- change replay：
  `(community_id, change_id)` unique。

Document canonical tables 不为 Markdown 新建 FTS / vector index，v1 也不开放 NIP-50
Document search。通用 `events` 表现有 generated `tsvector` 可能仍会随 event row 物化；
读取路由必须拒绝 Document protocol kinds 的 `search` filter，不能把这个内部列误当成
已授权的产品能力。

### 10.7 deferred constraints 与 hard-delete guard

数据库 deferred validation 至少检查：

1. state active count 等于 active `project_documents` 数量；
2. current revision row 存在且与 current state shape 一致；
3. current source / head / revision pointer 与 revision / change 表一致；
4. active head / revision / meta event 存在、kind 正确、signer / generation 正确；
5. deleted current row 没有 active business fields；
6. revision 只能按 1 递增；
7. Document ID 不被移除或复用；
8. revisions / changes 禁止 hard delete；
9. ordinary update 不能改写历史 snapshot。

普通业务 delete 只新增 tombstone。若未来需要隐私擦除，必须是独立的治理协议、审计和
storage scrub 流程。

## 11. Transaction、锁与幂等

### 11.1 v0 锁策略

所有 Project Document create / update / delete 先取得现有 Community exclusive advisory
lock。Project View v3 mutation、Resource cutover、membership writer 也使用同一 lock
namespace。

统一锁顺序：

```text
1. begin PostgreSQL transaction
2. acquire Community exclusive advisory lock
3. lock project_document_state
4. lock target project_documents row（存在时）
5. transaction 内重验 Community / actor / runtime fence
6. receipt lookup
7. reference / revision validation
8. pure reduce + projection signing
9. write canonical state / events / receipt
10. deferred constraints
11. commit
12. live fan-out
```

先使用 coarse lock 的原因：

- 现有 Project View 和 membership 已验证这条顺序；
- Document delete 和 v3 新建 Live reference 不能竞态；
- Resource Guide replacement 和 target delete 不能竞态；
- catalog meta、active count 和 projection pointer 需要同一 observation boundary；
- 首版正确性比尚未出现的 Document write throughput 更重要。

这不会让 Document A 因 Document B revision 变化而返回 conflict。等待 lock 是物理
serialization，业务 CAS 仍然只比较 target Document。

后续只有在 production metrics 证明 lock wait 成为问题时，才实施：

```text
Community shared lock
    + per-document advisory exclusive lock
    + catalog sequence / meta 的独立短临界区
```

优化后 Project View / membership 继续拿 Community exclusive lock。实现前必须用并发测试
证明不存在相反锁顺序和 delete / new-reference race。

### 11.2 一次 create / update / delete

Relay 处理一次 command：

1. 在 transaction 外验证 Nostr signature、kind、exact tags、content byte limit 和
   credential 基线；
2. 检查 stable signer、capability enabled 和初步 current membership；
3. begin Document write transaction 并取得 Community lock；
4. transaction 内再次检查 Community 未 archived、capability 仍 enabled、actor / owner
   仍合格、ban / timeout 和 managed Assignment / Runtime fence；
5. 上述 gate 通过后才查 receipt；
6. receipt 存在则验证其 canonical shape，rollback read transaction 并返回原结果；
7. load current Document 和 `project_document_state`；
8. 执行 exact document revision CAS；
9. delete 时查询 Resource Guide / Live ref reverse indexes；
10. 从数据库取得 canonical time；
11. pure reducer 生成 next current、new immutable revision 和 projection plan；
12. 增加 catalog revision，计算 active count；
13. SDK 构造 revision projection，Relay 签名；
14. SDK 用已签 revision event ID 构造 head，Relay 签名；
15. SDK 用 head / revision IDs 构造 meta，Relay 签名；
16. 原子保存 command event、change receipt、current row、revision row、三个 projection
    events 和精确 current pointers；
17. deferred validation；
18. commit；
19. commit 后 schedule revision / head / meta fan-out。

任一签名、SQL、constraint 或 event insert 失败都 rollback。post-commit Redis / socket
delivery 失败不回滚，且当前 dispatch helper 不保证 socket / Redis 到达顺序；reader
只能根据 signed pointer、meta 和 snapshot query 恢复。

### 11.3 安全 gate 必须在 replay 之前

以下变化都必须让旧 command replay 失败：

- 成员被移除；
- managed Agent owner 不再是成员；
- actor 或 owner 被 ban / timeout；
- Assignment 已结束；
- Runtime fence 已推进；
- capability 已关闭；
- signer readiness 失效；
- Community 已 archived。

因此不能为了减少数据库读取把 receipt lookup 移到 gate 之前。这一点直接复用当前
Project View handler 的安全语义。

### 11.4 no-op 与 conflict

| 情况 | 结果 |
|---|---|
| expected revision 不等于 current | `conflict:project_document:revision` |
| update snapshot 与 current 完全相同 | `invalid:project_document:no_change` |
| create ID 已存在，包括 tombstone | `conflict:project_document:id_exists` |
| delete 有 Guide / Live ref | `conflict:project_document:still_referenced` |
| pinned target revision 不存在或是 tombstone | `invalid:project_document:revision_target` |
| replay 同一个 accepted event | 返回同一 accepted receipt |
| 相同意图重新签成不同 event | 作为新 command，重新执行 current CAS |

客户端不能在 conflict 后自动把 `expected_document_revision` 改成最新值并重发；那会把
原本基于旧内容的修改伪装成已经审阅过新内容。

### 11.5 command、workflow 与 audit

Document command 和 Relay projection 是平台控制协议：

- 不进入普通消息 thread counter；
- 不进入 NIP-50 body search；
- 不触发 workflow，避免把 Guide 内容当成可执行事件；
- 使用 `project_document_changes` 和 immutable revisions 作为可靠归因 ledger；
- 普通 event-created audit 若仍由 commit 后 worker 写入，只能视为 best effort。

首版不为了 hash-chain audit 把全部 Document 写串到 `buzz-audit` chain lock。若治理要求
“Document change 与 hash-chain entry 必须同事务”，后续可以复用
`buzz-audit::append_in_transaction`，但应作为明确 capability change 并评估 Community
级写吞吐，而不是在实现中隐式声称已经具备。

## 12. 读取、分页与实时同步

### 12.1 不新增专用 endpoint

规范读取继续使用：

```text
REQ / POST /query
COUNT / POST /count
```

SDK 和 CLI 只查询明确 kinds。Relay 已有 p-gate 要求开放查询指定 kinds；Document 不为
方便 list 而放宽 wildcard / search 限制。

### 12.2 读取操作

| 操作 | 数据源 | 是否含正文 |
|---|---|---:|
| catalog meta | kind `40907` | 否 |
| active list | kind `40905` active heads | 否 |
| current get | head + 它指向的 kind `40906` | 是 |
| pinned get | 指定 revision coordinate 的 `40906` | 是 |
| tombstone get | deleted head | 否 |
| history list | 完整 signed revision page；客户端默认只展示 metadata | wire 含正文 |
| history get | 指定 revision projection | active revision 含正文 |

客户端不能只收到一个 raw revision event 就认定它属于当前 Project：

- 先从 NIP-11 / trusted Relay identity 建立 expected signer；
- 校验 event signature / kind / tags / project coordinate；
- current get 校验 head 指针；
- pinned get 校验业务 document ID / revision；
- list 校验 meta observation。

### 12.3 `/query` extension

在现有 generic query body 中增加 closed `buzz_project_document` extension，支持两类读取：

```text
active_heads {
  projection_generation,
  catalog_revision,
  after_document_id?,
  limit
}

history {
  projection_generation,
  document_id,
  max_document_revision,
  before_revision?,
  limit
}
```

沿用 `parse_project_view_page_request` 的现有防混淆规则：extension 一次只接受一个 filter，
outer filter 必须带 exact `kinds`、expected Relay `authors`、Document subtype `#t` 和
bounded `limit`，并拒绝未列出的 outer / extension 字段。Relay pubkey 从 host-scoped
trusted identity 取得，不接受客户端在 extension 中另报一个 signer。

`active_heads` 每页 transaction 内要求 current state 仍等于调用者给定 generation /
catalog revision；已变化则返回 snapshot conflict，客户端从新 meta 重启分页。这样不需要
保留每个旧 catalog 时点的完整 head set。

和当前 Project View snapshot reader 一样，extension read transaction 先取得 Community
shared advisory lock，再以 `FOR SHARE` 固定 state / membership observation。它不与其他
reader 互斥，但会等待 Document / Project View / membership exclusive writer 完成。

`history` 首屏从已验证 head 取得 `projection_generation` 与
`max_document_revision`，后续每页都要求 state 仍是该 generation，并只读取
`revision <= max_document_revision`；并发追加新 revision 不会改变已开始的历史窗口。
generation 改变则返回 snapshot conflict，不能把两个 signer generation 的历史拼成一个
verified page。

通用 `/query` 当前返回完整 signed Nostr events。Relay 不能从 `40906` 中剥离 Markdown
再声称原签名仍然有效。因此 v1 history page 在 wire 上会携带完整 revision snapshot，
CLI / Tauri 验证后默认只向调用者暴露 metadata。它是用户显式打开 history 时的已知成本，
不是 list / Project View / Role Brief 的后台预取。若真实使用证明 history 体积不可接受，
后续应新增独立的 signed revision-index contract，不能返回未签名或被改写的“半个 event”。

v1 固定 limits：

```text
active list page: default 100, max 500
history page:     default 20,  max 50
```

所有 SQL 都有显式 limit，cursor 使用 UUID / revision keyset，不使用 offset。

### 12.4 current consistency

current get：

1. 读取并验证 head；
2. 按 `revision_event_id` 或 revision coordinate 读取 projection；
3. 验证同 signer、generation、project、document、revision 和 source；
4. 返回组合后的 verified Document。

如果 head 已更新而 revision event 暂未通过某条 live socket 到达，客户端从 canonical
query 读取；不能用旧 cached body 搭配新 head metadata。

### 12.5 live invalidation

订阅 `40905 / 40906 / 40907` 收到的 raw socket payload 只是**不可信 invalidation
hint**。前端 TypeScript 不从 live payload cache-warm，也不直接解析正文：

- revision event 只使对应 immutable coordinate 值得通过 native verified get 读取；
- head event 使 current Document cache stale；
- meta event 使 meta / active-list observation stale；
- native 层验证后的 `reset: true` 才清空整个 Document generation cache。

即使攻击者伪造一个看似 Relay event 的 live payload，最多触发一次受权限和签名校验保护的
refetch，不能把 title、summary、Markdown 或 projection pointer 注入 UI / Role Brief。
需要 warm immutable revision 时也必须调用 native / SDK verified read。

Desktop 沿用 Project View live sync 的短 burst coalescing。React Query key 必须包含
Community ID 与 Relay identity；不能因为 App remount 就省略作用域。

### 12.6 不做 search

v1 不实现：

- Markdown FTS；
- NIP-50 Document search；
- semantic search；
- title fuzzy search；
- vector embedding。

初版 list 可由客户端对已加载 metadata 做本地过滤。需要 server search 时必须单独设计
索引、新鲜度、权限过滤和 snippet 泄露边界。

## 13. 权限与信任边界

### 13.1 权限矩阵

| Actor | list / get / history | create / update / delete |
|---|---:|---:|
| current Human Community member | 允许 | 允许 |
| current managed Agent + owner 合格 | 允许 | 需要 active Assignment + Runtime fence |
| 已认证但不是 current member | 拒绝 | 拒绝 |
| channel-restricted credential | 拒绝 | 拒绝 |
| banned actor 或 managed owner | 拒绝 | 拒绝 |
| timed-out current member 或 managed owner | 允许 | 拒绝 |
| anonymous | 拒绝 | 拒绝 |

具体 scope：

- read：legacy empty scopes 或 `messages:read`，且 credential 必须 Community-global；
- write：通用 ingest allowlist 要求 `messages:write`，且 credential 必须 Community-global；
- membership / ban / managed owner 判断复用 Project View 当前 DB helper；
- Role、Assignment、Resource 或 Context Reference 不授予额外权限。

这里刻意保持当前 Buzz moderation 语义：timeout 是 write block，不是 read ban；当前
`project_view_authorized_pubkey` 读取 helper 检查 membership 与 active ban，并不因
timeout 隐藏 Project View。Document read helper 必须保持一致，Document write 的
transaction 内 security gate 则继续拒绝 actor 或 managed owner 的 active timeout。

首版不增加 Document / Resource ACL。

### 13.2 私有读取 gate 的覆盖面

Document protocol kinds 包括 command，因为 command content 也包含完整 Markdown。
Community-private gate 必须覆盖：

- WebSocket REQ；
- COUNT；
- HTTP `/query`；
- HTTP `/count`；
- wildcard / explicit filter classification；
- search rejection；
- initial subscription snapshot；
- local fan-out；
- Redis fan-out；
- reconnect replay。

应抽象：

```text
is_community_private_protocol_kind(kind)
credential_can_read_community_private(scopes, channel_ids)
current_principal_can_read(community, pubkey)
```

而不是复制一套只在 `/query` 生效的 Document 判断。安全测试必须证明无 kinds filter 的
wildcard 也不能混出 Document / Project View event。

### 13.3 Markdown 是不可信内容

Relay 保存和签名只证明：

- 谁提交；
- Project 接受了哪一版；
- revision / canonical time；
- 读取内容没有被未检测地修改。

它不证明内容安全、正确或仍然有效。Human / Agent 读取 Guide 后仍受：

- instruction hierarchy；
- sandbox；
- approval；
- tool permissions；
- Runtime capability；
- 外部系统 ACL；
- 不可逆操作安全规则。

Desktop 必须复用现有 Markdown 安全渲染路径，不能为 Document 单独开放 raw HTML /
script。代码块只展示，不自动运行。

### 13.4 Secret 规则的实现边界

Resource metadata 和 Document 不得保存 Secret，但通用系统无法用一个可靠 regex 证明任意
Markdown“不含 Secret”。

v1 实现：

- 不提供 `password`、`token`、`private_key` 等结构化字段；
- UI / CLI 在 create / update 和 legacy migration 时明确警告；
- 可做高置信度 heuristic warning，但不把它描述成安全证明；
- warning 默认不把疑似 Secret 值回显到 log；
- telemetry、metrics 和 tracing 不记录 title、summary、Markdown、locator；
- 迁移 legacy Resource 必须人工审查，不自动发布 locator；
- 需要 Secret 时 Guide 只说明如何从外部 Secret Manager 获取短期值。

普通 tombstone 仍保留历史，因此误写 Secret 后不能把 delete 当作擦除。Document v1
enable 之前必须随代码库交付最小 incident runbook，并至少覆盖：

1. 立即 disable 该 Community 的 Document capability，停止新写入，并让普通协议
   list / get / history 与 capability advertisement fail closed；
2. 立即在外部系统轮换或撤销泄露的 Secret，不能等待 Buzz 内的后续处理；
3. 只保留用于调查的 Community、Document / revision / event ID 与 audit coordinate，
   log、工单和通知中都不复制正文或 Secret；
4. 评估 Agent workspace、CLI 输出、Desktop cache、备份和已下载副本的暴露范围，按组织
   流程通知 / 升级；
5. 不把 tombstone 或数据库 hard delete 当成擦除；未来 scrub 是独立治理设计。只有
   incident owner 完成轮换与影响评估、确认残留历史可继续向成员开放后才可重新
   enable；若泄露的是必须擦除的数据而非可失效 credential，则保持 disabled等待独立
   scrub / recovery决定。

disabled 后的普通成员 / Agent protocol read 不可用。若组织确实需要 forensic access，
只能通过另行审计、最小授权的 operator procedure 实现，不能给普通 Document reader 留
绕过开关的后门。阶段 2 canary 前要演练一次“疑似 Secret → disable → rotate → assess →
reviewed re-enable”，确认监控和错误文本不泄露内容。

### 13.5 signer generation

当前 Project View 假设 stable Relay signer。Document history 会无限增长，不能假设 signer
rotation 可以在一个小 transaction 内重签全部 revision。

v1 规则：

- stable signer 是 enable readiness 的硬前提；
- signer 改变时立即保持 / 设置 `project_document_enabled = false`；
- 普通 reader 不接受 expected Relay identity 之外的 revision；
- 初始 v1 不复用只覆盖有限 current heads 的 Project View reproject，也不承诺分页构建后
  可以 O(1) 激活无限历史；
- signer 已改变时，只能恢复原 stable signer，或保持 capability unavailable，等待
  hardening migration。

hardening 阶段必须先增加明确的 inactive-generation staging / visibility model，再实现
`reproject --all-revisions`。所有 generic point query、history query 和 event lookup 都
必须只返回 active generation；不能仅把新 event 插入当前 `events` 表后声称“尚不可见”。
在这套表、query gate 和 activation transaction 落地前，在线或离线 signer rotation 都
不是 Document v1 已支持的操作。canonical Document / Revision 不会丢失，但客户端
capability 保持 fail closed。

## 14. Project View v3 领域演进

### 14.1 为什么一定是 v3

v3 变化同时涉及：

- Resource 删除旧 `resource_type / locator / description` authority；
- 增加开放 `resource_kind`；
- 增加 mandatory `guide_document_id`；
- 给所有 active Project View objects 增加新的 Context Reference set；
- 增加跨 Document domain 的目标校验和删除保护；
- 修改 Resource / Role projection；
- 修改 Desktop / CLI serializer；
- 修改 Role Brief closure。

这些都不是“旧客户端忽略也安全”的 display-only 字段。旧 writer 如果继续写 v2 Resource，
会丢失 Guide；旧 reader 会把 v3 Resource 判断为非法。必须使用 major cutover。

### 14.2 代码组织

在 `crates/buzz-project-view/src/v3/` 新增：

```text
mod.rs
model.rs
project_object.rs
role_continuity.rs
context_reference.rs
validation.rs
projection.rs
```

v3 可以抽取和复用 v1/v2 内部 reducer helper，但 public wire structs 必须独立且
`deny_unknown_fields`。不要给 `ProjectViewObject`、`ProjectResource`、
`ProjectObjectCommand` 或 v2 `RoleCommand` 原地加字段。

`SchemaVersion` 增加 `V3 = 3`；`communities.project_view_schema_version` constraint 扩为
`IN (1, 2, 3)`。

这绝不等于“只改一个 enum / CHECK”。当前 Rust 与 migrations 中有大量
`schema_version == 2` / `schema_version = 2` guard，它们保护：

- Runtime supervisor / active Assignment lookup；
- moderation 对 owner / active Assignment 的保护；
- membership-coupled Role consistency；
- Work responsibility / Commitment；
- Checkpoint / Handoff append-only history；
- Project View readiness / snapshot query；
- NIP-11 与 Relay handler routing；
- ACP v2 Role Brief。

阶段 4 必须先用代码搜索形成逐项 inventory，再决定每一处是：

```text
仍只适用于 v2
或
Role-continuity semantics 适用于 v2 / v3
或
必须拆成独立 v3 branch
```

SQL migration 要 `DROP / ADD` 所有 state / object schema constraints，并
`CREATE OR REPLACE FUNCTION` 更新以 `schema_version = 2` 早退的 deferred validators。
Rust 侧必须审计 `project_runtime`、`moderation`、membership、`project_view_v2` DB /
Relay、NIP-11、admin 和 ACP。禁止全局文本替换成 `>= 2`；parser / wire branch 仍然必须
精确区分 major。

当前分支至少需要显式处理以下数据库硬编码，而不是期待新 CHECK 自动放行：

```text
communities_project_view_schema_version_check
project_view_state_schema_version_check
project_view_objects_schema_check
project_view_objects_v2_fields_check
```

v3 migration 必须 drop / recreate 这四个约束，并保留 v2 已建立的
`role_level / responsible_role_id` shape。还必须 `CREATE OR REPLACE` 当前已知的：

- `0026_project_role_continuity.sql` 中
  `project_role_continuity_validate_community` 及 object schema = 2 checks；
- `0027_project_role_assignment_state.sql` 中
  `project_role_continuity_validate_counts`；
- `0028_project_work_commitments.sql` 中 schema 不是 2 就早退的 validator；
- `0029_project_role_history.sql` 中 schema 不是 2 就早退的 validator；
- checked-in schema-hardcode inventory 中登记的其余同类 function。

Rust inventory 不能只看 `buzz-project-view`。应把约三十余处 schema-2 判断逐项登记到
迁移 checklist，覆盖 `project_runtime`、`moderation`、membership、DB readiness、
Relay handler / query、NIP-11、admin 与 ACP。schema 3 的回归 fixture 必须分别证明
owner / ban、Assignment、Runtime fence、Work responsibility、Commitment、Checkpoint
和 Handoff 仍受原有 v2 规则保护。

### 14.3 Project Resource v3

目标 body：

```rust
ProjectResourceV3 {
    name: String,
    resource_kind: String,
    summary: Option<String>,
    guide_document_id: Uuid,
}
```

规则：

- `name` required，最多 256 bytes；
- `resource_kind` required，最多 64 bytes；
- `resource_kind` 必须匹配 `[a-z0-9][a-z0-9._-]{0,63}`；
- unknown `resource_kind` 合法；
- UI 可以建议常用词，但 Relay 不用它驱动行为；
- `summary` optional，最多 4,096 bytes；
- `guide_document_id` required；
- Guide 必须是同 Community active Document；
- 不保留 locator 作为第二权威字段；
- 地址、安装、配置和访问方式进入 Guide Markdown。

推荐 kind 是 UI vocabulary，不是协议 enum：

```text
repository
mcp
skill
plugin
server
database
dataset
secret_manager
service
environment
design
external_document
external_link
artifact
```

### 14.4 Context Reference

v3 在现有 structural relations 旁增加 closed tagged union，wire 固定为：

```rust
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum ProjectContextReference {
    Resource {
        resource_id: Uuid,
    },
    Document {
        document_id: Uuid,
        mode: DocumentReferenceMode,
        document_revision: Option<u64>,
    },
}

#[serde(rename_all = "snake_case")]
enum DocumentReferenceMode {
    Live,
    Pinned,
}

struct ProjectViewObjectV3 {
    // identity / revision / actors / data / existing structural relations
    context_references: Vec<ProjectContextReference>,
}
```

shape 与语义：

- Resource 不允许 `mode / document_revision`；
- Document `mode = live` 必须省略 `document_revision`；
- Document `mode = pinned` 必须携带正数 `document_revision`；
- strict parser 通过 presence-aware deserializer 区分 omitted 与 explicit `null`；
  `document_revision: null` 不是 canonical Live wire，必须拒绝；
- Context Reference 不进入原 `ProjectViewRelations` 五个槽；
- 不表达 permission、dependency、execution、ownership 或 state propagation；
- Markdown URL / deep link 不自动成为 Context Reference。

### 14.5 source / target 规则

| Source | 可引用 Resource | 可引用 Document |
|---|---:|---:|
| Project Profile | 是 | 是 |
| Goal | 是 | 是 |
| Role | 是 | 是 |
| Plan | 是 | 是 |
| Stage | 是 | 是 |
| Requirement | 是 | 是 |
| Issue | 是 | 是 |
| Work | 是 | 是 |
| Resource | 否 | 是 |
| Checkpoint / Handoff | v0 不直接支持 | v0 不直接支持 |

target：

- Resource target 必须是同 Project active Resource；
- Live Document target 必须是同 Project active current Document；
- Pinned target 必须存在对应 active-content revision；
- Pinned target 的 Document current head 可以已经 tombstone；
- tombstone revision 不能成为 Pinned target。

Resource 的 primary Guide 使用独立 `guide_document_id`，不从 context set 推断。

### 14.6 canonical set

每个 object 最多 64 个 Context Reference。它们是 canonical set，不是用户排序列表：

- 排序顺序固定为 Resource 在前、Document 在后；
- 同类先比较 UUID 的 16 个 canonical bytes；
- Document 同 UUID 时 Live 在 Pinned 前，Pinned 再按 revision 升序；
- duplicate 定义为完整 coordinate
  `(type, target UUID, mode?, document_revision?)` 完全相同；
- 同一 Document 可以同时有 Live、多个不同 Pinned revision；
- 不保存 UI position；
- 序列化和 projection 始终使用同一 canonical order。

这避免仅拖动顺序就推进 `object_revision`，也让 projection / fixture 确定性更强。

### 14.7 v3 mutation

ordinary object command 保留：

```text
schema_version: 3
expected_project_revision
acting_assignment_id?
runtime_fence?
request
```

create 为 object 提供完整 initial `context_references`，默认空。update patch 增加：

```text
context_references: Option<Vec<ProjectContextReference>>
```

`Some` 表示替换完整 canonical set，`None` 表示保持不变。CLI 的 `context add/remove`
便利操作先读 exact project revision 和完整 set，在本地修改后提交 replacement；依旧受全局
Project View revision CAS，不做自动 rebase。

`project_context_enabled = false` 时 reducer/coordinator 要求 create set为空。update的
`None` 保持当前 set；replacement只有在 transaction内证明是当前 canonical set 的子集
时才允许，因此可以逐项 remove / clear，却不能新增、重新加入或改 Live / Pinned
coordinate。任何非 subset raw v3 command都返回
`unavailable:project_view:context_capability`。首次 Stage 5 rollout的 current set本来为空；
阶段 6 只有在 CLI、Desktop、RoleBriefV3 closure和旧 v3 writer round-trip tests全部
ready后才 enable并广告 `buzz-project-context-v1`。

Resource patch 只允许：

- name；
- resource kind；
- summary；
- guide document ID；
- context Document references。

Document capability 暂时 not-ready 时，不应让整个 Project View v3 停摆：

- 保持既有 Guide / Document refs 不变的普通 Project View update 可以继续；
- 新增或更换 Guide / Live / Pinned Document target 时返回
  `unavailable:project_view:document_capability`；
- Resource delete、移除 Context Reference 等不引入新 Document target 的操作可以继续；
- 新 target 的 existence 仍以 transaction 内 canonical Document tables 验证，不能从
  stale client metadata 判断。

### 14.8 pure reducer 与稀疏跨域 proof

`buzz-project-view` 不能为了验证 Document target 直接查 DB，也不能为了一个最多 64
条引用的 command 把全 Community 的所有 Document revision 载入内存。

v3 coordinator 先做不依赖状态的 command shape / byte / count 验证，再加载 current
Project View state，用 pure helper 计算 command **新引入或替换**的 target delta。它只为
最多 64 个新增 Document coordinates，加上一个更换后的 Resource Guide coordinate 做
point query；原 canonical object 中保持不变的 Guide / ref 不要求 Document capability
重新证明，因此与 14.7 的 degradation 规则一致。Resource target 直接从已加载的 Project
View state 验证；新增 Document target 通过 bounded point query 构造稀疏 proof：

```rust
ReferenceTargetProof {
    documents: BTreeMap<DocumentCoordinate, DocumentTargetState>,
}
```

pure reducer 接收：

- current Project View state；
- typed v3 command；
- canonical actor/time；
- `ReferenceTargetProof`；

然后完成所有同 Project target、source rule、duplicate、limit、delete incoming relation
校验。任何**新引入**的 Document coordinate 若 proof 缺失就 fail closed；不能把“没有
查到 proof”解释为 target 合法。已存在但保持不变的 coordinate 由 current canonical
state 与 deferred constraints 证明；一旦 command 移除后再添加，就重新视为 new target。

Resource / Document delete 的 incoming 校验不扫描全 state 或 JSON：Project View
Resource delete 使用 normalized reverse index，Document delete 使用 Guide / Live ref
reverse index。DB deferred trigger 再独立验证 canonical rows，防止 reducer /
persistence drift。

### 14.9 Role 在 v3 的特殊处理

当前 v2 的 Role 不是简单复用 ordinary object head；Role Continuity 有独立
`RoleDefinition` entity / projection，v2 `RoleCommand` 也硬校验 schema 2。

因此 v3 必须：

- `ProjectObjectCommandV3` 继续负责 Role definition 的 create / update / deactivate /
  tombstone；
- `RoleCommandV3` 只负责 Proposal、Assignment、Commitment、Checkpoint、Handoff 等
  continuity operation，不能再提供第二条 Role definition CRUD 路径；
- 定义携带 `context_references` 的 `RoleDefinitionV3`；
- 每个 non-tombstoned Role（包括业务字段 `active = false`）的 changed head **恰好只有
  一个** `RoleDefinitionV3` entity projection；
- `project_view_objects.projection_event_id` 与 Role continuity 的 Role pointer 都指向这
  一个 event，不再额外发 ordinary non-tombstoned Role head；
- Role tombstone 仍使用 ordinary v3 object tombstone，因为被删除后不再有 active
  `RoleDefinition`；
- 保留 Assignment、Commitment、Checkpoint、Handoff 与 Runtime fence 现有业务语义；
- 不得在 v3 capability 下继续发 schema 2 Role command；
- Role Context Reference 的 DB source identity 仍是稳定 Role object ID。

Checkpoint / Handoff 的现有 closed reference union 在 v0 不增加 Document variant。Role
Brief 可以沿它们引用到的 active Project View object，再读取该 object 的 context set。
projection / snapshot tests 必须断言一个 non-tombstoned Role 只计入一个 active object、
changed head coordinate 不重复、object pointer 与 Role entity pointer 相同；deactivate
只把 `active` 变成 false，仍保留 RoleDefinition head，只有 tombstone 才切 ordinary
tombstone head。

## 15. Project View v3 persistence 与跨域完整性

### 15.1 Resource Guide column

`project_view_objects` 增加 nullable：

```text
guide_document_id UUID
```

shape：

- v3 active Resource：required；
- 其他 active type：null；
- tombstone：null；
- v1/v2 legacy row：null。

它使用 `(community_id, guide_document_id)` FK 指向 `project_documents` identity row；
deferred trigger 再验证 target current state active。body 中的
`guide_document_id` 与 column 必须完全一致。

v3 同时必须修正 ordinary object 的 source topology。当前
`project_view_objects.source_event_id` 是 `NOT NULL`，无法表达 Resource 由 operator
cutover 形成的新业务 revision。migration 增加：

```text
source_type            nostr_event | operator | system
source_change_id       BYTEA NOT NULL
source_event_id        BYTEA nullable
source_provenance_id   UUID nullable
```

并移除旧 `source_event_id NOT NULL` 假设。v1/v2 row 的
`source_provenance_id` 保持 NULL；v3 current / tombstone row 必须非 NULL，并 deferred
FK 到新增 immutable ledger：

```text
project_view_object_provenance
├── community_id
├── provenance_id
├── object_id
├── object_type
├── source_type
├── source_change_id
├── source_event_id
├── source_project_revision
├── source_actor_pubkey       nullable
├── legacy_mutation_event_id nullable
└── project_view_change_id   nullable

PK (community_id, provenance_id)
UNIQUE (community_id, object_id, source_change_id)
FK legacy_mutation_event_id → project_view_mutations
FK project_view_change_id   → project_view_changes
```

legacy Resource cutover的 Human evidence不继续依赖可编辑 staging row。cutover
transaction把每个 reviewed entry复制到：

```text
project_view_v3_committed_resource_entries
├── community_id
├── cutover_change_id
├── resource_id
├── legacy_object_revision
├── legacy_projection_event_id
├── legacy_body_digest
├── mapping_entry_digest
├── reviewed_v3_payload
├── v3_payload_digest
├── guide_document_revision
├── guide_head_event_id
├── guide_revision_event_id
├── guide_content_digest
├── reviewed_by_pubkey
├── reviewed_at_unix_micros
├── review_digest
├── review_signature
└── committed_at

PK (community_id, cutover_change_id, resource_id)
FK cutover_change_id → project_view_changes
```

这张 restricted child ledger从 insert起禁止 UPDATE / DELETE；staging mapping在 commit后
可以标 consumed或按运维策略清理，但不再参与 provenance验证。Resource cutover
provenance以 `(community_id, source_change_id, object_id)` deferred FK到 committed
entry；普通 object不允许出现该 linkage。operation receipt持久化 base meta /
project revision / generation与 manifest digest，child持久化每项 legacy pins，因此
mapping / review digest可以只依赖 immutable records重算，不需要回读 staging或保存 legacy
locator正文。

closed shape：

- legacy v1 origin 只设置 `legacy_mutation_event_id`；v2/v3 / operator / system origin 只
  设置 `project_view_change_id`；
- `nostr_event` 要求 `source_event_id = source_change_id`；
- operator / system 要求 `source_event_id IS NULL`，并由 referenced
  `project_view_changes` 核对 source type / audit linkage；
- v3 object row 的 `(community_id, source_provenance_id)` FK 命中 ledger 后，deferred
  validator 再核对 object ID / type、source triple和 source project revision。

actor parity 分成两个独立不变量，不能把 `updated_by` 和 source actor混为一谈：

- `ledger.source_actor_pubkey` 与 referenced mutation / typed change 的 actor一致；member
  event source必须是 32-byte actor，operator / system change按其 closed source shape可为
  NULL；
- object 的业务 `updated_by` 按该 operation 的 outcome验证。普通 member change与
  source actor一致；Resource cutover则必须等于 immutable committed child entry 的
  `reviewed_by_pubkey`，而 operator ChangeSource actor可为 NULL；22.8 的机械 repair
  不改变 business payload，因此显式保留并验证 repair前的 business `updated_by`，不能
  套用普通 member equality。

不能给全部 object row 建到 `project_view_changes` 的无条件 FK：当前 0026 明确保留 v1
`project_view_mutations`，v1 → v2 cutover 没有为每个 legacy ordinary object 伪造 v2
change。v3 cutover 的 Rust preflight 负责为每个 current / tombstone object 构造一条
normalized provenance：

- v1 create / update / delete 可从 mutation row 核对 object pair；
- v1 `initialize` 是显式 special case：当前 receipt 的 object ID / type 都是 NULL，且
  一个 command 可同时创建 Profile 与多个 Goal。preflight 加载 exact stored signed
  event，用现有 strict parser 解析 `InitializeMutation`，核对 Profile ID = Community
  ID、每个 Goal ID / type、actor 与 revision，再写 per-object ledger row；
- v2/v3 origin 从 exact typed `project_view_changes` 构造；
- SQL deferred validator 只验证 closed ledger shape 与真实 FK，不尝试在 PL/pgSQL
  调 Rust parser。

provenance ledger、committed Resource entry、`project_view_mutations` 与
`project_view_changes` 增加 UPDATE / DELETE append-only guard；否则 object不变化时
source / reviewer evidence仍可能被静默篡改。verify会重跑 Rust strict provenance
parity。新 v3 change在业务 transaction内直接写 ledger；v1/v2 history不被改写成伪造的
v2 change。

v3 object projection 使用与 continuity head 相同的 typed source union，不把 operator
change ID 伪装成 Nostr event ID。

Resource cutover row 使用 operator `source_change_id`；`updated_by` 则来自下一节定义的
reviewer-signed mapping evidence。前者表示“谁执行 / 接受了原子状态转换”，后者表示“谁
审阅并授权了这个 Resource 的业务内容”。其他 object 保留原 source 三元组，并分别指向
由上述 preflight 验证的 immutable provenance row。

### 15.2 两张 normalized Context Reference 表

相比一个带 polymorphic UUID 的表，首版使用两张有真实 FK 的表。

migration 同时增加：

```text
communities.project_context_enabled BOOLEAN NOT NULL DEFAULT FALSE
```

CHECK 要求它只有在 `project_view_schema_version = 3` 时才可为 true。enable / disable
使用 Community exclusive lock和 audited admin coordinator；disable只关闭
advertisement、Role Brief消费和新增写入，不删除已有 normalized rows。Project View
snapshot / `context list`仍返回 verified existing refs；mutation只允许保持或替换为
canonical subset。这样故障降级不会悄悄改 Project View revision，Human仍可逐项 remove
或 clear。

```text
project_view_resource_context_references
├── community_id
├── source_object_id
└── target_resource_id

PK (community_id, source_object_id, target_resource_id)
FK source → project_view_objects
FK target → project_view_objects
```

```text
project_view_document_context_references
├── community_id
├── source_object_id
├── target_document_id
├── reference_mode              live | pinned
├── target_document_revision    nullable
└── revision_key                generated COALESCE(revision, 0)

PK (community_id, source_object_id, target_document_id,
    reference_mode, revision_key)

FK source → project_view_objects
FK target document → project_documents
```

PostgreSQL 不提供 conditional foreign key。`reference_mode / revision` shape 使用 CHECK；
Pinned revision 是否存在且是 active-content revision，由同一 deferred validator 查询
`project_document_revisions`。Rust domain 仍暴露一个 tagged union；两张表只是更强的
canonical persistence。

### 15.3 reference row shape

deferred constraints 检查：

- source active；
- source body projection 中的 context set 与 normalized rows 一致；
- Resource source 没有 resource target row；
- target Resource active 且 object type 为 Resource；
- Live target Document current active；
- Pinned target revision state active；
- 同 Community；
- 每 source 总数不超过 64；
- tombstone source 没有 outgoing row。

需要 reverse indexes：

```text
(community_id, target_resource_id, source_object_id)
(community_id, target_document_id, reference_mode, source_object_id)
(community_id, target_document_id, target_document_revision, source_object_id)
(community_id, guide_document_id, object_id)
```

删除保护不能扫描 JSON 或 Markdown。

### 15.4 delete matrix

| 删除目标 | 阻止条件 | 不发生的行为 |
|---|---|---|
| Resource | 现有 structural incoming ref；Resource Context Reference | 不删除 Guide |
| Document | Resource Guide；Live Document Context Reference | 不删除 Resource；不删除 Pinned ref |
| source Project View object | 仍适用现有 structural target 保护 | outgoing context rows 在同事务显式清除 |

Pinned reference 不阻止 Document ordinary delete，因为它仍解析历史 active revision。
Resource delete 后 Guide 仍是普通 active Document，可以被其他对象引用或稍后单独删除。

### 15.5 Resource + Guide create 不是跨域 batch

“创建 Resource 并新建 Guide”的 Desktop wizard 是两步 saga：

1. create Project Document；
2. 用返回的 `document_id` create Resource。

第二步失败时：

- 不自动 delete Document；
- 保留它作为可复用 draft / orphan Document；
- UI 展示明确重试和选择现有 Guide；
- 不得用补偿 delete 猜测该 Document 没被其他操作引用。

这样不需要设计跨 Document revision 与 Project View revision 的双 CAS command。

### 15.6 跨域锁

Document mutation、Project View v3 mutation、schema cutover 和 membership mutation 使用
同一 Community exclusive lock。典型 race 因此有确定结果：

```text
create Live ref 先提交 → Document delete 看到 incoming ref 并失败
Document delete 先提交 → create Live ref 看到 target tombstone 并失败
```

不能只依赖 deferred FK，因为 Live / active 是跨行 lifecycle 条件，不是普通 identity FK。

## 16. Project View v3 wire 与 projection

### 16.1 command / projection kinds

v3 继续使用：

| Kind | 用途 |
|---:|---|
| `44300` | schema 3 member command |
| `40903` | schema 3 ordinary object / Role Continuity entity head |
| `40904` | schema 3 Project View meta |

Relay 根据 Community `project_view_schema_version` 只解析对应 major。schema 2 Community
不会尝试把 content fallback 解析为 v3，反之亦然。

### 16.2 v3 object projection

每个 active object projection 携带完整 canonical：

- v3 data；
- 原 structural relations；
- canonical `context_references`；
- object / project revision；
- actor/time；
- projection generation；
- source change。

Resource 的 `guide_document_id` 同时出现在 signed content 和 canonical DB column，SDK /
DB 做 parity check。Document title、summary、current revision 不复制进 Project View
projection；它们由 verified Document head hydrate。

### 16.3 v3 meta

v3 必须完整继承当前 v2 meta 的 observation boundary，不能只保留普通 object 字段：

- project revision；
- active object count；
- open Proposal count；
- active Assignment count；
- active Work Commitment count；
- append-only Checkpoint count；
- append-only Handoff count；
- exact `membership_snapshot_event_id`；
- changed heads；
- source change；
- projection generation / reset；
- canonical updated time。

无需增加 Context Reference global count 作为 wire authority。normalized ref count可以作为
DB readiness / metrics invariant，但完整 object heads 已经携带 context set。

v2 → v3 cutover meta 是 generation reset：`reset = true`、`changed_heads = []`，绑定锁内
重新生成并验证的 exact current NIP-43 membership snapshot；六类 count 与 canonical
tables 完全一致。不能沿用 cutover 前可能已过时的 membership event pointer。

### 16.4 current Role / continuity projections

v3 continuity union 只有当前已经存在的六类：

- RoleDefinition；
- Proposal；
- Assignment；
- WorkCommitment；
- Checkpoint；
- Handoff。

Work responsibility 继续是 ordinary Work projection 的 `responsible_role_id`，Runtime
supervision 继续由 runtime domain / fence 表达；二者都不是
`RoleContinuityEntity`，不得为 v3 虚构新 entity kind。每个 Checkpoint / Handoff 的稳定
ID 本身就是 entity coordinate，不存在“每个 Role 一个 current pointer”。

cutover 后默认 Role Brief current page 使用 v3 heads：

- 所有 non-tombstoned Role，包括 `active = false`；
- open Proposal，以及 active Assignment 为验证 provenance 所需的 consumed Proposal；
- active Assignment；
- active WorkCommitment；
- 每个 relevant Role 的 latest Checkpoint 与最近三个 Handoff。

这是一组由当前 query 已经定义的有界 slice。它们在 cutover transaction 内重投影为 v3。
更老的 Proposal、ended Assignment、Checkpoint 与 Handoff 不需要为了切 major 改写业务
history；v3 history reader 使用明确的 v2/v3 versioned union，分别走 strict parser，
并验证 Relay signer、Project / entity coordinate、entity revision 与 canonical
`projection_event_id`。current snapshot 绝不混入 v2 event，显式 history 才允许返回旧
major 的历史 row。

schema-major cutover本身增加一次 global project revision 和 projection generation。
非 Resource ordinary object 与 continuity entity 保留 object / entity revision、各自上次
业务变化的 project revision、actor/time；
Resource 的 reviewed 业务转换使用第 22 节规定的独立 revision / actor 规则。

### 16.5 reader 规则

v3 SDK parser：

- 只在 NIP-11 advertised v3 下接受 schema 3 current heads；
- 验证 Relay identity、generation、project revision 和 exact tags；
- 拒绝 v2 Resource shape；
- 拒绝 v3 Resource 缺 Guide；
- canonical sort / duplicate 验证 Context Reference；
- 不因为 target metadata 暂时不可用而信任 projection 中不存在的 title / summary。

## 17. SDK 设计

### 17.1 Project Document builders

公开 API：

```rust
build_create_document(...)
build_update_document(...)
build_delete_document(...)

parse_document_head(...)
parse_document_revision(...)
parse_document_meta(...)

document_head_coordinate(...)
document_revision_coordinate(...)
document_meta_coordinate(...)

build_document_head_projection(...)
build_document_revision_projection(...)
build_document_meta_projection(...)
```

builder 在生成 event 前执行：

- domain validation；
- JSON size / depth；
- serialize → parse roundtrip；
- canonical exact tags。

parser 不接受“尽量解析”。任何 unknown field、extra tag、noncanonical number 或 pointer
mismatch 都返回 stable `SdkError::InvalidProjection`。

当前 `SdkError::InvalidProjection` 的 display 文案仍写死为
`invalid Project View projection`。阶段 1 必须把它泛化为
`invalid Relay projection`，或新增 Document-specific variant；不能让合法的 Document
错误在 CLI / Tauri 中被标成 Project View 错误。现有 enum variant 匹配方必须有回归测试。

### 17.2 Verified read types

SDK 区分：

```text
VerifiedDocumentHead
VerifiedDocumentRevision
VerifiedDocumentMeta
VerifiedCurrentDocument      # head + revision 已绑定
```

raw `nostr::Event` 不直接传到 UI / Role Brief。Tauri 和 CLI 只输出 verified types。

### 17.3 Project View v3

SDK v3：

- 保留 v2 parser 供 cutover 前读取；
- 新增 schema 3 ordinary / Role entity union；
- 新增 Context Reference canonical validator；
- Resource head 不 hydrate Document；
- Role Brief assembler 显式接收 verified Document metadata，而不是自行联网；
- 为 v2/v3 都保留 golden fixtures，防止 dual reader 漂移。

### 17.4 golden protocol tests

阶段 0 固定：

- create / update / delete command JSON；
- active / tombstone head；
- active / tombstone revision；
- empty catalog / incremental / reset meta；
- v3 Resource；
- 每种 Context Reference；
- v3 RoleDefinition；
- malformed / extra-tag / cross-project / wrong-signer fixture。

Rust SDK、CLI fixture、Tauri fixture 和协议文档使用同一份 golden bytes / IDs，避免四套手写
示例各自演进。

## 18. Relay ingest 与读取接线

### 18.1 ingest routing

当前 `handlers/ingest.rs` 会把
`is_command_kind(kind) && !is_project_view_mutation_kind(kind)` 交给通用
`command_executor`。Document 需要从这条 generic branch 显式排除，但不能在那里立即
return：当前 global / channel-token gate 位于 generic branch 之后。

1. 执行 event signature / auth tag / timestamp / scope 基线；
2. `required_scope_for_kind` 把 `44301` 映射到 `messages:write`；
3. generic 条件改为同时排除 Project View 与 Document command，使 Document 继续向下走
   现有 channel derivation；
4. `is_global_only_kind` 把它的 `channel_id` 固定为 `None`；
5. 随后的现有 `auth.channel_ids().is_some()` global-event gate 拒绝 channel-scoped
   credential；
6. 通过该 gate 后，在当前 Project View special route 旁识别 `44301`，路由
   `project_document::handle_command`；
7. handler 不调用 `command_executor::persist_command_event` 或普通 append path；
8. DB restricted transaction 一次保存 command、canonical state、receipt 和全部
   projection；
9. commit 后统一 delivery。

只把新 kind 加进 `buzz-core::is_command_kind` 不够：那会把 Document 错送进现有 workflow /
DM / approval command executor，而其 persist helper 不具备本文要求的 canonical +
projections 严格单事务。对应 routing-order test 必须同时证明它不进 generic executor，
又没有绕过 global-token gate。

Relay-only kinds 在普通客户端 submission path 一律拒绝，即使 signer 字段伪装成 Relay。

### 18.2 capability readiness

每次写前检查：

- feature flag；
- Community enabled；
- PV schema 2 / 3；
- stable signer；
- Document state；
- active generation；
- meta pointer；
- schema / signer parity。

NIP-11 只有 advertised readiness 全部满足才广告 capability。数据库开关是多 pod 共享
事实，不能只用单 pod config flag。Project View v3 明确拆成两个 predicate：

```text
project_view_v3_structural_ready
    schema 3 initialized
    + canonical object / continuity / Guide / Context parity
    + exact heads / meta / membership pointers
    + signer / generation / count invariants

project_view_v3_pre_enable_ready
    project_view_v3_structural_ready
    + deployment feature ready

project_view_v3_advertised_write_ready
    project_view_v3_pre_enable_ready
    + project_view_enabled = true
    + project_view_maintenance.state = normal
```

`project_view_v3_structural_ready` 刻意不检查 enabled 或 maintenance state，供 frozen /
disabled 状态下的 cutover verify、repair verify 和 resume transaction 使用。
`project_view_v3_pre_enable_ready` 也不要求 enabled / normal，专供 initialize enable和
resume在同一 transaction切换 operational gates前检查。
NIP-11、普通 member read / write 和 Agent admission只使用
`project_view_v3_advertised_write_ready`。即使 operator 误把
`project_view_enabled` 提前设回 true，`draining / frozen` 也必须 fail closed。

22.9 的 `ProjectViewInitializeV3` 是唯一 bootstrap exception：deployment feature已
ready、存在 exact `prepare-v3` receipt、schema 3、uninitialized且
`project_view_enabled = false` 时，专用 handler允许 eligible owner提交一次 initialize。
它不允许其他 mutation、不广告 capability；初始化后仍 disabled，等待 structural verify
和显式 enable。handler仍遵守现有“current security gate before replay”：完成 current
owner / ban / archive校验后，先查 exact accepted initialize event receipt，再做 fresh
uninitialized gate；同一 event在 commit后 response丢失时可 replay，另一 initialize
event则拒绝。不能把这个 exception复用成 disabled Community的通用写后门。

Project Document 是独立 capability；它不因 PV maintenance 自动 disabled，但被 migration
manifest pin 住的 Guide 若变化会让 cutover exact check失败。

先区分两类故障：

- **projection availability**：canonical Document rows与跨域 FK / lifecycle parity健康，
  但 Document meta pointer、projection signer / generation 或 delivery不 ready；
- **canonical integrity**：Document identity / current revision、Guide / Live ref target、
  active / tombstone或 normalized parity损坏。

两个 capability 的 degradation 规则：

- 只有 projection availability故障时，省略
  `buzz-project-document-v1`；
- 只要 Project View v3 自身 state、heads、membership pointer 与 signer ready，仍广告
  `buzz-project-view-v3`；
- 此时保持既有 Guide / Document refs 的普通 PV 写和不相关写可以继续；只有新增 / 更换
  Document target 的操作返回
  `unavailable:project_view:document_capability`。
- canonical integrity故障会破坏 v3 Resource / Context不变量，因此 Document 与
  Project View v3 capability都 fail closed；不能以“Role仍健康”为理由继续广告一个无法
  证明 Guide active的 v3 snapshot。

Project View v3 structural readiness 仍验证 canonical Guide / ref identity rows一致，
但不要求 Document catalog meta 当前可提供 body。只有 projection-layer Document故障不能
把健康的 Role / Assignment / Runtime supervision 从 NIP-11 一并抹掉。

Context sub-capability 单独要求 `project_context_enabled`、normalized ref parity、dual
client 与 RoleBriefV3 closure readiness；省略 `buzz-project-context-v1` 不隐藏健康的
`buzz-project-view-v3`，但 server-side nonempty Context write gate仍保持关闭。

### 18.3 query routing

明确 Document kinds 的 query：

- 先执行 private credential / member gate；
- 再要求 `project_document_enabled` 与普通 read readiness；disabled / degraded时所有
  member / Agent list、get、history、REQ与COUNT统一 unavailable，不能因知道 event ID
  绕过 incident disable；
- 拒绝 NIP-50 search；
- 存在 `buzz_project_document` extension 时进入 restricted DB reader；
- 普通 point query仍走 event store，但必须保持同一 gate；
- COUNT 不能通过 count 差异泄露 Document 数量。

active-list 与 history extension 都在 Community shared lock 下固定 membership /
Document state；请求携带 projection generation，history 还携带首屏固定的 max revision。
任一 state mismatch 返回 snapshot conflict。

### 18.4 fan-out

Document event 不进入 channel fan-out。它是 Community-global private event：

- 只发给当前仍可读的 connection；
- 连接上的 `read_eligible` 只是 credential 初筛；
- membership / ban 变化后必须失效或重新检查；
- Redis 接收侧不能因为事件来自另一个 pod 而跳过 private gate；
- commit 后把 revision、head、meta 全部交给 delivery scheduler；
- 不承诺 local socket、Redis 或跨 pod recipient 观察到同一顺序；
- 客户端不依赖到达顺序，仍按 pointer / query 验证。

## 19. CLI 设计

### 19.1 command surface

新增顶层：

```text
buzz documents list
buzz documents get <document-id>
buzz documents history <document-id>
buzz documents create
buzz documents update <document-id>
buzz documents patch <document-id>
buzz documents delete <document-id>
```

Resource / Context Reference 仍属于 Project View：

```text
buzz project-view create resource --expected-project-revision <n> --data <file|->
buzz project-view update resource <id> --expected-project-revision <n> --patch <file|->
buzz project-view context list <object-id>
buzz project-view context add <object-id> ...
buzz project-view context remove <object-id> ...
```

增加只读组合便利命令：

```text
buzz resources guide <resource-id> [--revision <n>] [--content-only]
```

它在客户端依次读取 verified Resource 和 Guide，不引入 server Resource endpoint。
在 v2 Resource 上调用时明确返回“Project View v3 / Guide unavailable”，不能把 legacy
locator 假装成 Guide，也不能自行生成 Document。

`--revision <n>` 始终表示 **Guide Document revision**，不是 Resource object revision。
输出同时带回：

```text
resource_project_revision
resource_object_revision
resource_head_event_id
guide_document_id
guide_document_revision
guide_head_or_revision_event_id
```

它表达“这个 Guide coordinate 由该 Resource revision 观察到”，不声称 Document body
冻结在 Resource revision。读取 current Resource + current Guide 时，CLI 用 bounded
Project View meta A / B retry 证明 Resource head 来自同一 current snapshot；然后按已验证
`guide_document_id` 读取 Document。显式 `--revision` 则读取该 Guide 的历史 snapshot。

### 19.2 list

```text
buzz --format json documents list
buzz --format compact documents list
```

默认只输出 active metadata：

```json
[
  {
    "document_id": "...",
    "title": "...",
    "summary": "...",
    "document_revision": 8,
    "updated_at": "...",
    "updated_by": "...",
    "head_event_id": "..."
  }
]
```

不预取正文。CLI 自动处理 snapshot pagination；catalog 变化导致 snapshot conflict 时，
有限次从新 meta 重启。耗尽后沿用现有 `CliError::Conflict`：exit 5、stderr
`retryable: false`；不额外创造一种“可自动重试的 conflict”，调用者如需重试必须重新开始
一次明确的 list。

### 19.3 get

```text
buzz documents get <id>
buzz documents get <id> --revision 7
buzz documents get <id> --content-only
```

- 无 `--revision`：读取 current active revision；
- 指定 revision：读取 pinned snapshot，包括 Document 当前已删除的情况；
- 默认遵循全局 JSON / compact output；
- 只有显式 `--content-only` 才把 raw Markdown 单独写 stdout，便于 Agent / pipe 使用；
- tombstone current get 返回 not-found / deleted metadata，不回退到最后 active body；
- `--content-only` 不向 stderr / log 复制正文。

### 19.4 create / update

输入方式：

```text
--title <text>
--summary <text>
--clear-summary               # update only
--content <text>
--content-file <path>
--content -                    # stdin
--document-id <uuid>           # create only；默认由 CLI 生成 UUID v4
```

closed 参数语义：

- create：`--title` required；`--content` 与 `--content-file` exactly one；
  `--summary` 省略表示 `None`，不接受 `--clear-summary`；
- update：`--title` required；`--content` 与 `--content-file` exactly one；
  `--summary` 与 `--clear-summary` exactly one；
- `--summary ""` 不是 clear，按非规范空值拒绝；
- update 不把 flags 缺失解释为“从最新值补全”，所以提交的确实是完整 snapshot。

大段内容推荐 file / stdin。当前 CLI 通用 `read_file_or_stdin` 会无界读入内存，不能直接
复用于 Document。新增 `read_bounded_file_or_stdin(limit)`：file 先检查 metadata 并仍以
`limit + 1` bounded read 防 TOCTOU，stdin 流式读到 `limit + 1` 立即拒绝；patch / JSON
input 同样使用各自的显式 cap。内存中字符串也在签名前执行相同 byte limit。

update 必须显式提供：

```text
--expected-revision <n>
```

delete 同样必须显式提供：

```text
buzz documents delete <id> --expected-revision <n>
```

默认不“先读最新再覆盖”，防止脚本无意 lost update。交互式 Desktop 可以在打开编辑器时
保存 base revision，再显式提交。

write：

- 使用 exact signed command；
- 网络 retry 只重发相同 event bytes；
- 验证 receipt；
- 读取并验证 Relay-signed head / revision；
- JSON 输出现有 write response 加 Document IDs；
- 409 映射现有 CLI exit 5。

不能直接复用当前 `BuzzClient::submit_event` 的最终 error mapping：它会 retry 503，并可能
把耗尽后的 502–504 变成 `DeliveryUnknown`，从而丢失 canonical
`unavailable:project_document:*`。新增供 Document 与 Project View v3 共用的 typed
project-command submit policy：

1. 若尚无任何 ambiguous attempt，Relay 返回可解析的 stable 409 / 503 /
   `error:project_document:*`，分别映射 conflict / unavailable / definitive internal；
2. connect-before-send 等可证明未到达的失败可以用同一 event bytes有限重试；
3. timeout、response body中断、proxy 502 / 504 等一旦使 delivery可能发生，就记录
   `delivery_may_have_occurred = true`；
4. ambiguous 后按预期新 revision coordinate读取 Relay-signed immutable revision，只有
   其 `source_event_id` 等于原 command event ID、document / revision / state都一致时才
   恢复 success；
5. 无法证明 accepted 时返回现有 `DeliveryUnknown`（exit 2、`retryable:false`）；后续即使
  看到 unavailable也不能倒推第一次没有提交。

stable `error:project_document:*` 只有在 transaction commit **之前**才能成为
definitive response。Document coordinator必须在 commit前构造并验证完整 receipt /
response message bytes，把它们与 canonical result一起落库；commit后只做不会改变
accepted outcome的 delivery scheduling / best-effort fan-out。若 commit后发生 response
transport中断或任何无法返回预构造 bytes的故障，client只能得到 generic ambiguous
failure并走 exact read-back，Relay不能再合成 stable internal error。这一边界不能照搬
当前某些 Project View 路径中“commit后才调用 `response_message`”的顺序。

read/list 没有写入歧义，canonical unavailable可直接映射 exit 4。相同 policy也用于 v3
Resource / Context mutation的 `unavailable:project_view:document_capability`。

### 19.5 exact patch

```text
buzz documents patch <id> \
  --expected-revision 7 \
  --patch-file change.diff \
  [--output merged.md]
```

流程：

1. 读取并验证 revision 7；
2. 对 `content_markdown` 应用 unified diff；
3. 要求每个 hunk exact context、zero fuzz、无 offset guessing；
4. 任一 hunk 不匹配则本地 input error，不发 event；
5. 生成完整 next snapshot；
6. 用 expected revision 7 发 update；
7. server conflict 时返回 exit 5；只有调用者显式给了 `--output` 才把 next content 写到该
   路径；
8. 不自动 fetch revision 8 并 rebase。

CLI 不静默创建临时文件，也不在 stderr 回显 next content。title / summary 局部变化仍
通过显式 flags 提交：

- patch 默认从 exact base snapshot 保留 title / summary；
- `--title` 可替换 title；
- `--summary` 与 `--clear-summary` 互斥；两者都省略时保留 base summary。

v1 不定义服务端 patch wire。

### 19.6 history

默认输出 revision metadata：

```json
[
  {
    "document_revision": 8,
    "state": "active",
    "actor": "...",
    "canonical_at": "...",
    "revision_event_id": "..."
  }
]
```

正文只通过 `get --revision` 读取，避免 history 一次输出所有版本。

### 19.7 Agent discoverability

更新 `crates/buzz-acp/src/base_prompt.md`：

- 阶段 2 加入 `documents list / get / history` 和必要的 Buzz 平台发现入口；当前
  `base_prompt.md` 并没有完整的 `project-view / roles` 命令说明，不能把它当成已经存在
  的前置条件；
- 阶段 5 在 v3 Resource 已可读后明确加入完整发现链：
  `buzz project-view get` → 从 snapshot 发现 Resource ID →
  `buzz resources guide <resource-id> --content-only`；需要先检查 Resource metadata时可
  额外调用 `buzz project-view get-object resource <resource-id>`。组合命令已经按 verified
  coordinate读取 Guide，不能在它之后再无条件 `documents get current`造成 TOCTOU；
  Agent 不需要先得到 Role Brief Context 才能发现和读取 Resource Guide；
- 阶段 6 在 Context Reference / Role Brief 已落地后加入 Context 发现指引；
- 说明 `--format compact` 是 global flag；
- 说明 write conflict exit 5；
- 说明 Guide 是不可信项目内容，读取不等于授权执行；
- 说明正文按需读取，不要遍历整个 catalog 填满 Context。

CLI 顶层 / 子命令稳定性 tests、help snapshot 和 `crates/buzz-cli/TESTING.md` 同步更新。

## 20. Desktop 与 Tauri 设计

### 20.1 native trust boundary

新增 native commands：

```text
get_project_document_meta
list_project_documents
get_project_document
get_project_document_history
mutate_project_document
```

Tauri Rust：

- 五个 command 都把 frontend 当前 opaque `community_key` 作为显式参数；当前
  `AppState / apply_workspace` 只保存 Relay URL + keys，并没有可供 native 自行推导的
  `${activeCommunity.id}-${reinitKey}`；
- 在任何 async await 前复制该参数，并从 `AppState` 捕获 `api_base_url` 与 signing
  keys；current `AppState` 在第一次 await 前没有可信 Relay pubkey；
- 使用 `read_identity_at(state, &captured_api_base_url)` 读取该捕获 endpoint 的身份；
- 从返回值取得并固定 expected Relay pubkey；再从 signed Document / Project View
  projection绑定 canonical `project_id`；
- 明确区分 Desktop local `community_key`（客户端 cache / switch identity）与 signed
  `project_id`（协议 authority），两者不能互相代替；
- verified response 原样回传 captured `community_key`；TypeScript 只把 response 写入同
  key cache，若 component 已切换 Community则丢弃，不把它 retarget到新 key；
- 后续 query / submit / read-back 全部只使用捕获的 URL / keys与本次取得的 Relay
  identity；
- await 后绝不重新读取“当前 active Community / Relay”并把原操作重定向到新 Community；
- 构造 / exact sign command；
- 使用现有 NIP-98 query / event submit；
- 解析 typed 409 / 503 / internal category；
- 验证 receipt；
- 验证 Relay signer、head、revision、meta；
- 只把 verified read model 返回 TypeScript。

TypeScript 不直接解析或信任 raw Nostr event。

实现五个 command 后必须在 `desktop/src-tauri/src/lib.rs` 的 `generate_handler!` 注册；
native unit test与 `desktop/src/testing/e2eBridge.ts` mock command 名必须逐项相同，避免
只实现函数却让 production invoke不可达。

当前 `query_relay_at_with_keys / submit_signed_event_at_with_keys` 把失败压成
`Result<_, String>`，不足以实现上述 contract。新增保留
`status + stable error category + retry provenance` 的 typed helper；既有调用方可以继续
使用 string wrapper。Document native error 映射至少区分：

```text
SnapshotConflict       # list / history 409
RevisionConflict       # mutation 409
Unavailable            # canonical unavailable 503
DeliveryUnknown        # write可能已经到达
Restricted / Unsupported / Internal
```

error type 不保留或回显 Document body。

`get_project_document_meta` 是 identity bootstrap：它在同一捕获 context 中读取 Relay
identity并验证 `40907`，返回 local community key、signed project ID、Relay pubkey 与
verified meta。`list_project_documents` 接收该 meta 的 generation / catalog revision，
server mismatch 返回 `SnapshotConflict`；TS 不自行从 raw event推导 meta。

native race test 在 identity request await 期间切换 Community，断言旧调用要么在捕获的
旧上下文完成、要么明确失败，绝不能向新 Relay 提交或用新 key 验证旧响应。

### 20.2 React Query keys

```text
["project-document-meta", communityKey, relayOrigin]
["project-documents", communityKey, signedProjectId, relayPubkey,
 projectionGeneration, catalogRevision]
["project-document", communityKey, signedProjectId, relayPubkey,
 projectionGeneration, documentId, "current"]
["project-document", communityKey, signedProjectId, relayPubkey,
 projectionGeneration, documentId, revision]
["project-document-history", communityKey, signedProjectId, relayPubkey,
 projectionGeneration, documentId, maxRevision]
```

原则：

- 先独立取得 verified meta，再以它的 generation / catalog revision 启动 catalog query；
- catalog key 不依赖它自身尚未读取出的 `metaEventId`；
- meta bootstrap key 使用 local Community identity + captured endpoint；verified result
  取得 Relay pubkey后，全部业务 key再带该 pubkey；
- list 不携 body；
- current body 可被 head invalidation；
- pinned revision 在同一 projection generation 内 immutable；
- signer / generation reset 通过 query key 和全量 invalidation重验所有 pinned cache；
- write success 只 invalidates catalog、target current 和相关 history；
- 所有 key 带 local Community key；verified 业务 key另带 signed Project ID 与 expected
  Relay pubkey；
- raw live event 只是 invalidation hint，不直接覆盖或 warm canonical UI state；
- identity / generation reset 时清除相应 Community 的全部 Document query。

若新增 module-level Map / cache，必须提供 reset 并加入
`resetCommunityState()`。更优选择是放在 React Query / component lifetime，避免新的
singleton。

### 20.3 Documents UI

首版页面：

```text
Documents
├── active metadata list
├── detail / safe Markdown viewer
├── create editor
├── update editor
├── revision metadata history
└── pinned revision viewer
```

编辑器 v0 可以复用 Canvas 的 textarea / Markdown preview，不需要先引入 block editor、
CRDT 或富文本。

加载策略：

- 打开 Documents 页面只读 metadata；
- 点击 Document 才读 body；
- 打开 history 时 native reader 会验证完整 signed revisions，但只把 metadata 交给
  TypeScript；点击 revision 后才把已验证 body 放入 viewer state；
- 切换 revision 不替换 current cache；
- 大正文显示明确 loading / error；
- 不后台预取整个 catalog 正文。

### 20.4 conflict UX

编辑器打开时保存：

```text
base_document_revision
base_snapshot
local_snapshot
```

提交 409：

- 保留 local snapshot；
- 显示 base 与 latest revision；
- 提供“复制本地内容”“重新加载 latest”“查看 diff”“基于 latest 重新编辑”；
- 只有用户显式选择后才产生新 command；
- 不静默覆盖、不自动 rebase。

沿用当前 Project View stale edit 保留 draft 的交互原则。

### 20.5 Resource Guide UI

Project View Resource form v3：

- name；
- 开放 `resource_kind` 输入 + suggestions；
- optional summary；
- Guide Document picker；
- “Create new Guide”入口。

Resource card / Inspector：

- 显示 kind、summary；
- 显示 verified Guide title / current revision；
- “Open guide”；
- Document metadata unavailable 时仍显示 `guide_document_id`，不显示 cached stale title。

创建向导遵循两步 saga。Guide 创建成功而 Resource 创建 conflict 时，界面保留 Guide 并
允许重试 / 选择它。

### 20.6 Context Reference UI

每个 active Project View object Inspector 增加 Context 区：

- Resource chips；
- Live Document chips；
- Pinned Document revision chips；
- add / remove；
- 当 Inspector 当前 source 是 Resource 时，target picker 隐藏 Resource target，只显示
  Live / Pinned Document；
- tombstone / unavailable target 显示明确状态；
- 点击时 lazy open Resource / Document。

UI只在 `buzz-project-context-v1` 广告时开放 add / remove picker。v3 parser始终
round-trip refs；capability被临时 disable后，既有 refs仍以 read-only chips和明确
“Context unavailable”状态显示，不开放 add / retarget；可以提供显式 remove / clear
cleanup，因为 server只接受当前 set的 canonical subset。不能因为编辑器关闭而把 refs从
serializer丢掉。首次 Stage 5 canary因 canonical set为空，不显示虚构的 Context能力。

UI 不使用颜色或图标暗示 Context Reference 已授予权限或已经安装。

### 20.7 Desktop live / community switching tests

至少覆盖：

- metadata list 不预取 body；
- current / pinned 缓存隔离；
- live head / meta invalidation；
- conflict draft preserve；
- Resource → Guide；
- Context add / remove；
- Community switch 无数据泄漏；
- tampered signer / pointer fail closed；
- delete / tombstone；
- Markdown safe render；
- 新 singleton reset（若存在）。

Playwright screenshot 需遵守仓库现有 animation wait、subject crop 和 hash distinctness 规则。

### 20.8 Mobile / Web

当前分支的 Mobile / Web 没有 Project View 客户端面。本阶段不为了 Document 单独复制一套
缺少 Role / Work Context 的 UI：

- server protocol、SDK wire 和 private gates 保持客户端无关；
- Mobile / Web transport 必须安全忽略或不订阅未知 kinds；
- Desktop / CLI 闭环稳定后，再以独立阶段接入 Project View v3 + Documents；
- 未接入前不把 Mobile / Web 标记为 capability-ready writer。

## 21. Agent、Role Brief 与 Context

### 21.1 Role Brief 只交付坐标

Role Brief 不内联 Document / Guide Markdown。它只可以包含：

```text
Resource
  resource_id
  name
  resource_kind
  summary
  guide_document_id
  verified guide current revision（可用时）
  fetch command

Document
  document_id
  reference mode
  Live: verified current title / summary / revision（metadata 可用时）
  Pinned: pinned revision；不附 current title / summary
  fetch command
```

示例：

```text
- Repository · Buzz source
  resource_id: ...
  guide: ... @ current revision 8
  fetch: buzz documents get ... --content-only
```

Pinned Reference 不在后台读取 `40906`，因为这个 event 带完整 Markdown body；也不能把
Document 当前 title / summary 贴到历史 revision 上。Pinned 条目只输出
`document_id + pinned revision + buzz documents get --revision ...`。未来若需要历史
metadata preview，应新增不含正文的 Relay-signed revision index contract。

当前 `RoleBrief` 及其嵌套类型全部 `deny_unknown_fields`，因此不能原地加
`resources / documents / document_source_revisions`。SDK 新增独立 `RoleBriefV3`；
内部 resolver 可以返回 `ResolvedRoleBrief::V2(RoleBrief) | V3(RoleBriefV3)`，但不能把
这个 enum 的 tag强塞到现有 serialized surface。

- v2 Community 继续输出当前裸 `RoleBrief` JSON / prompt，保持 byte-compatible；
- v3 Community 输出新的 `RoleBriefV3`，其自身含固定
  `project_view_schema_version: 3`；
- caller 已通过 verified NIP-11 capability知道应使用哪个 strict parser；
- v3 parse失败不 fallback成 v2。

`RoleBriefV3` 复用 v2 的逻辑 section，但类型本身使用 v3 object / entity source，并新增：

```text
context {
  availability:
    not_advertised_empty
    | ready
    | unavailable_preserved {resource_count, document_count}
  resources[]
  live_documents[]
  pinned_documents[]
  truncation
}
```

`RoleBriefV3` 的 source revisions 除 v2 已有 Project / membership 字段外，增加：

```text
document_metadata:
  not_required
  | verified {
    meta_event_id
    catalog_revision
    projection_generation
  }
  | unavailable
```

这是阶段 5 cutover 前就必须交付的 base v3 contract，而不是阶段 6 才出现的类型。Context
sub-capability 尚未开启时，`RoleBriefV3.context` 的三个列表必须为空、truncation 表示
没有 Context 输入；canonical refs也为空时 availability =
`not_advertised_empty`、`document_metadata = not_required`。它仍以完整 v3 wire shape和
strict parser输出。

若 Stage 6 已有 refs后 Context capability临时消失，Role Brief不把 preserved refs静默
当作 ready Context，也不谎称“从未启用”：三个自动注入列表为空，availability =
`unavailable_preserved`并给出 verified coordinate counts，`document_metadata =
not_required`，同时提示可用 `buzz project-view get`显式检查。Project View snapshot /
CLI / Desktop仍显示原坐标。重新 enable后下一次 resolve切回 `ready`并重新构造 closure。

阶段 6 只填入这些已存在字段，并把 document metadata state切换为 `verified` 或
`unavailable`，不会在已切到 v3后再次改变 Role Brief major。

每个 metadata item 仍带精确 Resource / Document coordinate。Resource name、kind、
summary，以及 Document title / summary 都只是 Relay 接受的**不可信描述数据**，不是
system instruction；prompt renderer 对它们做单行化、delimiter escaping 和明确的
“project-provided metadata”分区，不能允许换行或伪造 delimiter 提升指令层级。

### 21.2 有界一跳 closure（阶段 6）

v3 Role Brief 从当前已经验证的 Project View snapshot 出发：

1. Project Profile 和 Brief 中已有的 Goals；
2. 当前 Role；
3. 该 Role 负责的 nonterminal Work；
4. 当前 Brief 已包含的 related Issue / handling Work；
5. latest Checkpoint / recent Handoff 的 Object reference 指向的 active Project View
   objects；
6. 收集以上 source 的 Context Reference；
7. hydrate Resource；
8. 对 Resource 加入 primary Guide 和它直接引用的 Document；
9. 停止，不递归形成任意 Resource graph。

Resource v0 不能引用 Resource，因此不会出现 Resource dependency traversal。
Checkpoint / Handoff 指向 tombstone object 时只保留原坐标，不展开 Context。

Context slice 使用 64 KiB 的**最终 escaped prompt UTF-8 byte budget**。选择时把一个
Resource 与它的 primary Guide coordinate 视为不可拆的 pair：

1. 按 canonical key 遍历 Resource，最多 64 个；
2. 先计算只含 required coordinate fields 的最小 pair：
   `resource_id / name / resource_kind / guide_document_id / fetch command`；
3. 最小 pair 放不进剩余 budget 时停止纳入后续 Resource，并记录 omitted count；绝不
   纳入 Resource 却丢掉 Guide；
4. 对已纳入 pair，再按顺序尝试加入完整 optional Resource summary 与 verified Guide
   title / revision；字段整体放不下就省略并标记 `metadata_omitted_due_to_budget`，不截断
   字符串造成错误语义；
5. 最后才从剩余 budget 选择 direct Context / Resource context 的 supplementary
   Document。

因此每个**实际纳入**的 Resource 的 primary Guide coordinate 都保证进入
`mandatory_guides`，但“最多 64”不是无视 byte budget 的最低承诺。

限制：

```text
最多 64 个 unique Resource 坐标
最多 64 个 mandatory Guide Document 坐标
最多 64 个 supplementary Document 坐标
最终 escaped context block ≤ 65,536 bytes
```

同一 coordinate 同时是 Guide 和 direct Context 时只输出一次，但按 mandatory 计。超过
任一上限或 byte budget 时先按上述规则省略 optional metadata，再成对停止 Resource /
Guide，最后截断 supplementary 项，并写明：

- total / included count；
- omitted Resource / optional metadata / supplementary count；
- 未注入正文；
- 使用 `buzz project-view get` / `buzz documents list` 显式继续发现。

### 21.3 Document metadata 稳定窗口

Document 编辑不推进 Project View revision。若 Brief 缓存只以 PV meta 为 key，会长期保留
旧 Guide title / current revision。

full resolve：

1. 验证 PV v3 meta / snapshot；
2. 读取 verified Document meta A；
3. 只查询 closure 中 Live Document 与 Resource Guide 的 heads；
4. 再读 Document meta B；
5. A / B event ID、generation、catalog revision 相同才接受；
6. 不同则有限重试。

cache key 至少包括：

```text
relay identity
member / assignment
PV meta event ID
PV project revision / generation
Document meta event ID
Document catalog revision / generation
```

简单首版允许任一 Document 编辑使 Role Brief metadata slice refresh；后续再做 per-head cache。

Pinned coordinate 不参与这个稳定窗口，也不触发 revision body fetch；它已经由 signed PV
projection 固定，只输出 coordinate。Document meta A / B 只证明 Live head metadata。

### 21.4 metadata failure

当前 ACP 的 full resolver 把 Project View identity / meta / snapshot 任一错误映射为
`ResolutionFailure::Project`，随后 suspend managed runtime。v3 必须把解析拆成两段：

1. authority-bearing Project View / membership / Assignment / Runtime fence resolve，继续
   使用现有 fail-closed / suspend 路径；
2. optional Document metadata enrichment，使用独立 cache / result，不返回
   `ResolutionFailure::Project`，失败也不能跳过正常 runtime reconcile。

Document metadata 暂时读取失败时：

- 不使用 stale cached title / revision；
- 保留来自 verified PV v3 的 `document_id / guide_document_id`；
- 标记 metadata unavailable；
- 提供显式 fetch command；
- 不因为一个 Guide metadata瞬时失败自动结束 / suspend Assignment。

Agent 真正执行前仍必须用 CLI 取得并验证正文。这样不会用 stale Guide 冒充 current，也
不会让普通 Document 编辑造成全体 Runtime 抖动。

测试必须覆盖 title / summary / resource name 中的换行、Markdown fence、角色前缀和
renderer delimiter 注入；这些 metadata 即使签名有效，也只能作为 quoted data 出现在
Brief 中。

### 21.5 v2 / v3 cutover 前提

当前 ACP resolver 只接受 `buzz-project-view-v2`。在任何 Community 切 v3 前必须先：

- 读取 NIP-11 v2 / v3；
- 实现 v3 projection parser；
- 实现 v3 RoleDefinition / continuity snapshot；
- 实现 `RoleBriefV3`、`ResolvedRoleBrief::V2 | V3` 与 strict versioned serialized
  surface；首次 Context gate关闭且 refs为空时输出
  `availability:not_advertised_empty`、empty context +
  `document_metadata:not_required`；
- 更新 cache key；
- 通过 v2 regression、base v3 Role Brief fixture 和 v3 strict-parser fixture；
- 部署到所有 managed Agent hosts。

否则 cutover 后每轮 Role Brief 会 fail closed 并 suspend runtime。

### 21.6 明确不自动执行

ACP 不因 Role Brief 出现 Resource 而：

- clone repository；
- 安装 Skill / Plugin；
- 修改 MCP config；
- 重启 Agent；
- 连接服务器 / database；
- 请求 Secret；
- 运行 Guide code block。

这些行动必须来自当前 task、Agent 判断、已有 tool permission 和必要 approval。后续
Adapter 若出现，也只能是显式工具调用，不改变 Context Reference 的 inert 语义。

## 22. Legacy Resource 迁移与 Project View v3 cutover

### 22.1 不做无审查自动迁移

当前 Resource：

- description 最长可达 32 KiB；
- locator 最长 4,096 bytes；
- URL validation 只 parse 并禁止 userinfo，不等于只允许 HTTP(S)；
- query、fragment 或其他 locator 可能意外包含 token；
- description 可能已有命令、地址或敏感信息。

因此不能在 migration SQL 中直接把 locator / description 发布成 Guide 并声称符合
“no Secret”。也不能让 AI 自动总结、截断或猜测 Resource kind。

### 22.2 reviewed migration manifest

新增 operator tool 与一个使用当前 Human member key 的本地 approval command：

```text
buzz-admin project-view v3 resources export --community <id> --out <dir>
buzz-admin project-view v3 resources validate --community <id> --manifest <file>
buzz-admin project-view v3 cutover --community <id> --manifest <file> \
  --maintenance-epoch <n> --idempotency-key <key>

buzz project-view v3 resources approve \
  --manifest <draft.json> \
  --out <reviewed.json>
```

`approve` 只读取、校验和签名本地 manifest；它不提交 Nostr event、不调用新的业务
mutation endpoint，也不执行 cutover。这样 operator 可以准备迁移，真正决定 locator
如何变为 Guide、Resource 最终叫什么以及 summary 是否保留的仍是一个可验证的 Human
member。

export 为每个 active legacy Resource 生成 draft：

- stable resource ID；
- exact cutover-base Project View meta event ID、project revision 与 projection generation；
- legacy Resource object revision、projection event ID 与 canonical body digest；
- legacy resource type；
- legacy locator type / exact locator；
- legacy description；
- 建议 `resource_kind`；
- 建议 Guide Markdown draft；
- 预分配并持久记录的 `guide_document_id`；
- 待填写的 reviewed Guide current revision、head event ID、revision event ID 与 content
  digest；
- review 状态。

输出是本地受控迁移材料，不自动发布。operator / Project member：

1. 检查和移除 Secret；
2. 修正不再有效的 locator；
3. 确认 resource kind；
4. 用正常 `buzz documents create` 发布 Guide；
5. 在 final entry 中写入确切 Guide pointers 与最终 v3 Resource body；
6. 由 current Human member 运行 `approve`，重新读取并验证 legacy Resource、Guide 与
   cutover base，再对每个 entry 生成 detached Schnorr signature；
7. operator 用 `validate` 验证全部 mapping、digest、signature 与 current eligibility。

重复 export / validate 必须复用持久 mapping，不能每次随机生成新 Guide ID。新增受限
staging 表：

```text
project_view_v3_resource_mappings
  (community_id, resource_id) PK
  guide_document_id
  legacy_object_revision
  legacy_projection_event_id
  legacy_body_digest
  guide_document_revision
  guide_head_event_id
  guide_revision_event_id
  guide_content_digest
  reviewed_v3_payload
  v3_payload_digest
  mapping_entry_digest
  reviewed_by_pubkey
  reviewed_at
  review_digest
  review_signature
  manifest_digest
  status
```

`reviewed_v3_payload` 是 closed `CanonicalResourceCutoverV1`：

```text
resource_data: CanonicalProjectResourceV3
  name
  resource_kind
  summary
  guide_document_id
context_references: []        # outer ProjectViewObjectV3 field，v0 cutover 必须为空
```

这避免把 14.3 的 Resource data type 和 14.4 的 outer object Context field 混成一个类型，
同时让 reviewer signature 绑定“初始 Context 必为空”。建议 kind、建议 summary、Guide
draft、reviewer comment 等材料可以留在 export bundle，但不属于 final entry，也不能在
cutover 时覆盖 `reviewed_v3_payload`。

manifest 是这张稳定 mapping 的可审查导出，不是唯一事实源。`validate` 把 exact canonical
value、digest、signature 与 event pointers 写回 staging；cutover 在 maintenance fence +
Community lock 后逐项 point-check。任一 legacy Resource、Guide、base meta 或 final payload
在 review 后变化都 abort，并要求重新 export / review，不能“使用最新值继续”。

这里最终可接受的 reviewer pubkey 必须出现在 manifest 绑定的 NIP-43 membership
snapshot 中，并在 `validate` 与 cutover 当下都被 canonical DB 判定为未 ban / timeout
的 direct Human Community member；managed Agent 不能成为 legacy authority 转换的
`updated_by`。operator 只持有 cutover authority，不能把自己的 key 或另一 member key
替换为 reviewer。

draft generator 必须使用 deterministic Markdown escaping / 可容纳原值的动态 code fence；
locator 和 description 只作为内容数据写入文件，绝不能被插入或执行为 shell command，也
不做 AI 总结或静默截断。输出目录 / 文件使用 owner-only permissions，并在完成迁移后按
运维流程清理本地敏感副本。

### 22.3 canonical digest 与 reviewer attestation

不能把 `serde_json::to_vec`、JSON object key 顺序、缩进或原文件 bytes 当作签名合同。
实现定义只含 binary primitives 的 closed canonical structs，并用 workspace 已有的
`postcard` 序列化：

```text
ResourceMappingManifestV1
├── schema_version = 1
├── community_id: [u8; 16]
├── base_meta_event_id: [u8; 32]
├── base_project_revision: u64
├── base_projection_generation: u64
└── entries: Vec<ReviewedResourceMappingV1>

ReviewedResourceMappingV1
├── resource_id: [u8; 16]
├── legacy_object_revision: u64
├── legacy_projection_event_id: [u8; 32]
├── legacy_body_digest: [u8; 32]
├── reviewed_v3_payload: CanonicalResourceCutoverV1
├── v3_payload_digest: [u8; 32]
├── guide_document_revision: u64
├── guide_head_event_id: [u8; 32]
├── guide_revision_event_id: [u8; 32]
├── guide_content_digest: [u8; 32]
├── mapping_entry_digest: [u8; 32]
├── reviewed_by_pubkey: [u8; 32]
├── reviewed_at_unix_micros: i64
├── review_digest: [u8; 32]
└── review_signature: [u8; 64]
```

canonical rules：

- v0 manifest最多 4,096 entries、JSON envelope最多 256 MiB；reader执行 bounded /
  streaming read并在分配大型 `Vec` 前检查。超限 Community不切 v3，等待独立 hardening，
  不能绕过 cap；
- entries 按 Resource UUID 的 16-byte lexicographic order 排列，duplicate 拒绝；
- UUID、event ID、pubkey、signature 进入 canonical struct 时都是 fixed bytes，不使用
  hex / bech32 文本；
- string 使用 exact UTF-8 bytes，不做 trim、大小写折叠或 Unicode normalization；
- `summary` 使用 `Option<String>`；`Some("")` 在 domain validation 阶段拒绝，不能和
  `None` 合并；
- `resource_kind` 先通过 v3 canonical token validator，再保存 exact accepted bytes；
- JSON 文件只是该 closed type 的 Human-readable envelope；unknown / missing field、
  非 canonical decimal、重复 ID 或解析后 re-encode 不一致都拒绝；
- 实现必须提供跨 crate golden bytes / digest fixtures；更换 serializer 或字段顺序是新
  schema version，不能原地改变 v1。

cutover receipt / audit只保存 manifest digest、entry count、base / epoch和 per-entry
mapping digest / reviewer linkage；不把整个 manifest、legacy locator、Guide Markdown
或全部 signatures复制进单个 audit JSON。cutover transaction写入的 restricted immutable
committed-entry ledger保存逐项证据。

所有 digest 都是：

```text
SHA-256(domain || postcard(canonical_value))
```

固定 domain：

```text
buzz-pv3-legacy-resource-v1\0
buzz-pv3-resource-cutover-payload-v1\0
buzz-pv3-guide-snapshot-v1\0
buzz-pv3-resource-mapping-v1\0
buzz-pv3-resource-review-v1\0
buzz-pv3-resource-manifest-v1\0
```

各 value 的闭合边界：

- `legacy_body_digest` 覆盖 base 时 canonical `ProjectViewObject` 的 schema version、
  identity、object / project revision、state、完整 `ProjectResource` body和关系字段；
  它不是 raw JSON digest，也不包含随时会变化的数据库物理列；
- `v3_payload_digest` 覆盖 final `CanonicalResourceCutoverV1`，包括 Resource data 与
  canonical empty Context set；
- `guide_content_digest` 覆盖 exact active `ProjectDocumentRevision` business snapshot：
  document ID、document revision、title、summary 与 Markdown，不覆盖 Relay signature；
- `mapping_entry_digest` 覆盖 community / base meta / base project revision /
  generation、Resource identity 与 legacy pins、final v3 payload digest 和全部 Guide
  pins；
- `review_digest` 覆盖 `mapping_entry_digest`、reviewer pubkey 与
  `reviewed_at_unix_micros`；
- `manifest_digest` 覆盖 header 与按上述顺序排列的完整 reviewed entries，包括 reviewer
  signature。

`approve` 使用 Buzz 当前 Nostr key 对 32-byte `review_digest` 做现有
secp256k1/BIP-340 Schnorr detached signature；复用 `nostr::Keys::sign_schnorr` /
`verify_schnorr` 的模式，不定义第二套 key format。签名前它必须从 Relay 读取 verified
current state，在本地拒绝当前明显非 member / Agent / ban / timeout 的 signer，并确认
final payload、Guide snapshot 与 base pins 一致。但这是本地 UX guard，不是 server-side
authorization evidence。

`mapping_entry_digest` 已绑定 v2 base meta；该 meta 绑定的 exact NIP-43 snapshot 只证明
base 时的 member pubkey / role，不含 ban / timeout，也不单独证明这是 Human。detached
signature 证明该 pubkey 审阅了 exact content；权威的 Human / ban / timeout eligibility
只在 `validate` 与 cutover 的 current canonical gate 确认。`reviewed_at_unix_micros`
只是签名内的审阅记录，不被当作 Relay canonical time，也不能证明 review-time
eligibility。

`validate` 不能相信 manifest 内现成的 digest。它必须：

1. parse closed JSON envelope；
2. 构造 canonical structs 并重新生成 postcard bytes；
3. 逐层重算所有 digest；
4. 验证 Schnorr signature；
5. 确认 reviewer pubkey / role 出现在 base meta 绑定的 membership snapshot，并在
   validate 当前 canonical state 中验证其为 eligible direct Human member；
6. 把 exact final payload、digest 与 signature 写入 staging；
7. 生成最后的 `manifest_digest`。

cutover 再重复 1–5，并要求 reviewer 在 cutover 时仍 eligible。operation receipt /
audit记录 operator `ChangeSource`、`manifest_digest`、entry count和 committed child
ledger linkage；每个 `mapping_entry_digest`、reviewer pubkey与 signature只保存在
对应 immutable child entry，不复制进单个 receipt JSON。归因因此是双层且不可混淆：

- operator source / audit sequence 表示谁执行并接受了原子 cutover；
- Resource `updated_by` 表示哪个 Human 用签名审阅了最终业务内容。

### 22.4 deterministic type mapping

初始建议值：

| legacy `resource_type` | v3 `resource_kind` |
|---|---|
| repository | repository |
| document | external_document |
| design | design |
| service | service |
| environment | environment |
| artifact | artifact |
| url | external_link |

这是 migration suggestion，不是 v3 enum。Reviewer 可以选择其他 canonical token。

legacy description：

- 完整原文放在 reviewed Guide draft；
- 只有不超过 4,096 bytes 且 reviewer 明确确认时才复制到 Resource summary；
- 否则 summary 为 `None`，不静默截断。

### 22.5 durable runtime maintenance fence

当前实现不能仅靠 `project_view_enabled = false` 完成 cutover maintenance：

- `project_runtime::record_runtime_evidence` 当前没有读取该 flag；
- `claim_unrecoverable_runtime_assignments` 跨 Community claim，也没有该 gate；
- ACP 的 `RuntimeSupervisorClient::suspend()` 只清本地 fence file，不是 fleet-wide
  acknowledgement，也不保证当前 turn 已终止。

因此 v3 migration 必须先增加一个独立、持久化的 operational fence，而不是把现状描述成
已有能力：

```text
project_view_maintenance
├── community_id PK
├── state                    normal | draining | frozen
├── current_epoch            BIGINT nullable
└── updated_at

project_view_maintenance_epochs
├── community_id
├── maintenance_epoch
├── base_meta_event_id
├── base_project_revision
├── base_projection_generation
├── required_client_protocol_version
├── requested_by
├── requested_at
├── begin_audit_seq
├── begin_idempotency_key_hash
├── begin_request_hash
├── begin_receipt
├── outcome                  active | aborted | cutover_committed | resumed
├── completed_at             nullable
└── updated_at

PK (community_id, maintenance_epoch)
UNIQUE (community_id, begin_idempotency_key_hash)

project_view_maintenance_operations
├── community_id
├── maintenance_epoch
├── operation_id
├── operation
├── idempotency_key_hash
├── canonical_request_hash
├── requested_by
├── audit_seq
├── result_receipt
└── accepted_at

PK (community_id, maintenance_epoch, operation_id)
UNIQUE (community_id, idempotency_key_hash)

project_view_maintenance_invalidations
├── community_id
├── maintenance_epoch
├── invalidation_id
├── phase                    pre_cutover | post_cutover
├── source_type              project_view_change | community_audit
├── source_change_id         nullable
├── source_audit_seq         nullable
├── invalidated_at
├── resolved_by_operation_id nullable
├── resolved_meta_event_id   nullable
├── resolved_project_revision nullable
└── resolved_projection_generation nullable

PK (community_id, maintenance_epoch, invalidation_id)
FK project_view_change source → project_view_changes
FK community_audit source     → audit_log
FK resolved_by_operation      → project_view_maintenance_operations

project_view_maintenance_assignment_baselines
├── community_id
├── maintenance_epoch
├── assignment_id
├── member_pubkey
├── binding_id
├── supervisor_pubkey
├── state_at_begin           idle | has_runtime
├── last_polled_at           nullable
├── client_protocol_version  nullable
└── client_build             nullable

PK (community_id, maintenance_epoch, assignment_id)
UNIQUE (community_id, maintenance_epoch, binding_id)
UNIQUE
  (community_id, maintenance_epoch, binding_id, assignment_id, member_pubkey,
   supervisor_pubkey)

project_view_maintenance_ack_requests
├── community_id
├── maintenance_epoch
├── ack_request_id
├── agent_pubkey
├── ack_type                 assignment | runtime
├── idempotency_key_hash
├── canonical_request_hash
├── auth_event_id
├── result_receipt
└── accepted_at

PK (community_id, maintenance_epoch, ack_request_id)
UNIQUE (community_id, idempotency_key_hash)

project_view_maintenance_assignment_acks
├── community_id
├── maintenance_epoch
├── ack_request_id
├── binding_id
├── assignment_id
├── member_pubkey
├── supervisor_pubkey
├── status                   quiesced
├── client_protocol_version
├── client_build
└── acked_at

PK (community_id, maintenance_epoch, assignment_id)
UNIQUE (community_id, maintenance_epoch, ack_request_id)
FK
  (community_id, maintenance_epoch, binding_id, assignment_id, member_pubkey,
   supervisor_pubkey)
  → project_view_maintenance_assignment_baselines

project_view_maintenance_runtime_baselines
├── community_id
├── maintenance_epoch
├── binding_id
├── assignment_id
├── runtime_id
├── runtime_epoch
├── supervisor_pubkey
└── availability_at_begin

PK (community_id, maintenance_epoch, binding_id, assignment_id, runtime_id, runtime_epoch)
UNIQUE
  (community_id, maintenance_epoch, binding_id, assignment_id, runtime_id, runtime_epoch,
   supervisor_pubkey)
FK
  (community_id, maintenance_epoch, binding_id, assignment_id, supervisor_pubkey)
  → project_view_maintenance_assignment_baselines
FK (community_id, binding_id, assignment_id, runtime_id, runtime_epoch)
  → project_runtime_leases

project_view_maintenance_acks
├── community_id
├── maintenance_epoch
├── ack_request_id
├── binding_id
├── assignment_id
├── runtime_id
├── runtime_epoch
├── supervisor_pubkey
├── status                   suspended | terminal
└── acked_at

PK (community_id, maintenance_epoch, binding_id, assignment_id, runtime_id, runtime_epoch)
UNIQUE (community_id, maintenance_epoch, ack_request_id)
FK full identity，包括 assignment / supervisor
  → project_view_maintenance_runtime_baselines
```

current pointer 对 `(community_id, current_epoch)` 建 nullable deferred FK；operation、
invalidation、assignment / runtime baseline、unified ack request和两类 ack都以
`(community_id, maintenance_epoch)` 建 FK 到 epoch row。两类 ack再以
`(community_id, maintenance_epoch, ack_request_id)` 建 FK 到 unified request ledger；
deferred validator要求每个 request 按 `ack_type` 恰有一个对应 child row。这样 current /
historical run 的任何子记录都不能成为孤儿，同一 idempotency key也不能跨 assignment /
runtime variant重复使用。

epoch 的 `begin_audit_seq` 与每个 operation 的 `audit_seq` 都以
`(community_id, seq)` FK到 `audit_log`，`requested_by`必须等于 authenticated operator。
invalidation使用 closed source union：`project_view_change`只设置
`source_change_id`并 FK typed change；`community_audit`只设置
`source_audit_seq`并 FK audit row。ban / owner变更走阶段 4 新 coordinator的前者；
archive / unarchive等不推进 Project View revision的 Community admin path走后者，但也
必须在同一 exclusive lock transaction插入 invalidation，不能留下无法追溯的裸 flag。

为让上述 FK 真正表达 exact baseline，migration 同时给
`project_runtime_supervisor_bindings
(community_id, binding_id, assignment_id, supervisor_pubkey)` 和
`project_runtime_leases
(community_id, binding_id, assignment_id, runtime_id, runtime_epoch)` 增加 supporting
`UNIQUE`；assignment baseline 以完整 binding identity 建 FK，runtime baseline 以完整
lease identity 建 FK。不能只靠 `binding_id` 后再相信表中重复保存的 Assignment /
supervisor 值。

epoch、baseline identity、unified ack request、ack、operation receipt 都不可 UPDATE /
DELETE；只允许
assignment baseline 的 `last_polled_at / client_protocol_version / client_build` 做
单调诊断更新，以及 epoch terminal outcome / invalidation resolution做受 trigger约束的
单向更新。invalidation identity不可改，`resolved_*`只能从全 NULL一次性写成完整
coordinate。ack 的 hash、auth provenance 和 receipt 必须落库，才能区分 exact replay、同 key
改请求和同 coordinate 换 key。`project_view_maintenance` 只是当前状态指针；历史 status
和跨多个 maintenance cycle 的 idempotency evidence 从 epoch / operation ledger读取，
不能在下一次 begin 时覆盖。

安全 compatibility gate只比较 run-pinned `required_client_protocol_version`；协议版本是
有序整数并进入 begin request hash / receipt。`client_build`只用于 fleet诊断和定位发布
批次，不做字符串大小比较，也不作为 freeze条件；若未来需要 build allowlist，必须给
maintenance contract升版。

状态机：

```text
normal --begin--> draining --freeze--> frozen
draining --abort(pre-commit, no cutover receipt)--> normal
frozen   --abort(pre-commit, no cutover receipt)--> normal
frozen   --resume(post-commit, verified v3)--> normal
```

- `begin` 只接受 non-archived、`state = normal`、schema 2、
  `project_view_enabled = true` 且 v2 structural ready 的 Community。原本因 signer /
  integrity故障 disabled 的 Community 必须先修复，不能借 migration 或 abort意外启用；
- `begin` 在 Community exclusive advisory lock 下递增 epoch、进入 `draining`，原子设置
  `project_view_enabled = false`，保存 exact Project View base、最低 ACP maintenance
  protocol版本，以及 begin 时所有 current managed Assignment / supervisor binding /
  runtime epoch 的 immutable baseline；
- begin 时每个 active managed-Agent Assignment 必须恰有一个 non-revoked supervisor
  binding。数据库中暂时没有 runtime 的 `idle` 只是一条 baseline observation，不是
  “没有旧 ACP 在运行”的安全证明；每个 Assignment 都必须在 exact epoch 后由满足最低
  protocol version 的 supervisor提交 assignment-level `quiesced` ack。`client_build`
  只随 ack记录为诊断。`has_runtime`
  baseline 还必须逐条提交 runtime ack。NoBinding、duplicate binding、旧版 / 离线 ACP
  或“运行但没有 lease”的旧 host 因无法完成 assignment ack 都会阻止 freeze；
- v3 把 `RequireSupervisedRuntime` 变成 managed write / admission invariant；无 binding
  的 v2 legacy Assignment 必须在 cutover 前补齐，切到 v3 后不能继续依赖当前
  `validate_runtime_command_fence_in_tx` 的 optional-supervision行为；
- `draining` 立即阻止新的 ACP turn admission、Runtime `Start` /
  `RecoveryAttempt` / `RecoverySucceeded` 和 scheduler claim，但允许 baseline runtime
  上报 `GracefulStop`、terminal failure 或 maintenance `suspended` ack；
- ACP 增加独立于 turn 的 long-lived maintenance watcher。进程启动时先解析 binding /
  auth并做一次同步 maintenance GET，**然后**才允许 Role Brief resolve、
  `RuntimeSupervisor::prepare_startup` 的 Start / Recovery、`AgentPool`创建或
  `initial_message`；不能沿用当前 RoleBrief-first启动顺序；
- state normal时才进入现有 startup。draining / frozen时进入 maintenance-only mode：
  不依赖被隐藏的 Project View capability，不发 Runtime Start / Recovery、不解析 Role
  Brief、不创建 pool，只处理 exact baseline cleanup / ack并继续 polling。这样 begin后
  重启的 ACP仍能完成 drain；
- watcher按 server返回的 bounded `poll_after_seconds`持续轮询，即使 Assignment online
  但当前没有 Runtime / work也不能停。turn admission仍额外同步检查；active turn共享同一
  epoch signal，不能各自维护容易漂移的状态；
- watcher看到 draining 后停止领取新 work，
  先 latch exact maintenance epoch、撤掉 agent-readable fence并 pause lease renewal；
  latch 让 concurrent `reconcile / publish_current_fence` fail closed，不能重建旧 fence；
- maintenance signal 随后进入 current main loop。现有 cancel 只覆盖 prompt task，实施
  必须把 cancellation token贯穿 heartbeat、Role Brief / canvas / session prepare、
  `initial_message` 和 prompt / child process全生命周期；暂时不能安全 cancel 的路径只能
  自然结束并让 ack pending，不能提前确认；
- current `AgentPool` 的 idle `OwnedAgent`也持有 persistent child，不能只 join in-flight
  prompt。latch后先暂停 pool wake / respawn与queue intake，再 shutdown / reap active和
  idle slots的全部 child process。ACP增加 host-local durable child/process-group registry；
  mid-drain restart先按 registry清理并证明没有 surviving owned child，不能仅凭旧进程已
  退出就 ack；
- 对已有 prompt batch 复用 cancel + `requeue_as_cancelled`，把触发事件 park 到 queue 并
  等待 heartbeat、pre-prompt preparation、task 与 child process真正退出；它不声称保存
  Agent 未提交的 partial work。顺序固定为
  `latch / withdraw fence / pause renewal → cancel and join all work → runtime ack
  （若有）→ assignment quiesced ack`，不能先 ack 再等待 task；
- ack 只由该 binding 已注册的 `supervisor_pubkey` 通过 NIP-98 提交；Relay 在同一事务中
  验证 maintenance epoch、binding、Assignment 与 exact baseline runtime row / epoch，
  再按 ack status核对 materialized runtime state；不能一律要求 `ended_at IS NULL`，否则
  已 GracefulStop 的 terminal ack 永远无法通过；
- `suspended` ack 可从 available / recovering baseline 以 maintenance 原因清除 lease /
  recovery state 并设置 `ended_at`，不生成 failure evidence；`terminal` ack 要求已有
  graceful / terminal evidence，必要时只把 unavailable lease 标记 ended。两者都保留
  Assignment，沿用当前 `GracefulStop` 的“结束 runtime、不结束 Assignment”边界；
  恢复后由正常 admission 产生新的 runtime ID / epoch，旧 fence 永不复用；
- assignment-level ack 由同一 supervisor在确认该 Assignment 没有 active / pending
  runtime、pool slot、durable child-registry entry或可发布 fence后提交，并绑定 exact
  epoch、binding、Assignment、protocol version、diagnostic build与 authenticated
  request。即使
  `state_at_begin = idle` 也不能省略；
- v0 不提供 operator “代 ack”。无法安全停止的 turn 会让 drain 保持 pending；operator
  只能等待或请求 pre-commit abort，不能靠 timeout 假装 cutover 已可执行；
- `freeze` 只有在每个 Assignment baseline 都有 compatible durable quiesced ack、每个
  runtime baseline 都有 durable ack、idle baseline在 begin 后仍未产生 runtime、没有
  in-flight scheduler claim、没有 active / recovering lease、且 drain 后未出现新
  epoch时成功；
- pre-commit abort 是另一条带 exact epoch / idempotency 的 exclusive-lock
  transaction：只在 schema仍为 2且没有 cutover receipt时，把该 epoch 的所有 baseline
  lease / fence标记 ended并记录 aborted outcome。若 Community仍 non-archived且 current
  v2 structural / signer readiness健康，则原子恢复 enabled + normal；若期间被 archive或
  v2 integrity已故障，则只回到 normal并保持 disabled，不能用 abort掩盖故障。它不伪造
  quiesced ack，也不复活旧 lease。已 latch ACP看到 aborted outcome 后只能 fresh
  admission；未看到 epoch 的旧 ACP下一次 evidence / managed write也因旧 runtime
  coordinate已失效而被拒绝。部分 drain 后 abort同样使用新 runtime ID / epoch；
- watcher看到 aborted / resumed outcome且 current state normal后先丢弃旧 latch对应的
  pool / child registry / Runtime coordinate，再做 fresh `prepare_startup`、创建新 pool；
  任何 idle slot都不能跨 maintenance epoch复用；
- 在 `draining` 与 `frozen`，所有 runtime evidence、binding register / revoke、
  scheduler claim 与 Project View member / operator / system change path 都先取得同一
  Community advisory-lock namespace 并检查 maintenance state；runtime hot path 使用
  shared form，既有 Project View canonical writer 与 binding control 保留 exclusive
  form。`draining` 只白名单 exact baseline 的 `GracefulStop`、terminal evidence 与
  maintenance ack，以及 begin 前 scheduler claim 的 compare-and-clear release；
  `frozen` 只白名单 maintenance control、structural verify、下文定义的 operator-only
  repair / reproject，以及同样的单调 claim cleanup，绝不允许普通 member、Runtime 或
  claimed system action commit；
- ban、owner removal、archive 等 security-critical Community action 不能被 migration
  阻塞；它们保留当前 exclusive Community lock并执行，但必须插入 typed invalidation
  row。锁内尚无 cutover receipt时标为 `pre_cutover`，freeze / fresh cutover看到任何这类
  row或 base mismatch都拒绝，只能 abort；receipt已 commit时标为 `post_cutover`，不能
  rollback，Community继续 frozen；
- post-cutover invalidation由后续 `maintenance verify`、typed repair或reproject在
  exclusive lock下读取最新 canonical security / membership / meta，运行 v3 structural
  verify并写 operation receipt，再把 exact operation和verified meta coordinate一次性
  链到 invalidation。resume要求每条 post-cutover invalidation都已被其后发生的 operation
  resolve；它不要求 ledger为空，也不删除 /清零历史。resolve后再发生 security action会
  插入新 row并再次阻止 resume；
- 这不是当前代码已经具备的能力：当前 schema 2 `ban_member / unban_member` 仍返回
  `unavailable:project_view_v2:membership_coordinator`。阶段 4 必须先实现 v2/v3
  membership / moderation coordinator，把 security action、Role continuity、membership
  projection、audit 与 typed change ID 原子提交；maintenance invalidation 只能接在这个
  canonical path 上，不能靠旁路 UPDATE；
- cutover transaction 持有同一 Community **exclusive** advisory lock，所以不能与刚
  通过检查的 runtime evidence或 scheduler action交错；
- `project_view_enabled` 仍用于 public Project View capability/readiness，但不是
  maintenance fence，也不能替代上述 state。

具体接线不能只给 SQL 增加一个 `maintenance.state = 'normal'` WHERE：

- `record_runtime_evidence` 在读取 binding 前新增
  `community_lock::acquire(..., true)`（当前 helper 的 `true` 才是 shared），随后在同
  transaction 检查 state / epoch；
- binding register / revoke 和所有 Project View transition 复用同一 lock ordering，并
  保留当前 exclusive form；
- 当前 `claim_unrecoverable_runtime_assignments` 是一次跨 Community CTE claim，必须拆为
  按 Community ID 固定排序的 bounded candidate discovery + 每 Community transaction；
  每个 transaction 先用 `community_lock::acquire(..., true)` 取 shared lock、确认
  state = normal，再 revalidate 并 `SKIP LOCKED` claim；
- scheduler inventory 必须同时覆盖 candidate discovery、claim、release 与 claimed
  system-action commit；begin 前已有 claim 会阻止 freeze，release 可清理，system action
  在 non-normal state fail closed；
- 所有路径保持统一 lock order：Community advisory lock → maintenance / Community row →
  binding / runtime row → Project View / Document rows，禁止反向获取。

新增 controlled admin commands：

```text
buzz-admin project-view maintenance begin --community <id> --idempotency-key <key>
buzz-admin project-view maintenance status --community <id> [--epoch <n>]
buzz-admin project-view maintenance freeze --community <id> --epoch <n> --idempotency-key <key>
buzz-admin project-view maintenance resume --community <id> --epoch <n> --idempotency-key <key>
buzz-admin project-view maintenance abort --community <id> --epoch <n> --idempotency-key <key>
buzz-admin project-view maintenance verify --community <id> --epoch <n> --idempotency-key <key>
buzz-admin project-view maintenance repair --community <id> --epoch <n> \
  --plan <file> --idempotency-key <key>
buzz-admin project-view maintenance reproject --community <id> --epoch <n> \
  --idempotency-key <key>
```

在当前 tenant-bound、NIP-98 authenticated 的 operational runtime surface 上只扩展：

```text
GET  /api/project-runtime/maintenance
POST /api/project-runtime/maintenance/ack
```

GET 返回 current state、latest epoch outcome、bounded `poll_after_seconds`、调用
supervisor所负责的 exact baseline coordinates，以及这些 coordinates现有的 assignment /
runtime ack status、canonical request hash与receipt；这既让 online-idle ACP能参与 drain，
也让 POST response丢失后可以 exact read-back。POST 接受一个 closed tagged union：

```text
assignment_quiesced {
  maintenance_epoch, binding_id, assignment_id,
  client_protocol_version, client_build, idempotency_key
}

runtime_suspended_or_terminal {
  maintenance_epoch, binding_id, assignment_id,
  runtime_id, runtime_epoch, status, idempotency_key
}
```

`assignment_quiesced` 只有在该 Assignment 的全部 runtime baseline 已 ack、main loop /
child 已 join，且没有 pending runtime / fence publish 时才可写；idle baseline也必须走
这一分支。DB 以 full baseline identity + canonical request hash 幂等：相同 ack replay
返回落库 receipt，同一 key 改请求、同一 coordinate 换 status / key或跨 epoch replay都
返回 conflict。这两个 endpoint 延续现有 runtime evidence 的 operational HTTP 边界，
不成为 Document / Resource CRUD API。

maintenance GET 在同一 authenticated DB transaction 中更新对应 Assignment baseline 的
`last_polled_at / client_protocol_version / client_build`；这些字段只用于诊断，不能替代
assignment quiesced ack或 runtime ack。`begin` 对同一 canonical request +
`idempotency_key_hash` 返回原 epoch；Community 已 non-normal 时用不同 key 重试返回
conflict，不得再递增 epoch。`status --community` 可找回 current epoch，显式
`--epoch` 用于读取历史 / exact CAS 诊断。

`status` 列出 baseline、ack、remaining runtime、claim 与 durable last poll；不泄露 prompt /
turn content。commit 前失败时保持 `frozen`，直到 operator显式执行上述 abort
transaction；commit 后失败也保持 `frozen`，只能通过 `repair` 写入 schema 3 的 typed
monotonic forward change，或通过 `reproject` 生成新 generation；canonical本来已健康时
可用 `verify`只记录 structural proof并 resolve post-cutover invalidations。三条路径都要求
exact epoch、schema 3、operator auth / audit、idempotency receipt和 Community
exclusive lock；成功后仍保持 frozen，不能开放 member / system turn。只有 structural
verify通过且没有 unresolved post-cutover invalidation后，`resume` 才能原子回到 normal；
non-archived且 pre-enable readiness健康时enable，已 archived则保持 disabled。任何代码
路径都不能用 defer / drop 自动恢复。

### 22.6 preflight

cutover 前全部满足：

1. Community 当前 schema = 2；v1 必须先走已有 v1 → v2；
2. Project Document v1 enabled / ready；
3. stable signer ready；
4. manifest `resource_id` set 与 cutover base 的 exact active legacy Resource ID set
   完全相等；每个 active Resource 恰有一个 reviewed mapping，且无 extra / stale entry；
5. 每个 mapped Guide 是同 Community active Document；
6. 不存在 nil / duplicate / cross-project ID；
7. 所有 v3 Resource body 可构造并通过 limits；
8. 所有 v3 ordinary / Role continuity heads 和 meta 可预签名并验签；
9. DB normalized ref constraints ready；
10. Relay、SDK、CLI、Desktop 已部署 dual reader / writer；每个 managed Assignment 的
    exact-epoch quiesced ack满足该 run固定的最低 ACP protocol version，build仅记录为
    诊断；
11. operator audit / idempotency input 已固定；
12. 每个 mapping 的全部 canonical digest 已重算、review signature 已验证，且
    `reviewed_by_pubkey` 仍是合格 current Human member；
13. 导出 cutover 前 canonical backup / manifest digest；
14. maintenance state = `frozen`，epoch exact match，全部 Assignment baseline 有
    compatible durable quiesced ack、全部 runtime baseline 有 durable ack，idle
    Assignment仍无 runtime，且无 active / recovering Runtime、scheduler claim或
    pre-cutover invalidation；
15. `project_context_enabled = false`，所有 v3 object 的初始 Context set 都是 canonical
    empty。

Context Reference 不从 Issue.about、Markdown link 或历史聊天自动猜测。cutover 初始全部为空，
后续由 Human / Agent 显式添加。

### 22.7 cutover transaction

流程：

1. operator 先用 22.5 的
   `begin → drain → durable assignment/runtime ack → freeze` 完成 maintenance；
2. cutover command 完成 Host / operator auth 与 audit input校验、以 closed codec解析
   manifest并计算 canonical request hash；
3. 取得 Community exclusive lock后，**先于 schema、enabled、maintenance state和 fresh
   preflight** 查询 immutable cutover operation receipt：
   - 同一 Community、maintenance epoch、idempotency key hash、manifest digest、target
     schema和 canonical request hash完全一致时，直接返回原 receipt并标记
     `replayed = true`；不写 revision、projection或 audit；
   - 同一 key 但 epoch / manifest / request任一不同返回 conflict；
   - 只有没有 receipt时才进入后续 fresh path；
4. fresh path 要求 schema = 2、maintenance epoch exact `frozen`、
   `project_view_enabled = false`，并确认 NIP-11 不再广告 Project View capability；
5. 重跑 preflight，重新 parse canonical manifest、重算 digest、验 reviewer signature，
   并验证 meta / project revision / generation、每个 legacy Resource
   object revision / body digest / projection pointer、每个 Guide revision / head /
   content digest 和 manifest digest 全部 exact match；
6. 构造一个 operator authenticated `ChangeSource`；
7. 把 exact reviewed entries逐项写入 15.1 的 immutable committed Resource child
   ledger，绑定本次 change ID；entry count / digest set必须与 manifest完全一致；
8. 为每个 current / tombstone object写一条 15.1 定义的 immutable provenance ledger
   row；Resource 指向本次 operator change，其他 object 指向 preflight核验过的 legacy
   mutation / typed change，Resource provenance同时命中 committed entry FK；
9. 对每个 object 回填 exact `source_provenance_id`，再把
   `project_view_schema_version` 和 row schema从 2改为 3；切换顺序必须让 deferred
   v3 non-null / FK constraint在 transaction末成立；
10. 对每个 Resource：
   - 在受限 migration record 中保存 legacy projection pointer、body digest 和 manifest
     entry；不再复制一份可能含敏感 locator 的业务 body；
   - 写 v3 body；
   - 写 `guide_document_id` column；
   - `object_revision + 1`；
   - `project_revision = cutover project revision`；
   - `updated_at = cutover canonical time`；
   - `updated_by = verified signed entry.reviewed_by_pubkey`；
   - `source_type = operator`；
   - `source_change_id = cutover ChangeSource.change_id`；
   - `source_event_id = NULL`；
11. 非 Resource object 与 continuity entity保留 object / entity revision、各自上次业务
    变化的 project revision、actor/time 与 source 三元组；context set初始化为空；
12. 整个 cutover 只增加一次 global project revision；
13. projection generation +1；
14. 重投影全部 v3 current ordinary / bounded-current continuity heads；
15. 写绑定 exact current NIP-43 snapshot 的 v3 reset meta；
16. 在同一 transaction写 immutable cutover receipt / operation idempotency / audit
    linkage，receipt绑定 maintenance epoch、manifest digest、target schema和 canonical
    request hash；
17. 执行 deferred parity；
18. commit；
19. 在 frozen / disabled 状态运行 `project_view_v3_structural_ready` full verify；
20. 验证 maintenance baseline 中每个仍 active 的 Assignment 在 v3 映射到同一 Role
    Brief authority；不一致则保持 disabled / frozen；
21. `maintenance resume --epoch <n>` 在一个 Community exclusive-lock transaction 中
    最后重查 v3 pre-enable readiness、manifest receipt、全部 post-cutover invalidation
    resolution与 epoch；若 non-archived则原子设置 `project_view_enabled = true` +
    `state = normal`，若已 archive则保持 disabled但结束 maintenance；
22. transaction commit 后，只有 eligible non-archived Community的 NIP-11才广告
    `buzz-project-view-v3`；ACP观察到 `normal`且 capability ready后才恢复新 Agent turn。

这样 cutover transaction 已 commit、response却超时的调用可以用同一 epoch / key /
manifest重试并拿到同一 receipt；即使后来已经 resume到 schema 3 / normal，replay-first
分支仍然成立。不能让重试先撞 `schema = 2` fresh preflight后误报失败。

多个 Resource 的转换共享一次 project revision，因为它们是同一个不可分割 schema
cutover；但每个 Resource 的 locator → reviewed Guide 转换是经过 Human 审查的业务语义
变化，因此各自增加一次 object revision，并归因给 manifest 中的 reviewer。这一点刻意
不同于当前 v1 → v2 给 Role 增加机械 `level` 字段的迁移。非 Resource 对象才沿用
mechanical-major-cutover 的 revision 保留语义。

### 22.8 失败处理，不提供提交后的 v3 → v2 rollback

区分两个边界：

```text
cutover transaction commit 之前
    任一错误触发普通 PostgreSQL rollback
    canonical v2、pointers 和 revision 不变
    maintenance仍保持 frozen、Project View capability保持 disabled
    显式 abort 终止 baseline；仅当 current v2 readiness健康才重开 capability

cutover transaction commit 之后
    schema 3 change、project revision、generation、audit / receipt已经成为历史
    不删除 change、不恢复旧 revision、不重新激活旧 pointer
    maintenance保持 frozen，直到 forward-fix + verify + explicit resume
```

首版不提供提交后的 v3 → v2 reverse migration，即使尚无 member command：

- Runtime supervisor、membership-coupled change、moderation、operator/system action 也可能
  已经推进 schema 3 canonical state；
- append-only Role history / audit 不能“倒带”；
- 旧 locator authority 已被 reviewed Guide 取代，自动反推是 lossy；
- Guide Documents 保留为正常项目资产，不做补偿删除。

commit 后发现问题：

1. disable Project View；
2. 保持 Document capability按自身 readiness运行；
3. 验证最后 change / project revision / generation；
4. 修复代码、mapping 或 canonical inconsistency；
5. 只通过 22.5 的 exact-epoch operator `maintenance repair` 产生新的、revision继续递增
   的 typed audited forward change，或通过 `maintenance reproject` 产生新 generation；
6. 保持 frozen，运行 structural verify；
7. verify通过后显式 `resume`，原子 enable + normal。

v0 不开放“任意 typed repair”占位符，而是固定：

```text
CanonicalMaintenanceRepairPlanV1
├── community_id
├── maintenance_epoch
├── cutover_change_id
├── expected_project_revision
├── expected_projection_generation
└── actions[]                 canonical sorted，max 4,096

RepairActionV1
├── ReapplyCommittedResource {
│     resource_id, mapping_entry_digest
│   }
├── RebuildObjectProvenance {
│     object_id, expected_business_body_digest, expected_source_digest
│   }
└── RebuildNormalizedContext {
      object_id, expected_business_body_digest
    }
```

JSON 只是 closed Human envelope；canonical bytes继续用 `postcard`，plan digest固定为
`SHA-256("buzz-pv3-maintenance-repair-plan-v1\0" || postcard(plan))`。admin先 dry-run并
输出 digest，执行时把 exact digest / idempotency / audit写 maintenance operation
receipt。

三个 action都只能从 immutable committed Resource entry、immutable source ledger或
当前 exact business body机械重建，不能提供新的 name / summary / Guide /
Context coordinate。一个 plan只推进一次 project revision；受影响 object推进 revision并
写 operator repair source，但保留已验证的 last business `updated_by`，Resource仍核对
committed reviewer。`reproject`只推进 generation，`verify`不推进任何业务 coordinate。
任何新的 Resource payload / Guide target必须等恢复 normal后走 member-signed v3 command；
若故障无法由上述三种 action修复，v0保持 frozen并要求新 repair contract版本，operator
不能使用 raw SQL / JSON patch绕过。

repair / reproject / verify receipt都写入同一 maintenance operation ledger，重试遵守
exact request replay。post-commit projection或canonical fault不能迫使 operator先解除
frozen再修复。

若未来确有法规或灾难恢复要求，v3 → v2 必须是新的显式 major reverse-migration设计：
重建 exact v2 canonical body、继续增加 project revision / generation、重签全套 heads /
meta，并定义所有 schema 3 history 的保留方式；它不属于本首版。

### 22.9 新 Community 直接初始化 v3

legacy cutover 不能成为新 Community 永久的必经路。当前 migration把
`communities.project_view_schema_version` 默认设为 1；v3 additive migration先保留这个
兼容默认，不静默改变任何既有或尚未初始化的 Community。另提供显式 provisioning：

```text
project_view_provisioning_operations
├── community_id
├── operation_id
├── operation                 prepare_v3
├── target_schema_version     3
├── idempotency_key_hash
├── canonical_request_hash
├── requested_by
├── audit_seq
├── result_receipt
├── accepted_at
├── consumed_by_change_id     nullable
└── consumed_at               nullable

PK (community_id, operation_id)
UNIQUE (community_id, idempotency_key_hash)
FK audit_seq             → audit_log
FK consumed_by_change_id → project_view_changes

communities.project_view_preparation_operation_id UUID nullable
FK (community_id, project_view_preparation_operation_id)
  → project_view_provisioning_operations
```

这个 ledger不依赖 `project_view_state` 或 maintenance epoch，正好能表示 uninitialized
bootstrap。identity / request / receipt禁止 UPDATE / DELETE；`consumed_*`只能由全 NULL
一次性写为 exact initialize change。prepare / initialize receipt都绑定 operation ID。

```text
buzz-admin project-view prepare-v3 \
  --community <id> \
  --idempotency-key <key>
```

`prepare-v3` 完成 operator auth / audit并计算 closed
`{community_id, target_schema_version:3}` request hash后取得 Community exclusive lock，
先按 idempotency key查询 provisioning ledger：exact hash返回原 receipt，即使准备已经
成功或 initialize已经消费；同 key不同 request返回 conflict。只有没有 receipt才检查
fresh preconditions。

fresh `prepare-v3` 只接受 non-archived、Project View disabled且完全 uninitialized 的
Community：没有 `project_view_state`、object、mutation / change、continuity或 projection
row。它在 Community exclusive lock下把目标 schema设为 3、
`project_context_enabled = false`，原子写 provisioning operation / Community pointer /
receipt，但不创建业务对象、不启用 capability。任一 v1/v2 history存在都拒绝；那类
Community仍走 22.1–22.8 的 v2 → v3 cutover。

随后由 eligible direct Human owner提交 kind `44300` 的 closed
`ProjectViewInitializeV3`。它在现有 Profile / initial Goals之外显式包含：

```text
initial_roles[]
  RoleDefinitionV3 complete fields
  level
  context_references = []

initial_governance_assignments[]
  member_pubkey
  role_id
  proposal_id
  assignment_id
```

request必须把 initialize 当下 current direct Human owner + admins集合**完整且恰好一次**
映射到不同 active admin-level Role；Role名称、职责、Role ID、proposal / Assignment ID
都由 Human显式给出，Relay不按成员名猜 Role，也不随机补 mapping。managed、banned或
timed-out governor拒绝；若某个 admin不应进入连续性模型，应先通过正常 membership /
moderation coordinator调整治理状态，再重新提交 initialize。

Relay只在 18.2 定义的 prepared-but-disabled bootstrap branch接受这一个 request；
generic v3 create / update / delete在同一状态全部 unavailable。

initialize coordinator在 Community lock内、current owner / ban / archive gate之后先按
signed command event ID查询 typed `initialize_v3` change / receipt。event、
preparation operation ID和request digest全部一致时直接 replay原业务结果并解析 current
projection pointers，不再创建 object / revision；同 event内容不可能变，另一 event在
initialized state返回 `already_initialized`。只有 receipt不存在时才执行下列 fresh
uninitialized checks。这样 commit后网络歧义不会卡在 bootstrap gate之外。

一个 restricted transaction 在同一 Community exclusive lock中：

1. 重验 schema 3 prepared、uninitialized、disabled、Context gate关闭和 owner签名；
2. 固定 exact current NIP-43 membership snapshot；
3. 以 project revision 1创建 Profile、Goals、Roles及每个 object provenance；
4. 创建 consumed authorization Proposal和 active Human Assignment seeds，复用 v2 已有
   uniqueness、Role level、governor和 membership continuity invariants；
5. 写 command event、typed change / receipt、canonical rows、continuity rows、v3
   heads、membership projection和 reset meta；
6. 把 preparation的 `consumed_by_change_id`绑定这次 initialize change；
7. 执行 deferred parity并原子 commit。

初始 Resource不是必填；后续只能在 Document capability ready后通过普通 v3 command创建
带 active Guide 的 Resource。初始化成功仍保持 disabled，先运行
`project_view_v3_pre_enable_ready`（包含 structural与dual-client deployment
preflight），再由 admin显式 enable。它不需要 legacy Resource manifest或 runtime maintenance，因为前置
条件证明没有旧 Project View / Assignment / Runtime state可迁移。

阶段 4 交付 prepare / initialize domain、DB、Relay、SDK与 rollback tests；阶段 5 只在
bounded canary验证真实初始化和 enable。阶段 7 broad-rollout gate通过后，新 Community
provisioner才可以把“显式选择 v3”作为产品默认；migration仍不通过改变 SQL default替
existing rows做隐式决定。本文末尾延期的“v1 → v3 direct cutover”仅指跳过 v2迁移已有
v1 history，不包括这条 empty-state v3初始化路径。

## 23. Error model

### 23.1 stable server categories

```text
invalid:project_document:<reason>
conflict:project_document:<reason>
restricted:project_document:<reason>
unavailable:project_document:<reason>
unsupported:project_document:<reason>
error:project_document:<reason>
```

重点 reason：

```text
invalid_json
content_too_large
invalid_document_id
invalid_snapshot
no_change
revision_target

revision
id_exists
still_referenced
snapshot_changed

global_credential_required
membership_required
assignment_required
runtime_fence

disabled
not_ready
stable_signer
schema
```

Project View v3 对 cross-domain target 使用：

```text
invalid:project_view:context_reference
conflict:project_view:reference_target
conflict:project_view:object_still_referenced
unavailable:project_view:document_capability
unavailable:project_view:context_capability
```

### 23.2 CLI exit mapping

沿用现有契约：

| Exit | 含义 |
|---:|---|
| 0 | success |
| 1 | input / not found |
| 2 | network / generic relay / delivery unknown |
| 3 | auth / restricted |
| 4 | unsupported / unavailable / definitive protocol internal |
| 5 | write conflict |

stderr JSON 沿用当前：

```json
{"error":"conflict","message":"...","retryable":false}
```

字段名是 `error`，不是 `category`。Document command 层要把 stable
`unsupported:* / unavailable:* / error:project_document:*` Relay response 显式映射为
exit 4；若原样保留为 generic `CliError::Relay`，503 / 500 会误落到 exit 2。无法证明是
Relay protocol response 的 generic proxy / HTTP 500 仍是 exit 2。write 曾有 ambiguous
attempt时优先遵守 19.4 的 `DeliveryUnknown` 规则。snapshot restart 耗尽和 write
revision conflict都映射 exit 5、`retryable: false`。正文、summary 和疑似 Secret 不进入
error text。

### 23.3 deleted 与 not found

区分：

- 从未存在；
- current tombstone；
- active revision不存在；
- pinned revision是 tombstone；
- 无权限。

外部 response 不得通过差异向无权限调用者泄露 Document 是否存在；先做 auth，再做 existence
lookup。已授权 CLI 可以把 tombstone current 显示为 deleted metadata。

## 24. Observability 与运维

### 24.1 metrics

新增：

```text
buzz_project_document_commands_total{operation,result}
buzz_project_document_command_duration_seconds{operation}
buzz_project_document_conflicts_total{operation,reason}
buzz_project_document_projection_failures_total{projection_type}
buzz_project_document_query_duration_seconds{query_type}
buzz_project_document_snapshot_restarts_total{surface}
buzz_project_document_active_count
buzz_project_document_revision_count
buzz_project_document_body_bytes
buzz_project_document_lock_wait_seconds
buzz_project_document_reproject_progress{generation}

buzz_project_view_v3_context_reference_count{target_type}
buzz_project_view_v3_cutover_total{result}
buzz_project_view_maintenance_transitions_total{from,to,result}
buzz_project_view_maintenance_pending_assignment_count
buzz_project_view_maintenance_pending_runtime_count
buzz_project_view_maintenance_duration_seconds{terminal_state}
```

不得用 document ID、title、resource name、event ID 作 metric label。

### 24.2 structured logs

允许记录：

- Community host / ID；
- operation；
- command event ID；
- actor pubkey；
- expected / committed revision；
- document ID（仅受控 debug / operation log，非 metric label）；
- result code；
- duration / bytes count；
- maintenance epoch / state / baseline count / pending ack count；
- manifest digest / mapping entry digest（不记录 Guide content）。

禁止记录：

- title；
- summary；
- Markdown；
- locator；
- patch body；
- suspected Secret value。

### 24.3 readiness / admin

新增 admin read-only / controlled operations：

```text
buzz-admin project-document status
buzz-admin project-document preflight
buzz-admin project-document enable
buzz-admin project-document disable
buzz-admin project-document verify
```

Project View v3 cutover 另提供 22.5 定义的 maintenance begin / status / freeze / abort /
resume；普通 `project-document disable` 不等同于 Runtime drain。

Context sub-capability另提供：

```text
buzz-admin project-view context status --community <id>
buzz-admin project-view context enable --community <id> --idempotency-key <key>
buzz-admin project-view context disable --community <id> --idempotency-key <key>
```

enable transaction要求 schema 3 structural ready、Project View advertised-ready、
normalized parity与部署侧 closure preflight通过；disable可随时 fail closed且不删除 ref。
disabled期间 verified readers仍可列出 refs，writer只能提交 canonical subset，RoleBrief
标记 `unavailable_preserved`且不自动注入。re-enable重新验证全部 preserved targets /
normalized parity并在 commit后恢复 advertisement；下一次 RoleBrief resolve才重新
hydrate，不能使用 disable前 cache。普通 member command不能自行翻转这个 gate。

阶段 7 的 generation-aware visibility 落地后才增加：

```text
buzz-admin project-document reproject --all-revisions
```

status 至少显示：

- enabled；
- schema；
- signer / generation；
- catalog revision；
- active / revision count；
- meta parity；
- orphan / pointer mismatch count；
- reproject state（阶段 7 后）。

enable / cutover / reproject 是有审计影响的外部写操作，不能由普通 client command 模拟。

## 25. 测试与质量门

### 25.1 pure domain tests

`buzz-project-document`：

- UUID v4 / nil / variant；
- create revision 1；
- exact update CAS；
- 不同 Document revision independence；
- full snapshot；
- no-op；
- title / summary / Markdown byte limits；
- JSON depth / escaped size；
- active / tombstone shape；
- delete；
- no restore / ID reuse；
- MAX_SAFE_REVISION overflow；
- deterministic projection plan；
- actor / canonical time；
- property tests：任意 accepted transition 后 invariants 恒成立。

Project View v3：

- unknown valid `resource_kind` accepted；
- invalid token rejected；
- Guide required；
- 每种 source / target matrix；
- Live / Pinned distinction；
- full-coordinate duplicate / Resource-first canonical order / max 64；
- 同一 Document 的 Live + 多个不同 Pinned revision；
- Resource source不能引用 Resource；
- `project_context_enabled = false` 时 create只接受 empty set，update只接受
  `None`或当前 canonical set的 subset；新增 / retarget raw v3 command稳定返回
  capability unavailable；
- gate enabled后相同 canonical nonempty set才进入 target proof / reducer；
- target active / revision state；
- delete incoming protection；
- Resource delete不删除 Guide；
- RoleDefinition v3 context；
- non-tombstoned Role 只产生一个 RoleDefinition head，object / entity pointer相同；
- greenfield `ProjectViewInitializeV3` 要求 exact current Human owner/admin集合、unique
  admin Role与 empty Context，不能缺项、重复或自动猜 mapping；
- v2 payload 不被 v3 parser 接受，反之亦然。

### 25.2 SDK protocol tests

- golden event IDs；
- exact tags；
- wrong signer；
- wrong kind；
- content / tag mismatch；
- cross-project coordinate；
- noncanonical decimal；
- duplicate / extra tag；
- bad source / revision pointer；
- active head + wrong revision binding；
- tombstone泄露旧 body；
- meta changed-head parity；
- v2 / v3 dual parser separation；
- v3 meta完整继承 v2 membership pointer 与六类 counts；
- current v3 Role slice不接受 v2 event，history versioned union分别 strict parse；
- base `RoleBriefV3` 在 Context gate关闭时 roundtrip为 empty context +
  `availability:not_advertised_empty` + `document_metadata:not_required`，v2 / v3
  strict parser互不 fallback；
- preserved refs但 Context capability unavailable时 roundtrip为 empty injected lists +
  `availability:unavailable_preserved` counts；
- serialize → parse roundtrip；
- all new kinds classifier coverage；
- generic `InvalidProjection` error 不再把 Document误标为 Project View。

### 25.3 database / migration tests

- migration from current `0031` schema；
- default disabled；
- bootstrap empty meta；
- command / current / revision / event / receipt atomic rollback；
- `project_document_changes` 四种 strict source shape 与三个 partial unique index；
- history append-only guard；
- hard delete guard；
- current pointer parity；
- active count parity；
- replay after commit；
- replay after membership removal rejected；
- signer mismatch readiness fail；
- revision overflow；
- Document A/B 无 revision conflict；
- concurrent same-Document updates only one succeeds；
- Document delete vs new Live ref serialized；
- Resource delete vs new Resource ref serialized；
- pinned ref survives Document tombstone；
- live ref / Guide blocks delete；
- additive migration保留 schema default 1；`prepare-v3` 只接受 disabled / empty state并
  idempotent，已有 v1/v2 history或半初始化 row都拒绝；
- prepare commit后 response丢失、甚至 initialize消费后，用同 key重试仍 replay原
  provisioning receipt；changed request conflict，audit / consumed linkage不可篡改；
- owner-signed greenfield v3 initialize一次原子创建 Profile / Goal / Role /
  governance continuity / provenance / heads / meta，任一失败全 rollback；
- prepared-but-disabled只接受 owner InitializeV3；同一 accepted event在 commit后
  response丢失可 exact replay，另一 initialize、其他 mutation或无 prepare receipt都拒绝；
- initialize receipt replay仍先重验 current owner / ban / archive，失权 signer不能借旧
  receipt绕过；
- cutover idempotency / manifest digest；commit后 response丢失、frozen中重试或已经
  resume到 schema 3 / normal 后重试都先返回同一 receipt且不增 revision；
- 同一 cutover key更换 epoch / manifest / target schema / request hash返回 conflict；
- manifest Resource ID set 少一项、多一项、duplicate 或含 stale/tombstone entry 都 abort；
- manifest base meta / Resource / Guide 任一 pointer、revision 或 digest变化都会 abort；
- postcard canonical bytes / digest golden fixtures；
- 修改 final payload、Guide pointer、reviewer、review time、signature 或 entry order 后
  validate 都失败；
- invalid signature、operator 替换 reviewer、非 Human / timeout / banned reviewer 被拒绝；
- Resource cutover object revision +1、`updated_by` 来自 verified attestation；
- Resource cutover 使用 operator `source_type / source_change_id` 且
  `source_event_id = NULL`；
- Resource ledger source actor与 operator typed change一致（可为 NULL），业务
  `updated_by` 则独立核对 signed reviewer receipt；
- legacy member-authored object source topology backfill 正确：v1 source 从
  `project_view_mutations` 验证，v2/v3 source 从 `project_view_changes` 验证，missing /
  mismatched provenance fail closed；
- untouched v1-initialized Profile / Goal 通过 exact stored initialize command验证，而
  malformed payload / wrong Goal ID fail closed；
- object provenance ledger 与被引用的 v1 mutation / v2-v3 change source 都拒绝 UPDATE /
  DELETE；
- cutover逐项写 immutable committed Resource entry；staging后续变化不影响归因，
  committed entry UPDATE / DELETE、missing Resource provenance FK或reviewer不一致都
  fail closed；
- 其他 object 的 source 三元组与 revision / actor / time 全部保留；
- projection 可从 operator source union 构造，不能把 change ID 解析为 Nostr event ID；
- maintenance begin 固定 exact Assignment + Runtime baseline；
- active managed Assignment无 binding / duplicate binding时 begin拒绝；bound idle
  Assignment必须进入 Assignment baseline；
- begin在 v2 disabled / structurally unready时拒绝，不会让 abort误启用 Community；
- begin后 archive / readiness故障再 abort只恢复 maintenance normal并保持 PV disabled；
- 多个 maintenance run保留 historical epoch / begin receipt；restart后 exact begin /
  transition replay仍返回原结果；
- missing / forged / stale-epoch ack 阻止 freeze；
- idle Assignment没有 exact quiesced ack、client protocol version过低或只做过 GET
  poll时 freeze拒绝；build值不参与未定义的字符串排序；
- assignment / runtime baseline full FK、unified cross-variant ack idempotency、
  append-only guard和 changed-request collision全部生效；
- Runtime evidence 与 freeze 并发时只有一个能先持有 Community lock，不能穿透 fence；
- scheduler claim 与 drain 并发不产生 drain 后新 claim；
- begin 前 existing scheduler claim 阻止 freeze，compare-and-clear release 可清理，而
  claimed system action commit被拒绝；
- draining / frozen 期间 ban / owner removal / archive不被阻塞；pre-cutover typed
  invalidation使 fresh cutover fail closed，post-cutover invalidation必须由后续
  verify / repair / reproject exact resolve后才能 resume；
- archive / unarchive使用 audit-backed invalidation，membership / moderation使用
  change-backed invalidation；wrong source shape / FK和重复 resolution都拒绝；
- schema 2 / 3 membership-moderation coordinator把 ban / unban、Role continuity、
  NIP-43 projection、audit和 maintenance invalidation原子提交；
- cutover pre-commit 失败后仍 frozen，显式 v2 verify + abort 才恢复；
- partial runtime ack 后 abort使全部 baseline coordinates失效；恢复后只能 fresh runtime
  ID / epoch；
- partial cutover rollback；
- post-commit canonical / projection fault只能通过 exact-epoch typed repair /
  reproject前进；成功仍 frozen，structural verify后才 resume；
- repair plan包含任何新 Resource payload / Guide / Context coordinate、unknown action或
  committed digest不匹配都拒绝；
- frozen / disabled 可以 structural-ready，但 advertised readiness、NIP-11和 member
  writes仍为 false；
- Context gate disabled时 nonempty raw command即使 target有效也拒绝，enabled后才写入
  normalized rows；
- v3 write 后 downgrade blocked；
- schema 3 下 owner / ban / Assignment / Runtime / Work / Commitment / Checkpoint / Handoff
  validators继续生效；
- 四个 schema CHECK 和 `0026 / 0027 / 0028 / 0029` validator 已显式支持 v3；
- reproject generation activation。

### 25.4 Relay security tests

对 WebSocket 和 HTTP 同时覆盖：

- anonymous REQ / COUNT；
- non-member；
- channel-restricted auth；
- missing `messages:read` / `messages:write`；
- banned actor read / write 都拒绝；
- timed-out current member read允许、write拒绝；
- managed owner removal；
- ended Assignment；
- stale runtime fence；
- managed actor无 supervisor binding时 Document v1 / Project View v3写拒绝，而
  legacy v2 optional-supervision回归保持到 cutover；
- capability disabled；
- `project_context_enabled = false` 时不广告 `buzz-project-context-v1` 且 raw nonempty
  v3 mutation拒绝；gate + closure ready后才广告并接受；
- archived Community；
- wildcard query；
- mixed kind filter；
- unsupported search；
- forged relay-only event；
- Document command 被 generic command executor 排除，但仍先经过现有 global /
  channel-token auth gate，再在 Project View sibling route 进入专用 handler；
- command、canonical、receipt、三类 projection 任一失败全事务 rollback；
- success receipt / response bytes在 commit前已构造；模拟 commit后 response transport
  failure只能得到 ambiguous结果并 exact read-back，不能返回 definitive stable internal；
- Redis fan-out recipient；
- subscription established 后 membership removal；
- security gate before receipt。

maintenance operational HTTP 另覆盖：

- wrong Host / resolved Community mismatch；
- missing / invalid / expired NIP-98 与 body-hash mismatch；
- auth-event replay、same ack idempotency replay 与 changed-request conflict；
- wrong supervisor / binding / Assignment / runtime coordinate；
- cross-epoch / stale baseline；
- draining 与 frozen allowlist；
- maintenance non-normal 时 Project View NIP-11 不广告且 write拒绝；
- frozen / disabled structural verify可以成功，但 advertised readiness仍失败；
- Relay restart 后 maintenance state、baseline、poll 与 ack仍然生效。

### 25.5 CLI / admin tests

- top-level command stability；
- global `--format compact`；
- create / update 都要求 title 与 exactly-one content input；
- create 的 omitted summary = `None`，update 要求 summary / clear-summary exactly one，
  empty summary 拒绝；
- bounded file / stdin / patch input 在 `limit + 1` bytes 拒绝；
- list pagination / snapshot restart；
- current / pinned / tombstone get；
- `--content-only`；
- stdin / file；
- exact patch success / context mismatch；
- patch conflict 只在显式 `--output` 时写文件，不创建隐式 temp；
- update requires expected revision；
- delete requires expected revision；
- conflict exit 5；
- snapshot conflict耗尽 exit 5 且 `retryable:false`；
- unsupported / unavailable exit 4；
- 第一次 definitive canonical unavailable 直接 exit 4；
- ambiguous write 后再遇 unavailable 仍返回 `DeliveryUnknown` exit 2；
- ambiguous write 可由 exact revision read-back + matching source event 恢复 success；
- `error:project_document:*` definitive internal 映射 exit 4；
- stderr envelope使用 `error` key；
- auth exit 3；
- delivery unknown；
- receipt / projection tamper；
- approval command 只生成 detached review signature，不提交 Relay mutation；
- approval signer eligibility 与 signature verification；
- `resources guide` 返回 Resource source revision，`--revision`只选择 Guide revision；
- 不依赖 Role Brief的
  `project-view get → resources guide --content-only`
  Agent发现链；
- v2 / v3 Project View read / write selection。

`buzz-admin` 另覆盖：

- prepare-v3 / initialize preflight与 closed arguments；
- maintenance begin / freeze / abort / verify / repair / reproject / resume 的 exact epoch CAS、
  required idempotency key和 restart replay；
- Context status / enable / disable要求 schema3、readiness与idempotency，disable不删除
  normalized refs；
- cutover `--maintenance-epoch / --idempotency-key` 必填，同 key changed manifest
  conflict；
- manifest bounded input、entry cap、closed parse、digest / signature error不回显
  locator / Guide content；
- repair plan只接受 v1三个 fixed action、canonical digest与expected coordinates，不能
  退化成 raw SQL / JSON patch或业务内容编辑。

### 25.6 Tauri / TypeScript tests

Rust native fixtures：

- capability / signer；
- verified assemble；
- closed command；
- receipt；
- 409 conflict；
- typed snapshot 409 / canonical 503 / definitive internal；
- wrong head / revision pointer；
- Community changed during await不会 retarget captured URL / key；
- 五个 command 都要求并原样回传 explicit `community_key`，且全部注册到
  `generate_handler!`；
- local Community key 与 signed Project ID不会混用；
- `get_project_document_meta` identity bootstrap；
- v2 / v3 Resource parse。

TypeScript：

- normalizer / serializer integrity；
- Context Reference canonical order；
- Context disable后 preserved refs仍显示 / round-trip，add拒绝而 subset cleanup可用；
- re-enable重新验证并 invalidates旧 Context / Document metadata cache；
- query keys含 local Community key、signed Project ID、Relay pubkey和generation；
- meta query不自依赖 catalog result；
- live burst coalescing；
- forged raw live event只能触发 verified refetch，不能 cache-warm；
- pinned cache immutable；
- mutation invalidation；
- no module singleton leak。

### 25.7 Playwright

重点 E2E：

1. metadata list 不预取 body；
2. create / read / update full snapshot；
3. conflict 保留 draft；
4. history / pinned；
5. delete / tombstone；
6. create Resource + existing Guide；
7. create Resource + new Guide saga 第二步失败；
8. Resource → Guide lazy open；
9. Context add / remove；
10. Live Document metadata更新不推进 PV revision；
11. Document delete 被 Guide / Live ref 阻止；
12. Pinned ref 在 delete 后可读；
13. community switching；
14. live invalidation；
15. tampered projection fail closed；
16. Markdown code / link 安全渲染。

新增固定入口 `desktop/tests/e2e/project-document.spec.ts` 后，必须：

- 把 spec 加入 `desktop/playwright.config.ts` 的 `smoke.testMatch`；
- 在真实 invoke switch `desktop/src/testing/e2eBridge.ts` 为
  `get_project_document_meta / list_project_documents / get_project_document /
  get_project_document_history / mutate_project_document` 五个 native command 提供
  per-Relay mock state；`desktop/tests/helpers/bridge.ts` 只负责安装 / 注入配置；
- 增加 Document live-event emitter 与 subscription-ready probe，测试先确认订阅再发
  invalidation；
- 每次 screenshot 前调用现有 `waitForAnimations`；
- focused 运行
  `pnpm --dir desktop test:e2e:smoke -- project-document.spec.ts`。

### 25.8 ACP / Role Brief tests

- v2 Role Brief regression；
- Context gate关闭时 base `RoleBriefV3` 使用 empty context /
  `availability:not_advertised_empty` / `document_metadata:not_required`，strict v3
  output在 canary cutover前可用；
- 有 preserved refs的 disable输出 `unavailable_preserved` counts且不hydrate；re-enable
  后重新 resolve为 ready，不使用旧 metadata cache；
- v3 current Role / Work closure；
- Resource / Document 一跳；
- 不内联正文；
- fetch command；
- Pinned exact revision；
- Pinned不后台读 revision body、不借用 current title / summary；
- tombstone object不展开；
- Resource先选、Guide必入、supplementary context cap / truncation；
- Document meta A/B retry；
- cache key包含 Document meta；
- metadata unavailable 不使用 stale值；
- PV identity failure仍 suspend；
- Document metadata瞬时失败不 suspend Assignment；
- Document enrichment失败仍完成 runtime reconcile；
- 没有 Role Brief Context时，base prompt仍能通过 Project View Resource ID发现并按需读
  Guide；
- metadata newline / delimiter / prompt-injection fixture被安全引用；
- community switch / relay identity change清 cache；
- normal 时允许新 turn，draining 后 admission fail closed；
- online-idle Assignment没有 turn时，long-lived watcher仍按 poll hint看到新 epoch并提交
  quiesced ack；watcher停掉则保持 pending；
- ACP在 draining中冷启动先进入 maintenance-only mode，不先做 Role Brief、Runtime
  Start / Recovery或创建 pool，仍能清理 exact baseline并 ack；
- active turn 观察新 maintenance epoch 后严格执行 latch、撤 fence / pause renewal、
  join全部任务、runtime ack、assignment ack；任一步未完成都不得提前 ack；
- heartbeat、Role Brief / canvas、session create、`initial_message`、prompt child各阶段
  注入 drain时都能 cancel或保持 pending直到真正 join；
- maintenance latch与 concurrent `reconcile / publish_current_fence` race时不能重发
  fence或续 lease；
- drain会 pause wake / respawn并 shutdown / reap AgentPool所有 active + idle
  `OwnedAgent`；durable child registry未清空不得 ack；
- ack 网络歧义通过 exact baseline coordinate read-back，不重复启动 runtime；
- resume 后只用新 runtime ID / epoch，旧 fence 不复用；
- partial drain后 abort使旧 coordinate在 server失效；已 latch / 未见 epoch的 ACP都只能
  fresh admission，不能 republish旧 fence；
- abort / resume后重建 fresh Runtime与AgentPool，idle slot / child不跨 epoch复用；
- ACP host 离线或未 ack 时 admin status保持 pending，freeze拒绝。

### 25.9 quality commands

按阶段运行 focused gates：

```bash
. ./bin/activate-hermit
just project-document-test-unit
just project-document-test-db
just test-migrations
just project-document-test-e2e
just project-view-test
just desktop-tauri-test
pnpm --dir desktop test
pnpm --dir desktop test:e2e:smoke -- project-document.spec.ts
```

这些 recipe / script 也是交付物，不是文档中的假想别名：

- `project-document-test-unit` 覆盖新 crate 以及 core / SDK / Relay / CLI / ACP 的
  `project_document` no-infra slice，并覆盖 `buzz-admin` 的 Document status / enable /
  verify argument与preflight；
- `project-document-test-db` 使用独立脚本启动 Postgres 并显式运行 ignored transaction /
  race tests，模式与 `scripts/test-project-view-db.sh` 相同；
- `test-migrations` 扩展 fresh / upgrade / concurrent schema drift 检查；
- `project-document-test-e2e` 使用真实 Relay + CLI；
- `project-document-test` 聚合以上 gates；
- 现有 `project-view-test-unit` 的 nextest package list与
  `scripts/run-tests.sh` cargo fallback都扩展到 `buzz-acp --lib` 的 v3 Role Brief /
  maintenance slice；`buzz-admin` 当前只有 bin target，因此单独用
  `cargo nextest run -p buzz-admin` / `cargo test -p buzz-admin`覆盖 prepare /
  manifest / maintenance / cutover tests，不能写一个不存在的 `--lib` target，也不能只把
  测试写进 crate却不让 recipe运行；
- `project-view-test-db / e2e` 扩展 greenfield v3 initialize、Context gate、durable
  maintenance、cutover replay与frozen repair；
- `scripts/run-tests.sh` 的 nextest 与 cargo fallback 都包含新 crate。

当前 `just ci` / `test-unit` 是显式 package / recipe 列表，新建 workspace crate不会自动获得
覆盖。因此阶段 1 必须把 `project-document-test-unit` 接入 `test-unit`，并保证
`just ci` 间接运行它；不能只在本地手动执行一次 `cargo test -p
buzz-project-document`。

infra gates 也不会被 `just test` 自动覆盖：当前 `scripts/run-tests.sh` 明确跳过 ignored
Postgres tests，且不运行 migration / real Relay + CLI E2E。必须更新
`.github/workflows/ci.yml`：

- paths filter覆盖 `buzz-project-document`、相关 DB / Relay / SDK / CLI / ACP、migration
  与 `scripts/test-project-document-*.sh`；
- `project-view` paths filter显式加入 `crates/buzz-acp/**`，并继续覆盖
  `crates/buzz-admin/**`、maintenance migration与相关 scripts；
- dedicated integration job依次运行 `project-document-test-db`、`test-migrations`、
  `project-document-test-e2e`；
- cache hash包含新 scripts / crate；
- 新增或扩展 release-contract script，断言 recipes、nextest与cargo fallback package
  list、ACP/admin paths filter和 CI jobs没有漂移。

触及 Relay / DB / auth 时运行需要 Postgres / Redis 的 integration suite。完整 PR 合并前：

```bash
. ./bin/activate-hermit
just ci
just test
just project-document-test
just project-view-test
```

Desktop Tauri、mobile 和 screenshot 继续遵守仓库 `AGENTS.md` 的 worktree / quality rules。

## 26. 分阶段开发计划

总体依赖：

```text
阶段 0  协议与不变量
   ↓
阶段 1  Document kernel / DB / SDK（flag off）
   ↓
阶段 2  Relay + CLI Document 纵向闭环
   ├──────────────┐
   ↓              ↓
阶段 3          阶段 4
Desktop         Project View v3 backend / migration（flag off）
Documents         │
   └──────┬───────┘
          ↓
阶段 5  v3 dual clients + Resource Guide cutover
   ↓
阶段 6  Context Reference + Role Brief / Agent
   ↓
阶段 7  hardening / rollout / adapter observation
```

阶段之间不以“代码写完”为完成标准，而以对应 exit criteria 为准。

### 阶段 0：协议、limits 与 golden fixtures

目标：在实现前固定跨 crate 合同。

> 交付状态（2026-07-31）：已完成。规范入口为
> [`docs/nips/NIP-PD.md`](../../../nips/NIP-PD.md) 与
> [`docs/nips/NIP-PV3.md`](../../../nips/NIP-PV3.md)，共享 fixture 位于
> [`docs/nips/fixtures/project-document-v1/`](../../../nips/fixtures/project-document-v1/)，
> 具体冻结决策和阶段边界记录在 [`changelog.md`](changelog.md)。阶段 0 只注册 kind 和
> pure contract；未创建 migration、未增加 Relay route、未广告 capability，也未开放读取。

开发内容：

- 新建 Project Document protocol / NIP 文档；
- 注册 `44301 / 40905 / 40906 / 40907`；
- 更新 kind classifiers / collision tests；
- 固定 command / head / revision / meta JSON 和 exact tags；
- 固定 coordinates；
- 固定 limits、error codes、receipt shape；
- 固定 v3 Resource / Context Reference shape；
- 固定 greenfield `ProjectViewInitializeV3`、base `RoleBriefV3` 和 Context
  sub-capability gate；
- 固定 migration canonical structs、digest domains、review signature 与 maintenance state
  machine；
- 固定 capability names 和 version matrix；
- 生成 shared golden fixtures；
- 记录 signer rotation v1 运维边界。

验收：

- 协议示例可由独立 test parser roundtrip；
- 所有 kind classifier tests 通过；
- v1/v2/v3 payload 互相 fail closed；
- 没有未决 wire 字段会迫使阶段 1 改 migration。

建议 PR：

1. protocol docs + kind allocation；
2. golden fixture harness + pure type skeleton。

### 阶段 1：Project Document kernel、SDK 与 canonical DB

目标：建立 flag-off 的可信内核，不暴露半成品 capability。

> 交付状态（2026-07-31）：已完成。新增 pure reducer、SDK strict protocol adapter、
> additive migration `0032`、restricted canonical DB transaction、只读 admin status /
> preflight，以及 Relay private deny skeleton。`project_document_enabled` 没有 enable 写入口，
> 真实 Community 不执行 bootstrap，NIP-11 不广告 capability，Document public handler与 CLI
> 仍留在阶段 2。具体交付和验证记录见 [`changelog.md`](changelog.md)。

开发内容：

- 新建 `buzz-project-document` pure crate；
- domain validation / reducer / projection plan；
- SDK builders / strict parsers，并泛化 `SdkError::InvalidProjection` 文案；
- 下一可用 additive migrations；
- `project_document_state / documents / revisions / changes`；
- constraints / indexes / hard-delete guard；
- `buzz-db::project_document` restricted transaction；
- empty catalog bootstrap 的 DB builder / tests，但不在真实 Community 执行；
- admin status / preflight；
- signer readiness；
- relay-only submission rejection 和 private protocol query / fan-out deny skeleton，确保任何
  测试或误插入 event 都不能通过旧 wildcard 路径泄露；
- 新增 `project-document-test-unit / test-db / test-e2e / test` recipes 与 scripts，并把
  no-infra gate接入现有 `test-unit` / `just ci`；
- 无 Relay public routing。

验收：

- pure / property / SDK golden tests；
- DB transaction atomicity；
- same-Document race；
- tombstone history；
- receipt replay；
- migration从 current schema干净通过；
- `project_document_enabled` 始终 false；
- 真实 Community没有 Document projection event；
- 普通客户端无法提交或读取新 kinds。

建议 PR：

1. pure domain + SDK；
2. migration + DB canonical read/write；
3. admin preflight + private deny skeleton。

### 阶段 2：Relay 与 Agent-first CLI 纵向闭环

目标：Human / managed Agent 可以可靠维护和读取独立 Project Document。

> 交付状态（2026-07-31）：已完成。Relay 已接入 atomic command、全部 private read surface、
> closed catalog/history pagination、current-membership final fan-out 和 host-scoped NIP-11；admin
> 提供 bootstrap / verify / enable / disable；Agent-first CLI 提供完整 CRUD、exact patch、verified
> current / pinned / history、typed ambiguous delivery read-back。Secret incident runbook、隔离 canary
> 与验证记录见 [`secret-incident-runbook.md`](secret-incident-runbook.md)、
> [`stage2-canary.md`](stage2-canary.md) 和 [`changelog.md`](changelog.md)。Desktop、Project View v3
> Resource / Context 与正文 prompt 注入仍不属于本阶段。

开发内容：

- Relay command handler；
- private read gate 抽象；
- REQ / COUNT / `/query` / `/count` / fan-out 接线；
- catalog query extension；
- NIP-11 Document capability；
- admin bootstrap / verify / enable / disable；
- CLI `documents list/get/history/create/update/delete/patch`；
- receipt + read-back confirmation；
- ACP base prompt只增加 Document discoverability，不提前声称 Resource Guide / Context已
  可用；
- 交付 13.4 的 Secret incident runbook并完成一次 canary drill；
- focused WS / HTTP / Redis security tests。

首个 canary：

- 只选 Project View v2 Community；
- 先由 Human 创建普通 Document；
- 再由 active Assignment managed Agent 读取和更新；
- 验证 Role / Runtime fence；
- 验证 body 不进入普通 prompt；
- 验证 delete / pinned history。

验收：

- CLI 构成完整 Document CRUD + history 闭环；
- current / pinned 可复现；
- 全部读取面无非成员泄露；
- stale Runtime 不能写；
- 不同 Document 不出现 revision conflict；
- capability disabled 时 fail closed；
- commit后 response故障走 ambiguous read-back，不能误报 definitive internal；
- Secret drill能在不复制正文的前提下 disable / rotate / assess / reviewed re-enable；
- canary 可 disable 并保留 canonical state。

建议 PR：

1. Relay write vertical；
2. Relay read / query / private gates；
3. CLI / base prompt；
4. integration / canary tooling。

### 阶段 3：Desktop Documents

目标：Human 不依赖 CLI 也能维护可靠 Markdown revision。

> 交付状态（2026-08-01）：已完成。Desktop 已提供五个 native verified command、metadata-first
> React Query cache、Documents list / reader / editor / history、safe Markdown、current/pinned
> 隔离、conflict draft preserve、live hint invalidation与 Community switch隔离；native success同时
> 验证 receipt和 exact immutable revision read-back。Desktop contract tests、6条 Playwright E2E与
> hash互异截图已覆盖本阶段 exit criteria，具体记录见 [`changelog.md`](changelog.md)。Project View
> v3 Resource / Context backend与 cutover tooling仍属于阶段 4，未在本阶段提前实现。

开发内容：

- Tauri verified read / mutate commands；
- React Query metadata / body / history cache；
- Documents list / viewer / editor；
- safe Markdown；
- conflict draft preserve；
- pinned history；
- live invalidation；
- community switch tests；
- exact patch / diff UI；
- UX 中的 Secret warning。

验收：

- 页面初开不拉正文；
- current / pinned 区分；
- 409 不丢本地内容；
- live event 不把 raw body直接塞 UI；
- tampered signer / pointer fail closed；
- Desktop E2E 和 screenshots；
- 无新 community-scoped singleton 泄漏。

建议 PR：

1. native verified API；
2. list / reader；
3. editor / conflict / history；
4. live / E2E / screenshots。

### 阶段 4：Project View v3 backend 与 cutover tooling

目标：实现但暂不启用新的 Resource / Context schema。

开发内容：

- `buzz-project-view::v3` closed types；
- v3 Resource / Context reducer；
- `project_context_enabled DEFAULT FALSE`、nonempty server write gate与独立
  `buzz-project-context-v1` advertisement readiness；
- `ProjectObjectCommandV3` Role CRUD、continuity-only `RoleCommandV3` 与单一
  non-tombstoned `RoleDefinitionV3` head；
- normalized ref tables / Guide column；
- immutable per-object source provenance ledger / verified legacy backfill /
  operator-system projection source union；
- cutover-time immutable committed Resource reviewer-entry ledger与staging隔离；
- bounded sparse Document target proof；
- delete protection；
- v3 SDK projections；
- v3 Relay handler，并拆分 structural readiness与advertised write readiness；
- 四个 schema CHECK、`0026 / 0027 / 0028 / 0029` validator 及全部 Rust schema-2
  branch inventory；
- closed manifest canonical codec / digest golden fixtures / Human approval signature；
- migration staging mapping / export / approve / validate / cutover / transaction rollback；
- durable maintenance current pointer、historical epoch / operation ledger、Assignment /
  Runtime baseline、unified ack receipt与两类 ack tables；
- runtime evidence、binding、scheduler 与 Project View system/operator path 的 shared-lock
  fence；
- maintenance admin begin / status / freeze / abort / verify / repair / reproject / resume；
- v2/v3 membership / moderation coordinator，把 ban / unban、Role continuity、NIP-43
  projection、audit和 maintenance invalidation原子化；
- archive / unarchive Community admin path接入同一 lock与 audit-backed maintenance
  invalidation；
- uninitialized Community的 idempotent `prepare-v3` 与 owner-signed
  `ProjectViewInitializeV3`；
- v2 history versioned reader；
- feature flag / schema constraint；
- `RoleBriefV3` protocol fixture可先固定，但 runtime implementation在阶段 5；
- 扩展 `project-view-test-unit` nextest / cargo fallback、DB / E2E recipes、CI ACP/admin
  paths与release-contract；
- 不切任何真实 Community。

验收：

- v2 regression全绿；
- v3 domain / DB / relay tests全绿；
- schema 3 下 owner / ban / Assignment / Runtime / Work / Commitment / Checkpoint / Handoff
  semantics全绿；
- non-tombstoned Role（含 `active = false`）无重复 head / active-object count；
- Document delete vs new ref concurrency；
- Resource delete vs new ref concurrency；
- fixture Community dry-run 可重复且 manifest digest稳定；
- manifest 任一 final field / signature 被替换都 fail closed；
- Resource operator source 与其他 object source-preservation tests；
- evidence / scheduler 与 drain / freeze race tests；
- missing Assignment / Runtime ack、旧 ACP protocol或离线 supervisor不能 freeze，失败后
  不会自动解除 frozen；
- membership / moderation / archive security action原子生效，pre/post-cutover
  invalidation和resolution可审计；
- direct-v3 prepare / initialize rollback与exact governor mapping；
- cutover失败原子 rollback；
- post-commit fault只能 frozen repair / reproject后 resume；
- Context set始终 empty且 nonempty raw command拒绝；
- NIP-11 在 schema 2 时仍只广告 v2，在 flag-off schema 3 fixture也不广告 Context。

建议 PR：

1. v3 domain / SDK；
2. normalized DB / cross-domain constraints / source topology；
3. Relay v3；
4. canonical manifest / approval / dry-run tooling；
5. durable maintenance / moderation / repair fence + race tests；
6. greenfield v3 prepare / initialize。

### 阶段 5：dual clients、Resource Guide 与 v3 cutover

目标：让 Resource 以“资产坐标 + Guide”正式取代 legacy locator authority。

开发内容：

- CLI v2/v3 dual reader / writer；
- Tauri / TypeScript v3；
- Desktop Resource form / Guide picker / saga；
- `buzz resources guide`；
- ACP v3 snapshot / Role Continuity reader；
- base `RoleBriefV3`、`ResolvedRoleBrief::V2 | V3` 与 strict versioned surface；Context
  保持 `not_advertised_empty`、`document_metadata:not_required`；
- ACP base prompt加入
  `project-view get → resources guide --content-only`完整链，并把 `get-object resource`
  作为可选 metadata检查；
- ACP maintenance-aware turn admission / full-lifecycle cancel / durable Assignment +
  Runtime ack、maintenance-first startup与AgentPool active / idle child reap；
- fleet readiness 与 maintenance-ack probe；
- 逐 Community reviewed legacy Resource export / Guide publish；
- reviewer detached approval；
- Runtime maintenance / preflight / disable / exact manifest + signature recheck / cutover /
  verify / enable / explicit resume；
- 选择一个 empty-state Community验证 direct-v3 initialize；
- 观测旧客户端 unsupported；
- 保留 migration archive。

验收：

- 每个 active Resource 都有 active Guide；
- 每个 converted Resource object revision +1，并由可验证 reviewer signature 归因；
- legacy locator只存在于 reviewed Guide 和受限 pre-cutover v2 history / backup，不是 v3
  authority；
- 未知 resource kind完整工作；
- Resource → Guide 读取闭环；
- 所有 managed runtime 在 v3 下继续 strict base `RoleBriefV3`；
- cutover 前 exact maintenance epoch 的全部 Assignment / Runtime baseline已有 durable
  ack，恢复后不复用旧 runtime fence；
- 没有 v2/v3 dual write；
- `project_context_enabled` 仍为 false，NIP-11不广告 Context，nonempty write拒绝；
- 只允许单个或小规模声明过的 canary cohort；即使稳定也不在本阶段广泛扩张。

建议 PR：

1. dual CLI / ACP；
2. dual Tauri / Desktop Resource；
3. cutover readiness / canary；
4. bounded canary cohort。

### 阶段 6：Context Reference 与 Role Brief / Agent 闭环

目标：让 Resource / Document 成为真正可沿项目工作坐标发现的 Context。

开发内容：

- Project View Context add / remove / list CLI；
- Desktop Context chips / picker；
- Role Context；
- 填充阶段 5 已存在的 `RoleBriefV3.context`有界一跳 closure；
- Document meta稳定窗口 / cache key；
- 独立 optional Document enrichment，把 `document_metadata` 从 `not_required`切换为
  `verified / unavailable`；
- Pinned只交付 coordinate，不后台读取含正文 revision event；
- metadata unavailable降级；
- fetch commands；
- truncation / observability；
- CLI、Desktop、ACP closure与round-trip gates全绿后，原子启用
  `project_context_enabled`并广告 `buzz-project-context-v1`；
- Agent scenario E2E：由显式 task驱动
  Role → Work → Resource → Guide → permission / approval → external action；
- 不因 Context或Guide出现而自动触发 external action。

验收：

- Work / Role 可稳定引用 Resource / Live / Pinned Document；
- pinned 在 Document delete 后仍可读；
- Guide / Live 删除保护；
- Brief 无正文；
- 每个纳入 Resource 的 Guide coordinate不被 supplementary truncation丢弃；
- Document edit 不推进 PV revision但会刷新 metadata；
- Agent 按需 CLI 读取 Guide；
- 旧 Runtime /旧 Assignment不能借 Context Reference 提权；
- capability缺失时 nonempty Context不可写，capability出现后 CLI / Desktop /
  RoleBrief都能立即观察同一 canonical set；
- 仍只在声明过的小规模 canary cohort运行，不在阶段 7 gate前 broad rollout。

建议 PR：

1. CLI / Desktop Context UX；
2. SDK Role Brief closure；
3. ACP cache / prompt；
4. Agent E2E / bounded canary。

### 阶段 7：hardening、运维与真实使用观察

目标：在扩大使用前解决容量、恢复和长期运维。

开发内容：

- body / revision growth dashboard；
- lock wait / snapshot restart dashboard；
- inactive generation staged reproject；
- signer rotation runbook；
- backup / restore演练；
- projection parity repair；
- large history pagination压测；
- rate limit / abuse tests；
- Secret incident演练、通知 / forensic治理与未来 scrub requirements；
- 文档 retention / compliance 需求调研；
- 评估 shared + per-document lock；
- 收集 Guide 中重复步骤的 Adapter 候选。

验收：

- signer change可以在 capability disabled 状态完成全量重投影并恢复；
- million-scale revision synthetic test有有界 memory / query；
- restore 后 canonical / projection parity；
- lock 优化若实施，有正式 race proof / test；
- 没有因“支持 Resource”而暗中引入 Secret Store / installer；
- Adapter proposal有真实 usage evidence，而不是预先扩协议；
- Stage 5/6 canary的错误率、maintenance drill、restore、security与capacity gates全部通过
  后，才允许 broad Community rollout；新 Community此后才可显式默认选择 v3。

本阶段之后才讨论：

- Repository adapter；
- MCP / Skill / Plugin 安装器；
- Resource health；
- 全文 / semantic search；
- 细粒度 ACL；
- external document sync；
- CRDT / realtime co-edit。

## 27. 阶段依赖与可并行工作

阶段 2 exit后，阶段 3 Desktop Documents和阶段 4 v3 backend可以并行；阶段 5同时等待
两者完成。其余可并行关系：

| 工作流 | 依赖 |
|---|---|
| pure domain | 阶段 0 wire / invariants |
| SDK builders / parser | 阶段 0 golden fixtures |
| DB migration | 阶段 0 row shape / limits |
| CLI UX | SDK verified types / response contract |
| Tauri native | SDK + Relay query contract |
| Desktop UI | native mock contract |
| v3 domain | Document identity / ref semantics已固定 |
| migration export | legacy Resource facts + v3 Resource shape + canonical digest contract |
| runtime maintenance | current supervision binding / lease / Community lock semantics |
| base RoleBriefV3 | v3 verified snapshot；Context / Document enrichment可为空 |
| Context closure | base RoleBriefV3 + Document meta |

不可并行越过的 gates：

```text
没有 strict SDK parser → 不做 Tauri / ACP raw event接入
没有 private read gate → 不 enable Document capability
没有 managed fence → 不允许 managed Agent write
没有 dual ACP reader + strict base RoleBriefV3 → 不切 v3 Community
没有 reviewer-signed Guide mapping → 不切含 legacy Resource 的 Community
没有 durable Assignment + Runtime baseline / ack / frozen fence → 不执行 v3 cutover
没有 cross-domain lock / constraints → 不开放 Context Reference
没有 dual Context clients + RoleBrief closure → 不 enable/广告 Context capability
没有阶段 7 rollout gate → 不做 broad Community rollout
```

## 28. 总体验收标准

全部阶段完成时，系统必须证明：

1. Project Document 是独立 Community capability，不是 Project View 第十种 object。
2. Document A 更新不会因为 Document B / Project View revision变化而 conflict。
3. 每个 active revision 是完整不可变 Markdown snapshot。
4. update / delete 使用 exact expected document revision。
5. current、pinned 和 tombstone 读取语义明确且可复现。
6. Document ID 删除后不复用，普通 delete不是擦除。
7. non-member、channel-restricted credential 和 wildcard query不能读取任何 Document
   protocol event。
8. managed Agent 旧 Assignment /旧 Runtime不能写 Document。
9. list / Project View / Role Brief不携 Markdown 正文。
10. Resource v3 使用开放 `resource_kind` 和 mandatory Guide，不再以 locator 为权威。
11. 所有 Guide / Context target同 Project且符合 active / pinned规则。
12. Guide / Live ref 阻止 Document delete，Pinned不阻止。
13. incoming Context ref 阻止 Resource delete。
14. 删除 target不级联；删除 source显式清除 outgoing rows。
15. 更新 Guide body 不推进 Resource object revision。
16. 更换 Guide推进 Resource object / project revision。
17. v1/v2 closed clients不误读 /误写 v3。
18. v3 cutover没有长期 dual write；cutover commit 后只允许 disable + forward-fix，不做
    history rewind 或 lossy自动降级。
19. Role Brief只沿有界一跳 Context，metadata cache包含 Document meta。
20. Agent 只能显式读取和行动；读取 Guide没有执行副作用。
21. Resource / Document 不提供 Secret字段，migration不自动发布未审查 locator。
22. command、canonical state、receipt、revision / head / meta projection原子提交。
23. signer / projection generation失配时 capability fail closed。
24. Desktop community switch、live invalidation和conflict不泄漏 /不丢用户内容。
25. 每个开发阶段有 focused tests、exit criteria和可关闭 capability。
26. timed-out current member仍可读但不能写；banned actor / owner不能读写。
27. non-tombstoned Role（含 `active = false`）只有一个 RoleDefinition head，v3 meta继续
    绑定 membership snapshot和完整 continuity counts。
28. reviewed Resource cutover 对每个 Resource 增加 object revision；final v3 payload /
    Guide / base digest 或 pointer 漂移、review signature 无效、reviewer 不再 eligible
    都会中止。
29. Pinned Context只交付坐标，不在 Role Brief后台读取含正文 revision event，也不借用
    current metadata。
30. Document enrichment故障不隐藏健康的 Project View v3 capability，也不触发 managed
    Runtime suspend。
31. Resource cutover 的 operator execution provenance 与 Human business attribution
    分离且都可审计，operator 不能替换 reviewer。
32. v3 cutover 只在 exact maintenance epoch 已 durable drain + freeze 后执行；失败不会
    自动恢复 turn，resume 必须显式发生且不复用旧 Runtime fence。
33. 每个 managed Assignment都提交 compatible exact-epoch quiesced ack；只有有 runtime
    的 baseline再提交 runtime ack，idle / offline状态不能被当作已确认。
34. Context capability缺失时 server拒绝 nonempty set；出现时 CLI、Desktop与
    RoleBrief已经能 round-trip同一 canonical Context，不存在不可见可写状态。
35. base `RoleBriefV3` 在首次 v3 cutover前已经部署；阶段 6只填充既有 Context /
    Document metadata字段，不临时改变 wire major。
36. cutover支持 after-commit replay-first；post-commit fault只通过 frozen、
    exact-epoch、audited repair / reproject前进。
37. empty-state Community可显式 direct initialize v3；任何已有 v1 history不走该捷径。
38. Stage 5/6只运行 bounded canary，Stage 7 gate前不 broad rollout。

## 29. 明确延期的设计

本文不留下阻止首版实现的产品选择。以下事项已经明确延期，而不是实现者自行补全：

- Document folder / hierarchy；
- attachment / image asset；
- 通用 Document comment / suggestion / approval workflow；
- CRDT / OT；
- server-side patch；
- 全文 / semantic search；
- Document / Resource ACL；
- privacy scrub；
- external document sync；
- Mobile / Web Project View v3 + Document客户端；
- Resource dependency graph；
- Resource health；
- 自动 clone / install / connect；
- SkillHub / Plugin Registry；
- Secret Store；
- 通用 Resource Resolver；
- 自动执行 Guide；
- Checkpoint / Handoff 直接 Document reference；
- 在线无停机 signer rotation；
- 已有 v1 history 的 v1 → v3 direct cutover。

任何一项都需要新概念 /协议设计，不能作为“顺手优化”进入上述阶段。

## 30. 最终实现模型

```text
Project / Community
│
├── Project View v3
│   ├── Profile / Goal / Role / Plan / Stage
│   ├── Requirement / Issue / Work
│   ├── Resource
│   │   ├── name / open resource_kind / summary
│   │   └── guide_document_id ────────────────┐
│   └── Context Reference                     │
│       ├── Resource ID                       │
│       └── Document ID [+ pinned revision] ──┤
│                                             │
└── Project Document v1                       │
    ├── stable document_id ◄──────────────────┘
    ├── per-document revision CAS
    ├── current lightweight head
    ├── immutable full active revisions
    └── bodyless tombstone revision

Role / Work
    ↓ one-hop verified Context coordinates
Role Brief
    ↓ metadata + fetch command, no body
buzz documents get / buzz resources guide
    ↓ explicit on-demand read
Agent understands Guide
    ↓ existing tools + permissions + approvals
External repository / MCP / Skill / Plugin / server / database
```

实现顺序的核心不是先做一个更大的“资源管理平台”，而是先把项目内容的 identity、
revision、trust 和按需交付做对。Document v1 提供可继承的内容坐标；Project View v3
提供 Resource Guide 和结构化 Context Reference；CLI、Desktop 与 ACP 再把这些坐标交给
Human 与 Agent。

Buzz 因此知道“Project 拥有什么、说明在哪里、当前是哪一版、当时引用哪一版”，但仍不
冒充外部资源的事实源、权限系统或执行器。这是首版可扩展性与实现边界同时成立的关键。
