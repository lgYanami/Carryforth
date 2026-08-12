# Carryforth 源码仓库素材权利清理实施方案

> 状态：已实施，自动化验收通过；人工视觉/试听与 Git 历史处理待独立完成
> 日期：2026-08-11
> 范围：当前 Carryforth 工作树中跟踪的图像、图标、字体、音频、波形、截图与源码内嵌媒体
> 不包含：Git 历史改写、branch/tag/ref 清理、远程仓库可见性切换
> 关联：[Packaged Asset Provenance and License Inventory](../../../release/THIRD_PARTY_ASSETS.md)、
> [开源发布面收敛计划](open-source-release-surface-plan.md)、
> [源码与本地开发面去 Buzz 收口计划](source-and-local-development-surface-de-buzz-closure-implementation-plan.md)

## 1. 结论

本阶段采用一条保守且可审计的素材清理路线：

1. 无法证明权利的旧素材直接删除，不把“上游仓库曾经包含”当成授权证据；
2. 需要保留的产品功能使用 Carryforth 自有的代码绘制图形或确定性生成素材替代；
3. 保留稳定的数据 ID、设置键和功能合同，不以素材清理为理由清空用户数据；
4. 新建面向公开源码树的素材清单和失败关闭门禁，不再只审计安装包中的部分资产；
5. 当前 HEAD 的素材清理与 Git 历史处理分开交付。本文不执行 `filter-repo`、force push、
   tag 删除或远程仓库切换。

本文是工程与来源证据方案，不代替法律意见。对商标、角色形象或第三方授权仍存在疑问时，
实现应选择删除或自有替代，不自行宣告已获授权。

## 2. 范围边界

### 2.1 本次必须清理

| 类别 | 当前位置 | 目标状态 |
| --- | --- | --- |
| Runtime Provider Logo | `desktop/public/runtime-icons/**` | 全部删除；使用中性、代码绘制的 CLI glyph |
| Onboarding Provider Logo | `desktop/src/features/onboarding/assets/harness-logos/**` | 全部删除；与 Settings 共用同一个中性 glyph |
| Starter Team APNG | `desktop/public/onboarding/starter-team/**` | 全部删除；使用代码绘制的角色 badge |
| 内嵌 Agent 头像 | `managed_agents/personas.rs`、未使用的 `persona_avatars.rs` | 删除六个大型 base64 PNG 与未使用模块；Starter 默认头像改为 `None` |
| 源码内嵌 SVG/data URI | `composer.css`、`animations.css`、`markdown.css` 及其他 tracked text | 删除旧 Buzz bee 图形；第三方品牌图形与来源不明 mask 改为 Carryforth 自有中性 glyph；保留项逐个入清单 |
| 通知音频与波形 | `desktop/public/sounds/**` | 旧 MP3/SVG 全部替换为确定性生成的 WAV/SVG |
| Mobile Geist | `mobile/assets/fonts/Geist*.ttf` | 删除四个字体；代码字体改用系统 monospace |
| Mobile Buzz 图标 | Mobile 配对页、Android/iOS launcher 与 launch image | launcher/AppIcon 由 Carryforth 源 SVG 生成；launch image 改实色背景 |
| Sprout 图片 | `crates/buzz-agent/sprout-agent.png`、`docs/assets/sprout*.png` | 无引用的素材直接删除 |
| 旧产品截图 | `docs/assets/screenshots/*.png` | 删除；本阶段不保留图像型 seed fixture |
| Android/iOS 媒体测试 fixture | `crates/buzz-media/tests/fixtures/**`、Mobile 对应 fixture | 补全程序化源像、生成工具链、副本一致性和本地权利记录 |

### 2.2 已有可保留素材

- `../../../desktop/src-tauri/icons/carryforth-source.svg` 及已记录的 Carryforth 派生图标。现有 `icon.icns`
  作为项目自有 artwork rendition 记录源 SVG、创建提交和当前哈希，不虚称其容器字节可由当前 Tauri CLI
  确定性重建；
- Carryforth glyph、wordmark 和可复现生成的 card texture；
- `desktop/public/pow/**` 中随文件携带 MIT 许可的已核对素材；
- Mobile Inter 字体，但必须继续与 `../../../mobile/assets/fonts/Inter-LICENSE.txt` 一起分发；
- 具有本地 license、固定上游版本和字节哈希的第三方素材。

### 2.3 明确非目标

本阶段不做：

- 不改写 Git commit、branch、tag、remote ref 或 object database；
- 不执行 `git filter-repo`、BFG、force push 或远程 garbage collection；
- 不宣称当前仓库的旧历史已经不再包含旧素材；
- 不处理 Desktop bundle identifier、签名、SBOM 或安装包发布；
- 不删除 Docker volume、数据库、keyring、Desktop app-data 或 Agent workspace；
- 不修改 SQL migration，也不纳入或改动当前未跟踪的 `../../../migrations/0057_project_context_semantic_foundation.sql`；
- 不全局重命名 `buzz-*`、`BUZZ_*`、数据库表或 wire/storage 兼容坐标。

