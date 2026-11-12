# 模型供应商配置

可以在程序中添加供应商和 API Key，也可以在当前工作区或独立会话目录下编辑 `.lumen/providers.yaml`。配置保存在本机，不进入知识仓库同步；会话只保存模型选择与实际配置摘要。

## 供应商、认证与模型选择

流程参考 [pi 的供应商认证](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/providers.md)和[模型选择](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/models.md)：供应商目录、认证信息和模型定义分别管理。

```text
/login
/model
```

1. `/login` 打开供应商目录，输入名称筛选、↑↓ 选择、Enter 继续。也可 `/login <供应商ID>` 直接打开认证配置。
2. 预设填入地址、协议和环境变量名。认证方式通过 ←→ 选择 API Key、环境变量或无认证。已有环境变量时预选环境变量；否则云服务焦点位于隐藏的 Key 输入框。Tab/↑↓ 切换字段，Ctrl+U 清空，最后一项 Enter 或 Ctrl+Enter 保存，Esc 取消。
3. `/model` 打开统一模型选择器。已有模型可筛选后 Enter 直接切换；首次使用时选择供应商的“查询/刷新远端模型”，随后从远端列表选择模型并配置容量、别名和工具能力。

内置目录包括 OpenAI、Anthropic、DeepSeek、OpenRouter、Groq、Mistral、xAI 和本地 Ollama，所有预设使用同一套逻辑。目录定义在 `src/provider/catalog.json`，新增使用已有协议的预设无需修改界面分支。目录会合并当前配置中的自定义供应商；已配置的地址、协议和账号作用域优先于预设。重复 `/login` 只修改认证，不改路由。

“认证可用”表示本机能取得凭据或配置为无认证，**不表示服务端已验证密钥**。保存认证不发送网络请求；模型查询成功会缓存到 `.lumen/model-cache.json`，后续 `/model` 可离线读取。连接或认证引用改变时忽略旧缓存；缓存损坏不影响已注册模型和会话。缺少凭据的模型不出现在交互选择器中，仍可通过 `/models` 查看本机定义或 `/model <别名>` 显式选择。

### 自定义服务

在 `/login` 列表选择“添加自定义供应商”，填写 ID、Base URL、API 协议、认证方式。支持现有三种协议：`openai_chat`、`openai_responses`、`anthropic_messages`。无需认证的本地服务显式选择“无认证”。供应商品牌和协议分开，不根据名称或 URL 猜测协议。

兼容旧入口：

```text
/provider add
/provider add openai
/provider add myapi https://gateway.example/v1
/provider key myapi
```

无参数 `/provider add` 打开同一供应商目录；指定名称和地址则直接打开表单。`/provider key <名称>` 打开隐藏输入框修改已有 Key。高级传输选项仍可编辑 YAML。

### 模型查询和退出认证

```text
/models myapi
/provider models myapi
/models
/providers
/model research
/logout
/logout myapi
```

`/models <供应商>` 与 `/provider models <供应商>` 查询 `GET <Base URL>/models`。OpenAI 兼容协议使用 Bearer 认证；Anthropic 协议使用对应请求头和 `after_id` 分页。无参数 `/models` 查看本机模型定义，`/providers` 查看已配置连接。

接口仅返回 ID 时，模型容量必须按实际规格手工填写，不按名称猜测。配置模型时确认工具能力及是否设为新会话默认；默认不替换已有默认模型，首个模型自动成为默认。保存后尝试切换当前会话；原生状态不兼容时配置仍保留，可新建会话后切换。查询失败不删除认证、已有模型或上次成功缓存，也不会发起聊天请求。

`/logout` 选择已配置供应商并移除本机 Key，保留模型、会话和连接；`/logout <名称>` 直接执行。环境变量由启动终端管理，退出认证不会清除环境变量或撤销服务端密钥。本机 Key 被移除后，预设供应商恢复其标准环境变量入口，自定义供应商使用 `<ID转大写并将短横线换为下划线>_API_KEY`；已经使用环境变量/无认证的配置保持其认证方式。

Key 隐藏显示，不进入命令消息、聊天记录或会话草稿。它保存在同一配置根目录的 `.lumen/provider-credentials.json`，YAML 只记录凭据引用；轮换或改用其他认证方式会清理不再引用的旧 Key。它是本机明文文件，**不是加密存储**，Unix 权限为 `0600`。整个 `.lumen/` 不进入知识仓库同步。配置仍以当前工作区/独立会话目录为范围。

目前提供 API Key、环境变量和无认证三种方式；OAuth、订阅账号登录、云平台 IAM 和 Google 原生协议尚未实现，目录不展示这些登录选项。没有内置完整的模型规格数据库，模型来源为手工配置与远端发现缓存。

## 行模式和手工配置

行模式也能添加供应商、设置环境变量认证、查询模型和手工添加规格：

```text
/provider add deepseek
/provider auth deepseek env DEEPSEEK_API_KEY
/provider models deepseek
/provider model deepseek <模型ID> ds 128000 4096
/model ds
```

先在启动 Lumen 的终端设置相应环境变量。自定义地址使用 `/provider add <名称> <Base URL>`，无认证使用 `/provider auth <名称> none`。手工模型命令参数依次为供应商、模型 ID、本机别名、上下文容量、输出上限；末尾可加 `--no-tools` 声明不支持工具，但现有 TUI Agent 工作流需要支持工具的模型。规格需按所选模型实际能力填写。

行模式不接受把实际 Key 放进斜杠命令；需要直接录入 Key 时使用全屏表单。接口未提供模型列表（例如返回 404/405）时，可以按供应商文档用 `/provider model` 手工添加模型。添加供应商后、尚未配置任何模型前，本地功能和演示模式仍可使用。

已有 YAML 配置与环境变量入口继续兼容，例如：

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
- `/login [名称]` 配置供应商认证，`/logout [名称]` 移除本机 Key。
- `/provider add` 打开目录；`/provider key <名称>` 修改 Key。
- `/model` 在列表中切换模型或选择供应商刷新模型。
- `/provider models <名称>` 或 `/models <名称>` 查询接口的可用模型。
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

预设端点参考：[Groq](https://console.groq.com/docs/overview)、[OpenRouter](https://openrouter.ai/developers)、[Mistral](https://docs.mistral.ai/api/endpoint/models)、[xAI](https://x.ai/)、[Ollama](https://github.com/ollama/ollama/blob/main/docs/api/openai-compatibility.mdx)。

模型发现协议参考：[Anthropic Models API](https://platform.claude.com/docs/en/api/models/list)、[DeepSeek 模型列表](https://api-docs.deepseek.com/api/list-models/)、[DeepSeek 接入地址](https://api-docs.deepseek.com/)、[OpenAI Models API](https://developers.openai.com/api/reference/resources/models/methods/list)。模型 ID 和可选容量字段以服务实际响应为准。
