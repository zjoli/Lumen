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

构建发布版本：

```bash
cargo build --release
```

## 计划中的工作

- [x] 搭建基础终端交互与命令解析
- [ ] 创建与编辑学习目标
- [ ] 生成、查看和调整学习路线
- [ ] 接入导师对话能力
- [x] Markdown 知识对象、学习记录与会话持久化
- [x] 本地全文检索、关系导航及可注入的向量、摘要和重排能力
- [x] 内嵌 libgit2 的版本管理、多设备同步与冲突恢复
- [ ] 支持资料检索与研究主题探索
- [ ] 支持可替换的模型和搜索服务提供商
- [x] 知识目录、内容视图与 Agent 输入面板的全屏 TUI
- [ ] 完善跨平台发布与配置管理

## 设计方向

Lumen 将学习过程视为一个持续循环：

```text
目标 → 路线 → 学习 → 提问 → 练习 → 复盘 → 调整路线
```

应用层计划按领域拆分为路线规划、学习会话、研究探索和本地存储等模块；模型调用、联网检索和数据存储会保持可替换，方便接入不同服务并优先保护用户的学习记录。

## 知识工作区

### TUI 界面

```bash
cargo run --bin lumen-tui
cargo run --bin lumen-tui -- --workspace /path/to/knowledge
```

左侧为知识目录，右上在 Markdown 与会话记录之间切换，右下固定保留多行 Agent 输入。布局参考 [Reddix](https://github.com/ck-zhang/reddix) 的面板分区，默认使用终端的前景、背景和 ANSI 调色板，适配 VS Code 的浅色或深色集成终端。

所有文字沿用终端字号；标题通过粗体和颜色区分。输入区默认获得焦点，使用可见的原生插入光标，支持中文、组合字符、左右移动、换行、粘贴和长草稿滚动。VS Code 可按习惯将 `terminal.integrated.fontSize` 设置为 12–14，例如 13；程序不改动字体设置。

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

模型对话使用 `LUMEN_MODEL_ENDPOINT`、`LUMEN_MODEL_KEY`、`LUMEN_MODEL`；未配置时保留本地命令与示例响应。向量、重排、摘要通过 Rust provider trait 注入；无 provider 时检索降级为全文，概览使用确定性导航。后台索引任务随工作区关闭而取消并等待退出。

Rust API 示例见 [knowledge_workspace.rs](examples/knowledge_workspace.rs)。`/help` 列出全部命令，包括冲突解决、历史恢复和索引维护。开发构建需要 C 编译器、make、Perl 和 pkg-config；libgit2、SQLite 及 Linux 的 OpenSSL、D-Bus 库从源码构建。

## 参与开发

欢迎提交 Issue 和 Pull Request。对于较大的功能变更，建议先通过 Issue 讨论使用场景和设计方案。

## 架构设计

- [Agent Loop](docs/architecture/agent-loop.md)
- [知识存储、检索与同步](docs/architecture/knowledge-storage.md)

## License

License 尚未确定。
