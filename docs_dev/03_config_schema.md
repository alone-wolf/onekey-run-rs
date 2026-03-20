# 配置 Schema 设计

## 1. 配置目标

`onekey-tasks.yaml` 是项目最核心的外部契约。Schema 必须先于实现冻结，否则运行时行为会持续返工。

当前实现同时把这套原始配置结构作为：

- YAML 读取时的反序列化目标
- `init` / `init --full` 模板生成时的内存模型
- YAML 输出时的序列化来源

也就是说，原始配置 schema 只有一份，不单独维护第二套“模板结构”。

## 2. 顶层结构建议

```yaml
defaults:
  stop_timeout_secs: 10
  restart: "no"

log:
  file: "./logs/onekey-run.log"
  append: true
  max_file_bytes: 10485760
  overflow_strategy: "rotate"
  rotate_file_count: 5

actions:
  action_name:
    executable: "..."

services:
  service_name:
    executable: "..."
```

## 3. 顶层字段

- `defaults`
  全局默认值，服务级配置可覆盖。
- `services`
  服务定义集合，key 为服务名。
- `actions`
  短时动作定义集合，供 `service.hooks` 引用。
- `log`
  可选，实例级日志配置。用于记录基于当前配置文件启动的 `onekey-run` 实例生命周期事件。

当前已实现的最小能力：

- 顶层 `actions`
- `service.hooks`
- `before_start` hook 的同步串行执行
- `args` 中的基础占位符校验与渲染
- `init` / `init --full` 通过 preset builder 按当前运行平台生成对应模板

## 4. 服务字段建议

- `executable`
  必填，可执行文件名或可执行文件路径。
- `args`
  可选，命令参数数组。
- `cwd`
  可选，服务独立工作目录。相对路径按 `onekey-tasks.yaml` 所在目录解析，绝对路径原样使用。
- `env`
  可选，环境变量键值对。
- `log`
  可选，日志配置。当前阶段仅支持保存到文件。
- `depends_on`
  可选，依赖服务列表。
- `restart`
  可选，重启策略。
- `stop_signal`
  可选，停止时发送的信号。
- `stop_timeout_secs`
  可选，优雅停止等待时间。
- `autostart`
  可选，是否默认由 `up` 拉起。
- `disabled`
  可选，是否禁用该服务。
- `hooks`
  可选，service 生命周期 hook 配置。

## 4.0 顶层 `log`

顶层 `log` 与 `service.log` 复用同一组字段：

- `file`
- `append`
- `max_file_bytes`
- `overflow_strategy`
- `rotate_file_count`

但二者记录对象不同：

- 顶层 `log`
  记录 `onekey-run` 实例自身事件
- `service.log`
  记录对应 service 的 stdout/stderr 输出

当前建议顶层实例日志至少记录：

- 实例启动 / 停止 / 清理事件
- service 启动成功 / 启动失败 / 运行期异常退出
- hook 开始 / 完成 / 失败
- action 开始 / 完成 / 失败 / 超时

当前不建议把 hook/action 的原始 stdout/stderr 全量镜像进顶层实例日志；实例日志更适合记录状态摘要，而不是原始输出全文。

## 4.1 `actions` 字段

- `executable`
  必填，可执行文件名或可执行文件路径。
- `args`
  可选，参数数组。当前已实现 `${service_name}`、`${action_name}`、`${service_cwd}` 等占位符的校验与渲染框架。
- `cwd`
  可选，action 独立工作目录。相对路径按 `onekey-tasks.yaml` 所在目录解析。
- `env`
  可选，环境变量键值对。
- `timeout_secs`
  可选，action 超时时间，必须大于 `0`。
- `disabled`
  可选，是否禁用该 action。

## 4.2 `hooks` 子结构

当前 schema 允许以下 hook 名：

- `before_start`
- `after_start_success`
- `after_start_failure`
- `before_stop`
- `after_stop_success`
- `after_stop_timeout`
- `after_stop_failure`
- `after_runtime_exit_unexpected`

当前已真正接入运行时的 hook 只有：

