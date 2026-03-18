# 日志设计

## 1. 目的

本文件用于冻结日志文件写入、容量上限和超限处理策略的配置命名与行为语义。

当前项目已经支持：

- `log.file`
- `log.append`
- 单文件容量上限
- 超限后的 `rotate` / `archive` 策略

## 2. 当前配置命名

当前实现采用以下字段：

```yaml
log:
  file: "./logs/app.log"
  append: true
  max_file_bytes: 10485760
  overflow_strategy: "rotate"
  rotate_file_count: 5
```

## 3. 为什么不用更短的 key

不建议使用这类过短命名：

- `mode`
- `strategy`
- `max_files`
- `limit`

原因是放在 `log` 下仍然有歧义。例如：

- `mode` 不明确是“文件写入模式”还是“超限处理模式”
- `max_files` 不明确是“总文件数”还是“历史文件数”
- `limit` 不明确是“大小上限”还是“文件数量上限”

因此建议使用更直接的名字：

- `max_file_bytes`
- `overflow_strategy`
- `rotate_file_count`

## 4. 字段语义

### `file`

- 主日志文件路径
- 相对路径按 `onekey-tasks.yaml` 所在目录解析

### `append`

- 是否在启动时追加写入当前活动文件
- 默认值建议为 `true`
- 若为 `false`，启动时会清空活动文件重新写入

### `max_file_bytes`

- 单个活动日志文件的大小上限
- 单位为字节
- 若未配置，则表示不启用容量切换
- 当前实现要求它与 `overflow_strategy` 一起出现

### `overflow_strategy`

- 当活动日志文件达到 `max_file_bytes` 后的处理方式
- 当前实现只接受两个值：
  - `rotate`
  - `archive`

### `rotate_file_count`

- 仅在 `overflow_strategy: "rotate"` 时有效
- 表示历史轮转文件保留数量
- 当前实现要求该值大于 `0`
- 该值不包含当前活动文件本身
- 例如配置为 `5` 时：
  - 当前活动文件有 1 个
  - 额外历史文件最多保留 5 个

### 组合校验

- 仅配置 `file` / `append` 是合法的
- 配置 `max_file_bytes` 时必须同时配置 `overflow_strategy`
- `overflow_strategy: "rotate"` 时必须配置 `rotate_file_count`
- `overflow_strategy: "archive"` 时不得配置 `rotate_file_count`

## 5. `rotate` 语义

示例：

```yaml
log:
  file: "./logs/app.log"
  max_file_bytes: 10485760
  overflow_strategy: "rotate"
  rotate_file_count: 3
```

行为：

- 当前活动文件始终是 `app.log`
- 达到上限后：
  - `app.log` 变为 `app.log.1`
  - `app.log.1` 变为 `app.log.2`
  - `app.log.2` 变为 `app.log.3`
  - 超出 `rotate_file_count` 的最老文件被删除
- 新的 `app.log` 继续写入

特点：

- 历史文件数量固定
- 磁盘占用可控
- 最老历史会被淘汰

## 6. `archive` 语义

示例：

```yaml
log:
  file: "./logs/app.log"
  max_file_bytes: 10485760
  overflow_strategy: "archive"
```

行为：

- 当前活动文件初始为 `app.log`
- 达到上限后，当前内容转为归档文件，当前实现命名类似：
  - `app.log.1742011200123.001`
  - `app.log.1742011200456.002`
- 新的 `app.log` 继续写入
- 当前阶段不使用 `rotate_file_count` 控制 `archive` 文件保留数量
- 若未来需要限制归档数量，应新增独立字段，而不是复用 `rotate_file_count`

特点：

- 更适合保留完整历史
- 语义上强调“归档保留”，而不是固定窗口轮转
- 若不设置保留上限，磁盘占用会持续增长

## 7. 命名规则建议

### `rotate`

- 活动文件：`app.log`
- 历史文件：`app.log.1`、`app.log.2`、`app.log.3`

### `archive`

- 活动文件：`app.log`
- 归档文件：`app.log.<unix_millis>.<index>`

推荐原因：

- 重启后不容易和旧归档冲突
- 可直接按文件名近似排序出时间顺序

## 8. 边界行为建议

- 写入前检查是否超出 `max_file_bytes`
- 如果单条日志本身就大于上限：
  - 不拆分
  - 直接写入新活动文件
  - 允许该文件临时超过上限

## 9. 当前结论

当前实现直接采用以下 key：

- `log.file`
- `log.append`
- `log.max_file_bytes`
- `log.overflow_strategy`
- `log.rotate_file_count`

不要再引入 `mode`、`policy`、`max_files` 这类更短但更模糊的名字，也不要让 `rotate` 和 `archive` 共享同一个数量控制字段。
