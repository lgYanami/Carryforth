# Project Context 统一语义计算实现计划

> 状态：实现设计已冻结；U0–U1 已交付；U2–U7 待交付
>
> 日期：2026-08-16
>
> 代码基线：`feat/semantic-engine`，`ab395ff6f`
>
> 上位规范：
> [Project Context 统一语义检索引擎规范](project-context-unified-semantic-retrieval-engine-spec.md)
>
> 兼容基线：
> [Project Context 语义检索兼容基线记录](project-context-semantic-retrieval-compatibility-baseline.md)
>
> 本阶段范围：统一 query input、Provider encoding、generation-bound query vector 与
> current-head exact scorer；保持四个逻辑 operation 的 closed policy/result 及三个公开 surface 不变

## 0. 已确认决策

1. 第一阶段是零产品行为迁移，不调整 query 文本、权重、floor、budget、root、beam、path retention、
   result 或公开错误合同。
2. 四个逻辑 operation 继续独立；两个 one-hop operation 继续共享既有 closed tagged family。
3. 三个既有公开 surface、Event kind `40912/40913/40914`、capability、gate、SDK 与 CLI 保持不变。
4. 所有操作共享一套 query-input 表达、Provider encoding 能力、generation-bound vector 和
   current-head exact-scoring kernel。
5. “query encoding contract digest”只证明实际输入由哪份 closed serializer 产生；它不是 CLI 身份，
   也不决定向量能否参与 exact cosine。
6. Phase 1 仍逐字保留当前 Coordinate template、Q0 和 Qi 输入。移除 JSON 字段或 contract marker 属于
   后续 query-text v2 迁移，必须另行评测，不能混入本轮重构。
7. 不新增万能查询 DSL、动态 SQL、caller-supplied vector、模型、scope、权重、floor 或执行计划。
8. 不新增 schema、migration、semantic index 或 canonical Project Context 图变化。
9. 不在本阶段实现排队、重试、backoff、circuit breaker、公平调度或统一资源配额；这些属于第二、
   第三阶段。
10. 旧路径在差分与回滚窗口结束前保留；任何不一致先停止迁移，不用 fallback 掩盖差异。

## 1. 目标

当前系统不是四套完全独立的语义引擎：one-hop 与完整路径已经共享 graph Q0、query encoder、ticket、
current-head exact scorer；Coordinate search 也共享 Provider transport、one-shot envelope、active generation
和 read snapshot。

真正需要收口的主要问题是：

- graph query ticket 把 source generation、embedding space 与 graph query template digest 绑定在同一个
  `QueryCompatibilityFences` 中；
- Coordinate search 因使用独立模板，只能维护另一种 encoded vector 和 DB vector wrapper；
- Coordinate search 复制了授权、active generation、current-head、embedding join、cosine 量化和 top-K SQL；
- Provider 对 Coordinate 与 Q0/Qi 暴露两套 adapter；
- 同一种已验证向量进入 exact scorer 前有不同的类型与验证路径。

本阶段完成后的目标数据流是：

~~~text
closed operation request
  -> operation-owned canonical input builder
  -> shared validated semantic input bundle
  -> shared Provider encoder
  -> shared Provider-encoded embedding bundle
  -> authorized ticket binder
  -> shared generation-bound vector bundle
  -> existing authorized RR snapshot
  -> shared current-head exact scorer
  -> operation-owned scope/ranking/budget/projection
  -> existing release fence and public result
~~~

成功标准不是“文件变少”，而是四个 operation 在相同 generation、相同输入和相同 snapshot 下，由同一
组基础原语产生与兼容基线完全相同的基础分数和公开结果。

## 2. 范围与非目标

### 2.1 本阶段交付

- 一种 closed、不可任意构造的 canonical semantic input 表达；
- 一种有序 input/vector bundle，支持单输入与 Q0 加多个 Qi；
- source generation / embedding-space fence 与 input encoding contract 的解耦；
- 一种 generation-bound、input-digest-bound 的 validated query vector；
- 一个共享 Provider batch encoder；
- 一个共享 current-head exact-scoring kernel；
- 四个 operation 的 typed adapter；
- 同 vector、同 RR snapshot 的 legacy/new differential seam；
- 分 operation 灰度、回滚和删除旧路径的机械门；
- deterministic、disposable pgvector 和有条件真实 Provider 资格记录。

### 2.2 明确不做

- 不统一公开 DTO、wire extension、Event kind、capability 或 CLI；
- 不统一四种 operation 的最终排名公式或 result；
- 不改变 Coordinate 的 direct cosine/no-floor/K+1 语义；
- 不改变 one-hop 的 scope、Edge aggregation、preview、coverage 或 omission；
- 不改变完整路径的 Q0/Qi fusion、anchor、coherence、root/MMR、traversal 或 packing；
- 不改变 one-shot exact snapshot release 与完整路径现有 release 语义；
- 不跨请求缓存 query vector，也不持久化 input、vector 或 score；
- 不让 Relay、SDK 或 CLI 直接调用内部 scorer；
- 不把 approximate ANN 引入现有 exact-scoring 合同；
- 不以 Phase 1 的成功宣称可靠性运行时、资源治理或 production SLO 已完成。

## 3. 当前实现基线

### 3.1 四个 operation / 三个 surface

| 逻辑 operation | 当前 input/vector | 当前 exact scoring | 当前公开 surface |
| --- | --- | --- | --- |
| whole-graph Coordinate discovery | Coordinate template + `EncodedCoordinateSearchQuery` | `semantic_coordinate_search.rs` 专用 SQL | 40913 Coordinate search |
| Coordinate → incident Edge | graph Q0 + `EncodedSemanticQuery` | common exact scorer + scoped relation Document policy | 40914 one-hop tagged family |
| Edge → member Coordinate | graph Q0 + `EncodedSemanticQuery` | common exact scorer + complete Edge member policy | 40914 one-hop tagged family |
| bounded complete path | ordered Q0 + Qi bundle | common exact scorer + graph ranking/traversal | 40912 semantic graph query |

