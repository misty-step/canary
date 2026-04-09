/**
 * Dagger-first CI for the Canary monorepo.
 *
 * Canonical local gate:  ./bin/validate
 * Fast hook subset:      ./bin/dagger call fast
 */
import {
  dag,
  Container,
  Directory,
  object,
  func,
  check,
  argument,
} from "@dagger.io/dagger"

const ELIXIR_IMAGE =
  "hexpm/elixir:1.17.3-erlang-27.3-debian-bookworm-20250224-slim@sha256:e900346c099e9fe54f0be4b4cc015106798ba244b70adef029b5d9f2860ae269"
const NODE_IMAGE =
  "node:22.22.0-bookworm-slim@sha256:dd9d21971ec4395903fa6143c2b9267d048ae01ca6d3ea96f16cb30df6187d94"
const PYTHON_IMAGE =
  "python:3.13-slim-bookworm@sha256:f13a6b7565175da40695e8109f64cbc4d2e65f4c9ef2e3b321c3a44fa3c06fe7"
const DIGEST_PREFIX = "sha256:"

const CODEX_AGENT_ROLE_VALIDATION = `
from pathlib import Path
import sys
import tomllib

base = Path("/work/.codex/agents")
if not base.is_dir():
    sys.exit(0)

errors = []
for path in sorted(base.glob("*.toml")):
    try:
        tomllib.loads(path.read_text())
    except Exception as exc:
        errors.append((path, exc))

if not errors:
    sys.exit(0)

for path, exc in errors:
    print(f"{path.relative_to(Path('/work'))}: {exc}", file=sys.stderr)

sys.exit(1)
`

