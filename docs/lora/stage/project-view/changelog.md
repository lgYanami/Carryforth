# Project View 变更记录

## 2026-07-27 — Slice 5：CI、可观测性与发布

### 专用质量门

- 新增 `project-view-test-unit`、`project-view-test-db`、
  `project-view-test-e2e`、`project-view-test` 与 `test-migrations` Just
  recipes。`test-unit` 和无 nextest 的 `scripts/run-tests.sh unit` 都显式覆盖领域
  crate、kind registry、SDK、Relay adapter 与 CLI；领域 crate 的 property、关系和
  wire integration targets 不会被 `--lib` 误过滤。
- 新增隔离 PostgreSQL 脚本。DB 测试各自创建/删除 scratch database；migration gate
  使用另一个精确命名的临时数据库，执行 fresh、0024→0025、ledgerless schema、
  populated upgrade、并发 migrator，并用 `pgschema plan` 阻断 migration 25 与
  `schema/schema.sql` 的 Project View 漂移。
- Project View E2E 现在启动独立数据库和 Relay，使用 packaged
  `buzz-admin project-view enable` 开启中心 DB flag，并由真实 `buzz` 子进程完成一次
  typed create。测试继续覆盖 NIP-11 signer/capability、WS/HTTP、COUNT、revision-pinned
  pagination、冲突、projection 签名、mixed query 隐藏和 live membership 撤权。
- 新增真实 pre-feature rollback smoke：CI 固定从 `ab3af828` 构建 Project View
  出现前的 Relay，在已由当前 `buzz-admin` 迁移到 25、全部开关为 false 的数据库上以
  `BUZZ_AUTO_MIGRATE=false` 启动，验证 readiness 与既有 NIP-11 路径。该测试不会用
  当前 Relay 模拟旧版本。
- 新增 post-mutation compatible rollback smoke：先由当前 Relay 接受初始化，再用同一
  migration 25 数据库启动固定 `8ef125c1`（Slice 4）Relay，验证 capability、稳定
  signer、revision 1 完整快照和 Project View 专属非成员拒绝。由此分别覆盖首次
  mutation 前的 pre-feature 回滚与已有数据后的兼容回滚边界。

### CI 与制品

- backend nextest archive 加入 `buzz-db`、`e2e_project_view`、`buzz` 和
  `buzz-admin`；独立 Project View integration job 顺序执行 DB transaction、
  migration/schema drift 与真实 Relay/CLI E2E。rollback job 使用固定 pre-feature
  与 compatible 源码 binary cache，避免用当前 binary 模拟旧版本，也避免把 ignored
  migration tests 留在非必经路径。
- Docker PR path filter 新增 `migrations/**` 与 `schema/**`。Relay release
  `LOG_PATHS` 新增 Project View crate、CLI/admin、schema、协议/运维文档、Chart、
  Compose 和专用测试脚本。
- Sprig archive 新增实际 `buzz -> sprig` symlink 与 manifest entry；Sprig workflow
  增加 CLI/SDK/Project View 相关 PR 构建路径，使 managed Agent 拿到与 Relay
  capability 对应的 typed CLI。
- 新增 package/deploy contract，静态验证 Relay image 含 `buzz-admin`、CI archive
  含真实 CLI/admin/E2E、完整 metrics 名称、Chart 使用稳定 signer，并禁止 Chart 或
  Compose 引入 Pod-local `BUZZ_PROJECT_VIEW_ENABLED`。

### 可观测性与运维

- 新增八组低基数指标：mutation count/duration/conflict、snapshot
  duration/revision retry、按闭集 type 的 active object gauge、projection dispatch
  error 与 schema readiness。operation、result、type、reason 都来自闭集，不使用
  Community/object/event ID 作为 label。
- mutation 结果日志包含 `community_host`、command/actor 坐标、operation、
  object type/id、expected/committed revision 与 result code；正文、patch、title 和
  Resource locator 不进入普通日志。
- 新增 `docs/project-view-operations.md`，固化 server-first 顺序：全部 Pod 先以
  auto-migrate false 升级，再运行 migration 25，验证 schema/signer/read gate 后由
  admin 开启。Runbook 同时记录 disable、诊断 SQL、告警信号、signer rotation、
  首次 mutation 前与之后两种不同的回滚边界；Chart/Compose 文档与 Helm NOTES
  指向同一流程。

### 验证

- `just project-view-test-unit`：59 项 Project View 定向测试通过；其中领域层 36 项，
  关系设计 21 条清单全部自动测试。
