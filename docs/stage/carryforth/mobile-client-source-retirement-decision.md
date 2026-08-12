# Carryforth Mobile 客户端源码退役决策

> 状态：已实施；Mobile 退役专项自动化验收通过，待提交
> 日期：2026-08-12
> 范围：活动源码树中的 mobile/ Flutter 客户端及其构建、测试、素材和文档接线
> 关联：[开源发布面收敛计划](open-source-release-surface-plan.md)、
> [源码与本地开发面收口计划](source-and-local-development-surface-de-buzz-closure-implementation-plan.md)、
> [源码资产权利清理计划](source-repository-asset-rights-cleanup-implementation-plan.md)

## 1. 决策

Carryforth 当前由 Desktop 产品面继续演进，不再把继承自 Buzz 的 Flutter 客户端作为活动源码面维护。

因此，本阶段从当前活动树中完整退役 mobile/。

退役必须是原子操作：不仅删除 Flutter 源码，还要同步删除移动端 CI、开发命令、Git hooks、素材清单、
fixture 复制合同和当前产品文档中的移动端入口。不得留下一个无法构建、无人维护但仍被公开仓库暗示为
可用产品的半成品目录。

web/ 与 admin-web/ 不属于本次决策。它们是否保留、退役或重新定义产品职责，必须另行审计和决策。

## 2. 背景

Carryforth 的实际产品开发集中在 desktop/。当前 mobile/ 是从上游 Buzz 继承的 Flutter 客户端，
维护者没有在该客户端上继续产品开发，也不准备在当前开源阶段承诺 Android、iOS 或商店发行。

保留这套源码会形成与实际维护能力不一致的公开信号：

- 外部贡献者会把 Mobile 理解为当前受支持客户端；
- 默认 CI、Git hooks 和本地质量门禁继续要求 Flutter 工具链；
- Android/iOS 图标、字体和 fixture 扩大素材盘点与再分发审计范围；
- Mobile 的依赖、平台工程和测试会继续产生安全更新与兼容维护义务；
- Desktop-only 的产品边界被一个 source-only 目录稀释。

与其把 Mobile 标记为“暂时不支持”后长期保留，本项目选择让活动树与真实产品边界一致。Git 历史和
上游仓库继续保存其来源；从当前树删除源码不改变 Apache-2.0 归属，也不删除
[LICENSE](../../../LICENSE)、[NOTICE](../../../NOTICE) 或
[UPSTREAM.md](../../../UPSTREAM.md) 中的上游说明。

## 3. 决策时的事实基线

2026-08-12 的仓库审计确认：

- mobile/ 有 313 个跟踪文件，跟踪内容约 3.7 MB；
- 工作树中没有未提交或未跟踪的移动端源码；
- 本地目录约 119 MB，差额主要来自 Flutter、Dart、Gradle 和平台构建缓存；
- Mobile 不属于根 Cargo workspace 或 pnpm workspace；
- Desktop、Relay 和 cf 的运行时不依赖 Mobile；
- 当前公开 Docker/Relay artifact 不包含 Mobile；
- 当前公开发行矩阵不包含 Android、iOS 或 Flutter artifact；
- Mobile 仍接入默认 CI、Justfile、Lefthook、源码素材清单和媒体 fixture 复制检查。

因此，Mobile 不是运行时依赖，但也不能只执行一次目录删除；外围接线必须同步收口。

## 4. 实施范围

### 4.1 删除客户端源码

- 删除完整的 mobile/ 跟踪树；
- 删除仓库内该路径下的忽略型构建产物和缓存；
- 不操作 Flutter SDK、Android SDK、模拟器、真机、用户主目录或仓库外的应用数据；
- 不运行会影响外部平台状态的发布、签名、商店或设备命令。

### 4.2 收口构建和质量门禁

同步删除：

- .github/workflows/ci.yml 中的 Mobile path filter、Flutter 安装、缓存、format、analyze 和 test job；
- Justfile 中的 mobile-* recipes，以及 check、fmt-all、fix-all、ci 对它们的依赖；
- lefthook.yml 中的 Mobile format/test hooks；
- .dockerignore 中已经失去意义的 Mobile 排除项；
- 只为 Mobile 存在的 Flutter 依赖说明和开发入口。

