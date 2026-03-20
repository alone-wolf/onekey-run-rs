# 配置文件生成器开发任务拆分

## 1. 目的

本文档把 `docs_design/09_config_generator_design.md` 收敛成一份可以直接落地的开发任务清单。

目标不是再次讨论方案，而是明确：

- 先改哪些文件
- 每一步要交付什么
- 什么叫“做完”
- 需要补哪些测试

本任务的最终目标是：

- `init` / `init --full` 不再依赖硬编码 YAML 字符串
- 统一使用现有 `ProjectConfig` 系列作为唯一配置 schema
- 通过 builder 组合配置对象，再序列化输出 YAML

## 1.1 当前执行状态

截至当前代码状态，本专题核心任务已基本落地。

- Task 1：`done`
- Task 2：`done`
- Task 3：`done`
- Task 4：`done`
- Task 5：`done`
- Task 6：`done`
- Task 7：`done`
- Task 8：`done`

当前未单独落地但也不构成缺口的项：

- `src/config_render.rs` 拆分：`deferred`
- 测试命名进一步统一整理：`optional`
- 将本文档改成长期维护的进度看板：`optional`

## 2. 范围与非目标

### 2.1 本次范围

- 为原始配置结构补充 YAML 输出能力
- 为内置模板实现 `preset_minimal` / `preset_full`
- 用结构化生成替换 `src/orchestrator.rs` 中的模板字符串
- 补齐 round-trip 测试，确保生成结果可重新加载

### 2.2 非目标

- 不做交互式配置向导
- 不做项目扫描式自动生成器
- 不重构 `ResolvedServiceConfig` / `ResolvedActionConfig` / `ResolvedLogConfig`
- 不在本期改 CLI 参数或新增命令

## 3. 任务完成定义

以下条件同时满足，视为本专题完成：

1. `onekey-run init` 生成最小模板
2. `onekey-run init --full` 生成完整模板
3. 模板生成路径不再使用硬编码 YAML 常量
4. 生成出的 YAML 可被 `ProjectConfig::load(...)` 重新加载并通过校验
5. 相关测试全部落地

## 4. 推荐实施顺序

建议严格按下面顺序推进，避免中途出现半完成状态。

1. 给原始配置结构补 `Serialize`
2. 为空字段添加合理的序列化裁剪
3. 实现 preset builder
4. 实现统一 YAML 渲染入口
5. 替换 `run_init(...)`
6. 补单元测试和回归测试
7. 更新相关文档

## 5. 任务拆分

### Task 1：让原始配置结构具备可序列化能力

状态：`done`

#### 目标

让 `src/config.rs` 中的原始配置结构既能读 YAML，也能写 YAML。

#### 需要修改的文件

- `src/config.rs`

#### 具体改动

- 为以下类型补 `Serialize`：
  - `ProjectConfig`
  - `DefaultsConfig`
  - `RestartPolicy`
  - `ServiceConfig`
  - `ActionConfig`
  - `ServiceHooksConfig`
  - `LogConfig`
  - `LogOverflowStrategy`
- 保持现有 `Deserialize` 和校验逻辑不变
- 不要给 `Resolved*` 增加不必要的输出职责

#### 完成标准

- `cargo test` 中至少有一条测试能把 `ProjectConfig` 成功序列化成 YAML 字符串
- 现有配置读取逻辑不受影响

#### 风险点

- `serde(rename_all = "kebab-case")` / `snake_case` 要保持与现有 YAML 契约一致
- 新增 `Serialize` 不能改变已有反序列化行为

### Task 2：补齐最小必要的序列化裁剪规则

状态：`done`

#### 目标

避免生成 YAML 时出现大量空字段、空 map、空数组，影响模板可读性。

#### 需要修改的文件

- `src/config.rs`

#### 具体改动

- 为可选字段补充：
  - `#[serde(skip_serializing_if = "Option::is_none")]`
- 为空容器字段补充：
  - `#[serde(skip_serializing_if = "Vec::is_empty")]`
  - `#[serde(skip_serializing_if = "BTreeMap::is_empty")]`
- 为 `ServiceHooksConfig` 增加：
  - `fn is_empty(&self) -> bool`
- 在 `ServiceConfig.hooks` 上使用：
  - `#[serde(skip_serializing_if = "ServiceHooksConfig::is_empty")]`

#### 完成标准

