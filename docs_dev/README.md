# docs_dev

本目录用于沉淀 `onekey-run-rs` 在规划设计阶段必须先写清楚的设计文档和工程约定。

当前项目还处于早期，优先目标不是补充大量实现细节，而是先把以下问题写死：

- 这个工具解决什么问题，不解决什么问题
- CLI 对用户暴露哪些稳定行为
- `onekey-tasks.yaml` 的结构、默认值和兼容策略
- 多进程编排的生命周期语义
- 工程实现分层、测试边界和错误处理原则

## 文档清单

| 文件 | 目的 | 当前状态 |
| --- | --- | --- |
| `01_scope_and_goals.md` | 定义产品定位、目标用户、范围和非目标 | draft |
| `02_cli_contract.md` | 定义 CLI 命令面、输入输出和退出码约定 | draft |
| `03_config_schema.md` | 定义 `onekey-tasks.yaml` 的字段模型和校验规则 | draft |
| `04_runtime_contract.md` | 定义启动、就绪、失败、重启、停止等运行时语义 | draft |
| `05_architecture_plan.md` | 定义代码结构、核心模块和实现分层 | draft |
| `06_engineering_conventions.md` | 定义必须落盘的工程约定 | draft |
| `07_decision_log.md` | 跟踪设计期未决问题和已确认决策 | active |
| `08_cross_platform_strategy.md` | 定义 Unix-like / Windows 的进程控制差异和实现策略 | draft |
| `09_runtime_state_persistence.md` | 定义 `down` 命令依赖的 pid / state / lock 落盘规则 | draft |
| `10_logging_design.md` | 定义日志文件写入、容量上限、rotate/archive 语义与命名规则 | draft |

## 规划阶段必须准备的文档

在进入正式开发前，至少需要把以下文档写到“可执行”的程度：

1. 范围文档
   明确项目边界，避免把轻量编排工具做成半个容器平台。
2. CLI 契约文档
   明确首版命令、参数、输出和退出码，避免实现期频繁改用户接口。
3. 配置 Schema 文档
   明确 YAML 字段、默认值、兼容策略和校验错误格式，这是整个项目最核心的契约。
4. 运行时契约文档
   明确依赖顺序、PID 存活判定、失败策略、信号转发和关停流程，这是行为一致性的基础。
5. 架构设计文档
   明确模块边界、状态流转和依赖方向，防止逻辑堆进 `main.rs`。
6. 工程约定文档
   明确命名、错误处理、日志、测试和平台支持约定，减少实现阶段风格分裂。
7. 决策日志
   对仍有争议的问题保留记录，避免口头决策丢失。
8. 跨平台策略文档
   明确 Windows 与 Unix-like 的实现边界、差异点和阶段性目标。
9. 运行时状态持久化文档
   明确 `up` / `down` 如何通过状态文件协作，否则 `down` 的语义无法稳定落地。

## 文档使用顺序

建议按以下顺序推进设计：

1. 先确认 [01_scope_and_goals.md](/Users/wolf/RustroverProjects/onekey-run-rs/docs_dev/01_scope_and_goals.md)
2. 再确认 [02_cli_contract.md](/Users/wolf/RustroverProjects/onekey-run-rs/docs_dev/02_cli_contract.md) 和 [03_config_schema.md](/Users/wolf/RustroverProjects/onekey-run-rs/docs_dev/03_config_schema.md)
3. 然后确认 [04_runtime_contract.md](/Users/wolf/RustroverProjects/onekey-run-rs/docs_dev/04_runtime_contract.md) 和 [08_cross_platform_strategy.md](/Users/wolf/RustroverProjects/onekey-run-rs/docs_dev/08_cross_platform_strategy.md)
4. 再确认 [09_runtime_state_persistence.md](/Users/wolf/RustroverProjects/onekey-run-rs/docs_dev/09_runtime_state_persistence.md)
5. 最后根据前面契约落实 [05_architecture_plan.md](/Users/wolf/RustroverProjects/onekey-run-rs/docs_dev/05_architecture_plan.md) 和 [06_engineering_conventions.md](/Users/wolf/RustroverProjects/onekey-run-rs/docs_dev/06_engineering_conventions.md)

## 进入开发的最小准入条件

以下事项未确认前，不建议进入大规模编码：

- 首版目标命令集已经确认
- 配置字段和默认值已经冻结
- PID 存活 / shutdown 语义已经确认
- 平台支持范围已经确认
- `down` 的状态落盘方案已经确认
- 错误码、日志行为和测试策略已经确认
