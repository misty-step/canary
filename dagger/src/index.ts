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

const NODE_IMAGE =
  "node:22.22.0-bookworm-slim@sha256:dd9d21971ec4395903fa6143c2b9267d048ae01ca6d3ea96f16cb30df6187d94"
const PYTHON_IMAGE =
  "python:3.13-slim-bookworm@sha256:f13a6b7565175da40695e8109f64cbc4d2e65f4c9ef2e3b321c3a44fa3c06fe7"
const RUST_IMAGE =
  "rust:1.94.0-bookworm@sha256:365468470075493dc4583f47387001854321c5a8583ea9604b297e67f01c5a4f"
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

function cacheIdentityPart(part: string): string {
  if (part.startsWith(DIGEST_PREFIX)) {
    return digestSuffix(part)
  }

  return part
    .replace(/[^a-z0-9]+/gi, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 32)
    .toLowerCase()
}

function cacheVolumeName(namespace: string, ...parts: string[]): string {
  return [cacheIdentityPart(namespace), ...parts.map(cacheIdentityPart)].join("-")
}

// Dagger's TypeScript introspector evaluates @argument metadata in isolation.
// Keep the ignore lists literal in this file and sync them from
// dagger/scripts/sync_source_arguments.py.
async function cachePlatformKey(): Promise<string> {
  return cacheIdentityPart(await dag.defaultPlatform())
}

async function lockfileDigest(
  source: Directory,
  path: string,
): Promise<string> {
  return source.file(path).digest({ excludeMetadata: true })
}