- `just project-view-test-db`：14 项隔离 PostgreSQL 测试通过。
- `just test-migrations`：6 项 migration 测试与 Project View schema drift gate
  通过。
- `just project-view-test-e2e`：真实 Relay、admin 与 CLI E2E 通过。
- 固定 `ab3af828` pre-feature Relay + migration 25 rollback smoke，以及当前 Relay
  写入后切换到固定 `8ef125c1` compatible Relay 的 post-mutation smoke 均通过。
- 实际构建 Sprig archive，确认包含 `buzz -> sprig` 和 manifest entry。
- `just test` 13 组 unit/integration tests 全部通过；仓库级 `just ci` 全部通过。
- 受影响 Rust packages 的 `cargo check --all-targets`、workflow YAML 解析、
  shell syntax、release contract 与 `git diff --check` 通过。

## 2026-07-27 — Slice 4：Typed SDK 与 Agent CLI

### SDK command 与 projection 契约

- `buzz-sdk::project_view` 新增 `build_initialize`、`build_create`、`build_update`、
  `build_delete`。Builder 只接受领域层 typed input，不接收 project/community ID，
  并固定生成 `44300` 的精确 `-`、`t` tags。
- 领域层新增不依赖当前服务端状态的 submission validation：在签名前检查 revision safe
  range、UUID v4、初始 Goal 基数与去重、字符串/list/locator 限制、required-field
  `null`、空 patch、Issue 自引用和 Work target 类型；Relay 仍在归约时重复校验当前状态、
  CAS、对象存在性和关系目标。
- 新增 `parse_meta_projection`、`parse_object_projection` 与
  `verify_projection`。读取端验证事件签名、NIP-11 Relay signer、kind、精确 tag 顺序、
  规范 UUID/hex/RFC3339/decimal、content/tag/coordinate 一致性、revision/generation
  范围，以及 reset/source 和 active/tombstone 互斥；未知 projection 可选 content
  字段保持向前兼容，tombstone 明确拒绝业务正文。

### `buzz project-view` Agent 操作面

- 新增 `get`、`get-object`、`init`、`create`、`update`、`delete` 六个子命令。
  Profile、Goal、create data 与 update patch 均从 JSON file/stdin 进入 typed
  deserialization；Create 的对象 UUID 在签名前由 CLI 生成，调用者不能覆盖
  `id`/`object_type`。
- 所有命令先读取 NIP-11 并要求 `buzz-project-view-v1` 与规范 `self` signer。Project
  View mutation 使用 closed-tag 精确签名；NIP-OA 仍通过独立 `x-auth-tag` header
  传递，不污染 mutation tags。网络重试复用同一 signed event bytes。
- `get` 先验证 meta，再按 `(generation, revision)` 使用 `/query` 扩展分页读取 active
  heads，检查排序、唯一 ID、数量、generation/revision 和全部 projection，最后重读
  同一 meta 后才交给 `ProjectView::assemble`。并发变化触发有界重试，无法取得一致快照
  时返回 conflict，不输出混合 revision。
- `get-object` 使用规范 `d` coordinate、`limit:2` 和 Relay signer 做 point read，
  同时支持 active object 与 tombstone。写命令保留调用者显式
  `expected_project_revision`，成功后回读 meta/object 确认；HTTP `409` 映射
  `CliError::Conflict` 与 exit code `5`。
- 默认 `get` JSON 一次返回 project、Goals/Plans/Stages、未规划 Requirements/Issues、
  Roles、Resources 和 Issue reverse references；未初始化状态明确输出
  `initialized:false`、revision `0` 与空集合。全局 `--format compact` 保留同一逻辑
  结构，但移除每个对象的 provenance/revision 冗余字段。

### 验证

- SDK 协议测试覆盖四类 mutation builder、projection round-trip、错误 signer、未知可选
  字段和 tombstone 正文拒绝。
- CLI HTTP integration test 使用真实 `BuzzClient` 与本地 Axum bridge，覆盖
  meta → revision-pinned page → meta 的一致快照组装，以及 revision conflict 的进程
  exit `5`；命令 inventory 与 typed input/null semantics 同步受单测保护。
- `cargo clippy -p buzz-project-view -p buzz-sdk -p buzz-cli --all-targets -- -D warnings`
  通过；`cargo test -p buzz-cli commands::project_view --lib` 9 项测试全部通过；
  仓库级 `just ci` 全部通过。

## 2026-07-27 — Slice 3：Relay 原生协议接入

### 写入、签名与原子提交