## 3. 实施不变量

1. **无权利证据就不保留字节**：不为旧 Provider Logo、角色图、声音或截图补写推测性许可。
2. **功能与品牌解耦**：Claude Code、Codex、Goose 等 runtime 名称可作为事实性文字标签，但当前产品不分发、
   远程加载或默认持久化其商标图形。
3. **不改用户数据**：本次不自动清理已存在 app-data 或 Relay profile 中的头像；只停止当前源码和新默认值继续携带旧素材。
4. **不改稳定 ID**：`builtin:fizz`、`builtin:honey`、`builtin:bumble` 以及十二个通知声音 ID 继续作为兼容坐标。
5. **本次新生成物必须可复现**：本计划新增或替换的 generated output 不依赖网络、系统字体、旧素材或
   非固定随机源，能够离线逐字节重建。既有项目 artwork rendition 若容器编码本身不确定，必须按源文件、
   创建提交和当前哈希清权，并明确分类为 `project-art-rendition`，不得冒充 `generated/reproducible`。
6. **生成物不模仿旧作品**：新图形、角色 badge 和通知音从空白参数独立设计，不以旧图片或旧 MP3
   作为输入、描图、采样或相似性目标。
7. **当前源码树必须全覆盖**：每个跟踪的非代码素材必须在公开源码清单中恰好匹配一次，不存在默认允许。
8. **删除源码素材不等于删除用户数据**：本计划的删除对象只能是仓库文件或可证明的系统默认值。
9. **文本产品语义不搭车改动**：本计划清理媒体字节，不借机修改 Agent 业务角色、system prompt 或已有会议/项目行为。

## 4. 目标实现

### 4.1 Provider Runtime 使用中性图形

新增一个共享的 `RuntimeGlyph` 组件：

- Carryforth 内置 Agent 继续使用 `CarryforthMark`；
- 所有外部 ACP runtime 使用 Carryforth 自有的中性 terminal/CLI 几何 glyph；
- 视觉区分继续由 runtime 文字标签和状态提供，不为不同供应商复制商标图案；
- Settings Doctor 和 Onboarding 必须使用同一组件，不再维护两套 Logo 映射；
- 已知外部 runtime 不再回退到 `runtime.avatarUrl` 或硬编码外部 Logo URL；
- 用户显式为自定义 runtime/persona 设置的头像 URL 可继续按现有安全策略处理。

Tauri runtime metadata 也必须收敛：

- 保持 `KnownAcpRuntime.avatar_url`、`AcpRuntimeCatalogEntry.avatar_url` 和 TS `avatarUrl` 的当前字符串 wire 合同；
- 外部已知 runtime 的 catalog `avatar_url` 输出空字符串，不再返回 Goose/Anthropic/OpenAI 远程图标；
- `managed_agent_avatar_url` 对外部已知 runtime 返回 `None`，对内置 Agent 仍可返回 Carryforth 图标；
- 当前用户数据中的 Provider URL 可能是历史默认值，也可能是用户手工选择的相同 URL，本次不猜测、不清除。

### 4.2 Starter Team 使用代码绘制 badge

新增 `StarterPersonaBadge` 组件，使用 Carryforth palette、基本几何形状与文字首字母组成，不引用外部图像、
字体或旧角色帧。

- `CommunityOnboardingFlow` 必须按稳定 persona ID 选择 badge，不按可变 `display_name` 匹配；
- `WelcomeKickoffStage` 使用同一 badge，保留现有分段入场、退场、timeout 和 reduced-motion 时序；
- 删除三个 APNG 与对应 `<img>` 路径；
- `personas.rs` 中三个内嵌 PNG 常量删除，内置 persona 的 `avatar_url` 改为 `None`；
- 删除当前未使用但仍内嵌 SOLO/KIT/SCOUT 三张大型 PNG 的 `persona_avatars.rs` 及其模块声明；
- 新建或新默认的无头像场景只能落到现有 Carryforth runtime mark 或首字母 fallback，不能恢复旧角色图。

当前三个 `builtin:*` ID、默认展示名、`name_pool` 和 system prompt 全部保持不变。本次只删除角色图像字节，
避免把素材清理扩大为 Agent 行为迁移。Fizz/Honey/Bumble 名称与蜂类文本人设是单独的产品/商标评审项；
本文不宣称其已清权，也不在未经明确产品决策时修改。

### 4.3 通知音使用确定性 PCM 生成