### 3.2 已经共享、不得重复实现

- Foundation source overview、semantic units、embedding generation 与 current heads；
- physical Provider HTTP transport、model/dimension validation；
- one-shot admission/ticket/reservation/confirm/release envelope；
- graph query ticket 与 repeatable-read read transaction；
- one-hop 和完整路径使用的 exact score SQL；
- Hyperedge、incident relation、canonical source hydration helpers；
- SDK/CLI request-result binding 与签名验证。

### 3.3 当前兼容差异

Phase 1 必须把以下差异当作 oracle，而不是顺手“修正”：

- Coordinate 输入与 graph Q0 字节不同；
- Q0 与 one-hop 输入相同；Qi 包含当前 context overview；
- Coordinate result 报告 Coordinate query contract digest；one-hop/完整路径报告 graph query digest；
- Coordinate tie 使用 `score DESC, ProjectContextCoordinate::Ord ASC`；
- graph exact scorer 的 source ordering 不是 Coordinate canonical ordering；
- Coordinate top-K 使用 K+1 判断 `truncated`；
- one-hop 在 closed scope 内评分后由各 variant 排名与投影；
- 完整路径可包含多个 channel，并依赖 channel identity；
- one-shot 无内部 retry并要求 exact release snapshot；完整路径保留其现有 retry/release画像。

## 4. 目标类型模型

以下名称是本计划冻结的责任边界。实现时可以在不改变责任和可验证性的前提下微调 Rust 名称，但不得退化
为弱类型 map、任意字符串 scope 或 caller-controlled DSL。

### 4.1 `SemanticGenerationFences`

共同的 generation 兼容性只包含：

~~~text
community_id
generation_id
source_generation_contract_digest
embedding_space_fence
model
dimensions
~~~

`community_id + generation_id`组成exact generation key；数据库并不承诺generation UUID跨Community全局
唯一。即便两个Community故意复用同一个generation UUID和相同model contract，vector也不能跨tenant使用。
同一Community中，即便两个generation恰好使用相同model contract，前一个generation产生的vector也不能
自动进入后一个generation。digest与active generation绑定，model/dimensions由validated model contract
提供。该结构不携带Coordinate、Q0或Qi的template digest。

`SemanticGraphQueryTicket` 在迁移期间可以保留现名，但其 scorer-facing 约束必须改为
`SemanticGenerationFences`。旧 `QueryCompatibilityFences` 暂时作为兼容 view 保留，用于当前 result、fleet
或测试需要的 graph query digest；不能继续作为所有 vector 的唯一准入类型。

### 4.2 `SemanticQueryInput`

一个输入包含：

~~~text
request_id
channel_id
channel_kind
encoding_contract_digest
input_digest
exact_utf8_text
~~~

约束：

- `exact_utf8_text` 是实际送入 Provider 的完整字节；
- `input_digest` 对实际字节做 domain-separated 绑定；
- `encoding_contract_digest` 标识 serializer/template 版本，只用于 closed builder、审计和结果观察；
- `channel_kind` 是 closed internal identity，例如 Coordinate query、problem Q0、conditioned Qi；
- 构造函数不公开接受任意 contract digest；只有已审查的 Coordinate、Q0、Qi builder 能创建输入；
- `Debug`、错误、日志与 metrics 只暴露类型和字节数，不暴露文本、digest关联身份或内容。

Phase 1 的三个 builder 必须保持现有字节：

1. Coordinate search v1；
2. semantic graph problem Q0 v1；
3. semantic graph conditioned-context Qi v1。

### 4.3 `SemanticQueryInputBundle`

bundle 是一个有界有序集合：

- Coordinate 与两个 one-hop operation 的长度严格为 1；
- 完整路径为 Q0 后跟零到多个按现有 canonical Coordinate 顺序排列的 Qi；
- request identity 必须一致；
- channel identity 必须唯一；
- input contract 与 channel kind 的组合必须是 closed allowlist；
- 总输入数量、单项字节和总字节保持现有限制；
- Qi omission 的现有 coverage 继续由完整路径 owner 记录。

bundle 不负责把不同 operation 或不同请求合批。跨请求 batching、cache 和调度属于后续资源治理。

### 4.4 `ProviderEncodedSemanticInput`与`GenerationBoundQueryVector`

Provider encoder先返回一个仍未声称绑定exact generation UUID的内部结果：

~~~text
ProviderEncodedSemanticInput
  request_id
  channel_id
  channel_kind
  source_generation_contract_digest
  embedding_space_fence
  encoding_contract_digest
  input_digest
  response_model
  validated_embedding
~~~

它只能证明response与closed input及目标model space相符。DB层随后使用authorized ticket创建最终vector。

共享 vector 包含：

~~~text
request_id
channel_id
channel_kind
SemanticGenerationFences
encoding_contract_digest
input_digest
response_model
validated_embedding
~~~

构造时必须验证：

- Provider response 数量、顺序和模型与 input bundle 一致；
- exact generation ID、generation contract、model/dimensions/embedding-space 与 ticket 一致；
- vector 有限、非零、维度精确；
- input digest 与 Provider 前验证过的 exact input 一致；
- request/channel identity 没有错配或重复。

Provider本身只返回model与raw embedding，并不能证明active generation UUID。final vector必须由DB暴露的
ticket binder创建：binder接收authorized ticket和`ProviderEncodedSemanticInput`，把ticket的exact
`generation_id`与generation fences写入vector；scorer在同一read ticket上再次比较。Relay只能请求该binder，
不能仅凭model相同自行构造`GenerationBoundQueryVector`。

exact scorer 只以 generation/model/embedding-space 判断向量能否与 source embedding 比较；它不把
`encoding_contract_digest` 当作 cosine 兼容条件。operation adapter 仍必须检查自己接受的 closed
channel kind/encoding contract，并把现有 query contract digest投影到公开 result。

