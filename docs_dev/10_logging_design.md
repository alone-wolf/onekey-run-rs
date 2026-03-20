# 日志设计

## 1. 目的

本文件用于冻结日志文件写入、容量上限和超限处理策略的配置命名与行为语义。

当前项目已经支持：

- `log.file`
- `log.append`
- 单文件容量上限
- 超限后的 `rotate` / `archive` 策略

并且这套 `log` 子结构应同时适用于两类对象：

- 顶层实例日志
- `service.log`

## 2. 当前配置命名

当前实现采用以下字段：

```yaml
log:
  file: "./logs/app.log"
  append: true
  max_file_bytes: 10485760
  overflow_strategy: "rotate"
  rotate_file_count: 5
```

同一组字段可用于：

```yaml
log:
  file: "./logs/onekey-run.log"
  append: true
  max_file_bytes: 10485760
  overflow_strategy: "rotate"
  rotate_file_count: 5
```

其中：

- 顶层 `log`
  表示 `onekey-run` 实例日志
- `services.<name>.log`
  表示对应 service 的输出日志

## 3. 为什么不用更短的 key

不建议使用这类过短命名：

- `mode`
- `strategy`
- `max_files`
- `limit`

原因是放在 `log` 下仍然有歧义。例如：

- `mode` 不明确是“文件写入模式”还是“超限处理模式”
- `max_files` 不明确是“总文件数”还是“历史文件数”
- `limit` 不明确是“大小上限”还是“文件数量上限”

因此建议使用更直接的名字：

- `max_file_bytes`
- `overflow_strategy`
- `rotate_file_count`

## 4. 字段语义

### `file`

- 主日志文件路径
- 相对路径按 `onekey-tasks.yaml` 所在目录解析

### `append`

- 是否在启动时追加写入当前活动文件
- 默认值建议为 `true`
- 若为 `false`，启动时会清空活动文件重新写入

### `max_file_bytes`

- 单个活动日志文件的大小上限
- 单位为字节
- 若未配置，则表示不启用容量切换
- 当前实现要求它与 `overflow_strategy` 一起出现

### `overflow_strategy`

- 当活动日志文件达到 `max_file_bytes` 后的处理方式
- 当前实现只接受两个值：
  - `rotate`
  - `archive`

### `rotate_file_count`

- 仅在 `overflow_strategy: "rotate"` 时有效
- 表示历史轮转文件保留数量
- 当前实现要求该值大于 `0`
- 该值不包含当前活动文件本身
- 例如配置为 `5` 时：
  - 当前活动文件有 1 个
  - 额外历史文件最多保留 5 个

### 组合校验

- 仅配置 `file` / `append` 是合法的
- 配置 `max_file_bytes` 时必须同时配置 `overflow_strategy`
- `overflow_strategy: "rotate"` 时必须配置 `rotate_file_count`
- `overflow_strategy: "archive"` 时不得配置 `rotate_file_count`

## 5. 顶层实例日志与 `service.log` 的区别

二者共享字段命名和 overflow 规则，但记录内容不同。

### 顶层 `log`

用于记录 `onekey-run` 实例自身事件，建议至少覆盖：

- 实例启动 / 停止 / 清理完成
- service 启动成功 / 失败 / 意外退出
- hook started / finished / failed
- action started / finished / failed / timeout

顶层实例日志的目标是“审计实例生命周期”，不是完整复制业务输出。

因此当前建议：

- 不把 service stdout/stderr 全量汇总进实例日志
- 不把 action 原始 stdout/stderr 全量镜像进实例日志
- 只保留 hook/action 的状态摘要和必要错误摘要

### `service.log`

用于记录单个 service 的 stdout/stderr 输出，便于排查业务进程自身问题。

### 路径冲突建议

建议禁止：

- 顶层 `log.file`
  与任一 `service.log.file` 指向同一绝对路径

否则会出现：

- 日志语义混杂
- 多个 writer 并发处理 overflow
- rotate / archive 行为相互干扰

## 6. `rotate` 语义

示例：

