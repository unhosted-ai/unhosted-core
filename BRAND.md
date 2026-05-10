# Unhosted brand guide

The brand is a tool for the project's mission: AI on hardware you own. Every visual and verbal choice should make that mission feel inevitable, technical, and trustworthy.

This document is the source of truth. If something on the website, in a tweet, or on a swag item contradicts this file, the file wins.

---

## The mark

The Unhosted mark is **three concentric circles**: a filled center, a solid ring, a dashed ring.

It maps directly to the product:

- **Filled center** — you. Devices you own. Total trust.
- **Solid ring** — your trusted circle. Friends, team, family. Verified, encrypted, no money.
- **Dashed ring** — the public swarm. Strangers, paid in stablecoin. Discontinuous on purpose.

This is not abstract. **The logo IS the architecture diagram.** When in doubt, the mark always uses the trust gradient — never replace it with something generic.

### Asset files

| File | Use |
|---|---|
| [`assets/logo.svg`](assets/logo.svg) | The mark, monochrome (uses `currentColor`) |
| [`assets/wordmark.svg`](assets/wordmark.svg) | Wordmark only |
| [`assets/lockup.svg`](assets/lockup.svg) | Mark + wordmark, horizontal |
| [`assets/banner.svg`](assets/banner.svg) | Social/repo card, 1280×640 |
| [`assets/favicon.svg`](assets/favicon.svg) | Favicon |

### Construction

- Outer ring: stroke 3, `stroke-dasharray="2 6"` (sparse, scattered — reads as "swarm")
- Middle ring: stroke 3, solid
- Inner: filled disc, no stroke
- Spacing ratios approximately 1 : 2.3 : 3.7 (inner radius to outer)

Do not invent new ratios. Do not add a fourth ring. Do not put text inside the rings.

---

## Color

| Role | Hex | Notes |
|---|---|---|
| Background | `#0A0A0A` | Near-black, never pure `#000000` |
| Foreground | `#F5F5F0` | Warm off-white, never pure `#FFFFFF` |
| Accent | `#FF3B30` | Signal red. Tagline, links on dark surfaces, "live" indicators. **Never** as a fill for the mark. |
| Mute | `#737373` | Secondary text, dividers, the dashed-ring hint |
| Trust-public | `#737373` | Outer dashed ring when explicitly differentiated |

The mark is monochrome by default. Only use the trust-tier colors when the diagram is the explicit subject — e.g. an architecture page that walks through what each ring represents.

---

## Typography

- **Headings**: [Geist](https://vercel.com/font) or Inter. Söhne Breit if a paid family is later licensed.
- **Body**: Inter or Geist.
- **Code/terminal/wordmark**: [JetBrains Mono](https://www.jetbrains.com/mono/) (free) or Berkeley Mono (paid).

The wordmark is **always lowercase**: `unhosted`. Even at the start of a sentence on the website. The word is a logotype, not a noun.

In prose:

- "Unhosted" — capital U at the start of a sentence
- "unhosted" — lowercase mid-sentence when referring to the brand-as-mark or the CLI
- Never `UnHosted`, `UNHOSTED`, `un-hosted`, or `Un-Hosted`

---

## Voice

We write the way the project's user thinks. Plain. Specific. Not selling.

### Do

- Show numbers. "47 tokens/sec on M3 Max + RTX 4090" beats "blazing fast."
- Use first person where it's true. The manifesto is "I." A docs page is "we."
- Admit limits. "Won't run a 405B model unless you have serious hardware. Here's what does work."
- Short sentences. Short paragraphs. Whitespace is content.

### Don't

- Use the word "leverage." Use the word "use."
- Use the word "platform." Unhosted is software, a network, or a cluster — never a platform.
- Use emojis in serious posts (manifesto, README, blog posts, HN comments). Discord and casual Twitter are fine.
- Use AI-generated marketing prose. If a sentence could appear on any SaaS landing page, delete it.

### Banned phrases

- "Cutting-edge"
- "State-of-the-art"
- "Empower" / "empowering"
- "Solution" (we make tools, not solutions)
- "Game-changing"
- "Democratize" (sounds insincere even when true; show, don't claim)
- "Seamless"
- "World-class"

---

## Naming rules

- Project name: **Unhosted** (capital U at sentence start), brand-as-CLI: **unhosted** (lowercase always)
- Main repo: `unhosted` under `unhosted-ai`
- Satellite repos: `unhosted-<thing>` (e.g. `unhosted-desktop`, `unhosted-sdk-python`)
- Three modes are always **local**, **trusted**, **public** — lowercase except at sentence start
- Stablecoin: **USDC**. Pick one through MVP. Adding more later is a feature post, not a brand change.
- Domain priority: `unhosted.dev` > `unhosted.ai` > `getunhosted.com` > `unhosted.org`

---

## Imagery

When you need a hero image and don't have a real screenshot:

1. A real terminal recording is **always** better than a marketing render. Use [vhs](https://github.com/charmbracelet/vhs) or asciinema.
2. A real benchmark chart is better than a fake architecture diagram.
3. A photograph of actual hardware (a MacBook + a tower PC on a desk) is on-brand.
4. Stock photos of "diverse team in office" are off-brand and forbidden.

If you must use illustration: keep to the palette, keep it geometric, keep it monochrome. The trust-gradient mark is enough decoration for most surfaces.

---

## Tone in conflict situations

**When something breaks publicly**: state what broke, why, and what we're doing. Don't make excuses, don't blame users, don't promise dates we can't hit.

**When asked about competitors** (exo, Petals, Bittensor, io.net): be direct, name them, name where they're better, name where we're different. Pretending they don't exist looks weak.

**When asked about money**: tell the truth. Funding sources are public. There is no secret VC.

**When someone is rude**: respond once, factually, then disengage. Do not feed.

---

## What we don't do

This is the inverse of a typical brand guide and the most important section.

- We don't have a Discord bot that auto-replies with marketing copy.
- We don't ask for stars in the README.
- We don't have a "Sponsors" section listing logos that paid for placement.
- We don't gate the docs behind email signup.
- We don't run a "community" that's secretly a sales funnel.
- We don't quietly start a SaaS tier and rename it to "Unhosted Cloud."
- We don't print a token. There is no `$UNHOSTED`.
- We don't post engagement-bait threads on Twitter ("a 🧵 on why X is broken…").

If you find yourself doing one of these, the brand has drifted. Stop and re-read the [manifesto](MANIFESTO.md).

---

## Updates

Brand decisions belong in this file. Changing the mark, palette, wordmark casing, or voice rules requires a PR explaining why, and a row in the changelog.

### Changelog

- **2026-05-09** — Initial brand guide. Trust-gradient mark established. Color palette set. Voice rules drafted.
