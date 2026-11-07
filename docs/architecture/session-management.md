# Lumen Session 管理设计

状态：首版已实现。日期：2026-11-07。第 1–12 节保留设计目标，第 13 节说明落地接口与边界。

本方案在现有会话归档和上下文运行时之上补齐会话管理。配套设计：[Agent Loop](agent-loop.md)、[上下文窗口](context-window.md)、[上下文实现](context-management-implementation.md)、[知识存储](knowledge-storage.md)。对应实现位于 [session 模块](../../src/session/mod.rs)。

## 1. 目标与首版范围

用户能够找到历史会话、切换后继续学习、退出后恢复，并清楚知道哪一个会话正在运行。不同会话不串上下文、Skill 状态、草稿或模型配置，已完成消息不因切换或异常丢失。

首版范围：

- 创建、列举、搜索、打开、切换、重命名、关闭、归档及取消归档。
- 稳定会话身份、元数据、模型配置引用与学习对象关联。
- 统一 run 执行、片段写入、取消、收尾与恢复。
- 恢复模型上下文和可浏览的聊天历史；持久化设备本地草稿与阅读位置。
- 工作区和独立目录使用同一套管理接口；兼容旧 JSONL 文件。

首版不提供物理删除、自动合并并发分支、复制整个会话、后台多会话并行执行或恢复后自动重放工具。归档保留全部历史，可以撤销。分支冲突必须识别，不能用文件名排序掩盖。

## 2. 现状与需要修正的问题

| 已有实现 | 能力 | 当前限制 |
| --- | --- | --- |
| `SessionArchive` | JSONL 片段 begin/append/finish/list/load | 列表只有 ID 和片段；没有标题、归档、配置或稳定的续接链 |
| `SessionRuntime` | 原始消息、context journal、检查点、Skill 和工具 artifact 恢复 | 位于 context 模块；缺少显式 session 身份，写入者可以随时 set_writer |
| `Application` | `/new`、`/resume`、每 run 创建片段 | 同时承担业务命令、会话状态、模型构建和归档收尾 |
| TUI | 展示实时消息、切换阅读/对话 | `/resume` 恢复模型状态，尚未完整重建聊天显示和会话专属草稿 |

需要优先修正：

1. `SessionRuntime::from_records` 不返回会话身份，独立文件恢复后 Application 可能仍展示新生成的 ID。
2. 工作区 load 未找到片段时可返回空 transcript，`/resume` 不能把不存在的会话当作新会话。
3. 当前恢复把片段直接展平，缺少显式 parent 链；跨设备或多进程写入时可能形成不可继续的伪线性历史。
4. 片段锁防止同一片段重复写入，但还需要 session 级写入租约，防止同一会话同时创建两个续接片段。
5. 会话恢复、模型配置和 TUI 历史替换缺少共同的成功边界。

## 3. 核心设计决策

| 问题 | 决策 | 理由与代价 |
| --- | --- | --- |
| Session 管理放在哪里 | 应用服务层 `SessionManager` | 管理发生在 run 之外，不能依赖某个插件被调用 |
| run 谁执行 | `SessionRunner` 组合 AgentLoop | 统一绑定 writer、配置、取消和 finally 收尾 |
| 上下文状态谁持有 | 每个已打开会话一个 `SessionRuntime` | 继续复用已实现的窗口和 checkpoint 协议 |
| 权威历史是什么 | 不可改写的已关闭 JSONL 片段 | 重命名、归档和 UI 操作不改历史内容 |
| 元数据放在哪里 | `sessions/<id>/session.yaml` | 可独立编辑、比较、同步；需要扩展现有扫描器与路径校验 |
| 列表和搜索从哪里读 | 可重建的 session 索引 | 避免每次列表重放所有大 artifact |
| 首版并发模型 | 单个应用一个活跃可写会话、一个运行中的 run | 生命周期明确，后续可增加后台会话 |
| 切换时遇到运行中任务 | 默认返回 `SessionBusy` | 不隐式取消用户正在进行的工作；用户先 `/stop` |
| 切换提交点 | 目标恢复成功后替换当前 handle | 目标损坏或锁定时当前会话继续可用 |
| 归档语义 | 元数据中的可逆状态 | 已提交 transcript 不允许删除，历史学习证据仍可引用 |
| 恢复依据 | 消息、控制记录和 segment parent 链 | 时间戳只用于展示和列表排序，不能决定因果关系 |

