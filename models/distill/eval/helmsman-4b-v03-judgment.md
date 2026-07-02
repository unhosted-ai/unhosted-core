# Helmsman 4B v0.3 — judgments

## Early-stop checkpoint (MLX bf16, lr 5e-5, iter-150 = best val 2.08)

**22.5/46 (49%) — REGRESSION vs v0.2's 32.5.** Mean 80 tok, 0 truncated.
Pathologies: role-token leaks ("$60user", "50user" mid-answer), MORE
verdict-body contradictions than any 4B before (#9 verdict says model,
body argues paralegal; #19 says sacrifice speed, body says cheap; #38
argues test suite then concludes integration; #4 states a plan then calls
it wrong), plus re-broken shapes v0.2 had fixed (#1 CPM).

**Diagnosis: recipe, not data.** The winning v0.2 was torch lr~2e-4, 3 FULL
epochs, no early stop. Best-val checkpointing selects ~1-epoch adapters
whose style is half-baked — and a half-learned decisive voice produces
confident contradictions. VAL LOSS IS A BAD SELECTOR for behavioral
quality at this scale: every best-val checkpoint (8B t1/t2, 4B v0.3)
judged WORSE than fully-trained comparators. Bright spot: #24 (silent
client) got the best answer of any variant — keep building + weekly
check-ins.

Per-row: 1:0 2:0.5 3:0 4:0 5:0 6:1 7:1 8:0.5 9:0 10:1 11:1 12:0.5 13:0
14:0.5 15:0 16:0 17:0.5 18:0.5 19:0 20:1 21:0 22:0 23:0.5 24:1 25:0.5
26:1 27:1 28:0 29:0.5 30:0.5 31:0.5 32:1 33:0.5 34:0.5 35:1 36:1 37:0
38:0 39:1 40:0 41:0 42:0.5 43:1 44:1 45:1 46:0.5 = 22.5

## Full-3-epoch checkpoint (iter-435): pending below.

## Full-3-epoch checkpoint (iter-435): 29/46 (63%)

Full-bake beats best-val (29 vs 22.5) — third confirmation that val-loss
checkpoint selection is the wrong tool. Still below v0.2 (32.5), so
**v0.2 remains the release**. But the signal is mixed, not negative:
- FIRST 4B to get #21 senior-vs-juniors right ("Direction and capability
  are not something juniors build").
- Best-of-any-variant answers on #14 (on-call distributional effects),
  #24 (silent client: build + explicit check-in + pause trigger),
  #34 (90-day loop), #43 (dry-run-without-the-person).
- Lost the math cluster v0.2 won (#1 CPM, #2 contractor, #3 queue all 0)
  and "user" glitch tokens appear mid-answer (#3 "60user") -> MLX chat
  template applied at train time doesn't match llama.cpp's at inference.

Per-row: 1:0 2:0 3:0 4:0.5 5:0 6:0.5 7:1 8:0 9:0.5 10:1 11:1 12:1 13:0.5
14:1 15:1 16:0.5 17:0.5 18:1 19:0.5 20:1 21:1 22:0.5 23:0.5 24:1 25:0
26:1 27:0.5 28:0 29:1 30:1 31:0.5 32:0.5 33:1 34:1 35:1 36:1 37:0.5
38:0.5 39:1 40:0.5 41:0 42:0.5 43:1 44:0.5 45:1 46:0.5 = 29

## Next experiment (fresh session): the clean data-vs-recipe test

Train v0.3's 305 pairs with the PROVEN torch recipe (lr 2e-4, LoRA r=16,
3 full epochs, bf16 MPS) — requires rebuilding /tmp venv (macOS purged
torch). If that beats 32.5, the v0.3 data wins and ships; if not, v0.2's
216+59 dataset was already at the sweet spot. Also debug the MLX template
mismatch before trusting any further MLX 4B runs.
