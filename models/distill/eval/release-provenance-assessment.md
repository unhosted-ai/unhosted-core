# Helmsman release-provenance assessment (2026-07-01)

Question: can the Helmsman weights (trained on Claude Opus 4.8-generated
instruction-response pairs) be published publicly on Hugging Face?

## What Anthropic's terms say

The Claude Help Center article "Can I use my Outputs to train an AI model?"
draws the line as: outputs MAY train models that don't compete with
Anthropic's (examples: sentiment analysis, categorization, summarization
tools, extraction, semantic search, anomaly detection) — and MAY NOT train
"general purpose chatbots", "models designed for open-ended text
generation", including "using Outputs as training targets for models".
Consumer and commercial terms both carry the no-competing-model clause, and
2026 enforcement (xAI/Cursor block, third-party OAuth crackdown) shows the
policy is actively applied.

## Where Helmsman sits

Honestly: on the wrong side of the letter for a public release. Helmsman is
an instruct-tuned LLM capable of open-ended generation, and its training
targets were Claude outputs (both the authored orchestration pairs and the
earlier HF dataset). It plainly doesn't *compete* with Anthropic in any
market sense — a 4B local specialist is not a Claude substitute — but the
help center's prohibited list doesn't hinge on market impact, and
"specialized but open-ended" is not one of its permitted examples.

## Recommendation

1. **Keep the weights private** (HF repo stays private; local/LM Studio use
   for the maintainer). Creation for personal, non-competing use is the
   defensible zone; public redistribution is not.
2. **Publish the recipe, not the weights.** This aligns exactly with the
   Unhosted product thesis: "turn a big model into your own small private
   one." Users run `distill run` with their own API key and their own
   teacher; the public artifacts are the pipeline, the eval harness, the
   gap-report methodology, and the training-data design lessons — all
   already open in unhosted-core.
3. **If public weights are wanted later**, regenerate the training set with
   a permissively-licensed teacher (e.g. an Apache/MIT large open model) and
   train from that. The v0.2 data design (working-first ordering, gap-class
   targeting) transfers unchanged; only the gold-answer author changes.

Decision owner: Ankur. This note is the input, not the verdict.

Sources:
- https://support.claude.com/en/articles/12326764-can-i-use-my-outputs-to-train-an-ai-model
- https://www.anthropic.com/news/usage-policy-update
- https://venturebeat.com/technology/anthropic-cracks-down-on-unauthorized-claude-usage-by-third-party-harnesses