保留当前十二个 `SoundName` ID：

```text
bong, boo, dng, doo, doodone, doong, doop, flirl, flutter, oh-no, ping, unison
```

新增无网络、无第三方输入的确定性 Node 生成器，同时生成：

- `desktop/public/sounds/<name>.wav`；
- `desktop/public/sounds/<name>.svg`。

生成器固定使用 mono、16-bit PCM、48 kHz，音高、扫频、包络、时长和随机种子都由仓库参数定义。
核心样本计算使用整数/定点表达或已固定查找表，避免把跨平台 `Math.sin` 差异当成可复现合同。
随机调制只允许明确定义的固定种子整数 PRNG，不使用 `Math.random`。量化的 rounding/clamp 规则必须写入生成器合同，
WAV sample 通过显式 little-endian 写入接口输出，不依赖宿主端序。
它不读取旧 MP3、外部 sample pack 或网络资源。SVG 波形直接从新 PCM 派生。

- `sound.ts` 的 URL 由 `.mp3` 改为 `.wav`；
- 现有 `HTMLAudioElement`、`pause`、`ended`、`currentTime = 0` 和 best-effort 播放语义保持不变；
- `SoundPicker` 继续使用同名 SVG mask；
- 用户现有的 `buzz-notification-settings.v2` 无需迁移；
- 原 `generate-sound-waveforms.mjs` 由新的单一权威生成器取代，不再依赖 ffmpeg 解码旧 MP3。

生成器必须使用 Hermit/lockfile 固定的 Node 工具链，并提供 `--write` 和 `--check`：校验预期 ID 集合、WAV header、
声道、采样率、数值化的 peak/DC offset/时长阈值、SVG 尺寸与字节一致性，并拒绝任何遗留 `.mp3`。

### 4.4 Mobile 使用 Carryforth 图标与系统等宽字体

以 `../../../desktop/src-tauri/icons/carryforth-source.svg` 为唯一图标源，使用仓库内确定性生成器输出：

- Mobile 配对页使用的 Carryforth 图标；
- Android `mipmap-*` launcher、foreground 和 round icon；
- iOS `AppIcon.appiconset`。

生成器优先复用 lockfile 固定的 Tauri icon 工具链，在临时目录生成后按显式映射拷贝所需 rendition。它必须使用
Hermit 与 lockfile 固定的渲染工具，固定所有尺寸、背景色、alpha 处理和输出路径，支持离线 `--check`，
并验证 Android/iOS manifest
引用的每个 rendition 都存在且尺寸正确。删除 `mobile/assets/images/buzz-icon.png`，配对页及测试改用
新坐标。

启动图不必保留独立位图：Android 删除五份 `launch_image.png` 与 XML bitmap item，iOS 删除
`LaunchImage.imageset` 与 storyboard image view，改为 Carryforth 实色背景。配对页复用已有 `flutter_svg` 渲染一份已跟踪的
Carryforth SVG，不为同一标记增加第二份无来源位图。

Geist 采用最小权利负担方案：

- 删除四个 `Geist*.ttf`；
- 从 `../../../mobile/pubspec.yaml` 删除 `GeistMono` family；
- 所有 `fontFamily: 'GeistMono'` 改为共享的系统 monospace 字体选择；
- 修改相关 widget 测试，不再断言 Geist 字体名；
- Inter 保留，并在公开源码素材清单中记录 OFL-1.1 及本地 license 证据。

### 4.5 删除 Sprout 和旧截图

- 删除无生产引用的 `crates/buzz-agent/sprout-agent.png`；
- 删除无引用的 `docs/assets/sprout.png` 和 `docs/assets/sprout-icon.png`；
- 删除 `docs/assets/screenshots/create-channel.png`、`media-comments.png`、`channel-thread.png`、
  `channel-agents.png`；
- `../../../scripts/seed-admin-dashboard.sh` 删除三个截图上传和对应 attachment 构造，保留文本型虚构 diagnostics fixture；
- 文档和测试不再引用旧截图路径。

如未来需要截图，必须用 Carryforth E2E mock 数据重新生成，只截取应用窗口，使用程序化头像和虚构文本，
不包含操作系统壁纸、真人头像、旧角色图或未清权媒体。

### 4.6 补齐媒体 sanitizer 测试 fixture 来源

Android 与 iOS 媒体 sanitizer fixture 是源码树分发的字节，即使它们只用于测试也必须进入 source inventory。

- Android 3 x 2 fixture 继续使用 README 中已记录的程序化像素和 API 36 `Bitmap.compress` 路径；
- 将 Android 生成程序、系统/API 版本、输出哈希和 sanitized 派生关系固定为本地来源证据；
- iOS fixture 不再引用未跟踪的含糊 `source.png`；源像改由已跟踪的固定像素生成器产生，再经记录版本的
  iOS Simulator/UIKit 重编码；
