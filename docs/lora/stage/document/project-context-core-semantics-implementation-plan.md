# Project Context 最小核心语义实现计划

> 状态：已完成
> 日期：2026-08-02
> 目标分支：`feat/project-view-v0`
> 决议来源：[role-brief-context-provenance-gap.md](role-brief-context-provenance-gap.md#13-2026-08-02-初版上下文治理决议)

## 1. 目标

在不改变 Project View、Project Document、Role Brief v3 或权限协议的前提下，为所有 Buzz
managed Agent 增加一段稳定、简短且 platform-owned 的 Project Context 核心语义，使
Agent 从 session 开始就知道：

1. 系统支持 Project Document，Document 是版本化长文本资产和可直接引用的项目坐标；
2. Resource 是 Project View 资产坐标，使用方式由 mandatory Guide Document 说明；
3. Project View 对象可以关联 Resource 或直接关联 Document；
4. 工作实质改变 Project View、Resource、Document 或 Context Reference 后，需要显式写回。

本次优化只改善 Agent 的稳定认知与行为提醒。动态项目事实仍由 verified Role Brief / Role
Binding 交付，正文仍由 CLI 按需读取，最终授权仍由 Buzz 工具和 Relay 强制执行。

## 2. 已确认原则

### 2.1 初版保持轻量

初版不把 source reachability、Context provenance、Document stewardship、完整 CLI 手册或
所有协议例外写入 system prompt。当前模型可以结合 Role Brief 中的坐标、fetch command、
`buzz --help` 和按需 Project View 读取完成判断。

### 2.2 固定语义与动态事实分离

稳定 system contract 只定义概念和行为规则，不包含：

- 当前 Project / Community 名称或 ID；
- 当前 Document / Resource ID、标题、summary 或 revision；
- 当前 Context Reference set；
- 当前 Role、Assignment、Member、Runtime 或 Role Directory；
- project-authored Markdown；
- 当前 Relay capability 开关状态。

### 2.3 Capability-neutral

固定文案使用 `Buzz supports ...`，说明平台能力，不声称当前 Community 已经启用该能力。
实际可用性继续由 NIP-11、verified Role Brief、CLI 和 Relay 响应判断，不把 capability 状态
变成 system contract 的动态输入。

### 2.4 只提醒实质变化写回

固定文案使用 `materially changes`。单纯读取、临时猜测、尚未验证的讨论或模型内部分析
不应触发 Project 写入；真正改变规范状态、资产说明或关联关系时才显式写回。

## 3. 当前实现基线

### 3.1 Project Space contract

`crates/buzz-acp/src/project_space.rs` 当前提供独立的 `[Project Space]` 稳定契约，版本为
`2`。它不接收 Project、Community、Member、Role 或 revision 输入，符合本次新增语义的
所有权边界。

其 `contract_id()` 同时绑定显式版本与完整内容 hash。文案或版本变化都会让旧 session
contract ID 失配。

### 3.2 Session 生命周期

`crates/buzz-acp/src/pool.rs` 在每个完整 turn 选择 Full / Incremental Role Context 之前检查
Project Space contract ID。发现旧 contract 时会：

1. 使该 channel 的旧 ACP session 失效；
2. 创建携带新 system contract 的 session；
3. 强制重新生成 Full Role Brief，而不是沿用旧 compact Role Binding。

因此本次不需要新增 session migration、数据库状态或人工 cache 清理。

### 3.3 Base prompt

`crates/buzz-acp/src/base_prompt.md` 已经包含：

- `buzz documents` command group；
- metadata-first、正文按需读取规则；
- `buzz resources guide <resource-id> --content-only`；
- Resource Guide discovery chain；
- revision conflict exit 5；
- Document 不是 Secret Store；
- Guide / Document Markdown 不授予执行权限。

这些具体工具和安全说明继续留在 `[Base]`。本次不把它们全部复制到 `[Project Space]`。

### 3.4 Role Brief v3

Role Brief v3 已经以 body-free 方式输出相关 Resource / Live Document / Pinned Document
坐标、可选 verified metadata 和 fetch command。该实现不需要为本次核心语义调整 DTO、
closure、cache key 或 renderer。

## 4. 最终 system contract 增量

在现有 `[Project Space]` 中，放在 Project View / Role 基本概念之后、Full Role Brief 说明
之前，增加以下独立段落：

```text
Buzz supports versioned Project Documents for durable long-form project
knowledge. Documents are first-class project assets and may be referenced
directly from Project View. Resources are Project View asset coordinates with
a Guide Document explaining how the resource is used. When a Resource is
relevant, read its Guide; when a Document is relevant, read only the needed
body on demand. Project View objects may associate relevant Resources and
Documents through Context References. Chat, local files, and model memory do
not update the Project automatically. When your work materially changes
Project View state, Resource information or Guide linkage, Document content,
or Context References, explicitly write the change back through Buzz.
```

实现时允许进行不改变语义的英语润色，但不得加入当前 Community 动态事实，也不得扩张为
完整命令手册。

## 5. 代码改动范围

### 5.1 必须修改

#### `crates/buzz-acp/src/project_space.rs`

- 把 `PROJECT_SPACE_CONTRACT_VERSION` 从 `2` 提升到 `3`；
- 在 `PROJECT_SPACE_SECTION` 加入第4节确认的最小段落；
- 扩展稳定语义测试；
- 更新 contract version/hash change测试中的对照版本。

#### `crates/buzz-acp/src/pool.rs` tests

- 复用或扩展现有 system prompt framing tests；
- 证明 modern system prompt 中包含新增核心语义；
- 证明 `base_prompt = None` 时 `[Project Space]` 和新语义仍存在；
- 证明旧 contract ID 仍会触发 session invalidation和Full Brief refresh。

#### `crates/buzz-acp/src/lib.rs` prompt tests

- 保留现有 Document / Resource Guide discoverability断言；
- 只在确有必要时增加直接 Document / Resource定位的最小断言；
- 不复制 Project Space 全文 snapshot。

### 5.2 评估后才修改

#### `crates/buzz-acp/src/base_prompt.md`

现有 Base 已覆盖具体读取命令和安全边界。只有在实现检查发现 Agent仍无法从
`[Project Space] + [Base]` 得到以下操作闭环时，才增加一句最小补充：

```text
Project View may reference a Resource or a Project Document directly.
```

不得在 Base 中重复整段 Project Space contract。

#### `docs/lora/stage/document/changelog.md`

实现和验收完成后新增交付记录；计划阶段不提前声称已经交付。

### 5.3 不修改

本次明确不修改：

- `buzz-project-view` wire、Context Reference shape 或 event kind；
- `buzz-project-document` wire、revision / tombstone模型；
- Relay、数据库、migration、NIP-11 capability；
- `RoleBriefV3` DTO、Context closure、provenance或budget；
- CLI command surface；
- Desktop、Mobile或Web；
- Community、Role或Document权限；
- 自动读取、自动执行或自动写回行为。

## 6. 实施步骤

### 步骤 1：固定回归测试

先扩展 `project_space.rs` 单元测试，要求 contract 包含以下语义锚点：

- `versioned Project Documents`；
- `first-class project assets`；
- `referenced directly from Project View`；
- `Guide Document`；
- `read only the needed body on demand`；
- `Context References`；
- `materially changes`；
- `explicitly write the change back through Buzz`。

同时保留“无动态字段、无模板插槽”的现有负向断言。

### 步骤 2：更新稳定契约

- 将确认文本加入 `PROJECT_SPACE_SECTION`；
- contract version提升到 `3`；
- 保持 `[Base] → [Project Space] → [System]` 的现有顺序；
- 不把 Document / Resource动态 metadata 加入 system prompt。

### 步骤 3：验证 modern / legacy delivery

- protocol-v2 Agent继续通过`session/new.systemPrompt`获得契约；
- Goose继续走其已验证的system-prompt扩展或legacy fallback；
- legacy Agent继续通过带明确section label的user-context获得等价契约；
- `--no-base-prompt`不移除`[Project Space]`。

本步骤只增加或调整测试，不新建第二条prompt组装路径。

### 步骤 4：检查 Base discoverability

以最终组合prompt为准检查以下场景：

1. Agent知道Document可以直接被Project View关联；
2. Agent知道Resource使用方式通过Guide读取；
3. Agent能从Base得到实际CLI命令；
4. Agent知道变化需要显式写回；
5. Agent不会被要求自动读取全部正文。

只有第1项仍明显缺失时才对`base_prompt.md`增加一句补充。

### 步骤 5：更新交付记录

实现与测试通过后：

- 将本计划状态改为`已完成`；
- 在`changelog.md`记录核心语义、contract version和验证结果；
- 保持provenance补强为延期观察，不把它写成已实现。

## 7. 测试计划

### 7.1 定向测试

```bash
. ./bin/activate-hermit
cargo test -p buzz-acp --lib project_space
cargo test -p buzz-acp --lib shared_base_prompt_teaches_document_and_resource_guide_reads
cargo test -p buzz-acp --lib legacy_agent_gets_project_space_contract_without_base_prompt
```

若测试名称在实现中调整，应按实际模块过滤器运行等价测试。

### 7.2 Buzz ACP 回归

```bash
. ./bin/activate-hermit
cargo test -p buzz-acp --lib
cargo clippy -p buzz-acp --all-targets -- -D warnings
cargo fmt --all -- --check
```

### 7.3 Git / 文档检查

```bash
. ./bin/activate-hermit
git diff --check
```

本次没有 Desktop、Relay、DB 或协议改动，因此不要求 Desktop E2E、PostgreSQL integration或
Project View migration canary。若实现超出第5节范围，则必须重新评估测试矩阵。

## 8. 验收标准

### 8.1 稳定认知

- 新 modern ACP session 的 system prompt 包含四条最小语义；
- Agent能区分直接Document Reference与Resource → Guide；
- Agent知道只按需读取相关正文；
- Agent知道实质变化需要显式写回。

### 8.2 所有权边界

- system contract不包含当前Project / Document / Resource动态内容；
- project-authored Markdown仍只存在于按需读取路径；
- Role Brief仍是动态、revision-bound、body-free交付；
- prompt不承担授权，Relay和Runtime fence行为不变。

### 8.3 兼容与刷新

- `--no-base-prompt`时新增核心语义仍存在；
- legacy Agent获得等价、明确标记的兼容上下文；
- contract version变化使旧session失效；
- replacement session获得新的Full Role Brief而不是只收到compact Binding；
-普通Project revision变化不会仅因本次实现而重建system contract。

### 8.4 不扩张范围

- 没有新增或修改Nostr kind、migration、HTTP endpoint或capability；
- 没有修改RoleBriefV3 serialized surface；
- 没有实现provenance、scope、ACL、自动读取或自动写回；
- Base与Project Space没有出现大段重复说明。

## 9. 发布与回滚

### 9.1 生效方式

该变更位于`buzz-acp`。需要重新构建并重启实际承载managed Agent的harness；若本地Desktop
使用打包后的harness，则需要重新构建对应Desktop bundle。

新进程在下一完整turn检查contract ID并重建旧session。Relay和数据库不需要迁移或因本次
变更单独重启。

### 9.2 回滚

回滚代码会恢复上一版contract内容和版本。由于contract ID再次变化，下一完整turn会再次
使session失效并重建。回滚不涉及Project、Document或Context数据转换。

## 10. 风险与控制

| 风险 | 控制 |
|---|---|
| 固定prompt变长 | 只加入一个短段落，不复制CLI手册 |
| 对未启用Community虚构能力 | 使用`Buzz supports`，实际可用性由capability决定 |
| Agent频繁写回 | 使用`materially changes`，不要求写入猜测或纯读取 |
| 项目文本被提升为system | contract不包含任何project-authored内容 |
| 旧session继续使用旧规则 | version + content hash触发现有session invalidation |
| Base与Project Space漂移 | Core只定义语义，Base只提供具体命令和安全细节 |

## 11. 构建产物清理

测试和交付完成后，按仓库本地约定：

1. 枚举workspace与Tauri `target`下实际存在的`incremental`目录；
2. 只删除确认后的Cargo incremental缓存；
3. 保留`deps`、最终二进制、Desktop构建结果和Docker数据；
4. 记录本次释放空间。

## 12. 完成定义

同时满足以下条件后，本计划才可标记为完成：

1. 四条核心语义进入platform-owned Project Space contract；
2. contract version提升并有session invalidation测试；
3. modern、legacy和`--no-base-prompt`路径通过；
4. Buzz ACP全量单测、Clippy、fmt和`git diff --check`通过；
5. 没有扩大到RoleBrief、协议、数据库、Desktop或权限模型；
6. changelog记录实际交付；
7. Cargo incremental缓存已按约定清理。

## 13. 实施结果

2026-08-02按本计划完成交付：

- `PROJECT_SPACE_CONTRACT_VERSION`已从`2`提升到`3`；
- 四条核心语义已进入platform-owned`[Project Space]`；
- modern、legacy和`--no-base-prompt`等价交付路径已由测试固定；
- 现有contract ID失配、session invalidation和Full Role Brief refresh机制直接复用；
- Base prompt现有Document / Resource Guide读取闭环足够，因此未增加重复文案；
- Role Brief v3 Context provenance、协议、数据库、CLI与客户端均未修改。

验证结果：`buzz-acp`全量641项单元测试通过，Clippy以`-D warnings`通过，Rust fmt与
`git diff --check`通过。测试批次使用`CARGO_INCREMENTAL=0`，交付前再次清理workspace与Tauri
目标目录中的Cargo incremental缓存。
