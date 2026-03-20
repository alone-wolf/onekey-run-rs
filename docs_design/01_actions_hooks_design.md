# `actions` + service hooks 功能设计

## 1. 目标

当前 `onekey-run-rs` 只支持编排“持续运行的 service 进程”。

这对本地开发已经够用，但仍有一类很常见的需求还没有被覆盖：某些步骤本身不是常驻服务，而是一个很快结束的动作，例如：

- 启动前准备目录
- 启动前生成配置
- 启动成功后打印通知
- 启动失败后导出诊断信息
- 停止前发送清理命令
- 停止超时后执行兜底脚本

因此需要在配置中引入一个与 `services` 平级的 `actions` section，并允许 service 在生命周期 hook 中引用这些 action。

本设计文档只做规划，不涉及当前阶段的代码实现。

## 2. 核心设计结论

建议采用两层模型：

1. 顶层新增 `actions`
   用来定义可复用的“短时动作”
2. 每个 `service` 新增 `hooks`
   用来在 service 生命周期事件上挂接 action

也就是：

```yaml
defaults:
  stop_timeout_secs: 10

actions:
  action_name:
    executable: "..."

services:
  service_name:
    executable: "..."
    hooks:
      before_start: ["action_name"]
```

## 3. 为什么要单独引入 `actions`

不建议直接把 action 混进 `services`，原因有三点：

1. 语义不同
   `service` 是长生命周期进程；`action` 是预期会很快结束的短时命令。
2. 调度方式不同
   `service` 需要被持续监控；`action` 更像一次性执行并等待退出码。
3. 配置意图更清晰
   使用者一眼就能区分“常驻服务”和“流程动作”。

## 4. 建议配置结构

### 4.1 顶层结构

```yaml
defaults:
  stop_timeout_secs: 10

actions:
  migrate-db:
    executable: "python"
    args: ["scripts/migrate.py"]
    cwd: "./backend"
    timeout_secs: 120

services:
  api:
    executable: "cargo"
    args: ["run"]
    cwd: "./backend"
    hooks:
      before_start: ["migrate-db"]
```

### 4.2 `actions` 子结构建议

建议每个 action 先支持以下字段：

- `executable`
  必填，可执行文件名或路径
- `args`
  可选，参数数组
- `cwd`
  可选，工作目录；相对路径按配置文件目录解析
- `env`
  可选，环境变量
- `timeout_secs`
  可选，action 自身最大执行时间
- `disabled`
  可选，是否禁用该 action

首版建议不支持：

- `depends_on`
- `restart`
- `stop_signal`
- `log`
- 后台执行
- 并行执行

因为 action 的目标是“简单、一次性、短时、同步”。

### 4.3 `service.hooks` 子结构建议

建议每个 service 支持：

```yaml
hooks:
  before_start: ["prepare-env"]
  after_start_success: ["notify-up"]
  after_start_failure: ["dump-start-failure"]
  before_stop: ["notify-stop"]
  after_stop_success: ["cleanup"]
  after_stop_timeout: ["dump-timeout"]
  after_stop_failure: ["final-alert"]
  after_runtime_exit_unexpected: ["dump-crash-info"]
```

值先建议使用“action 名数组”，而不是内联动作定义。

原因：

- 易复用
- 易校验
- 易文档化
- 易避免配置重复

## 5. 建议 hook 集合

用户已经提出的 hook：

- 启动之前
- 启动成功后
- 启动失败后
- 停止前
- 停止成功后
- 停止超时后

在此基础上，建议补充一个运行期异常退出 hook：

- `after_runtime_exit_unexpected`

最终建议首版 hook 集合为：

### 5.1 启动侧

- `before_start`
  在 service 真正执行 `spawn` 之前触发
- `after_start_success`
  在 service 成功启动且被判定为“已进入 Running”后触发
- `after_start_failure`
  在 service 启动失败时触发，包括：
  - `spawn` 失败
  - 启动后立即退出

### 5.2 运行侧

- `after_runtime_exit_unexpected`
  在 service 运行期意外退出时触发

### 5.3 停止侧

- `before_stop`
  在尝试停止 service 之前触发
- `after_stop_success`
  在 service 于优雅停止窗口内正常退出后触发
- `after_stop_timeout`
  在优雅停止超时、系统决定升级为强制停止后触发
- `after_stop_failure`
  在停止流程最终失败后触发

## 6. 各 hook 的精确定义

### 6.1 `before_start`

触发时机：

- 依赖服务已经按当前语义启动完成
- 当前 service 还没有执行 `spawn`

典型用途：

- 创建目录
- 生成配置文件
- 做一次数据库迁移

失败建议：

- `before_start` 下的 action 必须同步串行执行
- 任一 action 失败，则取消该 hook 后续 action
- 当前 service 不启动，直接视为该 service 启动失败
- 依赖该 service 的后续 service 因 `depends_on` 无法满足而不再启动
- 该异常需要进入日志记录链路；当前阶段先保留接口，后续接入 `onekey-run-rs` 本体日志能力

