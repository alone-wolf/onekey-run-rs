# `up --tui --keep --manage` 设计

## 1. 背景

当前 `onekey-run up --tui` 的行为是：

- TUI 运行中展示 services / logs / events
- 一旦运行期进入终态，TUI 就退出

这里的“终态”包括：

- 用户在 TUI 中请求退出，随后服务全部停止
- 某个 service 运行期异常退出，随后实例进入失败收尾
- 其它导致 `run_up_tui(...)` 收尾的路径

现在希望把“退出后是否继续留在 TUI”进一步拆成两层语义：

1. `onekey-run up --tui --keep`
   所有 services 退出后，仍保留 TUI 供用户查看最终状态，但默认不允许继续操作
2. `onekey-run up --tui --keep --manage`
   所有 services 退出后，TUI 继续保留，并进入一个常驻控制台语义，允许用户继续进行管理动作

这两个 flag 的差异点不在“是否保留 TUI”，而在：

- 保留后是否只读
- 保留后是否允许继续控制

## 2. 目标与非目标

### 2.1 目标

本设计希望做到：

1. `--keep` 与 `--manage` 形成清晰分层，而不是一个含混的 keep 模式
2. `up --tui --keep` 在终态后保留 TUI，只读查看最终状态
3. `up --tui --keep --manage` 在终态后保留 TUI，并允许继续管理
4. 默认 `up --tui` 行为不变
5. 非 TUI 模式行为不变

### 2.2 非目标

首版不做：

- 不实现 top-level `management` 子命令与 `up --tui --manage` 的完全统一
- 不在本设计中决定所有未来管理动作的完整菜单
- 不支持 keep 策略矩阵，例如 `on-failure` / `always`
- 不把 TUI 变成新的 runtime truth source
- 不在本期设计中加入配置编辑器

## 3. CLI 语义建议

### 3.1 新增参数

建议在 `UpArgs` 中新增：

```text
--keep
--manage
```

建议约束：

- `--keep` requires `--tui`
- `--manage` requires `--keep`
- 因为 `--keep` 已 requires `--tui`，所以 `--manage` 也间接要求 `--tui`
- `--daemon` 与 `--tui` 仍互斥，因此 `--daemon` 与 `--keep` / `--manage` 也天然不兼容

### 3.2 合法 / 非法组合

合法：

- `onekey-run up --tui`
- `onekey-run up --tui --keep`
- `onekey-run up --tui --keep --manage`

非法：

- `onekey-run up --keep`
- `onekey-run up --manage`
- `onekey-run up --tui --manage`
- `onekey-run up --daemon --keep`
- `onekey-run up --daemon --manage`

### 3.3 帮助文案建议

建议：

- `--keep`
  `Keep the TUI open after services stop so final state can be inspected`
- `--manage`
  `After services stop, keep the TUI in interactive management mode`

## 4. 核心设计结论

推荐把 TUI 明确拆成三个阶段，而不是两个：

1. `Running`
   运行期实时监控
2. `PostRunReadonly`
   终态后的只读查看
3. `PostRunManage`
   终态后的常驻管理

它们和 flag 的关系如下：

- 无 flag：
  `Running -> Exit`
- `--keep`：
  `Running -> PostRunReadonly -> Exit`
- `--keep --manage`：
  `Running -> PostRunManage -> Exit`

这能明确回答两个问题：

1. “所有 services 都停了以后，TUI 是否保留？”
   由 `--keep` 决定
2. “保留后是否允许继续操作？”
   由 `--manage` 决定

## 5. 当前结构的约束

当前 `src/orchestrator.rs` / `src/tui.rs` 的结构里：

- `run_dashboard(...)` 自己 enter / run / exit terminal
- `run_up_tui(...)` 在 `run_dashboard(...)` 返回后才做 shutdown / cleanup

这意味着当前结构不适合直接扩展成 keep/manage，原因有两个：

1. TUI 返回之前，还没完成 shutdown / cleanup
2. cleanup 之后，当前 TUI 还依赖 `.onekey-run/events.jsonl` 等运行时文件

因此 keep/manage 不能简单做成：

- `if keep { 不退出 }`

必须先解决：

- terminal session 生命周期
- 运行时文件 cleanup 后的数据来源
- post-run 阶段的命令边界

## 6. 用户可见行为

### 6.1 默认 `--tui`

`onekey-run up --tui`

