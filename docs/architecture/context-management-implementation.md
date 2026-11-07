# 上下文管理的插件集成与实现决策

状态：首版已实现；第 1–13 节保留目标设计与取舍，第 14 节记录实际接口和后续边界。设计日期：2026-11-04。实现更新：2026-11-05。

本文回答“如何在当前插件式 Agent Loop 中实现上下文管理”。[上下文窗口设计](context-window.md) 定义 A–H 区域和策略，本文定义执行顺序、接口、状态所有权、持久化边界与迁移步骤。代码示意均为目标接口，不能直接当作已有 API。

## 1. 核心决策

采用 **一个上下文协调插件 + 多个候选提供插件/服务 + 应用持有的会话运行时**。

| 决策 | 采用方式 | 原因与代价 |
| --- | --- | --- |
| 谁决定最终窗口 | 唯一 `ContextWindowPlugin` | 统一排序、预算、压缩和缓存；协调插件需承担明确的装配职责 |
| 检索、Skill、学习状态如何参与 | 提交带来源的 `ContextContribution` | 不直接修改 system 或最终消息，迁移现有插件接口 |
| 插件顺序如何保证 | Loop 识别协调角色并强制分阶段调度 | 不依赖注册顺序，Loop 增加少量通用生命周期编排 |
| 压缩放在哪里 | 协调插件调用 `CompactionService` | 避免独立压缩插件与检索插件互相覆盖 |
| 窗口状态放在哪里 | 应用持有 `SessionRuntime`，插件只持服务句柄 | 复用跨 run 状态；不能依赖临时 AgentLoop 或插件实例存活 |
| 权威数据是什么 | 追加 transcript + 显式 context journal/checkpoint | 可恢复、可检查，避免插件维护隐藏消息副本 |
| 何时持久化 | 原始消息同步追加；上下文在发送前提交 | 不依赖 UI 消费事件或 run 结束后批量写入 |
| 谁负责厂商格式和计数 | Provider 提供的 `RequestCompiler` | 按实际编码内容计数，避免最后再编码改变预算与前缀 |
| 是否修改核心 Loop | 扩展通用阶段、写入边界和只读状态访问 | Loop 不实现摘要、检索或 Skill 业务 |
| 兼容旧插件的方式 | 显式迁移，关闭协调模式时保留旧路径 | 不把直接改消息的旧插件悄悄混入新流水线 |

第一版不引入通用插件依赖图、全局服务定位器或独立后台压缩任务。并发请求、分支合并及原生 compaction 可在统一边界建立后扩展。

## 2. 为什么现有 hook 不能直接组合出完整方案

当前 [AgentLoop](../../src/agent/loop_runner.rs) 在请求前克隆 `state.messages`，按注册顺序执行全部 `before_request`，过滤 Custom，再调用 Provider。

具体限制：

1. `BeforeRequest` 只有可变消息列表，插件看不到完整工具定义、输出设置和模型容量，不能可靠计算总预算。
2. 后注册插件可以在压缩后继续插入大段资料，前面的预算检查失效。
3. [RetrievalPlugin](../../src/retrieval/plugin.rs) 向 system 拼资料；重新检索会改变前部内容。
4. [CompactionPlugin](../../src/agent/compaction.rs) 生成的 Custom 摘要会被后续过滤，也没有检查点恢复。
5. `PluginContext` 暴露整个可变 AgentState，插件可以改写原始历史，没有只读/投影边界。
6. [Application::chat](../../src/tui/service.rs) 每次创建新的 AgentLoop，run 结束后才归档新增消息。插件内部缓存无法天然跨 run 延续，提前提交的 checkpoint 也可能引用尚未落盘的消息。
7. `after_tool_call` 的结果会直接成为历史；若结果很大，需要在这一时刻归档原文并生成有界结果，不能等下一请求再处理。

仅把压缩插件注册在最后，只能解决部分顺序问题，无法保证持久化、工具总预算与恢复。因此本方案保留插件集成方式，同时补齐必要的核心协议。

## 3. 结构与依赖方向

```text
Application / SessionRunner                         生命周期所有者
  ├─ SessionRuntime                                每会话持久状态与串行写入
  │    ├─ TranscriptWriter                         原始消息追加与游标
  │    ├─ ContextStore                             journal / checkpoint
  │    └─ ArtifactStore                            原始工具结果与资料快照
  ├─ ModelProvider
  │    └─ RequestCompiler                          厂商编码、计数、能力
  └─ AgentLoop                                     通用编排
       ├─ 普通 AgentPlugin                         贡献内容、权限、业务处理
       ├─ ContextWindowPlugin（唯一协调插件）
       │    ├─ WindowAssembler / BudgetPlanner
       │    ├─ CompactionService
       │    ├─ SkillRegistry / SkillLoader
       │    └─ SessionRuntime、RequestCompiler 句柄
       └─ PreparedRequest → ModelProvider          请求封存后直接发送
```

