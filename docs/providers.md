# 模型供应商配置

在当前工作区或独立会话目录下创建 `.lumen/providers.yaml`。此文件保存在本机，不进入 Git；会话只保存模型选择与实际配置摘要，密钥通过环境变量读取。

```yaml
schema_version: 1
default_model: research
providers:
  gateway:
    protocol: openai_chat
    base_url: https://gateway.example/v1
    state_scope: my-account
    auth:
      kind: env
      variable: LUMEN_GATEWAY_KEY
    transport:
      mode: sse
      connect_timeout_ms: 10000
      first_byte_timeout_ms: 60000
      idle_timeout_ms: 30000
      request_timeout_ms: 120000
    protocol_options:
      output_token_field: max_completion_tokens
      stream_usage: true
models:
  research:
    provider: gateway
    model_id: actual-model-id
    capabilities:
      context_tokens: 64000
      max_output_tokens: 4096
      tools: true
      tool_choice: [auto, none, required, named]
      temperature: false
    defaults:
      max_output_tokens: 4096
      cache: provider_default
```

示例地址、模型 ID 和容量需替换成所用服务的配置。自定义模型容量必须显式填写，程序不根据名称猜测规格。有效窗口与输出取模型限制和会话设置中的较小值。TUI 已有工具工作流要求模型声明 tools=true；不支持工具的模型可以通过空工具请求调用标准 Provider 接口。

连接的 state_scope 是非敏感的账号/项目标识，不用于认证。同一账号轮换密钥时保持不变，更换账号或项目时修改它。无需认证的本地服务显式设置 `auth: { kind: none }`。

## 协议与端点

| protocol | 在 base_url 后追加 | protocol_options |
| --- | --- | --- |
| openai_chat | chat/completions | output_token_field（max_tokens 或 max_completion_tokens）、stream_usage |
| openai_responses | responses | 首版留空 |
| anthropic_messages | messages | cache_control（默认 true，仍需模型声明缓存能力） |

base_url 应包含服务前缀，例如 `/v1`，无需包含最后的 API 路径。新配置不允许 URL 用户信息、query 或 fragment。旧 profile/env 中的完整 endpoint 保留原样。

transport.mode 默认 sse，支持实时文本和工具参数增量。服务不支持 SSE 时显式设置 json；程序不会在流式失败后自动重发非流式请求。streaming 能力默认由协议/传输模式提供；显式声明 streaming=false 的模型必须使用 json。

## 使用命令

- `/models` 查看模型别名、容量和默认模型。
- `/providers` 查看连接、协议和认证方式，不要求配置好密钥。
- `/model research` 选择配置中的别名；`/model mock` 显式选择演示模型。
- `/new [title]` 使用配置的 default_model 创建会话。

运行中可以查看模型和连接列表；修改模型需要停止当前任务并等待收尾。缺少密钥时历史仍可浏览，发送返回配置错误。配置变更从下一 run 生效，同一 run 的工具续轮和摘要使用冻结的连接。

## 缓存、用量与错误

cache 支持 provider_default 和 disabled。前者按协议与能力发送缓存提示；后者不申请缓存，不保证关闭服务端自动缓存。Anthropic 的显式提示还需要配置 `capabilities.cache_modes: [disabled, provider_default]`，连接可以用 cache_control=false 禁用该扩展。兼容网关不支持时不发送未声明的缓存参数。

输入缓存读取/写入是总输入的分类，推理用量是总输出的分类，不重复相加；未知计数保持未知。实际 usage、模型结束原因、请求 ID 与耗时保存在运行记录中，摘要单独记账。窗口预算当前使用字节启发式估算，并保留安全余量。

达到输出上限和拒绝会显示独立状态，本轮工具不执行；断流不把未完成的 Assistant 当作已保存消息。HTTP 不自动重试、不自动换供应商；取消会释放响应体。单个 SSE event 上限 1 MiB，单次响应累计上限 32 MiB。

## 旧配置和原生状态

没有新配置时，保留 LUMEN_MODEL_ENDPOINT、LUMEN_MODEL_KEY、LUMEN_MODEL、LUMEN_MODEL_FORMAT 和 `.lumen/model-profiles.yaml` 的原有入口。旧 openai 明确映射 Chat Completions，anthropic 映射 Messages，沿用完整 JSON 响应模式。新配置不会重解释旧会话或用这些变量覆盖已选别名。旧 `/model <name>` 在没有新配置时继续使用原模型名语义。

旧 metadata 可读，修改或执行新 run 时通过 CAS 升级到 V2；旧 JSONL 和 context checkpoint 不改写。

Responses 使用 store=false 和本地历史回放；Messages 的必要原生 block 和签名也随所属消息保存。原生状态只回传到匹配的连接、路由、state_scope、协议与模型。切换遇到 IncompatibleState 时，可在原模型执行 `/compact`，完成下一次请求中的压缩后重查；如果近期窗口仍有不兼容项，使用 `/new`。压缩不会承诺清空所有近期原生状态。

实现决策见 [实施方案](architecture/provider-management.md)。协议与生命周期测试通过可注入的 HTTP 边界运行；允许本地监听端口的环境可额外执行：

```bash
cargo test --test provider_management reqwest_transport_loopback_smoke -- --ignored
```
