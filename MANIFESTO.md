# The Unhosted Manifesto

I got tired of paying OpenAI.

I looked around my apartment. MacBook on the desk. Gaming PC under it. An old ThinkPad in a drawer. A Raspberry Pi running Home Assistant. A Mac mini I forgot I owned.

I added it up. I have more compute in my house than I'll ever need for the AI I actually use.

The problem is none of it works together. Ollama runs on one machine. vLLM assumes a datacenter. llama.cpp is single-host. Nothing in the open-source world was built for a house — a few mismatched devices on a flaky home network, owned by one person, used for personal things.

So I'm building Unhosted.

It pools the computers you already own into one inference cluster. You run a model. It figures out which devices have free VRAM, splits the layers, streams tokens back. One endpoint. Your laptop and your gaming PC behave like one machine.

That's it. That's the whole thing.

Three modes:

- **Local** — devices you own, on your network.
- **Trusted** — your friends' or team's devices, paired with yours.
- **Public** — a swarm of strangers renting idle GPUs, paid in stablecoin per token, opt-in.

The first two are free forever. The third is the safety net, for when your hardware can't keep up. You can use Unhosted for the rest of your life and never spend a dollar. That's the design.

---

The economics flipped while no one was watching.

The MacBook I'm typing this on runs models that needed eight A100s in 2023. A 4090 runs models that cost $20/hour to rent. A Raspberry Pi cluster runs models that are good enough for 80% of what people actually do with ChatGPT.

The compute is here. It's already paid for. It's already in our homes.

We're missing software, not hardware.

---

I don't think AI should require a credit card.

I don't think the AI that reads your email, drafts your code, and sees your medical history should run on someone else's server.

I don't think a company should get to decide which questions you're allowed to ask.

This used to be normal. Your text editor doesn't phone home. Your spreadsheet doesn't rate-limit you. Your operating system doesn't train on your files.

AI should work the same way.

---

Unhosted is not a crypto project. No token. No airdrop. No yield farm. The public swarm uses stablecoins because that's how you do permissionless global payments without a US LLC and a Stripe account — same reason a freelancer in Argentina takes USDC. We don't print a coin. We don't run a chain. We don't want you to speculate on us.

It's not a hosted company in disguise. No managed tier we secretly want you to upgrade to. There is no managed tier.

It's not anti-AI. I use AI every day. I want everyone to use AI every day. I just want it to run on hardware they own.

---

What I'm promising:

**Honest benchmarks.** If a model runs at 4 tokens/sec on your hardware, the docs will say 4 tokens/sec. Not "blazing fast." Not "comparable to cloud." 4 tokens/sec.

**Open code.** AGPL-3.0. Read it, fork it, audit it, ship it. You can't take it and turn it into a hosted service and pretend you wrote it.

**No telemetry by default.** If we ever ask for it, it'll be opt-in, with a plain-English explanation of exactly what gets sent and why.

**No surprise pivots.** This won't quietly become a SaaS product in 18 months because we ran out of runway. There is no runway. I'm building it because I want to use it.

---

If any of this resonates:

Star the repo. Run it on your hardware. Tell me what broke.

That's the whole ask.
