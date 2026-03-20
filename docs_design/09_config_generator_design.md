# 配置文件生成器设计

## 1. 背景

当前项目已经有一套可用的配置读取与校验链路：

- `src/config.rs`
  定义 `ProjectConfig`、`ServiceConfig`、`ActionConfig`、`LogConfig` 等原始配置结构
- `ProjectConfig::load(...)`
  负责读取 YAML、反序列化并执行静态校验
- `ResolvedServiceConfig` / `ResolvedActionConfig` / `ResolvedLogConfig`
  负责把原始配置进一步解析成运行时可直接使用的结果

但 `init` 生成模板的方式仍然比较原始：

- `src/orchestrator.rs` 中的 `INIT_TEMPLATE`
- `src/orchestrator.rs` 中的 `INIT_TEMPLATE_FULL`

目前它们是两段硬编码 YAML 字符串。

这会带来几个问题：

- 新增配置字段时，模板字符串容易漏改
- 模板内容与真实 schema 的一致性只能靠人工维护
- 未来若要做“配置文件生成器”，缺少可复用的数据建模与输出链路

因此需要把模板生成从“写死字符串”升级为“组合结构体并输出 YAML”。

## 2. 本次设计目标

本次希望先完成最小但稳固的基础设施：

1. 明确配置文件生成器的数据模型
2. 避免维护两套原始配置 schema
3. 将内置预置模板改为“builder 组合配置结构 -> 输出 YAML”
4. 为未来交互式生成器、项目扫描生成器保留复用空间

非目标：

- 本期不做交互式问答式配置向导
- 本期不做项目自动探测并猜测服务命令
- 本期不引入单独的“模板 schema”
- 本期不修改运行时 `Resolved*` 结构的职责边界

## 3. 关键设计结论

### 3.1 单一原始配置 Schema

本期不建议新增第二套“用于输出 YAML 的原始配置结构”。

建议直接复用现有原始配置模型：

- `ProjectConfig`
- `DefaultsConfig`
- `ServiceConfig`
- `ActionConfig`
- `LogConfig`
- `ServiceHooksConfig`

这些结构既作为：

- YAML 读取时的反序列化目标
- 预置模板与未来生成器的内存表示
- YAML 输出时的序列化来源

这样可以保证配置 schema 只有一个真源。

### 3.2 继续保留 `Resolved*` 结构

以下运行时结构继续单独保留，不参与配置文件生成：

- `ResolvedServiceConfig`
- `ResolvedActionConfig`
- `ResolvedLogConfig`

原因是它们的职责与原始配置不同：

- 原始配置强调“外部契约”
- `Resolved*` 强调“运行时直接消费后的结果”

这里保留两层结构是合理分层，而不是重复维护 schema。

### 3.3 预置模板使用 Builder，而不是字符串常量

建议把 `init` 模板生成逻辑改造成 builder：

- `ProjectConfig::preset_minimal() -> ProjectConfig`
- `ProjectConfig::preset_full() -> ProjectConfig`

或者采用自由函数形式：

- `build_preset_minimal() -> ProjectConfig`
- `build_preset_full() -> ProjectConfig`

二者都可以，推荐优先放在 `src/config.rs` 或新增 `src/config_render.rs` 中集中管理。

### 3.4 YAML 输出与“最小 / 完整”风格分离

“最小模板”和“完整模板”的差异，不应通过两套 schema 表达，而应通过：

- 相同的 `ProjectConfig`
- 不同的 builder 输入
- 必要时不同的渲染裁剪策略

换句话说：

- 配置的字段集合只有一份
- 模板输出风格可以有多种

## 4. 为什么不建议维护两套原始配置结构

如果把“读取配置”和“生成配置”分别做成两套平级结构，会产生稳定的维护风险：

1. 新增字段时一侧更新、另一侧漏更
2. 一侧字段默认值语义变化，另一侧未同步
3. 一侧允许空值，另一侧不允许
4. 文档、模板、解析、校验四处容易漂移

这些问题即使通过转换函数和测试补救，也会增加额外的同步成本。

对当前项目来说，更稳的做法是：

- 原始配置 schema 只有一份
- 运行时 resolved schema 单独存在
- 模板生成器只负责构造这唯一的一份原始配置结构

## 5. 建议的模块划分

### 5.1 `src/config.rs`