### 6.2 `after_start_success`

触发时机：

- `spawn` 成功
- service 没有立即退出
- 当前实现已经认为它进入 `Running`

典型用途：

- 发送通知
- 打印链接
- 记录 PID 或端口信息

失败建议：

- 首版建议默认视为“当前 service 启动后置动作失败”
- 是否升级为整体失败，需要单独约定，见第 9 节

### 6.3 `after_start_failure`

触发时机：

- `before_start` 通过后，真正启动 service 失败
- 或 service 启动后立即退出

典型用途：

- 导出启动诊断
- 收集日志片段
- 发送失败提醒

失败建议：

- 不再覆盖原始启动错误
- action 自己的失败只作为附加信息记录

### 6.4 `after_runtime_exit_unexpected`

触发时机：

- service 已经进入运行态
- 运行过程中意外退出

典型用途：

- 保存 crash 信息
- 写额外告警
- 导出最后日志片段

失败建议：

- 不覆盖原始 service 异常退出结论

### 6.5 `before_stop`

触发时机：

- service 仍被认为在运行
- 还没真正发送 stop signal / kill

典型用途：

- 发停机通知
- 执行提前清理动作

失败建议：

- 首版建议记录错误，但不阻止停止流程继续

### 6.6 `after_stop_success`

触发时机：

- service 在优雅停止超时前退出

典型用途：

- 清理临时文件
- 打印收尾提示

### 6.7 `after_stop_timeout`

触发时机：

- service 在优雅停止窗口内没有退出
- 系统决定进入强制停止路径

典型用途：

- dump 诊断
- 触发更强提醒
- 保存当时的状态信息

注意：

- 这个 hook 表示“优雅停止超时事件发生了”
- 不等于最终停止一定失败

### 6.8 `after_stop_failure`

触发时机：

- 停止流程最终仍失败

典型用途：

- 发送最终告警
- 记录需要人工介入

## 7. action 执行语义建议

首版建议 action 采用以下规则：

- 同步执行
- 串行执行
- 一个 hook 下的 action 按数组顺序依次执行
- 一个 action 完成后才执行下一个 action
- action 只看退出码，不做健康检查
- 阻断型 hook 在任一 action 失败后立即停止后续 action 调度
- action 可在 `args` 等字符串字段中使用 hook 提供的上下文占位符，例如 `${service_name}`、`${action_name}`、`${service_cwd}`

例如：

```yaml
hooks:
  before_start:
    - "prepare-a"
    - "prepare-b"
```

表示：

1. 先执行 `prepare-a`
2. 成功后执行 `prepare-b`
3. 全部成功后才真正启动 service

## 8. 建议提供的上下文变量

为了让 action 感知自己是在哪个 hook、哪个 service 上运行，建议执行前先展开字符串字段中的上下文占位符。

建议首版至少支持在 `args` 中使用：

- `${service_name}`
- `${action_name}`
- `${service_cwd}`

例如：

```yaml
actions:
  notify-start:
    executable: "python"
    args: ["scripts/notify.py", "${service_name}", "${action_name}", "${service_cwd}"]
```

此外，建议逐步统一一组标准上下文变量，不同 hook 按需提供：

- `ONEKEY_PROJECT_ROOT`
- `ONEKEY_CONFIG_PATH`
- `ONEKEY_SERVICE_NAME`
- `ONEKEY_HOOK_NAME`
- `ONEKEY_TOOL_PID`

对部分 hook，可额外注入：

- `ONEKEY_SERVICE_PID`
  对 `after_start_success`、`before_stop`、`after_stop_success`、`after_stop_timeout` 等可用
- `ONEKEY_STOP_REASON`
  例如 `ctrl_c`、`down`、`dependency_failure`、`runtime_failure`
- `ONEKEY_EXIT_STATUS`
  对 `after_runtime_exit_unexpected` 等可用

这样能让常见上下文值以统一方式传入 action，而不必在每个脚本里自行推断。
最终形态可以是“占位符展开 + 环境变量注入”并存，但当前规划先以占位符展开为主。

## 9. 建议失败策略

action 失败时怎么影响主流程，是这个功能最关键的设计点。

建议首版采用“按 hook 固定策略”，不要一开始就引入可配置的失败策略字段。

### 9.1 阻断型 hook

这些 hook 的 action 失败，会直接影响主流程结果：

- `before_start`

建议行为：

- 任一 action 失败，则停止后续 action，并判定当前 service 启动失败
- 依赖当前 service 的后续 service 不再启动

### 9.2 附加型 hook

这些 hook 更像“补充动作”，失败不应覆盖主错误：

- `after_start_failure`
- `after_runtime_exit_unexpected`
- `before_stop`
- `after_stop_timeout`
- `after_stop_failure`

建议行为：

- action 失败只记录，不覆盖原始 service 结果