- 将 kind `44300` 接入 WebSocket `EVENT` 与 HTTP `POST /events` 的统一 ingest
  管线。命令必须具备协议规定的精确 tags，并依次通过全局凭证、`MessagesWrite`
  scope、当前 Community 成员身份与 ban 检查。
- Relay 在回执前完成 mutation 解析、领域归约、projection 规划与稳定密钥签名，再由
  `ProjectViewWriteTx` 原子提交 command、receipt、规范状态和全部新 head；幂等重放
  返回既有 receipt，不重复分配 revision 或 fan-out。
- 新增 SDK projection builder，严格构造 object/meta projection 的 kinds、坐标、tags
  与 content；数据库边界再次验证覆盖集合、事件签名、稳定 signer、revision 和
  generation，避免把内部自洽但不对应命令的投影写入规范状态。
- Project View command 只进入 command audit，不触发通用 workflow；Relay 生成的
  projection 不重复进入 audit/workflow。kind `40903`、`40904`、`44300` 使用显式的
  有界 metrics 标签。

### 统一读取、分页与实时撤权

- WS `REQ`/`COUNT` 与 HTTP `/query`/`count` 共用严格 reader gate：允许当前 Relay
  member，或 owner 仍是当前 member 的 persisted managed agent；actor ban 总是拒绝，
  owner ban 在 managed-agent 路径拒绝，且全部查询以 Community 为租户边界。
- 未授权的 Project View-only filter 明确拒绝；mixed filter 在 SQL `LIMIT`/COUNT 和
  普通查询前排除 Project View kinds，防止分页与数量侧信道；NIP-50 的现有正向索引
  allowlist 不包含这些 kinds，并在返回端继续 fail closed。授权后的返回结果仍执行末端
  防御检查。
- HTTP `/query` 支持 revision/generation 固定的 active snapshot 分页。首屏返回规范
  游标，续页必须携带相同 revision、generation 与 canonical cursor；并发 mutation
  后的旧游标返回 `409 Conflict`，不会拼接跨 revision 页面。
- 本机与 Redis 跨 pod 的 live fan-out 都在实际发送 chokepoint 重新查询当前授权。
  reader 被移出 Community 或被 ban 后，无需重连即可停止接收后续 projection。

### Capability 与 signer rotation

- NIP-11 仅在 Community 显式开启、数据库 schema 完整、稳定 signer 已配置且规范状态
  可读取时声明 `buzz-project-view-v1`；deployment readiness 只在至少一个 Community
  开启 Project View 时要求全局前置条件。
- `buzz-admin project-view enable` 改为 checked enable，在持锁状态下验证 schema、
  signer 与完整性；`disable` 保持无需私钥。
- 新增 `project-view reproject --community|--all --expected-pubkey
  [--relay-key-file]`。operator 轮换顺序固定为先 disable，再运行只允许 disabled 状态的
  重签，验证后显式执行 checked enable；重签只递增 projection generation，不改变
  project revision，并原子退休全部旧 head。
- 私钥不接受 argv 传入；key file 必须是普通文件，Unix 下拒绝 group/world 权限。
  `--all` 在写入前先验证全部目标均 disabled，避免只完成部分 Community。

### 验证

- `cargo test -p buzz-sdk -p buzz-project-view`：SDK 237 个测试、领域层 36 个测试通过。
- Relay Project View handler/filter 测试与 admin 测试通过；两个真实 PostgreSQL
  integration test 通过，覆盖成员/managed-agent reader gate、ban、tombstone 重签、
  generation CAS、旧 signer 拒绝和新 signer checked enable。
- 新增真实 Relay 协议 E2E，并在本地 Postgres/Redis 上显式运行通过：覆盖 WS/HTTP
  写入与读取、projection 签名、NIP-11 capability、COUNT、revision-pinned pagination、
  stale mutation/page `409`、mixed historical 隐藏，以及 membership 撤销后的 live
  fail-closed。

### 范围边界

- 本阶段完成 Relay、数据库、SDK projection 与 operator rotation 闭环；面向人和 Agent
  的 typed `buzz project-view` CLI 读写命令、客户端 read-model 组装及契约化输出属于
  Slice 4。

## 2026-07-27 — Slice 2：数据库规范状态与原子写事务

### Migration 与规范状态

- 新增 additive migration `0025_project_view.sql`，并同步
  `schema/schema.sql`；已有和新建 Community 的
  `project_view_enabled` 均默认 `false`，迁移不回填 Project View 对象，也不改写
  `events` 大表。
