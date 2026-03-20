# `management` 最近事件展示设计

## 1. 目标

当前 `management` 已能展示运行中的 onekey 实例、运行时长、服务摘要。

下一步希望它还能回答两个更具体的问题：

- 这个实例最近发生了什么
- 某个 service 最近一次 hook / action 是成功还是失败

因此需要让 `management` 读取 `.onekey-run/events.jsonl`，并汇总出“最近事件摘要”。

补充约定：

- 若项目配置了顶层实例 `log`，hook/action 的 started / finished / failed / timeout 状态也应写入该实例日志
- 但 `management` 侧仍以 `.onekey-run/events.jsonl` 作为机器可读聚合源，避免从文本实例日志反向解析

## 2. 核心设计结论

建议分两层输出：

1. 实例级最近事件摘要
2. service 级最近 hook 状态摘要

这样既能保持当前 `management` 列表简洁，也能给后续 `--json` / `--watch` 更丰富的数据。

## 3. 建议展示内容

### 3.1 实例级

每个实例可新增：

- `last_event_type`
- `last_event_at_unix_secs`
- `last_event_detail`

例如：

- `service_runtime_exit_unexpected`
- `hook_failed`
- `service_stopped`

### 3.2 service 级

每个 service 可新增最近状态摘要：

- `last_hook_name`
- `last_hook_status`
- `last_action_name`
- `last_action_status`
- `last_event_detail`

其中状态建议先收敛为：

- `running`
- `finished`
- `failed`
- `timeout`
- `unknown`

## 4. 数据来源建议

直接读取：

- `.onekey-run/events.jsonl`

不建议现在额外引入新的数据库或二级缓存。

原因：

- 当前事件量不大
- JSONL 已经存在
- `management` 是读多写少场景

## 5. 读取策略建议

### 5.1 首版

首版建议：

- 每次 `management` 执行时直接读取整个 `events.jsonl`
- 按时间顺序扫描
- 生成最近事件摘要

### 5.2 后续

若未来事件量增大，再考虑：

- 只读取文件尾部若干 KB
- 增加轻量索引文件

当前不必过早优化。

## 6. 建议内部聚合结果

建议在 `management` 侧增加一个聚合结构：

```text
InstanceEventSummary
  - last_event
  - service_summaries[]

ServiceEventSummary
  - service_name
  - last_hook_name
  - last_hook_status
  - last_action_name
  - last_action_status
  - last_detail
```

## 7. 文本输出建议

当前文本输出已经是：

```text
- pid 123 | status running | uptime 00:32 | root ... | config ... | services: api, worker
```

建议扩展为：

```text
- pid 123 | status running | uptime 00:32 | last hook_failed | root ... | config ... | services: api, worker
```

如需更多信息，可追加一行：

```text
  recent: api before_start failed | action=prepare-env | detail=exit status 1
```

首版建议保持最多 1 行附加摘要，避免输出过于冗长。

## 8. `--json` 输出建议

`management --json` 建议扩展字段：

- `last_event`
- `service_summaries`

例如：

```json
{
  "tool_pid": 12345,
  "status_summary": "running",
  "last_event": {
    "event_type": "hook_failed",
    "service_name": "api",
    "hook_name": "before_start",
    "action_name": "prepare-env",
    "detail": "action exited with status 1"
  },
  "service_summaries": [
    {
      "service_name": "api",
      "last_hook_name": "before_start",
      "last_hook_status": "failed",
      "last_action_name": "prepare-env",
      "last_action_status": "failed",
      "last_detail": "action exited with status 1"
    }
  ]
}
```

## 9. `--watch` 行为建议

`management --watch` 已支持定时刷新。

扩展最近事件后，建议：

- 每次刷新重新读取事件文件并重建摘要
- 不做增量 tail 状态机
- 先保证正确性与稳定性

## 10. 事件到状态的映射建议

建议先固定一版简单映射：

- `hook_started` -> hook status `running`
- `hook_finished` -> hook status `finished`
- `hook_failed` -> hook status `failed`
- `action_started` -> action status `running`
- `action_finished` -> action status `finished`
- `action_failed` -> action status `failed`
- `service_stop_timeout` -> hook/action status `timeout`

## 11. 风险点

- 事件文件不存在时要优雅降级
- 旧实例若没有 `events.jsonl`，`management` 不能报错退出
- 文本输出不能被最近事件信息淹没
- `--json` 结构扩展后要保持字段语义稳定

## 12. 分阶段实施建议

### Phase 1

- 读取 `events.jsonl`
- 展示实例级 `last_event`
- `--json` 增加 `last_event`

### Phase 2

- 增加 `service_summaries`
- 文本模式展示每实例 1 行 recent 摘要

### Phase 3

- `--watch` 中增加更明显的最近事件变更提示
- 若需要，再支持按 service 展开

## 13. 当前建议结论

推荐先做最小版：

- `management` 读取 `.onekey-run/events.jsonl`
- 输出每个实例最近一条事件
- `--json` 同步带上结构化 `last_event`

在此基础上再补 service 级摘要。
