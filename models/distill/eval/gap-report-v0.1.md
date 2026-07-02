# Gap report — Helmsman 4B v0.1 vs base Qwen3-4B-Instruct-2507

46 held-out prompts, temp 0, Q4_K_M both sides, max_tokens 512. Judged per
`helmsman-4b-v0.1-judgment.md` rubric (1 / 0.5 / 0). Base per-row scores below.

## Headline

| | Helmsman v0.1 | Base Qwen3-4B |
|---|---|---|
| **Rubric score** | **30 / 46 (65%)** | **39.5 / 46 (86%)** |
| Mean completion tokens | 90 | 424 |
| Truncated at 512 tokens | 0 / 46 | 29 / 46 |
| Wall time, 46 prompts | 1.0 min | 4.6 min |

**The v0.1 tuning traded correctness for concision.** The earlier 3-prompt eval
("same conclusions, better delivery") does not survive a 46-prompt eval. The base
model's verbosity IS its reasoning: it thinks out loud, then lands on the right
answer. Helmsman v0.1 learned the Opus surface style — answer first, then a short
justification — but a 4B can't back-solve the way the teacher can. It commits to a
headline before computing, and ~half its failures are wrong or self-contradictory
headlines sitting on top of working that was actually correct.

Failure-mode asymmetry matters as much as the totals:
- **Helmsman's 16 lost points are real reasoning failures** — wrong calls,
  self-contradictions, invented assumptions to dodge constraints.
- **Base's 6.5 lost points are mostly delivery failures** — right analysis cut off
  at the token limit before the verdict (e.g., #3, #16, #24, #38), bullet sprawl,
  one comedy failure (#41: spends half the answer wondering if "band" means a
  music band).

Where Helmsman held or won: summarizing (tight and right, 3/3 vs base's verbose
3/3), one-liners, decisive complete delivery, 4.7x fewer tokens, zero truncation.
Out-of-domain retention intact on both.

## What v0.2 training data must do

1. **Reorder reasoning-before-answer for anything computational or decisive.**
   Current pairs are answer-first ("40 mph. Average speed is..."). For a 4B
   student, gold answers must show 2-4 sentences of compact working FIRST, then a
   committed final call. Keep total length short (the concision win is real);
   change the ORDER. This targets the #1 failure class (headline-first
   commitment: eval #1, #3, #25, #27, #31, #33).
2. **Binding-constraint pairs.** Problems where a stated constraint is tight or
   zero-slack and the gold answer names it and honors it (capacity exactly equal
   to demand, windows too small for naive plans, thresholds that exempt some
   options). Targets #5, #16, #2.
3. **Final-comparison synthesis pairs.** Compute subtotals, then an explicit
   "compare: A=.., B=.., C=.. → pick B" step. Targets #2.
4. **Live-impact priority pairs.** Encode "live customer impact > visible
   deadline > anticipated risk" and "irreversible > everything". Targets #25, #27.
5. **Own-your-share communication pairs.** Speaker's side caused part of the
   problem; gold owns it specifically and first. Targets #29.
6. **Verdict-consistency pairs.** Fix-or-kill / accept-or-push-back style
   questions where the gold verdict word matches the body. Targets #31, #33.
7. **Preserve:** summarizing, consequence-prediction, hiring, tradeoffs pairs —
   these delivered.

Also for v0.2 eval: raise max_tokens to 768+ for the base comparison so its score
isn't deflated by truncation (report both raw and truncation-adjusted).

## Base per-row scores

1:1, 2:1, 3:0.5(t), 4:0.5(t), 5:1, 6:1, 7:0.5, 8:0.5(t), 9:1, 10:1, 11:1, 12:1,
13:0.5(t), 14:1, 15:1, 16:0.5(t), 17:1, 18:1, 19:1, 20:1, 21:1, 22:1, 23:0.5,
24:0.5(t), 25:0.5(t), 26:0.5(t), 27:1, 28:0.5, 29:1, 30:1, 31:1, 32:1, 33:1,
34:1, 35:1, 36:1, 37:1, 38:0.5(t), 39:1, 40:1, 41:0.5, 42:1, 43:1, 44:1, 45:1,
46:1.  — (t) = lost points primarily to truncation of correct analysis.

Total: 33×1 + 13×0.5 = 39.5 / 46.
