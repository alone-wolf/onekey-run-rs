# 运行时契约

## 1. 目的

运行时契约定义编排器对服务生命周期的统一解释。当前首版不做健康检查，因此本文件重点不是业务 ready，而是“进程是否仍然存活”和“如何被正确停止”。

## 2. 服务生命周期建议

建议用统一状态机描述服务：

- `Pending`
- `Starting`
- `Running`
- `Stopping`
- `Stopped`
- `Failed`

## 3. 启动语义

- 依赖服务必须先于当前服务启动
- 若 service 配置了 `hooks.before_start`，则这些 actions 会在真正 `spawn` 前按数组顺序同步串行执行
- `before_start` 中任一 action 失败，会直接阻止当前 service 启动
- 当前首版不等待业务级 ready
- `spawn` 成功且进程仍存活，即可进入 `Running`
- 启动顺序由依赖拓扑排序决定
- 若使用 `up -d`，CLI 前台进程会启动一个后台监督进程，由该监督进程继续负责运行期监控与停止协作

## 4. 进程状态语义

首版只通过 PID 和子进程句柄判断进程状态：

- `spawn` 失败即启动失败
- `spawn` 成功但子进程立即退出，应视为启动失败
- 运行期间若检测到子进程退出，则服务进入 `Failed` 或 `Stopped`
- 若配置了 `after_runtime_exit_unexpected`，则会在检测到运行期异常退出后执行对应 actions
- 不尝试判断服务是否真正“可用”或“健康”

## 5. 失败策略

首版建议默认 `fail-fast`：

- 任一关键服务启动失败，整体启动流程失败
- `before_start` action 失败属于启动失败的一部分
- 任一关键服务在运行中异常退出，整体进入停止流程
- 若存在重启策略，则先在服务级尝试恢复，超过上限后再判定整体失败

## 6. 重启策略建议

首版建议收敛为少量枚举值：

- `no`
- `on-failure`
- `always`

如果支持重启，必须再明确：

- 最大重试次数
- 退避策略
- 重启是否影响依赖服务

## 7. 停止语义

- 停止顺序应为依赖逆序
- 若 service 配置了停止侧 hooks，则当前已接入：
  - `before_stop`
  - `after_stop_success`
  - `after_stop_timeout`
  - `after_stop_failure`
- 先发送约定信号，再等待优雅退出超时
- 超时后可升级为强制终止
- 需要记录哪些服务是“正常退出”，哪些是“被强制杀死”
- 对于 `up -d` 启动的实例，`down` 需要同时停止服务进程和后台监督进程

## 8. 信号处理

主进程至少需要处理：

- `SIGINT`
- `SIGTERM`

待确认是否需要处理：

- `SIGHUP`

同时要明确：

- Unix-like 和 Windows 采用何种等价停止机制
- 是否需要对子进程树而不只是主 pid 执行停止

## 9. 日志语义

- 每条服务日志应带服务名前缀
- stdout / stderr 是否合并展示，需要统一
- 内部调度日志与服务业务日志是否分流，需要明确
- 当前已存在一条内部事件输出通道：`.onekey-run/events.jsonl`
- 该事件流会记录 service / hook / action 的关键生命周期事件，供后续本体日志与 TUI 复用

## 9.1 实例日志与运行时文件

当前运行时目录为：

- `.onekey-run/state.json`
- `.onekey-run/events.jsonl`
- `.onekey-run/lock.json`

其中：

- `state.json`
  记录当前实例的运行时快照
- `events.jsonl`
  记录机器可读的生命周期事件流
- `lock.json`
  用于运行时互斥与归属判断

当前 `state.json` 至少包含：

- `instance_id`
- `project_root`
- `config_path`
- `instance_log_file`
- `tool_pid`
- `started_at_unix_secs`
- `services`

其中 `instance_log_file` 为可选字段：

- 未配置顶层 `log` 时可为空
- 配置顶层 `log` 时应为解析后的实例日志绝对路径

当前 registry 也会记录：

- `project_root`
- `config_path`
- `instance_log_file`
- `tool_pid`
- `started_at_unix_secs`
- `service_names`

这样 `management` 即使不直接读取配置文件，也能展示实例日志路径。

## 9.2 事件 detail 约定

`events.jsonl` 中的 `detail` 当前仍为字符串，但建议尽量采用稳定的结构化文本，而不是随意的人类句子。

推荐约定：

- 使用稳定的 `key=value` 片段串联
- 字符串值使用带引号的稳定编码
- 新增字段优先追加，不轻易改已有 key 名

例如实例级事件：

- `instance_started`
  - `mode="plain|tui|daemon"`
  - `config="..."`
  - `service_count="2"`
- `instance_stopping`
  - `reason="ctrl_c|runtime_failure|shutdown"`
- `instance_stopped`
  - `runtime_ok="true|false"`
  - `shutdown_ok="true|false"`
  - `cleanup_ok="true|false"`

这样做的原因：

- 便于 `management` / 调试脚本做轻量解析
- 便于未来把 detail 平滑升级到更结构化字段
- 也能让实例日志与 `events.jsonl` 保持一致的语义来源

## 9.3 实例日志路径暴露

当配置了顶层实例 `log` 时：

- owning process 会向该文件写入实例生命周期事件
- `state.json` 会记录 `instance_log_file`
- registry 会记录 `instance_log_file`
- `management` 文本与 JSON 输出都应带出该路径

这使得用户可以从 `management` 直接找到对应实例日志，而不必再次解析配置文件。

## 10. 待确认问题

- 当依赖服务重启时，已启动的上游服务是否需要联动处理
- 用户手动停止单服务是否在首版支持
- 主进程异常退出时是否需要恢复现场或清理 pid 文件
- Windows 下 `down` 如何可靠终止整个子进程树
