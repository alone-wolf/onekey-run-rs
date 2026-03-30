# `--tui` service 重启交互设计

## 1. 背景

当前 `onekey-run-rs --tui` 已具备：

- service 列表、状态、日志与 events 展示
- `q` / `Esc` 退出并停止全部 services
- `watch` 触发后的单 service 重启

但仍缺少一个最直接的运维交互：

- 在 TUI 中选中某个 service
- 按 `R`
- 仅重启该 service

这类能力本质上属于“实例内控制面动作”，不应要求用户退出 TUI 再手动重启整个实例。

## 2. 目标与非目标

### 2.1 目标

本设计希望做到：

1. 在 `--tui` 中选中 service 后按 `R` 即可重启该 service
2. 重启流程复用现有 stop/start、hook、runtime state、events 语义
3. 不影响当前 `watch` 自动重启路径
4. 重启结果能够在 TUI notice、service 状态和 events 中体现
5. 实现改动尽量收敛，避免再造一套独立生命周期

### 2.2 非目标

首版不做：

- 一键重启全部 services
- 级联重启依赖它的 downstream services
- 弹窗确认、复杂快捷键映射配置
- 真正异步后台任务队列式的重启执行器
- 在 TUI 中直接修改配置

## 3. 现状分析

### 3.1 TUI 当前结构

`src/tui.rs` 当前 `run_dashboard(...)` 持有：

- `RunPlan`
- `running: &mut Vec<SpawnedProcess>`
- `runtime_state: &mut RuntimeState`
- `watch_runtime: Option<&mut WatchRuntime>`
- `ShutdownController`

事件循环每轮会：

1. 同步 `running` 到面板状态
2. 读取日志与事件
3. 处理 `watch_runtime.tick(...)`
4. 检查 service 是否意外退出
5. 绘制 UI
6. 消费键盘事件

这意味着 TUI 自身已经拿到了执行“单 service 重启”所需的全部运行时上下文。

### 3.2 当前已存在的重启能力

`src/orchestrator.rs` 已有 `restart_service_from_watch(...)`，其语义是：

1. 发出 `watch_restart_requested`
2. 停止目标 service
3. 从 `running` 中移除旧进程并更新 `runtime_state`
4. 调 `start_service(...)` 重新拉起
5. 失败时发出 `watch_restart_skipped`

其 stop/start 已复用现有主流程，因此天然继承：

- `before_stop`
- `after_stop_success`
- `after_stop_timeout`
- `after_stop_failure`
- `before_start`
- `after_start_success`
- `after_start_failure`
- `.onekey-run/state.json`
- `.onekey-run/events.jsonl`

这说明 TUI 手动重启不应新写第三套逻辑，而应复用并泛化这条路径。

## 4. 核心设计结论

建议新增一个“通用单 service 重启 helper”，由：

- `watch`
- TUI `R`

共同调用。

推荐抽象：

```text
restart_service(...)
  <- watch trigger
  <- tui key trigger
```

不要让 `src/tui.rs` 直接复制 stop/start 细节，也不要把“手动重启”写成单独的旁路状态机。

## 5. 交互语义建议

### 5.1 按键

建议在 TUI 中新增：

- `R`
  重启当前选中的 service

这里建议使用大写 `R`，避免与未来可能的其他单键语义冲突。

### 5.2 用户可见反馈

按下 `R` 后：

1. footer notice 立即显示 `restarting <service>...`
2. 若重启成功，notice 更新为 `service <name> restarted`
3. 若重启失败，notice 更新为 `failed to restart <name>: ...`

同时在 Events 面板中能看到显式事件。

### 5.3 对未运行 service 的处理

建议首版采用“只要该 service 在当前 plan 中存在，就允许 `R`”的策略。

也就是说：

- 若 service 当前正在运行：执行真正的 restart
- 若 service 当前已停止或失败：执行“重新启动到运行态”

原因：

1. 对用户来说，`R` 的核心意图是“把这个 service 拉回可用态”
2. 当前 watch 重启路径本身已经支持“找不到运行中进程时直接 start”
3. 若失败后必须改成另一套 `S` / `Enter` 语义，交互会变碎

但在事件命名上仍建议统一归类为 `service_restart_*`，因为用户发起的是 restart 意图，而不是普通冷启动。

### 5.4 与依赖关系的边界

建议首版只重启目标 service，不联动其依赖或依赖它的服务。

原因：