继续作为原始配置 schema 的唯一来源，负责：

- 原始配置结构定义
- `Deserialize`
- `Serialize`
- `validate(...)`
- `resolve_service(...)`
- `resolve_actions(...)`
- `resolve_project_log(...)`
- 预置模板 builder

建议新增内容：

- 为原始配置结构补充 `Serialize`
- 增加 `ProjectConfig` 的预置模板构造函数

### 5.2 `src/config_render.rs`（可选）

如果后续 `src/config.rs` 变得过大，可以新增渲染模块，负责：

- `render_project_config_yaml(...)`
- 针对最小模板的裁剪规则
- 未来更复杂的输出风格控制

本期若改动量不大，也可以先不拆文件。

### 5.3 `src/orchestrator.rs`

`run_init(...)` 不再持有模板 YAML 字符串，只负责：

1. 选择 minimal / full preset
2. 调用 YAML 渲染
3. 写入目标文件

这样 `orchestrator` 不再承担 schema 维护责任。

## 6. 推荐的数据建模方式

建议直接在现有原始配置结构上增加 `Serialize`，并为可选字段补上合适的序列化裁剪。

### 6.1 原则

- `None` 字段不输出
- 空 `Vec` / 空 `BTreeMap` 默认不输出
- 默认值是否省略，应按模板风格决定

### 6.2 建议做法

先让结构具备基础序列化能力：

- `#[derive(Serialize, Deserialize)]`

然后逐步补这些裁剪能力：

- `#[serde(skip_serializing_if = "Option::is_none")]`
- `#[serde(skip_serializing_if = "Vec::is_empty")]`
- `#[serde(skip_serializing_if = "BTreeMap::is_empty")]`

对于 `ServiceHooksConfig` 这类多字段对象，建议实现：

- `fn is_empty(&self) -> bool`

再配合：

- `#[serde(skip_serializing_if = "ServiceHooksConfig::is_empty")]`

### 6.3 关于默认值的处理

默认值有两种来源，需要区分：

1. schema 默认值
   - 例如 `autostart` 的默认语义是 `true`
   - 例如 `disabled` 的默认语义是 `false`
2. 模板显式展示值
   - `full` 模板可以故意把默认值写出来，帮助用户理解字段
   - `minimal` 模板可以省略默认值，保持更简洁

因此不要把“默认值是否展示”硬编码进 schema 结构本身。

建议优先采用：

- `preset_minimal()` 只填真正想展示的字段
- `preset_full()` 显式填入完整示例字段

这样可以避免在序列化阶段引入过重的 profile 分支逻辑。

## 7. 预置模板设计

### 7.1 `preset_minimal`

目标：

- 生成最少但可运行的示例
- 让用户快速理解最关键字段

建议包含：

- 顶层 `defaults.stop_timeout_secs`
- 至少 1~2 个 service
- service 的：
  - `executable`
  - `args`
  - `cwd`
  - `depends_on`（仅在确实需要示例时）
  - `log`

不必强制包含：

- 顶层 `actions`
- `hooks`
- `restart`
- `autostart`
- `disabled`
- 顶层实例 `log`

### 7.2 `preset_full`

目标：

- 展示当前 schema 支持的主要字段组合
- 兼作用户写配置时的参考大全

建议包含：

- `defaults`
- 顶层 `actions`
- 顶层实例 `log`（如果当前 schema 已支持）
- service 的：
  - `executable`
  - `args`
  - `cwd`
  - `env`
  - `depends_on`
  - `restart`
  - `stop_signal`
  - `stop_timeout_secs`
  - `autostart`
  - `disabled`
  - `log`
  - `hooks`

### 7.3 模板生成的边界

模板 builder 只负责：

- 生成合法配置对象
- 提供合理示例值

模板 builder 不负责：

- 根据本地项目自动推断命令
- 读取系统环境
- 检测用户机器上的真实可执行文件是否存在

## 8. YAML 输出链路建议

建议新增统一输出入口，例如：

- `ProjectConfig::to_yaml_string(&self) -> AppResult<String>`

或者：

- `render_project_config_yaml(config: &ProjectConfig) -> AppResult<String>`

内部可直接使用：

- `serde_yaml::to_string(...)`

如果后续发现 `serde_yaml` 直接输出的样式不够稳定，再考虑增加后处理，但本期不建议一开始就自写 YAML formatter。