- 记录 Xcode/SDK/simulator target、编码参数、输出哈希与 sanitizer 派生关系；
- `../../../crates/buzz-media`、Android unit/instrumentation 和 iOS RunnerTests 中的重复 fixture 必须逐字节相等，不维护多个无关来源；
- 若当前 iOS 字节无法从上述记录流程重建，就删除并使用新的程序化 fixture，不为旧字节填写推测性来源。

### 4.7 清理源码内嵌 SVG 与 data URI

源码内嵌媒体与独立 PNG/音频文件具有相同的分发属性，不能因为它们位于 CSS、Rust 或测试文本中就跳过权利审计。
本阶段至少处理以下现行生产路径：

- `../../../desktop/src/shared/styles/globals/composer.css` 中 GitHub、Linear、Google Drive/Docs/Sheets/Slides 的
  内嵌品牌 SVG path，统一替换为 Carryforth 自有的中性 link/file glyph；域名或服务名可作为事实性文字保留，
  但不继续分发其商标图形；
- `../../../desktop/src/shared/styles/globals/animations.css` 中旧 Buzz bee wing-flap sprite/mask：先以引用和构建图确认；
  无调用则删除整段死 CSS，有调用则以 Carryforth 自有的中性 activity 图形和动画替换；
- `../../../desktop/src/shared/styles/globals/markdown.css` 中来源未证明的 robot mask，替换为与 `RuntimeGlyph` 同源的
  Carryforth 自有 agent glyph；
- 其他 tracked text 中的 base64、percent-encoded 或 raw media data URI 必须逐个解码、识别和入清单，
  或删除/替换；测试与协议 fixture 只能使用窄化的显式条目，不设置目录级或全局豁免。

若中性 glyph 仍以 data URI 形式服务于 CSS mask，它必须以仓库内已清权源 SVG 为权威来源，并在清单中记录
文件、selector/symbol locator、编码形式、decoded SHA-256 与生成规则；优先复用一个共享图形，避免复制多份
难以追踪的 path 字节。

## 5. 用户数据边界

本阶段不创建 SQL migration，也不修改 Desktop app-state 或 Relay profile 中已存在的头像。原因是：

- 现有 `avatar_url = None` 同时承担“显式无头像”和“旧记录待回填”的过载语义；
- `reconcile_agent_profile` 可以从 persona 或 Relay kind:0 picture 回填 `None`；
- 新建 persona-backed instance 在 persona 无头像时可以回退到内置 runtime Carryforth mark；
- 本地 JSON 与 Relay profile event 不是一个跨系统原子事务。

为了不把“源码素材清理”扩大为高风险的用户数据迁移，本次固定以下语义：

1. 新版本源码、内置 persona 定义和已知外部 runtime 不再包含或默认写入旧图像；
2. 已存在的本地头像、用户自定义 URL、uploaded-media 和 Relay profile 完全保留；
3. 不修改 `migration.rs`、`agents_profile.rs` 或 profile-sync 状态机来追求旧用户 UI 立即去图；
4. 如后续要求清理已有用户头像，必须单独设计“显式无头像”tombstone、migration version、本地原子写、
   Relay 幂等同步和用户可选回复；不在本文中暗中实现。

因此，本计划完成时的结论是“公开源码树和新默认值不再分发旧素材”，而不是“所有已安装用户的本地副本已被删除”。

## 6. 公开源码素材清单与门禁

### 6.1 单独建立 source inventory

新增：

- `release/source-assets.json`；
- `../../../scripts/check-source-asset-inventory.mjs`。

它们与现有 `packaged-assets.json` 职责不同：

| 清单 | 回答的问题 |
| --- | --- |
| `source-assets.json` | 公开 Git 当前树会分发哪些非代码素材，是否都有来源与授权证据？ |
| `packaged-assets.json` | Desktop/Relay/CLI 发行物实际携带哪些素材和程序，发布义务是否满足？ |

当前实现从文件扩展名与内容 magic 的并集动态发现素材；本次交付基线为 141 个独立素材文件、26 组已解码
内嵌媒体和 23 组不携带媒体字节的显式 data-URI 构造/占位。这个数量只是文档日期时的快照，
门禁不得硬编码数量，必须从当前跟踪文件动态枚举。

### 6.2 清单字段

每个条目至少包含：

