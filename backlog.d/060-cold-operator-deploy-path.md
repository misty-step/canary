# Make Canary cold-operator deployable

Priority: P0 · Status: pending · Estimate: XL

## Goal
Make Canary deployable by a fresh operator, starting with Adminifi/R90, without
tribal knowledge, the `canary-obs` instance, Phaedrus-specific dogfood data, or
hidden bootstrap-key recovery steps.

## Oracle
- [ ] Given a clean checkout and a fresh Fly account, when an operator follows
      the public docs, then they can create their own Fly app, volume, Tigris
      backup bucket, required secrets, and first admin key without reading
      source code or private notes.
- [ ] Given the first boot bootstrap key was missed, then README documents the
      exact `canary-server mint-key` recovery path without data loss or secret
      leakage.
- [ ] Given repo examples, scripts, workflows, CLI defaults, SDK docs, and DR
      docs are scanned, then `canary-obs` remains only in historical evidence
      or explicitly instance-scoped examples, not product defaults.
- [ ] Given dogfood or owned-service inventory is needed, then the product can
      run with instance-local config instead of tracked Phaedrus service data.
- [ ] Given a forked repo, then deploy and witness workflows are safe to leave
      disabled or configure for the operator's own instance without accidentally
      targeting Misty Step production.
- [ ] Given Adminifi/R90 attempts the path, then a receipt shows a first
      onboarded service with health/readback evidence from their own instance.

## Verification System
- Claim: Canary is cold-self-hostable by an operator who is not the original
  maintainer.
- Falsifier: the operator must know `canary-obs`, recover a key from source
  archaeology, edit product code to remove Phaedrus data, or disable workflows
  by guessing.
- Driver: docs lint/grep for instance defaults, a local fresh-instance config
  smoke, and an Adminifi/R90 deployment transcript or equivalent clean-room
  rehearsal.
- Grader: docs contain exact commands, outputs identify blocked credentials
  without printing secrets, and live readback proves a service was onboarded.
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

## Children
1. Document first-key and lost-key recovery in README and key-rotation docs.
2. Add a fresh Fly deploy guide covering app creation, volume, Tigris, secrets,
   Litestream requirement, and smoke/readback commands.
3. Replace hardcoded `canary-obs` product defaults with `CANARY_FLY_APP`,
   `CANARY_ENDPOINT`, `FLY_APP`, or documented placeholders.
4. Add fork-defusal notes for deploy and witness workflows.
5. Move `priv/dogfood/owned_services.json` and `VERCEL_SCOPES` usage toward
   instance-local config while preserving dogfood audit behavior.
6. Produce an Adminifi/R90 or equivalent clean-room deployment receipt.
7. Decide the disposition of `020-adminifi-http-surface-verification.md` from
   the new deployment evidence.
