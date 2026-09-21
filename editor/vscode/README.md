# TypeLua for VSCode

Language support for **TypeLua** (`.tua`): TextMate + semantic highlighting and
go-to-definition, driven by the compiler's own `typelua plugin` command.

## What it does

- **Syntax highlighting** — a TextMate grammar colors keywords, strings, numbers,
  comments and operators; editor behavior (comment toggling, bracket matching,
  auto-closing) comes from the language configuration.
- **Semantic highlighting** — identifiers are colored by their real role
  (type, typeParameter, function, method, property, parameter, variable, label)
  as resolved by the analyzer, layered on top of TextMate.
- **Go to definition** — jumps to the definition of a name, including
  cross-module `import { A } in "mod"` targets.

## How it works

The extension is a thin client over the CLI. On **open** and **save** of a
`.tua` file it runs

```
typelua plugin <workspace-folder>
```

which prints a JSON array (one object per file) of byte-offset `tokens` and
`links`. The extension converts those UTF-8 byte offsets to editor positions and
feeds VSCode's semantic-tokens and definition providers. Whole-folder scope is
used so cross-module jumps resolve to the right file.

Because the CLI reads from disk, results reflect the **saved** file — highlights
and jumps update after you save.

## Configuration

| Setting | Default | Description |
| --- | --- | --- |
| `typelua.binaryPath` | `""` | Absolute path to the `typelua` binary. Empty = use the binary bundled in `bin/`. |
| `typelua.trace` | `false` | Log analyzer invocations/errors to the **TypeLua** output channel. |

Command: **TypeLua: Re-analyze workspace** (`typelua.reanalyze`).

## Project configuration (`typelua.json`)

You can control which directories the extension analyzes and compiles by
creating `.vscode/typelua.json` at the root of your workspace:

```json
{
  "search_path": ["./src", "./examples"]
}
```

- `search_path` is an array of paths **relative to the workspace folder**.
- If the file is missing, empty, or does not contain `search_path`, the
  extension falls back to analyzing the entire workspace folder.
- The paths are passed to `typelua plugin` and `typelua compile` as the search
  roots.

For example, to analyze only the `examples` directory:

```json
{
  "search_path": ["./examples"]
}
```

## The bundled binary

A release build of `typelua` ships under `bin/`. If it does not match your
platform, build one (`cargo build --release`) and point `typelua.binaryPath` at
`target/release/typelua`.
