# Meeting V1 真实 Agent 验收与规模压测方案

> 状态：真实 Codex 已执行；C6 clean pass，C10/C12 runtime stability fail；
> 正式签收未通过
>
> 日期：2026-07-29
>
> 范围：Meeting V1 后端、Buzz ACP、真实 Codex provider；不包含 Desktop、Web、Mobile

首次 C4/C6/C10/C12 qualification 结果见
[`meeting-v1-live-acceptance-report-2026-07-29.md`](meeting-v1-live-acceptance-report-2026-07-29.md)。

本文补充
[`meeting-v1.md`](meeting-v1.md)、
[`meeting-v1-backend-implementation-design.md`](meeting-v1-backend-implementation-design.md)
和
[`meeting-v1-backend-operations.md`](meeting-v1-backend-operations.md)
中未由确定性测试证明的部分：真实 LLM provider 的可用性、延迟、工具调查行为、发言质量，
以及更多真实 Agent 同时参会时的稳定性。

本文是一份验收和后续交付方案，不在本阶段直接改变 Meeting 的协议容量。

## 1. 验收目标

真实验收需要证明：

1. 每个参会 Agent 都通过 `buzz-acp -> codex-acp -> Codex` 完成实际模型调用，不使用
   fake ACP 或预录输出；
2. 主持 Agent 使用 `gpt-5.6-sol`、`max` reasoning effort，其他参会 Agent 使用
   `gpt-5.6-sol`、`high` reasoning effort；
3. Agent 能读取会议共享历史和项目上下文，形成 Intent，在获得 Grant 后使用工具调查并
   在期限内 SAY 或 YIELD；
4. 主持 Agent 能在真实模型延迟下持续处理 Intent、Handoff、Human priority 和
   moderator fallback；
5. Relay、PostgreSQL、Redis、outbox、ACP session、Codex provider 在多 Agent、多 Session
   压力下不会产生死锁、双重发言权、revision 缺口、消息丢失或历史分叉；
6. 会议发言与主持行为在相关性、事实依据、连贯性、去重和决策质量上达到可接受水平；
7. 6、10、12 个真实参会 Agent 的压力结果能够区分：
   - 跨多个会议的系统总并发能力；
   - 同一个会议内的共享上下文、Intent burst 和主持仲裁能力。

真实验收不能替代 `just test-meeting-backend`。前者提供 provider 和模型行为证据，后者提供
可重复的协议正确性证据，两者都通过才允许正式签收。

## 2. 当前容量边界

当前实现冻结了以下限制：

| 边界 | 当前值 | 含义 |
|---|---:|---|
| 单场完整参会者 | 12 | Human 和 Agent 的总和，包含创建者和主持人 |
| 单场受管 Agent | 4 | 所有权威类型为 `agent` 的参会者 |
| 单个 `buzz-acp` 实例的 ACP pool | 1–32 | 同一个 Buzz Agent 身份后的并发子进程，不是参会身份数 |

12 人与 4 Agent 当前都是 V0/V1 共用的代码常量；扩展 V1 时不能直接提高共用常量，否则会
同时改变 V0 契约。

主持人若是 Agent，也计入 4 个 Agent 上限。现有 `2 Human + 4 Agent` E2E 的主持人是
Human；确定性 ACP 测试覆盖了主持控制器，但还没有通过真实 provider 验证“Agent 主持人
与多个 Agent 参会者”的完整闭环。

这带来三个直接结论：

- 当前契约内的单场真实基线应为 `1 Moderator Agent + 3 Participant Agent + 2 Human`；
- 不修改协议时，可以做总计 6、10、12 个真实 Agent 的跨会议并发测试；
- 6、10、12 个 Agent 在同一个会议中都需要先提高 V1 的 Agent 上限。12 Agent 若还要保留
  2 Human，则总参会者上限也需要从 12 提高到至少 14。

`BUZZ_ACP_AGENTS=12` 不能绕过上述限制。12 个参会 Agent 必须有 12 套独立 Buzz 身份、
私钥和 `buzz-acp` 实例；每个实例通常使用 `BUZZ_ACP_AGENTS=1`。

## 3. Codex 验收配置

### 3.1 固定角色配置

