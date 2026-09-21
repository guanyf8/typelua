"use strict";
// TypeLua VSCode extension.
//
// Thin client over the `typelua plugin` CLI. On open/save of a .tua file we run
// `typelua plugin <workspaceFolder>` (whole-folder scope so cross-module
// go-to-definition resolves), parse the JSON array it prints to stdout, and back
// editor features from the cache:
//   - DocumentSemanticTokensProvider  (tokens[]      -> colored identifiers)
//   - DefinitionProvider              (links[]       -> go to definition)
//   - DiagnosticCollection            (diagnostics[] -> editor squiggles)
//
// The CLI reports byte offsets (UTF-8). VSCode positions are UTF-16. All the
// fiddly work here is converting between the two against the live document text.

const vscode = require("vscode");
const cp = require("child_process");
const path = require("path");
const fs = require("fs");

// Order defines the numeric id used in the SemanticTokens stream; must match the
// legend below. Every name here is a standard LSP semantic token type, so themes
// color them without any extra contribution in package.json.
const TOKEN_TYPES = [
  "keyword",
  "string",
  "number",
  "comment",
  "type",
  "typeParameter",
  "function",
  "method",
  "property",
  "parameter",
  "variable",
  "label",
];
const LEGEND = new vscode.SemanticTokensLegend(TOKEN_TYPES, []);
const TYPE_INDEX = new Map(TOKEN_TYPES.map((name, i) => [name, i]));

// module state
/** @type {Map<string, {tokens: any[], links: any[], diagnostics: any[]}>} keyed by resolved fs path */
const cache = new Map();
/** @type {Map<string, Set<string>>} workspace folder -> files returned by its last analysis */
const analyzedFiles = new Map();
let output = null;
/** @type {vscode.DiagnosticCollection | null} */
let diagnosticCollection = null;
/** @type {vscode.EventEmitter<void>} */
let onDidChangeEmitter = null;
// Coalesce concurrent/rapid analyses of the same folder.
/** @type {Map<string, Promise<void>>} */
const inflight = new Map();

function trace(msg) {
  const on = vscode.workspace.getConfiguration("typelua").get("trace", false);
  if (on && output) {
    output.appendLine(`[${new Date().toISOString()}] ${msg}`);
  }
}

function normPath(p) {
  return path.resolve(p);
}

// Resolve the typelua binary: explicit setting wins, else the bundled bin/.
function resolveBinary(context) {
  const cfg = vscode.workspace.getConfiguration("typelua").get("binaryPath", "");
  if (cfg && cfg.trim()) {
    return cfg.trim();
  }
  const name = process.platform === "win32" ? "typelua.exe" : "typelua";
  const bundled = path.join(context.extensionPath, "bin", name);
  // A .vsix install may drop the exec bit on the bundled binary; restore it so
  // execFile works portably. Best effort — a configured binaryPath is left alone.
  if (process.platform !== "win32") {
    try {
      fs.chmodSync(bundled, 0o755);
    } catch (e) {
      trace(`chmod ${bundled} failed: ${e.message}`);
    }
  }
  return bundled;
}

// The plugin config lives at <folder>/.vscode/typelua.json. `search_path` lists
// the roots (relative to the folder) to analyze/compile; resolved to absolute so
// the CLI's returned `path` is absolute and matches document paths in the cache.
// Falls back to the folder itself when there is no config or it is empty.
function readSearchTargets(folderPath) {
  const cfgPath = path.join(folderPath, ".vscode", "typelua.json");
  try {
    const cfg = JSON.parse(fs.readFileSync(cfgPath, "utf8"));
    if (Array.isArray(cfg.search_path) && cfg.search_path.length) {
      return cfg.search_path
        .filter((p) => typeof p === "string" && p.length)
        .map((p) => path.resolve(folderPath, p));
    }
  } catch (e) {
    if (e.code !== "ENOENT") {
      trace(`config read error ${cfgPath}: ${e.message}`);
    }
  }
  return [folderPath];
}