完成后，外部贡献者构建 Desktop-only Carryforth 不应再被要求安装 Flutter 或 Dart。

### 4.3 收口素材与权利清单

release/source-assets.json 必须与删除后的活动树一致：

- 删除 carryforth-mobile-icons 条目；
- 删除 inter-mobile-fonts 条目及 Mobile 字体许可文件引用；
- 从 Android/iOS media fixture 条目中移除 Mobile copy 路径，并重新计算文件数与 tree hash；
- 删除 Mobile Dart 测试中的 inline media 和 data-URI marker 记录；
- 重新运行源码素材与发布素材完整性检查。

Mobile 中的 Inter TTF 已有 OFL-1.1 许可证据，本身不是当前发布 blocker。当前
inter-variable-font blocker 针对 Desktop/Vite 发行包中 Inter WOFF2 的许可文本随包交付义务；
删除 Mobile 不得错误地关闭或减少该 blocker。

### 4.4 保留后端规范 fixture

以下位于 crates/buzz-media/tests/fixtures/ 的规范素材继续保留：

- Android Bitmap 编码与 sanitizer fixture；
- iOS UIKit 编码与 sanitizer fixture；
- synthetic source 和 sanitizer 结果；
- 对规范文件自身 hash、生成方式和许可的验证。

只删除复制到 Mobile 平台测试目录的副本和对应 duplicate contracts。这样会失去 Android/iOS 客户端原生
解码测试，但不会删除 Relay/media sanitizer 的规范输入、回归样本或来源证据。

### 4.5 收口当前文档和代码引用

更新当前入口文档和说明：

- 根 README.md 不再列出 source-only Mobile；
- CONTRIBUTING.md 不再要求 Flutter；
- AGENTS.md 不再包含 Mobile 规则和质量门禁；
- .env.example 中把 Mobile 当作未来客户端的当前表述改为真实产品边界；
- Mobile fixture README 改为只描述保留的规范素材；
- Desktop 源码中以 Mobile 实现作为权威参照的注释改写为独立技术合同；
- scripts/generate-inline-media-test-fixture.mjs 不再把已删除的 Dart 测试作为生成目标。

历史设计、事故记录和协议文档可以继续提及曾经存在的 Mobile，只要语境明确是历史事实或非目标，而不是
当前构建入口。

### 4.6 防止无意回流

在 Carryforth 当前产品表面检查中把根 mobile/ 设为退役路径。后续若要恢复移动客户端，必须先建立新的
产品阶段和验收矩阵，而不是直接复制上游目录或绕过门禁。

桌面通知音中的 flutter 是一个声音预设名称，不是 Flutter 客户端依赖；相关 WAV/SVG 和前端枚举不因本次
决策删除。

## 5. 明确非目标

本次决策不做：

- 不删除或修改 web/、admin-web/；
- 不删除 Desktop、Relay、ACP、cf 或本地依赖栈能力；
- 不重命名 buzz-* crate、binary、环境变量、数据库或协议坐标；
- 不修改数据库 schema、migration、Nostr event 或已有用户数据；
- 不清理仓库外的 Flutter、Android、iOS 或模拟器数据；
- 不把删除 Mobile 解释为已关闭 Desktop 字体许可 blocker；
- 不承诺未来永远不提供 Carryforth Mobile。

## 6. 后果

### 6.1 正向后果

- 活动源码树与实际的 Desktop 产品维护范围一致；
- 默认开发与 CI 不再依赖 Flutter/Dart 工具链；
- 减少平台依赖、安全更新、素材归属和测试维护面；
- README、贡献指南和发布范围不再暗示 Mobile 受支持；
- 新公开仓库可以只携带真正准备维护和验收的客户端源码。

### 6.2 负向后果

