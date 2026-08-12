# Project Context 图语义查询单 Relay 信任策略实施方案

> 状态：已实施，首轮 Live 验收通过；跨三个旧 TTL 窗口的长时观察待完成
>
> 日期：2026-08-12
>
> 当前产品边界：Carryforth 仅支持本地源码构建与单 Relay 运行；暂无生产、多 Pod、负载均衡或滚动发布需求
>
> 目标策略：本地默认 `trusted-single-relay`；保留当前 `attested-fleet` 严格路径，但在 policy 被纳入
> 完整 routing inventory 证明前，不宣称它已具备多实例生产资格
>
> 数据边界：本方案不新增或修改 SQL migration，不删除 Fleet 记录，不重建向量，不改写 Project、
> Project View、Document、Meeting、Project Context、Event、成员关系或 Desktop 本地数据
>
> 历史边界：Fleet Attestation 由语义查询提交 `b27829db3` 引入，并以等价 cherry-pick
> `4a54a2890` 合入当前集成分支；它不是 Carryforth 去 Buzz 改名产生的行为

## 0. 已确认结论

当前阶段采用以下产品决定：

1. 同一时刻只有一个为 Carryforth Desktop 服务的 Relay；
2. 当前没有负载均衡后的多实例、混合版本 Pod 或滚动升级流量；
3. 因此 Fleet Attestation 所证明的“完整路由清单中的所有 Relay 运行相同 query runtime”在当前拓扑中
   没有实际增益；
4. 本地语义查询不再依赖短期 Fleet 租约，也不需要人工或后台续租；
5. 不删除 Fleet 机制，而是增加明确的拓扑策略：
   `trusted-single-relay | attested-fleet`；
6. 当前本地默认 `trusted-single-relay`；未来出现多 Relay、负载均衡或生产发布时，必须先完成 policy
   homogeneity 证明，再显式切换全部实例为 `attested-fleet`；
7. 单 Relay 策略只移除 topology / inventory / TTL 证明，不能弱化身份、权限、Community、Provider
   出域、generation/currentness 或结果签名检查；
8. `BUZZ_SEMANTIC_GRAPH_QUERY_HTTP_AVAILABLE` 继续作为 deployment master，默认仍为 `false`；
9. 每个 Community 的 `semantic_graph_query_enabled` 继续默认关闭；
10. `query-enable` 继续要求 `--acknowledge-problem-egress`，不能随 Relay 启动自动开启；
11. Fleet 表、严格模式代码与 migration 0058 原样保留；
12. 本次同时修正 `buzz-admin semantic query-readiness` 混用调用者环境与 Live Relay 状态导致的误诊。

一句话目标：

> 在当前单 Relay 本地拓扑中，语义查询随 Relay 持续可用，不再每 15 分钟因租约过期撤销能力；
> 切回 `attested-fleet` 会恢复现有 Attestation 失败关闭路径；但多实例生产资格仍要求控制面额外证明
> 每个被路由实例都运行 `attested-fleet`，不能只比较相同 binary runtime digest。

## 1. 问题复盘

### 1.1 当前故障链

当前实现的 Fleet 租约只允许 30–900 秒。Community query gate 是持久状态，但 Attestation 是短期状态：

```text
query_enabled=true
        +
Fleet Attestation 到期
        │
        ├── NIP-11 不再广告 buzz-project-context-semantic-query-http
        ├── cf 在 Provider 前返回 non-retry unsupported
        ├── HTTP /query 入口失败关闭
        └── /_readiness 因 semantic=false 返回 503
```

这次验收现场中，Relay 的 deployment master、deployment ID、instance ID、Provider 和索引都正常；
实际原因是 15 分钟 Attestation 在验收 Agent 启动前已经到期。

### 1.2 为什么当前不需要租约

Fleet 租约主要防止：

- 负载均衡后仍有旧版本 Pod；
- rolling deployment 中新旧 runtime digest 混跑；
- 控制面 inventory 与实际路由集合漂移；
- 某个实例没有 query handler，却仍被路由到 `/query`。

当前本地拓扑只有一个 Relay 监听固定地址。单个进程不可能同时运行两个 query runtime digest；
因此短租约没有增加同质性证明，只增加了一个必须持续维护的外部控制面。

### 1.3 诊断误归因

`buzz-admin semantic query-readiness` 当前从 **buzz-admin 自身进程环境**读取：

- `BUZZ_SEMANTIC_GRAPH_QUERY_HTTP_AVAILABLE`；
- `BUZZ_SEMANTIC_GRAPH_QUERY_DEPLOYMENT_ID`；
- `BUZZ_SEMANTIC_GRAPH_QUERY_INSTANCE_ID`。

Managed Agent 子进程没有继承这些变量时，命令会输出 `master=false`、ID 缺失；这不代表运行中的
Relay 配置。Live Relay 的真实运行配置只能来自 `/_status`。本方案必须关闭这个诊断歧义，但不能让
诊断命令成为新的授权旁路。

