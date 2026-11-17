# 核心规则与工具指导

应用核心规则位于 `src/agent/prompts/core.md`，由 `LUMEN_CORE_PROMPT` 编译进程序。当前版本为 v1，覆盖任务交流、学习支持、研究证据、工具与 Skill、知识写入、上下文解释和交付。修改行为契约时同时提升 `src/agent/prompt.rs` 中的版本号。

## 分层职责

- A 区的核心规则表达跨任务原则。时间、模型容量、工作区快照和用户学习状态不写入固定模板。
- 各工具的 description 表达用途、输入前提、结果边界与写入语义；schema 仍由工具实现定义，注册和执行权限不由提示词扩大。
- 具体任务的长流程由按需加载的 Skill 提供。核心模板不预装文献综述或精读流程，也不要求每个问题都检索或采用引导教学。

## 会话升级与恢复

每次运行前，SessionRunner 检查最后一条应用自有核心规则。若与当前模板不同，就在本次会话片段中追加新的 System 消息；同版本文本不变时不重复追加。比较完整文本，因此忘记提升版本号时也不会继续使用不同的旧文本。旧单段英文核心提示被明确识别为可升级版本。

应用自有规则使用 `Lumen core rules v` 前缀标识，只有 System 消息参与识别；用户消息或检索资料中的相同文字不会被升级为规则。普通 SDK 调用者传入的自定义 System 消息不被替换。

上下文装配将最新应用核心规则置于请求开头，移除投影中旧的应用规则，并提交新的 epoch。原始 transcript 保留全部旧消息，其他自定义 System 消息继续保留。更新从已经压缩的投影继续，不重放被摘要替代的旧交互。legacy 请求路径也执行相同的规则归一化。

升级在下次发送消息时生效，不在运行中修改已封存请求；写入或检查点提交失败时不发送依赖该更新的请求。恢复通过原有会话日志和 checkpoint 重放完成。原生的 prompt cache 命中不作保证。

## 设计参考与验证边界

参考项目中的通用机制，按 Lumen 实际能力重新编写：

- [Hermes Agent 提示组装](https://github.com/NousResearch/hermes-agent/blob/main/agent/system_prompt.py)：稳定规则与运行时上下文分离。
- [Hermes Agent 行为指导](https://github.com/NousResearch/hermes-agent/blob/main/agent/prompt_builder.py)：根据真实工具结果报告完成情况。
- [Open Deep Research](https://github.com/langchain-ai/open_deep_research/blob/main/src/open_deep_research/prompts.py)：范围、阶段职责与研究停止条件。
- [Open Paper](https://github.com/khoj-ai/openpaper/blob/master/server/app/llm/prompts.py)：结论对应来源证据，区分资料片段与全文。

自动化测试覆盖提示更新的追加保存、恢复去重、自定义规则与用户文本保留、压缩后升级不复活旧历史，以及模型请求采用最新核心规则。它们验证装配和持久化契约，不证明模型一定遵守自然语言规则。

模型行为评测应比较：最新纠正覆盖旧目标、摘要不冒充全文、写入失败不报告成功、没有证据不记录掌握、检索重复时停止或改变方法。此类结论需要真实模型任务评测。