const CI_CONTRACT_VALIDATION = `
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile

root = Path("/work")
workflow = (root / ".github/workflows/ci.yml").read_text()

errors = []

def require(condition, message):
    if not condition:
        errors.append(message)

with tempfile.TemporaryDirectory() as tmp:
    tmp_path = Path(tmp)
    log_path = tmp_path / "dagger.log"
    ssh_log_path = tmp_path / "ssh.log"
    dagger_path = tmp_path / "dagger"
    dagger_path.write_text(
        "#!/usr/bin/env bash\\n"
        f"printf '%s\\\\n' \\"$*\\" >> \\"{log_path}\\"\\n"
        "if [[ \\"$EXPECT_DOCKER_CALL\\" == \\"1\\" ]]; then\\n"
        "  docker version >/dev/null\\n"
        "fi\\n"
    )
    dagger_path.chmod(0o755)

    env = os.environ.copy()
    env["PATH"] = f"{tmp}:{env['PATH']}"
    env["HOME"] = tmp

    colima_dir = tmp_path / ".colima"
    colima_dir.mkdir()
    (colima_dir / "ssh_config").write_text("Host colima\\n")
    colima_path = tmp_path / "colima"
    colima_path.write_text(
        "#!/usr/bin/env bash\\n"
        "if [[ \\"$1\\" == \\"status\\" ]]; then\\n"
        "  exit 0\\n"
        "fi\\n"
        "exit 1\\n"
    )
    colima_path.chmod(0o755)
    ssh_path = tmp_path / "ssh"
    ssh_path.write_text(
        "#!/usr/bin/env bash\\n"
        f"printf '%s\\\\n' \\"$*\\" >> \\"{ssh_log_path}\\"\\n"
    )
    ssh_path.chmod(0o755)

    def run(command):
        return subprocess.run(
            command,
            cwd=root,
            env=env,
            text=True,
            capture_output=True,
        )

    def read_calls():
        if not log_path.exists():
            return []
        return [line.strip() for line in log_path.read_text().splitlines() if line.strip()]

    def reset_calls():
        log_path.write_text("")
        ssh_log_path.write_text("")

    reset_calls()
    result = run(["bash", "bin/validate", "--fast"])
    require(result.returncode == 0,
            "bin/validate --fast must succeed with a valid dagger binary on PATH")
    require(read_calls() == ["call --progress=plain fast"],
            "bin/validate --fast must call dagger fast exactly once")

    reset_calls()
    result = run(["bash", "bin/validate", "--advisories"])
    require(result.returncode == 0,
            "bin/validate --advisories must succeed with a valid dagger binary on PATH")
    require(read_calls() == ["call --progress=plain advisories"],
            "bin/validate --advisories must call advisories exactly once")

    reset_calls()
    result = run(["bash", "bin/validate", "--strict"])
    require(result.returncode == 0,
            "bin/validate --strict must succeed with a valid dagger binary on PATH")
    require(
        read_calls() == [
            "call --progress=plain codex-agent-roles",
            "check --progress=plain",
            "call --progress=plain advisories",
        ],
        "bin/validate --strict must run codex-agent-roles, check, then advisories",
    )

    reset_calls()
    result = run(["bash", ".githooks/pre-commit"])
    require(result.returncode == 0,
            "pre-commit hook must succeed with a valid dagger binary on PATH")
    require(read_calls() == ["call --progress=plain fast"],
            "pre-commit hook must delegate to the fast validation path")

    reset_calls()
    result = run(["bash", ".githooks/pre-push"])
    require(result.returncode == 0,
            "pre-push hook must succeed with a valid dagger binary on PATH")
    require(
        read_calls() == [
            "call --progress=plain codex-agent-roles",
            "check --progress=plain",
            "call --progress=plain advisories",
        ],
        "pre-push hook must delegate to the strict validation path",
    )

    reset_calls()
    env["CANARY_DAGGER_DOCKER_TRANSPORT"] = "colima-ssh"
    env["EXPECT_DOCKER_CALL"] = "1"
    result = run(["bash", "bin/dagger", "call", "fast"])
    require(result.returncode == 0,
            "bin/dagger must support the Colima transport override")
    require(read_calls() == ["call fast"],
            "bin/dagger must still delegate to the installed dagger binary under the Colima transport override")
    ssh_calls = [line.strip() for line in ssh_log_path.read_text().splitlines() if line.strip()]
    require(
        ssh_calls == [f"-F {colima_dir / 'ssh_config'} -T colima docker version"],
        "bin/dagger must route Docker calls through Colima over SSH",
    )
    env.pop("CANARY_DAGGER_DOCKER_TRANSPORT", None)
    env.pop("EXPECT_DOCKER_CALL", None)

require("steps.dagger_version.outputs.version" in workflow,
        "GitHub workflow must source the Dagger version from dagger.json")
require(
    re.search(
        r"name:\\s+Run Dagger codex role validation[\\s\\S]*?uses:\\s+dagger/dagger-for-github@[\\s\\S]*?verb:\\s+call[\\s\\S]*?args:\\s+codex-agent-roles",
        workflow,
    ),
    "GitHub workflow must run codex-agent-roles through the Dagger action",
)
require(
    re.search(
        r"name:\\s+Run Dagger CI[\\s\\S]*?uses:\\s+dagger/dagger-for-github@[\\s\\S]*?verb:\\s+check",
        workflow,
    ),
    "GitHub workflow must run dagger check through the Dagger action",
)
require(
    re.search(
        r"name:\\s+Run Dagger advisories[\\s\\S]*?uses:\\s+dagger/dagger-for-github@[\\s\\S]*?verb:\\s+call[\\s\\S]*?args:\\s+advisories",
        workflow,
    ),
    "GitHub workflow must run advisories through the Dagger action",
)

if not errors:
    sys.exit(0)

for error in errors:
    print(error, file=sys.stderr)

sys.exit(1)
`

function digestSuffix(digest: string): string {
  return digest.replace(DIGEST_PREFIX, "").slice(0, 16)
}

function imageIdentity(image: string): string {
  const digest = image.split("@")[1]

  if (digest?.startsWith(DIGEST_PREFIX)) {
    return digestSuffix(digest)
  }

  return image.replace(/[^a-z0-9]+/gi, "-").slice(0, 16).toLowerCase()
}

function cacheVolumeName(namespace: string, ...parts: string[]): string {
  return [namespace, ...parts.map(digestSuffix)].join("-")
}