## 2. 目标与非目标

### 2.1 目标

- 本地单 Relay 在 Fleet 记录不存在、已过期或已撤销时仍可持续提供语义查询；
- 本地 Relay 重启后不需要 `fleet-attest` 或续租 supervisor；
- NIP-11、`/_readiness`、HTTP `/query`、Provider 前复核与签名前复核使用同一拓扑策略；
- 通过强类型策略保证单 Relay 分支只省略 Fleet 行检查；
- 严格 Fleet 模式的现有行为、数据库合同和 operator 命令保持可用；
- Live 状态和 admin-process 环境在诊断输出中明确区分；
- 配置切换可逆，不需要数据迁移。

### 2.2 非目标

- 不交付生产或多 Pod 资格；
- 不实现 Fleet 自动续租控制器；
- 不延长 0058 的 900 秒上限；
- 不删除 `semantic_graph_http_fleet_attestations`；
- 不自动启用任何 Community 的 query gate；
- 不自动执行 Provider canary；
- 不修改 Provider、embedding、召回、评分、路径搜索或 Desktop 语义图 UI；
- 不改变 `/query` wire schema、kind `40912` 或 `cf` 的成功响应合同；
- 不把 `debug_assertions`、Host、PID 或“看起来像 localhost”作为隐式绕过条件；
- 不宣称 `trusted-single-relay` 可用于负载均衡、多实例或生产部署。

## 3. 必须保持的安全与功能不变量

`trusted-single-relay` 只跳过 Fleet inventory / TTL / deployment membership 证明。以下检查必须原样保留：

1. `BUZZ_SEMANTIC_GRAPH_QUERY_HTTP_AVAILABLE` deployment master；
2. 每 Community 的 `semantic_graph_query_enabled`；
3. `query-enable --acknowledge-problem-egress`；
4. pgvector schema、索引 gate、active generation 和非零 current heads；
5. Project Context capability 和 canonical currentness；
6. 稳定 Relay signer；
7. Provider、model、dimensions 与 source contract；
8. NIP-98 exact body、Host / Community / Project 绑定；
9. caller 身份、成员资格和读取权限；
10. Provider admission、并发限制、deadline 和预算；
11. Provider 出域前的 principal / generation / Context / source epoch 原子复核；
12. Provider 返回后的当前 principal / ban、query / index gate、source-family capability / readiness 复核；
13. request binding、虚拟结果 Event 签名和 response binding；
14. `query-disable` 的即时 kill-switch 语义；
15. 日志不得输出 API key、完整 problem、overview、向量或签名私钥。

禁止用 `Option<deployment_id>` 或裸布尔参数表示“是否检查 Fleet”。调用者必须传递强类型拓扑信任，
避免未来新增调用点把缺失配置错误解释成授权。

## 4. 配置与类型设计

### 4.1 新配置

新增：

```text
BUZZ_SEMANTIC_GRAPH_QUERY_FLEET_POLICY=trusted-single-relay
```

允许值严格限定为：

```text
trusted-single-relay
attested-fleet
```

非法值在 Relay 启动及所有依赖 topology policy 的 `buzz-admin` 命令中直接报错，不能回退。
`query-disable` 与 Foundation `disable` 不依赖也不解析该 policy，确保即使 policy 拼错，operator 仍可
关闭 query / Provider eligibility。

未设置该变量时，Config 缺省为 `trusted-single-relay`；`.env.example` 仍显式写出该值，使拓扑信任
不会被隐藏。当前支持面没有生产部署，因此该值是本地默认；启动日志必须打印一次内容无敏感信息的
警告：该模式不适用于多个 Relay 或负载均衡。

这是一个有意的兼容性变化：若旧环境同时满足
`BUZZ_SEMANTIC_GRAPH_QUERY_HTTP_AVAILABLE=true` 且数据库中 Community query gate 已经是 `true`，
升级重启后将不再依赖过期租约关闭 Provider 出域。升级前若不希望恢复查询，operator 必须先执行
`query-disable`，或显式配置 `attested-fleet`。实现不得自动修改或提交用户的真实 `.env`；只更新
`.env.example`，Live 验收时再由 Human 在保密环境中确认当前 `.env`。

`attested-fleet` 继续要求非空 deployment ID 和 instance ID；`trusted-single-relay` 不需要这两个 ID
参与授权。为了观测或未来切换，可以继续配置它们，但不能让可选 ID 重新触发部分 Fleet 检查。

### 4.2 共享策略类型

在 `buzz-semantic-query` 中定义 closed enum，例如：

```rust
pub enum SemanticGraphQueryFleetPolicy {
    TrustedSingleRelay,
    AttestedFleet,
}
```

Relay Config 解析后再构造只能由配置层产生的运行时信任类型：

```rust
pub enum SemanticGraphQueryRoutingTrust<'a> {
    TrustedSingleRelay,
    AttestedFleet {
        deployment_id: &'a str,
        instance_id: &'a str,
    },
}
```