- minimal 模板不会输出空 `actions`
- 未配置 hooks 的 service 不会输出空 `hooks: {}`
- 未设置的 `env` / `depends_on` / `args` 不会污染输出

#### 注意事项

- 不要一开始就引入复杂的“按 profile 剪枝”逻辑
- 先靠 builder 控制字段是否显式出现，裁剪只做最小必要处理

### Task 3：实现最小模板 builder

状态：`done`

#### 目标

用结构化方式生成 `init` 默认模板。

#### 需要修改的文件

- `src/config.rs`

#### 具体改动

- 在 `impl ProjectConfig` 中增加：
  - `pub fn preset_minimal() -> Self`
- builder 应返回一个合法的 `ProjectConfig`
- 生成内容建议与当前默认模板语义一致：
  - `defaults.stop_timeout_secs`
  - 顶层实例 `log`
  - `app` service
  - `worker` service
  - `worker.depends_on = ["app"]`
  - service 级 `log`
- 不要求包含：
  - `actions`
  - `hooks`
  - `restart`
  - `autostart`
  - `disabled`

#### 完成标准

- `ProjectConfig::preset_minimal()` 返回值可直接通过 `validate(...)`
- 生成内容和当前 `init` 模板的用途一致

#### 推荐验收测试

- 断言最小模板序列化后包含 `services:`
- 断言最小模板序列化后不包含 `actions:`

### Task 4：实现完整模板 builder

状态：`done`

#### 目标

用结构化方式生成 `init --full` 模板。

#### 需要修改的文件

- `src/config.rs`

#### 具体改动

- 在 `impl ProjectConfig` 中增加：
  - `pub fn preset_full() -> Self`
- builder 应显式填入完整示例字段，覆盖当前主要 schema：
  - `defaults`
  - 顶层 `log`
  - 顶层 `actions`
  - service 的 `env`
  - `depends_on`
  - `restart`
  - `stop_signal`
  - `stop_timeout_secs`
  - `autostart`
  - `disabled`
  - `log`
  - `hooks`
- 示例值尽量保持与现有 `INIT_TEMPLATE_FULL` 等价，避免文档和用户心智突然变化

#### 完成标准

- `ProjectConfig::preset_full()` 返回值可直接通过 `validate(...)`
- 生成 YAML 中包含 `actions` 和 `hooks`
- 生成 YAML 的字段命名与当前 schema 完全一致

#### 推荐验收测试

- 断言完整模板包含 `actions:`
- 断言完整模板包含 `hooks:`
- 断言完整模板包含顶层 `log:`

### Task 5：实现统一 YAML 渲染入口

状态：`done`

#### 目标

把“结构体 -> YAML 字符串”的过程统一起来，避免 `run_init(...)` 直接碰 `serde_yaml` 细节。

#### 需要修改的文件

- `src/config.rs`
- 或新增 `src/config_render.rs`

#### 具体改动

- 二选一实现：
  - `pub fn to_yaml_string(&self) -> AppResult<String>`
  - `pub fn render_project_config_yaml(config: &ProjectConfig) -> AppResult<String>`
- 内部统一调用 `serde_yaml::to_string(...)`
- 错误需转换为项目已有 `AppError`

#### 完成标准

- minimal / full builder 都能通过同一入口输出 YAML
- 输出入口不依赖 `orchestrator`

#### 建议

- 本期优先放在 `src/config.rs`
- 只有当 `config.rs` 已明显过大时，再拆 `src/config_render.rs`

当前落地说明：

- 已在 `src/config.rs` 中实现 `ProjectConfig::to_yaml_string()`
- 尚未拆出 `src/config_render.rs`
- 当前明确标记为“暂不分拆”
- 因此该 task 已完成，文件拆分本身属于后续可选重构项，不作为当前阶段交付内容

### Task 6：替换 `run_init(...)` 的硬编码模板

状态：`done`

#### 目标

让 CLI `init` 命令正式切换到结构化生成链路。

#### 需要修改的文件

- `src/orchestrator.rs`

#### 具体改动

- 删除：
  - `INIT_TEMPLATE`
  - `INIT_TEMPLATE_FULL`
- `run_init(...)` 改为：
  1. 判断目标文件是否已存在
  2. 创建父目录
  3. 根据 `full` 选择：
     - `ProjectConfig::preset_minimal()`
     - `ProjectConfig::preset_full()`
  4. 调统一 YAML 渲染入口
  5. 写入目标文件