Loop 依赖 trait 和标准类型，不依赖具体工作区、SQLite、检索服务或摘要模型。协调插件不直接调用 TUI，不重新进入 AgentLoop。业务模块经构造函数获得所需依赖；不开启自动检索时不需要工作区服务。

### 3.1 状态所有权

| 数据 | 所有者 | 是否持久化 | 访问方式 |
| --- | --- | --- | --- |
| Transcript 与稳定消息游标 | SessionRuntime / TranscriptWriter | 是 | Loop 追加，插件读取快照 |
| 任务与学习证据 | 现有任务/知识/学习服务 | 是 | 贡献插件读取并附来源 |
| Epoch、checkpoint、注入顺序、Skill 激活 | SessionRuntime / ContextStore | 是 | 协调插件通过受控事务提交 |
| 检索候选、预算计划、候选摘要 | 本次 RequestDraft / ContextPlan | 未提交前否 | 一次请求使用 |
| 编码后的 PreparedRequest | 本次请求 | 通常不存网络报文 | 不可变，只记录安全的内容 hash 和诊断 |
| token 估算与渲染加速缓存 | SessionRuntime 本地缓存 | 可重建 | 按内容与配置 hash 查找 |
| 工具执行中的原始结果 | ToolResultDraft，随后归档 | 大结果先归档再发引用 | 结束工具流水线时处理 |

`Arc<ContextWindowPlugin>` 可以复用，但插件实例不是持久状态来源。每个方法显式接收 `session_id`、`branch_id`、`run_id` 和请求游标，不以全局“当前会话”区分状态。

应用每个会话持有一个 SessionRuntime；`/new` 创建新会话运行时，切工作区不把旧工作区上下文带入新会话。模型切换保留原 transcript，按兼容性建立新 epoch。当前 Application 可以继续每次构建 Loop，但必须注入同一个会话运行时。

## 4. 插件协议：贡献与最终装配分开

### 4.1 引入唯一协调角色

扩展 AgentPlugin 的角色元数据，默认普通插件：

```rust
enum PluginRole {
    Standard,
    ContextCoordinator,
}

trait AgentPlugin {
    fn role(&self) -> PluginRole { PluginRole::Standard }
    // 既有生命周期保留，before_request 的事件类型迁移为分阶段草案。
}
```

AgentLoop 在构建/启动验证时分出普通插件与协调插件：

- 协调模式最多且必须有一个 `ContextCoordinator`，重复或缺失均报配置错误。
- 普通插件保留原注册顺序。协调插件的位置由 Loop 决定，注册先后不影响阶段语义。
- 旧 CompactionPlugin 不能与协调模式同时开启，防止双重压缩。
- 无上下文管理的基础示例可以显式使用 legacy 模式；不能在协调模式失败时自动退回 legacy 并发送未经预算的请求。

角色只改变指定的上下文阶段，不改变权限 hook 的短路规则或 `Continuation::End` 的优先级。不允许插件通过协调角色自行提高执行权限。

### 4.2 RequestDraft：普通插件只贡献候选

`BeforeRequest.messages` 迁移为两种受限视图：普通插件得到 `ContextCollector`，协调插件得到可消费候选的 `ContextAssembly`。这两种事件可作为 `BeforeRequest` 的枚举分支，仍使用既有 `before_request` hook；不另加独立 ContextTransformer 链。

```rust
struct ContextContribution {
    id: ContributionId,
    placement: Placement,       // 当前输入后、某个合法交互边界后等
    region: ContextRegion,     // A–G 逻辑归属
    source: ContextSource,     // 来源、revision、范围
    content: ContextContent,
    requirement: Requirement, // Required / Optional
    authority: SourceAuthority,
    supersedes: Option<ContributionId>,
    dedupe_key: ContentKey,
}

enum BeforeRequest<'a> {
    Collect(ContextCollector<'a>),
    Assemble(ContextAssembly<'a>),
    Legacy { messages: &'a mut Vec<AgentMessage> },
}
```

普通插件可读取固定的 transcript/任务快照，调用 `collector.offer(item)`；不能修改既有消息、工具声明或最终请求。`Required` 表示缺失影响当前任务，不等于更高指令优先级；外部资料即使必需仍是数据。

