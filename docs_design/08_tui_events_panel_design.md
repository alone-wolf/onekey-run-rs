# `--tui` events 面板设计

## 1. 目标

当前 TUI 已能看服务运行情况和日志。

下一步希望补一个 events 面板，用来展示 orchestrator 级事件，而不是业务 stdout/stderr：

- service 什么时候开始启动
- 哪个 hook 开始/结束/失败
- 哪个 action 超时或失败
- 哪个 service 运行期异常退出

这能补齐“控制面事件”和“业务日志”之间的空白。

## 2. 核心设计结论

建议把 events 作为一个独立 tab，而不是混进 service 日志 tab。

推荐布局：

1. 顶部：实例/服务总览
2. 中部：tabs
   - `Overview`
   - `Logs`
   - `Events`
3. 下部：当前选中区域内容

## 3. 为什么单独做 Events Tab

- 事件是 orchestrator 级语义，不属于任何单个 stdout/stderr 流
- hook / action 失败信息很容易被普通日志淹没
- 后续若要加筛选、颜色、高亮，也更适合单独面板

## 4. 数据来源建议

首版建议直接复用当前内存事件流 + `.onekey-run/events.jsonl`：

- 运行中的事件：由 orchestrator 直接推送到 TUI
- 启动前历史：可选从 `.onekey-run/events.jsonl` 回放最近若干条

如果当前 TUI 管道还没承载内部事件，首版也可以只读 `events.jsonl`。

## 5. 建议展示字段

每条 event 至少展示：

- 时间
- event type
- service
- hook
- action
- detail

例如：

```text
21:33:01 | hook_started | api | before_start | - | started with 1 action(s)
21:33:01 | action_started | api | before_start | prepare-env | started
21:33:02 | action_failed | api | before_start | prepare-env | exited with status 1
```

## 6. 交互建议

首版建议只做最小交互：

- `Tab` / `Shift-Tab`
  切换主 tabs
- 上下方向键
  滚动 events 列表
- `g` / `G`
  跳转顶部 / 底部

当前不必一开始就做复杂搜索。

## 7. 视觉语义建议

建议按事件类型做轻量高亮：

- `*_failed`
  红色
- `*_timeout`
  黄色
- `*_started`
  蓝色
- `*_finished` / `service_running` / `service_stopped`
  绿色

这样用户能快速扫到异常。

## 8. 事件过滤建议

首版建议先提供最简单两种过滤：

- `All`
- `Current Service`

如果当前选中了某个 service，则 `Current Service` 只显示该 service 相关事件。

后续再考虑：

- 仅失败事件
- 仅 hooks
- 仅 actions

## 9. 缓存与容量建议

TUI 不建议无限堆积事件。

建议首版：

- 内存只保留最近 `500` 或 `1000` 条 events
- 若事件文件更长，不必全部载入内存

原因：

- 保持 UI 流畅
- 避免 watch 久了内存持续膨胀

## 10. 与日志面板的关系

建议明确分工：

- `Logs`
  展示业务进程 stdout/stderr
- `Events`
  展示 orchestrator 级生命周期事件

这样用户能分清：

- “服务打印了什么”
- “编排器做了什么”

## 11. 失败信息呈现建议

对于 `hook_failed` / `action_failed` / `service_runtime_exit_unexpected`，建议：

- 列表中直接展示简短 detail
- 选中某条后，可在底部状态栏显示完整 detail

首版如果不想增加复杂布局，也可以先仅展示单行 detail。

## 12. 实现落点建议

大概率涉及：

- `src/tui.rs`
- `src/orchestrator.rs`
- `src/runtime_state.rs`

建议拆分：

1. 在 TUI 状态里新增 `events` 缓冲区
2. 增加 `Events` tab 与渲染函数
3. 增加内部事件到 TUI 状态的接入

## 13. 分阶段实施建议

### Phase 1

- 新增 `Events` tab
- 读取并显示最近事件
- 支持基本滚动

### Phase 2

- 增加 `All` / `Current Service` 过滤
- 增加事件类型颜色

### Phase 3

- 增加失败事件快速跳转
- 增加底部 detail 区

## 14. 风险点

- TUI 现有日志刷新节奏与事件刷新节奏要协调
- 若直接从文件轮询读取，需避免重复载入同一批 events
- 事件行过长时需要稳定截断，不要撑坏布局

## 15. 当前建议结论

最推荐的第一步是：

- 给 TUI 增加独立 `Events` tab
- 先显示最近一批 orchestrator 事件
- 用轻量颜色区分 started / finished / failed / timeout

这样能最快把 `.onekey-run/events.jsonl` 的价值在界面里体现出来。
