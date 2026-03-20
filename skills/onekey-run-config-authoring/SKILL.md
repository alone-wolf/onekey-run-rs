---
name: onekey-run-config-authoring
description: Create, modify, and repair `onekey-tasks.yaml` files for projects that use `onekey-run-rs` (`onekey-run` CLI). Use when the user needs a new config, wants to adapt a config to a codebase, or needs help with services, actions, hooks, working directories, logging, dependencies, or validation errors.
---

# onekey-run config authoring

Use this skill when working on `onekey-tasks.yaml` for `onekey-run-rs`.

This skill is intentionally self-contained. Do not assume repository-local docs exist.
Do not read the current repository's Rust source or `docs_dev` just to recover schema facts that are already captured here.
Only inspect the user's target project to infer actual services, commands, directories, and env requirements.

## What this skill should do

- Create a new `onekey-tasks.yaml`
- Modify an existing `onekey-tasks.yaml`
- Repair invalid fields, broken dependency wiring, hook references, or log settings
- Infer `executable`, `args`, `cwd`, env, dependencies, actions, and hooks from a project layout
- Keep the config aligned with the current implementation, not an imagined future schema

## Execution assumption

- Assume the `onekey-run-rs` executable is already installed somewhere on the user's `PATH`
- User-facing examples in docs may show `onekey-run`, but when this skill actually needs to invoke the installed binary, use `onekey-run-rs` directly
- If validation or inspection requires invoking the tool, call `onekey-run-rs` directly
- If the command fails because `onekey-run-rs` is not found, stop all further work immediately and report the problem to the user
- Do not silently guess an install path, edit shell startup files, or continue editing configs after that failure

## Current config model

Top-level structure:

```yaml
defaults:
  stop_timeout_secs: 10
  restart: "no"

log:
  file: "./logs/onekey-run.log"

actions:
  action_name:
    executable: "..."

services:
  service_name:
    executable: "..."
```

Top-level keys:

- `defaults`
- `log`
- `actions`
- `services`

Supported `defaults` keys:

- `stop_timeout_secs`
- `restart`

Supported top-level `log` keys:

- `file`
- `append`
- `max_file_bytes`
- `overflow_strategy`
- `rotate_file_count`

Supported `actions.<name>` keys:

- required:
  - `executable`
- optional:
  - `args`
  - `cwd`
  - `env`
  - `timeout_secs`
  - `disabled`

Supported `services.<name>` keys:

- required:
  - `executable`
- optional:
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

Supported `services.<name>.hooks` keys:

- `before_start`
- `after_start_success`
- `after_start_failure`
- `before_stop`
- `after_stop_success`
- `after_stop_timeout`
- `after_stop_failure`
- `after_runtime_exit_unexpected`

Compact schema reference:

```yaml
defaults:
  stop_timeout_secs: 10            # optional
  restart: "no"                    # optional: no | on-failure | always

log:                               # optional instance log
  file: "./logs/onekey-run.log"    # required when log exists
  append: true                     # optional, default true
  max_file_bytes: 10485760         # optional, requires overflow_strategy
  overflow_strategy: "rotate"      # optional: rotate | archive
  rotate_file_count: 5             # only valid for rotate

actions:                           # optional map
  prepare-env:
    executable: "python3"          # required
    args: ["scripts/prepare.py"]   # optional
    cwd: "."                       # optional
    env: {}                        # optional
    timeout_secs: 30               # optional
    disabled: false                # optional

services:                          # required, must be non-empty
  api:
    executable: "cargo"            # required
    args: ["run"]                  # optional
    cwd: "./backend"               # optional
    env: {}                        # optional
    depends_on: ["db"]             # optional
    restart: "no"                  # optional: no | on-failure | always
    stop_signal: "term"            # optional
    stop_timeout_secs: 10          # optional
    autostart: true                # optional
    disabled: false                # optional
    log:                           # optional service stdout/stderr log
      file: "./logs/api.log"
      append: true
      max_file_bytes: 10485760
      overflow_strategy: "rotate"
      rotate_file_count: 5
    hooks:                         # optional
      before_start: ["prepare-env"]
      after_start_success: []
      after_start_failure: []
      before_stop: []
      after_stop_success: []
      after_stop_timeout: []
      after_stop_failure: []
      after_runtime_exit_unexpected: []
```

## Rules to obey