- 写入前建议先对 preset 调一次 `validate(...)`

#### 完成标准

- `onekey-run init` 行为保持不变
- `onekey-run init --full` 行为保持不变
- 模板来源改为结构化 builder

#### 注意事项

- 不要改变“目标文件已存在时拒绝覆盖”的现有行为
- 不要顺手改 CLI 输出文案，除非测试明确要求

### Task 7：补齐配置生成链路测试

状态：`done`

#### 目标

给新链路加自动化保护，避免以后 schema 改了但模板没跟上。

#### 需要修改的文件

- `src/config.rs`
- `src/orchestrator.rs`

#### 具体改动

- 在 `src/config.rs` 新增测试：
  - 最小配置可序列化
  - `preset_minimal()` 可序列化
  - `preset_full()` 可序列化
  - 空 hooks / 空 actions 按预期省略
- 在 `src/orchestrator.rs` 调整或新增测试：
  - `run_init(..., false)` 写出内容非空
  - `run_init(..., true)` 写出内容非空
  - 仍然拒绝覆盖已有文件

#### 核心回归测试

- `preset_minimal() -> YAML -> 写文件 -> ProjectConfig::load(...)`
- `preset_full() -> YAML -> 写文件 -> ProjectConfig::load(...)`

#### 完成标准

- 两条 round-trip 测试都通过
- `init` 相关原有测试全部保持通过

### Task 8：更新开发文档

状态：`done`

#### 目标

保证实现落地后，开发文档和代码保持一致。

#### 需要修改的文件

- `docs_dev/02_cli_contract.md`
- `docs_dev/03_config_schema.md`
- `docs_dev/05_architecture_plan.md`
- `docs_dev/README.md`

#### 具体改动

- 在 `02_cli_contract.md` 明确：
  - `init` / `init --full` 来源于内置 preset，而非硬编码模板
- 在 `03_config_schema.md` 明确：
  - 原始配置 schema 是唯一配置模型
  - 模板生成复用同一份 `ProjectConfig`
- 在 `05_architecture_plan.md` 明确：
  - `config` 模块除了解析和校验，还负责配置模板 builder / YAML render

#### 完成标准

- 开发文档不再暗示“模板一定来自手写字符串”

## 6. 推荐提交切片

为了降低 review 成本，建议按下面切片提交。

### Slice A：可序列化基础设施

状态：`done`

- Task 1
- Task 2

验收重点：

- schema 结构可写 YAML
- 空字段裁剪合理

### Slice B：preset builder

状态：`done`

- Task 3
- Task 4
- Task 5

验收重点：

- minimal / full 都能生成合法 YAML
- builder 和 render 职责清晰

### Slice C：CLI 切换与回归

状态：`done`

- Task 6
- Task 7

验收重点：

- `run_init(...)` 完成替换
- round-trip 测试覆盖到位

### Slice D：文档同步

状态：`done`

- Task 8

验收重点：

- `docs_dev` 描述与实际实现一致

## 7. 开发时的约束

实现本专题时，建议严格遵守以下约束：

- 不新增第二套“模板配置结构”
- 不让 `Resolved*` 参与模板输出
- 不在 `orchestrator` 中继续维护 schema 细节
- 不把“模板最小化风格控制”做成过度复杂的框架
- 新增字段时优先更新 `ProjectConfig` 和 preset builder，而不是额外复制一层数据模型

## 8. 最小测试清单

合入前至少执行并通过：

1. `cargo test` 中的配置模块测试
2. `cargo test` 中的 `run_init` 相关测试
3. minimal round-trip 测试
4. full round-trip 测试

如果测试命名需要新建，建议保持一眼可读，例如：

- `project_config_can_serialize_to_yaml`
- `preset_minimal_round_trips_through_yaml`
- `preset_full_round_trips_through_yaml`
- `init_writes_structured_minimal_template`
- `init_writes_structured_full_template`

## 9. 直接开工建议

如果现在立刻开始编码，建议按下面顺序操作：

1. 先在 `src/config.rs` 给原始结构补 `Serialize`
2. 立即补一条最小序列化测试
3. 再做 `preset_minimal()` 和 `preset_full()`
4. 接着实现统一 YAML render
5. 最后替换 `src/orchestrator.rs` 的 `run_init(...)`

这样每一步都能单独验证，不会一下子改太多导致回归难查。
