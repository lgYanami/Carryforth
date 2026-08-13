# Carryforth 当前状态与能力边界

> 本文区分代码已经实现、当前本地流程可以评估、需要显式启用、仍在资格化和尚未承诺的能力。
> 分支名、代码目录或历史 tag 都不能单独作为正式发布证据。

## 1. 总体结论

Carryforth 正在准备第一次独立开源发行，仍处于活跃开发阶段。当前仓库仅供：

- 从源码进行本地构建和开发；
- 在本地单 Relay 环境评估核心协作；
- 开发和验证 Desktop、Relay、ACP、`cf` 及项目领域能力；
- 参考学习本地优先、Nostr Relay 与 Human—Agent 项目协作的实现。

当前仓库尚未承诺：

- 稳定的二进制安装包；
- 一条命令安装全新机器的所有系统依赖；
- 生产部署或多实例高可用；
- 既有数据的稳定长期升级路径；
- macOS / Windows 的正式支持矩阵；
- 完全离线运行。

## 2. 当前计划支持面

第一次独立发行准备聚焦：

- Linux x86_64 Carryforth Desktop；
- 本地单 Relay 及持久依赖；
- ACP Managed Agents 与 sidecar；
- Linux x86_64 `cf` CLI；
- Channels、Messages、Project View、Documents、Project Context 和 Meetings。

Relay OCI 的 Linux amd64 / arm64 发布仍属于发行准备工作。Desktop community artifact 的签名、
安装、自动更新和升级体验尚未形成稳定承诺。

`web/` 当前只是源码中的浏览器客户端，不代表正式发布或支持承诺。
继承而来的 Mobile、Harbor benchmark、Helm / Kubernetes 和旧 Hosted Push
已退出当前活动发布面。

## 3. 开箱即用与显式启用

### 3.1 启动后可建立的基础

`./start.sh` 可以检查本地依赖、准备私有 `.env`、启动开发 Compose、执行 migration、
构建 Relay / CLI / Desktop 并连接本地 Relay。

它建立的是源码开发运行基础，不会自动替 owner、operator 或用户作出领域授权。

### 3.2 Desktop 预览开关

以下界面当前默认隐藏，需要在 Settings → Experiments 中显式开启：

- Projects；
- Project View；
- Documents 与 Project Context（跟随 Project View 预览面）；
- Meetings。

打开预览开关只显示界面。Relay capability、Community durable gate、初始化状态和成员权限
仍然必须独立满足。

### 3.3 Community 初始化

新 Community 不会因进程启动自动获得完整项目模型：

- Project View v3 需要 operator 执行准备步骤；
- owner 必须审核并签署初始化命令；
- Documents、Context、Meetings 和 semantic 各有 readiness / enable 合同；
- 稳定 Relay signer 是多项能力的前置；
- 禁用或未准备时应失败关闭，而不是由客户端伪造能力。

## 4. 能力状态

### 4.1 Channels、Messages 与身份

状态：**核心本地能力**。

- Human 与 Agent 使用稳定 Nostr 身份；
- 消息与 Relay 操作保持签名；
- Community membership 和权限继续生效；
- Desktop 不会在本地 Relay 不可用时回退旧 hosted 服务。

这不表示系统匿名、端到端加密或已经取得合规认证。

### 4.2 Managed Agents 与 `cf`

状态：**本地核心能力，外部 Runtime / Provider 需另行准备**。

- `cf` 提供面向 Agent 的签名 Relay 操作；
- ACP 支持 Agent 池、会话和并发调度；
- Desktop 可发现和管理 Built-in、Goose、Claude Code、Codex 等 Runtime。

Built-in Agent 仍需要受支持的模型 Provider 和凭据；外部 Runtime 需要相应 CLI / adapter。
远程 backend Provider 不能被视为当前稳定核心产品。

### 4.3 Project View 与 Role Continuity

状态：**已实现，默认预览，需初始化**。

- 支持 Project Profile、Goal、Role、Plan、Stage、Requirement、Issue、Work、Resource；
- Relay 验证签名、Revision 和关系合同；
- 支持角色提议、Assignment、Work commitment、Checkpoint 与 Handoff。

新 Community 需要 operator prepare 和 owner 签名初始化，数据库能力默认不会由 `start.sh` 打开。

### 4.4 Project Documents

状态：**已实现，默认预览，需 capability 准备**。

- 稳定文档身份；
- 不可变完整 Revision 与 current Revision；
- 历史读取、固定 Revision、tombstone；
- 乐观并发与冲突时草稿保护。

Document 是项目记录，不是 secret store。它依赖已就绪的 Project View v3 和稳定 signer。

