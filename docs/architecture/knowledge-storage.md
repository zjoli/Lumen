# 知识存储、检索与同步

状态：设计基线，已落地；实现说明见第 10 节。更新：2026-11-03。

## 1. 设计决策

| 决策 | 结论 | 原因 |
| --- | --- | --- |
| 权威数据 | Markdown 正文 + YAML frontmatter | 可读、可编辑、可比较、可迁移 |
| 物理组织 | 按对象类型存放，以稳定 ID 引用 | 支持跨领域、多归属，分类变化不移动文件 |
| 版本与同步 | 独立的标准 Git 仓库，单分支 `main`、单远端 `origin` | 支持离线提交、跨设备同步、历史恢复 |
| Rust Git 后端 | `git2` 封装 libgit2，随应用静态链接 | 用户无需安装 Git；支持本地版本、传输与合并 |
| 检索数据 | 每设备一份 SQLite，全部可重建 | 不同步数据库，避免二进制冲突和设备耦合 |
| 召回 | 精确匹配 + 全文 + 可选向量，关系展开后按预算读取 | 同时支持术语定位、自然语言查询和知识导航 |
| 上下文层级 | L0 摘要、L1 概览、L2 正文 | 减少无关正文进入模型上下文 |
| 写入 | 默认只保存到工作区，提交与同步显式触发 | 区分编辑、版本保存和远端发布 |
| 学习状态 | 独立事件文件，汇总状态从事件推导 | 保留学习证据，减少多设备覆盖 |
| Agent 集成 | 业务服务 → `AgentTool` / `AgentPlugin` | 保持现有 Agent Loop 与存储实现解耦 |

第一版不提供实时协同、自建 Git 协议、分支管理、通用 Shell/Git 工具、大文件同步或服务端索引。

## 2. 存储格式

### 2.1 仓库结构

知识仓库与 Lumen 源码仓库独立；一个工作区绑定一个知识仓库。

```text
<workspace>/
├── repository.yaml
├── domains/<id>.md
├── topics/<id>.md
├── concepts/<id>.md
├── units/<id>.md
├── questions/<id>.md
├── resources/<id>.md
├── records/YYYY/MM/<event-id>.md
├── sessions/<session-id>/<segment-id>.jsonl
├── assets/<blake3>.<ext>
├── .gitignore
├── .gitattributes
├── .git/
└── .lumen/                       # 不入 Git
    ├── config.yaml               # 设备配置，不存明文凭据
    ├── index.sqlite              # 元数据、全文、关系、向量、自动摘要
    ├── transactions/             # 写入恢复日志
    └── locks/
```

`repository.yaml` 仅保存 `schema_version: 1`、`repository_id: <UUIDv7>`、`default_language: zh-CN`。设备路径、模型地址、凭据引用、Git 作者配置放在本地配置；实际凭据沿用系统凭据设施。

六类对象、学习记录、会话片段、小附件和仓库配置进入 Git；`.lumen/` 全部忽略。文本统一 UTF-8、LF，通过 `.gitattributes` 固定换行。设备自行维护索引，不共享 SQLite/WAL 文件。

### 2.2 知识对象

每对象一个 Markdown 文件；ID 为 `<type>_<UUIDv7>`，文件名固定为 `<id>.md`。ID、类型不可修改，标题可修改。以下 ID 为简写：

```markdown
---
schema_version: 1
id: concept_019abc
type: concept
title: 事务隔离性
aliases: [Transaction Isolation]
summary: 隔离性描述并发事务之间相互影响的约束。
member_of: [topic_019def]
requires: [concept_019aaa]
sources: [resource_019bbb]
attachments: []
extensions: {}
---

# 事务隔离性

## 核心解释

……
```

公共字段：`schema_version`、`id`、`type`、`title` 必填；其余集合默认空，`summary` 可缺省。保留字段按类型校验，未知顶层字段报错；扩展数据放入 `extensions`，读写必须保留。

摘要、正文中的普通链接不创建类型关系。正文引用对象使用 `[标题](lumen:<id>)`，附件使用仓库相对路径；解析器负责跳转，原始 Markdown 仍可阅读。反向链接由索引生成。

### 2.3 类型字段与关系

| 字段 | 声明方 → 目标 | 语义 |
| --- | --- | --- |
| `member_of` | topic → domain/topic；其余四类 → topic | 多归属；领域与主题构成导航结构 |
| `requires` | concept → concept；unit → concept/unit | 学习前置 |
| `covers` | unit → concept | 单元学习目标 |
| `tests` | question → concept | 问题检验的概念 |
| `sources` | topic/concept/unit/question → resource | 内容依据 |
| `steps[].target` | unit → resource/question | 有序活动；`kind` 为 `read` / `answer` |

每条关系只在声明方存储，逆向关系一律反查。`member_of` 和 `requires` 分别禁止循环；关系目标必须存在且类型合法。单元是学习活动，不能作为概念的物理父目录。

其他类型字段：question 的 `question_kind: inquiry | assessment`；resource 的 `origin: { uri, captured_at }` 可缺省。资源正文保存已导入的 Markdown 快照，原件可作为附件；导入后的正文进入版本管理。

