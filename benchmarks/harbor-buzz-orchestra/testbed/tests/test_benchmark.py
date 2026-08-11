"""just benchmark must default to leaderboard-eligible settings."""

import importlib.util
import json
import sys
from pathlib import Path

import pytest
import yaml

_SCRIPT = Path(__file__).parents[2] / "scripts" / "benchmark.py"
_spec = importlib.util.spec_from_file_location("benchmark", _SCRIPT)
benchmark = importlib.util.module_from_spec(_spec)
sys.modules["benchmark"] = benchmark
_spec.loader.exec_module(benchmark)


@pytest.fixture
def state_dir(tmp_path, monkeypatch):
    monkeypatch.setattr(benchmark, "STATE_DIR", tmp_path / ".benchmark")
    return tmp_path / ".benchmark"


def test_defaults_are_leaderboard_eligible():
    args = benchmark.parse_args([])
    assert args.attempts == 5
    assert args.dataset is None and args.path is None  # dataset default applied later
    argv = benchmark.leaderboard_argv(args, Path("prov.json"), Path("linux-bin"))
    assert argv[argv.index("--dataset") + 1] == "terminal-bench/terminal-bench-2-1"
    assert argv[argv.index("--attempts") + 1] == "5"
    assert argv[argv.index("--manifest") + 1].endswith("tb-cobol-sonnet-haiku.yaml")
    assert argv[argv.index("--agent-bin-dir") + 1] == "linux-bin"
    # In-container agents reach the host relay through the forwarder gateway.
    assert argv[argv.index("--relay-gateway") + 1] == (
        f"host.docker.internal:{benchmark.RELAY_HTTP_PORT}"
    )


def test_selectors_pass_through():
    args = benchmark.parse_args(
        [
            "--path",
            "/tmp/task",
            "-i",
            "cobol*",
            "-x",
            "flaky*",
            "-k",
            "1",
            "--job-name",
            "smoke",
            "--dry-run",
        ]
    )
    argv = benchmark.leaderboard_argv(args, Path("p.json"), Path("b"))
    assert argv[argv.index("--path") + 1] == "/tmp/task"
    assert argv[argv.index("--include-task") + 1] == "cobol*"
    assert argv[argv.index("--exclude-task") + 1] == "flaky*"
    assert argv[argv.index("--attempts") + 1] == "1"
    assert "--dry-run" in argv
    assert "--dataset" not in argv


def test_state_is_generated_once_and_reused(state_dir):
    first = benchmark.load_state()
    second = benchmark.load_state()
    assert first["user_secret_key"] == second["user_secret_key"]
    assert first["owner_secret_key"] != first["user_secret_key"]
    assert len(first["user_pubkey"]) == 64
    stored = json.loads((state_dir / "state.json").read_text())
    assert "user_pubkey" not in stored  # derived, never persisted


def test_provisioner_config_pins_user_and_keeps_channels(
    state_dir, tmp_path, monkeypatch
):
    monkeypatch.setenv("FAKE_KEY_ENV", "sk-test")
    endpoints = tmp_path / "endpoints.json"
    endpoints.write_text(
        json.dumps(
            {"model-a": {"provider": "anthropic", "api_key_env": "FAKE_KEY_ENV"}}
        )
    )
    state = benchmark.load_state()
    path = benchmark.write_provisioner_config(state, endpoints)
    config = json.loads(path.read_text())
    assert config["user_secret_key"] == state["user_secret_key"]
    assert config["archive_on_teardown"] is False
    assert config["llm_api_keys"] == {"model-a": "sk-test"}
    assert str(benchmark.RELAY_HTTP_PORT) in config["relay_http_url"]
    # Both views dial the relay's canonical host-bound address; inside the
    # task container the loopback forwarder bridges it to the host gateway.
    assert config["relay_ws_url"] == f"ws://localhost:{benchmark.RELAY_HTTP_PORT}"
    assert config["relay_http_url"].startswith("http://localhost:")


def test_provisioner_config_missing_api_key_is_explicit(
    state_dir, tmp_path, monkeypatch
):
    monkeypatch.delenv("MISSING_KEY_ENV", raising=False)
    endpoints = tmp_path / "endpoints.json"
    endpoints.write_text(
        json.dumps({"model-a": {"provider": "x", "api_key_env": "MISSING_KEY_ENV"}})
    )
    with pytest.raises(SystemExit, match="MISSING_KEY_ENV"):
        benchmark.write_provisioner_config(benchmark.load_state(), endpoints)


def test_env_file_requires_an_explicit_carryforth_image(state_dir, monkeypatch):
    monkeypatch.delenv(benchmark.BENCHMARK_IMAGE_ENV, raising=False)
    with pytest.raises(SystemExit, match=benchmark.BENCHMARK_IMAGE_ENV):
        benchmark.write_env_file(benchmark.load_state())


def test_env_file_rejects_retired_block_registry(state_dir, monkeypatch):
    monkeypatch.setenv(benchmark.BENCHMARK_IMAGE_ENV, "ghcr.io/block/buzz:main")
    with pytest.raises(SystemExit, match="retired Block registry"):
        benchmark.write_env_file(benchmark.load_state())


