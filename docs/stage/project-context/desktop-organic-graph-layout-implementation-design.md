# Desktop Project Context 有机网状图布局实现设计

> 状态：已实现
>
> 日期：2026-08-09
>
> 设计基线：`feat/project-view-summary` @ `2acf99fea`
>
> 范围：Buzz Desktop 的 Project Context 图布局、连线端口、Island 外观及对应测试
>
> 明确排除：Project Context 领域或协议、查询结果、Coordinate / Edge / Context Document
> 数据模型、节点位置持久化、拖拽编辑、Agent 检索、Web 与 Mobile
>
> 关联文档：[Project Context Desktop 产品规格](desktop-spec.md)、
> [Project Context Desktop 分阶段实现计划](desktop-implementation-plan.md)、
> [Project Context V2 领域规范](project-context.md)

## 1. 目标

当前 Desktop 将一个 Context Island 排成从左到右的规则层级：先选择高连接度节点作为根，
再按 BFS 距离分列，并在每列内纵向对齐。它稳定、清楚，但视觉上更像流程图或组织图，不能
充分表现 Project Context 是一个无向、可从任意 Coordinate 进入的关系网络。

本次把默认布局改为“稳定的有机网状布局”：
1. 一个 Context Island 围绕结构中心向外发散；
2. Edge Hub 与 Coordinate 保持真实的二部 incidence graph 结构；
3. 节点不重叠，连线可读，Island 仍可独立定位；
4. 同一份图数据得到稳定、可复现的布局；
5. 布局计算完成后立即冻结，不在页面中持续运行物理动画；
6. 视觉中心、角度和距离不产生任何领域含义。

目标不是做一个可编辑图库，而是在保持现有可信查询与 Inspector 行为不变的前提下，让图更像
一组自然展开的关系星团。

## 2. 当前实现基线

### 2.1 当前图模型已经正确

`../../../desktop/src/features/project-context/graph.ts` 已把查询结果转换为准确的 incidence graph：

```text
Coordinate Node ── Spoke ── Edge Hub ── Spoke ── Coordinate Node
```

其中：

- 每个真实 Coordinate 只生成一个矩形节点；
- 每条领域 Edge 只生成一个菱形 Hub；
- 每个 Hub 与完整 Coordinate 集之间各生成一条 Spoke；
- Context Document 仍是 Edge 上的 binding，不会被错误投影成图节点；
- Island 仍由 Hub 共享 Coordinate 的连通性派生。

本次不修改以上图模型。

### 2.2 当前布局是确定性分层布局

`../../../desktop/src/features/project-context/layout.ts` 当前执行：

1. 为每个 Island 建立无向邻接表；
2. 选择最高 degree 的节点作为根；
3. BFS 得到距离层；
4. 使用 barycenter pass 调整层内顺序；
5. 将层排成横向列；
6. 将多个 Island 放进规则网格；
7. 按最终相对位置选择 source 的上下左右连接端口，再把 target 强制设为 opposite side。

Focused Query 另走固定三列：`Anchors -> Hubs -> other Coordinates`。

因此当前截图中的整齐布局不是领域方向，而只是 presentation 算法的结果。

### 2.3 当前 React Flow 边界可直接复用

现有 `ProjectContextGraph.tsx` 已提供：

- 固定位置的 React Flow nodes / edges；
- pan、zoom、fit all、fit Island、fit selection；
- Coordinate / Edge 点击、hover 与 Inspector；
- `prefers-reduced-motion` 下的相机动画降级；
- `onlyRenderVisibleElements`；
- route-owned 稳定 selection；
- 不可拖动、不可连线、不可删除的只读约束。

布局继续输出相同的 `ProjectContextLayout`；selection、Inspector 与 route 事件契约可以直接复用。
`ProjectContextGraph.tsx` 中布局 memo、auto-fit 与 text-scale viewport effect 需要随本设计调整。

## 3. 总体实现决策

采用：

```text
确定性径向初始化
        ↓
固定次数的有界约束松弛
        ↓
矩形碰撞收敛与边界归一化
        ↓
冻结后的 React Flow 静态位置
```

这里的“力导向”只表示布局纯函数内部的一次有限计算，不表示在 React 生命周期中保持一个
simulation，也不表示节点会持续晃动。

首版不引入 `d3-force`、ELK、Dagre 或独立图布局依赖。原因是：

- 当前图已经是简单、准确的二部 incidence graph；
- Coordinate 是宽矩形而非等半径圆，仍然需要自定义矩形碰撞；
- 稳定排序、固定 tick、缩放和 Island bounds 都需要 Buzz 自己控制；
- 当前 `layout.ts` 已是隔离良好的纯函数边界；
- 新增通用图库不能减少本次真正的领域工作。

如果未来实测证明单线程布局成为瓶颈，可以把同一纯算法移动到 Worker；这不属于首版。

## 4. 必须保持的不变量

### 4.1 领域不变量

