# `services.<name>.watch` 开发任务拆分

## 1. 目的

本文档把 `/Users/wolf/RustroverProjects/onekey-run-rs/docs_design/13_service_watch_design.md` 收敛成一份可以直接执行的开发任务清单。

目标不是再次讨论 watch 该放在哪里，而是明确：

- 先改哪些文件
- 每一步要交付什么
- 什么叫完成
- 需要补哪些测试

本专题的最终目标是：

- 在 `onekey-tasks.yaml` 中支持 `services.<name>.watch`
- 可监控文件或目录变化
- 变化后自动重启对应 service
- 复用现有 stop/start、hooks、日志与事件流
- 不引入顶层全局 `watches`

## 1.1 当前执行状态

截至当前这一轮实现，本专题核心任务已完成。

- Task 1：`done`
- Task 2：`done`
- Task 3：`done`
- Task 4：`done`
- Task 5：`done`
- Task 6：`done`
- Task 7：`done`
- Task 8：`done`

## 2. 范围与非目标

### 2.1 本次范围

- 在 `ServiceConfig` 下增加 `watch` 配置
- 支持 `paths` + `debounce_ms`
- 支持文件与目录监控
- 相对路径按配置目录解析
- 变更后重启当前 service
- watch 重启复用 stop/start hooks
- 将 watch 状态写入实例日志与 `events.jsonl`
- 补齐校验、运行时和回归测试

### 2.2 非目标

- 不新增顶层全局 `watches`
- 不支持一条 watch 规则同时重启多个 service
- 不支持 `ignore` / `enabled` / `restart_target` 等扩展字段
- 不把 `depends_on` 扩展成运行期联动重启语义
- 不在本期实现配置热重载

## 3. 任务完成定义

以下条件同时满足，视为本专题完成：

1. `services.<name>.watch` 可被 YAML 正确解析、序列化与 `check`
2. `watch.paths` 支持文件和目录，且相对路径按配置目录解析
3. watch path 不存在或非法时，`check` 能稳定报错
4. service 启动后，配置的 watch 能开始工作
5. watch 触发后，仅重启对应 service 本身
6. watch 重启会经过现有 stop/start hooks
7. watch 不会因实例日志、service 日志或 `.onekey-run/` 内部文件变化而自触发循环
8. 连续文件变更不会导致无限重启风暴
9. `down` / Ctrl-C / 全局退出期间不会再被 watch 拉起 service

## 4. 推荐实施顺序

建议按以下顺序推进，避免一开始就把运行时并发问题和 schema 变更耦在一起：

1. 先补 schema 与校验
2. 再补 resolved model 与 run plan 透传
3. 抽出 watcher 后端与事件通道
4. 接入单 service watch 重启主流程
5. 补日志、事件与内置排除
6. 再补单飞重启、dirty 标记与退出保护
7. 最后补测试和相关文档

## 5. 任务拆分

### Task 1：为 service 增加 `watch` schema 与校验

状态：`done`

#### 目标

先把配置模型冻结，让 `check` 能理解 `services.<name>.watch`。

#### 需要修改的文件

- `src/config.rs`
- 如有需要可补充 `/Users/wolf/RustroverProjects/onekey-run-rs/onekey-tasks.yaml` 示例

#### 具体改动

- 在 `src/config.rs` 中新增：
  - `ServiceWatchConfig`
  - 如有需要新增 `ResolvedServiceWatchConfig`
- 在 `ServiceConfig` 中增加：
  - `watch: Option<ServiceWatchConfig>`
- 建议字段：
  - `paths: Vec<PathBuf>`
  - `debounce_ms: Option<u64>`
- 复用现有 serde 风格：
  - `#[serde(default, skip_serializing_if = "Option::is_none")]`
  - `#[serde(default, skip_serializing_if = "Vec::is_empty")]`
- 在 `ProjectConfig::validate(...)` 中补以下规则：
  - `watch` 若存在必须是 object
  - `watch.paths` 必须为非空数组
  - `watch.paths[*]` 不能为空
  - 解析后路径必须存在
  - 必须是文件或目录
  - `debounce_ms` 若存在必须大于 `0`
  - 同一 service 解析后的 watch path 重复时应报错或稳定去重

#### 完成标准

- `serde_yaml::from_str::<ProjectConfig>(...)` 能正确解析合法 `watch`
- `ProjectConfig::validate(...)` 会拒绝非法 `watch`
- 旧配置不写 `watch` 时完全兼容

#### 推荐验收测试

- `services.api.watch.paths` 为空数组时报错
- `services.api.watch.paths[0]` 指向不存在路径时报错
- `services.api.watch.debounce_ms: 0` 报错
- 相对路径在配置目录下存在时可通过校验

