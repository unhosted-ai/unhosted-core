<p align="center">
  <img src="assets/lockup.svg" alt="unhosted" width="420">
</p>

<p align="center">
  <strong>AI that lives where you do.</strong><br>
  Frontier-class inference on hardware you own.
</p>

<p align="center">
  <a href="MANIFESTO.md">manifesto</a> ·
  <a href="#trust-radius">how it works</a> ·
  <a href="#whats-honest">what's honest</a> ·
  <a href="#roadmap">roadmap</a> ·
  <a href="BRAND.md">brand</a>
</p>

---

> **Status: pre-alpha.** Reading this README is currently the only thing that works. The manifesto is real. The product is being built in public.

## What it is

Unhosted pools the computers you already own — and, optionally, the computers your friends own, and beyond that a public swarm of strangers' GPUs — into a single inference cluster. One endpoint. Mac, Linux, Windows. CUDA, Metal, ROCm.

Run Llama 70B across a MacBook and a 4090. Run smaller models on a Pi mesh. Your hardware. Your model. Your data.

## Trust radius

Unhosted has three modes. You decide how far the radius goes.

```
       ╭───────────────────────────────╮
       │   public · pay (USDC)         │   strangers' GPUs, opt-in
       │   ╭───────────────────────╮   │
       │   │  trusted · free       │   │   friends, family, team
       │   │   ╭───────────────╮   │   │
       │   │   │ local · free  │   │   │   devices you own
       │   │   ╰───────────────╯   │   │
       │   ╰───────────────────────╯   │
       ╰───────────────────────────────╯
```

- **Local** — your laptop, gaming PC, home server. No internet required.
- **Trusted** — your roommate's PC, your homelab, your team. End-to-end encrypted, no public exposure, no payment.
- **Public** — a swarm of strangers renting idle GPUs in exchange for USDC per token. You set a price ceiling. Used only when your circle can't fulfill the request.

The first two are free forever. The third is the safety net. You can use Unhosted for the rest of your life and never spend a dollar.

## Quickstart

> Aspirational. The CLI does not exist yet. This block describes the day-one product.

```bash
# install
curl -fsSL https://unhosted.dev/install | sh

# add a node on your LAN
unhosted node add 192.168.1.42

# pair with a trusted peer over the internet
unhosted peer pair friend@example.com

# run inference (local first, trusted next, public last)
unhosted run llama3.1:70b "explain quantum tunneling"

# cap public-swarm spend
unhosted config set public.max-usd-per-month 5
```

## What's honest

This section replaces the typical "Features" list. It's the truth about what works:

| Capability                    | Status      | Notes                                                              |
|-------------------------------|-------------|--------------------------------------------------------------------|
| Single-machine inference      | shipped     | v0.0.1. Wraps llama.cpp `llama-server`. Smoke-tested on M-series.  |
| LAN cluster (request routing) | shipped     | v0.0.2. Round-robin across local + peers; verified end-to-end.     |
| mDNS peer discovery + pairing | shipped     | v0.0.3. One-click pair in the app sidebar; hot-reload routing.     |
| Model management (`unhosted pull`) | shipped | v0.0.3. Known short names + direct GGUF URL support.               |
| VRAM-pooling (layer splitting) | building   | v0.0.4+. Needs llama.cpp built with `-DGGML_RPC=ON`.              |
| Trusted-peer pairing          | designed    | v0.1.0. WireGuard-style.                                           |
| Public swarm (USDC)           | designed    | v0.3.0+. See [design/0001](design/0001-public-mode-architecture.md). |
| Verifiable inference          | research    | Optimistic + redundancy planned; ZK proofs when affordable.        |
| Web UI / desktop app          | designed    | v0.1.0+ web, v0.2.0+ Tauri. See [design/0002](design/0002-application-frontends.md). |
| Windows GPU support           | designed    | After Mac + Linux are stable.                                      |

Reproducible benchmarks land in `benchmarks/` once any code exists. We will publish honest tokens-per-second numbers, not marketing language.

## Roadmap

1. Single-host inference wrapping llama.cpp (Mac, Linux)
2. Two-host LAN cluster running Llama 3.1 70B end-to-end
3. Three benchmark configurations published with reproducible scripts
4. Trusted-peer mode (friend cluster over WireGuard)
5. Public swarm MVP (testnet first, USDC mainnet later)

No dates. We will ship and tell you what works.

## License

[AGPL-3.0](LICENSE). Read it, fork it, audit it, ship it. You can't take it, host it as a paid service, and pretend you wrote it.

## Brand and project

The brand exists on purpose, in public. See [BRAND.md](BRAND.md) for visual identity and voice rules. See [MANIFESTO.md](MANIFESTO.md) for why this project exists.