1. 一条 Edge 仍只对应一个 Hub；
2. Hyperedge 不拆成多个虚构的二元 Edge；
3. Context Document 不成为隐式 Coordinate；
4. 图中没有箭头，Spoke 仍是无向 incidence segment；
5. 节点中心、半径、角度、距离、扇区和 Island 位置都只是 presentation；
6. 视觉邻近不代表重要性、因果、先后、owner、置信度或语义相似度；
7. 布局不修改查询集合、稳定 ID、selection 或 Inspector 目标。

### 4.2 稳定性不变量

对同一份规范化 layout topology 与同一个 text scale：

- 多次调用必须得到 byte-for-byte 相同的 canonical geometry；
- 输入数组顺序变化不能改变结果；
- 不调用 `Math.random()`、当前时间或浏览器布局测量；
- 所有遍历和冲突 pair 都按稳定 ID 排序；
- 最终坐标统一量化，避免浮点尾差进入 `layoutKey`；
- title、summary、状态、Context Document membership 等不改变 topology 的更新不能改变 geometry，
  但公开 Island 的事实字段必须反映当前 graph；
- 不把节点位置保存到 Local Storage、Project Context 或其他外部状态。

### 4.3 可读性不变量

- Coordinate 与 Hub 的矩形 bounds 不重叠；
- 不同 Island bounds 不重叠；
- 所有节点都位于其 Island bounds 内；
- Island label 的顶部安全区不能被节点覆盖；
- 每条 Spoke 仍连接真实 Hub 与真实 Coordinate；
- Spoke 两端必须分别选择最符合中心连线方向的上下左右 midpoint Handle；
- text scale 从 `0.75` 到 `1.5` 时，节点、间距、碰撞和 bounds 一起缩放。

## 5. 布局算法

### 5.1 统一的 component 输入

新增内部输入，不改变公开 DTO：

```ts
type RadialComponentInput = {
  stableKey: string;
  nodeIds: string[];
  adjacency: Map<string, Set<string>>;
  spokePairs: Array<{ sourceId: string; targetId: string }>;
  centerIds: string[];
};
```

`stableKey` 由该 component 的稳定 Node / Hub / Spoke 身份生成，不使用当前 Island index。这样在图中
新增一个不相连 Island 时，原有 Island 的局部角度不会改变；Island index 只属于后续跨 Island
packing。

All Context 中，一个 Island 对应一个 component。Focused Query 中，全部查询结果作为一个
`query-focus` component，但仍不生成 `ProjectContextIslandLayout`，保持现有产品语义。

### 5.2 选择中心

All Context 的每个 Island 使用确定性的近似 graph center，避免仅按最高 degree 选择靠近拓扑边缘
的星形节点：

1. 从稳定 ID 最小的节点开始 BFS，取稳定排序下最远的 `A`；
2. 从 `A` 再做一次 BFS，取最远的 `B` 并保留稳定 parent path；
3. 使用 `A -> B` 路径的中点作为 center candidate；
4. `A -> B` 的 node sequence 为偶数个节点、因而存在两个中点候选时，依次按 degree、Hub
   优先、稳定 node ID 破平局；奇数个节点时使用唯一中点。

该节点只用于布局中心，不表示最重要的项目对象。辅助技术说明继续明确这一点。

Focused Query 按当前 Anchor 与匹配结构选择中心，不需要给 `ProjectContextGraphModel` 增加新的领域
字段：

- 没有 Hub：所有 standalone Anchors 组成中心 constellation；
- `incident`：唯一 Anchor 放在中心；
- `exact`：唯一 Hub 放在中心，完整 Coordinate 集位于第一圈；
- 非空 `contains_all`：始终使用虚拟中心，全部 Anchors 围绕虚拟中心形成紧凑的第一圈，即使结果
  恰好只有一条与 Anchor 集完全相等的 Edge，也不能冒充 `exact` 布局；
- 其他 Hub / Coordinate 从 Anchor 集合向外展开。

### 5.3 建立确定性 BFS forest

从 `centerIds` 执行 multi-source BFS，得到：

```ts
depthById: Map<NodeId, number>
parentById: Map<NodeId, NodeId | undefined>
childrenById: Map<NodeId, NodeId[]>
```

规则：

- source、neighbor 和候选 parent 全部稳定排序；
- 一个节点有多个上一层邻居时，选择稳定排序最前者作为 layout parent；
- 其他真实 Spoke 保留为非树连线，不会从图中删除；
- 每个子树自底向上计算 `leafWeight`，用于分配角度扇区；
- 同级排序使用 presentation-only 的 `(stable 32-bit hash, stable node ID)`，而不是数组插入顺序。

32-bit Hash 只用于打散视觉角度；stable node ID 负责 hash collision 破平局。它不参与 EdgeKey、
Coordinate identity、topology canonical descriptor 或任何协议。

### 5.4 径向初始化

把虚拟中心视为一个完整圆周，并按子树 `leafWeight` 分配扇区：

```text
root / anchors              中心或内圈
depth 1                     第一圈
depth 2                     第二圈
depth N                     第 N 圈
```

