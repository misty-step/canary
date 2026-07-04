# Canary DESIGN.md

This file is the product's public-site brand contract. Keep it short and exact:
agents and humans should be able to update `site/` from this file without
inventing a second design system.

## Brand Voice

- Plain-spoken, concrete, and operator-facing.
- Lead with the user outcome, then the proof.
- Avoid marketing fog, mascot language, and decorative claims.

## Pitch One-Liner

`Canary helps operators of agent infrastructure catch outages and errors before an agent does, without running a hosted observability vendor.`

## Lucide Mark

- Icon: `bird`
- Reason: reused from the live dashboard itself (`.ae-app-mark` in
  `crates/canary-server/static/dashboard/index.html`) — it is already the
  product mark operators see in the running Canary dashboard.
- Rule: the mark is an inline Lucide SVG inside `.ae-app-mark`. No bespoke
  marks, logo images, emoji marks, or colored wordmarks.

## Palette Hooks

Canary's own dashboard runs the Aesthetic kit's base tokens with no per-product
override, so the marketing site reuses the exact same values rather than
inventing a distinct brand color:

```css
:root {
  --ae-accent: #2643d0;
  --ae-accent-dark: #8c9eff;
}
```

## Screenshot Inventory

| File                                        | Surface                       | State                                                    | Caption                                                          |
| -------------------------------------------- | ------------------------------ | ----------------------------------------------------------- | -------------------------------------------------------------------- |
| `site/assets/screenshots/01-overview.png`   | Dashboard overview             | Seeded instance, 3 monitors, 3 open incidents, 1 error/24h | The whole fleet's health at a glance — applications, uptime, incidents. |
| `site/assets/screenshots/02-incident.png`   | Incident detail (work trail)   | `ingest-worker` incident opened from a real ingested error | Click into one service's incident stream without leaving the dashboard. |
| `site/assets/screenshots/03-narrow.png`     | Dashboard overview, narrow     | Same seeded instance at mobile width                      | The same read, on a phone screen.                                    |

## Footer Links

- Misty Step: `https://mistystep.io`
- GitHub: `https://github.com/misty-step/canary` — present; the repo is public.
- Weave: omitted; Canary is a Misty Step fleet product, not a Weave-family
  product surface.

## Release Notes Rule

`site/changelog.html` is user-facing. Write entries as product outcomes, not
commit logs. Each entry needs a date, a version or release label, and one or two
plain-language bullets.

Canary has real tagged releases (`v1.14.0` latest), so the landmark-902 export
path was attempted: `landmark extract-prs` + `landmark synthesize` (audience
`end-user`, `gpt-4o-mini`) against the real v1.14.0 PR history. The synthesis
came back marked `valid` by Landmark's own quality gate but **fabricated**
content not present in the source — invented "Breaking Changes" and "Bug
Fixes" sections for a release that shipped exactly one feature PR and nothing
else. That output was discarded rather than published; publishing it would
have violated the showcase contract's "no fabricated claims" rule. `changelog.html`
instead carries a hand-written, fact-checked stub built directly from the real
`CHANGELOG.md`/PR history. See the canary-912 card comment for the full repro
(exact commands, exact fabricated output) filed as kit/pipeline friction.
