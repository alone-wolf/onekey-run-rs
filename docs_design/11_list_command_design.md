# `list` 命令设计

## 1. 目标

当前 CLI 已支持：

- `init`
- `check`
- `up`
- `down`
- `management`

但还缺少一个“只读配置结构”的命令，用户无法快速回答这些问题：

- 当前配置里定义了哪些 `services`
- 当前配置里定义了哪些 `actions`
- 某个 service 依赖了谁
- 某个 service 的 hooks 引用了哪些 action
- 某个对象的完整配置长什么样

因此建议新增：

- `onekey-run list`

用于读取 `onekey-tasks.yaml` 并列出其中编排的 `services`、`actions` 及其依赖关系。

## 2. 命令契约

### 2.1 基本用法

建议支持以下形式：

```bash
onekey-run list [--all]
onekey-run list --services
onekey-run list --actions
onekey-run list --DAG
onekey-run list --detail [--all|--services|--actions]
```

其中：

- `list`
  默认分别列出全部 `services` 和 `actions` 的名称
- `list --all`
  与默认行为等价，显式表示“同时选择 services 和 actions”
- `list --services`
  仅列出 `services`
- `list --actions`
  仅列出 `actions`
- `list --DAG`
  以有向无环图的文本形式列出 service / action 的依赖与引用关系
- `list --detail`
  输出选中对象的详细信息；若未显式选择对象范围，则等价于 `--detail --all`

### 2.2 标志语义

建议约束如下：

- `--all` 等价于 `--services --actions`
- `list` 不带任何范围参数时，默认按 `--all` 处理
- `--detail` 只改变输出粒度，不改变选择范围
- `--DAG` 与 `--detail` 互斥
- `--DAG` 与 `--services` / `--actions` / `--all` 首版建议互斥，避免语义歧义

补充建议：

- Clap 层可保留用户要求的 `--DAG`
- 为了风格统一，可同时加一个隐藏或等价别名 `--dag`

## 3. 输出范围与原则

### 3.1 输出对象

`list` 应直接基于配置文件中的原始对象输出：

- 顶层 `services`
- 顶层 `actions`

而不是基于“可运行计划”输出。

原因：

- `build_run_plan(...)` 面向 `up` / `check`
- `resolve_actions(...)` 会跳过 disabled action
- `resolve_service(...)` 会拒绝 disabled service

`list` 的本质是“读取配置”，不是“筛选可执行对象”。

因此建议：

- disabled 的 service 仍然展示
- disabled 的 action 仍然展示
- 并在文本中显式标记其状态

### 3.2 排序原则

当前 `services` / `actions` 在配置模型中都是 `BTreeMap`，因此首版输出顺序建议定义为：

- 按名称字典序稳定输出

不承诺保留 YAML 原始书写顺序。

## 4. 输出格式建议

### 4.1 名称模式

`onekey-run list`

建议输出为：

```text
services:
- app
- worker

actions:
- notify-exit
- notify-stop
- notify-up
- prepare-app
```

若对象被禁用，可追加标记：

```text
- worker [disabled]
```

### 4.2 detail 模式

`onekey-run list --detail --services`

建议为每个对象输出一个独立块，至少包含：

- `name`
- `executable`
- `args`
- `cwd`
- `env`
- `disabled`

其中 service 额外包含：

- `depends_on`
- `restart`
- `stop_signal`
- `stop_timeout_secs`
- `autostart`
- `log`
- `hooks`

其中 action 额外包含：

- `timeout_secs`

首版建议 detail 输出原始配置值为主。

如果后续需要，可再扩展为：

- 原始值
- effective 值

例如：

- `cwd` 的解析后绝对路径
- service 最终生效的 `stop_timeout_secs`

但这不是首版必须项。

### 4.3 DAG 模式

`onekey-run list --DAG`

首版建议输出“文本化边列表”，而不是复杂 ASCII 树图。

原因：

- service 依赖是 DAG，但 hooks 引用 action 后整体结构更适合边列表表达
- 更容易测试
- 更容易保持跨平台稳定输出

建议边类型：

```text
service: worker --depends_on--> service: app
service: app --hooks.before_start--> action: prepare-app
service: app --hooks.after_start_success--> action: notify-up
service: app --hooks.before_stop--> action: notify-stop
service: app --hooks.after_runtime_exit_unexpected--> action: notify-exit
service: worker --hooks.before_start--> action: prepare-app
service: worker --hooks.after_runtime_exit_unexpected--> action: notify-exit
```

对于未被任何 hook 引用的 action，建议单独输出：

```text
standalone actions:
- some-action
```

以避免 DAG 模式下“有定义但无边”的 action 被完全隐藏。

## 5. 依赖关系建模建议

### 5.1 service -> service

来自：

- `service.depends_on`

语义：

- `service A --depends_on--> service B`

