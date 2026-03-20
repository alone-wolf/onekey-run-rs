# docs_design

本目录用于沉淀已经进入功能设计阶段、但尚未开始编码的专题方案。

和 `docs_dev` 的区别：

- `docs_dev`
  更偏总体规划、工程约定、现有契约
- `docs_design`
  更偏某个具体功能的详细设计、Schema 草案、状态机、执行语义、分阶段实施建议

当前文档：

| 文件 | 目的 | 状态 |
| --- | --- | --- |
| `01_actions_hooks_design.md` | 规划 `actions` 顶层配置与 service 生命周期 hooks 能力 | draft |
| `02_actions_context_variables.md` | 细化 `actions` 可用上下文变量、占位符展开规则与校验语义 | draft |
| `03_actions_hooks_schema_draft.md` | 细化 `actions` / `service.hooks` 的配置形状、字段语义与默认值建议 | draft |
| `04_actions_hooks_execution_flow.md` | 规划 orchestrator 中 hooks 的执行时序、失败传播与状态流转 | draft |
| `05_actions_hooks_check_errors.md` | 梳理 `check` 命令未来需要新增的校验项、错误分级与建议报错文本 | draft |
| `06_actions_hooks_implementation_plan.md` | 将前述设计收敛为可执行的实现阶段、任务清单、测试项与提交流程建议 | draft |
| `07_management_recent_events_design.md` | 规划 `management` 读取 `.onekey-run/events.jsonl` 并展示实例/服务最近事件摘要 | draft |
| `08_tui_events_panel_design.md` | 规划 `--tui` 中新增 Events 面板，用于展示 orchestrator 生命周期事件 | draft |
| `07_instance_log_top_level_design.md` | 规划 `onekey-tasks.yaml` 顶层 `log`，用于记录 onekey-run 实例生命周期日志并复用现有 rotate/archive 逻辑 | draft |
| `09_config_generator_design.md` | 规划配置文件生成器的建模、YAML 输出链路与分阶段实施方案，明确采用单一配置 schema + builder 的设计 | draft |
