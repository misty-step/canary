# Portable Runtime Contract

This is Canary's product-owned deployment boundary. It declares a future
portable release as an immutable OCI image plus a signed
`canary.release-manifest.v1` document. The product defines runtime behavior and
verification commands. Each deployer
chooses resource sizing, placement, networking, persistence, credentials,
promotion, rollback, and recovery policy outside this repository.

## Release artifact

This section is a declarative acceptance contract. Canary's current release
workflow does not build, publish, sign, or attach this artifact. A live pull and
signature verification have not been proved. Publication must remain disabled
until the artifact, manifest, and signatures can be produced before the stable
GitHub release becomes visible.

Once an atomic publisher exists, acceptance uses the published tag to download
and verify its manifest before pulling the image:

```bash
export CANARY_RELEASE_TAG=vX.Y.Z
gh release download "$CANARY_RELEASE_TAG" \
  --repo misty-step/canary \
  --pattern 'canary-release-manifest*'
bin/release-manifest verify --file canary-release-manifest.json

identity='https://github.com/misty-step/canary/.github/workflows/release.yml@refs/heads/master'
cosign verify-blob \
  --bundle canary-release-manifest.bundle.json \
  --certificate-identity "$identity" \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com \
  canary-release-manifest.json

image="$(jq -r '.artifact.reference' canary-release-manifest.json)"
cosign verify \
  --certificate-identity "$identity" \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com \
  "$image"
docker pull "$image"
```

The digest reference in the signed manifest is the release identity. Tags are
discovery aliases only. `contracts/release-manifest.v1.schema.json` and
`bin/release-manifest` are the machine-readable schema and fail-closed verifier.

## Runtime

The OCI image runs one `canary-server` process and one SQLite writer. Runtime
inputs are classified by `contracts/runtime-config.v1.schema.json`. The image
does not select external exposure, filesystem persistence, resource limits, or
placement. A deployer supplies those values and preserves exactly one writable
SQLite database per instance.

When S3-compatible replication inputs are complete, the entrypoint restores a
missing database and runs the server under Litestream replication. When
`CANARY_REQUIRE_LITESTREAM=1`, incomplete backup inputs fail startup. When it is
not required, Canary may run without object-storage replication.

## Health

- `GET /healthz` proves the process can answer HTTP.
- `GET /readyz` proves the writable SQLite store and supervised workers are
  ready on the live request path.
- `GET /api/v1/openapi.json` exposes the running release version in
  `info.version`.

The deployer constructs the endpoint from its own network allocation. Canary
does not publish DNS, ingress, or exposure policy.

## Version

`canary-server version` emits `canary.runtime-version.v1` JSON containing the
compiled product version and expected SQLite schema version. Compare the
compiled version with the signed release manifest and the running OpenAPI
version; all three must agree.

## Migration

Migrations are forward-only and run before the HTTP server accepts traffic.
They are transactional and fail closed on a partial schema. To rehearse an
upgrade without touching live data, copy a restored database and run:

```bash
canary-server migrate --database ./disposable-copy.db
```

The command migrates only the named copy and emits the same data-verification
evidence used by the recovery drill. A manifest sets
`automatic_previous_image_rollback` to `true` only after that release has an
explicit previous-image compatibility proof. Otherwise recovery requires a
verified database restore; the deployer owns the decision and policy.

## Backup and restore

Canary supports Litestream replication to generic S3-compatible object
storage. The product inputs are the bucket, replica path, optional endpoint and
region, and S3-compatible credentials. Provider selection, bucket lifecycle,
retention, encryption, replication, credential brokering, and recovery
objectives belong to the deployer.

Run the contract wherever the image and its configuration are available:

```bash
bin/canary-recovery status --config ./litestream.yml
bin/canary-recovery restore-check \
  --config ./litestream.yml \
  --database "$CANARY_DB_PATH" \
  --server-bin ./canary-server
```

`restore-check` materializes a temporary database, copies it, migrates only the
copy, verifies it, emits a bounded JSON receipt, and deletes both temporary
files. It never replaces the live database.

## Data verification

`canary-server verify-data --database <file>` opens the named SQLite file
read-only and emits `canary.data-verification.v1` JSON with:

- full SQLite integrity result;
- foreign-key violation count;
- stored and expected schema versions; and
- deterministic row counts for every application table present.

The command exits non-zero unless integrity, foreign keys, and schema currency
all pass. HTTP health is not data-recovery evidence.