- 稳定 ID、路径 pattern、跟踪文件数和 `tree_sha256`；
- 分类：project-art、project-art-rendition、generated、third-party-font、third-party-media、test-fixture、screenshot；
- 使用面：source、docs、desktop、mobile、test、package；
- 权利人、SPDX license 以及本地 license 文件与哈希；
- 固定上游 URL/ref/SHA-256，或仓库内生成器和已清权源素材；
- 商标使用状态；
- 截图的 privacy review 和 mock/synthetic 数据证明；
- SVG 中允许使用的 `declared_fonts` 及它们是字体名偏好还是实际嵌入字节；
- 源码内嵌媒体的文件、symbol/locator、decoded SHA-256 与长度；
- `cleared` 或 `blocked` 状态及理由。

### 6.3 门禁行为

`check-source-asset-inventory.mjs` 必须使用 Hermit 固定的 Node 和标准库实现枚举、magic signature、SHA-256、
data-URI decode 与 JSON 校验，不假定系统存在 GNU `find`、`file(1)` 或 `sha256sum`。这样同一门禁可在 Linux、
macOS 和 Windows 的 pre-commit/CI 运行。它必须：

1. 对当前 tracked tree 中 mode 100644/100755 的所有普通文件执行 MIME/magic 识别，并按文档化的闭集枚举
   png/jpg/jpeg/svg/audio/font/ico/icns/pdf/video 及真正的媒体归档等素材；
2. 同时校验扩展名和 MIME/magic，防止换后缀或把媒体放入无后缀文件绕过；
3. 要求每个素材恰好匹配一个清单条目，拒绝缺失、重叠，并禁止 inventory 指向 symlink；tracked mode 120000
   不被跟随或当成素材字节，Hermit `.pkg`/toolchain shim 由现有工具链完整性门禁管理；
4. 核对文件数、路径与字节集合哈希；
5. 校验 license、source、generator 文件均已跟踪、非 symlink 且哈希相符；
6. 校验本计划新增及所有声明为 `generated` 的 rendition 必须指向已清权 source 和可复现生成器；既有
   `project-art-rendition` 必须记录源文件、创建提交与当前哈希，并且不能携带可复现声明；
7. 扫描 SVG 的外部 `href`/`image`、data URL 与未申报字体；
8. 扫描所有 tracked 文本中的全部 `data:(image|audio|font|video)/...` URI；按 RFC 2397 元数据区分并
   规范解码 `;base64`、percent-encoded 与 raw payload，再校验媒体类型、decoded hash 和长度。默认拒绝，
   必须保留时要求按 path + selector/symbol locator + 编码形式 + decoded hash 记录 inventory 条目与权利证据；
   小型协议/单元测试 data URI 也只能使用窄化的显式分类，不设全局豁免；
9. 对 quoted bare-base64 和源码数字数组执行强媒体 magic 检查；真实媒体字节必须改为完整、可清单化的
   data URI、已清权文件 fixture 或运行时程序化构造，不允许通过拆分字符串或字节数组绕过；
10. 对截图要求显式 privacy/mock 证据；
11. 当前公开源码清单只接受 `cleared`，任何未清权条目直接使门禁失败；
12. 在普通 PR/CI 中运行，新增或修改二进制素材而不更新清单时立即失败。

现有 `../../../scripts/check-release-asset-inventory.sh` 继续用于发行物。被删除的素材 root 必须从 packaged inventory
中移除；新生成声音必须以 Carryforth-generated / Apache-2.0 重新记录。与素材无关的 SBOM、容器、
bundle identity 和 clean-room 发布 blocker 保持不变。

## 7. 实施阶段

### 阶段 A：冻结当前基线

1. 列出本文范围内的所有跟踪素材、路径、字节哈希和引用点；
2. 记录 `personas.rs` 和 `persona_avatars.rs` 中六个内嵌媒体的 locator、decoded hash 与长度，作为当前树禁入证据；
3. 记录 CSS 与其他 tracked text 中 base64、percent-encoded、raw media data URI 的 locator、decoded hash、
   引用状态和权利分类；
4. 记录旧 Provider URL、UI 状态、十二个声音设置 ID 和 sanitizer fixture 副本哈希；
5. 确认现有用户 app-data/Relay profile 不在修改范围；
6. 确认不触碰 SQL migration、数据库和受保护的未跟踪 0057 文件。

### 阶段 B：删除 Provider 商标图形

1. 实现共享 `RuntimeGlyph`；
2. 替换 Doctor 和 Onboarding 的 Logo 映射；
3. 删除两组六个 Logo 文件；
4. 删除 Tauri 已知 runtime 中的外部 Logo URL 默认值；
5. 把 `discovery/tests.rs` 和 `commands/agents_tests.rs` 中对外部 Provider URL 的 `unwrap`/等值断言改为 `None`/中性 fallback；
6. 替换 Composer 的第三方品牌 SVG、Markdown robot mask，并删除或替换旧 Buzz bee animation/mask；
7. 添加 runtime 图形、fallback、custom avatar、无外部图标请求及内嵌媒体禁入测试。

### 阶段 C：替换 Starter Team 素材