`AttestedFleet` 的两个 identity 必须在 Config 构造阶段完成语法验证；业务调用点不能自行从多个
`Option<String>` 拼装策略。

Query serving 的 Stage B / Stage D 需要 instance identity，而 strict `query-enable` 只需要 deployment
级 Fleet 行。实现应为这两个用途定义各自清晰的强类型 requirement，不能为了复用一个类型而伪造
instance ID。所有新增 public 类型必须有 doc comments。

### 4.3 默认关闭与本地默认信任的关系

“本地默认信任当前 Relay”不等于“默认开启语义出域”：

```text
HTTP master=false                         → handler 不可用
HTTP master=true + query gate=false       → 不广告、不出域
HTTP master=true + query gate=true
  + trusted-single-relay                  → 不需要 Fleet，其他门禁通过后可用
  + attested-fleet                        → 继续要求有效 Fleet Attestation
```

## 5. Relay 执行面改造

### 5.1 本机 handler readiness

当前 `semantic_graph_http_local_handler_ready()` 同时要求 deployment / instance ID。改造后：

- 两种策略都要求 HTTP master、稳定 signer、runtime digest 和 Provider；
- `trusted-single-relay` 不要求 Fleet identity；
- `attested-fleet` 继续要求两个 identity；
- Provider 或 signer 缺失时，两种策略均失败关闭。

### 5.2 统一 routing readiness

将现有 Fleet helper 策略化，而不是在每个调用点散落条件：

```text
routing_ready(community)
├── TrustedSingleRelay
│   └── local_handler_ready
└── AttestedFleet
    └── local_handler_ready + existing DB fleet readiness
```

同一个 helper 必须覆盖：

- NIP-11 capability；
- `/_readiness` 的所有 query-enabled Communities；
- `/query` HTTP 入口；
- Provider 出域前的 Relay 层防线。

只改 NIP-11 或只忽略 `expires_at` 都不合格：前者会让请求进入后仍 503，后者会把陈旧 inventory
错误冒充为当前路由事实，并且仍要求首次人工 Attestation。

### 5.3 NIP-11

`trusted-single-relay` 下，NIP-11 不读取 Fleet 表。在以下条件都满足时继续广告：

- Project Context ready；
- database query readiness ready；
- Community query gate enabled；
- HTTP master enabled；
- stable signer ready；
- Provider ready；
- local query runtime ready。

扩展名保持：

```text
buzz-project-context-semantic-query-http
```

该值是 wire compatibility 标识，本阶段不重命名。

### 5.4 `/_readiness` 与 `/_status`

`trusted-single-relay` 下，过期或缺失 Fleet 行不能再令全局 readiness 失败；其他 schema、worker、
Provider、signer、generation 或 query readiness 失败仍返回 503。

`/_status.semantic_graph_query_http` 增加：

```json
{
  "fleet_policy": "trusted-single-relay",
  "fleet_attestation_required": false,
  "fleet_attestation_status": "not_required"
}
```

严格模式输出 `fleet_attestation_required=true`，并保留 deployment / instance / runtime digest 状态。
Fleet 行是 per-Community，而 `/_status` 是 deployment-global；因此 strict status 只能输出
`fleet_attestation_status="community_scoped_not_evaluated"`，不能把某个 Community 的 ready / expired
冒充成全局结论。真实 Fleet 状态由带 Community 的 `query-readiness` 或 `/_readiness` 聚合判断。
不能把 `not_required` 表述为 `ready` 或 `attested`。

## 6. 数据库事务边界

### 6.1 Provider 出域前确认

当前 Provider slot 等待完成后，DB 在 Community/source 锁内复核 caller、query gate、generation、
Context、source epochs 和 Fleet。改造时把 request 中的 Fleet identity 替换为强类型 routing trust：

```text
TrustedSingleRelay
└── 跳过 Fleet row lock/check

AttestedFleet { deployment_id, instance_id }
└── 执行现有 Fleet row lock/check
```

两个分支必须共享其余完整事务；不能为本地模式新增一个绕开 Community/source/currentness 复核的
快捷查询。

### 6.2 结果释放与签名前确认

最终 release confirmation 采用相同强类型策略：

- 本地模式只省略 Fleet 行检查；
- Stage D 继续执行当前 composite security/readiness：当前 principal / ban、query / index gate、
  source-family capability / readiness 与 stable release permit；
- permit 必须紧邻结果签名消费；
- Provider 返回后发生 `query-disable` 或当前 caller 授权撤销时，结果不得签名；
- Stage D 不把结果重新绑定到 Stage C 的原 generation、Context revision 或 source heads，也不把
  Stage C 的 as-of snapshot 改写成当前数据。Result observations 与后续 canonical read commands 继续
  明确表达 snapshot/current 的差异；
- generation / Context / source churn 与原 ticket 的精确比较属于 Provider 出域前 Stage B，不得在本次
  Fleet policy 修复中扩大 Stage D 功能合同。

