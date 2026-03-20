# `actions` / `hooks` 配置 Schema 草案

## 1. 目标

本文档用于把 `actions` 与 `service.hooks` 的配置形状先钉住，便于后续：

- 实现配置解析
- 为 `check` 增加静态校验
- 编写 `init --full` 模板
- 给用户提供稳定文档

本文件是设计草案，不代表当前代码已支持。

## 2. 顶层结构建议

建议在现有顶层结构上新增 `actions`：

```yaml
defaults:
  stop_timeout_secs: 10

actions:
  action_name:
    executable: "python"
    args: ["scripts/task.py"]

services:
  service_name:
    executable: "cargo"
    args: ["run"]
    hooks:
      before_start: ["action_name"]
```

建议整体结构为：

- `defaults`
  现有默认配置
- `actions`
  新增，定义可复用短时动作
- `services`
  现有服务定义

## 3. `actions` section 草案

### 3.1 结构

`actions` 为一个 map：

- key: action 名
- value: action 定义

示例：

```yaml
actions:
  prepare-env:
    executable: "python"
    args: ["scripts/prepare.py", "${service_name}"]
    cwd: "."
    env:
      APP_ENV: "dev"
    timeout_secs: 30
    disabled: false
```

### 3.2 字段建议

每个 action 建议支持以下字段：

- `executable: string`
  必填；可执行文件名或路径
- `args: string[]`
  可选；参数数组；首版建议支持占位符展开
- `cwd: string`
  可选；工作目录；相对路径按配置文件目录解析
- `env: map<string, string>`
  可选；附加环境变量
- `timeout_secs: integer`
  可选；action 最长运行时间；必须大于 `0`
- `disabled: boolean`
  可选；默认 `false`

### 3.3 字段级语义

#### `executable`

- 必须是非空字符串
- 不允许数组形式
- 不做 shell 解析
- 若用户需要 shell 特性，应显式写 shell 可执行文件，例如：

```yaml
executable: "sh"
args: ["-c", "echo hello"]
```

#### `args`

- 必须是字符串数组
- 数组项按独立参数传递
- 不执行 shell 拆词
- 首版建议仅在该字段中支持 `${name}` 占位符

#### `cwd`

- 若省略，建议默认使用配置文件所在目录
- 若为相对路径，按配置文件所在目录解析
- 建议运行时统一转为绝对路径

#### `env`

- key 与 value 都应为字符串
- 建议不支持非字符串值自动转换
- 与宿主环境合并时，action 自身配置覆盖同名变量

#### `timeout_secs`

- 若省略，则表示不单独限制 action 超时时间，或回退到未来可能定义的默认值
- 若提供，必须为正整数

#### `disabled`

- `true` 时 action 本体不应被执行
- 是否允许 hook 引用被禁用 action，需要在实现前明确；当前建议 `check` 直接报错

## 4. `services[*].hooks` section 草案

### 4.1 结构

每个 service 可新增 `hooks`：

```yaml
services:
  api:
    executable: "cargo"
    args: ["run"]
    hooks:
      before_start: ["prepare-env"]
      after_start_success: ["notify-up"]
      after_start_failure: ["dump-start-failure"]
      before_stop: ["notify-stop"]
      after_stop_success: ["cleanup"]
      after_stop_timeout: ["dump-timeout"]
      after_stop_failure: ["final-alert"]
```

### 4.2 字段建议

`hooks` 为一个 object，key 为 hook 名，value 为 action 名数组。

建议支持的 hook：

- `before_start`
- `after_start_success`
- `after_start_failure`
- `before_stop`
- `after_stop_success`
- `after_stop_timeout`
- `after_stop_failure`
- `after_runtime_exit_unexpected`

其中：

- 前 7 个更适合作为首版实现范围
- `after_runtime_exit_unexpected` 可放到第二阶段

### 4.3 值类型建议

每个 hook 的值应为：

- `string[]`

例如：

```yaml
before_start: ["prepare-a", "prepare-b"]
```

当前不建议支持：

- 单字符串简写
- 内联 action 对象
- 混合数组

这样更容易校验，也更有利于保持配置风格一致。

## 5. 命名约束建议

### 5.1 action 名

建议 action 名：

- 在 `actions` 内唯一
- 非空
- 仅允许字母、数字、`-`、`_`

建议正则：

```text
^[A-Za-z0-9][A-Za-z0-9_-]*$
```

### 5.2 hook 名

hook 名必须来自受支持的固定集合，不允许用户自定义任意 hook 名。

## 6. 默认值建议

建议首版默认行为：

- `args`
  默认空数组
- `cwd`
  默认配置文件所在目录
- `env`
  默认空 map
- `disabled`
  默认 `false`
- `hooks`
  默认空 object

## 7. 推荐完整示例

```yaml
defaults:
  stop_timeout_secs: 10

actions:
  prepare-env:
    executable: "python"
    args: ["scripts/prepare.py", "${service_name}", "${service_cwd}"]
    cwd: "."
    env:
      APP_ENV: "dev"
    timeout_secs: 30
    disabled: false

  notify-up:
    executable: "python"
    args: ["scripts/notify_up.py", "${service_name}"]
    cwd: "."
    timeout_secs: 10

  dump-timeout:
    executable: "python"
    args: ["scripts/dump_timeout.py", "${service_name}", "${stop_reason}"]
    cwd: "."
    timeout_secs: 20

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

## 8. 首版不建议纳入 Schema 的字段

为了避免把一次性动作系统做得过重，首版不建议在 action 上支持：

- `depends_on`
- `restart`
- `stop_signal`
- `log`
- `daemon`
- `retry`
- `on_failure`
- `failure_policy`
- `shell`

若未来需要这些能力，建议另起迭代文档，不直接塞进第一版 Schema。

## 9. 解析层输出建议

实现期建议把 YAML 解析后的内部结构明确分成：

- 原始配置结构
- 解析后结构

其中解析后结构建议包含：

- action 名
- 解析后的绝对 `cwd`
- 标准化后的 `args`
- hook -> action 引用列表

这样后续 orchestrator、check、init 模板都能共享同一份解析结果。

## 10. 当前建议结论

建议把下面这些点尽快固定下来：

- 顶层新增 `actions`
- `service.hooks` 的值类型固定为 `string[]`
- action 使用 `executable + args + cwd + env + timeout_secs + disabled`
- 首版不支持内联 action 与复杂策略字段
- 占位符首版先只承诺在 `args` 中支持
