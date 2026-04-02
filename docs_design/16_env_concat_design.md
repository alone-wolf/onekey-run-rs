# `env` 分段拼接对象设计

## 1. 目标

当前只规划实现一件事：

- 为 `services.<name>.env` 与 `actions.<name>.env` 增加一种结构化 env 对象
- 该对象仅包含：
  - `separator`
  - `parts`

也就是说，这一版只解决“把一个长 env 值拆成多个有序片段，再拼回一个字符串”。

## 2. 范围收敛

### 2.1 当前要支持的能力

本次仅支持两种 env value 形态：

1. 普通字符串
2. 分段拼接对象

统一模型：

```text
env: map<string, string | concat_env_object>
```

其中：

```text
concat_env_object:
  separator?: string
  parts: string[]
```

### 2.2 当前明确不做的能力

这一版不包含以下内容：

- 不支持重复 `env` key 表达拼接
- 不支持 `parts` 中的对象片段
- 不支持 `{ env: "PATH" }` 之类宿主环境引用
- 不支持 `{ path: "./bin" }` 之类路径语义
- 不支持 `separator: "path-list"` 这类特殊保留值
- 不做 shell 级变量展开
- 不做表达式语言或模板语言

换句话说，当前只做“字符串数组 + 分隔符”的最小模型。

## 3. 为什么先做这一版

这样收敛后，当前实现成本最低，同时又能解决最核心的长值可读性问题：

1. 用户可以把长 env 值拆成多段
2. 顺序由数组天然表达
3. 配置意图清晰，明确是“拼接”不是“覆盖”
4. resolve 后运行时仍然只需要普通字符串 env

这一版不引入宿主环境、路径解析、平台特化语义，可以先把基础 schema、校验和 resolve 打稳。

## 4. 推荐配置形状

### 4.1 顶层形状

建议在 `services.<name>.env` 与 `actions.<name>.env` 中统一支持以下两种写法：

```yaml
services:
  api:
    env:
      RUST_LOG: "info"
      JAVA_TOOL_OPTIONS:
        separator: " "
        parts:
          - "-Dspring.profiles.active=dev"
          - "-Dfile.encoding=UTF-8"
          - "-XX:+UseContainerSupport"
```

### 4.2 普通字符串形态

原有写法保持不变：

```yaml
env:
  APP_ENV: "dev"
  RUST_LOG: "info"
```

语义不变：

- value 原样作为最终 env 值

### 4.3 分段拼接对象形态

建议形状：

```yaml
env:
  JAVA_TOOL_OPTIONS:
    separator: " "
    parts:
      - "-Dspring.profiles.active=dev"
      - "-Dfile.encoding=UTF-8"
      - "-XX:+UseContainerSupport"
```

字段定义：

- `separator`
  - 可选
  - 缺省值为 `""`
  - 作为片段间连接符
- `parts`
  - 必填
  - 必须是非空字符串数组
  - 按声明顺序拼接

## 5. 语义定义

### 5.1 渲染时机

建议在配置 resolve 阶段完成拼接：

1. YAML 读取阶段保留结构化 env 对象
2. `check` 阶段校验形状是否合法
3. `resolve_service(...)` / `resolve_actions(...)` 渲染出最终字符串
4. 进程启动层继续只消费普通 `map<string, string>`

### 5.2 渲染规则

对某个 env key 的拼接对象，按以下规则处理：

1. 读取 `parts`
2. 按顺序取出每个字符串片段
3. 空字符串片段跳过
4. 使用 `separator` 连接剩余片段
5. 结果作为该 env key 的最终值

### 5.3 空值行为

建议空字符串片段直接跳过，原因：

1. 不会制造重复分隔符
2. 对用户更宽容
3. 后续如果由生成器或模板生成空片段，也更稳定

例如：

```yaml
env:
  MY_FLAGS:
    separator: ","
    parts:
      - "a"
      - ""
      - "b"
```

最终结果建议为：

```text
a,b
```

## 6. 校验规则

`check` 阶段建议新增以下规则：

1. `env` 若存在，必须是 object
2. `env.<KEY>` 必须是 string 或 concat object
3. 若 `env.<KEY>` 为 object：
   - 只允许 `separator` 与 `parts`
   - `parts` 必须存在
   - `parts` 必须是非空数组
4. `separator` 若存在，必须是 string
5. `parts[*]` 必须是 string
6. 不接受 `parts[*]` 为 object、number、bool 或 null
7. 不支持重复 `env` key 作为合法语法

推荐错误风格示例：

```text
config error at `services.api.env.JAVA_TOOL_OPTIONS`: env value must be a string or concat object
config error at `services.api.env.JAVA_TOOL_OPTIONS.parts`: parts must be a non-empty string array
config error at `services.api.env.JAVA_TOOL_OPTIONS.separator`: separator must be a string
```

## 7. 示例

### 7.1 用空格拼接长参数

```yaml
services:
  api:
    env:
      JAVA_TOOL_OPTIONS:
        separator: " "
        parts:
          - "-Dspring.profiles.active=dev"
          - "-Dfile.encoding=UTF-8"
          - "-XX:+UseContainerSupport"
```

