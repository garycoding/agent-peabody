# Licensing

Copyright © 2026 Gary Frattarola.

## How this project is licensed

The code is licensed under `AGPL-3.0-or-later`. You may use, modify, and distribute it under those terms. The full text is in [`LICENSE`](LICENSE) and [`LICENSES/`](LICENSES/). No other licence is offered.

## Per-bucket licensing

Different kinds of content carry different licenses (encoded in [`REUSE.toml`](REUSE.toml) and per-file SPDX headers):

| Bucket | License |
|---|---|
| Code, build scripts, configs, the version pin | `AGPL-3.0-or-later` |
| Documentation (`docs/`, `README.md`, this file) | `CC-BY-4.0` |
| Brand marks: the app icon (`brand/`) and the icon files made from it (`crates/agent-peabody-app/icons/`) | `LicenseRef-AllRightsReserved` (all rights reserved) |

## Third-party software

Peabody is built from Rust crates, almost all under permissive licenses (MIT, Apache-2.0 or BSD). A few crates that Tauri brings in (`cssparser`, `cssparser-macros`, `selectors`, `dtoa-short`, `option-ext`) are under `MPL-2.0`, a weak copyleft licence that binds only changes to those crates themselves, which Peabody does not make.

## REUSE compliance

This repo is [REUSE](https://reuse.software/)-compliant. Every file has a license via an SPDX header or a `REUSE.toml` annotation. Verify with:

```bash
uvx --from "reuse[charset-normalizer]" reuse lint
```

Any new file needs an SPDX header or a `REUSE.toml` entry, or the lint breaks.
