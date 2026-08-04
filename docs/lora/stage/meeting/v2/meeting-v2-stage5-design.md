# Meeting V2 阶段五：后端综合验收与发布设计

> 状态：已完成
>
> 设计日期：2026-08-02
>
> 完成日期：2026-08-03
>
> 范围：Relay、DB、CLI、ACP、后端测试、真实 Agent qualification 与运维文档；不包含前端。

## 1. 阶段目标

阶段五不再增加新的会议生命周期语义。它把阶段一至阶段四已经完成的协议与实现收敛为一份
可以判断“能否灰度”的后端发布候选：

```text
确定性协议证据
        +
真实 Relay / CLI / DB 证据
        +
真实 Agent provider 运行证据
        +
能力声明、指标与故障处置契约
        ↓
Meeting V2 后端发布候选
```

发布资格必须来自硬门禁和可复现证据，不来自一次人工观看、模型回答质量或“进程看起来仍在
运行”。

## 2. 不改变的产品边界

阶段五继续保持以下边界：

- Meeting V2 可以完全独立于 Project View、Work、Issue、Git 和 Workflow；
- 不增加前端、会议模板、会议类型、主持权转移、投票或 Board 历史；
- 不把 Board 更新变成广播、通知或 participant semantic trigger；
- 不允许模型直接发布协议事件或产生外部写入；
- 不用模型内容质量替代 Relay、DB 和 wire 的确定性验证；
- 不改变 creator = owner = moderator、固定 roster 和拉取式 current Board；
- 不改变 Board Maintenance 先于 Floor Decision 且各自拥有独立 deadline 的顺序。

## 3. 阶段五交付结构

阶段五按四个可独立复核的交付面推进。

### 3.1 发布级自动化门禁

现有 `scripts/run-meeting-backend-tests.sh` 是阶段四已经建立的基础，不另起一套重复入口。
阶段五将它提升为 V0/V1/V2 统一后端发布门禁，并明确包含：

- ACP 全量 Meeting 单元测试；
- Relay Meeting 合同测试；
- Postgres 状态机、恢复和并发合同；
- 真实 Relay + 真实 CLI 的 V0/V1/V2 E2E；
- Create gate 关闭后已有 V2 继续运行与结束；
- 当前读权限撤销、重连与历史读取约束；
- migration fresh、upgrade 和 schema drift；
- capability/readiness 与低基数观测合同；
- qualification 证据校验器自身的正反 fixture。

专项门禁不调用真实 LLM。这样它可以在普通 CI 中确定性重放；真实 provider 由独立的
qualification runner 负责。

### 3.2 真实 Agent qualification

qualification 至少产生两类样本：

1. `mixed`：两名 Human CLI 与至少两名 Agent，其中主持人为 Agent；
2. `all-agent`：一名主持 Agent 与至少两名参会 Agent。

样本共同覆盖：

- 主持 Agent 至少两次完成 Board Maintenance，并多轮选择 speaker；
- Intent 与 Grant 各自拉取当前 Board，且二者之间至少发生一次 Board 变化；
- Human Request 抢占一次 Board window，迟到结果不得落地；
- 至少一段 Directed Handoff 被回答或被主持人按协议收敛；
- 主持人作为普通参会者经过 self Intent → Offer/ACK → Grant → Speech；
- 最终 Board 包含结论后正常 `closed`；
- 独立样本覆盖主持 Agent 主动 `aborted`；
- 独立故障路径覆盖 admin/security abort；
- 全部运行不依赖 Project View，也不改变工作区或外部系统。

真实模型可能产生不合格建议；runner 应让该次样本失败，而不是修补模型输出、伪造
`UNCHANGED`、替模型选择 speaker 或把确定性 wire trace 冒充真实 provider 证据。

### 3.3 capability、readiness 与观测

Relay 与 ACP 分别声明自己的能力，避免用“某一个实例能跑”推断整个 fleet 已经收敛。

Relay NIP-11 扩展：

- `buzz-meeting-v2`：该 Relay 有稳定 signer，且 V2 schema/runtime 可读取、恢复和结束 V2；
- `buzz-meeting-v2-create`：在前一能力成立的基础上，本实例允许创建新 V2。

