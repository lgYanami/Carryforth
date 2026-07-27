# Project View 变更记录

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