每个节点的初始角度是其扇区中点。整个 component 再根据 `stableKey` 产生一个稳定 rotation，
避免所有 Island 都以完全相同角度展开。

每一圈的半径同时满足：

1. 与上一圈之间保留节点尺寸和最小 Spoke 间距；
2. 本圈所有节点沿圆周的累计切向需求能够容纳；
3. Coordinate 的宽高和 Hub 的尺寸使用 `scale = 1` 的 canonical layout 尺寸。

初始化时将每个矩形保守地视为半径为 `hypot(width, height) / 2` 的外接圆。一个 band 的半径至少
同时满足相邻外接圆的弦长需求，以及与前一个 band 的径向 clearance。初始化完成后必须先通过
真实 axis-aligned rectangle overlap 检查，才可保存为后续 relaxation 的安全回退。

同一 BFS depth 按以下固定容量算法拆成相邻的 physical bands：

1. 每个节点的 `arcDemand = 2 * outerRadius + tangentialGap`；
2. 当前 depth 的 `baseRadius` 由上一 depth 的最外 band、两侧最大 `outerRadius` 和 `radialGap`
   得到；
3. `radialPitch = 2 * maxOuterRadiusAtDepth + radialGap`；候选 band `k` 的半径是
   `baseRadius + k * radialPitch`；
4. band `k` 的可用容量固定为 `2 * PI * radius * RING_FILL_RATIO`，首版
   `RING_FILL_RATIO = 0.78`，为矩形角部、Bezier 离开节点的空间和 relaxation 留余量；
5. 选择能让候选 band 总容量容纳累计 `arcDemand` 的最小 band 数，再按稳定角度顺序顺次填充；
   当前 band 放不下完整下一个节点时，该节点进入下一 band。

band 只用于容纳矩形卡片；其中节点仍具有相同 topological depth，不能被解释为更远或更不重要。
对尺寸相同的高 fan-out fixture，这个容量和半径序列使最外半径按 `O(sqrt(nodeCount))` 增长，而
不是把 500 / 1000 个节点塞进一个半径 `O(nodeCount)` 的单圈。测试固定要求从 500 增至 1000 个
同尺寸节点时，最外半径比例小于 `1.75`。拆 band 的输入只包含 canonical 节点尺寸、clearance 与
稳定顺序，不依赖设备或耗时。

初始位置使用节点中心坐标；完成后再转换成 React Flow 所需的左上角坐标。

### 5.5 有界约束松弛

径向初始化后执行同步、固定次数的 relaxation。每个 tick 依次计算：

#### A. Spoke spring

每条真实 Spoke 将 Hub 与 Coordinate 的中心距离拉向目标距离。目标距离由两端节点尺寸和最小
连线空隙共同决定。

它只改善可读性，不修改 BFS depth 或图结构。

#### B. Radial tether

每个非中心节点受到一个较弱的径向约束，趋向其初始化时的目标半径。该约束防止普通 force
relaxation 把图重新压成一团，并保留“从中心向外发散”的整体形态。

#### C. Rectangle collision

Coordinate 和 Hub 都按 axis-aligned rectangle 计算碰撞，并额外保留视觉 clearance。发生重叠时：

- 沿较小穿透轴分离；
- 两个中心完全相同时，由稳定 pair hash 选择分离方向；
- 唯一真实 center 节点保持 pinned，位移施加到另一端；
- 多 Anchor 使用不可见 virtual center；Anchor 只受内圈 radial tether，不全部 pin；
- 其他 pair 平分位移。

碰撞候选使用空间网格，而不是每个 tick 做全量 `O(n²)` 比较：

1. 按最大节点尺寸与 clearance 选择 cell size；
2. cell size 不小于最大的 expanded rectangle side，使每个 rectangle 最多登记到四个 cell；
3. 只比较同 cell 或相邻 cell 的 pair；
4. pair key 去重后按稳定 ID 排序。

空间网格改善常见情况，但不能单独消除一个极密 cell 的 `O(n²)` worst case。因此每个 tick 还使用
`COLLISION_PAIR_BUDGET = max(4096, 64 * nodeCount)`。一旦唯一 candidate pair 数超过预算，不截断
后继续产出看似有效的 candidate，而是把本次 relaxation 标记为 `budget_exceeded`，整 component
回退到已验证无碰撞的径向 seed。相同预算也适用于 collision-only projection pass。这个上限与
固定 tick 一起构成同步主线程的硬运算边界。

#### D. Damping 与步长

每个 tick 累积位移后统一应用：

- 最大位移有上限；
- 最大位移随 tick 递减；
- 不保留跨 tick、跨 render 的速度状态；
- 单 root / 单 Incident Anchor 固定在局部 `(0, 0)`；
- virtual center 不是 node，多 Anchor 可以在内圈内解决碰撞；
- tick 中不做 centroid recenter，全部 tick 完成后只做一次整体平移归一化。

这是一套确定性约束松弛，不是具备惯性的实时物理引擎。