### 6.3 Query enable

新增事务化 local enable 路径，或从现有 `enable_semantic_graph_query_with_http_fleet()` 抽取共同
database prerequisites：

```text
trusted-single-relay
└── database prerequisites + explicit egress acknowledgement

attested-fleet
└── database prerequisites + explicit egress acknowledgement + valid fleet row lock
```

本地路径不能退化为裸 `UPDATE communities SET semantic_graph_query_enabled=true`。

### 6.4 Migration 与现有 Fleet 行

- 不修改 `migrations/0058_project_context_semantic_query.sql`；
- 不改变 migration checksum；
- `trusted-single-relay` 仍必须通过完整 `semantic_graph_query_schema_ready()`，其中仍要求 migration
  0058 的 Fleet 表、query=>index 约束、非零 embedding 约束与 Provider admission 表存在；只忽略当前
  Attestation 行，不能从 schema readiness 中删除 Fleet 表；
- 不 DROP、TRUNCATE、DELETE Fleet 表；
- 不清理当前过期或 revoked Attestation；
- 本地策略将这些行视为不适用，而不是 ready；
- 切到严格策略后，旧过期行自然失败关闭，operator 必须创建新的有效 Attestation。

## 7. `buzz-admin` 行为与诊断修复

### 7.1 Query readiness 按策略分流

输出至少增加：

```json
{
  "fleet_policy": "trusted-single-relay",
  "fleet_attestation_required": false,
  "fleet_attestation_status": "not_required",
  "http_runtime_source": "admin_process_environment",
  "live_relay_observed": false
}
```

本地模式的 database/policy observation 不读取 Fleet 表；严格模式保持现有 Fleet 计算。由于显式
`/_status` endpoint 仍未与所选 Community 做身份绑定，legacy `base_enable_ready` 输出 `null`，并拆分
输出 `database_and_policy_ready`、`http_runtime_ready`、
`admin_process_configuration_ready` 与 `community_binding_verified=false`，避免再次把诊断组合误读成
授权事实。

### 7.2 Live Relay 状态

给现有命令增加显式可选参数：

```bash
buzz-admin semantic query-readiness \
  --relay-status-url http://127.0.0.1:8080/_status
```

指定后：

- 从 Live Relay 读取 master、policy、deployment、instance、runtime digest、parser 和 handler readiness；
- 用观察到的 strict identity 查询 Fleet；
- 响应失败、非 2xx、超时、超限、错误 schema 或 digest 不匹配时直接失败，不能回退到调用者 env；
- HTTP client 禁止 redirect、禁止 proxy、设置短 timeout 和响应上限；
- 默认只允许 loopback status URL；
- 不携带 Provider key、私钥或认证 header。

该 URL 只提供诊断观察，绝不能成为 `query-enable`、Stage B、Stage D 或任何写操作的授权来源。
显式选择一个 loopback endpoint 也不会自动证明它与 `RELAY_URL` 所对应的 Community 是同一进程；
输出必须保留 endpoint 与 source 标签，不得扩大为“已验证当前 Community Relay”。所有 mutation 继续
使用调用者明确加载并经 Config 校验的 deployment policy / identity。

未指定 URL 时保留兼容行为，但 stderr 必须明确提示这些字段来自 admin 进程，不代表 Live Relay；JSON
必须标记 `live_relay_observed=false`。

### 7.3 Fleet operator 命令

保留：

- `fleet-attest`；
- `fleet-check`；
- `fleet-revoke`。

它们只属于 `attested-fleet` 运维面，退出与写入语义固定为：

- local policy 下 `fleet-attest` 和 `fleet-revoke` 非零退出且不写数据库；
- local policy 下 `fleet-check` 输出 `applicable=false`、`status=not_required`，退出 0；
- local policy 下旧 Fleet 行不影响当前 admission，`fleet-revoke` 也不是 kill switch；需要立即停止出域
  必须执行 `query-disable` 或关闭 HTTP master；
- 如需检查、写入或撤销 dormant strict Fleet 行，operator 先显式设置 `attested-fleet`，再执行现有
  operator 命令。

任何输出都不能把 dormant Fleet 行称为当前授权。

## 8. 文件改动矩阵

