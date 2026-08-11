# Carryforth 源码与本地开发面去 Buzz 收口实现计划

> 状态：方案完成，待实施
> 日期：2026-08-11
> 范围：公开源码、本地开发入口、当前 CI/示例/部署目录中的产品身份与可执行旧入口
> 基线提交：`618f031e0 feat(carryforth): prepare open-source release surface`
> 关联：[开源发布面收敛计划](open-source-release-surface-plan.md)、
> [`cf` CLI 切换计划](cli-cf-cutover-implementation-plan.md)、
> [Desktop 产品面去 Buzz 计划](desktop-product-surface-de-buzz-implementation-plan.md)、
> [Desktop 本地化方案](../local/desktop-localization-plan.md)

## 1. 结论

当前阶段不交付安装包、商店版本或生产部署。目标收敛为：外部贡献者可以从公开仓库获取源码，使用公开依赖完成
本地构建，并启动 Carryforth Desktop、Local Relay、ACP 和 `cf`；从 README、帮助文本、开发日志、CI 名称、示例和
当前可执行部署入口中，不再把 Buzz/Block 当成现行产品或远程依赖。

权威本地路径固定为：

```text
公开源码
  -> Hermit / 公开依赖
  -> just setup / just dev（仓库开发命令）
  -> localhost Relay
  -> Carryforth Desktop + ACP + cf
```

`deploy/local` 当前只接受版本固定的预构建 Relay image，因此不作为“从源码构建启动”的成功前提。它继续作为
候选本地栈保留并接受去 Buzz 门禁，但本阶段的权威验收入口是仓库开发命令。未来发布版本固定的 local-stack 时，
再单独验收 `deploy/local` 的 artifact 链。

“去 Buzz”不等于对仓库做全局字符串替换。现有 `buzz-*` crate/binary、`BUZZ_*` Relay 环境变量、Nostr kind、数据库
表、migration、bundle/keyring/app-data 坐标仍承担运行或数据兼容职责；本阶段只清理**当前产品面与可执行旧入口**。

## 2. 本阶段的交付边界

### 2.1 必须完成

1. 退役仍可拉取 Block 镜像、连接 Buzz 域名或执行旧生产部署的 Helm/Kubernetes 入口；
2. 收敛 `.github/workflows` 中仍会公开发布旧 Buzz 产品、或仍以 Buzz 命名的当前工作流；
3. 将本地开发窗口、脚本输出、日志说明、package metadata 和公开示例统一为 Carryforth / `cf`；
4. 保留旧技术坐标所需的数据与运行兼容，不因品牌清理重建数据库、Community、身份或 Desktop 状态；
5. 建立机器门禁，阻止新的 Buzz 产品名、Block 制品地址、旧远程服务或错误 `buzz` CLI 示例回流；
6. 用全新隔离环境证明公开源码可以构建并启动本地垂直切片。

### 2.2 暂不完成

- 不制作、签名或发布 Desktop 安装包；
- 不承诺生产 Kubernetes、Helm、HA、Push Gateway、Web 或 Mobile；
- 不迁移 Tauri bundle identifier、keyring service、app-data 根目录或浏览器存储 key；
- 不全局重命名 Rust crate、内部 binary、数据库、Docker volume、Nostr kind 或 capability；
- 不恢复 Buzz 远程 Community、Builderlab、Push、updater 或任何远程 fallback；
- 不处理未跟踪的 `migrations/0057_project_context_semantic_foundation.sql`；该文件必须继续独立设计、审核和提交。

## 3. 名称与兼容分类

所有命中 `Buzz`、`buzz`、`Block` 或旧坐标的内容必须先归类，再决定修改或保留。

| 类别 | 例子 | 本阶段处理 |
| --- | --- | --- |
| 当前产品面 | 窗口名、帮助、日志、README、workflow 名、Release 文案 | 必须改为 Carryforth |
| 当前可执行入口 | Helm chart、旧 GHCR/OCI 命令、Buzz 远程域名、旧 CLI 示例 | 删除或改为当前本地入口 |
| 运行兼容坐标 | `buzz-relay`、`buzz-acp`、`BUZZ_*`、Nostr capability | 允许保留；不得暴露成产品名 |
| 存储兼容坐标 | DB 表、migration、volume、bundle/keyring/app-data key | 保留，除非另有无损迁移方案 |
| 历史与归属 | `LICENSE`、`NOTICE`、`UPSTREAM.md`、历史 bug/design 文档 | 保留真实事实 |
| 负向测试 | 断言拒绝 `ghcr.io/block/*`、旧 updater 或旧域名 | 保留，并明确是拒绝样例 |