### 5.2 service -> action

来自：

- `service.hooks.<hook_name>`

语义：

- `service A --hooks.before_start--> action X`

这里表达的是“引用关系”，不是运行期阻塞/调度图的完整时序图。

### 5.3 action -> service

首版不建议构造反向边。

原因：

- action 当前不声明 `depends_on`
- action 与 service 的关系是“被 hook 引用”，不是真正拥有反向依赖字段

## 6. 与现有实现的边界

### 6.1 不建议直接复用的路径

不建议直接复用：

- `src/orchestrator.rs` 中的 `build_run_plan(...)`
- `src/config.rs` 中的 `resolve_service(...)`
- `src/config.rs` 中的 `resolve_actions(...)`

原因：

- 它们服务于运行计划与可执行性校验
- 会过滤或拒绝 disabled 对象
- 不适合作为“配置浏览器”语义的基础

### 6.2 建议复用的路径

建议继续复用：

- `ProjectConfig::load(...)`
- `ProjectConfig::validate(...)`

即：

1. 先读取并校验配置
2. 再执行 list 渲染

这样 `list` 输出的始终是“合法配置”的结构。

## 7. 建议代码落点

### 7.1 `src/cli.rs`

新增：

- `Command::List(ListArgs)`
- `ListArgs`

建议字段：

- `all: bool`
- `services: bool`
- `actions: bool`
- `detail: bool`
- `dag: bool`

### 7.2 `src/app.rs`

在命令分发中新增：

- `Command::List(args)`

流程建议：

1. `ProjectConfig::load(&cli.config)?`
2. `orchestrator::run_list(&cli.config, &config, args)`

### 7.3 `src/orchestrator.rs`

建议新增：

- `run_list(...) -> AppResult<()>`
- `render_list_output(...) -> AppResult<String>`

其中：

- `run_list(...)` 负责调用渲染并 `println!`
- `render_list_output(...)` 负责纯文本拼装，便于测试

如需复用服务拓扑逻辑，可考虑把与 service DAG 相关的纯函数抽成可共享 helper，但不要把“运行计划选择逻辑”直接混进 list。

## 8. detail 字段建议

### 8.1 service

建议至少展示：

- `name`
- `executable`
- `args`
- `cwd`
- `env`
- `depends_on`
- `restart`
- `stop_signal`
- `stop_timeout_secs`
- `autostart`
- `disabled`
- `log`
- `hooks`

### 8.2 action

建议至少展示：

- `name`
- `executable`
- `args`
- `cwd`
- `env`
- `timeout_secs`
- `disabled`

### 8.3 hooks 展示建议

建议 detail 中保留 hooks 的原始分组，而不是扁平化：

```text
hooks:
  before_start: [prepare-app]
  after_start_success: [notify-up]
  before_stop: [notify-stop]
  after_runtime_exit_unexpected: [notify-exit]
```

这样更贴近配置文件心智模型。

## 9. 错误处理建议

`list` 建议沿用现有错误策略：

- 配置文件不存在 -> `ConfigIo`
- YAML 解析失败 -> `ConfigInvalid`
- schema / 引用关系非法 -> `ConfigInvalid`

即：

- `list` 不应在非法配置上做“尽力展示”
- 而是应先报清楚配置错误

## 10. 测试建议

建议至少补以下测试：

### 10.1 CLI 解析

- `list`
- `list --all`
- `list --services`
- `list --actions`
- `list --detail --services`
- `list --detail --actions`
- `list --DAG`
- `list --detail --DAG` 应报参数冲突

### 10.2 渲染测试

- 默认输出同时包含 `services:` 与 `actions:`
- `--services` 不输出 `actions:` 段
- `--actions` 不输出 `services:` 段
- detail 模式包含关键字段
- DAG 模式包含 service dependency 边
- DAG 模式包含 hook -> action 引用边
- orphan action 会在 standalone 区域出现

### 10.3 语义测试

- disabled service 仍会被列出
- disabled action 仍会被列出
- 名称输出按字典序稳定
- DAG 中 service 依赖关系顺序稳定

## 11. 分阶段实施建议

### Phase 1

- 增加 `list` 子命令
- 支持默认名称输出
- 支持 `--services` / `--actions` / `--all`

### Phase 2

- 增加 `--detail`
- 规范 service / action 详细字段输出

### Phase 3

- 增加 `--DAG`
- 增加 orphan action 展示
- 完善文档与示例

## 12. 当前建议结论

推荐按以下原则实现：

- `list` 是配置浏览命令，不是运行计划命令
- 默认同时展示 `services` 和 `actions`
- `--detail` 输出对象完整信息
- `--DAG` 输出 service 依赖与 hook 引用关系
- disabled 对象也应展示，并显式标记
- 首版输出以稳定、可测、文本友好为优先