### 5.6 计算预算

只有结构可由初始化直接完整表达的简单 component 才走 analytic fast path：单节点、binary Edge、
单 Hub star，且 seed 已满足 rectangle clearance、有限值、Spoke 长度与 band postcondition。含有
多个 Hub、非树 Spoke、cycle 或重叠 Hyperedge 的 component 即使 seed 不重叠，也仍执行
relaxation，以改善真实 incidence links 的长度和网络形态。

其余 component 按 node 与 Spoke 数量使用固定预算：

| Component 规模 | Relaxation ticks |
|---|---:|
| simple analytic component 且 seed 已满足全部 postcondition | 0 |
| `nodes <= 80 && spokes <= 160` | 64 |
| `nodes <= 300 && spokes <= 600` | 40 |
| 其他 | 20 |

每个 tick 的 Spoke、radial tether 与 spatial-grid collision 都有确定上限。不存在“直到收敛”为止
的无界循环。

若最后仍有 rectangle overlap：

1. 再执行 8 次确定性的 collision-only projection pass；
2. candidate 仍不满足 postcondition 时，先回退到保存的 collision-free 径向初始化；
3. 径向初始化本身若未通过 postcondition，则按固定 `1.12` spacing factor 扩张 ring / band，
   最多重建三次；
4. 仍失败时，该 component 回退到现有分层布局。

回退只影响 presentation，不影响图内容和用户操作。

### 5.7 归一化、量化与 bounds

所有 component 先在 `scale = 1` 的 canonical presentation coordinate system 中求解。这样 Desktop
文字缩放只线性缩放尺寸和位置，不会重新选择 center、角度、band 或 relaxation path。

relaxation 完成后：

1. 计算所有 node rectangle 的最小包围盒；
2. 加入左右、底部 padding 和 Island label 顶部安全区；
3. 把 node center 转成左上角 `x / y`；
4. canonical node 坐标量化到 `0.25` unit，并把 `-0` 规范化为 `0`；
5. 在量化后的 rectangle 上重新验证 collision，重新计算并向外取整 bounds；量化导致 postcondition
   失败时按第 5.6 节回退，不能把量化前通过的 candidate 直接输出；
6. 使用量化后的中心差和两端各自的宽高，分别计算 `sourcePort()` 与 `targetPort()`；
7. 整体乘当前 text scale。缩放后不再次舍入，只做 finite 与 `-0` 规范化，避免破坏 canonical
   geometry 的线性比例；
8. 每次从当前 `graph.islands` 重新水合 `ProjectContextIslandLayout` 的事实字段。

端口选择按中心连线首先穿过节点矩形的哪一条边判断：比较
`abs(deltaX) / halfWidth` 与 `abs(deltaY) / halfHeight`。由于 Hub 与 Coordinate 的长宽比
不同，两端端口不强制互为 opposite。现有四个 React Flow Handle 仍位于各边中点，因此结果是
分别选择与中心射线最匹配的 side / midpoint Handle，不承诺连线落在射线与矩形的精确交点。

已有 `ProjectContextLayout`、`ProjectContextLayoutNode`、`ProjectContextLayoutSpoke` 的公开形状
保持不变。

### 5.8 输出顺序与几何顺序分离

角度、band 和 relaxation position 不得决定 React Flow node 的 DOM 顺序。最终 `layout.nodes`
按稳定逻辑顺序输出：

1. Focused Query 的 Anchors 优先；
2. Island index；
3. BFS depth；
4. node kind 与稳定 ID。

这样视觉位置改变不会把键盘 Tab 顺序变成近似随机的角度顺序。Island 背景仍由
`presentation.ts` 先输出，Spoke 继续 `aria-hidden`。

### 5.9 多 Island 打包

首版保留当前确定性的矩形 Island packing：

- 每个 Island 内部变为有机径向网络；
- Island 之间继续保留明确 whitespace；
- Island navigation、fit Island 与 factual label 保持稳定；
- packing 不表达 Island 之间的关系。

不在首版增加“Island 之间也做力导向”。互不连通的 component 没有真实关系，用视觉力把它们
互相吸引反而容易暗示不存在的语义。

## 6. Focused Query 行为

Focused Query 不再使用 `Anchors -> Hubs -> other Coordinates` 三列，但必须保留查询可解释性。

标准形态：

```text
                 Coordinate
                     |
       Coordinate — Edge Hub
                    /      \
              Query Anchor  Edge Hub — Coordinate
                    \      /
                     Coordinate
```

规则：

- Anchor 始终有现有 query-anchor 视觉标识；
- `incident` 的单 Anchor 位于中心；
- `exact` 的唯一 Hub 位于中心、完整 Coordinate Anchor 集位于第一圈；
- 非空 `contains_all` 始终使用 virtual center，Anchors 位于紧凑内圈；
- Hub 和其他 Coordinate 根据真实 incidence topology 向外发散；
- 没有匹配 Edge 时，单 Anchor 居中，多 Anchor 只展示 Anchor constellation；
- `layout.islands` 继续为空，页面继续说明这不是 Project-level Island count；
- selection、fit query、fit selection 和 deep link 行为不变。

