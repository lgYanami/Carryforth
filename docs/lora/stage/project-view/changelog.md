# Project View 变更记录

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