附件条目为 `{ path, media_type, hash }`。附件按 BLAKE3 寻址、不可原位覆盖，单文件上限 10 MiB；超限返回 `AssetTooLarge`，第一版只记录外部来源。引用外部 URL 不代表已离线保存。未引用附件不自动删除。

### 2.4 学习记录与会话

学习记录仍采用 YAML + Markdown：

```markdown
---
schema_version: 1
id: event_019ccc
type: learning_event
event_kind: answer
occurred_at: "2026-11-02T10:00:00Z"
targets: [question_019ddd, concept_019abc]
session_id: session_019eee
supersedes: null
data: { outcome: partial, hints_used: 1 }
---

## 回答

……

## 反馈

……
```

`event_kind` 为 `study | answer | review | assessment | correction`。已保存事件不可修改；纠错新增事件并通过 `supersedes` 引用旧事件。汇总按纠错链消除被替代事件；排序采用 `(occurred_at, id)`，不将设备时钟视为严格因果顺序。推导算法版本记在本地索引中。

会话沿用 `src/agent/session.rs` 的 `SessionRecord { message: AgentMessage }` JSONL 格式。每次运行使用独立片段 ID，只追加本次新增消息。活动文件使用 `.jsonl.open` 后缀，由 Git 忽略；关闭时原子重命名为 `.jsonl`，此后不可修改。恢复会话读取全部已关闭片段，片段内保持消息顺序，跨片段以 UUIDv7 稳定排序；并行产生的片段保留各自边界，不伪造线性对话。

知识正文与学习证据用 Markdown；transcript 保留现有 JSONL 协议，不转换为知识对象。工作模式切换、同步和正常退出前关闭当前片段；启动时恢复中断片段，截断不完整尾行并保留原件后关闭。

### 2.5 写入与校验

- 返回值携带 `revision = BLAKE3(原始文件字节)`；更新、删除必须提供 `expected_revision`，不匹配返回 `RevisionConflict`。
- 写操作统一进入 `WorkspaceService`。同一工作区采用进程内锁和跨进程文件锁串行写入；写临时文件、持久化恢复日志、原子替换，成功后再更新索引。
- 多文件变更使用 `ChangeSet`：先校验最终对象图，再写入。服务读请求不观察半完成批次；崩溃后先完成恢复再开放服务。不承诺外部编辑器观察到跨文件原子快照。
- 删除对象默认拒绝存在入向关系或学习记录引用的情况；调用者须在同一批次修复引用。历史会话中的自由文本不作为删除约束。
- 外部编辑通过文件扫描发现；入库前重算指纹。无效文件列入诊断，不返回其旧版索引结果；重复 ID 阻断相关对象检索及仓库提交。
- 提交、合并继续前校验完整候选树：格式、ID 唯一性、关系、循环、附件和会话片段完整性；与父提交比较，禁止修改或删除已提交事件及已关闭片段。

## 3. 检索

### 3.1 本地索引

采用 `rusqlite` 的 bundled SQLite 与 FTS5。SQLite 只存派生数据：

| 表 | 内容 |
| --- | --- |
| `documents` | 对象、事件、已关闭会话片段的路径、类型、标题、元数据、指纹 |
| `edges` | 类型关系及普通正文链接；两者分开标记 |
| `chunks` / `chunks_fts` | 章节文本、定位信息、全文索引 |
| `summaries` | 自动 L0/L1、输入指纹、生成器版本、覆盖范围 |
| `embeddings` | 分块指纹、模型标识、维度、归一化 f32 向量 |
| `learning_state` | 从事件推导的学习状态及算法版本 |
| `index_state` | schema、切分器版本、工作区 generation、任务及错误状态 |

首版向量采用 Rust 内存精确余弦搜索，从 SQLite 加载当前模型的向量；不引入向量服务或 ANN。索引接口保留替换能力，性能超标后再换后端。

中文全文索引采用汉字单字及双字切分，拉丁词归一化；查询与文档使用同一版本的切分器。将查询词转义后交给 FTS5，不直接执行用户输入的 MATCH 表达式。

短对象整体索引；长正文按 Markdown 标题切分，超过 800 tokens 再分段，重叠最多 100 tokens。块记录对象 ID、标题路径、字节范围与源指纹；tokens 使用统一估算器，结果返回实际估算量。

### 3.2 L0 / L1 / L2

| 层级 | 内容 | 生成与保存 |
| --- | --- | --- |
| L0 | 对象或集合的短摘要，默认预算 256 字符 | 优先使用源文件 `summary`，否则生成并缓存 |
| L1 | 领域、主题、单元的范围、关键点、关系导航，默认预算 4000 字符 | 按逻辑成员聚合并缓存；无生成器时输出确定性导航 |
| L2 | 完整正文或指定章节 | 读取 Markdown，不将自动摘要当作原文 |

摘要针对逻辑集合，不依赖物理目录。成员清单由关系确定，模型只负责总结。缓存键包含成员及其内容指纹、生成配置；预算超限时标记覆盖范围，不能声称覆盖完整集合。

自动摘要默认不入 Git；人工采纳的摘要通过对象更新写回 `summary` 或正文。设备需自行生成缓存；首版不实现跨设备生成结果缓存。

### 3.3 查询流程

