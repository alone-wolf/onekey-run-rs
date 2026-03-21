# CLI 契约

## 1. 命令设计目标

CLI 必须保持简单、稳定、可预期。首版重点是“读配置并编排运行”，而不是提供复杂命令组合。

## 2. 建议命令集

### 必做命令

- `onekey-run init`
  在当前目录生成 `onekey-tasks.yaml` 模板；若目标文件已存在则拒绝覆盖。
  当前实现通过内置 minimal preset 构造 `ProjectConfig` 后序列化为 YAML，不再依赖硬编码模板字符串。
  模板内容会按当前运行平台选择对应 preset。
- `onekey-run init --full`
  在当前目录生成带完整字段示例的 `onekey-tasks.yaml` 模板；若目标文件已存在则拒绝覆盖。
  当前实现通过内置 full preset 构造 `ProjectConfig` 后序列化为 YAML，不再依赖硬编码模板字符串。
  模板内容会按当前运行平台选择对应 preset。
- `onekey-run up`
  读取当前目录配置，启动全部可用服务。
  默认仅显示运行中的服务列表和已运行时长，不直接输出各 service 日志。
  若配置了顶层 `log`，实例生命周期事件以及 hook/action/watch 的执行状态会额外落盘到实例日志。
  若 service 配置了 `watch`，前台运行期间会监控对应文件或目录变化，并自动重启该 service。
  第一次 `Ctrl-C` 触发优雅停止，第二次 `Ctrl-C` 触发强制退出。
- `onekey-run up -d` / `onekey-run up --daemon`
  在后台启动服务并立即返回，行为类似 `docker compose up -d`。
  前台命令退出后，会保留一个后台 `onekey-run` 监督进程持续监控服务；可通过 `down` 停止。
  若配置了顶层 `log`，应由后台 supervisor 负责持续写入实例日志；若配置了 `watch`，也由后台 supervisor 持续处理 watch 重启。
- `onekey-run up --tui`
  启动后进入简易终端监控界面，显示服务状态、按服务切换日志页，并提供独立的 Events 面板查看编排器内部事件。
  若配置了 `watch`，TUI 期间同样会继续处理 watch 事件与重启。
  第一次 `Ctrl-C` 触发优雅停止，第二次 `Ctrl-C` 触发强制退出。
- `onekey-run up <service>...`
  启动指定服务及其依赖。
- `onekey-run check`
  仅校验配置、依赖图和命令可执行性。
  当配置包含顶层 `log`、`actions`、`hooks`、`watch` 时，也应校验日志组合合法性、路径冲突、hook/action 引用关系以及 watch 路径合法性。
- `onekey-run list`
  读取配置并列出其中定义的 `services`、`actions`、详细信息或 DAG 风格依赖关系。
- `onekey-run run --service <service_name>`
  只执行单个 service，默认不运行 hook；适合调试某个 service 本身。
- `onekey-run run --service <service_name> --with-all-hooks`
  单独执行一个 service，并按真实生命周期触发其全部 hook。
- `onekey-run run --service <service_name> --hook <hook_name>...`
  单独执行一个 service，但只在命中的生命周期节点触发指定 hook。
- `onekey-run run --action <action_name> [--arg key=value ...]`
  直接执行一个 action；未显式提供的上下文参数使用 `onekey-run` 的默认值。
  在执行任何 action 前，CLI 都会先打印本次实际使用到的参数值。
- `onekey-run management`
  显示当前机器上所有已登记、仍在运行中的 `onekey-run` 实例。
- `onekey-run management --watch`
  持续刷新显示当前运行中的实例列表，按 `Ctrl-C` 退出观察模式。
- `onekey-run management --json`
  以 JSON 格式输出实例列表、状态摘要、运行时长以及最近事件摘要；当前与 `--watch` 互斥。
- `onekey-run down`
  读取当前目录对应的运行时状态文件，优雅停止此前由 `up` 启动的服务。

### 可选命令

- `onekey-run graph`
  输出服务依赖关系。
- `onekey-run ps`
  输出运行状态摘要。
- `onekey-run logs [service]`
  查看聚合日志或指定服务日志。

## 3. 全局参数建议

- `-c, --config <path>`
  指定配置文件路径，默认值为当前目录下的 `onekey-tasks.yaml`。
