# 顶层 `log` 设计（实例日志）

## 1. 背景

当前项目已经支持 `service.log`：

- 配置层已有通用的 `LogConfig`
- 校验层已有 `max_file_bytes` / `overflow_strategy` / `rotate_file_count` 规则
- 运行时已有 `LogSink`，支持 `rotate` / `archive`

但现在缺少“实例级”日志：

- `onekey-run up -d` 会把后台监督进程的 `stdout` / `stderr` 置空
- service 自己的日志只能覆盖子进程输出，不能覆盖 orchestrator / supervisor 的生命周期事件
- 出现启动失败、依赖跳过、运行期异常退出、强制停止时，缺少稳定的落盘记录

因此需要为 `onekey-tasks.yaml` 增加顶层 `log`，专门记录“基于该配置文件启动出来的 onekey-run 实例”的运行日志。

## 2. 目标

本设计希望做到：

1. 在 `onekey-tasks.yaml` 顶层新增一个实例日志配置
2. 配置字段尽量与 `service.log` 完全一致
3. 文件超限策略继续支持：
   - `rotate`
   - `archive`
4. 尽量复用现有日志校验与文件切换逻辑，避免重新发明一套日志系统
5. 尤其覆盖 `up -d` 场景下 supervisor 自身的落盘可观测性

非目标：

- 不在本期引入结构化 JSON 日志
- 不把所有 service stdout/stderr 自动汇总到实例日志
- 不把 hook/action 的原始 stdout/stderr 全量镜像进实例日志
- 不在本期引入复杂日志级别过滤器或全局 logging backend
- 不在本期解决“历史 archive 数量上限”问题

## 3. 建议的配置形状

推荐在顶层直接增加 `log`：

```yaml
defaults:
  stop_timeout_secs: 10

log:
  file: "./logs/onekey-run.log"
  append: true
  max_file_bytes: 10485760
  overflow_strategy: "rotate"
  rotate_file_count: 5

services:
  app:
    executable: "cargo"
    args: ["run"]
```

理由：

- `service.log` 已经建立了用户心智，顶层继续叫 `log` 最直观
- 子结构完全复用已有字段，学习成本最低
- 文档里只需明确“顶层 `log` = 实例日志；`service.log` = 服务输出日志”

不建议改成：

- `instance_log`
- `runtime_log`
- `supervisor_log`

这些名字虽然更长，但会让配置风格不统一，也会削弱与 `service.log` 的复用价值。

## 4. 字段语义

顶层 `log` 的字段语义与 `service.log` 完全一致：

- `file`
  实例日志文件路径；相对路径按配置文件目录解析
- `append`
  启动时是否追加到当前活动日志文件；默认 `true`
- `max_file_bytes`
  活动日志文件大小上限；未配置表示不做容量切换
- `overflow_strategy`
  超限处理策略；仅支持 `rotate` 或 `archive`
- `rotate_file_count`
  仅在 `overflow_strategy: "rotate"` 时有效；表示历史轮转文件保留数量

建议继续沿用现有组合校验：

- 仅配置 `file` / `append` 合法
- `max_file_bytes` 和 `overflow_strategy` 必须成对出现
- `overflow_strategy: "rotate"` 必须配置 `rotate_file_count`
- `overflow_strategy: "archive"` 不允许配置 `rotate_file_count`

## 5. 记录内容范围

顶层实例日志记录的是 `onekey-run` 自身事件，而不是 service 原始业务输出。

这里的“自身事件”应明确包含：

- orchestrator / supervisor 生命周期事件
- hook 执行状态
- action 执行状态

也就是说，实例日志虽然不保存 action 原始输出全文，但要保存 hook/action 的开始、完成、失败、超时等状态摘要。

建议首批记录这些事件：

- 实例启动：
  - 配置路径
  - 项目根目录
  - 启动模式（plain / tui / daemon supervisor）
  - 目标服务列表
- 服务拉起流程：
  - 准备启动某个 service
  - service 启动成功及 pid
  - service 启动失败及错误摘要
- hook / action 流程：
  - hook 开始 / 完成 / 失败
  - action 开始 / 完成 / 失败 / 超时
  - 某个 hook 因 action 失败而中断后续 action
  - service 因 `before_start` hook 失败而跳过启动
- 运行期事件：
  - service 意外退出
  - 收到第一次中断，准备优雅停止
  - 收到第二次中断，进入强制停止
- 停止流程：
  - 开始停止某个 service
  - 优雅停止成功
  - 停止超时后强制终止
  - 实例退出和清理完成

明确不记录：

- service stdout/stderr 的完整镜像
- 高频 TUI 刷新内容
- `management` 仅查看信息时的输出
- `check` / `init` 这类非运行期命令的普通输出
- action 脚本的完整 stdout/stderr 全量复制

对于 action 输出，建议首版只在实例日志里记录状态摘要与必要错误摘要，例如：