```text
查询 + 类型/领域/主题范围
  → ID、标题、别名精确匹配
  → 全文与可选向量分别召回
  → 按 RRF 融合、块级去重、可选重排
  → 按显式关系展开
  → 按 token 预算装配摘要、章节与来源
```

默认：全文、向量各取 50 个候选，RRF 常数 60，返回最多 20 个结果；精确匹配置顶。关系展开默认 1 跳、最多 50 个对象，可配置上限 2 跳、100 个对象。上下文默认预算 4000 tokens，扣除来源标识后选择内容，不足则截断并返回 `truncated`。

范围过滤覆盖逻辑成员及调用者选择的子主题，不使用文件路径前缀代替知识归属。源类型可选择知识、学习记录、会话；默认只搜六类知识对象。查询可直接命中任意层级，不强制逐层遍历。

`SearchRequest` 包含 `query, mode, scope, source_types, levels, limit, rerank`；`mode = lexical | semantic | hybrid`，默认 `hybrid`。缺少向量模型、缓存未就绪或模型失败时，hybrid 降级为全文；显式 semantic 请求返回 `CapabilityUnavailable` 或 `IndexNotReady`。缺少重排器则保留融合顺序并报告降级。

`SearchHit` 返回 `id, kind, level, chunk_id, heading, excerpt, score, match_sources, source_ref`；`source_ref` 包含仓库 ID、相对路径、对象 ID、文件指纹和行号。结果附 `generation, freshness, degraded, diagnostics`，不返回无法定位原文的片段。

### 3.4 更新与一致性

1. 启动扫描、服务写入、外部文件变化和 Git 更新均产生变更集；删除同步移除索引。
2. 元数据、关系、全文在本地事务中更新后再报告其 generation 就绪；向量与自动摘要由应用管理的任务队列更新。
3. 内容指纹是更新依据，Git HEAD 仅作加速提示；未提交修改同样参与检索。重命名按稳定 ID 识别。
4. 查询固定一个索引 generation；读取原文时再次校验指纹。发现变化则刷新或跳过并标记 `partial`，不引用已失效位置。
5. 分开报告 `files_ready`、`lexical_ready`、`semantic_ready`；拉取完成不等于语义索引完成。更新中的查询先补齐全文或退回文件扫描，不静默使用旧内容。
6. Embedding 模型、维度或切分版本改变时分代重建，禁止混用向量空间；任务失败不撤销已保存的正文。
7. 后台任务归应用生命周期管理，支持取消、重启恢复和退出等待；插件不启动脱离生命周期的任务。

## 4. 工作模式

### 4.1 本地模式（默认）

打开仓库 → 恢复未完成写入 → 扫描并更新基础索引 → 学习、检索、编辑 → 保存工作区。

离线可读写、全文检索、记录与本地提交；依赖远端模型的生成和检索能力按配置降级。保存不自动提交，提交不自动推送，启动不自动拉取。应用保存新 transcript，不在退出时重复追加全部历史消息。

### 4.2 提交与同步模式

| 操作 | 行为 |
| --- | --- |
| `commit` | 关闭当前会话片段、冻结受管写入、暂存受管变更、校验暂存树、创建本地提交 |
| `pull` | 要求工作区干净；fetch 后按提交关系快进或三方合并；校验成功才完成合并 |
| `push` | 推送本地 `main` 到 `origin/main`，不隐式提交或合并 |
| `sync` | 有变更时 commit → pull → push → 更新索引；无远端时返回配置错误 |

首次推送允许创建空远端的 `main`。本地初始化仓库建立初始提交；连接已有历史必须通过 clone，禁止自动合并无共同祖先的仓库。离线 sync 若已提交但未推送，保留本地提交并报告 `pending_push`。

pull 决策：相同则无操作；本地落后则校验远端树，通过 libgit2 更新工作树、索引和分支引用完成快进；本地领先则无须合并；分叉则三方合并到工作树与索引，校验候选后创建双父提交。远端树无效时拒绝更新，返回诊断，不进入本地合并状态。

sync 期间暂停知识写入和新会话片段；网络阶段可读当前有效内容。收到远端新提交导致 push 被拒绝时，返回 `RemoteAdvanced`，由下一次 sync 重试，不无限重试。网络取消不能保证远端未接收推送，返回 `remote_state: unknown`，下次 fetch 核实。

只暂存受管路径；检测到无关暂存项、外部 Git 操作或 HEAD 改变时停止并报告。Lumen 不与外部 Git 写操作并发运行；外部编辑需在提交及合并期间暂停。提交校验针对实际暂存树，不以先前扫描结果代替。

### 4.3 冲突与恢复模式

文本冲突、删除/修改冲突、同一 ID 的不兼容创建以及合并后的语义校验失败，均进入 `NeedsResolution`：暂停普通写入、推送及自动上下文注入，保留工作树与 base/ours/theirs。

用户逐项提交解决结果；对象类结果通过格式校验后写入并暂存。`continue_merge` 对整棵候选树校验，无未解决项后提交并重建索引；`abort_merge` 中止合并。pull 入口要求干净，因此中止以合并前版本为基准；中止前将人工解决内容备份至 `.lumen/transactions/`。