```yaml
log:
  file: "./logs/app.log"
  max_file_bytes: 10485760
  overflow_strategy: "rotate"
  rotate_file_count: 3
```

行为：

- 当前活动文件始终是 `app.log`
- 达到上限后：
  - `app.log` 变为 `app.log.1`
  - `app.log.1` 变为 `app.log.2`
  - `app.log.2` 变为 `app.log.3`
  - 超出 `rotate_file_count` 的最老文件被删除
- 新的 `app.log` 继续写入

特点：

- 历史文件数量固定
- 磁盘占用可控
- 最老历史会被淘汰

## 7. `archive` 语义

示例：

```yaml
log:
  file: "./logs/app.log"
  max_file_bytes: 10485760
  overflow_strategy: "archive"
```

行为：

- 当前活动文件初始为 `app.log`
- 达到上限后，当前内容转为归档文件，当前实现命名类似：
  - `app.log.1742011200123.001`
  - `app.log.1742011200456.002`
- 新的 `app.log` 继续写入
- 当前阶段不使用 `rotate_file_count` 控制 `archive` 文件保留数量
- 若未来需要限制归档数量，应新增独立字段，而不是复用 `rotate_file_count`

特点：

- 更适合保留完整历史
- 语义上强调“归档保留”，而不是固定窗口轮转
- 若不设置保留上限，磁盘占用会持续增长

## 8. 顶层实例日志建议格式

建议实例日志使用人类可读的单行文本：

```text
[2026-03-20T09:11:01Z] [INFO] instance started mode=daemon config=/path/to/onekey-tasks.yaml services=[api,worker]
[2026-03-20T09:11:01Z] [INFO] hook started service=api hook=before_start action_count=1
[2026-03-20T09:11:02Z] [INFO] action finished service=api hook=before_start action=prepare-env exit=0 duration_ms=812
[2026-03-20T09:11:03Z] [ERROR] action failed service=worker hook=after_runtime_exit_unexpected action=notify detail="exit status 1"
```

建议稳定包含：

- 时间戳
- 级别
- service 名
- hook 名
- action 名
- exit / timeout / duration 等结果字段

机器可读聚合仍建议优先依赖 `.onekey-run/events.jsonl`，而不是反向解析文本实例日志。

## 9. 命名规则建议

### `rotate`

- 活动文件：`app.log`
- 历史文件：`app.log.1`、`app.log.2`、`app.log.3`

### `archive`

- 活动文件：`app.log`
- 归档文件：`app.log.<unix_millis>.<index>`

推荐原因：

- 重启后不容易和旧归档冲突
- 可直接按文件名近似排序出时间顺序

## 10. 边界行为建议

- 写入前检查是否超出 `max_file_bytes`
- 如果单条日志本身就大于上限：
  - 不拆分
  - 直接写入新活动文件
  - 允许该文件临时超过上限

## 11. 当前结论

当前实现直接采用以下 key：

- `log.file`
- `log.append`
- `log.max_file_bytes`
- `log.overflow_strategy`
- `log.rotate_file_count`

不要再引入 `mode`、`policy`、`max_files` 这类更短但更模糊的名字，也不要让 `rotate` 和 `archive` 共享同一个数量控制字段。

同时建议明确：

- 顶层 `log` 用于实例日志
- `service.log` 用于服务输出日志
- hook/action 的执行状态应进入实例日志
- hook/action 的原始 stdout/stderr 不要求全量进入实例日志

## 12. 可直接落地的任务拆解

下面的任务拆解按“先 schema 与复用，再前台闭环，再后台闭环，最后补文档和测试”的顺序排列，目的是让实现时可以直接按步骤开工。

### 12.1 Task A：顶层 `log` schema 接入

目标：

- 让 `onekey-tasks.yaml` 可以合法解析顶层 `log`
- 让顶层 `log` 与 `service.log` 复用同一套字段和校验规则

涉及文件：

- `src/config.rs`
- `src/orchestrator.rs`

实现任务：

1. 在 `src/config.rs` 的 `ProjectConfig` 增加：
   - `pub log: Option<LogConfig>`
