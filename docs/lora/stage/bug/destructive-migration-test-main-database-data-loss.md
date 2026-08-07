# 迁移测试误清空本地主数据库事故与防复发修复

> 状态：根因已确认；破坏性 fresh-schema 测试已删除；剩余迁移测试已增加双层数据库隔离保护
>
> 记录日期：2026-08-06
>
> 范围：`buzz-db` migration tests、本地 `DATABASE_URL`、Local Dev 消息、Project View、
> Project Document、Role 与 Meeting 持久化数据

## 1. 结论

2026-08-06 本地 Local Dev 的消息、Channel、Project View、Document、Role 和 Meeting 数据再次
消失，不是 Desktop 缓存、Relay 未启动、`cargo clean`、Docker volume 删除或 Meeting 迁移
`0047` 导致的。

直接原因是执行了
`run_migrations_applies_consolidated_initial_schema_on_fresh_database`。该测试原本用于空数据库，
但测试连接逻辑在没有 `BUZZ_TEST_DATABASE_URL` 时会回退到普通 `DATABASE_URL`；本地
`DATABASE_URL` 指向持久化开发主库 `buzz`。测试随后执行：

```sql
DROP SCHEMA IF EXISTS public CASCADE;
CREATE SCHEMA IF NOT EXISTS public;
```

这会删除 `public` 下全部业务表及数据，再从 migration 1 开始建立一个空 schema。Docker volume
仍然存在，但 volume 中原来的逻辑数据已经被数据库测试删除。

本次决定不恢复旧快照，优先移除事故测试并封死所有同类误连主库路径。

## 2. 用户可见影响

事故后出现以下现象：

- Inbox 与 Channel 消息历史消失；
- Project View 页面显示 `Project View is not supported by this Relay`；
- Project View、Document、Context、Role Assignment 与 Meeting canonical 数据为空；
- Desktop 本地保存的 managed Agent 配置仍可能存在，造成“Agent 还在，但 Community 数据没了”
  的表面不一致。

重新启动时，`seed-local-community.sh` 只会幂等插入 `localhost`、`localhost:3000`、
`127.0.0.1` 和 `127.0.0.1:3000` Community。新 Community 使用数据库默认值：

```text
project_view_enabled = false
project_view_schema_version = 1
project_document_enabled = false
project_context_enabled = false
```

因此 Relay 正常运行但不会在 NIP-11 中广告 `buzz-project-view-v3`，Desktop 随即显示“不支持”。

## 3. 证据与时间线

数据库中留下了仅由事故测试创建的 Community：

```text
project-view-default-<uuid>.test
```

同时，`_sqlx_migrations` 的 migration 1～47 全部具有同一轮重新安装时间。已确认时间线为：

```text
2026-08-06 17:33:30 CST  destructive migration test 重建 public schema
2026-08-06 18:30:18 CST  dev-start 重新插入 loopback Community
2026-08-06 18:31:10 CST  Relay 启动并连接到 postgres://.../buzz
```

因此数据在本次重新构建和启动之前已经消失。

本地仍存在以下只读快照，但用户已确认本次不恢复：

- `target/dev-lifecycle/buzz-before-migration-reconcile-20260804-211955.dump`；
- `target/dev-lifecycle/buzz-before-meeting-merge-20260805.dump`。

后者包含 1027 条事件、9 个 Channel、1 份 Project View v3 state、25 个 Project View 对象、
13 份 Document、24 个 Document revision 和 6 个 Role Assignment，但不覆盖快照之后产生的数据。

## 4. 根因

### 4.1 破坏性测试复用了普通开发数据库变量

原测试连接优先级为：

```text
BUZZ_TEST_DATABASE_URL
    -> DATABASE_URL
    -> postgres://.../buzz
```

`DATABASE_URL` 是 Relay 和本地 Desktop 开发环境连接持久化主库的正常配置，不能作为破坏性
测试的 fallback。默认 URL 本身也直接指向 `buzz`，使“未配置测试库”从安全失败变成删除主库。

### 4.2 `#[ignore]` 不是安全边界

事故测试带有 `#[ignore = "requires Postgres"]`，只表示普通 `cargo test` 默认不执行；一旦开发者
为了验证 migration 显式传入 `--ignored`，测试仍会使用当前进程环境。`ignore` 不能证明目标库是
临时库，也不能阻止 `DROP SCHEMA`。

### 4.3 删除单一测试不足以覆盖同类路径

同一文件中还有 migration upgrade 测试需要重建 schema。这些测试有保留价值，但如果仍允许
连接主库，未来执行其他 ignored test 仍可能复现事故。因此修复同时收紧所有共用连接和 reset
入口。

## 5. 已落地修复

### 5.1 删除事故测试

删除：

```text
run_migrations_applies_consolidated_initial_schema_on_fresh_database
```

不再保留一个可以直接清空所连接数据库、再验证 fresh install 的单测入口。migration 的空库和
升级验证继续由创建独立 scratch database 的脚本及其他隔离测试承担。

### 5.2 禁止回退到 `DATABASE_URL`

剩余 Postgres migration tests 必须显式设置：

```text
BUZZ_TEST_DATABASE_URL=postgres://.../buzz_<unique-test-name>
```

未设置时测试立即失败。代码不再读取普通 `DATABASE_URL`，也不再提供指向 `buzz` 的默认值。

### 5.3 URL 侧数据库名保护

测试连接前从 `BUZZ_TEST_DATABASE_URL` 提取数据库名，只接受以 `buzz_` 开头的 disposable
database。以下名称必须拒绝：

- `buzz`；
- `postgres`；
- `production`；
- 任何没有 `buzz_` 前缀的数据库。

当前正式测试脚本创建的 `buzz_meeting_contract_*`、`buzz_pv_migrations_*` 等独立数据库满足该
规则。

### 5.4 执行 DROP 前回读实际数据库名

`reset_public_schema()` 在执行任何 DDL 前调用 `SELECT current_database()`，再次确认实际连接的
数据库以 `buzz_` 开头。该检查不依赖 URL 文本，防止连接重定向、helper 回归或未来调用者绕过
第一层校验。

核心不变量为：

```text
No explicit BUZZ_TEST_DATABASE_URL
    -> no database connection

Actual database name is not buzz_*
    -> no DROP SCHEMA
```

## 6. 非目标与后续建议

本次不执行：

- 恢复或合并旧数据库快照；
- 自动重新初始化 Project View v3；
- 修改当前 Local Dev 的空数据状态；
- 把 `cargo clean` 与数据库生命周期绑定。

建议后续再补充独立的测试数据库管理封装，使所有需要数据库的 destructive tests 都采用：

```text
创建唯一 scratch database
    -> 在 scratch database 中迁移与测试
    -> 测试结束删除 scratch database
```

同时可在重新构建脚本开始前增加轻量数据库快照选项，但快照只能降低事故损失，不能替代测试库
隔离和 fail-closed 检查。