schema 更新、悬空引用等语义错误作为诊断项解决，即使 Git 未产生冲突标记也不能直接提交。合并中断或进程退出后，重新打开仍进入恢复模式，不自动丢弃现场。可读取 Git 历史与冲突三方内容。

历史恢复只把指定提交的对象/附件复制成新的工作区变更，并使用 revision 校验；用户随后提交。不得移动 HEAD、覆盖无关变更或重写远端历史。

## 5. 模块与 Rust 接口

### 5.1 模块边界

```text
src/workspace/    生命周期、写事务、锁、配置、状态、事件、错误
src/knowledge/    类型、Markdown 编解码、对象/资源/记录服务、工具适配
src/retrieval/    索引、召回、摘要、模型适配、RetrievalPlugin
src/versioning/   VersionService、GitBackend、Libgit2Backend、CredentialProvider
src/agent/       保留现有 loop、tool、plugin、session 协议
```

依赖方向：TUI/工具/插件 → 业务服务 → 文件存储/索引/Git 后端。`WorkspaceService` 协调跨模块事务；知识及检索服务不直接执行 Git。新增模块由 `src/lib.rs` 导出。

以下为接口约定；具体 DTO 以代码为准。所有服务绑定一个工作区，以 `Arc` 注入；异步接口统一接受末尾参数 `cancel: CancellationToken`，返回 `WorkspaceResult<T>`，表中省略重复参数和 `Result`。同步磁盘、SQLite、Git 操作运行在每工作区的专用线程，持锁阶段不允许被丢弃后留下不可恢复状态。

```rust
pub type WorkspaceResult<T> = Result<T, WorkspaceError>;

// 读写版本分别表示文件指纹与 Git 提交，不能混用。
pub struct Revision(pub String);
pub struct CommitId(pub String);
pub enum ExpectedRevision { Absent, Match(Revision) }
pub enum ReadView { Abstract, Overview, Full, Section(SectionSelector) }
pub enum Version { WorkingTree, Commit(CommitId) }
```

分页统一 `Page<T> { items, next_cursor, generation }`；游标失效返回 `CursorExpired`。外部 ID、路径与 Git 引用使用校验后的类型；路径限制在仓库内，拒绝 `..`、绝对路径和逃逸符号链接。

### 5.2 工作区与配置

| 接口 | 返回与用途 |
| --- | --- |
| `WorkspaceService::init(InitRequest)` | `WorkspaceHandle`；创建目录、仓库与初始提交 |
| `WorkspaceService::open(path)` | `WorkspaceHandle`；校验版本、恢复现场、启动索引 |
| `WorkspaceService::clone(CloneRequest)` | `WorkspaceHandle`；克隆、验证仓库、创建本地索引 |
| `info()` | `WorkspaceInfo`；仓库 ID、根路径、schema |
| `status()` | `WorkspaceStatus`；脏文件、同步/索引/恢复状态 |
| `config()` / `configure(LocalConfigPatch)` | `LocalConfig`；本地模型、作者、检索预算等 |
| `validate(ValidationScope)` | `ValidationReport`；工作区或指定提交/暂存树 |
| `recover()` | `RecoveryReport`；完成写入日志恢复，识别 Git 合并现场 |
| `subscribe()` | `broadcast::Receiver<WorkspaceEvent>`；状态与任务进度 |
| `close()` | `CloseReport`；关闭片段、取消并等待任务、释放锁 |

`init/clone` 目标必须为空，禁止嵌套仓库；`open` 不支持的新 schema 返回 `UnsupportedSchema`，不自动迁移。`configure` 不更改远端，远端由版本服务管理。

### 5.3 对象、资源与学习记录

| 服务/接口 | 返回与约束 |
| --- | --- |
| `KnowledgeService::list(ObjectFilter, PageRequest)` | `Page<ObjectHeader>`；类型、逻辑范围过滤 |
| `resolve(IdOrTitle)` | `ResolveResult`；歧义返回候选，不任意选取 |
| `read(ObjectId, ReadView)` | `ObjectView`；元数据、正文/摘要、revision、来源 |
| `read_many(Vec<ReadRequest>)` | `Vec<ReadOutcome>`；逐项结果，保持请求顺序 |
| `create(CreateObject)` | `ObjectView`；服务生成 ID |
| `update(UpdateObject)` | `ObjectView`；完整对象替换 + expected_revision |
| `patch(PatchObject)` | `ObjectView`；元数据 merge patch、可选正文替换 + expected_revision |
| `delete(DeleteObject)` | `ChangeReceipt`；expected_revision，默认检查入向引用 |
| `apply(ChangeSet)` | `ChangeReceipt`；原子业务批次，支持创建时预分配 ID 与相互引用 |
| `relations(ObjectId, RelationQuery, PageRequest)` | `Page<Relation>`；入向/出向、类型、普通链接过滤 |
| `ResourceService::import(ImportSnapshot)` | `ObjectView`；Markdown 快照、来源、附件一次写入 |
| `read_asset(AssetRef, ByteRange)` | `AssetContent`；校验哈希，支持分段 |
| `add_asset(AddAsset)` | `AssetRef`；限额检查、去重；经 apply 关联对象 |
| `remove_asset(AssetRef)` | `ChangeReceipt`；仅允许无引用附件 |
| `LearningService::append(AppendEvent)` | `LearningEvent`；生成事件 ID，校验目标 |
| `events(EventFilter, PageRequest)` | `Page<LearningEvent>` |
| `state(Vec<ObjectId>)` | `Vec<LearningState>`；附计算版本与证据 ID |

