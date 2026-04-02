# `env` 分段拼接对象第一阶段开发任务拆分

## 1. 目的

本文档把 `docs_design/16_env_concat_design.md` 中已经收敛好的第一阶段方案，拆成一份可以直接编码落地的任务清单。

目标不是继续讨论是否要支持 `env` 引用、`path` 片段或 `path-list`，而是明确：

- 第一阶段先改哪些文件
- 每一步要交付什么
- 什么叫“做完”
- 需要补哪些测试

本专题的第一阶段最终目标是：

- `services.<name>.env` / `actions.<name>.env` 在保留普通字符串写法的同时
- 新增支持 `{ separator, parts }` 结构化 env 对象
- `parts` 在第一阶段仅支持字符串数组
- resolve 后继续产出普通 `map<string, string>`

## 2. 范围与非目标

### 2.1 本次范围

- 为 `ServiceConfig.env` / `ActionConfig.env` 引入结构化 env value
- 支持 `string | concat object` 两种 env value 形态
- 在 `check` 中补齐第一阶段静态校验
- 在 `resolve_service(...)` / `resolve_actions(...)` 中渲染 concat object
- 更新 `init --full` / `preset_full()` 对应的 full sample 配置示例
- 更新 `skills/onekey-run-config-authoring/SKILL.md` 中与配置 schema、示例和 authoring 指南相关的内容
- 保证 `to_yaml_string()` 与 `ProjectConfig::load(...)` 能 round-trip
- 补齐配置层与展示层测试

### 2.2 非目标

- 不支持 `{ env: "PATH" }`
- 不支持 `{ path: "./bin" }`
- 不支持 `separator: "path-list"`
- 不支持 `parts[*]` 为 object
- 不做 shell 级变量展开
- 不引入新的 process 层 env 模型

## 3. 任务完成定义

以下条件同时满足，视为本专题第一阶段完成：

1. `env` 继续支持普通字符串值
2. `env` 新增支持 `{ separator, parts }`
3. `parts` 仅接受非空字符串数组
4. `check` 能稳定拒绝非法 concat object 形状
5. `resolve_service(...)` / `resolve_actions(...)` 能输出最终字符串 env map
6. `list --detail` 不因新 env value 类型而回归
7. `init --full` 生成的 full sample 已体现新的 env 对象写法
8. `skills/onekey-run-config-authoring/SKILL.md` 已与新 schema 保持一致
9. `to_yaml_string()` 输出的 concat object 可被重新加载
10. 相关测试已补齐

## 4. 推荐实施顺序

建议按以下顺序推进：

1. 先引入新的 env value 数据结构
2. 再替换 `ServiceConfig.env` / `ActionConfig.env` 的类型
3. 再补 `check` 校验和错误信息
4. 再补 service / action 的 resolve 渲染逻辑
5. 再修正 `list --detail` 等原始配置展示路径
6. 再更新 `preset_full()` / `init --full` 示例
7. 再更新 `skills/onekey-run-config-authoring/SKILL.md`
8. 最后补 round-trip、校验与 resolve 测试

## 5. 任务拆分

### Task 1：引入第一阶段 env value 数据结构

状态：`done`

#### 目标

先把第一阶段需要的最小 schema 固定下来，避免后面在校验和 resolve 时到处写临时分支。

#### 需要修改的文件

- `src/config.rs`

#### 具体改动

- 新增 `EnvValueConfig`，至少包含：
  - `String`
  - `ConcatObject`
- 新增 `EnvConcatConfig` 或等价类型，字段至少包含：
  - `separator: Option<String>`
  - `parts: Vec<String>`
- 为新类型补齐：
  - `Debug`
  - `Clone`
  - `Serialize`
  - `Deserialize`
- 建议直接采用能同时接受 string 与 object 的 `serde` 形态，例如：
  - `#[serde(untagged)]`
- 明确第一阶段对象只允许：
  - `separator`
  - `parts`

#### 完成标准

- 新类型可以被 `serde_yaml` 正确读写
- 第一阶段最小对象形状已经在类型层固定下来

#### 注意事项

- 不要在这个阶段提前引入 `literal` / `env` / `path` 多态
- 不要把后续阶段的字段先塞进类型里“占位”

### Task 2：替换 `ServiceConfig` / `ActionConfig` 的 `env` 类型

状态：`done`

#### 目标

让原始配置模型正式支持新的 env value，而不是继续固定死为 `BTreeMap<String, String>`。

#### 需要修改的文件

- `src/config.rs`

#### 具体改动

- 将以下字段从：
  - `BTreeMap<String, String>`
  改为：
  - `BTreeMap<String, EnvValueConfig>`
- 影响字段：
  - `ServiceConfig.env`
  - `ActionConfig.env`
- 保持以下结构不变：
  - `ResolvedServiceConfig.env`
  - `ResolvedActionConfig.env`