- `--verbose`
  打开更详细的内部日志。
- `--quiet`
  减少非必要输出，只保留关键结果和错误。
- `--no-color`
  禁用彩色输出，便于 CI 或日志采集。

## 4. 当前参数说明

以下说明基于当前已经实现的 CLI 参数行为。

### 4.1 顶层命令格式

```bash
onekey-run [global options] <command> [command options]
```

常见示例：

```bash
onekey-run check
onekey-run check -c ./onekey-tasks.yaml
onekey-run list
onekey-run list --services
onekey-run list --detail
onekey-run list --DAG
onekey-run run --service api
onekey-run run --service api --with-all-hooks
onekey-run run --service api --hook before_start --hook after_start_success
onekey-run run --action notify --arg service_name=api
onekey-run up
onekey-run up app worker
onekey-run up -d
onekey-run up --tui
onekey-run down
onekey-run down --force
onekey-run management
onekey-run management --watch
onekey-run management --json
```

### 4.2 全局参数

- `-c, --config <path>`
  指定配置文件路径。
  默认值为当前目录下的 `onekey-tasks.yaml`。
  `check` / `up` / `down` 都会使用这个路径；`down` 会按该配置文件所在目录定位运行时状态，而不是按命令执行时的当前目录定位。
- `--verbose`
  预留给更详细的内部输出；当前阶段已接入参数定义，但尚未扩展出明显差异化输出。
- `--quiet`
  预留给精简输出；当前阶段已接入参数定义，但尚未对全部命令做细粒度输出裁剪。
- `--no-color`
  预留给禁用彩色输出；当前阶段已接入参数定义。

### 4.3 `init` 参数

命令格式：

```bash
onekey-run init [--full]
```

- `--full`
  生成一份包含较完整字段示例的配置模板。
  未传入时生成最小可读模板。

补充约定：

- `init` / `init --full` 的模板示例按当前运行平台生成
- Windows 用户应优先使用 `onekey-run init` 生成模板，而不是直接复用仓库工作区中的临时示例配置

### 4.4 `check` 参数

命令格式：

```bash
onekey-run check
onekey-run check -c <path>
```

`check` 当前没有命令专属参数，主要依赖全局 `--config`。

### 4.5 `list` 参数

命令格式：

```bash
onekey-run list [--all]
onekey-run list --services
onekey-run list --actions
onekey-run list --detail [--all|--services|--actions]
onekey-run list --DAG
```

- 默认行为
  未传入范围参数时，分别列出全部 `services` 和 `actions` 的名称。
- `--all`
  同时选择 `services` 与 `actions`，语义上等价于 `--services --actions`。
- `--services`
  仅列出 `services`。
- `--actions`
  仅列出 `actions`。
- `--detail`
  输出所选对象的详细配置字段；若未显式指定范围，则默认按 `--all` 处理。
- `--DAG`
  输出 service 依赖与 hook -> action 引用关系的文本化 DAG 边列表。
  当前与 `--detail`、`--all`、`--services`、`--actions` 互斥。

补充约定：

- `list` 基于已通过 schema 校验的原始配置对象输出，而不是基于运行计划输出
- disabled 的 service / action 也会被列出，并在文本中显式标记

### 4.5.1 `run` 参数

命令格式：

```bash
onekey-run run --service <service_name> [--with-all-hooks | --without-hooks]
onekey-run run --service <service_name> [--hook <hook_name> --hook <hook_name> ...]
onekey-run run --action <action_name> [--arg key=value ...]
```

- `--service <service_name>`
  单独执行一个 service。
  当前不会自动补齐 `depends_on`，语义上不同于 `up <service>`。
- `--action <action_name>`
  单独执行一个 action。
- `--with-all-hooks`
  service 模式下，按实际生命周期触发全部 hook。
- `--without-hooks`
  service 模式下，跳过全部 hook。
- `--hook <hook_name>`
  service 模式下，仅触发指定 hook；该参数可重复。
- `--arg key=value`
  action 模式下，为 standalone action 提供或覆盖上下文变量；该参数可重复。
  未显式提供的上下文值会由 `onekey-run` 自动补默认值。

补充约定：

