# Project Context 统一语义计算资格记录

> 状态：U0–U1 已通过；U2–U7 待交付
>
> 日期：2026-08-16
>
> 实现计划：
> [Project Context 统一语义计算实现计划](project-context-unified-semantic-computation-implementation-plan.md)

## 1. 阶段状态

| 阶段 | 状态 | 结论 |
| --- | --- | --- |
| U0 设计与差分门 | 通过 | 历史 v1 oracle 与独立 Phase 1 gate 可在同一工作树运行 |
| U1 共同 input/fence/vector | 通过 | 三种 closed input 与两类旧 wrapper 已委托共同类型；exact generation 只由 writer DB ticket 绑定 |
| U2 共同 Provider encoder | 待执行 | — |
| U3 one-hop tagged family | 待执行 | — |
| U4 whole-graph Coordinate | 待执行 | — |
| U5 bounded complete path | 待执行 | — |
| U6 默认切换与 legacy 收口 | 待执行 | — |
| U7 最终资格与文档关闭 | 待执行 | — |

## 2. U0 证据

U0 保留了冻结的历史 manifest 及其 SHA-256：

~~~text
e7b18cdba9c40fa941a6a70fd8beb2629ecc4232dcc5d94316edbaf4fdae097e
~~~

历史 runner 只复核 `e8f26d6e65..ab395ff6f` 的冻结交付，不会把后续 Phase 1 生产代码误判成
历史交付漂移；原有 production freeze allowlist 没有扩大。

新增的 Phase 1 differential fixture SHA-256 为：

~~~text
52a062ad800ae0f8503fc9748a4be45e487fb964b551977c41666ccc3591ab19
~~~

它冻结：

- 四个逻辑 operation / 三个 surface；
- 当前 ordered input/vector bundle；
- legacy/new 必须消费同一次 Provider encoding 与同一 RR snapshot；
- 每个 operation 的 typed normalized result 与 closed error comparator；
- production 默认 route 仍是 legacy，request 内 mismatch 不 fallback。

本阶段实际通过：

~~~text
SEMANTIC_COMPATIBILITY_ENFORCE_FREEZE_DIFF=1 \
  ./scripts/check-semantic-retrieval-compatibility-baseline.sh manifest-only
./scripts/check-semantic-retrieval-computation.sh deterministic
./scripts/check-semantic-retrieval-computation.sh all
cargo fmt --all -- --check
git diff --check
~~~

完整 Phase 1 gate 已覆盖 `buzz-semantic-query`、`buzz-db` 与 `buzz-relay` 的相关定向测试和 production
`cargo check`。U0 没有修改 wire、Event kind、SDK、CLI、schema、migration、semantic index 或 canonical
Project Context 图。

## 3. 资格边界

### 3.1 U1 实现结论

U1 已交付以下 production 类型边界：

- `SemanticQueryInput` / `SemanticQueryInputBundle` 统一承载 Coordinate、Q0、Qi 的 exact UTF-8、input
  digest、operation contract digest 与有序 batch identity；各 closed builder 仍生成原有字节，不允许跨合同
  重新解释；
- `SemanticModelSpaceFences` 只描述 generation contract/model/dimensions 可比较空间，不携带 operation
  template，也不伪造 active generation identity；
- `ProviderEncodedSemanticInput` / bundle 绑定 Provider 输出与 exact input/model space，但不携带 generation
  UUID；
- `SemanticGenerationKey` 使用 host-derived `CommunityId + generation UUID`，由 writer DB ticket 创建
  `GenerationBoundQueryVector`；相同 generation UUID 出现在不同 Community 时仍 fail closed；
- 旧 `EncodedCoordinateSearchQuery`、`EncodedSemanticQuery`、`SemanticCoordinateSearchVector` 与
  `SemanticExactQueryVector` 保持为兼容 wrapper。Coordinate 仍使用独立 template/digest，Q0/Qi 仍使用
  graph-query template/digest；
- Coordinate 与 graph operation-facing DB adapter 都会再次验证 closed input kind、operation contract、
  model space、tenant/generation 与 vector shape，授权与 currentness 没有移入通用数学类型。

U1 没有修改 Provider transport、调用批次、admission、retry、permit、RR snapshot、release、ranking、scope、
wire、Event kind、SDK、CLI、schema、migration、semantic index 或 canonical Project Context 图。

### 3.2 U1 执行证据

以下门在 U1 最终工作树通过：

~~~text
cargo test -p buzz-semantic-query --lib                         # 47 passed
cargo test -p buzz-semantic-query --test compatibility_baseline # 1 passed
cargo test -p buzz-semantic-query --test computation_differential # 2 passed
cargo test -p buzz-db --lib semantic_                            # 31 passed, 4 ignored
cargo test -p buzz-db --lib coordinate_search                    # 3 passed, 1 ignored
cargo test -p buzz-db --lib one_hop                              # 2 passed
cargo test -p buzz-relay --lib semantic_                         # 72 passed
cargo test -p buzz-relay --lib coordinate_search                 # 4 passed
cargo test -p buzz-relay --lib one_hop                           # 6 passed
cargo clippy -p buzz-semantic-query -p buzz-db -p buzz-relay --all-targets -- -D warnings
./scripts/check-semantic-retrieval-computation.sh all
git diff --check
~~~

历史 manifest 与 Phase 1 differential manifest 均未改写；aggregate computation gate 继续输出
`semantic retrieval computation gate passed (all)`。

### 3.3 尚未宣称的能力

U1 只完成共同 production types 与 DB generation binder；共同 Provider encoder 与共同 exact scorer 尚未
切换 production operation。它不代表可靠性 runtime、资源治理或 production SLO 已交付。

真实 Provider canary 仍缺少受支持的 `BUZZ_SEMANTIC_*` 配置，未在 U0–U1 运行；不得挪用 `LLM_*`。
