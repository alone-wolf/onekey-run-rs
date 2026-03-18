# 配置 Schema 设计

## 1. 配置目标

`onekey-tasks.yaml` 是项目最核心的外部契约。Schema 必须先于实现冻结，否则运行时行为会持续返工。

## 2. 顶层结构建议

```yaml
defaults:
  stop_timeout_secs: 10
  restart: "no"

services:
  service_name:
    executable: "..."
```

## 3. 顶层字段

- `defaults`
  全局默认值，服务级配置可覆盖。
- `services`
  服务定义集合，key 为服务名。

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

## 5. 校验规则建议

以下规则必须在 `check` 阶段尽量前置验证：

- `services` 不能为空
- 服务名必须唯一，且仅允许稳定字符集
- `executable` 不允许为空字符串
- `depends_on` 中的服务必须存在
- 依赖图不能有环
- `disabled` 服务是否允许被依赖，需要明确
- 若配置 `log.file`，其值不允许为空

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
- `prestart`
- `poststop`
- `log`

## 9. 待确认问题

- 是否允许通过环境变量模板引用外部值
- 是否允许 service 名与保留字冲突
- 未知字段是直接报错还是仅告警