### 7.2 用逗号拼接普通值

```yaml
services:
  worker:
    env:
      FEATURE_FLAGS:
        separator: ","
        parts:
          - "a"
          - "b"
          - "c"
```

### 7.3 直接拼接不加分隔符

```yaml
actions:
  build-label:
    env:
      IMAGE_TAG:
        parts:
          - "release-"
          - "2026"
          - "0401"
```

## 8. 与当前实现的关系

当前实现可视为：

```text
env: map<string, string>
```

本次变更后建议扩展为：

```text
env: map<string, EnvValueConfig>
```

其中 `EnvValueConfig` 至少包含：

- `String`
- `ConcatObject`

而 resolve 后的结果仍保持为：

```text
map<string, string>
```

因此这次改动主要集中在：

- 配置 schema
- 反序列化
- `check`
- `resolve_service(...)`
- `resolve_actions(...)`

不需要扩散到进程启动层。

## 9. 第一阶段落地规划

本节专门描述第一阶段要落地的内容，不讨论后续 `env` 引用、`path` 语义或 `path-list` 扩展。

### 9.1 第一阶段目标

第一阶段只交付一个最小但完整可用的能力：

- `env.<KEY>` 继续支持普通字符串
- `env.<KEY>` 新增支持 `{ separator, parts }` 对象
- `parts` 在第一阶段仅为 `string[]`
- resolve 后仍输出普通字符串 env map

也就是说，第一阶段只解决“长 env 值拆段 + 按顺序拼接”。

### 9.2 第一阶段代码改动范围

第一阶段建议只改配置解析链路，避免扩大影响面。

主要改动点：

- `ServiceConfig.env`
- `ActionConfig.env`
- 对应的反序列化结构
- `check`
- `resolve_service(...)`
- `resolve_actions(...)`

尽量不动：

- 进程启动层接口
- process 层 env 传递模型
- 与宿主环境读取相关的逻辑
- 路径解析逻辑

### 9.3 第一步：配置模型

- 为 `ServiceConfig.env` / `ActionConfig.env` 引入新的 value 枚举
- 增加 `ConcatObject { separator, parts }`
- `parts` 定义为 `Vec<String>`
- 保持普通字符串写法继续兼容

建议心智模型：

```text
EnvValueConfig =
  | String
  | ConcatObject { separator?: String, parts: Vec<String> }
```

### 9.4 第二步：校验

- 在 `check` 中支持 string 或 concat object
- 校验 concat object 只允许 `separator` 与 `parts`
- 校验 `parts` 为非空字符串数组
- 校验 `separator` 为 string
- 补充错误路径和错误文案

这一阶段应明确拒绝：

- `parts[*]` 为 object
- `parts[*]` 为 number / bool / null
- `separator: "path-list"`

### 9.5 第三步：resolve

- 在 `resolve_service(...)` / `resolve_actions(...)` 中渲染 concat object
- 缺省 `separator` 按 `""` 处理
- 统一空字符串片段跳过逻辑
- 最终输出普通字符串 env map

建议伪流程：

```text
if env value is string:
  use as-is
if env value is concat object:
  filter empty parts
  join with separator
```

### 9.6 第四步：序列化与文档

- 确保 `to_yaml_string()` 能保留 concat object 形状
- 更新文档示例
- 若已有 `init --full` 或生成器示例，再同步补充这种写法

## 10. 第一阶段验收标准

第一阶段至少满足：

1. 普通字符串 env 行为保持不变
2. `separator: ""` 时按顺序直接拼接
3. `separator: " "` 时按空格拼接
4. `separator: ","` 时按逗号拼接
5. 空字符串片段会被跳过
6. 空 `parts` 会被 `check` 拦截
7. 非字符串 `parts[*]` 会被 `check` 拦截
8. 非字符串 `separator` 会被 `check` 拦截
9. service 与 action 级 env 都支持
10. resolve 结果仍为普通字符串 env map
11. 序列化后仍保留 concat object 结构

## 11. 第一阶段测试建议

至少覆盖以下场景：

1. service 级普通字符串 env 保持旧行为
2. action 级普通字符串 env 保持旧行为
3. `parts` 在无 `separator` 时直接拼接
4. `separator: " "` 时正确拼接
5. `separator: ","` 时正确拼接
6. 空字符串片段被跳过
7. 全为空字符串片段时结果为空字符串
8. 空 `parts` 在 `check` 阶段报错
9. `parts[*]` 非字符串时报错
10. `separator` 非字符串时报错
11. 对象中出现未知字段时报错
12. `to_yaml_string()` 后结构仍可 round-trip

## 12. 当前结论

当前建议先把下面这个第一阶段最小模型落地：

```yaml
env:
  SOME_KEY:
    separator: " "
    parts:
      - "foo"
      - "bar"
```

先把：

- schema
- 校验
- resolve
- 文档

这四块做稳定，再决定后续是否增加宿主 env 引用、路径语义或平台特化能力。
