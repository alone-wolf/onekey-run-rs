# `run` 命令设计

## 1. 目标

当前 `onekey-run` 已支持：

- `up`：按依赖图批量启动一个或多个 service
- `down`：停止当前实例
- `actions` + `service.hooks`：在 service 生命周期节点执行 action

但在日常调试中，还需要一种更轻量的入口：

- 单独执行某个 service，而不是走完整 `up`
- 单独执行某个 action，而不是只能通过 hook 间接触发
- 在 action 执行前，明确向用户展示本次运行使用到的全部参数值

因此需要新增 `run` 子命令。

## 2. 命令定位

`run` 的定位是“单次调试执行”，而不是实例级编排。

它与现有命令的关系如下：

- `up`
  用于按依赖关系编排并持续监控一组 service
- `run --service`
  用于只执行一个 service，便于单点调试
- `run --action`
  用于只执行一个 action，便于验证 action 本身以及占位符渲染结果

`run` 不应自动演变为新的实例管理命令，也不应替代 `up` / `down`。

## 3. CLI 设计

### 3.1 命令格式

建议新增以下命令形态：

```bash
onekey-run run --service <service_name> [--with-all-hooks | --without-hooks]
onekey-run run --service <service_name> [--hook <hook_name> --hook <hook_name> ...]
onekey-run run --action <action_name> [--arg key=value ...]
```

说明：

- `--service` 与 `--action` 必须互斥，且二选一
- `--with-all-hooks`、`--without-hooks`、`--hook ...` 三组参数必须互斥
- `--hook` 允许重复传入
- `--arg` 允许重复传入

### 3.2 建议默认行为

建议：

- `run --service <name>` 默认等价于 `run --service <name> --without-hooks`
- `run --action <name>` 默认使用手工执行场景的参数默认值

原因：

- `run --service` 的主要用途是快速调试单个 service，本能预期更接近“直接拉起进程”
- 若默认执行全部 hook，容易与 `up` 的行为重叠，也更容易引入副作用

## 4. `run --service` 语义

### 4.1 基本行为

`run --service <service_name>` 只执行指定 service 本身：

- 不自动补齐 `depends_on`
- 不参与实例级 registry / state 管理
- 使用该 service 已解析后的：
  - `executable`
  - `args`
  - `cwd`
  - `env`
  - `stop_signal`
  - `stop_timeout_secs`
  - `log`

其运行期表现应尽量复用当前 `up` 的单 service 逻辑：

- 前台启动
- 响应 `Ctrl-C`
- 先优雅停止，再按现有规则升级为强制停止
- 若 service 异常退出，命令返回失败

### 4.2 hook 选择行为

#### `--without-hooks`

完全不执行任何 hook。

#### `--with-all-hooks`

在 service 生命周期内按真实运行阶段执行全部已配置 hook：

- `before_start`
- `after_start_success`
- `after_start_failure`
- `before_stop`
- `after_stop_success`
- `after_stop_timeout`
- `after_stop_failure`
- `after_runtime_exit_unexpected`

是否真正触发某个 hook，仍取决于运行时是否到达对应阶段。

#### `--hook <hook_name>`

只执行用户显式选择的 hook。

例如：

```bash
onekey-run run --service api --hook before_start --hook after_start_success
```

表示：

- 仅当运行流转到 `before_start`、`after_start_success` 时才执行 hook
- 其他 hook 即使配置存在，也跳过

### 4.3 hook 过滤规则

建议在 orchestrator 内增加统一判断：

- 若为 `all`，允许全部 hook
- 若为 `none`，全部跳过
- 若为 `selected(set)`，仅允许集合中的 hook

这样可以继续复用现有 hook 执行入口，而不必复制整套 hook 运行逻辑。

## 5. `run --action` 语义

### 5.1 基本行为

`run --action <action_name>` 表示直接执行一个已定义 action，而不依赖某个 service hook 触发。

它仍然遵守 action 原有定义：

- `executable`
- `args`
- `cwd`
- `env`
- `timeout_secs`

但在渲染 `${...}` 占位符时，运行上下文来自：

- 用户显式传入的 `--arg key=value`
- `onekey-run` 为 standalone action 提供的默认值

### 5.2 参数输入形式

建议采用：

```bash
onekey-run run --action notify --arg service_name=api --arg hook_name=manual
```

其中：

- `--arg` 后必须是 `key=value`
- 不允许只写 key
- 不允许空 key
- 同一个 key 若重复出现，后者覆盖前者

### 5.3 允许传入的参数名

建议仅允许当前占位符系统已支持的变量名：

- `project_root`
- `config_path`
- `service_name`
- `action_name`
- `hook_name`
- `service_cwd`
- `service_executable`
- `service_pid`
- `stop_reason`
- `exit_code`
- `exit_status`

若用户传入未知参数名，应直接报错，而不是静默忽略。

例如：

- `--arg service_name=api`：合法
- `--arg servie_name=api`：报错

这样可以避免拼写错误悄悄回退到默认值。

## 6. standalone action 的默认值策略

### 6.1 默认值来源

当 `run --action` 未显式提供某些参数时，建议由 `onekey-run` 提供默认值。

最小默认值建议如下：

- `project_root`
  配置文件所在目录
- `config_path`
  配置文件绝对路径
- `action_name`
  当前 action 名
- `hook_name`
  `manual`
- `service_name`
  `manual`
- `service_cwd`
  配置文件所在目录