Legacy 分支仅在显式旧模式中调用，不会出现在协调流水线。插件元数据声明是否需要 legacy 消息变换；协调模式启动时拒绝这种插件。兼容指保留旧运行行为与历史文件读取，不承诺 Rust 源码接口零改动：现有插件、测试和示例需迁移新的事件视图。只做权限或观测、无需请求变换的插件可参与两种模式。

新候选的 placement 只能落在尚未发送的尾部合法边界，不能插进本 epoch 已发送前缀。相同边界内按稳定的来源命名空间与候选序号定序；首次采用后将顺序写入 journal，后续不重排。确实需要重组旧内容时显式建立新 epoch。

工具列表、模型和输出上限由应用在 run 开始冻结。需要改变它们时走显式配置/新 epoch，首版不允许普通 `before_request` 插件偷偷切模型。ContextCollector 提供只读模型能力与建议配额，最终是否采纳及多少预算由协调插件决定。

### 4.3 封存最终请求

协调插件消费 `ContextAssembly`，生成计划、编码计数、提交检查点，然后写入一次性的 `PreparedRequest` 槽位。返回后 Loop 不再运行任何可改请求的 hook。

```text
普通插件 collect → 协调插件 assemble/compile/commit
                → Loop 检查 prepared 结果 → Provider 发送
```

PreparedRequest 的字段对外不可变，包含 Provider/配置指纹、请求 ID、最终消息来源映射、计数报告与编码载荷。Loop 验证它对应本次水位和已冻结的工具/模型配置；出现缺失结果或指纹不匹配就结束当前请求。

注册最后一个普通插件、遥测插件或 UI 观察者都不能再使最终输入变大。遥测通过只读事件观察 PreparedRequest 的安全摘要。

## 5. 生命周期的具体接入点

| 阶段 | 调度顺序 | 上下文工作 | 持久化要求 |
| --- | --- | --- | --- |
| Run 开始 | SessionRunner 初始化，再提交当前 user；协调插件恢复，普通 start hook 随后执行 | 恢复 epoch、水位、活跃 Skill；检测配置变化 | 当前 user 已有 durable cursor，恢复不能重复插入 |
| before_request 收集 | 普通插件按原顺序 | 检索、状态 delta、Skill 选择提交候选 | 候选尚未被采用，不写为可见历史 |
| before_request 装配 | 协调插件最后 | 去重、分区、计数、压缩、编码、校验 | 提交被采用的 context item 和 checkpoint 后才发送 |
| 模型 Complete | Loop 统一提交消息 | 保存完整 assistant；增量仅展示 | assistant 写入成功后才调度其工具 |
| before_tool_call | 普通权限/业务插件，遵守既有短路语义 | 上下文插件不在这里压缩或放行权限 | 记录最终实际执行参数；不能与历史调用不一致 |
| after_tool_call | 普通结果插件先执行；协调插件最后规范化 | 归档原始输出，形成有界模型可见结果；捕获 Skill/知识变化 | 结果与必要 artifact 成功提交后进入 transcript |
| after_turn | 普通插件决定 continuation；协调插件只观察最终结果 | 合并待处理状态、标记下次压缩，不在这里再执行一次摘要 | 新状态以有来源的记录保存；不凭摘要写学习证据 |
| on_agent_end | 普通清理后协调插件释放本次句柄 | 清理临时候选、报告结果 | 不承担唯一一次保存；超时不能丢已提交数据 |

`after_tool_call` 必须先完成所有普通结果变换，再执行协调器的输出限额。协调器只能改变结果的可见内容和引用，不能改 `tool_call_id`、`is_error`、`terminate`；Loop 验证这些字段。所有工具完成后才形成下一个请求，维持现有顺序执行行为。

协调模式首版把 `BeforeToolCall.call` 改为只读，只允许既有 Allow/Block 决策；禁止在 assistant 已持久化后悄悄修改名称或参数。确需替换参数的未来扩展必须同时设计执行记录与模型可见更正，不能通过改写旧 assistant 实现。

协调角色的 after_turn 观察使用独立只读事件，包含普通插件已聚合的 continuation；Loop 忽略其 continuation 返回能力，或在类型层直接取消该返回值。这样不会因协调插件增加一个 Default/End 改变既有业务终止语义。

普通插件不得在长工具过程中注入到未完成的工具调用组中。Skill 加载若通过工具完成，正文保存在对应工具结果中，同时更新活跃记录；组结束后再追加其他 context item，避免重复正文。

