# Lumen Agent Loop 架构

**状态：** 设计基线（Design Baseline）  
**日期：** 2026-10-14  
**适用范围：** Lumen 的 Rust Agent runtime、TUI、模型适配器、工具和可插拔上下文能力

**实现更新（2026-11-05）：** 已接入分阶段上下文插件、SessionRuntime、请求封存和结构化压缩。本文保留早期接口示意；当前接口以 `src/agent/` 和 [插件集成方案第 14 节](context-management-implementation.md#14-首版实现说明2026-11-05) 为准。

**会话管理更新（2026-11-07）：** 应用通过 SessionManager/SessionRunner 管理身份、片段与运行状态；Loop 仍只提交消息和执行模型/工具。事件由 Runner 附加会话、run 和激活标识，恢复时集中剥离管理记录，不改变上下文检查点的编号。详见 [Session 管理](session-management.md)。

本文是 Agent Loop 的架构事实依据。它记录 Lumen 当前采用的边界和行为约定；实现可以逐步落地，但不应在没有更新本文的情况下改变核心语义。

## 背景

Lumen 是面向长期学习与研究的终端助手，核心循环需要支持学习路线、导师对话、复盘、检索和本地记录。

## 核心概念

### Run、turn 和 message

- **Run**：一次从用户输入开始、直到 Agent 结束的完整执行。
- **Turn**：一次 assistant 响应以及该响应触发的工具执行。工具结果存在时，通常会继续下一个 turn。
- **Message**：Agent 内部的标准消息。它与具体模型供应商的消息格式分离。

典型执行如下：

```text
user
  -> assistant(tool_call)
  -> tool_result
  -> assistant(tool_call)
  -> tool_result
  -> assistant(final text)
```

## Agent Loop 职责

Loop 只负责以下事情：

1. 将用户消息加入当前上下文。
2. 在每次模型请求前运行请求准备和上下文变换。
3. 调用 `ModelProvider` 并消费标准化流事件。
4. 将 assistant 消息加入上下文。
5. 发现、校验并调度工具调用。
6. 将工具结果加入上下文。
7. 发出生命周期事件。
8. 根据工具结果、排队消息、插件决定和取消信号判断是否继续。

Loop 不直接依赖 TUI、具体模型、数据库、上下文压缩、检索或权限系统。

### 推荐的状态机

```text
Idle
  -> Starting
  -> Requesting
  -> Streaming
  -> ExecutingTools
  -> (Requesting | WaitingForInput | Completed | Failed | Aborted)
```

如果 assistant 没有 tool call，且没有插件要求继续，Run 进入 `Completed`。如果存在工具结果或 steering message，则回到 `Requesting`。

## Rust 核心接口

### AgentContext

```rust
pub struct AgentContext {
    pub messages: Vec<AgentMessage>,
    pub system_prompt: Option<String>,
    pub tools: Vec<Arc<dyn AgentTool>>,
    pub metadata: serde_json::Value,
}
```

`messages` 是当前 Agent 的事实来源。插件不应维护不可见的消息副本；需要持久状态时应使用 session 或插件状态存储。

### Message

```rust
pub enum AgentMessage {
    System { content: String },
    User { content: Vec<ContentPart> },
    Assistant { content: Vec<AssistantPart>, stop_reason: StopReason },
    ToolResult { tool_call_id: String, content: Vec<ContentPart>, is_error: bool },
    Custom { kind: String, content: serde_json::Value },
}
```

`Custom` 消息可承载学习进度、检索状态、复盘结果等内部信息。它们是否进入模型上下文由 `ContextTransformer` 决定。

### ModelProvider

```rust
#[async_trait]
pub trait ModelProvider: Send + Sync {
    async fn stream(
        &self,
        request: ModelRequest,
        cancel: CancellationToken,
    ) -> Result<ModelEventStream>;
}
```

Provider 负责厂商协议适配，必须输出统一的 `ModelEvent`，例如文本增量、思考增量、工具调用增量、完成和错误。Loop 不读取厂商原始响应。

### AgentTool

```rust
#[async_trait]
pub trait AgentTool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn input_schema(&self) -> JsonSchema;

    async fn execute(&self, args: Value, ctx: ToolContext) -> Result<ToolResult>;
}
```

工具结果包含模型可见的 `content`、供 UI 和持久化使用的 `details`、错误标记和可选的 `terminate`。工具默认顺序执行；确认无共享可变状态后才允许并行执行。

## 上下文和模型请求边界

核心上下文与 provider 上下文必须分离：

```text
AgentMessage[]
  -> ContextTransformer（检索、压缩、过滤、注入系统信息）
  -> Message[]（provider 格式）
  -> ModelProvider
```

`ContextTransformer` 只改变本次请求的投影，不应悄悄修改长期 transcript。上下文压缩产生的摘要、分支或编辑应通过 Session 层显式保存。

后续插件可以在 `before_request` 阶段：

- 根据 token 预算压缩旧消息；
- 注入学习目标、路线和近期笔记；
- 检索相关资料；
- 切换模型或 reasoning level；
- 过滤仅供 UI 使用的消息。

上下文窗口的区域、预算、缓存、Skill 生命周期与压缩检查点方案见 [上下文窗口设计](context-window.md)。首版实现及保守策略见配套实现方案第 14 节。

对应的 [插件集成与实现决策](context-management-implementation.md) 已落地唯一协调角色、候选收集/最终装配、统一消息提交与请求封存。普通插件仍按注册顺序执行，协调插件在请求和工具结果阶段最后处理；PluginContext 的 state 现为只读。

## 插件模型

插件围绕生命周期注册，而不是重新实现 Agent：

```rust
#[async_trait]
pub trait AgentPlugin: Send + Sync {
    fn name(&self) -> &str;
    async fn before_agent_start(&self, event: &mut AgentStartEvent, ctx: &mut PluginContext) -> Result<()>;
    async fn before_request(&self, event: &mut BeforeRequestEvent, ctx: &mut PluginContext) -> Result<()>;
    async fn before_tool_call(&self, event: &mut BeforeToolCallEvent, ctx: &mut PluginContext) -> Result<ToolDecision>;
    async fn after_tool_call(&self, event: &mut AfterToolCallEvent, ctx: &mut PluginContext) -> Result<()>;
    async fn after_turn(&self, event: &mut TurnEndEvent, ctx: &mut PluginContext) -> Result<Continuation>;
    async fn on_agent_end(&self, event: &AgentEndEvent, ctx: &mut PluginContext) -> Result<()>;
}
```

`PluginContext` 提供取消信号、事件发送、工具注册、session 和受控状态存储。插件不直接持有 Agent 的可变引用。

插件返回值必须能够表达行为决策：

- `ToolDecision::Allow`；
- `ToolDecision::Block { reason, terminate }`；
- `ToolDecision::ReplaceArguments(Value)`；
- `Continuation::Default`、`Continue`、`End`。

处理器按注册顺序执行。变换型结果采用前一个处理器的结果作为下一个处理器的输入；通知型事件只观察，不改变 Loop。

## 生命周期事件

```rust
pub enum AgentEvent {
    AgentStart,
    AgentEnd { reason: StopReason },
    TurnStart,
    TurnEnd { assistant: AssistantMessage, tool_results: Vec<ToolResultMessage> },
    MessageStart(AgentMessage),
    MessageDelta { message_id: String, delta: String },
    MessageEnd(AgentMessage),
    ToolCallStart(ToolCall),
    ToolExecutionStart(ToolCall),
    ToolExecutionUpdate(ToolUpdate),
    ToolExecutionEnd(ToolExecutionResult),
    ContextCompactionStart,
    ContextCompactionEnd { summary: String },
    Error(AgentError),
}
```

TUI、CLI、JSONL 和测试都只消费事件，不读取 Loop 内部状态。这保证不同界面共享同一套 Agent 行为。

## 消息调度

需要区分三种输入：

- **当前消息**：当前 turn 正在处理的用户输入或工具结果。
- **Steering message**：Agent 运行期间用户插入的消息，在当前 assistant turn 完成后进入下一次请求。
- **Follow-up message**：Agent 已经准备结束后到达的消息，触发新的 turn。

插件生成的上下文消息应带有来源标记，避免将 UI 状态、检索结果和用户原话混为同一类型。

所有网络、模型和工具操作都必须绑定 `CancellationToken`。取消后 Loop 应停止当前工作，并保留已完成的消息和工具结果。

## Lumen 的基础插件

第一批插件建议固定为：

1. **ContextWindowPlugin**：估算 token、压缩上下文、保留最近对话和关键学习状态。
2. **SessionPersistencePlugin**：保存 transcript、模型变更、工具调用和压缩记录。
3. **PermissionPlugin**：在危险或外部副作用工具执行前阻断或请求确认。
4. **RetrievalPlugin**：检索本地笔记、学习路线和外部资料。
5. **LearningStatePlugin**：维护掌握程度、薄弱知识点、练习记录和复习时间。
6. **TelemetryPlugin**：记录模型耗时、token、工具耗时和错误。

这些插件可以独立启用，不应改变核心 Loop 的消息协议。

## 推荐模块结构

```text
src/
├── agent/       # loop、message、event、tool、provider、plugin、error
├── plugins/     # context、compaction、memory、retrieval、permissions、telemetry
├── session/     # transcript、store、持久化
├── learning/    # goals、plans、progress、review
├── model/       # provider adapters and mock provider
└── tui/         # event consumer and presentation
```

依赖方向应保持为：

```text
Agent Loop -> traits (Provider / Tool / Plugin / Session)
TUI        -> Agent events
Concrete implementations -> traits
```

## 实施顺序和验收标准

1. 用 mock provider 实现无工具的单 turn Loop。
2. 加入工具调用、参数校验和 tool result。
3. 加入流式事件和取消。
4. 加入插件注册与 `before_request`、工具和 `after_turn` hook。
5. 实现上下文窗口、权限和 session 三个基础插件。
6. 接入 Lumen 的学习目标、路线、笔记和复盘能力。

每个阶段都应有可重复测试，至少验证：

- 普通文本响应只产生一个 turn；
- 工具调用会形成下一次模型请求；
- 工具错误可被模型看到；
- 插件可以阻断工具和要求结束；
- 上下文变换不破坏长期消息；
- 取消不会丢失已经完成的消息；
- 事件顺序稳定；
- 并行工具的结果仍按 assistant 原始调用顺序写入上下文。

## 早期实现记录（2026-10-14）

实现位于 `src/agent/`，可以运行 `cargo run --example minimal_agent` 和 `cargo test`。

- `AgentLoop::run` 支持顺序工具、多轮调度、每次 run 独立的最大轮数预算。
- `ModelProvider` 接收标准消息与完整工具声明；供应商适配器负责协议编码。
- `AgentPlugin` 提供 start、request、before/after tool、after turn 和 end hook；默认实现为空。
- `before_request` 修改请求消息副本；未转换的 `Custom` 消息会被过滤，原 transcript 保留。
- 工具 schema 使用 JSON Schema 校验；不存在、无效参数、执行错误和超时转换为错误工具结果，交给模型处理。
- 多个插件按注册顺序组合：工具阻断立即生效；End 优先，Continue 不被后续 Default 覆盖。全部工具结果要求 terminate 时才停止自动续轮。
- `with_event_sink` 可将事件实时发送至 Tokio channel；结果也包含完整事件记录。
- `AgentResult.error` 保存运行错误，结果包含已完成消息。取消和错误也发送 AgentEnd。end hook 的错误发送 Error，不覆盖原始停止原因。
- 默认工具超时为 30 秒，整个 run 超时为 300 秒，end hook 超时为 5 秒。取消会丢弃正在等待的 future；插件和工具不得启动脱离运行生命周期的后台工作。

该阶段接口为 Rust API 草案，不保证二进制 ABI 稳定，当时 PluginContext 提供可变 state；2026-11-04 已改为只读。插件采用进程内 Rust trait；动态库、WASM 或脚本插件加载尚未实现。

该阶段尚未实现真实模型适配器、TUI、队列输入、session 持久化和上下文压缩，后续已陆续落地。`before_request` 仍是上下文策略入口；流式增量用于展示，只有 Complete 消息写入 transcript。事件 channel 和结果 trace 当前没有大小限制，调用方应持续消费 channel。

## 第二阶段模块

当前仓库还提供以下可插拔实现：

- `ProviderRegistry` 统一解析连接、模型和能力，三个独立适配器覆盖 Chat Completions、Responses 与 Messages；HTTP/SSE 输出统一的最终结果和增量事件。`HttpProvider::openai/anthropic` 保留旧 JSON 构造入口。详细边界、错误和兼容规则见 [供应商模块](provider-management.md)。
- `SessionStore`：以 JSONL 追加 `AgentMessage`，支持异步 `append` 和 `load`。它是 transcript 存储，不自动改变 Loop；应用层可在 `MessageEnd` 或 `AgentEnd` 时写入。
- `MessageQueue`：区分 `User`、`Steering`、`FollowUp` 和 `Stop` 输入，为外层 session runner 提供有界异步队列。
- `CompactionPlugin`：保留为显式 legacy 路径，按消息数截取旧文本；协调模式禁止与它混用。应用现使用 `ContextWindowPlugin` 的预算、模型摘要和可恢复检查点。
- `lumen-tui`：最小终端交互程序，展示当前 Agent 输出并支持 `/quit`。完整键盘驱动 TUI 应继续消费 `with_event_sink` 的事件 channel。