- `service_executable`
  空字符串
- `service_pid`
  空字符串语义
- `stop_reason`
  `manual`
- `exit_code`
  空字符串语义
- `exit_status`
  `manual`

### 6.2 service 感知型默认推导

若用户显式提供了 `service_name=<name>`，且该 service 在配置中存在，建议进一步推导：

- `service_cwd`
  使用该 service 解析后的 `cwd`
- `service_executable`
  使用该 service 的 `executable`

这样可以提升这类 action 的可用性：

```yaml
args: ["--service", "${service_name}", "--cwd", "${service_cwd}"]
```

### 6.3 不建议做的事情

当前阶段不建议：

- 自动推导 `service_pid`
- 自动推导 `exit_code`
- 自动推导 `exit_status`
- 为 standalone action 新增新的占位符语法

这些字段只有在明确生命周期上下文中才有稳定语义。

## 7. 执行前参数展示

这是本次 `run` 设计的强制要求：

- 执行任何 action 前
- 必须把该 action 本次会使用到的全部参数值写出来给用户查看

### 7.1 展示范围

建议只展示“该 action 的 `args` 实际引用到的占位符”，而不是所有可用变量。

例如 action：

```yaml
args: ["--service", "${service_name}", "--hook", "${hook_name}"]
```

则执行前输出：

```text
action `notify` resolved params:
- service_name=api
- hook_name=before_start
```

### 7.2 适用范围

该规则不应只覆盖 `run --action`，还应统一覆盖：

- `run --action`
- `run --service` 中被 hook 执行的 action
- 现有 `up` / `down` / runtime hook 流程中的 action

也就是说，应把“执行前打印参数值”沉淀为 action 执行链路的公共行为。

### 7.3 输出媒介

建议至少：

- 打印到终端
- 同时写入 runtime event / instance log 摘要

这样即使未来在 daemon / TUI 场景中，也能保留可审计线索。

## 8. 与现有结构的衔接建议

### 8.1 CLI 层

在 `src/cli.rs` 中新增：

- `Command::Run(RunArgs)`
- `RunArgs`
- `RunTarget`
- hook 选择策略参数

建议通过 `clap` 的互斥组表达：

- `--service` vs `--action`
- `--with-all-hooks` vs `--without-hooks` vs `--hook`

### 8.2 app 层

在 `src/app.rs` 中新增分发：

- `Command::Run(args) => orchestrator::run_single(...)`

### 8.3 orchestrator 层

建议新增两个清晰入口：

- `run_single_service(...)`
- `run_single_action(...)`

不建议直接把 `run` 硬塞进 `build_run_plan()`：

- `build_run_plan()` 当前天然带有“目标 service + 依赖补齐 + topo 排序”的 `up` 语义
- `run --service` 强调“只跑单个 service”，语义不同

### 8.4 action 上下文层

当前 `ActionRenderContext` 更偏 hook 场景。

建议二选一：

1. 把 `hook_name` 改为更通用的字符串字段
2. 或新增 standalone action 专用上下文结构

无论采用哪种方式，都建议把以下能力抽成公共 helper：

- 扫描 action `args` 中引用了哪些占位符
- 解析这些占位符的最终值
- 生成执行前展示文本

### 8.5 process 层

当前 `process::run_action(...)` 要求传入 `HookName`。

若要支持 standalone action，建议改成传入 hook 名字符串，或新增 standalone 包装函数。

目标是让以下场景共用同一套 action 执行逻辑：

- hook 触发 action
- `run --action`

## 9. 失败语义

### 9.1 `run --service`

- `before_start` hook 失败：整个命令失败，service 不启动
- service 启动失败：命令失败
- service 运行中异常退出：命令失败
- 停止侧 hook 失败：保留当前策略，优先停止主流程，hook 错误输出到终端并写事件

### 9.2 `run --action`

- action 启动失败：命令失败
- action 非零退出：命令失败
- action 超时：命令失败
- 参数渲染失败：命令失败
- 未知 `--arg`：命令失败

## 10. 测试建议

至少补充以下测试：

- CLI 解析：
  - `--service` 与 `--action` 互斥
  - hook 选择参数互斥
  - `--hook` 可重复
  - `--arg key=value` 可重复
- service 运行：
  - `run --service --without-hooks`
  - `run --service --with-all-hooks`
  - `run --service --hook before_start`
- action 运行：
  - `run --action` 使用默认参数
  - `run --action --arg service_name=api` 能正确推导 service 相关默认值
  - 未知 `--arg` 直接报错
- 参数展示：
  - 执行前确实输出 action 实际使用到的全部参数值
  - hook 场景与 standalone action 场景表现一致

## 11. 文档同步建议

若该设计进入实现，建议同步更新：

- `docs_dev/02_cli_contract.md`
- `docs_dev/03_config_schema.md`
- `docs_design/02_actions_context_variables.md`
- `docs_design/04_actions_hooks_execution_flow.md`

## 12. 当前建议结论

本设计建议采用以下收敛方案：

- 新增 `run` 子命令
- `run --service` 默认不跑 hook
- `run --service` 只执行单个 service，不补齐依赖
- `run --action` 使用 `--arg key=value` 提供手工上下文
- 未显式提供的 action 参数由 `onekey-run` 给出默认值
- 执行任何 action 前，统一打印本次实际使用到的全部参数值

这样可以在不破坏现有 `up` / `down` 主语义的前提下，为调试场景提供更直接、可预期的入口。