2. 在 `src/orchestrator.rs` 的 `RunPlan` 增加：
   - `pub instance_log: Option<ResolvedLogConfig>`
3. 在 `build_run_plan(...)` 中解析顶层 `log`：
   - 相对路径按配置文件目录解析
   - 绝对路径保持原样
4. 将现有 `validate_log_config(...)` 重构为可同时服务：
   - 顶层 `log`
   - `service.log`
5. 保持以下组合校验一致：
   - `max_file_bytes` 与 `overflow_strategy` 成对出现
   - `rotate` 必须带 `rotate_file_count`
   - `archive` 不允许 `rotate_file_count`

验收标准：

- 旧配置不写顶层 `log` 时保持兼容
- 新配置写顶层 `log` 时能通过解析与 `check`
- 顶层 `log` 的解析结果是配置目录下的绝对路径

### 12.2 Task B：日志路径冲突校验

目标：

- 防止顶层实例日志与某个 `service.log` 复用同一文件

涉及文件：

- `src/config.rs`

实现任务：

1. 在顶层 `log` 和所有 `service.log` 都完成路径解析之后，增加冲突检查
2. 至少检查：
   - 顶层 `log.file`
     与任一 `service.log.file`
     是否解析到同一绝对路径
3. 报错文案中明确指出：
   - 顶层 `log.file`
   - 冲突的 service 名
   - 冲突路径

建议报错：

```text
top-level log.file conflicts with service `api` log.file: /abs/path/logs/onekey-run.log
```

验收标准：

- 顶层实例日志与 service 日志同路径时，`check` 直接失败
- 不同 service 仅正常使用各自日志路径时，不受影响

### 12.3 Task C：抽离共享文件日志 sink

目标：

- 把当前 `service.log` 已有的文件写入、rotate、archive 能力抽成公共模块
- 为实例日志和 service 日志共用同一套底层能力

涉及文件：

- `src/process.rs`
- 新增 `src/file_log.rs` 或 `src/logging.rs`

实现任务：

1. 从 `src/process.rs` 抽出与文件日志直接相关的能力：
   - 打开 writer
   - 维护当前文件大小
   - `rotate`
   - `archive`
   - 写一行文本
2. 保持现有 `service.log` 行为不变：
   - `append`
   - `rotate`
   - `archive`
3. 对外暴露一个通用接口，例如：
   - `FileLogSink::open(config)`
   - `FileLogSink::write_line(line)`
4. 保证共享 sink 不感知：
   - service stdout/stderr 前缀语义
   - hook/action 事件语义
   - JSONL 事件格式

验收标准：

- 原有 `service.log` 相关测试继续通过
- 抽离后 service 输出写文件行为无回归
- 新模块可被实例日志直接复用

### 12.4 Task D：定义实例生命周期事件模型

目标：

- 为 `.onekey-run/events.jsonl` 和顶层实例日志建立同源事件模型

涉及文件：

- 新增 `src/events.rs` 或 `src/logging.rs`
- `src/orchestrator.rs`
- `src/runtime_state.rs`

实现任务：

1. 定义统一事件结构，至少包含：
   - 时间戳
   - level
   - event_type
   - service_name（可选）
   - hook_name（可选）
   - action_name（可选）
   - detail
2. 首批事件类型至少覆盖：
   - `instance_started`
   - `instance_stopping`
   - `instance_stopped`
   - `service_spawn_started`
   - `service_started`
   - `service_start_failed`
   - `service_runtime_exit_unexpected`
   - `hook_started`
   - `hook_finished`
   - `hook_failed`
   - `action_started`
   - `action_finished`
   - `action_failed`
   - `action_timed_out`
3. 为同一个事件模型提供两种输出：
   - JSONL formatter，用于 `.onekey-run/events.jsonl`
   - 文本 formatter，用于顶层实例日志
4. 约定顶层实例日志只记录状态摘要，不记录 action 原始输出全文

验收标准：

- 同一个 lifecycle event 可以同时写成 JSONL 和文本日志
- hook/action 事件字段完整，足以支撑 `management` / TUI / 人工排查