`buzz-meeting-v2-create` 可以随 rollout gate 消失，`buzz-meeting-v2` 不能因关闭 Create 而消失。
若数据库中存在 active V2，则 Relay readiness 必须要求稳定 signer 与完整 V2 runtime schema；
不能让不理解 active V2 的实例进入服务。

ACP 提供机器可读的静态 capability 输出，至少声明：

- V2 participant Intent 与 Granted Speech；
- V2 moderator Board Maintenance 与 Floor Decision；
- 每个语义 Turn 的 authoritative current-board read；
- 支持的 schema、policy 和 ledger generation。

静态 capability 证明二进制理解协议；provider 登录、模型目录和实际 Turn 成功仍由
qualification preflight 与运行证据证明。调度层只能把 V2 分配给同时满足静态能力和运行
健康条件的 ACP。

### 3.4 发布与故障处置

阶段五固化：

- 部署顺序与 fleet 收敛检查；
- Create 默认关闭及测试 Community 灰度；
- SLO、告警和最小运维查询；
- Board/Floor timeout、read failure、worker backlog、ACP 不可用的处置；
- 关闭新建、已有 Session 排空和前向兼容二进制回滚；
- 已知边界和明确不支持的降级方式。

## 4. qualification 证据契约

每次 run 使用全新隔离数据库、隔离 Redis 实例、身份和 Meeting，并生成不可覆盖的 run
目录。证据目录至少包含：

- `manifest.json`：run ID、场景、commit、模型、adapter、配置、结果和失败 gate；
- `roster.tsv`：只含 role、participant type 和公钥，不含私钥；
- `meetings.tsv`：场景与 Session 对应关系；
- `acceptance-events.ndjson`：经过 allowlist 的 ACP observer 事件；
- `protocol-invariants.json`：从权威 DB 计算的低基数计数；
- `metrics.prom`：运行结束时的 Relay 指标快照；
- `processes.tsv` 与每个 ACP/Relay 的进程日志：关联场景、角色、模型与实际 Runtime；
- `security-probes.json`：非参会者读写、Create 关闭与 End 后写入的拒绝结果；
- `preflight/`：Codex 登录、adapter 包身份、模型目录、ACP/Relay capability 与可执行文件哈希；
- `workspace-before.status` / `workspace-after.status` 与前后 diff SHA-256：在不保存源码 diff
  正文的前提下证明 Agent 没有修改工作区；
- `sha256.txt`：runner、二进制、配置和全部核心证据的哈希；

私钥只存在权限为 `0700` 的临时 secret 目录，cleanup 时删除。manifest、指标、普通日志和
聚合报告不得包含 Board、Intent、Speech 或 Prompt 正文，也不得把 Session ID、公钥、event
ID 用作 metric label。

qualification verifier 只相信相互可交叉检查的结构化字段和 DB/Relay 证据。它必须确认
全部不可变证据都被哈希，独立比较 workspace 快照，并把 roster、Meeting Session、Agent
日志中的实际 model、adapter/capability preflight、指标与 observer 场景关联起来；manifest
的自声明不能替代这些检查。日志关键词可以作为诊断附件，不能单独决定 PASS。

## 5. 指标冻结

### 5.1 Relay

新增或固化以下低基数指标：

- `meeting_v2_board_command_total{action,outcome,duplicate}`；
- `meeting_v2_board_command_latency_seconds{action,outcome}`；
- `meeting_v2_board_read_total{transport,outcome}`；
- `meeting_v2_board_read_latency_seconds{transport,outcome}`；
- `meeting_v2_end_total{outcome,reason_code,duplicate}`；
- 共享 Baton command、recovery、outbox 和 worker 指标带有低基数 `protocol` 维度，或提供
  等价的 V2 可区分指标。

固定枚举：

- `transport`：`http`、`websocket`；
- Board read `outcome`：`success`、`not_found`、`denied`、`error`；
- Board command `outcome`：`accepted`、`conflict`、`expired`、`error`；
- `duplicate`：`true`、`false`；
- End `outcome`：`closed`、`aborted`；
- abort `reason_code` 对已知协议值使用固定 allowlist，其他合法但未知值统一折叠为
  `other`，closed 使用 `none`；不得把原始 reason 直接作为 label。

### 5.2 ACP acceptance observer