## 6. 受控状态访问与会话写入

### 6.1 收紧 PluginContext

当前 `state: &mut AgentState` 迁移为只读 `AgentSnapshot`，并提供显式的受控能力：

- transcript 的稳定游标和已提交消息读取；
- 本次收集器或工具结果事件允许的修改；
- 业务服务的窄接口、取消信号与事件发送；
- 协调角色专用的 ContextStore 提交接口。

插件不直接 append/drop/reorder `state.messages`。Loop 的 `commit_message` 是原始 user、assistant 和模型可见 tool result 的统一入口；context journal 使用同一个会话写入序列。TUI 继续只消费事件。

### 6.2 为什么写入不是普通旁路事件插件

UI 事件消费者可能延迟、断开或被取消；当前 MessageEnd 也只覆盖部分原始消息。检查点不能依赖这些事件最终被消费。因此耐久写入作为 SessionRuntime 的基础能力，由 Loop 和协调插件在明确边界 `await`；具体文件与工作区实现仍隐藏在 trait 后面。

```text
生成待提交消息
  → TranscriptWriter.append（成功写入并确认游标）
  → 更新 AgentState 的已提交视图
  → 发出 MessageCommitted / 原有展示事件
```

有存储时，写入失败不报告成功提交，也不继续发送依赖该消息的模型请求。无工作区运行采用独立文件 backend；测试或用户显式选择临时会话时可用 InMemory backend，并声明恢复能力只到进程内。

取消不再接受新模型/工具工作；已完成消息可以在有界清理预算内完成落盘，不使用已经取消的主令牌立即放弃所有提交，也不创建脱离 run 的后台保存任务。

这会替换 Application::chat 末尾的 `state.messages[before..]` 批量保存，避免消息被写两次。片段 begin/finish 仍由 SessionRunner 管理；on_agent_end 的 5 秒 hook 不充当事务提交的唯一机会。

### 6.3 稳定标识与旧记录迁移

消息引用使用 `MessageCursor = { branch_id, segment_id, record_index }`，另带内容 hash 校验；不能用 `state.messages.len()` 作为长期 ID。所有原始记录和内部 Custom 控制记录都占据各自固定位置，流式 delta 不占消息游标。

现有 JSONL 的 `SessionRecord { message }` 保持可读。旧已关闭片段按行派生游标和 hash，不回写文件。当前活动片段由唯一 writer 预留位置；普通消息追加若返回前断开，以预留游标、预期 tail 和内容 hash 读取确认后重试，不能只凭内容相同判断重复。context commit 另使用持久化在其 Custom 内容中的 operation ID。追加遇到不完整尾行时先按既有恢复协议修复，不在破损尾行后直接追加。

## 7. 一次请求与一次压缩的算法

### 7.1 请求准备

```text
prepare_request(snapshot, contributions):
  校验 session/branch 和配置指纹
  从 checkpoint + journal 恢复同一 epoch 的原前缀
  按游标追加本次新 transcript；按 placement 合并新候选
  按来源、版本、hash 去重，不修改旧记录
  计算 A–G 占用与 H 预留
  若达到水位：调用 compact，得到候选新 epoch
  通过 RequestCompiler 编码并取得最终计数
  若可选内容导致超额：缩小本次候选，重新装配一次
  校验必保项、工具配对、指令权限和硬预算
  提交 context delta / checkpoint / request manifest
  返回不可变 PreparedRequest
```

每个候选记录已采用、未采用、被替代或降级的原因。未采用的检索结果不冒充模型已见资料。新的用户输入通常在本次装配前已经持久化；发给模型的投影可以只覆盖与该输入相关的上下文。

### 7.2 压缩是可取消的子操作，不是新的 Agent Run

`CompactionService` 接收固定 snapshot、旧摘要、精确保留项、近期组和预算，返回 `CheckpointCandidate`。它直接调用摘要推理接口，不重新调用 `AgentLoop::run`，不运行检索插件、不执行工具、不把摘要请求伪装成用户消息写入 transcript。

摘要模型输出只进入候选状态，经校验后成为显式 checkpoint。摘要使用与普通模型调用分开的 `purpose=compaction` 用量分类，输出限额、重试、超时独立配置，但仍受本次 run 总截止时间约束。

默认请求前同步压缩，最多两次最终摘要尝试，分块策略沿用窗口方案。首版不后台抢先压缩，避免结果过期、并发写入和取消处理复杂化。需要延迟优化时，后续可在固定水位上生成候选，提交时仍校验父版本。

### 7.3 安全边界与未完成工具

