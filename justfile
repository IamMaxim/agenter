set dotenv-load := true

database_url := env_var_or_default("DATABASE_URL", "postgres://agenter:agenter@127.0.0.1:5432/agenter")
bind_addr := env_var_or_default("AGENTER_BIND_ADDR", "127.0.0.1:7777")
runner_token := env_var_or_default("AGENTER_DEV_RUNNER_TOKEN", "dev-runner-token")
control_plane_ws := env_var_or_default("AGENTER_CONTROL_PLANE_WS", "ws://127.0.0.1:7777/api/runner/ws")
workspace := env_var_or_default("AGENTER_WORKSPACE", ".")
admin_email := env_var_or_default("AGENTER_BOOTSTRAP_ADMIN_EMAIL", "admin@example.com")
admin_password := env_var_or_default("AGENTER_BOOTSTRAP_ADMIN_PASSWORD", "agenter-dev-password")

default:
    @just --list

db-up:
    docker compose up -d postgres

db-down:
    docker compose down

db-reset:
    docker compose down -v
    docker compose up -d postgres

db-logs:
    docker compose logs -f postgres

logs-up:
    docker compose --profile logging up -d loki grafana promtail

logs-down:
    docker compose --profile logging stop loki grafana promtail

logs-tail:
    tail -F tmp/agenter-logs/agenter-control-plane.log tmp/agenter-logs/agenter-runner.log

db-test:
    DATABASE_URL='{{database_url}}' cargo test -p agenter-db -- --ignored

control-plane:
    DATABASE_URL='{{database_url}}' AGENTER_BIND_ADDR='{{bind_addr}}' AGENTER_DEV_RUNNER_TOKEN='{{runner_token}}' AGENTER_COOKIE_SECURE=0 AGENTER_BOOTSTRAP_ADMIN_EMAIL='{{admin_email}}' AGENTER_BOOTSTRAP_ADMIN_PASSWORD='{{admin_password}}' AGENTER_LOG_DIR=tmp/agenter-logs cargo run -p agenter-control-plane

control-plane-json:
    DATABASE_URL='{{database_url}}' AGENTER_BIND_ADDR='{{bind_addr}}' AGENTER_DEV_RUNNER_TOKEN='{{runner_token}}' AGENTER_COOKIE_SECURE=0 AGENTER_BOOTSTRAP_ADMIN_EMAIL='{{admin_email}}' AGENTER_BOOTSTRAP_ADMIN_PASSWORD='{{admin_password}}' AGENTER_LOG_FORMAT=json AGENTER_LOG_DIR=tmp/agenter-logs cargo run -p agenter-control-plane

runner mode="fake" workspace=workspace:
    AGENTER_RUNNER_MODE='{{mode}}' AGENTER_WORKSPACE='{{workspace}}' AGENTER_CONTROL_PLANE_WS='{{control_plane_ws}}' AGENTER_DEV_RUNNER_TOKEN='{{runner_token}}' AGENTER_LOG_DIR=tmp/agenter-logs cargo run -p agenter-runner --bin agenter-runner

runner-json mode="fake" workspace=workspace:
    AGENTER_RUNNER_MODE='{{mode}}' AGENTER_WORKSPACE='{{workspace}}' AGENTER_CONTROL_PLANE_WS='{{control_plane_ws}}' AGENTER_DEV_RUNNER_TOKEN='{{runner_token}}' AGENTER_LOG_FORMAT=json AGENTER_LOG_DIR=tmp/agenter-logs cargo run -p agenter-runner --bin agenter-runner

fake-runner workspace=workspace:
    just runner fake '{{workspace}}'

codex-runner workspace=workspace:
    just runner codex '{{workspace}}'

qwen-runner workspace=workspace:
    just runner qwen '{{workspace}}'

acp-runner workspace=workspace:
    just runner acp '{{workspace}}'

gemini-runner workspace=workspace:
    just runner gemini '{{workspace}}'

opencode-runner workspace=workspace:
    just runner opencode '{{workspace}}'

codex-spike workspace prompt="Reply with one short sentence. Do not edit files or run commands.":
    RUST_LOG=codex_app_server_spike=debug,agenter_runner=debug,agenter_core=debug AGENTER_LOG_PAYLOADS=1 AGENTER_SPIKE_PROMPT='{{prompt}}' cargo run -p agenter-runner --bin codex_app_server_spike -- '{{workspace}}'

web:
    cd web && npm run dev

web-debug:
    cd web && VITE_AGENTER_DEBUG=1 npm run dev

fmt:
    cargo fmt --all -- --check

check:
    cargo check --workspace

clippy:
    cargo clippy --workspace -- -D warnings

test:
    cargo test --workspace

verify:
    cargo fmt --all -- --check
    cargo check --workspace
    cargo clippy --workspace -- -D warnings
    cargo test --workspace
    cd web && npm run check
    cd web && npm run lint
    cd web && npm run test
    cd web && npm run build