这项检查不能只停在Relay。每个operation-facing DB method在进入scope前必须再次验证closed
`channel_kind + encoding_contract_digest + bundle shape`：Coordinate只接受单个Coordinate-v1输入，两个
one-hop variant只接受单个problem-Q0，完整路径只接受现有Q0后接canonical Qi。共同scorer内部不能提供绕过
这些adapter的公开任意vector入口。

因此，本阶段不采用 `GenerationBoundQueryVector<OperationContract>` 这种按 CLI 泛型隔离的设计。输入版本
仍被完整绑定，但同一份实际语义输入不会因为调用入口不同而被 scorer 人为隔离。

### 4.5 `GenerationBoundQueryVectorBundle`

vector bundle 与 input bundle 一一对应，并保留顺序和 channel identity。它是一次 Provider batch 的唯一
ticket-bound输出，也是differential与snapshot重试未来可复用的最小单元。Provider边界本身的输出是有序
`ProviderEncodedSemanticInput` bundle。

Phase 1 只建立类型与验证，不新增复用、cache 或 retry。完整路径仍一次提交现有 ordered Q0/Qi batch；
one-shot 仍一次提交单输入 batch。

## 5. 统一 query encoding

### 5.1 共同 Provider 边界

`VolcengineSemanticProvider` 对交互式查询只保留一个 bounded batch encoding primitive：

~~~text
validated SemanticQueryInputBundle
  -> exact ordered UTF-8 texts
  -> one Provider request
  -> ordered raw embeddings
  -> ProviderEncodedSemanticInputBundle
~~~

Coordinate 的专用 `encode_coordinate_search` 与 graph `encode_queries` 在迁移期作为 thin compatibility
adapter；所有 operation 切换后删除重复 adapter，不删除底层 Foundation indexing encoder。

共同边界必须保持：

- one logical request 的 Provider attempt/batch 画像不变；
- 发送文本字节不变；
- response model、index、count、dimension、finite/non-zero 验证不变；
- 任何一个向量失败则整个 bundle fail closed；
- 不记录请求文本、response body 或 vector；
- 不在 Phase 1 增加网络 retry、redirect、cache 或跨请求合批。

### 5.2 deterministic fake 边界

现有 deterministic fake 生成会使用 request-local channel identity，因此不能用“不同 request 下同文本的
fake vector相同”证明生产 Provider 语义相同。

Phase 1同时保留两种用途明确不同的test seam：

1. 现有legacy deterministic fake保持算法与golden不变，只服务历史兼容测试；
2. 新增shared recording/byte-deterministic Provider seam，其embedding只由exact Provider bytes与model
   space决定，channel/request只用于response绑定，不参与数值生成。它用于证明相同输入字节经过共同encoder
   得到相同向量。

legacy/new差分测试必须：

- 对 legacy/new 路径只编码一次；或
- 注入同一组固定 `f32::to_bits()` vector bundle。

不能悄悄修改现有fake后重写golden。兼容基线v1的synthetic vector digest保持不变。Phase 1新建独立
differential fixture，不改写已经冻结的baseline manifest来迎合新实现。

### 5.3 后续 query-text v2 的边界

Phase 1 完成只表示共同 encoder 已建立，不表示 Coordinate 与 Q0 的实际文本已经统一。

若后续确认 Provider 只应接收自然语言本身，必须单独设计 query-text v2：

- 明确 problem、context overview 和分隔方式；
- 对相同文本验证跨 operation vector一致；
- 重新评测候选、排名和known-negative；
- 版本化 input/query contract digest；
- 灰度、回滚和fleet attestation。

这项工作不得回填到 Phase 1 的“零行为”提交。

## 6. 统一 current-head exact scorer

### 6.1 scorer 的唯一责任

共享 scorer 负责：

1. 在已授权的 repeatable-read snapshot 中确认 active generation；
2. 只读取 active generation 下 eligible、current、model-compatible 的 overview embedding；
3. 将 closed scope 解析成有界 source集合；
4. 对一个或多个 query channel 计算 exact cosine；
5. 使用现有固定点量化生成 direct `Score`；
6. 保留closed adapter当前需要的raw-distance排序语义与fixed-point score，两者不得互相替代；
7. 返回 source identity、current-head evidence、graph role evidence、channel identity 和 direct score；
8. 在调用者要求的 closed global top-K scope 中执行稳定 K+1。

scorer 不负责 context gain、anchor、coherence、Edge grouping、root/MMR、beam、path score、coverage文本或
public result packing。

完整路径中的source-to-source coherence比较两个current source embedding，并不是query-to-source scoring。
它可以复用current-head observation与fixed-point quantizer，但不能伪装成`GenerationBoundQueryVector`输入，
也不在Phase 1强行并入同一个API。

### 6.2 closed scope resolver

内部只支持实现代码声明的 closed scope，不公开给 wire/SDK/CLI：

| internal scope | 来源集合 | 排序/上限 owner |
| --- | --- | --- |
| global graph Coordinates | active Edge 中去重后的 Coordinate sources，all-current | Coordinate operation，canonical Coordinate tie + K+1 |
| graph recall sources | eligible Coordinates与relation Documents，含现有 lifecycle/initial规则 | complete-path recall contract |
| explicit source set | 调用者已通过结构读取获得的有界 source identities | one-hop或graph policy |

不得提供任意 SQL predicate、source family列表、caller-supplied lifecycle表达式或动态 ordering。Relay不能直接
构造 scope；它只能调用 DB 的 closed operation method。

### 6.3 共享 SQL/row kernel

共享内核必须只保留一份：

- authorized reader/project与projection parity；
- active generation与model contract验证；
- active Edge Coordinate / relation binding role materialization；
- current source head、unit set、overview unit与embedding join；
- vector dimensions/norm与exact distance；
- cosine到fixed-point `Score`量化；
- current-head与graph-role row解析。

实现可以使用一个静态 closed scope参数，或由不可注入的静态SQL片段组合多个closed wrapper；不得复制整套
authorization/current-head/scoring CTE，也不得拼接caller输入。

