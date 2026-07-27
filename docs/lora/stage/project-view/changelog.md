# Project View 变更记录

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
