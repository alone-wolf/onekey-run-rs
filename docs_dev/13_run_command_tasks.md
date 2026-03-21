# `run` 命令开发任务拆分

## 1. 目的

本文档把 `docs_design/12_run_command_design.md` 收敛成一份可以直接执行的开发任务清单。

目标不是再次讨论方案，而是明确：

- 先改哪些文件
- 每一步要交付什么
- 什么叫“做完”
- 需要补哪些测试

本专题的最终目标是：

- 新增 `onekey-run run` 子命令
- 支持单独执行一个 service
- 支持单独执行一个 action
- 支持 standalone action 的参数默认值与 `--arg key=value` 覆盖
- 在执行任何 action 前，把本次实际使用到的全部参数值展示给用户

## 1.1 当前执行状态

截至当前这轮实现，本专题核心任务已完成。

- Task 1：`done`
- Task 2：`done`
- Task 3：`done`
- Task 4：`done`
- Task 5：`done`
- Task 6：`done`
- Task 7：`done`
- Task 8：`done`
- Task 9：`done`

## 2. 范围与非目标

### 2.1 本次范围

- 为 CLI 增加 `run` 子命令
- 为 `run` 增加 `--service` / `--action` 两种执行模式
- 为 `run --service` 增加 hook 选择策略
- 为 `run --action` 增加 `--arg key=value` 参数注入
- 为 standalone action 提供默认上下文值
- 将“执行 action 前展示已解析参数值”沉淀为公共执行行为
- 补齐对应测试与文档

### 2.2 非目标

- 不改现有 `up` / `down` 的主语义
- 不新增新的占位符语法
- 不引入表达式、条件分支或 `${var:-default}` 风格扩展
- 不把 `run --service` 扩展成实例级后台管理命令
- 不在本期实现 `run` 的 `--json` 输出

## 3. 任务完成定义

以下条件同时满足，视为本专题完成：

1. `onekey-run run --service <name>` 可以执行单个 service
2. `onekey-run run --service <name> --without-hooks` 可以跳过全部 hook
3. `onekey-run run --service <name> --with-all-hooks` 可以按生命周期执行全部 hook
4. `onekey-run run --service <name> --hook ...` 可以按选择执行部分 hook
5. `onekey-run run --action <name>` 可以直接执行单个 action
6. `run --action` 对未显式传入的参数会回填默认值
7. `run --action --arg key=value` 可以覆盖合法上下文参数
8. 执行任何 action 前，终端都能看到本次实际使用到的全部参数值
9. 相关单元测试与回归测试全部落地

## 4. 推荐实施顺序

建议严格按下面顺序推进，避免出现中途半完成的 CLI：

1. 先补 CLI 参数结构
2. 再补 hook 选择策略与 standalone action 参数建模
3. 抽离 action 参数解析与展示公共逻辑
4. 落地 `run --action`
5. 落地 `run --service`
6. 回接现有 hook/action 执行链路的公共参数展示
7. 补齐测试
8. 更新相关文档

## 5. 任务拆分

### Task 1：为 CLI 增加 `run` 子命令与参数结构

状态：`done`

#### 目标

让 CLI 能正确表达 `run --service` / `run --action` 两种用法和互斥关系。

#### 需要修改的文件

- `src/cli.rs`
- `src/app.rs`

#### 具体改动

- 在 `src/cli.rs` 中新增：
  - `Command::Run(RunArgs)`
  - `RunArgs`
  - `RunHookSelectionArgs` 或等价参数结构
- 通过 `clap` 表达以下约束：
  - `--service` 与 `--action` 互斥
  - `--with-all-hooks`、`--without-hooks`、`--hook` 互斥
  - `--hook` 可重复
  - `--arg` 可重复
- 在 `src/app.rs` 中补充 `Command::Run(...)` 分发入口

#### 完成标准

- CLI 帮助中能看到 `run` 子命令
- 非法参数组合会在参数解析阶段直接报错
- 合法参数组合可以进入 app 分发层

#### 推荐验收测试

- `run --service api --action prepare` 解析失败
- `run --service api --with-all-hooks --without-hooks` 解析失败
- `run --service api --hook before_start --hook after_start_success` 解析成功
- `run --action notify --arg service_name=api --arg hook_name=manual` 解析成功

### Task 2：定义 `run` 模式内部数据模型

状态：`done`

#### 目标

把 CLI 原始参数收敛成 orchestrator 可直接消费的内部结构。

#### 需要修改的文件

- `src/orchestrator.rs`
- 如有必要可新增 `src/run_command.rs`

#### 具体改动