共享 row 至少保留当前 graph scorer需要的：

- source identity与current-head evidence；
- `is_coordinate`与incident Edge keys；
- relation binding provenance；
- per-channel direct score与rank；
- Coordinate canonical family/subtype ordering key。

现有graph recall以raw distance产生per-channel rank；Coordinate则先量化为fixed score，再用Coordinate canonical
identity打破量化同分。共同kernel必须同时支持这两种已冻结排序语义，不能把某一条的`channel_rank`直接
复用于另一条。

### 6.4 Coordinate 的稳定排序与 K+1

Coordinate search 不能直接复用当前 graph source lexical tie，因为这会改变同分结果。它必须在同一个
snapshot中保持：

~~~text
score DESC
ProjectContextCoordinate::Ord ASC
LIMIT K + 1
~~~

且必须满足：

- relation-only Context Document永不进入候选；
- 一个 Document同时是Coordinate时可进入；
- 同一Coordinate连接多个Edge时只出现一次；
- terminal Coordinate继续是all-current候选；
- missing/building/failed/stale/ineligible head不进入候选，但不会令整个搜索失败；
- `truncated=true`只由同snapshot的第K+1个eligible Coordinate产生。

测试必须包含“raw distance不同、但量化后fixed score相同”的候选，证明Coordinate仍按canonical identity
打破量化同分，而不是被raw-distance顺序提前决定。

因此 Coordinate 的 closed top-K projection仍由 Coordinate operation拥有；共享 scorer只提供同一套
current-head、score和必要的canonical ordering key。

### 6.5 One-hop 继续保留专属 policy

Coordinate → incident Edge：

- 先加载完整incident relation refs；
- 仅对current binding Documents评分；
- 按既有合同以最佳Document分数聚合Edge；
- 保持每Edge preview、coverage、omission、tie和truncation；
- 不返回member Coordinates。

Edge → member Coordinate：

- 先验证完整Hyperedge与identity/materialization上限；
- 只对该Edge的完整member Coordinate sources评分；
- 保持all-current、preview、coverage、omission、tie和truncation；
- 不返回relation Documents或其他Edge。

两者只是改用共同 vector与scorer，不修改现有 `scoped_search` policy。

### 6.6 完整路径继续保留专属 policy

完整路径仍由现有 owner完成：

- Q0/Qi channel解释与context gain；
- automatic/explicit root selection与neutral lane；
- relation/target/coherence/floor；
- 无向完整Hyperedge traversal；
- cycle、beam、global budget与stop precedence；
- path retention、coverage和response packing。

Phase 1 只让它消费共同 input/vector bundle与共同 direct score row。不能在迁移中调整已知相关性问题。

## 7. Snapshot、安全与生命周期边界

### 7.1 必须共享

- host-derived Community/Project；
- caller membership、owner与ban判断；
- projection signer与parity；
- active generation、model/dimensions和embedding-space；
- writer DB repeatable-read snapshot；
- current source head与active graph role；
- Provider egress前授权和release前共同安全条件。

### 7.2 必须保持operation-specific

- Coordinate one-shot开始RR read时只要求active generation contract仍相同；它以实际read ticket作为结果
  snapshot，并在release传`expected_snapshot: Some(read_snapshot)`；不得悄悄收紧为bootstrap projection/
  context revision必须不变；
- one-hop除相同generation外，在评分前还要求read ticket的projection generation与context revision等于
  bootstrap ticket；release同样传`expected_snapshot: Some(read_snapshot)`；
- 完整路径继续使用当前多阶段snapshot与`expected_snapshot: None`兼容语义；
- 完整路径现有generation/context churn受限root retry不迁入one-shot；
- 各surface现有HTTP status、closed code、`retryable`和CLI exit映射不变。

共同 vector/scorer不得成为统一runtime的偷渡入口。Phase 1 不新增等待、retry、backoff、snapshot恢复、
circuit breaker或取消策略。

### 7.3 生命周期外的迟到工作

本阶段只保持现有deadline/cancel画像，不能宣称已经解决跨surface取消。新公共原语仍必须：

- 不在调用future被取消后自行spawn脱离请求的Provider/DB工作；
- 不持有超出当前request/snapshot的vector或source row；
- 不在transaction关闭后复用current-head observation；
- 不在错误、Debug或metrics中输出input/vector。

### 7.4 现有资源线性化顺序

Phase 1虽不统一runtime，仍必须冻结现有安全顺序：

1. Provider reservation是已经消费的rate slot，不是授权permit，也不因后续失败refund；
2. reservation事务提交后，在不持有DB transaction的情况下完成有界等待；
3. 等待结束后，以READ COMMITTED执行最终egress/fleet/authorization recheck；
4. 只有recheck成功才立即进行本次attempt唯一一次Provider调用；one-shot每logical request只有一个attempt，
   完整路径只保留现有指定generation/context churn下最多第二个root attempt；
5. Provider成功后才开始用于exact scoring的RR read；
6. one-shot在同一RR read完成scope、score和projection，commit read后再执行exact release recheck；
7. 完整路径在Provider后取得traversal permit，再开始唯一RR read；permit与transaction随同一个root/traversal
   session持有到遍历结束和snapshot关闭；
8. shared encoder不得自行retry、重新reserve、refund、提前开启snapshot、拆分snapshot或spawn/detach工作。

完整路径若进入第二个root attempt，process permit仍跨attempt持有，但旧ticket、context observation、reservation、
egress permit、input和vector都不得跨attempt复用；第二attempt按现有顺序重新ticket/context、reserve、confirm与
encode。这项行为属于现状characterization，不表示第二阶段已经设计通用retry。

需要用recording DB/Provider与受控permit测试调用顺序、无transaction等待、单次egress、RR transaction数量、
permit归还和错误后零新增工作。共同计算重构不能改变这些current runtime profile事实。

## 8. Operation接入顺序

迁移顺序冻结为：

