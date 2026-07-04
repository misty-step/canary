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
  Secret,
  Service,
  object,
  func,
  check,
  argument,
} from "@dagger.io/dagger"
import { createHash } from "node:crypto"

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

async function sourceTreeDigest(source: Directory): Promise<string> {
  return source.digest()
}

async function rustTargetCacheDigest(source: Directory): Promise<string> {
  const scope = process.env.CANARY_DAGGER_CACHE_SCOPE?.trim()

  if (scope) {
    return `${DIGEST_PREFIX}${createHash("sha256").update(scope).digest("hex")}`
  }

  return sourceTreeDigest(source)
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
  const targetDigest = await rustTargetCacheDigest(source)
  const platformKey = await cachePlatformKey()
  const imageKey = imageIdentity(RUST_IMAGE)
  const registryCache = dag.cacheVolume(
    cacheVolumeName("canary-rust-registry", platformKey, imageKey, digest),
  )
  const gitCache = dag.cacheVolume(
    cacheVolumeName("canary-rust-git", platformKey, imageKey, digest),
  )
  const targetCache = dag.cacheVolume(
    cacheVolumeName("canary-rust-target", platformKey, imageKey, targetDigest),
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
      .withExec(["apt-get", "install", "-y", "--no-install-recommends", "git", "jq"])
      .withMountedDirectory("/work", source)
      .withWorkdir("/work")
      .withExec(["bash", "test/bin/entrypoint_test.sh"])
      .withExec(["bash", "test/bin/dr_test.sh"])
      .withExec(["bash", "test/bin/dogfood_audit_test.sh"])
      .withExec(["bash", "test/bin/dogfood_inventory_test.sh"])
      .withExec(["bash", "test/bin/canary_witness_test.sh"])
      .withExec(["bash", "test/bin/canary_write_path_rehearsal_test.sh"])
      .withExec(["bash", "-n", "bin/canary"])
      .withExec(["bash", "-n", "bin/canary-witness"])
      .withExec(["bash", "-n", "bin/canary-write-path-rehearsal"])
      .withExec(["bash", "bin/check-aesthetic-currency"])
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

  private async productionImageService(
    source: Directory,
  ): Promise<{ service: Service; adminKey: Secret }> {
    const prepared = this.productionImageContainer(source)
      .withEnvVariable("CANARY_DB_PATH", "/tmp/canary-smoke.db")
      .withEnvVariable("PORT", "4000")
      .withEnvVariable("CANARY_DISCLOSE_BOOTSTRAP_KEY", "false")
      .withEnvVariable("ALLOW_PRIVATE_TARGETS", "true")
      .withExec([
        "bash",
        "-ceu",
        "/app/bin/canary-server mint-key --scope admin --name production-smoke-admin >/tmp/canary-admin-key 2>/tmp/canary-mint-key.log",
      ])

    const rawKey = (await prepared.file("/tmp/canary-admin-key").contents()).trim()
    const adminKey = dag.setSecret("canary-smoke-admin-key", rawKey)
    const service = prepared
      .withExposedPort(4000)
      .asService()

    return { service, adminKey }
  }

  private readinessProbeScript(): string {
    return [
      "for _ in {1..60}; do",
      "  if curl --fail --silent --show-error http://canary:4000/healthz >/tmp/healthz.json &&",
      "     curl --fail --silent --show-error http://canary:4000/readyz >/tmp/readyz.json &&",
      "     grep -F '\"status\":\"ok\"' /tmp/healthz.json &&",
      "     grep -F '\"status\":\"ready\"' /tmp/readyz.json &&",
      "     jq -e '",
      "      .checks.database == \"ok\" and",
      "      .checks.supervisor == \"ok\" and",
      "      (.checks.workers | length) == 5 and",
      "      ([.checks.workers[].name] | sort) == [\"monitor_overdue\", \"retention_prune\", \"target_probe\", \"tls_scan\", \"webhook_delivery\"] and",
      "      all(.checks.workers[];",
      "        .state == \"started\" and",
      "        .health == \"ok\" and",
      "        .failure_count == 0 and",
      "        .consecutive_failures == 0 and",
      "        (.last_success_at | type) == \"string\" and",
      "        ((.last_success_age_ms | type) == \"number\") and",
      "        ((.due_count | type) == \"number\") and",
      "        ((.in_flight_count | type) == \"number\") and",
      "        ((.oldest_due_age_ms == null) or ((.oldest_due_age_ms | type) == \"number\")) and",
      "        ((.backoff_or_circuit_open | type) == \"boolean\"))",
      "    ' /tmp/readyz.json; then",
      "    exit 0",
      "  fi",
      "  sleep 1",
      "done",
      "cat /tmp/healthz.json /tmp/readyz.json 2>/dev/null || true",
      "exit 1",
    ].join("\n")
  }

  private sdkSmokeScript(): string {
    return `
import { initCanary, captureException } from "./clients/typescript/dist/index.js";

const endpoint = process.env.CANARY_ENDPOINT;
const apiKey = process.env.CANARY_API_KEY;
const service = \`canary-sdk-smoke-\${Date.now()}-\${process.pid}\`;

if (!endpoint || !apiKey) {
  throw new Error("missing Canary smoke endpoint or API key");
}

initCanary({
  endpoint,
  apiKey,
  service,
  environment: "ci",
});

const result = await captureException(new Error(\`Canary SDK production smoke \${service}\`), {
  fingerprint: ["canary-sdk-production-smoke", service],
  context: { source: "dagger-production-image-smoke" },
});

if (!result?.id || !result.group_hash) {
  throw new Error(\`SDK ingest did not return an error id and group hash: \${JSON.stringify(result)}\`);
}

const query = await fetch(
  \`\${endpoint}/api/v1/query?service=\${encodeURIComponent(service)}&window=1h\`,
  { headers: { Authorization: \`Bearer \${apiKey}\` } },
);
if (!query.ok) {
  throw new Error(\`SDK readback query failed with HTTP \${query.status}\`);
}
const body = await query.json();
if (
  body.service !== service ||
  body.total_errors < 1 ||
  !body.groups?.some((group) => group.group_hash === result.group_hash && group.service === service)
) {
  throw new Error(\`SDK readback did not include group \${result.group_hash}: \${JSON.stringify(body)}\`);
}
`
  }

  private writePathSmokeScript(): string {
    return `
prefix="dagger-prod-$(date -u +%Y%m%d%H%M%S)-$$"
receipt="$(
  CANARY_REHEARSAL_POLL_ATTEMPTS=20 \
  CANARY_REHEARSAL_POLL_SLEEP=1 \
  bin/canary-write-path-rehearsal \
    --endpoint "$CANARY_ENDPOINT" \
    --webhook-url https://httpbingo.org/status/204 \
    --target-url http://127.0.0.1:4000/healthz \
    --prefix "$prefix" \
    --no-dr-status \
    --json
)"
printf '%s' "$receipt" >/tmp/canary-write-path-rehearsal.json
jq -e '
  .status == "ok"
  and (.resources.immutable_error_id | startswith("ERR-"))
  and (.resources.immutable_webhook_delivery_id | startswith("DLV-"))
  and ([.steps[].name] | index("api_key_create_ingest"))
  and ([.steps[].name] | index("webhook_test"))
  and ([.steps[].name] | index("error_ingest"))
  and ([.steps[].name] | index("error_query_readback"))
  and ([.steps[].name] | index("report_readback"))
  and ([.steps[].name] | index("timeline_readback"))
  and ([.steps[].name] | index("error_detail_readback"))
  and ([.steps[].name] | index("webhook_delivery_lookup"))
  and ([.steps[].name] | index("post_cleanup_targets"))
  and ([.steps[].name] | index("post_cleanup_monitors"))
  and ([.steps[].name] | index("post_cleanup_webhooks"))
' /tmp/canary-write-path-rehearsal.json
`
  }

  private doctorAndMcpSmokeScript(): string {
    return `
bin/canary doctor --json >/tmp/canary-doctor.json
jq -e '
  .response.worker_readiness.available == true
  and .response.worker_readiness.status == "ready"
  and .response.worker_readiness.worker_count == 5
  and .response.worker_readiness.failing_workers == 0
  and (.response.reachability.readyz.response.checks.workers | length) == 5
' /tmp/canary-doctor.json

bin/canary mcp-manifest >/tmp/canary-mcp-manifest.json
jq -e '
  .schema_version == 1
  and (.tools | type == "array")
  and (.tools | length) >= 4
  and all(.tools[]; (.name | type == "string") and (.description | type == "string") and .input_schema.type == "object" and (.input_schema.properties | type == "object"))
' /tmp/canary-mcp-manifest.json

printf '%s\n' \
  '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"dagger-smoke","version":"0"}}}' \
  '{"jsonrpc":"2.0","method":"notifications/initialized"}' \
  '{"jsonrpc":"2.0","id":2,"method":"tools/list"}' \
  | bin/canary mcp-server >/tmp/canary-mcp-server.jsonl
jq -s -e '
  length == 2
  and .[0].result.protocolVersion == "2025-11-25"
  and .[0].result.capabilities.tools.listChanged == false
  and (.[1].result.tools | type == "array")
  and any(.[1].result.tools[]; .name == "canary_summary" and .inputSchema.type == "object")
' /tmp/canary-mcp-server.jsonl
`
  }

  private alertPlaneImpairmentRehearsalScript(): string {
    return `
prefix="dagger-alert-plane-$(date -u +%Y%m%d%H%M%S)-$$"
monitor="canary-alert-plane-$prefix"
observed_at="$(date -u -d '10 minutes ago' +%Y-%m-%dT%H:%M:%SZ)"

monitor_payload="$(
  jq -cn \
    --arg name "$monitor" \
    '{
      name: $name,
      service: "canary",
      mode: "ttl",
      expected_every_ms: 1000,
      grace_ms: 0
    }'
)"
curl --fail --silent --show-error \
  -X POST "$CANARY_ENDPOINT/api/v1/monitors" \
  -H "Authorization: Bearer $CANARY_API_KEY" \
  -H "Content-Type: application/json" \
  --data "$monitor_payload" >/tmp/canary-alert-plane-monitor.json

check_in_payload="$(
  jq -cn \
    --arg monitor "$monitor" \
    --arg observed_at "$observed_at" \
    '{
      monitor: $monitor,
      status: "alive",
      observed_at: $observed_at,
      ttl_ms: 1000,
      summary: "Dagger production-image alert-plane impairment rehearsal"
    }'
)"
curl --fail --silent --show-error \
  -X POST "$CANARY_ENDPOINT/api/v1/check-ins" \
  -H "Authorization: Bearer $CANARY_API_KEY" \
  -H "Content-Type: application/json" \
  --data "$check_in_payload" >/tmp/canary-alert-plane-check-in.json

for attempt in $(seq 1 30); do
  bin/canary doctor --json >/tmp/canary-alert-plane-doctor.json
  if jq -e '
    .response.reachability.readyz.ok == true
    and .response.reachability.readyz.http_status == 200
    and .response.reachability.readyz.response.status == "ready"
    and .response.worker_readiness.status == "ready"
    and .response.worker_readiness.pressured_workers >= 1
    and .response.alert_plane.status == "impaired"
    and (.response.alert_plane.reasons | index("monitor_overdue pressured") != null)
    and any(.response.alert_plane.workers[]?;
      .name == "monitor_overdue"
      and .health == "pressured"
      and ((.oldest_due_age_ms // 0) > 120000))
    and any(.response.verdict.blocking_signals[]?;
      startswith("alert-plane impaired:"))
  ' /tmp/canary-alert-plane-doctor.json >/dev/null; then
    set +e
    bin/canary-witness \
      --endpoint "$CANARY_ENDPOINT" \
      --read-api-key "$CANARY_API_KEY" \
      --ingest-api-key "$CANARY_API_KEY" \
      --receipt /tmp/canary-alert-plane-witness.json \
      --require-check-in \
      --json >/tmp/canary-alert-plane-witness.out
    witness_status=$?
    set -e
    test "$witness_status" != "0"
    jq -e '
      .status == "degraded"
      and .alert_plane.status == "impaired"
      and (.alert_plane.reasons | index("monitor_overdue pressured") != null)
      and .check_in.skipped == true
    ' /tmp/canary-alert-plane-witness.json >/dev/null
    jq '{
      route_ready: .response.reachability.readyz.response.status,
      alert_plane: .response.alert_plane,
      verdict: .response.verdict.overall
    }' /tmp/canary-alert-plane-doctor.json
    jq '{
      status: .status,
      alert_plane: .alert_plane,
      check_in: .check_in
    }' /tmp/canary-alert-plane-witness.json
    exit 0
  fi
  sleep 1
done

cat /tmp/canary-alert-plane-doctor.json 2>/dev/null || true
cat /tmp/canary-alert-plane-witness.out 2>/dev/null || true
curl --silent --show-error "$CANARY_ENDPOINT/readyz" || true
exit 1
`
  }

  private async productionImageNodeSmokeContainer(
    source: Directory,
    service: Service,
    adminKey: Secret,
  ): Promise<Container> {
    return (
      await nodeContainer(
        source,
        "clients/typescript",
        "clients/typescript/package-lock.json",
        "canary-typescript",
      )
    )
      .withWorkdir("/work")
      .withExec(["apt-get", "update", "-q"])
      .withExec(["apt-get", "install", "-yq", "--no-install-recommends", "curl", "jq"])
      .withServiceBinding("canary", service)
      .withSecretVariable("CANARY_API_KEY", adminKey)
      .withEnvVariable("CANARY_ENDPOINT", "http://canary:4000")
      .withExec(["bash", "-ceu", this.readinessProbeScript()])
      .withExec(["npm", "--prefix", "clients/typescript", "run", "build"])
      .withExec(["node", "--input-type=module", "-e", this.sdkSmokeScript()])
      .withExec(["bash", "-ceu", this.writePathSmokeScript()])
  }

  private async productionImageDoctorSmokeContainer(
    source: Directory,
    service: Service,
    adminKey: Secret,
  ): Promise<Container> {
    return (await rustContainer(source))
      .withExec(["apt-get", "update", "-q"])
      .withExec(["apt-get", "install", "-yq", "--no-install-recommends", "curl", "jq"])
      .withServiceBinding("canary", service)
      .withSecretVariable("CANARY_API_KEY", adminKey)
      .withEnvVariable("CANARY_ENDPOINT", "http://canary:4000")
      .withExec(["bash", "-ceu", this.readinessProbeScript()])
      .withExec(["bash", "-ceu", this.doctorAndMcpSmokeScript()])
  }

  private async productionImageAlertPlaneRehearsalContainer(
    source: Directory,
    service: Service,
    adminKey: Secret,
  ): Promise<Container> {
    return (await rustContainer(source))
      .withExec(["apt-get", "update", "-q"])
      .withExec(["apt-get", "install", "-yq", "--no-install-recommends", "curl", "jq"])
      .withServiceBinding("canary", service)
      .withSecretVariable("CANARY_API_KEY", adminKey)
      .withEnvVariable("CANARY_ENDPOINT", "http://canary:4000")
      .withExec(["bash", "-ceu", this.readinessProbeScript()])
      .withExec(["bash", "-ceu", this.alertPlaneImpairmentRehearsalScript()])
  }

  private async productionImageIntegration(source: Directory): Promise<void> {
    const { service, adminKey } = await this.productionImageService(source)

    await (await this.productionImageNodeSmokeContainer(source, service, adminKey)).sync()
    await (await this.productionImageDoctorSmokeContainer(source, service, adminKey)).sync()
    await (await this.productionImageAlertPlaneRehearsalContainer(source, service, adminKey)).sync()
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
    await this.productionImageIntegration(source!)
  }

  @func()
  async productionImageAlertPlaneRehearsal(
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
    const { service, adminKey } = await this.productionImageService(source!)

    await (await this.productionImageAlertPlaneRehearsalContainer(source!, service, adminKey)).sync()
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