| 文件 | 计划改动 |
|---|---|
| `crates/buzz-semantic-query/src/fleet.rs` | 新增 closed Fleet policy / routing trust 类型与严格 parser |
| `crates/buzz-semantic-query/src/lib.rs` | 导出共享类型和文档 |
| `crates/buzz-relay/src/config.rs` | 解析 policy；仅 strict 要求 deployment / instance ID；配置测试 |
| `crates/buzz-relay/src/semantic_fleet.rs` | 将 Fleet-only helper 策略化为 routing readiness；保留严格实现 |
| `crates/buzz-relay/src/nip11.rs` | NIP-11 使用统一 policy readiness |
| `crates/buzz-relay/src/router.rs` | readiness 按策略计算；status 输出 policy 和 applicability |
| `crates/buzz-relay/src/api/bridge.rs` | HTTP 入口与签名前 release confirmation 传递 typed trust |
| `crates/buzz-relay/src/semantic_graph_query.rs` | Provider 前 egress confirmation 传递 typed trust |
| `crates/buzz-relay/src/main.rs` | 本地信任模式启动警告；不自动 enable / attest |
| `crates/buzz-db/src/semantic_query.rs` | Stage B / Stage D 仅在 strict 分支检查 Fleet，保留其他事务锁 |
| `crates/buzz-db/src/semantic_fleet.rs` | 保留 strict API；抽取共同 enable prerequisites，增加 local 原子 enable |
| `crates/buzz-admin/src/semantic.rs` | readiness / query-enable 按策略分流；新增 Live status 诊断 |
| `crates/buzz-admin/Cargo.toml` | 如需要，加入固定版本且禁 redirect/proxy 的 HTTP client 依赖 |
| `.env.example` | 记录本地默认 policy 和 strict 切换说明 |
| `docs/semantic-pgvector-operations.md` | 区分本地单 Relay 与未来 attested Fleet runbook |
| 本文档 | 实施后记录提交、验证结果和未完成的生产边界 |

不修改：

- migration 0057 / 0058；
- Desktop semantic request / result DTO；
- `cf` wire contract；
- Provider model contract；
- Project Context canonical schema；
- `deploy/charts` 或已退役部署面。

## 9. 分阶段实施

### Phase F0：冻结策略合同，保持当前严格行为

交付：

- 新增 shared enum、parser、Display / serialization；
- Config 持有 policy；
- 先保持 `attested-fleet` 作为测试基线，证明无行为变化；
- 补非法值、strict identity 缺失和 status schema 单测。

退出门：

- 当前 strict Fleet 全部测试原样通过；
- 没有调用点通过 `None` 或布尔绕过 Fleet；
- migration 与锁文件没有非预期变化。

### Phase F1：数据库 typed routing trust

交付：

- Provider 出域前 confirmation 支持两种显式策略；
- 结果释放 confirmation 支持两种显式策略；
- query-enable 抽取共同 prerequisites 并增加 local 原子路径；
- strict 分支 SQL、锁顺序和失败码保持不变。

退出门：

- local 分支只省略 Fleet 行检查；
- Stage B 仍按 expected ticket / Context expectations 阻止 generation / Context / source churn 后出域；
- Stage D 仍在 query-disable 或当前权限撤销后阻止签名，但不新增原 snapshot identity 比较；
- strict missing / expired / revoked / mismatch 全部继续失败关闭。

### Phase F2：Relay capability、readiness 与入口统一

交付：

- local handler readiness 按 policy 处理 identity；
- NIP-11、全局 readiness、HTTP 入口、Provider 前与签名前全部传递相同 policy；
- status 输出 policy / attestation applicability；
- 本地模式启动时打印一次警告。

退出门：

- missing / expired Fleet 在 local 模式不影响广告或查询；
- 同一状态在 strict 模式仍撤广告并拒绝 Provider egress；
- Provider/signer/schema/query gate 任一失败，两种模式都不广告。

### Phase F3：Admin 与诊断闭环

交付：

- `query-readiness` 按 policy 计算；
- 增加 `--relay-status-url`；
- 明确 admin env 与 Live Relay source；
- query-enable 两种策略均保持 egress acknowledgement；
- 更新活动运维文档。

退出门：

- Agent 子进程缺少 BUZZ deployment env 时，不会再把 Live Relay 误报为 master=false；
- 显式 status 请求失败不能静默回退；
- local 输出 `not_required`，strict 输出真实 Fleet failure。

### Phase F4：切换本地默认并执行验收

交付：

- `.env.example` 与本地源码启动面设为 `trusted-single-relay`；
- 保持 HTTP master 与 Community query gate 的显式配置；
- 重构建并重启 Relay / Desktop；
- 使用非敏感合成 problem 执行真实 Provider 和 `cf` / Desktop 验收。

退出门：

- 本地运行超过三个原 15 分钟租约窗口，NIP-11 能力持续存在；
- `/_readiness` 持续 200；
- 没有新的 Fleet Attestation 写入也能完成真实查询；
- canonical 数据、向量 generation 和现有业务对象计数不变；
- 不宣称多实例或生产资格。

## 10. 测试矩阵

### 10.1 配置与策略单测

- policy 缺省值符合当前本地产品决定；
- 两个合法值准确解析、显示和序列化；
- 非法值 hard error；
- local + master 可不配置 Fleet identity；
- strict + master 缺 deployment / instance 任一项均启动失败；
- HTTP master=false 时策略不能意外广告 query。

### 10.2 DB 集成测试

`trusted-single-relay`：

