# `list` 命令开发任务拆分

## 1. 目的

本文档把 `docs_design/11_list_command_design.md` 收敛成一份可以直接执行的开发任务清单。

目标不是继续讨论命令该不该做，而是明确：

- 先改哪些文件
- 每一步交付什么
- 什么叫完成
- 需要补哪些测试

本专题最终目标是：

- 新增 `onekey-run list`
- 支持列出 `services` / `actions`
- 支持 `--detail`
- 支持 `--DAG`
- 保持输出稳定、可测、贴近当前配置模型

## 2. 范围与非目标

### 2.1 本次范围

- 新增 `list` 子命令及参数
- 基于 `ProjectConfig::load(...)` 输出配置浏览结果
- 输出名称列表、详细信息、DAG 关系
- 补齐 CLI 解析测试与渲染测试
- 更新 CLI 文档

### 2.2 非目标

- 不修改 `onekey-tasks.yaml` schema
- 不新增交互式筛选、grep、搜索等能力
- 不在本期支持 JSON 输出
- 不在本期保留 YAML 原始书写顺序
- 不复用 `build_run_plan(...)` 作为 `list` 的数据来源

## 3. 任务完成定义

以下条件同时满足，视为本专题完成：

1. `onekey-run list` 默认列出全部 `services` 和 `actions`
2. `onekey-run list --services` / `--actions` 可单独工作
3. `onekey-run list --detail` 可输出所选对象详细信息
4. `onekey-run list --DAG` 可输出 service 依赖与 hook 引用关系
5. disabled 的 service / action 仍会被展示
6. 相关测试与文档已补齐

## 4. 推荐实施顺序

建议按以下顺序推进：

1. 定义 CLI 参数和互斥关系
2. 在应用入口接入 `list`
3. 实现选择范围归一化
4. 实现名称输出
5. 实现 detail 输出
6. 实现 DAG 输出
7. 补测试
8. 更新文档

## 5. 任务拆分

### Task 1：新增 `list` CLI 契约

状态：`todo`

#### 目标

在参数层正式接入 `list` 子命令，并把基础互斥关系声明清楚。

#### 需要修改的文件

- `src/cli.rs`

#### 具体改动

- 在 `Command` 中新增：
  - `List(ListArgs)`
- 新增：
  - `pub struct ListArgs`
- 建议字段：
  - `all: bool`
  - `services: bool`
  - `actions: bool`
  - `detail: bool`
  - `dag: bool`
- 为 `--DAG` 提供用户可见参数名
- 可选增加 `--dag` 等价别名
- 声明参数冲突关系：
  - `--detail` 与 `--DAG` 互斥
  - 首版建议 `--DAG` 与 `--all` / `--services` / `--actions` 互斥

#### 完成标准

- `Cli::try_parse_from(...)` 能成功解析合法用法
- 非法组合能由 clap 返回参数错误

### Task 2：在应用入口接入 `list`

状态：`todo`

#### 目标

让主执行路径能正确分发到 `list` 命令。

#### 需要修改的文件

- `src/app.rs`

#### 具体改动

- 在 `match cli.command` 中新增：
  - `Command::List(args)`
- 复用：
  - `ProjectConfig::load(&cli.config)?`
- 调用新的执行入口：
  - `orchestrator::run_list(&cli.config, &config, args)`

#### 完成标准

- `run(...)` 能正常处理 `list`
- `list` 复用现有配置加载与校验错误语义

### Task 3：定义 `list` 的内部选择模型

状态：`todo`

#### 目标

把 CLI 参数归一化成稳定的内部语义，避免后续渲染逻辑到处判断布尔组合。

#### 需要修改的文件

- `src/orchestrator.rs`
- 如有必要可新增内部小型辅助类型

#### 具体改动

- 新增一个内部选择模型，例如：
  - 范围：`All` / `Services` / `Actions`
  - 模式：`Names` / `Detail` / `Dag`
- 实现归一化规则：
  - 默认 `list` -> `Names + All`
  - `--all` -> `Names + All`
  - `--detail` 未选范围 -> `Detail + All`
  - `--services` -> 仅 service
  - `--actions` -> 仅 action
- 确保后续渲染层只接收已归一化结果

#### 完成标准

- 参数含义只在一个地方解释一次
- 后续渲染代码不再直接依赖原始 flag 组合

### Task 4：实现名称输出

状态：`todo`

#### 目标

先落地最小可用版本：默认列出 `services` 和 `actions` 的名称。

#### 需要修改的文件

- `src/orchestrator.rs`

#### 具体改动

- 新增：
  - `run_list(...) -> AppResult<()>`
  - `render_list_output(...) -> AppResult<String>`
- 在名称模式下输出：
  - `services:`
  - `actions:`
- 读取数据源时直接遍历：
  - `config.services`
  - `config.actions`
- disabled 对象仍然输出，并在名称后追加标记

#### 完成标准

- `onekey-run list` 可输出两个段落
- `onekey-run list --services` 只输出 services
- `onekey-run list --actions` 只输出 actions

#### 注意事项

- 不要复用 `resolve_service(...)`
- 不要复用 `resolve_actions(...)`

### Task 5：实现 detail 输出

状态：`todo`

#### 目标

在最小名称输出可用后，补全详细信息展示。

#### 需要修改的文件

