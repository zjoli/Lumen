# Lumen

> 在终端中规划学习，深入知识，探索未知。

Lumen 是一款面向长期学习与研究的终端助手。它帮助你把一个模糊的目标拆解成可执行的学习路线，在学习过程中提供导师式指导，并协助探索前沿资料与研究方向。

项目使用 Rust 构建，目标是提供一个专注、快速、可扩展的 TUI（Terminal User Interface）体验。

## 主要能力

- **学习路线规划**：根据目标、基础和可用时间生成阶段化路线。
- **导师式学习**：通过提问、解释、举例和练习帮助你真正理解知识。
- **进度与复盘**：记录学习会话、笔记和掌握情况，并据此调整后续计划。
- **研究探索**：整理主题脉络、发现前沿方向，辅助形成研究问题。
- **终端优先**：在键盘驱动的环境中完成规划、阅读、对话和记录。

## 项目状态

Lumen 目前处于早期开发阶段，产品和技术方案仍在持续演进中。

## 快速开始

### 环境要求

- Rust stable（建议通过 [rustup](https://rustup.rs/) 安装）
- 支持 ANSI/Unicode 的终端

### 从源码运行

```bash
git clone <repository-url>
cd Lumen
cargo run
```

### 构建二进制文件

在项目根目录执行，构建 release 版本：

```bash
cargo build --release --bin lumen-tui --locked
```

生成的二进制文件位于 `target/release/lumen-tui`（Windows 为 `target/release/lumen-tui.exe`），可以直接运行或复制到测试目录执行。例如在 Linux/macOS 上：

```bash
mkdir -p /tmp/lumen-test
cp target/release/lumen-tui /tmp/lumen-test/
cd /tmp/lumen-test
LUMEN_SESSION_DIR="$PWD/sessions" ./lumen-tui
```

此命令将测试会话保存在测试目录的 `sessions/` 中。首次使用时，可在程序内执行 `/workspace init /tmp/lumen-test/knowledge` 创建知识工作区；已有工作区使用 `/workspace open /tmp/lumen-test/knowledge` 打开。

## 计划中的工作

- [x] 搭建基础终端交互与命令解析
- [x] 创建与编辑学习目标
- [x] 生成、查看和调整学习路线
- [x] 导师对话、答案评估、学习进度与复盘
- [x] Markdown 知识对象、学习记录与会话持久化
- [x] 本地全文检索、关系导航及可注入的向量、摘要和重排能力
- [x] 内嵌 libgit2 的版本管理、多设备同步与冲突恢复
- [x] 联网搜索、研究资料检索、网页/PDF 阅读与来源快照入库
- [x] 可替换的模型、搜索、Embedding、重排与知识摘要服务
- [x] 知识目录、内容视图与 Agent 输入面板的全屏 TUI
- [ ] 完善跨平台发布与配置管理

## 设计方向

Lumen 将学习过程视为一个持续循环：

```text
目标 → 路线 → 学习 → 提问 → 练习 → 复盘 → 调整路线
```

应用层计划按领域拆分为路线规划、学习会话、研究探索和本地存储等模块；模型调用、联网检索和数据存储会保持可替换，方便接入不同服务并优先保护用户的学习记录。

## 学习目标与路线

打开知识工作区后，可以离线创建目标和路线：

```text
/goal create "学习数据库事务" "能解释隔离级别与并发异常"
/goal set "学习数据库事务" baseline "了解基本 SQL"
/goal set "学习数据库事务" weekly_minutes 180
/plan add "学习数据库事务" 30 "事务基础" "解释 ACID 并完成练习"
/progress "学习数据库事务"
```

配置真实模型后，`/plan generate <目标>` 生成或结合近期复盘调整路线，`/study <unit-id>` 开始导师学习，`/answer <unit-id> <答案>` 保存评估反馈；`/plan done`、`/plan reopen` 和 `/review` 管理完成与复习。目标、路线、知识和学习记录共用已有 Markdown 工作区，支持持久化及同步。完整流程见 [学习使用说明](docs/study.md)。

联网搜索、研究资料检索、网页/PDF 阅读、快照入库，以及向量检索、重排和知识摘要使用本机 `.lumen/services.yaml`，可按需启用。`/web`、`/research`、`/fetch`、`/import` 提供直接命令，Agent 也能调用对应工具。配置模板见 [services.yaml](examples/services.yaml)，接入和数据发送范围见 [外部服务说明](docs/services.md)。

## 知识工作区

### TUI 界面

```bash
cargo run --bin lumen-tui
cargo run --bin lumen-tui -- --workspace /path/to/knowledge
```

左侧为知识目录，右上在 Markdown 与会话记录之间切换，右下固定保留多行 Agent 输入。面板共用细线边界，内部标题栏与正文对齐，蓝紫强调色区分导航和 Agent。默认继承终端配色，适配 VS Code 的浅色或深色集成终端；`cargo run --bin lumen-tui -- --theme light` 可查看与设计图一致的浅色配色。

所有文字沿用终端字号；标题通过粗体和颜色区分。输入区默认获得焦点，使用不闪烁的原生方块光标，支持中文、组合字符、左右移动、换行、粘贴和长草稿滚动。VS Code 可按习惯将 `terminal.integrated.fontSize` 设置为 12–14，例如 13；程序不改动字体设置。

| 操作 | 功能 |
| --- | --- |
| F6 / Shift+Tab | 顺序 / 反向切换区域 |
| F1 / F2 / F3 | 聚焦目录 / 显示阅读 / 显示对话 |
| Esc / i | 浏览内容 / 返回输入 |
| Enter / Ctrl+J | 输入时发送 / 换行 |
| ↑↓ / ←→ / Enter | 目录选择 / 折叠展开 / 打开知识 |
| PgUp / PgDn / Home / End | 翻页 / 顶部 / 底部；对话到达底部恢复追尾 |
| / | 输入时命令候选；目录或正文中查找 |
| @ / Tab | 输入时知识引用候选 / 确认候选 |
| ↑↓ / Enter | 候选列表中选择 / 确认填入命令或引用；再次 Enter 执行补全后的命令 |
| a / v | 阅读时引用当前章节 / 开始行选区；用 ↑↓ 扩展后按 a 引用 |
| Tab / Enter | 内容区中选择链接 / 打开来源 |
| Ctrl+R / Ctrl+B | 移除引用 / 显示或收起目录 |
| Ctrl+C / Ctrl+Q | 停止任务 / 退出；有草稿时再次按 Ctrl+Q 确认 |

`/open <标题或 ID>` 打开文档，`/chat` 查看会话，`/read` 返回原文。阅读时发问会保留文档，回复就绪后显示提示；后台任务不抢走阅读视图。切换保留阅读位置和输入草稿。`/help` 查看全部命令。

主题默认为 `auto`，可用 `/theme light`、`/theme dark`、`/theme auto` 切换，也可通过 `--theme` 或 `LUMEN_THEME` 指定。`LUMEN_WORKSPACE` 可设置启动工作区。窄屏自动收起目录，F1 可再次打开。

未配置模型时显示演示模式，本地知识功能仍可使用。重定向输入或输出时自动使用行模式；`--plain` 可显式启用行模式。细节见 [TUI 使用说明](docs/tui.md)。

运行 `cargo run --bin lumen-tui`，通过以下命令创建独立于源码仓库的知识工作区：

```text
/workspace init /path/to/knowledge
/workspace open /path/to/knowledge
/search 数据库事务
/commit 整理事务笔记
/sync remote https://example.com/user/knowledge.git
/sync
```

六类知识对象与学习记录保存为 Markdown，会话保存为独立 JSONL 片段；SQLite 索引位于 `.lumen/`，可以删除后重建。编辑默认只保存到工作区，提交与推送显式触发。用户无需安装 Git。

作者可通过 `LUMEN_AUTHOR_NAME`、`LUMEN_AUTHOR_EMAIL` 设置，或通过 `/workspace config author_name "Your Name"` 和 `author_email` 修改。`/workspace config credential_ref <name>` 引用系统密钥存储中服务名 `lumen` 对应的凭据；HTTPS 使用令牌，SSH 可配置 `ssh_private_key`，主机必须已在 `~/.ssh/known_hosts` 中受信任。

模型对话通过本机 `.lumen/providers.yaml` 统一配置连接、协议、模型别名和能力，支持 OpenAI Chat Completions、Responses 与 Anthropic Messages 的 SSE 流式调用。`/login` 搜索供应商并配置认证，目录包含 OpenAI、Anthropic、DeepSeek、OpenRouter、Groq、Mistral、xAI、Ollama，也支持自定义地址和协议。API Key 隐藏输入并保存到本机凭据文件；保存认证后通过 `/model` 列表选择已有模型或选择供应商查询模型，远端列表缓存支持离线读取，缺少容量时需按实际规格填写。`/models <供应商>` 刷新列表，`/logout` 移除本机 Key，`/models`、`/providers` 查看本机配置。完整示例、凭据保存方式和旧配置兼容规则见 [供应商配置](docs/providers.md)。向量、重排、摘要可通过 [外部服务配置](docs/services.md) 接入或通过 Rust trait 注入；无 provider 时检索降级为全文，概览使用确定性导航。后台索引任务随工作区关闭而取消并等待退出。

上下文由统一插件管理，检索与 Skill 按需追加，达到水位时生成结构化摘要并提交可恢复检查点。`/context` 查看窗口分区与预留，`/compact` 安排下一请求前压缩，`/skills` 查看本地 Skill。

会话管理支持 `/sessions [query]` 列表与搜索、`/new [title]` 新建、`/resume <session-id>` 切换与恢复、`/session rename <title>` 重命名、`/session archive [id]` 归档、`/session unarchive <id>` 取消归档及 `/session close` 关闭。TUI 列表支持输入过滤、↑↓ 选择、Enter 打开、PgDn 下一页；聊天顶部 PgUp 加载更早历史。切换会恢复聊天、草稿和阅读位置，运行中需要先 `/stop` 并等待收尾；列表和详情仍可查看。

工作区会话保存于 `sessions/<id>/session.yaml` 和不可变 JSONL 片段。无工作区时使用 `LUMEN_SESSION_DIR` 指定的目录，默认位于平台应用数据目录下的 `lumen/`；启动恢复最近会话。旧 `LUMEN_SESSION` 文件保留原样，目标目录默认为同路径的 `.sessions` 目录；用 `/session preview <file>` 只读浏览、`/session import <file>` 幂等导入，再 `/resume <id>` 继续。

新会话元数据保存模型选择和窗口设置；每次运行冻结实际模型配置，凭据只在运行前解析。旧 `LUMEN_MODEL_ENDPOINT`、`LUMEN_MODEL_KEY`、`LUMEN_MODEL` 和 `.lumen/model-profiles.yaml` 仍可使用，旧会话保持原来的协议映射。缺失配置或凭据时历史仍可阅读；`/model mock` 显式选择演示模型，真实请求失败不会自动降级。

`LUMEN_CONTEXT_TOKENS` 设置会话窗口上限（默认 128000），`LUMEN_OUTPUT_TOKENS` 设置输出上限（默认不超过 4096 和窗口的 1/8）；实际预算同时受模型配置约束，小窗口会缩小工具与安全预留。首版按 UTF-8 字节和协议开销估算 token，真实 API usage 单独报告。旧配置的 `LUMEN_MODEL_FORMAT=anthropic` 选择 Messages，`LUMEN_PROMPT_CACHE=false` 停止申请显式缓存；旧 endpoint/profile 保留 JSON 模式，新配置默认 SSE。

Skill 从 `~/.agents/skills/`、工作区 `.agents/skills/` 和 `LUMEN_SKILLS_DIR` 指定的额外目录发现。模型通过 `skill_search`、`skill_load`、`skill_resource`、`skill_progress`、`skill_finish` 使用工作流；活跃正文和步骤随检查点恢复。工作区 `LUMEN.md`、`AGENTS.md` 可提供持续指导，相关学习状态从已有学习证据推导。

Rust API 示例见 [knowledge_workspace.rs](examples/knowledge_workspace.rs)。`/help` 列出全部命令，包括冲突解决、历史恢复和索引维护。开发构建需要 C 编译器、make、Perl 和 pkg-config；libgit2、SQLite 及 Linux 的 OpenSSL、D-Bus 库从源码构建。

## 参与开发

欢迎提交 Issue 和 Pull Request。对于较大的功能变更，建议先通过 Issue 讨论使用场景和设计方案。

## 架构设计

- [Agent Loop](docs/architecture/agent-loop.md)
- [上下文窗口、缓存、Skill 与压缩设计](docs/architecture/context-window.md)
- [上下文管理的插件集成与实现决策](docs/architecture/context-management-implementation.md)
- [Session 管理设计](docs/architecture/session-management.md)
- [知识存储、检索与同步](docs/architecture/knowledge-storage.md)

## License

License 尚未确定。
