# `actions` / `hooks` 实现任务清单

## 1. 目标

本文档把前面的设计文档收敛为一份可直接执行的实现清单，便于后续按阶段推进编码、测试与文档落地。

本计划默认基于当前项目已有模块：

- `src/config.rs`
- `src/app.rs`
- `src/orchestrator.rs`
- `src/process.rs`
- `src/runtime_state.rs`
- `src/tui.rs`

## 2. 总体实施策略

建议按“先静态契约，后运行时调度；先最小闭环，后增强能力”的顺序推进：

1. 配置模型与解析
2. `check` 静态校验
3. `before_start` 最小可用执行链
4. 其余启动/停止 hooks
5. 事件流、日志接口与管理面展示

这样可以尽早得到：

- 能写配置
- 能被 `check` 拦住错误
- 能在 `up` 中跑通最关键的阻断型 `before_start`

## 3. Phase 1：配置模型落地

### 3.1 目标

把 `actions` 与 `service.hooks` 接进配置解析层，但先不接运行时。

### 3.2 任务

- 在 `src/config.rs` 新增顶层 `actions` 配置结构
- 在 `src/config.rs` 为 service 新增 `hooks` 配置结构
- 定义受支持 hook 的枚举或标准常量集合
- 为 action 定义字段：
  - `executable`
  - `args`
  - `cwd`
  - `env`
  - `timeout_secs`
  - `disabled`
- 在解析后结构中保存：
  - action 名
  - 标准化后的 `args`
  - 解析后的绝对 `cwd`
  - service -> hook -> action 引用关系

### 3.3 验收标准

- `onekey-tasks.yaml` 可以合法解析出 `actions`
- service 可以合法解析出 `hooks`
- 旧配置在不写 `actions` / `hooks` 时保持兼容

## 4. Phase 2：占位符与 hook 上下文校验

### 4.1 目标

先把“哪些写法合法”钉死到 `check`，避免运行时才爆雷。

### 4.2 任务

- 在 `src/config.rs` 或单独校验模块中增加占位符扫描逻辑
- 识别 `${name}` 语法
- 校验未知变量名
- 校验未闭合占位符、空占位符、非法变量名
- 校验某个 action 被某 hook 引用时，该 hook 是否允许 action 中用到的变量
- 校验引用不存在的 action
- 校验引用 `disabled: true` 的 action
- 校验 hook 名非法、hook 值类型非法、action 未知字段等问题

### 4.3 建议实现方式

- 先把 action 中所有占位符抽取成集合
- 再按每个“引用点”逐一验证
- 同一个 action 被多个 hook 复用时，分别对每个 hook 进行可用性校验

### 4.4 验收标准

- `check` 能一次性输出多条配置错误
- 典型错误都能定位到字段路径
- `before_start` 中使用 `${service_pid}` 会被拦截

## 5. Phase 3：`before_start` 最小可用执行链

### 5.1 目标

先实现最关键的阻断型 hook，让 action 真正参与启动流程。

### 5.2 任务

- 在 `src/orchestrator.rs` 启动 service 前插入 `before_start` 调度
- 按数组顺序同步串行执行 action
- 将 action 作为一次性子进程执行，而不是注册为长期 service
- 支持：
  - `executable`
  - `args`
  - `cwd`
  - `env`
  - `timeout_secs`
- 任一 action 失败时：
  - 终止后续 action
  - 当前 service 标记为启动失败
  - 后续依赖该 service 的服务不再启动
- 为 action 执行前构造上下文：
  - `${service_name}`
  - `${action_name}`
  - `${service_cwd}`
  - 以及后续预留变量结构

### 5.3 可能涉及的文件

- `src/orchestrator.rs`
- `src/process.rs`
- `src/runtime_state.rs`
- `src/error.rs`

### 5.4 验收标准

- `before_start` 成功时 service 正常启动
- `before_start` 失败时 service 不会 `spawn`
- 下游 `depends_on` service 会被跳过

## 6. Phase 4：启动后与停止侧 hooks

### 6.1 目标

在最小可用闭环跑通后，补齐主要 lifecycle hooks。

### 6.2 建议实现顺序

1. `after_start_success`
2. `after_start_failure`
3. `before_stop`
4. `after_stop_success`
5. `after_stop_timeout`
6. `after_stop_failure`
7. `after_runtime_exit_unexpected`

### 6.3 任务

- 在 `src/orchestrator.rs` 的启动成功路径上挂入 `after_start_success`
- 在启动失败路径上挂入 `after_start_failure`
- 在停止流程里挂入 `before_stop`
- 在优雅停止成功/超时/最终失败路径上挂入对应 stop hooks
- 在运行期 service 意外退出路径上挂入 `after_runtime_exit_unexpected`
- 明确这些 hook 默认都是“附加型”，失败只记录，不覆盖主流程结论