### Task 2：把 watch 解析结果接入 resolved model 与运行计划

状态：`done`

#### 目标

让运行时不再直接依赖原始 YAML 字段，而是消费已解析好的绝对路径与默认值。

#### 需要修改的文件

- `src/config.rs`
- `src/orchestrator.rs`

#### 具体改动

- 在 `ResolvedServiceConfig` 中增加：
  - `watch: Option<ResolvedServiceWatchConfig>`
- 在 `resolve_service(...)` 中：
  - 把 watch path 解析为绝对路径
  - 把 `debounce_ms` 填成稳定默认值
- 明确默认值建议：
  - `debounce_ms` 默认 `500`
- 让 `RunPlan.services[*]` 能拿到 resolved watch 配置

#### 完成标准

- `build_run_plan(...)` 产出的 service 已包含 watch 运行时所需全部信息
- 后续 orchestrator 不需要再自己解析相对路径或默认值

#### 注意事项

- 不要把 watcher 运行时状态写回 `RuntimeState`
- `RuntimeState` 仍只存“当前实例”和“当前 service 进程”状态

### Task 3：引入文件监听后端与最小抽象

状态：`done`

#### 目标

在不碰 orchestrator 主流程的前提下，先把“文件系统事件 -> 内部消息”的桥打通。

#### 需要修改的文件

- `Cargo.toml`
- `src/main.rs`
- 建议新增 `src/watch.rs`

#### 具体改动

- 选择并接入跨平台文件监听实现，建议使用 `notify` crate 或等价方案
- 在 `src/watch.rs` 中封装最小能力：
  - 注册某个 service 的 watch paths
  - 接收文件系统事件
  - 输出归一化内部消息，例如：
    - `service_name`
    - `changed_path`
    - `timestamp`
- 不在这一层直接做 stop/start
- 尽量把第三方 crate API 隔离在 `src/watch.rs` 中，避免蔓延进 orchestrator

#### 完成标准

- 可以为一个 service 建立 watcher
- 文件或目录变化时，内部能收到稳定消息
- watcher 代码与业务重启逻辑解耦

#### 推荐验收测试

- 监听临时目录中的文件变更时能收到事件
- 监听单文件时能收到事件
- 多个 path 注册后不会 panic

### Task 4：在 `run_up(...)` 生命周期中接入 watch 重启流程

状态：`done`

#### 目标

让已启动 service 在运行期真正具备“文件变化 -> 重启自己”的能力。

#### 需要修改的文件

- `src/orchestrator.rs`
- 如有需要可补 `src/process.rs`

#### 具体改动

- 在 service 成功启动后，若其配置了 `watch`，为它注册 watcher
- 在 orchestrator 主循环中消费 watch 消息
- 把 watch 事件转换为“目标 service 的重启请求”
- 首版只允许重启当前 service，不联动其他 service
- 建议把“单个 service 的停止 + 再启动”抽成可复用 helper，避免和全量 `up` 启动路径复制逻辑

#### 完成标准

- watch 事件能触发对应 service 重启
- 未配置 watch 的 service 行为保持不变
- 一个 service 的 watch 不会误重启其他 service

#### 注意事项

- 不要偷偷把 `depends_on` 变成运行期联动语义
- 不要为 watch 另造一套独立 hook 体系

### Task 5：让 watch 重启复用现有 stop/start hooks 与原因语义

状态：`done`

#### 目标

把 watch 触发的重启明确纳入现有生命周期，而不是另造“热重载特例”。

#### 需要修改的文件

- `src/orchestrator.rs`
- 视实现情况可补 `src/process.rs`

#### 具体改动

- watch 触发重启时：
  - 先走停止路径
  - 再走启动路径
- 明确 hook 语义：
  - `before_stop` 正常执行
  - `after_stop_success` / `after_stop_timeout` / `after_stop_failure` 正常执行
  - `before_start` / `after_start_success` / `after_start_failure` 正常执行
  - `after_runtime_exit_unexpected` 不应触发
- 给停止原因传入稳定值：
  - 建议 `${stop_reason}` = `watch`

#### 完成标准

- 用户已有 hooks 时，watch 重启能复用这些 hooks
- watch 主动重启不会被误记成异常退出

#### 推荐验收测试

- 配置 `before_stop` action 时，watch 重启会执行该 action
- `after_runtime_exit_unexpected` 不会因 watch 重启而执行
- `before_stop` 中可以看到 `stop_reason=watch`

### Task 6：补 watch 事件、实例日志与自触发保护

状态：`done`

#### 目标