正常压缩在本轮全部工具结果已提交、下一请求尚未发送时执行；保留当前未完成任务及最近完整交互。不能把未完成调用单独摘要掉，也不能执行一半工具后在历史中移动剩余调用。

若中断导致 assistant 已提交但部分工具没有结果，恢复后先进入工具恢复阶段：读取执行记录，已完成结果补入；对状态未知的调用向用户报告，并写入明确的中断/未知工具结果。首版不自动重放可能有副作用的未知调用。只有调用组形成合法终态，才发起下一次模型请求；上下文压缩不能掩盖工具恢复问题。

### 7.4 检查点提交协议

文件 backend 用单个追加 commit 记录作为生效点，避免假设 transcript、artifact 和多个索引文件能跨文件原子更新：

```text
1. 固定 parent_checkpoint、transcript 水位、context journal 水位
2. 保证覆盖范围内原始消息已 durable
3. 生成候选并校验；需要的 artifact 先写入内容寻址存储
4. RequestCompiler 验证候选窗口能被编码且满足预算
5. 串行 writer 验证 parent 未改变，追加一个完整 context_commit
6. context_commit 落盘后，内存切到新 epoch；最后更新可重建索引
7. 才调用模型发送 PreparedRequest
```

`context_commit` 包含 operation ID、schema 版本、父 checkpoint、覆盖范围、context delta、候选 checkpoint 或其耐久引用、manifest hash。它可以编码为现有 Custom 记录；一个事务的必要 context delta 在同一 commit 中，或通过已经耐久的内容寻址对象引用。无压缩时也用同一协议提交本次新增注入项。

首个 epoch 同时持久化 A/B 的实际模板、工作区规则、Skill 目录快照、工具声明及渲染配置，或引用其耐久内容对象；只保存版本号不足以在安装升级后恢复相同前缀。manifest 不保存 API 密钥或认证头。`fsync`/rename 等耐久动作由 backend 实现，提交确认不能仅表示数据已进入进程缓冲区。

步骤 5 之前取消，旧周期继续有效；commit 已经落盘却在返回前取消，恢复通过 operation ID 识别已生效记录，不能因内存未切换而重复压缩。没有被 commit 引用的临时 artifact 可稍后回收，不能把它当作有效窗口。

本次水位之后的用户消息保留在尾部，不纳入已提交摘要覆盖区。一个分支的会话 writer 串行执行，摘要期间不持有 SQLite/文件写锁；提交时仅短时检查父版本并写入。多设备产生的并发提交作为分支保留，不靠 wall clock 覆盖。

## 8. Provider 边界与最终预算

### 8.1 编码一次，计数后封存

目标拆分为两个职责，均由同一 Provider 配置创建：

```rust
trait RequestCompiler {
    fn capabilities(&self) -> ModelCapabilities;
    async fn prepare(&self, request: ModelRequest) -> Result<PreparedRequest>;
}

trait ModelProvider {
    async fn stream_prepared(
        &self,
        request: PreparedRequest,
        cancel: CancellationToken,
    ) -> Result<ModelEventStream>;
}
```

`ModelRequest` 需要新增输出上限和通用缓存偏好；厂商字段仍由适配器生成。编译器使用最终消息、工具、输出设置和厂商包装生成载荷并计数。发送阶段仅附认证/传输参数，不再插入提示文本、重排工具或改输出上限。

若计数是估算，PreparedRequest 保留估算方式与余量；不能声称获得服务端精确 token 数。服务器仍可能因不可见开销拒绝，按可识别预算错误返回协调层重新收缩一次。它是新的准备尝试，必须产生新的 manifest 并按同一提交协议记录；不能原地修改已封存载荷。无副作用的推理请求可以有界重试，已经执行的工具不随之重复执行。

为减少首轮迁移成本，可保留 `stream(ModelRequest)` 包装入口：内部 prepare 后发送，供 legacy 路径使用。协调模式必须使用同一 Provider 的 PreparedRequest，禁止绕过最终编译/预算检查。

### 8.2 Usage 与厂商状态

扩展 ModelEvent 的 Usage/完成元数据，归一化输入、输出、缓存读写和计数来源；没有字段的厂商保留 `None`。不要把 usage 写成可见聊天消息。

第一版使用可读摘要和标准消息；原生 compaction 的不透明项、response chaining 等后续通过 Provider 专用上下文表示接入。其兼容性和耐久恢复有独立记录，不能简单塞进文本 Custom 后宣称跨 Provider 可用。

## 9. 工具结果、检索与 Skill 的落地

