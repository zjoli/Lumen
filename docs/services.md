# 联网研究与检索增强服务

聊天模型使用 [providers.yaml](providers.md)，外部检索与检索增强使用同一知识工作区或独立会话目录下的 `.lumen/services.yaml`。未配置的能力保持关闭，本地全文检索仍可使用。

可复制 [配置模板](../examples/services.yaml)，取消所需段落的注释并填写自己的地址、模型和环境变量名。不必一次启用全部服务。知识工作区内的 `.lumen/` 不进入 Git；凭据只通过环境变量解析，不要把实际 Key 写进 YAML。

## 搜索与阅读

SearXNG 示例：

```yaml
schema_version: 1
search:
  protocol: searxng
  connection:
    endpoint: http://127.0.0.1:8080/search
    timeout_ms: 25000
reader:
  endpoint: https://r.jina.ai/
  auth:
    kind: env
    variable: LUMEN_READER_KEY
```

SearXNG 接口需要在实例中启用 JSON 输出，使用表单 POST；`/research` 使用 `science` 分类，具体引擎由该实例决定。也可替换搜索段为 Jina 兼容接口：

```yaml
search:
  protocol: jina
  connection:
    endpoint: https://s.jina.ai/search
    auth:
      kind: env
      variable: LUMEN_SEARCH_KEY
```

命令：

```text
/web 数据库事务隔离
/research serializable snapshot isolation
/fetch https://example.org/article
/import https://example.org/article "事务资料"
```

`/web` 和 `/research` 展示标题、摘要和原始 URL。`/fetch` 通过配置的 Jina Reader 兼容接口读取网页或 PDF 的 Markdown。`/import` 抓取后保存为本地 Resource，记录原始 URL、抓取时间和是否截断，可继续本地检索、引用、提交和同步。搜索不会自动保存结果。

Reader 的请求为 JSON `{"url":"..."}`，响应从 `data.title` 和 `data.content` 读取。内容上限为 100000 字符，截断会标记。读取公开无需认证的服务时可省略 `auth`，或设置 `auth: {kind: none}`。来源网页会由所配置的 Reader 服务抓取。

Agent 会按已配置能力获得 `web_search`、`web_fetch`，打开工作区后还可使用 `web_import`；结果作为参考资料处理。Jina 的研究查询会增加 `research paper`，不保证结果全部是论文，也不代表已经验证论文结论。

## 向量检索、重排与知识摘要

```yaml
embedding:
  connection:
    endpoint: https://embedding.example/v1/embeddings
    auth:
      kind: env
      variable: LUMEN_EMBEDDING_KEY
  model: actual-embedding-model
  # 只在服务和模型支持时指定 dimensions
  dimensions: 1024
  state_scope: my-account
reranker:
  connection:
    endpoint: https://rerank.example/v1/rerank
    auth:
      kind: env
      variable: LUMEN_RERANK_KEY
  model: actual-rerank-model
summary:
  model: research
```

把需要的段落合并到含 `schema_version: 1` 的同一个文件中。`summary.model` 引用 providers.yaml 的模型别名，也兼容已有旧模型配置。

Embedding 使用 OpenAI 兼容 JSON：发送 `model`、字符串数组 `input`、`encoding_format: float` 和可选维度，按响应的 `index` 恢复顺序并检查数量、维度和向量。缓存身份包含服务地址、模型、维度及 `state_scope`，修改账号或项目时修改 scope，避免混用不同向量空间。

重排使用 Jina 兼容 JSON：发送 `model`、`query`、`documents`、`top_n`，解析 `results[].index` 和 `relevance_score`。知识摘要复用配置的聊天模型，生成结果使用已有摘要缓存，不修改知识原文。

```text
/services
/services reload
/index rebuild vectors
/index status
/search --hybrid --rerank 事务隔离
/search --semantic 并发事务可见性
/summary concept_<uuid> abstract
/summary topic_<uuid> overview
```

工作区打开时自动接入这些服务，修改配置后用 `/services reload`。启用 Embedding 或摘要会启动已有后台维护机制，可能发送工作区中的知识、学习记录及会话索引内容；重排仅在请求时发送候选内容。Key 在调用时才读取，缺少 Key 不阻止本地知识读取。

默认 `/search` 使用混合检索并在无 Embedding 时降级为全文。可通过 `--lexical`、`--hybrid`、`--semantic` 指定模式，`--rerank` 开启重排。Agent 的 `knowledge_search` 也可显式选择对应模式及 `rerank: true`。仅配置重排服务不会改变每个查询的默认行为，显式开启重排才调用它。

## 边界与验证

所有 `endpoint` 都是完整接口 URL，不再自动追加路径。地址不可包含凭据、query 或 fragment；`timeout_ms` 为 100–120000，默认 25000。Agent 工具本身沿用 30 秒上限。HTTP 不自动重试或跟随重定向；单次请求上限 4 MiB，响应上限 16 MiB，取消会中止等待。配置状态不输出密钥或完整服务 URL。

协议和生命周期测试通过现有可注入 HTTP 边界运行，覆盖搜索编码、抓取入库、Embedding 索引、重排、摘要缓存、超时、取消及错误响应。真实第三方网络、账号权限和模型可用性需要在配置好凭据的环境中验证。

协议参考：[OpenAI Embeddings](https://developers.openai.com/api/reference/resources/embeddings/methods/create)、[SearXNG Search API](https://docs.searxng.org/dev/search_api.html)、[Jina API](https://docs.jina.ai/api.md)。
