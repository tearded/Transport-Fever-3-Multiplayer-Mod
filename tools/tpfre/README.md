# tpfre

A static binary-analysis kit for decoding a game executable fast, built for
release day (docs/DAY_ONE.md) and for coding agents that ask many small
questions. `tpfre index` reads a stripped MSVC x64 executable once, in
parallel, into one SQLite file; `tpfre q` answers questions about it with
short lines, one fact per line; `tpfre diff` compares two builds.

It replaces the slow path of the TPF2 work: a full Ghidra auto-analysis
("expect hours", 48 GB of RAM) followed by Python queries over CSV dumps
(`tpf2-multiplayer/tools/re/xq.py`, `whois.py`, `tpfdis.py`). On Transport
Fever 2 (72 MB) the whole index takes about 4 seconds, and a query answers in
10 to 180 ms.

It reads the binary and never writes to it, runs nothing, and refuses what it
cannot do rather than guess. It has its own Cargo workspace and lockfile, so
the main workspace's build is not affected.

## Build and run

```bash
cd tools/tpfre
cargo build --release
./target/release/tpfre index "/path/to/TransportFever3.exe" -o tpf3.tpfdb
./target/release/tpfre q tpf3.tpfdb info
```

The database is written to the current directory (`<stem>.tpfdb`) unless
`-o` says otherwise, never next to the game. It is written under a temporary
name and renamed when complete.

## Game update workflow (Windows PE builds)

Run from `tools/tpfre`; keep archives and caches private, outside Git. Before
Steam replaces a supported build, snapshot its install. After the update,
snapshot again under a new directory and compare:

```powershell
$tpfre = './target/release/tpfre.exe'
& $tpfre archive --game 'D:/SteamLibrary/steamapps/common/Transport Fever 3' --build 25686323 --out "$env:USERPROFILE/TPF3-MP-builds/25686323-sources"
& $tpfre audit "$env:USERPROFILE/TPF3-MP-builds/25533170-sources" "$env:USERPROFILE/TPF3-MP-builds/25686323-sources" --profiles ../../profiles --cache "$env:USERPROFILE/TPF3-MP-builds/indexes" --json > audit.json
# After manual investigation and a profile for the exact new executable:
& $tpfre verify "$env:USERPROFILE/TPF3-MP-builds/25686323-sources" --profiles ../../profiles --json > verification.json
```

`archive` copies the chosen root executable (`--exe` overrides the name),
root EXE/DLL/SO/dylib files, loose `.tl`/`.lua`/`.json`/`.gs` sources and
those sources extracted from ZIP containers. It supports stored/deflated ZIP
and TF3's `UG` local headers. It records SHA-256 and sizes of archived files.
For loose inputs `sources.hash_scope` is `file`; for ZIPs it is
`script_entries`, the hash of the sorted JSON array of `(entry name, size,
content SHA-256)` tuples, excluding unrelated textures/audio and compression.
The Steam manifest is copied when found beside `common`
(`--steam-manifest` overrides discovery); its build ID must match `--build`.
It retains depot IDs and the branch, when present. Without that manifest,
the build label is supplied by the operator; it is not independently verified.
Assets such as textures and audio are omitted. ZIP64, encrypted entries,
linked inputs, unsafe paths and excessive script sizes are refused.

A destination must be new and outside both the install and Git worktrees.
The install is read only; selected contents are hashed again and the complete
file inventory (paths, sizes, modification times) is checked before completion
to catch updates during copying. Failure leaves `.incomplete`; consumers refuse it.
`build.json` is written last. Archive input is checked against every recorded
file's hash and size, including scripts and libraries.

`audit` uses the hookcore profile parser/scanner for every target in every
profile matching the old executable's exact identity. It reports missing or
ambiguous signatures, invalid offsets, changed prologues, old/new RVAs and
normalized containing-function differences. Normalization excludes address
operands, retaining field offsets and other constants. Equality does **not**
prove callee/data equivalence or ABI compatibility. Matcher suggestions are
investigation hints, never automatically accepted targets. SHA-keyed indexes
are cached without modifying profiles or executables.