def test_env_file_wires_owner_and_ports(state_dir, monkeypatch):
    monkeypatch.setenv(benchmark.BENCHMARK_IMAGE_ENV, "carryforth-relay:test")
    monkeypatch.delenv(benchmark.COMPOSE_PROJECT_ENV, raising=False)
    state = benchmark.load_state()
    env_path = benchmark.write_env_file(state)
    env = dict(line.split("=", 1) for line in env_path.read_text().splitlines() if line)
    assert env["RELAY_OWNER_PUBKEY"] == state["owner_pubkey"]
    assert env["BUZZ_HTTP_PORT"] == str(benchmark.RELAY_HTTP_PORT)
    assert env["BUZZ_PG_HOST_PORT"] == str(benchmark.PG_HOST_PORT)
    assert env["BUZZ_REQUIRE_RELAY_MEMBERSHIP"] == "true"
    assert env["BUZZ_IMAGE"] == "carryforth-relay:test"
    assert env["CARRYFORTH_BENCHMARK_PROJECT"] == benchmark.COMPOSE_PROJECT


def test_compose_command_isolates_the_project(state_dir, monkeypatch):
    monkeypatch.delenv(benchmark.COMPOSE_PROJECT_ENV, raising=False)
    command = benchmark.compose_command("up", "-d")
    assert command[:2] == ["docker", "compose"]
    assert command[command.index("--project-name") + 1] == "buzz-benchmark"
    files = [command[i + 1] for i, part in enumerate(command) if part == "-f"]
    assert files == [str(benchmark.COMPOSE_FILES[0])]
    assert files[0].endswith("testbed/compose.benchmark.yml")
    assert "deploy/compose" not in files[0]


def test_benchmark_compose_is_self_contained_and_local_only():
    manifest = yaml.safe_load(benchmark.COMPOSE_FILES[0].read_text())
    services = manifest["services"]
    assert {"relay", "postgres", "redis", "minio", "minio-init"} <= set(services)
    assert services["relay"]["image"].startswith("${BUZZ_IMAGE:?")
    assert services["relay"]["ports"] == [
        "127.0.0.1:${BUZZ_HTTP_PORT:-3600}:3000",
        "127.0.0.1:${BUZZ_METRICS_HOST_PORT:-9602}:9102",
    ]
    assert services["postgres"]["ports"] == [
        "127.0.0.1:${BUZZ_PG_HOST_PORT:-5633}:5432"
    ]


def test_bring_up_fails_closed_on_a_stale_credential_volume(monkeypatch):
    calls = []

    def fake_run(command, check=True):
        calls.append(command)
        raise benchmark.subprocess.CalledProcessError(1, command)

    monkeypatch.setattr(benchmark.subprocess, "run", fake_run)
    monkeypatch.setattr(benchmark, "stale_credential_volume", lambda state: True)
    with pytest.raises(SystemExit, match="refusing to delete its volumes"):
        benchmark.bring_up_stack({})
    assert [c[-3:] for c in calls] == [["up", "-d", "--wait"]]


def test_bring_up_reraises_unrelated_failures(monkeypatch):
    def fake_run(command, check=True):
        raise benchmark.subprocess.CalledProcessError(1, command)

    monkeypatch.setattr(benchmark.subprocess, "run", fake_run)
    monkeypatch.setattr(benchmark, "stale_credential_volume", lambda state: False)
    with pytest.raises(benchmark.subprocess.CalledProcessError):
        benchmark.bring_up_stack({})


def test_fresh_requires_an_explicit_non_default_project(monkeypatch):
    monkeypatch.delenv(benchmark.COMPOSE_PROJECT_ENV, raising=False)
    monkeypatch.setattr(
        benchmark.subprocess,
        "run",
        lambda *_args, **_kwargs: pytest.fail("must fail before Docker"),
    )
    with pytest.raises(SystemExit, match="explicit non-default"):
        benchmark.reset_environment()


def test_fresh_resets_only_the_explicit_project(tmp_path, monkeypatch, capsys):
    commands = []
    monkeypatch.setenv(
        benchmark.COMPOSE_PROJECT_ENV, "carryforth-benchmark-unit-checkout"
    )
    monkeypatch.setattr(
        benchmark.subprocess, "run", lambda cmd, check=True: commands.append(cmd)
    )
    monkeypatch.setattr(benchmark.sys, "platform", "darwin")
    monkeypatch.setattr(benchmark.Path, "home", classmethod(lambda cls: tmp_path))
    gui_state = tmp_path / "Library" / "WebKit" / benchmark.GUI_BUNDLE_IDENTIFIER
    gui_state.mkdir(parents=True)
    (gui_state / "localstorage.sqlite3").touch()

    benchmark.reset_environment()

    assert ["down", "-v"] == commands[0][-2:]
    assert commands[0][commands[0].index("--project-name") + 1] == (
        "carryforth-benchmark-unit-checkout"
    )
    assert "carryforth-benchmark-unit-checkout" in capsys.readouterr().err
    assert not gui_state.exists()
    assert benchmark.parse_args(["--fresh"]).fresh
    assert not benchmark.parse_args([]).fresh


def test_compose_project_rejects_unsafe_scope(monkeypatch):
    monkeypatch.setenv(benchmark.COMPOSE_PROJECT_ENV, "../../shared")
    with pytest.raises(SystemExit, match=benchmark.COMPOSE_PROJECT_ENV):
        benchmark.compose_project()