让 watch 具备可观测性，同时避免最常见的“日志写入又触发 watch”死循环。

#### 需要修改的文件

- `src/orchestrator.rs`
- `src/runtime_state.rs`
- 如有需要可补 `src/watch.rs`

#### 具体改动

- 为 watch 增加事件类型，建议至少包括：
  - `watch_triggered`
  - `watch_debounced`
  - `watch_restart_requested`
  - `watch_restart_skipped`
- 若配置了顶层实例 `log`，把这些事件写入实例日志
- 把这些事件同步写入 `.onekey-run/events.jsonl`
- 运行时增加内置排除，至少忽略：
  - `.onekey-run/`
  - 顶层 `log.file`
  - 任意 `service.log.file`

#### 完成标准

- 用户能从实例日志或 `events.jsonl` 看见 watch 的触发与重启摘要
- watch 不会因为内部状态文件或日志文件变化而陷入自触发循环

#### 推荐验收测试

- 写入 service log 文件不会触发 watch 重启
- 写入 `.onekey-run/events.jsonl` 不会触发 watch 重启
- watch 触发后 `events.jsonl` 中能看到新增事件

### Task 7：补单飞重启、防抖和全局退出保护

状态：`done`

#### 目标

解决 watch 真正落地后最容易出现的三个问题：抖动、并发重启、退出时误拉起。

#### 需要修改的文件

- `src/orchestrator.rs`
- `src/watch.rs`

#### 具体改动

- 实现 `debounce_ms`
- 对同一 service 增加单飞重启保护：
  - 同时只允许一个 watch 重启流程在跑
- 若重启期间又收到新事件，建议只记一个 dirty 标记
- 当前重启结束后，若 dirty 仍为真，再补一次重启
- 当实例收到 `down` / Ctrl-C / 全局退出信号时：
  - watcher 停止接收新事件
  - 队列中的 watch restart 请求被丢弃

#### 完成标准

- 连续保存多次文件，不会产生无限排队的 restart
- service 重启进行中再次改文件，不会并发跑第二套 stop/start
- 退出阶段不会再被 watch 拉起 service

#### 推荐验收测试

- 短时间内多次改文件，最终只触发有限次重启
- 重启过程中再次改文件，行为符合 dirty 补一次的预期
- `down` 期间修改文件，不会再次启动 service

### Task 8：补全回归测试与文档更新

状态：`done`

#### 目标

让 watch 变更具备稳定回归面，避免后续演进时悄悄破坏。

#### 需要修改的文件

- `src/config.rs`
- `src/orchestrator.rs`
- `src/runtime_state.rs`
- `docs_dev/03_config_schema.md`
- `docs_dev/04_runtime_contract.md`
- `docs_dev/10_logging_design.md`
- 如有需要可更新 `/Users/wolf/RustroverProjects/onekey-run-rs/onekey-tasks.yaml`

#### 具体改动

- 为 schema、路径解析、默认值和错误提示补测试
- 为 watch 重启、hook 复用、日志事件、自触发保护补测试
- 把 watch 字段写回配置契约文档
- 把 watch 重启时序写回运行时契约文档
- 把 watch 事件与日志行为写回日志设计文档

#### 完成标准

- watch 相关测试进入常规测试集
- `docs_dev` 中相关契约文档已同步更新
- 示例配置能体现最小 watch 用法

## 6. 推荐提交拆分

建议按以下粒度提交，减少 review 压力：

1. schema + validate + tests
2. resolved model + watcher backend skeleton
3. orchestrator watch restart 主流程
4. 日志事件 + 自触发保护
5. 单飞重启 + dirty + 退出保护
6. 文档与剩余测试收尾

## 7. 风险提示

实现期最容易踩坑的点有四个：

1. 文件系统事件风暴
   保存一次文件可能收到多条底层事件，必须有 debounce 与单飞保护。
2. 日志/状态文件自触发
   如果 watch 仓库根目录，又不排除 `.onekey-run/` 与日志文件，很容易进入重启循环。
3. hooks 重复语义
   若单独实现一条“watch 专用重启流程”，很容易与现有 stop/start hooks 分叉。
4. 退出阶段竞争
   若全局退出时 watcher 还在产生日志和重启请求，会导致 service 被错误重新拉起。

## 8. 推荐结论

这项功能建议按“先冻结 schema，再接运行时，再补并发保护”的顺序推进。

优先级上应先完成：

1. Task 1：schema 与校验
2. Task 2：resolved model
3. Task 4：watch 重启主流程

因为这三步做完后，`services.<name>.watch` 就已经具备第一批真实可用价值；剩余任务则是在此基础上把行为变稳、变可观测、变可维护。