- Commands must be expressed as `executable` + `args`
- `command` is not a supported field
- `services` must not be empty
- service names must use only lowercase letters, digits, `_`, or `-`
- action names must start with an ASCII letter or digit, then use only ASCII letters, digits, `-`, or `_`
- `executable` must not be empty
- relative `cwd` is resolved relative to the config file directory
- relative log file paths are resolved relative to the config file directory
- `depends_on` only controls startup and shutdown order; it is not a health check
- every dependency in `depends_on` must exist
- dependency graphs must be acyclic
- hook-referenced actions must exist and must not be `disabled: true`
- top-level `log.file` must not resolve to the same absolute path as any `service.log.file`

## Defaults and runtime assumptions

- `stop_timeout_secs` defaults to `10`
- `autostart` defaults to `true`
- `disabled` defaults to `false`
- `log.append` defaults to `true`
- a service is considered started when the process spawns successfully and is still alive
- the current implementation does not do readiness or health checks
- shutdown order is the reverse of dependency order
- `before_start` actions run synchronously and serially before the service process is spawned
- if any `before_start` action fails or times out, that service does not start, and later dependent services should be treated as blocked by dependency failure
- process state is judged from process spawn/exit/PID state only; there is no health or readiness probe layer

## Logging model

- top-level `log` records instance/orchestrator lifecycle events for the current `onekey-run` instance
- `service.log` records that service's stdout/stderr
- `.onekey-run/events.jsonl` is the internal machine-readable event stream
- normal `up` should stay quiet: it shows running services and elapsed time, not each service's logs
- `up --tui` shows a terminal monitor and can display logs/events interactively
- `up -d` starts a background supervisor; its lifecycle should still be discoverable through runtime files and `management`
- instance and service file paths may be relative; resolve them against the config file directory

Log shape:

```yaml
log:
  file: "./logs/app.log"
  append: true
  max_file_bytes: 10485760
  overflow_strategy: "rotate"
  rotate_file_count: 5
```

Rules:

- `file` is required when `log` exists
- `append` defaults to `true`
- `max_file_bytes` requires `overflow_strategy`
- `overflow_strategy` must be `rotate` or `archive`
- `rotate` requires `rotate_file_count`
- `rotate_file_count` must be greater than `0`
- `archive` must not include `rotate_file_count`

Semantics:

- `rotate`: keep a bounded number of rotated history files
- `archive`: create additional archived files when size is exceeded; current implementation does not limit archive count via config

## Actions and hooks

Use `actions` for short-lived commands that should be attached to service lifecycle points.

Current behavior:

- `before_start` runs before spawn, serially, and is blocking
- `after_start_success` runs after a service is considered started
- `after_start_failure` runs when startup fails
- `before_stop` runs before stop logic
- `after_stop_success` runs after a successful stop path
- `after_stop_timeout` runs when graceful stop times out
- `after_stop_failure` runs when stop fails
- `after_runtime_exit_unexpected` runs when a running service exits unexpectedly

Current design guidance:

- use actions for quick tasks such as environment preparation, notifications, marker files, or cleanup helpers
- do not model long-running background processes as actions; those should be services
- keep action commands explicit and foreground

Recommended mental model:

- `services` are long-running foreground processes managed for their full lifetime
- `actions` are short-lived helper commands triggered by a hook
- `depends_on` orders service startup/shutdown only; it does not trigger actions by itself

## Placeholder support in action args

Action `args` may contain `${...}` placeholders.

Known placeholders:

- always available:
  - `${project_root}`
  - `${config_path}`
  - `${service_name}`
  - `${action_name}`
  - `${hook_name}`
  - `${service_cwd}`
  - `${service_executable}`
- available except in `before_start`:
  - `${service_pid}`
- available only in stop-related hooks:
  - `${stop_reason}`
- available only in selected failure/exit hooks:
  - `${exit_code}`
  - `${exit_status}`

Hook constraints:

- `before_start` must not use `${service_pid}`
- `before_stop`, `after_stop_success`, `after_stop_timeout`, and `after_stop_failure` may use `${stop_reason}`
- `after_start_failure`, `after_stop_success`, `after_stop_failure`, and `after_runtime_exit_unexpected` may use `${exit_code}` and `${exit_status}`

Validation rules:

- unknown placeholders are errors
- malformed placeholders are errors
- a placeholder that is not allowed for the referenced hook is an error
- if a placeholder is syntactically valid but unavailable at runtime for that hook, treat it as invalid config rather than guessing

Example:

