# Project Context 图语义化基础资格报告

> 状态：本地自动化资格通过；生产灰度待 Operator 确认
>
> 日期：2026-08-11
>
> 实现分支：`feat/context-semantic`
>
> 规范：
> [Project Context 图语义化基础规范](./project-context-graph-semantic-foundation-spec.md)
>
> 阶段计划：
> [Project Context 图语义化基础分阶段开发计划](./project-context-graph-semantic-foundation-implementation-plan.md)

## 1. 资格结论

图语义化 foundation 已满足进入受控生产灰度前的本地实现门：

- PostgreSQL 17 + pgvector 0.8.5 能存取 2048 维向量并执行 cosine 运算；
- `overview-v1` 只提取来源类型、title/name 与 source-owned optional summary；
- Project View、Project Document 与 Meeting 由 canonical writer 数据生成 typed observation；
- 来源事务先同步撤销旧 head 的 current 资格，再异步生成新 embedding；
- worker 通过 lease、invalidation epoch、typed basis 与 snapshot digest 防止陈旧任务激活；
- generation 的 unit set 与 embedding 只能作为一个完整集合原子切换；
- disabled Community 不会被 worker claim，且 currentness capture 不停止；
- Volcengine 外部边界只允许 `overview`，`content_chunk` 在网络调用前被拒绝；
- Edge、Coordinate、Project Context revision 与 canonical source ownership 均未改变；
- 没有公开 semantic query 或正文切片能力。

这份报告不等于生产发布批准。数据库扩展安装、真实 volume 演练、首个 Community 数据出域授权、provider
quota 与成本观察仍由部署 Operator 负责。

## 2. 冻结合同

```text
provider:                        Volcengine Ark
request model alias:             doubao-embedding-vision
resolved model:                  doubao-embedding-vision-251215
dimensions:                      2048
distance metric:                 cosine
client normalization:            none
extractor:                       overview-v1
allowed external content:        source type + title/name + optional summary
disallowed external content:     body, Board, Speech, topology, Role/Work lens, chunks
```

API key、endpoint credential、原始输入和原始向量均未写入源码、文档、fixture或日志。worker 只读取专用
`BUZZ_SEMANTIC_*` 配置，不会回退到通用 `LLM_*` 环境变量。

## 3. 自动化证据

以下命令已在仓库 Hermit 环境中通过：

```bash
cargo check -p buzz-semantic -p buzz-db -p buzz-admin -p buzz-relay
cargo test -p buzz-semantic
cargo test -p buzz-relay semantic_runtime::tests
cargo clippy -p buzz-semantic -p buzz-db -p buzz-admin -p buzz-relay \
  --all-targets -- -D warnings
just semantic-test
```

`just semantic-test` 覆盖：

- 隔离 PostgreSQL 17.10 容器；
- pgvector 0.8.5 extension / `vector` / `halfvec`；
- cosine distance 与 2048 维 SQLx bind/decode；
- `0056 → 0057` brownfield migration；
- ledger-less `schema/schema.sql` fresh apply；
- migration 与 desired schema 漂移；
- source invalidation、job lease、fake encoder、完整 set CAS；
- 8 维与 12 维测试 generation 共存，证明模型维度隔离；
- durable rebuild 完成 fence；
- generation ready、activate、rollback-ready、retire 与 purge；
- status-only semantic text 复用；
- tombstone 旧 head 失效；
- 多 worker 共享 provider rate slot。

本地长期使用的 `buzz-postgres-data` volume 最初由不含 pgvector 的镜像创建。切换到固定 pgvector 镜像后，
初始化脚本按 PostgreSQL 语义不会在旧 volume 上重跑。本次按 brownfield runbook 在保留该 volume 的前提下
显式执行 `CREATE EXTENSION IF NOT EXISTS vector`，确认版本为 `0.8.5`，随后 `0057` 迁移与真实 DB
semantic pipeline 测试通过。这证明旧 volume 不需要清空，但不能省略 Operator 的 extension 安装步骤。

仓库级 `just ci` 已完成 root workspace、Desktop / Web / Mobile lint、root unit、Desktop 3643 项前端测试、
Desktop production build 与绝大多数 Tauri 测试，但没有形成全绿结果。当前分支基线的 Carryforth Desktop
已把 `relay_api_base_url_with_override()` 固定到本地 Relay，两个旧测试仍尝试通过
`relay_url_override` 指向临时 HTTP server，因此稳定得到 `relay unreachable`：

```text
relay_admission::tests::gate_armed_by_one_path_withholds_another_path
relay_admission::tests::http_429_withholds_next_relay_command_until_expiry_then_resumes
```

同一测试进程随后有两个依赖旧 override 行为的 Community-switch capture 测试等待不结束。本次交付对
`desktop/` 没有任何 diff，因此没有在 semantic foundation 范围内改写这组 Carryforth 测试。该仓库基线
问题需要单独修复，不能把本报告解释成仓库级 `just ci` 全绿。

需要常驻基础设施的 `just test` 已完成两次：迁移、seed、`buzz-db` 真实 PostgreSQL 测试以及本次新增的
semantic pipeline 均通过；31 个测试组通过。最终 workspace integration 聚合组中的既有
`buzz-agent/tests/fake_llm.rs::steer_folds_into_active_turn_without_cancelling` 在并发全量运行时发生时序
失败，单独连续重跑三次均通过。本次交付没有修改 `buzz-agent` 或该测试，因此按 flaky 基线记录，不能
把本报告解释成仓库级 `just test` 全绿。

## 4. Fail-closed 证据

| 场景 | 结果 |
|---|---|
| source 在 encode 中途更新 | 旧任务 activation CAS 失败，不能恢复旧 head |
| source 删除或变为 ineligible | 旧 head 在 canonical 事务内立即撤销；物理向量稍后 GC |
| provider timeout / 429 / 错误向量 | job 有界 retry / poison；canonical source 写入不回滚 |
| provider 返回错误模型或维度 | worker 拒绝激活 |
| Community disabled | 不 claim / 不外发；source fence 继续推进 |
| rebuild 未完整扫描三类来源 | generation 不能 ready / activate |
| generation 覆盖不完整 | verify 与 cutover 拒绝 |
| rollback generation 已陈旧 | rollback/activate 前 coverage verify 拒绝 |
| `content_chunk` 进入 external adapter | 网络调用前拒绝 |
| semantic schema 或 pgvector 未就绪 | worker 不启动；readiness 对已启用 Community fail closed |

## 5. 仍待生产 Operator 完成

1. 在目标 writer PostgreSQL 使用特权通道安装并确认 pgvector 0.8.5；
2. 对真实现有 volume 演练镜像切换、备份恢复与旧镜像回退；
3. 关闭首次自动 migration，按 runbook 单次执行 preflight 与 migration；
4. 选择一个非关键、明确授权 title/summary 出域的灰度 Community；
5. 使用 2048 维 Volcengine generation 完成 durable rebuild、drain、verify 与 activate；
6. 观察 queue age、429、延迟、provider 成本、CAS superseded、WAL 与存储；
7. 验证 disable、reconcile、generation rollback 与 purge runbook；
8. 完成 Operator / 数据负责人签字后，再扩大 Community 范围。

在完成上述步骤前，推荐保持所有生产 Community 的 `semantic_index_enabled = false`。
