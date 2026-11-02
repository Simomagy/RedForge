# Porting a format from the C# RedForge

The Rust port reads, writes and converts the same game files as the C# RedForge
(`../RedForge-Source`, frozen as the reference). ""Done" for a format means that on a
varied sample of game files the Rust and the C# produce **identical XML**, and a
file the Rust writes from XML is read by the C# exactly like one the C# writes
itself.

## The reference: `rf-oracle`

`tools/oracle` is a console app that runs the C# code headless on Linux. Build it
once (it lands in `target/oracle`):

```sh
cd tools/oracle && dotnet build -c Release --artifacts-path ../../target/oracle
```

`rf compare` drives it:

```sh
export REDFORGE_GAME_DIR="/run/media/msstu/Duomo/Games/Red Dead Redemption 2"
export REDFORGE_ORACLE=/path/to/target/oracle/bin/Oracle/release/rf-oracle   # if not next to rf
cargo build --release
target/release/rf compare ytd --limit 300 --write --work target/compare/ytd
```

It picks files of that extension spread over the whole game and converts each to
XML with both. With `--write`, it also turns the C#'s XML back into a game file
with both and has the C# read both again. Every difference is listed with the first
differing line, and with `-v` all of them are. `--filter` narrows the paths.

The C# also writes files next to some XML files (textures as `.dds`, in a folder
named after the file). These are compared byte for byte too.

## Where the code goes

- **`crates/rf-formats/src/formats/<format>.rs`** (or a `<format>/` folder). Each
  module exposes `to_xml(file, cx)` and `from_xml(xml, cx)`, which
  `formats/mod.rs` already dispatches to, plus its plain-data structs. Modules that
  aren't ported return `todo(...)`.
- **The data structs** are plain data: no rendering, no engine types. Name fields
  after the C# ones in snake_case (`unknown_10h`, `block_count`), keep every
  unknown field, and keep the C# defaults (`Unknown_18h = 1` → `1`).
- **Only read the C# you are porting.** It is at
  `../RedForge-Source/RedForge.Games.RDR2/` (`RSC8/`, `Files/`) and
  `../RedForge-Source/RedForge.Core/` (engine base classes whose XML read/write the
  format inherits, e.g. `Engine/Texture.cs`, `Engine/Mesh.cs`).

## Infrastructure

**RSC8 reading** (`rsc8::Rsc8Reader`)
- Primitives: `r.u32()`, `f32`, `vec3`, `floats::<N>`, `pod::<T>()`.
- Pointers: `r.u64()` reads one; `r.follow(ptr, |r| ...)` and `r.at(ptr, ...)`
  read there. `r.ptr(|r| ...)` does both.
- Arrays and strings: `array_at::<T>(ptr, n)`, `items_at(ptr, n, |r| ...)`,
  `string_at(ptr)`.
- `ArrayHeader::read` is the `Rsc8Arr`/`Rsc8Lst` header (ptr, u16 count,
  capacity, unknown); `ArrayHeader32` is the 32-bit one.
- C#'s `Rsc8Ptr`, `Rsc8Lst`, `Rsc8PtrArr` and so on are all expressible with
  these. Shared blocks (the C# `BlockPool`) can simply be read twice. Watch out for
  back-pointers (parent pointers), which would recurse forever: read those as raw
  pointers.

**RSC8 writing** (`rsc8::Rsc8Writer`, `BlockBuf`)
- A struct is a `BlockBuf`: `b.u32(..)`, `b.ptr(Some(id))`,
  `b.ptr_offset(id, off)` (C#'s `WritePtrEmbed`), `b.pad_to(len)`.
- Add it with `w.add(buf, physical)`. Use `w.reserve()` + `w.set(id, ..)` to add a
  struct before its children; **the root must be the first block reserved**.
- Strings and plain arrays: `w.string(s)` (equal strings are written once),
  `w.array(Some(&items))`, `w.array_in(.., physical)`, `w.pointer_array(&ids)`.
- The file base: `rsc8::BlockMap::write`, then `w.build(version)` returns the
  loose file (RSC8 header, deflate), like `Rsc8DataWriter.Build`.
- The layout follows the C# (`BuildPages3`): root first, then blocks from largest
  to smallest, 16-byte aligned, one page per segment. Byte-identical output isn't
  required (the C# sort isn't stable), but count/capacity fields, flags and
  version must match.

**XML** (`xml::XmlWriter`, `xml::XmlNode`)
- The writer reproduces `XmlMetaNodeWriter` exactly. Its method names mirror the
  C# ones: `WriteUInt32` → `w.u32(id, v)`, `WriteSingle` → `w.f32`,
  `WriteString` → `w.string`, `WriteJenkHash` → `w.hash`,
  `WriteNodeArray` → `w.node_array`/`node_array_with`,
  `WriteNode` → `w.node`, raw arrays → `w.f32_array` etc., and
  `WriteExternal` → `w.external`.
- Ids starting with `@` are attributes, as in the C#.
- Floats come out exactly like .NET's (verified on 1.9 million values), so use
  `w.f32` and never `format!` a float yourself.
- The reader mirrors `XmlMetaNodeReader`: `node.u32(id, default)`,
  `node.string(id)`, `node.hash(id)`, `node.map_array(id, |n| ...)`,
  `node.external(name)`. Arrays return `None` when absent or empty, as the C#
  "Nullable" readers do.

**Hashes** (`hash::HashIndex`)
- The C#'s `JenkIndex.Ensure(name, ...)` calls add names *while reading*, and
  those names change later XML output. Port every `Ensure` with
  `cx.hashes.ensure(name)`.
- `w.hash(id, h)` writes a known name or the C#'s collision name
  (`oIZmQFA_0xE9F32960`).

## Checklist

1. Port the structs with RSC8 read and write, keeping the C# field order.
2. Port `Read`/`Write(MetaNodeReader/Writer)` into `from_xml`/`to_xml`, keeping the
   C# conditions ("written only if non-zero" and so on) exactly.
3. While fixing, run small targeted compares (`--limit 30`, `--filter`). To
   finish, run one `rf compare <ext> --limit 100 --write` over varied files.
   Fix what shows up if it's quick. Rare edge cases that would take long go in
   the module doc as known issues instead: perfection isn't the goal, and bugs
   get fixed as they come up. When the C# itself fails on a file, that's fine;
   note it.
4. Add a test (unit or under `tests/`, skipped without `REDFORGE_GAME_DIR`).
5. `cargo clippy --all-targets` clean, `cargo fmt`.
6. Only touch your format's files. If shared code (`xml/`, `rsc8/`, `hash.rs`)
   really needs something, add it without changing existing behaviour, and say
   so in your report.