1. 现有 `watch` 也是单 service 重启
2. 级联重启会显著放大状态机复杂度
3. 用户在 TUI 中选中某个 service 时，直觉上更接近“只操作这一项”

文档中应明确：若该 service 被其它 service 依赖，短暂中断由用户自行承担。

## 6. 运行时语义

### 6.1 建议事件模型

在现有 `service_stopping` / `service_stopped` / `service_running` 之外，建议增加显式 restart 事件：

- `service_restart_requested`
- `service_restart_succeeded`
- `service_restart_skipped`

建议 detail 至少带上：

- `trigger`
  如 `tui` / `watch`
- 触发补充信息
  如 `path=<changed_path>` 或 `key=R`

示例：

```text
service_restart_requested  svc=api  detail=trigger="tui" key="R"
service_restart_succeeded  svc=api  detail=trigger="tui"
service_restart_skipped    svc=api  detail=trigger="tui" reason="start_failed" ...
```

这样有两个好处：

1. Events 面板里能直接区分“普通启动”和“人为重启”
2. 后续若 panel / agent 也接入同类能力，事件语义可复用

### 6.2 Stop reason 建议

当 restart 需要先停旧进程时，建议 `stop_reason` 明确区分来源：

- watch 触发：`watch_restart`
- TUI 触发：`tui_restart`

不要继续使用宽泛的 `watch` 或 `shutdown`，否则 hook 中难以判断真实来源。

### 6.3 成功 / 失败规则

建议通用 helper 使用以下判定：

1. 若 service 配置不存在：
   发 `service_restart_skipped`
2. 若 service 正在运行且停止失败：
   发 `service_restart_skipped`
3. 若启动失败：
   发 `service_restart_skipped`
4. 若启动成功：
   发 `service_restart_succeeded`

并保持现有 stop/start 事件继续照常产生。

### 6.4 与 shutdown 的关系

若实例已收到全局退出信号，则：

- 拒绝新的 TUI restart 请求
- notice 显示 `shutdown in progress; restart skipped`
- 发 `service_restart_skipped(trigger=tui, reason=shutdown)`

避免在整体停机流程中又插入新的 start。

### 6.5 与 watch pending 的关系

当前 `WatchRuntime` 会维护 `pending` 的 debounce 重启请求。

建议首版规则：

1. TUI 手动重启不清空其它 service 的 pending 请求
2. 若目标 service 自己存在 pending watch restart，则在手动重启成功后清掉该 service 的 pending 项

原因：

- 用户已经手动完成了一次更强意图的重启，短时间内没必要再让同一 service 因旧的 debounce 记录重复重启一次

这意味着 `WatchRuntime` 需要新增一个很小的接口，例如：

- `clear_pending(service_name: &str)`

## 7. TUI 控制流改造建议

### 7.1 不建议继续让 `handle_event(...) -> bool`

当前 `DashboardState::handle_event(...) -> bool` 只能表达“是否退出”。

加入 restart 后，建议改成命令式返回值，例如：

```rust
enum DashboardCommand {
    None,
    Exit,
    RestartSelectedService,
}
```

然后在 `run_dashboard(...)` 主循环中统一处理：

1. 读取键盘事件
2. 由 `DashboardState` 只负责解析 UI 命令
3. 外层根据命令调用 orchestrator 暴露的重启 helper
4. 根据结果更新 notice / 面板状态

这样职责更清晰：

- `DashboardState`
  只关心输入和渲染
- orchestrator helper
  只关心 service 生命周期

### 7.2 为什么不在 `DashboardState` 里直接重启

不建议把 `RunPlan` / `running` / `runtime_state` 都塞进 `DashboardState`，原因：

1. 会让 UI 状态结构体持有过多运行时控制权
2. 会把渲染层和编排层耦合得更死
3. 后续测试更难做

因此推荐保留现有 `DashboardState` 的轻量 UI 定位。

## 8. orchestrator 侧抽象建议

### 8.1 新增通用入口

建议把当前 `restart_service_from_watch(...)` 重构为更通用的接口，例如：

```rust
pub(crate) enum ServiceRestartTrigger {
    Watch { changed_path: PathBuf },
    Tui,
}

pub(crate) fn restart_service(
    plan: &RunPlan,
    service_name: &str,
    trigger: ServiceRestartTrigger,
    running: &mut Vec<SpawnedProcess>,
    runtime_state: &mut RuntimeState,
    shutdown: &ShutdownController,
    output_context: &RuntimeOutputContext,
) -> AppResult<()>
```

其中：

