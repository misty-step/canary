# Canary DESIGN.md

This file is the product's public-site brand contract. Keep it short and exact:
agents and humans should be able to update `site/` from this file without
inventing a second design system.

## Operator Lock

- Lock date: operator lock-in 2026-07-07, `misty-step-936`.
- Locked tagline and homepage `h1`: `Agent-first observability.`
- Layout: Mural.
- Hero image: `site/assets/hero.jpg`, copied from the locked production asset
  `canary-hero.jpg`.
- Image provenance: `gpt-image-1`, Misty Step fresco language.
- Background opacity: `0.35`.
- Homepage structure: hero only, one viewport, no scroll.
- Header: Lucide `bird` mark in `.ae-app-mark`, uppercase wordmark, and
  `features · get started · changelog · github`.
- Footer: mode toggle on the left; on the right, `a Misty Step project` with
  `Misty Step` linked to `https://mistystep.io`, followed by the GitHub glyph
  linked to `https://github.com/misty-step/canary`.

## Brand Voice

- Plain-spoken, concrete, and operator-facing.
- Lead with the user outcome, then the proof.
- Avoid marketing fog, mascot language, and decorative claims.

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

| File                                      | Surface                     | State                                                  | Caption                                                        |
| ----------------------------------------- | --------------------------- | ------------------------------------------------------ | -------------------------------------------------------------- |
| `site/assets/screenshots/01-overview.png` | Dashboard overview          | Seeded instance, 3 monitors, 3 open incidents, 1 error/24h | The whole fleet's health at a glance — applications, uptime, incidents. |
| `site/assets/screenshots/02-incident.png` | Incident detail (work trail) | `ingest-worker` incident opened from a real ingested error | Click into one service's incident stream without leaving the dashboard. |
| `site/assets/screenshots/03-narrow.png`   | Dashboard overview, narrow  | Same seeded instance at mobile width                   | The same read, on a phone screen.                              |

## Release Notes Rule

`site/changelog.html` is user-facing. Write entries as product outcomes, not
commit logs. Each entry needs a date, a version or release label, and one or two
plain-language bullets.
