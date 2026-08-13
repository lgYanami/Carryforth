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
- 当前平台所需的 Tauri 原生依赖。

Linux 上，启动脚本还会检查 `pkg-config` 以及 WebKitGTK、GTK、libsoup、ALSA
和 appindicator 等 Desktop 依赖。macOS 上会检查 Xcode Command Line Tools。

启动脚本只检查这些外部系统依赖，**不会**自动安装 Docker、Python、系统包或 Xcode 工具。

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

1. 检查 Docker、Compose、Python、Docker daemon 和平台原生依赖；
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

本地源码启动默认打开以下两个**进程开关**：

```dotenv
BUZZ_SEMANTIC_WORKER_ENABLED=true
BUZZ_SEMANTIC_GRAPH_QUERY_HTTP_AVAILABLE=true
```

任一开关启用时，必须明确提供三个没有默认值的 Provider 配置：

| 环境变量 | 交互方式 | 含义 |
|---|---|---|
| `BUZZ_SEMANTIC_API_KEY` | 隐藏输入 | Provider API Key |
| `BUZZ_SEMANTIC_BASE_URL` | 明文输入 | HTTPS Provider Base URL |
| `BUZZ_SEMANTIC_REQUEST_MODEL` | 明文输入 | Embedding Request Model |

在交互终端中，脚本会询问缺失值。非交互环境中如有缺失，会直接失败并列出变量名，
不会猜测 Provider，也不会填充 URL 或模型默认值。

值只写入本地、被 Git 忽略的 `.env`。API Key 不应出现在终端回显、日志、文档、Issue 或测试夹具中。

如完全不使用语义能力，可先在 `.env` 中同时关闭两个进程开关：

```dotenv
BUZZ_SEMANTIC_WORKER_ENABLED=false
BUZZ_SEMANTIC_GRAPH_QUERY_HTTP_AVAILABLE=false
```

### 4.1 进程开关不等于 Community 授权

启动 Worker 与 Query HTTP handler 只表示本机进程具备承载能力。它不会自动完成：

- Project View / Project Context 初始化；
- Community 的持久语义索引 gate；
- generation 创建、构建、验证和激活；
- Community 的持久语义查询 gate；
- 将 problem / overview 发往外部 Provider 的出境确认。

这些步骤必须由 operator 和 Community owner 按当前运维合同显式完成。
详见[语义 pgvector 运维](../semantic-pgvector-operations.md)。

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

Desktop 使用本地 Relay，不会在 Relay 不可用时回退到旧 hosted 服务。
但“本地优先”不等于“完全离线”：Provider、工具链与依赖下载、远程媒体和外部链接仍可能访问网络。

## 8. 数据保护

- 不要对含有重要数据的环境使用 destructive reset；
- 不要执行 `docker compose down -v` 或删除 Carryforth 开发 volume；
- 不要手工重写已经执行的 migration；
- migration、OOM、故障注入和破坏性集成测试必须使用单独的 scratch 数据库与 volume；
- 停止或重建应用前，确认目标进程属于当前 checkout，避免影响并行开发实例；
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