## 4. 模块职责与所有权

```text
Application / TUI / plain CLI
               │ commands / read-only events
               ▼
        SessionManager
        ├─ SessionCatalog：元数据、列表、搜索、归档
        ├─ ActiveSession：稳定身份、配置、本地视图状态
        └─ SessionRunner：串行执行、取消、片段租约与收尾
                   ├─ SessionRuntime
                   │    └─ ContextState / journal / checkpoint
                   └─ AgentLoop + ContextWindowPlugin + Providers/Tools
                            │ durable append
                            ▼
             SessionRepository / SessionWriter
             ├─ 工作区：已有 WorkspaceService / SessionArchive
             └─ 独立目录：文件 backend
```

### 4.1 各模块的边界

- **SessionManager** 管理创建、打开、切换和关闭，不生成摘要，不执行模型工具。
- **SessionRunner** 为一次 run 冻结配置、绑定专属 writer、执行 Loop，并关闭片段；不决定上下文保留哪些内容。
- **SessionRuntime** 是单个会话的已提交运行时视图。它保留只读快照与原有耐久提交语义，不列举其他会话。
- **ContextWindowPlugin** 仍然统一候选装配、预算、压缩和窗口提交，不创建或切换会话。
- **AgentLoop** 调度模型和工具，保留消息提交屏障；只依赖会话能力接口，不依赖 SessionManager 或具体工作区。
- **SessionRepository** 负责存储、版本校验和租约；工作区实现继续经 WorkspaceService 执行受管写入。

将 `SessionRuntime`、`TranscriptWriter` 和会话归档适配移到 `src/session/`，上下文状态与压缩实现留在 `src/context/`。迁移期间可从 context 重新导出旧名称以减少调用点改动。运行时通过窄接口组合 ContextStore，避免 session 管理逻辑与上下文插件互相依赖。

### 4.2 只保留一个身份来源

`SessionHandle` 包含不可变 `SessionIdentity`、当前元数据 revision、runtime、写入租约和设备视图状态。Application 不再另存可独立变更的 `session_id`、`AgentState.messages` 和 session 三份状态。

AgentState 若仍为兼容 Loop 接口保留，只能由 SessionRunner 从 runtime 创建，并在 run 结束后丢弃；不能成为第二份权威历史。AgentResult 优先返回游标与统计，现有消息副本在迁移阶段仍可兼容。

## 5. 标识、状态和元数据

### 5.1 标识

| 标识 | 含义 |
| --- | --- |
| `store_id` | 工作区 repository_id，或独立目录初始化时生成的 UUID |
| `session_id` | 稳定的 `session_<UUIDv7>`，新建一次后不再改变 |
| `run_id` | 一次用户输入触发的执行，跨取消/失败保留 |
| `segment_id` | 一次 run 的物理追加片段，关闭后不可变 |
| `record_ref` | segment_id + 文件内行号 + 内容 hash，用于 UI、来源和诊断 |

跨工作区同名 ID 不视为同一会话；完整身份是 `(store_id, session_id)`。标题不是标识，可重复。UUID 时间不代替创建时间或因果顺序。

### 5.2 生命周期状态与归档状态分开

```text
Closed ── open/restore ──▶ Opening ── success ──▶ Ready
  ▲                         │                    │  ▲
  │                         └─ failure → Closed  │  │
  │                                             │  │
  └──────── Closing ◀── close ───────────────────┘  │
                          Ready ── send ──▶ Running │
                          Ready ◀─ finish ── Running │
                          Ready ◀─ drain ── Cancelling

持久化/收尾失败 → Faulted → 显式重新打开并校验
```

这些状态是本机运行状态，不把 `running=true` 同步到 Git。另用 `archived: bool` 表达持久归档状态，用 `health` 表达完整、待恢复、损坏、分叉等诊断。

关闭只释放本机句柄和锁；不归档、不删除。归档会话可浏览，发送新输入前必须取消归档。普通模型错误可回 Ready；耐久写入或片段收尾失败进入 Faulted，不能假装已保存成功。

### 5.3 元数据格式

```yaml
schema_version: 1
id: session_019...
created_at: "2026-11-07T09:00:00Z"
title: 事务隔离级别学习
title_source: user         # user | first_message | imported
archived: false
targets:                  # 可选关联知识/学习目标；不会自动加载全部正文
  - concept_019...
model:
  profile_ref: tutor-default
  model: configured-model
  format: openai
context:
  context_tokens: 128000
  output_tokens: 4096
```

