# Contributing to Unhosted

The project is being built in public, in the open. Small contributions are welcome. The bar is the same as the brand: honest, specific, no marketing.

## Before you start

Read the [manifesto](MANIFESTO.md). If the values there don't match yours, this project will frustrate you — there are many other ways to build AI infrastructure, and that's a good thing.

## Filing issues

- **Bugs**: use the bug report template. Include hardware, OS, model, exact command, and what happened.
- **Feature requests**: use the feature request template. Tell us what problem you're trying to solve, not what implementation you want.
- **Questions**: use Discussions, not Issues. Don't @ maintainers.

If you're not sure whether something is a bug or expected behavior, file it as an issue. We'd rather close as "expected" than miss something.

## Pull requests

Right now: there's no code to PR against. Once there is, the rules are:

1. **One thing per PR.** Refactors, features, and fixes get separate PRs.
2. **Tests for behavior, not coverage.** A PR that adds a feature without tests will be closed. A PR that adds tests for trivial getters will not be merged.
3. **No marketing copy in code or comments.** Comments explain non-obvious WHY, not WHAT. See [BRAND.md](BRAND.md) for voice rules — they apply to code too.
4. **Sign your commits.** `git commit -s`. We use the [Developer Certificate of Origin](https://developercertificate.org/).

## Local development

The runtime doesn't exist yet. Once it does, this section will describe how to run a local two-node cluster on your dev machine.

## What we won't accept

- Code that adds telemetry without an opt-in flag.
- Code that gates a feature behind a hosted service.
- Cosmetic refactors that don't change behavior.
- AI-generated PRs without disclosure of which parts are model-written.

## Communication

GitHub Issues for bugs and tracking. GitHub Discussions for questions and ideas. Discord for real-time chatter (link will appear on the [org profile](https://github.com/unhosted-ai) once it exists).
