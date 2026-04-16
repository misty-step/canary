import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile

errors = []


def workspace_root():
    candidate = os.environ.get("GITHUB_WORKSPACE")

    if candidate:
        return Path(candidate)

    return Path.cwd()


def read_required_text(path, label):
    try:
        return path.read_text()
    except OSError as exc:
        errors.append(f"unable to read {label} at {path}: {exc}")
        return ""


root = workspace_root()
workflow = read_required_text(root / ".github/workflows/ci.yml", "GitHub workflow")
dagger_source = read_required_text(root / "dagger/src/index.ts", "Dagger source")
dagger_config = read_required_text(root / "dagger.json", "Dagger config")
real_path = os.environ.get("PATH", "")
real_bash = shutil.which("bash", path=real_path) or "/bin/bash"
real_uname = shutil.which("uname", path=real_path) or "/usr/bin/uname"


def require(condition, message):
    if not condition:
        errors.append(message)


def path_with_shims(*shim_dirs):
    entries = [str(path) for path in shim_dirs if path]
    entries.extend(entry for entry in real_path.split(os.pathsep) if entry)
    return os.pathsep.join(dict.fromkeys(entries))


def shadow_missing_command(shim_dir, command_name):
    shadow_path = shim_dir / command_name
    shadow_path.write_text("#!/usr/bin/env bash\nexit 127\n")
    shadow_path.chmod(0o755)


def reset_shadow_commands(shim_dir):
    for shadow_path in shim_dir.iterdir():
        shadow_path.unlink()


def extract_method_body(source_text, signature):
    start = source_text.find(signature)

    if start == -1:
        errors.append(f"missing method signature: {signature}")
        return ""

    params_start = source_text.find("(", start)

    if params_start == -1:
        errors.append(f"missing parameter list for signature: {signature}")
        return ""

    depth = 0
    params_end = None

    for index in range(params_start, len(source_text)):
        char = source_text[index]

        if char == "(":
            depth += 1
        elif char == ")":
            depth -= 1
            if depth == 0:
                params_end = index
                break

    if params_end is None:
        errors.append(f"unterminated parameter list for signature: {signature}")
        return ""

    brace_start = source_text.find("{", params_end)

    if brace_start == -1:
        errors.append(f"missing method body for signature: {signature}")
        return ""

    depth = 0

    for index in range(brace_start, len(source_text)):
        char = source_text[index]

        if char == "{":
            depth += 1
        elif char == "}":
            depth -= 1
            if depth == 0:
                return source_text[brace_start + 1 : index]

    errors.append(f"unterminated method body for signature: {signature}")
    return ""


check_methods = re.findall(r"@check\(\)\s+async\s+(\w+)\(", dagger_source)
strict_body = extract_method_body(dagger_source, "async strict(")
strict_calls = re.findall(r"await this\.([A-Za-z0-9_]+)\(repo\)", strict_body)
expected_strict_calls = ["codexAgentRoles", *check_methods, "advisories"]
dagger_version_match = re.search(r'"engineVersion"\s*:\s*"v([^"]+)"', dagger_config)
required_dagger_version = (
    dagger_version_match.group(1) if dagger_version_match else None
)

require(
    check_methods,
    "dagger/src/index.ts must declare at least one @check gate",
)
require(
    strict_calls == expected_strict_calls,
    "Ci.strict must execute codexAgentRoles, every @check gate in source order, then advisories",
)
require(
    required_dagger_version is not None,
    "dagger.json must define engineVersion",
)