### 12.5 Task E：前台 `up` / `up --tui` 接入实例日志

目标：

- 先完成前台模式的实例日志闭环

涉及文件：

- `src/orchestrator.rs`
- 可能新增 `src/instance_logger.rs`

实现任务：

1. 在 `run_up_plain(...)` 初始化实例 logger
2. 在 `run_up_tui(...)` 初始化实例 logger
3. 在以下节点发出并写入事件：
   - 实例开始启动
   - 即将启动某个 service
   - service 启动成功
   - service 启动失败
   - 收到第一次 Ctrl-C
   - 收到第二次 Ctrl-C
   - 开始停止 service
   - service 停止成功
   - service 停止超时
   - 实例清理完成
4. 若 hook/action 已接入运行时，同步记录：
   - hook started / finished / failed
   - action started / finished / failed / timed_out

验收标准：

- `onekey-run up` 配置顶层 `log` 后能持续写实例日志
- `onekey-run up --tui` 期间产生的实例事件同样能落盘
- `rotate` / `archive` 行为与 `service.log` 一致

### 12.6 Task F：后台 `up -d` 接入实例日志

目标：

- 让 daemon supervisor 成为顶层实例日志的唯一 owner

涉及文件：

- `src/app.rs`
- `src/orchestrator.rs`

实现任务：

1. 明确前台 `up -d` 启动器不写顶层实例日志
2. 在 `run_up_daemonized(...)` 中初始化实例 logger
3. 在后台 supervisor 里记录：
   - daemon 实例启动
   - service 拉起成功 / 失败
   - hook/action 生命周期状态
   - 运行期异常退出
   - 停止与清理
4. 确保只有后台进程持有实例日志 sink，避免双写

验收标准：

- `onekey-run up -d` 返回后，后台实例日志仍继续增长
- 终端关闭后，service 异常退出、hook/action 失败等事件仍能落盘

### 12.7 Task G：`down` 边界先冻结

目标：

- 避免在本轮就引入多进程并发写同一个实例日志的问题

涉及文件：

- `src/orchestrator.rs`
- `docs_dev/02_cli_contract.md`

实现任务：

1. 保持当前 `down` 不直接写顶层实例日志
2. 在文档中明确：
   - 当前实例日志 owner 是 `up` / `up --tui` / `__daemon-up`
   - `down` 的统一落盘留待后续通过“请求 daemon 自行停止”再解决

验收标准：

- 本轮没有两个独立进程同时对同一实例日志文件做 rotate / archive

### 12.8 Task H：hook/action 状态接入实例日志

目标：

- 把前面 actions/hooks 设计中的 lifecycle 状态正式接进实例日志

涉及文件：

- `src/orchestrator.rs`
- `src/process.rs` 或未来 `src/actions.rs`
- `src/tui.rs`

实现任务：

1. 在 hook 开始时发出：
   - `hook_started`
2. 在 hook 全部 action 成功完成时发出：
   - `hook_finished`
3. 在某个 hook 因 action 失败而中断时发出：
   - `hook_failed`
4. 在 action 生命周期中发出：
   - `action_started`
   - `action_finished`
   - `action_failed`
   - `action_timed_out`
5. 顶层实例日志文本至少携带：
   - service
   - hook
   - action
   - exit code / timeout / duration
6. `.onekey-run/events.jsonl` 与实例日志使用同源事件，不重复造格式

验收标准：

- hook/action 状态在实例日志与 `events.jsonl` 中都可见
- `management` 后续可以直接复用 `events.jsonl` 聚合最近状态
- TUI 后续可以直接复用同源事件流

### 12.9 Task I：测试任务拆解

目标：

- 把本轮日志设计变成可以稳定回归验证的测试集

涉及文件：

- `src/config.rs` 对应测试
- `src/process.rs` 或共享 sink 模块测试
- `src/orchestrator.rs` 集成测试

实现任务：