### 9.3 待定型 hook

这两个 hook 是否应升级为主流程失败，需要明确：

- `after_start_success`
- `after_stop_success`

建议首版先采用：

- 失败只记录，主流程仍以 service 原始结果为准

原因：

- 这两个 hook 本质更接近“后置动作”
- 若把它们变成强阻断，会让用户很难理解“服务其实启动成功了，但因为后置通知失败导致整体失败”

## 10. 校验规则建议

若未来支持 `actions` 和 `service.hooks`，`check` 阶段应补充以下校验：

- `actions` 中 action 名必须合法
- action 名必须唯一
- `disabled: true` 的 action 是否允许被引用，需要提前约定
- `hooks` 中引用的 action 必须存在
- `timeout_secs` 必须大于 `0`
- 不允许未知 hook 名
- 不允许在 hook 中形成递归引用结构
  - 首版由于 action 不支持依赖与嵌套调用，这一点天然较简单

## 11. 日志与可观测性建议

action 不是常驻服务，但仍然应该有可观测性。

建议首版：

- action 输出不单独持久化为复杂日志结构
- 但需要在 orchestrator 内部输出里记录：
  - action 名
  - 绑定的 service
  - hook 名
  - 开始时间
  - 结束时间
  - 退出码
- 若项目配置了顶层 `log`，同样应把 hook/action 的状态摘要写入实例日志
  - 至少包含 started / finished / failed / timeout
  - 面向人工排查，不要求镜像 action 全量 stdout/stderr

示例：

```text
[hook] service=api hook=before_start action=migrate-db started
[hook] service=api hook=before_start action=migrate-db finished exit=0 duration=1.2s
```

如果未来要扩展，可以再给 action 增加独立日志策略。

## 12. 首版不建议支持的能力

为了控制复杂度，首版建议明确不支持：

- action 之间的依赖图
- hook 内并行 action
- action 作为后台进程
- action 重试策略
- action 的独立 `depends_on`
- 表达式级别的复杂模板语法
- 基于 service 健康状态的 hook
- 全局 hook
  - 例如“整个 up 开始前”“整个 up 成功后”

这些能力都可能有价值，但不应和第一版 `actions` 混在一起实现。

## 13. 推荐 Schema 草案

```yaml
defaults:
  stop_timeout_secs: 10

actions:
  prepare-env:
    executable: "python"
    args: ["scripts/prepare_env.py", "${service_name}"]
    cwd: "."
    env: {}
    timeout_secs: 30
    disabled: false

  notify-up:
    executable: "python"
    args: ["scripts/notify.py", "service-up"]
    cwd: "."
    timeout_secs: 10

  dump-timeout:
    executable: "sh"
    args: ["-c", "echo timeout for ${service_name}"]
    cwd: "."
    timeout_secs: 5

services:
  api:
    executable: "cargo"
    args: ["run"]
    cwd: "./backend"
    depends_on: []
    hooks:
      before_start: ["prepare-env"]
      after_start_success: ["notify-up"]
      after_stop_timeout: ["dump-timeout"]
```

## 14. 与当前实现的关系

这个设计与当前实现的关系如下：

- 保持顶层 `services` 语义不变
- 保持 `service` 的 `executable + args + cwd` 表达方式不变
- 只是在其上新增：
  - 顶层 `actions`
  - 每个 `service` 的 `hooks`

因此它是“向现有模型增量扩展”，不是推翻当前编排方式。

## 15. 分阶段实施建议

建议按 3 个阶段推进：

### Phase 1：最小可用版

支持：

- 顶层 `actions`
- `service.hooks`
- hook 名：
  - `before_start`
  - `after_start_success`
  - `after_start_failure`
  - `before_stop`
  - `after_stop_success`
  - `after_stop_timeout`
  - `after_stop_failure`
- 同步串行执行
- 固定失败策略

不支持：

- `after_runtime_exit_unexpected`
- 复杂表达式模板
- action 日志持久化

### Phase 2：运行期 hook 完善

新增：

- `after_runtime_exit_unexpected`
- 更多上下文变量
- 更清晰的错误输出

### Phase 3：可观测性与策略增强

新增：

- action 独立执行日志
- 更细的失败策略配置
- `management` 中展示最近 hook/action 结果

## 16. 当前建议结论

建议引入：

- 顶层 `actions`
- `service.hooks`

首版 hook 集合建议为：

- `before_start`
- `after_start_success`
- `after_start_failure`
- `before_stop`
- `after_stop_success`
- `after_stop_timeout`
- `after_stop_failure`
- `after_runtime_exit_unexpected`（可放到第二阶段）

首版 action 执行建议为：

- 短时
- 同步
- 串行
- 无依赖图
- 无后台化
- 无复杂策略字段

这样能最大化覆盖你提出的“把很快结束的行为编组进服务执行流程”的需求，同时不把当前实现复杂度一下子抬得过高。