- Fleet row missing / expired / revoked 均不影响 local egress confirmation；
- query gate off 仍失败；
- caller 权限撤销仍失败；
- Provider slot 等待期间 active generation / source epoch / Context 改变仍被 Stage B 拦截；
- Provider 返回后 query-disable 或当前 caller 授权撤销仍被 Stage D 拦截；
- Stage D 不要求原 generation / Context / source heads 与 Stage C snapshot 相同，as-of result 合同不变；
- release confirmation 失败时不签名。

`attested-fleet`：

- valid Attestation 正常；
- missing / expired / revoked / deployment mismatch / instance missing / runtime mismatch 全部失败；
- 当前 Fleet row lock 与数据库时钟语义不变。

### 10.3 Relay / NIP-11 / HTTP 测试

- local + query ready + expired Fleet：NIP-11 广告、readiness 200、HTTP query 可执行；
- strict + 同一 expired Fleet：NIP-11 不广告、readiness 503、Provider 调用计数为零；
- local + Provider missing / signer missing / schema not ready / project-context off：不广告；
- `/_status` 不泄露 secret，并准确标记 `not_required`；
- strict `/_status` 只报告 `community_scoped_not_evaluated`；两个 Community 一 ready、一 expired 时，
  tenant-aware readiness 各自准确且 deployment-global status 不伪造单一结论；
- 从 local 切到 strict 且无新 Attestation：四个表面立即失败关闭；
- strict 补齐 Attestation 后恢复，不需要数据迁移。

### 10.4 Admin 回归

- 无 status URL 时输出 source=`admin_process_environment` 和 warning；
- 有 status URL 时 caller env 缺失或冲突，仍以 Live Relay 为准；
- Live Relay master=true + expired strict Fleet 正确报告 `expired`，不能报告 `missing`；
- malformed URL、redirect、proxy trap、非 2xx、超时、超限 JSON 均失败关闭；
- local query-enable 不需要 Fleet，但仍要求 acknowledgement 和 DB prerequisites；
- strict query-enable 继续要求有效 Attestation。
- local `fleet-attest/revoke` 非零且零写入，`fleet-check` 返回 not_required；
- trusted 模式下 dormant row 即使 revoked 也不影响查询，只有 `query-disable` 能立即关闭当前出域。

### 10.5 Live 本地验收

在不执行 reset、down-volume 或数据清理的前提下：

1. 记录当前 Community、Project View、Document、Meeting、Context、Event、semantic heads 和 generation 计数；
2. 启动 `trusted-single-relay` Relay；
3. 验证 status、readiness、NIP-11；
4. 用真实 Agent Memory 问题覆盖：无坐标召回、显式 initial、soft context、lifecycle、预算、结果签名；
5. 用 canonical read commands 回读结果引用的 Project / Context / Document；
6. 等待超过三个旧 TTL 窗口并重复查询；
7. 验证 Provider error/success 指标中没有租约到期错误；
8. 再次记录权威数据计数、active generation 与 Fleet 行；
9. 确认无删除、无隐式 query-enable、无新 migration。

### 10.6 可执行质量门与不变证据

实现至少运行：

```bash
. ./bin/activate-hermit
cargo fmt --all -- --check
cargo check --workspace --locked
cargo test --locked -p buzz-semantic-query --lib
cargo test --locked -p buzz-db --lib
cargo test --locked -p buzz-relay --lib
cargo test --locked -p buzz-admin
cargo test --locked -p carryforth-cli --lib
cargo clippy --locked \
  -p buzz-semantic-query -p buzz-db -p buzz-relay -p buzz-admin -p carryforth-cli \
  --all-targets -- -D warnings
./scripts/check-carryforth-current-product-surface.sh
./scripts/check-open-source-release-surface.sh
git diff --check
just ci
```

定向测试用于定位策略边界；`just ci` 是提交 / PR 前的最终仓库级质量门，不能用定向通过替代。

Migration integration 只允许在脚本自建的隔离 pgvector 数据库执行：

```bash
CARGO_INCREMENTAL=0 ./scripts/test-semantic-migrations.sh
```

不得把该脚本指向 Live `buzz` 数据库。

实施前后记录并比较：

```bash
git hash-object migrations/0057_project_context_semantic_foundation.sql
git hash-object migrations/0058_project_context_semantic_query.sql
```

对 Live Fleet 表执行只读证据查询，固定比较以下字段，禁止把数据库 URL 或其他 secret 写入日志：

```sql
SELECT
  community_id,
  attestation_id,
  deployment_id,
  encode(runtime_digest, 'hex') AS runtime_digest,
  encode(inventory_digest, 'hex') AS inventory_digest,
  attested_at,
  expires_at,
  revoked_at,
  revoked_by
FROM semantic_graph_http_fleet_attestations
ORDER BY community_id, transport;
```

在不执行 Fleet operator mutation 的 Live local 验收中，上述结果必须逐字段保持不变。另行比较
canonical/Event/semantic heads/active generation 计数；唯一允许的查询期 DB 变化是下节列明的派生
Provider admission 状态。