1. 配置解析测试：
   - 顶层 `log` 正常解析
   - 顶层 `log` 相对路径按配置目录解析
   - 顶层 `log` 与 `service.log` 组合校验一致
2. 冲突校验测试：
   - 顶层 `log.file` 与某个 `service.log.file` 相同
3. sink 行为测试：
   - `rotate`
   - `archive`
   - `append: false`
4. orchestrator 集成测试：
   - `up` 时实例启动事件落盘
   - `up -d` 时后台事件落盘
   - service 启动失败事件落盘
   - hook/action 的 started / finished / failed / timeout 事件落盘

验收标准：

- 每一类行为至少有一条自动化测试覆盖
- 顶层实例日志相关变更不会破坏既有 `service.log` 行为

### 12.10 Task J：建议提交拆分

为了降低回归风险，建议不要把所有改动揉成一个大提交，推荐按下面顺序拆：

1. 顶层 `log` schema + 校验 + 路径冲突检查
2. 共享文件日志 sink 抽离
3. 前台 `up` / `up --tui` 实例日志
4. 后台 `up -d` 实例日志
5. hook/action 事件接入实例日志与 `events.jsonl`
6. 文档、模板、帮助文案与测试补齐

这样每一步都能独立评审，也更容易定位回归点。

## 13. 推荐实施顺序

为了尽快形成一个可运行、可验证、可继续迭代的闭环，建议按下面顺序实施：

### Phase 1：先做配置与校验闭环

先做：

- Task A：顶层 `log` schema 接入
- Task B：日志路径冲突校验

原因：

- 这是最小风险改动
- 能先把外部契约冻结到代码里
- 后续所有运行时实现都依赖这一步的配置解析结果

本阶段完成标志：

- `onekey-tasks.yaml` 能写顶层 `log`
- `check` 能拦住无效组合和路径冲突

### Phase 2：抽共享 sink，避免实例日志重复造轮子

接着做：

- Task C：抽离共享文件日志 sink

原因：

- 实例日志和 `service.log` 都依赖同一套 rotate / archive 行为
- 若不先抽底层，后面接实例日志时容易复制代码、埋下行为漂移

本阶段完成标志：

- `service.log` 行为保持不变
- 实例日志已经有可直接复用的文件写入能力

### Phase 3：接入实例日志最小闭环

然后做：

- Task D：定义实例生命周期事件模型
- Task E：前台 `up` / `up --tui` 接入实例日志

原因：

- 先从单进程前台模式切入，最容易验证
- 这一步就能看到顶层 `log` 是否真正落盘
- 也是后续 daemon 模式的基础

本阶段完成标志：

- `up`
- `up --tui`

在配置顶层 `log` 后，可以看到实例启动、service 启动/停止、基础 hook/action 状态写入实例日志。

### Phase 4：扩到 daemon 模式

再做：

- Task F：后台 `up -d` 接入实例日志
- Task G：冻结 `down` 边界

原因：

- daemon 模式最需要实例日志
- 同时要明确 owner，避免多个进程并发写同一个日志文件

本阶段完成标志：

- `up -d` 后后台 supervisor 能持续写实例日志
- `down` 仍不直接写实例日志

### Phase 5：补齐 hook/action 生命周期落盘

然后做：

- Task H：hook/action 状态接入实例日志

原因：

- 这一步依赖前面已经有：
  - 顶层 schema
  - 共享 sink
  - 实例事件输出通道
- 做到这里，实例日志才真正具备“排查 orchestrator 行为”的价值

本阶段完成标志：

- hook/action 的 started / finished / failed / timed_out 同时进入：
  - `.onekey-run/events.jsonl`
  - 顶层实例日志

### Phase 6：最后补测试与提交拆分

最后做：

- Task I：测试任务拆解
- Task J：建议提交拆分

原因：

- 前面的行为已经基本稳定
- 这时补测试最不容易反复返工
- 提交也能按功能边界清晰切开

最终建议的实际编码顺序就是：

1. Task A
2. Task B
3. Task C
4. Task D
5. Task E
6. Task F
7. Task G
8. Task H
9. Task I
10. Task J
