# `--tui` service 重启开发任务拆分

## 1. 目的

本文档把 `/Users/wolf/RustroverProjects/onekey-run-rs/docs_design/14_tui_service_restart_design.md` 收敛成一份可以直接执行的开发任务清单。

目标不是再次讨论 TUI 是否应该支持手动重启，而是明确：

- 先改哪些文件
- 每一步要交付什么
- 什么叫“做完”
- 需要补哪些测试

本专题的最终目标是：

- 在 `onekey-run-rs --tui` 中选中某个 service 后按 `R`
- 仅重启该 service
- 复用现有 stop/start、hooks、runtime state 与 events 语义
- 不破坏当前 `watch` 自动重启路径

## 1.1 当前执行状态

截至当前这一轮实现，本专题核心任务已完成。

- Task 1：`done`
- Task 2：`done`
- Task 3：`done`
- Task 4：`done`
- Task 5：`done`
- Task 6：`done`
- Task 7：`done`

## 2. 范围与非目标

### 2.1 本次范围

- 为单 service 重启抽出通用 orchestrator helper
- 让 `watch` 与 TUI 共用同一重启入口
- 在 TUI 中增加 `R` 快捷键
- 为手动重启补齐 notice、events 与 stop reason 语义
- 处理 TUI 手动重启与 `watch pending` 的最小协调
- 补齐对应测试

### 2.2 非目标

- 不实现“重启全部 services”
- 不实现依赖链级联重启
- 不新增弹窗确认或快捷键自定义
- 不在本期实现真正异步后台 restart job
- 不在 TUI 中直接编辑配置

## 3. 任务完成定义

以下条件同时满足，视为本专题完成：

1. `watch` 触发的单 service 重启逻辑已收敛到通用 helper
2. TUI 中按 `R` 能重启当前选中的 service
3. 若 service 当前未运行，`R` 会按设计直接把它重新启动
4. 手动重启会复用现有 stop/start hooks，而不是旁路逻辑
5. `.onekey-run/events.jsonl` 中可区分 restart request / success / skip
6. 实例收到 shutdown 信号后，TUI 不会再插入新的 restart
7. 手动重启成功后，同 service 已挂起的 watch debounce 请求不会立刻再次触发重复重启
8. 相关单元测试与回归测试已落地

## 4. 推荐实施顺序

建议严格按下面顺序推进，避免一开始就把 UI 交互和生命周期重构搅在一起：

1. 先抽出 orchestrator 通用 restart helper
2. 再把现有 `watch` 重启迁移到新 helper
3. 补 `watch pending` 清理接口
4. 改造 TUI 输入模型
5. 接入 `R` 重启与 notice
6. 补 restart 事件与颜色表现
7. 最后补测试和文档收尾

## 5. 任务拆分

### Task 1：抽出通用单 service 重启 helper

状态：`done`

#### 目标

把当前 `watch` 专用的重启路径抽象成 orchestrator 通用能力，避免 TUI 再复制 stop/start 细节。

#### 需要修改的文件

- `src/orchestrator.rs`

#### 具体改动

- 从当前 `restart_service_from_watch(...)` 收敛出新的通用 helper，例如：
  - `ServiceRestartTrigger`
  - `restart_service(...)`
- 通用 helper 需要统一处理：
  - 根据 trigger 生成 restart detail
  - 判断 service 是否存在
  - 判断 shutdown 是否已开始
  - 若 service 正在运行则优雅停止
  - 更新 `running`
  - 更新 `runtime_state.services`
  - 调 `start_service(...)` 重新拉起
- stop reason 明确区分：
  - `watch_restart`
  - `tui_restart`

#### 完成标准

- orchestrator 中存在一个独立、可复用的单 service 重启入口
- 该入口不依赖 TUI 类型
- 该入口本身就能表达 `watch` 与 `tui` 两种触发来源

#### 注意事项

- 不要让 `src/tui.rs` 直接调用 `stop_spawned_process(...)` + `start_service(...)`
- 不要把 restart 逻辑散落在多个调用点里

### Task 2：把 `watch` 重启迁移到通用 helper

状态：`done`

#### 目标

确保现有 `watch` 逻辑不回归，并成为新 helper 的第一个调用方。

#### 需要修改的文件

- `src/orchestrator.rs`

#### 具体改动

- 让 `WatchRuntime::tick(...)` 不再直接依赖旧的 `restart_service_from_watch(...)`
- 若保留旧函数，则它只能成为对通用 helper 的薄封装
- 保持当前 `watch` 的行为不变：
  - debounce 到期后触发重启
  - 仍只重启当前 service
  - 仍输出 plain 模式下的 watch notice

#### 完成标准

- `watch` 功能继续可用
- `watch` 重启与 TUI 重启最终走到同一条 stop/start 主路径

#### 推荐验收测试

- 原有 watch 重启测试全部继续通过
- `watch` 触发后仍会更新 `runtime_state`

### Task 3：补齐 restart 事件模型与 `watch pending` 清理接口

状态：`done`

#### 目标

把“这是一次 restart 而不是普通 start”显式写入事件流，并处理手动重启与 debounce 的冲突。

#### 需要修改的文件

- `src/orchestrator.rs`
- 如有需要可轻微调整 `src/tui.rs`

#### 具体改动

- 在 restart helper 中新增显式事件：
  - `service_restart_requested`
  - `service_restart_succeeded`
  - `service_restart_skipped`
- 建议 `detail` 至少带：
  - `trigger=tui|watch`
  - `key=R` 或 `path=<changed_path>`
  - `reason=<...>`（仅 skip 时）
- 在 `WatchRuntime` 中新增最小接口，例如：
  - `clear_pending(service_name: &str)`