async function nodeContainer(
  source: Directory,
  workdir: string,
  lockfilePath: string,
  cacheNamespace: string,
): Promise<Container> {
  const digest = await lockfileDigest(source, lockfilePath)
  const platformKey = await cachePlatformKey()
  const imageKey = imageIdentity(NODE_IMAGE)
  const npmCache = dag.cacheVolume(
    cacheVolumeName(`${cacheNamespace}-npm`, platformKey, imageKey, digest),
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

async function rustContainer(source: Directory): Promise<Container> {
  const digest = await lockfileDigest(source, "Cargo.lock")
  const platformKey = await cachePlatformKey()
  const imageKey = imageIdentity(RUST_IMAGE)
  const registryCache = dag.cacheVolume(
    cacheVolumeName("canary-rust-registry", platformKey, imageKey, digest),
  )
  const gitCache = dag.cacheVolume(
    cacheVolumeName("canary-rust-git", platformKey, imageKey, digest),
  )
  const targetCache = dag.cacheVolume(
    cacheVolumeName("canary-rust-target", platformKey, imageKey, digest),
  )

  return dag
    .container()
    .from(RUST_IMAGE)
    .withExec(["rustup", "component", "add", "rustfmt", "clippy"])
    .withMountedDirectory("/work", source)
    .withMountedCache("/usr/local/cargo/registry", registryCache)
    .withMountedCache("/usr/local/cargo/git", gitCache)
    .withMountedCache("/work/target", targetCache)
    .withWorkdir("/work")
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
  private scriptsQualityContainer(source: Directory): Container {
    return dag
      .container()
      .from(PYTHON_IMAGE)
      .withExec(["apt-get", "update"])
      .withExec(["apt-get", "install", "-y", "--no-install-recommends", "jq"])
      .withMountedDirectory("/work", source)
      .withWorkdir("/work")
      .withExec(["bash", "test/bin/entrypoint_test.sh"])
      .withExec(["bash", "test/bin/dr_test.sh"])
      .withExec(["bash", "test/bin/dogfood_audit_test.sh"])
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

  private async rustFastContainer(source: Directory): Promise<Container> {
    return (await rustContainer(source))
      .withExec(["cargo", "fmt", "--all", "--check"])
      .withExec(["cargo", "check", "--workspace", "--all-targets", "--locked"])
  }

  private async rustQualityContainer(source: Directory): Promise<Container> {
    return (await this.rustFastContainer(source))
      .withExec([
        "cargo",
        "clippy",
        "--workspace",
        "--all-targets",
        "--locked",
        "--",
        "-D",
        "warnings",
      ])
      .withExec(["cargo", "test", "--workspace", "--locked"])
  }

  private async rustAdvisoryContainer(source: Directory): Promise<Container> {
    return (await rustContainer(source))
      .withExec(["cargo", "install", "cargo-audit", "--version", "0.22.1", "--locked"])
      .withExec(["cargo", "audit"])
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

  private productionImageContainer(source: Directory): Container {
    return source.dockerBuild()
  }

  private productionImageSmokeContainer(source: Directory): Container {
    const service = this.productionImageContainer(source)
      .withEnvVariable("CANARY_DB_PATH", "/tmp/canary-smoke.db")
      .withEnvVariable("PORT", "4000")
      .withEnvVariable("CANARY_DISCLOSE_BOOTSTRAP_KEY", "false")
      .withExposedPort(4000)
      .asService()

    return dag
      .container()
      .from(RUST_IMAGE)
      .withExec(["apt-get", "update", "-q"])
      .withExec(["apt-get", "install", "-yq", "--no-install-recommends", "curl"])
      .withServiceBinding("canary", service)
      .withExec([
        "bash",
        "-ceu",
        [
          "for _ in {1..60}; do",
          "  if curl --fail --silent --show-error http://canary:4000/healthz >/tmp/healthz.json &&",
          "     curl --fail --silent --show-error http://canary:4000/readyz >/tmp/readyz.json; then",
          "    grep -F '\"status\":\"ok\"' /tmp/healthz.json",
          "    grep -F '\"status\":\"ready\"' /tmp/readyz.json",
          "    exit 0",
          "  fi",
          "  sleep 1",
          "done",
          "cat /tmp/healthz.json /tmp/readyz.json 2>/dev/null || true",
          "exit 1",
        ].join("\n"),
      ])
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
        "clients/typescript/node_modules",
        "clients/typescript/dist",
        "clients/typescript/coverage",
        "dagger/node_modules",
        "target",
      ],
    })
    source?: Directory,
  ): Promise<void> {
    const repo = source!

    await codexAgentRolesContainer(repo).sync()
    await this.scriptsQualityContainer(repo).sync()
    await (await this.typescriptFastContainer(repo)).sync()
    await (await this.rustFastContainer(repo)).sync()
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
        "clients/typescript/node_modules",
        "clients/typescript/dist",
        "clients/typescript/coverage",
        "dagger/node_modules",
        "target",
      ],
    })
    source?: Directory,
  ): Promise<void> {
    const repo = source!.withoutDirectory(".git")

    await (await this.typescriptAdvisoryContainer(repo)).sync()
    await (await this.rustAdvisoryContainer(repo)).sync()
  }

  @func()
  async strict(
    @argument({
      defaultPath: "/",
      ignore: [
        "_build",
        "deps",
        "cover",
        "clients/typescript/node_modules",
        "clients/typescript/dist",
        "clients/typescript/coverage",
        "dagger/node_modules",
        "target",
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
        "clients/typescript/node_modules",
        "clients/typescript/dist",
        "clients/typescript/coverage",
        "dagger/node_modules",
        "target",
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
        "clients/typescript/node_modules",
        "clients/typescript/dist",
        "clients/typescript/coverage",
        "dagger/node_modules",
        "target",
      ],
    })
    source?: Directory,
  ): Promise<void> {
    const repo = source!.withoutDirectory(".git")

    await this.ciContract(repo)
    await this.openapiContract(repo)
    await this.scriptsQualityContainer(repo).sync()
    await this.typescriptQuality(repo)
    await this.rustQuality(repo)
    await this.productionImageSmoke(repo)
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
        "clients/typescript/node_modules",
        "clients/typescript/dist",
        "clients/typescript/coverage",
        "dagger/node_modules",
        "target",
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
        "clients/typescript/node_modules",
        "clients/typescript/dist",
        "clients/typescript/coverage",
        "dagger/node_modules",
        "target",
      ],
    })
    source?: Directory,
  ): Promise<void> {
    await (await this.openapiContractContainer(source!)).sync()
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
        "clients/typescript/node_modules",
        "clients/typescript/dist",
        "clients/typescript/coverage",
        "dagger/node_modules",
        "target",
      ],
    })
    source?: Directory,
  ): Promise<void> {
    await (await this.typescriptQualityContainer(source!)).sync()
  }

  @func()
  async rustQuality(
    @argument({
      defaultPath: "/",
      ignore: [
        ".git",
        "_build",
        "deps",
        "cover",
        "clients/typescript/node_modules",
        "clients/typescript/dist",
        "clients/typescript/coverage",
        "dagger/node_modules",
        "target",
      ],
    })
    source?: Directory,
  ): Promise<void> {
    await (await this.rustQualityContainer(source!)).sync()
  }

  @func()
  async productionImageSmoke(
    @argument({
      defaultPath: "/",
      ignore: [
        ".git",
        "_build",
        "deps",
        "cover",
        "clients/typescript/node_modules",
        "clients/typescript/dist",
        "clients/typescript/coverage",
        "dagger/node_modules",
        "target",
      ],
    })
    source?: Directory,
  ): Promise<void> {
    await this.productionImageSmokeContainer(source!).sync()
  }

  @func()
  async rustAdvisories(
    @argument({
      defaultPath: "/",
      ignore: [
        ".git",
        "_build",
        "deps",
        "cover",
        "clients/typescript/node_modules",
        "clients/typescript/dist",
        "clients/typescript/coverage",
        "dagger/node_modules",
        "target",
      ],
    })
    source?: Directory,
  ): Promise<void> {
    await (await this.rustAdvisoryContainer(source!)).sync()
  }

  @func()
  async secrets(
    @argument({
      defaultPath: "/",
      ignore: [
        "_build",
        "deps",
        "cover",
        "clients/typescript/node_modules",
        "clients/typescript/dist",
        "clients/typescript/coverage",
        "dagger/node_modules",
        "target",
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
        "clients/typescript/node_modules",
        "clients/typescript/dist",
        "clients/typescript/coverage",
        "dagger/node_modules",
        "target",
      ],
    })
    source?: Directory,
  ): Promise<void> {
    await secretsContainer(source!, "git").sync()
  }
}