不得通过逐文件随意加例外来通过扫描。允许项必须属于上表中的稳定类别，并在门禁中以窄范围、带原因的 allowlist
表达。

## 4. 当前剩余问题

### 4.1 `deploy/charts` 仍是可执行的旧产品入口

`deploy/charts/buzz` 仍包含 Block metadata、`ghcr.io/block/buzz`、旧 OCI chart 地址与 Buzz 用户文案；
`deploy/charts/buzz-push-gateway` 还包含旧 Push image、`push.buzz.xyz` 和旧移动端 profile。它们不是单纯历史源码，
而是可以被复制执行并连接旧产品面的部署清单。

当前阶段不支持 Helm/Kubernetes，因此不对这两套 chart 做半成品改名。Git 历史已经保存上游实现，应从活动树原子退役：

- 删除 `deploy/charts/**`；
- 删除 `.github/workflows/helm-chart.yml` 与 `ct.yaml`；
- 删除 `deploy/local/build-and-deploy.sh` 与 `deploy/local/quickstart-ha-values.yaml`；
- 同提交修复 CI path filter、Justfile、脚本和公开文档中的 chart 引用。

删除仓库清单不等于操作任何现有集群，不运行 `helm uninstall`、`kubectl delete` 或 PVC 清理。

### 4.2 GitHub Actions 仍有独立旧发布面

主 release、Docker 和 canary 工作流已在上一轮收敛并保持 fail closed，但 `.github/workflows/sprig.yml` 仍可从
`main`/dispatch 更新 `sprig-latest`，也可从 `sprig-v*` 创建 GitHub Release，公开文案仍出现 Buzz 与旧 CLI。

本阶段将 Sprig 收敛为源码构建/测试工作流：

- 保留内部 `buzz-acp`、`buzz-agent`、`buzz-dev-mcp` binary 名作为技术坐标；
- 用户可见名称和说明改为 Carryforth；
- 去掉独立 GitHub Release 与可覆盖 `latest` 的发布路径；
- 如需保留构建结果，只使用短期 Actions artifact，不声明正式发行物。

`.github/workflows/benchmark-harbor.yml` 的显示名称同步改为 Carryforth；benchmark 目录名可作为兼容路径暂留。
此外，benchmark 运行脚本仍默认拉取 `ghcr.io/block/buzz:main`，leaderboard metadata 仍提交 `block/buzz`、Buzz 与
Block 产品坐标。本阶段应改为要求显式本地/Carryforth image，并把公开 metadata 指向 Carryforth；内部 Python
package、类名和 `buzz-*` sidecar 名可作为技术兼容路径暂留。

### 4.3 本地开发面仍暴露 Buzz 产品名

当前需要逐项收敛的典型位置包括：

- `scripts/instance-env.sh` 的开发窗口 `productName = "Buzz Dev"`；
- `scripts/dev-start.sh`、`dev-rebuild-start.sh`、`dev-stop.sh`、`dev-setup.sh`、`dev-reset.sh` 与
  `reset-desktop-dev-state.sh` 的用户可见说明；
- `scripts/grab-emoji.sh` 对旧 `buzz` 命令的检查与提示；
- `Justfile` 中把当前 `cf` 误称为 Buzz CLI 的 recipe 说明；
- 根 `.env.example` 中的旧产品文案；
- 根、Web 与 Admin Web 的私有 package 名；
- `scripts/build-sprig.sh` 生成的 README，以及 `crates/sprig` 的用户帮助文本；
- `deploy/local/README.md` 中对旧 Hosted/Buzz 帐号的现时表述。

内部进程名可在诊断中以 `Local Relay (buzz-relay)` 形式出现，但标题和操作指导必须使用 Carryforth。

### 4.4 本地生命周期文件需要无损过渡

当前开发脚本使用 `target/dev-lifecycle/buzz-dev.pid` 与 `buzz-dev.log`。它们虽不是产品协议，却直接出现在开发者的
本地运行面。

若实施改名，应采用：

1. 新写入只使用 `carryforth-dev.pid` / `carryforth-dev.log`；
2. 启动与停止脚本先读新坐标，未找到时再读旧坐标；
3. 识别到旧进程后只接管/停止明确匹配当前 checkout 的进程组；
4. 不因坐标改名删除日志、误杀其他 checkout 或触发 Relay/数据库重建；
5. 经过至少一个完整 start/stop/restart 周期后，再决定是否停止读取旧坐标。

