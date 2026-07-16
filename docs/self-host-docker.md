# OCI Runtime Contract

The Release workflow builds and keylessly signs the multi-platform image and
portable release manifest, stages both signed files in a draft GitHub release,
and publishes only after the uploads succeed. A successful release run is
required before treating the artifact as pullable; this document defines how
that published image must behave. The declarative artifact contract is in
[`portable-runtime-contract.md`](portable-runtime-contract.md#release-artifact).

After a conforming artifact exists, the deployer runs its digest with values satisfying
`contracts/runtime-config.v1.schema.json`. Canary intentionally does not ship a
orchestration file or prescribe external networking, persistent-storage placement,
resource sizing, service supervision, or secret delivery. Those are properties
of a deployment instance, not of the Canary product.

The image starts one server and uses the configured SQLite file. On first boot
it prints a one-time bootstrap admin key unless
`CANARY_DISCLOSE_BOOTSTRAP_KEY=false`; capture that value into the deployer's
secret manager. If it is lost, run this image-owned recovery command against
the configured database:

```bash
canary-server mint-key --scope admin --name operator-recovery
```

After startup, prove `/healthz`, `/readyz`, the OpenAPI version, an ingest/query
roundtrip, and the external witness appropriate to the deployment. Backup and
restore behavior is documented in
[`portable-runtime-contract.md`](portable-runtime-contract.md#backup-and-restore).