```yaml
actions:
  notify-up:
    executable: "sh"
    args:
      - "-c"
      - "echo ${service_name} started with pid ${service_pid}"

services:
  api:
    executable: "cargo"
    args: ["run"]
    cwd: "./backend"
    hooks:
      after_start_success: ["notify-up"]
```

Full placeholder availability table:

| Placeholder | Meaning | Available in hooks |
| --- | --- | --- |
| `${project_root}` | directory containing the config file | all |
| `${config_path}` | absolute or resolved config path | all |
| `${service_name}` | current service name | all |
| `${action_name}` | current action name | all |
| `${hook_name}` | current hook name | all |
| `${service_cwd}` | resolved service working directory | all |
| `${service_executable}` | current service executable value | all |
| `${service_pid}` | current service pid | all except `before_start` |
| `${stop_reason}` | stop reason string | `before_stop`, `after_stop_success`, `after_stop_timeout`, `after_stop_failure` |
| `${exit_code}` | numeric exit code when available | `after_start_failure`, `after_stop_success`, `after_stop_failure`, `after_runtime_exit_unexpected` |
| `${exit_status}` | rendered exit status string | `after_start_failure`, `after_stop_success`, `after_stop_failure`, `after_runtime_exit_unexpected` |

## Relevant CLI behavior

Useful user-facing commands:

```bash
onekey-run init
onekey-run init --full
onekey-run check -c <path-to-onekey-tasks.yaml>
onekey-run up -c <path-to-onekey-tasks.yaml>
onekey-run up --tui -c <path-to-onekey-tasks.yaml>
onekey-run up -d -c <path-to-onekey-tasks.yaml>
onekey-run down -c <path-to-onekey-tasks.yaml>
onekey-run down --force -c <path-to-onekey-tasks.yaml>
onekey-run management
onekey-run management --watch
onekey-run management --json
```

Important behavior:

- `-c` / `--config` selects the config file path
- `init` and `init --full` generate platform-specific presets for the current OS
- prefer generated templates as the starting point instead of copying Unix-only examples into Windows projects or the reverse
- `down -c ...` resolves runtime state from the config file directory
- `up -d` starts a background supervisor process
- `management` lists active instances and can show status summary, runtime duration, and recent event summary

Practical command policy for this skill:

- when you need to explain usage to the user, prefer `onekey-run ...` examples
- when you need to invoke the installed binary yourself for validation, use `onekey-run-rs ...`
- if the installed binary is missing, stop and report; do not continue guessing

## Authoring workflow

1. Inspect the target project and identify long-running processes that should become services.
2. Identify any short-lived lifecycle tasks that should become actions.
3. For each service, determine:
   - executable name or path
   - args array
   - working directory
   - environment variables
   - whether it should autostart
   - whether it depends on another service
   - whether file logging is needed
   - whether hooks should trigger actions
4. Prefer stable, explicit commands over shell-heavy wrappers when possible.
5. Use `cwd` instead of embedding `cd ... && ...` in a shell string when possible.
6. Add `depends_on` only for actual startup ordering requirements.
7. Keep the config minimal unless the user asks for a fully explicit style.

## How to infer good definitions

- Use one service for each long-running process the user would otherwise start in its own terminal.
- Use one action for each quick lifecycle task the user would otherwise script around startup or shutdown.
- Prefer direct binaries first:
  - `cargo`, `python`, `node`, `npm`, `pnpm`, `yarn`, `uvicorn`, `gunicorn`, `docker`, custom binaries
- Use shell wrappers only when:
  - the project already depends on shell scripts
  - multiple commands must be chained
  - environment setup is embedded in shell syntax
- Put project-specific startup directories into `cwd` instead of baking them into command strings.
- Use `env` only for variables that are really required to start the process.
- On Windows, if a shell wrapper is unavoidable, prefer native Windows command forms such as `cmd` + `["/C", "..."]` instead of copying Unix `sh -c` examples.

Cross-platform authoring rules:

- if the target project is clearly Unix-like only, Unix commands are fine
- if the target project must run on Windows, avoid `sh`, `bash`, `sleep`, `pkill`, `export`, `&&` chains that assume POSIX shell semantics
- if the target project must run on both Windows and Unix-like systems, prefer direct executables and argument arrays over shell syntax
- if cross-platform parity is unclear, tell the user what part is platform-specific instead of pretending it is universal

Shell-wrapped example:

```yaml
executable: "sh"
args:
  - "-c"
  - "npm run dev"
```

