# 架构设计规划

## 1. 设计原则

- 配置解析、依赖规划、进程管理、日志输出分层明确
- CLI 仅负责参数解析和入口调度，不承载业务逻辑
- 运行时状态必须有统一模型，避免多个模块各自维护状态
- 外部契约优先于内部实现细节

## 2. 建议模块划分

```text
src/
  main.rs
  cli.rs
  app.rs
  error.rs
  config/
  orchestrator/
  process/
  output/
```

建议职责如下：

- `cli.rs`
  定义命令和参数解析。
- `app.rs`
  负责根据 CLI 命令调度应用流程。
- `error.rs`
  定义统一错误类型和退出码映射。
- `config/`
  负责配置加载、解析、校验，以及内置模板 preset / YAML render。
- `orchestrator/`
  负责依赖图、状态机和全局编排流程。
- `process/`
  负责子进程启动、信号转发、PID 状态检查。
- `output/`
  负责日志聚合与终端输出。

## 3. 关键数据模型

实现前建议先定义以下核心模型：

- `ProjectConfig`
- `ServiceConfig`
- `RestartPolicy`
- `ActionConfig`
- `LogConfig`
- `ServiceState`
- `RunPlan`
- `RuntimeEvent`
- `RuntimeStateFile`

## 4. 关键流程

### `check`

1. 读取配置文件
2. 解析 YAML
3. 做结构校验和依赖图校验
4. 输出校验结果

### `up`

1. 读取并校验配置
2. 根据目标服务计算实际运行集合
3. 拓扑排序
4. 逐个启动并记录运行时状态
5. 进入运行监控循环
6. 收到错误或退出信号后执行统一停止流程

### `down`

1. 读取当前目录对应的运行时状态文件
2. 校验状态文件是否属于当前项目目录
3. 按依赖逆序停止已记录的服务
4. 清理状态文件和锁文件

## 5. 依赖方向约束

建议保持单向依赖：

- `cli` -> `app`
- `app` -> `config`, `orchestrator`, `output`
- `orchestrator` -> `process`, `config`
- `process` 不依赖 `cli`

避免：

- `config` 依赖运行时模块
- `process` 直接决定用户输出格式
- `main.rs` 直接操作多个底层模块细节

## 6. 平台支持策略

建议先明确：

- `v1` 是否一次性交付 Unix-like 与 Windows
- 若采用分阶段策略，哪些能力可在 Windows 上稍后补齐

## 7. 待确认问题

- 是否在首版引入事件总线模型
- 是否需要持久化运行态到本地文件
- `logs` / `ps` 如果首版不做，是否保留接口位置