V2 evidence allowlist 包含：

- Board load started/completed/failed/discarded；
- Board Turn queued/completed；
- Floor Turn queued/completed；
- host Turn discarded；
- 复用的 Intent、Grant、Speech、Offer、moderator attempt 与 State sync 事件。

observer 可以在私有 qualification 包中保存关联所需的 ID，但不得保存 Board 正文、Speech
正文、Prompt、模型 raw output 或原始错误正文。

### 5.3 必须为零的发布不变量

qualification 与自动化门禁必须证明以下计数为零：

- Board 与 Floor provider Turn 对同一 Session 同时 active；
- Board 未 terminal 就启动 Floor；
- Offer/Grant active 时 Board command 被接受；
- 未成功拉取 current Board 却 dispatch V2 语义 Turn；
- timeout/preemption 后的 Board 结果被持久化；
- Board action 产生 canonical speech、reply 或 speech revision；
- ended Session 的 State/Speech revision 继续变化；
- pending outbox、活动 Offer/Grant、运行中 moderator attempt 未收敛；
- 未授权 current-board read/write 成功；
- Agent 修改仓库、Project View 或其他外部状态。

## 6. SLO 与告警建议

初版 SLO 是发布/运维门槛，不改变协议 deadline：

- Relay command 可用性：非客户端拒绝的 5 分钟成功率 ≥ 99.9%；
- current-board read 可用性：授权读取 5 分钟成功率 ≥ 99.9%；
- outbox：P99 delivery latency < 5 秒，pending oldest age < 30 秒；
- recovery：due Session oldest lag < 30 秒，scan saturation 连续 3 个周期即告警；
- ACP：Board/Floor read failure 或 timeout 在 15 分钟窗口显著高于基线时告警；
- 所有硬不变量计数只要非零立即阻断扩灰。

真实 provider 延迟不与 Relay deadline 混为同一 SLO。provider 变慢可以导致一次 Board 或
Floor timeout，但不得破坏后续阶段预算、协议恢复或其他 Session。

## 7. 灰度与回滚契约

发布顺序固定为：

1. 保持 `BUZZ_MEETING_V2_CREATE_ENABLED=false`；
2. 迁移数据库并部署能恢复/结束 V2 的全部 Relay 与 worker；
3. 验证所有 Relay NIP-11 均有 `buzz-meeting-v2`；
4. 部署 ACP，并对每个候选二进制运行机器可读 capability probe；
5. 运行 V0/V1/V2 专项门禁与真实 Agent smoke；
6. 只在测试 Community 的完整 fleet 上开启 Create；
7. 验证 `buzz-meeting-v2-create`、指标和零不变量；
8. 再按 Community 扩大灰度。

关闭 Create 只禁止新建。已有 V2 的 Board、Floor、Agent、recovery 和 End 必须继续。安全
回滚目标必须仍然报告 `buzz-meeting-v2`，并能处理所有 active V2；完全不理解 V2 的旧二进制
不是回滚目标。禁止 down migration，也禁止把 V2 Session 改写成 V1。

## 8. 阶段五完成定义

阶段五完成需要同时满足：

1. 发布级 Meeting backend gate 在干净环境通过；
2. V0/V1 无回归，migration fresh/upgrade/drift 通过；
3. mixed、all-agent、moderator abort 和 admin/security abort 都有合格证据；
4. Relay/ACP capability 可以机器读取，Create 关闭不删除 runtime capability；
5. V2 Board read、Board command、End 和共享 runtime 可区分观测；
6. 发布、告警、故障处置、关闭新建和前向兼容回滚说明完整；
7. `just ci`、`just test` 和 V2 专项门禁通过；
8. 没有未解释的硬不变量、运行异常或已知后端发布阻断。

若真实 provider 环境不可用，可以完成代码、runner 和确定性门禁，但不能把阶段五标记为
“已完成”或生成 PASS 发布候选；必须明确保留 qualification 阻断项。

本次交付已使用真实 `codex-acp` 完成全部四类场景，十八项 qualification gate 全部通过，
且 `just ci`、`just test` 与 Meeting 专项后端 gate 均已通过。可复核摘要见
[Meeting V2 阶段五真实 Agent Qualification 报告](./meeting-v2-qualification-report.md)。
