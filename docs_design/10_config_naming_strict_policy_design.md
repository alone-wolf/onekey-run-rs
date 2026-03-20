# 配置命名严格规范与可选优化方案

## 1. 背景

当前 `onekey-tasks.yaml` 已有基础静态校验，但命名规则仍然偏“可运行即可”：

- service 名当前只要求非空，且字符属于 `a-z`、`0-9`、`-`、`_`
- action 名当前只要求首字符为 ASCII 字母或数字，后续字符属于 ASCII 字母、数字、`-`、`_`

这套规则足以防止明显非法输入，但不足以保证：

- 跨团队配置风格一致
- service / action 的语义一眼可读
- 未来自动生成、自动修复、批量重构时具备确定性
- 避免 `_` / `-` 混用、前后缀噪音、弱语义命名、跨对象歧义

因此需要单独规划一套“严格程度高于当前 `check`”的命名规范，并把它设计成可选启用的配置优化能力，而不是直接替代现有基础校验。

## 2. 目标

本方案目标：

1. 为 `onekey-tasks.yaml` 中可命名对象建立统一且严格的命名准则
2. 严格程度显著高于当前 `check` 的合法性校验
3. 保持向后兼容，默认不影响现有项目通过 `check`
4. 为未来“配置文件优化”提供稳定的诊断与自动修复基础

非目标：

- 本期不改变 YAML schema
- 本期不强制现有用户立即迁移
- 本期不尝试修改 `args` / `env` / 脚本内容中的自由文本语义

## 3. 适用范围

严格命名规范首版覆盖以下对象：

- `services.<name>`
- `actions.<name>`
- `services.<name>.depends_on[*]` 中的 service 引用
- `services.<name>.hooks.<hook>[*]` 中的 action 引用
- `service.log.file` 的相对文件名约定
- 顶层 `log.file` 的相对文件名约定

首版不纳入严格命名规则的对象：

- `env` 的 key
- `executable`
- `cwd`
- `args` 中的普通字面量

原因是这些字段常受外部程序、操作系统或既有项目目录约束，不适合采用 onekey-run 自己的强命名风格。

## 4. 当前基线与主要缺口

当前实现的命名校验偏“字符集合法”，存在以下缺口：

### 4.1 service 名过宽

按当前实现，以下名称都可能通过：

- `-`
- `_`
- `api__v2`
- `app-`
- `01`

这些名称不利于阅读、排序、路径派生和后续自动规范化。

### 4.2 action 名风格不统一

按当前实现，以下名称都可能通过：

- `Prepare_App`
- `1prepare`
- `notify_up`
- `run`

虽然技术上可解析，但缺乏统一的语义约束。

### 4.3 缺少“规范化后冲突”检测

当前没有识别这类语义上几乎等价的命名冲突：

- `api-server` vs `api_server`
- `notify-up` vs `NotifyUp`
- `db-migrate` vs `db_migrate`

如果未来引入自动优化或模板生成，这类名字会造成不确定性。

### 4.4 缺少角色语义

- service 应表达“长期运行角色”
- action 应表达“短时动作”

当前规则没有强制区分，导致 `services.run`、`actions.app` 这种弱语义命名难以及时发现。

## 5. 设计原则

- 默认兼容：现有 `check` 的基础规则继续保留
- 严格可选：新增严格命名 profile，仅在用户显式启用时生效
- 先诊断后修复：先做稳定诊断，再做自动改写
- 结构化重写：仅自动修改结构化 key 和结构化引用，不碰自由文本
- 结果确定：同一输入在任意机器上应产出相同建议和相同自动修复结果

## 6. 严格命名总规则

### 6.1 统一大小写与分隔符

严格模式下，用户定义的 key 一律使用 `lower-kebab-case`：

- 仅允许小写 ASCII 字母、数字、`-`
- 不允许 `_`
- 不允许大写字母
- 不允许连续 `--`
- 不允许首尾为 `-`

通用正则建议：

```text
^[a-z][a-z0-9]*(?:-[a-z0-9]+)*$
```

该规则比当前实现更严格，主要收紧了：

- 禁止 `_`
- 禁止首字符为数字
- 禁止单独的 `-`
- 禁止尾部 `-`
- 禁止大小写混用

### 6.2 规范化冲突检测

新增统一规范化函数：

1. 转小写
2. 将 `_` 视为 `-`
3. 折叠连续分隔符
4. 去除首尾分隔符

