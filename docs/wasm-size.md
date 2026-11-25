# Contract wasm size

Soroban has to upload, store and instantiate each contract's wasm, so size is a
deployment cost and, at the limit, a thing that stops a contract being
installable at all. Two files keep it in check:

| File | Role |
| --- | --- |
| `size-budgets.json` | Hard ceiling per contract, checked by `scripts/check_wasm_sizes.py`. A build over budget fails CI. |
| `size-baselines.json` | Last recorded size per contract, checked by `scripts/check_wasm_growth.py`. A build more than 5% over baseline gets a warning annotation and CI still passes. |

This page records what the current builds are made of, so a future regression
can be judged against a shape rather than a single number.

## Measuring

Everything below comes from `scripts/wasm_size_report.py`, which needs nothing
but Python: it reads the wasm section table, then maps each function body in the
code section to its export name and reports the largest.

```bash
stellar contract build
python3 scripts/wasm_size_report.py target/wasm32v1-none/release/coverage_ledger.wasm
python3 scripts/wasm_size_report.py            # every built wasm
```

`twiggy` gives the same view with call-graph aggregation, which is what you want
when a function is large because of what it calls:

```bash
cargo install twiggy
twiggy top -n 20 target/wasm32v1-none/release/coverage_ledger.wasm
twiggy dominators target/wasm32v1-none/release/coverage_ledger.wasm
```

`wasm-opt -Oz` is a quick second opinion on how much a more aggressive
optimiser could recover; it is not part of the build:

```bash
wasm-opt -Oz --enable-bulk-memory --enable-reference-types \
  target/wasm32v1-none/release/coverage_ledger.wasm -o /tmp/ledger.opt.wasm
ls -l /tmp/ledger.opt.wasm
```

Release builds strip the name section (see `Cargo.toml`: `strip = "symbols"`),
so twiggy shows unnamed functions alongside the exports. That is normal and is
why `wasm_size_report.py` reports the unnamed rest as one total.

## Current sizes

Recorded sizes, from `size-budgets.json` and its history comment. The budget is
the measured size plus about 20%, so the two columns together show headroom.

| Contract | Measured | Budget | Headroom |
| --- | ---: | ---: | ---: |
| `coverage_ledger` | 18 017 | 21 700 | 3 683 |
| `reporter_registry` | 6 230 | 7 500 | 1 270 |
| `covered_vault` | ~4 833 | 5 800 | ~967 |
| `upgrade_fixture_ledger` | 18 017 | 21 700 | 3 683 |
| `upgrade_fixture_registry` | 6 230 | 7 500 | 1 270 |

The upgrade fixtures compile their contract's `lib.rs` (see
`contracts/upgrade-fixture-*/src/lib.rs`), so each is the size of the contract
it mirrors.

## Where the bytes go

Measured with `scripts/wasm_size_report.py` on the committed builds in
`contracts/upgrade-fixture-*/testdata/`. Those testdata files are the last build
of each contract that was checked in, so their totals (15 010 and 4 043 bytes)
are smaller than the recorded sizes above; the section proportions are what is
worth reading, and re-running the script against a fresh `stellar contract
build` updates them.

`coverage_ledger` (15 010 bytes):

| Section | Bytes | Share |
| --- | ---: | ---: |
| code | 9 524 | 63.5% |
| custom | 4 059 | 27.0% |
| data | 757 | 5.0% |
| import | 205 | 1.4% |
| export | 194 | 1.3% |
| type | 148 | 1.0% |
| everything else | 82 | 0.5% |

`reporter_registry` (4 043 bytes):

| Section | Bytes | Share |
| --- | ---: | ---: |
| code | 1 992 | 49.3% |
| custom | 1 451 | 35.9% |
| data | 207 | 5.1% |
| import | 121 | 3.0% |
| export | 114 | 2.8% |
| everything else | 120 | 2.9% |

The `custom` section is not padding. It is mostly `contractspecv0`, the XDR
spec of every exported function, argument and struct the contract returns
(3 799 of the ledger's 4 059 custom bytes, 1 191 of the registry's 1 451), plus
`contractmetav0` and `contractenvmetav0`. The host and the CLI need it to encode
calls and decode results, so it is not removable, but it grows with the public
interface: adding a function or returning a wider struct costs spec bytes as
well as code bytes.

