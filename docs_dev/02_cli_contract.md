# CLI 契约

## 1. 命令设计目标

CLI 必须保持简单、稳定、可预期。首版重点是“读配置并编排运行”，而不是提供复杂命令组合。

## 2. 建议命令集

### 必做命令

- `onekey-run init`
  在当前目录生成 `onekey-tasks.yaml` 模板；若目标文件已存在则拒绝覆盖。
- `onekey-run init --full`
  在当前目录生成带完整字段示例的 `onekey-tasks.yaml` 模板；若目标文件已存在则拒绝覆盖。
- `onekey-run up`
  读取当前目录配置，启动全部可用服务。
  默认仅显示运行中的服务列表和已运行时长，不直接输出各 service 日志。
  第一次 `Ctrl-C` 触发优雅停止，第二次 `Ctrl-C` 触发强制退出。
- `onekey-run up -d` / `onekey-run up --daemon`
  在后台启动服务并立即返回，行为类似 `docker compose up -d`。
  前台命令退出后，会保留一个后台 `onekey-run` 监督进程持续监控服务；可通过 `down` 停止。
- `onekey-run up --tui`
  启动后进入简易终端监控界面，显示服务状态并按服务切换日志页。
  第一次 `Ctrl-C` 触发优雅停止，第二次 `Ctrl-C` 触发强制退出。
- `onekey-run up <service>...`
  启动指定服务及其依赖。
- `onekey-run check`
  仅校验配置、依赖图和命令可执行性。
- `onekey-run management`
  显示当前机器上所有已登记、仍在运行中的 `onekey-run` 实例。
- `onekey-run management --watch`
  持续刷新显示当前运行中的实例列表，按 `Ctrl-C` 退出观察模式。
- `onekey-run management --json`
  以 JSON 格式输出实例列表、状态摘要和运行时长；当前与 `--watch` 互斥。
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

### 4.4 `check` 参数

命令格式：

```bash
onekey-run check
onekey-run check -c <path>
```

`check` 当前没有命令专属参数，主要依赖全局 `--config`。

### 4.5 `up` 参数

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
  启动后进入终端监控界面，显示实例状态和每个服务的日志页签。
- `-d, --daemon`
  后台运行模式。
  启动完成后前台命令立即返回，后台会保留一个监督进程继续监控服务并维护运行时状态。
  该参数当前与 `--tui` 互斥。

### 4.6 `down` 参数

命令格式：

```bash
onekey-run down
onekey-run down --force
onekey-run down -c <path>
```

- `--force`
  直接按强制停止路径终止服务，而不等待优雅退出超时。

### 4.7 `management` 参数

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
  输出中包含实例数量、项目路径、配置路径、服务列表、运行时长、存活服务数和状态摘要。
  当前与 `--watch` 互斥。

### 4.8 状态摘要字段说明

`management` 当前会输出一个 `status` / `status_summary` 字段，语义如下：

- `running`
  监督进程存活，且当前记录的服务都存活。
- `partial (x/y alive)`
  部分服务仍存活，或监督进程仍在但服务不是全量存活。
- `stale`
  运行时文件仍存在，但监督进程与服务都已不活跃，属于残留实例记录。

### 4.9 隐藏内部命令

- `__daemon-up`
  这是 `up -d` 使用的内部隐藏命令，不面向普通用户直接调用。

## 5. 输出约定

- 面向用户的命令输出保持简洁，避免泄露内部状态细节
- 错误输出进入 `stderr`
- 服务日志应包含服务名前缀，便于区分来源
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