~~~text
shared input/vector primitives
  -> Edge → member Coordinate
  -> Coordinate → incident Edge
  -> whole-graph Coordinate discovery
  -> bounded complete path
  -> legacy removal
~~~

原因：

1. Edge → Coordinate 已使用 graph Q0与common exact scorer，scope最小，最适合先验证 facade；
2. Coordinate → Edge 同样已共享Q0/scorer，但增加relation Document grouping和hydration，可验证复杂projection；
3. whole-graph Coordinate 才处理独立template/vector wrapper、global closed scope、canonical tie和K+1，是主要去重点；
4. complete path最后迁移，避免Q0+Qi、root/traversal、现有retry/snapshot语义掩盖基础计算差异。

这个顺序只规定内部接入，不改变任何公开命令的可用顺序。

## 9. 文件与模块设计

### 9.1 `buzz-semantic-query`

建议新增或收口：

~~~text
src/semantic_input.rs
  SemanticModelSpaceFences
  SemanticQueryInput / Bundle
  closed input channel/encoding identities

src/encoder.rs
  ProviderEncodedSemanticInput / Bundle
  shared SemanticQueryEncoder
  deterministic fixed-vector test adapter
~~~

修改：

- `query_text.rs`：Q0/Qi builder投影为共同input；精确字节不变；
- `coordinate_search.rs`：Coordinate builder投影为共同input；保留public request/result和兼容wrapper；
- `fence.rs`：拆generation/space与input encoding binding；保留兼容view；
- `one_hop_search.rs`：只适配共同problem input，不改ranking/result；
- `lib.rs`：记录并导出新增public API文档。

### 9.2 `buzz-db`

建议从6324行的`semantic_query.rs`拆出：

~~~text
src/semantic_query/exact_scoring.rs
  SemanticGenerationKey / SemanticGenerationFences
  vector-to-ticket binding
  GenerationBoundQueryVector / Bundle
  closed scope request
  current-head exact SQL/kernel
  row validation and direct score

src/semantic_query/exact_scoring_tests.rs
  fixed-vector, scope, tie, K+1, auth/currentness differential
~~~

修改：

- `semantic_query.rs`：保留ticket/read transaction与operation-facing methods，委托新kernel；
- `semantic_query/scoped_search.rs`：只切换vector/kernel API；policy不动；
- `semantic_coordinate_search.rs`：保留Coordinate result projection、K+1和qualification入口，删除重复
  authorization/current-head/cosine SQL；
- `semantic_coordinate_search_qualification_tests.rs`：legacy/new同snapshot差分并保留10k测试。

本阶段不修改migration或`schema/schema.sql`。

### 9.3 `buzz-relay`

修改：

- `semantic_provider.rs`：一个共同query bundle encoder，旧adapter薄转发后删除；
- `semantic_one_hop_search.rs`：按两个variant顺序接入共同vector；
- `semantic_coordinate_search.rs`：接入共同input/vector与DB kernel；
- `semantic_graph_query.rs`：最后接入ordered Q0/Qi vector bundle；
- `semantic_graph_traversal.rs`/`semantic_graph_response.rs`：除类型适配外不改policy；
- `semantic_one_shot.rs`与runtime/resource模块：本阶段不改变行为。
- `semantic_fleet.rs`及`buzz-semantic-query/fleet.rs`：只为production cutover绑定closed computation route
  profile并重新attest；不改变公开wire、capability或error。

### 9.4 不应修改

- `buzz-core` Event kind；
- `buzz-sdk` wire verifier；
- `carryforth-cli`公开command/result；
- NIP-11 capability与fleet inventory wire shape；compiled runtime digest允许因closed computation route
  profile显式版本化，但必须保留旧baseline oracle并记录批准的old→new digest迁移；
- semantic worker、extractor、source index与canonical graph schema；
- Desktop、Web和Agent Skill。

若编译适配必须触及这些protected paths，需先证明只是内部type re-export且wire bytes/digest不变；否则停止
Phase 1并另开兼容迁移。

## 10. 分阶段实施

当前交付状态：

| 阶段 | 状态 | 独立退出证据 |
| --- | --- | --- |
| U0 设计与差分门 | 已完成 | 历史 v1 oracle 与 Phase 1 differential/protected-surface gate 同时通过 |
| U1 共同 input、fence 与 vector | 已完成 | 冻结 bytes/digest 不变；共同 Provider 结果只由 writer DB 绑定 tenant-scoped generation |
| U2–U7 | 待交付 | 按下列阶段逐项审查、提交与记录 |

### U0：设计与差分门

- 冻结本文；
- 保留现有baseline v1 manifest、hash和历史oracle，不扩大它从`e8f26d6e65`开始的production freeze allowlist；
- 把现有baseline runner收口为“验证v1历史oracle仍可重放”，移除对当前文档必须写“统一引擎尚未实现”的
  时态断言；若compiled runtime digest显式版本化，v1用冻结legacy digest生成，不改写golden；
- 新建以`ab395ff6f`为起点的Phase 1 protected-surface/diff scope gate；
- 新建独立computation differential fixture，不修改baseline v1 manifest；
- 建立同input/vector bundle、同RR snapshot的legacy/new执行seam；
- 冻结每operation normalized result与closed error comparator；
- 默认production route仍为legacy。

退出条件：v1历史oracle与Phase 1 gate可以在同一工作树同时通过；新production代码不会被旧freeze diff误判，
也不能借扩大旧allowlist绕过Phase 1 review。差分seam本身不会第二次调用Provider，不会跨snapshot比较，也
不会写入真实query/vector。

### U1：共同input、fence与vector类型

- 引入pure `SemanticModelSpaceFences`与DB-owned `SemanticGenerationKey/SemanticGenerationFences`；
- 引入共同input/vector bundle；
- 为Coordinate、Q0、Qi添加closed adapter；
- 保留当前exact UTF-8 bytes、input digest和query contract digest；
- graph与Coordinate旧encoded类型薄包装共同Provider-encoded result；旧DB vector wrapper薄包装
  `GenerationBoundQueryVector`；