- `revision = BLAKE3(session.yaml 原始字节)` 由读接口返回，更新必须提供 expected_revision。
- 模型 profile 的 endpoint、凭据引用等设备配置按 profile_ref 解析；元数据不存密钥、认证头或明文令牌。
- `updated_at`、最后一条摘要、消息数量、最近 run 状态和 usage 从持久记录派生，缓存在索引中，避免每轮改写 session.yaml。
- 首条用户问题可确定性生成默认短标题；用户重命名后不再自动覆盖。首版不为标题单独调用模型。
- 旧目标被删除时显示失效引用，不级联删除会话。关联只用于导航和候选检索，不直接改变既有知识对象删除约束。

恢复默认采用会话保存的配置。显式 `/model` 修改当前会话配置并记录后续 run 的实际配置；缺少 profile/凭据时仍允许阅读历史，发送时明确报告缺少配置，不偷偷换模型。模型或工具定义变化由现有上下文兼容性机制建立新 epoch。

## 6. 存储与写入协议

### 6.1 工作区与独立目录

```text
<workspace>/
├─ sessions/<session-id>/
│  ├─ session.yaml                    # 可变元数据，进入 Git
│  ├─ <segment-id>.jsonl              # 不可变归档，进入 Git
│  └─ <segment-id>.jsonl.open         # 活动追加，Git 忽略
└─ .lumen/
   ├─ locks/session-<id>.lock         # 设备租约，不入 Git
   ├─ session-ui/<id>.json            # 草稿/阅读/滚动位置，不入 Git
   └─ index.sqlite                    # session 列表与搜索，可重建
```

独立模式使用同样的 `sessions/<id>/...` 布局，根目录由 `LUMEN_SESSION_DIR` 指定，未指定时使用平台应用数据目录。其 catalog/backend 不依赖知识仓库或 Git。

`LUMEN_SESSION` 继续表示旧单文件兼容入口；不把旧文件悄悄改成目录。未迁移时可以打开其中某个会话只读，显式迁移后在目录模式继续；旧文件保留。

新增 session.yaml 必须同步更新现有 session 扫描器、segment_paths、知识 Catalog、索引、Git 提交和同步校验。它们目前假设 sessions 下全是 JSONL，不能只添加文件而跳过校验链。未知文件类型报告诊断，不误当会话片段解析。

### 6.2 一个 run 一个片段

保持每次 run 分配新 segment，消息、工具 artifact 和 context journal 继续沿用现有 SessionRecord。新片段新增显式管理记录：

| Custom 种类 | 内容 |
| --- | --- |
| `lumen.segment_open` | schema、session_id、run_id、segment_id、parent_segment、父片段内容 hash、运行配置指纹、开始时间 |
| `lumen.run_end` | run_id、停止原因、错误分类、完成消息范围、普通/摘要 usage、结束时间 |

第一条 segment_open 先耐久写入，再执行用户输入提交；run_end 在消息和 context commit 之后写入。结束记录不代表所有工具一定成功，停止原因须与实际结果一致。

**管理记录与上下文记录分开解释。** Repository 加载时先把 segment_open/run_end 提取为管理信息，再把原始消息及现有 context 控制记录按原顺序交给 SessionRuntime。新管理记录不进入模型、不计入现有 raw_hash/through 编号；必须集中使用同一 decoder，禁止 UI 与 runtime 各自过滤一遍造成水位不一致。

### 6.3 片段父链与分叉

新 segment 指向上一已关闭片段及 hash。线性链是首版可继续写入的要求；timestamp 和 UUID 仅帮助展示。

旧片段保持原字节，按已有顺序作为 legacy 前缀读取；第一个新版片段保存该前缀的有序片段清单/hash，后续走显式 parent 链。不回写旧 header，不重新编号旧 context commit。

发现两个片段指向同一个父片段时，返回 `Diverged`，保留两条历史分支供只读查看；首版不自动展平成一个窗口，不覆盖其中任意片段。旧数据没有因果信息且检查点父链不兼容时同样报告歧义，不能声称已自动推断正确历史。

### 6.4 锁、提交与收尾

