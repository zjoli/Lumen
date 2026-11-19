# Bash 工具

应用在原有工具之外注册 `bash`。`web_search`、`web_fetch`、`web_import` 的注册条件、接口和实现保持原样。Bash 不添加聊天斜杠命令。

## 调用

```json
{
  "command": "cat /workspace/notes.txt | wc -l; curl -fsSL --max-time 20 https://example.com",
  "inputs": ["notes.txt"],
  "cwd": "/work",
  "timeout_ms": 30000
}
```

- `command` 必填，最长 65536 字符；支持管道、重定向和多行脚本。
- `inputs` 可选，默认没有宿主机文件。最多选择 32 个相对路径，来自当前工作区；无工作区时来自应用启动目录。目录会递归复制，重叠选择去重。快照最多 16 MiB、1024 个文件/目录、32 层递归。
- `cwd` 默认 `/workspace`；相对路径以 `/workspace` 为基准，只接受 `/workspace`、`/work`、`/tmp` 及其子目录。目录必须存在，路径不允许 `..`。
- `timeout_ms` 默认 30000，范围 1–120000；包含准备输入的时间。

输入的隐藏/私有路径禁止显式选择；递归时跳过此类名称。符号链接和非常规文件被拒绝，目录遍历用目录文件描述符与 `openat(O_NOFOLLOW)`，不采用“先检查再跟随链接”的读取方式。它不是内容分类器：显式选择的普通文件可能包含敏感内容，不应将凭据放入任务输入。

每次调用都启动 `bash --noprofile --norc -c`，不提供交互输入、持久 shell 或后台任务。需要修改输入副本时先从只读 `/workspace` 复制到 `/work`。`cd`、变量和临时文件不跨调用保留。

## 隔离与网络

首版支持 Linux x86_64/aarch64，依赖系统 `/usr/bin/bwrap` 或 `/bin/bwrap`、Bash 和允许创建用户命名空间的内核配置。执行器不会从工作区或模型提供的 PATH 选择隔离程序。不支持的平台、缺失依赖、命名空间或 seccomp 建立失败，均返回工具错误；没有宿主机直接执行降级。

Bubblewrap 使用独立的用户、PID、挂载、IPC、UTS 等命名空间，禁用进一步创建用户命名空间，去掉全部 capabilities，设置 no_new_privs。UID/GID 映射为 65534。只挂载：

| 路径 | 来源与权限 |
| --- | --- |
| `/usr`、`/bin`、`/sbin`、`/lib`、`/lib64` | 存在的系统程序和依赖，只读 |
| `/etc` 中选定文件 | DNS、hosts、NSS、动态库缓存和公开 TLS 证书，只读 |
| `/workspace` | 所选输入的临时只读快照 |
| `/proc`、`/dev` | 隔离进程视图和最小设备，挂载只读 |
| `/work` | 64 MiB 临时文件系统 |
| `/tmp` | 32 MiB 临时文件系统 |
| `/dev/shm` | 8 MiB 临时文件系统 |

根文件系统只读，不挂载宿主机 home、工作区原件、宿主 `/proc`、Docker socket、SSH agent 或凭据目录。环境变量清空，只设置固定的 PATH、HOME、TMPDIR 和 LANG，不继承模型 API Key、代理凭据、BASH_ENV 或 shell 启动配置。

**网络默认开启**，共享宿主网络命名空间，无域名/IP 白名单，包含宿主可达的公网、内网和 loopback。联网命令能够发送输入内容，HTTP 请求也可能有远端副作用。现有网络研究工具的私有认证配置不会自动注入 Bash。

共享网络也会共享 Linux 抽象 Unix socket 空间，因此 payload 额外应用 seccomp：只允许创建 AF_INET/AF_INET6 socket，拒绝 Unix socket 等其他协议族，以及可绕过 socket 创建路径的 io_uring_setup；拒绝兼容架构系统调用。它仍支持普通 TCP/UDP 联网，但依赖 Unix socket 的软件可能无法运行。

## 资源与生命周期

设置不可提升的硬 rlimit：每进程地址空间 512 MiB、CPU 时间约为请求期限向上取整加 1 秒、进程数 64、打开文件数 256、单文件 16 MiB，禁用 core dump。地址空间和 CPU 是每进程限制，进程数限制受 Linux 用户计数语义影响；这些不是 cgroup 的整棵任务树总内存/CPU 配额。临时文件系统容量限制是整个挂载的总量。

超时、取消或执行 future 被丢弃都会终止 Bubblewrap；私有 PID 命名空间随之清理，包括 `setsid` 脱离进程组的子进程。正常命令结束也不保留后台进程。工具注册表为 Bash 使用独立执行期限并提供 2 秒取消收尾窗口，其余工具仍默认 30 秒。外层强制丢弃有进程清理兜底。

## 结果与归档

返回内容首先是 `Bash result: {JSON}`，包含退出码/信号、超时/取消标志、用时、cwd、网络状态、输出字节数和截断标志，随后分别给出 stdout/stderr。状态置于文本前部，窗口裁剪时保留。非零退出码、超时、取消或读取错误均为工具错误；超时/取消不撤销已经发生的网络副作用。

两条输出流并发读取，各采集最多 512 KiB；超额继续排空并丢弃，明确标记截断，避免阻塞或无界内存增长。大结果由已有 ContextWindowPlugin 归档，通过 `context_artifact_read` 回读已采集内容；超过采集上限的部分无法恢复。磁盘临时文件在调用后销毁；要保存结果，应输出所需文本并通过专用知识工具写入，不能将临时文件路径当成持久附件。

## 验证

`cargo test --test bash_tool` 需要上述 Linux 隔离依赖，实际启动沙箱验证命令、curl、管道、输入快照、只读写入、环境与权限、IP/Unix socket 边界、资源限制、输出限额、超时/取消/丢弃后的子进程清理，以及 Agent Loop 续轮与大结果归档。

隔离以操作系统、Bubblewrap 和系统程序可信为前提。此实现不承诺对内核漏洞、恶意宿主管理员或远端服务副作用提供隔离。