### 6.4 验收标准

- 启动成功后置 action 失败，不影响 service 继续处于运行态
- 停止侧 hook 失败，不影响主停止流程推进
- stop timeout 与最终 stop failure 能被区分

## 7. Phase 5：统一 action 执行器

### 7.1 目标

不要把 hook 执行逻辑散落在 orchestrator 多处，建议抽出统一执行器。

### 7.2 任务

- 新增统一 action runner
- 输入建议包括：
  - service 名
  - hook 名
  - action 定义
  - 已解析的上下文变量
- 输出建议包括：
  - 退出码 / 退出状态
  - 开始时间 / 结束时间 / duration
  - 成功 / 失败 / 超时

### 7.3 建议落点

- 可放在 `src/process.rs`
- 或新增 `src/actions.rs`

如果新增模块，建议后续把占位符展开也收敛到同一侧，减少 orchestrator 负担。

## 8. Phase 6：运行状态、事件流与日志接口

### 8.1 目标

先不做复杂本体日志系统，但要把可观测性接口留好，并让 hook/action 状态能够进入实例级日志。

### 8.2 任务

- 在 `src/runtime_state.rs` 增加更细粒度的 service 阶段/原因字段
- 定义 hook / action 执行事件
- 在 orchestrator 中发出事件：
  - hook started / finished
  - hook failed
  - action started / finished / failed
  - action timed out
  - service start aborted by before_start
- 将同一批 lifecycle event 同步输出到：
  - `.onekey-run/events.jsonl`
  - 顶层 `log` 对应的实例日志（若配置）
- 为未来本体日志保留统一输出接口

### 8.3 对现有命令的影响

- `up`
  后续可以输出更明确的 hook/action 摘要，并在实例日志中保留落盘记录
- `management`
  后续可以展示最近 hook 结果摘要
- `--tui`
  后续可以增加 hook/action 事件面板

## 9. Phase 7：模板与文档同步

### 9.1 目标

配置能力一旦落地，模板、帮助文本与用户文档必须同步。

### 9.2 任务

- 更新 `init` 生成模板
- 更新 `init --full` 模板
- 在 `docs_dev/03_config_schema.md` 增加 `actions` / `hooks`
- 在 `docs_dev/02_cli_contract.md` 和帮助文案中补充 `check` 对新字段的说明
- 如有必要，更新 skill：
  - `skills/onekey-run-config-authoring/SKILL.md`

### 9.3 验收标准

- `init --full` 能生成包含最小示例 action/hook 的模板
- 用户仅看模板和文档就能写出一份可通过 `check` 的配置

## 10. 测试任务清单

建议至少补三层测试：

### 10.1 配置解析测试

- `actions` 正常解析
- `hooks` 正常解析
- 相对 `cwd` 正确按配置文件目录解析

### 10.2 `check` 校验测试

- 未知 hook 名
- 引用不存在 action
- 引用 disabled action
- `before_start` 使用 `${service_pid}`
- `${service_naem}` 拼写错误
- 未闭合 `${...`

### 10.3 orchestrator 集成测试

- `before_start` 成功 -> service 启动
- `before_start` 失败 -> service 不启动
- 上游 service 因 `before_start` 失败，下游依赖 service 被跳过
- `after_start_success` 失败不影响 service 运行
- `before_stop` 失败不阻断停止流程

## 11. 建议拆分的提交粒度

为了便于评审与回归，建议按下面粒度拆提交：

1. 配置模型与解析结构
2. `check` 新校验
3. `before_start` 执行链
4. 其余 hooks
5. 事件流 / 状态展示
6. 模板与文档

## 12. 风险点

需要重点关注的风险：

- 同一个 action 被多个 hook 复用时的上下文合法性校验
- 跨平台下 action 超时、中断、退出状态的统一抽象
- action `cwd` 与 service `cwd` 的相对路径解析不要混淆
- stop hooks 与现有 `down` / Ctrl+C 清理路径的耦合
- 输出过多 hook 日志可能干扰当前普通 `up` 的简洁输出

## 13. 建议优先级

如果要以“最快形成可用能力”为目标，建议优先级如下：

1. 配置模型
2. `check` 校验
3. `before_start`
4. `after_start_success` / `after_start_failure`
5. stop hooks
6. 运行期异常退出 hook
7. 事件流增强

## 14. 当前建议结论

最推荐的第一轮编码范围是：

- 配置模型落地
- `check` 新校验
- `before_start` 最小可用实现

做到这一步后，`actions` / `hooks` 就已经具备第一批真实价值，同时复杂度仍然可控。
