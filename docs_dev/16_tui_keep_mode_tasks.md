# `--tui --keep --manage` 开发任务拆分

## 1. 目的

本文档把 `/Users/wolf/RustroverProjects/onekey-run-rs/docs_design/15_tui_keep_mode_design.md` 收敛成一份可以直接执行的开发任务清单。

目标不是再次讨论 `keep` 和 `manage` 是否应该拆开，而是明确：

- 先改哪些文件
- 每一步要交付什么
- 什么叫“做完”
- 需要补哪些测试

本专题的最终目标是：

- `onekey-run up --tui`
  维持当前行为
- `onekey-run up --tui --keep`
  在 services 退出后保留 TUI，但默认只读
- `onekey-run up --tui --keep --manage`
  在 services 退出后保留 TUI，并允许继续作为常驻控制台进行管理

## 1.1 当前执行状态

截至当前这一轮执行，本专题已完成实现与测试校验。

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

- 为 `up` 增加 `--keep` / `--manage` CLI 参数与约束
- 在 orchestrator 中透传 keep/manage 运行选项
- 把 TUI 明确拆成 `Running` / `PostRunReadonly` / `PostRunManage`
- 在 runtime 结束后保留 TUI，并切换到 post-run phase
- 让 post-run phase 基于内存冻结快照而不是运行时文件
- 在 `PostRunManage` 中开放最小管理动作
- 补齐测试和文档索引

### 2.2 非目标

- 不在本期实现完整的 TUI 管理控制台
- 不在 `PostRunReadonly` 中允许继续操作
- 不新增配置编辑能力
- 不实现 keep/manage 的复杂策略矩阵
- 不把 post-run UI 变成新的 runtime truth source

## 3. 任务完成定义

以下条件同时满足，视为本专题完成：

1. `up --keep` / `up --manage` 等非法参数组合会在 CLI 层被拒绝
2. `up --tui --keep` 可以在运行结束后进入只读 post-run
3. `up --tui --keep --manage` 可以在运行结束后进入可管理 post-run
4. post-run 阶段在 cleanup 之后仍能显示最终 logs / events / services 状态
5. `PostRunReadonly` 中 `R` 不会继续生效
6. `PostRunManage` 中 `R` 能继续生效
7. 默认 `up --tui` 行为不回归
8. 最终退出码不因 keep/manage 改变

## 4. 推荐实施顺序

建议按下面顺序推进，避免一开始就把 CLI、phase、post-run 管理动作混成一坨：

1. 先补 CLI 参数与 `RunOptions`
2. 再把 TUI 从单阶段函数改造成 session + phase 结构
3. 实现 `Running -> PostRunReadonly` 的最小 keep 链路
4. 冻结最终快照并接 cleanup 后展示
5. 再补 `PostRunManage`
6. 最后补最小 post-run 管理动作、测试和文档

## 5. 任务拆分

### Task 1：为 `up` 增加 `--keep` / `--manage` 参数与约束

状态：`done`

#### 目标

先把 CLI 语义固定下来，避免后续 orchestrator / TUI 改完后又回头改参数关系。

#### 需要修改的文件

- `src/cli.rs`
- `src/app.rs`

#### 具体改动

- 在 `UpArgs` 中新增：
  - `keep: bool`
  - `manage: bool`
- 用 `clap` 表达以下关系：
  - `--keep` requires `--tui`
  - `--manage` requires `--keep`
  - `--daemon` 仍与 `--tui` 互斥
- 在 app 层把 keep/manage 透传给 orchestrator `RunOptions`

#### 完成标准

- 合法与非法组合在参数解析阶段就能稳定区分
- app 层已经能拿到明确的 keep/manage 选项

#### 推荐验收测试

- `up --keep` 解析失败
- `up --manage` 解析失败
- `up --tui --keep` 解析成功
- `up --tui --manage` 解析失败
- `up --tui --keep --manage` 解析成功

### Task 2：扩展 `RunOptions` 与 `run_up(...)` 分发模型

状态：`done`

#### 目标

让 orchestrator 层能明确区分：

- 普通 TUI
- keep 只读 TUI
- keep + manage TUI

#### 需要修改的文件

- `src/orchestrator.rs`

#### 具体改动

- 在 `RunOptions` 中增加：
  - `keep_tui: bool`
  - `manage_tui: bool`
- 让 `run_up(...)` / `run_up_tui(...)` 消费这些选项
- 保持 plain / daemon 模式完全不受影响

#### 完成标准

- orchestrator 内部已有稳定的 keep/manage 选项入口
- 非 TUI 模式路径不回归

### Task 3：把 TUI 从单阶段函数改造成 session + phase 结构

状态：`done`

#### 目标

为运行期结束后仍留在同一个 alternate screen 做准备。

#### 需要修改的文件

- `src/tui.rs`

#### 具体改动

- 从当前单一 `run_dashboard(...)` 收敛出更明确的结构，例如：
  - `DashboardSession`
  - `DashboardPhase`
- `DashboardPhase` 至少包括：
  - `Running`
  - `PostRunReadonly`
  - `PostRunManage`
- 保留同一个 terminal session，避免“先退出 terminal 再重新进 terminal”

#### 完成标准

- TUI 结构已经能表达多 phase
- terminal 生命周期不再被强绑在单次 live loop 上

#### 注意事项

- 不要在这个阶段就把 post-run 管理动作一起塞进去
- 先把结构搭稳，再接功能

