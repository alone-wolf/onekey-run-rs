# 运行时状态持久化设计

## 1. 目的

`down` 不是对当前内存中的进程表操作，而是对“此前某次 `up` 启动出来的项目实例”执行停止。因此必须定义稳定的运行时状态落盘协议。

没有这份协议，`down` 会立刻遇到几个问题：

- 去哪里找需要停止的进程
- 如何确认这些进程属于当前项目实例
- 如何避免误杀 pid 已复用的无关进程
- 当配置文件路径不是默认 `./onekey-tasks.yaml` 时，如何定位对应实例

## 2. 与当前配置结构的关系

当前 `onekey-tasks.yaml` 已不只是最初的 `defaults + services`，还包含：

- 顶层 `log`
- 顶层 `actions`
- `service.hooks`

同时 `init` / `init --full` 模板也已经体现了这些字段。

这对运行时状态持久化有两个直接影响：

1. 运行时状态不能只记“项目目录”
   - 还必须记住本次实例实际使用的 `config_path`
   - 因为同一个项目目录下理论上可以通过 `-c/--config` 选择不同配置文件启动
2. 运行时状态不应完整复制整份配置
   - `actions` / `hooks` / 顶层 `log` 仍然属于配置 schema
   - 持久化层只记录“停止和识别实例所必需”的最小事实

当前实现采用：

- 用 `config_path` 记录本次实例对应的配置文件
- 用 `state.json` 记录已启动 service 的最小运行事实
- 停止阶段如需补充 hook 信息，再按 `config_path` 最佳努力重新加载当前配置

## 3. 当前落盘文件布局

当前实现已经收敛为“项目内运行目录 + 临时全局注册表”两层。

### 3.1 项目内运行目录

路径：

- `.onekey-run/`

当前文件：

- `.onekey-run/state.json`
- `.onekey-run/lock.json`
- `.onekey-run/events.jsonl`

职责：

- `state.json`
  当前活动实例的核心运行状态
- `lock.json`
  防止同一项目目录重复 `up`
- `events.jsonl`
  记录实例、service、hook、action 生命周期事件

### 3.2 临时目录下的实例注册表

当前实现还维护：

- `${temp_dir}/onekey-run/registry.json`

职责：

- 给 `management` 提供实例列表入口
- 让工具不必扫描所有项目目录

因此，运行时持久化不再只是单个 `state.json`，而是：

- 项目内运行状态
- 项目内事件流
- 临时全局注册表

## 4. `state.json` 当前结构

当前 `src/runtime_state.rs` 中的运行时状态结构为：

```json
{
  "instance_id": "...",
  "project_root": "...",
  "config_path": "...",
  "tool_pid": 12345,
  "started_at_unix_secs": 1710000000,
  "services": [
    {
      "service_name": "app",
      "pid": 12346,
      "cwd": "...",
      "executable": "sleep",
      "args": ["30"],
      "log_file": "...",
      "stop_signal": "term",
      "stop_timeout_secs": 10,
      "platform": {
        "process_group_id": 12346
      }
    }
  ]
}
```

其中顶层字段的职责如下：

- `instance_id`
  当前实例的唯一标识，用于区分多次启动
- `project_root`
  配置文件所在目录，也是 `.onekey-run/` 的归属目录
- `config_path`
  本次实例实际使用的配置文件路径
- `tool_pid`
  当前 onekey-run 主进程 pid
- `started_at_unix_secs`
  实例启动时间戳
- `services`
  已成功进入运行态的服务列表

每个 service 记录的职责如下：

- `service_name`
  配置中的服务名
- `pid`
  实际启动出的子进程 pid
- `cwd`
  已解析后的工作目录
- `executable`
  实际执行的可执行文件
- `args`
  实际执行参数
- `log_file`
  解析后的 service 日志文件路径；仅记录 service 输出日志，不记录顶层实例日志
- `stop_signal`
  停止阶段优先使用的信号
- `stop_timeout_secs`
  停止阶段使用的等待时间
- `platform`
  平台特定识别信息；当前主要是 `process_group_id`

## 5. 为什么这些字段要持久化

### 5.1 必须持久化的字段

- `config_path`
  因为当前配置支持顶层 `actions` / `hooks` / `log`，停止阶段可能需要按原配置路径回读配置
- `pid`
  停止目标的直接标识
- `stop_signal`
  停止行为不能依赖“当前配置文件里现在写了什么”
- `stop_timeout_secs`
  同上，停止等待时间应以启动时记录为准
- `log_file`
  便于管理面或后续排障关联到具体 service 输出文件

### 5.2 不应持久化整份配置

当前不建议把下面这些内容原样塞进 `state.json`：

