# Project Context 统一语义计算资格记录

> 状态：U0 已通过；U1–U7 待交付
>
> 日期：2026-08-16
>
> 实现计划：
> [Project Context 统一语义计算实现计划](project-context-unified-semantic-computation-implementation-plan.md)

## 1. 阶段状态

| 阶段 | 状态 | 结论 |
| --- | --- | --- |
| U0 设计与差分门 | 通过 | 历史 v1 oracle 与独立 Phase 1 gate 可在同一工作树运行 |
| U1 共同 input/fence/vector | 待执行 | — |
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

U0 只建立历史 oracle、差分合同和路径保护，不代表共同 production types、encoder 或 exact scorer 已经交付。
真实 Provider canary 仍缺少受支持的 `BUZZ_SEMANTIC_*` 配置，未在 U0 运行；不得挪用 `LLM_*`。