## 7. 视觉与交互调整

### 7.1 节点和 Hub

现有 Coordinate card 与菱形 Hub 继续使用，不调整数据密度。布局变化本身已经提供主要的网状感，
不需要给 Hub 增加新的名称、方向或关系类型。

### 7.2 Spoke

继续使用现有无箭头 Bezier Spoke。两端端口根据各自矩形与最终中心连线独立选择，使曲线自然从
节点周围发散，避免宽 Coordinate 卡片与方形 Hub 被强制使用不合适的相反端口。

首版不增加 bundling。将多个 Spoke 合并成视觉总线可能掩盖一条 Hyperedge 的完整成员集合。

### 7.3 Island 背景

`project-context-graph.css` 中 Island 的主要 radial gradient 从偏左上调整到 component 中心，
使背景光晕与径向结构一致。Island 仍使用圆角矩形 bounds，因为 React Flow 需要准确的 fitBounds
和点击之外的可见边界。

Island label 仍固定在左上方，并由 layout 顶部安全区避免遮挡。

### 7.4 动画

- 不渲染运行中的 simulation；
- 不允许节点在空闲时漂移；
- 不新增自动“呼吸”或旋转动画；
- 保留现有相机 zoom / fit 的 220ms 动画；
- reduced-motion 下相机仍立即完成；
- live replacement 只有在 topology 真正改变时才重算布局；新布局一次性采用冻结位置，首版不对
  React Flow node transform 增加 CSS transition。

视觉高级感来自稳定的空间组织、曲线、留白与强调，而不是持续运动。

### 7.5 辅助技术文案

现有 screen-reader 描述需要扩充为：

> This is an undirected incidence graph. Radial center, angle, distance, and node placement do not
> express source, target, order, importance, similarity, or causality.

画布底部的可见提示调整为：

```text
Pan · Scroll to zoom · Undirected · placement carries no rank or causality
```

除现有 Query Anchor 标识外，中心节点不能因为被算法选为 center 而获得额外尺寸、颜色或领域标签。

### 7.6 键盘与可见区域

保持 Coordinate / Hub 内部 button、`aria-pressed`、Escape 返回焦点、Spoke `aria-hidden` 与
28px Spoke pointer target。`onlyRenderVisibleElements` 与当前 `minZoom = 0.12` 意味着超大图不可能
无条件把所有 offscreen node button 同时留在 DOM，因此验收边界为：

- 能在当前 min zoom 完整 fit 的 E2E fixture，Fit All 后全部节点可按稳定 DOM 顺序 Tab / Enter；
- 更大的图继续通过查询、Island navigation 或已有 selection route 定位目标，先 pan / Fit Selection
  使节点进入 viewport，再恢复内部 button focus；若 culling 导致 focus 早于 mount，图组件增加一个
  presentation-only 的 pan-then-focus helper，而不是关闭大图 culling；
- `onlyRenderVisibleElements` 不会使程序化 focus 丢失；
- reduced-motion 只改变相机 duration，不改变最终布局。

## 8. 性能与降级

### 8.1 同步纯函数边界

首版继续在 `React.useMemo()` 中同步调用 `layoutProjectContextGraph()`。因此实现必须满足：

- 固定 tick 数；
- spatial-grid collision；
- 不创建持久的 solver / simulation React state 或 module-level cache；
- 不订阅 animation frame；
- 不因 selection / hover 重新计算布局；
- canonical solver 只在 layout topology 变化时运行；text scale 变化只做线性尺寸、位置与 bounds
  变换。

完整 `ProjectContextGraphModel` 含 title、summary、lifecycle、Context Document membership 与
unavailable reason；这些非几何事实变化不能触发 solver。新增 presentation-only
`ProjectContextLayoutTopology` 与 collision-safe canonical descriptor：

```text
query mode / isAllContext
canonical Anchor IDs
canonical node IDs + kinds
Hub edgeKey + exact Coordinate membership
sorted Spokes
Island stable keys + membership
```

descriptor 使用上述字段的稳定排序、closed serialization 作为精确内容键，而不是 32-bit hash；
等键即等结构，不允许 hash collision 复用另一张图的位置。32-bit hash 只用于旋转与角度 tie-break。

构建轻量 topology 是 `O(nodes + spokes)`；React 按 canonical descriptor 保留 topology reference；
canonical solver 只依赖这份 topology。缓存的也只能是 `ProjectContextLayoutGeometry`：node positions、
sizes、ports 和 Island bounds。公开 `ProjectContextLayout.islands` 还携带 `coordinateKeys`、`edgeKeys`、
`contextDocumentIds` 与 index 等当前事实，必须在每次 render 从当前 `graph.islands` 重新水合。这样给
现有 Edge attach / detach Context Document 时位置不变，但 Island 的 Document count 会立即更新。