- 新增 `project_view_state`、`project_view_objects` 和
  `project_view_mutations`。全部领域表、主键、唯一约束、外键和索引都以
  `community_id` 为租户前缀，没有加入 operator-global allowlist，也没有
  `ON DELETE CASCADE`。
- 对 revision safe range、schema version、32-byte ID/pubkey、Profile identity、对象类型、
  relation shape、active body 与 tombstone 空正文增加数据库 CHECK；对象行禁止 hard
  delete 和 tombstone 复活。
- ordinary trigger 只按 active insert / active→tombstone 对
  `active_object_count` 做机械 `+1/-1`。deferred trigger 使用主键和关系索引验证最终
  Profile/Goal 聚合、变化对象的 typed active target，以及 tombstone 的 active 入向引用；
  mutation 提交不运行全表 `COUNT(*)`。
- migration 固定数量更新为 `25`，同时增加 tenant-key 静态断言、0024→0025 实际升级测试，
  以及不含 `_sqlx_migrations` ledger 的 `schema/schema.sql` 从零建库测试。

### 原子写路径

- 新增 `buzz-db::project_view::ProjectViewWriteTx`。写事务先取得按 Community 派生的
  exclusive advisory lock，再从 writer DB 读取中心开关；未开启或已归档 Community
  fail closed。
- 状态行使用 `FOR UPDATE`，project revision 使用 CAS；canonical time 来自数据库，并以
  `max(clock_timestamp(), previous + 1µs)` 保证随 revision 单调。
- `load_current()` 的纯领域基线保存在事务内部。提交前 DB 层重新运行同一 typed mutation，
  要求得到的 next state 和 changed entries 与 prepared bundle 完全一致，避免调用方把
  “内部自洽但不对应签名命令”的状态写入数据库。
- accepted command event、幂等 receipt、state、canonical object/tombstone、object
  projection、meta projection 和旧 head 退休全部进入同一个 SQL transaction；任一步
  失败即整体回滚。
- event store 抽取 caller-owned transaction helper；普通 `insert_event()` 继续走同一
  字段与校验实现。Project View head 只按 state/object 保存的精确 event ID 和预期 kind
  soft-retire，不复用 NIP-33 的作者/时间戳 replacement。
- 重试先命中 `(community_id, event_id)` durable receipt，不分配新 revision，也不重复
  fan-out；同 expected revision 的并发写在 Community lock/CAS 下恰好一个成功。

### Operator 控制面

- 新增 `buzz-admin project-view status [--community <host>]`，展示中心开关、归档状态、
  revision、projection generation 和 signer pubkey。
- 新增 `enable|disable --community <host>|--all`。单 Community 和全量操作复用 mutation
  advisory lock；`--all` 按 UUID 稳定顺序取锁，只更新非归档 Community，且不在 argv
  接受任何私钥。

### 验证

- `cargo test -p buzz-db`：87 个无基础设施测试通过，132 个基础设施测试保持 ignored。
- 8 个 Project View 临时数据库 integration test 已显式执行并通过，覆盖初始化/幂等、
  projection 失败全回滚、prepared bundle 推导校验、并发 CAS、tenant key 与跨
  Community 引用、tombstone/count/head retirement、最后 Goal deferred guard 和中心开关。
- 0024→0025 升级与 ledger-less `schema/schema.sql` 建库测试均在独立临时数据库通过；
  临时数据库已清理。
- `cargo test -p buzz-project-view`：36 个领域、关系、wire 与属性测试通过。
- `cargo test -p buzz-admin` 和
  `cargo clippy -p buzz-db -p buzz-admin --all-targets -- -D warnings` 通过。
- `just test`：core、auth、db、conformance、project-view、push-gateway 和 workspace
  integration 共 9 个测试组通过。

### 范围边界

- kind `44300` 仍未接入 Relay ingest；成员写安全门、统一读门禁、实际 projection
  builder/signing、NIP-11 capability、post-commit fan-out 和 reproject 属于 Slice 3。
- 当前写事务用一次 set-based object query 重建完整纯领域状态，因此没有 N+1，并能在
  DB 边界复核全状态；设计中的 mutation-targeted loader 和 10k 规模性能门在 Relay
  接入前继续收敛。

## 2026-07-27 — Slice 1B：协议与 indexed d-tag 基础

### 协议

- 新增 `docs/nips/NIP-PV.md`，冻结 mutation、object projection 和 meta projection
  的 kind、签名者、精确 tags、content、revision/generation、读取与实时一致性语义。
- 固定 `44300` 为成员签名 append-only mutation，`40903`/`40904` 为 Relay 签名的
  object/meta current-state projection。