- ticket开始同时提供generation-only view和旧compatibility view。

退出条件：pure baseline byte/digest/vector fixture全部不变，所有旧调用仍编译并通过。

### U2：共同Provider encoder

- 把Coordinate与graph query adapter委托给共同batch encoder；
- 保持1/1/1/Q0+Qi的input count与batch顺序；
- 保持现有model/index/count/dimension/nonfinite/zero错误映射；
- 请求开始前可由server-owned mode选择旧adapter或共同adapter；任一路径失败后不得在同一请求fallback，
  也不得发出第二次Provider请求。

退出条件：fake Provider记录的exact texts、batch count、attempt count和closed error与legacy一致。

### U3：迁移one-hop tagged family

先迁移Edge → Coordinate，再迁移Coordinate → Edge：

- 使用共同Q0 input/vector；
- 使用共同scorer facade；
- 保持scope loader、ranking、hydration、coverage和result不变；
- 每个variant拥有独立server-owned route mode；
- 差分失败立即关闭新mode，不影响另一个variant。

退出条件：两个variant在同vector/snapshot下typed result与closed error全等；40914 wire回归全绿。

### U4：迁移whole-graph Coordinate discovery

- 新增`global graph Coordinates` closed scope；
- 用共享current-head/scoring kernel替代专用Coordinate SQL；
- 保持Coordinate canonical tie、K+1、all-current与no-floor；
- 保留旧SQL用于acceptance differential与回滚窗口；
- 默认先legacy；在isolated trusted-single-relay完成acceptance/canary后，按§14关gate、drain并让整个
  attested fleet切到同一个compiled new profile，不做正常fleet内单pod混合route灰度。

退出条件：边界score、完全tie、多Edge去重、Document双重角色、terminal、missing/building/failed、K+1及
10k EXPLAIN/测量均等价或不退化到批准范围外。

### U5：迁移bounded complete path

- 将现有Q0/Qi build结果转为共同ordered bundle；
- 将root、relation、target scorer输入改为共同vector；
- 不改candidate/root/path score或traversal；
- 保持generation/context churn retry与release policy；
- 分别比较root、relation、target、transition、retained path、coverage和packed result。

退出条件：四象限差分均闭合：legacy scorer/legacy policy、new scorer/legacy policy、legacy scorer/new
adapter、new/new；40912 wire、budget和known-negative现状无意外变化。

### U6：默认切换与legacy删除

- 四个operation分别完成默认new route；
- 保留一个明确回滚窗口；
- 完成真实pgvector同snapshot矩阵、目标规模EXPLAIN和错误/race矩阵；
- 根据回滚要求决定是否删除旧SQL：
  - 若仍要求无需恢复旧源码即可profile rollback，旧SQL继续保留；rollback仍需关gate、drain、部署统一
    legacy profile并重新attest，不称为即时flag rollback；
  - 若批准只通过二进制回滚，且旧版本可部署，再删除旧SQL与wrapper；
- 删除后重新执行production-path重复代码与依赖审计。

退出条件：不能同时声称“旧SQL已删除”和“无需恢复旧二进制即可切回legacy实现”；任何attested fleet
rollback都必须遵守统一profile与重新attestation。

### U7：资格与阶段关闭

- deterministic aggregate gate；
- disposable PostgreSQL/pgvector全矩阵；
- Coordinate与graph target-scale测试；
- 三surface真实Provider canary（配置可用时）；
- feature-off/gate-off/capability-off零新增egress；
- content-free资格记录与rollback状态；
- 更新上位spec、baseline、TODO和current-status，但不宣称后续runtime/governance完成。

## 11. 差分策略

### 11.1 核心原则

legacy/new不能各自重新编码、各自打开snapshot再比较。正确seam是：

~~~text
one canonical input bundle
  -> one fixed/Provider vector bundle
  -> one authorized RR snapshot
  -> legacy scorer/policy
  -> new scorer/policy
  -> normalized typed comparison
~~~

真实用户请求默认只执行一个route。双算只允许在test/acceptance模式、受控数据库和明确资源上限内执行；
不得双调用真实Provider，也不得把差异、query、vector或candidate正文写日志。

### 11.2 比较维度

每个operation比较：

- input count、顺序、exact bytes digest、encoding contract digest；
- vector count、channel identity、generation/space/model/dimension binding；
- candidate source identity、current-head evidence、direct fixed score；
- operation-specific tie、floor、limit、omission、coverage和truncation；
- normalized public result；
- snapshot observation与release结果；
- closed error category及public status/code/retryable/CLI exit；
- Provider attempt/batch count与DB statement count；
- response byte cap。

时间戳、签名、随机request id、metrics emission order和内部文件名不作字节golden。

### 11.3 四象限隔离

对完整路径和复杂one-hop policy，差分应能区分：

1. legacy scorer + legacy policy；
2. new scorer + legacy policy；
3. legacy scorer + new type/adapter；
4. new scorer + new type/adapter。

只有第2与第1相等，才能说明scorer迁移正确；只有第3与第1相等，才能说明input/vector adapter正确；
第4用于最终端到端确认。不能只比较第1与第4后猜测差异来源。

## 12. 测试矩阵

### 12.1 Pure input/vector

- Coordinate/Q0/Qi exact UTF-8 bytes、escaping、Unicode、NUL和边界字节；
- encoding contract/input digest tamper；
- request/channel重复、错序、跨request bundle；
- Q0必为首项、Qi canonical order、Qi omission；
- Provider response count/index/model/dimension/nonfinite/zero/oversize；
- generation/source digest、embedding-space、model和dimension mismatch；
- 两个Community复用同generation UUID和相同model contract时仍拒绝跨tenant vector；
- input digest与vector错绑；
- Debug/Error/metrics无query/vector。

### 12.2 Shared exact kernel