- 当前树不再提供 Android/iOS/Flutter 客户端；
- Mobile 的 Dart 单元/widget 测试和原生平台测试一并消失；
- Android/iOS media fixture 的客户端复制一致性不再验证；
- 未来恢复 Mobile 不能依赖当前主分支直接构建，需要从历史或上游选择基线并重新完成产品化。

这些后果是本次范围收敛的预期结果，不应通过保留隐藏 workflow、孤立目录或不受门禁约束的源码副本来规避。

## 7. 验收条件

实施完成必须同时满足：

1. 根 mobile/ 不存在，并被退役路径门禁覆盖；
2. 活动 CI、Justfile、Lefthook 和当前贡献说明不再调用 Flutter/Dart；
3. 当前产品 README、AGENTS 和环境模板不再把 Mobile 描述为现有客户端；
4. Mobile 专用素材、inline payload 和 marker 已从素材清单删除；
5. 保留的 Android/iOS 规范 fixture 检查在没有 Mobile copies 时通过；
6. Desktop、Relay、cf、Web 和 Admin Web 的源码不因本次决策被删除；
7. Carryforth 当前产品表面检查通过；
8. 源码素材和发布素材完整性检查通过；
9. 开源发布表面检查通过，既有发布 blocker 不被错误隐藏；
10. git diff --check 通过，且提交中没有仓库外状态或生成缓存。

## 8. 实施结果

2026-08-12 已按本决策完成活动树收口：

- 删除 `mobile/` 的 313 个跟踪文件，并清理该目录下本地忽略型 Flutter/Gradle 构建缓存；
- 删除只为该客户端提供工具链入口的 `bin/flutter`、`bin/dart` 和 Hermit package marker；
- 从 CI、Justfile、Lefthook、环境模板、贡献指南、Agent 指南和根 README 删除 Mobile 构建与产品入口；
- 从源码素材清单删除 Mobile 图标、Inter TTF、Dart inline payload 和 data-URI marker；
- 保留后端 Android/iOS sanitizer 规范 fixture，把跨树 duplicate contract 收敛为 0；
- 将根 `mobile/` 加入当前产品面退役路径门禁，并从允许的源码素材 usage 中删除 `mobile`；
- `web/` 与 `admin-web/` 的改动文件数为 0，Desktop 媒体处理只改写了依赖已删除实现的注释，没有改变逻辑。

专项验证结果：

- inline media generator 检查通过；
- 13 个 canonical media fixture 通过，duplicate contract 为 0；
- `buzz-media` 103 个单元测试通过，1 个需要 live MinIO 的集成测试保持忽略；
- 源码素材清单通过：100 个文件、21 个内嵌 payload、21 个显式 URI marker；
- 发布素材完整性检查通过，既有 8 个发布 blocker 原样保留，其中包括 Desktop 的 `inter-variable-font`；
- current-product、open-source release surface 和 public package metadata 门禁通过；
- 根与 Desktop `cargo metadata --locked`、Rust fmt、Justfile 解析、workflow YAML 解析和 `git diff --check` 通过；
- `just ci` dry-run 中没有 Mobile/Flutter/Dart 步骤。

全量 `just ci` 在进入编译前被既有 Project View 文档门禁阻断：当前 `HEAD` 已不存在
`docs/lora/stage/document/stage2-canary.md`，但 `scripts/check-project-view-v3-runtime.sh` 仍要求该文件。该问题在本次
Mobile 退役前已经存在，且不属于本决策范围；本次没有通过放宽 Project View 合同来掩盖它。

## 9. 恢复策略

如果未来重新启动 Mobile 产品，必须建立独立方案，至少重新确认：

- 当前 Carryforth 功能、协议和权限模型；
- Android/iOS 支持矩阵与 clean-machine 构建；
- bundle/application identity、签名和升级路径；
- 密钥、深链、媒体、通知和本地数据安全；
- 独立 CI、测试、依赖更新和素材许可；
- 与 Desktop/Relay 当前版本的端到端互操作。

恢复应从一个明确审核过的历史或上游基线开始，并以 Carryforth 当前产品身份重新验收；不得因为 Git 历史中仍有
旧 Mobile 源码，就把它视为可直接重新发布的现成产品。
