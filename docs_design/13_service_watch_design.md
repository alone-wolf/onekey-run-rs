# service 级 `watch` 配置设计

## 1. 背景

当前 `onekey-run-rs` 已能管理 service 的启动、停止、hook 与实例日志，但还缺少一个常见开发期能力：

- 监控某个文件或目录
- 当内容发生变化时
- 自动重启对应 service

这个能力天然面向“某个 service 的运行策略”，但在 schema 设计上仍有一个分歧：

- 放在 `services.<name>` 下
- 还是新增一个全局 `watches` / `watch` section，再反向引用 service

本文档用于先把这个问题收敛，并给出首版推荐形状、校验规则与运行时语义。

## 2. 核心结论

建议首版采用 service 级配置：

```yaml
services:
  api:
    executable: "cargo"
    args: ["run"]
    watch:
      paths:
        - "./src"
        - "./Cargo.toml"
      debounce_ms: 500
```

不建议首版做成顶层全局 `watches`，原因如下：

1. `watch` 当前语义是“这个 service 因哪些路径变化而重启”，本质上属于 service 生命周期策略
2. 现有 `ServiceConfig` 已承载 `restart`、`stop_timeout_secs`、`log`、`hooks` 等运行策略，`watch` 放在同层最自然
3. 若放到顶层，全局项仍需额外声明“目标 service 是谁”，只会增加引用、校验和运行时分派复杂度
4. 当前项目已经形成较清晰的分层心智：
   - 顶层放实例级能力或可复用定义，如 `log`、`actions`
   - service 下放该 service 自身行为，如 `hooks`、`log`
5. 首版先解决“一处变化重启一个 service”即可，不必过早为了未来复用把模型做重

未来若真的出现“同一组 watch 规则复用给多个 service”或“一次变更需要同时重启多个 service”的需求，再单独评估顶层 `watches`。

## 3. 目标

本设计希望做到：

1. 为单个 service 增加简单直观的路径监控与自动重启能力
2. 目录与文件都可作为监控目标
3. 尽量复用现有 stop/start、hook、日志与事件流
4. 避免把 watch 做成新的独立调度系统
5. 保持旧配置完全兼容

非目标：

- 首版不引入顶层全局 `watches`
- 首版不支持“一条 watch 规则同时重启多个 service”
- 首版不支持复杂 include/exclude DSL
- 首版不支持配置热重载
- 首版不改变 `depends_on` 的现有语义

## 4. 推荐 schema

### 4.1 `services.<name>.watch`

建议在 `ServiceConfig` 下新增可选字段：

```yaml
services:
  api:
    executable: "cargo"
    args: ["run"]
    watch:
      paths:
        - "./src"
        - "./Cargo.toml"
      debounce_ms: 500
```

建议首版字段：

- `paths`
  必填，数组；每项表示要监控的目录或文件路径
- `debounce_ms`
  可选；表示从“首次观察到变更”到“真正发起重启”之间的防抖窗口，默认建议 `500`

建议暂不引入：

- `ignore`
- `recursive`
- `settle_ms`
- `restart_target`
- `enabled`

原因：

- `paths + debounce_ms` 已足够覆盖第一批真实场景
- 额外字段会立即拉高校验、序列化与运行时复杂度
- 保持 `watch` 为 object，后续仍可向内扩展，不会挡住未来演进

### 4.2 为什么不用更短的写法

不建议做成：

```yaml
watch:
  - "./src"
  - "./Cargo.toml"
```

或：

```yaml
watch: "./src"
```

原因：

- 不利于后续扩展
- 会让解析逻辑出现多形态分支
- 文档和错误提示更难统一

因此建议从第一版就固定为 object。

## 5. 路径语义

建议沿用当前配置的一致规则：

- 相对路径按 `onekey-tasks.yaml` 所在目录解析
- 绝对路径原样使用

`paths` 中每一项可为：

- 单个文件
- 单个目录

对于目录，建议默认递归监控其内部变更。

### 5.1 配置示例

```yaml
services:
  frontend:
    executable: "npm"
    args: ["run", "dev"]
    cwd: "./frontend"
    watch:
      paths:
        - "./frontend/src"
        - "./frontend/package.json"
      debounce_ms: 300
```

```yaml
services:
  api:
    executable: "cargo"
    args: ["run"]
    cwd: "./backend"
    watch:
      paths:
        - "./backend/src"
        - "./backend/Cargo.toml"
```