### Largest exported functions

`coverage_ledger`:

| Function | Body bytes |
| --- | ---: |
| `list_asset` | 1 088 |
| `post_reserve` | 985 |
| `post_supply` | 485 |
| `coverage` | 322 |
| `is_covered` | 194 |
| `get_reports` | 165 |
| `get_report` | 146 |
| `get_supply` | 111 |
| `get_asset` | 109 |
| `transfer_admin` | 100 |

`reporter_registry`:

| Function | Body bytes |
| --- | ---: |
| `set_reporter` | 351 |
| `revoke` | 127 |
| `get_reporter` | 101 |
| `transfer_admin` | 90 |
| `role_of` | 80 |
| `upgrade` | 52 |
| `__constructor` | 24 |
| `admin` | 4 |

The fixture builds predate the time-locked upgrade functions, which is why
`upgrade` appears instead of `propose_upgrade` / `execute_upgrade` /
`cancel_upgrade`; a fresh build ranks those alongside the others.

Two things follow from the table:

- `list_asset` is the single largest export because it parses the SAC's
  `CODE:GISSUER` name: `token::TokenClient::name`, a strkey length check,
  `String::from_bytes` and `Address::from_string`. That is the price of not
  trusting a caller-supplied code and issuer, and it is worth it.
- The write paths (`post_reserve`, `set_reporter`) dominate over the read paths,
  because each one validates every error before touching storage. Error paths
  are cheap in expectation but not in bytes: each `panic_with_error!` site and
  its format machinery is real code.

## Guidance: what could be feature-flagged

Nothing here is enabled by a feature today. These are the levers, largest and
safest first, if a contract ever needs to shrink:

1. **The v0.1 migration path** (`Report(Address, Tier)`, `migrate_legacy`, the
   legacy branch in `reports_at`). It exists to upgrade the live testnet
   deployment in place. A `legacy-v01` cargo feature, enabled until that
   deployment has migrated and then dropped, would remove the legacy read, the
   migration write and the legacy `DataKey` variant's spec bytes. This is the
   single largest removable feature in `coverage_ledger`.
2. **Query helpers a consumer does not call.** `get_reports`, `get_report` and
   `get_asset` are for dashboards and humans. A contract that only gates on
   `is_covered` pays for them anyway. A `full-queries` feature, on by default,
   would let a size-pressed integrator build without them: about 400 bytes of
   code and their spec entries.
3. **The upgrade entry points.** Removing `propose_upgrade` /
   `execute_upgrade` / `cancel_upgrade` and `PendingUpgrade` from a build saves
   code and spec bytes in both contracts, but it also removes the only way to
   fix a bug in place. Not recommended; listed because it is measurable.
4. **Panic strings.** The `data` section (757 bytes on the ledger) is mostly
   `expect` messages and panic text. Shortening `"admin is set in the
   constructor"` and friends saves data bytes one string at a time, at the cost
   of a worse debugging experience.
5. **Do not ship `release-with-logs`.** The profile exists for debugging; it is
   larger than `release` and is never deployed.

What is *not* worth doing: stripping `contractspecv0` (the contract stops being
callable by normal tooling), or hand-rolling integer formatting to avoid
`core::fmt` (a few hundred bytes for a class of bugs).

## CI

CI runs, after `stellar contract build`:

1. `scripts/check_wasm_sizes.py` — fails when a contract is over its budget in
   `size-budgets.json`. Raise a budget only in the commit that needs the room.
2. `scripts/check_wasm_growth.py` — warns when a contract is more than 5% over
   its baseline in `size-baselines.json`. It never fails the build.

After a deliberate size change, refresh the baseline in the same commit:

```bash
stellar contract build
python3 scripts/check_wasm_growth.py --update
```

That rewrites the recorded sizes from `target/wasm32v1-none/release/`, so the
next PR is measured against what actually shipped rather than against an older
build.