// Run `typelua plugin <folder>` and fold the JSON result into the cache.
function analyzeFolder(binary, folderPath) {
  const key = normPath(folderPath);
  const running = inflight.get(key);
  if (running) {
    return running;
  }
  const p = new Promise((resolve) => {
    const targets = readSearchTargets(folderPath);
    trace(`analyze: ${binary} plugin ${targets.join(" ")}`);
    cp.execFile(
      binary,
      ["plugin", ...targets],
      { cwd: folderPath, maxBuffer: 128 * 1024 * 1024, windowsHide: true },
      (err, stdout, stderr) => {
        if (err) {
          const detail = stderr && stderr.trim() ? `\n${stderr.trim()}` : "";
          trace(`analyze failed: ${err.message}${detail}`);
          if (output) {
            output.appendLine(`TypeLua analyzer error: ${err.message}${detail}`);
          }
          resolve();
          return;
        }
        let parsed;
        try {
          parsed = JSON.parse(stdout);
        } catch (e) {
          trace(`JSON parse error: ${e.message}`);
          if (output) {
            output.appendLine(`TypeLua: could not parse analyzer output: ${e.message}`);
          }
          resolve();
          return;
        }
        let count = 0;
        const currentFiles = new Set();
        for (const fa of parsed) {
          if (!fa || typeof fa.path !== "string") {
            continue;
          }
          const filePath = normPath(fa.path);
          const diagnostics = Array.isArray(fa.diagnostics) ? fa.diagnostics : [];
          cache.set(filePath, {
            tokens: Array.isArray(fa.tokens) ? fa.tokens : [],
            links: Array.isArray(fa.links) ? fa.links : [],
            diagnostics,
          });
          currentFiles.add(filePath);
          publishDiagnostics(filePath, diagnostics);
          count++;
        }
        const previousFiles = analyzedFiles.get(key);
        if (previousFiles) {
          for (const filePath of previousFiles) {
            if (!currentFiles.has(filePath)) {
              cache.delete(filePath);
              if (diagnosticCollection) {
                diagnosticCollection.delete(vscode.Uri.file(filePath));
              }
            }
          }
        }
        analyzedFiles.set(key, currentFiles);
        trace(`analyze ok: ${count} file(s) cached`);
        resolve();
      }
    );
  }).finally(() => {
    inflight.delete(key);
  });
  inflight.set(key, p);
  return p;
}

// Kick off analysis for the folder that owns `uri` (or every workspace folder if
// the file is loose), then notify the editor to re-pull semantic tokens.
async function refreshFor(context, uri) {
  const binary = resolveBinary(context);
  const targets = [];
  const folder = uri ? vscode.workspace.getWorkspaceFolder(uri) : undefined;
  if (folder) {
    targets.push(folder.uri.fsPath);
  } else if (vscode.workspace.workspaceFolders) {
    for (const f of vscode.workspace.workspaceFolders) {
      targets.push(f.uri.fsPath);
    }
  } else if (uri) {
    // No workspace: analyze the single file's directory.
    targets.push(path.dirname(uri.fsPath));
  }
  await Promise.all(targets.map((t) => analyzeFolder(binary, t)));
  if (onDidChangeEmitter) {
    onDidChangeEmitter.fire();
  }
}

// Build: `typelua compile <search_path...> --out <folder>/dist` emits Lua 5.3
// into dist/ under the folder. The CLI always exits 0 (it logs and returns on
// failure), so success is judged by the completion line on stderr, not the code.
function buildFolder(context, folderPath) {
  const binary = resolveBinary(context);
  const targets = readSearchTargets(folderPath);
  const dist = path.join(folderPath, "dist");
  output.show(true);
  output.appendLine(`TypeLua build: ${binary} compile ${targets.join(" ")} --out ${dist}`);
  cp.execFile(
    binary,
    ["compile", ...targets, "--out", dist],
    { cwd: folderPath, maxBuffer: 128 * 1024 * 1024, windowsHide: true },
    (err, stdout, stderr) => {
      const combined = `${stdout || ""}${stderr || ""}`.trim();
      if (combined) {
        output.appendLine(combined);
      }
      if (!err && /compile \u5b8c\u6210/.test(combined)) {
        vscode.window.showInformationMessage(`TypeLua: \u5df2\u7f16\u8bd1\u5230 ${dist}`);
      } else {
        vscode.window.showErrorMessage("TypeLua \u7f16\u8bd1\u5931\u8d25\uff0c\u8be6\u89c1 TypeLua \u8f93\u51fa\u9762\u677f\u3002");
      }
    }
  );
}

