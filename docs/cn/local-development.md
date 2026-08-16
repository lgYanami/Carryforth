# Carryforth 本地源码开发

> 本文说明当前仓库支持的本地源码构建和启动流程。
> 它不是生产部署、稳定安装包或既有数据升级手册。

## 1. 适用范围

当前推荐入口是根目录的：

```bash
./start.sh
```

它适合两种情况：

- 第一次从源码构建并启动 Carryforth；
- 已经构建或运行过，希望保留现有数据并再次启动。

脚本会识别当前 checkout 已受管理的进程和已有 Docker 容器。重复执行时，健康的容器与构建缓存
可以继续复用；脚本不会为了“干净启动”删除数据库 volume、Desktop 状态或用户数据。

## 2. 外部依赖

启动前需要自行准备：

- Docker 24+；
- Docker Compose v2 插件；
- 正在运行的 Docker daemon；
- Python 3；
- `curl`，用于本地 readiness 检查；
- 当前平台所需的 Tauri 原生依赖。

Linux 上，启动脚本还会检查 `pkg-config` 以及 WebKitGTK、GTK、libsoup、ALSA
和 appindicator 等 Desktop 依赖。macOS 上会检查 Xcode Command Line Tools。

启动脚本只检查这些外部系统依赖，**不会**自动安装 Docker、Python、`curl`、系统包或
Xcode 工具。