### 9.1 工具输出规范化

工具执行先产生 `ToolResultDraft`，保留原始结果和普通插件变换后的结果；协调插件最后执行：

1. 检查调用 ID 和状态字段；完整原始大结果归档到 ArtifactStore。
2. 提取工具特定关键字段、错误证据、预览与可读范围，形成有界 content。
3. 保存原文 artifact 引用和裁剪标记，允许模型用既有读取工具继续查看。
4. 统一提交模型可见 ToolResult；内部元数据保存原文引用，不在当前代码中假定已有 `details` 字段。

限额必须考虑同一 assistant 的全部调用结果总和；不能只给每个工具固定 N tokens，却允许几十次调用合计超窗。每个单项有限额，当前调用组共享结果预算；必需错误与 ID 保留。最终请求仍需全量计数。

若工具已产生副作用但原文归档失败，保留执行记录并阻止后续模型请求，报告保存失败；不能重跑工具来“补结果”。正常小结果可直接内联，无需额外 artifact。

### 9.2 检索插件迁移

RetrievalPlugin 的 `before_request` 改为向 Collect 视图贡献候选，不插入 system。触发键包含用户消息游标、查询、工作区 generation、检索配置 hash；同一工具续轮复用已提交的注入项。

候选阶段可以缓存检索结果，但只有被协调插件选中的实际片段和顺序进入 journal。工作区处于冲突/恢复状态时沿用既有禁止自动注入规则。检索失败是可观测降级，不阻断不依赖该资料的普通问答。

### 9.3 Skill 与学习状态

SkillLoader 负责选择后读取正文与依赖，协调插件负责去重、放置和窗口预算；SkillRegistry 保存激活、版本、渲染 hash、步骤和失活信息。压缩服务只提出步骤摘要，不能擅自改变用户调用或执行权限。

Skill 在 Collect 阶段激活时通过贡献进入 journal；通过工具激活时利用工具结果内正文，journal 只记录引用与状态，压缩恢复时再重附。必要正文放不下时按窗口方案降级，不加载一半却标记完整。

LearningState 提供有证据来源的相关状态；从回答中生成新评估时，通过现有 learning event 接口写入，只有写入成功后才能在后续窗口标记为已确认状态。模型摘要不充当证据写入接口。

## 10. 错误、取消和事件语义

| 情况 | 决策 |
| --- | --- |
| 普通可选贡献失败 | 记录降级，继续收集其余候选 |
| 必须资料/Skill 无法加载 | 明确缺失，阻止依赖它的请求；不假装已加载 |
| 摘要失败且旧窗口能发 | 保留旧 checkpoint，使用旧周期追加内容 |
| 硬预算无法满足 | 不发送请求；保留所有已提交原始消息 |
| context_commit 写入失败 | 不发送依赖新窗口的请求，不提前切内存 epoch |
| checkpoint 已提交，模型网络失败 | 保留新 checkpoint，重试相同 manifest 或从其后继续，不回滚已生效历史 |
| 取消时已有完整 assistant/工具结果 | 在有界提交中保存；不执行新的工具 |
| 只有模型流式片段，没有 Complete | 作为展示/可选诊断保存，不作为完整 assistant 或合法工具调用 |
| checkpoint 版本不兼容 | 从原始消息与仍能理解的控制记录重建；不可解释的必要规则/资料明确报告，不能假装等价恢复 |

建议新增只读事件：`MessageCommitted`、`ContextPrepared`、`ContextCompactionStart`、`ContextCompactionEnd`、`ContextCompactionFailed`、`ContextDegraded`、`UsageReported`。事件带 session/run/request/epoch 标识、区域占用、原因与游标；不通过事件回调驱动事务。

`ContextCompactionEnd` 在 checkpoint commit 确认后发出，不在模型刚生成摘要时发出。失败事件区分重试后仍可继续与硬性停止，TUI 据此展示。事件 trace 使用摘要和引用，避免把大工具原文复制到无界事件列表。

## 11. 替代方案与取舍