- 当 TUI 手动重启某个 service 成功后，清理该 service 的 pending watch restart

#### 完成标准

- `events.jsonl` 能稳定表达 restart request / success / skip
- 同一 service 手动重启后，不会立即因旧 debounce 记录再次重启

#### 注意事项

- 不要清掉其它 service 的 pending watch 请求
- 不要把 restart event 取代已有 `service_stopping` / `service_running` 事件；两者应并存

### Task 4：把 TUI 键盘处理改造成命令式返回

状态：`done`

#### 目标

为 `R` 重启接入运行时控制留出干净入口，而不是继续把 `handle_event(...)` 限制成单一布尔值。

#### 需要修改的文件

- `src/tui.rs`

#### 具体改动

- 把当前：
  - `DashboardState::handle_event(...) -> bool`
- 改成命令式返回，例如：
  - `DashboardCommand::None`
  - `DashboardCommand::Exit`
  - `DashboardCommand::RestartSelectedService`
- `DashboardState` 继续只负责：
  - 输入解析
  - 选中态切换
  - 滚动
  - notice 文本更新
- `run_dashboard(...)` 外层循环统一消费 `DashboardCommand`

#### 完成标准

- TUI 输入层已经能表达“退出”和“重启当前 service”两种命令
- 方向键、Tab、滚动等现有交互不回归

#### 推荐验收测试

- `q` / `Esc` 仍映射为 `Exit`
- `R` 映射为 `RestartSelectedService`
- 数字键切换选中 service 的行为不变

### Task 5：在 TUI 主循环中接入 `R` 重启当前 service

状态：`done`

#### 目标

把 `R` 真正打通到 orchestrator 通用 helper。

#### 需要修改的文件

- `src/tui.rs`
- `src/orchestrator.rs`

#### 具体改动

- 在 `run_dashboard(...)` 事件循环中消费 `DashboardCommand::RestartSelectedService`
- 获取当前选中的 `service_name`
- 调用通用 `restart_service(...)`
- 将 trigger 标记为 `tui`
- 在成功后更新 notice：
  - `service <name> restarted`
- 在失败后更新 notice：
  - `failed to restart <name>: ...`
- 在 restart 开始前先设置：
  - `restarting <name>...`

#### 完成标准

- TUI 中按 `R` 能真正重启当前选中的 service
- service 的 pid、状态和日志管道仍能正常工作
- service 若当前未运行，按 `R` 也能重新进入运行态

#### 注意事项

- 若全局 shutdown 已开始，应拒绝本次 restart 并显示 skip notice
- 不要在 TUI 层自行拼装 stop/start 流程

### Task 6：补 TUI footer / events 呈现与 restart 可见性

状态：`done`

#### 目标

让用户在界面里知道 `R` 存在，并能从 events 面板中快速分辨 restart 过程。

#### 需要修改的文件

- `src/tui.rs`

#### 具体改动

- 更新 footer 帮助文案，补上：
  - `R restart service`
- 调整 events 面板颜色规则，让 restart 事件更直观：
  - `service_restart_requested` 归入 started/requested 色
  - `service_restart_succeeded` 归入 success 色
  - `service_restart_skipped` 归入 warn / failed 色
- 如有需要，微调 notice 与 selected service detail 区，保证重启反馈可见

#### 完成标准

- 用户进入 TUI 后能看见 `R` 的提示
- restart 事件在 Events 面板中可直观识别

#### 注意事项

- 首版不强制引入 `ServiceStatus::Restarting`
- 不要为视觉优化重构整个 TUI 布局

### Task 7：补齐 orchestrator / TUI 回归测试与文档索引

状态：`done`

#### 目标

让本专题具备稳定回归能力，并把新增任务文档接入目录索引。

#### 需要修改的文件

- `src/orchestrator.rs` 中现有测试模块
- 如有需要可为 `src/tui.rs` 增加测试
- `docs_dev/README.md`

#### 具体改动

- 为 orchestrator 补至少以下测试：
  - TUI trigger 重启运行中 service
  - TUI trigger 重启已停止 service
  - shutdown 中 TUI restart 被拒绝
  - start 失败时产生 `service_restart_skipped`
  - 手动重启成功后会清理同 service 的 pending watch restart
- 为 TUI 输入层补至少以下测试：
  - `R` -> `RestartSelectedService`
  - `q` / `Esc` -> `Exit`
- 更新 `docs_dev/README.md`，加入本专题索引

#### 完成标准

- 本专题核心路径具备自动化测试覆盖
- 文档目录中能看到本 task 文档

## 6. 最小可交付切片建议

如果要尽快落第一版，建议按下面三个切片提交：

1. 切片 A：
   Task 1 + Task 2
   先收敛通用 restart helper，并确保 watch 全量回归
2. 切片 B：
   Task 4 + Task 5
   再接入 TUI `R` 的主功能
3. 切片 C：
   Task 3 + Task 6 + Task 7
   最后补 restart 事件、pending 清理、可见性和测试

这样做的好处是：

- 每个切片都能单独验证
- 出问题时容易判断是生命周期层还是 UI 层
- 不会把 watch 回归和 TUI 新交互混成一个超大提交

## 7. 当前建议结论

这个专题最关键的不是“把 `R` 键绑上去”，而是先把现有单 service 重启能力抽象成 orchestrator 公共能力。

推荐执行顺序仍然是：

1. 先统一 restart helper
2. 再迁移 watch
3. 再接入 TUI `R`
4. 最后补可见性与测试

这样落地成本最低，语义最一致，后续若要给 panel / agent 增加同类重启能力，也能直接复用这条链路。
