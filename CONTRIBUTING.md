# Contributing to Common

Thanks for helping build the VLC of AI compute. This project is Apache-2.0 licensed; by contributing you agree your contribution is licensed the same way.

## Ground rules

1. **Real over fake.** No mocked inference presented as real, no fabricated benchmark numbers, no feature marked done without evidence. Read `AGENTS.md`; it applies to humans too.
2. **Requirements are tracked.** Every feature maps to a row in `docs/REQUIREMENTS.csv`. Status changes need evidence in `docs/VERIFICATION.md`. Run `python3 scripts/validate_registry.py` before you commit.
3. **Secure by default.** Never add a plaintext peer transport, a non-loopback default bind, or anything that sends prompts off-device without explicit consent.
4. **Licenses.** Do not copy code from GPL/AGPL projects. New dependencies must be permissively licensed (see ADR-0001) and justified in the PR.

## Development

```sh
cargo build
cargo test                     # unit tests
scripts/fetch-fixtures.sh      # pinned engine + tiny fixtures into ./.common
COMMON_HOME=$PWD/.common cargo test   # also runs real-inference tests
cargo clippy --all-targets -- -D warnings
cargo fmt --all
```

Decisions that change architecture get an ADR in `docs/decisions/`. Keep PRs focused; include tests; explain what you verified and on which hardware.

## Tests that need hardware

Tests that need a second physical machine, a GPU we do not have, or a mobile device are labelled in their names and skip with a message when the hardware is absent. Never make them pass by faking the hardware.