## 11. 数据安全与破坏性边界

本方案实施与验收期间禁止：

- 修改已发布 migration 0057 / 0058；
- `DROP`、`TRUNCATE`、业务 `DELETE`；
- `docker compose down -v`；
- 删除 PostgreSQL / Redis / MinIO volume；
- reset Desktop state、keyring、Community 或 managed Agent identity；
- 重建、purge 或 GC 当前 semantic generation；
- 自动清理旧 Fleet Attestation；
- 自动 query-enable；
- 把 Provider API key 写入 DB、日志、status 或证据文档。

允许的状态变化仅限：

- 显式 operator `query-enable/query-disable`；
- 正常语义查询产生的运行指标；
- 正常 Provider admission 对 `semantic_query_provider_admission.next_admission_at / updated_at`
  的预期派生限流状态更新；
- 已有查询合同允许的虚拟结果签名与返回；
- 测试隔离数据库中的临时行。

这些允许项不包含 canonical Event、semantic heads、active generation 或 Fleet Attestation 行的变更。

实施本身无需数据迁移。现有业务数据和向量索引不需要转换、复制或回填。

## 12. 切换、回滚与未来生产化

### 12.1 本地回滚

若 local policy 实现出现异常：

1. `query-disable`，先关闭 Provider 出域；
2. 将 policy 切回 `attested-fleet`；
3. 重启 Relay；
4. 需要继续查询时，重新创建当前 Fleet Attestation，再显式 query-enable；
5. 不删除 Fleet 表、向量或 canonical 数据。

### 12.2 未来切换到多实例

在出现任何以下条件前，必须把生产配置切为 `attested-fleet`：

- 第二个 Relay 实例；
- 负载均衡；
- rolling update；
- 多主机或 Kubernetes；
- 同一 Community 的 `/query` 可能路由到不同进程。

本方案仅保留当前 strict Attestation 机制，不完成多实例生产资格。因为 runtime digest 只证明二进制
合同，相同 binary 仍可分别配置 `trusted-single-relay` 与 `attested-fleet`。未来生产化前必须：

- 在每个 Relay `/_status` 中读取 policy；
- 将每个 instance 的 policy 纳入 closed routing inventory 与 inventory digest；
- 要求全部 routable instances 明确报告 `attested-fleet`；
- 使不含 policy 的旧 Attestation 在新合同下失败关闭并重新创建；
- 明确评审是否需要 bump `SEMANTIC_GRAPH_HTTP_RUNTIME_CONTRACT`；
- 完成多 Pod、LB、rolling upgrade 与 policy mismatch 资格测试。

该未来增强可以演进 JSONB inventory 而不修改 0058 表结构，但在独立方案完成前不得声称
`attested-fleet` 已证明 policy homogeneity。

安全切换顺序：

```text
query-disable
→ 部署全部目标 Relay
→ 配置 attested-fleet + deployment/instance identity
→ 从真实控制面枚举完整 routing inventory
→ fleet-attest
→ fleet-check / query-readiness
→ query-enable --acknowledge-problem-egress
```

在 `query-disable` 已生效时，没有 query-enabled Community 需要 Fleet，因此全局 `/_readiness` 可以保持
200；NIP-11 不广告，语义入口、Provider egress 和签名仍关闭。若 query gate 仍为 true 而 Attestation
无效，则 NIP-11、`/_readiness`、入口、Provider egress 和签名全部失败关闭。

### 12.3 不允许的“回滚”

- 不得把 expired 当作 ready；
- 不得把 TTL 改为无限；
- 不得通过 Host=`localhost`、debug build 或 PID 猜测自动绕过 strict mode；
- 不得恢复一个既不检查 Fleet、又对外宣称 Fleet 已认证的中间状态；
- 不得为了恢复 readiness 删除 Community query 配置或用户数据。

## 13. 完成标准

本方案只有在以下条件全部满足时才算交付：

1. 当前本地源码路径默认使用明确的 `trusted-single-relay`；
2. 本地语义查询不再依赖 Fleet Attestation 或租约续租；
3. 超过三个旧 TTL 窗口后 capability、readiness 和真实查询仍持续正常；
4. local 模式只省略 Fleet 检查，所有权限、currentness、Provider 和签名门禁都有回归证据；
5. strict 模式的现有行为保持一致，missing / expired / mismatch 继续失败关闭；同时明确记录 policy
   homogeneity 仍是未来多实例生产资格 blocker；
6. `query-readiness` 不再把 admin 子进程环境误报为 Live Relay；
7. status / admin 明确显示 policy 及 Attestation 是否适用；
8. migration 0057 / 0058 checksum 不变；
9. Fleet 表和现有记录未删除；
10. Project、Project View、Document、Meeting、Context、Event、成员、向量和 generation 数据未被清理或迁移；
11. 当前 `cf`、Desktop 和 ACP 的语义查询功能无回归；
12. 活动运维文档明确：`trusted-single-relay` 不具备多实例或生产资格；
13. 本轮不宣称生产、多 Pod、LB、rolling upgrade、policy-homogeneous Fleet 或正式发布资格。