- fixed vector与brute-force cosine/量化一致；
- active generation、current head、eligible、overview unit和embedding精确绑定；
- unauthorized、跨Community、ban/owner revocation、projection signer/parity；
- source更新、head building/failed/missing/stale、generation切换；
- multi-channel score matrix顺序和channel identity；
- explicit source cap、duplicate与unknown source；
- relation-only Document、Coordinate Document、Meeting、各Project View subtype；
- statement/lock/idle timeout与transaction rollback。
- 新增直接调用production shared kernel的ignored target-scale Rust qualification；不能只用手写等价SQL或
  现有synthetic exact脚本冒充新kernel证据。

### 12.3 Coordinate

- active Edge Coordinate-only；
- 多Edge去重；
- Document仅binding时排除、兼为Coordinate时允许；
- terminal包含；
- no-floor，包括最低有效fixed score；
- score完全tie时`ProjectContextCoordinate::Ord`；
- K-1/K/K+1与`truncated`；
- legacy/new同snapshot全量与top-K相等；
- 10k规模EXPLAIN和并发画像不超批准范围。

### 12.4 One-hop

- 两个tagged scope不能交叉返回字段；
- incident完整集合、binding provenance、Edge=max Document、每Edge preview cap；
- complete Hyperedge membership、identity/materialization cap；
- closed tie、limit、coverage、omission、truncation；
- canonical preview/read descriptor与current head一致；
- 40914 request/result verifier与错误映射不变。

### 12.5 Complete path

- Q0+0/1/multiple Qi ordered bundle；
- root recall/matrix、neutral lane、MMR与explicit initial；
- relation/target/coherence/floor与environment gain；
- beam、cycle、global budget、stop precedence；
- endpoint retention、path identity、coverage、atomic packing和response cap；
- generation/context churn现有retry次数；
- release snapshot兼容语义；
- known-negative现状作为characterization，不在Phase 1改写。

### 12.6 Cross-surface

- 四operation / 三surface matrix无空项；
- 同Q0输入在one-hop与完整路径得到同一vector（同run/fixed bundle）；
- Coordinate独立v1输入继续得到其现有vector，不伪装成Q0；
- feature/process master/gate/capability/fleet/provider任一关闭时pre-Provider fail closed；
- request断开/取消画像不因共享primitive恶化；
- ordinary REQ/COUNT/NIP-50/ingest仍拒绝40912/40913/40914；
- SDK/CLI canonical verification、no retry/no redirect/body cap不变。

### 12.7 Egress、snapshot与release race

- reservation等待期间membership/ban/gate/fleet被撤销：final confirm拒绝，Provider egress delta为0，已消费
  reservation不refund；
- Provider完成后、RR read开始前generation变化：Coordinate/one-hop不retry，`begin_semantic_graph_read`的
  current generation mismatch继续映射为现有public `restricted`分类；完整路径继续映射
  `SemanticGenerationChanged`，并只在现有指定churn路径最多启动第二个完整root attempt；
- one-shot RR read形成并关闭后、release前generation/projection/context变化：exact release的
  `SnapshotChanged`继续映射既有public `conflict`，不能签名迟到结果；
- Coordinate bootstrap ticket到RR read之间仅projection/context revision变化：保持现有“接受new read snapshot并
  对其exact release”的语义；
- one-hop bootstrap ticket到RR read之间projection/context revision变化：评分前conflict且不进入scope work；
- 完整路径release继续使用`expected_snapshot: None`，不得被共同scorer误收紧成one-shot exact snapshot；当只有
  generation/projection/context snapshot变化，而当前membership、gate与fleet仍ready时，保持现有release可
  permitted的compat语义，并有正向断言；
- membership、owner、ban、gate或fleet在两类release前变化时均fail closed；generation/projection/context
  snapshot变化只对one-shot exact release强制拒绝；
- 每个failure path断言Provider attempt、RR transaction、traversal permit和后续DB work没有额外放大。

## 13. 质量门

新增聚合入口建议为：

~~~bash
just semantic-retrieval-computation
~~~

至少串联：

~~~bash
just semantic-retrieval-compatibility-baseline
cargo test -p buzz-semantic-query --lib
cargo test -p buzz-semantic-query --test compatibility_baseline
cargo test -p buzz-db --lib semantic_
cargo test -p buzz-db --lib coordinate_search
cargo test -p buzz-db --lib one_hop
cargo test -p buzz-relay --lib semantic_
cargo test -p buzz-relay --lib coordinate_search
cargo test -p buzz-relay --lib one_hop
cargo test -p buzz-sdk --lib
cargo test -p carryforth-cli
just semantic-test
just semantic-query-qualification
just coordinate-search-qualification
just semantic-retrieval-exact-qualification
cargo clippy -p buzz-semantic-query -p buzz-db -p buzz-relay --all-targets -- -D warnings
just test-unit
just ci
~~~

阶段中可以运行受影响子集；最终关闭必须运行聚合入口、service-backed资格和`just ci`。真实Provider canary
需要受支持的`BUZZ_SEMANTIC_*`配置；缺失时必须记录为外部阻断，不能挪用`LLM_*`或冒充通过。

## 14. Rollout与rollback

### 14.1 Server-owned route

迁移route按逻辑operation独立：

- `edge_member_coordinate`；
- `coordinate_incident_edge`；
- `whole_graph_coordinate_discovery`；
- `bounded_complete_path`。

route只由server-owned closed deployment profile控制，不进入公开request、wire或capability。一个请求从开始
到结束固定在同一路径，不能中途从new fallback到legacy。

内部迁移mode为closed三态：

~~~text
Legacy
AcceptanceCompareReturnLegacy
Migrated
~~~

默认production build只编译、导出和解析`Legacy/Migrated`。`AcceptanceCompareReturnLegacy`只在
`cfg(test)`或显式acceptance-only feature/binary中存在，默认配置不能选中；它只允许isolated
trusted-single-relay fixture/canary使用：同一vector、同一RR snapshot双算，比较后仍返回legacy结果。普通
用户流量与attested fleet不得启用该mode。production profile默认从`Legacy`开始；`Migrated`失败时也不能在
同一请求自动fallback，因为这会改变deadline、DB工作量与错误语义。