- 可写打开持有 session 级租约，绑定 store/session；segment lease 继续保护活动文件。其他进程可读已关闭快照，可写打开返回 SessionBusy。
- 固定锁顺序：Manager 切换锁 → 目标 session 租约 → runtime run_lock → 片段写入。执行模型/摘要时不持有工作区数据库事务。
- SessionRunner 为本次 run 创建 writer 绑定；执行期间禁止 set_writer，writer generation 与 run_id 校验，不能把旧 run 结果写入新片段。
- 原始消息与检查点仍在提交边界同步落盘；run_end、文件同步、原子 rename 和目录同步成功后，片段才报告 Closed。
- 失败时记录 primary_error 和 cleanup_error，收尾错误不能覆盖模型或工具的原始错误；健康状态仍显示 Faulted。
- 崩溃后仅恢复无有效租约的 `.open`，保留原件，修复不完整尾行，再关闭为 Interrupted；完整行损坏需显式诊断，不能跳过后继续写。
- 退出和 `/stop` 等待有界收尾。无法完成时保留活动文件供下次恢复，不返回“全部保存成功”。

元数据更新与追加记录不假定跨文件原子。原始历史是运行事实，session.yaml 是属性；索引未更新时可重建。新建先原子写入 metadata 才发布 ID；元数据 CAS 冲突不会导致历史删除。

## 7. SessionManager 与 SessionRunner 接口

以下为目标接口示意，不要求逐字使用这些 Rust 类型。

| 接口 | 行为 |
| --- | --- |
| `create(CreateSession) -> SessionInfo` | 持久化身份和元数据，不调用模型；可立即出现在列表 |
| `list(SessionQuery, PageRequest)` | 按更新时间列举，支持归档过滤、标题和正文搜索 |
| `inspect(SessionId)` | 返回元数据、revision、可恢复性、片段链、占用状态 |
| `open(SessionId, OpenMode) -> SessionHandle` | 只读或可写打开，校验存在性、归档、锁和父链 |
| `switch(SessionId) -> SessionView` | 目标准备成功后切换活跃 handle，失败保留当前会话 |
| `rename(SessionId, title, expected_revision)` | 更新标题，不改 transcript |
| `archive/unarchive(SessionId, expected_revision)` | 更新可逆归档状态；运行中不允许 |
| `close()` | 完成收尾并释放活跃 handle，保留 archive |
| `run(SessionHandle, Input, CancelToken)` | SessionRunner 执行一次 run，返回类型化结果 |
| `cancel(RunId)` | 只取消指定 run，不串到新会话 |
| `history(SessionId, before, limit)` | 返回用于 UI 的分页历史与稳定 record_ref |

`SessionInfo` 至少包含身份、标题、创建/更新时间、归档、关联对象、模型、最近停止原因、健康状态和消息数量。`SessionView` 包含元数据、历史首屏、本地草稿/视图和 restore diagnostics；不把密钥或完整内部 checkpoint 发给 TUI。

搜索默认只索引 user/assistant 可见文本，系统提示、context journal、原始大工具 artifact 不参与普通会话搜索。标题匹配置前，正文可用现有全文设施；同更新时间按 session_id 稳定排序，分页 cursor 带索引 generation，变化时明确刷新。

## 8. 生命周期流程

### 8.1 新建与切换

```text
用户选择目标 / 创建新会话
  → 检查当前 Runner 是否空闲（运行中返回 Busy）
  → 获取目标写入租约，读取 metadata 与片段链
  → 解析与恢复 runtime，校验配置，构建 SessionView
  → 重新校验 manager generation 与目标 head
  → 持久化当前本地草稿/位置
  → 原子替换活跃 handle，发布 SessionActivated
  → 释放旧租约，更新设备最近会话指针
```

目标准备失败时释放目标资源，保持旧 handle、草稿、窗口和显示。打开当前会话是幂等操作，不再创建新 runtime 或重复消息。首版只锁定一个活跃会话，切换事务可短暂同时持有旧/目标租约，获取采用非阻塞 try-lock，避免双进程互换会话造成死锁。

运行中 `/new`、`/resume`、归档当前会话和切工作区均返回 Busy；不先清空 UI。`/stop` 后等待 RunEnded，再重发切换命令。只读列表/详情不需要取消模型工作。

### 8.2 执行一次 run