1. 实现 `StarterPersonaBadge`；
2. 替换 Onboarding 与 Welcome Kickoff 两个渲染路径；
3. 删除 APNG、`personas.rs` base64 PNG 和未使用的 `persona_avatars.rs` 模块；
4. 保留内置默认展示名、`name_pool`、prompt 和 `builtin:*` ID；
5. 把 `migration_avatar_tests.rs` 中对新内置图片的 `unwrap`/替换断言改为“无新头像、既有用户字节保留”；
6. 用 fixture 证明既有 app-state 和 Relay profile 未被本阶段修改。

### 阶段 D：替换通知音

1. 实现新的 WAV/SVG 生成器与来源说明；
2. 生成十二组新输出，删除旧 MP3/SVG；
3. 切换播放 URL，保持旧 sound ID 与播放合同；
4. 把 `--check` 接入 Desktop check；
5. 完成自动测试与人工试听。

### 阶段 E：清理 Mobile 与文档素材

1. 由 Carryforth source SVG 重新生成 Mobile 全套图标；
2. 替换配对页图标及测试；
3. 删除 Geist，收敛系统 monospace 字体语义；
4. 删除 Sprout 图和旧截图；
5. 让 admin seed 只使用文本型虚构 fixture；
6. 补全 Android/iOS sanitizer fixture 的程序化源像、工具链、哈希和副本一致性证据。

### 阶段 F：建立公开源码素材门禁

1. 全量生成并人工审核 `source-assets.json`；
2. 实现路径、magic、license、生成器、隐私、哈希与三种 data URI 编码检查；
3. 接入 `just check`、pre-commit 和公共 CI；
4. 更新 packaged inventory 与 `THIRD_PARTY_ASSETS.md`；
5. 公开源码严格模式达到零 `blocked`。

### 阶段 G：功能与视觉验收

1. 运行定向静态门禁和单元测试；
2. 生成 Desktop 前端产物，确认产物中无旧素材；
3. 人工检查 Desktop 深色/浅色、Onboarding、Doctor、Starter Team 动画与通知音；
4. 运行 Mobile format/analyze/test 与图标生成检查；
5. 确认本次无 DB/Docker/SQL migration 修改。

## 8. 测试与验收矩阵

### 8.1 静态与权利门禁

- 被删除的 Provider、Starter Team、Sprout、截图和 Geist 路径不再存在跟踪文件；
- 当前源码树不含 Provider 硬编码 Logo URL、旧 APNG 路径或旧头像 base64；
- 所有 tracked 文本中不存在未入清单的图像/音频/字体/视频 data URI，包括 base64、percent-encoded 和 raw payload；
- Composer 不再携带第三方品牌 SVG path，旧 Buzz bee sprite/mask 不再存在，Markdown agent mask 来自已清权的
  Carryforth 中性 glyph；
- `../../../desktop/public/sounds` 恰好包含十二个 WAV 和十二个 SVG，不含 MP3；
- 本计划新生成的 Mobile 图标、音频和波形与仓库生成器输出逐字节一致；现有 ICNS 只按
  `project-art-rendition` 的来源与当前哈希验收，不做虚假的逐字节重建承诺；
- `source-assets.json` 覆盖当前 tracked tree 中 100% 候选素材，且严格模式零 `blocked`；
- `packaged-assets.json` 不再宣告已删除 root，新声音标记为 Carryforth-generated / Apache-2.0；
- `THIRD_PARTY_ASSETS.md` 不再把已删除/替换的五类素材列为 unresolved。

### 8.2 Desktop 回归

- Doctor 的内置 Agent 显示 CarryforthMark，外部 runtime 显示中性 glyph；
- Onboarding 中外部 runtime 可识别，但不发起 Provider Logo 请求；
- Starter Team 三个 badge 可见，分段入场、退场、timeout 和 reduced-motion 不回退；
- 源码中内置 persona 的新默认头像为 `None`，新建记录不包含旧 data URL，既有记录不被本次清理；
- 用户已有 avatar/name/prompt 语义值不变；
- 十二个通知声音都可预览、暂停和重头播放，无 clipping/DC offset/明显爆音；
- 单元测试覆盖 `.wav` URL、`currentTime = 0`、play rejection best-effort、picker pause/ended 与 WAV 到 SVG 的逐字节重生成；
- DM、mention、thread reply 和 needs-action 实际通知仍按现有映射播放。

### 8.3 已有用户数据保护

- 本次不新增或调用素材专用 app-state migration；在隔离 profile reconcile 和其他既有 migration 的定向 fixture 中，
  `managed-agents.json`、persona definition 与 linked instance 的语义字段前后相等；