`update` 替换完整对象；`patch` 中缺省字段不修改，null 清除可选元数据，空字符串清空正文；两者都校验 revision，禁止改变 ID/类型。`knowledge_update` 工具支持这两种输入。事件纠错使用 `append(correction)`，不提供覆盖及删除接口。资源下载、PDF 解析不属于存储服务；调用方提供 `ImportSnapshot`。

### 5.4 会话

| 接口 | 返回与用途 |
| --- | --- |
| `SessionArchive::begin(SessionId)` | `SegmentHandle`；分配本次运行独立片段 |
| `append(SegmentId, AgentMessage)` | `()`；复用 `SessionStore::append`，每条只写一次 |
| `finish(SegmentId)` | `SegmentInfo`；关闭片段，纳入索引与提交 |
| `list(SessionFilter, PageRequest)` | `Page<SessionInfo>` |
| `load(SessionId)` | `SessionTranscript`；按片段恢复，复用 `SessionStore::load` |

归档层补充片段管理和严格完整性校验；现有 `load` 跳过错误行的行为不能用于提交校验。

### 5.5 检索、上下文与维护

| 接口 | 返回与用途 |
| --- | --- |
| `RetrievalService::search(SearchRequest)` | `SearchResult`；召回、融合、可选重排 |
| `context(ContextRequest)` | `ContextBundle`；查询、关系展开、预算裁剪、来源 |
| `IndexService::status()` | `IndexStatus`；各层 generation、进度、错误 |
| `refresh(ChangeScope)` | `IndexReport`；扫描并同步基础索引，安排派生任务 |
| `rebuild(IndexSelection)` | `IndexReport`；重建全文/关系/学习状态/向量/摘要 |
| `SummaryService::read(SummaryRequest)` | `SummaryResult`；读取有效缓存或确定性导航 |
| `generate(SummaryRequest)` | `SummaryResult`；显式生成并缓存；无模型返回能力错误 |

`ContextRequest = { search, expansion, token_budget }`。`ContextBundle` 包含片段、来源、使用预算、截断/降级标记。summary 请求明确对象/集合、层级与预算；`IndexSelection` 不影响源文件。

模型适配使用 `#[async_trait]`、`Send + Sync`：`Embedder::embed(Vec<String>) -> EmbeddingBatch`、`Reranker::rerank(query, candidates) -> RankedCandidates`、`SummaryGenerator::generate(SummaryInput) -> String`；均接受取消信号。Embedding 批次必须返回稳定模型标识与维度；查询与文档使用同一配置。

### 5.6 版本与同步

| `VersionService` 接口 | 返回与用途 |
| --- | --- |
| `remote()` / `set_remote(RemoteConfig)` | `Option<RemoteInfo>` / `RemoteInfo`；唯一 origin |
| `status()` | `VersionStatus`；HEAD、dirty、ahead/behind、merge；附最后 fetch 时间 |
| `diff(DiffRequest)` | `DiffResult`；工作区/暂存区/提交间差异，支持对象范围；扫描补充未跟踪文件 |
| `history(HistoryQuery, PageRequest)` | `Page<CommitInfo>`；提交及对象历史 |
| `read_at(ObjectId, CommitId)` | `ObjectView`；读取历史对象及来源 |
| `restore(RestoreRequest)` | `ChangeReceipt`；指定提交对象及必要附件写回工作区，校验预期 revision |
| `commit(CommitRequest)` | `CommitOutcome::Created(CommitId) / NoChanges` |
| `fetch()` | `FetchReport`；只更新远端引用 |
| `pull()` | `PullReport`；fetch + 快进/合并，不推送 |
| `push()` | `PushReport`；不强制推送 |
| `sync(SyncRequest)` | `SyncReport`；commit + pull + push 的阶段结果 |
| `conflicts()` | `ConflictReport`；文本/结构/语义诊断，含三方引用 |
| `resolve(ResolveRequest)` | `ResolutionReceipt`；写入解决内容或确认删除并暂存 |
| `continue_merge()` | `CommitId`；完整校验后创建合并提交 |
| `abort_merge()` | `AbortReport`；备份人工解决内容后中止 |

`CommitRequest` 包含提交说明，作者来自本地配置；首次缺失时返回配置错误。`SyncRequest` 只额外提供本地提交说明，不暴露 Git 参数。`SyncReport` 分开记录 `local_commit, fetch, merge, push, pending_push, remote_state, index_status`，失败时保留已完成阶段。

`VersionStatus` 的 ahead/behind 仅针对最后 fetch 的远端引用；没有 fetch 时标记未知。历史删除对象仍可按 ID 读取，不依赖当前索引。`RestoreRequest` 包含提交 ID 及对象/预期版本列表，`Absent` 表示当前必须不存在；默认带回所需附件，不自动恢复其他知识对象，缺少关系目标时要求扩大恢复批次。

### 5.7 事件与错误