若两个对象规范化后相同，则视为冲突。

示例：

- `api-server` 与 `api_server` 冲突
- `Notify-Up` 与 `notify_up` 冲突

即使当前 schema 原始 key 不完全一致，严格模式也应判定为不可共存。

### 6.3 长度预算

建议限制：

- service 名长度：`2..=32`
- action 名长度：`3..=48`

原因：

- 太短可读性差
- 太长会污染日志、TUI、状态摘要和派生文件名

### 6.4 保留字

严格模式下以下名称直接禁止：

- `all`
- `default`
- `none`
- `null`
- `self`
- `services`
- `actions`
- `hooks`
- `log`
- `up`
- `down`
- `check`
- `management`

原因：

- 避免与 CLI、顶层字段、未来保留标识混淆

## 7. service 命名规则

### 7.1 硬规则

service 名必须满足：

1. 通过通用 `lower-kebab-case` 规则
2. 长度在 `2..=32`
3. 语义上表示长期运行角色，优先使用名词或名词短语
4. 不得与任何 action 名在规范化后冲突
5. 不得使用环境前缀作为主语义

禁止示例：

- `run`
- `start-app`
- `prepare-db`
- `prod-api`
- `dev-worker`
- `service-api`
- `svc-cache`

推荐示例：

- `api`
- `web`
- `worker`
- `scheduler`
- `postgres`
- `redis`
- `edge-proxy`

### 7.2 语义约束

service 名应表达“它是什么”，而不是“它做什么”。

因此以下规则建议作为严格模式 `error`：

- 首 token 落在明显动作词集合：`run`、`start`、`stop`、`prepare`、`notify`、`sync`
- 含有后缀 `-service`、前缀 `service-`、前缀 `svc-`

原因是这类命名通常重复表达了对象类型，降低信息密度。

### 7.3 环境维度处理

严格模式建议禁止把环境信息写进 service 名：

- `dev-api`
- `test-worker`
- `prod-web`

环境差异应优先放在：

- 配置文件路径
- `env`
- `cwd`
- 外部部署层

## 8. action 命名规则

### 8.1 硬规则

action 名必须满足：

1. 通过通用 `lower-kebab-case` 规则
2. 长度在 `3..=48`
3. 必须以动词开头
4. 不得与任何 service 名在规范化后冲突
5. 不得使用类型噪音后缀

禁止示例：

- `prepare_app`
- `app`
- `task-run`
- `notify-action`
- `1prepare`

推荐示例：

- `prepare-app`
- `wait-db`
- `notify-up`
- `cleanup-cache`
- `render-config`
- `migrate-schema`

### 8.2 动词白名单

为了让 action 一眼可读，严格模式建议首 token 来自可扩展白名单。

首版内置集合建议：

- `prepare`
- `wait`
- `check`
- `validate`
- `render`
- `build`
- `sync`
- `migrate`
- `seed`
- `notify`
- `register`
- `unregister`
- `warmup`
- `backup`
- `restore`
- `cleanup`
- `archive`
- `rotate`

若首 token 不在白名单中，严格模式可先给 `warning`，待项目内样本稳定后再升级为 `error`。

### 8.3 类型噪音禁用

严格模式下建议禁止：

- 后缀 `-action`
- 前缀 `action-`
- 前缀 `hook-`
- 后缀 `-hook`

原因是对象类型已由所处节点表达，不需要重复。

### 8.4 hook 语义一致性

严格模式可额外检查 action 与被引用 hook 的语义一致性。

示例规则：

- `before_start` 优先接受 `prepare`、`wait`、`check`、`render`、`migrate`
- `after_start_success` 优先接受 `notify`、`register`、`warmup`
- `before_stop` 优先接受 `notify`、`backup`、`cleanup`
- `after_runtime_exit_unexpected` 优先接受 `notify`、`archive`、`cleanup`

此项首版建议作为 `warning`，不直接阻断自动修复。

## 9. 派生命名规则

### 9.1 service 日志文件

若 `service.log.file` 是相对路径，严格模式建议文件名与 service 名保持一致：

```text
./logs/<service-name>.log
```

示例：

- service `api` -> `./logs/api.log`
- service `edge-proxy` -> `./logs/edge-proxy.log`

若 service 重命名，优化器可同步建议重写该路径的 basename。

### 9.2 顶层实例日志

顶层 `log.file` 建议固定为：

```text
./logs/onekey-run.log
```

不建议使用与 service 类似的文件名，避免实例日志和 service 日志角色混淆。