- watch 调用方负责传入 `Watch { changed_path }`
- TUI 调用方传入 `Tui`

### 8.2 旧函数如何收敛

建议：

- 保留 `restart_service_from_watch(...)` 作为很薄的 wrapper，或直接删除
- 让 `WatchRuntime::tick(...)` 直接调用新的 `restart_service(...)`
- 让 TUI 主循环也调用同一个 helper

### 8.3 helper 内部职责

推荐 helper 统一负责：

1. 根据 trigger 发出 `service_restart_requested`
2. 判断 shutdown / service 是否存在
3. 若旧进程存在则优雅停止，使用明确的 `stop_reason`
4. 更新 `running` 与 `runtime_state`
5. 调 `start_service(...)`
6. 发出 `service_restart_succeeded` 或 `service_restart_skipped`
7. 向 plain 输出保留必要提示

这样 watch 与 TUI 的差异只剩“触发来源”和“补充 detail”。

## 9. UI 呈现建议

### 9.1 footer 文案

当前 footer 文案应补上：

- `R restart selected service`

建议完整提示类似：

```text
Up/Down select service | Left/Right/Tab switch panel | j/k or PgUp/PgDn scroll | Home/End jump | R restart service | q/Esc stop
```

### 9.2 service 状态显示

首版不强制新增 `RESTARTING` 状态。

原因：

1. 当前重启流程本身是同步的
2. watch 重启已经采用相同风格
3. 首版先打通动作比引入更多中间态更重要

但建议在 `notice` 和 `events` 中补足过程信息。

如果后续发现 stop timeout 较长、用户感知明显卡顿，再考虑新增：

- `ServiceStatus::Restarting`

以及将 restart 流程改造成异步 job。

### 9.3 Events 高亮

建议把 `service_restart_*` 归入现有配色策略：

- `service_restart_requested`
  蓝色
- `service_restart_succeeded`
  绿色
- `service_restart_skipped`
  黄色或红色

## 10. 测试建议

建议至少补以下测试。

### 10.1 orchestrator 单元 / 集成测试

1. TUI trigger 重启运行中 service：
   断言 stop/start 都发生，`runtime_state` 中 pid 更新
2. TUI trigger 重启已停止 service：
   断言会直接重新启动
3. shutdown 中触发 TUI restart：
   断言返回 skip，不会启动新进程
4. stop 失败：
   断言 `service_restart_skipped`
5. start 失败：
   断言 `service_restart_skipped`
6. watch 与 TUI 共用同一 helper：
   断言 watch 现有行为不回归

### 10.2 TUI 状态测试

1. `R` 能映射为 `RestartSelectedService`
2. `q` / `Esc` 仍映射为 `Exit`
3. 方向键、Tab、滚动键行为不变

## 11. 分阶段实施建议

### Phase 1

- 抽出 orchestrator 通用 `restart_service(...)`
- 让 watch 逻辑迁移到通用 helper
- 补充 restart 事件
- 保证现有 watch 测试通过

### Phase 2

- 改造 `src/tui.rs` 事件处理返回 `DashboardCommand`
- 接入 `R` 重启当前 service
- 更新 footer 文案与 notice
- 补充 TUI 输入映射测试

### Phase 3

- 增加 `WatchRuntime::clear_pending(service_name)`
- 手动重启成功后清理同 service 的 pending watch 重启
- 评估是否需要 `RESTARTING` 中间态

## 12. 涉及文件

大概率涉及：

- `src/tui.rs`
- `src/orchestrator.rs`
- `src/watch.rs`
- `src/runtime_state.rs`

如果事件名和日志格式有调整，也可能轻微影响：

- `docs_design/08_tui_events_panel_design.md`
- 相关测试

## 13. 推荐落地顺序

最推荐的实现顺序是：

1. 先把 watch 专用重启逻辑抽成通用 helper
2. 再让 TUI `R` 调用同一入口
3. 最后处理 watch pending 清理和更细的 UI 状态

这样收益最大：

- 实现最稳
- 与现有语义最一致
- 回归面最小

## 14. 当前建议结论

对这个需求，最合理的规划不是“给 TUI 硬塞一个重启分支”，而是：

- 把现有 watch 的单 service 重启路径抽象成通用 orchestrator 能力
- 让 TUI 的 `R` 只负责发出 `RestartSelectedService` 命令
- 由统一 helper 复用 stop/start/hooks/runtime state/events

这样能在最小改动下得到一致、可测试、可继续扩展到 panel/agent 的 service 重启机制。