- `--service` 与 `--action` 互斥，且必须二选一
- `--with-all-hooks`、`--without-hooks`、`--hook` 互斥
- `run --service <name>` 默认等价于 `run --service <name> --without-hooks`
- `run --action` 允许覆盖的 key 必须属于受支持占位符集合，未知 key 直接报错
- 执行任何 action 前，CLI 会先打印该 action 本次实际使用到的全部参数值；若没有占位符，则打印 `(none)`

### 4.6 `up` 参数

命令格式：

```bash
onekey-run up [services...]
onekey-run up --tui [services...]
onekey-run up -d [services...]
```

- `services...`
  可选的位置参数。
  未传入时，启动所有 `autostart: true` 且未禁用的服务，并自动补上它们的依赖。
  传入时，仅启动指定服务及其依赖。
- `--tui`
  启动后进入终端监控界面，显示实例状态、每个服务的日志页签，以及独立的 Events 面板。
  顶层实例 `log` 若已配置，TUI 期间产生的 hook/action/watch 生命周期状态也应写入实例日志。
- `-d, --daemon`
  后台运行模式。
  启动完成后前台命令立即返回，后台会保留一个监督进程继续监控服务并维护运行时状态。
  该参数当前与 `--tui` 互斥。

### 4.7 `down` 参数

命令格式：

```bash
onekey-run down
onekey-run down --force
onekey-run down -c <path>
```

- `--force`
  直接按强制停止路径终止服务，而不等待优雅退出超时。

### 4.8 `management` 参数

命令格式：

```bash
onekey-run management
onekey-run management --watch
onekey-run management --json
```

- `--watch`
  持续刷新显示当前运行中的 `onekey-run` 实例列表。
  默认每秒刷新一次，按 `Ctrl-C` 退出。
- `--json`
  以 JSON 形式输出当前实例快照。
  输出中包含实例数量、项目路径、配置路径、服务列表、运行时长、存活服务数、状态摘要、最近事件和 service 级最近 hook/action 摘要。
  当前与 `--watch` 互斥。

### 4.9 状态摘要字段说明

`management` 当前会输出一个 `status` / `status_summary` 字段，语义如下：

- `running`
  监督进程存活，且当前记录的服务都存活。
- `partial (x/y alive)`
  部分服务仍存活，或监督进程仍在但服务不是全量存活。
- `stale`
  运行时文件仍存在，但监督进程与服务都已不活跃，属于残留实例记录。

### 4.10 最近事件摘要

`management` 当前会尝试读取 `.onekey-run/events.jsonl` 并展示最近事件摘要：

- 文本模式会在每个实例行里显示最近事件类型，并追加一行 `recent` 摘要
- `--json` 模式会额外输出：
  - `last_event`
  - `service_summaries`

若事件文件不存在，则对应字段为空，不视为错误。

### 4.11 隐藏内部命令

- `__daemon-up`
  这是 `up -d` 使用的内部隐藏命令，不面向普通用户直接调用。

## 5. 输出约定

- 面向用户的命令输出保持简洁，避免泄露内部状态细节
- 错误输出进入 `stderr`
- 服务日志应包含服务名前缀，便于区分来源
- 执行任何 action 前，应先输出该 action 本次使用到的占位符参数值
- 若配置顶层 `log`，实例日志应记录 orchestrator 生命周期事件以及 hook/action 状态摘要
- 人类输出和机器输出应明确分层；若未来支持 `--json`，需单独定义输出 schema

## 6. 退出码约定

建议统一定义退出码，而不是全部返回 `1`：

- `0`：成功
- `2`：CLI 参数错误
- `3`：配置文件不存在或不可读
- `4`：配置格式错误或校验失败
- `5`：启动失败
- `6`：运行中服务异常退出导致整体失败
- `7`：优雅停止超时后强制终止

## 7. 兼容性约定

- 首版命令名、参数名和退出码一旦发布，不轻易做破坏性修改
- 若后续扩展命令，优先新增，不重载现有行为
- 所有默认行为必须在文档中明示，不能依赖隐式实现细节

## 8. 待确认问题

- `init` 是否需要 `--force` 覆盖已有配置
- `check` 是否需要检查命令路径存在性
- `up` 是否默认启动 `disabled: false` 且 `autostart: true` 的服务
- 是否提供 `--dry-run` 用于仅打印执行计划
- `down` 是否允许 `--force` 直接跳过优雅等待
