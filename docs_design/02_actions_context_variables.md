# `actions` 上下文变量与占位符设计

## 1. 目标

在 `actions` + `service.hooks` 方案里，action 往往需要知道自己是为哪个 service、在哪个 hook、以什么工作目录运行。

因此需要定义一套稳定、可文档化、可校验的上下文变量规则，供 action 在 `args` 等字符串字段中引用。

本设计文档最初只做规划。

补充说明：

- 当前代码已经将同一套占位符能力用于 `service.hooks` 与 `run --action`
- `run --action` 对未显式提供的上下文值会补默认值
- 执行任何 action 前，CLI 会先打印该 action 本次实际使用到的参数值

## 2. 核心结论

当前阶段建议采用两层机制：

1. 首版主机制：字符串占位符展开
2. 后续补充机制：环境变量注入

也就是：

- 用户主要通过 `${...}` 在 `args` 中引用上下文值
- 运行时再逐步补齐同语义的环境变量，方便 shell / script 使用

## 3. 首版支持范围建议

首版建议只承诺：

- 在 `actions[*].args` 中支持占位符展开
- 占位符只支持“完整 token 内的简单变量替换”
- 不支持表达式、默认值、条件判断、函数调用

例如：

```yaml
actions:
  notify-start:
    executable: "python"
    args: ["scripts/notify.py", "${service_name}", "${action_name}", "${service_cwd}"]
```

首版不建议一开始就支持：

- `${var:-default}`
- `${var?err}`
- `${var/old/new}`
- 嵌套占位符
- 在 map key 中插值
- 在 `hooks` 的 action 名引用中插值

## 4. 建议占位符集合

建议首版最少支持以下占位符：

- `${service_name}`
  当前 hook 绑定的 service 名
- `${action_name}`
  当前正在执行的 action 名
- `${service_cwd}`
  当前 service 的工作目录；若 service 未显式配置 `cwd`，则解析后的值等于配置文件所在目录

建议同时预留以下占位符，按 hook 可用性逐步开放：

- `${project_root}`
  配置文件所在目录
- `${config_path}`
  配置文件绝对路径
- `${hook_name}`
  当前 hook 名
- `${service_executable}`
  当前 service 的 `executable`
- `${service_pid}`
  当前 service 进程 PID
- `${stop_reason}`
  停止原因，例如 `ctrl_c`、`down`、`dependency_failure`、`runtime_failure`
- `${exit_code}`
  service 或 action 的退出码
- `${exit_status}`
  更完整的退出状态字符串表示

对 `run --action` 这类 standalone action，当前实现也允许传入同名上下文 key，并为未显式提供的值补默认值。

## 5. 各 hook 的可用变量建议

不同 hook 所处的生命周期不同，因此不应暴露完全相同的变量集合。

### 5.1 所有 hook 都可用

- `${project_root}`
- `${config_path}`
- `${service_name}`
- `${action_name}`
- `${hook_name}`
- `${service_cwd}`
- `${service_executable}`

### 5.2 `before_start`

可额外使用：

- 无

说明：

- 此时 service 尚未 `spawn`
- 因此不应提供 `${service_pid}`

### 5.3 `after_start_success`

可额外使用：

- `${service_pid}`

### 5.4 `after_start_failure`

可额外使用：

- `${exit_code}`
- `${exit_status}`

说明：

- 若失败发生在 `spawn` 前或 `spawn` 时，退出码可能不可用
- 因此这类变量只应在对应 hook 中使用；若引用点与 hook 可用性不匹配，应在 `check` 阶段直接报错

### 5.5 `after_runtime_exit_unexpected`

可额外使用：

- `${service_pid}`
- `${exit_code}`
- `${exit_status}`

### 5.6 `before_stop`

可额外使用：

- `${service_pid}`
- `${stop_reason}`

### 5.7 `after_stop_success`

可额外使用：

- `${service_pid}`
- `${stop_reason}`
- `${exit_code}`
- `${exit_status}`

### 5.8 `after_stop_timeout`

可额外使用：

- `${service_pid}`
- `${stop_reason}`

说明：

- 该 hook 代表“优雅停止已超时”
- 不代表最终一定停止失败

### 5.9 `after_stop_failure`

可额外使用：

- `${service_pid}`
- `${stop_reason}`
- `${exit_code}`
- `${exit_status}`

### 5.10 `run --action` 的 standalone 上下文

当 action 不是经由 service hook 触发，而是通过：

```bash
onekey-run run --action <action_name> [--arg key=value ...]
```

直接执行时，当前实现约定：

- 占位符名仍然必须来自同一套受支持集合
- `--arg` 中显式传入的值优先
- 未显式传入的值使用 `onekey-run` 默认值

当前默认值为：