### 4.5 Push Gateway 仍是活动构建目标

虽然本地 Relay 已永久关闭旧 Push 外连，`crates/buzz-push-gateway` 仍是根 workspace member，默认单元测试会构建它，
`Dockerfile.push-gateway` 也仍提供可执行镜像入口，源码配置继续绑定旧 hosted Push 产品。仅删除 Helm chart 不能把它
变成非活动历史。

本阶段应完整退役当前 Push Gateway executable：

- 从根 workspace、默认 CI 与 Justfile 中移除该 package；
- 删除 `Dockerfile.push-gateway`、当前部署 runbook 与活动构建入口；
- 删除或移出活动树中的 `crates/buzz-push-gateway` 实现，Git 历史承担来源保存；
- 保留 Relay 为读取历史 canonical event、数据库 schema 与 migration 所必需的协议数据；
- 不通过另一个 feature flag、隐藏 workflow 或 source-only Dockerfile 保留可启动旧 hosted Push 的旁路。

## 5. 目标源码拓扑

本阶段完成后，源码开发与候选本地栈应明确分开：

```text
源码开发（本阶段权威）
  . ./bin/activate-hermit
  just setup
  just dev

候选预构建本地栈（保留、但不作为本阶段完成前提）
  deploy/local/
```

部署目录收敛为：

```text
deploy/local/
  README.md
  compose.yml
  run.sh
  .env.example
  .gitignore

deploy/compose/
  README.md
  run.sh
  compose.yml（空 services）
  .env.example（仅退役说明）

deploy/charts/
  不存在
```

`deploy/local` 是唯一可以继续演进为预构建本地栈的部署入口；源码开发仍以仓库开发命令为权威。Push Gateway、
Helm/Kubernetes 和旧 Compose 不得通过 README、workflow 或脚本重新成为隐式第二入口。

旧 `deploy/compose` tombstone 必须删除 `Caddyfile`、`compose.caddy.yml` 与 `compose.dev.yml`；否则 Adminer、Prometheus、
Caddy 和旧域名片段仍可绕过 fail-closed wrapper 被直接启动。结构门禁只允许上表中的最小 tombstone 文件。

`RELAY_IMAGE` 只在未来构建版本固定的 local-stack 发布包时生成；它不是当前源码树必须存在的文件，也不属于本阶段
的交付物。

## 6. 实施阶段

### 阶段 A：冻结基线与引用图

1. 记录当前分支、tracked 文件状态和受保护 `0057` 的 SHA-256；
2. 用 `rg` 建立活动产品面、旧域名、Block registry、Helm/chart 与错误 CLI 示例清单；
3. 区分当前产品面、兼容坐标、历史归属和负向测试；
4. 记录当前 Local Relay pubkey、Community/消息/Agent 数量，以及 Project View、Document、Context、Meeting revision
   基线；本阶段不得通过清库制造 clean state。

### 阶段 B：原子退役旧部署入口

在同一阶段删除 chart、Helm workflow、`ct.yaml`、旧 local Kubernetes mesh 脚本和 Push Gateway executable，并同步
更新所有消费者：

- `.github/workflows/ci.yml` 的 path filter；
- `Justfile` 中 chart/release 路径；
- `scripts/test-project-view-release-contract.sh`；
- `docs/project-view-operations.md`、`docs/multi-tenant-relay.md` 与 `docs/push-gateway-deployment.md`；
- 根 workspace、Cargo lock、默认 CI/Justfile 中的 Push Gateway package/build 引用；
- `deploy/compose` 中 Caddy/Adminer/Prometheus 等残余可执行片段；
- 公开 README、发布说明和开源面门禁中的支持矩阵。

不得出现“chart 已删，但 CI、文档或 release bundle 仍引用”的中间提交。

### 阶段 C：收敛 workflow 与本地开发产品面

1. 把 Sprig workflow 改为只构建/测试，不再创建独立 GitHub Release；
2. 更新 workflow、benchmark 与 artifact 的用户可见 Carryforth 文案；
3. 删除 benchmark 的 Block image 默认值，要求显式本地/Carryforth image，并更新 leaderboard 公开 metadata；
4. 修改本地窗口、脚本帮助、Justfile 描述、`.env.example` 与私有 package metadata；
5. 实施生命周期 PID/log 坐标的双读单写过渡；
6. 保留内部 crate/binary/env/storage 名，不做机械替换；
7. review 每项改动，确保没有顺带改变 Relay capability、认证、Community Owner bootstrap 或 Desktop local-only 语义。

