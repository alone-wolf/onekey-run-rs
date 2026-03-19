---
name: onekey-run-config-authoring
description: Create, modify, and repair `onekey-tasks.yaml` files for projects that use `onekey-run-rs`. Use when the user wants a new config, wants to adapt an existing config to a codebase, or needs help fixing invalid fields, dependency wiring, working directories, log settings, or command argument structure for onekey-run-rs.
---

# onekey-run config authoring

Use this skill when working on `onekey-tasks.yaml` for `onekey-run-rs`.

This skill is intentionally self-contained. Do not assume repository-local docs exist.

## What this skill should do

- Create a new `onekey-tasks.yaml`
- Modify an existing `onekey-tasks.yaml`
- Repair invalid config fields or broken dependency wiring
- Infer service commands, args, `cwd`, dependencies, and log config from a project layout
- Keep the config aligned with the current `onekey-run-rs` implementation, not an imagined future schema

## Current config model

Top level structure:

```yaml
defaults:
  stop_timeout_secs: 10
  restart: "no"

services:
  service_name:
    executable: "..."
```

Top level keys:

- `defaults`
- `services`

Supported `defaults` keys:

- `stop_timeout_secs`
- `restart`

Supported service keys:

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

## Rules to obey

- Commands should be expressed as **`executable` + `args`**
- `command` is not a supported field
- `services` must not be empty
- service names must use only lowercase letters, digits, or `-`
- `executable` must not be empty
- relative `cwd` is resolved relative to the config file directory
- relative log file paths are resolved relative to the config file directory
- `depends_on` only affects startup and shutdown order; it is not a health check
- every dependency in `depends_on` must exist
- dependency graphs must be acyclic

## Defaults and runtime assumptions

- `stop_timeout_secs` defaults to `10`
- `autostart` defaults to `true`
- `disabled` defaults to `false`
- `log.append` defaults to `true`
- a service is considered started when the process spawns successfully and is still alive
- the current implementation does not do readiness or health checks
- shutdown order is the reverse of dependency order

## Log rules

Supported shape:

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

## Relevant CLI behavior

Useful commands when validating or explaining a config:

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

- `-c/--config` selects the config file path
- `down -c ...` resolves runtime state from the config file directory
- `up -d` starts a background supervisor process
- `management` lists active `onekey-run` instances

Execution assumption:

- Assume the `onekey-run-rs` executable is already installed somewhere on the user's `PATH`
- If validation or inspection requires invoking `onekey-run-rs`, call it directly
- If the command fails because `onekey-run-rs` is not found, stop all further work immediately and report the problem to the user
- Do not silently fall back to guessing an install path, changing shell configuration, or editing config files further after that failure

## Authoring workflow

1. Inspect the target project and identify long-running processes that should become services.
2. For each service, determine:
   - executable name or path
   - args array
   - working directory
   - environment variables
   - whether it should autostart
   - whether it depends on another service
   - whether file logging is needed
3. Prefer stable, explicit commands over shell-heavy wrappers when possible.
4. Use `cwd` instead of embedding `cd ... && ...` in a shell string when possible.
5. Add `depends_on` only for actual startup ordering requirements.
6. Keep the config minimal unless the user asks for a fully explicit style.

## How to infer good service definitions

- Use one service for each long-running process the user would otherwise start in its own terminal.
- Prefer direct binaries first:
  - `cargo`, `python`, `node`, `npm`, `pnpm`, `yarn`, `uvicorn`, `gunicorn`, `docker`, `docker-compose`, custom binaries
- Use shell wrappers only when:
  - the project already depends on shell scripts
  - multiple commands must be chained
  - environment setup is embedded in shell syntax
- Put project-specific startup directories into `cwd` instead of baking them into command strings.
- Use `env` only for variables that are really required to start the service.

Shell-wrapped example:

```yaml
executable: "sh"
args:
  - "-c"
  - "npm run dev"
```

## Editing rules

- Do not introduce unsupported fields such as `version`, `command`, `healthcheck`, or `ready`
- Do not convert `args` into a single shell string unless the underlying command truly requires a shell
- Do not assume `restart` is fully implemented operationally; preserve it as config intent only
- Prefer `cwd: "."` when the service should run from the config directory
- Keep service names lowercase and use only lowercase letters, digits, or `-`
- Do not invent future-only fields to "prepare for extensibility"
- If a field is unknown to the current implementation, remove it rather than keeping it as a guess

## Validation workflow

After editing a config:

1. Check that every referenced dependency exists
2. Check for dependency cycles
3. Check that `log` combinations are valid
4. Check that each service uses `executable` + `args`, not `command`
5. If possible, validate with one of:

```bash
cargo run -- check -c <path-to-onekey-tasks.yaml>
```

or

```bash
onekey-run-rs check -c <path-to-onekey-tasks.yaml>
```

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

More explicit pattern:

```yaml
defaults:
  stop_timeout_secs: 10
  restart: "no"

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

## Output style

When you change a config, explain:

- which services were added or changed
- how `executable`, `args`, and `cwd` were chosen
- which dependencies were encoded in `depends_on`
- any assumptions that still need user confirmation

## When to be careful

- Monorepos where each service lives in a different subdirectory
- Cross-platform commands that work on Unix-like systems but not Windows
- Projects where the user wants `up -d`, `management`, or per-service logging behavior to be important operationally
- Projects where startup commands differ between development and production
- Services that daemonize themselves, because `onekey-run` expects to supervise a foreground process