保持现有语义：

- 运行结束后直接退出 TUI

### 6.2 `--tui --keep`

`onekey-run up --tui --keep`

建议语义：

1. 正常运行 dashboard
2. 进入终态后，完成 shutdown / cleanup
3. TUI 切换到 `PostRunReadonly`
4. 用户仍可浏览 `Overview` / `Logs` / `Events`
5. 默认不允许继续执行管理动作
6. 按 `q` / `Esc` 退出

### 6.3 `--tui --keep --manage`

`onekey-run up --tui --keep --manage`

建议语义：

1. 正常运行 dashboard
2. 进入终态后，完成 shutdown / cleanup
3. TUI 切换到 `PostRunManage`
4. 用户可继续浏览最终状态
5. 用户可继续执行允许的管理动作
6. TUI 成为“当前配置对应实例的常驻控制台”
7. 按 `q` / `Esc` 才退出

## 7. PostRun 两种模式的边界

### 7.1 `PostRunReadonly`

建议允许：

- `Tab` / `Shift-Tab`
- `Left` / `Right`
- `Up` / `Down`
- `j` / `k`
- `PgUp` / `PgDn`
- `Home` / `End`
- `q` / `Esc`

建议不允许：

- `R`
- 任何 start / stop / restart / run action 类型命令

### 7.2 `PostRunManage`

建议允许：

- `PostRunReadonly` 的全部浏览命令
- 明确授权的管理命令

首版最值得保留的管理动作建议先从最小集合开始：

- `R`
  重新启动当前选中 service

也就是：

- `--keep` 不允许 post-run 时操作
- `--keep --manage` 允许 post-run 时操作

这样和你的语义一致，也不会让 `--keep` 变成一个意外可操作模式。

### 7.3 为什么不要让 `--keep` 默认可操作

原因：

1. 用户对 `keep` 的直觉更接近“停住给我看最终状态”
2. 一旦 `keep` 默认可操作，`keep` 和 `manage` 就没有实质区别
3. 只读 / 可操作 分层能降低误操作风险

## 8. 运行时语义

### 8.1 进入 post-run 之前必须完成的动作

无论进入 `PostRunReadonly` 还是 `PostRunManage`，建议都先完成：

1. 停止剩余服务
2. 写完最终 events
3. 关闭 watch runtime
4. 清理 runtime files
5. 释放 lock
6. 从 registry 中注销实例

原因：

- post-run 阶段不应继续占着运行中实例身份
- `management` / `down` 不应误以为实例仍在运行
- `keep` / `manage` 的本质是“保留 UI”，不是“保留运行时资源”

### 8.2 post-run 数据来源

进入 post-run 后，TUI 不应继续依赖：

- `.onekey-run/state.json`
- `.onekey-run/events.jsonl`
- 运行中的 child handle

因为：

- cleanup 后这些文件已删除
- child 也已结束

因此建议在进入 post-run 前做一次显式冻结：

- 最终 service 状态
- 已采集 logs
- 已采集 events
- 当前 selection / scroll
- 最终 summary / notice

### 8.3 `PostRunManage` 的重新进入运行态

这里建议明确：

- `PostRunManage` 允许执行管理动作
- 但这些动作不意味着“恢复旧实例”

建议语义是：

1. TUI 自己继续活着
2. 用户在 post-run 中发起某个管理动作
3. TUI 内部重新创建该动作对应的运行期资源
4. 必要时从 `PostRunManage` 切回 `Running`

也就是说，`manage` 更像：

- “常驻控制台”

而不是：

- “让旧的 orchestrator 实例半死不活地挂着”

## 9. TUI 建模建议

建议把当前 `DashboardState` 的阶段字段设计成：

```rust
enum DashboardPhase {
    Running,
    PostRunReadonly,
    PostRunManage,
}
```

### 9.1 渲染语义

建议在 header / footer 明确显示：

- `mode running`
- `mode post-run`
- `mode manage`

并在 footer 中给出符合当前阶段的 keys。

例如：

- `PostRunReadonly`
  只显示浏览相关 keys
- `PostRunManage`
  在浏览 keys 之外，显示允许的管理 keys，例如 `R`

### 9.2 命令边界

当前 `DashboardCommand` 未来可以扩展为：

- 运行期命令
- post-run 只读命令
- post-run 管理命令

但建议仍保持原则：