Display graph 仍可以因 metadata 更新重建 Flow element data，但复用同一 geometry。这里允许组件级
memo / ref 保存上一份 descriptor 和纯 geometry 结果，但不保存可继续运行的 simulation，也不形成
跨 Community 的 module singleton。

### 8.2 大图

现有 scale tests 覆盖 100、500、1000 Edge。新实现继续要求完整保留所有 Hub、Coordinate 和
Spoke，不能为了视觉效果采样或隐藏领域节点。

大 component 通过减少 tick 数降级；大量彼此独立的小 Island 则分别计算，避免形成一个全局
`O(totalNodes²)` 问题。

`radialLayout.ts` 在测试构建中暴露不进入产品 DTO 的 diagnostics：实际 ticks、grid registrations、
unique collision candidate pairs、是否命中 pair budget 以及 fallback reason。Scale tests 用这些计数
验证 500 -> 1000 的稀疏/星形 fixture 近线性增长；普通 CI 不用 wall-clock 作为硬门槛。实现验收另
记录一次 production build、DevTools 关闭条件下的布局 profile，用来判断同步首版是否出现明显主
线程 long task。

若完整图过大，React Flow 的 viewport virtualization、min zoom 与 Island navigation 继续承担浏览
能力。本次不引入 semantic clustering 或 level-of-detail，因为它们会改变用户看到的事实集合。

### 8.3 故障降级

布局函数必须拒绝向 React Flow 输出 `NaN`、`Infinity` 或负尺寸。任一 relaxed candidate 在以下
情况先回退到保存的确定性径向 initialization：

- 非有限位置；
- relaxation 后仍存在 rectangle overlap；
- bounds 无法完整包含节点；
- 内部 invariant 失败。

只有径向 initialization 在固定 spacing 扩张后仍不满足 postcondition，才回退现有分层布局。
UI 不展示错误 Toast，因为图事实仍然完整；开发测试必须能明确命中并验证两级 fallback。

## 9. 代码结构与修改清单

### 9.1 `radialLayout.ts`

新增 feature-local 纯算法文件，避免把当前已经约 450 行的 `layout.ts` 扩张为难以维护的单文件。
该文件只接收排序后的节点尺寸、incidence links、center / Anchor ID 与 stable seed，不依赖 React、
React Flow 或来源 metadata：

```text
buildDeterministicBfsForest()
seedRadialPositions()
splitDepthIntoPhysicalBands()
relaxRadialPositions()
resolveRectangleCollisions()
finalizeRadialPositions()
```

### 9.2 `layout.ts`

保留公开输出类型和入口，负责 orchestration：

```text
buildProjectContextLayoutTopology(graph, queryMode)
projectContextLayoutCanonicalDescriptor()
adjacencyForIsland()
chooseLayoutCenters()
layoutRadialIsland()
layoutFocusedRadialGraph()
layoutLayeredFallback()
normalizeComponentLayout()
packIslandLayouts()
materializeProjectContextLayout(geometry, graph)
layoutProjectContextGraph()
```

当前 `layeredNodeIds()` 与 `layoutOneIsland()` 的主要逻辑移动到 fallback；不再作为正常默认路径。

### 9.3 `ProjectContextGraph.tsx`

不改变领域数据或事件结构。增加稳定 topology reference 与独立的 query identity：

- canonical solver memo 只依赖 topology canonical descriptor，text scale 由后续
  `O(nodes + islands)` 变换应用；
- 每次使用当前 graph 重新 materialize Island 的 Coordinate / Edge / Context Document IDs 和 count，
  不能缓存陈旧事实；
- Flow element data 仍可随 title / summary / lifecycle 更新；
- 只在首次结果或 query identity 变化时自动 `fitView`；
- metadata refresh、text zoom、Inspector resize 和同 query 的 live topology replacement 不重置
  用户 viewport；
- text scale 变化前记录 focal screen point：优先选中节点中心，否则取当前 viewport 中心对应的
  graph point；应用线性缩放 geometry 后只调整 viewport translation，使同一焦点继续落在原 screen
  point，不调用 Fit All，也不改变 React Flow camera zoom；
- selection 仍由 route 保留，用户可以继续使用 Fit Selection；
- `layoutKey` 仍可用于清理 hover，但不再作为无条件 auto-fit key。

这样新的 solver 不会因为一个 summary 更新重复运行，也不会在用户探索图时自动夺走相机控制。

### 9.4 `ProjectContextSpoke.tsx` 与 handles

首版继续使用四个现有 Handle 和 `getBezierPath()`。只有测试发现径向图中存在明显反向弯折时，才
允许根据端点距离调整 curvature；不增加八方向 Handle 或自定义 route point。

### 9.5 `project-context-graph.css`

只调整 Island 中心光晕和必要的径向视觉留白，不改变 Coordinate / Hub 的领域标签、选中强调、
tombstone 或 unavailable 表现。

### 9.6 不修改的文件域

本次不修改：