- 顶层 `actions`
- `service.hooks`
- 顶层实例 `log`
- `defaults`
- 整个 `services` 原始配置对象

原因：

- 这些字段属于配置 schema，不属于运行时最小事实
- 持久化整份配置会让状态文件和配置文件双向漂移
- 新增配置字段时会增加状态格式演进负担

## 6. 当前写入时机

当前实现中，`up` 的写入流程已经比较明确：

1. 获取 `.onekey-run/lock.json`
2. 创建初始 `RuntimeState`
3. 立即写入空的 `state.json`
4. 每成功启动一个 service，就把该 service 追加到 `services` 并重写 `state.json`
5. 当全部 service 启动完成后，将实例登记到临时注册表 `registry.json`
6. 运行期间持续写 `events.jsonl`

这样做的好处是：

- 中途启动失败时，清理逻辑仍能看到已启动的那部分 service
- `down` 不需要重新猜测哪些服务已经拉起
- `management` 能在实例稳定启动后读取到完整服务列表

## 7. `events.jsonl` 的职责

随着 `actions` / `hooks` 和顶层实例 `log` 的引入，单独的 `state.json` 已不足以承载可观测性信息。

因此当前实现还会把运行时事件追加到：

- `.onekey-run/events.jsonl`

典型事件包括：

- 实例启动 / 停止
- service 启动中 / 运行中 / 启动失败 / 运行期异常退出
- hook started / finished / failed
- action started / finished / failed / timed out

这里要明确边界：

- `state.json`
  保存“当前活动实例的最小状态快照”
- `events.jsonl`
  保存“按时间追加的生命周期事件流”

二者不能互相替代。

## 8. `down` 的停止语义

当前 `down` 的核心流程是：

1. 按项目目录读取 `.onekey-run/state.json`
2. 校验状态中的进程身份，避免误杀无关进程
3. 按记录顺序逆序停止 service
4. 清理 `.onekey-run/` 内运行时文件
5. 从全局注册表中注销该实例

这里有一个和新配置结构强相关的点：

- 停止 service 本身，主要依赖 `state.json` 中记录的 pid / stop signal / timeout
- 停止侧 hook 的配置，则通过 `config_path` 最佳努力重新加载当前配置

也就是说：

- service 停止的权威事实在 `state.json`
- hook / action 定义的权威事实仍在配置文件

如果 `config_path` 对应的配置已经被删除、改坏或发生不兼容变化：

- `down` 仍应尽量完成对记录中 service 的停止
- hook 执行可以降级为跳过，而不应阻塞主停止流程

## 9. 为什么实例日志不进入 `state.json`

当前配置已经支持顶层 `log` 作为实例日志配置，但不建议把它完整复制进 `state.json`。

原因：

- 顶层实例日志用于记录 onekey-run 自身事件，不影响 `down` 识别和停止目标
- 实例日志的真实写入过程已经由运行时 logger 和 `events.jsonl` 驱动
- `state.json` 的目标是小而稳定，不应承担日志 schema 镜像

因此当前约定是：

- `service.log` 的解析结果可下沉为每个 service 的 `log_file`
- 顶层 `log` 不进入 `state.json`

## 10. 当前文件格式结论

这部分已经基本落地，不再是开放问题：

- `state.json`
  使用 JSON
- `lock.json`
  使用 JSON
- `events.jsonl`
  使用 JSON Lines
- `registry.json`
  使用 JSON

这套选择的理由是：

- 结构化状态和注册表更适合 JSON
- 事件流天然适合 JSONL 追加写入
- 不需要为运行时内部文件复用 YAML schema

## 11. 后续字段演进建议

未来如果配置 schema 继续扩展，运行时状态层建议坚持以下规则：

1. 只有“停止、识别、管理面展示”真正需要的字段才进入 `state.json`
2. 配置 schema 的新字段默认不自动镜像到 `state.json`
3. 若某字段只影响 hook / action / 日志行为，优先继续通过 `config_path` 回读配置
4. 若某字段会改变停止语义或进程识别语义，才考虑新增到 `state.json`

这样可以避免：

- 配置文件 schema 与状态文件 schema 强耦合
- 模板演进带来状态格式频繁膨胀

## 12. 仍待确认的问题

虽然大方向已经实现，但还有一些细节可继续收敛：

- `config_path` 回读失败时，是否要在 `down` 输出更明确的降级提示
- `registry.json` 是否需要记录更多摘要信息，例如实例模式或最后事件时间
- `events.jsonl` 的保留策略是否需要进一步明确
- 后续若引入真正的配置文件生成器，是否要增加“由哪份模板生成”的来源标记