// Dagger's TypeScript introspector evaluates @argument values in isolation.
// Keep the ignore list literal inline at each parameter site or defaultPath breaks.
async function lockfileDigest(
  source: Directory,
  path: string,
): Promise<string> {
  return source.file(path).digest({ excludeMetadata: true })
}

/** Elixir container with system deps, Hex/Rebar caches, and mix deps fetched. */
async function elixirContainer(
  source: Directory,
  workdir: string,
  mixEnv: string,
  cacheNamespace: string,
  lockfilePath: string,
): Promise<Container> {
  const digest = await lockfileDigest(source, lockfilePath)
  const imageKey = imageIdentity(ELIXIR_IMAGE)
  const depsCache = dag.cacheVolume(
    cacheVolumeName(`${cacheNamespace}-deps`, imageKey, digest),
  )
  const buildCache = dag.cacheVolume(
    cacheVolumeName(`${cacheNamespace}-build`, imageKey, digest),
  )
  const hexCache = dag.cacheVolume(
    cacheVolumeName(`${cacheNamespace}-hex-home`, imageKey, digest),
  )
  const rebarCache = dag.cacheVolume(
    cacheVolumeName(`${cacheNamespace}-rebar-home`, imageKey, digest),
  )
  const packageDir = workdir === "." ? "/work" : `/work/${workdir}`

  return dag
    .container()
    .from(ELIXIR_IMAGE)
    .withExec(["apt-get", "update", "-q"])
    .withExec([
      "apt-get",
      "install",
      "-yq",
      "--no-install-recommends",
      "build-essential",
      "git",
    ])
    .withExec(["mix", "local.hex", "--force"])
    .withExec(["mix", "local.rebar", "--force"])
    .withEnvVariable("HOME", "/root")
    .withEnvVariable("MIX_ENV", mixEnv)
    .withMountedCache(`${packageDir}/deps`, depsCache)
    .withMountedCache(`${packageDir}/_build`, buildCache)
    .withMountedCache("/root/.hex", hexCache)
    .withMountedCache("/root/.cache/rebar3", rebarCache)
    .withMountedDirectory("/work", source)
    .withWorkdir(packageDir)
    .withExec(["mix", "deps.get"])
}

async function nodeContainer(
  source: Directory,
  workdir: string,
  lockfilePath: string,
  cacheNamespace: string,
): Promise<Container> {
  const digest = await lockfileDigest(source, lockfilePath)
  const imageKey = imageIdentity(NODE_IMAGE)
  const npmCache = dag.cacheVolume(
    cacheVolumeName(`${cacheNamespace}-npm`, imageKey, digest),
  )
  const packageDir = workdir === "." ? "/work" : `/work/${workdir}`

  return dag
    .container()
    .from(NODE_IMAGE)
    .withMountedDirectory("/work", source)
    .withMountedCache("/root/.npm", npmCache)
    .withWorkdir(packageDir)
    .withExec(["npm", "ci", "--ignore-scripts"])
}

function codexAgentRolesContainer(source: Directory): Container {
  return dag
    .container()
    .from(PYTHON_IMAGE)
    .withMountedDirectory("/work", source)
    .withWorkdir("/work")
    .withExec(["python", "-c", CODEX_AGENT_ROLE_VALIDATION])
}

function ciContractContainer(source: Directory): Container {
  return dag
    .container()
    .from(PYTHON_IMAGE)
    .withMountedDirectory("/work", source)
    .withWorkdir("/work")
    .withExec(["python", "-c", CI_CONTRACT_VALIDATION])
}

@object()
export class Ci {
  private async rootQualityContainer(source: Directory): Promise<Container> {
    return (await elixirContainer(source, ".", "test", "canary-root-test", "mix.lock"))
      .withExec(["mix", "compile", "--warnings-as-errors"])
      .withExec(["mix", "format", "--check-formatted"])
      .withExec(["mix", "credo", "--strict"])
      .withExec(["mix", "sobelow", "--config", "--exit", "--threshold", "medium"])
      .withExec(["mix", "test", "--cover"])
  }

