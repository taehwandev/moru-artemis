# Contributing to moru-artemis

Thank you for your interest in contributing to **moru-artemis**!

## Submitting Pull Requests

1. Create a feature or fix branch (`git checkout -b feat/my-feature`).
2. Verify your changes with:
   ```sh
   cargo fmt --check
   cargo clippy --all-targets -- -D warnings
   cargo test
   ```
3. Keep device operations tied to an explicit `device_serial` and never commit device data, credentials, or local machine paths.
4. Open a Pull Request with a concise summary of your change.

> The legacy Python fork of `google/artemis` is preserved on the `python` branch (`python-archive` tag).