- 本次没有新的 app-state migration version，不改动 profile-sync pending 状态；
- 新建内置 Starter persona/instance 不再写入旧 data URL；
- 新建外部已知 runtime Agent 不再写入 Provider Logo URL；
- 用户显式选择的 custom avatar 仍按现有合同保留。

### 8.4 Mobile 回归

- 配对页显示 Carryforth 图标；
- Android/iOS 清单引用的所有 launcher/AppIcon rendition 存在、尺寸正确且来自同一源 SVG；
- Android XML 和 iOS storyboard 不再引用 launch image，启动面使用 Carryforth 实色背景；
- `pubspec.yaml` 不再包含 Buzz icon 或 Geist family；
- pairing、settings、invite、markdown editor 和 compose bar 使用系统 monospace 且 widget 测试通过；
- `dart format --output=none --set-exit-if-changed .`、`flutter analyze`、`flutter test` 通过；
- Agent 不运行 `flutter build`、`flutter run`、`flutter clean` 或 `flutter upgrade`。

### 8.5 文档与 seed fixture

- `../../../scripts/seed-admin-dashboard.sh` 在不读取任何 PNG 的情况下生成同等文本诊断数据；
- 活动文档无 `sprout-agent.png`、`docs/assets/sprout*.png` 或旧 screenshot 引用；
- 未来新截图必须在 source inventory 中有生成和隐私证据。

### 8.6 媒体 sanitizer fixture

- Android fixture 的程序化像素、API 版本、编码输出和 sanitized 派生关系均有已跟踪证据；
- iOS fixture 的源像可从固定像素生成，不依赖未跟踪 `source.png`；
- Rust、Android unit/instrumentation 和 iOS RunnerTests 之间对应 fixture 逐字节相等；
- `buzz-media` sanitizer 合同测试继续验证“未清理输入被拒绝、已清理输出被接受”。

## 9. 主要文件矩阵

| 范围 | 主要文件 |
| --- | --- |
| Runtime 图形 | `DoctorSettingsPanel.tsx`、`RuntimeIcon.tsx`、新共享 `RuntimeGlyph.tsx` |
| Runtime metadata | `managed_agents/discovery.rs`、`discovery/runtime_metadata.rs`、`discovery/tests.rs`、`commands/agents_tests.rs` |
| 内嵌 SVG/data URI | `../../../desktop/src/shared/styles/globals/composer.css`、`animations.css`、`markdown.css`、全树 tracked text scanner 与 UI tests |
| Starter UI | `CommunityOnboardingFlow.tsx`、`WelcomeKickoffStage.tsx`、新 `StarterPersonaBadge.tsx`、motion/E2E 测试 |
| Persona 源码清理 | `managed_agents/personas.rs`、`managed_agents/persona_avatars.rs`、`managed_agents/mod.rs`、`migration_avatar_tests.rs`、persona/team tests |
| 通知声音 | `desktop/public/sounds/**`、`sound.ts`、`SoundPicker.tsx`、Desktop package scripts、新生成器 |
| Mobile 字体与图标 | `../../../mobile/pubspec.yaml`、`mobile/assets/**`、Android/iOS asset catalogs、生成器与 widget tests |
| 旧文档素材 | `crates/buzz-agent/sprout-agent.png`、`docs/assets/sprout*.png`、`docs/assets/screenshots/**`、`seed-admin-dashboard.sh` |
| 媒体 sanitizer fixture | `crates/buzz-media/tests/fixtures/**`、Android test resources、iOS RunnerTests Fixtures 与对应 README/生成器 |
| 源码素材门禁 | 新 `source-assets.json`、新 `check-source-asset-inventory.mjs`、`Justfile`、`../../../lefthook.yml`、CI changes/gate |
| 安装素材记录 | `packaged-assets.json`、`THIRD_PARTY_ASSETS.md`、`check-release-asset-inventory.sh` |

## 10. 风险与取舍

### 10.1 视觉会更中性

删除 Provider Logo 与角色 APNG 后，产品的个性化会暂时降低。这是可接受的初版取舍：先获得权利清晰、
可复现的源码面，再在后续独立设计 Carryforth 品牌视觉。

### 10.2 现有用户可能继续看到旧头像

本次不改写用户 app-data 或 Relay profile，所以已安装用户可能继续显示其历史副本。这是有意的安全取舍：
优先保留用户数据，并把高风险的跨本地/Relay 头像迁移留给独立方案。它不影响“当前公开源码树不再分发这些字节”的结论。

### 10.3 源码清单工作量较大

全树素材门禁会把原本未被 packaged inventory 发现的 test fixture、Mobile 素材和文档媒体暴露出来。
不允许为了尽快通过而给这些文件批量标记 `cleared`；每一类都需要真实的 license、来源或生成证据。

### 10.4 Git 历史仍未处理

