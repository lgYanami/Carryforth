# Stage 6 Context / Role Brief 本地真实 canary

本文固定阶段6的可重复本地真实验收。它运行真实PostgreSQL、Redis、Relay、CLI、Admin、ACP supervisor与
Agent child，不以mock代替服务端事务、签名、权限、Runtime fence、NIP-11或跨域删除保护。

本canary只操作脚本创建的scratch数据库与本地随机端口，不部署环境、不处理真实Community数据，也不扩大
canary cohort。它从一个空的schema-v3 Community开始，自行创建全部Document、Resource、Role、Proposal、
Assignment与supervisor binding；不调用Stage 5、legacy migration canary或任何Project View v1/v2 runtime。
第一方图形客户端只验收Desktop；Mobile与Web不在阶段6范围。

## 1. 入口与前置条件

从仓库根目录运行：

```bash
./scripts/test-project-view-stage6-canary.sh
```

脚本会自行创建独立scratch数据库并构造exact schema-v3前置状态。需要：

- 已激活或可由脚本激活的Hermit工具链；
- Docker可用，且本地PostgreSQL与Redis容器可启动；
- `cargo`、`curl`、`docker`、`jq`、`node`、`rg`和`sha256sum`；
- 本地端口可用。

默认会重建`buzz-relay`、`buzz`、`buzz-admin`与`buzz-acp`。只有调用者已确认这些二进制来自当前源码时，
重复诊断才可显式使用：

```bash
PROJECT_VIEW_STAGE6_NO_BUILD=1 ./scripts/test-project-view-stage6-canary.sh
```

这只是本地重跑优化，不能用于首次交付验收或源码变化后的验收。

## 2. 固定验收路径

### 2.1 独立schema-v3 greenfield前置状态

脚本创建带显式owner与managed Agent identity的独立scratch数据库，固定执行：

```text
operator prepare-v3
  → Human owner签名init-v3
  → checked Project View enable
  → Project Document bootstrap / verify / enable
  → 创建Guide Document与schema-v3 Resource
  → 创建member Role
  → owner发Offer、Agent接受，形成active Assignment
  → operator为Assignment注册独立supervisor binding
```

任何步骤若出现Project View旧版本广告、旧普通命令或legacy canary输出依赖，均属于验收入口本身失效，而不是
可接受的兼容路径。

### 2.2 Context control plane

脚本验证：

- 初始`project_context_enabled = false`且NIP-11不广告`buzz-project-context-v1`；
- disabled状态的nonempty Context add被拒绝；
- Human admin以idempotency key原子enable；
- 相同请求replay返回同一durable receipt，不追加operation或audit；
- ready后NIP-11同时广告Project View v3、Project Document v1与Context v1。

### 2.3 managed Agent supervision与canonical Context

脚本启动真实`buzz-acp`和fixture ACP child，从child进程环境读取current Runtime fence，并以managed Agent身份：

1. 用`project-runtime status`证明独立supervisor binding与第一代current lease；
2. 为当前Role加入一个Resource；
3. 停止ACP并启动下一Runtime generation；
4. 证明第一代lease已retire、第二代lease是唯一current且available；
5. 以active Assignment加入同一Document的Live与Pinned revision 1坐标；
6. 列出并核对exact canonical三项集合；
7. Assignment结束后证明binding/lease均被撤销。

Runtime supervision只治理进程lease、恢复与观测，不作为Context业务ACL。Context便利命令每次都读取verified
v3 snapshot并按Community/Role governance提交完整replacement；不做自动rebase。脚本在Assignment结束前预签
一个以结束后revision为基线的mutation，结束后以有效NIP-98重新提交，必须得到Assignment conflict，而不是
依赖Runtime fence拒绝。

### 2.4 Role Brief与显式正文读取

Role Brief必须满足：

- `project_view_schema_version = 3`、member处于`assigned`；
- Context availability为`ready`；
- Document metadata source为`verified`；
- Resource与mandatory Guide coordinate成对出现；
- Live Document含current title / summary / revision；
- Pinned只含document ID、pinned revision与fetch command；
- synthetic正文marker不出现在Role Brief或ACP日志。

Guide正文只通过显式Agent-facing命令读取：

```bash
buzz resources guide <resource-id> --content-only
```

Context或Guide本身不授予权限，也不自动触发clone、安装、配置修改、Secret请求、代码执行或其他external
action。

### 2.5 Document metadata独立刷新

脚本把Live Document从revision 1更新到2，并断言：

- Project View revision完全不变；
- 下一次Role Brief读取新的Document meta A / heads / B窗口；
- Live title与revision刷新到2；
- Pinned仍固定revision 1；
- 新旧正文marker均未注入Brief。

### 2.6 disable、保留与恢复

Context disable后：

- NIP-11立即撤销Context advertisement；
- Role Brief为`unavailable_preserved`，自动注入列表为空；
- `context list`仍显示三项verified canonical coordinate；
- add / retarget拒绝；
- 移除Live这一canonical subset成功。

重新enable必须重验Project View、Document、signer、normalized parity与preserved target；下一次Role Brief重新
hydrate，只显示Resource与Pinned坐标，不复用disable前缓存。

### 2.7 删除矩阵与最终清理

脚本验证：

- Live ref存在时Document delete返回`still_referenced`；
- 移除Live后，即使Pinned仍存在，ordinary Document delete成功；
- 删除后仍能按exact revision 1读取immutable正文；
- Resource primary Guide delete返回`still_referenced`；
- 被Role Context引用的Resource delete返回`object_still_referenced`；
- 移除Resource与Pinned后，Role Context set为空；
- enable、disable、re-enable恰好形成3条operation与3条hash-chain audit，enable replay不重复计数。

## 3. 证据与清理

每次运行写入：

```text
test-results/stage6-canary/<UTC-run-id>/
```

`acceptance-summary.json`只保存坐标、revision、Runtime / Assignment ID和状态结论；正文仅存在于专门的显式
fetch证据文件，不进入日志或summary。`artifact-digests.sha256`覆盖该次运行的全部其他证据。

验证digest时必须从证据目录执行，因为manifest使用相对路径：

```bash
cd test-results/stage6-canary/<UTC-run-id>
sha256sum -c artifact-digests.sha256
```

无论成功或失败，退出钩子都会停止本次Relay / ACP进程、删除scratch数据库与临时目录，并清理
`../../../target`、`../../../desktop/src-tauri/target`下所有`incremental`目录。默认保留普通二进制与依赖产物供后续质量门
复用。数据库名必须匹配`buzz_pv_stage6_canary_<pid>_<random>`安全前缀，脚本才允许执行DROP。

## 4. 验收证据状态

历史Stage 5链式fixture的证据不再作为当前入口的有效通过证明。schema-v3 greenfield重构后，交付阶段先以
`bash -n`、v3静态门禁与release contract确认入口封闭；下一次获准运行真实canary时，必须生成新的
`acceptance-summary.json`与digest，并且该summary的`project_view.schema_version`必须为3、
`fixture_origin`必须为`greenfield_v3`。