### 8.1 输出流程

建议流程如下：

1. builder 生成 `ProjectConfig`
2. 可选：对生成结果执行一次 `validate(...)`
3. `serde_yaml::to_string(...)`
4. `run_init(...)` 写文件

这里第 2 步很重要：

- 预置模板本身应该能通过当前校验规则
- 这样可以防止未来 schema 变更后模板悄悄失效

## 9. 具体实施计划

### Phase 1：统一 schema 并打通序列化

目标：

- 让原始配置结构既能读也能写

任务：

- 为 `ProjectConfig`、`DefaultsConfig`、`ServiceConfig`、`ActionConfig`、`LogConfig`、`ServiceHooksConfig`、`RestartPolicy`、`LogOverflowStrategy` 补充 `Serialize`
- 为可选字段和空容器字段增加必要的 `skip_serializing_if`
- 为 `ServiceHooksConfig` 增加空判断方法

验收标准：

- 任意合法 `ProjectConfig` 可被序列化为 YAML
- 空字段不会大面积污染输出

### Phase 2：引入预置模板 builder

目标：

- 去掉硬编码 YAML 模板字符串

任务：

- 实现 `preset_minimal()`
- 实现 `preset_full()`
- 让 builder 直接返回 `ProjectConfig`

验收标准：

- 两个 preset 都能稳定生成配置对象
- preset 生成结果可通过现有 `validate(...)`

### Phase 3：替换 `run_init(...)`

目标：

- `init` 命令改为走结构化生成

任务：

- 删除 `INIT_TEMPLATE` / `INIT_TEMPLATE_FULL`
- `run_init(...)` 改成：
  - 选择 preset
  - 渲染 YAML
  - 写文件

验收标准：

- `onekey-run init`
  仍然生成最小模板
- `onekey-run init --full`
  仍然生成完整模板
- 行为与当前 CLI 契约保持一致

### Phase 4：补测试与回归保护

目标：

- 防止 schema 与模板生成链路再次漂移

任务：

- 为 minimal / full 模板新增测试
- 增加 round-trip 测试：
  - builder -> YAML
  - YAML -> `ProjectConfig::load(...)`
- 保留“拒绝覆盖已有配置”的现有测试

验收标准：

- minimal 模板能重新被解析并校验通过
- full 模板能重新被解析并校验通过

## 10. 推荐测试清单

建议至少覆盖以下测试。

### 10.1 序列化测试

- `ProjectConfig` 最小实例可以被序列化
- 空 `actions` 不会出现在 minimal 输出中
- 空 `hooks` 不会出现在未使用 hooks 的 service 中

### 10.2 预置模板测试

- `preset_minimal()` 生成的 YAML 包含：
  - `services`
  - `executable`
- `preset_full()` 生成的 YAML 包含：
  - `actions`
  - `hooks`
  - service 级 `log`

### 10.3 回读测试

- `preset_minimal()` 输出后可被 `ProjectConfig::load(...)` 重新解析
- `preset_full()` 输出后可被 `ProjectConfig::load(...)` 重新解析

### 10.4 CLI 回归测试

- `run_init(..., false)` 仍拒绝覆盖已有文件
- `run_init(..., true)` 生成的文件内容不是空字符串

## 11. 新增字段时的维护规则

为了防止未来继续出现模板与 schema 漂移，建议明确下面这条工程约束：

当原始配置 schema 新增字段时，必须同步检查四处：

1. `src/config.rs`
   原始配置结构与校验
2. preset builder
   minimal / full 模板是否需要展示该字段
3. 文档
   `docs_dev/03_config_schema.md` 与相关设计文档
4. round-trip 测试
   新字段加入后模板是否仍可成功回读

其中第 4 条是最关键的自动化保护。

## 12. 最终建议结论

本功能最稳妥的推进方式是：

- 不新增第二套原始配置结构
- 直接把现有 `ProjectConfig` 系列扩展为“唯一配置 schema”
- 继续保留 `Resolved*` 作为运行时结构
- 用 preset builder 取代硬编码 YAML 模板
- 用 `serde_yaml` 打通结构化输出链路
- 用 round-trip 测试保证“生成出的 YAML 一定能被当前解析器重新读回”

这样既能解决你担心的“双侧结构不同步”问题，也能为后续真正的配置文件生成器打下稳定基础。