- action 名
- hook 名
- exit code / timeout
- duration
- 必要时截断后的错误原因

## 6. 输出格式建议

建议实例日志先使用人类可读的单行文本，不引入 JSON。

推荐格式：

```text
[2026-03-19T13:45:12Z] [INFO] instance started mode=daemon config=/path/to/onekey-tasks.yaml services=[app,worker]
[2026-03-19T13:45:12Z] [INFO] hook started service=api hook=before_start action_count=2
[2026-03-19T13:45:12Z] [INFO] action started service=api hook=before_start action=prepare-env
[2026-03-19T13:45:13Z] [INFO] action finished service=api hook=before_start action=prepare-env exit=0 duration_ms=824
[2026-03-19T13:45:13Z] [INFO] service started name=app pid=12345
[2026-03-19T13:45:20Z] [WARN] shutdown requested source=signal
[2026-03-19T13:45:21Z] [ERROR] action failed service=worker hook=after_runtime_exit_unexpected action=notify detail=\"exit status 1\"
[2026-03-19T13:45:23Z] [ERROR] service exited unexpectedly name=worker code=1
```

建议最小字段：

- 时间戳
- 级别：`INFO` / `WARN` / `ERROR`
- 事件消息

针对 hook/action 事件，建议稳定携带：

- `service`
- `hook`
- `action`（hook 级事件可省略）
- `status` 或等价语义
- `exit` / `timeout` / `duration_ms` 等结果字段

这样既方便人工排查，也便于以后再做简单 grep。

## 7. 代码复用建议

这项功能应该尽量复用现有 `service.log` 的实现，建议分两层复用。

### 7.1 配置层复用

当前 `src/config.rs` 已有通用 `LogConfig` / `ResolvedLogConfig`，不需要再定义一套实例专用 schema。

建议改动：

- 在 `ProjectConfig` 增加：
  - `pub log: Option<LogConfig>`
- 在 `RunPlan` 增加：
  - `pub instance_log: Option<ResolvedLogConfig>`
- 在 `build_run_plan(...)` 中把顶层 `log` 一并解析成绝对路径

### 7.2 文件写入层复用

当前 `src/process.rs` 内部的 `LogSink` 已经实现了：

- 目录创建
- append / truncate
- rotate
- archive

建议把这部分下沉为共享模块，例如：

- `src/file_log.rs`
- 或 `src/logging.rs`

共享模块只负责：

- 打开文件
- 写一条已格式化好的文本行
- 处理 overflow

实例日志和未来 `events.jsonl` 最好共享同一批内部 lifecycle event，只是在不同 sink 上做不同格式化：

- `events.jsonl`
  面向机器聚合
- 顶层 `log`
  面向人工排查

然后：

- service 输出采集继续复用它
- 实例日志新增一个 `InstanceLogger`，也复用它

这样能避免把“service stdout/stderr 前缀拼接”与“文件轮转”强耦合在一起。

## 8. 运行时 ownership 设计

这里是本功能最需要提前说清楚的点。

### 8.1 前台 `up` / `up --tui`

这两种模式由当前进程完整拥有实例生命周期，因此可以由同一个 `InstanceLogger` 写完整日志：

- 启动
- 监控
- 信号触发的停止
- 退出清理

### 8.2 后台 `up -d`

真正拥有实例生命周期的是隐藏命令 `__daemon-up` 对应的后台监督进程。

因此建议：

- 顶层实例日志由后台 supervisor 进程负责打开和写入
- 前台负责拉起 daemon 的那个短命 CLI 进程不写实例日志
- 这样可以避免两个进程同时持有同一套 rotate / archive 状态

### 8.3 `down` 的边界

当前 `down` 是另一个独立 CLI 进程，它会：

- 读取 runtime state
- 自己停止 services
- 最后再终止 tool pid

这意味着它不是当前 daemon 实例内部的一部分。

建议分阶段处理：

- Phase 1：
  `down` 不写顶层实例日志，避免多进程同时对同一个日志文件做 rotate / archive
- Phase 2：
  如果未来需要补齐“停止事件也写入实例日志”，应优先把 `down` 改成“向 daemon 发关闭请求，由 daemon 自己执行停服并记日志”

这样可以保证实例日志始终只有一个 owner。

## 9. 校验与兼容性建议

### 9.1 顶层校验

在现有 `ProjectConfig::validate(...)` 中补充：

- 顶层 `log.file` 不允许为空
- 顶层 `log` 使用与 `service.log` 相同的组合校验规则

建议把现有：

- `validate_log_config(service_name, log)`

重构成更通用的形式，例如：

- `validate_log_config(owner_label, log)`

这样错误文案既能服务：

- `service \`app\` log.rotate_file_count ...`
- `top-level log.rotate_file_count ...`

### 9.2 路径解析