### 4.5 Project Context

状态：**已实现，默认预览，读写面不对称**。

- 跨 Project View、Document 和 Meeting 的经验证 Edge / Hyperedge；
- exact、incident、contains-all 查询；
- Desktop 图形画布、Inspector 与实时更新；
- `cf` / Agent 维护 Edge 关系。

Desktop 当前主要是可信只读关系面；规范 attach / detach 主要通过 CLI。
Meeting 只有在符合生命周期和 Action Finalization 条件时才能创建新的 Context binding。

### 4.6 图语义路径查询

状态：**实验性、可选、受门控，尚未生产就绪**。

已经实现：

- 语义 Provider 适配与 pgvector 索引；
- 自然语言 problem、可选起始坐标与查询上下文；
- 图根和路径返回；
- NIP-98 请求绑定、Relay 签名结果和 canonical read command；
- 本地单 Relay 的受控真实请求链路。

仍需资格化：

- 不同 Role / Work context 下的相关性与排序质量；
- known-negative 与 relevance floor 校准；
- PostgreSQL 资源隔离、并发阶梯和长期 soak；
- production LB / multi-pod；
- 目标规模、冻结 SLO 和完整故障恢复证据。

源码启动默认打开 Worker / Query HTTP 进程开关，但 Community index/query gate 默认不由此开启。
启用查询还必须显式确认 problem / overview 会发送给用户配置的 Provider。

### 4.7 Meetings

状态：**预览能力，多个进程与 durable gate 默认关闭**。

已实现 V2 roster、Board、Floor、发言时间线、handoff、主持人决定、租约 / timeout、
关闭 / 中止与 Action Finalization，并支持 Human 和 Agent 共同参与。

Meeting 创建、V2 direct actions 和 Community read 都有独立开关与批准流程。
默认可见性不能仅靠客户端扩大；完整运行矩阵仍在资格化。

### 4.8 Git Projects

状态：**预览版 Git 协作工作区**。

包括仓库发现、README / 源码浏览、分支、标签、提交与 diff、Issue、PR、行内评论、审批、
合并和冲突恢复，并包含 Git smart HTTP 与对象存储支持。

它不是成熟的 GitHub 替代品；Desktop 默认隐藏，本地操作还要求系统 `git` 可用。

### 4.9 媒体

状态：**本地可评估，仍有格式和资源边界**。

Relay 提供内容寻址上传与 MinIO 存储。Desktop 支持图片、视频和普通文件附件，
包括拖放 / 粘贴、图片处理、视频转码、poster 和 Range 流式播放。

限制包括：

- 视频和 HEIC 处理依赖系统 `ffmpeg`，根启动脚本不安装或检查它；
- 音频当前被拒绝；
- PDF 作为下载附件，不提供内联预览；
- 尚无持久化的每用户总存储配额；
- 大型视频处理不能从 Relay 单文件上限推导出 Desktop 的安全内存保证。

## 5. 发布准备边界

正式发行门当前仍会失败关闭。未完成项包括但不限于：

- 第三方字体和依赖许可证、SBOM 与漏洞证据；
- Relay runtime / container provenance；
- bundle identity 与既有数据迁移；
- owner-signed Project capability bootstrap；
- 既有数据升级与 canonical readback；
- 已发布制品的 clean-room E2E；
- 私密安全报告渠道与发行治理。

因此，不能把当前分支名、源码可构建或本地 smoke test 描述成“已经发布”“生产就绪”
“下载即用”或“稳定升级”。详细计划见
[开源发行面方案](../stage/carryforth/open-source-release-surface-plan.md)。

## 6. 与本地优先相关的限制

本地 Relay 和数据依赖运行在用户控制的环境中，Desktop 不使用旧 hosted control plane。
但以下行为仍可能联网：

- 首次 Hermit、Cargo、pnpm 与容器依赖下载；
- 用户配置的模型或语义 Provider；
- Git remote；
- 远程媒体、Resource 和外部链接。

源码开发 Compose 使用开发凭据并向宿主机发布多个端口，不是生产加固部署。

## 7. 如何阅读“已实现”

在 Carryforth 文档中，“代码已实现”只表示对应协议或组件存在，并通过了当时记录的测试边界。
它不自动意味着：

- 新环境默认开启；
- 所有 Community 已迁移或初始化；
- 所有平台正式支持；
- 已完成真实规模和长期运行资格；
- 已成为稳定公共 API；
- 能取代外部事实源或自动理解整个项目。

具体启用前应同时核对当前代码、活动运维文档、migration ledger、readiness 和对应资格报告。
