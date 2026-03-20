# `check` 命令未来需要新增的校验与报错清单

## 1. 目标

`actions` / `hooks` 一旦引入，`check` 会成为防止运行期踩坑的第一道关口。

本文档的目的，是把未来应补充的校验项和建议报错方式先整理清楚，避免实现时东补一点、西补一点。

本文件只做规划，不代表当前代码已支持这些校验。

## 2. 设计原则

建议 `check` 对这类配置问题采用“尽早、明确、可定位”的风格：

- 尽量在启动前发现
- 报错信息要指出字段路径
- 报错内容要说明为什么错
- 若能给出建议修正方式，就直接写在报错里

建议输出尽量包含：

- 配置文件路径
- YAML 路径
- 具体错误原因
- 可选的修复建议

## 3. `actions` section 结构校验

### 3.1 顶层类型错误

需要校验：

- `actions` 若存在，必须是 map

示例报错：

```text
config error: `actions` must be a mapping of action_name -> action definition
```

### 3.2 action 名非法

需要校验：

- action 名不能为空
- action 名必须满足命名规则
- action 名不能重复

示例报错：

```text
config error at `actions.bad name`: action name must match ^[A-Za-z0-9][A-Za-z0-9_-]*$
```

### 3.3 action 定义类型错误

需要校验：

- `actions.<name>` 必须是 object

示例报错：

```text
config error at `actions.prepare-env`: action definition must be an object
```

## 4. action 字段级校验

### 4.1 `executable`

需要校验：

- 必填
- 必须是非空字符串

示例报错：

```text
config error at `actions.prepare-env.executable`: executable is required and must be a non-empty string
```

### 4.2 `args`

需要校验：

- 若存在，必须是字符串数组
- 数组元素不能是 null / object / number / bool

示例报错：

```text
config error at `actions.prepare-env.args[1]`: each arg must be a string
```

### 4.3 `cwd`

需要校验：

- 若存在，必须是非空字符串
- 解析后路径必须存在，或至少在严格模式下要求存在

建议：

- 若项目当前 `services[*].cwd` 已要求路径存在，则 action 保持同标准

示例报错：

```text
config error at `actions.prepare-env.cwd`: resolved directory does not exist
```

### 4.4 `env`

需要校验：

- 若存在，必须是 string -> string map

示例报错：

```text
config error at `actions.prepare-env.env.APP_ENV`: env value must be a string
```

### 4.5 `timeout_secs`

需要校验：

- 若存在，必须是正整数

示例报错：

```text
config error at `actions.prepare-env.timeout_secs`: timeout_secs must be greater than 0
```

### 4.6 `disabled`

需要校验：

- 若存在，必须是 boolean

示例报错：

```text
config error at `actions.prepare-env.disabled`: disabled must be a boolean
```

### 4.7 未知字段

建议首版：

- 对 action 未知字段直接报错

原因：

- 这类配置是工具契约
- 宽松忽略很容易让用户误以为字段已生效

示例报错：

```text
config error at `actions.prepare-env.retry`: unknown field `retry`
```

## 5. `hooks` section 结构校验

### 5.1 `hooks` 类型

需要校验：

- `services.<name>.hooks` 若存在，必须是 object

### 5.2 hook 名合法性

需要校验：

- hook 名必须属于受支持集合

示例报错：

```text
config error at `services.api.hooks.pre_start`: unknown hook name `pre_start`; did you mean `before_start`?
```

### 5.3 hook 值类型

需要校验：

- 每个 hook 的值必须是字符串数组
- 不接受单字符串
- 不接受对象数组

示例报错：

```text
config error at `services.api.hooks.before_start`: hook value must be an array of action names
```

## 6. action 引用关系校验

### 6.1 引用不存在的 action

需要校验：

- hook 中引用的 action 必须在 `actions` 中存在

示例报错：

```text
config error at `services.api.hooks.before_start[0]`: referenced action `prepare-env` is not defined in `actions`
```

