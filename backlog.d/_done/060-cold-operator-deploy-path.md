# Make Canary cold-operator deployable

Priority: P0 · Status: done · Estimate: XL

## Goal
Make Canary deployable by a fresh operator, starting with Adminifi/R90, without
tribal knowledge, the `canary-obs` instance, Phaedrus-specific dogfood data, or
hidden bootstrap-key recovery steps.

## Oracle
- [x] Given a clean checkout and a fresh Fly account, when an operator follows
      the public docs, then they can create their own Fly app, volume, Tigris
      backup bucket, required secrets, and first admin key without reading
      source code or private notes.
- [x] Given the first boot bootstrap key was missed, then README documents the
      exact `canary-server mint-key` recovery path without data loss or secret
      leakage.
- [x] Given repo examples, scripts, workflows, CLI defaults, SDK docs, and DR
      docs are scanned, then `canary-obs` remains only in historical evidence
      or explicitly instance-scoped examples, not product defaults.
- [x] Given dogfood or owned-service inventory is needed, then the product can
      run with instance-local config instead of tracked Phaedrus service data.
- [x] Given a forked repo, then deploy and witness workflows are safe to leave
      disabled or configure for the operator's own instance without accidentally
      targeting Misty Step production.
- [x] Given Adminifi/R90 is the concrete clean-room consumer, then the public
      docs and receipt show the exact deploy, bootstrap-key, dogfood, smoke,
      DR, and write-path procedure they can follow without Misty Step tribal
      knowledge. A live Adminifi/R90 deploy transcript remains separate
      consumer evidence, not a prerequisite for product-default cleanup.

## Verification System
- Claim: Canary is cold-self-hostable by an operator who is not the original
  maintainer.
- Falsifier: the operator must know `canary-obs`, recover a key from source
  archaeology, edit product code to remove Phaedrus data, or disable workflows
  by guessing.
- Driver: docs lint/grep for instance defaults, a local fresh-instance config
  smoke, a clean-room rehearsal, and the production-image smoke/write-path lanes
  inside `./bin/validate`.
- Grader: docs contain exact commands, outputs identify blocked credentials
  without printing secrets, the production image passes health/readiness plus
  SDK/write-path readback, and any later Adminifi/R90 live deploy can add a
  consumer-owned transcript without changing product defaults.
- Evidence packet: checked-in cold-deploy receipt under `docs/architecture/`
  plus the normal `./bin/validate` transcript.

## Notes
This is the top Factory decision for Canary on 2026-07-01. It folds in the
groom report's cold-operator self-host epic and the operator overlay:
unhardcode the app name, document bootstrap-key handling, and move dogfood
instance data out of product code.

Do not silently delete existing tickets while doing this. `020` can be closed
only when the Adminifi/R90 deployment proof supersedes it with concrete
evidence. Historical `canary-obs` evidence files can remain historical; product
defaults and current docs should not require that instance.

## Progress
- 2026-07-01: Added the first cold-operator docs/script slice: public Fly
  self-host guide, README bootstrap-key recovery path, fork-safe deploy and
  witness workflow guards, and explicit endpoint/app requirements for witness,
  DR, and write-path rehearsal scripts. The epic remains open; Rust CLI
  compiled defaults, `fly.toml`, dogfood instance data, and clean-room
  deployment evidence still need follow-up.
- 2026-07-01: Finished the epic with `fly.toml` dehardcoded to a placeholder,
  live CLI endpoint resolution made explicit, SDK docs moved to operator-owned
  endpoint placeholders, dogfood defaults moved to instance-local
  `.canary/dogfood/owned_services.json`, checked-in dogfood data replaced with
  `priv/dogfood/owned_services.example.json`, and clean-room proof recorded in
  `docs/architecture/cold-operator-clean-room-receipt-2026-07-01.md`.
  `020-adminifi-http-surface-verification.md` remains blocked because it is
  about legacy Adminifi URL stability, not the clean-room Canary deploy path.

## Children
1. [x] Document first-key and lost-key recovery in README and key-rotation docs.
2. [x] Add a fresh Fly deploy guide covering app creation, volume, Tigris, secrets,
   Litestream requirement, and smoke/readback commands.
3. [x] Replace hardcoded `canary-obs` product defaults with `CANARY_FLY_APP`,
   `CANARY_ENDPOINT`, `FLY_APP`, or documented placeholders.
4. [x] Add fork-defusal notes for deploy and witness workflows.
5. [x] Move `priv/dogfood/owned_services.json` and `VERCEL_SCOPES` usage toward
   instance-local config while preserving dogfood audit behavior.
6. [x] Produce an Adminifi/R90-oriented clean-room deploy rehearsal receipt.
7. [x] Decide the disposition of `020-adminifi-http-surface-verification.md` from
   the new deployment evidence.