1. 校验 handle 为 Ready、未归档且 lease 有效，确定 run_id 和冻结配置。
2. 创建片段并保存 segment_open；绑定 writer，进入 Running。
3. 从 runtime 创建 Loop 视图，提交用户输入，再执行现有插件流水线。
4. 实时事件附带 store/session/run_id 与 UI activation generation；消息 durable commit 仍是成功依据。
5. 正常、失败或取消都进入 finally：保存结束记录、关闭片段、解除 writer 绑定。
6. 成功收尾后 Ready；耐久异常后 Faulted，要求从磁盘重新打开。

SessionRunner 拥有运行取消令牌；任务 join handle 不能由切换操作直接 drop。工具结果未知时延续现有“不自动重放、明确 unknown”的策略。

### 8.3 启动恢复

设备本地保存 `last_session_id`，工作区与独立 store 分开记录。启动先显示最近会话状态；可恢复的非归档最近会话恢复为 Ready，不自动发送模型请求。已归档、租约冲突、profile 不可用或损坏时显示对应状态，允许选择其他会话。

打开不存在 ID 必须 NotFound；合法 UUID 不表示存在会话。新建但尚无消息的会话因 metadata 存在而有效，区别于“路径不存在且 transcript 为空”。

profile 缺失是发送限制，不阻断只读浏览。自动恢复不会改变 user 选择的主题或把旧引用直接发给模型。

## 9. 上下文恢复与降级边界

按顺序检查 JSONL 完整性、segment parent 链、管理记录、context schema、checkpoint 父 manifest/覆盖 hash，以及必要 Skill/artifact 来源。

| 情况 | 恢复行为 |
| --- | --- |
| 只有 legacy transcript，没有 checkpoint | 从消息重建窗口，保留来源；不是空会话 |
| 孤立活动片段、末尾不完整 | 备份与修复后标记 Interrupted，保留已完成记录 |
| 最新 checkpoint 派生投影损坏，底层必需记录仍完整 | 选择完整的旧基点并重放；所有水位与 hash 需重新校验 |
| 新版本规则、必要控制记录无法解释 | 只读打开并报告限制，禁止静默丢弃后继续推理 |
| 只有普通消息可读，Skill/必要资料缺失 | 历史可浏览；续聊被限制，允许用户显式从可读历史建立新会话 |
| 两个并发续接片段或父 hash 不符 | Diverged/Corrupt，只读诊断，不自动合并 |
| 缺失工具结果 | 按现有规则补充明确 unknown 结果后继续，不重新执行原调用 |

这是对当前 `from_records` 严格报错行为的细化：先返回结构化 ReplayReport，区分可重建投影与无法解释的必要记录。恢复不能只是 catch error 后清空 context。只读 SessionView 可以使用独立历史 decoder，不要求 SessionRuntime 已可写启动。

## 10. TUI 与命令

| 命令 | 语义 |
| --- | --- |
| `/sessions [query]` | 列表与搜索；TUI 显示会话选择面板，plain 输出分页列表 |
| `/session` | 当前会话摘要、模型、关联对象、健康与运行状态 |
| `/new [title]` | 创建并选择新会话，成功后才替换显示 |
| `/resume <id>` | 打开并切换到已有会话，兼容现有命令 |
| `/session rename <title>` | 当前会话重命名 |
| `/session archive [id]` | 归档指定或当前空闲会话，保留历史 |
| `/session unarchive <id>` | 取消归档 |
| `/session close` | 释放当前会话，不自动创建新 ID |
| `/stop` | 取消当前 run 并收尾 |
| `/clear` | 仅清空当前显示；不清除模型上下文，不创建会话 |

首版列表面板复用 TUI 浮层，不挤占常驻知识目录：展示标题、时间、模型、短 ID、最近状态，支持过滤与选择。完整恢复取最近一页聊天，向上滚动加载更早消息，避免一次渲染全部历史。

`HistoryProjector` 仅将 user/assistant 及必要的工具状态转换为 UI 历史；context summary、Skill envelope、系统规则和 artifact 原文默认隐藏。显示分页不改变模型窗口预算。UI 消息用 record_ref 去重，避免恢复首屏与实时 MessageCommitted 各插入一次。

草稿、阅读对象/revision/章节、滚动位置和引用选区按 `(store_id, session_id)` 保存到设备缓存。旧 revision 选区失效时明确标记，不将旧选区误当新正文。切换成功后恢复目标草稿，失败保留原草稿；本地 UI 缓存丢失不影响 transcript 或上下文。

