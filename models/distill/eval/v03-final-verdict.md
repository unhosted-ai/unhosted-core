# v0.3 campaign verdict (2026-07-02)

Every configuration trained on the 305-pair v0.3 data, judged on the same
46 held-out prompts:

| run | recipe | score |
|---|---|---|
| **v0.2 (champion, unchanged)** | torch bf16, r16, lr~2e-4, 3ep | **32.5 (71%)** |
| v0.3 MLX full-bake | r8, lr 5e-5, 3ep | 29 (63%) |
| v0.3 MLX best-val | r8, lr 5e-5, ~1ep | 22.5 (49%) |
| v0.3 MLX r16 hot | r16, lr 1e-4, 3ep | 16.5 (36%) |
| v0.3 torch (3 attempts) | champion recipe | all NaN'd (env) |

## Findings

1. **v0.2 remains the release.** Nothing beat it.
2. **The torch/MPS training stack rebuilt on 2026-07-02 is unusable**:
   NaN under accelerate>=1.13 (pinned, PR #15), NaN at grad-accum 8, and
   NaN even at the smoke-verified batch2/accum4 config at full scale —
   the champion recipe is currently unreproducible on this machine.
3. **Overfit signature at r16/lr1e-4**: training-data catchphrases leak
   verbatim into unrelated answers ("flip condition", "zero slack",
   "don't buy this dilemma a second time") while correctness collapses.
   Lesson for future data authoring: vary the phrasing of recurring
   concepts — a 4B imitates surfaces before it learns substance.
4. The v0.3 data was never tested under the champion recipe (env
   blocked); that comparison remains open, but with three MLX points all
   below v0.2, the burden of proof rose substantially.

## State of the release (unchanged from 2026-07-02 00:30)

Helmsman 4B v0.2 in LM Studio + Unhosted; model card final; weights
private, recipe public; HF card push awaiting a fresh token.
