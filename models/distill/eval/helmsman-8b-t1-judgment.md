# Helmsman 8B take-1 — rubric judgment (46 held-out prompts, temp 0, Q4_K_M)

MLX QLoRA on 4-bit Qwen3-8B, iter-100 checkpoint (best val of an overfitting
lr=1e-4 run), fused with --dequantize, requantized Q4_K_M.

**Total: 23 / 46 (50%)** — WORSE than 4B v0.2 (32.5) and 4B v0.1 (30).
Mean 101 tokens, 0/46 truncated, no <think> leakage.

## Dominant pathology: degeneration loops

Seven answers collapse into sentence-repetition spirals (#4, #6, #18, #23,
#40, #41, #43) — e.g. "The onboarding docs are live. The test coverage is
live." repeated four times. This is under-trained SFT (iter-100 = 0.77
epochs) + the 4-bit-train → dequantize → requantize roundtrip, not a
capacity problem.

## Capacity signal is real where coherence holds

- #21 senior-vs-juniors: the ONLY variant to make the correct call, with
  clean reasoning ("juniors add two people who need mentoring; the senior
  can set direction"). Both 4B versions and their training data failed this.
- #3 queue math: clean 4 hours. #33 postmortem: crisp symptom-vs-root.
- But #1 botches max(3,5) and #38 prices an engineer at $12/hour — the
  arithmetic slips aren't gone.

## Fix in flight (take-2)

Same data, lr 5e-5 (half), val every 50 iters, pick best checkpoint
properly. MLX makes each attempt ~15 min, so hyperparameter iteration is
cheap. If take-2's best checkpoint still degenerates, next lever is
training on the bf16 base in MLX (memory permitting) to skip the quantize
roundtrip.

Per-row: 1:0 2:0.5 3:1 4:0 5:0 6:0 7:1 8:0 9:0.5 10:0.5 11:1 12:0.5 13:0.5
14:0.5 15:0.5 16:0 17:0.5 18:0 19:1 20:0.5 21:1 22:1 23:0 24:0.5 25:0 26:1
27:1 28:0.5 29:0.5 30:0.5 31:0.5 32:1 33:1 34:0.5 35:0 36:1 37:0.5 38:0
39:0.5 40:0 41:0 42:1 43:0 44:0.5 45:1 46:1

---

# Take-2 addendum (lr 5e-5, iter-100 checkpoint)

**Total: 25.5/46 (55%)** — mean 92 tok, 0 truncated. Degeneration mostly gone
(#4 still loops), but coherence stays below 4B v0.2 (32.5). New signature
failure: #2 computes all three totals correctly ($10,400/$10,100/$12,000)
then declares the $12,000 quote cheapest — synthesis failure AFTER correct
arithmetic. #19 sacrifices "good" on a compliance audit (worst answer any
variant gave). #1 max(3,5)=3 error persists across both takes — possibly
damage baked into the 4-bit base.

Per-row: 1:0 2:0 3:0 4:0 5:0 6:0.5 7:1 8:0 9:1 10:1 11:1 12:0.5 13:0.5 14:1
15:1 16:0 17:0.5 18:0.5 19:0 20:1 21:0 22:1 23:0.5 24:0.5 25:0 26:1 27:1
28:0.5 29:0 30:1 31:0.5 32:1 33:1 34:0.5 35:0 36:1 37:1 38:0 39:0.5 40:0.5
41:0 42:1 43:0.5 44:1 45:1 46:1

**Verdict on the MLX 4-bit path: rejected for release.** Two takes, two
checkpoint strategies, both below the 4B. The quantize-train-dequantize-
requantize roundtrip costs more coherence than 8B capacity buys. Last
untested config: MLX LoRA on bf16 weights (skips the roundtrip).