// Build both directions of the UTF-8-byte <-> UTF-16-index mapping for `text`.
//   byteToChar[b]  -> UTF-16 index of the char whose UTF-8 encoding starts at b
//   charToByte[i]  -> byte offset where the UTF-16 unit i begins
function buildOffsetMaps(text) {
  const charToByte = new Array(text.length + 1);
  const byteToChar = [];
  let byte = 0;
  let i = 0;
  while (i < text.length) {
    const code = text.codePointAt(i);
    const u16 = code > 0xffff ? 2 : 1;
    let blen;
    if (code < 0x80) blen = 1;
    else if (code < 0x800) blen = 2;
    else if (code < 0x10000) blen = 3;
    else blen = 4;
    charToByte[i] = byte;
    if (u16 === 2) {
      charToByte[i + 1] = byte; // low surrogate maps to the same byte start
    }
    for (let b = 0; b < blen; b++) {
      byteToChar[byte + b] = i;
    }
    byte += blen;
    i += u16;
  }
  charToByte[text.length] = byte;
  byteToChar[byte] = text.length;
  return { charToByte, byteToChar, byteLength: byte, charLength: text.length };
}

function byteToCharIndex(maps, byteOffset) {
  if (byteOffset <= 0) return 0;
  if (byteOffset >= maps.byteLength) return maps.charLength;
  const c = maps.byteToChar[byteOffset];
  return c === undefined ? maps.charLength : c;
}

function charToByteOffset(maps, charIndex) {
  if (charIndex <= 0) return 0;
  if (charIndex >= maps.charLength) return maps.byteLength;
  const b = maps.charToByte[charIndex];
  return b === undefined ? maps.byteLength : b;
}

function buildLineStarts(text) {
  const starts = [0];
  for (let i = 0; i < text.length; i++) {
    if (text.charCodeAt(i) === 13) {
      if (text.charCodeAt(i + 1) === 10) i++;
      starts.push(i + 1);
    } else if (text.charCodeAt(i) === 10) {
      starts.push(i + 1);
    }
  }
  return starts;
}

function positionAtChar(lineStarts, charIndex) {
  let low = 0;
  let high = lineStarts.length;
  while (low < high) {
    const mid = Math.floor((low + high) / 2);
    if (lineStarts[mid] > charIndex) high = mid;
    else low = mid + 1;
  }
  const line = Math.max(0, low - 1);
  return new vscode.Position(line, charIndex - lineStarts[line]);
}

function diagnosticSeverity(severity) {
  switch (severity) {
    case "warning":
      return vscode.DiagnosticSeverity.Warning;
    case "info":
      return vscode.DiagnosticSeverity.Information;
    case "error":
    default:
      return vscode.DiagnosticSeverity.Error;
  }
}

function publishDiagnostics(filePath, entries) {
  if (!diagnosticCollection) return;
  const uri = vscode.Uri.file(filePath);
  if (entries.length === 0) {
    diagnosticCollection.delete(uri);
    return;
  }

  let text;
  try {
    text = fs.readFileSync(filePath, "utf8");
  } catch (e) {
    trace(`diagnostic source read failed: ${filePath}: ${e.message}`);
    diagnosticCollection.delete(uri);
    return;
  }

  const maps = buildOffsetMaps(text);
  const lineStarts = buildLineStarts(text);
  const diagnostics = [];
  for (const entry of entries) {
    if (
      !entry ||
      typeof entry.start !== "number" ||
      typeof entry.end !== "number" ||
      typeof entry.message !== "string"
    ) {
      continue;
    }
    const start = byteToCharIndex(maps, entry.start);
    const end = byteToCharIndex(maps, Math.max(entry.start, entry.end));
    const range = new vscode.Range(
      positionAtChar(lineStarts, start),
      positionAtChar(lineStarts, end)
    );
    const diagnostic = new vscode.Diagnostic(
      range,
      entry.message,
      diagnosticSeverity(entry.severity)
    );
    diagnostic.source = typeof entry.source === "string" ? entry.source : "TypeLua";
    diagnostics.push(diagnostic);
  }
  diagnosticCollection.set(uri, diagnostics);
}

// Push a token span, splitting across line boundaries because a SemanticTokens
// entry may not straddle lines (e.g. block comments).
function collectToken(document, maps, startByte, endByte, typeIndex, out) {
  if (endByte <= startByte) return;
  const startPos = document.positionAt(byteToCharIndex(maps, startByte));
  const endPos = document.positionAt(byteToCharIndex(maps, endByte));
  if (startPos.line === endPos.line) {
    out.push([startPos.line, startPos.character, endPos.character - startPos.character, typeIndex]);
    return;
  }
  const firstLen = document.lineAt(startPos.line).text.length;
  out.push([startPos.line, startPos.character, firstLen - startPos.character, typeIndex]);
  for (let ln = startPos.line + 1; ln < endPos.line; ln++) {
    const len = document.lineAt(ln).text.length;
    if (len > 0) {
      out.push([ln, 0, len, typeIndex]);
    }
  }
  if (endPos.character > 0) {
    out.push([endPos.line, 0, endPos.character, typeIndex]);
  }
}