Windows shell-wrapped example:

```yaml
executable: "cmd"
args:
  - "/C"
  - "npm run dev"
```

## Editing rules

- Do not introduce unsupported fields such as `version`, `command`, `healthcheck`, `ready`, or action-specific log fields
- Do not convert `args` into a single shell string unless the underlying command truly requires a shell
- Do not assume `restart` is operationally complete; preserve it as config intent unless the user explicitly wants to rely on current behavior
- Prefer `cwd: "."` when the process should run from the config directory
- Keep service names lowercase and stable
- Do not invent future-only fields to "prepare for extensibility"
- If a field is unknown to the current implementation, remove it rather than keeping it as a guess

## Validation workflow

After editing a config:

1. Check that every referenced dependency exists.
2. Check for dependency cycles.
3. Check that hook-referenced actions exist and are not disabled.
4. Check that all `log` combinations are valid.
5. Check that top-level and service log files do not collide.
6. Check that each service and action uses `executable` + `args`, not `command`.
7. Check that placeholder names are known and hook-compatible.
8. If possible, validate with one of:

```bash
cargo run -- check -c <path-to-onekey-tasks.yaml>
```

or

```bash
onekey-run-rs check -c <path-to-onekey-tasks.yaml>
```

If you cannot run validation:

- still perform a static review against the rules in this skill
- call out assumptions explicitly
- say which part remains unverified

## Config templates

Minimal pattern:

```yaml
defaults:
  stop_timeout_secs: 10

services:
  app:
    executable: "cargo"
    args: ["run"]
    cwd: "./backend"
```

Minimal Windows-oriented pattern:

```yaml
defaults:
  stop_timeout_secs: 10

services:
  app:
    executable: "cmd"
    args: ["/C", "timeout /T 30 /NOBREAK >NUL"]
    cwd: "."
```

More explicit pattern with instance log, actions, and hooks:

```yaml
defaults:
  stop_timeout_secs: 10
  restart: "no"

log:
  file: "./logs/onekey-run.log"
  append: true
  max_file_bytes: 10485760
  overflow_strategy: "rotate"
  rotate_file_count: 5

actions:
  prepare-env:
    executable: "python3"
    args: ["scripts/prepare.py", "${service_name}", "${hook_name}"]
    cwd: "."
    timeout_secs: 30

services:
  app:
    executable: "cargo"
    args: ["run"]
    cwd: "./backend"
    env:
      RUST_LOG: "info"
    log:
      file: "./logs/app.log"
      append: true
      max_file_bytes: 10485760
      overflow_strategy: "rotate"
      rotate_file_count: 5
    depends_on: []
    restart: "no"
    stop_signal: "term"
    stop_timeout_secs: 10
    autostart: true
    disabled: false
    hooks:
      before_start: ["prepare-env"]
```

## Good patterns

Simple binary:

```yaml
services:
  api:
    executable: "./bin/api-server"
    args: []
    cwd: "."
```

Subdirectory project:

```yaml
services:
  web:
    executable: "npm"
    args: ["run", "dev"]
    cwd: "./frontend"
```

Dependency chain:

```yaml
services:
  db:
    executable: "docker"
    args: ["compose", "up", "db"]
    cwd: "."

  api:
    executable: "cargo"
    args: ["run"]
    cwd: "./backend"
    depends_on: ["db"]
```

Hooked service with explicit action:

```yaml
actions:
  prepare-api:
    executable: "python3"
    args: ["scripts/prepare.py", "${service_name}"]
    cwd: "."
    timeout_secs: 20

services:
  api:
    executable: "cargo"
    args: ["run"]
    cwd: "./backend"
    hooks:
      before_start: ["prepare-api"]
```

## Output style

When you change a config, explain:

- which services or actions were added or changed
- how `executable`, `args`, and `cwd` were chosen
- which dependencies were encoded in `depends_on`
- which hooks were added and why
- any assumptions that still need user confirmation

## When to be careful

- monorepos where each service lives in a different subdirectory
- cross-platform commands that work on Unix-like systems but not Windows
- projects where the user wants `up -d`, `management`, TUI, or instance/service logging to matter operationally
- projects where startup commands differ between development and production
- services that daemonize themselves, because `onekey-run` expects to supervise a foreground process
- hook actions that depend on hook-specific placeholders or platform-specific shell syntax
- requests that silently assume health checks, readiness gates, or post-start waiting, because the current model does not provide those