| 替代方案 | 本次不采用的原因 | 适用情形 |
| --- | --- | --- |
| 只固定插件注册顺序 | 没有接口保证；后续插件仍能破坏预算与前缀 | 临时原型 |
| 每个插件自行裁剪自身内容 | 无法分配全局空间，容易丢约束或拆工具组 | 仅用于候选内部预裁剪 |
| 把整个上下文策略写进 Loop | 核心与知识、Skill、模型策略耦合 | 本方案只把通用边界放进核心 |
| 把协调器做成全部业务的巨型插件 | 无法独立替换检索、摘要或 Skill loader | 协调器组合独立服务 |
| 用事件监听器保存全部状态 | 缺少耐久屏障，可能丢消息或提前提交 checkpoint | UI、遥测等观察者 |
| 用插件 mutex 保存当前 messages | 跨 run/会话混用、重启丢失、隐藏事实来源 | 可重建缓存可用 mutex，不持有权威历史 |
| 先后台压缩再覆盖当前窗口 | 需处理并发版本和失效结果，首版收益不足以覆盖复杂度 | 后续固定水位预计算 |
| 只使用厂商原生压缩 | 无法统一跨 Provider 恢复与 Skill 生命周期 | 后续可选压缩后端 |

## 12. 文件与交付拆分

```text
src/agent/
  plugin.rs              角色、受限 before_request 视图、只读 PluginContext
  loop_runner.rs         阶段调度、统一消息提交、PreparedRequest 发送
  provider.rs            compiler/capabilities/prepared/usage 标准边界
  event.rs               提交、上下文、usage 事件
  session.rs             writer 抽象与 legacy JSONL 适配

src/context/             新增模块
  mod.rs / plugin.rs     ContextWindowPlugin 与构造入口
  contribution.rs        候选、来源、放置与去重
  window.rs / budget.rs  分区装配、水位与保留策略
  compaction.rs          摘要服务、候选和验证
  store.rs / runtime.rs  journal、checkpoint、单会话 writer
  artifact.rs            可复用现有内容寻址附件能力

src/skills/              registry、loader、活跃状态
src/retrieval/plugin.rs  贡献式检索适配
src/workspace/session.rs 工作区 writer/checkpoint 适配
src/tui/service.rs       持有 runtime，移除 run 末重复持久化
```

| 步骤 | 主要交付 | 验收门槛 |
| --- | --- | --- |
| 1 协议与调度 | coordinator 角色、Collect/Assemble、只读状态、显式 legacy 路径 | 协调插件注册位置不改变阶段顺序；禁止双协调器与双压缩 |
| 2 会话与耐久边界 | SessionRuntime、cursor、统一 append、commit 操作 | run 中崩溃仍保留已确认消息；移除应用末尾重复写入 |
| 3 编译与预算 | Provider compiler、输出/工具预算、封存请求、工具结果限额 | 实际编码后仍不过预算，后续插件无法追加内容 |
| 4 窗口插件与摘要 | A–H 装配、检查点、通用摘要、失败降级 | 连续多轮与恢复能保留约束，摘要确实到达模型 |
| 5 贡献迁移 | Retrieval、学习状态、Skill registry/loader | 同一续轮不重复检索，Skill 去重与压缩恢复正确 |
| 6 观测与 Provider 优化 | usage、区域展示、缓存参数、回放调参 | 用实际费用、延迟和任务连续性评价 |

步骤 1–4 在 mock 环境完成后统一启用协调模式；真实应用的检索在步骤 5 迁移完成前仍走明确的旧路径，不能出现一半新预算一半旧 system 注入的混用状态。每一步文档标注“已实现/待实现”，不通过添加空 trait 宣称能力完成。

## 13. 必须覆盖的验证场景

1. 普通插件注册顺序不同，协调器仍最后装配；其返回后请求不再被修改。
2. 两次 chat 重建 AgentLoop，但同一 SessionRuntime 保留 epoch、活跃 Skill 和注入顺序；另一个 session 不共享这些状态。
3. 显式用户纠正、来源权限和约束跨三次压缩仍保留；检索数据没有升级为系统指令。
4. 工具 schema、输出额度和全部工具结果参与预算；大单条输入和多工具聚合输出都被处理。
5. 在 artifact 写入后、commit 前、commit 后/内存切换前、模型发送失败等位置注入故障，恢复结果确定且操作不重复。
6. 新用户消息在摘要期间进入队列，恢复后位于覆盖水位之后，没有被遗漏或重复摘要。
7. 取消只留下流式片段时不执行工具；已有副作用但未知结果时不自动重放。
8. 摘要失败、低收益、超预算、未知 Custom 版本都能有界终止或安全降级。
9. 删除可重建缓存、恢复旧 JSONL、切换模型后仍能从耐久来源重建。
10. 现有 Agent Loop 的权限阻断、工具顺序、终止优先级、事件和取消语义不被上下文阶段改变。


## 14. 首版实现说明（2026-11-05）

应用已默认开启协调模式。实现位于 [context](../../src/context/mod.rs)、[Skill registry](../../src/skills/mod.rs)、[Loop](../../src/agent/loop_runner.rs) 和 [应用集成](../../src/tui/service.rs)。