- `before_start`
- `after_start_success`
- `after_start_failure`
- `before_stop`
- `after_stop_success`
- `after_stop_timeout`
- `after_stop_failure`
- `after_runtime_exit_unexpected`

## 4.3 内部事件输出

当前运行时会将 hooks / actions / service 生命周期事件写入：

- `.onekey-run/events.jsonl`

该文件当前主要作为内部事件通道，供后续本体日志、TUI 或管理面复用；普通 `up` 不会直接把这些事件刷到终端。

## 5. 校验规则建议

以下规则必须在 `check` 阶段尽量前置验证：

- `services` 不能为空
- `actions` 中被引用的 action 必须存在
- 被 hook 引用的 action 不能是 `disabled: true`
- 服务名必须唯一，且仅允许稳定字符集
- action 名必须唯一，且仅允许稳定字符集
- `executable` 不允许为空字符串
- `depends_on` 中的服务必须存在
- 依赖图不能有环
- `disabled` 服务是否允许被依赖，需要明确
- 若配置 `log.file`，其值不允许为空
- action `timeout_secs` 若出现必须大于 `0`
- action `args` 中的占位符必须是受支持集合
- 某个 hook 中引用 action 时，占位符必须对该 hook 可用

## 5.1 `log` 子结构

```yaml
log:
  file: "./logs/app.log"
  append: true
  max_file_bytes: 10485760
  overflow_strategy: "rotate"
  rotate_file_count: 5
```

- `file`
  日志输出文件路径。相对路径按 `onekey-tasks.yaml` 所在目录解析。
- `append`
  是否以追加模式写日志文件。默认值为 `true`。
- `max_file_bytes`
  单个活动日志文件的大小上限，单位为字节。当前实现要求它与 `overflow_strategy` 成对出现；未配置时表示不做容量切换。
- `overflow_strategy`
  当日志文件达到 `max_file_bytes` 后的处理策略。当前实现仅接受 `rotate` 或 `archive`。
- `rotate_file_count`
  仅在 `overflow_strategy: "rotate"` 时有效，表示要保留的历史轮转文件数量；当前实现要求该值大于 `0`。

推荐约定：

- `overflow_strategy: "rotate"`
  固定保留有限数量的旧日志文件，超过上限后淘汰最老文件。
- `overflow_strategy: "archive"`
  超限后生成新的归档文件，旧文件保留。当前阶段不复用 `rotate_file_count` 控制 `archive` 数量。

相比更短的 `mode` / `max_files` 命名，`overflow_strategy` 和 `rotate_file_count` 在配置阅读时更直接，不需要额外猜测“这个字段到底约束什么”。

当前实现的组合校验：

- 仅配置 `log.file` / `log.append` 是合法的
- 配置了 `max_file_bytes` 就必须同时配置 `overflow_strategy`
- `overflow_strategy: "rotate"` 必须同时配置 `rotate_file_count`
- `overflow_strategy: "archive"` 不允许再配置 `rotate_file_count`

当 `log` 用于顶层实例日志时，额外建议：

- `log.file` 不得与任何 `service.log.file` 解析到同一绝对路径
- 实例日志文本应记录 hook/action 状态摘要，供人工排查
- 机器可读聚合仍优先使用 `.onekey-run/events.jsonl`

## 6. 默认值策略

默认值必须文档化，不允许散落在代码各处：

- `restart` 默认值建议为 `no`
- `stop_timeout_secs` 默认值建议为 `10`
- `autostart` 默认值建议为 `true`
- `disabled` 默认值建议为 `false`
- `log.append` 默认值建议为 `true`

## 7. 兼容策略

- 新增字段时应保持向后兼容
- 废弃字段时需要给出迁移路径
- 当未来 schema 稳定后，再引入 `version` 字段管理配置升级
- 未知字段是报错还是警告，必须提前约定

## 8. 后续可扩展字段

首版不实现，但值得提前留意的字段：

- `ready`
- 更复杂的 action 模板表达式
- action 独立日志策略

## 9. 待确认问题

- 是否允许通过环境变量模板引用外部值
- 是否允许 service 名与保留字冲突
- 未知字段是直接报错还是仅告警