- 设计并实现：
  - `RunTarget`
  - `HookSelection`
  - `StandaloneActionArgs` 或等价结构
- 明确以下内部表示：
  - service 模式
  - action 模式
  - hook 全部启用 / 全部禁用 / 指定集合
- 把 CLI 输入转成稳定内部结构，避免 orchestrator 直接处理 `clap` 类型

#### 完成标准

- orchestrator 能拿到明确的内部参数对象
- hook 过滤逻辑不依赖分散的布尔判断

#### 注意事项

- 建议让 `run --service <name>` 默认映射到 `HookSelection::None`
- 默认行为不要散落在多个入口函数中

### Task 3：抽出 action 占位符解析与“参数展示”公共能力

状态：`done`

#### 目标

把 action 执行前的参数解析、占位符收集与展示做成公共逻辑，供 `run --action` 和现有 hook 执行链路共用。

#### 需要修改的文件

- `src/config.rs`
- `src/orchestrator.rs`
- 如有必要可新增 `src/action_runtime.rs`

#### 具体改动

- 从现有占位符渲染逻辑中补出可复用 helper：
  - 扫描某个 action `args` 中引用了哪些占位符
  - 解析这些占位符的最终值
  - 生成稳定顺序的展示文本
- 建议把“参数展示范围”限定为该 action 实际引用到的占位符
- 为后续公共执行链路准备一个统一入口，例如：
  - `prepare_action_execution(...)`
  - 或 `resolve_action_args_and_params(...)`

#### 完成标准

- 可以独立拿到：
  - 渲染后的 `args`
  - 本次用到的参数名和值
- 参数展示顺序稳定，便于测试

#### 推荐验收测试

- 只引用 `${service_name}` 与 `${hook_name}` 的 action，不会额外展示未使用变量
- 展示结果中的值与最终渲染后的 `args` 保持一致

### Task 4：为 standalone action 建立默认上下文与 `--arg` 覆盖规则

状态：`done`

#### 目标

补齐 `run --action` 所需的手工执行上下文。

#### 需要修改的文件

- `src/config.rs`
- `src/orchestrator.rs`

#### 具体改动

- 定义 standalone action 的默认参数来源：
  - `project_root`
  - `config_path`
  - `service_name`
  - `action_name`
  - `hook_name`
  - `service_cwd`
  - `service_executable`
  - `service_pid`
  - `stop_reason`
  - `exit_code`
  - `exit_status`
- 实现 `--arg key=value` 解析规则：
  - 空 key 报错
  - 非 `key=value` 报错
  - 未知 key 报错
  - 重复 key 后者覆盖前者
- 若提供 `service_name=<name>` 且 service 存在，按设计文档补 service 相关默认推导

#### 完成标准

- `run --action` 在没有 `--arg` 时也能构造完整上下文
- 合法 `--arg` 可以覆盖默认值
- 拼写错误的参数名会直接失败

#### 推荐验收测试

- `--arg servie_name=api` 报错
- `--arg service_name=api` 时可推导出 `service_cwd`
- 未显式提供 `hook_name` 时默认为 `manual`

### Task 5：让 `process::run_action(...)` 支持 standalone action 场景

状态：`done`

#### 目标

让单独执行 action 与 hook 执行 action 共用底层进程执行逻辑。

#### 需要修改的文件

- `src/process.rs`
- `src/orchestrator.rs`

#### 具体改动

- 解除 `process::run_action(...)` 对 `HookName` 强类型的硬依赖
- 可选方案：
  - 改为接收 `&str`
  - 或新增 standalone action 包装函数
- 确保现有错误消息仍能带出可读 hook 名

#### 完成标准

- hook 场景不回归
- standalone action 场景可以复用同一套超时、退出码、失败处理逻辑

#### 风险点

- 不要为了支持 standalone action 破坏现有 hook 事件文本

### Task 6：落地 `run --action`

状态：`done`

#### 目标

让用户可以直接运行单个 action，并在执行前看到完整参数值。

#### 需要修改的文件

- `src/app.rs`
- `src/orchestrator.rs`
- 如有必要可新增 `src/run_command.rs`

#### 具体改动

- 新增 `run_single_action(...)`
- 加载配置并解析目标 action
- 构造 standalone action 上下文
- 执行前打印本次 action 使用到的全部参数
- 渲染参数后执行 action
- 根据结果返回：
  - 成功
  - 非零退出
  - 超时
  - 渲染失败

#### 完成标准

- `onekey-run run --action <name>` 可运行
- action 成功时命令返回成功
- action 失败或超时时命令返回失败
- 用户在执行前能看到解析后的参数值