本文完成后只能得出“当前 tracked tree 的素材权利已收敛”，不能得出“当前所有历史对象可公开”。
Git 历史与公开 refs 必须在独立方案中处理和验收。

### 10.5 角色文本不属于本次媒体替换

保留 Fizz/Honey/Bumble 文本可以避免素材清理改变 Agent 行为，但不等于完成这些名称和人设的商标/内容评审。
如开源决策要求同时退出这些文本身份，应单独设计默认 persona、`name_pool`、prompt、本地数据和用户可见文案迁移，
不在本文实施中暗中处理。

## 11. 完成标准

以下条件全部满足时，本计划才能标记为“已实施”：

1. 本文列出的旧 Provider Logo、Starter Team 图、内嵌头像、内嵌第三方品牌/旧 Buzz bee SVG、MP3、Geist、
   Buzz Mobile 图标、Sprout 图和旧截图已从当前跟踪树移除；
2. 所有功能性替代都来自 Carryforth 自有代码/源 SVG 或有本地许可证据的第三方素材；
3. Provider runtime、Starter onboarding、Welcome kickoff、通知声音、Mobile pairing 和应用图标功能回归通过；
4. 已有 app-state、Relay profile 和用户自定义 fixture 未被迁移，其所有字段前后逐字节一致；
5. 公开源码素材门禁覆盖当前 tracked tree，并且零缺失、零重叠、零 `blocked`；
6. `THIRD_PARTY_ASSETS.md` 和 `packaged-assets.json` 与实际文件集合一致，不用状态文字掩盖未清权字节；
7. 本计划新增或替换并标记为 `generated` 的素材可以离线逐字节重建，其生成器、参数和来源说明均已跟踪；
   既有 `project-art-rendition` 具有源文件、创建提交和当前哈希证据，且没有被错误标记为可复现生成物；
8. Desktop/Mobile 定向测试、媒体 sanitizer fixture 合同、静态门禁和人工视觉/试听验收通过；
9. 没有 SQL migration、DB/Docker 数据变更、Git 历史改写或受保护 0057 文件改动；
10. 交付说明明确写出：当前树素材已清理，Git 历史仍待独立处理，不宣称整个现有仓库可直接切换为 public。

## 12. 后续独立工作

本计划实施后再另立 Git 公开边界方案，仅处理：

- 采用清洁快照新建公开仓库，还是改写现有历史；
- 哪些 branch、tag 和 remote ref 会被公开；
- 旧素材 blob 的可达性验证；
- 历史签名、commit/tag hash、协作者重新 clone 和远程缓存的处理。

该后续工作未完成前，不对现有仓库执行 visibility flip。

## 13. 本次实施结果

截至 2026-08-11，本方案中的当前源码树清理已落地：

- 两组 Provider Logo、Starter APNG、六组 Rust 内嵌角色图、Geist、旧 Buzz Mobile 图标、Sprout 图、
  旧截图和通知 MP3 已从当前树删除；
- Runtime 与 Starter Team 改用 Carryforth 自有的代码绘制中性图形，稳定 runtime/persona ID、展示名、
  prompt、通知设置 ID 和用户数据合同保持不变；
- 通知音、Mobile 图标、PNG/WebP 测试 fixture 均具有仓库内生成器、固定工具链证据和逐字节检查；
- `source-assets.json` 当前覆盖 141 个素材文件、26 组已解码内嵌媒体和 23 组不携带媒体字节的显式
  URI 构造/占位；quoted bare-base64 与完整 numeric-array 媒体扫描结果均为 0；
- `packaged-assets.json` 已移除被删除素材的旧 blocker，并把新 WAV/SVG 记录为 Carryforth-generated /
  Apache-2.0。剩余 8 项只涉及未来安装包、容器、SBOM、身份迁移及发布治理，不是本次源码素材漏项。

本次自动化证据：

- Desktop `check`、TypeScript typecheck、production build 与 3634 条前端单元测试通过；
- Mobile format/analyze/assets check 与完整测试通过（570 passed、1 个既有 skip）；
- `buzz-dev-mcp` data-URL round-trip、合成 WebP generator Clippy、媒体 fixture 合同与所有确定性生成器
  检查通过；
- Carryforth current-product、open-source surface、source inventory、packaged inventory integrity 和
  `git diff --check` 通过；
- 没有启动 Docker/数据库，没有 SQL/schema 变更，没有用户数据迁移，没有 Git 历史改写；受保护的
  `0057_project_context_semantic_foundation.sql` 内容哈希保持不变。

仍需独立完成两类 Human 工作：深色/浅色 UI 的最终视觉确认与十二个通知音的主观试听，以及第 12 节所述
Git 历史/公开 ref 方案。它们不应通过恢复旧素材或绕过当前门禁来解决。