## 14. 实施与首轮验收记录（2026-08-12）

### 14.1 已实施内容

- 新增共享 closed policy：`trusted-single-relay | attested-fleet`；未设置时默认
  `trusted-single-relay`，非法值失败关闭；
- Relay 的 NIP-11、全局 readiness、HTTP 查询入口、Provider 出域前确认和签名前确认统一使用 typed
  routing trust；local 分支只省略 Fleet 行、TTL 与 deployment/instance membership 检查；
- strict 分支保留原有 Fleet 行锁、租约、runtime digest、deployment 和 instance 检查；
- query-enable 继续以事务方式校验完整数据库 prerequisites 和显式 problem egress acknowledgement；
- `buzz-admin semantic query-readiness` 增加只读 Live `/_status` 观察，并明确区分调用者进程环境与
  Live Relay；该观察不参与 mutation、Stage B 或 Stage D 授权；
- local policy 下 `fleet-check` 明确返回 `not_required`，`fleet-attest` / `fleet-revoke` 在连接数据库前
  拒绝写入；`query-disable` 不依赖 policy 解析，继续作为错误配置下的即时 kill switch；
- 活动运维文档和 `.env.example` 已更新；未修改真实 `.env`，未新增或改写 migration。

### 14.2 自动化验证

以下验证已通过：

- `RUST_TEST_THREADS=1 just ci`；
- `CARGO_INCREMENTAL=0 ./scripts/test-semantic-migrations.sh`，使用一次性 pgvector 数据库；
- changed-crates `cargo clippy --locked ... --all-targets -- -D warnings`；
- workspace / Desktop Rust format、`git diff --check`；
- shared policy、DB、Relay、Admin 定向测试；
- Admin Live status 的 2xx、非 2xx、redirect、超限响应和 malformed JSON 回归；
- Relay consumer matrix：missing / expired / revoked Fleet 下，local 模式仍广告 capability、readiness
  为 200 且进入查询；strict 模式撤销 capability、readiness 为 503，并在 Provider-equivalent callback
  前停止；
- disposable DB 的授权真实分支：Stage B / Stage D 在 local 模式为 `Permitted`，strict 模式为
  `FleetUnavailable`；query-enable local/strict 分支均通过各自预期矩阵。

第一次并行执行普通 `just ci` 时，Desktop native `relay_admission` 的既有全局计时测试在并行调度下
停滞且没有断言失败；终止该次运行后，以 `RUST_TEST_THREADS=1 just ci` 完整重跑并通过。此记录不把
中止的并行运行计为通过。

### 14.3 Live 验收

在保留现有 PostgreSQL、Redis、MinIO volume、Desktop keyring 和 Community 数据的前提下，重新构建并
启动当前集成分支：

- Relay 报告 `fleet_policy=trusted-single-relay`、
  `fleet_attestation_required=false`、`fleet_attestation_status=not_required`；
- `/_readiness` 返回 200，semantic 分项为 ready；
- canonical `localhost:3000` Community 的 NIP-11 广告
  `buzz-project-context-semantic-query-http`；
- 使用现有 Desktop identity 和已配置 Provider 发起一条非敏感、合成 problem 的真实
  `cf project-context semantic-query`，查询成功并返回 4 个 roots、8 条 paths、25 条 canonical
  `read_commands`；
- 从返回结果选择一条 `project_view_object` read command，使用 `cf project-view get-object` 完成
  canonical 回读；
- Desktop 已由标准开发生命周期重新启动并保持运行，供人工 UI 验收。

### 14.4 数据不变证据

- migration ledger 仍为 58 条成功记录，无待执行 migration；
- 0057 SHA-256 仍为
  `ed4483984abc53496ef4658ab118b3a58a614773dd7f364cf2859631807cb59e`；
- 0058 SHA-256 仍为
  `ee15144b372c05437e34dd773438c6170bb381ab6b04ebda5e9708a73f34c755`；
- Live 查询前后，下列行数保持不变：channels 29、events 3430、meeting sessions 27、Project Context
  Edges 23、Project Documents 30、Project View objects 36、semantic embeddings 81、active generation 1、
  semantic heads 81；
- 现有 Fleet Attestation 行的 ID、attested/expires/revoked 状态、runtime digest 和 inventory digest
  前后完全不变；
- 唯一允许且观察到的数据库状态变化是 Provider admission 的派生限流时间戳；未改 canonical Event、
  semantic heads、active generation 或业务数据。

### 14.5 尚未完成的验收边界

完成标准第 3 项要求跨越三个旧 15 分钟 TTL 窗口持续运行。首轮 Live 查询已通过，但本次记录时尚未
经过完整 45 分钟，因此该长时观察仍待完成。当前结果不宣称生产、多实例、负载均衡、滚动发布或
policy-homogeneous Fleet 资格。