- `${project_root}` -> 配置文件所在目录
- `${config_path}` -> 配置文件绝对路径
- `${action_name}` -> 当前 action 名
- `${hook_name}` -> `manual`
- `${service_name}` -> `manual`
- `${service_cwd}` -> 配置文件所在目录
- `${service_executable}` -> 空字符串
- `${service_pid}` -> 空字符串
- `${stop_reason}` -> `manual`
- `${exit_code}` -> 空字符串
- `${exit_status}` -> `manual`

若显式传入 `service_name=<name>` 且该 service 存在，则当前实现还会推导：

- `${service_cwd}`
- `${service_executable}`

## 6. 占位符展开规则建议

首版建议采用最简单、最稳定的展开语义：

1. 只对声明为“支持占位符”的字符串字段进行展开
2. 当前阶段先只承诺 `args`
3. 每个字符串按字面扫描 `${name}` 片段并替换
4. 不做递归展开
5. 不执行 shell，不进行转义解释
6. 展开后的每个 `args[i]` 仍然是一个独立参数

例如：

```yaml
args: ["--label", "service=${service_name}", "--cwd", "${service_cwd}"]
```

若 service 为 `api`，则结果等价于：

```text
["--label", "service=api", "--cwd", "/abs/path/to/backend"]
```

当前实现还增加了一个执行前展示步骤：

- 在 action 真正启动前
- 先扫描该 action 实际引用到的占位符
- 把它们本次解析出的值打印给用户查看

## 7. 未定义变量的处理建议

这是实现时必须尽早钉死的一点。

建议首版采用：

- `check` 阶段：
  - 若引用了未知占位符名，直接报错
  - 若占位符名合法，但属于某 hook 明确不可用的变量，直接报错
- 运行阶段：
  - 理论上不应再出现未知占位符
  - 若因内部缺陷导致无法取值，应视为 action 执行前错误，并记录日志

对于 `run --action`：

- 若 `--arg` 传入未知 key，直接报错
- 若未提供某个上下文字段，则优先使用 `onekey-run` 的默认值
- 因此 standalone action 不采用“缺失即报错”的策略，而采用“默认值补齐”

例如：

- `before_start` 中使用 `${service_pid}` -> `check` 直接失败
- 写成 `${service_naem}` -> `check` 直接失败

这样比“静默替换为空字符串”更安全，也更符合工具型 CLI 的可预期性。

## 8. 与 `cwd` 解析规则的关系

需要特别强调：

- `service_cwd` 指的是“当前 service 解析后的工作目录”
- 若配置中 `cwd: "."`，其含义是“配置文件所在目录”
- 不应解释为执行 `onekey-run-rs` 时 shell 的当前目录

因此：

- `${service_cwd}` 应始终是基于配置文件目录解析后的结果
- 建议在运行时保存为绝对路径，再提供给 action

## 9. 与环境变量注入的关系

后续可以补充同语义环境变量，建议命名为：

- `ONEKEY_PROJECT_ROOT`
- `ONEKEY_CONFIG_PATH`
- `ONEKEY_SERVICE_NAME`
- `ONEKEY_ACTION_NAME`
- `ONEKEY_HOOK_NAME`
- `ONEKEY_SERVICE_CWD`
- `ONEKEY_SERVICE_EXECUTABLE`
- `ONEKEY_SERVICE_PID`
- `ONEKEY_STOP_REASON`
- `ONEKEY_EXIT_CODE`
- `ONEKEY_EXIT_STATUS`

建议关系为：

- 占位符语义与环境变量语义保持一致
- 文档以占位符为主
- shell action 或脚本 action 可同时读取环境变量

## 10. `check` 需要新增的校验

若未来开始实现该功能，`check` 应补充：

- 仅允许在支持的字段中使用占位符
- 占位符名必须属于受支持集合
- 某 hook 中引用的占位符必须对该 hook 可用
- 字符串中存在未闭合 `${...` 片段时直接报错
- 空占位符 `${}` 直接报错

## 11. 推荐示例

```yaml
actions:
  announce-before-start:
    executable: "python"
    args:
      - "scripts/announce.py"
      - "--service"
      - "${service_name}"
      - "--hook"
      - "${hook_name}"
      - "--cwd"
      - "${service_cwd}"

  dump-stop-timeout:
    executable: "python"
    args:
      - "scripts/dump_timeout.py"
      - "--service"
      - "${service_name}"
      - "--reason"
      - "${stop_reason}"

services:
  api:
    executable: "cargo"
    args: ["run"]
    cwd: "./backend"
    hooks:
      before_start: ["announce-before-start"]
      after_stop_timeout: ["dump-stop-timeout"]
```

## 12. 当前建议结论

建议当前就把下面这些约定写死到设计层：

- 首版主机制是 `${name}` 占位符展开
- 首版先只承诺在 `args` 中可用
- `before_start` 不提供 `${service_pid}`
- `${service_name}`、`${action_name}`、`${service_cwd}` 作为第一批稳定变量
- 未知变量和 hook 不可用变量都应在 `check` 阶段报错
- 后续再补环境变量注入，但语义必须与占位符保持一致