### Task 4：实现 `--keep` 的最小 post-run 只读链路

状态：`done`

#### 目标

先打通 `Running -> PostRunReadonly`，验证 keep 模式的核心语义。

#### 需要修改的文件

- `src/orchestrator.rs`
- `src/tui.rs`

#### 具体改动

- 当 live phase 结束且 `keep_tui = true` 时：
  - 先完成 shutdown
  - 再完成 cleanup
  - 然后进入 `PostRunReadonly`
- 当 `keep_tui = false` 时：
  - 保持当前直接退出的行为
- 在 post-run readonly 中只允许浏览与退出相关命令

#### 完成标准

- `up --tui --keep` 在 runtime 结束后不会立刻退出
- 用户可以继续浏览界面直到按 `q` / `Esc`

### Task 5：实现 post-run 快照冻结，脱离运行时文件

状态：`done`

#### 目标

确保 cleanup 之后 TUI 仍能继续显示最终状态，而不是因为 `.onekey-run/` 被删掉而丢数据。

#### 需要修改的文件

- `src/tui.rs`
- `src/orchestrator.rs`
- 如有需要可轻微调整 `src/runtime_state.rs`

#### 具体改动

- 在进入 post-run 之前做一次显式冻结：
  - drain 最后日志
  - 读取最后一批 events
  - 固化 service 最终状态
  - 固化当前 selection / scroll / notice
- post-run 阶段不再依赖：
  - `state.json`
  - `events.jsonl`
  - child handle 存活检查

#### 完成标准

- cleanup 后 post-run 仍能看见最终 logs / events / service 状态
- 不会因为 `.onekey-run/` 被清掉而让界面空掉

### Task 6：为 post-run 渲染 phase 语义与只读边界

状态：`done`

#### 目标

让用户明确知道当前是在：

- 运行中
- 只读 post-run
- 可管理 post-run

#### 需要修改的文件

- `src/tui.rs`

#### 具体改动

- 在 header / footer 中补 phase 提示，例如：
  - `mode running`
  - `mode post-run`
  - `mode manage`
- 在 `PostRunReadonly` 下：
  - 禁用 `R`
  - footer 只显示浏览相关 keys
- notice 建议明确提示：
  - `post-run view; press q or Esc to exit`

#### 完成标准

- 用户能区分当前 phase
- `PostRunReadonly` 中不会误以为还能继续操作

### Task 7：实现 `--keep --manage` 的 post-run 常驻控制台

状态：`done`

#### 目标

在 keep 的基础上再开放“保留后可继续管理”的语义。

#### 需要修改的文件

- `src/tui.rs`
- `src/orchestrator.rs`

#### 具体改动

- 当 `manage_tui = true` 时，进入 `PostRunManage`
- `PostRunManage` 在保留浏览能力的同时，允许继续执行授权的管理命令
- phase 切换建议支持：
  - `Running -> PostRunManage`
  - `PostRunManage -> Running`
    当用户重新触发运行期动作后

#### 完成标准

- `up --tui --keep --manage` 能进入可管理 post-run
- TUI 本身成为“当前配置对应实例的常驻控制台”

#### 注意事项

- post-run manage 不应复活旧 runtime 实例身份
- 重新进行管理动作时，必要的 runtime 资源应重新建立

### Task 8：在 `PostRunManage` 中开放首个最小管理动作并补测试

状态：`done`

#### 目标

先用一个最小动作验证 manage 语义，而不是一下子把整个控制台做满。

#### 需要修改的文件

- `src/tui.rs`
- `src/orchestrator.rs`
- `docs_dev/README.md`

#### 具体改动

- 首版建议只开放：
  - `R`
    在 `PostRunManage` 中启动或重启当前选中 service
- 保持 `PostRunReadonly` 中 `R` 无效
- 补以下测试：
  - keep=false 运行结束后直接退出
  - keep=true/manage=false 进入 `PostRunReadonly`
  - keep=true/manage=true 进入 `PostRunManage`
  - `PostRunReadonly` 中 `R` 不生效
  - `PostRunManage` 中 `R` 生效
  - cleanup 后 post-run 仍能展示最终状态
  - 最终退出码不因 keep/manage 改变
- 更新 `docs_dev/README.md` 索引

#### 完成标准

- manage 模式已经有一个真实可用的后续动作
- 本专题关键路径有自动化测试覆盖

## 6. 最小可交付切片建议

如果要分几次稳定提交，建议这样切：

1. 切片 A：
   Task 1 + Task 2
   先固定 CLI 和 orchestrator 入口
2. 切片 B：
   Task 3 + Task 4 + Task 5
   先打通 `--keep` 只读 post-run
3. 切片 C：
   Task 6 + Task 7 + Task 8
   再补 phase 呈现、manage 模式和最小 post-run 管理动作

这样做的好处是：

- 可以先验证 keep 的核心收益
- 不会让 manage 模式阻塞 keep 模式落地
- 一旦出现问题，容易判断是 phase 结构问题还是 manage 动作问题

## 7. 当前建议结论

这个专题最重要的不是“多加两个 flag”，而是把：

- 保留 TUI
- 继续管理

拆成两层清晰语义。

推荐的落地顺序仍然是：

1. 先把 CLI 和 `RunOptions` 冻结
2. 再把 TUI 改造成多 phase 结构
3. 先落 `--keep` 的只读 post-run
4. 最后再落 `--manage` 的常驻控制台和最小管理动作

这样风险最低，行为也最容易解释给用户。