const semanticTokensProvider = {
  onDidChangeSemanticTokens: undefined, // set in activate
  provideDocumentSemanticTokens(document) {
    const entry = cache.get(normPath(document.uri.fsPath));
    if (!entry) {
      return new vscode.SemanticTokens(new Uint32Array(0));
    }
    const maps = buildOffsetMaps(document.getText());
    const rows = [];
    for (const t of entry.tokens) {
      const idx = TYPE_INDEX.get(t.kind);
      if (idx === undefined) continue;
      collectToken(document, maps, t.start, t.end, idx, rows);
    }
    // SemanticTokensBuilder needs tokens in (line, char) order.
    rows.sort((a, b) => (a[0] - b[0]) || (a[1] - b[1]));
    const builder = new vscode.SemanticTokensBuilder(LEGEND);
    for (const [line, char, len, idx] of rows) {
      if (len > 0) {
        builder.push(line, char, len, idx, 0);
      }
    }
    return builder.build();
  },
};

const definitionProvider = {
  async provideDefinition(document, position) {
    const entry = cache.get(normPath(document.uri.fsPath));
    if (!entry || entry.links.length === 0) {
      return undefined;
    }
    const maps = buildOffsetMaps(document.getText());
    const off = charToByteOffset(maps, document.offsetAt(position));
    const link = entry.links.find((l) => off >= l.start && off < l.end);
    if (!link) {
      return undefined;
    }
    const targetPath = link.targetPath && link.targetPath.length ? link.targetPath : document.uri.fsPath;
    let targetDoc;
    try {
      targetDoc = await vscode.workspace.openTextDocument(vscode.Uri.file(targetPath));
    } catch (e) {
      trace(`open target failed: ${targetPath}: ${e.message}`);
      return undefined;
    }
    const tMaps = buildOffsetMaps(targetDoc.getText());
    const range = new vscode.Range(
      targetDoc.positionAt(byteToCharIndex(tMaps, link.targetStart)),
      targetDoc.positionAt(byteToCharIndex(tMaps, link.targetEnd))
    );
    return new vscode.Location(vscode.Uri.file(targetPath), range);
  },
};

function activate(context) {
  output = vscode.window.createOutputChannel("TypeLua");
  diagnosticCollection = vscode.languages.createDiagnosticCollection("typelua");
  onDidChangeEmitter = new vscode.EventEmitter();
  semanticTokensProvider.onDidChangeSemanticTokens = onDidChangeEmitter.event;
  context.subscriptions.push(output, diagnosticCollection, onDidChangeEmitter);

  const selector = { language: "typelua", scheme: "file" };
  context.subscriptions.push(
    vscode.languages.registerDocumentSemanticTokensProvider(selector, semanticTokensProvider, LEGEND),
    vscode.languages.registerDefinitionProvider(selector, definitionProvider)
  );

  // Refresh triggers: open + save (byte offsets always match on-disk bytes).
  context.subscriptions.push(
    vscode.workspace.onDidOpenTextDocument((doc) => {
      if (doc.languageId === "typelua") refreshFor(context, doc.uri);
    }),
    vscode.workspace.onDidSaveTextDocument((doc) => {
      if (doc.languageId === "typelua") refreshFor(context, doc.uri);
    }),
    vscode.commands.registerCommand("typelua.reanalyze", () => {
      const ed = vscode.window.activeTextEditor;
      refreshFor(context, ed ? ed.document.uri : undefined);
    }),
    vscode.commands.registerCommand("typelua.build", () => {
      const ed = vscode.window.activeTextEditor;
      const folder = ed ? vscode.workspace.getWorkspaceFolder(ed.document.uri) : undefined;
      const folders = vscode.workspace.workspaceFolders;
      const target = folder
        ? folder.uri.fsPath
        : folders && folders.length
          ? folders[0].uri.fsPath
          : undefined;
      if (!target) {
        vscode.window.showErrorMessage("TypeLua: \u6ca1\u6709\u6253\u5f00\u7684\u5de5\u4f5c\u533a\u6587\u4ef6\u5939");
        return;
      }
      buildFolder(context, target);
    })
  );

  // Analyze whatever is already open at activation.
  const active = vscode.window.activeTextEditor;
  if (active && active.document.languageId === "typelua") {
    refreshFor(context, active.document.uri);
  } else {
    refreshFor(context, undefined);
  }
}

function deactivate() {}

module.exports = { activate, deactivate };
