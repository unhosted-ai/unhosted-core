# unhosted brand + launch plan

## org identity

- **GitHub org**: `unhosted-ai`
- **fallback org names** (in order): `unhosted-labs`, `getunhosted`, `unhosted-co`
- **primary tagline**: **AI that lives where you do.**
- **technical tagline**: **Frontier-class inference on hardware you own.**
- **HN/Reddit one-liner**:  
  **Unhosted pools the computers you already own into one inference cluster. No datacenter, no API key, no rate limit.**

### elevator paragraph

Unhosted is open-source software that turns your laptop, gaming PC, and home server into a single AI inference cluster. Run Llama 70B across a MacBook and a 4090. Run smaller models on a Raspberry Pi mesh. Your hardware, your model, your data — no hosted service in the loop, ever.

---

## manifesto draft (`unhosted-ai/manifesto`)

```markdown
# The Unhosted Manifesto

Inference shouldn't require a credit card.

For two years we've watched a handful of companies decide
who can use AI, what they can ask it, and what it costs.
We've watched prices climb. We've watched usage policies tighten.
We've watched our prompts become training data.

We've also watched the hardware in our homes get faster.
A modern MacBook runs models that needed a datacenter in 2023.
A gaming PC with a 4090 runs models that cost $20/hour to rent.
A Raspberry Pi cluster runs models good enough for most things.

The compute is here. It's already paid for. It's already in our homes.

What's missing is the software to make it work together.

Unhosted is that software.

## What we believe

**Sovereignty.** The most important AI in your life should run on
hardware you own, on a network you control, on data that never
leaves your house.

**Composition.** A MacBook, a gaming PC, and an old laptop are
more powerful than any one of them alone. The orchestration layer
should be free.

**Honesty.** We will tell you what runs well and what doesn't.
Some models won't fit. Some setups will be slow. We won't pretend
otherwise to make the marketing easier.

**Open by default.** Code is open source. Models are documented.
Benchmarks are reproducible. Choices are yours.

## What we're not

We're not a crypto project. There is no token.
We're not a cloud company in disguise. There is no managed tier
that we secretly want you to graduate to.
We're not anti-AI. We're anti-monopoly.

## Join us

Star the repo. Run it on your hardware. Tell us what broke.
That's the whole ask.
```

---

## repository plan

### launch day (3 repos only)

```text
unhosted-ai/
├── unhosted    # main product: runtime + CLI
├── manifesto   # brand + recruiting doc
└── .github     # org profile README
```

### add in month 1 (when content exists)

```text
├── benchmarks  # reproducible perf numbers
├── recipes     # community cluster configs
└── docs        # docs site source
```

### add later (when real artifacts exist)

```text
├── desktop
├── migrate
└── models
```

---

## naming

- main repo name: **`unhosted`**  
  URL target: `github.com/unhosted-ai/unhosted`
- avoid early suffixes like `-core`/`-engine`

---

## visual direction

- **background**: `#0A0A0A`
- **foreground**: `#F5F5F0`
- **accent options**:
  1. `#FF3B30` (signal red, recommended)
  2. `#00FF88` (electric green)
  3. `#FF6B1A` (burnt amber)

Typography:
- headings: Söhne / Inter Display / Geist
- body: Inter / Geist
- code: Berkeley Mono / JetBrains Mono

Logo concept:
- distributed tower mark (multiple towers, connected line), or
- heavy lowercase mono wordmark: `unhosted`

---

## org profile README draft (`unhosted-ai/.github/profile/README.md`)

```markdown
# unhosted

**AI that lives where you do.**

We build open-source software for running AI on hardware you own.

→ [Read the manifesto](https://github.com/unhosted-ai/manifesto)
→ [Try Unhosted](https://github.com/unhosted-ai/unhosted)
→ [See benchmarks](https://github.com/unhosted-ai/benchmarks)

---

## Projects

**unhosted** — Pool your devices into one inference cluster.
Mac, Linux, Windows. CUDA, Metal, ROCm. One endpoint.

**recipes** — Real cluster configurations from real users.
M-series Mac + gaming PC. Pi cluster. Mixed homelab.

**migrate** — Leave hosted AI services. Bring your history with you.

**benchmarks** — Honest numbers. Reproducible. Updated.

---

## Why we exist

[Manifesto.](https://github.com/unhosted-ai/manifesto)
```

---

## voice and tone

- short, direct sentences
- no fluff
- show numbers whenever possible
- admit limits plainly
- no emojis in serious docs/posts
- casual lowercase brand usage (`unhosted`), formal sentence case (`Unhosted`)

---

## domain + handles to lock now

Domains to check (priority):
1. `unhosted.ai`
2. `unhosted.dev`
3. `getunhosted.com`
4. `unhosted.org`

Handles:
- GitHub: `unhosted-ai`
- X/Twitter: `@unhosted_ai` or `@getunhosted`
- Mastodon: `@unhosted@hachyderm.io`
- Discord server: `unhosted`
- Reddit: `r/unhosted`

---

## launch sequence

- **Day -14**: lock org/domain/handles, commit manifesto
- **Day -7**: MVP running on 2-device cluster, 3+ real benchmark configs
- **Day 0**: HN post at 8am PT Tue/Wed  
  Title: *Unhosted: Pool your devices to run frontier AI without a datacenter*
- **Day 0 +2h**: tailored Reddit cross-posts (`r/selfhosted`, `r/LocalLLaMA`, `r/homelab`)
- **Day +1**: PRs to awesome lists
- **Day +3**: outreach to 3 mid-tier YouTubers

---

## what to do this week

1. lock GitHub org + domain today
2. rewrite manifesto in your own voice
3. choose accent color (red/green/amber)
4. ship one reliable working demo before any announcement

---

## day-one contents for `unhosted` repo

Must include:
- `README.md` (pitch, quickstart, hardware matrix, status, roadmap, links)
- `LICENSE` (AGPL-3.0 recommended for ethos alignment)
- `CONTRIBUTING.md`
- `CODE_OF_CONDUCT.md`
- `.github/ISSUE_TEMPLATE/` (bug + feature templates)
- `.github/workflows/` (basic PR CI)
- `SECURITY.md`
- `CHANGELOG.md`