## 6. 校验规则建议

`check` 阶段建议新增以下规则：

1. `services.<name>.watch` 若存在，必须是 object
2. `services.<name>.watch.paths` 必须存在且不能为空数组
3. `services.<name>.watch.paths[*]` 必须是非空字符串
4. 每个 watch path 解析后必须存在
5. 每个 watch path 必须是文件或目录
6. 同一 service 的 watch path 解析后若重复，应报错或至少去重
7. `debounce_ms` 若存在，必须大于 `0`

建议错误风格示例：

```text
config error at `services.api.watch.paths`: watch.paths must be a non-empty array
config error at `services.api.watch.paths[0]`: resolved watch path does not exist
config error at `services.api.watch.debounce_ms`: debounce_ms must be greater than 0
```

### 6.1 关于“不存在路径”的取舍

首版建议要求 watch path 在启动时必须存在。

理由：

- 语义最稳定，最容易解释
- 便于 `check` 提前发现配置错误
- 避免运行时还要维护“先挂起，等路径未来出现再补注册”的额外状态机

若用户确实想感知某个未来生成的文件，建议首版改为监控其父目录。

### 6.2 内置排除建议

即使用户 watch 的目录范围较大，也建议运行时自动忽略以下内部/高风险路径变更，避免自触发重启循环：

- `.onekey-run/`
- 顶层 `log.file`
- 任意 `service.log.file`

特别是当用户 watch 仓库根目录，而日志文件也落在仓库内时，如果不做内置排除，很容易形成“日志写入 -> 触发 watch -> 重启 -> 再写日志”的循环。

这类内置排除更适合做运行时保护，不必在 schema 中暴露额外配置字段。

## 7. 运行时语义

### 7.1 基本流程

建议把 watch 视为“挂在 service 运行期旁边的观察器”，而不是独立 service。

单个 service 的基本流程为：

1. service 按正常流程启动
2. 若配置了 `watch`，则在该 service 进入运行态后挂上 watcher
3. watcher 观察到任一目标路径变化
4. 进入 `debounce_ms` 防抖窗口
5. 防抖窗口结束后，触发一次“原因是 watch 的 service 重启”

可简化为：

```text
service running
  -> watch event detected
  -> debounce
  -> restart requested(reason=watch)
```

### 7.2 重启语义

建议 watch 触发的重启尽量复用现有 stop/start 主流程：

1. 对目标 service 发起停止
2. 停止完成后，再重新启动该 service

这样可以直接复用：

- `before_stop`
- `after_stop_success`
- `after_stop_timeout`
- `after_stop_failure`
- `before_start`
- `after_start_success`
- `after_start_failure`

不建议额外造一条“轻量热重启专用流程”，否则 hook、日志、事件与错误处理都会分叉。

### 7.3 与 `restart` 的关系

`restart` 描述的是“service 因失败退出后是否自动拉起”；
`watch` 描述的是“外部文件变化时是否主动重启”。

二者应视为两套并列机制：

- `restart: no` 时，只要配置了 `watch`，仍可因文件变化而重启
- watch 触发的停止/启动不应被记为“失败重启”
- watch 触发的重启次数不应计入 failure-based restart 语义

### 7.4 与 hooks 的关系

watch 触发的重启，本质上是一次“有明确原因的主动 stop + start”。

因此建议：

- 停止侧 hooks 正常执行
- 启动侧 hooks 正常执行
- `before_stop` 中的 `${stop_reason}` 建议渲染为 `watch`
- 不触发 `after_runtime_exit_unexpected`，因为这不是异常退出

这样用户若已有通知、清理、准备动作，无需为 watch 再单独学习一套机制。

### 7.5 与 `depends_on` 的关系

当前 `depends_on` 只描述启动/停止顺序，不描述运行期联动。

因此建议首版保持一致：

- `api` 因 watch 重启时
- 不自动级联重启依赖它的 `worker`

这样更符合当前系统心智，也能避免一次文件变化把整棵依赖树都拉着重启。

未来若要支持“重启当前 service 及其下游”，应单独设计，避免偷偷改变 `depends_on` 的含义。

### 7.6 与失败后的继续观察

若 watch 触发重启后，新的启动失败，建议 watcher 仍继续保留。

理由：

