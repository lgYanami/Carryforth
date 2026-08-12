# Carryforth 实验性 Benchmark 源码退役决策

> 状态：已实施，自动化验收通过，待提交
> 日期：2026-08-12
> 范围：`benchmarks/` Harbor orchestra/testbed、独立 CI workflow、开发命令与当前文档接线
> 关联：[开源发布面收敛计划](open-source-release-surface-plan.md)、
> [源码与本地开发面收口计划](source-and-local-development-surface-de-buzz-closure-implementation-plan.md)

## 1. 决策

Carryforth 当前开源产品面由 Desktop、Local Relay、ACP 和 `cf` 构成。继承的 Harbor orchestra benchmark 是一套
独立的实验评测环境，不是这些组件的运行、构建或发行依赖，也不属于当前维护目标。

因此，从活动树完整退役 `benchmarks/`，并同步删除其独立 workflow、Justfile 命令、Python package metadata
检查和当前产品文档入口。根 `benchmarks/` 加入退役路径门禁，防止未经独立决策的实验环境重新进入公开源码面。

## 2. 事实基线

退役前审计确认：

- `benchmarks/` 有 40 个跟踪文件，约 1.19 MB；
- 两份 Python lockfile 合计约 1 MB，是该目录绝大部分跟踪体积；
- 本地目录约 768 MB，差额来自可再生成的 Python 虚拟环境、pytest/ruff cache 和字节码；
- 根 Cargo workspace、pnpm workspace、Desktop、Relay、ACP 和 `cf` 都不依赖该 Python package；
- benchmark 使用独立 GitHub Actions workflow 和独立 Compose testbed；
- 默认 `just ci` 不运行 benchmark，但仓库仍维护其依赖、测试、容器和公开 metadata 合同。

## 3. 实施边界

本次删除：

- 完整 `benchmarks/` 跟踪树及该路径下忽略型生成缓存；
- `.github/workflows/benchmark-harbor.yml`；
- Justfile 的 `benchmark` 和 `benchmark-down` recipes；
- current-product、open-source surface、`cf` cutover 和 package metadata 门禁中的活动 benchmark 接线；
- README、AGENTS、RELEASING 和当前开源范围计划中把 benchmark 描述为保留源码面的表述。

本次不删除：

- Desktop 内用于性能回归的 Playwright `.perf.ts` 场景；
- Relay 测试客户端中的负载生成器或普通 Rust benchmark 辅助代码；
- RFC 中名为 benchmarking 的保留网络地址判断；
- 历史实施记录中对 Harbor benchmark 当时状态和测试结果的真实记载；
- Web、Admin Web、Desktop、Relay、ACP、`cf` 或任何数据库数据。

## 4. 后果与恢复

公开仓库不再携带 Harbor/Terminal-Bench 评测适配器，也不再提供 `just benchmark`。这会减少 Python 依赖、独立
workflow、Compose 环境、锁文件和外部评测平台的维护面。

如果未来需要正式的 Carryforth Agent 评测体系，应从审核过的 Git 历史或新的设计重新建立，明确指标、数据集授权、
模型供应商边界、可复现容器、成本与安全门禁；不得仅复制旧目录并绕过退役路径检查。

## 5. 验收条件

1. `benchmarks/` 和 benchmark workflow 不存在；
2. Justfile、CI、当前入口文档和检查脚本不再引用已删除路径；
3. 根 `benchmarks/` 被 current-product 退役门禁覆盖；
4. public package metadata 不再要求已删除的 Python packages；
5. `docs/lora/**` 的活动门禁引用同时迁移到真实的 `docs/**` 路径；
6. Project View/Document 文档合同恢复通过；
7. current-product、open-source surface、`cf` cutover、素材和 package metadata 门禁通过；
8. Cargo/pnpm workspace 图不因本次删除发生非预期变化；
9. `git diff --check` 和完整 `just ci` 通过，或明确记录与本次无关的既有阻断。

## 6. 实施与验收结果

2026-08-12 已完成本决策的实施：删除 40 个 `benchmarks/` 跟踪文件、独立 benchmark workflow、
两项 Justfile recipe，以及 package metadata、CI 和当前产品文档中的活动接线。根 `benchmarks/`
已加入 current-product 退役路径门禁。

文档目录此前被收口时，58 个仍由 Rust 和 Desktop 测试通过 `include_str!` 编译使用的协议 fixture
也随之消失。本次没有恢复已退役的 NIP 文档，而是将这些测试数据迁移到
`crates/buzz-sdk/tests/fixtures/`，并更新所有消费者路径。Project Context、Project Document 和
Desktop 相关测试均从新位置读取并通过。

本地忽略型 Python 虚拟环境、pytest/ruff cache 和字节码已删除，释放约 768 MB；这些缓存不可从
Git 恢复，但可由工具重新生成。被删除的跟踪源码仍可从当前私有 Git 历史恢复。

验收结果：

- Project View/Document 文档合同、current-product、open-source surface、`cf` cutover、素材清单和
  public package metadata 门禁通过；
- 根与 Desktop Cargo workspace 的格式检查、Clippy、单元测试和构建通过；
- Desktop 与 Web 检查、测试和构建通过；
- `git diff --check` 通过；
- 完整 `just ci` 以退出码 0 通过。

发布素材清单仍列出 8 项既有发布阻断，它们不由本次源码退役引入，也不影响上述 CI 通过结论。