### 阶段 D：建立回归门禁

扩展现有 `scripts/check-open-source-release-surface.sh`，或新增职责单一的
`scripts/check-carryforth-current-product-surface.sh`，覆盖：

- 根 README/CONTRIBUTING/AGENTS/RELEASING 的当前操作说明；
- `.env.example`、Justfile、`scripts/` 和私有 package metadata；
- `.github/workflows` 的显示名称、公开 notes 与发布目标；
- `deploy/local` 与所有仍活动的部署入口；
- 当前用户文档中的命令、registry、域名和产品名。

门禁必须：

- 拒绝活动范围内的 `ghcr.io/block/*`、旧 chart OCI、`*.buzz.xyz`、Builderlab、旧 updater/release endpoint；
- 拒绝把 `buzz` 当成当前 CLI 的示例；
- 拒绝重新出现活动 `deploy/charts`、Helm workflow 或 local Kubernetes mesh；
- 拒绝 Push Gateway executable/Docker build 入口，以及 `deploy/compose` 额外 compose/Caddy 片段；
- 拒绝 benchmark 默认拉取 Block image 或提交旧产品 metadata；
- 允许经过分类的 crate/env/wire/storage/history/negative-test 坐标；
- 运行快速、只读，并接入 `just check`、CI 与 pre-commit 中至少一个提交前门禁。

### 阶段 E：公开源码与本地启动验收

在 clean machine/CI 或确认没有其他开发栈占用端口的独立环境中执行，不使用当前开发数据库：

1. 只使用公开网络和公开依赖完成 Hermit 激活与 locked dependency resolution；
2. `cargo metadata --locked`、根 workspace 构建、Desktop frontend/native 构建通过；
3. 使用 Desktop 固定的 canonical `localhost:3000` 启动隔离 Relay，`cf --help`、Relay readiness 和 Carryforth
   Desktop local-only 连接通过；隔离依赖独立 project/volume/database，而不是修改产品端口；
4. 首次 Owner 认领及 Project View、Document、Project Context、Meeting capability 可用；
5. 至少完成消息、managed Agent、`cf` 只读与一次 4–6 轮 Meeting smoke；
6. 屏蔽旧 Buzz/Block 域名后，上述本地链路仍正常且不发生远程 fallback；
7. 停止并重启后，身份、Community、消息、Agent 与三个 Project revision 域保持一致。

若 clean-room 需要测试 migration，必须由脚本创建并验证 scratch database 名；任何无法证明是 scratch 的连接均应
fail closed。

## 7. 文件级实施矩阵

| 范围 | 主要文件 | 预期动作 |
| --- | --- | --- |
| Helm/K8s | `deploy/charts/**`、`ct.yaml`、Helm workflow | 原子删除 |
| 旧 local mesh | `deploy/local/build-and-deploy.sh`、`quickstart-ha-values.yaml` | 删除并移除引用 |
| Push Gateway | crate、Dockerfile、workspace/CI/Justfile、部署 runbook | 退役 executable；保留历史协议数据 |
| 源码开发 | Hermit、`just setup`、`just dev`、dev scripts | 本阶段权威构建启动路径 |
| 当前 local 栈 | `deploy/local/{README,compose,run,.env.example}` | 保留候选预构建入口，补 Carryforth 文案/门禁 |
| 旧 Compose | `deploy/compose/**` | 只保留最小 fail-closed tombstone，删除额外 fragments |
| Workflow | `sprig.yml`、benchmark、CI path filter | 去旧发布面、改当前文案 |
| Benchmark | `benchmark.py`、`run_leaderboard.py`、workflow | 无 Block image 默认值；公开 metadata 改 Carryforth |
| 开发脚本 | `instance-env.sh`、`dev-*.sh`、reset/grab 脚本 | 改用户面；PID/log 无损过渡 |
| 开发命令 | `Justfile`、`.env.example` | 使用 Carryforth / `cf` 文案，保留必要技术坐标 |
| Package metadata | 根/Web/Admin Web package、Sprig help | 改私有包名与帮助文本 |
| 文档 | 当前 README/operations/deployment docs | 删除旧可执行说明；历史归属不改写 |
| Guard | OSS surface/brand/release contract scripts | 增加分类 allowlist 与结构断言 |