production `Legacy/Migrated`四operation route matrix必须形成closed computation runtime profile，并参与
compiled fleet runtime digest。这样同一个attested inventory不能混入不同route matrix。baseline v1 manifest
保持不可修改；每次批准的profile切换单独记录old/new runtime digest及其“公开合同零变化”差分证据。

### 14.2 切换顺序

~~~text
compile + deterministic differential
  -> disposable DB differential
  -> isolated trusted-single-relay acceptance/canary
  -> gate/capability off and drain old fleet
  -> deploy one compiled route profile to every routable instance
  -> publish one-runtime-digest inventory and re-attest
  -> enable gate/capability with default new route
  -> rollback window
  -> legacy removal decision
~~~

正常attested fleet内禁止混合route。切换期间旧inventory撤销后才部署/登记新profile；所有可路由实例报告同一
new runtime digest并重新attest后才恢复广告和gate。rollback同样先关gate/撤inventory/drain，再部署旧profile
并重新attest；不能让请求随机落到不同内部route的pod。

### 14.3 回滚条件

以下任一发生即回旧route并停止该operation迁移：

- candidate/source/direct score差异；
- tie、floor、coverage、truncation或result差异；
- Provider attempt/batch增加；
- snapshot/currentness/release差异；
- public error/status/CLI exit差异；
- authorization或cross-Community异常；
- response cap、statement rows、内存或目标规模明显退化；
- query/vector/content泄露。

回滚不需要改变schema，因为Phase 1不含schema迁移。删除legacy路径后只允许二进制回滚；这一能力必须在
删除前实际演练。

## 15. 可观测性与隐私

Phase 1只补共同计算阶段的content-free指标：

- operation class；
- input/vector bundle size；
- encode/scoring阶段成功与closed failure class；
- exact scorer候选/返回/截断计数；
- differential equal/mismatch低基数计数；
- legacy/new route；
- DB statement latency与rows的有界直方图。

禁止标签或日志：

- query、context overview、canonical input正文；
- input/vector digest的完整值；
- embedding、distance或逐候选真实score；
- Project/Community/caller/request/Coordinate/Edge/Document identity；
- Provider URL、headers、authorization或response body。

差分mismatch只记录operation、stage和closed mismatch class；具体合成fixture可在test失败中显示，真实数据
只能写入ignored本地产物并继续脱敏。

## 16. 风险与禁止做法

### 16.1 主要风险

1. 把Coordinate vector误绑定为graph query contract，导致result observation或准入漂移；
2. 为了共用top-K使用graph lexical source tie，改变Coordinate同分顺序；
3. legacy/new各自调用Provider或各开snapshot，制造不可解释差异与额外egress；
4. 共同SQL无意纳入relation-only Document或排除terminal Coordinate；
5. common input constructor允许任意文本/contract，形成隐藏DSL；
6. 完整路径迁移同时调整相关性策略，使基础计算差异无法隔离；
7. 过早删除旧SQL，失去安全回滚和同snapshot oracle；
8. 把内部type统一误报为runtime可靠性或性能提升。

### 16.2 明确禁止

- 不用`serde_json::Value`或字符串map表示scope/result；
- 不接受caller-provided embedding、digest、model或ranking参数；
- 不动态拼接caller SQL；
- 不在生产双发Provider；
- 不跨request缓存vector；
- 不在generation变化后复用旧vector；
- 不把partial Hyperedge当作完整Edge；
- 不把relation Document隐式当Coordinate；
- 不用近似结果替代exact scorer；
- 不以更新golden解决未解释差分；
- 不在本阶段加入retry、queue、circuit或fairness策略。

## 17. 完成定义

只有同时满足以下条件，Phase 1才能标为完成：

1. 四个逻辑operation都消费共同input/vector与同一current-head exact scorer；
2. Coordinate专用authorization/current-head/cosine SQL重复已删除，或因明确的profile rollback窗口保留且有
   删除日期，并遵循§14的gate/drain/deploy/re-attest流程；
3. Q0/Qi/Coordinate现有Provider输入字节和公开query contract digest未变；
4. 相同实际输入在同generation中通过同一encoder/vector合同；
5. Coordinate canonical tie、K+1、no-floor和eligible scope不变；
6. 两个one-hop closed variant及40914共享family不变；
7. 完整路径ranking/traversal/packing及现有snapshot/retry画像不变；
8. 三个public surface的wire、capability、gate、result、error和CLI不变；
9. 每个operation的同vector、同snapshot differential全部通过；
10. deterministic、disposable pgvector、目标规模与条件性真实Provider资格完成或明确记录外部阻断；
11. feature-off/gate-off/capability-off仍在Provider前fail closed；
12. 无schema/index/canonical graph变化，无query/vector/content泄露；
13. 聚合门、strict Clippy、`just test-unit`与`just ci`通过；
14. 回滚演练完成，qualification记录包含route、binary、manifest hash、Provider attempt和最终gate状态；
15. 上位spec、baseline、TODO与资格文档同步，且明确第二阶段可靠性运行时尚未开始。

完成这些条件后，系统可以声明“统一语义计算已交付”。仍不能声明统一可靠性运行时、统一资源治理或完整
统一语义检索引擎已经production-ready。

## 18. 资格记录模板

最终资格至少记录：

~~~text
code_commit
baseline_manifest_sha256
computation_fixture_sha256
operation_route_matrix
exact_input_digest_match_count
vector_bundle_match_count
same_snapshot_differential_count
candidate_score_mismatch_count
normalized_result_mismatch_count
closed_error_mismatch_count
provider_attempt_delta
db_statement/row bounded evidence
target-scale evidence
real_provider_canary status
feature/gate/capability/fleet rollback state
commands run / commands not run
known deviations
~~~

不得把真实query、context overview、vector、title、summary、Document正文、secret或完整identity写入资格记录。