  private async rootDialyzerContainer(source: Directory): Promise<Container> {
    return (await elixirContainer(source, ".", "dev", "canary-root-dev", "mix.lock")).withExec(
      ["mix", "dialyzer"],
    )
  }

  private async rootFastContainer(source: Directory): Promise<Container> {
    return (await elixirContainer(source, ".", "test", "canary-root-test", "mix.lock"))
      .withExec(["mix", "format", "--check-formatted"])
      .withExec(["mix", "compile", "--warnings-as-errors"])
  }

  private async sdkQualityContainer(source: Directory): Promise<Container> {
    return (
      await elixirContainer(
        source,
        "canary_sdk",
        "test",
        "canary-sdk-test",
        "canary_sdk/mix.lock",
      )
    )
      .withExec(["mix", "compile", "--warnings-as-errors"])
      .withExec(["mix", "format", "--check-formatted"])
      .withExec(["mix", "test", "--cover"])
  }

  private async sdkFastContainer(source: Directory): Promise<Container> {
    return (
      await elixirContainer(
        source,
        "canary_sdk",
        "test",
        "canary-sdk-test",
        "canary_sdk/mix.lock",
      )
    )
      .withExec(["mix", "format", "--check-formatted"])
      .withExec(["mix", "compile", "--warnings-as-errors"])
  }

  private async typescriptQualityContainer(source: Directory): Promise<Container> {
    return (
      await nodeContainer(
        source,
        "clients/typescript",
        "clients/typescript/package-lock.json",
        "canary-typescript",
      )
    )
      .withExec(["npm", "run", "typecheck"])
      .withExec(["npm", "run", "test:ci"])
      .withExec(["npm", "run", "build"])
  }

  private async typescriptFastContainer(source: Directory): Promise<Container> {
    return (
      await nodeContainer(
        source,
        "clients/typescript",
        "clients/typescript/package-lock.json",
        "canary-typescript",
      )
    ).withExec(["npm", "run", "typecheck"])
  }

  private async rootAdvisoryContainer(source: Directory): Promise<Container> {
    return (await elixirContainer(source, ".", "test", "canary-root-test", "mix.lock")).withExec(
      ["mix", "deps.audit"],
    )
  }

  private async sdkAdvisoryContainer(source: Directory): Promise<Container> {
    return (
      await elixirContainer(
        source,
        "canary_sdk",
        "test",
        "canary-sdk-test",
        "canary_sdk/mix.lock",
      )
    ).withExec(["mix", "deps.audit"])
  }

  private async typescriptAdvisoryContainer(source: Directory): Promise<Container> {
    return (
      await nodeContainer(
        source,
        "clients/typescript",
        "clients/typescript/package-lock.json",
        "canary-typescript",
      )
    ).withExec(["npm", "audit", "--audit-level", "high"])
  }

  private async openapiContractContainer(source: Directory): Promise<Container> {
    return (
      await nodeContainer(source, "dagger", "dagger/package-lock.json", "canary-dagger")
    ).withExec(["npx", "redocly", "lint", "/work/priv/openapi/openapi.json"])
  }

  @func()
  async fast(
    @argument({
      defaultPath: "/",
      ignore: [
        ".git",
        "_build",
        "deps",
        "cover",
        "canary_sdk/_build",
        "canary_sdk/deps",
        "canary_sdk/cover",
        "clients/typescript/node_modules",
        "clients/typescript/dist",
        "clients/typescript/coverage",
        "dagger/node_modules",
      ],
    })
    source?: Directory,
  ): Promise<void> {
    const repo = source!

    await codexAgentRolesContainer(repo).sync()
    await (await this.rootFastContainer(repo)).sync()
    await (await this.sdkFastContainer(repo)).sync()
    await (await this.typescriptFastContainer(repo)).sync()
  }

  @func()
  async advisories(
    @argument({
      defaultPath: "/",
      ignore: [
        ".git",
        "_build",
        "deps",
        "cover",
        "canary_sdk/_build",
        "canary_sdk/deps",
        "canary_sdk/cover",
        "clients/typescript/node_modules",
        "clients/typescript/dist",
        "clients/typescript/coverage",
        "dagger/node_modules",
      ],
    })
    source?: Directory,
  ): Promise<void> {
    const repo = source!

    await (await this.rootAdvisoryContainer(repo)).sync()
    await (await this.sdkAdvisoryContainer(repo)).sync()
    await (await this.typescriptAdvisoryContainer(repo)).sync()
  }

