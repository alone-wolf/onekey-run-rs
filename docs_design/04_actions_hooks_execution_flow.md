# `actions` / `hooks` 执行时序与状态流转设计

## 1. 目标

本文档用于回答两件事：

1. orchestrator 在 `up` / `down` / 运行期异常退出时，应该按什么顺序调度 hooks
2. action 失败后，service 状态与后续依赖服务应该如何流转

本文件只做设计，不涉及当前代码实现。

## 2. 核心结论

建议把 hook 调度视为“挂在 service 生命周期节点上的同步步骤”：

- `before_start` 是启动路径上的前置阻断步骤
- 其余 hook 默认是附加步骤，不覆盖主流程结论
- 同一个 hook 下多个 action 串行执行
- `depends_on` 只看 service 主状态，不直接看 action 状态
- 但 `before_start` 失败会直接导致 service 启动失败，因此依赖链自然被阻断

## 3. `up` 路径总时序

对单个 service，建议启动时序如下：

1. 等待其 `depends_on` 全部满足
2. 执行 `before_start` actions
3. 若 `before_start` 全部成功，执行 service `spawn`
4. 判定 service 是否成功进入 Running
5. 若成功，执行 `after_start_success`
6. 若失败，执行 `after_start_failure`

可简化为：

```text
deps ready
  -> before_start
  -> spawn
  -> running ? after_start_success : after_start_failure
```

## 4. `before_start` 详细语义

### 4.1 执行方式

- 按配置数组顺序同步串行执行
- 一个 action 完成后才执行下一个
- 不并行

### 4.2 失败语义

- 任一 action 非零退出、超时、无法执行，都视为 `before_start` 失败
- 失败后立即停止同一 hook 后续 action
- 当前 service 不进入 `spawn`
- 当前 service 直接记为启动失败
- 所有依赖当前 service 的后续 service 都因 `depends_on` 不满足而不启动

### 4.3 记录要求

建议内部事件流至少记录：

- hook 开始
- action 开始
- action 完成 / 失败
- action 超时
- 是否中断后续 action
- service 因 hook 失败而未启动

若项目配置了顶层实例 `log`，这些 hook/action 生命周期事件应同步写入实例日志，作为 onekey-run 实例级审计线索。

## 5. 启动成功路径

当 service `spawn` 成功并被当前语义判定为 Running 后：

1. service 主状态进入 `running`
2. 执行 `after_start_success`
3. 即使 `after_start_success` 失败，service 仍保持 `running`

建议原因：

- service 已经成功起来了
- 后置动作失败不应反向篡改主状态

## 6. 启动失败路径

当 service 启动失败时：

可能原因包括：

- `spawn` 失败
- 启动后立即退出
- `before_start` 失败

建议区分：

- 若是 `before_start` 失败，service 主结论仍为“启动失败”，但失败阶段可标记为 `before_start_failed`
- 若是 `spawn` 或刚启动即退出，则进入 `after_start_failure`

建议实现期把 service 启动失败细分为内部原因码，便于日志和管理命令展示。

## 7. 运行期异常退出路径

当 service 已经处于 `running`，随后意外退出：

1. service 主状态从 `running` 变为 `exited_unexpectedly`
2. 若该 hook 已实现，则执行 `after_runtime_exit_unexpected`
3. 依赖该 service 的其他 service 是否自动联动停止，当前仍建议与现有行为保持一致，单独决策

这里建议先不要把“依赖服务在运行期上游退出后如何处理”与 actions 一起耦合。

## 8. `down` 路径总时序

对单个 service，建议停止时序如下：

1. 若 service 当前仍存活，执行 `before_stop`
2. 发送优雅停止信号
3. 等待在停止窗口内退出
4. 若按时退出，执行 `after_stop_success`
5. 若超时，执行 `after_stop_timeout`
6. 若最终停止失败，执行 `after_stop_failure`

可简化为：

```text
before_stop
  -> graceful stop
  -> exited in time ? after_stop_success : after_stop_timeout
  -> still failed ? after_stop_failure : done
```

## 9. 停止路径上的 hook 策略

建议首版：

- `before_stop`
  失败只记录，不阻断停止流程
- `after_stop_success`
  失败只记录，不改写停止成功结论
- `after_stop_timeout`
  失败只记录，不改写“已超时”这一事实
- `after_stop_failure`
  失败只记录，不覆盖原始停止失败结论

原因很简单：

- 停止路径的第一目标是尽可能完成资源回收
- 不应因为附加动作失败而丢失主错误

## 10. 建议内部状态切分

为了让 `management`、日志和未来 TUI 更容易展示，建议 service 内部状态不要只有一个粗粒度字段。

建议拆成两层：

### 10.1 主状态

- `pending`
- `starting`
- `running`
- `stopping`
- `stopped`
- `failed`

### 10.2 附加原因 / 阶段

- `waiting_dependencies`
- `before_start_running`
- `before_start_failed`
- `spawn_failed`
- `start_exited_early`
- `after_start_success_failed`
- `runtime_exit_unexpected`
- `before_stop_failed`
- `stop_timeout`
- `after_stop_success_failed`
- `after_stop_failure_failed`

这样对外可以继续输出简化状态，对内则保留足够诊断信息。

## 11. 推荐事件流

建议后续实现一个统一事件流，事件类型至少包括：

- `hook_started`
- `hook_finished`
- `hook_failed`
- `action_started`
- `action_finished`
- `action_failed`
- `action_timed_out`
- `service_spawn_started`
- `service_spawn_succeeded`
- `service_spawn_failed`
- `service_running`
- `service_stop_started`
- `service_stop_timeout`
- `service_stopped`
- `service_failed`

这样能同时服务：

- 控制台输出
- 顶层实例日志
- 未来本体日志
- TUI
- `management` 状态摘要

## 12. 单 service 启动状态机草案

```text
pending
  -> waiting_dependencies
  -> before_start_running
  -> starting
  -> running

before_start_running
  -> failed(before_start_failed)

starting
  -> failed(spawn_failed)
  -> failed(start_exited_early)
  -> running
```

## 13. 单 service 停止状态机草案

```text
running
  -> stopping
  -> stopped

stopping
  -> stop_timeout
  -> stopped
  -> failed(stop_failed)
```

其中：

- `stop_timeout` 更适合作为附加阶段或事件
- 最终主状态仍应落到 `stopped` 或 `failed`

## 14. 多 service 与 `depends_on` 的关系

`depends_on` 目前仍然只约束 service 之间的主执行顺序。

建议与 action 的关系为：

- `before_start` 成功，等价于“service 允许继续进入启动流程”
- `before_start` 失败，等价于“service 启动失败”
- 因而依赖链上的下游 service 因 service 启动失败而跳过

不建议让下游 service 直接依赖某个 action 的结果对象。

## 15. 推荐日志示例

```text
[hook] service=api hook=before_start started
[hook] service=api hook=before_start action=prepare-env started
[hook] service=api hook=before_start action=prepare-env failed exit=1 duration=0.8s
[hook] service=api hook=before_start aborted remaining_actions=2
[service] service=api start_aborted reason=before_start_failed
[service] service=worker skipped reason=dependency_failed dependency=api
```

## 16. 当前建议结论

建议在实现前先把以下语义视为固定：

- `before_start` 是唯一明确阻断主启动流程的 hook
- 启动路径上的 action 按数组顺序同步执行
- `before_start` 失败会让当前 service 启动失败，并阻断依赖链下游
- 其余 hook 默认只记录错误，不改写 service 主结论
- 内部应尽早采用“主状态 + 附加原因/阶段”的表示