新增独立的 `WorkspaceEvent`：`FilesChanged`、`IndexProgress`、`IndexReady`、`IndexFailed`、`SyncProgress`、`SyncCompleted`、`NeedsResolution`、`RecoveryRequired`。均带 `operation_id`；有状态的事件带 generation。TUI 同时消费现有 `AgentEvent` 与工作区事件，不把后台任务伪装成 Agent turn。

`WorkspaceError` 分类：`NotFound`、`Ambiguous`、`InvalidFormat`、`UnsupportedSchema`、`InvalidRelation`、`DuplicateId`、`RevisionConflict`、`Referenced`、`AssetTooLarge`、`InvalidPath`、`WorkspaceBusy`、`DirtyWorktree`、`NeedsResolution`、`CursorExpired`、`CapabilityUnavailable`、`IndexNotReady`、`Versioning`、`Configuration`、`AuthRequired`、`RemoteAdvanced`、`Network`、`Cancelled`、`RecoveryRequired`、`Io`。结果携带可恢复阶段与诊断；原始凭据不得进入错误或日志。

## 6. Git 后端

### 6.1 Rust 选型

| 选项 | 结论 |
| --- | --- |
| `git2` / libgit2 | 采用；静态链接 libgit2，开发构建承担 C 依赖，用户无需安装 Git |
| 系统 Git + Tokio process | 不采用；增加用户安装依赖 |
| `gix` / gitoxide | 不采用；当前官方能力表的 push 和提交级 merge 尚未完成，不能覆盖同步闭环 |

接口使用 `#[async_trait] pub trait GitBackend: Send + Sync`，首版实现 `Libgit2Backend`。业务服务不接触 git2 类型，不提供通用 Git 命令接口。后端仅处理 Git 机制；业务校验、路径白名单和索引更新由上层协调。

git2 显式启用 `vendored-libgit2`、`https`、`ssh`；Linux 发布构建另启用 `vendored-openssl`。SSH/TLS 等原生依赖在发布构建中静态链接，不要求用户安装 libgit2、OpenSSL 或 SSH 客户端；不要求整份可执行文件完全静态。构建与发布 CI 负责 C 工具链和依赖检查，不增加 Tokio `process` feature。

其他选型：`serde_yaml_ng` 编解码 frontmatter；`pulldown-cmark` 解析章节；`rusqlite` bundled SQLite/FTS5；`uuid` v7 生成 ID；`blake3` 内容指纹；`fs2` 文件锁。依赖版本在实施时锁入 Cargo.lock。YAML 使用受限数据子集：字符串、布尔、数值、数组、映射、null；禁用自定义 tag，不依赖 YAML 的隐式日期类型。

### 6.2 必需的后端接口与实现映射

下表是完整接口；方法均绑定仓库路径，省略共同的 `&self`、`CancellationToken` 和 `WorkspaceResult`。`init/clone_repo` 由工作区生命周期调用，其余由版本服务调用。

| 分类 | `GitBackend` 方法 | git2 实现 |
| --- | --- | --- |
| 生命周期 | `init() -> ()`；`clone_repo(RemoteUrl) -> ()` | `Repository::init_opts` 固定初始分支 main；`RepoBuilder::clone` 固定 main |
| 配置 | `read_remote() -> Option<RemoteInfo>`；`write_remote(Option<RemoteUrl>) -> ()` | `find_remote`、`remote`、`remote_set_url`、`remote_delete` |
| 状态 | `status() -> GitStatus`；`head() -> Option<CommitId>`；`compare(CommitPair) -> CommitRelation` | `statuses`、`head`、`graph_ahead_behind`、`graph_descendant_of` |
| 对象读取 | `list_tree(TreeRef) -> Vec<TreeEntry>`；`read_blob(BlobRef) -> Vec<u8>` | `Tree::walk`、`Index::iter`、`find_blob` |
| 差异/历史 | `diff(GitDiffRequest) -> GitDiff`；`log(GitLogRequest) -> Page<CommitInfo>` | 文件树比较与 `Patch::from_buffers`、`revwalk` |
| 提交 | `stage(Vec<RepoPath>) -> ()`；`commit(GitCommitRequest) -> CommitId` | `Index::add_path/remove_path/write/write_tree`、`Repository::commit` |
| 传输 | `fetch() -> FetchReport`；`push() -> PushReport` | `Remote::fetch/push`，限定 origin 与 main 引用 |
| 合并 | `fast_forward(CommitId) -> ()`；`begin_merge(CommitId) -> MergeState`；`abort_merge() -> ()` | 祖先检查 + `checkout_tree` + 分支引用更新；`Repository::merge`；恢复合并前快照 + `cleanup_state` |
| 冲突 | `read_conflicts() -> Vec<GitConflict>` | `Index::conflicts` 获取三方条目，再读取 blob |

候选树读取必须覆盖暂存文件、新增与删除项；`read_blob` 支持 HEAD、已验证 commit、暂存区及冲突三方。`continue_merge` 使用同一 `stage/commit`，恢复使用 `read_blob + 文件写事务`。合并提交明确指定本地、远端两个父提交，成功后清理合并状态；`cleanup_state` 只清理状态，不能代替中止时的工作树与索引恢复。

