# Meeting V2 阶段五真实 Agent Qualification 报告

> 结论：PASS
>
> 完成日期：2026-08-03（Asia/Shanghai）
>
> 范围：Meeting V2 后端；不包含 Desktop、Web、Mobile。

## 1. 运行身份

本报告记录阶段五发布候选的一次真实 provider 运行。原始证据保留在本机私有临时目录，不
提交 Board、Speech、Prompt、模型原始输出、私钥或 ACP ledger。

| 项目 | 结果 |
|---|---|
| Run ID | `meeting-v2-20260802T180006Z-2160937` |
| UTC 时间 | `2026-08-02T18:00:06Z` 至 `2026-08-02T18:03:54Z` |
| 本地证据目录 | `/tmp/buzz-meeting-v2-qualification-runs/meeting-v2-20260802T180006Z-2160937` |
| Buzz commit | `435383d6913e841c273a3a9ae1fe27395468cf5f` |
| source status SHA-256（前/后） | `6f0b545a1ca20dcbdf56fe362f68a4b2e4b2df753691d0962b89b7eb5f59d0d8` |
| source diff SHA-256（前/后） | `c30ae239b00bb1bffa1afd87be27b4574d30094d5c6e50ac00691363803ece44` |
| 协议 | `v=3 + moderated-board-v1` |
| ACP adapter | `@agentclientprotocol/codex-acp 1.1.7` |
| Provider | 已认证真实 Codex，会话前完成模型目录检查 |
| 模型 | `gpt-5.6-sol[max]`（主持）、`gpt-5.6-sol[high]`（参会） |
| 实际 Agent Session | 8 |

证据 verifier 独立校验 `sha256.txt`、workspace 前后 status/diff 哈希、可执行文件、adapter
包身份、模型目录、ACP/Relay capability、精确的 8 Agent 进程与日志拓扑，不把 manifest
自声明或日志关键词单独当作 PASS 依据。主持和参会进程的模型及 reasoning 配置也与实时模型
目录逐项匹配。

## 2. 场景结果

runner 在 Create 开启时创建四场独立会议，再重启同一 Relay 并关闭 Create。关闭后
`buzz-meeting-v2-create` 消失，但 `buzz-meeting-v2` runtime capability 保留，四场存量会议
继续运行至终态。

| 场景 | Roster | 关键覆盖 | 终态 |
|---|---|---|---|
| mixed | 2 Human + 2 Agent；Agent 主持 | 3 次 Board 更新、4 名不同 speaker、1 次 Human Board 抢占、1 次 Handoff、1 次主持 self Speech、2 次 Floor Decision | `closed` |
| all-agent | 3 Agent；Agent 主持 | 3 次 Board 更新、3 名不同 speaker、1 次 Handoff、1 次主持 self Speech、2 次 Floor Decision | `closed` |
| moderator abort | 2 Agent；Agent 主持 | 主持模型主动判断无法形成有效结论 | `aborted / unable_to_form_conclusion` |
| admin/security abort | 1 Human + 1 Agent；Agent 主持 | 参会身份被安全撤权 | `aborted / participant_revoked` |

mixed 与 all-agent 均分别证明 Intent 和随后 Granted Speech 各自读取权威 current Board；两场
都观察到两次读取之间 Board 已变化，并与数据库计算一致。observer 共记录 656 条经过
allowlist 的事件，其中包括 36 次 Board load、9 次 Board Turn、5 次 Floor Turn、5 次
Speech 与 115 次权威 State 观测。

## 3. 安全与不变量

以下安全探针全部通过：

- 非参会者读取 current Board 被拒绝；
- 非参会者写 Board 被拒绝；
- Relay 关闭 Create 后新建 V2 被拒绝；
- Meeting End 后继续写入被拒绝；
- Agent 运行前后 workspace 快照一致；
- Project View 依赖数与外部写入数均为零。

以下十项硬不变量均为零：

- Board/Floor provider Turn 重叠；
- Board 未 terminal 就启动 Floor；
- Offer/Grant active 时接受 Board command；
- 未读 current Board 就启动语义 Turn；
- timeout/preemption 后迟到 Board 落地；
- Board action 改变 canonical speech revision；
- End 后 revision 继续变化；
- 结束时仍有 runtime reservation；
- 未授权 Board 访问成功；
- 外部系统写入。

运行异常为零。observer 包不含 `content`、Prompt、raw output 或 raw error 字段。

## 4. Qualification gates

独立 verifier 的十八项 gate 全部 PASS：

```text
manifest_identity
real_provider
capability_preflight
artifact_integrity
workspace_and_external_effects
scenario_and_roster_topology
observer_topology_and_privacy
participant_current_board_refresh
mixed_roster
mixed_lifecycle
all_agent_roster
all_agent_lifecycle
moderator_abort
admin_abort
security_probes
runtime_health
zero_invariants
v2_observer_evidence
```

## 5. 自动化门禁

阶段收口同时通过：

```bash
. ./bin/activate-hermit
just test-meeting-backend
RUST_TEST_THREADS=1 just ci
DATABASE_URL=<fresh-database-url> PGDATABASE=<fresh-database> just test
```

`RUST_TEST_THREADS=1` 用于规避桌面既有 `relay_admission` 测试在并行 test runtime 间竞争
全局串行锁而挂起；该组 1661 个 Tauri 测试串行运行通过，本次没有为此修改桌面代码。

第一次直接运行 `just test` 时，共享开发数据库报告 migration 32 的历史校验和来自另一代码
状态。没有修改或删除该数据库；在一次性全新数据库上重跑相同命令后全部十五组测试通过，
数据库随后删除。

## 6. 复现与复核

```bash
. ./bin/activate-hermit
cargo build --release -p buzz-cli -p buzz-admin -p buzz-relay
cargo build --release -p buzz-acp --features meeting-acceptance
./scripts/meeting-v2-live-qualification.sh
./scripts/verify-meeting-v2-qualification.sh <run-directory>
jq . <run-directory>/qualification-gates.json
```

runner 每次创建新的 DB、Redis、身份、Meeting 和不可覆盖 run 目录。真实模型输出可能令样本
失败；不得手工修补输出、伪造 `UNCHANGED`、替模型选择 speaker，或用确定性 wire trace
替代本报告所代表的真实 provider qualification。
