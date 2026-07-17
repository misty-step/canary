# Upgrade and Rollback Contract

Canary declares the required shape of immutable, signed OCI releases. The
Release workflow builds and signs those artifacts, stages the manifest and
bundle in a draft GitHub release, and publishes only after both assets upload
successfully. Live pull and signature verification remain operator evidence
from a successful release run. Once a conforming artifact exists, the deployer
owns promotion, placement, cutover, rollback policy, and recovery evidence.

Before any future promotion:

1. Verify the signed manifest and image as documented in
   [`portable-runtime-contract.md`](portable-runtime-contract.md#release-artifact).
2. Confirm the manifest digest is the exact desired release.
3. Run the provider-neutral recovery check against the configured
   S3-compatible replica.
4. Rehearse migration on the restored disposable copy.
5. Record incumbent release and data-verification evidence in the deployer's
   control plane.

After activation, require live `/healthz`, `/readyz`, OpenAPI version, and
authenticated product readback. The running image digest and version must equal
the signed manifest.

Migrations are forward-only. Automatic image-only rollback is permitted only
when the signed manifest says `automatic_previous_image_rollback: true` and the
deployer's own policy allows it. Otherwise restore the verified pre-upgrade
database before activating an older image.