分支引用更新使用预期旧 OID 校验，并记录恢复日志；不以强制检出覆盖外部修改。不公开 branch/checkout、rebase、stash、reset、clean、cherry-pick、tag、force push；不改写已存在提交。

### 6.3 执行约束

- 直接调用库 API，显式打开仓库；不启动 Git、SSH 或 Shell 进程，不依赖 PATH。URL 限定 SSH、HTTPS 或本地仓库路径。
- 从结构化状态、索引及对象读取数据；OID 使用后端解析后的类型，不假设固定长度。首版初始化 SHA-1 仓库，其他对象格式在库能力未启用时返回明确的不支持错误。
- git2 为同步 API：每仓库使用一个应用管理的专用线程和有界任务队列。在线程内打开 Repository、构造 Remote/回调，不跨 await 共享这些对象；仅传递自有 DTO。
- 网络进度和凭据回调检查取消信号，配置传输超时；本地长任务在阶段间检查。正在执行的原生调用不能强制中断，取消/超时后须等待安全退出再释放写锁；不能把丢弃 future 当作任务终止。
- 写操作全程持工作区锁，记录 HEAD、索引和阶段日志；退出时等待工作线程。中断后核查仓库状态，返回已完成阶段及恢复要求，不盲删锁文件。
- push 同时检查方法返回值与 `push_update_reference` 中的远端拒绝信息；不能把传输结束视为引用更新成功。
- 不执行仓库 hooks、外部 diff、textconv 或系统 Git credential helper。

### 6.4 认证接口

注入 `#[async_trait] CredentialProvider: Send + Sync`，提供 `resolve(AuthRequest, CancellationToken) -> WorkspaceResult<CredentialMaterial>`；网络操作前准备凭据，在 `RemoteCallbacks::credentials` 中按允许类型选择，认证失败返回 `AuthRequired`，不在回调中交互等待。

HTTPS 使用令牌，SSH 使用指定私钥及可选口令，也可使用已有 SSH agent；agent 不是必需依赖。凭据由系统密钥存储提供，本地配置只保存引用；不默认调用 `Cred::credential_helper`，避免重新引入外部程序依赖。TLS 证书和 SSH 主机校验不得跳过。

## 7. 接入现有 Agent 与 TUI

### 7.1 Agent 工具

实现现有 `AgentTool::name/description/input_schema/execute`。工具构造时注入 `Arc<...Service>`，从 `ToolContext.cancel` 传递取消信号；通过 `AgentLoop::register_tool` 注册。

| 分类 | 工具名 | 映射 |
| --- | --- | --- |
| 查找 | `knowledge_list`、`knowledge_resolve` | list / resolve |
| 读取 | `knowledge_read`、`knowledge_read_many`、`knowledge_relations` | read / read_many / relations |
| 检索 | `knowledge_search`、`knowledge_context` | search / context |
| 写入 | `knowledge_create`、`knowledge_update`、`knowledge_delete`、`knowledge_apply` | 对象写接口 |
| 资源 | `resource_import`、`asset_read` | 导入快照 / 读取附件 |
| 学习 | `learning_record`、`learning_events`、`learning_state` | append / events / state |
| 历史 | `knowledge_history`、`knowledge_diff`、`knowledge_read_at` | history / diff / read_at |

工具参数使用严格 JSON Schema；不接受任意路径写入。`ToolResult.content` 中返回 JSON 文本，包含数据、revision、来源与诊断；当前项目没有 `details` 字段，不依赖该字段。可预期业务错误以结构化错误内容和 `is_error=true` 返回；内部错误映射为 `AgentError::Tool`；取消映射为 `AgentError::Aborted`。普通工具 `terminate=false`。

Git 写操作、远端配置、冲突解决、历史恢复、索引重建仅通过应用命令调用，不注册为默认模型工具。这样应用可以执行用户明确的提交/同步操作，不让模型自行发布工作区。

### 7.2 自动上下文

`RetrievalPlugin` 通过 `AgentPlugin::before_request` 的 Collect 阶段提供带来源的候选；ContextWindowPlugin 在统一预算下决定采用的内容，随后追加到窗口并保存 context journal。它不修改原始用户消息或系统提示；续轮复用同一用户输入的检索，知识写入工具会使触发键变化。