仓库使用 [Hermit](https://cashapp.github.io/hermit/) 提供固定项目工具链。
首次使用会下载当前固定版本的 Rust、Node.js、pnpm 和 `just`；构建还会下载 Rust 与前端依赖。

## 3. 第一次启动

```bash
git clone https://github.com/lgYanami/Carryforth.git
cd Carryforth
./start.sh
```

`start.sh` 只是稳定的根目录入口，实际调用 `scripts/dev-start.sh`。
启动流程依次进行：

1. 检查 Docker、Compose、Python、`curl`、Docker daemon 和平台原生依赖；
2. 激活仓库内 Hermit 工具链；
3. 创建或更新被 Git 忽略的本地 `.env`，并设置为 `0600`；
4. 检查语义 Provider 配置；
5. 启动或恢复开发用 Docker Compose 服务，不删除 volume；
6. 等待 PostgreSQL、Redis、MinIO、Keycloak 和 Prometheus 就绪；
7. 在受管理的后台进程组中运行 `just dev`；
8. 应用待执行的前向 migration，构建 Relay、CLI 与 Desktop；
9. 等待 Relay readiness 与 Desktop 进程就绪后返回。

运行状态和日志保存在：

```text
target/dev-lifecycle/
```

默认 Relay 地址是：

```text
ws://localhost:3000
```

## 4. 语义 Provider 配置

受支持的 `./start.sh` 与 `just dev` 默认打开全部四个语义**进程开关**：

```dotenv
BUZZ_SEMANTIC_WORKER_ENABLED=true
BUZZ_SEMANTIC_GRAPH_QUERY_HTTP_AVAILABLE=true
CARRYFORTH_PROJECT_CONTEXT_COORDINATE_SEARCH_HTTP_AVAILABLE=true
CARRYFORTH_PROJECT_CONTEXT_ONE_HOP_SEMANTIC_SEARCH_HTTP_AVAILABLE=true
```

它们分别承载后台索引、完整路径型查询、自然语言 Coordinate 发现，以及两个方向的一跳检索。
Relay 只有在 canonical 数据和 coverage 等全部 readiness 校验通过后，才会广告并执行对应能力。

任一语义索引或查询进程启用时，都必须明确提供一组完整、没有默认值的 Provider 配置。
由于源码启动器默认开启全部四个语义进程，它会在缺失时询问这些值：

| 首选共享变量 | 兼容专用变量 | 交互方式 | 含义 |
|---|---|---|---|
| `LLM_API_KEY` | `BUZZ_SEMANTIC_API_KEY` | 隐藏输入 | Provider API Key |
| `LLM_BASE_URL` | `BUZZ_SEMANTIC_BASE_URL` | 明文输入 | HTTPS Provider Base URL |
| `LLM_MODEL` | `BUZZ_SEMANTIC_REQUEST_MODEL` | 明文输入 | Embedding Request Model |

两组变量不能按字段混用。只要设置了任意一个`BUZZ_SEMANTIC_*` Provider变量，Relay就把这组兼容变量作为
整体并要求其完整；否则使用完整的`LLM_*`三元组。这样保留既有部署兼容性，同时允许本地Agent与语义
Provider复用同一连接配置。

在交互终端中，脚本会询问缺失值。非交互环境中如有缺失，会直接失败并列出变量名，
不会猜测 Provider，也不会填充 URL 或模型默认值。

值只写入本地、被 Git 忽略的 `.env`。API Key 不应出现在终端回显、日志、文档、Issue 或测试夹具中。

如完全不使用语义能力，在 `.env` 中保持四个进程开关全部关闭：

```dotenv
BUZZ_SEMANTIC_WORKER_ENABLED=false
BUZZ_SEMANTIC_GRAPH_QUERY_HTTP_AVAILABLE=false
CARRYFORTH_PROJECT_CONTEXT_COORDINATE_SEARCH_HTTP_AVAILABLE=false
CARRYFORTH_PROJECT_CONTEXT_ONE_HOP_SEMANTIC_SEARCH_HTTP_AVAILABLE=false
```

### 4.1 受支持的本地语义 bootstrap

运行 `./start.sh`（或 `just dev`）即表示本地 operator 授权已配置的 Provider 接收当前批准的语义
输入：problem/query 文本，以及来源类型、当前可见标题/名称和可选摘要。当前 foundation 不发送
Document 正文或 chunk。

启动过程只针对 loopback `RELAY_URL` 精确解析出的那个 Community：

1. 复用兼容的 active generation；没有时才创建；
2. 开启持久 semantic-index gate，并执行可恢复的 canonical scan；
3. 启动 Relay Worker，等待 generation 达到精确 coverage；
4. 激活 generation，并预先开启持久 query gate；
5. 从 live Relay 验证 Worker 与三个 HTTP surface 确实已开启。

bootstrap 命令会拒绝非 loopback 的 Relay、bind 或数据库坐标，也会拒绝 multi-Relay fleet policy；
这些环境必须使用正常 operator 流程。
重复启动是幂等的，不会扫描或修改其他 Community。直接运行原始 Relay 或生产启动仍保持 capability-off；
这些环境应继续遵循[语义 pgvector 运维](../semantic-pgvector-operations.md)中的正式部署合同。
Worker drain 默认等待 600 秒，可在 `.env` 中用
`BUZZ_LOCAL_SEMANTIC_BOOTSTRAP_TIMEOUT_SECONDS` 调整为 1–3600 秒；超时会让启动失败，而不是把尚未
完整初始化的语义栈报告为 ready。

启动过程**不会**代替 Human Owner 创建、决定或签署 Project View / Project Context。若 canonical
状态尚未初始化，query gate 只处于预开启状态；现有授权 SQL 仍会拒绝检索，Relay 也不会广告能力。
Owner 完成初始化后，已经运行的 Worker 会索引 eligible 状态，并在 coverage 完整时通过正常
readiness fence 开放能力。

将四个开关全部显式设为 `false` 会退出并跳过本地语义 bootstrap。部分开启属于高级/手工模式：
指定进程仍会启动，但不会自动执行完整能力的 Community bootstrap。

## 5. `just start`、`just dev` 与 `./start.sh`

`Justfile` 是仓库的任务入口，记录构建、检查、测试、启动和停止等 recipe。

如果当前 shell 已经激活 Hermit，下面的命令与根目录入口等价：

```bash
. ./bin/activate-hermit
just start
```

两者最终都会调用 `./start.sh`。对于新用户，直接使用 `./start.sh` 更稳妥，因为它会自行激活 Hermit。

需要在前台观察构建和服务输出时，可以使用贡献者流程：

```bash
. ./bin/activate-hermit
just dev
```

`just dev` 不是独立安装器；它仍使用相同 `.env`、Docker 服务、migration 和本地 Relay 坐标。

## 6. 重建与停止

```bash
./start.sh                      # 构建或复用构建，后台启动
./scripts/dev-rebuild-start.sh  # 清理 Carryforth 可执行产物后重新构建并启动
./scripts/dev-stop.sh           # 停止应用与 Compose 容器，保留数据
```

`dev-rebuild-start.sh` 只清理 Carryforth 自己的可执行程序构建产物，保留依赖缓存与 Docker 数据。
它适合排除旧二进制或增量构建错配，不等同于清空整个 Cargo / pnpm 缓存。

仅停止应用而保持 Docker 容器运行：

```bash
./scripts/dev-stop.sh --app-only
```

## 7. 本地服务与开发边界

源码开发 Compose 会运行 PostgreSQL / pgvector、Redis、MinIO、Keycloak、Prometheus 和 Adminer
等依赖。它使用开发配置，并会向宿主机发布多个端口；它不是生产加固部署。

仓库中的 `.env.example` 与 Relay 原始默认值都会把 Relay 绑定到 `127.0.0.1`。日常源码开发
应保留这一 loopback 边界：本地认证与 membership gate 较宽松，Compose 依赖也使用开发凭据；
仓库中的 Compose 文件会把这些宿主端口绑定到 loopback。
这仍不是生产加固配置，因此只能在可信本机运行；不要修改绑定后直接暴露到局域网或 Internet。

Desktop 使用本地 Relay，不会在 Relay 不可用时回退到旧 hosted 服务。
但“本地优先”不等于“完全离线”：Provider、工具链与依赖下载、远程媒体和外部链接仍可能访问网络。

## 8. 数据保护

- 不要对含有重要数据的环境使用 destructive reset；
- 不要执行 `docker compose down -v` 或删除 Carryforth 开发 volume；
- 不要手工重写已经执行的 migration；
- migration、OOM、故障注入和破坏性集成测试必须使用单独的 scratch 数据库与 volume；
- 仓库中的开发 Compose 栈是整台机器共享的单例：project、container、端口、network 和 volume
  都是固定坐标，不能从另一个 checkout 并行运行。应用进程的停止逻辑会核对 checkout，但普通
  `dev-stop.sh` 也会停止这些共享 Compose 依赖；需要时使用 `--app-only`；
- `.env`、私钥和 Provider 凭据不得提交到 Git。

## 9. 常用检查

修改代码后，按范围运行仓库 recipe：

```bash
. ./bin/activate-hermit
just test-unit
just desktop-check
just desktop-test
just desktop-tauri-check
just desktop-tauri-test
```

完整本地 PR gate：

```bash
just ci
```

更多贡献和测试要求见 [CONTRIBUTING.md](../../CONTRIBUTING.md) 与 [TESTING.md](../../TESTING.md)。