### 6.2 引用被禁用 action

当前建议：

- 若某 hook 引用了 `disabled: true` 的 action，`check` 直接报错

示例报错：

```text
config error at `services.api.hooks.before_start[0]`: action `prepare-env` is disabled and cannot be referenced
```

### 6.3 重复引用

可选建议：

- 同一 hook 数组里重复引用同一个 action，先给 warning 或 info
- 不一定要直接报错

因为有时用户可能就是想跑两次，但通常这更像配置失误。

## 7. 占位符校验

### 7.1 语法错误

需要校验：

- 存在未闭合 `${...`
- `${}` 空变量名
- 变量名包含非法字符

示例报错：

```text
config error at `actions.prepare-env.args[1]`: invalid placeholder syntax `${service_name`
```

### 7.2 未知变量

需要校验：

- 占位符名必须来自受支持集合

示例报错：

```text
config error at `actions.prepare-env.args[2]`: unknown placeholder `${service_naem}`
```

### 7.3 hook 不可用变量

需要校验：

- 某些变量只在特定 hook 可用
- 若 action 被某 hook 引用，而其 `args` 中用了该 hook 不可用变量，直接报错

示例报错：

```text
config error at `services.api.hooks.before_start[0]`: action `prepare-env` uses `${service_pid}`, which is not available in `before_start`
```

### 7.4 多 hook 复用 action 的兼容性

需要特别校验：

- 同一个 action 可能被多个 hook 引用
- 若 action 内使用了只适用于部分 hook 的变量，则必须检查所有引用点

例如：

- `dump-meta` 同时被 `before_start` 和 `after_start_success` 引用
- 若它用了 `${service_pid}`，那么 `before_start` 这个引用点应报错

## 8. 路径与可执行文件相关校验

建议区分两层：

### 8.1 静态可校验项

- `cwd` 路径能否解析
- 解析后目录是否合法

### 8.2 运行时项

- `executable` 是否真能在 PATH 中找到

关于 `executable`：

- `check` 阶段可选做一次本机 PATH 探测
- 但如果要保持跨平台一致性，也可以先只做语义校验，不做存在性保证

建议把这点单独作为实现期决策，不与基础 Schema 校验耦死。

## 9. 运行语义相关校验

### 9.1 空 hook 数组

可选建议：

- 允许空数组，但给出 info

### 9.2 action 未被任何 hook 引用

可选建议：

- 不报错
- 可给 warning，提示“定义了但未使用”

### 9.3 超时与 hook 类型的组合

首版建议：

- 所有 hook 类型都允许 action 配 `timeout_secs`
- 不额外限制

## 10. 建议错误分级

为了后续 CLI 输出更清晰，建议内部把问题至少分成：

- `error`
  阻止 `up`
- `warning`
  不阻止，但建议修复
- `info`
  仅提示

建议首版最少有以下 `error`：

- action 名非法
- action 缺少 `executable`
- hook 名非法
- hook 引用不存在 action
- hook 引用 disabled action
- 占位符未知
- 占位符对当前 hook 不可用
- 字段类型不匹配
- 未知字段

## 11. 建议报错汇总策略

建议 `check` 不要遇到第一个错误就退出，而是：

- 尽量收集并输出所有可确定错误
- 最后统一返回失败

这样用户修配置时效率更高。

建议输出风格：

```text
found 3 configuration errors:
1. `actions.prepare-env.args[1]`: unknown placeholder `${service_naem}`
2. `services.api.hooks.before_start[0]`: referenced action `prepare-env` is disabled
3. `services.api.hooks.pre_start`: unknown hook name `pre_start`; did you mean `before_start`?
```

## 12. 当前建议结论

未来实现 `actions` / `hooks` 时，`check` 至少应新增三大类能力：

- Schema 与字段类型校验
- hook -> action 引用关系校验
- 占位符语义与 hook 可用性校验

如果这三类没做好，运行期问题会明显增多，因此建议实现时优先级很高。