建议继续沿用当前规则：

- 相对路径统一按配置文件目录解析

这样顶层 `log` 与 `service.log` 的用户心智保持一致。

### 9.3 路径冲突校验

建议新增一条校验：

- 顶层 `log.file` 不应与任何 `service.log.file` 解析到同一绝对路径

原因：

- 现有 rotate / archive 状态在单个 sink 内维护
- 若实例日志和 service 日志共用一个文件，语义会混乱
- 多个 writer 同时管理同一文件的 overflow 很容易出错

是否顺手扩展为“所有 service.log 之间也禁止同路径”，可以单独评估；但至少应先禁止“顶层实例日志”和任一 service 日志冲突。

## 10. 模块改动建议

建议影响这些模块：

- `src/config.rs`
  - 顶层 `log` schema
  - 通用 log 校验
  - 顶层 log 路径解析
- `src/orchestrator.rs`
  - 创建实例 logger
  - 在启动/监控/停止关键路径写事件
- `src/process.rs`
  - 抽离共享文件日志 sink
- `src/app.rs`
  - `up -d` 场景下保证后台 supervisor 能独立初始化实例 logger
- `src/runtime_state.rs`
  - 可选：记录 `instance_log_file`
  - 便于未来 `management` / `logs` 能直接展示实例日志路径

## 11. 实施阶段建议

### Phase 1：Schema 与底层复用抽取

- 顶层增加 `log: Option<LogConfig>`
- `RunPlan` 增加 `instance_log`
- 抽离共享 `LogSink` 到独立模块
- 补充顶层 log 解析与校验测试

验收标准：

- 旧配置不受影响
- 新配置可通过 `check`
- 顶层 `log` 能被解析成绝对路径

### Phase 2：前台实例日志闭环

- 在 `run_up_plain` / `run_up_tui` 接入 `InstanceLogger`
- 记录启动、拉起、失败、Ctrl-C、停止、清理完成等关键事件
- 记录 hook/action 的 started / finished / failed / timeout 状态摘要

验收标准：

- `up` / `up --tui` 在配置顶层 `log` 后能稳定落盘实例事件
- `up` / `up --tui` 能在实例日志中看到 hook/action 状态变化
- `rotate` / `archive` 行为与 service.log 一致

### Phase 3：后台 supervisor 场景

- 在 `run_up_daemonized` 接入 `InstanceLogger`
- 记录 daemon supervisor 的启动与监控事件
- 记录后台 hook/action 生命周期事件
- 确保前台短命 `up -d` 进程不重复写入同一日志

验收标准：

- `up -d` 后即使终端关闭，实例日志仍持续写入
- service 异常退出等事件能落盘
- hook/action 的失败或超时状态能落盘

### Phase 4：可观测性增强

- 可选在 `runtime_state` / `registry` 保存实例日志路径
- 可选让 `management --json` 暴露实例日志路径
- 可选新增未来 `logs --instance` 的演进空间

## 12. 测试建议

至少补这些测试：

### 12.1 配置解析测试

- 顶层 `log` 正常解析
- 顶层 `log.file` 相对路径按配置目录解析
- 顶层 `log` 组合校验与 service.log 保持一致

### 12.2 共享 sink 测试

- 实例日志 `rotate` 行为正确
- 实例日志 `archive` 行为正确
- `append: false` 时会重建活动文件

### 12.3 集成测试

- `up` 有顶层 `log` 时会写入启动/停止事件
- `up -d` 有顶层 `log` 时会写入后台实例事件
- service 启动失败时实例日志能留下错误摘要
- hook/action 执行时实例日志能留下 started / finished / failed / timeout 摘要

### 12.4 冲突校验测试

- 顶层 `log.file` 与某个 `service.log.file` 相同应报错

## 13. 文档同步项

实现时建议同步更新：

- `docs_dev/03_config_schema.md`
  - 顶层字段增加 `log`
- `docs_dev/10_logging_design.md`
  - 明确“顶层实例日志”和“service 日志”的关系
- `docs_dev/02_cli_contract.md`
  - 在 `up` / `up -d` 行为说明里补充实例日志语义
- `skills/onekey-run-config-authoring/SKILL.md`
  - 顶层支持 `log`
  - 说明它记录的是实例事件，不是 service 输出

## 14. 建议结论

这项需求适合按“顶层 schema 复用 + 文件 sink 复用 + 实例事件单独格式化”的方式推进。

推荐最终方案是：

1. 顶层新增 `log`
2. 完全复用 `service.log` 的字段命名与 overflow 规则
3. 抽离通用文件日志 sink，供 service 日志和实例日志共用
4. MVP 先让实例日志只由 owning process 写入
5. `down` 的统一落盘问题留到后续通过“请求 daemon 自行停止”再补齐

这样改动范围可控，复用率高，也能先把 `up -d` 最缺失的可观测性补上。
