# C# bugs the port reproduces

The format ports match the C# RedForge exactly, bugs included, so files and XML
stay interchangeable with it and with Sollumz. These are the bugs found along
the way. Each can be fixed later, at the cost of some XML or files coming out
differently from the C#'s.

## Data that changes or gets lost

| Format | Bug | Effect |
|---|---|---|
| ybn (and bounds in ydr/yft) | Material names are written as the decimal 16-bit hash (`materials.meta` is never loaded), and reading hashes that text again. | Collision material hashes change on **every** XML round trip. |
| yed | Jump offsets for buffer 2 are computed differently from the game's: usually 12 more, about 2% of the time 4 more. | Expressions with jumps written from XML may be wrong in game. |
| ypmd | Points are written separated by spaces but parsed split on commas. | Every point is `0 0 0` after an XML round trip. |
| yldb | String lengths are stored in UTF-16 units but written as UTF-8 bytes. | Non-ASCII text (e.g. "Sporgerò") is cut short after a write. |
| ycd | AnimDictImports are never written to XML (the wrong count is checked). | They're lost through XML. |
| yfd | An empty `<Entries />` is written as a null pointer. Two filters whose names hash the same make the write fail. | Empty entries disappear after a round trip. |
| ybn | Every triangle area is `float.Epsilon`: the first vertex is used three times, giving NaN. | Areas in written files are wrong. |
| rbf | Content without a name is written as `<>`. | That XML can't be read back by anyone. |

## Files the C# can't write (the Rust can)

| Format | Bug |
|---|---|
| pso | `EndOfStreamException` when the data section doesn't end on 16 bytes: most PSO files fail to write. |
| ydr | A shader with no constant buffers has no `BufferSizes` in the XML, and writing throws a `NullReferenceException`. 25 `out_*` map pieces in 1000 files fail. |
| ytd | An empty dictionary (no `<Textures>`) throws in `Build(null)`. The Rust also fails here. |
| pso (parsed-data `.ymt`) | The root class isn't in `schemas.txt`, so the C# writes an empty file. The Rust also fails here. |
| ydr/yft with `Rsc8DrawablePtfx` | Can't be written (not ported to Rust either). |

## Smaller oddities kept

- **ypmd:** after a trip through XML, the name hash and owner pointers are 0.
- **ycd:** IncrementalFloat channels are rewritten as RawFloat.
- **ycd:** QuantizeFloat ranges are recomputed from the values.
- **yed:** files are written as resource version 25, though the game's are 30.
- **ydr:** a `Dec3N` component stored as `-0` comes back as `0`.
- **ydr:** `Vector4ToShort4N`/`Vector2ToShort2N` take every component's sign from X.
- **Integers through RBF XML:** they come back as floats.
- **ynv:** positions outside the box wrap around when packed (a float to ushort goes through int).