- 实现前复核 NIP-01 和官方 `registry-of-kinds`；截至本次交付，三个值均未发生外部登记
  冲突。
- 明确 signer rotation 产生的 `reset: true` meta 没有成员 command source，因此省略
  source `e` tag 和 `source_event_id`；普通 `reset: false` meta 两者仍为必填且必须一致。

### 实现

- 在 `buzz-core::kind` 注册三个 kind 并纳入重复值检查；`40903`/`40904` 同时进入
  Relay-only classifier，客户端在专用 handler 尚未实现时也不能写入 projection。
- 新增共享 `has_indexed_d_tag()`：保留 `30000..=39999` 的既有行为，并只额外识别
  `40903`/`40904`。没有扩大 `is_parameterized_replaceable()`，因此 Project View
  projection 不会误走 NIP-33 的作者/时间戳替换规则。
- `buzz-db::event::extract_d_tag()` 改用共享 classifier；Project View object/meta 坐标会
  写入 `events.d_tag`，mutation 和其他普通 40xxx kind 仍保持 `NULL`。
- WS REQ、HTTP `/query` 与 WS/HTTP COUNT 共用的 filter builder 改用同一 classifier。
  只有显式、非空且全部可索引的 kinds 才会在 SQL `LIMIT` 前下推单值或多值 `#d`；
  mixed-kind 与 kindless filter 继续安全回退。

### 范围边界

- `44300` 仍是 Relay ingest 的 unknown/rejected kind；本阶段没有接入 mutation handler、
  scope 或 capability。
- 没有新增 migration、Project View 状态表、projection transaction 或 NIP-11 宣告。
- 新增一个需 Postgres 的 ignored 回归测试，覆盖“同 kind 新行超过 limit、目标 head
  较旧”时 point read 仍能由 SQL `d_tag` 谓词精确命中；默认测试集继续忽略该用例，
  本次交付已在迁移后的真实 Postgres 上显式执行。

### 验证

- `cargo test -p buzz-core`：231 个单元测试与 2 个文档测试通过。
- `cargo test -p buzz-db`：84 个无基础设施测试通过；Postgres point-read 回归测试另行
  显式执行并通过。
- `cargo test -p buzz-relay handlers::req::tests`：47 个 REQ/filter 测试通过。
- `cargo clippy -p buzz-core -p buzz-db -p buzz-relay --all-targets -- -D warnings`
  通过。
- `just test`：core、auth、db、conformance、project-view、push-gateway、数据库及
  workspace integration 共 9 个测试组通过。

## 2026-07-27 — Slice 1A：纯领域模型

### 实现

- 新增零 I/O crate `buzz-project-view`，并接入 workspace 与 infra-free unit test gate。
- 实现九类 Project View 对象、闭集状态枚举、强类型关系、统一 active object 信封与
  tombstone。
- 实现 schema v1 的 Initialize、Create、typed Update、Delete mutation；所有写入使用
  project revision CAS，失败时内存状态保持不变。
- 实现字段、UUID、revision、关系目标、删除保护和最终聚合状态校验。
- 实现完整确定性 read model，包括 Unbound Plan 子树、Unplanned
  Requirement/Issue 下的 Work，以及按目标分组的 Issue 反向引用。
- 新增 mutation JSON 大小、嵌套深度和封闭 schema 解析入口。

### 首版解释

- 延续后端实现设计：Stage 不承载业务顺序；无显式顺序时仅按
  `(created_at, id)` 稳定输出。
- 延续后端实现设计：Work 的 `handles` 可以显式替换，但不能清空。
- 延续后端实现设计：Delete 生成 tombstone，ID 永久不可复用；删除前仍须先解除全部
  active 入向引用。
- 必填字符串以 `trim()` 结果判断是否为空，但规范值保留调用方原始文本，不隐式改写。
- 在 Nostr address/event 与 Buzz deep-link 的更细语法尚未由协议文档固定前，领域层只
  校验它们非空、长度与控制字符；URL 另外拒绝 userinfo 和密码。

### 验证

- 关系设计中的 21 项清单使用一一对应的具名契约测试。
- mutation wire、patch 三态、字段边界、ID/tombstone 与 safe revision 使用示例和边界
  测试。
- 属性测试覆盖合法 mutation 序列不变量、非法 mutation 原子性、旧 revision 重放、
  snapshot 输入排列无关、Issue `about` 环和确定性重放。

### 尚未包含

- `docs/nips/NIP-PV.md`、event kinds 与 indexed `d` tag classifier。
- PostgreSQL、Relay、projection、SDK、CLI 与发布接入。