- 这类功能主要服务开发场景
- 用户修复文件后，下一次变更应仍有机会再次触发启动

也就是说，只要整个 `onekey-run` 实例还活着，该 service 的 watcher 就不应因一次失败而永久失效。

### 7.7 关停行为

当用户执行 `down`、收到中断信号或实例整体进入退出流程时：

- watcher 应先停止接收新事件
- 已在排队中的重启请求应被丢弃
- 不应在全局退出过程中再次触发 service 重启

## 8. 防抖与并发建议

文件系统事件通常是突发且成批出现的，因此建议首版至少做两层保护：

1. `debounce_ms`
   合并短时间内连续事件
2. 单 service 单飞重启
   同一时刻只允许一个 watch 重启流程在跑

进一步建议：

- 若 service 正在执行 watch 重启，又来了新事件，则只记一个“dirty”标记
- 当前重启流程结束后，若仍为 dirty，再补一次重启
- 不要无限排队多个 restart job

这样能避免：

- 保存一次文件触发多次重启
- 编译器/包管理器一次写入很多文件时疯狂抖动
- 重启时间较长时队列不断堆积

## 9. 事件与日志建议

若项目配置了顶层实例 `log`，watch 相关状态也应写入实例日志与 `.onekey-run/events.jsonl`。

建议增加的事件至少包括：

- `watch_triggered`
- `watch_debounced`
- `watch_restart_requested`
- `watch_restart_skipped`

建议文本日志示例：

```text
[2026-03-21T10:00:01Z] [INFO] watch triggered service=api path=/project/backend/src/main.rs
[2026-03-21T10:00:01Z] [INFO] watch restart requested service=api debounce_ms=500
[2026-03-21T10:00:03Z] [INFO] service stopping name=api reason=watch
[2026-03-21T10:00:05Z] [INFO] service started name=api pid=12345 trigger=watch
```

## 10. 关于全局 `watches` 的未来演进

虽然首版不建议做顶层全局 `watches`，但可以提前明确它只在以下场景下才值得引入：

1. 多个 service 需要复用完全相同的 watch 路径集合
2. 一次变更要触发多个 service 协同重启
3. 希望把 watch 当成与 `actions` 类似的可命名资源管理

在这些需求真正出现前，不建议先设计类似：

```yaml
watches:
  backend-src:
    paths:
      - "./backend/src"
    services:
      - "api"
      - "worker"
```

因为这会立即带来：

- 新的顶层命名空间
- `watch -> service` 的引用校验
- 多 service 重启顺序定义
- 与 `depends_on`、hooks、日志的更多耦合

## 11. 实现分阶段建议

### Phase 1：schema 与校验

- 在 `ServiceConfig` 中增加 `watch: Option<ServiceWatchConfig>`
- 支持 YAML 解析、序列化与 `check`
- 补充相对路径解析与错误提示测试

### Phase 2：单 service watcher 接入

- 为已启动的 service 创建 watcher
- 打通 `paths` + `debounce_ms`
- 变更后触发当前 service 的 stop/start

### Phase 3：日志、事件与并发保护

- 把 watch 状态写入实例日志与 `events.jsonl`
- 实现单飞重启与 dirty 标记
- 全局退出时正确停止 watcher

## 12. 验收标准建议

至少覆盖以下行为：

1. 配置 `services.api.watch.paths` 后可通过解析与 `check`
2. 相对路径按配置目录解析
3. 目录与文件都可被接受
4. 路径不存在时，`check` 明确失败
5. service 运行中修改 watch 目标，会触发该 service 重启
6. 多次连续修改只触发有限次重启，不发生抖动风暴
7. watch 重启会正常经过 stop/start hooks
8. `after_runtime_exit_unexpected` 不会因 watch 主动重启而触发
9. `down` 期间不会被 watcher 再次拉起 service

## 13. 推荐结论

本需求建议按“service 自身运行策略”建模，而不是按“全局资源”建模。

因此首版推荐：

1. 在 `services.<name>` 下新增 `watch`
2. 首版只支持 `paths` + `debounce_ms`
3. 目录默认递归监控
4. 复用现有 stop/start、hooks、日志与事件模型
5. 明确不把 `depends_on` 扩展为运行期联动语义

这样可以用最小 schema 变化换来足够高的开发期价值，同时给未来更复杂的全局 watch 模型保留空间。