所有异步事件附带 activation generation；切换后的旧事件只能更新其所属会话的记录，不能写进当前聊天面板。当前 `Application::command -> String` 扩展为类型化 `CommandOutcome`，会话激活返回 SessionView；plain 再渲染为字符串，避免在 TUI 中解析命令输出 JSON 来切换状态。

## 11. Git 同步、元数据冲突与兼容迁移

session.yaml 随 Git 同步，使用原有受管事务和 CAS。重命名或归档冲突进入显式处理，不采用按设备时间“最后写入者胜出”。元数据冲突可允许查看无歧义的历史，但不能在冲突工作区继续写入；沿用既有工作区恢复限制。

已关闭片段继续不可改写；run 管理记录也遵守这一规则。跨设备锁不具备全局互斥能力，segment parent 链负责同步后发现分叉。SQLite、租约、草稿和最近打开指针不进入 Git。

迁移步骤：

1. 先支持 session.yaml 与新管理记录的读取、校验、索引和导出，再启用新写入。
2. 对只有旧 JSONL 的工作区会话，派生只读 SessionInfo；首次显式打开可写时新增 session.yaml，旧片段保持原样。
3. 默认标题从首个可见用户输入派生；创建时间优先旧 session_start 或 ID 的可解释时间，并标记 derived，不能凭文件 mtime 宣称精确历史。
4. standalone 旧单文件先按 `lumen.session_start` 切分；缺失起始标记的第一组确定性分配 ID，持久保存迁移映射。
5. 显式导入使用临时目录、逐会话 replay 校验与目标内容 hash，成功后再发布迁移结果；旧文件保留。重复导入通过源 hash/记录范围映射幂等处理。
6. 必须由同一个记录选择器保留旧会话组内 context 控制记录与 raw 编号；不能过滤 artifact 或重新写消息导致 raw_hash 失效。

未知 schema 明确返回 UnsupportedSchema。非完整迁移只读可用，不提前标注“已支持继续”。

## 12. 实施拆分与验收

建议模块：

```text
src/session/
  mod.rs / types.rs       身份、metadata、handle、错误与事件
  manager.rs              create/list/open/switch/rename/archive
  runner.rs               run、cancel、writer 绑定和 finally
  runtime.rs              迁入现有 SessionRuntime
  repository.rs           存储接口、租约、CAS、原始记录读取
  replay.rs               管理/上下文记录解码、segment 链与诊断
  history.rs              UI 分页投影与稳定引用
  standalone.rs           独立目录和旧文件导入

src/workspace/session.rs  工作区 backend，保留已有归档能力
src/context/              窗口、摘要与候选策略
src/tui/                  会话选择、类型化激活事件与状态恢复
```

| 阶段 | 交付 | 验收标准 |
| --- | --- | --- |
| P0 身份和执行边界 | SessionHandle、runtime 迁移、Runner、writer 生命周期、session 租约 | ID 唯一来源；运行中不切换；收尾失败不误报保存成功 |
| P1 存储与恢复 | metadata、segment parent、ReplayReport、兼容旧记录 | 不存在/空会话可区分；恢复合法；分叉可诊断；旧 hash 不变 |
| P2 会话操作 | list/search/open/switch/rename/archive 与类型化命令 | 目标失败保留原会话，归档可逆，CAS 冲突明确 |
| P3 TUI 与本地状态 | 选择面板、分页历史、草稿/位置和最近会话 | 模型状态与聊天显示一致；事件不串；重启恢复 |
| P4 独立目录与迁移 | 独立 backend、旧文件导入、配置文档 | 幂等导入且原件保留，两种模式命令语义一致 |

每阶段保留现有上下文/Loop/TUI/存储测试，并增加：

1. 两个会话交替打开，模型、Skill、约束、草稿和引用互不污染。
2. 目标不存在、metadata 无效、父链冲突、profile 缺失或租约占用时，原活跃会话仍可用。
3. 运行中切换/归档被拒绝；取消后完成消息与 context commit 能重放，unknown 工具不自动重放。
4. 两个进程可写打开同一会话被租约拦截；两设备同步分叉被 parent 链识别。
5. 用户消息、工具结果、run_end、rename、片段关闭等位置注入失败，primary_error/cleanup_error 可区分。
6. 删除索引和 UI 缓存后仍可重建列表及上下文；UI 损失仅限草稿与位置。
7. 恢复历史首屏与实时事件没有重复，分页稳定且无内部 context 文本泄漏到聊天正文。
8. 元数据更新与归档不修改已提交片段；学习事件的 session_id 关联保持有效。
9. standalone 重启保持相同身份，`/new` 与旧组选择/导入不混淆；重复导入不产生重复会话。
10. 旧/new JSONL、metadata、git 校验和同步兼容，原有连续压缩与缓存前缀测试继续通过。