ContextStore 的 Custom 记录作为内部控制格式，恢复时显式重建模型可见投影；未知 Custom 不直接发送给模型。PluginContext.state 为只读，cancel 继续贯穿各 hook。检索失败只报告降级，不阻断普通对话；恢复/冲突状态禁用自动注入。首版内部格式与恢复边界见 [上下文实现说明](context-management-implementation.md#14-首版实现说明2026-11-05)。

当前工具执行上限 30 秒；模型工具只执行有界操作，不在工具中等待完整重建或长时间摘要生成。维护任务由应用管理，通过工作区事件报告进度。

### 7.3 应用命令

扩展 `src/commands.rs::Command` 与解析器，保留现有命令：

| 命令 | 行为 |
| --- | --- |
| `/workspace init <path>` / `open <path>` / `clone <url> <path>` | 创建、打开、克隆；切换前关闭当前工作区 |
| `/workspace config [key value]` | 查看或修改本地配置 |
| `/status` | 增加工作区、索引、待推送、合并状态 |
| `/search <query>` | 默认 hybrid 检索 |
| `/commit <message>` | 本地提交 |
| `/sync [message]` | 提交、拉取、推送；有本地变更且无说明则报参数错误 |
| `/sync pull` / `push` / `fetch` | 单独执行同步阶段 |
| `/sync remote [url\|none]` | 查看、设置或移除 origin |
| `/sync conflicts` / `resolve <item> <file\|delete>` | 查看冲突、导入人工解决内容或确认删除 |
| `/sync continue` / `abort` | 完成或中止合并 |
| `/history [object-id]` / `/diff [object-id]` | 查看历史、当前工作区差异 |
| `/restore <object-id> <commit>` | 展示变更后执行对象恢复，使用当前 revision |
| `/index status` / `refresh` / `rebuild <selection>` | 查看、刷新、重建索引 |
| `/validate` | 校验完整工作区 |

`/sync` 子命令为保留词；与提交说明冲突时使用 `/sync --message <text>`。解析为类型化命令，不透传 Shell/Git 参数。现有 `/stop` 取消当前 Agent 或维护操作，TUI 必须并发消费输入及事件，不能等待同步调用完成才读取停止命令。

## 8. 验收标准

1. 无系统 Git、SSH 客户端和系统 libgit2 时，发布包仍可初始化、提交并通过 HTTPS/SSH 同步；认证材料单独配置。
2. 删除 `.lumen/` 后可重建全部索引，知识、关系、学习记录和会话不丢失。
3. Markdown 往返保留正文与扩展字段；外部编辑可检测，旧 revision 写入被拒绝。
4. 跨领域引用、多归属、前置关系正确；循环、重复 ID、悬空关系阻断提交。
5. 无网络/无模型时可编辑、全文检索、本地提交；降级状态明确。
6. 新增、更新、删除、未提交编辑与拉取内容均进入检索；失效块不返回旧来源。
7. 两设备可完成提交/推送/拉取；分叉自动合并或进入可恢复冲突状态，不覆盖任何一方。
8. Git 自动合并产生的语义错误阻断合并提交；中止、取消、崩溃后能恢复现场。
9. 推送失败保留本地提交；推送取消报告远端状态未知；无强制推送及历史重写路径。
10. 工具结果可定位原文；上下文不超预算，不修改长期 transcript；同一消息不重复归档。

## 9. 依据

- OpenViking：[内容与索引分离](../../../OpenViking/docs/zh/concepts/05-storage.md)、[L0/L1/L2](../../../OpenViking/docs/zh/concepts/03-context-layers.md)、[当前全局检索实现](../../../OpenViking/openviking/retrieve/hierarchical_retriever.py)。借鉴存储边界和分层读取，不直接依赖其存储服务。
- Lumen：[Agent Loop 设计](agent-loop.md)、[实际工具接口](../../src/agent/tool.rs)、[实际插件接口](../../src/agent/plugin.rs)、[现有会话格式](../../src/agent/session.rs)。接口以当前代码为准。
- Git：[合并与快进语义](https://git-scm.com/docs/git-merge)。
- Rust：[git2 构建 features](https://github.com/rust-lang/git2-rs/blob/master/Cargo.toml)、[仓库操作](https://docs.rs/git2/latest/git2/struct.Repository.html)、[传输回调](https://docs.rs/git2/latest/git2/struct.RemoteCallbacks.html)、[凭据](https://docs.rs/git2/latest/git2/struct.Cred.html)、[gitoxide 能力表](https://github.com/GitoxideLabs/gitoxide#feature-discovery)、[rusqlite](https://docs.rs/rusqlite/latest/rusqlite/)、[serde_yaml_ng](https://docs.rs/serde_yaml_ng/latest/serde_yaml_ng/)。选型核验日期：2026-11-02。

## 10. 实现说明（2026-11-03）

- 四个模块已由 `src/lib.rs` 导出；通过 `WorkspaceService::init/open/clone` 获得工作区句柄，再取得各业务服务。GitBackend 的 init/clone_repo 是接收初始化参数的关联构造函数。
- 文件写入采用日志重放；活动会话有独立文件锁，其他进程不会将其误判为孤立片段。外部编辑在读取、检索和显式刷新时扫描发现。
- provider 通过 `set_embedder`、`set_reranker`、`set_summary_generator` 注入；后台任务生成缺失向量与摘要，close 取消并等待任务退出。未配置 provider 时使用全文检索和确定性概览。
- 终端入口包含工作区、检索、提交、同步、历史、冲突和索引命令；输入与操作并发，`/stop` 可发送取消。应用在创建运行时前配置 libgit2 连接/读取超时；库嵌入方可在启动线程前调用 `configure_transport_timeouts`。
- 会话归档复用现有 SessionRecord 格式，使用严格校验；每次运行只保存新增消息。RetrievalPlugin 仅修改请求投影，不改写 transcript。
- 使用 `cargo test --all-targets` 验证；`tests/knowledge_storage.rs` 覆盖多设备合并、文本/语义冲突、恢复、索引重建和取消。真实 HTTPS/SSH 服务认证及跨平台发布仍需在对应环境验证。