with tempfile.TemporaryDirectory() as tmp:
    tmp_path = Path(tmp)
    shadow_path = tmp_path / "shadow"
    shadow_path.mkdir()
    log_path = tmp_path / "dagger.log"
    docker_log_path = tmp_path / "docker.log"
    ssh_log_path = tmp_path / "ssh.log"
    mix_log_path = tmp_path / "mix.log"
    npm_log_path = tmp_path / "npm.log"
    dagger_path = tmp_path / "dagger"
    dagger_path.write_text(
        "#!/usr/bin/env bash\n"
        "if [[ \"$1\" == \"version\" ]]; then\n"
        "  if [[ -v DAGGER_STUB_VERSION ]]; then\n"
        "    version=\"$DAGGER_STUB_VERSION\"\n"
        "  else\n"
        f"    version=\"{required_dagger_version}\"\n"
        "  fi\n"
        "  printf 'dagger v%s (image://registry.dagger.io/engine:v%s) darwin/arm64/v8\\n' \"$version\" \"$version\"\n"
        "  exit 0\n"
        "fi\n"
        f"printf '%s\\n' \"$*\" >> \"{log_path}\"\n"
        "if [[ \"$EXPECT_DOCKER_CALL\" == \"1\" ]]; then\n"
        "  docker version >/dev/null\n"
        "fi\n"
    )
    dagger_path.chmod(0o755)
    docker_path = tmp_path / "docker"
    docker_path.write_text(
        "#!/usr/bin/env bash\n"
        f"printf '%s\\n' \"$*\" >> \"{docker_log_path}\"\n"
        "if [[ -n \"$DOCKER_VERSION_DELAY_SECONDS\" ]]; then\n"
        "  sleep \"$DOCKER_VERSION_DELAY_SECONDS\"\n"
        "fi\n"
        "if [[ \"$DOCKER_VERSION_STATUS\" == \"fail\" ]]; then\n"
        "  exit 1\n"
        "fi\n"
    )
    docker_path.chmod(0o755)

    colima_dir = tmp_path / ".colima"
    colima_dir.mkdir()
    (colima_dir / "ssh_config").write_text("Host colima\n")
    colima_path = tmp_path / "colima"
    colima_path.write_text(
        "#!/usr/bin/env bash\n"
        "if [[ \"$1\" == \"version\" && \"$COLIMA_VERSION_STATUS\" != \"fail\" ]]; then\n"
        "  exit 0\n"
        "fi\n"
        "if [[ \"$1\" == \"status\" && \"$COLIMA_STATUS\" != \"fail\" ]]; then\n"
        "  exit 0\n"
        "fi\n"
        "exit 1\n"
    )
    colima_path.chmod(0o755)
    ssh_path = tmp_path / "ssh"
    ssh_path.write_text(
        "#!/usr/bin/env bash\n"
        f"printf '%s\\n' \"$*\" >> \"{ssh_log_path}\"\n"
    )
    ssh_path.chmod(0o755)
    mix_path = tmp_path / "mix"
    mix_path.write_text(
        "#!/usr/bin/env bash\n"
        f"printf '%s\\n' \"$*\" >> \"{mix_log_path}\"\n"
    )
    mix_path.chmod(0o755)
    npm_path = tmp_path / "npm"
    npm_path.write_text(
        "#!/usr/bin/env bash\n"
        f"printf '%s\\n' \"$*\" >> \"{npm_log_path}\"\n"
    )
    npm_path.chmod(0o755)
    git_path = tmp_path / "git"
    git_path.write_text(
        "#!/usr/bin/env bash\n"
        "if [[ \"$*\" == *\"rev-parse --is-inside-work-tree\"* ]]; then\n"
        "  exit 1\n"
        "fi\n"
        "exit 0\n"
    )
    git_path.chmod(0o755)
    uname_path = tmp_path / "uname"
    uname_path.write_text(
        "#!/usr/bin/env bash\n"
        "if [[ \"$1\" == \"-s\" && -n \"$UNAME_OVERRIDE\" ]]; then\n"
        "  printf '%s\\n' \"$UNAME_OVERRIDE\"\n"
        "  exit 0\n"
        "fi\n"
        f"exec {real_uname} \"$@\"\n"
    )
    uname_path.chmod(0o755)

    env = os.environ.copy()
    env["HOME"] = tmp
    env["PATH"] = path_with_shims(shadow_path, tmp_path)

    def run(*command):
        return subprocess.run(
            [real_bash, *command],
            cwd=root,
            env=env,
            text=True,
            capture_output=True,
        )

    def read_lines(path):
        if not path.exists():
            return []
        return [line.strip() for line in path.read_text().splitlines() if line.strip()]

    def reset_logs():
        log_path.write_text("")
        docker_log_path.write_text("")
        ssh_log_path.write_text("")
        mix_log_path.write_text("")
        npm_log_path.write_text("")

    reset_logs()
    result = run("bin/validate", "--fast")
    require(
        result.returncode == 0,
        "bin/validate --fast must succeed with a valid dagger binary on PATH",
    )
    require(
        read_lines(log_path) == ["call fast"],
        "bin/validate --fast must call dagger fast exactly once",
    )

    reset_logs()
    reset_shadow_commands(shadow_path)
    stale_version = "0.20.4"
    env["DAGGER_STUB_VERSION"] = stale_version
    result = run("bin/dagger", "call", "fast")
    require(
        result.returncode != 0,
        "bin/dagger must fail fast when the installed CLI version drifts from dagger.json",
    )
    require(
        f"Installed dagger CLI version v{stale_version} does not match repo-required version v{required_dagger_version}" in result.stderr,
        "bin/dagger must explain the pinned-version mismatch",
    )
    require(
        read_lines(log_path) == [],
        "bin/dagger must stop before delegating when the installed CLI version does not match dagger.json",
    )
    env.pop("DAGGER_STUB_VERSION", None)

    reset_logs()
    result = run("bin/validate", "--advisories")
    require(
        result.returncode == 0,
        "bin/validate --advisories must succeed with a valid dagger binary on PATH",
    )
    require(
        read_lines(log_path) == ["call advisories"],
        "bin/validate --advisories must call advisories exactly once",
    )

    reset_logs()
    result = run("bin/validate", "--strict")
    require(
        result.returncode == 0,
        "bin/validate --strict must succeed with a valid dagger binary on PATH",
    )
    require(
        read_lines(log_path) == ["call strict"],
        "bin/validate --strict must delegate to the strict Dagger entrypoint",
    )

    reset_logs()
    result = run("bin/validate")
    require(
        result.returncode == 0,
        "bin/validate must succeed with a valid dagger binary on PATH",
    )
    require(
        read_lines(log_path) == ["check"],
        "bin/validate without flags must delegate to dagger check exactly once",
    )

    reset_logs()
    result = run(".githooks/pre-commit")
    require(
        result.returncode == 0,
        "pre-commit hook must succeed with a valid dagger binary on PATH",
    )
    require(
        read_lines(log_path) == ["call fast"],
        "pre-commit hook must delegate to the fast validation path",
    )

    reset_logs()
    result = run(".githooks/pre-push")
    require(
        result.returncode == 0,
        "pre-push hook must succeed with a valid dagger binary on PATH",
    )
    require(
        read_lines(log_path) == ["call strict"],
        "pre-push hook must delegate to the strict validation path",
    )

    reset_logs()
    reset_shadow_commands(shadow_path)
    env["UNAME_OVERRIDE"] = "Linux"
    result = run("bin/dagger", "call", "fast")
    require(
        result.returncode == 0,
        "bin/dagger auto mode must stay direct on non-macOS hosts",
    )
    require(
        read_lines(log_path) == ["call fast"],
        "bin/dagger auto mode must still delegate to the installed dagger binary on non-macOS hosts",
    )
    require(
        read_lines(docker_log_path) == [],
        "bin/dagger auto mode must not probe Docker on non-macOS hosts",
    )
    require(
        read_lines(ssh_log_path) == [],
        "bin/dagger auto mode must not route through SSH on non-macOS hosts",
    )
    env.pop("UNAME_OVERRIDE", None)

    reset_logs()
    reset_shadow_commands(shadow_path)
    env["UNAME_OVERRIDE"] = "Darwin"
    result = run("bin/dagger", "call", "fast")
    require(
        result.returncode == 0,
        "bin/dagger auto mode must use the active Docker client on macOS when it is available",
    )
    require(
        read_lines(log_path) == ["call fast"],
        "bin/dagger auto mode must still delegate to the installed dagger binary",
    )
    require(
        read_lines(docker_log_path) == ["version"],
        "bin/dagger auto mode must probe the local Docker client before falling back",
    )
    require(
        read_lines(ssh_log_path) == [],
        "bin/dagger auto mode must not route through SSH when direct Docker access works",
    )
    env.pop("UNAME_OVERRIDE", None)

    reset_logs()
    reset_shadow_commands(shadow_path)
    env["UNAME_OVERRIDE"] = "Darwin"
    env["CANARY_DAGGER_DOCKER_TRANSPORT"] = "direct"
    result = run("bin/dagger", "call", "fast")
    require(
        result.returncode == 0,
        "bin/dagger must support the direct transport override",
    )
    require(
        read_lines(log_path) == ["call fast"],
        "bin/dagger direct transport must still delegate to the installed dagger binary",
    )
    require(
        read_lines(docker_log_path) == [],
        "bin/dagger direct transport must not probe the Docker client first",
    )
    require(
        read_lines(ssh_log_path) == [],
        "bin/dagger direct transport must not route through SSH",
    )
    env.pop("UNAME_OVERRIDE", None)
    env.pop("CANARY_DAGGER_DOCKER_TRANSPORT", None)

    reset_logs()
    reset_shadow_commands(shadow_path)
    env["UNAME_OVERRIDE"] = "Darwin"
    env["EXPECT_DOCKER_CALL"] = "1"
    shadow_missing_command(shadow_path, "docker")
    result = run("bin/dagger", "call", "fast")
    require(
        result.returncode == 0,
        "bin/dagger auto mode must fall back to Colima over SSH when the docker binary is unavailable",
    )
    require(
        read_lines(log_path) == ["call fast"],
        "bin/dagger auto mode must still delegate to the installed dagger binary when the docker binary is unavailable",
    )
    require(
        read_lines(docker_log_path) == [],
        "bin/dagger auto mode must skip the direct probe when the docker binary is unavailable",
    )
    require(
        read_lines(ssh_log_path) == [f"-F {colima_dir / 'ssh_config'} -T colima docker version"],
        "bin/dagger auto mode must route Docker calls through Colima over SSH when the docker binary is unavailable",
    )
    reset_shadow_commands(shadow_path)
    env.pop("UNAME_OVERRIDE", None)
    env.pop("EXPECT_DOCKER_CALL", None)

    reset_logs()
    reset_shadow_commands(shadow_path)
    env["UNAME_OVERRIDE"] = "Darwin"
    shadow_missing_command(shadow_path, "docker")
    shadow_missing_command(shadow_path, "colima")
    result = run("bin/dagger", "call", "fast")
    require(
        result.returncode != 0,
        "bin/dagger auto mode must fail when neither direct Docker nor the Colima fallback is available",
    )
    require(
        "no Colima fallback is installed" in result.stderr,
        "bin/dagger auto mode must explain when the Colima fallback is unavailable",
    )
    reset_shadow_commands(shadow_path)
    env.pop("UNAME_OVERRIDE", None)

    reset_logs()
    reset_shadow_commands(shadow_path)
    env["UNAME_OVERRIDE"] = "Darwin"
    env["DOCKER_VERSION_STATUS"] = "fail"
    env["EXPECT_DOCKER_CALL"] = "1"
    result = run("bin/dagger", "call", "fast")
    require(
        result.returncode == 0,
        "bin/dagger auto mode must fall back to Colima over SSH on macOS when direct Docker access fails",
    )
    require(
        read_lines(log_path) == ["call fast"],
        "bin/dagger auto mode must still delegate to the installed dagger binary after the Colima fallback",
    )
    require(
        read_lines(docker_log_path) == ["version"],
        "bin/dagger auto mode must attempt the direct Docker probe before falling back",
    )
    require(
        read_lines(ssh_log_path) == [f"-F {colima_dir / 'ssh_config'} -T colima docker version"],
        "bin/dagger auto mode must route Docker calls through Colima over SSH after a failed direct probe",
    )
    env.pop("UNAME_OVERRIDE", None)
    env.pop("DOCKER_VERSION_STATUS", None)
    env.pop("EXPECT_DOCKER_CALL", None)

    reset_logs()
    reset_shadow_commands(shadow_path)
    env["UNAME_OVERRIDE"] = "Darwin"
    env["COLIMA_STATUS"] = "fail"
    shadow_missing_command(shadow_path, "docker")
    result = run("bin/dagger", "call", "fast")
    require(
        result.returncode != 0,
        "bin/dagger auto mode must fail when the Colima fallback is installed but not running",
    )
    require(
        "Colima fallback is not running" in result.stderr,
        "bin/dagger auto mode must explain when the Colima fallback is installed but not running",
    )
    reset_shadow_commands(shadow_path)
    env.pop("UNAME_OVERRIDE", None)
    env.pop("COLIMA_STATUS", None)

    reset_logs()
    reset_shadow_commands(shadow_path)
    env["UNAME_OVERRIDE"] = "Darwin"
    env["DOCKER_VERSION_DELAY_SECONDS"] = "4"
    env["EXPECT_DOCKER_CALL"] = "1"
    result = run("bin/dagger", "call", "fast")
    require(
        result.returncode == 0,
        "bin/dagger auto mode must fall back to Colima over SSH when the direct Docker probe hangs",
    )
    require(
        read_lines(log_path) == ["call fast"],
        "bin/dagger auto mode must still delegate to the installed dagger binary after a hung direct probe",
    )
    require(
        read_lines(docker_log_path) == ["version"],
        "bin/dagger auto mode must attempt the direct Docker probe before timing out",
    )
    require(
        read_lines(ssh_log_path) == [f"-F {colima_dir / 'ssh_config'} -T colima docker version"],
        "bin/dagger auto mode must route Docker calls through Colima over SSH after a hung direct probe",
    )
    env.pop("UNAME_OVERRIDE", None)
    env.pop("DOCKER_VERSION_DELAY_SECONDS", None)
    env.pop("EXPECT_DOCKER_CALL", None)

    reset_logs()
    reset_shadow_commands(shadow_path)
    env["CANARY_DAGGER_DOCKER_TRANSPORT"] = "colima-ssh"
    env["EXPECT_DOCKER_CALL"] = "1"
    result = run("bin/dagger", "call", "fast")
    require(
        result.returncode == 0,
        "bin/dagger must support the Colima transport override",
    )
    require(
        read_lines(log_path) == ["call fast"],
        "bin/dagger must still delegate to the installed dagger binary under the Colima transport override",
    )
    require(
        read_lines(docker_log_path) == [],
        "bin/dagger must not probe the direct Docker client when Colima transport is forced",
    )
    require(
        read_lines(ssh_log_path) == [f"-F {colima_dir / 'ssh_config'} -T colima docker version"],
        "bin/dagger must route Docker calls through Colima over SSH",
    )
    env.pop("CANARY_DAGGER_DOCKER_TRANSPORT", None)
    env.pop("EXPECT_DOCKER_CALL", None)

    reset_logs()
    reset_shadow_commands(shadow_path)
    env["UNAME_OVERRIDE"] = "Darwin"
    result = run("bin/bootstrap")
    require(
        result.returncode == 0,
        "bin/bootstrap must succeed with stubbed package managers and a valid dagger binary on PATH",
    )
    require(
        "==> tooling:" not in result.stdout,
        "bin/bootstrap must stay quiet about Docker runtimes when the active Docker client works",
    )
    require(
        read_lines(mix_log_path) == ["setup", "deps.get"],
        "bin/bootstrap must run mix setup for the root app and deps.get for the Elixir SDK",
    )
    require(
        read_lines(npm_log_path) == ["ci"],
        "bin/bootstrap must run npm ci for the TypeScript SDK",
    )
    env.pop("UNAME_OVERRIDE", None)

    reset_logs()
    reset_shadow_commands(shadow_path)
    env["UNAME_OVERRIDE"] = "Darwin"
    shadow_missing_command(shadow_path, "docker")
    shadow_missing_command(shadow_path, "colima")
    result = run("bin/bootstrap")
    require(
        result.returncode == 0,
        "bin/bootstrap must succeed when Docker and Colima are both unavailable",
    )
    require(
        "macOS local validation needs a working Docker runtime" in result.stdout,
        "bin/bootstrap must explain how to restore local validation when no Docker runtime is available on macOS",
    )
    reset_shadow_commands(shadow_path)
    env.pop("UNAME_OVERRIDE", None)

    reset_logs()
    reset_shadow_commands(shadow_path)
    env["UNAME_OVERRIDE"] = "Darwin"
    env["COLIMA_STATUS"] = "fail"
    shadow_missing_command(shadow_path, "docker")
    result = run("bin/bootstrap")
    require(
        result.returncode == 0,
        "bin/bootstrap must succeed when Colima is installed but not running",
    )
    require(
        "no working Docker runtime detected" in result.stdout,
        "bin/bootstrap must direct Colima users to start Colima when Docker probing fails on macOS",
    )
    reset_shadow_commands(shadow_path)
    env.pop("UNAME_OVERRIDE", None)
    env.pop("COLIMA_STATUS", None)

require(
    "steps.dagger_version.outputs.version" in workflow,
    "GitHub workflow must source the Dagger version from dagger.json",
)
require(
    re.search(
        r"name:\s+Run Dagger strict CI[\s\S]*?uses:\s+dagger/dagger-for-github@[\s\S]*?verb:\s+call[\s\S]*?args:\s+strict",
        workflow,
    ),
    "GitHub workflow must run the strict Dagger CI entrypoint through the Dagger action",
)
require(
    "Run Dagger codex role validation" not in workflow,
    "GitHub workflow must not duplicate codex-agent-roles outside the strict Dagger entrypoint",
)
require(
    "Run Dagger advisories" not in workflow,
    "GitHub workflow must not duplicate advisories outside the strict Dagger entrypoint",
)

if not errors:
    sys.exit(0)

for error in errors:
    print(error, file=sys.stderr)

sys.exit(1)
