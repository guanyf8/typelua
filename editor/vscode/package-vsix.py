#!/usr/bin/env python3
"""Package the TypeLua VS Code extension into a portable .vsix.

A .vsix is an OPC (Open Packaging Conventions) ZIP: two metadata parts at the
archive root (`extension.vsixmanifest` and `[Content_Types].xml`) plus every
shipped file under an `extension/` prefix. We build it by hand because the
bundled @vscode/vsce install is incomplete and offline packaging via npx is not
possible here.

Run from the extension directory:  python3 package-vsix.py
Output:  typelua-<version>.vsix
"""

import json
import os
import stat
import sys
import zipfile
from xml.sax.saxutils import escape

HERE = os.path.dirname(os.path.abspath(__file__))

# Runtime assets that must ship inside the .vsix. Directories are walked
# recursively; everything else in the folder (node_modules, out, *.vsix, this
# script, .vscodeignore, .DS_Store) is intentionally left out.
INCLUDE_FILES = [
    "package.json",
    "extension.js",
    "language-configuration.json",
    "README.md",
]
INCLUDE_DIRS = [
    "syntaxes",
    "bin",
]

# OPC content types keyed by file extension (no leading dot). Files with no
# extension (the bundled binary) get an explicit <Override> below.
CONTENT_TYPES = {
    "json": "application/json",
    "js": "application/javascript",
    "md": "text/markdown",
    "vsixmanifest": "text/xml",
    "png": "image/png",
}


def collect_files():
    """Return [(absolute_path, arcname_relative_to_extension_dir)]."""
    out = []
    for rel in INCLUDE_FILES:
        full = os.path.join(HERE, rel)
        if os.path.isfile(full):
            out.append((full, rel))
    for d in INCLUDE_DIRS:
        base = os.path.join(HERE, d)
        if not os.path.isdir(base):
            continue
        for root, _dirs, files in os.walk(base):
            for name in files:
                if name == ".DS_Store" or name == ".gitkeep":
                    continue
                full = os.path.join(root, name)
                arc = os.path.relpath(full, HERE)
                out.append((full, arc))
    return out


def build_content_types(files):
    exts = {os.path.splitext(arc)[1].lstrip(".").lower() for _f, arc in files}
    exts.add("vsixmanifest")
    lines = [
        '<?xml version="1.0" encoding="utf-8"?>',
        '<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">',
    ]
    for ext in sorted(e for e in exts if e):
        ctype = CONTENT_TYPES.get(ext, "application/octet-stream")
        lines.append(f'  <Default Extension="{ext}" ContentType="{ctype}" />')
    # Extensionless parts (e.g. bin/typelua) need per-part overrides.
    for _full, arc in files:
        if not os.path.splitext(arc)[1]:
            part = "/extension/" + arc.replace(os.sep, "/")
            lines.append(
                f'  <Override PartName="{part}" ContentType="application/octet-stream" />'
            )
    lines.append("</Types>")
    return "\n".join(lines) + "\n"


def build_manifest(pkg):
    ident = {
        "id": pkg["name"],
        "version": pkg["version"],
        "publisher": pkg.get("publisher", "unknown"),
    }
    display = escape(pkg.get("displayName", pkg["name"]))
    desc = escape(pkg.get("description", ""))
    engine = escape(pkg.get("engines", {}).get("vscode", "*"))
    categories = escape(",".join(pkg.get("categories", [])))
    return f"""<?xml version="1.0" encoding="utf-8"?>
<PackageManifest Version="2.0.0" xmlns="http://schemas.microsoft.com/developer/vsx-schema/2011" xmlns:d="http://schemas.microsoft.com/developer/vsx-schema-design/2011">
  <Metadata>
    <Identity Language="en-US" Id="{ident['id']}" Version="{ident['version']}" Publisher="{ident['publisher']}" />
    <DisplayName>{display}</DisplayName>
    <Description xml:space="preserve">{desc}</Description>
    <Tags></Tags>
    <Categories>{categories}</Categories>
    <GalleryFlags>Public</GalleryFlags>
    <Properties>
      <Property Id="Microsoft.VisualStudio.Code.Engine" Value="{engine}" />
      <Property Id="Microsoft.VisualStudio.Code.ExtensionDependencies" Value="" />
      <Property Id="Microsoft.VisualStudio.Code.ExtensionPack" Value="" />
      <Property Id="Microsoft.VisualStudio.Code.ExtensionKind" Value="workspace" />
      <Property Id="Microsoft.VisualStudio.Code.LocalizedLanguages" Value="" />
    </Properties>
  </Metadata>
  <Installation>
    <InstallationTarget Id="Microsoft.VisualStudio.Code" />
  </Installation>
  <Dependencies />
  <Assets>
    <Asset Type="Microsoft.VisualStudio.Code.Manifest" Path="extension/package.json" Addressable="true" />
    <Asset Type="Microsoft.VisualStudio.Services.Content.Details" Path="extension/README.md" Addressable="true" />
  </Assets>
</PackageManifest>
"""


def add_str(zf, arcname, text):
    info = zipfile.ZipInfo(arcname)
    info.compress_type = zipfile.ZIP_DEFLATED
    info.external_attr = (0o644 & 0xFFFF) << 16
    zf.writestr(info, text)


def add_file(zf, full, arcname):
    is_exec = bool(os.stat(full).st_mode & stat.S_IXUSR)
    mode = 0o755 if is_exec else 0o644
    info = zipfile.ZipInfo(arcname)
    info.compress_type = zipfile.ZIP_DEFLATED
    # High 16 bits carry the unix mode; the regular-file flag keeps unzip happy
    # and, crucially, preserves the exec bit on the bundled binary.
    info.external_attr = (mode | stat.S_IFREG) << 16
    with open(full, "rb") as fh:
        zf.writestr(info, fh.read())


def main():
    with open(os.path.join(HERE, "package.json"), encoding="utf-8") as fh:
        pkg = json.load(fh)
    files = collect_files()
    out_name = f"{pkg['name']}-{pkg['version']}.vsix"
    out_path = os.path.join(HERE, out_name)
    if os.path.exists(out_path):
        os.remove(out_path)

    manifest = build_manifest(pkg)
    content_types = build_content_types(files)

    with zipfile.ZipFile(out_path, "w", zipfile.ZIP_DEFLATED) as zf:
        add_str(zf, "extension.vsixmanifest", manifest)
        add_str(zf, "[Content_Types].xml", content_types)
        for full, arc in sorted(files, key=lambda t: t[1]):
            add_file(zf, full, "extension/" + arc.replace(os.sep, "/"))

    size = os.path.getsize(out_path)
    print(f"wrote {out_name} ({size} bytes, {len(files) + 2} entries)")
    for _full, arc in sorted(files, key=lambda t: t[1]):
        print(f"  extension/{arc.replace(os.sep, '/')}")


if __name__ == "__main__":
    sys.exit(main())