- `src/orchestrator.rs`

#### 具体改动

- 为 service detail 输出字段：
  - `name`
  - `executable`
  - `args`
  - `cwd`
  - `env`（原始配置视图）
  - `resolved_env`（合并后的最终 env 视图）
  - `depends_on`
  - `restart`
  - `stop_signal`
  - `stop_timeout_secs`
  - `autostart`
  - `disabled`
  - `log`
  - `hooks`
- 为 action detail 输出字段：
  - `name`
  - `executable`
  - `args`
  - `cwd`
  - `env`（原始配置视图）
  - `resolved_env`（合并后的最终 env 视图）
  - `timeout_secs`
  - `disabled`
- 当原始配置包含结构化 env 值时，detail 输出应至少包含 merged env，避免用户无法直接看到最终生效值

#### 完成标准

- `onekey-run list --detail` 默认输出全部对象详细信息
- `onekey-run list --detail --services` 仅输出 service detail
- `onekey-run list --detail --actions` 仅输出 action detail

#### 注意事项

- hooks 建议按原始分组展示，不要扁平化
- 输出格式要稳定，方便测试断言

### Task 6：实现 DAG 输出

状态：`todo`

#### 目标

支持查看 service 依赖与 hook 引用关系。

#### 需要修改的文件

- `src/orchestrator.rs`

#### 具体改动

- 基于原始配置生成两类边：
  - `service --depends_on--> service`
  - `service --hooks.<name>--> action`
- 对未被任何 hook 引用的 action，增加独立输出区：
  - `standalone actions:`
- 输出顺序保持稳定：
  - service 名按字典序
  - hook 顺序按固定枚举顺序
  - action 名按原数组顺序或稳定顺序

#### 完成标准

- `onekey-run list --DAG` 能输出 service dependency 边
- `onekey-run list --DAG` 能输出 hook -> action 引用边
- 未被引用的 action 不会在 DAG 模式下丢失

### Task 7：补齐测试

状态：`todo`

#### 目标

保证新增命令在 CLI、语义、输出三个层面都可回归验证。

#### 需要修改的文件

- `src/cli.rs`
- `src/app.rs`
- `src/orchestrator.rs`
- 如有需要可新增测试辅助函数

#### 具体改动

- CLI 解析测试至少覆盖：
  - `list`
  - `list --all`
  - `list --services`
  - `list --actions`
  - `list --detail --services`
  - `list --detail --actions`
  - `list --DAG`
  - `list --detail --DAG` 冲突
- 渲染测试至少覆盖：
  - 默认模式输出两个段落
  - `--services` 不输出 `actions:`
  - `--actions` 不输出 `services:`
  - detail 模式包含关键字段
  - DAG 模式包含 dependency 边
  - DAG 模式包含 hook 引用边
  - orphan action 出现在 standalone 区域
- 语义测试至少覆盖：
  - disabled service 仍被列出
  - disabled action 仍被列出
  - 名称输出按字典序稳定

#### 完成标准

- 新增测试能稳定通过
- 不引入现有命令回归

### Task 8：更新文档

状态：`todo`

#### 目标

让设计、契约、实现清单保持一致。

#### 需要修改的文件

- `docs_dev/02_cli_contract.md`
- `docs_dev/README.md`
- 如有必要，补充 `docs_design/11_list_command_design.md`

#### 具体改动

- 在 CLI 契约中补充：
  - `list`
  - `list --services`
  - `list --actions`
  - `list --detail`
  - `list --DAG`
- 在 `docs_dev/README.md` 中登记本任务文档

#### 完成标准

- 新命令在开发文档中可被检索到
- 文档内容与实际实现保持一致

## 6. 推荐文件改动总览

本专题预计主要涉及：

- `src/cli.rs`
- `src/app.rs`
- `src/orchestrator.rs`
- `docs_dev/02_cli_contract.md`
- `docs_dev/README.md`

如实现过程中发现 `orchestrator.rs` 过重，可选再拆：

- `src/list.rs`

但这不是首版前置条件。

## 7. 风险点与实现注意事项

- 不能把 `list` 建在 `build_run_plan(...)` 之上，否则 disabled 对象会丢失
- 不能把 `list` 的输出顺序依赖 YAML 原始顺序，当前模型无法保证
- DAG 模式输出应优先选择“边列表”，避免 ASCII 图过早复杂化
- detail 输出字段要克制，首版先保证稳定和可测
- 既然 `ProjectConfig::load(...)` 已包含校验，`list` 就不应对非法配置做容错展示

## 8. 建议验收顺序

建议按以下顺序验收：

1. `cargo test` 中 CLI 解析测试先通过
2. 名称模式输出测试通过
3. detail 模式输出测试通过
4. DAG 模式输出测试通过
5. 手工执行以下命令做烟测：
   - `cargo run -- list`
   - `cargo run -- list --services`
   - `cargo run -- list --actions`
   - `cargo run -- list --detail`
   - `cargo run -- list --DAG`

## 9. 当前建议结论

推荐把本专题分成 8 个任务顺序落地：

1. CLI 契约
2. 应用入口
3. 选择模型
4. 名称输出
5. detail 输出
6. DAG 输出
7. 测试
8. 文档

这样能尽快先交付一个可用的 `list` 最小版本，再逐步补全 detail 和 DAG。