Profile discovery accepts flat `*.toml` files and immediate per-build
directories containing `hooks.toml`, in deterministic path order. Other
metadata inside a bundle is not parsed as a hook profile. The runtime's
native data and profile are paired in these bundles; `verify` checks profile
bytes and does not certify their Rust ABI data (see [HOOKS.md](../../docs/HOOKS.md#reviewing-the-native-data-for-a-game-update)).

An undecodable function body (for example embedded data) is reported with
`normalized_function_equal: null` and `comparison_error`; it requires manual
review, while the remaining targets are still checked.

Two complete archives also produce added/removed/changed script lists.
An explicit EXE path is accepted for older EXE-only archives, but reports the
script comparison as unavailable and requires review. `verify` checks the
exact new identity and **all** profile targets, including optional ones; absent
executables or unknown builds are errors, never skipped tests.

Exit codes: `0` = no static differences requiring review (`audit`) or all
profile bytes verified (`verify`); `1` = review required/target failure;
`2` = CLI usage error; `3` = invalid input or incomplete analysis. Reports
always say `runtime_verified: false`. These commands never activate hooks,
approve a build or replace real-game acceptance. Keep the existing
feature → dev → acceptance → main release gates.

## What the index holds

| table | what |
|---|---|
| `functions`, `chunks` | Every function: `.pdata` entries with chained unwind info collapsed onto their primary (a split function keeps its chunks), plus functions without unwind info (leaves) found as targets of direct calls, tail jumps, `lea` of code, vtable slots and relocated data pointers, bounded by following their control flow. |
| `calls` | Every direct call, tail jump (a `jmp`/`jcc` leaving the function) and call or jump through the IAT, with its call site. |
| `xrefs` | Every RIP-relative operand, with the instruction, its function and the access (`addr` for `lea`, `read`, `write`, `rw`, `icall`, `ijmp`). |
| `dataptrs` | Absolute pointers in data (from the base relocations) to a function or a string: vtables aside, function tables and `{name, function}` pairs. |
| `strings` | ASCII and UTF-16LE strings in data sections (NUL-terminated, at least 4 characters, starting on a NUL boundary, plus any string code points at). |
| `names` | Every naming source, kept apart with a confidence (below). |
| `func_src` | Source file per function: `direct`, `ambiguous` or `inferred`. |
| `types`, `vtables`, `vslots` | MSVC RTTI: type descriptors, vtables found through their complete object locators, and their slots. |
| `imports`, `sections`, `meta` | IAT slots as `dll!func`; sections with entropy; the binary's SHA-256, size, PE timestamp, image base, counts and warnings. |

Disassembly is linear, per chunk, with iced-x86. MSVC puts switch jump tables
in `.text` after a function's code; a `[base + index*4 + disp]` operand whose
displacement lies later in the same chunk marks one, and it is skipped rather
than decoded as code.

### Naming sources

| source | confidence | meaning |
|---|---|---|
| `funcsig` | `exact` / `ambiguous` | The function references a `__FUNCSIG__`-shaped assert string: it is that function. Ambiguous when it references several that other functions use too. A port of `tools/re/name_functions.py`; see "Agreement" below. |
| `pretty` | `exact` / `ambiguous` | The same for Clang/GCC `__PRETTY_FUNCTION__` strings. |
| `rtti` | `vtable` | The vtable of a class: `Class::vftable` (`Class::vftable@0x10` for a secondary base). |
| `rtti` | `unique` | The function fills exactly one vtable slot: `Class::vfN`. |
| `rtti` | `inherited(N)` | It fills slot N of several vtables at the same index: an implementation classes inherit. Named after the vtable with the fewest slots. |
| `rtti` | `folded(N)` | It fills slots at different indexes: identical code the linker folded. Listed, never used as the name. |
| `import` | `exact` / `delay` / `thunk` | An IAT slot, `dll!func`, or a function that only jumps through one. |
| `export` | `exact` | An exported name. |

A function's shown name is the best of these: an exact `__FUNCSIG__` name,
then an export or import thunk, then a unique vtable slot. A name marked `~`
(`~Twin::A`, `~UI::Base::vf3`) is uncertain: an ambiguous `__FUNCSIG__` or an
inherited slot. Unnamed functions show as `sub_<rva>`.

`__FUNCSIG__` names shown by queries keep template arguments and operators
whole (`ecs::Engine::AddComponent<struct ecs::component::GameSpeed>`); the
Python pipeline's shorter rule cuts inside them (`ecs::component::GameSpeed>`).
The `names` table keeps the Python name, so the two can be compared; both are
searchable.

`__FILE__` attribution follows the same port: a function that references a
source path is `direct` (or `ambiguous` with several), and the functions
between the first and last direct function of one file are `inferred`, since
the linker keeps a translation unit together.

### Packed code and memory dumps

`index` reports packers: a SteamStub `.bind` section, and any executable
section whose entropy is above 7.2, which means its code must come from a
memory dump of the running game. TPF2's `.bind` holds only the stub; its
`.text` is not encrypted on disk (entropy 6.50), and `index` says so.

A raw memory dump of the image (sections at their RVAs, as a debugger's
"dump module" writes it) is indexed with the load address it was taken at:

```bash
tpfre index TransportFever3.dump --image-base 0x7ff6a0000000 -o tpf3.tpfdb
```

Pointers are read against that base, and RVAs stay RVAs, so names and
signatures compare with the file on disk.

### The binary check

The database records the binary's SHA-256, size and PE timestamp. Commands
that need the bytes (`dis`, `sig`, `bytes`) read the file it was made from,
or `--bin <path>`, and refuse it unless its SHA-256 matches. Any command
given `--bin` checks it first. A database of another schema version is
refused too: re-index.

## Commands

Arguments naming an address take an RVA (`0x15aa00`), a VA inside the image
(`0x14015aa00`), `sub_15aa00`, an exact name from any source, or else a
case-insensitive regex over names. Where one address is needed and a name
matches several, the command lists them and exits 2. Every command takes
`--json` (a JSON array of the same facts). Lists are cut at `--limit` with a
`more N` line. Exit status: 0 found, 1 nothing found or refused, 2 ambiguous,
3 error (including a refused binary).

The samples below are from Transport Fever 2 build 35924, some cut short.

### `index <binary> [-o out.tpfdb] [--image-base HEX] [--threads N]`

```
indexed TransportFever2.exe -> tpf2.tpfdb
sha256 782b904a8f7bbdac1f7a18528f1a5c778691e5aa3087c37c351bf6912585175c
functions 138221
functions_pdata 115484
functions_discovered 22737
instructions 11372867
call_edges 1049287
xrefs 779486
strings 58897
rtti_class_type_descriptors 12146
rtti_vtables 7250
naming.functions_named_funcsig 20746
naming.functions_named_ambiguous 3454
naming.functions_file_direct 20062
seconds 3.62 db_bytes 161120256
warning SteamStub (Steam DRM) section .bind present (entropy 7.95); the entry point is in it
warning code in .text (entropy 6.50) is not encrypted on disk: static analysis of it is valid
```

### `q <db> info`

Identity, sections, every count, naming statistics and warnings.

```
sha256 782b904a8f7bbdac1f7a18528f1a5c778691e5aa3087c37c351bf6912585175c
pe_timestamp 0x675ABCC6
image_base 0x140000000
section .text 0x1000 vsize=0x2f08308 raw=0x2f08308 r-x entropy=6.50
section .bind 0x469a000 vsize=0x33810 raw=0x33810 r-x entropy=7.95
warning SteamStub (Steam DRM) section .bind present (entropy 7.95); the entry point is in it
```

### `q <db> func <rva|name|regex> [--limit N] [--lines N]`

Bounds, every name with its source, source file, callers, callees, strings
and vtable slots.

```
func 0x15aa00 GameSim::Step
  bounds 0x15aa00-0x15ac89 size=649 kind=pdata insns=153
  name GameSim::Step funcsig exact | void __cdecl GameSim::Step(__int64,int)
  src game\gamesim.cpp direct
  callers 1 sites=1 address-refs=0
  callee 0x2877a0 sub_2877a0 @0x15aa30 x2
  callee 0xaeaa70 ecs::time_util::TickDays @0x15ab27
  callee 0x2bf61ca MSVCP140.dll!?_Xbad_function_call@std@@YAXXZ @0x15ac62
  string 0x2f32e28 "millis > 0 && dt > .0f" @0x15ac34
```

### `q <db> dis <x> [--max N] [--context N] [--bytes]`

A whole function (up to `--max` instructions), or `--context` instructions
around an address inside one, marked `>>`. Call targets and RIP-relative
operands are named; string operands are quoted.

```
func 0x15aa00 GameSim::Step size=649 chunks=1
0x15aa00  push    rbx
0x15aa0e  cmp     rdx, 0x3e8
0x15aa15  jl      0x15ac41
0x15aa23  call    sub_15ac90
0x15aa30  call    sub_2877a0
```

### `q <db> callers <x> [--depth N]`, `q <db> callees <x> [--depth N]`

Direct edges, breadth first, one per line: depth, caller, callee, first call
site, `xN` for several sites, `[tail]` or `[import]`.

```
1 0x1184d0 CGame::RunGameSimLoop -> 0x15aa00 GameSim::Step @0x1185a0
2 0x112450 sub_112450 -> 0x1184d0 CGame::RunGameSimLoop @0x1124d3
1 0x119140 CGame::Sync -> 0x2bf61dc MSVCP140.dll!_Mtx_lock @0x11926e
```

### `q <db> path <a> <b> [--max-depth N]`

The shortest chain of direct calls (and tail jumps) from A to B.

```
path 2 calls from 0x1184d0 CGame::RunGameSimLoop to 0xaeaa70 ecs::time_util::TickDays
1 0x1184d0 CGame::RunGameSimLoop -> 0x15aa00 GameSim::Step @0x1185a0
2 0x15aa00 GameSim::Step -> 0xaeaa70 ecs::time_util::TickDays @0x15ab27
```

### `q <db> xrefs <x>`

Everything that references an address: calls, RIP-relative operands, data
pointers (with the pointer before them, which names a `{name, function}`
pair), vtable slots. A vtable's references are its constructors and
destructors.

```
xrefs 0x301dc38 UI::CMenuUI::vftable calls=0 refs=2 data=0 vslots=0
ref addr 0x64e2c0 in 0x64e220 UI::CMenuUI::CMenuUI
ref addr 0x65231d in 0x652300 sub_652300
```

### `q <db> str <regex> [--case]`, `q <db> fnstr <x>`

Strings matching a regex (case-insensitive unless `--case`), each with the
functions that reference it; and the strings one function references.

```
0x2f1db30 "trackHighSpeedLength" <- 0xd2670 DataLogger::UpdateStats, 0x569f00 UI::CGameUI::CreateUI
0x67861b 0x3020d40 "CMenuUI::StartSavegame: Game initialization is already active!\n"
```

### `q <db> names <regex>`

Names from every source, one per line with source and confidence.

```
0x1184d0 CGame::RunGameSimLoop funcsig exact
0x118e90 CGame::Step funcsig exact
```

### `q <db> class <regex>`, `q <db> vtable <x>`

Vtables of matching classes with their slots (`shared=N` when a slot's
function fills several), or the vtable at or around an address.

```
vtable 0x301dc38 UI::CMenuUI::vftable offset=0x0 slots=40 col=0x3a797a0
  slot 0 0x661160 UI::CMenuUI::vf0
  slot 1 0x4aca40 sub_4aca40 shared=157
```

### `q <db> file <substr>`

Functions attributed to source files containing a substring.

```
file game\command\commandlist.cpp functions=6 direct=2
0x9d2a00 sub_9d2a00 game\command\commandlist.cpp inferred
0x9d2cf0 CommandList::Swap game\command\commandlist.cpp direct
```

### `q <db> whois <x>`

Every naming source for an address, as `whois.py` did: section, containing
function, `__FUNCSIG__`, RTTI slots, import, file, string, type.

```
whois 0x663370
  section .text
  func 0x663370 UI::CMenuUI::CreatePage +0x0 size=2892 kind=pdata best=funcsig:exact
  funcsig UI::CMenuUI::CreatePage exact | void __cdecl UI::CMenuUI::CreatePage(enum UI::CMenuUI::Page)
  file game\ui\components\menuui.cpp direct
```

### `q <db> sig <x> [--steal N] [--max-length N] [--no-prologue] [--toml]`

A unique signature for a function start, by the rules of
`tools/re/make_profile.py` (and the hook engine's): relative targets,
RIP-relative displacements and absolute addresses become `??`; the signature
starts with the instructions the detour steals (14 bytes by default) and
grows until it matches once in its section; the prologue must be
straight-line code. It refuses what it cannot make unique or safe.
`--toml` prints the profile's `[[target]]` block. On TPF2 its output is
byte-identical to make_profile's for the five named hook targets.

```
sig 0x1184d0 CGame::RunGameSimLoop section=.text sig_len=33 prologue_len=14 unique
signature 48 8B C4 55 56 57 41 54 41 55 41 56 41 57 48 8D 68 A1 48 81 EC B0 00 00 00 48 C7 45 E7 FE FF FF FF
prologue 48 8B C4 55 56 57 41 54 41 55 41 56 41 57

refused sub_2877a0 at RVA 0x2877a0: the prologue must cover 14 bytes but `call` at +12 is not
straight-line code, which the detour engine refuses to steal (the first 12 bytes would do for a
near detour: --steal 5)
```

### `q <db> bytes <pattern>`

Search code for a byte pattern (`48 8B ?? 05` or `488B??05`).

```
matches 12
0xdaf94 in 0xdaf90 engine::GetGlobalGLContext +0x4
```

### `q <db> validate <spec>`

Check known RVAs the way `name_functions.py --validate` does: `<rva> <name>`
must be `__FUNCSIG__`-named, `<rva> ~ <file>` must not be (the binary embeds
no signature for it) and must be attributed to that file.

```
0x15aa00 OK GameSim::Step [exact] src=game\gamesim.cpp
0x2877a0 OK not funcsig-named; source=game\gametime.cpp
validate 8 checked: ALL OK
```

### `diff <old.tpfdb> <new.tpfdb> [--moved]`

Named functions that moved, resized, appeared or disappeared between two
builds, matched by `__FUNCSIG__` name and source file as
`tools/re/diff_builds.py` does. A uniform shift is summarised.

No second real build exists yet; TPF2 against itself, then a line from the
test fixture's patched build:

```
diff TransportFever2.exe (782b904a8f7b) -> TransportFever2.exe (782b904a8f7b)
named old=24200 new=24200
summary unchanged=24200 moved=0 resized=0 appeared=0 disappeared=0

resized Beta::Run game\alpha.cpp 0x1040->0x1040 size 27->28
```

### `match <old.tpfdb> <new.tpfdb> [--dry-run] [--limit N]`

Carries function names from an old build to a new one that lacks them.
Transport Fever 3 keeps RTTI and `__FILE__` but drops the `__FUNCSIG__`
strings that named 20,000 of TPF2's functions, and `diff` matches by those
names. `match` pairs functions by what survives in both builds instead:

- a string (an assert's condition, a log line) that exactly one function
  uses in each build: each shared string is a vote;
- the same slot of the same class's vtable, when the vtable has as many
  slots in both and no other slot holds the function;
- from those anchors, the call graph: a callee or caller with exactly one
  plausible partner (instruction and callee counts within a quarter),
  which has it as its only one too; hubs (over 64 neighbours) are skipped;
- between two matched functions of one source file, the unmatched ones in
  the gap, in address order, when both gaps hold as many.

A pair is kept only when each side is the other's single best candidate
and both builds' source files, where known, agree. The names go into the
new database's `names` table as source `matched` (confidence `string(N)`,
`rtti`, `calls` or `file-order`, with the evidence in `detail`), so `names`,
`func` and the rest find them; the old database is only read. It prints
its progress, and takes about 2 seconds for TPF2 onto TF3:

```
matched 10368 functions, 3942 of them named in the old build (calls 6165, file-order 154, rtti 2026, string 2023)
0x159390 GameSim::Step [string(3)] old 0x15aa00: "millis > 0 && dt > .0f"
0x11f3b0 CGame::Step [string(2)] old 0x118e90: "m_data->totalTime >= m_data->lastSyncTime"
```

## Performance

Transport Fever 2 build 35924 (72 MB, 115,484 `.pdata` functions), on a
Ryzen 9 5900X (24 threads), release build:

| step | time |
|---|---|
| `index` (hash, parse, disassemble 11.4 M instructions, strings, RTTI, naming, write) | 3.5 s (3.5–4.5 s wall); database 161 MB |
| `q info`, `whois`, `xrefs`, `vtable` | 9–13 ms |
| `q func`, `callers --depth 3`, `callees`, `class`, `path`, `file`, `fnstr` | 20–30 ms |
| `q str` (regex over all 58,897 strings) | 45–56 ms |
| `q dis`, `sig`, `bytes` (read and hash the 72 MB binary first) | 68–87 ms |
| `q names` (regex over every name) | 160 ms |
| `diff` of two TPF2 databases | 180 ms |

Times include process start. SQLite is written in one transaction with
indexes built afterwards; disassembly and string extraction run on every
core.

## Agreement with the Python pipeline

On TPF2 build 35924 the `__FUNCSIG__`/`__FILE__` naming agrees with
`tools/re/name_functions.py` exactly:

- 18,897 `__FUNCSIG__` strings, 729 `__FILE__` paths, 88,198 references;
- 24,200 functions named (20,746 exact, 3,454 ambiguous), each with the same
  name, kind and chosen signature;
- 20,062 functions attributed directly to a file, 725 files, and every one of
  the Python map's 55,077 source attributions identical;
- all 8 known RVAs in `investigation/tpf2-baseline/tpf2_known_rvas.txt` pass
  (`q validate`).

The differences are additions: tpfre also finds 22,737 functions without
`.pdata` entries, 8,379 of which fall inside an inferred translation-unit
range (42,753 inferred against Python's 34,374, which counts `.pdata`
functions only). The baseline README's "~4,100 RTTI type descriptors" is a
regex count of `.?AV`/`.?AU...@@` byte runs, which misses names containing
`<` (lambdas, templates) and counts prefixes of longer names; tpfre parses
the structures and finds 12,146 class type descriptors, 7,250 complete
object locators and 7,250 vtables (48,144 slots). About 300 type names too
complex for the demangler (sol2 usertypes) stay mangled.

## Tests

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

`tests/synthetic.rs` builds a small PE32+ in the test (functions with and
without unwind info, a split function, calls, a tail jump, an import, strings
in both encodings, `__FUNCSIG__`/`__FILE__` strings, RTTI, a jump table and
byte-identical twins) and checks every command on it, the refusal of a
different binary, and `diff`. `tests/tpf2.rs` runs against the real TPF2
executable; it is ignored unless asked for:

```bash
TPFRE_TPF2_EXE="E:/SteamLibrary/steamapps/common/Transport Fever 2/TransportFever2.exe" \
  cargo test --release --test tpf2 -- --ignored --nocapture
```

## Limits

- **Direct calls only.** Virtual calls, `std::function`, signals, callbacks
  and calls through registers are not edges. Use `xrefs` on a vtable (its
  constructors), `vtable`/`class` for slots, and `xrefs` on a function for
  who takes its address (`addr` references and data pointers).
- **Linear disassembly.** Data inside a function other than recognised jump
  tables would be decoded as code; on TPF2 no instruction fails to decode.
  Leaf functions are bounded by their control flow and stop at an indirect
  jump.
- **PE32+ x86-64 only.** ELF and Mach-O are recognised and refused with a
  message. TODO(ELF x86-64): function starts from `.symtab` and `.eh_frame`,
  PLT imports; the rest (strings, references, naming) is format-independent.
  The macOS build is arm64, which this decoder does not handle; the Python
  tools cover arm64 naming.
- **Packed code** (SteamStub-encrypted `.text`) must be indexed from a
  memory dump; `index` says when.
- **Names are evidence, not proof.** `~` marks the uncertain ones, and
  `whois` lists every source for an address.

## For agents

Paste this into an agent's context. `DB` is the `.tpfdb` file; addresses are
RVAs in `0x` hex; a name argument is an exact name or a case-insensitive
regex.

```
tpfre q DB info                      what binary, sections, packer warnings, counts
tpfre q DB names REGEX               find functions/classes/imports by name (all sources)
tpfre q DB func X                    one function: bounds, names+source, file, callers count,
                                     callees, strings, vtable slots
tpfre q DB whois X                   every naming source for an address (func, funcsig, rtti,
                                     import, file, string, type)
tpfre q DB str REGEX                 strings (log/assert/UI text) and who references them
tpfre q DB fnstr X                   strings one function uses
tpfre q DB callers X --depth N       who calls X (direct calls and tail jumps only)
tpfre q DB callees X --depth N       what X calls (imports named dll!func)
tpfre q DB path A B                  shortest direct-call chain A -> B
tpfre q DB xrefs X                   every reference to an address: calls, lea/read/write,
                                     data pointers, vtable slots
tpfre q DB class REGEX               vtables of a class, slot by slot (virtual methods)
tpfre q DB vtable X                  the vtable at/around an address
tpfre q DB file SUBSTR               functions of one .cpp (direct/inferred)
tpfre q DB dis X [--context N]       annotated disassembly (whole function, or around X)
tpfre q DB sig X [--toml]            unique hook signature + prologue (make_profile rules)
tpfre q DB bytes "48 8B ?? ??"       byte-pattern search in code
tpfre q DB validate SPEC             check known RVAs (rva name | rva ~ file)
tpfre diff OLD.tpfdb NEW.tpfdb       named functions moved/resized/appeared/disappeared
add --json to any q command for a JSON array of the same facts

Reading output: one fact per line; "0x15aa00 GameSim::Step" = RVA then name;
sub_XXXX = unnamed; a leading ~ = uncertain name (ambiguous __FUNCSIG__ or
inherited vtable slot); @0x.. = call site; xN = N call sites; [tail] tail jump;
[import] call through the IAT; "more N" = cut, raise --limit.
Exit 0 found, 1 none/refused, 2 ambiguous (lists matches: pick an RVA), 3 error.

Recipes:
- find a subsystem: str REGEX (its log/assert text) -> func on the referencing
  function -> callers/callees; or file NAME.cpp.
- name an unknown sub_XXXX: whois; func (its strings, callees, file); callers.
- find a virtual method: class REGEX -> slot N -> func/dis the target.
- find who installs a callback: xrefs FUNC (addr refs, data pointers).
- find a constructor: xrefs Class::vftable (writes of the vtable).
- hook it: sig FUNC --toml (refused = pick a caller or --steal 5 if near).
Not visible: virtual calls, std::function, signals (no direct edge exists).
```