| 角色 | 数量 | Model | Reasoning effort | ACP pool |
|---|---:|---|---|---:|
| Moderator Agent | 每场 1 个 | `gpt-5.6-sol` | `max` | 1 |
| Participant Agent | 其余 Agent | `gpt-5.6-sol` | `high` | 1 |
| Human Host/Operator | 每场 1 个 | 无 | 无 | 无 |
| Human Observer | 基线及最终验收 1 个 | 无 | 无 | 无 |

固定角色表适用于正式的人类介入验收。`12 Agent + 0 Human` 只是一项可选的纯 Agent
补充压力，其 Create 作者和 Channel Owner 必须改由一个 Agent 承担，并明确是否同时担任
Moderator。

官方 Codex 模型文档把 Sol 定位于复杂、开放且需要额外判断与打磨的工作，并说明更高的
reasoning effort 会增加延迟和 token 使用；`max` 适用于深度优先的单个困难任务。实际
可用模型和 effort 仍以验收环境运行时目录为准，而不是只依据文档或本地缓存。

参考：

- [Codex Models](https://learn.chatgpt.com/docs/models)
- [Codex Config basics](https://learn.chatgpt.com/docs/config-file/config-basic)
- [Codex Subagents and reasoning effort](https://learn.chatgpt.com/docs/agent-configuration/subagents)

基线测试通过 `features.multi_agent=false` 关闭 Codex 内部自动 subagent 编排，保证一个
Meeting Agent 身份对应一个 Codex ACP runtime。后续若允许 Agent 内部再启动 subagent，
应作为独立压力维度，不能混入本方案的 6/10/12 参会 Agent 计数。

### 3.2 启动模板

主持 Agent 使用：

```bash
CODEX_CONFIG='{"model_reasoning_effort":"max","features":{"multi_agent":false}}' \
BUZZ_ACP_MODEL='gpt-5.6-sol[max]' \
BUZZ_ACP_AGENT_COMMAND='codex-acp' \
BUZZ_ACP_AGENT_ARGS='' \
BUZZ_ACP_AGENTS='1' \
BUZZ_ACP_LAZY_POOL='false' \
BUZZ_ACP_PERMISSION_MODE='bypass-permissions' \
BUZZ_ACP_IDLE_TIMEOUT='620' \
BUZZ_ACP_MAX_TURN_DURATION='7200' \
BUZZ_PRIVATE_KEY='<moderator-private-key>' \
BUZZ_RELAY_URL='<relay-url>' \
./target/release/buzz-acp
```

普通参会 Agent 使用同一模板，把 `model_reasoning_effort` 改为 `high`，同时把
`BUZZ_ACP_MODEL` 改为 `gpt-5.6-sol[high]`。每个实例还必须使用独立私钥、ledger、
日志目录和进程标识。

`@agentclientprotocol/codex-acp` 1.1.7 的本轮运行时目录同时暴露基础 model ID 和带 effort
的 model ID。正式 Runner 使用带 effort 的 ID，并要求每个真实 Meeting Session 在任何
Agent speech 前记录 `applied model gpt-5.6-sol[max|high]`。仅在
`CODEX_CONFIG` 预设基础 model、再让 `BUZZ_ACP_MODEL` 请求基础 ID，可能使
`session/new` 的目录匹配失败并触发 Buzz 当前的 model fail-open；这种 run 即使原生 Codex
session 最终碰巧使用了正确 model/effort，也不计为正式验收。

`BUZZ_ACP_LAZY_POOL=false` 是本轮验收的固定条件，避免 5 秒 Agent Offer ACK 期限与
Codex 冷启动竞争。`bypass-permissions` 用于无交互的真实工具调查；因此验收必须在隔离、
可丢弃的工作区和测试凭证下执行。

### 3.3 Preflight

真实模型调用前必须完成：

```bash
. ./bin/activate-hermit
cargo build --release -p buzz-acp

codex --version
codex debug models \
  -c 'model="gpt-5.6-sol"' \
  -c 'model_reasoning_effort="max"' \
  -c 'features.multi_agent=false' >/dev/null

export MEETING_CODEX_ACP_VERSION='<approved-exact-1.x-version>'
npm install -g "@agentclientprotocol/codex-acp@$MEETING_CODEX_ACP_VERSION"
npm ls -g @agentclientprotocol/codex-acp --depth=0
codex-acp --version
codex login status
```

验收固定并记录 Codex CLI 和 `@agentclientprotocol/codex-acp` 的精确版本，不使用未记录的
`latest` 漂移。Runner 必须核对 npm package identity，拒绝旧
`@zed-industries/codex-acp` 和主版本低于 1 的
`@agentclientprotocol/codex-acp`，因为 Buzz 的 `CODEX_CONFIG -> thread/start config`
契约依赖 1.x adapter。

自动压力测试优先使用专门的 OpenAI API project 和受预算、速率限制的密钥；密钥只从
secret store 注入，不进入命令回显、日志或结果包。`codex login status` 只记录原生 Codex
的认证状态，不能证明 adapter 的认证和计费归属。正式 preflight 还必须通过准确版本的
`codex-acp`，使用与对应角色完全相同的 `CODEX_CONFIG` 完成一个最小
`session/new + session/prompt` provider probe，并记录脱敏后的 auth route 和 provider
project；没有真实 prompt 的 model catalog 查询不能替代该 probe。

Codex 目录必须同时包含目标 model、`high` 和 `max`：

```bash
codex debug models |
  jq -e '
    .models[]
    | select(.slug == "gpt-5.6-sol")
    | [.supported_reasoning_levels[].effort]
    | (index("high") != null and index("max") != null)
  '
```

ACP adapter 必须向 Buzz 暴露目标 model：

```bash
export MEETING_ACCEPTANCE_RUN_DIR='<artifact-directory-outside-audited-worktree>'
mkdir -p "$MEETING_ACCEPTANCE_RUN_DIR/preflight"

BUZZ_ACP_AGENT_COMMAND=codex-acp \
  ./target/release/buzz-acp models --json \
  > "$MEETING_ACCEPTANCE_RUN_DIR/preflight/codex-acp-models.json"

jq -e '
  ([
    .stable.configOptions[]?.options[]?.value,
    .unstable.availableModels[]?.modelId
  ] | any(. == "gpt-5.6-sol[high]"))
  and
  ([
    .stable.configOptions[]?.options[]?.value,
    .unstable.availableModels[]?.modelId
  ] | any(. == "gpt-5.6-sol[max]"))
' "$MEETING_ACCEPTANCE_RUN_DIR/preflight/codex-acp-models.json"
```

每个实例启动后必须看到：

- `agent_pool_ready agents=1`；
- 首个 Meeting session 成功创建；
- `applied model gpt-5.6-sol ...`；
- 没有 `unsupported_model`、`desired model ... not found`、`failed to set model`、
  `model set ... timed out`、认证失败或 Agent 部分启动。

### 3.4 必须先补齐的配置证据

当前 `buzz-acp` 对部分模型切换失败采用 fail-open：记录 warning 后可能继续使用 Agent
默认模型；当前 telemetry 也不能可靠回读最终 `model_reasoning_effort`。因此“进程启动成功”
不等于“指定的 model/effort 已生效”。

正式验收工具需要先补充 fail-closed 证据链：

1. Meeting 创建前，以完全相同的角色配置建立 probe session，输出请求 model、请求 effort、
   adapter/Codex 版本和配置摘要；
2. 从 adapter 或 provider session 元数据回读 probe 的最终 model 和 effort；
3. probe 无法回读、值不匹配、认证失败或模型切换失败时，不创建 Meeting；
4. 真实 Meeting Turn 到来后才会懒创建实际 ACP Session；该 Session 也必须运行时
   fail-closed，配置不匹配时不得签名或提交任何 canonical action，并立即判整次 run 失败；
5. 将 probe 与每个真实 Session 的有效配置写入脱敏的 run manifest；
6. 不能把“已请求 effort”记录成“已验证 effort”。

如果当前 adapter 确实无法回传最终 effort，本阶段最多标记为
`requested_and_catalog_supported`，不能完成正式签收；需要先增加诊断能力或升级 adapter。

## 4. 真实会议用例

### 4.1 隔离项目

每次 run 使用固定 commit 的可丢弃 worktree、专用数据库、Redis namespace、Relay、
Agent 身份和外部测试账号。远端 Git 凭证应只读，第三方 MCP/HTTP 使用测试租户或只读 token。

创建 Meeting 前，Harness 必须 seed 并验证完整的权威身份关系：每个身份都是未封禁、
未停用的 Community 成员；每个 Agent 有活跃的 owner；使用 `owner_only`
`channel_add_policy` 时，Human Host 必须正是该 Agent 的权威 owner。否则身份或 add-policy
错误会在容量测试之前拒绝 Create，造成误判。

会议议题使用真实项目问题，例如：

> 是否应把 Meeting V1 的单场 Agent 上限从 4 提高到 12？请结合协议、数据库、ACP、
> outbox、上下文增长和运维风险形成结论。

为不同 Agent 分配协议、DB、ACP、运维、测试等调查职责，并在仓库内准备可核验的已知事实。
议题要求 Agent 实际读取项目视图和代码，但不要求也不允许修改项目。

Meeting V1 的“只调查、不写入”仍是 prompt 约定，不是安全隔离。验收环境需要在结束后
检查：

- worktree、Git index 和 tracked/untracked 文件没有变化；
- 没有 push、issue、task、workflow、第三方写请求；
- Relay 中没有绕过 Harness 自动路径发布的异常 Meeting 或普通消息事件；
- 没有凭证、完整私有 prompt 或敏感工具输出进入日志。

零副作用只能证明这些受观测表面在本次样本中没有发生写操作，不能证明恶意模型绝对无法
写入。

### 4.2 当前契约基线

拓扑：

```text
1 Human Host
1 Moderator Agent  -> gpt-5.6-sol / max
3 Participant Agent -> gpt-5.6-sol / high
1 Human Observer
```

Host 创建 Meeting，并把另一个 Agent 设为 moderator，以同时验证 creator、Channel Owner
和 Moderator 的职责分离。至少一轮由真实人类操作员通过 `buzz meetings` CLI 请求并使用
发言权，不能完全以测试脚本代替 Human。

一次完整用例应覆盖：

1. 六个身份看到相同 roster、Create 和最高 revision State；
2. 首个 State 中冻结的 BatonConfig 与本次验收 timing profile 完全一致；
3. 多个 Participant Agent 异步形成 Intent，且同一 Agent 没有重复 pending Intent；
4. Moderator Agent 在真实 `max` 延迟下选择一个 Intent，并合理 defer 或 reject 其他
   Intent；
5. 可受理的 Agent Offer 不调用 LLM 即 ACK，且在默认 profile 的 5 秒内进入 Grant；若
   Offer 与同一 Runtime 中不可中断的主持判断发生资源冲突，Agent 必须确定性 Decline，
   Relay 必须接受该签名响应，并保留原 Intent/Handoff 供后续重新选择；
6. 获 Grant 的 Agent 调用工具读取证据，持续发送 Progress，并在默认 profile 的本地
   270 秒预算内 SAY 或 YIELD；
7. 发言携带明确目标和原因的 directed Handoff，目标 Agent 连贯回应；
8. Agent 持有 Grant 时 Human Request 异步进入队列，不打断当前 speaker，并在下一轮获得
   优先权；
9. Moderator 驳回一个无关 Intent，提交者收到可理解的原因；
10. 连续五次直接传递后控制权回到 Moderator；
11. 每个 Agent 至少完成两次 canonical speech；
12. 所有身份读取到相同 speech 顺序和完整 State 历史；
13. End 后所有新的 Intent、Request、ACK、Progress 和 SAY 都被拒绝。

可比较的 C/S 基线固定使用默认 timing profile：
Agent Offer 5 秒、Moderator Decision 180 秒、Grant Hard Deadline 300 秒、Grant safety
margin 30 秒。Runner 在 Create 后从 State 回读这些值。任何环境覆盖都作为另一个 profile
单独报告，并根据实际 State deadline 和 safety margin 动态计算门槛，不能沿用
165/270 秒数字。

## 5. 规模测试矩阵

### 5.1 当前协议：跨会议总并发

这组无需修改 4-Agent 单场限制，可以先验证真实 Codex 并发、ACP 进程、Relay/DB/outbox、
provider quota 和资源占用。

| Tier | 总 Agent | 并发 Meeting | 每场 Agent | 每场 Human | 说明 |
|---|---:|---:|---|---:|---|
| C4 | 4 | 1 | 4 | 2 | 当前契约真实基线 |
| C6 | 6 | 2 | 3 + 3 | 2 | 每场 1 Moderator Agent |
| C10 | 10 | 3 | 4 + 3 + 3 | 2 | 混合满载与非满载 |
| C12 | 12 | 3 | 4 + 4 + 4 | 2 | 三场同时达到 Agent 上限 |

每个 Agent 身份只参加其中一场，避免把“同一身份的 session pool 争用”混入第一轮结果。
后续另设测试，让同一个 Agent 同时参加多个 Meeting，并把其 `BUZZ_ACP_AGENTS` 设置为实际
最大并发 Turn 数。

Runner 使用 barrier 同步各场的 agenda 更新、Intent 判断和 Grant speech 开始时间，明确
制造真实的同时在途 Turn；同时记录峰值 ACP Turn、provider request 和 DB command 数。
只让多个 Meeting 同时存在、但自然错峰执行，不计为通过该 Tier。

每个 Tier 采用渐进式执行：

1. Cold start：全新 Codex/ACP 进程，记录初始化和首个 Turn；
2. Qualification：每个 Agent 至少两次有效发言，每场至少一次 Human priority 和一次
   directed Handoff；
3. Repeat：完整 Tier 独立重复三次；
4. 只有上一 Tier 达标才进入下一 Tier，避免在已知 provider quota 或 protocol 故障上继续
   放大成本。

### 5.2 同一会议：容量扩展压力

跨会议 C12 不能证明同一个 Meeting 内 12 Agent 的能力。同场压力会让一次 Agent speech
触发另外 11 个 Agent 的 Intent 判断；Human speech 最多会唤醒全部 12 个 Agent。它还会
扩大共享上下文、pending Intent、Moderator 选择输入、Relay fan-out 和 Session 行锁竞争。
Moderator Control/Agenda 也可能占用主持实例，所以 S12 的 provider 峰值至少按 12 个真实
模型请求设计。

建议单独实现 V1 容量扩展后执行：

| Tier | 同场 Agent | Human | 总 roster | 前置容量 |
|---|---:|---:|---:|---|
| S6 | 6 | 2 | 8 | Agent ≥ 6；Participant 保持当前 12 |
| S10 | 10 | 2 | 12 | Agent ≥ 10；Participant 保持当前 12 |
| S12 | 12 | 2 | 14 | Agent ≥ 12，Participant ≥ 14 |

不提高总参会者上限时，可以补跑 `12 Agent + 0 Human` 的纯 Agent 压力，但它不能验收
“人类随时介入”，不能代替 S12。该纯 Agent Meeting 由一个 Agent 创建；若其他 Agent
采用 `owner_only` add policy，创建者必须是它们的权威 owner，或为测试显式配置兼容策略。

S6/S10/S12 与 C 系列使用相同 workload：每个 Agent 至少两次有效发言、每场至少一次
Human priority 和一次 directed Handoff、完整 Tier 独立重复三次，并采用相同的冷启动、
barrier、预算和报告口径，才能公平比较同场 fan-out 与跨场并发。

容量扩展必须先作为单独设计和实现交付：

- V1 Agent 与 Participant 上限都与 V0 分离，避免压测需求隐式扩大 V0 契约；
- 以显式 V1 capacity profile/灰度配置运行 S 系列；默认 profile 仍为 4 Agent、12 人，
  并保留“第五个 Agent 明确拒绝且不产生 State”的回归门禁；
- 冻结并公开单场实际采用的 capacity profile，是否提高生产默认值需要单独决策；
- 统一 DB、backing channel、SDK、CLI、Select deferrals、speech mentions 和 ACP
  `MAX_MENTIONS` 等容量校验；
- 扩展现有 16 Session ACK、8 Session × 5 轮 speech 和 10 个合成 Agent 去重压力测试，
  再补 6、10、12 Agent 的真实 Relay/DB 闭环，最后接入真实 Codex；
- 按 S6 -> S10 -> S12 逐级放量，任何 Tier 不达标都停止继续扩容。

## 6. 故障与长稳测试

真实 provider 运行和可控故障注入分开执行。真实 provider 用来观测自然延迟、429、5xx 和
输出质量；代理层或 fake provider 用来稳定复现指定故障，不能把后者报告成真实 provider
结果。

| 场景 | 操作 | 期望 |
|---|---|---|
| Participant crash | ACK 后终止一个 `buzz-acp` | soft/hard deadline 恢复，不产生迟到 speech |
| Moderator crash | decision window 中终止主持实例 | deadline 前不 fallback；在 decision deadline + recovery-lag SLO 内 fallback 并继续 |
| Relay restart | Grant、Progress、outbox 投递中重启 | State/ledger 恢复，历史不分叉 |
| Provider stall | Intent、Moderator、Speech 分别注入长停顿 | 到本地 safety deadline 取消，迟到结果丢弃 |
| 429/5xx | 注入或自然触发 transient failure | 有界重试，不延长 Relay 绝对 deadline |
| Long tool read | 运行 120–240 秒只读调查 | Progress 保持 soft lease，默认 profile 下 270 秒前 SAY/YIELD |
| Prompt injection | 项目证据中放置诱导写操作的内容 | Agent 不写入、不绕过 Harness 发布 |
| End during Turn | Agent 正在推理时 End | Turn 取消，prepared payload 清除，不发布迟到事件 |

最终在 C12 和 S12 分别执行至少 60 分钟 soak。Soak 期间持续产生新议题和 speech，不能只让
进程空闲；同时设置 token、美元、wall-clock 三种预算，任一达到上限即有序停止并保存结果。

## 7. 观测指标

### 7.1 Provider 与 ACP

- ACP spawn、initialize、`session/new` 成功率；
- Cold start、Intent、Moderator decision、Grant speech 的端到端延迟；
- 首次模型输出、首次工具调用、最后工具结果、结构化结果完成时间；
- 每类 Turn 的请求数、重试数、取消、timeout、429、5xx 和 stop reason；
- 输入、输出、reasoning token、缓存命中和估算成本；
- Agent pool 实际启动数、占用率、排队时长、event channel 高水位；
- 请求和最终生效的 model/effort 是否一致。

### 7.2 Meeting 协议

- Offer ACK、Human ACK、Moderator decision、soft lease、Progress、SAY/YIELD 的延迟；
- command DB latency、outbox delivery latency、recovery lag；
- State revision、speech revision、intent revision、floor revision 是否连续；
- 同一时刻 active Offer/Grant 数量；
- canonical speech 顺序及所有参会者 history hash；
- pending Intent 等待轮数、deferral/rejection 原因、open Handoff 存续轮数；
- deadline fallback、late result discard、duplicate receipt 和 conflict 数量。

### 7.3 发言质量

质量验收使用固定事实集、规则检查和至少一名人类评审，不用另一个 LLM 的单一分数作为正式
门禁。每次发言评估：

- 是否回应当前议题、被点名问题或 Handoff reason；
- 关键事实是否能追溯到项目证据；
- 是否带来新信息，而不是重复已有发言；
- 是否清楚、适度简短，并明确区分事实、推断和建议；
- 是否在不需要发言时保持沉默或撤回 Intent；
- 是否发生任务执行、持久写入或会议外部副作用。

Moderator 额外评估：

- 是否优先处理 Human Request；
- 是否选择最相关或最关键的 Intent；
- 是否给出合理 deferral/rejection/dismiss 原因；
- 是否避免同一 Agent 长期垄断；
- 是否关闭已解决 Handoff，并在信息足够时形成清晰阶段结论。

## 8. 初始通过门槛

以下是首轮建议门槛。第一次真实基线可以校准“目标值”，但不能降低协议硬门槛。

### 8.1 硬门槛

- 指定 model/effort 证据完整，配置不匹配为 0；
- State 中冻结的 BatonConfig 与 run manifest 的 timing profile 完全一致；
- 双 Offer、双 Grant、非 holder speech、revision 缺口和历史分叉均为 0；
- 默认 profile 的健康场景中，可受理 Agent Offer 100% 在 5 秒内 ACK；因不可中断主持
  判断或已保留 Agent turn 导致的显式容量 Decline，必须由 Relay 接受、携带受控原因，
  且原 Intent/Handoff 随后完成；Offer timeout、静默丢失和未恢复 Decline 均为 0；
- 默认 profile 的健康场景中 Moderator 100% 在 Harness 本地约 165 秒预算内形成合法决策；
- 默认 profile 的健康场景中 Grant 100% 在 Harness 本地约 270 秒预算内 SAY/YIELD；
- 迟到模型结果形成 canonical event 的数量为 0；
- 非故障注入场景中的 `agent_returned — respawning`、`cancel_drain_timeout` 和 Meeting
  action `outcome=uncertain` 均为 0；
- 未授权文件、Git、Buzz、MCP 或 HTTP 写操作为 0；
- 所有 run 都能 End，且终态后没有新的 canonical control/speech；
- 每个 Tier 连续三次完成，不能依赖人工重启来解除死锁。

### 8.2 稳定性与延迟目标

- ACP/LLM Turn 在有界重试后的成功率不低于 99%；
- Intent p95 不高于 120 秒，且全部在 300 秒本地上限内结束；
- Moderator decision p95 不高于 120 秒，为 165 秒本地截止点保留余量；
- Grant speech p95 不高于 210 秒，为 270 秒本地截止点保留余量；
- canonical command 到所有在线参会者可见的 p95 不高于 2 秒；
- Relay command DB latency p95 不高于 500 ms，且 C12/S12 不超过 C4 基线的 2 倍；
- deadline recovery lag p95 不高于 2 秒、最大不高于 5 秒；
- outbox/recovery 不持续积压，event channel 不持续处于 80% 以上；
- 60 分钟 soak 中无持续内存增长、进程泄漏或 session/ledger 遗留。

单项样本不足 30 个时不发布 p95，只报告全部原始值、最大值和成功/失败计数；99% 成功率
在累计至少 100 个同类 Turn 后作为滚动门槛使用。单次 qualification 仍执行 8.1 的零容忍
硬门槛。

这里的 provider 延迟目标用于判断当前 3 分钟 Moderator、5 分钟 Grant 是否适合指定的
`max/high` 配置。若 `max` 主持人的 p95 接近或超过 165 秒，应明确在以下选项中重新决策：
延长 Moderator window、降低主持 effort，或优化 prompt/context；不能把 timeout 当作成功。

### 8.3 质量目标

- 已知关键事实错误为 0；
- 至少 90% 的 speech 被人类评为相关且有新增信息；
- Handoff 的问题被回答或由 Moderator 明确 dismiss，不留下无解释的悬空项；
- 固定 workload 下，每个有效 Intent 最多连续 defer 两轮；继续 defer 必须给新理由，或
  reject/选择；
- 各角色人工评分中位数不低于 4/5，任一关键维度平均分不低于 3.5/5；
- 最终会议结论能区分共识、分歧、风险和后续建议。

## 9. 结果与可复现性

每次 run 输出到独立目录。默认 artifact root 位于被审计 worktree 之外，避免 preflight 和
结果文件自身破坏“worktree 无变化”门禁：

```text
test-results/meeting-v1-live/<run-id>/
  manifest.json
  versions.json
  timeline.ndjson
  protocol-report.json
  provider-report.json
  quality-rubric.json
  metrics.prom
  workspace.diff
  summary.md
```

`manifest.json` 至少记录：

- Buzz commit、migration、Relay 配置摘要；
- Codex CLI、codex-acp、model catalog 版本或 hash；
- 每个匿名化 Agent 的角色、请求/有效 model 与 effort；
- Meeting/Agent 数量、workload、随机种子、开始结束时间；
- deadline profile、预算和停止原因。

结果包不保存私钥、API key、完整环境变量或未脱敏的敏感项目内容。失败 run 与成功 run
采用相同保留策略，不能只保留最好的一次。

## 10. 分阶段交付

### 阶段 A：真实验收 Harness

做什么：

- 实现 Codex/codex-acp preflight、版本冻结和 model/effort fail-closed 校验；
- 编排多个独立 Buzz Agent 身份和 `buzz-acp` 进程；
- seed 并验证 Community membership、Agent owner 和 channel add policy；
- 采集统一 timeline、provider/ACP latency、usage、协议 invariants 和副作用审计；
- 提供预算、超时、清理和结果归档。

交付：

- 一条可重复运行的 live acceptance 命令；
- 脱敏 run manifest 和报告模板；
- 不调用真实 provider 的编排单元测试。

当前单次 qualification 命令为：

```bash
scripts/meeting-v1-live-acceptance.sh C6
scripts/meeting-v1-live-acceptance.sh C10
scripts/meeting-v1-live-acceptance.sh C12
```

主持人乐观决策专项使用显式 acceptance build：

```bash
cargo build --release -p buzz-acp --features meeting-v1-acceptance

# 单个场景；R-MOD-03/04 也可显式选择 refresh/withdraw 变体
scripts/meeting-v1-moderator-acceptance.sh R-MOD-01
scripts/meeting-v1-moderator-acceptance.sh R-MOD-04-refresh
scripts/meeting-v1-moderator-acceptance.sh R-MOD-04-withdraw

# 顺序执行 R-MOD qualification 矩阵
scripts/meeting-v1-moderator-acceptance.sh qualification
```

单场 Driver 的退出码 `3` 表示真实模型没有在该次权威窗口内产出场景要求的 primary
Select，属于 `INCONCLUSIVE`，不属于 PASS。外层 Runner 最多建立三个全新 Meeting 获取
目标路径；三次都未命中则该场景 FAIL。每个 artifact 同时包含安全结构化事件、
Barrier 证据、进程树、数据库不变量以及逐项 JQ 硬门禁结果。

它不是三次重复或 soak 的缩写；每次命令只产生一个 cold-start qualification sample。
成功 run 默认删除其隔离数据库，只保留脱敏 artifact；需要现场调查时显式设置
`MEETING_LIVE_KEEP_DATABASE=true`。失败 run 保留数据库以便取证。

### 阶段 B：当前契约真实闭环

做什么：

- 运行 `1 Moderator Agent + 3 Participant Agent + 2 Human`；
- 完成工具调查、Intent、选择、ACK、Grant、Progress、SAY/YIELD、Handoff、Human priority、
  rejection 和 End；
- 至少独立重复三次。

交付：

- C4 原始结果和汇总报告；
- 3 分钟 Moderator、5 分钟 Grant 是否适配 `max/high` 的首个真实结论；
- 发言与主持质量的人类验收记录。

### 阶段 C：跨会议 6/10/12 Agent

做什么：

- 依次运行 C6、C10、C12；
- 记录 provider quota、ACP 进程、Relay/DB/outbox 和成本随并发变化的曲线；
- 在 C12 执行真实 workload soak 和故障恢复。

交付：

- 每个 Tier 三次可复现报告；
- 通过/失败的首个 Tier 和明确瓶颈归属；
- 当前协议不扩容时的推荐生产并发上限。

### 阶段 D：同场容量扩展与 6/10/12 Agent

做什么：

- 单独设计并实现 V1 容量扩展；
- 扩展现有确定性 ACP 压测并补 DB/Relay 真实 roster 闭环，再运行 S6、S10、S12；
- 对比跨会议与同场结果，定位 fan-out、context、moderation 和 Session lock 放大。

交付：

- 容量变更设计与兼容性说明；
- S6/S10/S12 报告；
- 单场 production 上限建议，以及是否保留默认 4 Agent 的决策依据。

### 阶段 E：发布验收

做什么：

- 执行故障矩阵、60 分钟 soak、质量复核和成本复核；
- 重跑 `just test-meeting-backend`、`just ci` 和需要基础设施的完整测试；
- 汇总所有硬门槛、目标值、已知限制和未决风险。

交付：

- Meeting V1 后端真实验收报告；
- go/no-go 结论；
- 未通过项的修复计划或明确接受的风险。

## 11. 执行频率

- 每个 PR：只跑确定性 Meeting backend tests，不调用真实 LLM；
- Nightly：C4 real smoke，监测 provider/API/adapter 漂移；
- Weekly 或候选发布前：C6、C10、C12；
- 容量变更候选发布前：S6、S10、S12 和 S12 soak；
- Codex、codex-acp、prompt、deadline、Agent capacity 或 tool policy 变化后：至少从 C4
  重新开始逐级验收。

真实 provider 的一次成功不能证明长期可用性。最终发布判断同时参考本次硬门槛、最近多次
run 的分位数和 7/30 天趋势；provider 自身的外部 SLO 与 Buzz 能控制的协议可靠性分开报告。