#### 推荐验收测试

- 无 `--arg` 时使用默认值成功执行
- 带 `--arg service_name=api` 时成功执行
- action 超时时返回错误

### Task 7：落地 `run --service`

状态：`done`

#### 目标

让用户可以只运行一个 service，并按所选策略决定是否执行 hook。

#### 需要修改的文件

- `src/orchestrator.rs`
- `src/process.rs`

#### 具体改动

- 新增 `run_single_service(...)`
- 只解析目标 service，不补依赖
- 复用现有：
  - `spawn_service(...)`
  - 运行监控
  - `Ctrl-C` 停止
  - 停止升级策略
- 在 hook 执行前增加过滤判断：
  - `all`
  - `none`
  - `selected(set)`

#### 完成标准

- `run --service <name>` 可运行单个 service
- 默认不执行 hook
- `--with-all-hooks` 能按生命周期执行 hook
- `--hook before_start` 只执行指定 hook

#### 推荐验收测试

- `run --service app` 默认不执行任何 hook
- `run --service app --with-all-hooks` 会执行 `before_start`
- `run --service app --hook before_start` 不会执行未选中的 `after_start_success`

### Task 8：把“执行前参数展示”接回现有 hook/action 执行链路

状态：`done`

#### 目标

确保不仅 `run --action`，现有 `up` / `down` / runtime hook 里的 action 也都遵守同一展示规则。

#### 需要修改的文件

- `src/orchestrator.rs`

#### 具体改动

- 将 `run_hook_with_context(...)`
- 以及 `run_hook_with_bundle(...)`
- 切到新的公共 action 准备/展示逻辑
- 在 action 真正执行前输出参数展示信息
- 如已有事件系统可复用，补充一条摘要事件

#### 完成标准

- 通过 hook 执行的 action 也会输出本次实际使用到的参数值
- hook 场景与 standalone action 场景的展示格式一致

#### 注意事项

- 输出不要膨胀成完整环境变量 dump
- 只展示 action 实际引用的参数

### Task 9：补齐测试与文档同步

状态：`done`

#### 目标

为 `run` 命令补齐回归保护，并同步现有契约文档。

#### 需要修改的文件

- `src/cli.rs`
- `src/orchestrator.rs`
- `src/config.rs`
- `docs_dev/02_cli_contract.md`
- `docs_dev/03_config_schema.md`
- `docs_design/02_actions_context_variables.md`
- `docs_design/04_actions_hooks_execution_flow.md`

#### 具体改动

- 为 CLI 互斥关系补测试
- 为 standalone action 默认值与参数覆盖补测试
- 为 hook 过滤补测试
- 为参数展示补测试
- 更新文档，使实现与契约一致

#### 完成标准

- 新增能力均有测试覆盖
- 文档不再缺失 `run` 命令
- 文档中的参数行为、默认值与实现一致

## 6. 推荐提交方式

建议不要一次性大改全部文件，推荐按以下粒度提交：

1. CLI 参数结构 + 内部模型
2. standalone action 上下文与公共参数解析
3. `run --action`
4. `run --service`
5. hook 公共参数展示回接
6. 文档与测试收尾

这样更利于 review，也更容易定位回归。

## 7. 关键风险与实现提醒

### 7.1 默认行为容易散落

`run --service` 默认不跑 hook、`run --action` 默认补上下文，这两类默认行为要集中在单一入口收敛，避免 CLI 层、app 层、orchestrator 层各自补一部分。

### 7.2 不要静默吞掉未知 `--arg`

这是最重要的可用性保护之一。拼写错误必须直接报错，否则用户会误以为参数已生效。

### 7.3 只展示 action 实际用到的参数

如果一口气展示所有可用上下文，输出会非常吵，也不利于定位真正参与渲染的变量。

### 7.4 `run --service` 不要偷偷补依赖

这会让它与 `up <service>` 的语义混淆。`run --service` 应坚持“只跑一个 service”。

### 7.5 公共逻辑优先于复制代码

参数渲染、参数展示、action 启动与超时处理都应优先抽公共能力，不要分别在 `run --action` 和 hook 链路里各写一套。

## 8. 当前建议结论

本专题最合适的实施方式是：

- 先把 CLI 与内部模型搭起来
- 先做 `run --action`，把 standalone action 上下文和参数展示跑通
- 再做 `run --service`，复用现有 service 生命周期链路
- 最后把公共参数展示能力接回全部 hook/action 执行路径

这样可以先解决最不确定的 action 上下文问题，再实现 service 模式，整体风险更低。