## 10. 诊断分级

严格命名模式建议引入四级结果：

- `error`
  明确违反严格规范，若用户启用严格 profile，则命令返回非零
- `warning`
  不阻断，但建议尽快优化
- `info`
  仅提供风格改进建议
- `autofix`
  可由优化器无歧义自动修复

### 10.1 典型诊断

示例：

```text
error: service `api_server` violates strict naming rule: use lower-kebab-case
autofix: rename `api_server` -> `api-server`

error: action `1prepare` violates strict naming rule: action must start with a lowercase letter
autofix: rename `1prepare` -> `prepare`

warning: action `sync` is too generic; prefer verb-object naming such as `sync-assets`

warning: service `prod-api` encodes environment in its name; move environment meaning out of the service key
```

## 11. 可选优化操作设计

### 11.1 定位

该能力不是替代当前 `check`，而是额外的可选优化操作。

建议拆成两层：

1. 严格诊断
2. 自动规范化修复

### 11.2 CLI 方向

建议未来提供以下能力之一：

```bash
onekey-run check --naming-profile strict
onekey-run config optimize --naming
onekey-run config optimize --naming --write
```

若暂不想新增 `config` 子命令，也可以先落成：

```bash
onekey-run check --naming-profile strict --suggest-fixes
```

建议优先顺序：

1. `check --naming-profile strict`
2. `config optimize --naming --write`

原因：

- 先把规则定稳
- 再做写回，风险更可控

### 11.3 优化器可安全改写的范围

自动优化首版只改以下结构化位置：

- `services` 的 key
- `actions` 的 key
- `depends_on[*]`
- `hooks.*[*]`
- `service.log.file` 的 basename

首版不自动改写：

- `args` 任意字符串中的普通字面量
- `env` value
- 外部脚本文件名
- 注释中的自由文本

原因是这些位置缺乏足够结构信息，自动修改容易误伤用户意图。

### 11.4 自动修复顺序

建议固定处理顺序：

1. 收集所有命名对象
2. 计算规范化候选名
3. 解决候选冲突
4. 生成 rename plan
5. 先改 action 及其 hook 引用
6. 再改 service 及其 `depends_on`
7. 最后同步可安全派生的日志文件名

这样可以保证写回结果稳定。

## 12. 冲突解决策略

自动修复遇到冲突时，不应静默覆盖。

建议策略：

1. 若只有大小写或 `_` / `-` 差异，则输出冲突错误并要求人工确认
2. 若规范化后为空或只剩保留字，则不给自动修复
3. 若 action/service 与另一类对象重名，则优先保留 service 名，action 需要加对象 token

示例：

- `service: api`
- `action: API`

规范化后都变为 `api`，此时建议：

- 保留 service `api`
- action 重命名为 `notify-api` 或 `prepare-api`

## 13. 分阶段实施建议

### 阶段 1：规则固化

- 在设计文档中冻结严格命名规则
- 统一 service / action 的规范化算法
- 明确保留字与长度预算

### 阶段 2：诊断引擎

- 新增命名诊断模块
- 支持 `error` / `warning` / `info` / `autofix`
- 支持按 YAML 路径输出问题定位

### 阶段 3：只读模式接入 CLI

- 支持 `check --naming-profile strict`
- 默认关闭
- 严格模式下命名 `error` 返回非零

### 阶段 4：自动优化

- 支持生成 rename plan
- 支持 dry-run 展示 before/after
- 支持 `--write` 写回 YAML

### 阶段 5：模板与生成器联动

- `init` / `init --full` 生成的默认命名必须天然符合严格规范
- 后续配置生成器输出统一采用严格规范

## 14. 验收标准

满足以下条件即可认为该规划可进入实现阶段：

1. 能明确区分“当前基础 `check`”与“可选严格命名模式”
2. 能给出稳定、可重复的命名诊断结果
3. 自动优化只修改结构化位置，不误改自由文本
4. action / service 重命名后，结构化引用保持一致
5. 生成后的 YAML 仍能通过现有 `check`
6. `init` / 配置生成器默认输出已符合严格规范

## 15. 推荐结论

建议将“严格命名规范”定位为：

- 一套高于当前 `check` 的维护性约束
- 一套面向未来配置优化器的判定基线
- 一项默认关闭、显式启用的可选能力

这样既不会打破现有用户配置，又能为后续配置整理、自动生成和批量重构提供清晰边界。