- 检查 preset / builder / 默认空值初始化逻辑，确保仍使用空 map 初始化

#### 完成标准

- 原始配置可同时表达：
  - `RUST_LOG: "info"`
  - `JAVA_TOOL_OPTIONS: { separator: " ", parts: [...] }`
- resolve 侧类型仍保持字符串 env map

#### 风险点

- 现有 `skip_serializing_if = "BTreeMap::is_empty"` 不能丢
- 新类型切入后，所有直接假设 `env` 为 `String` 的代码都要重新编译过一遍

### Task 3：补齐第一阶段 `check` 校验规则

状态：`done`

#### 目标

把第一阶段允许和不允许的形状，在配置校验阶段一次性拦住。

#### 需要修改的文件

- `src/config.rs`

#### 具体改动

- 为 `env` 增加统一校验入口，建议抽出辅助函数，例如：
  - `validate_env_map(...)`
  - `validate_env_value(...)`
- 至少校验以下规则：
  - `env.<KEY>` 必须是 string 或 concat object
  - concat object 仅允许 `separator` 与 `parts`
  - `parts` 必须存在且不能为空数组
  - `parts[*]` 必须是 string
  - `separator` 若存在必须是 string
  - `separator: "path-list"` 在第一阶段应明确拒绝
- 保持错误信息可定位到具体 service / action / env key

#### 完成标准

- 非法 concat object 在 `ProjectConfig::validate(...)` 中被拒绝
- 错误信息至少能指出：
  - service 或 action 名称
  - env key
  - 是 `parts` 还是 `separator` 出错

#### 推荐错误风格

- `service 'api' env 'JAVA_TOOL_OPTIONS' parts must be a non-empty string array`
- `action 'prepare' env 'FOO' separator must be a string`

### Task 4：实现 env concat 的 resolve 渲染逻辑

状态：`done`

#### 目标

让结构化 env 对象在真正启动前被渲染成最终字符串，不把新复杂度扩散到运行时进程层。

#### 需要修改的文件

- `src/config.rs`

#### 具体改动

- 抽出统一渲染入口，例如：
  - `resolve_env_map(...)`
  - `render_env_value(...)`
- 对 string value：
  - 原样返回
- 对 concat object：
  - 读取 `parts`
  - 过滤空字符串片段
  - 使用 `separator.unwrap_or_default()` 拼接
- 将该逻辑接入：
  - `resolve_service(...)`
  - `resolve_actions(...)`

#### 完成标准

- `ResolvedServiceConfig.env` 始终为 `BTreeMap<String, String>`
- `ResolvedActionConfig.env` 始终为 `BTreeMap<String, String>`
- 空字符串片段不会制造多余分隔符

#### 注意事项

- 第一阶段不要在 resolve 中读取宿主环境
- 第一阶段不要在 resolve 中做路径归一化

### Task 5：修正原始配置展示与 YAML round-trip 路径

状态：`done`

#### 目标

在引入新 env value 类型后，确保面向原始配置的展示和序列化路径仍然稳定。

#### 需要修改的文件

- `src/orchestrator.rs`
- `src/config.rs`

#### 具体改动

- 检查 `list --detail` 中 service / action 的 `env` 输出逻辑
- 将当前只适用于 `BTreeMap<String, String>` 的格式化逻辑扩展为适配新类型
- `list --detail` 至少应包含合并后的 env 列表，便于直接查看最终生效值
- 若同时保留原始结构化 env 展示，应把 raw 与 merged 两者区分清楚
- 第一阶段允许先采用“稳定可读”的文本展示格式，不要求把 concat object 渲染成内嵌 YAML 风格
- 确保 `to_yaml_string()` 输出的 concat object 结构不会被降格成普通字符串

#### 完成标准

- `list --detail` 在存在 concat object 时仍可正常输出 raw env
- `list --detail` 能同时输出 merged env 列表
- `ProjectConfig::to_yaml_string()` 后重新 `load(...)` 不丢失 concat object 结构

#### 风险点

- 如果展示层偷用 resolve 结果，会丢失用户原始配置意图
- 如果格式化函数只支持 `String`，编译期就会暴露问题

### Task 6：更新 full sample 配置示例

状态：`done`

#### 目标

让内置完整模板和相关 full sample 示例，能够真实体现第一阶段新增的 env concat object，而不是文档支持了、示例仍停留在旧写法。

#### 需要修改的文件

- `src/config.rs`

#### 具体改动

- 检查并更新：
  - `ProjectConfig::preset_full()`
  - `ProjectConfig::preset_full_unix()`
  - `ProjectConfig::preset_full_windows()`
- 在 full sample 中至少选取一个合适场景，改为使用：
  - `separator`
  - `parts`