## 13. 首版实现说明（2026-11-07）

实现位于 [manager](../../src/session/manager.rs)、[runner](../../src/session/runner.rs)、[repository](../../src/session/repository.rs)、[replay](../../src/session/replay.rs)、[runtime](../../src/session/runtime.rs) 和 [应用入口](../../src/tui/service.rs)。

| 设计阶段 | 实现状态 |
| --- | --- |
| P0 身份与执行边界 | SessionIdentity/Handle/Manager/Runner；关闭失效旧 handle，运行中拒绝变更，session 级 OS 租约，writer 绑定检查 |
| P1 存储与恢复 | session.yaml CAS、新 segment header/run_end、父链/hash 校验、结构化 ReplayReport、只读诊断、孤立 .open 恢复 |
| P2 管理操作 | 新建、列表、标题/正文搜索、切换、重命名、可逆归档、关闭；cursor 校验 generation 和查询条件 |
| P3 TUI | 类型化 CommandOutcome、可过滤/分页的会话浮层、分页历史去重、草稿与阅读/引用恢复、运行中只读列表/详情 |
| P4 独立模式与迁移 | 统一目录 backend、平台默认路径、最近会话恢复、旧文件显式预览与幂等导入，旧 checkpoint 不重编号 |

与示意结构的具体对应：

- SessionCatalog 合并在 Manager 与本地 SQLite cache 中；数据库为 `.lumen/session-index.sqlite`，按会话内容指纹缓存摘要和可见正文。索引可删除重建；首版扫描文件计算指纹，尚未使用文件监听减少扫描。
- 核心服务公开 `create/switch/inspect/list/update/rename/archive/close/history`。create 创建并选择新会话，switch 的 readonly 参数控制只读查看；归档恢复通过 archive 的布尔参数执行。
- SessionRuntime 迁入 session，WindowState 暂随 runtime 保留；`context::store` 兼容导出旧接口。受管 runtime 无 run writer 时拒绝追加，外部不能调用 set_writer 接管它。
- 片段管理记录保存具体模型/窗口配置及其 hash；run_end 保存消息范围、停止原因、错误分类和两类 usage。它们只由统一 replay 解码，不进入 runtime 的 raw_hash/through 计数或模型上下文。
- TUI 草稿和阅读位置在切换、关闭、正常退出时保存；不承诺每个按键都耐久落盘。引用 revision 改变时清除旧选区。历史默认每页 50 条，API 上限 200 条；会话列表默认每页 20 项。
- 新目录模式中，默认 profile 使用 `LUMEN_MODEL_ENDPOINT/KEY`。本机 `.lumen/model-profiles.yaml` 支持命名连接配置，密钥通过 api_key_env 在发送时读取，不写元数据。profile 不可用时可读历史，发送返回明确配置错误。
- 存在 `LUMEN_SESSION` 旧文件时不自动导入或改写；使用同路径 `.sessions` 作为新目录，显式 `/session preview` 或 `/session import`，导入返回可 `/resume` 的稳定 ID。后续追加不影响重复导入的幂等判断。
- 恢复采取保守策略：context 控制记录不兼容时只读，避免默默丢失活跃 Skill/约束；尚未自动挑选旧 checkpoint 绕过不可解释的控制记录。完整行损坏只展示已知有效前缀，不跳过损坏行继续推理。
- 单会话单写入者、跨设备父链分叉只读诊断已实现；分支选择与合并、物理删除、后台多会话执行继续保持首版排除范围。

验证见 [Session 集成测试](../../tests/session_management.rs) 与 [TUI 测试](../../src/tui/tests.rs)，包括跨进程租约子测试、目录重建、旧 checkpoint 导入、关闭旧句柄、故障收尾、分叉和损坏诊断，以及阅读/草稿隔离与分页历史。原上下文、Loop、存储与同步测试继续保留。