- phase 决定当前哪些命令可被触发

## 10. 控制流建议

推荐把当前 TUI 拆成 session + phase 驱动结构，例如：

```rust
struct DashboardSession {
    terminal: TerminalSession,
    state: DashboardState,
}
```

控制流建议收敛为：

```text
run_up_tui(...)
  -> create DashboardSession
  -> live phase
  -> shutdown / cleanup
  -> if !keep:
       exit
  -> freeze final snapshot
  -> if manage:
       post-run manage phase
     else:
       post-run readonly phase
  -> exit
```

### 10.1 为什么必须先 freeze 再进 post-run

因为 post-run 要在 cleanup 之后继续显示；
而 cleanup 后运行时文件已经不存在。

### 10.2 为什么 manage 也建议在 cleanup 之后进入

因为 `manage` 的“常驻”应属于 UI 层，而不是运行时实例层。

如果不先 cleanup：

- registry 会显示一个其实已经不再运行的实例
- lock 会阻止后续合理的管理动作重新拉起
- 生命周期边界会很混乱

## 11. `--manage` 首版建议能力

为了控制复杂度，建议首版 `PostRunManage` 只开放最小管理面：

1. `R`
   启动或重启当前选中 service

建议首版暂不开放：

- 批量启动全部 service
- 停止单 service
- 手动执行 action
- 修改配置

原因：

- 当前已经有 `R` 重启链路
- 用它验证 `manage` 的 phase 切换最直接
- 能避免一下子把 post-run 管理面做成第二套 orchestrator

## 12. 退出码语义

`--keep` / `--manage` 都不应改变命令最终退出码。

也就是说：

- 运行成功后进入 post-run，用户退出时命令返回成功
- 运行失败后进入 post-run，用户退出时命令仍返回失败

如果 `PostRunManage` 中用户又进行了新的管理动作，建议首版明确：

- 最终退出码仍以“最初这次 `up` 的运行结果”为准

不要在首版里把“后续交互动作结果”混进原始 `up` 退出码，否则语义会很难解释。

## 13. 测试建议

### 13.1 CLI 测试

1. `up --keep` 解析失败
2. `up --manage` 解析失败
3. `up --tui --keep` 解析成功
4. `up --tui --manage` 解析失败
5. `up --tui --keep --manage` 解析成功
6. `up --daemon --keep` 解析失败

### 13.2 TUI / orchestrator 测试

1. keep=false 时，运行结束后直接退出
2. keep=true, manage=false 时，进入 `PostRunReadonly`
3. keep=true, manage=true 时，进入 `PostRunManage`
4. `PostRunReadonly` 中 `R` 不生效
5. `PostRunManage` 中 `R` 生效
6. cleanup 后 post-run 仍可看到最终 logs / events
7. 最终退出码不因 keep/manage 改变

## 14. 涉及文件

大概率涉及：

- `src/cli.rs`
- `src/app.rs`
- `src/orchestrator.rs`
- `src/tui.rs`

如需同步说明，也可能影响：

- `docs_design/08_tui_events_panel_design.md`
- 后续 tasks 文档

## 15. 分阶段实施建议

### Phase 1

- 增加 `--keep` / `--manage` CLI 参数及约束
- 在 `RunOptions` 中透传 `keep_tui` / `manage_tui`

### Phase 2

- 把 TUI 重构成 session + phase 结构
- 支持 `Running -> PostRunReadonly / PostRunManage`

### Phase 3

- 实现 post-run 冻结快照
- 在 shutdown / cleanup 后进入对应 phase

### Phase 4

- 给 `PostRunReadonly` 补只读 footer / header
- 给 `PostRunManage` 补最小管理命令边界

### Phase 5

- 首版只在 `PostRunManage` 中开放 `R`
- 验证 post-run manage 能重新回到运行态
- 补测试

## 16. 当前建议结论

这次需求最关键的点是把“保留 TUI”与“允许继续管理”彻底拆开。

推荐的最终语义是：

1. `--tui`
   运行结束就退出
2. `--tui --keep`
   运行结束后保留 TUI，但只读查看
3. `--tui --keep --manage`
   运行结束后保留 TUI，并把它作为常驻控制台使用

这样 `keep` 与 `manage` 的职责清晰，也更容易在实现上收敛成：

- 一个共享的 post-run phase 机制
- 两种不同权限级别的 post-run 模式