- `../../../desktop/src-tauri`；
- `../../../crates/buzz-project-context`、SDK、Relay、DB、CLI、ACP；
- `tauriProjectContext.ts` DTO；
- query、live sync、route 或 Inspector；
- Project View / Document / Meeting 内容展示；
- Context Document binding 展示。

## 10. 测试设计

### 10.1 Layout 单元测试

新增 `../../../desktop/src/features/project-context/radialLayout.test.mjs` 覆盖纯 solver、碰撞和 fallback；扩展
`layout.test.mjs` 覆盖公开 layout contract：

1. 同一输入 byte-for-byte 稳定；
2. 对 Edge、Coordinate、detail 输入数组做 permutation 后结果不变；
3. title / summary / lifecycle / Context Document membership-only 更新得到相同 topology descriptor 与
   geometry，但公开 Island 事实与 count 从当前 graph 重新水合；
4. 一个星形 Hyperedge 从中心向外发散，而非落在同一纵列；
5. cycle、重叠 Hyperedge 与高 fan-out 图无 rectangle overlap；
6. Hub / Coordinate 都在 Island bounds 内；
7. Island label 顶部安全区无节点；
8. 不同 Island bounds 不重叠；
9. Spoke 两端分别选择最符合中心连线方向的 side / midpoint Handle；替换现有“端口必须
   opposite”断言；
10. text scale `0.75 / 1 / 1.5` 不重新选择 center / band / relaxation path；scaled geometry 与
    `canonical * scale` 的误差只允许浮点计算 epsilon，不允许二次量化漂移；
11. `incident` 单 Anchor、`exact` 唯一 Hub + 全 Anchors、`contains_all` virtual center 与 no-match
    focused layout；
12. relaxed candidate 的 overlap / NaN 回退径向 initialization，径向初始化失败再回退 layered；
13. 量化后所有位置均为 finite、符合 `0.25` canonical grid 且不存在 `-0`；
14. 添加一个不相连 Island 不改变原 Island 归一化后的局部 geometry；全局 packing 位置允许变化；
15. 同一 Island 新增叶子时允许受影响子树重新分配角度，但 stable rotation 不得随机整体镜像翻转；
16. `layout.nodes` 的 DOM 输出顺序不依赖最终 angle / x / y。
17. 同一 topological depth 拆成多个 physical bands 后保持相同 BFS depth，半径随高 fan-out 近似按
    面积扩展，而不是退化成一个超大单圈。
18. canonical descriptor 不使用 32-bit identity；角度 hash collision 由 stable ID 确定破平局。

避免只对一整份固定坐标写脆弱 golden；应主要验证确定性、无重叠、拓扑、bounds 和径向分布。

### 10.2 Scale 测试

保留 `scale.test.mjs` 的 100 / 500 / 1000 Edge 完整性检查，并增加：

- 输出 node / spoke / Island 数量完全不变；
- 布局重复调用稳定；
- 所有位置 finite；
- 无全局无界迭代；
- 1000 个独立小 Island 不进入全局 pairwise collision；
- 1000 条 Edge 共享一个 Coordinate 的单一巨型 Island；
- 一条包含 500 / 1000 Coordinate 的高 fan-out Hyperedge；
- 多条 Edge 大量共享 Coordinate 的 dense fixture。
- test-only diagnostics 证明 ticks 不超过表中预算，grid registrations 为 `O(nodes)`，星形 fixture
  从 500 增至 1000 时 collision candidate pair 数不超过三倍；dense fixture 超预算时必须明确回退，
  不能部分处理后宣称成功。

不使用严格毫秒断言，避免 CI 机器差异导致 flaky test。计算预算由 tick、pair budget 与 diagnostics
断言固定；人工验收记录 profile，但不把开发机耗时冒充跨设备 SLA。

### 10.3 Graph 与 Presentation 测试

`graph.test.mjs` 继续负责领域图投影：

- 每条领域 Edge 仍是一个 Hub；
- 每个 incidence 仍是一条 Spoke；
- selected Edge 对应完整 Coordinate 集；
- 重叠 Edge 不被合并；
- Context Document 不成为 Node。

`presentation.test.mjs` 负责 Flow 映射：

- selected / hovered emphasis、ARIA-hidden Spoke 与 query-anchor 不回归；
- Island hue 与稳定顺序不变；
- Flow node / edge 数量与 graph + layout 一致；
- positions、sizes 与两端 Handle IDs 精确来自最终 geometry；
- Context Document membership-only 更新不移动 geometry，但 Island count 使用当前事实。

### 10.4 Desktop E2E

更新 `../../../desktop/tests/e2e/project-context.spec.ts`：