  @func()
  async codexAgentRoles(
    @argument({
      defaultPath: "/",
      ignore: [
        ".git",
        "_build",
        "deps",
        "cover",
        "canary_sdk/_build",
        "canary_sdk/deps",
        "canary_sdk/cover",
        "clients/typescript/node_modules",
        "clients/typescript/dist",
        "clients/typescript/coverage",
        "dagger/node_modules",
      ],
    })
    source?: Directory,
  ): Promise<void> {
    await codexAgentRolesContainer(source!).sync()
  }

  @func()
  @check()
  async ciContract(
    @argument({
      defaultPath: "/",
      ignore: [
        ".git",
        "_build",
        "deps",
        "cover",
        "canary_sdk/_build",
        "canary_sdk/deps",
        "canary_sdk/cover",
        "clients/typescript/node_modules",
        "clients/typescript/dist",
        "clients/typescript/coverage",
        "dagger/node_modules",
      ],
    })
    source?: Directory,
  ): Promise<void> {
    await ciContractContainer(source!).sync()
  }

  @func()
  @check()
  async openapiContract(
    @argument({
      defaultPath: "/",
      ignore: [
        ".git",
        "_build",
        "deps",
        "cover",
        "canary_sdk/_build",
        "canary_sdk/deps",
        "canary_sdk/cover",
        "clients/typescript/node_modules",
        "clients/typescript/dist",
        "clients/typescript/coverage",
        "dagger/node_modules",
      ],
    })
    source?: Directory,
  ): Promise<void> {
    await (await this.openapiContractContainer(source!)).sync()
  }

  @func()
  @check()
  async rootQuality(
    @argument({
      defaultPath: "/",
      ignore: [
        ".git",
        "_build",
        "deps",
        "cover",
        "canary_sdk/_build",
        "canary_sdk/deps",
        "canary_sdk/cover",
        "clients/typescript/node_modules",
        "clients/typescript/dist",
        "clients/typescript/coverage",
        "dagger/node_modules",
      ],
    })
    source?: Directory,
  ): Promise<void> {
    await (await this.rootQualityContainer(source!)).sync()
  }

  @func()
  @check()
  async rootDialyzer(
    @argument({
      defaultPath: "/",
      ignore: [
        ".git",
        "_build",
        "deps",
        "cover",
        "canary_sdk/_build",
        "canary_sdk/deps",
        "canary_sdk/cover",
        "clients/typescript/node_modules",
        "clients/typescript/dist",
        "clients/typescript/coverage",
        "dagger/node_modules",
      ],
    })
    source?: Directory,
  ): Promise<void> {
    await (await this.rootDialyzerContainer(source!)).sync()
  }

  @func()
  @check()
  async sdkQuality(
    @argument({
      defaultPath: "/",
      ignore: [
        ".git",
        "_build",
        "deps",
        "cover",
        "canary_sdk/_build",
        "canary_sdk/deps",
        "canary_sdk/cover",
        "clients/typescript/node_modules",
        "clients/typescript/dist",
        "clients/typescript/coverage",
        "dagger/node_modules",
      ],
    })
    source?: Directory,
  ): Promise<void> {
    await (await this.sdkQualityContainer(source!)).sync()
  }

  @func()
  @check()
  async typescriptQuality(
    @argument({
      defaultPath: "/",
      ignore: [
        ".git",
        "_build",
        "deps",
        "cover",
        "canary_sdk/_build",
        "canary_sdk/deps",
        "canary_sdk/cover",
        "clients/typescript/node_modules",
        "clients/typescript/dist",
        "clients/typescript/coverage",
        "dagger/node_modules",
      ],
    })
    source?: Directory,
  ): Promise<void> {
    await (await this.typescriptQualityContainer(source!)).sync()
  }
}