退役 Push Gateway executable 时，不得连带删除 Relay 读取历史事件、schema 或 migration 所需的协议数据，也不得
通过 migration 删除既有 Push 记录。源码实现的来源由 Git 历史保存，不再以可直接构建/启动的当前 target 保存历史。

## 8. 测试与验收矩阵

### 8.1 静态与构建门禁

- `git diff --check`；
- workflow YAML parse 与 `actionlint`；
- 所有修改 shell 的 `bash -n`；
- `scripts/check-open-source-release-surface.sh`；
- 新增的 current-product-surface guard；
- `cargo metadata --locked`（根 workspace 与 Desktop）；
- Desktop Biome/typecheck/build；
- 公开 Markdown 链接和命令示例检查；
- `rg` 结构断言确认 chart/Helm/local mesh 不存在且无活动引用；
- workspace/Justfile/Dockerfile 中不存在 Push Gateway executable；
- `deploy/compose` 目录只包含批准的最小 tombstone 文件；
- benchmark 不含 Block image 默认值或旧公开 submission metadata。

### 8.2 本地开发回归

- `just --list` 与脚本 `--help` 只展示 Carryforth 产品名和 `cf`；
- `instance-env.sh` 生成 `Carryforth Dev`，但 bundle/keyring/app-data 坐标不变；
- 旧 PID/log 存在时，新脚本可识别当前受管进程且不重复启动；
- 新 PID/log 写入后，stop/start/rebuild 不误杀其他 checkout；
- Relay readiness、ACP managed Agent、Desktop 与 `cf` 正常；
- 未出现 Builderlab、Buzz Push、远程 Community 或旧 updater 网络请求。

### 8.3 数据安全回归

实施前后至少回读：

- Desktop Nostr pubkey 与 Local Relay pubkey；
- Community、消息、Agent 数量；
- Project View schema/revision；
- Document catalog revision；
- Project Context revision/Edge 数量；
- Meeting 数量、active 数量与最近终态。

清理构建缓存只允许删除仓库内明确的编译增量缓存，不得触碰 Docker volume、数据库、Desktop app-data、keyring、
ACP ledger 或 managed Agent 状态。

## 9. 数据安全与破坏性边界

本计划是源码和本地开发面收敛，不是数据 migration。实施期间禁止：

- `docker compose down -v`、volume/PVC 删除；
- `DROP`、`TRUNCATE`、reset、Desktop sign-out；
- 改写旧 migration 或自动纳入未审核 `0057`；
- 为了改名重建 localhost Community、Relay signer 或 Owner 身份；
- 把旧 bundle/keyring/app-data 目录“清理”为 Carryforth 新目录；
- 用恢复 Block image、Buzz 域名或远程 fallback 作为回滚方式。

删除 Helm 源码不会触碰任何已部署集群。旧 chart 用户只能固定旧源码/tag 自行维护；本阶段不宣称提供从 Buzz Helm
到 Carryforth 的升级路径。

## 10. 提交与 review 顺序

建议使用四个独立、可审查提交：

1. `chore(carryforth): retire legacy helm deployment surface`；
2. `chore(carryforth): close remaining workflow publication surface`；
3. `refactor(carryforth): align local development product surface`；
4. `test(carryforth): guard current source and local surfaces`。

每个阶段完成后：

1. 对照本文检查实际 diff 是否越过技术/存储兼容边界；
2. 确认没有数据删除、migration 或远程 fallback；
3. 运行该阶段定向门禁；
4. 最后一阶段再做隔离 clean-room 与已有数据 readback。

若删除入口和修复消费者不能保持原子性，应合并为一个提交，而不是留下可执行文档指向不存在文件的破损中间态。

## 11. 完成标准

只有同时满足以下条件，本计划才可标记完成：

1. 陌生贡献者从公开 README 到本地启动只经过 Carryforth、Local Relay 和 `cf`；
2. 仓库不存在活动 Helm、Push Gateway executable、Push-hosted 或 Buzz 远程部署入口；
3. workflow、脚本、帮助、日志和 package metadata 不再把 Buzz 当成当前产品；
4. 旧 Buzz/Block 域名不可达时，本地核心链路仍可构建、启动和使用；
5. 兼容 `buzz-*` 命中仅存在于批准的 crate/env/wire/storage/history/negative-test 类别；
6. 门禁可以阻止旧产品面重新进入提交；
7. 现有本地身份、Community、消息、Agent、Project View、Document、Context 与 Meeting 数据回读保持一致；
8. 未跟踪 `0057` 未被本计划修改或提交；
9. 不宣称已经提供安装包、生产部署或商店发行。