- 优先选择确实适合长值拆段的示例字段，例如：
  - `JAVA_TOOL_OPTIONS`
  - 其他长 flags 类 env
- 保持示例仍然简单、可读、跨平台语义稳定

#### 完成标准

- `onekey-run init --full` 生成的模板中，至少有一个 env 示例使用 concat object
- full sample 不引入第一阶段未支持的语法
- `preset_full()` 生成结果仍可通过 `validate(...)` 和 round-trip 测试

#### 注意事项

- 第一阶段不要在 full sample 中使用 `{ env: "PATH" }`
- 第一阶段不要在 full sample 中使用 `{ path: "./bin" }`
- 第一阶段不要在 full sample 中使用 `separator: "path-list"`

### Task 7：更新 `skills/onekey-run-config-authoring/SKILL.md`

状态：`done`

#### 目标

让配置 authoring skill 与第一阶段 schema 保持一致，避免后续 AI 继续按照旧的 `env: map<string, string>` 心智生成配置。

#### 需要修改的文件

- `skills/onekey-run-config-authoring/SKILL.md`

#### 具体改动

- 更新 skill 中关于 `env` 的 schema 描述：
  - 从仅支持纯字符串 map
  - 调整为支持 `string | concat object`
- 在 compact schema reference 中补充第一阶段合法示例
- 明确写出第一阶段限制：
  - 不支持 `{ env: "PATH" }`
  - 不支持 `{ path: "./bin" }`
  - 不支持 `separator: "path-list"`
  - 不支持 `parts[*]` 为 object
- 在 authoring guidance 中增加一条：
  - 当 env 值较长、需要提高可读性时，可使用 `{ separator, parts }`
- 检查 skill 内已有示例里的 `env: {}`、普通字符串 env、长值 env 写法，确保整体叙述不冲突

#### 完成标准

- skill 中的 schema、示例、规则与第一阶段实现一致
- skill 不再暗示 `env` 只能是纯字符串 map
- skill 不会引导用户写出第一阶段尚未支持的 env concat 变体

#### 风险点

- 如果 skill 仍保留旧 schema 表述，后续自动 authoring 很容易持续生成错误配置
- 如果 skill 抢跑写入未来语法，会与当前实现不一致

### Task 8：补齐第一阶段测试

状态：`done`

#### 目标

把第一阶段的核心行为锁进测试，避免后续扩展时把最小模型弄坏。

#### 需要修改的文件

- `src/config.rs`
- `src/orchestrator.rs`

#### 具体改动

- 在 `src/config.rs` 中增加：
  - concat object 解析测试
  - concat object 校验失败测试
  - service 级 resolve 测试
  - action 级 resolve 测试
  - round-trip 测试
- 在 `src/orchestrator.rs` 中增加：
  - `list --detail` 遇到 concat object 的展示测试
- 补充 full sample 相关断言，例如：
  - `preset_full()` 序列化结果包含 concat object 示例
  - full sample round-trip 后仍保留 concat object 结构

#### 第一阶段至少覆盖的场景

1. 普通字符串 env 保持旧行为
2. `separator: ""` 时直接拼接
3. `separator: " "` 时按空格拼接
4. `separator: ","` 时按逗号拼接
5. 空字符串片段会被跳过
6. 全为空字符串片段时结果为空字符串
7. 空 `parts` 被 `check` 拒绝
8. 非字符串 `parts[*]` 被 `check` 拒绝
9. 非字符串 `separator` 被 `check` 拒绝
10. `separator: "path-list"` 在第一阶段被拒绝
11. service 与 action 两侧都支持
12. `list --detail` 不回归
13. `preset_full()` 已体现第一阶段 concat object 示例
14. full sample round-trip 不丢失 concat object 结构

#### 完成标准

- 新测试覆盖第一阶段最小能力的解析、校验、resolve、展示和 round-trip
- 现有测试语义不因 env 类型扩展而回归

## 6. 推荐文件触点总览

第一阶段高概率需要修改：

- `src/config.rs`
- `src/orchestrator.rs`
- `skills/onekey-run-config-authoring/SKILL.md`

第一阶段原则上不应需要修改：

- `src/app.rs`
- `src/tui.rs`
- process 启动相关模块

## 7. 当前执行建议

建议按下面节奏推进：

1. 先完成 `src/config.rs` 的类型与校验
2. 再完成 `resolve_service(...)` / `resolve_actions(...)`
3. 然后修 `list --detail` 的 raw config 展示
4. 再更新 `preset_full()` / `init --full` full sample
5. 再更新 `skills/onekey-run-config-authoring/SKILL.md`
6. 最后补测试并跑回归

如果中途发现展示层或 preset 示例受到影响，也应优先保持：

- 第一阶段只支持 `separator + string[] parts`
- resolve 层继续输出普通字符串 env map
- 新能力收敛在配置层，不扩散到运行时进程层