| 能力 | 实际实现 |
| --- | --- |
| 分阶段插件 | `BeforeRequest::{Collect, Assemble, Legacy}`；唯一协调角色、legacy 冲突和运行时一致性检查 |
| 只读历史 | `PluginContext.state: &AgentState`；工具调用参数只读；Loop 统一提交完整消息 |
| 模型边界 | `ModelProvider::prepare` / `stream_prepared`，PreparedRequest 字段封存，HTTP 发送沿用已编码 payload |
| 窗口预算 | A–G 分区报告、H 预留、80% 水位、55% 摘要目标、硬超限拒绝；可调整 ContextConfig |
| 压缩 | 同步、最多两次最终尝试；结构化 JSON 摘要；超大历史按完整工具组分块，最多 16 块 |
| 用户意图 | 原始历史 user 消息逐字保留，作为 C 的保守快照；近期交互与最新纠正保留原文 |
| 数据来源 | 检索资料不写 system，工作区规则带 revision；相关学习状态从已有 evidence_ids 推导 |
| Skill | 名称空间目录、搜索、加载、资源读取、去重、progress、finish、重新激活；活跃指导与步骤随检查点恢复 |
| 耐久状态 | SessionRuntime、追加 `context_pending` / `context_commit`、父 manifest、覆盖 hash、operation ID、恢复重放 |
| 工具原文 | 超限或被插件变换时归档原始 ToolResult，有界预览通过 `context_artifact_read` 回读 |
| 应用命令 | `/context`、`/compact`、`/skills`、工作区 `/resume <session-id>`；standalone 文件自动恢复最新会话 |
| 观测 | 普通调用与摘要调用分别报告 usage；窗口事件带输入估算、预算和 manifest |

落地时作了以下收敛：

1. 编译职责直接作为 ModelProvider 的 `prepare` 方法提供，复用同一个 Provider 配置；它与发送分离，暂不额外建立 RequestCompiler trait。当前计数是保守 UTF-8 字节估算加协议余量，真实 usage 单独报告；未接入厂商计数接口或模型 tokenizer。
2. SessionRuntime 自身承担 ContextStore 和单会话 writer，使用原始消息序号与覆盖 hash 定位。旧 SessionRecord 文件保持可读；单分支串行恢复，父链冲突明确拒绝，不自动合并并发分支。
3. 首版在 JSONL 的内部 Custom 记录中保存工具原文及 context delta，保证删除 `.lumen/` 后仍可恢复。还没有把大文本外置为独立 assets；大输出的归档增长需按实际任务观察。
4. 固定核心规则由应用写为初始 System，工作区规则、目录和活跃内容作为带来源的后续指导/数据追加。逻辑分区不强行改变原始对话顺序。
5. 原始用户消息全部保留，减少约束提取错误；大量长用户输入仍可能耗尽窗口，此时要求分段或缩小输入。未实现自动推断长期偏好或用摘要修改学习证据。
6. `/compact` 设置下一模型请求的压缩意图；它不会额外制造一条用户消息。摘要失败且旧窗口可用时继续，硬预算不满足则停止发送。
7. 已完成工具结果在后处理取消时仍保存；中断后缺失结果使用明确的 unknown 错误补齐，不自动重放有副作用的调用。写入失败/超时使当前 runtime 停止接受新写入，要求从耐久记录重新打开。
8. 原生 compaction、厂商精确缓存诊断/定价、跨设备分支交接、SSE 和后台预压缩仍为后续能力。当前 HTTP adapter 支持 OpenAI 兼容 Chat Completions 与 Anthropic Messages 的完整响应和 usage。
9. Skill 工具返回内部更新 envelope，由协调插件在最终 after_tool_call 阶段批量持久化并移除 envelope；工具的可取消 future 内不写活跃状态。一次 finish 同时退休正文和步骤，防止取消或部分提交造成恢复后状态分裂。

配置见 [README](../../README.md)。验证包含原 Agent Loop/知识存储/TUI 测试，以及 [上下文集成测试](../../tests/context_management.rs)：连续压缩、前缀稳定、Skill 步骤、检查点取消边界、故障恢复、工具归档、分块摘要和会话重新打开。服务端真实缓存命中和模型摘要质量需要用配置好的实际服务继续评测，离线测试不作此保证。

后续会话身份、生命周期、历史展示与运行归档的管理方案见 [Session 管理设计](session-management.md)。该方案尚未实现，不改变本节现状。