1. All Context 的两个 Island 仍有准确 label、数量与不同 hue；
2. 通过 bounding boxes 验证单个 Island 内节点分布在中心多个方向，而非固定列；
3. binary Edge、Hyperedge overlap、Coordinate 点击与 Inspector 保持可用；
4. fit Island、fit all、fit selection 仍能完成；
5. light / dark 下 Island 与 Spoke 可读；
6. live replacement 合并或拆分 Island 后节点数和 Island 数准确；
7. reduced motion 下没有持续节点运动；
8. 窄窗口与 Desktop zoom 后无节点 overlap；
9. 重新加载同一 fixture 后关键节点位置一致；
10. layout ready 后跨两个 `requestAnimationFrame` 读取的位置完全相同；
11. 可完整 fit 的 fixture 中，Fit All 后 Coordinate / Hub 可通过 Tab / Enter 操作；超大 fixture 通过
    query / Island navigation -> pan / Fit Selection -> focus 的路径恢复 offscreen target；
12. 可见提示明确 placement 不表示 rank 或 causality；
13. 更新网状布局截图，按项目规则等待所有动画结束后再截图；
14. 重新生成阶段七记录列出的全部 7 张截图并检查 hash distinctness，只把实际变化者写入本次新的
    `desktop-organic-graph-layout-acceptance.md`；`desktop-stage7-acceptance.md` 继续保留
    `f660d8425` 的历史事实，不覆盖旧 SHA-256。

截图用于视觉 review，不作为领域正确性的唯一证明。

## 11. 实施阶段

### 阶段一：径向纯布局

- 新增 `radialLayout.ts` 与 presentation-only topology canonical descriptor；
- 抽出当前 layered fallback；
- 实现 center、BFS forest、扇区和自适应 ring；
- 实现高 fan-out physical band 拆分；
- 接入 All Context 与 Focused Query；
- 保持当前 Island packing 与公开输出类型。

### 阶段二：约束松弛

- 实现 Spoke spring、radial tether 与矩形 collision；
- 加入 spatial grid、固定 tick 与量化；
- 加入 overlap retry、finite guard 和 fallback。

### 阶段三：视觉与可访问性

- 调整 Island 中心光晕；
- 复核 Bezier 端口与曲率；
- 将 auto-fit 从 raw `layoutKey` 改为首次结果 / query identity；
- 更新 screen-reader 文案；
- 固定逻辑 DOM 顺序与可见 placement 提示；
- 验证 reduced motion、light / dark 与 Desktop zoom。

### 阶段四：测试与视觉验收

- 完成 radial layout / public layout / graph / scale / presentation tests；
- 更新 Project Context E2E；
- 生成单 Island、多 Island、高 fan-out、重叠 Hyperedge 的截图；
- 新建 `desktop-organic-graph-layout-acceptance.md`，记录本次代码基线、性能 profile、7 张回归截图
  与实际变化的 SHA-256；
- 运行 Desktop check、typecheck、unit tests、build:e2e 与 Project Context E2E。

## 12. 验收标准

交付完成必须满足：

1. 默认图不再是从左到右的规则分层列，而是围绕中心发散的网状布局；
2. 图仍准确呈现 Coordinate、Edge Hub、Spoke、Hyperedge 与 Island；
3. Context Document 不被画成节点；
4. 同一输入与 text scale 的布局完全确定；
5. 页面静止后不存在持续 simulation 或节点漂移；
6. Coordinate、Hub 和 Island bounds 不重叠；
7. Focused Query 按 Incident Anchor、Exact Hub 或多 Anchor virtual center 确定中心，no-match 仍只
   显示 Anchor；
8. selection、hover、Inspector、fit、deep link 与 live refresh 行为不回归；
9. radial center、角度和距离不被描述为领域语义；
10. 100 / 500 / 1000 Edge 测试仍完整保留所有图元素；
11. 没有新增协议、DTO、持久化位置或通用图引擎依赖；
12. metadata-only 更新不运行 solver，text zoom 与 live refresh 不无条件重置 viewport；
13. Desktop 静态检查、单元测试和 Project Context E2E 全部通过。

## 13. 实施结果

本设计已在 Desktop 前端完成实现：

- 新增纯函数、确定性的离线径向布局求解器；
- All Context 与 Focused Query 均改为有机网状布局，并保留旧分层算法作为确定性回退；
- 高扇出节点使用多物理环带，避免单环半径随节点数线性膨胀；
- 布局在 React Flow 首次渲染前完成并冻结，不存在持续 simulation；
- geometry 按 topology descriptor 复用，title、summary、状态与 Context Document membership 更新不会
  重跑求解器，展示事实仍从当前 graph 重新水合；
- text scale 只缩放 canonical geometry，并保持当前选中节点或 viewport 中心的屏幕位置；
- 初次查询按完整 layout bounds 自动适配，后续 metadata 与 text scale 更新不再无条件重置 viewport；
- Coordinate、Hub、Spoke、Island、selection、Inspector 与无向关系语义均保持不变；
- 未修改 Tauri、Relay、数据库、CLI、Nostr 协议或 Project Context 领域模型。

交付验证包括 Desktop 静态检查、TypeScript 类型检查、完整单元测试、E2E 构建，以及
Project Context Desktop E2E。视觉截图作为本次实现的人工 review 证据，不改写历史阶段验收记录。
