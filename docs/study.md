# 学习目标、路线与导师

启动 `cargo run --bin lumen-tui` 后，先通过 `/workspace init <绝对路径>` 创建独立知识工作区，或 `/workspace open <绝对路径>` 打开已有工作区。路径应位于源码仓库之外。

## 离线创建与调整

标题含空格时使用引号。下面的命令不需要模型或网络：

```text
/goal create "学习数据库事务" "能解释隔离级别并分析并发异常"
/goal set "学习数据库事务" baseline "了解基本 SQL"
/goal set "学习数据库事务" weekly_minutes 180
/goal
/goal show "学习数据库事务"
/plan add "学习数据库事务" 30 "事务基础" "解释 ACID，并用例子说明回滚"
/plan add "学习数据库事务" 45 "隔离级别" "解释脏读、不可重复读和幻读，并分析一个并发例子"
/plan show "学习数据库事务"
/progress "学习数据库事务"
```

`/goal set` 支持 `title`、`description`、`baseline`、`weekly_minutes` 和 `status`。每周时间为 1–10080 分钟；状态为 `active`、`paused`、`completed`、`archived`，用于标记目标，不自动改写单元的学习证据。列表包含全部状态。

路线显示单元 ID、预计时长、完成情况及下一步。以下命令中的 `unit_<uuid>` 应替换为列表中的实际 ID：

```text
/plan move "学习数据库事务" unit_<uuid> 1
/plan remove "学习数据库事务" unit_<uuid>
/plan done unit_<uuid> "已独立解释并完成练习"
/plan reopen unit_<uuid> "下周再复习一次"
/review "学习数据库事务" "隔离级别仍不熟，需要更多例子"
```

`move` 的位置从 1 开始。移除仅改变当前路线，原单元及学习记录保留。可以通过 `/open <unit-id>` 继续阅读历史单元。

## 生成路线与导师对话

按 [供应商配置](providers.md) 配置并选择真实模型，然后使用：

```text
/model research
/plan generate "学习数据库事务"
/study unit_<uuid>
```

生成器会结合目标、基础、每周时间、当前路线、单元的近期证据和最近三次复盘安排学习单元。每个单元包括目标、活动及验收要求。再次生成用于调整路线，保留已有单元 ID 和已完成的单元；模型遗漏已完成单元时拒绝替换。若希望主动移除它，使用 `/plan remove`。

`/study` 通过普通会话开始导师指导：先确认基础，再分步解释、举例和提问，继续直接输入即可。对话使用现有会话存储、引用、取消和恢复机制。

```text
/answer unit_<uuid> "这里是我对练习或验收问题的完整回答"
/progress "学习数据库事务"
/review "学习数据库事务" "例子能解释了，但还需要练习边界情况"
/plan generate "学习数据库事务"
```

`/answer` 按单元内容评估回答，保存原答案、反馈、评分、模型与单元版本。当前阈值为 80 分：达到阈值记为完成，低于阈值记为需要练习；这是模型评估记录，可以用 `/plan reopen` 安排复习。模型没有观察到的掌握情况不应被当成事实。用户也可用 `/plan done` 显式记录完成。

生成路线和答案评估是独立的一次模型操作，结果保存为知识对象或学习记录；`/study` 的导师对话保存在会话中。无效 JSON、拒绝、输出截断、请求中断或目标版本冲突不会提交生成结果。

## 存储与 Agent 工具

目标使用 Topic 对象的 `extensions.lumen_goal`，单元使用 Unit，时间存于 `extensions.minutes`。路线顺序保存在目标中，相邻单元通过 `requires` 关联；整条路线通过已有事务和 revision 校验一起写入。旧知识对象格式保持兼容，目标中的单元引用也参与工作区校验。

完成和评估使用已有的 Markdown 学习事件，进度从未被纠正记录取代的证据推导；生成一条路线本身不增加掌握证据。目标、单元和学习事件可用现有 Git 提交与同步功能保存。

Agent 可使用 `goal_list`、`goal_create`、`goal_read`、`goal_update`、`plan_apply` 和 `study_progress`，并复用现有知识、关系及 `learning_record` 工具。更新前需要读取目标 revision；`plan_apply` 保留已有单元时，还需提供该单元的 `expected_revision`，可从 `goal_read` 的单元进度中获取。生成器会自动绑定请求前读取的版本。
