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
const GITLEAKS_IMAGE =
  "zricethezav/gitleaks:latest@sha256:c00b6bd0aeb3071cbcb79009cb16a60dd9e0a7c60e2be9ab65d25e6bc8abbb7f"
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
    .withExec(["python", "dagger/scripts/ci_contract_validation.py"])
}

function secretsContainer(source: Directory, mode: "dir" | "git"): Container {
  const args =
    mode === "git"
      ? ["gitleaks", "git", ".", "--redact"]
      : ["gitleaks", "dir", ".", "--redact"]

  return dag
    .container()
    .from(GITLEAKS_IMAGE)
    .withMountedDirectory("/work", source)
    .withWorkdir("/work")
    .withExec(args)
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
      .withExec(["bash", "test/bin/entrypoint_test.sh"])
      .withExec(["bash", "test/bin/dr_test.sh"])
  }

  private async rootDialyzerContainer(source: Directory): Promise<Container> {
    return (
      await elixirContainer(source, ".", "dev", "canary-root-dev", "mix.lock")
    )
      .withEnvVariable("ERL_FLAGS", "+S 2:2")
      .withExec(["mix", "dialyzer"])
  }

  private async rootFastContainer(source: Directory): Promise<Container> {
    return (await elixirContainer(source, ".", "test", "canary-root-test", "mix.lock"))
      .withExec(["mix", "format", "--check-formatted"])
      .withExec(["mix", "compile", "--warnings-as-errors"])
      .withExec(["bash", "test/bin/entrypoint_test.sh"])
      .withExec(["bash", "test/bin/dr_test.sh"])
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

  private async apiContractsContainer(
    source: Directory,
  ): Promise<Container> {
    return (
      await elixirContainer(source, ".", "test", "canary-root-contract-test", "mix.lock")
    ).withExec(
      ["mix", "test", "--only", "contract"],
    )
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
    const repo = source!.withoutDirectory(".git")

    await (await this.rootAdvisoryContainer(repo)).sync()
    await (await this.sdkAdvisoryContainer(repo)).sync()
    await (await this.typescriptAdvisoryContainer(repo)).sync()
  }

  @func()
  async strict(
    @argument({
      defaultPath: "/",
      ignore: [
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

    await this.codexAgentRoles(repo)
    await this.deterministic(repo)
    await this.secretsHistory(repo)
    await this.advisories(repo)
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
    const repo = source!.withoutDirectory(".git")

    await codexAgentRolesContainer(repo).sync()
  }

  @func()
  @check()
  async deterministic(
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
    const repo = source!.withoutDirectory(".git")

    await this.ciContract(repo)
    await this.openapiContract(repo)
    await this.apiContracts(repo)
    await this.rootQuality(repo)
    await this.rootDialyzer(repo)
    await this.sdkQuality(repo)
    await this.typescriptQuality(repo)
    await this.secrets(repo)
  }

  @func()
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
  async apiContracts(
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
    await (await this.apiContractsContainer(source!)).sync()
  }

  @func()
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

  @func()
  async secrets(
    @argument({
      defaultPath: "/",
      ignore: [
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
    await secretsContainer(source!, "dir").sync()
  }

  @func()
  @check()
  async secretsHistory(
    @argument({
      defaultPath: "/",
      ignore: [
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
    await secretsContainer(source!, "git").sync()
  }
}
